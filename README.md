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

### The Windows app, from macOS

`scripts/build-windows.sh` cross-builds `antler.exe` for `x86_64-pc-windows-msvc` with LLVM and [cargo-xwin](https://github.com/rust-cross/cargo-xwin). It installs what is missing: Homebrew `llvm` and `lld` (put on `PATH` for the build), `cargo-xwin`, and the Rust target. On first use cargo-xwin downloads the MSVC CRT and the Windows SDK, under Microsoft's license.

```sh
scripts/build-windows.sh            # debug: target/x86_64-pc-windows-msvc/debug/antler.exe
scripts/build-windows.sh --release  # optimized: target/x86_64-pc-windows-msvc/release/antler.exe
scripts/build-windows.sh --check    # type-check only
```

A plain `cargo xwin build --release` does not work from macOS: GPUI compiles its shaders with the Windows SDK's `fxc.exe`, which only runs on Windows. The exe built here compiles them at startup instead, from the shader sources. Copy `antler.exe`, `install-shaders.cmd` and `gpui-shaders\` from the output folder into one folder on `C:` of the Windows machine, run `install-shaders.cmd` once, and start `antler.exe` from a folder on `C:`. This is for trying changes; release builds are made on Windows.

## Project status

Young and moving fast. macOS on Apple Silicon only, for now. Releases are published on GitHub (`v0.x`) and are ad-hoc signed; each also carries `antler-server` for Linux, macOS and Windows (x86_64 and aarch64), which the app installs on a Remote host.

## Contributing

- [`AGENTS.md`](AGENTS.md): how to work on the project.
- [`GLOSSARY.md`](GLOSSARY.md): the domain vocabulary.
- [`docs/adr/`](docs/adr/): architecture decisions.

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at your option.

The built-in file icon theme is Seti, from [Visual Studio Code](https://github.com/microsoft/vscode/tree/main/extensions/theme-seti) (MIT) and [seti-ui](https://github.com/jesseweed/seti-ui) (MIT, Jesse Weed); see `assets/seti/ThirdPartyNotices.txt`. Other icon and color themes install from a `.vsix` (for example from [Open VSX](https://open-vsx.org/?category=Themes)) and keep their own licenses.
