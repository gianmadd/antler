# The file and git layer and the remote server (stages 1 and 2 of #20)

All access to the Root's files and git goes through `host::Host` (crates/antler-host/src/host.rs; read its doc comments for the contract). Two implementations: `Local` (this machine) and `remote::Remote`, which forwards each call to `antler-server` on the Remote host (docs/adr/0005). Stage 3 starts the server over SSH and adds the Remote window; today `Remote` is only tested against a server spawned as a local child process.

## Crates
- `crates/antler-host` (library, no UI; meant to build on Linux, macOS and Windows): `host` (trait, `Local`, `remove_all`), `discovery` (Nested repository walk), `search` (`ignore` walk, `grep` matcher, `replace`), `exclude` (`files.exclude`), `protocol`, `remote` (client), `server` (the serve loop). Trash through objc2 is `cfg(target_os = "macos")`; elsewhere `Local::trash` deletes for good.
- `crates/antler-server`: the binary, `serve(stdin, stdout)`. Nothing else may print to stdout.
- The app (`src/`) keeps its module paths: `crate::host`, `crate::discovery`, `crate::search` re-export the library; `search.rs` also holds `SearchJob` / `ReplaceJob`.
- `cargo test --workspace` runs everything. `ANTLER_TEST_REMOTE=1 cargo test` runs the whole model suite through a spawned `antler-server` (the four tests of undoing a Trash skip themselves: a remote has none). `workspace/tests_remote.rs` and `crates/antler-server/tests/remote.rs` always run.

## Rules for new code
- A file or git access under the Root goes through the Root's host, never `std::fs`, `Command::new("git")` or `notify`.
- Calls are blocking, from background tasks, coarse (one call, whole result).
- The host belongs to the Root: `Workspace::host`. Jobs (`ScanJob`, `SearchJob`, `ReplaceJob`, `SaveJob`, `FilesJob`) carry it to their thread. Code below a job that has no host parameter (git.rs, the ops, the loaders) calls `host::current()`, which is the host of the enclosing `host::scoped(&host, || ...)`; outside any scope it is `Local`. A new `background_spawn` that touches files or git must wrap its body in `host::scoped(&self.ws.host, ...)`; a quick check on the UI thread uses `self.host` directly. Never cache `current()` across jobs.
- Settings, recent, keys, state, windows, update and askpass are not the Root's: they stay on `std::fs`.
- `git.rs` `diff_texts` writes temp files here and runs git on `host::local()`, whatever the Root's host.

## Protocol (`protocol.rs`)
- Transport: the server's stdin (requests) and stdout (answers); stderr is the server's log.
- Frame: `u32 header_len | u32 body_len` (big endian), a JSON header, a raw body. Why: the header stays readable in a captured stream and serde_json is already a dependency, while file bytes (any bytes, up to the 1 GiB limit) travel raw instead of through base64 (+33 %, an extra copy). No new dependency.
- First frame: the server's `Hello { protocol, os, arch }` (id 0). The client refuses another `VERSION`.
- Requests carry an `id`; answers carry the same id and may arrive in any order (a thread per request on the server), so several are in flight. One request per `Host` method, plus `Cancel { target }`.
- Streams: `Watch` is answered by `Ok`, then `Event { path }` frames with its id until `Cancel` (the client's `WatchGuard` drop). `Search` is one answer with the whole `Results`; its `Cancel` makes the server stop the walk early and answer with what it has (the client sends it as soon as the `cancel` flag is set). Search results are not streamed: the `Host` contract returns them whole.
- Errors: `Err { kind, msg }`; `kind` is an `io::ErrorKind` name (`NotFound`, `AlreadyExists`, `PermissionDenied`...) so the model's `kind()` matches work; empty for text errors (git, watch).
- `files.exclude`: the client sends the app's current patterns with `ListDir`, `Files` and `Search`; the server builds a `Local::with_exclude` per request (it has no settings).
- Trash: `Req::Trash` deletes for good; `Remote::trash` returns `Ok(None)`, so `Op::Trash` has no undo (the confirmation is the caller's).
- Lost connection: the reader thread ends, every waiting call fails with `ConnectionAborted`, new ones fail at once; `exists` is `false`, scans are empty, `search` has `error`. No reconnect yet.

## Paths on a Windows host
- On the wire a path is a string with `/` separators, drive included (`C:/Users/x`), made by `protocol::wire_path` and read by `native_path` (which puts the backslashes back on Windows). Nothing else changes: the app keeps `Path`/`PathBuf`, and `join`, `parent`, `file_name`, `strip_prefix`, `starts_with` work on `C:/Users/x` on a Mac. Windows accepts `/`.
- What the app model still assumes: the Root is a path the *server* gave or accepted, never `canonicalize`d, `is_absolute()`ed or `is_dir()`ed on the Mac (`main.rs` does that to the folder being opened: Remote windows must skip it); names are UTF-8 (lossy otherwise); no UNC (`\\server\share`) paths (`shortcut:` in `protocol.rs`). A file name with a backslash on a Unix host is fine (only a Windows server converts). Git on a Windows host reports paths with `/`, like on Unix.
- Not compiled here: the Windows branches (`Local::rename`'s case-only check, `copy` of links) and the Linux target were written blind; no Linux or Windows target is installed. Check them in stage 5's CI.

## Stage 3 hooks and still open
- `server::serve` returns when the connection ends and `antler-server`'s `main` exits; to survive a drop, keep the process (and its watches) and call `serve` again on the next connection. `Remote::connect` takes any reader/writer, `Remote::spawn` any `Command` (`ssh host antler-server`).
- Reconnect: `Remote` has no state to restore but the watches (`Shared::watches`); re-sending `Watch` on a new connection is the work.
- git credentials: `Envs` (askpass, `osascript`) are local UI; a remote git needs its own path. The envs are forwarded as given.
- Quick checks the model makes on the UI thread, which would block on a network call: `check_name`, `drop_entries` / `copy_target` (`metadata`, `exists`), `Workspace::resolve` (reads conflicted files), `restore_layout` (`metadata`), `ImageTab::load` (reads the whole image). Move them to background jobs or cache them.
- `Remote::search` / `files` copy the whole result per call; big Roots may want paging.
- The server starts a thread per request without a limit, and a `Cancel` that arrives before its `Watch` thread has stored the guard leaves the watch running until the connection closes.
