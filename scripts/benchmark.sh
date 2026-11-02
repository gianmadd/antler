#!/bin/bash
# Compares Antler, VS Code (no extensions) and Zed on the same Root: startup, idle memory, idle CPU,
# memory with 10 files open, disk size. Each app runs in a throwaway profile (Antler: an empty HOME, so ~/.config/antler is never read or written), so the user's own windows
# and settings are untouched; it only quits the processes it started. Results: target/bench/results.tsv.
#
# usage: scripts/benchmark.sh [ROOT] [RUNS]     (default ROOT: the current folder, RUNS 3)
# quick check of Antler alone (~5 min): APPS=antler CPU_WINDOW=30 scripts/benchmark.sh
set -euo pipefail

ROOT=${1:-$PWD} # before the cd below
cd "$(dirname "$0")/.."
RUNS=${2:-3}
IDLE_WAIT=${IDLE_WAIT:-30} # seconds after the window appears before measuring idle memory
CPU_WINDOW=${CPU_WINDOW:-60}
APPS=${APPS:-antler vscode zed} # APPS=antler: only Antler, a few minutes instead of ~25
OUT=target/bench
mkdir -p "$OUT"
xcrun swiftc -O scripts/winwait.swift -o "$OUT/winwait"
cargo build --release --quiet

ANTLER=target/release/antler
CODE="/Applications/Visual Studio Code.app/Contents/MacOS/Code"
ZED=/Applications/Zed.app/Contents/MacOS/zed

# the 10 largest tracked source files under ROOT, the same for every app
FILES=()
while IFS= read -r f; do FILES+=("$f"); done < <(
    find "$ROOT" -type f \( -name '*.py' -o -name '*.ts' -o -name '*.rs' -o -name '*.go' -o -name '*.java' \) \
        -not -path '*/node_modules/*' -not -path '*/.venv*' -not -path '*/dist/*' -not -path '*/target/*' \
        -size +3k -size -200k 2>/dev/null | head -200 | xargs ls -S | head -10)

tree() { # PID and all its descendants
    echo "$1"
    for c in $(pgrep -P "$1" 2>/dev/null); do tree "$c"; done
}
rss_mb() { # resident memory of the whole tree, MB
    tree "$1" | xargs ps -o rss= -p 2>/dev/null | awk '{s += $1} END {printf "%.0f", s / 1024}'
}
fp_mb() { # macOS phys_footprint of the whole tree (what Activity Monitor shows as Memory), MB
    tree "$1" | xargs footprint -f bytes 2>/dev/null | awk '/^Summary Footprint:/ {f = $3} /Footprint:/ && !/Summary/ {one = $(NF - 5)} END {printf "%.0f", (f ? f : one) / 1048576}'
}
cpu_s() { # total CPU time of the whole tree, seconds (ps prints [[dd-]hh:]mm:ss.cc)
    tree "$1" | xargs ps -o time= -p 2>/dev/null | awk '{
        d = 0; if (index($1, "-")) { split($1, x, "-"); d = x[1]; $1 = x[2] }
        n = split($1, p, ":"); s = p[n] + 60 * p[n - 1] + (n > 2 ? 3600 * p[n - 2] : 0) + 86400 * d
        t += s } END { printf "%.2f", t }'
}
stop() { # quits the tree started by this script
    local pids; pids=$(tree "$1")
    kill $pids 2>/dev/null || true
    sleep 3
    kill -9 $pids 2>/dev/null || true
    sleep 2
}

launch() { # app, profile dir, extra args...; prints the PID
    local app=$1 prof=$2; shift 2
    case $app in
        # Antler reads its config from $HOME/.config/antler: an empty HOME per launch = no settings, recent.json or state
        antler) rm -rf "$prof/home"; mkdir -p "$prof/home"; HOME="$prof/home" "$ANTLER" "$@" >/dev/null 2>&1 & ;;
        vscode) "$CODE" --user-data-dir "$prof/user" --extensions-dir "$prof/ext" --disable-extensions \
                    --new-window --skip-welcome --skip-release-notes --disable-workspace-trust "$@" >/dev/null 2>&1 & ;;
        zed) "$ZED" --user-data-dir "$prof" "$@" >/dev/null 2>&1 & ;;
    esac
    echo $!
}

printf 'app\trun\tstartup_s\tidle_rss_mb\tidle_fp_mb\tidle_cpu_pct\tloaded_rss_mb\tloaded_fp_mb\n' > "$OUT/results.tsv"
for run in $(seq 1 "$RUNS"); do
    for app in $APPS; do
        prof=$(mktemp -d)
        pid=$(launch "$app" "$prof" "$ROOT")
        startup=$("$OUT/winwait" "$pid" 90 || echo timeout)
        sleep "$IDLE_WAIT"
        idle=$(rss_mb "$pid"); idlefp=$(fp_mb "$pid")
        c0=$(cpu_s "$pid"); sleep "$CPU_WINDOW"; c1=$(cpu_s "$pid")
        cpu=$(awk -v a="$c0" -v b="$c1" -v w="$CPU_WINDOW" 'BEGIN { printf "%.1f", (b - a) * 100 / w }')
        stop "$pid"

        if [[ $app == antler ]]; then
            pid=$(ANTLER_BENCH_OPEN=$(IFS=:; echo "${FILES[*]}") launch antler "$prof" "$ROOT")
        else
            pid=$(launch "$app" "$prof" "$ROOT" "${FILES[@]}")
        fi
        "$OUT/winwait" "$pid" 90 >/dev/null || true
        sleep "$IDLE_WAIT"
        loaded=$(rss_mb "$pid"); loadedfp=$(fp_mb "$pid")
        stop "$pid"
        rm -rf "$prof"

        printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$app" "$run" "$startup" "$idle" "$idlefp" "$cpu" "$loaded" "$loadedfp" | tee -a "$OUT/results.tsv"
    done
done

{
    echo
    printf 'disk_mb\tantler\t%s\n' "$(du -sm /Applications/Antler.app 2>/dev/null | cut -f1)"
    printf 'disk_mb\tvscode\t%s\n' "$(du -sm '/Applications/Visual Studio Code.app' | cut -f1)"
    printf 'disk_mb\tzed\t%s\n' "$(du -sm /Applications/Zed.app | cut -f1)"
    printf 'files\t%s\n' "${FILES[*]}"
} | tee -a "$OUT/results.tsv"
