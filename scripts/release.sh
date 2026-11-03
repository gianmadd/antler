#!/bin/bash
# scripts/release.sh X.Y.Z [--dry-run]
# Bumps the version, tags vX.Y.Z, pushes, waits for the release workflow and updates the Homebrew cask.
# Runs on macOS and under Git Bash on Windows (BSD or GNU sed, shasum or sha256sum); needs git, gh and cargo on PATH.
set -euo pipefail

version=${1:-}
dry=false
[[ ${2:-} == --dry-run ]] && dry=true
if [[ ! $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "usage: $0 X.Y.Z [--dry-run]" >&2
    exit 2
fi
tag=v$version
repo=gianmadd/antler
tap=gianmadd/homebrew-tap
# the release carries the app zips (macOS, Windows) and antler-server for every host (src/ssh.rs asset_name)
mac_zip=Antler-$version-arm64.zip
assets=("$mac_zip" "Antler-$version-x86_64-windows.zip")
for t in linux-x86_64 linux-aarch64 macos-x86_64 macos-aarch64 windows-x86_64.exe windows-aarch64.exe; do
    assets+=("antler-server-$version-$t")
done

# run: executes the command, or only prints it with --dry-run
run() { if $dry; then echo "[dry-run] $*"; else "$@"; fi; }
# sed -i for BSD (macOS) and GNU (Git Bash) sed alike: both take a suffix glued to -i (BSD's -i '' is not GNU's)
sed_i() { sed -i.bak "$@" && rm "${@: -1}.bak"; }
sha256() { if command -v shasum >/dev/null; then shasum -a 256 "$1"; else sha256sum "$1"; fi | cut -d' ' -f1; }

cd "$(dirname "$0")/.."
[[ -z $(git status --porcelain) ]] || { echo "working tree not clean" >&2; exit 1; }
[[ $(git branch --show-current) == main ]] || { echo "not on main" >&2; exit 1; }
git rev-parse "$tag" >/dev/null 2>&1 && { echo "tag $tag already exists" >&2; exit 1; }
cargo test

# the first `version = ` line of Cargo.toml is the package's
run sed_i "1,/^version = /s/^version = \"[^\"]*\"/version = \"$version\"/" Cargo.toml
run cargo update -p antler --precise "$version"
# the first release can already carry this version: nothing to commit then
git diff --quiet || run git commit -am "Release $tag"
run git tag "$tag"
run git push origin main "$tag"

if $dry; then
    echo "[dry-run] expected assets in the release: ${assets[*]}"
    echo "[dry-run] gh run watch the $tag run, download $mac_zip, update Casks/antler.rb in $tap"
    exit 0
fi

# the run for the tag may take a moment to appear
run_id=
for _ in $(seq 30); do
    run_id=$(gh run list -R $repo --workflow release.yml --commit "$(git rev-parse HEAD)" --limit 1 --json databaseId -q '.[0].databaseId' | tr -d '\r')
    [[ -n $run_id ]] && break
    sleep 5
done
[[ -n $run_id ]] || { echo "no workflow run found for $tag" >&2; exit 1; }
gh run watch "$run_id" -R $repo --exit-status

have=$(gh release view "$tag" -R $repo --json assets -q '.assets[].name' | tr -d '\r')
for a in "${assets[@]}"; do
    grep -qxF "$a" <<<"$have" || { echo "release $tag lacks the asset $a" >&2; exit 1; }
done

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
gh release download "$tag" -R $repo -p "$mac_zip" -D "$work"
sha=$(sha256 "$work/$mac_zip")

gh repo clone $tap "$work/tap" -- -q
sed_i -e "s/^  version \".*\"/  version \"$version\"/" -e "s/^  sha256 \".*\"/  sha256 \"$sha\"/" "$work/tap/Casks/antler.rb"
git -C "$work/tap" commit -qam "antler $version"
git -C "$work/tap" push -q
echo "Released $tag; cask updated (sha256 $sha)"
