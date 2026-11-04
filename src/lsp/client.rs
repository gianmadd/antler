//! JSON-RPC transport. Only the worker threads touch the process pipes.
use crate::host::Process;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read, Write},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
};

type Reply = Result<Value, String>;
type Pending = Arc<Mutex<HashMap<u64, mpsc::Sender<Reply>>>>;
type Kill = Arc<Mutex<Box<dyn FnMut() + Send>>>;

pub struct Init {
    pub root: PathBuf,
    pub python: String,
}

pub struct Client {
    writer: mpsc::Sender<Value>,
    pending: Pending,
    next_id: AtomicU64,
    alive: Arc<AtomicBool>,
    kill: Kill,
}

impl Client {
    /// Blocking initialization; call from a background thread.
    pub fn start(
        process: Process,
        init: Init,
        log: PathBuf,
        on_exit: Box<dyn FnOnce() + Send>,
    ) -> Result<Arc<Self>, String> {
        let Process {
            mut stdin,
            stdout,
            mut stderr,
            kill,
        } = process;
        let kill = Arc::new(Mutex::new(kill));
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let alive = Arc::new(AtomicBool::new(true));
        let (writer, incoming) = mpsc::channel::<Value>();
        let client = Arc::new(Self {
            writer: writer.clone(),
            pending: pending.clone(),
            next_id: AtomicU64::new(1),
            alive: alive.clone(),
            kill: kill.clone(),
        });
        let stderr_log = log.clone();
        std::thread::spawn(move || {
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(stderr_log)
            {
                let _ = std::io::copy(&mut stderr, &mut file);
            }
        });
        let writer_alive = alive.clone();
        let writer_kill = kill.clone();
        std::thread::spawn(move || {
            while let Ok(value) = incoming.recv() {
                if !writer_alive.load(Ordering::Acquire) {
                    break;
                }
                let bytes = value.to_string();
                if write!(stdin, "Content-Length: {}\r\n\r\n{}", bytes.len(), bytes)
                    .and_then(|_| stdin.flush())
                    .is_err()
                {
                    (writer_kill.lock().unwrap())();
                    break;
                }
            }
        });
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            while let Ok(value) = read_frame(&mut reader) {
                if let Some(method) = value.get("method").and_then(Value::as_str) {
                    if let Some(id) = value.get("id") {
                        let response = if method == "workspace/configuration" {
                            let result: Vec<Value> = value
                                .pointer("/params/items")
                                .and_then(Value::as_array)
                                .into_iter()
                                .flatten()
                                .map(|item| match item.get("section").and_then(Value::as_str) {
                                    Some("python") => json!({"pythonPath": init.python}),
                                    Some("python.pythonPath") => json!(init.python),
                                    _ => json!({}),
                                })
                                .collect();
                            json!({"jsonrpc":"2.0", "id":id, "result":result})
                        } else {
                            json!({"jsonrpc":"2.0", "id":id, "error":{"code":-32601,"message":"Method not found"}})
                        };
                        let _ = writer.send(response);
                    } else if let Ok(mut file) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&log)
                    {
                        let _ = writeln!(file, "{value}");
                    }
                } else if let Some(id) = value.get("id").and_then(Value::as_u64) {
                    if let Some(reply) = pending.lock().unwrap().remove(&id) {
                        let result = match value.get("error") {
                            Some(error) => Err(error.to_string()),
                            None => Ok(value.get("result").cloned().unwrap_or(Value::Null)),
                        };
                        let _ = reply.send(result);
                    }
                }
            }
            alive.store(false, Ordering::Release);
            for (_, reply) in pending.lock().unwrap().drain() {
                let _ = reply.send(Err("Language server exited".into()));
            }
            (kill.lock().unwrap())();
            on_exit();
        });
        let root_uri = url::Url::from_directory_path(&init.root)
            .map_err(|_| "Invalid source folder")?
            .to_string();
        let result = client.request("initialize", json!({
            "processId": std::process::id(), "rootUri":root_uri,
            "workspaceFolders":[{"uri":root_uri,"name":init.root.file_name().unwrap_or_default().to_string_lossy()}],
            "capabilities": {
                "workspace":{"configuration":true,"workspaceFolders":true},
                "textDocument":{
                    "synchronization":{"dynamicRegistration":false,"didSave":false},
                    "definition":{"linkSupport":true},"typeDefinition":{"linkSupport":true},"implementation":{"linkSupport":true},
                    "references":{},"hover":{"contentFormat":["markdown","plaintext"]}
                },
                "general":{"positionEncodings":["utf-16"]}
            }
        }));
        if let Err(error) = result {
            (client.kill.lock().unwrap())();
            return Err(error);
        }
        client.notify("initialized", json!({}));
        Ok(client)
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    pub fn notify(&self, method: &str, params: Value) {
        let _ = self
            .writer
            .send(json!({"jsonrpc":"2.0","method":method,"params":params}));
    }

    pub fn request(&self, method: &str, params: Value) -> Reply {
        if !self.is_alive() {
            return Err("Language server exited".into());
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (send, receive) = mpsc::channel();
        self.pending.lock().unwrap().insert(id, send);
        if self
            .writer
            .send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .is_err()
        {
            self.pending.lock().unwrap().remove(&id);
            return Err("Language server writer stopped".into());
        }
        let result = receive
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| "Language server request timed out".to_string());
        self.pending.lock().unwrap().remove(&id);
        result?
    }

    /// The UI never waits for shutdown; kill bounds even a hung shutdown request.
    pub fn shutdown(self: &Arc<Self>) {
        let client = self.clone();
        std::thread::spawn(move || {
            let _ = client.request("shutdown", Value::Null);
            client.notify("exit", Value::Null);
        });
        // Keep the client alive until the queued exit has reached the writer.
        let client = self.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(2));
            (client.kill.lock().unwrap())();
        });
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        (self.kill.lock().unwrap())();
    }
}

fn read_frame(reader: &mut impl BufRead) -> Result<Value, String> {
    let mut length = None;
    let mut total = 0;
    loop {
        let mut line = String::new();
        let count = reader
            .take(8193)
            .read_line(&mut line)
            .map_err(|e| e.to_string())?;
        total += count;
        if count == 0 || total > 8192 {
            return Err("Invalid LSP header".into());
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("Content-Length") {
                length = Some(value.trim().parse::<usize>().map_err(|e| e.to_string())?);
            }
        }
    }
    let length = length
        .filter(|n| *n <= 64 * 1024 * 1024)
        .ok_or("Invalid LSP Content-Length")?;
    let mut body = vec![0; length];
    reader.read_exact(&mut body).map_err(|e| e.to_string())?;
    serde_json::from_slice(&body).map_err(|e| e.to_string())
}
