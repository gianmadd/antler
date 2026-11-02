# The file and git layer (remote development, stage 1 of #20)

All access to the Root's files and git goes through `host::Host` (`src/host.rs`); read its doc comments for the contract. `host::at(path)` returns the implementation for a path; today it is always `host::Local`, so behavior is the same as before the layer. The next stages add a remote implementation that forwards each call to `antler-server` on the Remote host (docs/adr/0005).

Rules for new code:
- A file or git access under the Root calls `host::at(path).…`, never `std::fs`, `Command::new("git")` or `notify`.
- Calls stay blocking and are made from background tasks, like the model's jobs. Keep operations coarse (one call, whole result).
- Settings, recent, keys, state, windows, update and askpass are not the Root's: they stay on `std::fs`.

## What stays outside the layer, and why
- `host.rs` `Local`: the local implementation. It delegates the scans to `discovery.rs` (Nested repository walk) and `search.rs` (`ignore` walk, `grep` matcher), which are therefore still `std::fs` code: `antler-server` will link the same modules.
- `git.rs` `diff_texts`: temp files in the Mac's temp folder, not the Root (git runs there, so it must stay local when the stage-2 router sends Root paths elsewhere).
- `main.rs` `canonicalize` / `is_dir` of the folder being opened and `settings_view.rs` creating `settings.json`: local, chosen by the user on the Mac. A Remote window gets its Root from the server.
- `git.rs` askpass (`osascript`): local UI; a remote git needs its own credential path (stage 3).

## For the remote stage
- `at(path)` picks the host by path alone: a local and a remote Root with the same path would clash. Tag remote paths or thread the host through the jobs.
- Paths are `Path`/`PathBuf` (`shortcut:` in `host.rs`); a Windows host needs a host-path type.
- `list_dir` applies `files.exclude` from the app's global (`workspace::excluded`), and `search`/`files` use it too: send the patterns with the request.
- Quick checks the model makes on the UI thread, which would block on a network call: `check_name`, `copy_target`, `Explorer paste` (`is_dir`), `Workspace::resolve` (reads conflicted files), session restore (`is_file`), `ImageTab::load` (reads the whole image). Move them to background jobs or cache them.
- `trash` returns `None` for "deleted for good": `Op::Trash` then has no undo (the confirmation is the caller's).
- `git::run_env` takes `envs`, used only for askpass today.
