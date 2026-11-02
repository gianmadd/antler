//! The wire protocol between the app (`remote::Remote`) and `antler-server` (`server::serve`), over the server's
//! stdin and stdout (docs/agents/remote.md has the rationale).
//!
//! A frame is `u32 header_len (BE) | u32 body_len (BE) | header | body`. The header is one JSON object (readable in
//! a captured stream); the body is raw bytes, used by file content only, so it needs no base64.
//! The client sends [`ReqFrame`]s, the server answers with [`ResFrame`]s carrying the same `id`. Requests are
//! multiplexed: answers come in any order. A watch is answered by one `Ok` and then any number of `Event`s with its id,
//! until the client cancels it. The first frame of a connection is the server's `Hello` (id 0).
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::ports::ListeningPort;
use crate::search::{FileMatches, LineMatch, Query, Results};

/// Bumped on any change a different version could not read; the client refuses a server with another one.
pub const VERSION: u32 = 2;

const MAX_HEADER: u32 = 64 << 20;
const MAX_BODY: u32 = 1 << 30;

/// A path as it travels: a string with `/` separators (`C:/Users/x` for a Windows host); never a backslash, so the
/// app's `Path::join` / `file_name` / `parent` work on it whatever the app's OS. Non-UTF-8 names are lossy.
// shortcut: `\\server\share` (UNC) paths and non-UTF-8 names are not representable; upgrade when a user needs them.
pub fn wire_path(p: &Path) -> String {
    let s = p.to_string_lossy();
    if cfg!(windows) { s.replace('\\', "/") } else { s.into_owned() }
}

/// The native path for a wire path (on Windows, with its backslashes).
pub fn native_path(s: &str) -> PathBuf {
    if cfg!(windows) { PathBuf::from(s.replace('/', "\\")) } else { PathBuf::from(s) }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReqFrame {
    pub id: u64,
    pub req: Req,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ResFrame {
    pub id: u64,
    pub res: Res,
}

/// One request per `Host` method, plus `Cancel`. File content (`Write`) is the frame's body.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Req {
    Read { path: String },
    Write { path: String },
    Create { path: String, folder: bool },
    Rename { from: String, to: String },
    Copy { from: String, to: String },
    /// There is no Trash on the host: deletes for good.
    Trash { path: String },
    RemoveFile { path: String },
    Metadata { path: String },
    Exists { path: String },
    ListDir { dir: String, exclude: Vec<String> },
    Discover { root: String, max_depth: usize },
    Files { root: String, repos: Vec<String>, exclude: Vec<String> },
    Search { root: String, repos: Vec<String>, query: Query, limit: usize, exclude: Vec<String> },
    /// Answered by `Ok`, then `Event`s with this id.
    Watch { root: String },
    /// Answered by `Ok`, then `Ports` (the whole list) with this id when it changes, once at the start; until the connection ends.
    WatchPorts,
    /// A stream to `host:port` as the host sees it (a forwarded port): answered by `Ok` or `Err`; then the server
    /// sends `Data` and finally `Closed` with this id. `Cancel { target }` closes it.
    Open { host: String, port: u16 },
    /// Bytes (the body) for the stream `target`, not answered. Handled in order by the reader, like `Cancel`.
    Data { target: u64 },
    Git { repo: String, args: Vec<String>, stdin: Option<String>, envs: Vec<(String, String)> },
    /// Stops the `Search`, the `Watch` or the stream that was sent with id `target`. Not answered.
    Cancel { target: u64 },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Res {
    Hello { protocol: u32, os: String, arch: String },
    Ok,
    /// The content is the frame's body.
    Bytes,
    Path { path: String },
    Bool { value: bool },
    Meta(WireMeta),
    Entries { entries: Vec<(String, bool)> },
    Repos { repos: Vec<WireRepo> },
    Files { files: Vec<String> },
    Results(WireResults),
    Text { text: String },
    Event { path: String },
    Ports { ports: Vec<ListeningPort> },
    /// Bytes of a stream (the body).
    Data,
    /// The host's side of a stream ended.
    Closed,
    /// `kind` is an `io::ErrorKind` name (see `kind_name`), empty for the errors that are plain text.
    Err { kind: String, msg: String },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WireMeta {
    pub is_dir: bool,
    pub is_file: bool,
    pub size: u64,
    /// Seconds and nanoseconds since the Unix epoch.
    pub mtime: Option<(u64, u32)>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WireRepo {
    pub path: String,
    pub worktree_of: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WireResults {
    pub files: Vec<(String, Vec<LineMatch>)>,
    pub truncated: bool,
    pub error: Option<String>,
}

impl From<Results> for WireResults {
    fn from(r: Results) -> Self {
        WireResults { files: r.files.into_iter().map(|f| (wire_path(&f.path), f.lines)).collect(), truncated: r.truncated, error: r.error }
    }
}

impl From<WireResults> for Results {
    fn from(r: WireResults) -> Self {
        Results {
            files: r.files.into_iter().map(|(path, lines)| FileMatches { path: PathBuf::from(path), lines }).collect(),
            truncated: r.truncated,
            error: r.error,
        }
    }
}

const KINDS: &[(&str, io::ErrorKind)] = &[
    ("NotFound", io::ErrorKind::NotFound),
    ("AlreadyExists", io::ErrorKind::AlreadyExists),
    ("PermissionDenied", io::ErrorKind::PermissionDenied),
    ("InvalidData", io::ErrorKind::InvalidData),
    ("InvalidInput", io::ErrorKind::InvalidInput),
    ("NotADirectory", io::ErrorKind::NotADirectory),
    ("IsADirectory", io::ErrorKind::IsADirectory),
    ("DirectoryNotEmpty", io::ErrorKind::DirectoryNotEmpty),
];

pub fn kind_name(kind: io::ErrorKind) -> &'static str {
    KINDS.iter().find(|(_, k)| *k == kind).map_or("Other", |(n, _)| n)
}

pub fn kind_from_name(name: &str) -> io::ErrorKind {
    KINDS.iter().find(|(n, _)| *n == name).map_or(io::ErrorKind::Other, |(_, k)| *k)
}

/// Writes one frame (a single `write_all`, so concurrent writers under one lock never interleave).
pub fn write_frame<W: Write>(w: &mut W, header: &impl Serialize, body: &[u8]) -> io::Result<()> {
    let header = serde_json::to_vec(header).map_err(io::Error::other)?;
    let mut buf = Vec::with_capacity(8 + header.len() + body.len());
    buf.extend((header.len() as u32).to_be_bytes());
    buf.extend((body.len() as u32).to_be_bytes());
    buf.extend(&header);
    buf.extend(body);
    w.write_all(&buf)?;
    w.flush()
}

/// The next frame; `None` at a clean end of the stream.
pub fn read_frame<R: Read, T: DeserializeOwned>(r: &mut R) -> io::Result<Option<(T, Vec<u8>)>> {
    let mut lens = [0u8; 8];
    match r.read_exact(&mut lens) {
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        other => other?,
    }
    let (h, b) = (u32::from_be_bytes(lens[..4].try_into().unwrap()), u32::from_be_bytes(lens[4..].try_into().unwrap()));
    if h > MAX_HEADER || b > MAX_BODY {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too large"));
    }
    let mut header = vec![0; h as usize];
    r.read_exact(&mut header)?;
    let mut body = vec![0; b as usize];
    r.read_exact(&mut body)?;
    Ok(Some((serde_json::from_slice(&header).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?, body)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_round_trips_with_its_binary_body() {
        let mut buf = Vec::new();
        write_frame(&mut buf, &ReqFrame { id: 7, req: Req::Write { path: "/a b".into() } }, &[0, 255, 10]).unwrap();
        write_frame(&mut buf, &ReqFrame { id: 8, req: Req::Cancel { target: 7 } }, &[]).unwrap();
        let mut r = buf.as_slice();
        let (f, body): (ReqFrame, _) = read_frame(&mut r).unwrap().unwrap();
        assert!(f.id == 7 && matches!(f.req, Req::Write { ref path } if path == "/a b") && body == [0, 255, 10]);
        assert!(matches!(read_frame::<_, ReqFrame>(&mut r).unwrap().unwrap().0.req, Req::Cancel { target: 7 }));
        assert!(read_frame::<_, ReqFrame>(&mut r).unwrap().is_none());
        // the header is plain JSON a person can read in a captured stream
        assert!(String::from_utf8_lossy(&buf).contains(r#"{"id":7,"req":{"op":"write","path":"/a b"}}"#));
    }

    #[test]
    fn stream_frames_round_trip() {
        let mut buf = Vec::new();
        write_frame(&mut buf, &ReqFrame { id: 5, req: Req::Open { host: "localhost".into(), port: 3000 } }, &[]).unwrap();
        write_frame(&mut buf, &ReqFrame { id: 6, req: Req::Data { target: 5 } }, b"GET /").unwrap();
        write_frame(&mut buf, &ResFrame { id: 5, res: Res::Ports { ports: vec![ListeningPort { port: 3000, pid: Some(9), process: "node".into() }] } }, &[]).unwrap();
        write_frame(&mut buf, &ResFrame { id: 5, res: Res::Closed }, &[]).unwrap();
        let mut r = buf.as_slice();
        assert!(matches!(read_frame::<_, ReqFrame>(&mut r).unwrap().unwrap().0.req, Req::Open { port: 3000, .. }));
        let (f, body): (ReqFrame, _) = read_frame(&mut r).unwrap().unwrap();
        assert!(matches!(f.req, Req::Data { target: 5 }) && body == b"GET /");
        assert!(matches!(read_frame::<_, ResFrame>(&mut r).unwrap().unwrap().0.res, Res::Ports { ports } if ports[0].process == "node"));
        assert!(matches!(read_frame::<_, ResFrame>(&mut r).unwrap().unwrap().0.res, Res::Closed));
    }

    #[test]
    fn error_kinds_survive_the_wire() {
        for k in [io::ErrorKind::NotFound, io::ErrorKind::AlreadyExists, io::ErrorKind::PermissionDenied] {
            assert_eq!(kind_from_name(kind_name(k)), k);
        }
        assert_eq!(kind_from_name(kind_name(io::ErrorKind::TimedOut)), io::ErrorKind::Other);
    }
}
