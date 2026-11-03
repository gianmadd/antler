//! The client side of the protocol: a [`Host`] that forwards each call to `antler-server` and waits for the answer.
//! Calls are blocking and may run concurrently from several threads (each waits for its own id).
//!
//! A `Remote` made by [`Remote::open`] / [`Remote::lazy`] knows how to make its connection again (a [`Connector`]):
//! once [`supervised`](Remote::supervise), a lost connection is retried on VS Code's schedule (`reconnect`), the
//! active watches are sent again on the new one, and [`Status`] changes are reported. Calls made meanwhile fail
//! with `ConnectionAborted`.
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant, UNIX_EPOCH};

use crate::discovery::Repo;
use crate::host::{Envs, Host, Meta, WatchGuard};
use crate::ports::{ListeningPort, bind_local};
use crate::protocol::*;
use crate::search::{FileMatches, Query, Results};

type Answer = (Res, Vec<u8>);

/// What the server's stdout and stdin are, and the process behind them (killed when replaced or dropped).
pub type Transport = (Box<dyn Read + Send>, Box<dyn Write + Send>, Option<Child>);
/// Makes a new connection; it applies its own timeout (VS Code: 30 s per attempt).
pub type Connector = dyn Fn() -> Result<Transport, String> + Send + Sync;

/// What a supervised `Remote` reports.
#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Connected,
    /// Attempt number (from 1) to get the connection back.
    Reconnecting { attempt: u32 },
    /// The grace time is over; the text is the last error.
    GaveUp(String),
}

type Watches = Arc<Mutex<HashMap<u64, (PathBuf, Box<dyn Fn(PathBuf) + Send>)>>>;

type Streams = Arc<Mutex<HashMap<u64, TcpStream>>>;
type PortsWatch = Arc<Mutex<Option<(u64, Arc<dyn Fn(Vec<ListeningPort>) + Send + Sync>)>>>;

/// Where the reader thread delivers what is not an answer; they outlive a connection.
#[derive(Clone, Default)]
struct Sinks {
    watches: Watches,
    /// The local sockets of the open forwarded streams, by stream id.
    streams: Streams,
    ports: PortsWatch,
}

/// Who is waiting for what; shared with the reader thread, which is the only one reading the server's output.
#[derive(Default)]
struct Shared {
    /// `closed` once the connection is gone: every waiter is released with an error, and new calls fail at once.
    pending: Mutex<(bool, HashMap<u64, Sender<Answer>>)>,
}

/// One connection.
struct Inner {
    writer: Mutex<Box<dyn Write + Send>>,
    shared: Arc<Shared>,
    next: Arc<AtomicU64>,
}

/// What lives across connections: the ids, the watches, the way to reconnect.
struct Core {
    conn: Mutex<Arc<Inner>>,
    next: Arc<AtomicU64>,
    sinks: Sinks,
    connector: Option<Arc<Connector>>,
    status: Mutex<Option<Arc<dyn Fn(Status) + Send + Sync>>>,
    /// The process behind the current connection.
    child: Mutex<Option<Child>>,
    closed: AtomicBool,
    supervised: AtomicBool,
    reconnecting: AtomicBool,
    /// Counts connections, so the end of an old one is not taken for the end of the current one.
    generation: AtomicU64,
}

/// What the server said about itself in its `Hello`.
#[derive(Debug, Clone, Default)]
pub struct ServerInfo {
    pub os: String,
    pub arch: String,
}

pub struct Remote {
    core: Arc<Core>,
    /// The first connection's server (empty for a `lazy` one that has not connected).
    pub server: ServerInfo,
}

fn lost() -> io::Error {
    io::Error::new(io::ErrorKind::ConnectionAborted, "the connection to the host was lost")
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Inner {
    /// Reads the `Hello` and starts the reader thread; `on_end` runs when the connection ends.
    fn start(mut input: Box<dyn Read + Send>, output: Box<dyn Write + Send>, next: Arc<AtomicU64>, sinks: Sinks, on_end: impl FnOnce() + Send + 'static) -> Result<(Arc<Inner>, ServerInfo), String> {
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
                match res {
                    Res::Event { path } => drop(lock(&sinks.watches).get(&id).map(|(_, f)| f(native_wire(&path)))),
                    Res::Ports { ports } => {
                        let cb = lock(&sinks.ports).as_ref().filter(|(i, _)| *i == id).map(|(_, f)| f.clone());
                        cb.map(|f| f(ports));
                    }
                    Res::Data => {
                        let s = lock(&sinks.streams).get(&id).and_then(|s| s.try_clone().ok());
                        // shortcut: a browser that does not read stalls every stream; upgrade with a writer thread per stream
                        if s.is_some_and(|mut s| s.write_all(&body).is_err()) {
                            lock(&sinks.streams).remove(&id).map(|s| s.shutdown(Shutdown::Both));
                        }
                    }
                    Res::Closed => drop(lock(&sinks.streams).remove(&id).map(|s| s.shutdown(Shutdown::Write))),
                    // part of an answer: the waiter stays registered until the closing frame
                    res @ (Res::FilesChunk { .. } | Res::SearchChunk { .. }) => drop(lock(&reader.pending).1.get(&id).map(|tx| tx.send((res, body)))),
                    res => {
                        if let Some(tx) = lock(&reader.pending).1.remove(&id) {
                            tx.send((res, body)).ok();
                        }
                    }
                }
            }
            lock(&sinks.streams).drain().for_each(|(_, s)| drop(s.shutdown(Shutdown::Both))); // their frames are gone with the connection
            *lock(&reader.pending) = (true, HashMap::new()); // dropping the senders wakes every waiter
            on_end();
        });
        Ok((Arc::new(Inner { writer: Mutex::new(output), shared, next }), server))
    }

    /// A connection that is already lost: what a `lazy` Remote has before it connects.
    fn dead(next: Arc<AtomicU64>) -> Arc<Inner> {
        let shared = Arc::new(Shared::default());
        lock(&shared.pending).0 = true;
        Arc::new(Inner { writer: Mutex::new(Box::new(io::sink())), shared, next })
    }

    fn is_lost(&self) -> bool {
        lock(&self.shared.pending).0
    }
}

impl Core {
    fn new(connector: Option<Arc<Connector>>) -> Arc<Core> {
        let next = Arc::new(AtomicU64::new(1));
        Arc::new(Core {
            conn: Mutex::new(Inner::dead(next.clone())),
            next,
            sinks: Sinks::default(),
            connector,
            status: Mutex::new(None),
            child: Mutex::new(None),
            closed: AtomicBool::new(false),
            supervised: AtomicBool::new(false),
            reconnecting: AtomicBool::new(false),
            generation: AtomicU64::new(0),
        })
    }

    fn notify(&self, s: Status) {
        let cb = lock(&self.status).clone();
        if let Some(cb) = cb {
            cb(s);
        }
    }

    /// Makes `transport` the current connection and sends the active watches again on it.
    fn install(core: &Arc<Core>, (input, output, mut child): Transport) -> Result<ServerInfo, String> {
        let generation = core.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let weak: Weak<Core> = Arc::downgrade(core);
        let on_end = move || {
            if let Some(core) = weak.upgrade() {
                Core::lost(&core, generation);
            }
        };
        let result = Inner::start(input, output, core.next.clone(), core.sinks.clone(), on_end).and_then(|(inner, info)| {
            let watches: Vec<(u64, PathBuf)> = lock(&core.sinks.watches).iter().map(|(id, (root, _))| (*id, root.clone())).collect();
            for (id, root) in watches {
                let rx = inner.register(id).map_err(text)?;
                inner.send(id, Req::Watch { root: wire_path(&root) }, &[]).map_err(text)?;
                match rx.recv() {
                    Ok((Res::Ok, _)) => {}
                    Ok(_) => drop(lock(&core.sinks.watches).remove(&id)), // its folder is gone: the watch ends
                    Err(_) => return Err(lost().to_string()),
                }
            }
            let ports = lock(&core.sinks.ports).as_ref().map(|(id, _)| *id);
            if let Some(id) = ports {
                let rx = inner.register(id).map_err(text)?;
                inner.send(id, Req::WatchPorts, &[]).map_err(text)?;
                rx.recv().map_err(|_| lost().to_string())?;
            }
            Ok((inner, info))
        });
        match result {
            Ok((inner, info)) => {
                *lock(&core.conn) = inner;
                if let Some(mut old) = std::mem::replace(&mut *lock(&core.child), child.take()) {
                    drop(old.kill());
                    drop(old.wait());
                }
                Ok(info)
            }
            Err(e) => {
                if let Some(c) = child.as_mut() {
                    drop(c.kill());
                    drop(c.wait());
                }
                Err(e)
            }
        }
    }

    /// The reader of connection `generation` ended.
    fn lost(core: &Arc<Core>, generation: u64) {
        if core.supervised.load(Ordering::SeqCst) && core.generation.load(Ordering::SeqCst) == generation {
            Core::start_reconnecting(core);
        }
    }

    fn start_reconnecting(core: &Arc<Core>) {
        if core.closed.load(Ordering::SeqCst) || core.connector.is_none() || core.reconnecting.swap(true, Ordering::SeqCst) {
            return;
        }
        let core = core.clone();
        std::thread::spawn(move || {
            let (lost_at, connector) = (Instant::now(), core.connector.clone().unwrap());
            let mut n = 0;
            loop {
                core.notify(Status::Reconnecting { attempt: n + 1 });
                let until = Instant::now() + crate::reconnect::wait(n);
                while Instant::now() < until && !core.closed.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(100).min(until.saturating_duration_since(Instant::now())));
                }
                if core.closed.load(Ordering::SeqCst) {
                    return;
                }
                match connector().and_then(|t| Core::install(&core, t)) {
                    Ok(_) if !lock(&core.conn).is_lost() => {
                        core.reconnecting.store(false, Ordering::SeqCst);
                        core.notify(Status::Connected);
                        return;
                    }
                    Ok(_) => {}
                    Err(e) if crate::reconnect::expired(lost_at.elapsed()) => {
                        core.reconnecting.store(false, Ordering::SeqCst);
                        core.notify(Status::GaveUp(e));
                        return;
                    }
                    Err(_) => {}
                }
                n += 1;
            }
        });
    }
}

impl Remote {
    /// Talks to a server on `input`/`output` (its stdout / its stdin). Reads the `Hello` first. Never reconnects.
    pub fn connect(input: impl Read + Send + 'static, output: impl Write + Send + 'static) -> Result<Remote, String> {
        let core = Core::new(None);
        let server = Core::install(&core, (Box::new(input), Box::new(output), None))?;
        Ok(Remote { core, server })
    }

    /// Starts `command` (a local `antler-server`) and talks to it.
    pub fn spawn(mut command: Command) -> Result<Remote, String> {
        let mut child = command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn().map_err(|e| format!("cannot start the server: {e}"))?;
        let (stdin, stdout) = (child.stdin.take().unwrap(), child.stdout.take().unwrap());
        let core = Core::new(None);
        let server = Core::install(&core, (Box::new(stdout), Box::new(stdin), Some(child)))?;
        Ok(Remote { core, server })
    }

    /// Connects once with `connector` (an error is returned, not retried); [`supervise`](Self::supervise) it to
    /// have later losses retried.
    pub fn open(connector: Arc<Connector>) -> Result<Remote, String> {
        let core = Core::new(Some(connector.clone()));
        let server = Core::install(&core, connector()?)?;
        Ok(Remote { core, server })
    }

    /// Not connected yet: [`supervise`](Self::supervise) makes the first connection with the retry schedule.
    pub fn lazy(connector: Arc<Connector>) -> Remote {
        Remote { core: Core::new(Some(connector)), server: ServerInfo::default() }
    }

    /// From now on a lost (or not yet made) connection is retried until the grace time ends, and `on_status` hears of it.
    pub fn supervise(&self, on_status: impl Fn(Status) + Send + Sync + 'static) {
        *lock(&self.core.status) = Some(Arc::new(on_status));
        self.core.supervised.store(true, Ordering::SeqCst);
        if self.inner().is_lost() {
            Core::start_reconnecting(&self.core);
        }
    }

    /// After `Status::GaveUp`: starts over with a new grace time.
    pub fn retry(&self) {
        if self.core.supervised.load(Ordering::SeqCst) {
            Core::start_reconnecting(&self.core);
        }
    }

    /// Whether there is a live connection now.
    pub fn is_connected(&self) -> bool {
        !self.inner().is_lost()
    }

    /// Reports the ports the host listens on: the whole list once now and again at each change (`ports::diff` tells
    /// what is new). The watch is sent again after a reconnect. Call it once per `Remote`.
    pub fn watch_ports(&self, on_ports: impl Fn(Vec<ListeningPort>) + Send + Sync + 'static) -> Result<(), String> {
        let id = self.core.next.fetch_add(1, Ordering::Relaxed);
        *lock(&self.core.sinks.ports) = Some((id, Arc::new(on_ports)));
        let inner = self.inner();
        let rx = inner.register(id).map_err(text)?;
        inner.send(id, Req::WatchPorts, &[]).map_err(text)?;
        rx.recv().map(drop).map_err(|_| lost().to_string())
    }

    /// Makes `host:port` (as the host sees it) reachable at `127.0.0.1:<local_port>`: the same number when free, else
    /// another free one. Only loopback is bound. Each connection to it is a stream through the current connection,
    /// so it works again after a reconnect; dropping the [`Forward`] closes the local port.
    pub fn forward(&self, host: &str, port: u16, preferred: u16) -> io::Result<Forward> {
        let listener = bind_local(preferred)?;
        let local_port = listener.local_addr()?.port();
        let (stop, core, host) = (Arc::new(AtomicBool::new(false)), self.core.clone(), host.to_string());
        let flag = stop.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                if flag.load(Ordering::SeqCst) {
                    return;
                }
                if let Ok(conn) = conn {
                    let (core, host) = (core.clone(), host.clone());
                    std::thread::spawn(move || drop(relay(&core, conn, &host, port)));
                }
            }
        });
        Ok(Forward { local_port, stop })
    }

    fn inner(&self) -> Arc<Inner> {
        lock(&self.core.conn).clone()
    }
}

/// A local port that leads to a port of the host; dropping it closes the local port (streams already open end).
pub struct Forward {
    pub local_port: u16,
    stop: Arc<AtomicBool>,
}

impl Drop for Forward {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        TcpStream::connect(SocketAddr::from((Ipv4Addr::LOCALHOST, self.local_port))).ok(); // wakes the accept
    }
}

/// One local connection: opens a stream on the current connection and copies local bytes into it.
fn relay(core: &Core, mut conn: TcpStream, host: &str, port: u16) -> io::Result<()> {
    let inner = lock(&core.conn).clone();
    let id = core.next.fetch_add(1, Ordering::Relaxed);
    let rx = inner.register(id)?;
    // before the request: the first bytes may follow its answer immediately
    lock(&core.sinks.streams).insert(id, conn.try_clone()?);
    let opened = inner.send(id, Req::Open { host: host.into(), port }, &[]).is_ok() && matches!(rx.recv(), Ok((Res::Ok, _)));
    if !opened {
        lock(&core.sinks.streams).remove(&id);
        return Err(lost());
    }
    let mut buf = vec![0u8; 32 << 10];
    while let Ok(n @ 1..) = conn.read(&mut buf) {
        if inner.send(core.next.fetch_add(1, Ordering::Relaxed), Req::Data { target: id }, &buf[..n]).is_err() {
            break;
        }
    }
    lock(&core.sinks.streams).remove(&id);
    inner.send(core.next.fetch_add(1, Ordering::Relaxed), Req::Cancel { target: id }, &[])
}

impl Drop for Remote {
    fn drop(&mut self) {
        self.core.closed.store(true, Ordering::SeqCst);
        if let Some(mut child) = lock(&self.core.child).take() {
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
        self.call_chunked(req, body, cancel, &mut |_| {})
    }

    /// `call` for an answer that comes in chunks: `on_chunk` gets each one, in order, before the closing answer.
    fn call_chunked(&self, req: Req, body: &[u8], cancel: Option<&AtomicBool>, on_chunk: &mut dyn FnMut(Res)) -> io::Result<Answer> {
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
                Ok((chunk @ (Res::FilesChunk { .. } | Res::SearchChunk { .. }), _)) => on_chunk(chunk),
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
        match self.inner().call(Req::Read { path: wire_path(path) }, &[], None)? {
            (Res::Bytes, body) => Ok(body),
            _ => unexpected("read"),
        }
    }

    fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        self.inner().call(Req::Write { path: wire_path(path) }, bytes, None).map(drop)
    }

    fn create(&self, path: &Path, folder: bool) -> io::Result<PathBuf> {
        match self.inner().call(Req::Create { path: wire_path(path), folder }, &[], None)? {
            (Res::Path { path }, _) => Ok(PathBuf::from(path)),
            _ => unexpected("create"),
        }
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.inner().ok(Req::Rename { from: wire_path(from), to: wire_path(to) })
    }

    fn copy(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.inner().ok(Req::Copy { from: wire_path(from), to: wire_path(to) })
    }

    /// There is no Trash on the host: deleted for good, so there is nothing to undo.
    fn trash(&self, path: &Path) -> Result<Option<PathBuf>, String> {
        self.inner().ok(Req::Trash { path: wire_path(path) }).map(|()| None).map_err(text)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        self.inner().ok(Req::RemoveFile { path: wire_path(path) })
    }

    fn metadata(&self, path: &Path) -> io::Result<Meta> {
        match self.inner().call(Req::Metadata { path: wire_path(path) }, &[], None)? {
            (Res::Meta(m), _) => Ok(Meta { is_dir: m.is_dir, is_file: m.is_file, size: m.size, mtime: m.mtime.map(|(s, n)| UNIX_EPOCH + Duration::new(s, n)) }),
            _ => unexpected("metadata"),
        }
    }

    fn exists(&self, path: &Path) -> bool {
        matches!(self.inner().call(Req::Exists { path: wire_path(path) }, &[], None), Ok((Res::Bool { value: true }, _)))
    }

    fn list_dir(&self, dir: &Path) -> Vec<(PathBuf, bool)> {
        match self.inner().call(Req::ListDir { dir: wire_path(dir), exclude: exclude() }, &[], None) {
            Ok((Res::Entries { entries }, _)) => entries.into_iter().map(|(p, d)| (PathBuf::from(p), d)).collect(),
            _ => Vec::new(),
        }
    }

    fn discover(&self, root: &Path, max_depth: usize) -> Vec<Repo> {
        match self.inner().call(Req::Discover { root: wire_path(root), max_depth }, &[], None) {
            Ok((Res::Repos { repos }, _)) => repos.into_iter().map(|r| Repo { path: PathBuf::from(r.path), worktree_of: r.worktree_of.map(PathBuf::from) }).collect(),
            _ => Vec::new(),
        }
    }

    fn files_chunked(&self, root: &Path, repos: &[PathBuf], cancel: &AtomicBool, on_chunk: &mut dyn FnMut(Vec<PathBuf>)) -> bool {
        if cancel.load(Ordering::Relaxed) {
            return true;
        }
        let req = Req::Files { root: wire_path(root), repos: strings(repos), exclude: exclude() };
        self.inner()
            .call_chunked(req, &[], Some(cancel), &mut |c| {
                if let Res::FilesChunk { files } = c {
                    on_chunk(files.into_iter().map(PathBuf::from).collect());
                }
            })
            .is_ok()
    }

    fn search_chunked(&self, root: &Path, repos: &[PathBuf], query: &Query, limit: usize, cancel: &AtomicBool, on_files: &mut dyn FnMut(Vec<FileMatches>)) -> Results {
        if cancel.load(Ordering::Relaxed) {
            return Results::default(); // like the walk that stops before its first file
        }
        let req = Req::Search { root: wire_path(root), repos: strings(repos), query: query.clone(), limit, exclude: exclude() };
        let chunk = |c| {
            if let Res::SearchChunk { files } = c {
                on_files(files.into_iter().map(|(path, lines)| FileMatches { path: PathBuf::from(path), lines }).collect());
            }
        };
        match self.inner().call_chunked(req, &[], Some(cancel), &mut { chunk }) {
            Ok((Res::Results(r), _)) => r.into(),
            Ok(_) => Results { error: Some("unexpected answer to search".into()), ..Default::default() },
            Err(e) => Results { error: Some(e.to_string()), ..Default::default() },
        }
    }

    fn watch(&self, root: &Path, on_change: Box<dyn Fn(PathBuf) + Send>) -> Result<WatchGuard, String> {
        let inner = self.inner();
        let id = self.core.next.fetch_add(1, Ordering::Relaxed);
        // before the request: events may follow its answer immediately; kept (with the root) to send it again after a reconnect
        lock(&self.core.sinks.watches).insert(id, (root.to_path_buf(), on_change));
        let guard = WatchStop { core: self.core.clone(), id };
        let rx = inner.register(id).map_err(text)?;
        inner.send(id, Req::Watch { root: wire_path(root) }, &[]).map_err(text)?;
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
        match self.inner().call(req, &[], None) {
            Ok((Res::Text { text }, _)) => Ok(text),
            Ok(_) => Err("unexpected answer to git".into()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// Dropping it stops the watch on the server and the callback here.
struct WatchStop {
    core: Arc<Core>,
    id: u64,
}

impl Drop for WatchStop {
    fn drop(&mut self) {
        lock(&self.core.sinks.watches).remove(&self.id);
        let cancel = self.core.next.fetch_add(1, Ordering::Relaxed);
        let inner = lock(&self.core.conn).clone();
        inner.send(cancel, Req::Cancel { target: self.id }, &[]).ok();
    }
}
