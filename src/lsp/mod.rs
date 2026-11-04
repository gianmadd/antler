mod client;
use crate::host::Host;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
static NEXT_LOG: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn source_folder(
    root: &Path,
    file: &Path,
    markers: &[&str],
    exists: impl Fn(&Path) -> bool,
) -> PathBuf {
    let mut folder = file.parent().unwrap_or(root);
    loop {
        if !folder.starts_with(root) {
            return root.to_path_buf();
        }
        if markers.iter().any(|n| exists(&folder.join(n))) {
            return folder.to_path_buf();
        }
        if folder == root || !folder.starts_with(root) {
            return root.to_path_buf();
        }
        folder = folder.parent().unwrap_or(root);
    }
}
pub fn interpreter(folder: &Path, exists: impl Fn(&Path) -> bool) -> String {
    let bin = if cfg!(windows) {
        "Scripts/python.exe"
    } else {
        "bin/python"
    };
    [".venv", "venv"]
        .iter()
        .map(|v| folder.join(v).join(bin))
        .find(|p| exists(p))
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "python".into())
}
#[derive(Clone, Default)]
pub struct LspSettings {
    pub python_off: bool,
    pub python_path: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ServerKey {
    pub language: &'static str,
    pub folder: PathBuf,
}
#[derive(Clone, Copy)]
pub struct ServerDef {
    pub language: &'static str,
    pub extensions: &'static [&'static str],
    pub programs: &'static [&'static str],
    pub args: &'static [&'static str],
    pub markers: &'static [&'static str],
}
pub const SERVERS: &[ServerDef] = &[ServerDef {
    language: "python",
    extensions: &["py", "pyi"],
    programs: &["basedpyright-langserver", "pyright-langserver"],
    args: &["--stdio"],
    markers: &["pyproject.toml", "setup.py", ".venv"],
}];
pub fn language(path: &Path) -> Option<&'static ServerDef> {
    let ext = path.extension()?.to_str()?;
    SERVERS
        .iter()
        .find(|server| server.extensions.contains(&ext))
}
#[derive(Debug)]
pub enum Event {
    NotFound { language: &'static str },
    Stopped { language: &'static str, folder: PathBuf, log: PathBuf },
}
pub fn should_restart(exits: &mut Vec<Instant>, now: Instant) -> bool {
    exits.retain(|exit| now.saturating_duration_since(*exit) < Duration::from_secs(180));
    exits.push(now);
    exits.len() < 5
}
struct Server {
    client: Arc<client::Client>,
    opened: HashSet<PathBuf>,
    log: PathBuf,
    exits: Vec<Instant>,
}
struct Starting {
    result: std::sync::mpsc::Receiver<(Option<Server>, bool)>,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
}
pub struct Servers {
    host: Arc<dyn Host>,
    root: PathBuf,
    settings: LspSettings,
    docs: HashMap<PathBuf, (String, ServerKey, i32)>,
    servers: HashMap<ServerKey, Server>,
    starting: HashMap<ServerKey, Starting>,
    notified: HashSet<&'static str>,
    events: Vec<Event>,
    exited: Arc<std::sync::Mutex<Vec<ServerKey>>>,
    crash_exits: HashMap<ServerKey, Vec<Instant>>,
    stopped: HashSet<ServerKey>,
    logs: HashMap<ServerKey, PathBuf>,
    clock: Box<dyn Fn() -> Instant + Send>,
}
impl Servers {
    pub fn new(host: Arc<dyn Host>, root: PathBuf, settings: LspSettings) -> Self {
        Self {
            host,
            root,
            settings,
            docs: HashMap::new(),
            servers: HashMap::new(),
            starting: HashMap::new(),
            notified: HashSet::new(),
            events: vec![],
            exited: Arc::new(std::sync::Mutex::new(vec![])),
            crash_exits: HashMap::new(),
            stopped: HashSet::new(),
            logs: HashMap::new(),
            clock: Box::new(Instant::now),
        }
    }
    pub fn set_settings(&mut self, s: LspSettings) {
        let disabling = !self.settings.python_off && s.python_off;
        let server_changed =
            self.settings.python_off != s.python_off || self.settings.python_path != s.python_path;
        if server_changed {
            self.notified.clear();
        }
        self.settings = s;
        if disabling || (server_changed && !self.settings.python_off) {
            self.stop();
        }
    }
    pub fn sync<'a>(&mut self, open: impl Iterator<Item = (&'a Path, &'a str)>) {
        if self.settings.python_off || !self.host.supports_processes() {
            return;
        }
        let ready = self.starting.keys().cloned().collect::<Vec<_>>();
        for key in ready {
            let result = self
                .starting
                .get(&key)
                .and_then(|s| s.result.try_recv().ok());
            if let Some((server, missing)) = result {
                self.starting.remove(&key);
                if let Some(server) = server {
                    self.servers.insert(key.clone(), server);
                } else if missing && self.notified.insert("python") {
                    self.events.push(Event::NotFound { language: "python" });
                }
            }
        }
        self.handle_exits();
        let mut now = HashMap::new();
        for (path, text) in open
            .filter(|(p, _)| matches!(p.extension().and_then(|s| s.to_str()), Some("py" | "pyi")))
        {
            let Some(def) = language(path) else { continue };
            let key = ServerKey {
                language: def.language,
                folder: self
                    .docs
                    .get(path)
                    .map(|(_, key, _)| key.folder.clone())
                    .unwrap_or_else(|| {
                        source_folder(&self.root, path, def.markers, |p| self.host.exists(p))
                    }),
            };
            if !self.servers.contains_key(&key)
                && !self.starting.contains_key(&key)
                && !self.stopped.contains(&key)
                && !self.notified.contains("python")
            {
                self.start(&key);
            }
            now.insert(path.to_path_buf(), (text.to_string(), key, 0));
        }
        let gone = self
            .docs
            .keys()
            .filter(|p| !now.contains_key(*p))
            .cloned()
            .collect::<Vec<_>>();
        let active_keys = now
            .values()
            .map(|(_, key, _)| key.clone())
            .collect::<HashSet<_>>();
        let unused = self
            .starting
            .keys()
            .filter(|key| !active_keys.contains(*key))
            .cloned()
            .collect::<Vec<_>>();
        for key in unused {
            if let Some(starting) = self.starting.remove(&key) {
                starting
                    .cancelled
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
        for p in gone {
            if let Some((_, k, _)) = self.docs.remove(&p) {
                if let Some(s) = self.servers.get_mut(&k) {
                    if s.opened.remove(&p) {
                        s.client.notify(
                            "textDocument/didClose",
                            serde_json::json!({"textDocument":{"uri":uri(&p)}}),
                        );
                    }
                }
            }
        }
        for (p, (text, k, v)) in &mut now {
            let changed = self.docs.get(p).is_some_and(|(old, _, _)| old != text);
            let version = self
                .docs
                .get(p)
                .map_or(1, |(_, _, v)| v + i32::from(changed));
            if let Some(s) = self.servers.get_mut(k) {
                if !s.opened.contains(p) {
                    s.client.notify("textDocument/didOpen",serde_json::json!({"textDocument":{"uri":uri(p),"languageId":"python","version":version,"text":text}}));
                    s.opened.insert(p.clone());
                } else if changed {
                    s.client.notify("textDocument/didChange",serde_json::json!({"textDocument":{"uri":uri(p),"version":version},"contentChanges":[{"text":text}]}));
                }
            }
            *v = version;
        }
        self.docs = now;
    }
    fn start(&mut self, k: &ServerKey) {
        let (tx, rx) = std::sync::mpsc::channel();
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.starting.insert(
            k.clone(),
            Starting {
                result: rx,
                cancelled: cancelled.clone(),
            },
        );
        let key = k.clone();
        let host = self.host.clone();
        let custom = self.settings.python_path.clone();
        let definition = SERVERS
            .iter()
            .find(|def| def.language == k.language)
            .copied()
            .unwrap();
        let exited = self.exited.clone();
        let exit_key = k.clone();
        let exits = self.crash_exits.get(k).cloned().unwrap_or_default();
        let log = crate::paths::cache().join("logs").join(format!(
            "python-{}-{}.log",
            std::process::id(),
            NEXT_LOG.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        self.logs.insert(k.clone(), log.clone());
        let _ = std::fs::create_dir_all(log.parent().unwrap());
        let _ = std::fs::OpenOptions::new().create(true).append(true).open(&log);
        std::thread::spawn(move || {
            let names = custom
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(|s| vec![s])
                .unwrap_or_else(|| definition.programs.to_vec());
            let mut process = None;
            let mut not_found = true;
            for name in names {
                if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }
                match host.spawn(name, definition.args, &key.folder) {
                    Ok(p) => {
                        process = Some(p);
                        break;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Unsupported => not_found = false,
                    _ => {}
                }
            }
            let Some(mut process) = process else {
                let _ = tx.send((None, not_found));
                return;
            };
            if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                (process.kill)();
                return;
            }
            let python = interpreter(&key.folder, |p| host.exists(p));
            let result = client::Client::start(
                process,
                client::Init {
                    root: key.folder.clone(),
                    python,
                },
                log.clone(),
                cancelled.clone(),
                Box::new(move || exited.lock().unwrap().push(exit_key)),
            )
            .ok()
            .map(|client| Server {
                client,
                opened: HashSet::new(),
                log,
                exits,
            });
            if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                if let Some(server) = result {
                    server.client.shutdown();
                }
                return;
            }
            if let Err(std::sync::mpsc::SendError((Some(server), _))) = tx.send((result, false)) {
                server.client.shutdown();
            }
        });
    }
    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }
    fn handle_exits(&mut self) {
        let keys = std::mem::take(&mut *self.exited.lock().unwrap());
        for key in keys {
            let server = self.servers.remove(&key);
            if server.is_none() && self.starting.contains_key(&key) {
                self.exited.lock().unwrap().push(key);
                continue;
            }
            let mut exits = server
                .as_ref()
                .map(|server| server.exits.clone())
                .or_else(|| self.crash_exits.get(&key).cloned())
                .unwrap_or_default();
            let restart = should_restart(&mut exits, (self.clock)());
            if restart {
                self.crash_exits.insert(key.clone(), exits);
                self.start(&key);
            } else {
                self.stopped.insert(key.clone());
                let log = server.map(|server| server.log).or_else(|| self.logs.get(&key).cloned());
                let Some(log) = log else { continue };
                self.events.push(Event::Stopped {
                    language: key.language,
                    folder: key.folder,
                    log,
                });
            }
        }
    }
    pub fn restart(&mut self) {
        self.stop_preserving_documents();
        self.notified.clear();
        self.crash_exits.clear();
        self.stopped.clear();
    }
    #[cfg(test)]
    pub fn set_clock(&mut self, clock: Box<dyn Fn() -> Instant + Send>) {
        self.clock = clock;
    }
    pub fn shutdown(&mut self) {
        self.stop();
    }
    pub fn begin_shutdown(&mut self) -> Vec<Arc<client::Client>> {
        for (_, starting) in self.starting.drain() {
            starting.cancelled.store(true, std::sync::atomic::Ordering::Release);
        }
        let clients = self.servers.drain().map(|(_, server)| server.client).collect();
        self.docs.clear();
        clients
    }
    pub fn shutdown_now(&mut self) {
        for starting in self.starting.values() {
            starting.cancelled.store(true, std::sync::atomic::Ordering::Release);
        }
        self.starting.clear();
        for (_, server) in self.servers.drain() {
            server.client.kill_now();
        }
        self.docs.clear();
    }
    fn stop(&mut self) {
        self.stop_preserving_documents();
        self.docs.clear();
        self.crash_exits.clear();
        self.stopped.clear();
    }
    fn stop_preserving_documents(&mut self) {
        for starting in self.starting.values() {
            starting
                .cancelled
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.starting.clear();
        for (_, s) in self.servers.drain() {
            s.client.shutdown();
        }
        self.servers.clear();
    }
}
fn uri(p: &Path) -> String {
    url::Url::from_file_path(p)
        .map(|url| url.to_string())
        .unwrap_or_default()
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub path: PathBuf,
    pub line: usize,
    pub column: usize,
    pub end_line: usize,
    pub end_column: usize,
}
#[derive(Clone)]
pub struct Handle {
    client: Arc<client::Client>,
    pub path: PathBuf,
    pub host: Arc<dyn Host>,
}
impl Handle {
    pub fn request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        self.client.request(method, params)
    }
}
impl Servers {
    pub fn handle(&self, path: &Path) -> Option<Handle> {
        let (_, key, _) = self.docs.get(path)?;
        let client = self.servers.get(key)?.client.clone();
        (client.is_alive()
            && self
                .servers
                .get(key)
                .is_some_and(|s| s.opened.contains(path)))
        .then(|| Handle {
            client,
            path: path.to_path_buf(),
            host: self.host.clone(),
        })
    }
}
#[cfg(all(test, unix))]
mod tests;

#[cfg(test)]
mod pure_tests {
    use super::*;
    #[test]
    fn source_folder_uses_nearest_marker_or_root() {
        let root = Path::new("/r");
        let markers = ["pyproject.toml", "setup.py", ".venv"];
        assert_eq!(
            source_folder(root, Path::new("/r/a/b/x.py"), &markers, |p| p
                == Path::new("/r/a/pyproject.toml")),
            Path::new("/r/a")
        );
        assert_eq!(
            source_folder(root, Path::new("/r/x.py"), &markers, |_| false),
            root
        );
        assert_eq!(
            source_folder(root, Path::new("/outside/x.py"), &markers, |_| true),
            root
        );
    }
    #[test]
    fn interpreter_uses_venv_then_python_path() {
        assert_eq!(
            interpreter(Path::new("/r"), |p| p.ends_with("venv/bin/python")),
            "/r/venv/bin/python"
        );
        assert_eq!(interpreter(Path::new("/r"), |_| false), "python");
    }

    #[test]
    fn restart_limit_allows_five_starts_in_three_minutes() {
        let start = std::time::Instant::now();
        let mut exits = vec![];
        for _ in 0..4 {
            assert!(should_restart(&mut exits, start));
        }
        assert!(!should_restart(&mut exits, start));
        assert!(should_restart(&mut exits, start + std::time::Duration::from_secs(180)));
    }
}
