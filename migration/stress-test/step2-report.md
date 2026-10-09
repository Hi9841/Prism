# Step 2 report: adversarial review of RULEBOOK.md and the GPUI spike

Date: 2026-10-09. Model: claude-opus-5-5, inline. Subagents: 0.
Posture: redesign (RULEBOOK §0). The bakeoff does not apply. This step replaces it with
an adversarial review of the design document and a throwaway spike (README, "If you're redesigning").

Gate status: **Step 2 gate passed (2026-10-09).** On the owner's instruction ("do everything"), the agent did
the gate acts: A1, A2, A4–A11 applied (A3 superseded by A11), the inventory proposals applied, R11 and R12
decided, M2–M4 run headless, `.claude/settings.json` installed, and the work committed. See section 7.
Sections 1–6 are the report as written at the gate; section 7 has the later results.

## 1. Burndown

| Item | Count |
|---|---|
| Spike rows in `inventory.tsv` (needs-spike or assumption) | 8 |
| Settled by measurement on this machine | 5 (rows 19, 92, 110, 111, 116) |
| Settled except one manual check | 2 (row 9: wheel px per line; row 12: Chromium mapping) |
| Not run: needs the real mouse button | 1 (row 112, OLE drag) |
| Rulebook review findings | 12 (4 high, 5 medium, 3 low) |
| Amendments queued | 8 (A4–A11); A1–A3 still wait for the owner |
| Manual checks for the owner | 4 (M1–M4, section 5) |

## 2. Incident: synthetic input while the owner played a game

The first two spike runs used `SendInput` and opened a topmost window for about 15 s each.
The foreground app was Fortnite. Windows blocked the injected input (the cursor did not move), and
the spike window did not get the foreground. After I found this, the spike changed method:
it posts messages to its own HWND only, it sets only its own thread's key state, and it never
activates its window. Runs 3–10 used this method. No global input was sent after run 2.

## 3. Spike results, row by row

Spike: `spike/step2` (throwaway, untracked). `gpui = "=0.2.2"`, `windows = "0.61"`.
Evidence: `spike/step2/tmp/run8.jsonl` (full run), `stab1-3.jsonl` (stability), `run10.jsonl` (rem/ListState).
Input path: messages posted to the spike's HWND go through GPUI's real window procedure.
GPUI reads modifiers with `GetKeyState` (events.rs:1459), so the spike sets this thread's key state.

### Row 9: Ctrl zoom keys and Ctrl+wheel. Settled, except the px-per-line constant (M3).

- **Keys.** Use `cx.intercept_keystrokes` on the root view. It is the only hook that runs before
  action bindings (window.rs:3779, `dispatch_keystroke_interceptors`). `capture_key_down` runs
  **after** bindings (window.rs:3834 then :3878), so the inventory's "capture phase" wording is wrong for GPUI.
- 13 chords tested. Runs 2 and 3 of the stability set are identical and match app.tsx:291-310:
  Ctrl+Up/=/+/NumpadAdd zoom in; Ctrl+Down/-/_/NumpadSubtract zoom out; Ctrl+0 and Ctrl+Numpad0 reset;
  Ctrl+Alt+Up is ignored; plain Up/Down still reach the list's actions. Ctrl+Up does not move the selection.
- GPUI key names: shifted OEM keys arrive as `"+"` and `"_"` with `shift` cleared (keyboard.rs:187);
  numpad keys arrive as `"+"`, `"-"`, `"0"`. Match on `key` only.
- **Quirk (GPUI 0.2.2):** after an interceptor stops propagation, GPUI still calls the first
  capture-phase key listener on the dispatch path once (window.rs:3885-3892: the listener runs, then the check).
  Seen in every intercepted chord. Rule: no `capture_key_down` listener acts on a zoom chord.
- **IME (code read, not run):** GPUI converts `VK_PROCESSKEY` into the real key (events.rs:1345),
  so the interceptor sees keys during composition. The original ignores them (`isComposing`, app.tsx:293).
  The interceptor must return early while the search input has marked text.
- **Wheel.** A window-level `on_mouse_event::<ScrollWheelEvent>` in the capture phase, with
  `cx.stop_propagation()` when Ctrl is down, zooms and the list does not scroll. Without Ctrl the list scrolls.
  Accumulation, the sign-change reset, and the 180 ms reset work.
- **Wrong inventory translation:** "line delta ×16" copies the `DOM_DELTA_LINE` branch of app.tsx:319.
  Chromium on Windows reports wheel deltas in pixels, so that branch does not run in the original.
  GPUI reports `ScrollDelta::Lines(notches × wheel_lines)` (events.rs:548). Proposed:
  `px = -lines × 100/3`. This is Chromium's pixels-per-line value, from memory, **not measured here** (M3).
  With the default 3 lines per notch, both formulas give 1 zoom step per notch. They differ with a
  1-line setting or a high-resolution wheel.

### Row 12: reduced motion. Settled for the GPUI side; the Chromium mapping needs M2.

- `SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION)` costs 22–44 µs. Read it at each use (in render
  and before the width animation). No cache and no WM_SETTINGCHANGE hook are necessary. This is also how the
  original behaves: app.tsx:510 calls `matchMedia` at the time of use, and CSS media rules apply live.
- Current value on this machine: animations on (`reduced_motion = false`).
- Not proven: that WebView2 maps `prefers-reduced-motion` to this setting. M2 checks it.

### Rows 19, 110, 111: reorder drag and category drag. Settled. The target translation changes.

GPUI `on_drag` cannot reproduce the original:
- threshold 2 px (div.rs:47, `DRAG_THRESHOLD`) against 4 px (Palette.tsx:210, :456, :592);
- the preview sits at the grab offset and is not clamped (the original: pointer −246/−27 px for rows,
  +14/−24 px for headers, clamped 8 px inside the window, Palette.tsx:1203-1219, :1245-1255);
- the preview disappears at the drop; the original animates it out for 110 ms (Palette.tsx:487-499).

Proven pointer model (all probes pass; identical in 3 of 3 stability runs):
- `on_mouse_down` on the row starts the drag state. Window-level `on_mouse_event` listeners for
  `MouseMoveEvent` and `MouseUpEvent` (capture phase) are registered from a `canvas` paint callback.
  GPUI holds `SetCapture` while the button is down (events.rs:451), so moves outside the window arrive:
  the pointer at (−300, −300) was tracked and the preview stayed clamped at (8, 8).
- Hit test: each row records its bounds in a `canvas` prepaint. The bounds are intersected with
  `ListState::viewport_bounds()`. This replaces `elementFromPoint(...).closest(...)`.
- One apply per frame: `window.on_next_frame` plus `cx.notify()`. 11 posted moves gave 2 applies.
- Below 4 px the drag stays inactive. Same-group targets only for rows; header targets only for headers.
- The release after an active drag does not activate the row; the next plain click does.
- The 110 ms exit preview is present at 40 ms and gone at 190 ms. Escape cancels with no exit animation,
  as in the original (`cancelOnEscape` sets the state to null, Palette.tsx:672-678).
- Auto-scroll near the list edges: the original has none (no scroll code in the drag paths). The port adds none.
- **GPUI 0.2.2 trap:** `Window::request_animation_frame()` panics outside paint or prepaint
  (window.rs:3362, `debug_assert_paint_or_prepaint`). The spike crashed on it from a mouse handler.

### Row 92: always on top. Settled.

- `WindowKind::PopUp` sets `WS_EX_TOOLWINDOW` (no taskbar entry) but **not** `WS_EX_TOPMOST`
  (platform/windows/window.rs:394).
- `SetWindowPos(HWND_TOPMOST | HWND_NOTOPMOST, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE)` on the GPUI HWND
  works at run time, and the state survives hide and show. Get the HWND through
  `raw_window_handle::HasWindowHandle` (gpui window.rs:4845). Run it through `Host` at start (default
  `alwaysOnTop: true`, types.ts:313) and on change.
- Related facts for the app shell (not one of the 8 rows): GPUI 0.2.2 has no API to show a hidden window.
  `activate_window` does not show it; `ShowWindow` on the HWND does. `activate_window` also injects a
  synthetic Alt key with `SendInput` (platform/windows/window.rs:707-739). Prism already has its own
  foreground logic (lib.rs:726-740). The port must use that logic through `Host`, not `activate_window`.
- Observation, not investigated: a window requested at (−4000, −4000) with 720×620 opened at (184, 104)
  with 1552×873 physical pixels.

### Row 112: file drag to other apps (OLE). Not run. Needs M1.

- `DoDragDrop` reads the real mouse button, so posted messages cannot drive it. A run with the real cursor
  would capture the mouse during the owner's game. I did not run it.
- Ready in the spike: the threshold starts `DoDragDrop` from a foreground task (`cx.spawn_in`), outside every
  entity update and every GPUI callback. `--ole-inline` runs the anti-pattern (inside the move callback) for comparison.
- Gap in the row: after a "dropped" result, the original hides the palette (lib.rs:1351-1356). The row does not say this.
- Risk to check: GPUI registers every window as an OLE drop target and answers `DROPEFFECT_COPY` for files
  (platform/windows/window.rs:1243, :899). A release over the palette itself then reports "dropped" and the
  palette hides. M1 compares this with the original.

### Row 116: keep the selection in view. Settled. Needs a wrapper.

- `ListState::scroll_to_reveal_item` moved a **fully visible** row by 1 px in every run (start
  `{item 2, 5 px}`, after `{item 2, 4 px}`). Cause: list.rs:376-386 compares item indexes, not pixels.
  `scrollIntoView({block: "nearest"})` does not scroll a visible row.
- Wrapper (proven): if `bounds_for_item(ix)` is inside `viewport_bounds()`, do nothing; else call
  `scroll_to_reveal_item(ix)`. Below the viewport the bottom edges align (gap 0 px); above, the top edges align.

### Also measured

- **A3 (queued):** `window.set_rem_size(16 px × zoom / 100)` scales rem lengths (row 44 px → 66 px at 150 %).
  But `ListState` keeps the old height for off-screen rows (44 px at 150 %) until `ListState::reset`
  (66 px after the reset). A3 is incomplete; A11 supersedes it.
- **Referee price:** cold `cargo build` of gpui 0.2.2 = 87 s. With a 1,244-line crate on gpui:
  incremental `cargo check` = 1.3 s, incremental debug build = 5.9 s. The typecheck is cheap, so
  Step 4 can dissolve into Step 3. That is the owner's edit to `.claude/settings.json` (README, Step 4).

## 4. Adversarial review of RULEBOOK.md (draft 2)

Each finding cites the rule and the evidence. Severity: high = agents would produce wrong code or miss work.

| # | Sev. | Section | Finding | Evidence | Proposal |
|---|---|---|---|---|---|
| R1 | high | §5, manifest | No unit covers `Prism/src-tauri/src/lib.rs` (2,610 lines, 44 `#[tauri::command]` bodies, show/hide/focus, shortcut, tray, width) or the `prism-core` extraction (D1). The 30 manifest units are UI files only. Behavior such as hide-after-drop (lib.rs:1351) and focus retries (lib.rs:726-740) has no owner. | manifest.tsv (31 lines); lib.rs | A4 |
| R2 | high | §2 | No mapping for window-level key handling. Agents would use `capture_key_down`, which runs after action bindings in GPUI. | window.rs:3779, :3834, :3878; spike row 9 | A5 |
| R3 | high | §2, inventory 19/110/111 | No mapping for pointer drags. The inventory chose `on_drag`, which breaks the threshold, the preview position, and the exit animation. | div.rs:47; Palette.tsx:1203-1255, :487-499; spike | A6 + inventory proposals |
| R4 | high | §1 | "Async bridge calls become `cx.background_spawn` tasks" is wrong for thread-affine calls: OLE drag (lib.rs:1343 runs it on the main thread), window operations, foreground logic. No rule forbids modal OS loops inside GPUI callbacks. | lib.rs:1343; drag.rs:200 | A7 |
| R5 | medium | §3 | The window bullet lists start-up values as fixed properties. Width is a setting (560/640/720, default 640, animated 240 ms; inventory row 91). Always-on-top is a setting (default on). GPUI facts are missing: PopUp is not topmost; no show API; `activate_window` injects Alt. | tauri.conf.json:21-35; types.ts:275, :310-313; spike | A8 |
| R6 | medium | §2 | No mapping for `requestAnimationFrame`. The obvious GPUI call, `Window::request_animation_frame`, panics in event handlers. | window.rs:3362; spike crash | A9 |
| R7 | medium | A3 (queued) | Zoom through the rem size leaves stale off-screen list heights. | spike run10 | A11 supersedes A3 |
| R8 | medium | inventory row 9 | "line delta ×16" is the dead `DOM_DELTA_LINE` branch. | app.tsx:319; events.rs:548 | inventory proposal; M3 |
| R9 | medium | inventory row 112 | Hide-after-drop is missing; drop-on-self behavior is unknown. | lib.rs:1351; GPUI drop target | inventory proposal; M1 |
| R10 | low | §2 | No mapping for `scrollIntoView({block: "nearest"})`. | list.rs:376-386; spike | A10 |
| R11 | low | §7 | §7 lists other spike items (Win key under 16 ms, IME, Segoe UI Variable, 1,000 rows at 60 fps, gradient match) than the inventory's 8 rows. This step did the 8 rows only. The §7 items stay open. | RULEBOOK §7; inventory | Owner: keep §7 as the remaining Step 2 scope, or move it to Step 5 |
| R12 | low | §2, D5 | The WebView sorts with `navigator.language` (the UI language). §2 says "the user's Windows locale" (often the regional format). These can differ. | RULEBOOK §2 collation row | Owner decision; M2 also logs `navigator.language` |

Process items for the gate:
- `.claude/settings.json` is **not installed**. Prompts 03 and 04 require it before any fan-out.
  Installing it is the owner's act; agents never install it.
- D3 (shipping) is still open. It decides the app id and updater key that prism-app uses.
- A1–A3 still wait for the owner.

## 5. Manual checks for the owner (when the machine is free)

Run from `C:\Users\hi\Desktop\Work\prism-gpui\spike\step2`. Set TMP and TEMP to `tmp` first.

- **M1 — OLE drag (row 112).** Run `target\debug\gpui-spike.exe`. Drag a FILES row out of the window and
  release it over Notepad: Notepad must open `drag-me.txt`. Drag again and press Esc. Drag again and release over
  the spike window. After each drag, click a row: a `click` event must appear in the panel. Read the
  `ole-done` lines (result, frames during the drag). Optional: run with `--ole-inline` to see the anti-pattern.
  In the original Prism, drag a file result and release it over the palette: record whether the palette hides.
- **M2 — reduced motion (row 12).** Run `target\debug\gpui-spike.exe chrome-probe` and `target\debug\gpui-spike.exe`
  side by side. Turn Settings > Accessibility > Visual effects > Animation effects off, then on.
  Both reduced-motion lines must change together.
- **M3 — wheel pixels (row 9).** In the chrome-probe window, hold Ctrl and turn the wheel one notch.
  Expected `deltaY` = ±100 and `deltaMode` = 0 with 3 lines per notch. This confirms 100/3 px per line.
- **M4 — key values.** In the chrome-probe window, press the chords listed on the page. Compare `key` with
  section 3 (row 9).

Result: M2, M3, and M4 ran headless (section 7). M1 moved to the Step 5 smoke list (RULEBOOK §7).
`spike/` is deleted; its source and logs are archived in `migration/stress-test/evidence/`.

## 6. Next step estimate

Translation fan-out (Step 3) cannot start yet: A4 adds work units (lib.rs, prism-core), and
`.claude/settings.json` is missing. Estimate after A4: 30 UI units + about 6 backend units (prism-core
extraction, lib.rs command bodies, window/focus Host, tray, shortcut, updater) = about 36 units.
Per-unit timings do not exist yet (no pilot ran), so a wall-clock estimate is not given.
Active attention for the owner: about 30 min to apply A1–A11 and the inventory proposals, install
settings.json, and run M1–M4.

## 7. Gate acts and headless results (after the gate)

No window opened and no input went to the desktop. Chrome 154 (the same Chromium as WebView2 154) ran headless.
Evidence: `migration/stress-test/evidence/`.

### Headless Chromium baselines

| Check | Result | Port rule |
|---|---|---|
| Row 116, `scrollIntoView({block: "nearest"})` | The same edge case as the spike (scroll top = item 2 + 29 px, row 15 fully visible, 1 px gap): Chromium does not scroll. Below: the bottom edges align. Above: the top edges align. | `reveal_nearest` (A10) gives the same result; raw `scroll_to_reveal_item` does not. **1:1 with the wrapper.** |
| M2, reduced motion | Animation effects off → `prefers-reduced-motion: reduce` true; on → false. The setting changed in memory only (no broadcast, no registry write) and was restored to on. | `!SPI_GETCLIENTAREAANIMATION`. **1:1.** |
| M3, wheel pixels | Chromium source: `ticks × wheel_lines × 100/3` px (web_input_event_builders_win.cc:299-335). Blink divides by `devicePixelRatio` (wheel_event.cc:111). Measured over CDP: 100 px reads 100 / 66.7 / 142.9 at zoom 100 / 150 / 70 %. Prism's zoom is WebView zoom (`webview.setZoom`, bridge.ts:118). | `-lines × 100/3 ÷ (zoom / 100)` (RULEBOOK §2). The spike with this formula: 6/6 wheel cases as the original rules, 2 identical runs (`final1-2.jsonl`). |
| M4, key values | Chromium source: the key is the character with the full modifiers; if Ctrl makes it non-printable, Ctrl is dropped and Shift stays (platform_key_map_win.cc:301-334). GPUI shifts OEM and digit keys the same way (keyboard.rs:187, :209-235). | 14/14 chords as the original, including Ctrl+Shift+0 → `)`, which is not a zoom key in either (`final1-2.jsonl`). |
| R12, locale | `navigator.language` = en-US; Windows UI language = en-US; regional format = en-US. | `icu_collator` with the Windows UI language (RULEBOOK §2). Same on this machine. |

### Defect found and fixed in the Step 1 tooling

`migration/manifest.tsv` had CRLF line ends (git `core.autocrlf=true` converts on checkout).
`queue_runner.mjs` rejects a CRLF header ("manifest must have header source\ttarget"), so Step 3 would have
stopped at its first command. Fix: `make-manifest.sh` writes LF, and `.gitattributes` keeps `*.tsv` and `*.sh` LF.

### Gate acts

- Rulebook: DRAFT 3. A1, A2, A4–A11 applied; A3 superseded by A11. Statuses in `rulebook-amendments.tsv`.
- R11: the §7 items (Win-key latency, IME, fonts, 60 fps list, gradient match) and the OLE drag (row 112)
  are Step 5 smoke tests. They need a visible window and real input.
- R12: collation locale = the Windows UI language.
- D3 (shipping): **still open.** It is a product decision for the owner; nothing before Step 5 depends on it.
- Inventory: 8 rows updated; 7 settled, row 112 = needs-step5. Coverage 1428/1428. No needs-spike rows remain.
- Manifest: 44 units = 14 backend units (`migration/backend-units.tsv`, A4) + 30 UI units (unchanged).
  `queue_runner.mjs status`: 0/44 done.
- Judge on the original: 277/277 pass.
- `.claude/settings.json`: installed from the kit template. The `cargo check` deny is removed (Step 4
  dissolves into Step 3: an incremental check takes 1.3 s). `bun test` and the judge are denied in loops.
