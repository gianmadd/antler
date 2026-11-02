//! The real `antler-server` binary as daemon + proxy (docs/adr/0005): a second proxy attaches to the daemon the
//! first one started, and the daemon leaves after its grace time with no client.
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use antler_host::host::Host;
use antler_host::remote::Remote;

fn proxy(dir: &Path) -> Remote {
    let mut c = Command::new(env!("CARGO_BIN_EXE_antler-server"));
    c.arg("proxy").env("ANTLER_SERVER_DIR", dir).env("ANTLER_SERVER_GRACE_SECS", "2");
    Remote::spawn(c).unwrap()
}

fn daemon_file(dir: &Path) -> std::path::PathBuf {
    dir.join(env!("CARGO_PKG_VERSION")).join("daemon")
}

#[test]
fn proxies_share_one_daemon_which_exits_after_the_grace_time() {
    let tmp = tempfile::tempdir().unwrap();
    let (dir, work) = (tmp.path().join("srv"), tmp.path().join("work"));
    std::fs::create_dir_all(&work).unwrap();
    let file = work.join("a.txt");

    let first = proxy(&dir);
    first.write(&file, b"kept").unwrap();
    let info = std::fs::read_to_string(daemon_file(&dir)).unwrap();
    drop(first); // the connection ends, the daemon stays

    let second = proxy(&dir);
    assert_eq!(second.read(&file).unwrap(), b"kept");
    assert_eq!(std::fs::read_to_string(daemon_file(&dir)).unwrap(), info, "the second proxy attached to the same daemon");
    // two clients at once
    let third = proxy(&dir);
    assert_eq!(third.read(&file).unwrap(), b"kept");
    drop((second, third));

    let until = Instant::now() + Duration::from_secs(15);
    while daemon_file(&dir).exists() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(!daemon_file(&dir).exists(), "the daemon exited after its grace time");
}
