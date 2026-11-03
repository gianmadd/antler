//! End to end without SSH: the real `antler-server` binary as a child process, driven through `Remote`, and the
//! same operations on `Local` for comparison.
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::channel;
use std::time::Duration;

use antler_host::host::{Host, Local};
use antler_host::remote::Remote;
use antler_host::search::Query;

fn remote() -> Remote {
    Remote::spawn(Command::new(env!("CARGO_BIN_EXE_antler-server"))).unwrap()
}

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git").arg("-C").arg(dir).args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args).status().unwrap().success();
    assert!(ok, "git {args:?}");
}

/// A Root with two repos (one nested in a folder), a skipped folder and a few files; canonical, like the app's Root.
fn fixture() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = antler_host::host::canonical(tmp.path()).unwrap();
    for (rel, text) in [("a/x.txt", "needle one\nhay\n"), ("a/y.rs", "fn needle() {}\n"), ("b/z.txt", "none here\n"), ("top.txt", "Needle on top\n"), ("skipme/s.txt", "needle\n")] {
        fs::create_dir_all(root.join(rel).parent().unwrap()).unwrap();
        fs::write(root.join(rel), text).unwrap();
    }
    for repo in ["a", "b/inner"] {
        fs::create_dir_all(root.join(repo)).unwrap();
        git(&root.join(repo), &["init", "-q"]);
    }
    fs::write(root.join("a/.gitignore"), "ignored.txt\n").unwrap();
    fs::write(root.join("a/ignored.txt"), "needle\n").unwrap();
    (tmp, root)
}

fn debug(r: &impl std::fmt::Debug) -> String {
    // a Windows path: the remote keeps the wire's `/`, Local has `\` (equal as paths, not as text)
    format!("{r:?}").replace(r"\\", "/")
}

#[test]
fn the_server_says_hello_and_files_round_trip_with_the_errors_local_gives() {
    let (_tmp, root) = fixture();
    let (r, l) = (remote(), Local::new());
    assert_eq!(r.server.os, std::env::consts::OS);
    assert_eq!(r.server.arch, std::env::consts::ARCH);

    let bytes: Vec<u8> = (0..=255).chain(0..=255).collect(); // not UTF-8, every byte value
    for h in [&r as &dyn Host, &l] {
        let dir = root.join(if std::ptr::addr_eq(h, &r as &dyn Host) { "remote" } else { "local" });
        assert_eq!(h.create(&dir.join("d/e.bin"), false).unwrap(), dir);
        assert_eq!(h.create(&dir.join("d/e.bin"), false).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        h.write(&dir.join("d/e.bin"), &bytes).unwrap();
        assert_eq!(h.read(&dir.join("d/e.bin")).unwrap(), bytes);
        assert_eq!(h.read_to_string(&dir.join("d/e.bin")).unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert_eq!(h.read(&dir.join("nope")).unwrap_err().kind(), io::ErrorKind::NotFound);
        let m = h.metadata(&dir.join("d/e.bin")).unwrap();
        assert!(m.is_file && !m.is_dir && m.size == 512 && m.mtime.is_some());
        assert_eq!(h.metadata(&dir.join("nope")).unwrap_err().kind(), io::ErrorKind::NotFound);
        h.write(&dir.join("f"), b"f").unwrap();
        assert_eq!(h.rename(&dir.join("f"), &dir.join("d/e.bin")).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        h.rename(&dir.join("f"), &dir.join("g")).unwrap();
        h.copy(&dir.join("d"), &dir.join("d2")).unwrap();
        assert_eq!(h.read(&dir.join("d2/e.bin")).unwrap(), bytes);
        assert!(h.exists(&dir.join("g")) && !h.exists(&dir.join("f")));
        h.remove_file(&dir.join("g")).unwrap();
        assert!(!h.exists(&dir.join("g")));
    }
    // the remote has no Trash: gone for good, and nothing to undo
    assert_eq!(r.trash(&root.join("remote/d2")).unwrap(), None);
    assert!(!root.join("remote/d2").exists());
    assert!(r.trash(&root.join("remote/d2")).is_err());
    assert_eq!(r.list_dir(&root.join("remote")), [(root.join("remote/d"), true)]);
}

#[test]
fn scans_and_search_give_what_local_gives() {
    let (_tmp, root) = fixture();
    antler_host::exclude::set_excluded(vec!["**/skipme".into()]); // the app's settings travel with each request
    let (r, l) = (remote(), Local::new());

    assert_eq!(debug(&r.list_dir(&root)), debug(&l.list_dir(&root)));
    assert!(!r.list_dir(&root).iter().any(|(p, _)| p.ends_with("skipme")));
    assert_eq!(r.discover(&root, 3), l.discover(&root, 3));
    assert_eq!(r.discover(&root, 3).len(), 2);
    let repos: Vec<PathBuf> = l.discover(&root, 3).into_iter().map(|x| x.path).collect();
    assert_eq!(r.files(&root, &repos), l.files(&root, &repos));
    assert!(!r.files(&root, &repos).iter().any(|p| p.ends_with("ignored.txt") || p.ends_with("s.txt")));

    let no = AtomicBool::new(false);
    for query in [
        Query { text: "needle".into(), ..Default::default() },
        Query { text: "Needle".into(), case_sensitive: true, ..Default::default() },
        Query { text: "n.*e".into(), regex: true, include: "*.rs".into(), ..Default::default() },
        Query { text: "needle".into(), use_ignore_files: false, ..Default::default() },
        Query { text: "(".into(), regex: true, ..Default::default() }, // an error travels as text
    ] {
        let (a, b) = (r.search(&root, &repos, &query, 100, &no), l.search(&root, &repos, &query, 100, &no));
        assert_eq!(debug(&a), debug(&b), "{query:?}");
    }
    assert_eq!(r.search(&root, &repos, &Query { text: "needle".into(), ..Default::default() }, 100, &no).count(), 3);
    // a limit truncates
    assert!(r.search(&root, &repos, &Query { text: "needle".into(), ..Default::default() }, 1, &no).truncated);
    // a search cancelled from the start still answers
    let yes = AtomicBool::new(true);
    assert!(r.search(&root, &repos, &Query { text: "needle".into(), ..Default::default() }, 100, &yes).error.is_none());
}

#[test]
fn watch_reports_changes_until_its_guard_is_dropped() {
    let (_tmp, root) = fixture();
    let r = remote();
    let (tx, rx) = channel();
    let guard = r.watch(&root, Box::new(move |p| drop(tx.send(p)))).unwrap();
    fs::write(root.join("b/new.txt"), "x").unwrap();
    let mut seen = false;
    while let Ok(p) = rx.recv_timeout(Duration::from_secs(10)) {
        if p.ends_with("b/new.txt") {
            seen = true;
            break;
        }
    }
    assert!(seen, "no event for the new file");
    drop(guard);
    std::thread::sleep(Duration::from_millis(300));
    while rx.try_recv().is_ok() {}
    fs::write(root.join("b/after.txt"), "x").unwrap();
    assert!(rx.recv_timeout(Duration::from_millis(800)).is_err(), "an event arrived after the guard was dropped");
    assert!(r.watch(&root.join("missing"), Box::new(|_| {})).is_err());
}

#[test]
fn git_runs_on_the_server_and_reports_its_errors() {
    let (_tmp, root) = fixture();
    let (r, l) = (remote(), Local::new());
    let repo = root.join("a");
    assert_eq!(r.git(&repo, &["status", "--porcelain"], None, &[]).unwrap(), l.git(&repo, &["status", "--porcelain"], None, &[]).unwrap());
    assert!(r.git(&repo, &["status", "--porcelain"], None, &[]).unwrap().contains("x.txt"));
    // stdin and environment reach git
    let sha = r.git(&repo, &["hash-object", "--stdin"], Some("hello\n"), &[]).unwrap();
    assert_eq!(sha.trim(), "ce013625030ba8dba906f756967f9e9ca394464a");
    let env = [("GIT_AUTHOR_NAME", std::ffi::OsStr::new("Zed"))];
    assert_eq!(r.git(&repo, &["var", "GIT_AUTHOR_IDENT"], None, &env).unwrap().split('<').next().unwrap().trim(), "Zed");
    assert!(r.git(&repo, &["no-such-subcommand"], None, &[]).unwrap_err().contains("no-such-subcommand"));
}

#[test]
fn many_requests_in_flight_from_several_threads_each_get_their_own_answer() {
    let (_tmp, root) = fixture();
    let r = std::sync::Arc::new(remote());
    for n in 0..8 {
        fs::write(root.join(format!("f{n}")), vec![n as u8; 100_000 * (n + 1)]).unwrap();
    }
    let threads: Vec<_> = (0..8)
        .map(|n| {
            let (r, root) = (r.clone(), root.clone());
            std::thread::spawn(move || {
                for _ in 0..20 {
                    let bytes = r.read(&root.join(format!("f{n}"))).unwrap();
                    assert!(bytes.len() == 100_000 * (n + 1) && bytes.iter().all(|b| *b == n as u8));
                    assert_eq!(r.git(&root.join("a"), &["rev-parse", "--is-inside-work-tree"], None, &[]).unwrap().trim(), "true");
                }
            })
        })
        .collect();
    threads.into_iter().for_each(|t| t.join().unwrap());
}

#[test]
fn a_server_that_is_gone_fails_calls_instead_of_hanging() {
    use antler_host::protocol::{Res, ResFrame, VERSION, write_frame};
    assert!(Remote::connect(io::empty(), io::sink()).is_err(), "no hello");
    let bad = |protocol| {
        let mut hello = Vec::new();
        write_frame(&mut hello, &ResFrame { id: 0, res: Res::Hello { protocol, os: "x".into(), arch: "y".into() } }, &[]).unwrap();
        hello
    };
    assert!(Remote::connect(io::Cursor::new(bad(VERSION + 1)), io::sink()).is_err(), "another protocol version");
    // a server that says hello and then dies: its output ends while a request is waiting
    let (reader, mut writer) = io::pipe().unwrap();
    let (_requests_in, requests_out) = io::pipe().unwrap();
    io::Write::write_all(&mut writer, &bad(VERSION)).unwrap();
    let r = Remote::connect(reader, requests_out).unwrap();
    let waiting = std::thread::scope(|s| {
        let call = s.spawn(|| r.read(Path::new("/x")));
        std::thread::sleep(Duration::from_millis(200));
        drop(writer);
        call.join().unwrap()
    });
    assert!(waiting.is_err());
    assert!(r.read(Path::new("/x")).is_err());
    assert!(!r.exists(Path::new("/x")));
}
