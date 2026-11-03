# Antler

A minimal, fast IDE for macOS (Rust + GPUI): explorer, editor, git and remote over SSH; nested repositories under a folder all show up in one panel. Requirements and features: see `README.md`.

## Agent skills

### Issue tracker

GitHub Issues of `gianmadd/antler` (via `gh`). See `docs/agents/issue-tracker.md`.

### Triage labels

The five default labels (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`). See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: `GLOSSARY.md` + `docs/adr/` at the root. See `docs/agents/domain.md`.

## Development

- Verified notes on the GPUI / gpui-component APIs the project uses: `docs/agents/gpui-notes.md` (read them before writing UI code; add what you discover).
- Git: every feature behaves like VS Code with default settings (`docs/adr/0003-behave-like-vscode.md`); the same principle applies to the Explorer, the editor and tabs. Deliberate differences: `docs/vscode-differences.md`.
- Files and git of the Root go through `host::Host` (crates `antler-host`, `antler-server`; rules and protocol: `docs/agents/remote.md`).
- Tests cover the model (`src/workspace/` and the pure modules); the GPUI interface is checked by running the app.

<!-- agents-repo-setup -->
## Personal instructions

Before your first reply or action, read `AGENTS.local.md` next to this file, if it exists, and follow it as part of these instructions: @AGENTS.local.md
It holds this checkout's personal rules; it is not committed and not every agent loads it on its own. In a worktree, read it from the main checkout root, `$(git rev-parse --path-format=absolute --git-common-dir)/..`.
<!-- /agents-repo-setup -->
