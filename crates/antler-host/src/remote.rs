//! The client side of the protocol: a [`Host`] that forwards each call to `antler-server` and waits for the answer.
//! Calls are blocking and may run concurrently from several threads (each waits for its own id).
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use crate::discovery::Repo;
use crate::host::{Envs, Host, Meta, WatchGuard};
use crate::protocol::*;
use crate::search::{Query, Results};

type Answer = (Res, Vec<u8>);

/// Who is waiting for what; shared with the reader thread, which is the only one reading the server's output.
#[derive(Default)]
struct Shared {
    /// `closed` once the connection is gone: every waiter is released with an error, and new calls fail at once.
    pending: Mutex<(bool, HashMap<u64, Sender<Answer>>)>,
    watches: Mutex<HashMap<u64, Box<dyn Fn(PathBuf) + Send>>>,
}

struct Inner {
    writer: Mutex<Box<dyn Write + Send>>,
    shared: Arc<Shared>,
    next: AtomicU64,
}

/// What the server said about itself in its `Hello`.
#[derive(Debug, Clone)]
pub struct ServerInfo {
    pub os: String,
    pub arch: String,
}

pub struct Remote {
    inner: Arc<Inner>,
    pub server: ServerInfo,
    /// The server process when this `Remote` started it; killed on drop.
    child: Mutex<Option<Child>>,
}

fn lost() -> io::Error {
    io::Error::new(io::ErrorKind::ConnectionAborted, "the connection to the host was lost")
}

impl Remote {
    /// Talks to a server on `input`/`output` (its stdout / its stdin). Reads the `Hello` first.
    pub fn connect(mut input: impl Read + Send + 'static, output: impl Write + Send + 'static) -> Result<Remote, String> {
        let hello = read_frame::<_, ResFrame>(&mut input).map_err(|e| format!("no answer from the server: {e}"))?;
        let server = match hello {
            Some((ResFrame { res: Res::Hello { protocol, os, arch }, .. }, _)) if protocol == VERSION => ServerInfo { os, arch },
            Some((ResFrame { res: Res::Hello { protocol, .. }, .. }, _)) => return Err(format!("the server speaks protocol {protocol}, this Antler {VERSION}")),
            _ => return Err("the server did not say hello".into()),
        };
        let shared = Arc::new(Shared::default());
        let reader = shared.clone();
        std::thread::spawn(move || {
            while let Ok(Some((ResFrame { id, res }, body))) = read_frame(&mut input) {
                if let Res::Event { path } = res {
                    reader.watches.lock().unwrap_or_else(|e| e.into_inner()).get(&id).map(|f| f(native_wire(&path)));
                } else if let Some(tx) = reader.pending.lock().unwrap_or_else(|e| e.into_inner()).1.remove(&id) {
                    tx.send((res, body)).ok();
                }
            }
            let mut pending = reader.pending.lock().unwrap_or_else(|e| e.into_inner());
            *pending = (true, HashMap::new()); // dropping the senders wakes every waiter
        });
        let inner = Inner { writer: Mutex::new(Box::new(output)), shared, next: AtomicU64::new(1) };
        Ok(Remote { inner: Arc::new(inner), server, child: Mutex::new(None) })
    }

    /// Starts `command` (a local `antler-server`; stage 3: `ssh host antler-server`) and talks to it.
    pub fn spawn(mut command: Command) -> Result<Remote, String> {
        let mut child = command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn().map_err(|e| format!("cannot start the server: {e}"))?;
        let (stdin, stdout) = (child.stdin.take().unwrap(), child.stdout.take().unwrap());
        let remote = Remote::connect(stdout, stdin).inspect_err(|_| drop(child.kill()))?;
        *remote.child.lock().unwrap_or_else(|e| e.into_inner()) = Some(child);
        Ok(remote)
    }
}

impl Drop for Remote {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.lock().unwrap_or_else(|e| e.into_inner()).take() {
            // closing its stdin makes the server finish; kill is for one that does not
            drop(child.kill());
            drop(child.wait());
        }
    }
}

/// A wire path as the app sees it: always `/`-separated, whatever the app's OS.
fn native_wire(s: &str) -> PathBuf {
    PathBuf::from(s)
}

impl Inner {
    fn send(&self, id: u64, req: Req, body: &[u8]) -> io::Result<()> {
        write_frame(&mut *self.writer.lock().unwrap_or_else(|e| e.into_inner()), &ReqFrame { id, req }, body)
    }

    /// Sends a request and waits for its answer; `cancel`, once set, sends a `Cancel` (the answer still comes).
    fn call(&self, req: Req, body: &[u8], cancel: Option<&AtomicBool>) -> io::Result<Answer> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let rx = self.register(id)?;
        self.send(id, req, body).inspect_err(|_| drop(self.shared.pending.lock().unwrap_or_else(|e| e.into_inner()).1.remove(&id)))?;
        let mut cancelled = false;
        loop {
            if !cancelled && cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
                cancelled = true;
                self.send(self.next.fetch_add(1, Ordering::Relaxed), Req::Cancel { target: id }, &[]).ok();
            }
            match rx.recv_timeout(Duration::from_millis(25)) {
                Ok((Res::Err { kind, msg }, _)) => return Err(io::Error::new(kind_from_name(&kind), msg)),
                Ok(answer) => return Ok(answer),
                Err(RecvTimeoutError::Disconnected) => return Err(lost()),
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
    }

    fn register(&self, id: u64) -> io::Result<Receiver<Answer>> {
        let (tx, rx) = channel();
        let mut pending = self.shared.pending.lock().unwrap_or_else(|e| e.into_inner());
        if pending.0 {
            return Err(lost());
        }
        pending.1.insert(id, tx);
        Ok(rx)
    }

    fn ok(&self, req: Req) -> io::Result<()> {
        self.call(req, &[], None).map(drop)
    }
}

/// The text of an error that is not an `io::Error` in the contract.
fn text(e: io::Error) -> String {
    e.to_string()
}

fn strings(paths: &[PathBuf]) -> Vec<String> {
    paths.iter().map(|p| wire_path(p)).collect()
}

/// The `files.exclude` patterns the app has now, which the server applies to this request.
fn exclude() -> Vec<String> {
    crate::exclude::current().to_vec()
}

fn unexpected<T>(what: &str) -> io::Result<T> {
    Err(io::Error::new(io::ErrorKind::InvalidData, format!("unexpected answer to {what}")))
}

impl Host for Remote {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        match self.inner.call(Req::Read { path: wire_path(path) }, &[], None)? {
            (Res::Bytes, body) => Ok(body),
            _ => unexpected("read"),
        }
    }

    fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        self.inner.call(Req::Write { path: wire_path(path) }, bytes, None).map(drop)
    }

    fn create(&self, path: &Path, folder: bool) -> io::Result<PathBuf> {
        match self.inner.call(Req::Create { path: wire_path(path), folder }, &[], None)? {
            (Res::Path { path }, _) => Ok(PathBuf::from(path)),
            _ => unexpected("create"),
        }
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.inner.ok(Req::Rename { from: wire_path(from), to: wire_path(to) })
    }

    fn copy(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.inner.ok(Req::Copy { from: wire_path(from), to: wire_path(to) })
    }

    /// There is no Trash on the host: deleted for good, so there is nothing to undo.
    fn trash(&self, path: &Path) -> Result<Option<PathBuf>, String> {
        self.inner.ok(Req::Trash { path: wire_path(path) }).map(|()| None).map_err(text)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        self.inner.ok(Req::RemoveFile { path: wire_path(path) })
    }

    fn metadata(&self, path: &Path) -> io::Result<Meta> {
        match self.inner.call(Req::Metadata { path: wire_path(path) }, &[], None)? {
            (Res::Meta(m), _) => Ok(Meta { is_dir: m.is_dir, is_file: m.is_file, size: m.size, mtime: m.mtime.map(|(s, n)| UNIX_EPOCH + Duration::new(s, n)) }),
            _ => unexpected("metadata"),
        }
    }

    fn exists(&self, path: &Path) -> bool {
        matches!(self.inner.call(Req::Exists { path: wire_path(path) }, &[], None), Ok((Res::Bool { value: true }, _)))
    }

    fn list_dir(&self, dir: &Path) -> Vec<(PathBuf, bool)> {
        match self.inner.call(Req::ListDir { dir: wire_path(dir), exclude: exclude() }, &[], None) {
            Ok((Res::Entries { entries }, _)) => entries.into_iter().map(|(p, d)| (PathBuf::from(p), d)).collect(),
            _ => Vec::new(),
        }
    }

    fn discover(&self, root: &Path, max_depth: usize) -> Vec<Repo> {
        match self.inner.call(Req::Discover { root: wire_path(root), max_depth }, &[], None) {
            Ok((Res::Repos { repos }, _)) => repos.into_iter().map(|r| Repo { path: PathBuf::from(r.path), worktree_of: r.worktree_of.map(PathBuf::from) }).collect(),
            _ => Vec::new(),
        }
    }

    fn files(&self, root: &Path, repos: &[PathBuf]) -> Vec<PathBuf> {
        match self.inner.call(Req::Files { root: wire_path(root), repos: strings(repos), exclude: exclude() }, &[], None) {
            Ok((Res::Files { files }, _)) => files.into_iter().map(PathBuf::from).collect(),
            _ => Vec::new(),
        }
    }

    fn search(&self, root: &Path, repos: &[PathBuf], query: &Query, limit: usize, cancel: &AtomicBool) -> Results {
        if cancel.load(Ordering::Relaxed) {
            return Results::default(); // like the walk that stops before its first file
        }
        let req = Req::Search { root: wire_path(root), repos: strings(repos), query: query.clone(), limit, exclude: exclude() };
        match self.inner.call(req, &[], Some(cancel)) {
            Ok((Res::Results(r), _)) => r.into(),
            Ok(_) => Results { error: Some("unexpected answer to search".into()), ..Default::default() },
            Err(e) => Results { error: Some(e.to_string()), ..Default::default() },
        }
    }

    fn watch(&self, root: &Path, on_change: Box<dyn Fn(PathBuf) + Send>) -> Result<WatchGuard, String> {
        let id = self.inner.next.fetch_add(1, Ordering::Relaxed);
        // before the request: events may follow its answer immediately
        self.inner.shared.watches.lock().unwrap_or_else(|e| e.into_inner()).insert(id, on_change);
        let guard = WatchStop { inner: self.inner.clone(), id };
        let rx = self.inner.register(id).map_err(text)?;
        self.inner.send(id, Req::Watch { root: wire_path(root) }, &[]).map_err(text)?;
        match rx.recv() {
            Ok((Res::Ok, _)) => Ok(Box::new(guard)),
            Ok((Res::Err { msg, .. }, _)) => Err(msg),
            _ => Err(lost().to_string()),
        }
    }

    fn git(&self, repo: &Path, args: &[&str], stdin: Option<&str>, envs: Envs) -> Result<String, String> {
        let req = Req::Git {
            repo: wire_path(repo),
            args: args.iter().map(|a| a.to_string()).collect(),
            stdin: stdin.map(str::to_string),
            envs: envs.iter().map(|(k, v)| (k.to_string(), v.to_string_lossy().into_owned())).collect(),
        };
        match self.inner.call(req, &[], None) {
            Ok((Res::Text { text }, _)) => Ok(text),
            Ok(_) => Err("unexpected answer to git".into()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// Dropping it stops the watch on the server and the callback here.
struct WatchStop {
    inner: Arc<Inner>,
    id: u64,
}

impl Drop for WatchStop {
    fn drop(&mut self) {
        self.inner.shared.watches.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.id);
        let cancel = self.inner.next.fetch_add(1, Ordering::Relaxed);
        self.inner.send(cancel, Req::Cancel { target: self.id }, &[]).ok();
    }
}
