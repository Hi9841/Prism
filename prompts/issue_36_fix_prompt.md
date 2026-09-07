# Task Prompt: Fix Issue #36 - windows start menu opening

## Context
In repository `Hi9841/Prism`, Win observation lives in `src-tauri/src/win_key.rs` and `src-tauri/shell-hook/src/lib.rs`. An Explorer `WH_GETMESSAGE` hook consumes `SC_TASKLIST` / bare Win `WM_HOTKEY` on Progman, the taskbar, and one app-manager thread so native Start does not appear. Prism then toggles its palette.

## Problem Statement
Pressing Win (shortcut is `Win`) opens the native Windows 11 Start menu (`Windows.UI.Core.CoreWindow` titled `Start` from `StartMenuExperienceHost`) as well as Prism. The app-manager hook only looks for `ApplicationManager_ImmersiveShellWindow`. This machine has `ApplicationManager_DesktopShellWindow` on a different Explorer thread, so that Start command is never consumed. PR #7 also removed post-show hiding, so the native menu stays in front.

## Objective
Consume the Start command on the current Windows 11 app-manager window class, keep the legacy class, and dismiss a native Start/Search CoreWindow if it still appears when Prism claims Win or the Start button.

## Key Files to Modify
1. `src-tauri/src/win_key.rs`
   - Attach `WH_GETMESSAGE` to `ApplicationManager_DesktopShellWindow` and `ApplicationManager_ImmersiveShellWindow` when their threads are not already Progman/taskbar.
   - Track one hook per thread. Dismiss visible `Windows.UI.Core.CoreWindow` windows titled `Start` or `Search` when Prism handles Win/Start, with a short retry so a late host cannot win the race.
2. `src-tauri/src/lib.rs`
   - Call that dismiss from Win-key and taskbar-Start toggles (open and close).

## Acceptance Criteria
- Win does not leave native Start in the foreground; Prism still toggles.
- `Win+R` and other chords still reach Windows and do not toggle Prism.
- Taskbar Start clicks still toggle Prism.
- Fail-open remains if the observer window is gone.
- Tests: `cargo test --manifest-path src-tauri/Cargo.toml --all-targets` and `bun test`.
