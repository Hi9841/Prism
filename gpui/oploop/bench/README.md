# Prism end-to-end bench

This bench measures the real Prism launcher on this machine: the GPUI build against the original React/Tauri build.
The journeys (J2-J5) and their start and end points are in `../JOURNEYS.md`.

## Files

| File | Purpose |
|---|---|
| `bench.py` | Runs J2 and J3 (and keeps the J5 frames), or J4. Writes raw data to `../results/raw/<stamp>/`. |
| `report.py` | Makes `summary.md` (tables with median, p95, min-max, n, and a rank-sum p value) and the J5 compare. |
| `probe.py` | Smoke test: open, type `c`, clear, close, and save PNGs. Use it after a product change that can break the bench. |
| `apps.py` | Starts, finds, drives, and stops one build. Stops and restarts the installed Prism. Guards `prism.json`. |
| `winapi.py` | Win32 calls: windows, posted keys, `PrintWindow` capture, frame compare. |
| `guardian.ps1` | A detached watcher. If the bench process stops for any reason, it stops the test build, starts the installed Prism, and restores `prism.json`. |

## Safety rules of the bench

- The bench sends keys only to the palette window with `PostMessage`. It does not send global input. It never sends Enter.
- The bench stops the installed Prism (`%LOCALAPPDATA%\Prism\prism.exe`) before a session. It starts the installed Prism again after each session, also after an error.
- `guardian.ps1` does the same restore if the bench process is killed.
- At the start of each session, after it stops the installed Prism, the bench copies `prism.json` to `%LOCALAPPDATA%\PrismGpuiE2E\backup\prism.json`. After the session, it compares the two files and restores the backup if a test build changed it. (Until 2026-10-27 the backup was taken once, so a later session restored it over the owner's newer changes.)
- The bench ignores `prism.exe --parity` processes and the Start-menu helper modes. These are not launcher instances.
- The palette opens on the desktop during a session. Do not type in the palette during a run.

## Procedure

Use Git Bash at the repo root.

1. Set the temporary folder (the shell cannot write `%TEMP%`):

   ```sh
   export TMP=$PWD/gpui/target/tmp TEMP=$PWD/gpui/target/tmp
   ```

2. Build the GPUI app (release, own target folder):

   ```sh
   (cd gpui && CARGO_TARGET_DIR=target/agent-e2e cargo build --release -p prism-app)
   ```

3. Build the original Tauri app (release, own target folder):

   ```sh
   bunx vite build
   (cd src-tauri && CARGO_TARGET_DIR=../gpui/target/agent-e2e-tauri cargo build --release)
   ```

4. Do a smoke test of each build:

   ```sh
   uv run gpui/oploop/bench/probe.py gpui
   uv run gpui/oploop/bench/probe.py tauri
   ```

5. Run J2, J3, and the J5 frames (about 25 minutes):

   ```sh
   uv run gpui/oploop/bench/bench.py run --j2-runs 20 --j3-reps 20 --blocks 4
   ```

6. Run J4 (about 4 minutes for each run and app, so 25 minutes for 3 runs):

   ```sh
   uv run gpui/oploop/bench/bench.py j4 --runs 3
   ```

7. Make the summary (J5 OCR takes some minutes):

   ```sh
   uv run gpui/oploop/bench/report.py gpui/oploop/results/raw/<run stamp> gpui/oploop/results/raw/<j4 stamp>-j4
   ```

8. Remove the build output that step 3 made at the repo root (untracked):

   ```sh
   rm -rf dist src-tauri/gen
   ```

If a run stops and the installed Prism does not run, do this command at once (it restores
prism.json from the backup that the failed run took at its start; later, it would undo your own changes):

```sh
uv run gpui/oploop/bench/bench.py restore
```

## Options

- `bench.py run --apps gpui` measures one app only (for an A/A noise check, or an A/B of two GPUI builds after you copy them).
- `bench.py run --blocks N` splits the runs into N sessions for each app, in the order GPUI, Tauri, GPUI, Tauri.
- `bench.py j4 --idle 60 --cycles 20` sets the idle time and the cycle count.

## Raw data

- `rows.json`: one row for each measurement. J2 and J3 rows have `first_ms`, `settled_ms`, `changes`
  (time, changed pixels, first and last changed row), `perf` (the app's own time events in that period), and `machine_cpu_pct`.
- `perf-<session>.jsonl`: the `PRISM_PERF_LOG` file of each session.
- `frames/`: the settled J3 frames (`<app>-qNN-rMM.png`) and the side-by-side PNGs (`pair-qNN.png`).
- `summary.md` and `j5-rows.json`: the output of `report.py`.
