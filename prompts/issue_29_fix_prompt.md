# Task Prompt: Fix Issue #29 - prevent Win-key combos from opening Prism

## Context
In repository `hi9841/prism`, the native Windows-key integration spans `src-tauri/shell-hook/src/lib.rs` and `src-tauri/src/win_key.rs`. The Explorer message hook consumes the shell Start command, while a raw-input state machine distinguishes a standalone Windows-key press from Windows-key combinations.

## Problem Statement
The Explorer hook posts `EVENT_TOGGLE_PRISM` immediately when it sees `WM_SYSCOMMAND/SC_TASKLIST`. Windows emits that shell command on Win-down, before a user can press the second key in a shortcut. As a result, pressing `Win+R` opens Prism from the shell hook and also opens the Windows Run dialog. The raw-input state machine correctly marks `Win+R` as a combo, but that decision cannot undo the toggle already queued by the Explorer hook.

## Reproduction Evidence
The installed build reproduced the issue end to end: `Win+R` made both Prism and the Windows Run dialog visible. Tracing a disposable debug build showed `Win-down`, `R-up`, and `Win-up`; Windows did not deliver a matching raw `R-down`. The state machine must therefore treat any non-Win event while Win is held as proof of a combo.

## Objective
Keep consuming the native Start command while Win observation is active, but let the raw-input state machine be the only path that toggles Prism for keyboard input. Win-key combinations such as `Win+R` must reach their normal Windows targets without opening Prism, while a bare Win press must still open Prism.

## Key Files to Modify
1. `src-tauri/shell-hook/src/lib.rs`
   - Preserve the Explorer `SC_TASKLIST` interception and observer-liveness fail-open behavior.
   - Stop turning the early shell command into a Prism toggle action.
2. `src-tauri/src/win_key.rs`
   - Keep standalone Win toggling on raw-input key-up and preserve taskbar Start-click handling.
   - Add regression coverage for the shell-command/raw-input handoff so a keyboard combo cannot queue a toggle through the shell path.

## Acceptance Criteria
- `Win+R` and other Win-key combinations do not open Prism.
- A bare left or right Win press still toggles Prism once.
- The native Start command remains suppressed only while the observer is alive; teardown still fails open.
- Taskbar Start-button clicks still toggle Prism.
- The focused Rust tests pass, and all available project validation commands are run and reported. The repository currently lacks the frontend package/build files required by the README's Bun commands.
