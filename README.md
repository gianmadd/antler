<p align="center"><img src="assets/icon.png" width="128" alt="Antler logo"></p>

# Antler

A minimal, fast IDE for macOS: explorer, editor, git and remote over SSH. Nested repositories under a folder all show up in one panel.

Built in Rust on [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui).

## Screenshots

![Git view with several repositories](docs/screenshots/git-view.png)

![Side-by-side diff](docs/screenshots/diff.png)

![Explorer and editor](docs/screenshots/explorer-editor.png)

## Install

With Homebrew (installs `Antler.app` and the `antler` command):

```sh
brew install --cask gianmadd/tap/antler
```

Or download `Antler-<version>-arm64.zip` from the [Releases](https://github.com/gianmadd/antler/releases) page and move `Antler.app` to `/Applications`.

Or build from source (requirements below):

```sh
scripts/bundle.sh --install
```

This builds in release mode, creates an ad-hoc signed `Antler.app`, copies it to `/Applications` (asking first if it exists; `--yes` skips the question) and installs the `antler` command in `~/.local/bin`. Re-run it to update. Without `--install` it only creates `target/bundle/Antler.app`; `--zip` also makes the release zip.

On Windows (x86_64), download `Antler-<version>-x86_64-windows.zip` from the Releases page and extract it anywhere: it holds `antler.exe` and `bin\antler.cmd`, the `antler` command (add that `bin` folder to `PATH`). It is not signed, so SmartScreen may ask the first time ("More info", "Run anyway"). To build the zip from source on Windows (MSVC Build Tools and a Windows 10/11 SDK), run `scripts/bundle-windows.ps1`.

**First launch:** Antler is not notarized yet, so macOS blocks it the first time. Open System Settings, Privacy & Security, and click "Open Anyway", or run `xattr -dr com.apple.quarantine /Applications/Antler.app`.

## Usage

```sh
antler [folder]
```

- `folder`: the folder to open. Without it, Antler shows a window with "Open Folder…" (Cmd+O).
- One app, many windows: with Antler running, `antler folder` opens a window in it, or brings forward the window that already shows that folder. New Window is ⇧⌘N, Close Window ⇧⌘W; at the next start every window comes back (`window.restoreWindows`).

## Building from source

- macOS on Apple Silicon.
- Full Xcode (not only the Command Line Tools) with the Metal Toolchain: `xcodebuild -downloadComponent MetalToolchain`.
- Rust via [rustup](https://rustup.rs) (stable).

```sh
cargo run --release -- ~/path/to/folder
cargo test
```

## Project status

Young and moving fast. macOS on Apple Silicon only, for now. Releases are published on GitHub (`v0.x`) and are ad-hoc signed; each also carries `antler-server` for Linux, macOS and Windows (x86_64 and aarch64), which the app installs on a Remote host.

## Contributing

- [`AGENTS.md`](AGENTS.md): how to work on the project.
- [`GLOSSARY.md`](GLOSSARY.md): the domain vocabulary.
- [`docs/adr/`](docs/adr/): architecture decisions.

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at your option.

The built-in file icon theme is Seti, from [Visual Studio Code](https://github.com/microsoft/vscode/tree/main/extensions/theme-seti) (MIT) and [seti-ui](https://github.com/jesseweed/seti-ui) (MIT, Jesse Weed); see `assets/seti/ThirdPartyNotices.txt`. Other icon and color themes install from a `.vsix` (for example from [Open VSX](https://open-vsx.org/?category=Themes)) and keep their own licenses.
