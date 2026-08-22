# Task Prompt: Fix Issue #24 - Missing a sleep button

## Context
In repository `Hi9841/Prism`, the React power menu sends a typed action through the Tauri bridge to `power::perform` in Rust. The Rust handler validates the action and invokes either a direct Windows API or `shutdown.exe`.

## Problem Statement
The power menu exposes Lock, Shut down, and Restart, but it has no Sleep option. The TypeScript `PowerAction` union and Rust `PowerAction::parse` also reject `"sleep"`, so adding only a button would fail at the command boundary. Sleep must use the native Windows suspend API with hibernation disabled, no forced suspension, and wake events preserved.

## Objective
Add a complete Sleep computer action to Prism's power menu and execute it through the native Windows power API with the required shutdown privilege enabled only for the call.

## Key Files to Modify
1. `src/components/PowerMenu.tsx`
   - Add a Sleep menu item with the Lucide `Moon` icon.
   - Keep keyboard navigation, busy state, dismissal, and error reporting consistent with the existing actions.
2. `src/lib/bridge.ts`
   - Add `"sleep"` to the `PowerAction` union so the Tauri command remains type-safe.
3. `src-tauri/src/power.rs`
   - Add the Sleep enum variant and parser mapping.
   - Call `SetSuspendState(false, false, false)` for sleep instead of routing it through `shutdown.exe`.
   - Enable `SeShutdownPrivilege` for the native call and restore the previous token state afterward.
   - Return a useful error if privilege setup or suspension fails.
4. `src-tauri/Cargo.toml`
   - Enable the `Win32_System_Power` Windows crate feature required by the native API binding.

## Acceptance Criteria
- The power menu shows Sleep alongside Lock, Shut down, and Restart.
- Selecting Sleep requests standby rather than hibernation and preserves wake events.
- Rust accepts only the four visible power actions and still rejects hibernate, logoff, and command injection strings.
- Shutdown and restart continue to avoid forced application closure.
- The frontend type check and build pass.
- All existing tests pass (`bun run test` and `cargo test --manifest-path src-tauri/Cargo.toml`).
