# 0002: Multi-Stage Container Responsiveness, DPI Synchronization, and Sizing Mechanics

## Context
Following the baseline logical DPI and geometry decisions in ADR 0001, Prism required concrete mechanics for responsive compact states, native window bounds, Win32 DPI change events, preset width switching, and scroll containment.

## Decision
1. **Multi-Stage Container Breakpoints**:
   - Width <= 680px: Secondary action hints and verbose path metadata collapse into compact icons/tooltips. Settings sheet becomes a full-width overlay.
   - Width <= 580px: Footer keyboard hints collapse entirely to leave room for action buttons, result row icons compact to 32px, and row grid tightens.
   - Height <= 520px: Header and search input vertical padding compacts; footer bar height compacts (2.25rem) to maintain maximum vertical space for search results.
2. **Native Window Bounds Safety Net**:
   - Retain `minWidth: 480, minHeight: 400` in `tauri.conf.json` as an OS-level safety floor, while designing Prism's primary container query breakpoints against the 640×480 permissive support floor.
3. **Dual DPI Synchronization**:
   - Recalculate physical dimensions and clamp to target work area on every presentation toggle (`reconcile_palette_position`).
   - Listen for native `tauri::WindowEvent::ScaleFactorChanged` / `WM_DPICHANGED` events while visible to reposition and resize immediately if dragged across mixed-DPI displays.
4. **Instant Native Width Transitions**:
   - Width preset changes (560px, 640px, 720px) resize the native Win32 window instantly (0ms) via `set_window_width`, eliminating intermediate redraw stutter, frame tearing, and multi-DPI interpolation overhead. Internal CSS properties transition smoothly.
5. **Pinned Header/Footer with Results Scroll Container**:
   - Search bar header and footer remain pinned at top and bottom.
   - Results list container uses `overflow-y: auto` with `scrollbar-gutter: stable`, preventing horizontal layout shifts when items are added or filtered.

## Consequences
- Guarantees seamless usability from 640×480 up to multi-monitor 4K displays.
- Eliminates visual stutter during preset switches and eliminates bottom clipping on short displays.
