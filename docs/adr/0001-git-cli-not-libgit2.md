# Git through the system CLI, not libgit2 or gix

Antler runs the `git` binary and parses its output, as VS Code does. Hooks, credential helpers, GPG/SSH signing, `includeIf` and LFS then work as they do in a terminal without being reimplemented; with libgit2, push over SSH on macOS is a known source of problems. Partial staging is done by applying a generated patch with `git apply --cached`.
