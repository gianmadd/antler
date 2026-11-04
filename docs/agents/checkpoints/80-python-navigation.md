# Python navigation checkpoint

Implementation of [spec #80](https://github.com/gianmadd/antler/issues/80) is paused at the user's request on 2026-11-04. All work is committed. The spec and its tickets remain open; final reviews and UI acceptance are unfinished.

## Branches and commits

| Branch | Checkpoint | Contents |
|---|---|---|
| `feat/80-python-navigation` | `a9a336c` before this note | Integration branch: #82 history, #87 LSP foundation, #83 definitions/type definitions/implementations and location picker |
| `feat/84-language-server-hover` | `1350103` | Hover request, provider, Markdown/null and UTF-16 tests; based on integration `a9a336c` |
| `feat/85-references-view` | `c35c181` | References request and side view, activity-bar wiring and tests; integration merge completed |
| `feat/86-language-server-restart` | `59befb3` | Crash budget/restart, document replay, Open Log, startup cancellation, process-tree cleanup and quit handling; based on integration `6071ba5` |

The last three branches have not been merged into integration. Their checkpoint commits preserve work in progress; they are not a declaration that the spec is complete. Local worktrees remain under `.scratch/worktrees/{82,83,84,85,86,87}`. The integration branch already contains the work from #82, #83 and #87, so their separate branches are not needed to recover those changes.

## Validation completed

- #82: History and key/palette tests passed. In the running test app, Go to Line moved from line 1 to 3, Back returned to 1, Forward returned to 3, and Back reopened a closed file at line 3. Commands were exercised through the palette; the automated Ctrl-minus injection did not navigate, so verify the physical shortcuts.
- #87: four focused LSP tests passed. A real basedpyright 1.40.2 accepted `python.pythonPath` through `workspace/configuration` and resolved a module available only in that interpreter. Notes are in `docs/agents/lsp.md`. The app started the real Python wrapper and Node child; normal quit ended both, without a crash report.
- #83: `cargo check --bin antler` and seven focused LSP tests passed after fixing build isolation. Tests cover response shapes, UTF-16 request columns and unsaved target text. The editor provider passes full text, and show-document file reads run in the background. The running-app navigation check had only just started when the user requested this checkpoint; it is still pending.
- #84: `cargo check -p antler` and nine focused LSP tests passed on this branch. Hover Markdown/null responses and UTF-16 ranges are covered. The real hover popover is pending.
- #85: eight focused LSP tests passed before the last integration merge commit. The activity-bar test result was not recovered after the connection interruption; rerun it. Recheck the final merged branch.
- #86: eight LSP tests, the host process-tree test and `cargo check --workspace` passed earlier in development. Later quit-flow changes and the final log-path assertion change have not been rechecked. The log-file existence assertion hit sandbox restrictions on the app cache directory; the current assertion checks the `.log` path contract instead.
- Integration at `6071ba5`: 458 app tests passed with one existing interactive SSH test ignored; 20 host tests, daemon and ports tests passed. The workspace run stopped on an FSEvents initialization failure in the reconnect test. That test passed in isolation outside the sandbox. A full suite must run again after all branches are merged.

## Build isolation

Concurrent Cargo commands using one `CARGO_TARGET_DIR` executed stale test binaries from another worktree. Do not trust earlier concurrent runs or run Cargo concurrently across these worktrees with a shared target directory.

The session used `/tmp/antler-cargo-check.py` to lock the entire Cargo command and run `cargo clean -p antler -p antler-host -p antler-server` before rebuilding local packages, retaining third-party dependency artifacts. That helper is temporary. Either recreate this serialization or use separate target directories. Once all work is merged and only one checkout is being built, clean the three local packages once before the final checks.

Rust is available through `PATH=/opt/homebrew/opt/rustup/bin:$PATH` on this machine. For the final full suite, allow the filesystem access needed by FSEvents and cache/log tests. Windows Rust targets are installed, but `cargo-xwin` is not installed. Check the Host crate for Windows after the process-cleanup changes; no Windows runtime check has been done.

## Resume work

1. Read `AGENTS.md`, `AGENTS.local.md`, the spec and ticket program designs. Continue the explicitly invoked `implement-spec` workflow. Workers in this session use `gpt-6-luna` with medium effort.
2. Merge #84, #85 and #86 into `feat/80-python-navigation`, using a merger worker and preserving all request/provider/lifecycle changes. #86 predates #83 and will need particular care around `Handle`, document state, actions and tests. Do not edit the main branch.
3. Recheck the latest #86 quit changes and crash/log tests. GPUI's `on_app_quit` future has a 200 ms deadline: normal quit must drain servers before `cx.quit()`, with immediate cleanup as the app-quit fallback. Check cancellation during initialization, idempotent process-tree termination, document replay and the five-start crash budget.
4. Add a Workspace-level stand-in test for one path in two editor groups, edits through both groups, and closing only one group. Existing foundational stdio tests exercise `Servers::sync` with duplicate paths, rather than that whole Workspace flow. Make the pure interpreter test portable: its current `venv/bin/python` fixture is Unix-specific.
5. Run the app with a real basedpyright. Check F12, Cmd-click, Type Definition, Implementation, outside-Root library/stub tabs, the location picker, References count/context/replacement/empty state/activity-bar rules, hover, history and restart/Open Log. Keep the user's existing Antler instance untouched; stop all test processes afterward.
6. Add the no-reference-count-CodeLens limitation to issue #2, as #85 requests. That tracker edit has not been made.
7. Run `code-review` (Standards and Spec axes) and `ponytail-review` in parallel, as required by `AGENTS.local.md`. Fix all code-review findings and the Must/Should findings from ponytail-review in one implementer worker. Keep reports separate; list Nice to have findings.
8. Run final checks and UI validation. Only then resolve #82–#87 and #80, and clean up the implementer worktrees. No PR has been opened; this tracker documents direct issue closure.

## Temporary UI fixtures

The session used `/tmp/AntlerNavigationCheck.app` with its own bundle identifier, `/tmp/antler-navigation-ui` (local function, `attrs` in `.venv`, builtin and abstract-class examples), and `/tmp/antler-lsp-check/bin/basedpyright-langserver`. These paths are temporary and may need recreation. The test app was quit at the checkpoint. No worker should continue implementation until the user resumes.
