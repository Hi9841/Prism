# Prism Domain Model: Display & Geometry Context

Language and architectural contracts for Prism's native window geometry, display scaling, and responsive layout across Windows devices.

## Language

### Logical DIP (Device-Independent Pixels)
The canonical unit for all Prism launcher dimensions, width presets (560, 640, 720), native IPC sizing commands, and stored settings. Corresponds directly to CSS pixels (`1 DIP = 1 CSS px`). All Rust IPC commands and frontend state accept and report DIP. Rust translates DIP to physical device pixels strictly at the Win32/Tauri boundary using the active monitor's DPI scale factor (`physical = logical * scale_factor`).

### Physical Pixels
Raw hardware pixels on the display panel. Used strictly at the Win32 API boundary (`HWND`, `RECT`, `SetWindowPos`, `MonitorFromPoint`, `PhysicalSize`). Must never leak into frontend settings, bridge APIs, or serialized preferences.

### Work Area
The usable screen rectangle on a target monitor, excluding the Windows taskbar and docked app bars (`MONITORINFO.rcWork`). Prism calculates palette placement and height clamping strictly against the work area of the monitor where the launcher is summoned.

### Permissive Degradation Floor
The lowest effective screen work area (640×480 logical DIP) Prism officially supports. Below standard desktop viewports, Prism enters compact degradation mode: results scroll, footer labels collapse, and the settings sheet expands to full width.

### Content-Only Zoom
View zoom (70%–150%) adjusts webview rendering and text scale internally within the launcher shell, leaving the native OS window frame dimensions fixed. Decouples text/accessibility enlargement from native window positioning and taskbar anchoring.
