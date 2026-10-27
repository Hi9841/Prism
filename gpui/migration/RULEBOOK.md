# Design document and rulebook - Prism UI: React/Tauri WebView → GPUI (Rust)

> The meta-rule: if two agents could answer a question differently, the answer goes in this file.
> Read-only inside any loop. Amendments are queued for the owner and applied between batches.

Status: DRAFT 3 (2026-10-09). Amendments A1, A2, A4-A11 applied at the Step 2 gate (A3 superseded by A11).
Owner decisions D1, D2, D4, D5 are recorded below. D3 is open.
This is a redesign of the UI host, so this file is a design document plus the rules that keep
agents consistent. The kit's bakeoff does not apply (README, "If you're redesigning").

## 0. Scope and posture

- **Goal:** the same product, pixel for pixel and behavior for behavior, without WebView2.
  The judge (`migration/judge`, 277 scenarios) defines behavior parity. Screenshots define visual parity.
- **Source of truth:** `C:/Users/hi/Desktop/Work/Prism` (`src/` for the UI, `src-tauri/` for the backend).
  It stays runnable and shippable until Step 6 is done.
- **Unit of work:** a subsystem (§5), not a file. Files inside a subsystem are translated together.
- **Referee:** `cargo check` is the in-loop typecheck (Step 4 dissolved: 1.3 s incremental). The parity judge
  (`PRISM_ADAPTER_CMD=prism-judge`) and running the real app verify behavior. **No unit tests** (owner, 2026-10-09).
- **Markers:** `TODO(port):`, `PERF(port):`, `BUG(port):` only. Every translated file ends with
  `// PORT STATUS: confidence=<high|medium|low> todos=<N>`, and N equals the TODO(port) count.
- **The BUG rule:** reproduce defective source behavior and mark it `BUG(port):`. Fixes wait for Step 6.

## 1. Architecture

```
prism-gpui (this repo, cargo workspace)
  crates/prism-ui        GPUI views, state entities, theme, motion   <- the port
  crates/prism-app       binary: window, tray, single instance, updater, Host impl
  crates/prism-judge     binary: judge adapter (JSON stdin -> JSON stdout), no window
backend (shared with the Tauri app; see DECISION D1)
  prism-core             catalog, apps, query, power, audio, taskbar, win_key, ... (no Tauri)
```

- **No IPC.** Each `src/lib/bridge.ts` function becomes a direct call on a `Backend` handle in
  `prism-ui`. Names map one to one (`launchApp` → `backend.launch_app`).
- **Threads (A7).** Blocking, thread-agnostic bridge work runs in `cx.background_spawn`; the result
  updates an entity. Window, OLE, clipboard, and foreground calls run on the UI thread. A call that enters
  a modal OS loop (`DoDragDrop`, `TrackPopupMenu`, common dialogs) runs from a foreground task
  (`cx.spawn` or `cx.spawn_in`) outside every entity update and GPUI callback, never inside an event listener.
- **Actions are data.** A row action is an enum value, not a closure:
  `enum RowAction { None, Backend(Vec<BackendCall>), Clipboard(String), TogglePin { path: String } }` with
  `BackendCall { cmd: &'static str, args: serde_json::Value }` named by the original Tauri command.
  `TogglePin` reads the pin state at click time; `Clipboard` keeps the palette open (A2). The UI executes it; the judge adapter serializes it. This is
  what makes the judge's `{calls:[{cmd,args}]}` records possible for the port.
- **State:** one GPUI entity per React provider: `AppState` (state/app.tsx) and `PaletteState`
  (state/palette.tsx). Views read them; mutations go through methods that call `cx.notify()`.
  React `useMemo` derivations become methods on the entity, cached by an explicit revision counter.
- **Command bodies (A4).** Each `#[tauri::command]` body in `Prism/src-tauri/src/lib.rs` moves into
  `prism-core` as the `Backend` method named in `migration/backend-commands.tsv`. Effects that need a window,
  the event loop, or Tauri go through the `Host` trait. The Tauri app keeps thin wrappers that call the same
  body. Show, hide, and focus (lib.rs:971-1076, :726-740), shortcut registration, tray, and window width go to
  `prism-app` through `Host`.
- **Pure logic first:** `math`, `search`, `query`, `types`, `iconLoading`, `launcherMotion`,
  `update-policy`, `update-progress`, `sections`, `phase1`, `emoji`, and the sanitizers in `state/app.tsx`
  are ported as plain Rust modules with no GPUI types, so the judge adapter can call them.

## 2. Constructs with no direct equivalent - one canonical mapping each

| Source construct | Canonical target | Notes |
|---|---|---|
| React function component | `struct` + `impl Render` for stateful views, `impl RenderOnce` for stateless rows | `ResultRow` (memo) → `RenderOnce` |
| `useState` in a component | field on the view struct | |
| React context provider | GPUI entity (`Entity<AppState>`) held by the root view | |
| `useEffect` with cleanup | `cx.observe` / `cx.subscribe` stored in a `Vec<Subscription>` field; timers are `Task`s stored in fields (dropping cancels) | never detached unless the source is fire-and-forget |
| `window.setTimeout(f, ms)` debounce | `cx.spawn` + `Timer::after(ms)`, task stored in a field, replaced on each keystroke | keeps the 35 ms file-search debounce |
| `className` (Tailwind) | `div().…` style builder calls with values from `Theme` (§3) | no magic numbers outside `theme.rs` |
| `lucide-react` icon | SVG from the lucide set, embedded as an asset, drawn with `svg().path(...)` and tinted | same icon names as the judge's `icon.name` |
| `null` / `undefined` | `Option<T>` | judge contract: None serializes as `null` for return values, omitted for optional fields |
| thrown error / rejected promise | `Result<T, String>` surfaced through the existing toast path | |
| `localeCompare(…, undefined, {numeric:true})` | `icu_collator` with the Windows UI language (first entry of `GetUserPreferredUILanguages`, the source of `navigator.language` in WebView2), numeric ordering on (D5, R12) | judge scenario `idle.sort-collation` |
| JS number | `f64` for math, `i64`/`u32` where the source only uses integers | NaN/Infinity/-0 must survive (judge canaries) |
| CSS `@keyframes` + transition classes | GPUI `with_animation` for fixed sequences; a target-driven spring for interruptible state | same durations and easings as `tokens.css` |
| `window.addEventListener("keydown", f, true)` (window-level keys) | `cx.intercept_keystrokes`, registered once by the root view, `Subscription` stored; `cx.stop_propagation()` when handled | A5. Interceptors run before action bindings; `capture_key_down` runs after them. Match `keystroke.key` (shifted OEM and digit keys arrive shifted with `shift` cleared; numpad keys as `+`, `-`, `0`). Return early while the focused input has marked (IME) text. No `capture_key_down` listener may act on a key an interceptor handles: gpui 0.2.2 still calls the first capture listener once |
| pointer drag (pointer capture + `elementFromPoint(...).closest(...)`) | not `on_drag`. `on_mouse_down` on the source starts the state; window-level `MouseMoveEvent`/`MouseUpEvent` listeners (capture phase) registered from a `canvas` paint; hit-test with bounds recorded in a `canvas` prepaint, intersected with `ListState::viewport_bounds()`; one apply per frame; the preview is an absolutely positioned child with the original offsets and clamp | A6. `on_drag` has a 2 px threshold, an unclamped grab-offset preview, and no exit animation |
| `requestAnimationFrame` (coalescing, per-frame work) | `window.on_next_frame(...)` plus `cx.notify()` | A9. Never `Window::request_animation_frame()` outside paint or prepaint: it panics in gpui 0.2.2 |
| `scrollIntoView({block: "nearest"})` | `reveal_nearest`: no scroll if `bounds_for_item(ix)` is inside `viewport_bounds()`; else `ListState::scroll_to_reveal_item(ix)` | A10. Never call `scroll_to_reveal_item` directly: it scrolls a fully visible row by up to `offset_in_item` px. Chromium does not (Step 2 headless baseline) |
| `WheelEvent.deltaY` (Chromium, Windows) | `ScrollDelta::Lines(l)` → `-l × 100/3 ÷ (zoom / 100)` CSS px; `Pixels(p)` → `-p ÷ (zoom / 100)`. With `SPI_GETWHEELSCROLLLINES == WHEEL_PAGESCROLL`, use the page branch of the source (`DOM_DELTA_PAGE`) | Chromium web_input_event_builders_win.cc:299-335 (100/3 px per line); Blink wheel_event.cc:111 divides by `devicePixelRatio`, which includes the WebView zoom (measured: 100 / 66.7 / 142.9 px at 100 / 150 / 70 %) |
| `matchMedia("(prefers-reduced-motion: reduce)")` | `!SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION)`, read at each use (about 30 µs); no cache | Measured: Chromium 154 reports `reduce` exactly when Windows "Animation effects" is off |

Gap decisions for individual sites live in `migration/inventory.tsv`. A row for a site wins over the
defaults in this section. A site listed in `migration/gap-sites.tsv` that has no inventory row is an
inventory bug: apply the UNKNOWN rule (most conservative translation plus `TODO(port):`) and flag it.
Do not invent a mapping (A1).

## 3. Visual system

- `crates/prism-ui/src/theme.rs` holds every token from `src/styles/tokens.css` (126 custom properties,
  dark and light themes, five accents). OKLCH values are converted once, at build time, to linear RGBA,
  and the visual check against screenshots of the original verifies them (no unit tests).
- Fonts: `Segoe UI Variable Text`, fallback `Segoe UI`; mono `Cascadia Code`. Font features `cv11`, `ss01`.
- Window at start (A8): 720×620 (min 480×400), transparent, no decorations, no shadow, not user-resizable,
  hidden, no taskbar entry (`WindowKind::PopUp` sets `WS_EX_TOOLWINDOW`). Width is a setting (560/640/720,
  default 640) applied after ready (inventory row 91). Always-on-top is a setting (default on): `Host` calls
  `SetWindowPos(HWND_TOPMOST or HWND_NOTOPMOST, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE)` on the GPUI HWND
  at start and on change; PopUp alone is not topmost. Show, hide, and focus go through `Host` with Prism's
  existing foreground logic. Never call `Window::activate_window`: it injects a synthetic Alt key.
  The shell is painted with the same semi-transparent gradients.
  The app sets no native material today (`set_effects(None)`, lib.rs:1120).
- Zoom (A11): every length is in rems (px / 16), including arbitrary Tailwind values and inline styles.
  View zoom sets `window.set_rem_size(16 px × zoom / 100)`; nothing else changes size with zoom. On every
  rem-size change, call `ListState::reset(item_count)` on each list: ListState keeps the old height for
  off-screen rows.
- **Known visual gap:** four surfaces use CSS `backdrop-blur` over the palette content (PowerMenu.tsx:152,
  SettingsSheet.tsx:835 and :844, Toast.tsx:17, ResultContextMenu.tsx:183). GPUI has no backdrop
  filter. Per D4 they get a near-opaque fill that matches the blurred average, logged as `BUG(port):` in the visual parity ledger.

## 4. Naming and paths

- `src/<path>.ts(x)` → `crates/prism-ui/src/<path>.rs`, file names in snake_case
  (`PaletteSearchInput.tsx` → `palette_search_input.rs`). One source file, one target file.
- `src/styles/tokens.css` → `crates/prism-ui/src/theme.rs`. `src/main.tsx` + `src/App.tsx` → `crates/prism-app/src/main.rs` and `crates/prism-ui/src/app.rs`.
- `*.test.ts(x)` are not translated and not re-expressed as Rust tests. The judge covers their behavior
  (owner, 2026-10-09: no unit tests).

## 5. Subsystems (units of work), dependency order

0. Backend (A4): the `prism-core` crate in the Prism repo (D1), its 10 Tauri-coupled modules behind `Host`,
   the `lib.rs` command bodies as `Backend` methods, the Tauri `Host` and wrappers, and the GPUI `Host` in
   `prism-app`. Listed in `migration/backend-units.tsv`; first in `manifest.tsv`. The Tauri app must still
   build and pass the judge after this subsystem.
1. Pure logic: math, search, query, types, emoji, iconLoading, launcherMotion, update-policy/progress.
2. Palette model: phase1, sections, the sanitizers (state/app.tsx), plus `prism-judge`. Gate: the judge
   passes with `PRISM_ADAPTER_CMD=prism-judge` before any view is written.
3. Theme and primitives: theme.rs, ui.tsx (Kbd, RowIcon, Toggle, Segmented, IconButton, …), Toast.
4. Palette view: PaletteSearchInput, ResultRow, Palette (incl. drag and reorder), ResultContextMenu.
5. Settings: SettingsSheet, TaskbarCustomization, PowerMenu, UpdateControl.
6. App shell: window, Win-key/shortcut toggle, launcher motion, single instance, tray, updater.

## 6. DECISIONS for the owner

- **D1 - Backend sharing. DECIDED 2026-10-09: extract `prism-core`.** Was recommended: extract a Tauri-free `prism-core` crate inside the Prism repo.
  The Tauri coupling is small outside `lib.rs`: about 64 sites in 10 modules
  (taskbar_customization 17, catalog/mod 16, win_key 5, start_menu 5, taskbar_icon_overlay 5,
  apps 4, windows_tools 4, audio_hook 4, audio 2, theme 2). They are mostly
  `AppHandle::emit` and window lookups and go behind a small `Host` trait.
  Both apps then share one backend.
  Alternatives: a path dependency on `src-tauri` (pulls Tauri into the GPUI binary), or a fork (two backends drift).
- **D2 - GPUI pin. DECIDED 2026-10-09: crates.io `gpui = "0.2.2"`.** Was recommended: crates.io `gpui = "0.2.2"` (latest published). Alternative: a Zed git
  commit (newer Windows fixes, less reproducible). The Step 2 spike decides if 0.2.2 is enough.
- **D3 - Shipping. OPEN.** Recommended: the GPUI build ships as a separate preview channel until Step 6 passes,
  then replaces the Tauri build under the same app id and updater key.
- **D4 - Backdrop blur. DECIDED 2026-10-09: option (a), near-opaque fill.** Options: (a) raise those four surfaces to an opaque or near-opaque fill that
  matches the blurred average (closest 1:1 without a blur pass); (b) write a custom blur
  shader for GPUI (large); (c) accept the difference. Recommended: (a), marked `BUG(port):`
  in the visual parity ledger.
- **D5 - Collation. DECIDED 2026-10-09: `icu_collator` with the user's locale.** Was recommended: `icu_collator` with the user's locale (the WebView uses
  `navigator.language`). Alternative: a simple case-insensitive numeric comparator that matches
  en-US for ASCII only.

## 7. Spike results and the remaining smoke items

Step 2 spike (done, `migration/stress-test/step2-report.md`): the 8 needs-spike inventory rows. Seven are
settled by measurement; the OLE file drag (row 112) needs the real mouse and moves to Step 5.

Moved to the Step 5 smoke tests (R11, decided at the Step 2 gate): they need a visible window and real
input. If one fails, stop and report before Step 6. The prism-app binary must prove on this machine: a transparent, borderless, always-on-top window with
no taskbar entry; show/hide in under 16 ms on the Win key (through `prism-core::win_key`); IME text input;
Segoe UI Variable rendering; a 1,000-row list scrolling at 60 fps; the shell gradient matching a
screenshot of the current app within a fixed tolerance; and the OLE file drag of inventory row 112 (drop on
another app, Esc cancels, a click works after the drag, and a release over the palette behaves as in the original).

## Deviation log

- 2026-10-27 (OpLoop, second pass): text and layout as Blink does them, found with
  `oploop/parity/line_shift.py` (best offset per text line) and probes in Chromium and the real WebView2.
  (1) The half-leading floors to a whole pixel, not to 1/64 px (patch 24 corrects item (4) of 2026-10-10;
  the glyph row now rounds, as Skia does). (2) Line heights floor to 1/64 px, as a LayoutUnit (patch 25).
  (3) The maximum scroll offset is in whole pixels (patch 26). (4) Kerning was off for all text: GPUI's
  per-run `IDWriteTypography` replaced DirectWrite's defaults without `kern` (patch 27). (5) The thin
  scrollbar gutter is 10 px, as WebView2 155 reserves it (read from the real Tauri build with a
  read-only CDP probe, `oploop/bench/webview_probe.py`); the port reserved 11. (6) The parity harness
  emulates WebView2 zoom with CSS `zoom` on the root at scale 1.0, not with the device scale: WebView2
  keeps scrollbars at 10 device px at 80-120%, and the device scale scaled them. (7) The Start-icon
  tiles keep the 24 px icon box (CSS grid rows do not shrink), and the Gem and Diamond SVGs draw as
  masks at their painted size. UI parity mean 9.767 -> 9.902 (9.913 without the two menu captures).
- 2026-10-27: Parity audit after release 1.0.10. (1) The original changed in 1.0.4 (app rows drop the
  "Application" subtitle), so 33 goldens failed against the original and the port alike. The port output
  was identical to the original on all 33. Goldens were captured again from the original: judge 277/277
  on both. (2) `mutate.mjs` normalizes CRLF in the copy (`core.autocrlf` checks out CRLF), so the two
  multi-line mutations apply again: 26/26 caught. (3) The manifest pointed at the old `../Prism-core`
  worktree; it now points at `crates/prism-core`. `src/lib/transientUi.ts` maps to
  `BackendEvent::DismissTransientUi` in `lib/bridge.rs`, and the Tauri host maps to `src-tauri/src/lib.rs`.
  (4) Port-only changes after 1.0.0, made on purpose and not in the original: shared popover menu look,
  32 px rows, neutral icons, red Shut down highlight, semibold Open (`eb6b035`); the search field draws no
  ring or caret until the first input (`98b30e0`); a click away within 600 ms of open closes the palette
  (`5615f88`); one launch at a time, gate released after the hide and after a power plan switch
  (`ea6b75f`, `3d0e136`, `18252d0`); menus occlude and paint above the footer (`6f91552`); the updater stays
  running when the installer cannot start (`32e24d7`).
  (5) Owner decision: `gpui/` is self-contained. `prism-core` moved from the repo root `crates/` into
  `gpui/crates/` as a workspace member (D1 still holds: one backend, and `src-tauri` uses it from here).
  Its own `Cargo.lock` was removed; `gpui/Cargo.lock` covers it. Installer signing uses the Tauri CLI
  from `gpui/package.json` instead of the root `node_modules`.
  (6) Spec commit `159dc17`: a failed index rebuild clears `file_indexing`, and the errors that the
  original now logs are logged with `eprintln!` at the same port sites.
  (7) Functional audit, fixed: Ctrl+Shift+K and Ctrl+Shift+W do nothing, as in the original (`event.key`
  is upper case with Shift). Escape in the power menu returns focus to the trigger button (the port
  blurred the window).
  (8) Platform limit, open: gpui 0.2.2 has no UI Automation provider. Screen readers get no names, roles,
  or states (`aria-label`, `role="menu"`, `aria-expanded`, `aria-activedescendant`). The visible
  tooltips are drawn (2026-10-10 entry, item 6). Closing this gap needs a UIA (or AccessKit) layer in gpui.

- 2026-10-11: Owner request: every result has a right-click menu. "Open" is the first entry, and
  "Copy as path" copies the quoted path (file, folder, app exe or shortcut, `shell:AppsFolder\{AUMID}`,
  settings URI). UX scenario `ux-context-keys` now differs from the original on purpose: Down+Enter
  picks the second entry, which is "Open location" in the port and "Properties" in the original.
- 2026-10-10: Real-app test on the owner's desktop (owner: "test for me"), with `SendInput` and clicks.
  Findings and fixes: (1) `prism-gpui` has an inherited Low Mandatory Level label (a Codex sandbox ACL), so
  `target/debug/prism.exe` starts at Low integrity: the catalog opens read-only and the Win-key observer
  and the Explorer bridge fail. The ACL was not changed; the port runs from a copy in
  `%LOCALAPPDATA%\PrismGpui`. (2) The second open stayed invisible: GPUI dropped next-frame callbacks on a
  re-entrant frame (vendored patch 6). (3) The focus ring filled transparent buttons (patch 7). (4) The
  shell treated a background WM_ACTIVATE as foreground and stopped the raise retries, so a Win open from
  Chrome left focus in Chrome; the shell now checks the real foreground. One stray "notepad" + Enter went
  to the owner's terminal before the input script checked the foreground; the script now refuses input
  unless the palette is in the foreground.
- 2026-10-10: Step 3 subsystems 3-6 written inline. Deviations, each measured with the new visual parity
  harness (`migration/parity`: the original in headless Chrome 154 with a mocked Tauri layer vs the port in a
  parked, never-activated window, same fixture, PrintWindow captures, SSIM-based score):
  (1) gpui 0.2.2 is vendored (`vendor/gpui`, patches in `PRISM-PATCHES.md`): fractional line heights as
  in Chromium. (2) The palette list is a scroll `div` with recorded row bounds, not `ListState`; A10's
  reveal-nearest rule is kept on those bounds. Rows are few (the original does not virtualize either).
  (3) Wheel scrolling uses Chromium's 100/3 px per line with a short animation (`gfx::WheelScroll`).
  (4) `text-wrap: pretty` is reproduced by a measured rule (`components/pretty_text.rs`, 2,320/2,320 cases).
  (5) The palette window removes GPUI's popup client insets with a window subclass and runs native
  move/resize outside GPUI updates.
- 2026-10-10: UX parity pass (`bun migration/parity/run.mjs --ux`, 37 scenarios: recorded backend calls
  must match and each capture must score >= 9.4). Found by measuring the original in Chromium:
  (1) Inherited: the row icon never starts a reorder or file drag. The full-row open button paints
  above it (positioned z-0 inside the z-[1] cell), so Chromium routes the press to the button. The port
  drags from the grips only. (2) The settings panel's `backdrop-blur-2xl` blurs nothing: the animated
  `.settings-backdrop` is the backdrop root, so the panel paints the backdrop fill a second time and the
  palette shows through, unblurred. The port composes `bg-raised` over the backdrop fill. (3) CSS box
  shadows blur with sigma = blur / 2 (GPUI takes the sigma); a 0 blur gets a sub-pixel sigma because
  GPUI's shader divides by it. (4) Blink's baseline: rounded font ascent and descent, half-leading
  floored to 1/64 px (vendored patch 2). (5) Keyboard focus as in WebView2 (`components/focus.rs`):
  buttons are tab stops, Tab past the end rests on the body, Enter and Space click, `:focus-visible`
  rings after keyboard input, the settings focus trap, roving segmented focus, Blink's
  center-if-needed reveal, and scroll offsets on whole pixels (vendored patches 3-5). (6) `title`
  tooltips are drawn like the Windows 11 tooltip control (headless Chrome cannot capture the native
  one, so parity mode hides them).

- 2026-10-09: Step 0b reviewers and Step 1 map reviewers ran on Opus per the model plan. No deviation.
- 2026-10-09: Step 3 runs inline on Opus, with no subagents and no unit tests (owner: "dont use subagents
  we dont have enough usage", "dont do unit testings"). The kit's 4-agent pipeline per unit is replaced by
  one inline translation plus an inline adversarial self-review per subsystem, `cargo check`, the judge,
  and running the app. The build, judge, and commit denies in `.claude/settings.json` are lifted for this
  session; the destructive git denies stay. The prism-core extraction runs in a git worktree of Prism
  (`../Prism-core`, branch `gpui/prism-core`) so the owner's dirty Prism checkout stays untouched.
- 2026-10-09: Step 2 gate acts (apply A1, A2, A4-A11; apply the inventory proposals; decide R11 and R12;
  install `.claude/settings.json`; commit) were done by the agent on the owner's explicit instruction
  ("do everything"). The kit assigns these acts to the owner. D3 stays open: it is a product decision.
- 2026-10-09: Step 2 spike, runs 1-2: synthetic input (`SendInput`) and a topmost window while the owner used
  the machine. Windows blocked the input. Later runs posted messages to the spike's own window only.
- 2026-10-09: Step 1 gap inventory, first 20 rows: the two skeptic reviewers were stopped at the owner's request (fewer subagents). One in-context review replaced them; it revised 3 rows (runItem evidence, prism:close broadcast, context-menu clamp). Owner instruction: review inline by default and ask before spawning.
