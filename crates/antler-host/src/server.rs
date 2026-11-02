//! The server side of the protocol: answers each request with [`Local`] on the machine it runs on.
//! `antler-server` (crates/antler-server) is `serve(stdin, stdout)`.
use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use crate::host::{Host, Local, WatchGuard};
use crate::protocol::*;

/// What a running request leaves behind for `Cancel`.
enum Active {
    Search(Arc<AtomicBool>),
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
            state.lock().unwrap_or_else(|e| e.into_inner()).remove(&target).inspect(|a| {
                if let Active::Search(flag) = a {
                    flag.store(true, Ordering::Relaxed);
                }
            });
            continue;
        }
        // registered here, not on the request's thread, so a `Cancel` right behind the request finds it
        let cancel = Arc::new(AtomicBool::new(false));
        if matches!(req, Req::Search { .. }) {
            state.lock().unwrap_or_else(|e| e.into_inner()).insert(id, Active::Search(cancel.clone()));
        }
        let (send, state) = (send.clone(), state.clone());
        threads.push(std::thread::spawn(move || {
            let (res, body) = handle(id, req, body, &cancel, &state, &send);
            // a closed output means the client is gone: nothing to tell
            send(id, res, &body).ok();
        }));
    }
    state.lock().unwrap_or_else(|e| e.into_inner()).clear(); // stops the watches
    threads.into_iter().for_each(|t| drop(t.join()));
    Ok(())
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
        Req::Cancel { .. } => unreachable!("handled by the reader"),
    }
}

