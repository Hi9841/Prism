# Task Prompt: Fix Issue #19 - window corners become square when switching to Solid material

## Context
In repository `hi9841/prism`, the React settings sheet writes the selected window material through `set_window_style`. The native `apply_window_look` function in `src-tauri/src/lib.rs` applies Acrylic and Mica as DWM backdrops, while Solid queues removal of the active native effects.

## Problem Statement
Switching from Acrylic or Mica to Solid calls `WebviewWindow::set_effects(None)`. When Tauri's queued DWM backdrop removal runs, the borderless Prism HWND falls back to its default square corner preference. Switching back to a DWM material makes the corners round again. The window shape should not change when the user changes material.

## Objective
Keep Prism's native window corners rounded after every successful material change, including Solid, without making the setting fail on Windows versions that do not support the DWM corner-preference attribute.

## Key Files to Modify
1. `src-tauri/src/lib.rs`
   - Extract the material selection logic into a callback-driven helper that can be covered without creating a Tauri window.
   - Queue `DWMWCP_ROUND` with `DwmSetWindowAttribute` on Tauri's main thread after the asynchronous Acrylic, Mica, or Solid effect operation.
   - Keep the corner call best-effort so unsupported DWM versions do not break material switching.
   - Add a regression test proving every supported material reaches the rounded-corner step and invalid materials do not.
2. `src-tauri/Cargo.toml`
   - Enable the existing `windows` dependency's `Win32_Graphics_Dwm` feature.

## Acceptance Criteria
- Switching to Solid no longer changes the outer Prism window from rounded to square corners.
- Acrylic, Mica, and Solid all reassert the same native rounded-corner preference after a successful material change.
- Unsupported DWM corner preference calls do not cause the Window material setting to fail.
- Unknown material values still return an error without changing the window.
- All existing tests pass (`cargo test --all-targets --all-features` and `bun run test`).
