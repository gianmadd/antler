use super::*;
use std::{fs, os::unix::fs::PermissionsExt, thread, time::Duration};
use tempfile::tempdir;

fn server_script(path: &Path) -> PathBuf {
    let script = path.join("standin.py");
    let body = format!(
        r##"#!/usr/bin/env python3
import json, os, sys
log = {log:?}
def send(value):
    body = json.dumps(value).encode(); sys.stdout.buffer.write(f"Content-Length: {{len(body)}}\r\n\r\n".encode() + body); sys.stdout.buffer.flush()
while True:
    headers = {{}}
    while True:
        line = sys.stdin.buffer.readline()
        if not line: sys.exit(0)
        if line in (b"\r\n", b"\n"): break
        name, value = line.decode().split(":", 1); headers[name.lower()] = value.strip()
    msg = json.loads(sys.stdin.buffer.read(int(headers["content-length"])))
    with open(log, "a") as f: f.write(json.dumps({{"cwd": os.getcwd(), "msg": msg}}) + "\n")
    method = msg.get("method")
    if method == "initialize": send({{"jsonrpc":"2.0", "id":msg["id"], "result":{{}}}})
    elif method == "initialized":
        send({{"jsonrpc":"2.0", "id":0, "method":"workspace/configuration", "params":{{"items":[{{"section":"python"}},{{"section":"basedpyright"}}]}}}})
        send({{"jsonrpc":"2.0", "id":999, "method":"standin/unknown", "params":{{}}}})
    elif method == "textDocument/references": send({{"jsonrpc":"2.0", "id":msg["id"], "result":[{{"uri":"file://" + os.getcwd() + "/b.py", "range":{{"start":{{"line":1,"character":2}},"end":{{"line":1,"character":5}}}}}},{{"uri":"file://" + os.getcwd() + "/a.py", "range":{{"start":{{"line":0,"character":0}},"end":{{"line":0,"character":3}}}}}},{{"uri":"file://" + os.getcwd() + "/b.py", "range":{{"start":{{"line":0,"character":1}},"end":{{"line":0,"character":4}}}}}}]}})
    elif method in ("textDocument/definition", "textDocument/typeDefinition", "textDocument/implementation"):
        line = msg["params"]["position"]["line"]
        uri = msg["params"]["textDocument"]["uri"]
        rng = {{"start":{{"line":0,"character":0}},"end":{{"line":0,"character":1}}}}
        loc = {{"uri":uri,"range":rng}}
        link = {{"targetUri":uri,"targetRange":rng,"targetSelectionRange":rng}}
        result = loc if line == 0 else [loc,loc] if line == 1 else [link,link] if line == 2 else None
        send({{"jsonrpc":"2.0", "id":msg["id"], "result":result}})
    elif method == "textDocument/hover":
        result = {{"contents":{{"kind":"markdown", "value":"```python\ndef f(x: int) -> str: ...\n```\nDocs"}}, "range":{{"start":{{"line":0,"character":4}},"end":{{"line":0,"character":6}}}}}} if msg["params"]["position"]["line"] == 0 else None
        send({{"jsonrpc":"2.0", "id":msg["id"], "result":result}})
    elif method == "shutdown": send({{"jsonrpc":"2.0", "id":msg["id"], "result":None}})
    elif method == "exit": sys.exit(0)
"##,
        log = path.join("trace.jsonl").display().to_string()
    );
    fs::write(&script, body).unwrap();
    let mut permissions = fs::metadata(&script).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&script, permissions).unwrap();
    script
}

fn wait_for(server: &mut Servers, open: &[(PathBuf, String)]) {
    for _ in 0..150 {
        server.sync(open.iter().map(|(p, t)| (p.as_path(), t.as_str())));
        if open.iter().all(|(p, _)| server.handle(p).is_some()) {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!("stand-in language servers did not initialize");
}
fn trace(dir: &Path) -> Vec<serde_json::Value> {
    let text = fs::read_to_string(dir.join("trace.jsonl")).unwrap_or_default();
    text.lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}
fn wait_trace_file(path: &Path, expected: impl Fn(&[serde_json::Value]) -> bool) -> Vec<serde_json::Value> {
    for _ in 0..150 {
        let messages = fs::read_to_string(path).unwrap_or_default().lines()
            .filter_map(|line| serde_json::from_str(line).ok()).collect::<Vec<_>>();
        if expected(&messages) { return messages; }
        thread::sleep(Duration::from_millis(20));
    }
    panic!("stand-in trace did not reach expected state: {}", fs::read_to_string(path).unwrap_or_default());
}
fn wait_trace(
    dir: &Path,
    expected: impl Fn(&[serde_json::Value]) -> bool,
) -> Vec<serde_json::Value> {
    for _ in 0..150 {
        let messages = trace(dir);
        if expected(&messages) {
            return messages;
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!(
        "stand-in trace did not reach the expected state: {:?}",
        trace(dir)
    )
}

#[test]
fn jump_maps_every_lsp_location_shape_and_uses_utf16_columns() {
    let temp = tempdir().unwrap();
    let root = temp.path();
    let program = server_script(root);
    let source = root.join("source.py");
    let text = "first\né😀value\nthird\nfourth".to_string();
    let open = vec![(source.clone(), text.clone())];
    let mut servers = Servers::new(crate::host::local(), root.to_path_buf(), LspSettings { python_path: Some(program.to_string_lossy().into_owned()), ..Default::default() });
    wait_for(&mut servers, &open);
    let handle = servers.handle(&source).unwrap();
    let one = handle.jump(Jump::Definition, &text, 0, 0);
    assert_eq!(one.len(), 1);
    let many = handle.jump(Jump::TypeDefinition, &text, 1, 2);
    assert_eq!(many.len(), 2);
    let links = handle.jump(Jump::Implementation, &text, 2, 0);
    assert_eq!(links.len(), 2);
    assert!(handle.jump(Jump::Definition, &text, 3, 0).is_empty());
    let requests = wait_trace(root, |messages| messages.iter().filter(|v| v.pointer("/msg/method").and_then(|m| m.as_str()).is_some_and(|m| m.starts_with("textDocument/"))).count() >= 4);
    assert!(requests.iter().any(|v| v.pointer("/msg/params/position/character").and_then(|v| v.as_u64()) == Some(3)));
}

#[test]
fn server_restart_replays_documents_before_navigation_requests() {
    let temp = tempdir().unwrap();
    let root = temp.path();
    let script = root.join("crash-once.py");
    let count = root.join("starts");
    let trace_path = root.join("restart-trace.jsonl");
    let body = format!(r##"#!/usr/bin/env python3
import json, os, sys
count = {count:?}; trace = {trace:?}
def send(value):
    body = json.dumps(value).encode(); sys.stdout.buffer.write(f"Content-Length: {{len(body)}}\r\n\r\n".encode() + body); sys.stdout.buffer.flush()
n = int(open(count).read()) + 1 if os.path.exists(count) else 1
open(count, "w").write(str(n))
while True:
    headers = {{}}
    while True:
        line = sys.stdin.buffer.readline()
        if not line: sys.exit(0)
        if line in (b"\r\n", b"\n"): break
        name, value = line.decode().split(":", 1); headers[name.lower()] = value.strip()
    msg = json.loads(sys.stdin.buffer.read(int(headers["content-length"])))
    with open(trace, "a") as f: f.write(json.dumps({{"pid": os.getpid(), "msg": msg}}) + "\n")
    method = msg.get("method")
    if method == "initialize":
        if n == 1: sys.exit(1)
        send({{"jsonrpc":"2.0", "id":msg["id"], "result":{{}}}})
    elif method == "initialized":
        send({{"jsonrpc":"2.0", "id":0, "method":"workspace/configuration", "params":{{"items":[{{"section":"python"}}]}}}})
    elif method == "shutdown": send({{"jsonrpc":"2.0", "id":msg["id"], "result":None}})
    elif method == "textDocument/definition": send({{"jsonrpc":"2.0", "id":msg["id"], "result":None}})
    elif method == "exit": sys.exit(0)
"##, count=count.display().to_string(), trace=trace_path.display().to_string());
    fs::write(&script, body).unwrap();
    let mut permissions = fs::metadata(&script).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&script, permissions).unwrap();
    let path = root.join("x.py");
    let open = [(path.clone(), "print(1)".to_string())];
    let mut servers = Servers::new(crate::host::local(), root.to_path_buf(), LspSettings {
        python_off: false,
        python_path: Some(script.to_string_lossy().into_owned()),
    });
    wait_for(&mut servers, &open);
    let _ = wait_trace_file(&trace_path, |messages| messages.iter().any(|m|
        m.pointer("/msg/method").and_then(|v| v.as_str()) == Some("textDocument/didOpen")));
    let handle = servers.handle(&path).unwrap();
    handle.request("textDocument/definition", serde_json::json!({"textDocument":{"uri":uri(&path)},"position":{"line":0,"character":0}})).unwrap();
    let messages = wait_trace_file(&trace_path, |messages| messages.iter().any(|m|
        m.pointer("/msg/method").and_then(|v| v.as_str()) == Some("textDocument/definition")));
    let pid = messages.iter().find(|m| m.pointer("/msg/method").and_then(|v| v.as_str()) == Some("textDocument/definition")).unwrap()["pid"].as_u64().unwrap();
    let restarted: Vec<_> = messages.iter().filter(|m| m["pid"].as_u64() == Some(pid)).collect();
    let opened = restarted.iter().position(|m| m.pointer("/msg/method").and_then(|v| v.as_str()) == Some("textDocument/didOpen")).unwrap();
    let requested = restarted.iter().position(|m| m.pointer("/msg/method").and_then(|v| v.as_str()) == Some("textDocument/definition")).unwrap();
    assert!(opened < requested, "restart must replay open documents before navigation requests");
    assert_eq!(fs::read_to_string(&count).unwrap().trim(), "2");
    servers.shutdown();
}

#[test]
fn hover_returns_markdown_and_none_when_server_has_no_answer() {
    let temp = tempdir().unwrap();
    let root = temp.path();
    let program = server_script(root);
    let source = root.join("source.py");
    let text = "aé😀z\nmissing".to_string();
    let open = vec![(source.clone(), text.clone())];
    let mut servers = Servers::new(crate::host::local(), root.to_path_buf(), LspSettings { python_path: Some(program.to_string_lossy().into_owned()), ..Default::default() });
    wait_for(&mut servers, &open);
    let handle = servers.handle(&source).unwrap();
    let hover = handle.hover(&text, 0, 3).unwrap();
    assert_eq!(hover.contents, lsp_types::HoverContents::Markup(lsp_types::MarkupContent {
        kind: lsp_types::MarkupKind::Markdown,
        value: "```python\ndef f(x: int) -> str: ...\n```\nDocs".into(),
    }));
    assert_eq!(hover.range.unwrap().start.character, 4);
    assert!(handle.hover(&text, 1, 0).is_none());
}

#[test]
fn crashing_server_stops_after_five_starts_and_restart_clears_the_state() {
    let temp = tempdir().unwrap();
    let root = temp.path();
    let count = root.join("starts");
    let script = root.join("crash.sh");
    fs::write(
        &script,
        format!("#!/bin/sh\nn=$(cat '{}' 2>/dev/null || echo 0); echo $((n+1)) > '{}'; exit 1\n", count.display(), count.display()),
    )
    .unwrap();
    let mut permissions = fs::metadata(&script).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&script, permissions).unwrap();
    let path = root.join("x.py");
    let open = [(path.clone(), "print(1)".to_string())];
    let mut servers = Servers::new(
        crate::host::local(),
        root.to_path_buf(),
        LspSettings { python_off: false, python_path: Some(script.to_string_lossy().into_owned()) },
    );
    let now = Instant::now();
    servers.set_clock(Box::new(move || now));
    let mut events = vec![];
    for _ in 0..200 {
        servers.sync(open.iter().map(|(p, t)| (p.as_path(), t.as_str())));
        events.extend(servers.take_events());
        if events.iter().any(|event| matches!(event, Event::Stopped { .. })) { break; }
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(fs::read_to_string(&count).unwrap().trim(), "5");
    assert!(events.iter().any(|event| matches!(event, Event::Stopped { language: "python", folder, log } if folder == root && log.extension().is_some_and(|ext| ext == "log"))), "events: {events:?}");
    servers.sync(open.iter().map(|(p, t)| (p.as_path(), t.as_str())));
    thread::sleep(Duration::from_millis(80));
    assert_eq!(fs::read_to_string(&count).unwrap().trim(), "5", "stopped server must stay off");
    servers.restart();
    for _ in 0..100 {
        servers.sync(open.iter().map(|(p, t)| (p.as_path(), t.as_str())));
        if fs::read_to_string(&count).is_ok_and(|n| n.trim() == "6") { break; }
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(fs::read_to_string(&count).unwrap().trim(), "6");
    servers.shutdown();
}

#[test]
fn cancelling_a_hung_initialize_kills_the_process_tree_promptly() {
    let temp = tempdir().unwrap();
    let root = temp.path();
    let pid_file = root.join("pid");
    let script = root.join("hang.sh");
    fs::write(&script, format!("#!/bin/sh\necho $$ > '{}'\nsleep 30\n", pid_file.display())).unwrap();
    let mut permissions = fs::metadata(&script).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&script, permissions).unwrap();
    let path = root.join("x.py");
    let mut servers = Servers::new(crate::host::local(), root.to_path_buf(), LspSettings {
        python_off: false,
        python_path: Some(script.to_string_lossy().into_owned()),
    });
    servers.sync(std::iter::once((path.as_path(), "x")));
    for _ in 0..100 {
        if pid_file.exists() { break; }
        thread::sleep(Duration::from_millis(10));
    }
    let pid = fs::read_to_string(&pid_file).unwrap();
    let started = Instant::now();
    servers.sync(std::iter::empty());
    for _ in 0..100 {
        let alive = std::process::Command::new("kill").args(["-0", pid.trim()]).stderr(std::process::Stdio::null()).status().unwrap().success();
        if !alive { break; }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(started.elapsed() < Duration::from_secs(2), "cancellation should not wait for the LSP request timeout");
    assert!(!std::process::Command::new("kill").args(["-0", pid.trim()]).stderr(std::process::Stdio::null()).status().unwrap().success());
    servers.shutdown();
}

#[test]
fn servers_follow_open_documents_and_answer_server_requests() {
    let temp = tempdir().unwrap();
    let root = temp.path();
    let nested = root.join("pkg");
    fs::create_dir_all(&nested).unwrap();
    fs::write(root.join("pyproject.toml"), "").unwrap();
    fs::write(nested.join("setup.py"), "").unwrap();
    let program = server_script(root);
    let mut servers = Servers::new(
        crate::host::local(),
        root.to_path_buf(),
        LspSettings {
            python_off: false,
            python_path: Some(program.to_string_lossy().into_owned()),
        },
    );
    let a = root.join("a.py");
    let b = root.join("b.py");
    let c = nested.join("c.py");
    let open = vec![
        (a.clone(), "one".into()),
        (a.clone(), "one".into()),
        (b.clone(), "two".into()),
        (c.clone(), "three".into()),
    ];
    wait_for(&mut servers, &open);
    let messages = wait_trace(root, |messages| {
        messages
            .iter()
            .filter(|v| {
                v.pointer("/msg/method").and_then(|m| m.as_str()) == Some("textDocument/didOpen")
            })
            .count()
            == 3
    });
    let opened = messages
        .iter()
        .filter(|v| {
            v.pointer("/msg/method").and_then(|m| m.as_str()) == Some("textDocument/didOpen")
        })
        .count();
    assert_eq!(
        opened, 3,
        "the duplicated file path opens once, and each source folder has its own server"
    );
    assert_eq!(
        messages
            .iter()
            .filter(|v| v.pointer("/msg/method").and_then(|m| m.as_str()) == Some("initialize"))
            .count(),
        2
    );
    assert!(messages.iter().any(
        |v| v.pointer("/msg/id").and_then(|x| x.as_i64()) == Some(999)
            && v.pointer("/msg/error/code").and_then(|x| x.as_i64()) == Some(-32601)
    ));
    assert!(messages.iter().any(|v| {
        v.pointer("/msg/id").and_then(|x| x.as_i64()) == Some(0)
            && v.pointer("/msg/result/0/pythonPath")
                .and_then(|x| x.as_str())
                == Some("python")
    }));

    let edited: Vec<(PathBuf, String)> = vec![
        (a.clone(), "updated".into()),
        (a.clone(), "updated".into()),
        (b.clone(), "two".into()),
        (c.clone(), "three".into()),
    ];
    servers.sync(edited.iter().map(|(p, t)| (p.as_path(), t.as_str())));
    let messages = wait_trace(root, |messages| {
        messages.iter().any(|v| {
            v.pointer("/msg/method").and_then(|m| m.as_str()) == Some("textDocument/didChange")
        })
    });
    let changes = messages
        .iter()
        .filter(|v| {
            v.pointer("/msg/method").and_then(|m| m.as_str()) == Some("textDocument/didChange")
        })
        .collect::<Vec<_>>();
    assert_eq!(changes.len(), 1);
    assert_eq!(
        changes[0]
            .pointer("/msg/params/textDocument/version")
            .and_then(|v| v.as_i64()),
        Some(2)
    );
    servers.sync(
        edited
            .iter()
            .take(1)
            .map(|(p, t)| (p.as_path(), t.as_str())),
    );
    let messages = wait_trace(root, |messages| {
        messages
            .iter()
            .filter(|v| {
                v.pointer("/msg/method").and_then(|m| m.as_str()) == Some("textDocument/didClose")
            })
            .count()
            == 2
    });
    assert_eq!(
        messages
            .iter()
            .filter(|v| v.pointer("/msg/method").and_then(|m| m.as_str())
                == Some("textDocument/didClose"))
            .count(),
        2
    );
    servers.sync(std::iter::empty());
    let messages = wait_trace(root, |messages| {
        messages
            .iter()
            .filter(|v| {
                v.pointer("/msg/method").and_then(|m| m.as_str()) == Some("textDocument/didClose")
            })
            .count()
            == 3
    });
    assert_eq!(
        messages
            .iter()
            .filter(|v| v.pointer("/msg/method").and_then(|m| m.as_str())
                == Some("textDocument/didClose"))
            .count(),
        3
    );
    servers.shutdown();
    wait_trace(root, |messages| {
        messages
            .iter()
            .filter(|v| v.pointer("/msg/method").and_then(|m| m.as_str()) == Some("exit"))
            .count()
            == 2
    });
}

#[test]
fn disabled_missing_and_remote_hosts_do_not_start_servers() {
    let temp = tempdir().unwrap();
    let root = temp.path();
    let file = root.join("x.py");
    let open: [(PathBuf, String); 1] = [(file.clone(), "".into())];
    let program = server_script(root);
    let mut disabled = Servers::new(
        crate::host::local(),
        root.to_path_buf(),
        LspSettings {
            python_off: true,
            python_path: Some(program.to_string_lossy().into_owned()),
        },
    );
    disabled.sync(open.iter().map(|(p, t)| (p.as_path(), t.as_str())));
    assert!(disabled.handle(&file).is_none());
    assert!(!root.join("trace.jsonl").exists());
    let missing = root.join("missing-server");
    let mut not_found = Servers::new(
        crate::host::local(),
        root.to_path_buf(),
        LspSettings {
            python_off: false,
            python_path: Some(missing.to_string_lossy().into_owned()),
        },
    );
    let mut events = vec![];
    for _ in 0..150 {
        not_found.sync(open.iter().map(|(p, t)| (p.as_path(), t.as_str())));
        events.extend(not_found.take_events());
        if !events.is_empty() {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(events.len(), 1);
    let remote = std::sync::Arc::new(crate::testing::antler_remote(
        &mut std::process::Command::new("cargo"),
    ));
    let mut remote_servers =
        Servers::new(remote.clone(), root.to_path_buf(), LspSettings::default());
    remote_servers.sync(open.iter().map(|(p, t)| (p.as_path(), t.as_str())));
    assert!(remote_servers.take_events().is_empty());
    assert!(remote_servers.handle(&file).is_none());
    remote.close();
}
#[test]
fn references_are_grouped_by_path_with_source_line_text() {
    let temp = tempdir().unwrap();
    let root = temp.path();
    let program = server_script(root);
    fs::write(root.join("a.py"), "alpha ref\n").unwrap();
    fs::write(root.join("b.py"), "beta ref\nsecond ref\n").unwrap();
    let a = root.join("a.py");
    let open = vec![(a.clone(), "alpha".into())];
    let mut servers = Servers::new(crate::host::local(), root.to_path_buf(), LspSettings {
        python_off: false, python_path: Some(program.to_string_lossy().into_owned()),
    });
    wait_for(&mut servers, &open);
    let result = servers.handle(&a).unwrap().references("alpha", 0, 0);
    assert_eq!(result.symbol, "alpha");
    assert_eq!(result.count(), 3);
    assert_eq!(result.files.len(), 2);
    assert_eq!(result.files[0].0.canonicalize().unwrap(), a.canonicalize().unwrap());
    assert_eq!(result.files[0].1[0].1, "alpha ref");
    assert_eq!(result.files[1].0.canonicalize().unwrap(), root.join("b.py").canonicalize().unwrap());
    assert_eq!(result.files[1].1.iter().map(|(_, line)| line.as_str()).collect::<Vec<_>>(), ["beta ref", "second ref"]);
}

