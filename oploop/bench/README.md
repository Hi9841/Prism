# Prism end-to-end bench

This bench measures the real Prism launcher (the GPUI build) on this machine: one build, or an A/B of
two or more builds. The journeys (J2-J4) and their start and end points are in `../JOURNEYS.md`. Until
release 1.0.11 it also measured the original React/Tauri app; that app left the repo after 1.0.11.

## Files

| File | Purpose |
|---|---|
| `bench.py` | Runs J2 and J3 (and keeps the settled J3 frames), or J4. Writes raw data to `../results/raw/<stamp>/`. |
| `compare.py` | Tables for any builds of one run: median, p95, n, and a rank-sum p value against the first build. |
| `probe.py` | Smoke test: open, type `c`, clear, close, and save PNGs. Use it after a product change that can break the bench. |
| `apps.py` | Starts, finds, drives, and stops one build. Stops and restarts the owner's Prism. Guards `prism.json`. |
| `winapi.py` | Win32 calls: windows, posted keys, `PrintWindow` capture, frame compare. |
| `guardian.ps1` | A detached watcher. If the bench process stops for any reason, it stops the test build, starts the owner's Prism, and restores `prism.json`. |
| `functest.py` | Functional test of the real app (host integration): launches, context menu, settings, taskbar settings, window size, topmost, global hotkey, single instance, Quit. See below. |
| `uimem.py`, `memmap.py` | Memory and threads of the UI layer alone (`prism.exe --parity`, no file index), and committed private memory by region. |
| `threads.py` | Read-only thread census of one process: name, start module, CPU time, and the CPU of each thread over a watch period. |
| `uiidle.py` | UI layer at rest in parity mode: context switches and CPU (cycle counters) while the palette is open and while it is hidden. |
| `uiframes.py` | Exact frame counts and frame time for the first open, the typed keys and a second open (parity mode, `ZED_MEASUREMENTS=1`). |
| `churn_ab.py` | Interleaved A/B of `churn_bench` builds: the catalog's cost while files change (a built-in script, or a replay with `--recording`). |
| `fsevents.py` | Records file name events below a folder (read-only) and tells how long new entries live. |
| `catalog_ab.py`, `sweep_check.py`, `launch_ab.py`, `launch_e2e.py` | The A/B drivers of sessions C and D (index load and search, sweep check, app launch). |

## Safety rules of the bench

- The bench sends keys only to the palette window with `PostMessage`. It does not send global input. It never sends Enter.
- The owner's Prism is the launcher that runs when the bench starts (the installed Prism, or a build the owner
  runs from `%LOCALAPPDATA%\PrismGpuiE2E\gpui`); the bench writes it to `owner-launcher.txt`. A session stops it,
  never deploys a build over it, and starts it again after the session, also after an error.
- `guardian.ps1` does the same restore if the bench process is killed.
- At the start of each session, after it stops the owner's Prism, the bench copies `prism.json` to
  `%LOCALAPPDATA%\PrismGpuiE2E\backup\prism.json`. After the session, it restores the backup if a test build
  changed it. (Until 2026-10-27 the backup was taken once, so a later session restored it over the owner's
  newer changes.)
- The bench ignores `prism.exe --parity` processes and the Start-menu helper modes. These are not launcher instances.
- The palette opens on the desktop during a session. Do not type in the palette during a run.

## Procedure

Use Git Bash at the repo root.

1. Set the temporary folder (the shell cannot write `%TEMP%`):

   ```sh
   export TMP=$PWD/target/tmp TEMP=$PWD/target/tmp
   ```

2. Build the GPUI app (release, own target folder). For an A/B, build the other commit too (for example in a
   worktree) and keep its exe:

   ```sh
   CARGO_TARGET_DIR=target/agent-e2e cargo build --release -p prism-app
   ```

3. Do a smoke test:

   ```sh
   uv run oploop/bench/probe.py gpui
   ```

4. Run J2 and J3 (about 6 minutes per build and block):

   ```sh
   uv run oploop/bench/bench.py run --apps gpui-base gpui --build gpui-base=<exe> --j2-runs 20 --j3-reps 20 --blocks 2
   ```

5. Run J4 (about 4 minutes for each run and build):

   ```sh
   uv run oploop/bench/bench.py j4 --apps gpui-base gpui --build gpui-base=<exe> --runs 3
   ```

6. Make the tables:

   ```sh
   uv run oploop/bench/compare.py oploop/results/raw/<run stamp> gpui gpui-base --j4 oploop/results/raw/<j4 stamp>-j4
   ```

If a run stops and the owner's Prism does not run, do this command at once (it restores
prism.json from the backup that the failed run took at its start; later, it would undo your own changes):

```sh
uv run oploop/bench/bench.py restore
```

## Options

- `bench.py run --apps gpui` measures one build only (an A/A noise check).
- `bench.py --build NAME=EXE` adds a build; each build deploys to `%LOCALAPPDATA%\PrismGpuiE2E\<NAME>`.
- `bench.py run --blocks N` splits the runs into N sessions for each build, interleaved.
- `bench.py j4 --idle 60 --cycles 20` sets the idle time and the cycle count.

## Raw data

- `rows.json`: one row for each measurement. J2 and J3 rows have `first_ms`, `settled_ms`, `changes`
  (time, changed pixels, first and last changed row), `perf` (the app's own time events in that period), and `machine_cpu_pct`.
- `perf-<session>.jsonl`: the `PRISM_PERF_LOG` file of each session.
- `frames/`: the settled J3 frames (`<build>-qNN-rMM.png`).

## Functional test of the real app (`functest.py`)

`uv run oploop/bench/functest.py run [test ...]` (`list` shows the tests). It drives the owner's Prism
(the bench's owner launcher, see above) with messages to its own window: keys, characters,
clicks and wheel steps. It does not send global input.

- It has REAL effects, approved by the owner on 2026-10-27: it launches Character Map and a test `.txt`
  (and closes them), opens Explorer, Properties and Windows Settings windows (and closes them), changes the
  taskbar alignment, density, auto-hide, combine mode and Start icon, and changes Prism's settings. It runs
  no power action and never unpins: on Windows builds that block programmatic pinning, an unpin could not
  be undone.
- Before the tests it saves `prism.json`, the taskbar registry values, the auto-hide state, the Start-icon
  files, the pinned-taskbar folder and the clipboard text. After the tests it stops the build, restores what
  changed, prints the differences that are left (expected: none), and starts the build hidden again.
- Results and window captures go to `%LOCALAPPDATA%\PrismGpuiE2E\functest\<stamp>\`.
- Click positions come from captures of a 560 px window at 100% zoom (`probe_settings_layout`,
  `probe_palette_layout`). Settings positions are relative to the panel, so other widths work.
- The tests skip, with a reason, when Notepad or Windows Settings already runs (a test does not close the
  owner's windows).

## Offline harnesses (no desktop session)

These run beside the owner's Prism and do not stop it. The backend benches are examples of `prism-core`
(`cargo build --release -p prism-core --example <name>`); they read and write only a copy of the app data.

1. Copy the app data once (catalog with the SQLite backup API, `apps.json`, `prism.json`, icons) into
   `target/tmp/bench/data`.
2. Keep the baseline binaries of the session (`target/oploop/<session>/base/`), and give each candidate its
   own copy: an A/B needs both exe files.

| Journey | Command |
|---|---|
| UI at rest | `uv run oploop/bench/uiidle.py --a <base prism.exe> --b <prism.exe> --rounds 3` |
| Frames for a query | `python -I oploop/bench/uiframes.py --a <base prism.exe> --b <prism.exe> --query report [--fixture FILE]` |
| Phase 1 of a keystroke | `query_bench.exe target/tmp/bench/data [results.tsv or --verify]` |
| Index load, search, first append | `uv run oploop/bench/catalog_ab.py --a <base catalog_bench.exe> --b <catalog_bench.exe> --db target/tmp/bench/data/catalog.db --rounds 3` |
| File churn (built-in script) | `uv run oploop/bench/churn_ab.py --a <base churn_bench.exe> --b <churn_bench.exe> --rounds 3` |
| File churn (real events) | the same with `--recording <slice.jsonl>` (from `fsevents.py record`, cut to the seconds to replay) |

- A churn A/B of 12 runs takes about 14 minutes (16 for the replay of 8 runs). Do not compile while it runs.
- `churn_bench` ends with exit code 1 when the catalog rows of the test tree differ from the disk. That
  check is the correctness test of each watcher change.
- The replay prints `REPLAY <n> operations done, <m> failed`: a replay with many failures measures less work.
