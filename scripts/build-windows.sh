#!/bin/bash
# Cross-builds the Windows app (x86_64-pc-windows-msvc) on macOS with LLVM and cargo-xwin (#42).
# Releases are NOT built this way: they build on a native Windows runner.
#
#   (no flag)  debug build:   target/x86_64-pc-windows-msvc/debug/antler.exe
#   --release  optimized build with gpui-pre-windows' debug assertions on:
#                             target/x86_64-pc-windows-msvc/release/antler.exe
#   --check    cargo xwin check only (type-checks the Windows build, no exe)
#
# Why not a plain `cargo xwin build --release`: gpui-pre-windows compiles its HLSL shaders with the
# Windows SDK's fxc.exe in its build script, only when that script runs on a Windows host, and its
# release code `include!`s the result. From macOS the file is never generated and the release build
# (and `cargo xwin check --release`) fails at `shaders_bytes.rs`. With debug assertions the crate
# compiles the shaders at run time instead, from the .hlsl files at the build machine's registry
# path; the script copies them next to the exe with `install-shaders.cmd`, which puts them at that
# path on the Windows machine (see docs/agents/gpui-notes.md, "Windows").
set -euo pipefail

target=x86_64-pc-windows-msvc
mode=debug
for arg in "$@"; do
    case $arg in
        --release) mode=release ;;
        --check) mode=check ;;
        *) echo "usage: $0 [--release | --check]" >&2; exit 2 ;;
    esac
done

cd "$(dirname "$0")/.."

# Prerequisites: LLVM (clang-cl, llvm-lib, llvm-rc), lld (lld-link), cargo-xwin, the Rust target.
if [[ $(uname -s) == Darwin ]]; then
    command -v brew >/dev/null || { echo "Homebrew is needed: https://brew.sh" >&2; exit 1; }
    for formula in llvm lld; do
        brew list --formula "$formula" >/dev/null 2>&1 || brew install "$formula"
        PATH="$(brew --prefix "$formula")/bin:$PATH"
    done
    export PATH
fi
for tool in clang-cl llvm-lib llvm-rc lld-link; do
    command -v "$tool" >/dev/null || { echo "$tool not found on PATH (install LLVM and lld)" >&2; exit 1; }
done
cargo xwin --version >/dev/null 2>&1 || cargo install --locked cargo-xwin
rustup target list --installed | grep -qx "$target" || rustup target add "$target"

# cargo-xwin downloads the MSVC CRT and the Windows SDK on first use (Microsoft's license applies).
case $mode in
    check) exec cargo xwin check --target "$target" ;;
    debug) cargo xwin build --target "$target"; profile=debug ;;
    release)
        cargo xwin build --release --target "$target" \
            --config 'profile.release.package.gpui-pre-windows.debug-assertions=true'
        profile=release
        ;;
esac

# The shader sources gpui-pre-windows reads at run time: env!("CARGO_MANIFEST_DIR")/src/*.hlsl, a
# path of this machine that Windows resolves against the current drive (/Users/me/... -> C:\Users\me\...).
manifest=$(cargo metadata --format-version 1 --filter-platform "$target" \
    | grep -o '"manifest_path":"[^"]*/gpui-pre-windows-[^"/]*/Cargo.toml"' | head -1 \
    | sed 's/^"manifest_path":"//; s/"$//')
[[ -n $manifest ]] || { echo "gpui-pre-windows not found in cargo metadata" >&2; exit 1; }
crate_dir=$(dirname "$manifest")
out=target/$target/$profile
rm -rf "$out/gpui-shaders"
mkdir -p "$out/gpui-shaders"
cp "$crate_dir"/src/*.hlsl "$out/gpui-shaders/"
windows_dir="C:${crate_dir//\//\\}\\src"
printf '@echo off\r\nrem Puts the GPUI shader sources where this antler.exe looks for them (scripts/build-windows.sh).\r\nmkdir "%s" 2>nul\r\ncopy /y "%%~dp0gpui-shaders\\*.hlsl" "%s\\" >nul && echo Shaders copied to %s\r\n' \
    "$windows_dir" "$windows_dir" "$windows_dir" > "$out/install-shaders.cmd"

echo
echo "Built $out/antler.exe"
echo "On the Windows machine copy antler.exe, install-shaders.cmd and gpui-shaders\\ into one folder on C:,"
echo "run install-shaders.cmd once, then start antler.exe from a folder on C: (it reads the shaders from"
echo "$windows_dir)."
