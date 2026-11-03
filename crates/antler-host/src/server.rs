//! The server side of the protocol: answers each request with [`Local`] on the machine it runs on.
//! `antler-server` (crates/antler-server) is `serve(stdin, stdout)`.
use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::time::UNIX_EPOCH;

use crate::host::{Host, Local, WatchGuard};
use crate::protocol::*;

/// What a running request leaves behind for `Cancel`.
enum Active {
    /// A search or the port poll: set to stop it.
    Flag(Arc<AtomicBool>),
    /// A stream to a port of this host (a forwarded port).
    Stream(TcpStream),
    /// Held: dropping it stops the watch.
    Watch(#[allow(dead_code)] WatchGuard),
}

type State = Arc<Mutex<HashMap<u64, Active>>>;

/// Serves one connection until `input` ends (the app closed it or the connection dropped); in-flight requests are
/// finished first. Every request runs on its own thread, so several are in flight at once.
///
/// Stage 3 hook: this is where a dropped connection is seen. The binary exits after `serve` returns; to stay alive
/// for a reconnect, `main` should keep the process (and its watches) and call `serve` again on the next connection.
pub fn serve<R: Read, W: Write + Send + 'static>(mut input: R, output: W) -> io::Result<()> {
    let out = Arc::new(Mutex::new(output));
    let send = {
        let out = out.clone();
        move |id: u64, res: Res, body: &[u8]| write_frame(&mut *out.lock().unwrap_or_else(|e| e.into_inner()), &ResFrame { id, res }, body)
    };
    send(0, Res::Hello { protocol: VERSION, os: std::env::consts::OS.into(), arch: std::env::consts::ARCH.into() }, &[])?;
    let state: State = Default::default();
    let mut threads = Vec::new();
    while let Some((ReqFrame { id, req }, body)) = read_frame(&mut input)? {
        threads.retain(|t: &std::thread::JoinHandle<()>| !t.is_finished());
        if let Req::Cancel { target } = req {
            state.lock().unwrap_or_else(|e| e.into_inner()).remove(&target).inspect(Active::stop);
            continue;
        }
        // in order, on the reader: bytes of one stream must not overtake each other
        match req {
            Req::Open { host, port } => {
                open_stream(id, &host, port, &state, &send);
                continue;
            }
            Req::Data { target } => {
                write_stream(target, &body, &state, &send);
                continue;
            }
            _ => {}
        }
        // registered here, not on the request's thread, so a `Cancel` right behind the request finds it
        let cancel = Arc::new(AtomicBool::new(false));
        if matches!(req, Req::Search { .. } | Req::WatchPorts) {
            state.lock().unwrap_or_else(|e| e.into_inner()).insert(id, Active::Flag(cancel.clone()));
        }
        let (send, state) = (send.clone(), state.clone());
        threads.push(std::thread::spawn(move || {
            let (res, body) = handle(id, req, body, &cancel, &state, &send);
            // a closed output means the client is gone: nothing to tell
            send(id, res, &body).ok();
        }));
    }
    state.lock().unwrap_or_else(|e| e.into_inner()).drain().for_each(|(_, a)| a.stop()); // stops the watches and streams
    threads.into_iter().for_each(|t| drop(t.join()));
    Ok(())
}

impl Active {
    fn stop(&self) {
        match self {
            Active::Flag(flag) => flag.store(true, Ordering::Relaxed),
            Active::Stream(s) => drop(s.shutdown(std::net::Shutdown::Both)),
            Active::Watch(_) => {}
        }
    }
}

fn connect(host: &str, port: u16) -> io::Result<TcpStream> {
    let mut last = io::Error::new(io::ErrorKind::NotFound, format!("no address for {host}"));
    for addr in (host, port).to_socket_addrs()? {
        match TcpStream::connect_timeout(&addr, Duration::from_secs(3)) {
            Ok(s) => return Ok(s),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// Connects to `host:port` and relays what it sends as `Data` frames with the stream's id until it closes.
fn open_stream(id: u64, host: &str, port: u16, state: &State, send: &(impl Fn(u64, Res, &[u8]) -> io::Result<()> + Clone + Send + 'static)) {
    let (stream, reader) = match connect(host, port).and_then(|s| Ok((s.try_clone()?, s))) {
        Ok(pair) => pair,
        Err(e) => return drop(send(id, io_err(e).0, &[])),
    };
    state.lock().unwrap_or_else(|e| e.into_inner()).insert(id, Active::Stream(stream));
    send(id, Res::Ok, &[]).ok();
    let (state, send) = (state.clone(), send.clone());
    std::thread::spawn(move || {
        let (mut reader, mut buf) = (reader, vec![0u8; 32 << 10]);
        while let Ok(n @ 1..) = reader.read(&mut buf) {
            if send(id, Res::Data, &buf[..n]).is_err() {
                break;
            }
        }
        // `Cancel` took it out already when the client closed first: then there is nobody to tell
        if state.lock().unwrap_or_else(|e| e.into_inner()).remove(&id).is_some() {
            send(id, Res::Closed, &[]).ok();
        }
    });
}

fn write_stream(target: u64, body: &[u8], state: &State, send: &impl Fn(u64, Res, &[u8]) -> io::Result<()>) {
    let stream = match state.lock().unwrap_or_else(|e| e.into_inner()).get(&target) {
        Some(Active::Stream(s)) => s.try_clone().ok(),
        _ => None,
    };
    // shortcut: a slow reader on the host stalls this connection's reader (one stream at a time); upgrade with a writer thread per stream
    if stream.is_some_and(|mut s| s.write_all(body).is_err()) && state.lock().unwrap_or_else(|e| e.into_inner()).remove(&target).is_some() {
        send(target, Res::Closed, &[]).ok();
    }
}

fn io_err(e: io::Error) -> (Res, Vec<u8>) {
    (Res::Err { kind: kind_name(e.kind()).into(), msg: e.to_string() }, Vec::new())
}

fn text_err(msg: impl ToString) -> (Res, Vec<u8>) {
    (Res::Err { kind: String::new(), msg: msg.to_string() }, Vec::new())
}

fn handle(id: u64, req: Req, body: Vec<u8>, cancel: &AtomicBool, state: &State, send: &(impl Fn(u64, Res, &[u8]) -> io::Result<()> + Clone + Send + 'static)) -> (Res, Vec<u8>) {
    let (p, ok) = (|s: &str| native_path(s), |r: io::Result<()>| r.map(|()| (Res::Ok, Vec::new())).unwrap_or_else(io_err));
    match req {
        Req::Read { path } => Local::new().read(&p(&path)).map(|b| (Res::Bytes, b)).unwrap_or_else(io_err),
        Req::Write { path } => ok(Local::new().write(&p(&path), &body)),
        Req::Create { path, folder } => Local::new().create(&p(&path), folder).map(|top| (Res::Path { path: wire_path(&top) }, Vec::new())).unwrap_or_else(io_err),
        Req::Rename { from, to } => ok(Local::new().rename(&p(&from), &p(&to))),
        Req::Copy { from, to } => ok(Local::new().copy(&p(&from), &p(&to))),
        Req::Trash { path } => ok(crate::host::remove_all(&p(&path))),
        Req::RemoveFile { path } => ok(Local::new().remove_file(&p(&path))),
        Req::Metadata { path } => Local::new()
            .metadata(&p(&path))
            .map(|m| {
                let mtime = m.mtime.and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| (d.as_secs(), d.subsec_nanos()));
                (Res::Meta(WireMeta { is_dir: m.is_dir, is_file: m.is_file, size: m.size, mtime }), Vec::new())
            })
            .unwrap_or_else(io_err),
        Req::Exists { path } => (Res::Bool { value: Local::new().exists(&p(&path)) }, Vec::new()),
        Req::ListDir { dir, exclude } => {
            let entries = Local::with_exclude(exclude).list_dir(&p(&dir)).into_iter().map(|(path, dir)| (wire_path(&path), dir)).collect();
            (Res::Entries { entries }, Vec::new())
        }
        Req::Discover { root, max_depth } => {
            let repos = Local::new().discover(&p(&root), max_depth);
            let repos = repos.into_iter().map(|r| WireRepo { path: wire_path(&r.path), worktree_of: r.worktree_of.map(|w| wire_path(&w)) }).collect();
            (Res::Repos { repos }, Vec::new())
        }
        Req::Files { root, repos, exclude } => {
            let repos: Vec<PathBuf> = repos.iter().map(|r| p(r)).collect();
            let files = Local::with_exclude(exclude).files(&p(&root), &repos).iter().map(|f| wire_path(f)).collect();
            (Res::Files { files }, Vec::new())
        }
        Req::Search { root, repos, query, limit, exclude } => {
            let repos: Vec<PathBuf> = repos.iter().map(|r| p(r)).collect();
            let results = Local::with_exclude(exclude).search(&p(&root), &repos, &query, limit, cancel);
            state.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
            (Res::Results(results.into()), Vec::new())
        }
        Req::Watch { root } => {
            let send = send.clone();
            let guard = Local::new().watch(&p(&root), Box::new(move |path| drop(send(id, Res::Event { path: wire_path(&path) }, &[]))));
            match guard {
                Ok(g) => {
                    state.lock().unwrap_or_else(|e| e.into_inner()).insert(id, Active::Watch(g));
                    (Res::Ok, Vec::new())
                }
                Err(e) => text_err(e),
            }
        }
        Req::Git { repo, args, stdin, envs } => {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            let envs: Vec<(&str, &OsStr)> = envs.iter().map(|(k, v)| (k.as_str(), OsStr::new(v))).collect();
            Local::new().git(Path::new(&p(&repo)), &args, stdin.as_deref(), &envs).map(|text| (Res::Text { text }, Vec::new())).unwrap_or_else(text_err)
        }
        Req::WatchPorts => {
            send(id, Res::Ok, &[]).ok();
            let (mut detector, mut last) = (crate::ports::Detector::default(), Vec::new());
            let mut first = true;
            'poll: while !cancel.load(Ordering::Relaxed) {
                let now = detector.snapshot();
                if first || now != last {
                    if send(id, Res::Ports { ports: now.clone() }, &[]).is_err() {
                        break;
                    }
                    (first, last) = (false, now);
                }
                for _ in 0..10 {
                    if cancel.load(Ordering::Relaxed) {
                        break 'poll;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
            (Res::Ok, Vec::new()) // nobody waits for it: the client forgot this id
        }
        Req::Open { .. } | Req::Data { .. } | Req::Cancel { .. } => unreachable!("handled by the reader"),
    }
}

