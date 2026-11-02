//! Ports the host listens on (stage 4 of #20): what `antler-server` polls and what the app diffs. Parsing is pure
//! (tests below); [`Detector::snapshot`] runs the OS's own tool: `/proc` on Linux, `lsof` on macOS and other
//! Unixes, `netstat` + `tasklist` on Windows. TCP only, any address.
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::process::Command;

use serde::{Deserialize, Serialize};

/// A TCP port in the LISTEN state on the host.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ListeningPort {
    pub port: u16,
    pub pid: Option<u32>,
    /// Command line or program name; empty when the OS does not say (another user's process on Linux).
    pub process: String,
}

/// `(opened, closed)` between two snapshots, by port number. A port that stays open but changes process is not an event.
pub fn diff(prev: &[ListeningPort], now: &[ListeningPort]) -> (Vec<ListeningPort>, Vec<u16>) {
    let opened = now.iter().filter(|n| !prev.iter().any(|p| p.port == n.port)).cloned().collect();
    let closed = prev.iter().filter(|p| !now.iter().any(|n| n.port == p.port)).map(|p| p.port).collect();
    (opened, closed)
}

/// Binds `127.0.0.1:preferred`, or any other free port of `127.0.0.1` when that one is taken or not allowed.
/// Never another interface (`remote.localPortHost` = `localhost`).
pub fn bind_local(preferred: u16) -> std::io::Result<TcpListener> {
    TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, preferred))).or_else(|_| TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))))
}

/// `/proc/net/tcp` or `tcp6`: `(port, inode)` of the sockets in state `0A` (LISTEN).
pub fn parse_proc_net_tcp(text: &str) -> Vec<(u16, u64)> {
    text.lines()
        .skip(1)
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.get(3) != Some(&"0A") {
                return None;
            }
            let port = u16::from_str_radix(f.get(1)?.rsplit(':').next()?, 16).ok()?;
            Some((port, f.get(9)?.parse().ok()?))
        })
        .collect()
}

/// `lsof -nP -iTCP -sTCP:LISTEN -F pcn`: a `p<pid>` line, a `c<command>` line, then one `n<addr>:<port>` per socket.
pub fn parse_lsof(text: &str) -> Vec<ListeningPort> {
    let (mut pid, mut cmd, mut out) = (None, String::new(), Vec::new());
    for l in text.lines() {
        match l.split_at_checked(1) {
            Some(("p", v)) => pid = v.parse().ok(),
            Some(("c", v)) => cmd = v.to_string(),
            Some(("n", v)) => {
                if let Some(port) = v.rsplit(':').next().and_then(|p| p.parse().ok()) {
                    out.push(ListeningPort { port, pid, process: cmd.clone() });
                }
            }
            _ => {}
        }
    }
    out
}

/// `netstat -ano -p tcp`: `(port, pid)` of the lines whose foreign address has port 0 (a listening socket; the
/// state's word depends on the language of Windows).
pub fn parse_netstat(text: &str) -> Vec<(u16, u32)> {
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.len() < 5 || !f[0].eq_ignore_ascii_case("TCP") || !f[2].ends_with(":0") {
                return None;
            }
            Some((f[1].rsplit(':').next()?.parse().ok()?, f[4].parse().ok()?))
        })
        .collect()
}

/// `tasklist /fo csv /nh`: `"name.exe","1136",...` to `pid -> name`.
pub fn parse_tasklist(text: &str) -> HashMap<u32, String> {
    text.lines()
        .filter_map(|l| {
            let mut f = l.trim().trim_matches('"').split("\",\"");
            let name = f.next()?;
            Some((f.next()?.parse().ok()?, name.to_string()))
        })
        .collect()
}

/// Keeps what it learned about processes between polls (scanning `/proc/*/fd` every second would be wasteful).
#[derive(Default)]
pub struct Detector {
    owners: HashMap<u64, (u32, String)>,
}

fn run(cmd: &str, args: &[&str]) -> String {
    Command::new(cmd).args(args).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
}

impl Detector {
    /// The ports listening now, sorted, one entry per port, without this process's own sockets.
    pub fn snapshot(&mut self) -> Vec<ListeningPort> {
        let mut ports = if cfg!(target_os = "linux") {
            self.linux()
        } else if cfg!(windows) {
            let names = parse_tasklist(&run("tasklist", &["/fo", "csv", "/nh"]));
            parse_netstat(&run("netstat", &["-ano", "-p", "tcp"])).into_iter().map(|(port, pid)| ListeningPort { port, pid: Some(pid), process: names.get(&pid).cloned().unwrap_or_default() }).collect()
        } else {
            parse_lsof(&run("lsof", &["-nP", "-iTCP", "-sTCP:LISTEN", "-F", "pcn"]))
        };
        ports.retain(|p| p.pid != Some(std::process::id()));
        ports.sort();
        ports.dedup_by_key(|p| p.port);
        ports
    }

    fn linux(&mut self) -> Vec<ListeningPort> {
        let listening: Vec<(u16, u64)> = ["/proc/net/tcp", "/proc/net/tcp6"].iter().flat_map(|f| parse_proc_net_tcp(&std::fs::read_to_string(f).unwrap_or_default())).collect();
        self.owners.retain(|i, _| listening.iter().any(|(_, l)| l == i));
        if listening.iter().any(|(_, i)| !self.owners.contains_key(i)) {
            self.scan_fds();
        }
        listening
            .into_iter()
            .map(|(port, inode)| {
                let (pid, process) = self.owners.get(&inode).cloned().unzip();
                ListeningPort { port, pid, process: process.unwrap_or_default() }
            })
            .collect()
    }

    /// Which process holds which socket inode (only the processes we may look into).
    fn scan_fds(&mut self) {
        for entry in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
            let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
            let Ok(fds) = std::fs::read_dir(entry.path().join("fd")) else { continue };
            for fd in fds.flatten() {
                let target = std::fs::read_link(fd.path()).unwrap_or_default();
                let Some(inode) = target.to_str().and_then(|t| t.strip_prefix("socket:[")).and_then(|t| t.strip_suffix(']')).and_then(|t| t.parse::<u64>().ok()) else { continue };
                let cmdline = std::fs::read(entry.path().join("cmdline")).unwrap_or_default();
                let mut name = String::from_utf8_lossy(&cmdline).replace('\0', " ").trim().to_string();
                if name.is_empty() {
                    name = std::fs::read_to_string(entry.path().join("comm")).unwrap_or_default().trim().to_string();
                }
                self.owners.entry(inode).or_insert((pid, name));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lp(port: u16) -> ListeningPort {
        ListeningPort { port, ..Default::default() }
    }

    #[test]
    fn proc_net_tcp_keeps_only_listening_sockets() {
        let text = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n   0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 12345 1 0000\n   1: 0100007F:D2A4 0100007F:1F90 01 00000000:00000000 00:00000000 00000000  1000        0 999 1 0000\n   2: 00000000000000000000000000000000:0016 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 777 1 0000\n";
        assert_eq!(parse_proc_net_tcp(text), vec![(8080, 12345), (22, 777)]);
    }

    #[test]
    fn lsof_fields_give_port_pid_and_command() {
        let text = "p100\ncnode\nn*:3000\nn127.0.0.1:3001\np200\ncpython3\nn[::1]:8000\n";
        let ports = parse_lsof(text);
        assert_eq!(ports.iter().map(|p| (p.port, p.pid, p.process.as_str())).collect::<Vec<_>>(), [(3000, Some(100), "node"), (3001, Some(100), "node"), (8000, Some(200), "python3")]);
    }

    #[test]
    fn netstat_and_tasklist_on_windows() {
        let text = "Active Connections\n\n  Proto  Local Address          Foreign Address        State           PID\n  TCP    0.0.0.0:135            0.0.0.0:0              LISTENING       1136\n  TCP    [::]:3000              [::]:0                 LISTENING       4242\n  TCP    10.0.0.2:50000         93.184.216.34:443      ESTABLISHED     4242\n";
        assert_eq!(parse_netstat(text), vec![(135, 1136), (3000, 4242)]);
        let names = parse_tasklist("\"svchost.exe\",\"1136\",\"Services\",\"0\",\"9,000 K\"\n\"node.exe\",\"4242\",\"Console\",\"1\",\"50 K\"\n");
        assert_eq!((names[&1136].as_str(), names[&4242].as_str()), ("svchost.exe", "node.exe"));
    }

    #[test]
    fn diff_reports_new_and_gone_ports_by_number() {
        let (opened, closed) = diff(&[lp(22), lp(3000)], &[lp(22), lp(8080)]);
        assert_eq!((opened, closed), (vec![lp(8080)], vec![3000]));
    }

    #[test]
    fn bind_local_takes_the_port_when_free_else_another_one_on_loopback() {
        let first = bind_local(0).unwrap();
        let taken = first.local_addr().unwrap().port();
        let second = bind_local(taken).unwrap();
        assert_ne!(second.local_addr().unwrap().port(), taken);
        assert!(second.local_addr().unwrap().ip().is_loopback());
        drop(first);
        assert_eq!(bind_local(taken).unwrap().local_addr().unwrap().port(), taken);
    }

    #[test]
    fn the_real_snapshot_sees_a_listener_of_another_process_but_not_ours() {
        let own = TcpListener::bind("127.0.0.1:0").unwrap();
        let own_port = own.local_addr().unwrap().port();
        let ports = Detector::default().snapshot();
        assert!(!ports.iter().any(|p| p.port == own_port), "own sockets are skipped");
        // a child listening is only checked where the tool exists
        if cfg!(unix) && Command::new("python3").arg("--version").output().is_ok() {
            let mut child = Command::new("python3").args(["-c", "import socket,time;s=socket.socket();s.bind(('127.0.0.1',0));s.listen();print(s.getsockname()[1],flush=True);time.sleep(5)"]).stdout(std::process::Stdio::piped()).spawn().unwrap();
            let mut line = String::new();
            std::io::BufRead::read_line(&mut std::io::BufReader::new(child.stdout.take().unwrap()), &mut line).unwrap();
            let port: u16 = line.trim().parse().unwrap();
            let seen = Detector::default().snapshot().into_iter().find(|p| p.port == port);
            child.kill().ok();
            child.wait().ok();
            assert_eq!(seen.and_then(|p| p.pid), Some(child.id()));
        }
    }
}
