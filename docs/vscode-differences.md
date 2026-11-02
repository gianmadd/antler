# Differences from VS Code

Antler copies VS Code's Explorer and Source Control; for git, ADR 0003 applies ("behave like VS Code with default settings"). This file lists the **deliberate** differences. Anything still missing is tracked as a parity gap in the issue tracker.

| Area | VS Code | Antler |
|---|---|---|
| Nested repositories | scan up to `git.repositoryScanMaxDepth` | same limit (default 3), plus: repos inside folders ignored by the parent repo, Linked worktrees labelled "worktree of …", `.venv*` folders skipped |
| Section header | name; branch in the status bar or in the Repositories view | name + branch in grey next to it; no Repositories view |
| Diff | side by side by default (inline below 900 px), right side editable, stage of selected lines; gutter bars with quick diff | side by side by default, inline below 900 px; the right side of the Changes diff is editable only side by side (inline, Staged Changes and commit diffs are read-only; the editor has no view zones to draw removed lines in an editable inline view); the two halves scroll together line by line, with no filler blank lines (a changed line faces the removed one, extra additions stay without a counterpart; the editor cannot insert blank space between lines); Stage, Unstage and Discard Block act on one change, so two nearby changes are two Blocks; gutter bars, Stage/Discard Block and Stage/Revert Selected Ranges work on the text in the editor, saved or not; a revert edits the text (one undo step, whole changes at a time) and then saves, with no confirmation, as VS Code does; Unstage Selected Ranges only in the Staged Changes diff, where VS Code has it too |
| Graph | current branch + upstream ("Auto" filter), Auto / All / branch picker; the choice is saved per repo and survives a restart | same; saved in `~/.config/antler/state.json` (`graph_filters`, `"all"` or a list of refs, Auto not saved) instead of VS Code's per-workspace storage, so it is per repo path, whichever folder is open |
| Actions on Graph commits | Checkout (Detached), Create Branch…, Create Tag…, Cherry Pick, Compare with Remote / Merge Base / Ref…, Copy; "Force Checkout" discards all changes, untracked files included (to the Trash), then checks out | same, but no Compare entries: they open a multi-file diff editor between two refs, which Antler has no view for; Esc on the tag message box cancels the tag (VS Code still creates a lightweight one) |
| Branch | branch name in a box at the top, with the live "Branch … already exists" / "The new branch will be …" line; "Stash & Checkout" / "Migrate Changes" ask for the stash message | same, also for Rename Branch…; no `git.branchPrefix`, random name or validation regex, which are off or empty in VS Code's defaults |
| Auto save | `off` by default | on by default after 0 ms |
| Markdown preview | new tab or side by side | in the same tab, eye button / Cmd+Shift+V |
| Typing in the Explorer tree | jumps to the row (`typeNavigationMode`); the find box at the top right highlights and jumps, with ↑/↓/Enter, Esc closes it; the Filter button switches to filtering | same as VS Code with find highlighting (default `find`); the filter stays behind the Filter button and applies only to rows already loaded. Every letter opens the find box, without Cmd+Alt+F |
| Settings | UI + JSON | JSON only with 7 keys, read at startup; Always / Never in the commit prompt and "Do not ask me again" on move update it (`git_enable_smart_commit`, `git_suggest_smart_commit`, `explorer_confirm_drag_and_drop`) |
| Moving onto an existing name (drag, Cut/Paste) | asks whether to replace | refuses with an error, both files stay |
| Windows | one app, several windows, reopening | one window per process; Open Folder replaces the folder |
| Theme | light/dark and themes | dark only, Amber accent, font 14 |
| Zoom keys | ⌘= / ⌘- zoom the whole window, ⌘0 focuses the side bar | ⌘= / ⌘- / ⌘0 change and reset the editor font size, like Zed |
| Keybindings | `keybindings.json` with `when` expressions, comments allowed | `~/.config/antler/keybindings.json`, same `[{ key, command, when }]` and `-command`, reloaded within a second of saving; plain JSON (no comments), `when` is one of `input`, `editor`, `explorer`, `git`, `search`, `commit` (unknown ones are ignored with a warning); command ids are VS Code's where VS Code has the command |
| Staged/Changes groups | collapsible | not collapsible (to be aligned) |

Excluded by choice: integrated terminal, extensions, AI-generated commit messages.
