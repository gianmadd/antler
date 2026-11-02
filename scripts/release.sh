#!/bin/bash
# scripts/release.sh X.Y.Z [--dry-run]
# Bumps the version, tags vX.Y.Z, pushes, waits for the release workflow and updates the Homebrew cask.
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

# run: executes the command, or only prints it with --dry-run
run() { if $dry; then echo "[dry-run] $*"; else "$@"; fi; }

cd "$(dirname "$0")/.."
[[ -z $(git status --porcelain) ]] || { echo "working tree not clean" >&2; exit 1; }
[[ $(git branch --show-current) == main ]] || { echo "not on main" >&2; exit 1; }
git rev-parse "$tag" >/dev/null 2>&1 && { echo "tag $tag already exists" >&2; exit 1; }
cargo test

# the first `version = ` line of Cargo.toml is the package's
run sed -i '' "1,/^version = /s/^version = .*/version = \"$version\"/" Cargo.toml
run cargo update -p antler --precise "$version"
run git commit -am "Release $tag"
run git tag "$tag"
run git push origin main "$tag"

if $dry; then
    echo "[dry-run] gh run watch the $tag run, download Antler-$version-arm64.zip, update Casks/antler.rb in $tap"
    exit 0
fi

# the run for the tag may take a moment to appear
run_id=
for _ in $(seq 30); do
    run_id=$(gh run list -R $repo --workflow release.yml --branch "$tag" --limit 1 --json databaseId -q '.[0].databaseId')
    [[ -n $run_id ]] && break
    sleep 5
done
[[ -n $run_id ]] || { echo "no workflow run found for $tag" >&2; exit 1; }
gh run watch "$run_id" -R $repo --exit-status

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
zip=Antler-$version-arm64.zip
gh release download "$tag" -R $repo -p "$zip" -D "$work"
sha=$(shasum -a 256 "$work/$zip" | cut -d' ' -f1)

gh repo clone $tap "$work/tap" -- -q
sed -i '' -e "s/^  version \".*\"/  version \"$version\"/" -e "s/^  sha256 \".*\"/  sha256 \"$sha\"/" "$work/tap/Casks/antler.rb"
git -C "$work/tap" commit -qam "antler $version"
git -C "$work/tap" push -q
echo "Released $tag; cask updated (sha256 $sha)"
