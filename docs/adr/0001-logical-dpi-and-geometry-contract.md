# 0001: Logical DIP and Geometry Contract Across Displays

## Context
Prism previously mixed logical CSS pixels and Win32 physical pixels inside its native window sizing command (`set_window_width` in `src-tauri/src/lib.rs`), causing window distortion and mispositioning on high-DPI displays (e.g. 125%, 150%, 200%). Changing view zoom did not differentiate between OS window geometry and webview zoom. Furthermore, small screens and portrait displays caused content clipping due to fixed 620px native height and `overflow: hidden` on root styles.

## Decision
1. **Logical DIP Everywhere**: All frontend configuration, IPC commands (`set_window_width`), and settings contracts operate strictly in logical DIP (CSS pixels). Conversion to physical pixels occurs exclusively at the Win32 / Tauri windowing boundary (`physical = (logical * scale_factor).round()`).
2. **Preserve Logical Size Across Mixed-DPI Displays**: When Prism is summoned on or moves to a monitor with a different DPI scale factor, it retains its configured logical DIP dimensions, recalculating the required Win32 physical window size and clamping it to the target monitor's work area.
3. **Permissive Floor & Height Clamping**: Prism guarantees support down to a 640×480 logical work area floor. If the monitor work area is shorter than the configured launcher height, the native window clamps to the available work area, and the internal results list becomes vertically scrollable with a stable scrollbar gutter.
4. **Content-Only Zoom**: `setViewZoom` scales webview rendering internally; the native OS window frame geometry remains governed by the window width preset and height clamping rules.

## Consequences
- Prevents multi-DPI sizing bugs and window shrinking/expanding across monitors.
- Eliminates visual clipping on laptops at 150%/200% DPI and vertical/portrait displays.
- Requires container-responsive adjustments in the React launcher shell when running in compact heights/widths.
