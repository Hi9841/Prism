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
   export TMP=$PWD/gpui/target/tmp TEMP=$PWD/gpui/target/tmp
   ```

2. Build the GPUI app (release, own target folder). For an A/B, build the other commit too (for example in a
   worktree) and keep its exe:

   ```sh
   (cd gpui && CARGO_TARGET_DIR=target/agent-e2e cargo build --release -p prism-app)
   ```

3. Do a smoke test:

   ```sh
   uv run gpui/oploop/bench/probe.py gpui
   ```

4. Run J2 and J3 (about 6 minutes per build and block):

   ```sh
   uv run gpui/oploop/bench/bench.py run --apps gpui-base gpui --build gpui-base=<exe> --j2-runs 20 --j3-reps 20 --blocks 2
   ```

5. Run J4 (about 4 minutes for each run and build):

   ```sh
   uv run gpui/oploop/bench/bench.py j4 --apps gpui-base gpui --build gpui-base=<exe> --runs 3
   ```

6. Make the tables:

   ```sh
   uv run gpui/oploop/bench/compare.py gpui/oploop/results/raw/<run stamp> gpui gpui-base --j4 gpui/oploop/results/raw/<j4 stamp>-j4
   ```

If a run stops and the owner's Prism does not run, do this command at once (it restores
prism.json from the backup that the failed run took at its start; later, it would undo your own changes):

```sh
uv run gpui/oploop/bench/bench.py restore
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

`uv run gpui/oploop/bench/functest.py run [test ...]` (`list` shows the tests). It drives the owner's Prism
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
