# Benchmark

`scripts/benchmark.sh` compares Antler, VS Code (no extensions) and Zed on the same folder: startup time, idle memory, idle CPU, memory with 10 files open, disk size.

```
scripts/benchmark.sh [ROOT] [RUNS]                     # default: current folder, 3 runs, ~25 min
APPS=antler CPU_WINDOW=30 scripts/benchmark.sh ROOT    # Antler alone, ~5 min
```

Every launch uses a throwaway profile. Antler runs with an empty `HOME`, so settings, `recent.json`, `state.json` and restored tabs from earlier sessions neither affect the numbers nor get modified. Results go to `target/bench/results.tsv`.

## Reading the results

- `*_rss_mb`: resident memory of the whole process tree.
- `*_fp_mb`: macOS physical footprint of the tree (what Activity Monitor shows as Memory). Prefer this one.
- `idle_*` is measured `IDLE_WAIT` (30 s) after the window appears; `loaded_*` with 10 files open.
- Do not read a single run in isolation: the first seconds after launch are not representative (see below).

## Reference numbers (Antler, footprint)

Release build on a large folder (~10k files, 3 repositories, ~20 linked worktrees), clean profile, sampled every second for 120 s, 6 runs:

- Transient peak of 133-149 MB in the first ~5 s, then a drop to a flat plateau after ~10-15 s.
- Plateau ~95 MB (one run, the first after a build with a cold disk cache, stayed at 110 MB).
- No growth, no spikes after 30 s, over 120 s.
- Breakdown at 95 MB (`footprint` / `vmmap --summary`): ~52 MB graphics (IOSurface ~30 MB + IOAccelerator ~22 MB), ~33 MB malloc, the rest small. The Rust heap is about a third of the total.

A one-off outlier of 176 MB at idle was seen once in an old benchmark (run against the real config dir) and not reproduced. If it shows up again, capture `footprint <pid>` and `vmmap --summary <pid>` while it is happening: the IOSurface/IOAccelerator vs Malloc split tells which side it is.
