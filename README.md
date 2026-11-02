<p align="center"><img src="assets/icon.png" width="128" alt="Antler logo"></p>

# Antler

A fast, lightweight macOS editor that shows every git repository under a folder in one panel: browse, edit, review diffs, stage and commit across all of them.

Built in Rust on [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui). Aimed at developers who work in folders with many nested repositories, and at anyone reviewing changes that code agents have made across them.

## Screenshots

![Git view with several repositories](docs/screenshots/git-view.png)

![Side-by-side diff](docs/screenshots/diff.png)

![Explorer and editor](docs/screenshots/explorer-editor.png)

## Install

There is no downloadable release yet; build from source (requirements below):

```sh
scripts/bundle.sh --install
```

This builds in release mode, creates an ad-hoc signed `Antler.app`, copies it to `/Applications` (asking first if it exists; `--yes` skips the question) and installs the `antler` command in `~/.local/bin`. Re-run it to update. Without `--install` it only creates `target/bundle/Antler.app`.

## Usage

```sh
antler [folder]
```

- `folder`: the folder to open. Without it, Antler shows a window with "Open Folder…" (Cmd+O).

## Building from source

- macOS on Apple Silicon.
- Full Xcode (not only the Command Line Tools) with the Metal Toolchain: `xcodebuild -downloadComponent MetalToolchain`.
- Rust via [rustup](https://rustup.rs) (stable).

```sh
cargo run --release -- ~/path/to/folder
cargo test
```

## Project status

Young and moving fast. macOS only, for now.

## Contributing

- [`AGENTS.md`](AGENTS.md): how to work on the project.
- [`GLOSSARY.md`](GLOSSARY.md): the domain vocabulary.
- [`docs/adr/`](docs/adr/): architecture decisions.

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at your option.

File and folder icons are from [Material Icon Theme](https://github.com/material-extensions/vscode-material-icon-theme) (MIT, Material Extensions); see `assets/material-icons/`.
