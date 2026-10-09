# Design document and rulebook — Prism UI: React/Tauri WebView → GPUI (Rust)

> The meta-rule: if two agents could answer a question differently, the answer goes in this file.
> Read-only inside any loop. Amendments are queued for the owner and applied between batches.

Status: DRAFT 1 (2026-10-09). Sections marked **DECISION** need the owner's answer before Step 2.
This is a redesign of the UI host, so this file is a design document plus the rules that keep
agents consistent. The kit's bakeoff does not apply (README, "If you're redesigning").

## 0. Scope and posture

- **Goal:** the same product, pixel for pixel and behavior for behavior, without WebView2.
  The judge (`migration/judge`, 277 scenarios) defines behavior parity. Screenshots define visual parity.
- **Source of truth:** `C:/Users/hi/Desktop/Work/Prism` (`src/` for the UI, `src-tauri/` for the backend).
  It stays runnable and shippable until Step 6 is done.
- **Unit of work:** a subsystem (§5), not a file. Files inside a subsystem are translated together.
- **Referee:** `cargo check -p prism-ui` is the in-loop typecheck. Full builds run only in Step 4.
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
  `prism-ui`. Names map one to one (`launchApp` → `backend.launch_app`). Async bridge calls become
  `cx.background_spawn` tasks whose results update an entity.
- **Actions are data.** A row action is an enum value, not a closure:
  `enum RowAction { Backend(Vec<BackendCall>), ... }` with `BackendCall { cmd: &'static str, args: serde_json::Value }`
  named by the original Tauri command. The UI executes it; the judge adapter serializes it. This is
  what makes the judge's `{calls:[{cmd,args}]}` records possible for the port.
- **State:** one GPUI entity per React provider: `AppState` (state/app.tsx) and `PaletteState`
  (state/palette.tsx). Views read them; mutations go through methods that call `cx.notify()`.
  React `useMemo` derivations become methods on the entity, cached by an explicit revision counter.
- **Pure logic first:** `math`, `search`, `query`, `types`, `iconLoading`, `launcherMotion`,
  `update-policy`, `update-progress`, `sections`, `phase1`, `emoji`, and the sanitizers in `state/app.tsx`
  are ported as plain Rust modules with no GPUI types, so the judge adapter can call them.

## 2. Constructs with no direct equivalent — one canonical mapping each

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
| `localeCompare(…, undefined, {numeric:true})` | ICU-equivalent collation (DECISION D5) | judge scenario `idle.sort-collation` |
| JS number | `f64` for math, `i64`/`u32` where the source only uses integers | NaN/Infinity/-0 must survive (judge canaries) |
| CSS `@keyframes` + transition classes | GPUI `with_animation` for fixed sequences; a target-driven spring for interruptible state | same durations and easings as `tokens.css` |

## 3. Visual system

- `crates/prism-ui/src/theme.rs` holds every token from `src/styles/tokens.css` (126 custom properties,
  dark and light themes, five accents). OKLCH values are converted once, at build time, to linear RGBA,
  and a unit test pins each conversion.
- Fonts: `Segoe UI Variable Text`, fallback `Segoe UI`; mono `Cascadia Code`. Font features `cv11`, `ss01`.
- Window: 720×620 (min 480×400), transparent, no decorations, no shadow, not resizable, always on top,
  no taskbar entry, hidden at start. The shell is painted with the same semi-transparent gradients.
  The app sets no native material today (`set_effects(None)`, lib.rs:1120).
- **Known visual gap:** four surfaces use CSS `backdrop-blur` over the palette content (PowerMenu.tsx:152,
  SettingsSheet.tsx:835 and :844, Toast.tsx:17, ResultContextMenu.tsx:183). GPUI has no backdrop
  filter. See DECISION D4.

## 4. Naming and paths

- `src/<path>.ts(x)` → `crates/prism-ui/src/<path>.rs`, file names in snake_case
  (`PaletteSearchInput.tsx` → `palette_search_input.rs`). One source file, one target file.
- `src/styles/tokens.css` → `crates/prism-ui/src/theme.rs`. `src/main.tsx` + `src/App.tsx` → `crates/prism-app/src/main.rs` and `crates/prism-ui/src/app.rs`.
- `*.test.ts(x)` are not translated. The judge covers their behavior; pure-logic assertions are
  re-expressed as Rust unit tests next to the module (same inputs and literal expected values).

## 5. Subsystems (units of work), dependency order

1. Pure logic: math, search, query, types, emoji, iconLoading, launcherMotion, update-policy/progress.
2. Palette model: phase1, sections, the sanitizers (state/app.tsx), plus `prism-judge`. Gate: the judge
   passes with `PRISM_ADAPTER_CMD=prism-judge` before any view is written.
3. Theme and primitives: theme.rs, ui.tsx (Kbd, RowIcon, Toggle, Segmented, IconButton, …), Toast.
4. Palette view: PaletteSearchInput, ResultRow, Palette (incl. drag and reorder), ResultContextMenu.
5. Settings: SettingsSheet, TaskbarCustomization, PowerMenu, UpdateControl.
6. App shell: window, Win-key/shortcut toggle, launcher motion, single instance, tray, updater.

## 6. DECISIONS for the owner

- **D1 — Backend sharing.** Recommended: extract a Tauri-free `prism-core` crate inside the Prism repo.
  The Tauri coupling is small outside `lib.rs`: about 64 sites in 10 modules
  (taskbar_customization 17, catalog/mod 16, win_key 5, start_menu 5, taskbar_icon_overlay 5,
  apps 4, windows_tools 4, audio_hook 4, audio 2, theme 2). They are mostly
  `AppHandle::emit` and window lookups and go behind a small `Host` trait.
  Both apps then share one backend.
  Alternatives: a path dependency on `src-tauri` (pulls Tauri into the GPUI binary), or a fork (two backends drift).
- **D2 — GPUI pin.** Recommended: crates.io `gpui = "0.2.2"` (latest published). Alternative: a Zed git
  commit (newer Windows fixes, less reproducible). The Step 2 spike decides if 0.2.2 is enough.
- **D3 — Shipping.** Recommended: the GPUI build ships as a separate preview channel until Step 6 passes,
  then replaces the Tauri build under the same app id and updater key.
- **D4 — Backdrop blur.** Options: (a) raise those four surfaces to an opaque or near-opaque fill that
  matches the blurred average (closest 1:1 without a blur pass); (b) write a custom blur
  shader for GPUI (large); (c) accept the difference. Recommended: (a), marked `BUG(port):`
  in the visual parity ledger.
- **D5 — Collation.** Recommended: `icu_collator` with the user's locale (the WebView uses
  `navigator.language`). Alternative: a simple case-insensitive numeric comparator that matches
  en-US for ASCII only.

## 7. Required Step 2 spike (before any fan-out)

A throwaway GPUI binary that proves on this machine: a transparent, borderless, always-on-top window with
no taskbar entry; show/hide in under 16 ms on the Win key (through `prism-core::win_key`); IME text input;
Segoe UI Variable rendering; a 1,000-row list scrolling at 60 fps; and the shell gradient matching a
screenshot of the current app within a fixed tolerance.

## Deviation log

- 2026-10-09: Step 0b reviewers and Step 1 map reviewers ran on Opus per the model plan. No deviation.
