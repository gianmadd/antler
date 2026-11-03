//! Port forwarding end to end: the real `antler-server` binary, a TCP echo server standing in for the host's app.
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::sync::mpsc::channel;
use std::time::Duration;

use antler_host::ports::{ListeningPort, diff};
use antler_host::remote::Remote;

fn remote() -> Remote {
    Remote::spawn(Command::new(env!("CARGO_BIN_EXE_antler-server"))).unwrap()
}

/// Echoes every connection; says on the channel when one ends.
fn echo() -> (u16, std::sync::mpsc::Receiver<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let (mut conn, tx) = (conn.unwrap(), tx.clone());
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                while let Ok(n @ 1..) = conn.read(&mut buf) {
                    conn.write_all(&buf[..n]).unwrap();
                }
                tx.send(()).unwrap();
            });
        }
    });
    (port, rx)
}

fn connect(port: u16) -> TcpStream {
    let s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s
}

#[test]
// on Windows about one run in five, one of the two host-side streams is not closed after the client's Cancel (the
// server shuts it down, the echo only sees a reset when the server exits): to be fixed in the forwarding code (#40 gaps)
#[cfg_attr(windows, ignore = "flaky on Windows: a host-side stream is sometimes not closed")]
fn bytes_round_trip_through_a_forwarded_port_and_it_closes_cleanly() {
    let (echo_port, ended) = echo();
    let r = remote();
    let fwd = r.forward("127.0.0.1", echo_port, 0).unwrap();
    // two connections at once, the bigger one larger than a frame's chunk
    let (mut a, mut b) = (connect(fwd.local_port), connect(fwd.local_port));
    let big: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    let writer = {
        let (mut w, big) = (a.try_clone().unwrap(), big.clone());
        std::thread::spawn(move || w.write_all(&big).unwrap())
    };
    b.write_all(b"hello").unwrap();
    let mut small = [0u8; 5];
    b.read_exact(&mut small).unwrap();
    assert_eq!(&small, b"hello");
    let mut got = vec![0u8; big.len()];
    a.read_exact(&mut got).unwrap();
    writer.join().unwrap();
    assert!(got == big);
    drop((a, b));
    for _ in 0..2 {
        ended.recv_timeout(Duration::from_secs(10)).expect("the host side of each stream is closed");
    }
    drop(fwd);
}

#[test]
fn a_port_nobody_listens_on_closes_the_local_connection() {
    let dead = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let r = remote();
    let fwd = r.forward("127.0.0.1", dead, 0).unwrap();
    let mut c = connect(fwd.local_port);
    let mut buf = [0u8; 1];
    assert_eq!(c.read(&mut buf).unwrap_or(0), 0);
}

#[test]
fn the_server_reports_a_new_listener_and_its_end() {
    let r = remote();
    let (tx, rx) = channel();
    r.watch_ports(move |ports| drop(tx.send(ports))).unwrap();
    let mut known: Vec<ListeningPort> = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut opened = None;
    while opened.is_none() {
        let now = rx.recv_timeout(Duration::from_secs(10)).expect("the new port is reported within a few polls");
        opened = diff(&known, &now).0.into_iter().find(|p| p.port == port);
        known = now;
    }
    assert_eq!(opened.unwrap().pid, Some(std::process::id()));
    drop(listener);
    loop {
        let now = rx.recv_timeout(Duration::from_secs(10)).expect("the end is reported");
        if diff(&known, &now).1.contains(&port) {
            break;
        }
        known = now;
    }
}
