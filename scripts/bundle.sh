#!/bin/bash
# Builds target/bundle/Antler.app (release binary, icon, ad-hoc signature).
# --install: also copies it to /Applications and installs the `antler` command in ~/.local/bin.
# --yes: replaces an existing /Applications/Antler.app without asking.
set -euo pipefail

install=false
yes=false
for arg in "$@"; do
    case $arg in
        --install) install=true ;;
        --yes) yes=true ;;
        *) echo "usage: $0 [--install] [--yes]" >&2; exit 2 ;;
    esac
done

cd "$(dirname "$0")/.."
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
cargo build --release

app=target/bundle/Antler.app
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/release/antler "$app/Contents/MacOS/antler"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
iconset=$tmp/AppIcon.iconset
mkdir "$iconset"
for size in 16 32 128 256 512; do
    sips -z $size $size assets/icon.png --out "$iconset/icon_${size}x${size}.png" >/dev/null
    sips -z $((size * 2)) $((size * 2)) assets/icon.png --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/AppIcon.icns"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Antler</string>
    <key>CFBundleDisplayName</key><string>Antler</string>
    <key>CFBundleIdentifier</key><string>dev.gianmadd.antler</string>
    <key>CFBundleShortVersionString</key><string>$version</string>
    <key>CFBundleVersion</key><string>$version</string>
    <key>CFBundleExecutable</key><string>antler</string>
    <key>CFBundleIconFile</key><string>AppIcon</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
</dict>
</plist>
PLIST

codesign --force --deep --sign - "$app"
echo "Built $app ($version)"

$install || exit 0

dest=/Applications/Antler.app
if [[ -e $dest ]] && ! $yes; then
    read -r -p "$dest exists. Replace it? [y/N] " answer
    if [[ $answer != [yY]* ]]; then
        echo "Not installed."
        exit 1
    fi
fi
rm -rf "$dest"
cp -R "$app" "$dest"
mkdir -p ~/.local/bin
install -m 755 scripts/antler ~/.local/bin/antler # replaces the plain binary that used to live there
echo "Installed $dest and ~/.local/bin/antler"
