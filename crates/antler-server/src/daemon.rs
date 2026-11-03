//! The daemon that outlives the SSH connection and the proxy each connection runs (docs/adr/0005).
//!
//! `antler-server daemon` listens on `127.0.0.1` (random port) and writes `<port> <token>` to
//! `<base>/<version>/daemon` (owner-only). `antler-server proxy` reads that file, connects, sends the token and then
//! copies its stdin/stdout to the socket, so the protocol of `serve` runs unchanged on top. No daemon (or a stale
//! file): the proxy starts one. The daemon exits after `grace` with no client (1 hour; VS Code uses 3, see docs/vscode-differences.md; the
//! client retries for the same hour).
//! Loopback TCP with a token, on every OS: the smallest thing that works on Windows too (no named pipes, no cfg).
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The daemon's idle time before it exits.
pub fn grace() -> Duration {
    Duration::from_secs(std::env::var("ANTLER_SERVER_GRACE_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(3600))
}

/// `~/.antler-server` (`ANTLER_SERVER_DIR` for tests).
pub fn base_dir() -> io::Result<PathBuf> {
    if let Some(d) = std::env::var_os("ANTLER_SERVER_DIR") {
        return Ok(d.into());
    }
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).ok_or_else(|| io::Error::other("no home folder"))?;
    Ok(Path::new(&home).join(".antler-server"))
}

fn info_file(base: &Path) -> PathBuf {
    base.join(VERSION).join("daemon")
}

/// 128 random bits: `RandomState` is seeded by the OS, which saves a dependency.
fn token() -> String {
    (0..2).map(|_| format!("{:016x}", RandomState::new().build_hasher().finish())).collect()
}

fn write_private(path: &Path, text: &str) -> io::Result<()> {
    std::fs::create_dir_all(path.parent().unwrap())?;
    let tmp = path.with_extension("tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(&tmp)?.write_all(text.as_bytes())?;
    std::fs::rename(tmp, path) // a reader never sees half a file
}

/// One line, byte by byte so nothing after it is consumed.
fn read_line(s: &mut impl Read) -> io::Result<String> {
    let (mut line, mut b) = (Vec::new(), [0u8]);
    while line.len() < 200 && s.read(&mut b)? == 1 && b[0] != b'\n' {
        line.push(b[0]);
    }
    Ok(String::from_utf8_lossy(&line).into_owned())
}

pub fn run_daemon(base: &Path, grace: Duration) -> io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    listener.set_nonblocking(true)?;
    let token = token();
    let file = info_file(base);
    write_private(&file, &format!("{} {token}\n", listener.local_addr()?.port()))?;
    let active = Arc::new(AtomicUsize::new(0));
    let mut idle_since = Instant::now();
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                active.fetch_add(1, Ordering::SeqCst);
                let (active, token) = (active.clone(), token.clone());
                std::thread::spawn(move || {
                    drop(client(stream, &token));
                    active.fetch_sub(1, Ordering::SeqCst);
                });
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(e),
        }
        if active.load(Ordering::SeqCst) > 0 {
            idle_since = Instant::now();
        } else if idle_since.elapsed() >= grace {
            break;
        }
    }
    std::fs::remove_file(file).ok();
    Ok(())
}

fn client(mut stream: TcpStream, token: &str) -> io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    if read_line(&mut stream)? != token {
        return Ok(());
    }
    stream.set_read_timeout(None)?;
    antler_host::server::serve(stream.try_clone()?, stream)
}

fn attach(base: &Path) -> io::Result<TcpStream> {
    let text = std::fs::read_to_string(info_file(base))?;
    let (port, token) = text.trim().split_once(' ').ok_or_else(|| io::Error::other("bad daemon file"))?;
    let port: u16 = port.parse().map_err(io::Error::other)?;
    let mut stream = TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_secs(3))?;
    stream.write_all(format!("{token}\n").as_bytes())?;
    Ok(stream)
}

fn start_daemon(base: &Path) -> io::Result<()> {
    std::fs::remove_file(info_file(base)).ok(); // stale: its daemon is gone
    let mut c = Command::new(std::env::current_exe()?);
    c.arg("daemon").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // out of the ssh session's process group / console, so closing the connection does not take it along
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut c, 0);
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut c, 0x0000_0008 | 0x0000_0200); // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
    c.spawn().map(drop)
}

pub fn run_proxy(base: &Path) -> io::Result<()> {
    let stream = match attach(base) {
        Ok(s) => s,
        Err(_) => {
            // shortcut: two proxies starting at the same instant may start two daemons (the later file wins, the
            // other leaves after its grace time); a lock file would close it.
            start_daemon(base)?;
            let until = Instant::now() + Duration::from_secs(10);
            loop {
                match attach(base) {
                    Ok(s) => break s,
                    Err(e) if Instant::now() > until => return Err(e),
                    Err(_) => std::thread::sleep(Duration::from_millis(50)),
                }
            }
        }
    };
    let mut up = stream.try_clone()?;
    std::thread::spawn(move || {
        let mut stdin = io::stdin().lock();
        let mut buf = vec![0u8; 64 * 1024];
        while let Ok(n) = stdin.read(&mut buf) {
            if n == 0 || up.write_all(&buf[..n]).is_err() {
                break;
            }
        }
        up.shutdown(Shutdown::Write).ok(); // the daemon's `serve` ends and answers what is in flight
    });
    let (mut down, mut stdout) = (stream, io::stdout().lock());
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        match down.read(&mut buf)? {
            0 => return Ok(()),
            n => {
                stdout.write_all(&buf[..n])?;
                stdout.flush()?;
            }
        }
    }
}
