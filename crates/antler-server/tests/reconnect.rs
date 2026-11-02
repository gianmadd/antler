//! A `Remote` whose connection is killed reconnects by itself, re-issues its watches and says what it is doing.
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use antler_host::host::Host;
use antler_host::remote::{Remote, Status, Transport};

#[test]
fn a_dropped_connection_comes_back_with_its_watches() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("srv");
    let work = std::fs::canonicalize(tmp.path()).unwrap().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let child: Arc<Mutex<Option<Child>>> = Default::default();
    let connector = {
        let (dir, child) = (dir.clone(), child.clone());
        Arc::new(move || -> Result<Transport, String> {
            let mut c = Command::new(env!("CARGO_BIN_EXE_antler-server"))
                .arg("proxy")
                .env("ANTLER_SERVER_DIR", &dir)
                .env("ANTLER_SERVER_GRACE_SECS", "2")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .map_err(|e| e.to_string())?;
            let (i, o) = (c.stdout.take().unwrap(), c.stdin.take().unwrap());
            *child.lock().unwrap() = Some(c);
            Ok((Box::new(i), Box::new(o), None))
        })
    };
    let remote = Remote::open(connector).unwrap();
    let (status, statuses) = channel();
    remote.supervise(move |s| drop(status.send(s)));
    let (tx, events) = channel();
    let _guard = remote.watch(&work, Box::new(move |p| drop(tx.send(p)))).unwrap();

    child.lock().unwrap().take().unwrap().kill().unwrap(); // the connection drops
    let seen: Vec<Status> = std::iter::from_fn(|| statuses.recv_timeout(Duration::from_secs(10)).ok()).take(2).collect();
    assert!(matches!(seen[0], Status::Reconnecting { attempt: 1 }) && matches!(seen[1], Status::Connected), "{seen:?}");

    std::fs::write(work.join("new.txt"), "x").unwrap();
    assert!(events.recv_timeout(Duration::from_secs(10)).is_ok(), "the watch works again on the new connection");
    assert_eq!(remote.read(&work.join("new.txt")).unwrap(), b"x");
}
