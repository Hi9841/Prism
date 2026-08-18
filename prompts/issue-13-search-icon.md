# Task Prompt: Fix Issue #13 - Taskbar Search Icon Opens Native Start Menu

## Context
In repository `Hi9841/Prism`, Prism intercepts Windows Start button clicks and the standalone Windows key using a custom shell bridge and mouse hook (`src-tauri/shell-hook/src/lib.rs` and `src-tauri/src/win_key.rs`).

## Problem Statement
When a user clicks the Search icon / Search box on the Windows Taskbar, the mouse click is not intercepted by Prism because the current hook rectangle only tracks `AutomationId == "StartButton"`. As a result, the click passes directly to Explorer / Windows Search, opening the native Windows search flyout or Start menu.

## Objective
Update Prism's shell bridge and taskbar hook to support intercepting the Windows taskbar Search icon / button clicks and toggling Prism (with search focus), or providing configurable suppression for taskbar search.

## Key Files to Modify
1. `src-tauri/src/win_key.rs`
   - Extend `StartButtonLocator` (or add `SearchButtonLocator`) to query the Taskbar's Search element (`AutomationId` matching `"SearchButton"`, `"SearchBox"`, or child class `TraySearch`).
   - Send the search button bounding rectangle to the shell hook.
2. `src-tauri/shell-hook/src/lib.rs`
   - In `PrismShellMouseHook`, check if mouse click coordinates fall within the Start or Search button bounds (`point_is_in_start_button` / `point_is_in_search_button`).
   - Forward the toggle event `EVENT_TOGGLE_PRISM` or `EVENT_TASKBAR_START_CLICK` to Prism's raw observer window.
3. `src-tauri/src/taskbar_customization.rs`
   - Optionally expose `SearchboxMode` registry preference (`HKCU\Software\Microsoft\Windows\CurrentVersion\Search`) in taskbar settings.

## Acceptance Criteria
- Clicking the Taskbar Search button triggers Prism palette toggle.
- Normal clicks on other taskbar items (pinned apps, system tray, clock) remain unaffected.
- All existing tests pass (`cargo test`).
