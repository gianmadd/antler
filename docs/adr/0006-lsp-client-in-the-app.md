# The language-server client lives in the app; a Remote host only pipes its stdio

Antler talks LSP to a Language server from the app, in local and Remote windows alike. On a Remote host `antler-server` only starts the server process and forwards its stdin/stdout unchanged, over the same kind of stream the port forwarding uses; it keeps no LSP state. Decided on 2026-11-04 for code navigation (definition, references, hover): one client for both cases, and the server stays stateless (`docs/agents/remote.md`). The cost is that every LSP message, edits included, crosses the SSH connection.

To be re-evaluated if latency on slow links or richer features (completion, diagnostics) make the round trips hurt.

## Considered options

- VS Code's model: the language extension and its client run on the host (extension host); the app sends high-level calls. Needs an extension host Antler does not have.
- Zed's model: the LSP client runs inside the remote server; the app sends typed requests (definition, references) and only those cross the network. Less traffic, but a second home for LSP state and a protocol message per feature.
