# Journeys: Prism GPUI against the original (React/Tauri)

> After release 1.0.11 the original React/Tauri app left the repo. The journeys stay the same; the bench now
> compares GPUI builds (`bench.py --build NAME=EXE`, `compare.py`). The Tauri rows in `results/` are the last
> measurements of the original. J5 (search rows against the original) has no reference any more.

These journeys measure the real Prism launcher on this Windows machine.
Each journey runs on two release builds of the same source tree:

- **GPUI**: `crates/prism-app` (`prism.exe`, native GPUI user interface).
- **Tauri**: the repo root (`src/` React in WebView2, `src-tauri/` host). This is the original app.

The two builds use the same backend (`crates/prism-core`) and the same app data (settings and file catalog).
The bench starts each build from a copy in `%LOCALAPPDATA%\PrismGpuiE2E\<app>\prism.exe`, with `--autostart` (hidden start).
Only one Prism runs at a time. The commands are in `bench/README.md`.

## Common rules

### Input

- **Input.** The bench sends input only to the palette with `PostMessage`. It sends no global input.
  The keys are `WM_CHAR` for text and `WM_KEYDOWN`/`WM_KEYUP` for Escape. The bench never sends Enter.
  - GPUI: the target is the palette window (class `Zed::Window`).
  - Tauri: the target is the WebView2 render widget (class `Chrome_RenderWidgetHostHWND`, a child of the `Tauri Window`).
- **Activation.** The bench uses the second-instance signal of each app, as a second launch sends it:
  - GPUI: `SetEvent` on `Local\PrismGpuiActivate`.
  - Tauri: `WM_COPYDATA` (tag 1542) to the `app.prism.launcher-siw` window of `tauri-plugin-single-instance` 2.4.
### Frames and time

- **Frames.** The bench reads the palette with `PrintWindow(PW_RENDERFULLCONTENT)` about every 7 ms.
  This is the content that DWM has for the window. Windows behind the palette and the mouse pointer do not change it.
  A hidden palette reads as a blank (all zero) frame.
- **Change.** A frame changed when pixels changed by more than 24 levels and the changed columns are 8 px wide or more.
  The rule ignores the text caret, which blinks (1 px wide). The bench compares frames at half resolution.
- **Resolution.** The capture interval is the time resolution: about 7 ms.
- **Clock.** All times are `QueryPerformanceCounter` time in the bench process.
### Sessions

- **Backend share.** Each build writes its own time events to `PRISM_PERF_LOG`.
  The bench keeps the events inside each measurement (`palette_toggle`, `palette_present`, `query_phase1`, `search_files`).
- **Order.** The bench runs blocks in the order GPUI, Tauri, GPUI, Tauri (4 blocks for each app).
  The spread of the block medians shows the drift between sessions.
- **Readiness.** After a start, the bench waits at least 20 s. Then it waits until the process tree uses less than 3% of one core for 3 s.

## J2: open latency

The time from the activation signal to the painted palette.

| Point | Definition |
|---|---|
| Start | Immediately before the bench sends the activation signal. |
| Window visible | The first poll where `IsWindowVisible` is true and DWM does not cloak the window. |
| First painted frame | The first `PrintWindow` frame that is not blank. This is the first frame of the open animation. |
| Settled frame | The last frame that is different from the frame before it, when no change follows for 700 ms. The open animation and late rows are complete at this point. |

- Each run starts with a hidden palette and an empty query.
- After each run, the bench sends Escape and waits until the window is hidden, plus 600 ms.
- Run 1 of each session is the first open after the start ("first open after start"). The other runs are "warm".
- The bench also records if the palette window has the foreground after the open.
- Runs: 20 for each app (5 in each of 4 sessions). 4 of the 20 runs are first opens.

## J3: type latency

The time from the posted keys to the painted result rows, for each query in the fixed list.

| Point | Definition |
|---|---|
| Start | Immediately before the bench posts the `WM_CHAR` messages of the query. The bench posts all characters of the query at the same time, as a fast paste. |
| First changed frame | The first frame that is different from the reference frame (the palette with an empty query). Usually the input box shows the query in this frame. |
| Rows done | The last change before the first quiet gap of 300 ms (`report.py`, from the change list). The result rows are complete at this point. A late change after the gap (for example a thumbnail that paints again after some seconds) does not move this point. This is the main J3 measure. |
| Settled frame | The last frame that is different from the frame before it, when no change follows for 1000 ms (limit 5 s). Late changes move this point. |

- The palette stays open. Before each query, the query is empty and the frame does not change for 400 ms.
- After each query, the bench sends one Escape. Escape clears the query and does not close the palette.
- Queries (13): `c`, `ch`, `chrome`, `notepad`, `uac`, `settings`, `calc`, `2+2*3`, `crhome` (typo),
  `HyperType-snippets.json`, `apply_rose_pine_moon_accent.ps1`, `problem solving practice.excalidraw`
  (three files in `%USERPROFILE%\Documents`), and `C:\Windows\` (path).
- Reps: 20 for each query and each app (5 in each of 4 sessions).

## J4: resources

The resource use of the full process tree. For Tauri, the tree includes the WebView2 processes (`msedgewebview2.exe`).

| Point | Definition |
|---|---|
| After start | 60 s after the start, hidden, no input. |
| Hidden idle | 60 s more, hidden, no input. The CPU time of this period is the idle CPU. |
| After cycles | 20 cycles of: open, wait until the frame does not change for 300 ms, type `chrome`, wait 500 ms, Escape until hidden. Then 60 s hidden idle. |

- Measures: private bytes (`PrivateUsage`), working set, handle count, thread count, process count, CPU time (user + kernel).
- A leak shows as private bytes or handles that increase across the 20 cycles.
- Runs: 3 for each app, in the order GPUI, Tauri.

## J5: search correctness

The visible result rows of each J3 query, in the two apps.

- The bench keeps the settled frame of the first 2 reps of each query in each session (8 frames for each app and query).
- `report.py` finds the text boxes in the union of all the frames of one query. Then it reads the same box in each frame with Windows OCR (`Windows.Media.Ocr`).
- J5 does not compare the search box (the top 80 px). The search box only shows the query, and J3 measures it. The blinking caret in it makes OCR errors.
- For each box, it compares the GPUI text with the Tauri text:
  - `same`: equal after normalization (case, spaces, ellipsis).
  - `ocr-noise`: 90% similar or more. OCR reads the same text in two different ways.
  - `truncated`: one text is the start of the other. The two apps cut long text at a different point.
  - `different`, `only gpui`, `only tauri`: the text is not the same.
- `report.py` also writes `frames/pair-qNN.png` (GPUI left, Tauri right) for each query.
- OCR can make errors. A person must examine each reported difference on the side-by-side PNG. The results file gives only the differences that a person examined.

## Limits

- The machine is in use. Other processes (for example other agents, builds, and parity runs) add noise. Each row records the machine CPU load before the measurement.
- The apps share the owner's app data. Dynamic sections (open windows, recent items) can change between sessions.
- The GPUI palette did not take the foreground on this activation path (see the results). Typed keys still arrive, because the bench posts them to the window.
- PrintWindow sees the frame that DWM has. It does not see the time that the monitor shows the frame (no display latency).
