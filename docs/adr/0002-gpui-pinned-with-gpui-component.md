# GPUI from `gpui-pre`, pinned to the version `gpui-component` requires

We use `gpui-component` (Longbridge, Apache-2.0) for the tree, lists, panels, editor and diff, and therefore `gpui-pre` at exactly the version it requires (`=0.3.10` with `gpui-component` 0.8.1), not the `gpui` from crates.io (0.2.2, unchanged since October 2025 and incompatible). The two dependencies are only upgraded together: GPUI is pre-1.0 and often changes in incompatible ways.

## Considered Options

- Zed's code (`editor`, `git_ui`): rejected, it is GPL-3.0, not published on crates.io and tied to the rest of Zed.
- A fork of GitComet: rejected, it is AGPL-3.0 and uses one repo per tab instead of a single panel.
