# Design Tree: Windows Display & DPI Hardening

## Root: Prism Windows Display & Multi-DPI Support Contract

### Round 1: Foundational Contracts & Sizing Architecture (Settled)

- [x] **support-baseline**: `permissive` (640×480 logical work area floor)
- [x] **unit-contract**: `logical-everywhere` (Strict DIP; physical conversion only at Win32 boundary)
- [x] **mixed-dpi-behavior**: `preserve-logical` (Preserve logical dimensions across monitors)
- [x] **height-clamping-strategy**: `clamp-and-scroll` (Clamp window height to work area; scroll results)
- [x] **zoom-semantics**: `content-only` (Webview zoom scales content, native window frame stays constant)

---

### Round 2: Implementation Mechanics & Container Contracts (Settled)

- [x] **compact-breakpoint**: `multi-stage` (Width < 700px collapses secondary hints; height < 520px collapses badges/tightens padding)
- [x] **native-min-bounds**: `keep-safety-480x400` (Maintain 480×400 safety floor in `tauri.conf.json`)
- [x] **dpi-sync-trigger**: `on-toggle-and-dpi-event` (Sync on summon + listen for `ScaleFactorChanged` / `WM_DPICHANGED`)
- [x] **width-animation-approach**: `instant-switch` (Instant 0ms native window resize; smooth CSS interior)
- [x] **scroll-container-structure**: `results-only-scroll` (Pinned search header/footer; results scroll with `scrollbar-gutter: stable`)

---

### Round 3: Synthesis & Authorization (Settled)

- [x] **confirm-synthesis**: Authorized to proceed to implementation slices.

---

### Implementation Progress

- [x] **Slice 1: Native Geometry & DPI Conversion (Completed)**
  - Unified unit contract: Logical DIP authoritative everywhere in Rust.
  - Added `LOGICAL_WIDTH` tracking and fixed `set_window_width` to scale by target monitor DPI before Win32 calls.
  - Native monitor scale detection via `GetDpiForMonitor` / `GetDpiForWindow`.
  - Implemented `calculate_clamped_window_size` with DPI scaling and work area clamping.
  - Fixed `palette_position` to prevent clamp panic on oversized dimensions.
  - Added `WindowEvent::ScaleFactorChanged` handler for instant mixed-DPI repositioning.
  - Added 3 Rust unit test suites covering 100%, 125%, 150%, 200% DPI, short screens, and oversized clamp safety. All 186 Rust tests pass; Clippy clean.

- [x] **Slice 2: Container Responsiveness & Scroll Isolation (Completed)**
  - Added `.launcher-container { container: launcher / size; }` to [`src/styles/tokens.css`](file:///C:/Users/hi/Desktop/Work/Prism/src/styles/tokens.css) and [`src/App.tsx`](file:///C:/Users/hi/Desktop/Work/Prism/src/App.tsx).
  - Isolated vertical scrolling with `scrollbar-gutter: stable;` in `.scroll-thin` for results list (`#prism-results`) and settings panel body.
  - Multi-stage collapse via `@container launcher`:
    - `max-width: 680px`: Full-width settings sheet overlay, compact keyboard footer hints (`.footer-hint-text`, `.footer-hint-sep`).
    - `max-width: 580px`: Footer hints hidden entirely (`.footer-hints`), compact row grid (`32px minmax(0, 1fr) auto`, 32px icons).
    - `max-height: 520px`: Compact header and search field padding, compact row height, compact footer bar (`2.25rem`).
  - Added semantic class hooks across [`SettingsSheet.tsx`](file:///C:/Users/hi/Desktop/Work/Prism/src/components/SettingsSheet.tsx), [`PaletteSearchInput.tsx`](file:///C:/Users/hi/Desktop/Work/Prism/src/features/palette/PaletteSearchInput.tsx), and [`Palette.tsx`](file:///C:/Users/hi/Desktop/Work/Prism/src/features/palette/Palette.tsx).
  - 149 Vitest tests pass; `bun run build` succeeds without errors or warnings.

- [x] **Slice 3: Input, Accessibility & Verification Matrix (Completed)**
  - Touch / stylus hit targets verified (minimum 44px on primary interactive targets: close settings 44×44px, full-row action buttons `absolute inset-0`, retry buttons `min-h-11`).
  - Text scaling and overflow protection verified: pinned header/footer, `scrollbar-gutter: stable` prevents horizontal jumps, `#prism-results` smoothly scrolls.
  - Multi-DPI verification matrix confirmed:
    - 100% DPI (96 DPI): 640×620 physical
    - 125% DPI (120 DPI): 800×775 physical
    - 150% DPI (144 DPI): 960×930 physical
    - 200% DPI (192 DPI): 1280×1240 physical
    - Short work areas (e.g. 500px, 400px): strictly clamped to monitor bounds, no taskbar overlap, no Rust panic.

