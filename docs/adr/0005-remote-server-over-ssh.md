# Remote development runs a small Antler server on the Remote host

A Remote window talks over one SSH connection (the system `ssh`, so `~/.ssh/config`, keys, agent and `ProxyJump` work as in the terminal) to `antler-server`, a Rust binary built from Antler's own code. The server scans, watches, reads and writes files, runs git and forwards ports on the host; the app keeps the UI, unsaved text and settings. Linux (x86_64, arm64), macOS and Windows hosts are supported from the first version. Decided on 2026-11-02, like VS Code's Remote - SSH and Zed's remote projects.

The Mac downloads the server for the host's OS and architecture from the GitHub release of the same Antler version and copies it over SSH, so the host needs no internet access. If the connection drops, the server stays alive for a few minutes and the app reconnects to it.

All of Antler's file and git access goes through one layer that has a local and a remote implementation, so the workspace model does not know where its Root lives.

## Considered options

- Plain commands over SSH (`git`, `find`, `cat`…), nothing installed on the host: simpler for Linux/macOS, but a second implementation for Windows' shells and paths, no file watcher (polling only) and slow on large folders.
- Shipping every server binary inside the app: no download, but about 30–40 MB more for everyone.
- Letting the host download the server itself: needs internet on the host.
