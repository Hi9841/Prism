# Task Prompt: Fix Issue #34 - Add pin/unpin taskbar/properties to rightclick

## Context
In repository `Hi9841/Prism`, the command palette launcher displays search results for apps, files, folders and Quick Access entries. Right-click opens `src/features/palette/ResultContextMenu.tsx` (wired from `src/features/palette/Palette.tsx`), which currently offers only "Open location" and "Run as administrator". Native filesystem operations go through Tauri commands in `src-tauri/src/lib.rs` backed by `src-tauri/src/apps.rs` (Shell `IShellLink`, `ShellExecuteW`, COM via `ComGuard::init`). Issue #32 added `open_path_location` following the exact layering: Rust fn -> Tauri command -> `src/lib/bridge.ts` -> `PaletteItem` action -> context menu item.

## Problem Statement
Prism's result context menu lacks the native Windows shell actions users expect from a launcher: "Pin to taskbar" / "Unpin from taskbar" and "Properties" (both visible in the Windows context menu in the issue screenshot). Windows exposes these as shell context-menu-handler verbs (`taskbarpin`, `taskbarunpin`, `properties`) that require `ShellExecuteExW` with `SEE_MASK_INVOKEIDLIST`; the existing `shell_execute` helper uses plain `ShellExecuteW`, which cannot invoke them. The menu also needs the live taskbar pin state to label the action ("Pin to taskbar" vs "Unpin from taskbar"); pinned state can be derived by resolving the `.lnk` targets inside `%APPDATA%\Microsoft\Internet Explorer\Quick Launch\User Pinned\TaskBar` (the codebase already resolves shortcuts via `resolve_lnk` and scans that folder for app discovery).

## Objective
Add "Pin to taskbar"/"Unpin from taskbar" (label driven by live pin state) and "Properties" to the result context menu for items with a local filesystem target: apps with local `.lnk`/exe targets, local files (pin only for launchable extensions), and Quick Access folders (properties only). Keep the existing menu actions, keyboard navigation and clamping behavior intact; menu height must account for the new rows.

## Key Files to Modify
1. `src-tauri/src/apps.rs`
   - Add `unsafe fn shell_execute_verb(verb: &str, path: &Path) -> Result<(), isize>` using `SHELLEXECUTEINFOW` with `SEE_MASK_INVOKEIDLIST` + `SW_SHOWNORMAL` (error = `hInstApp` value, mirroring `shell_execute`).
   - Add `pub fn set_taskbar_pinned(path: &Path, pinned: bool) -> Result<(), String>` invoking verb `taskbarpin`/`taskbarunpin`; reject missing paths first.
   - Add `pub fn show_properties(path: &Path) -> Result<(), String>` invoking verb `properties`; reject missing paths first.
   - Add `pub fn is_pinned_to_taskbar(path: &Path) -> bool` + `fn taskbar_pins_dir() -> Option<PathBuf>` (via `known_folder(&FOLDERID_RoamingAppData)`): a path counts as pinned when a TaskBar `.lnk` equals it, resolves (via `resolve_lnk`) to it, or (UWP pins with empty target) shares its file name.
   - Add `fn path_key(path: &str) -> String` normalizing Windows paths for comparison (trim, `/`->`\`, lowercase).
   - Tests in the existing `tests` module: `path_key` normalization, missing-path rejections for the two verbs, unknown target reported unpinned. No test may invoke a live shell verb that mutates the real taskbar or opens UI.
2. `src-tauri/src/lib.rs`
   - Add commands `is_pinned_to_taskbar(path) -> bool`, `set_taskbar_pinned(path, pinned) -> Result<(), String>`, `show_path_properties(path) -> Result<(), String>` following the `open_path_location`/`run_path_as_admin` pattern (`spawn_blocking` + absolute-path validation) and register all three in `generate_handler!`.
3. `src/lib/bridge.ts`
   - Export `isPinnedToTaskbar(path): Promise<boolean>`, `setTaskbarPinned(path, pinned): Promise<void>`, `showPathProperties(path): Promise<void>` with the usual `inTauri` guards.
4. `src/lib/types.ts`
   - Add to `PaletteItem`: `shellPath?: string` (local target for shell actions), `toggleTaskbarPin?: () => Promise<void> | void`, `showProperties?: () => Promise<void> | void`.
   - Add `isTaskbarPinablePath(path: string | undefined): boolean` mirroring `isElevatablePath` over extensions `exe, lnk, bat, cmd, msc`.
5. `src/features/palette/sections.ts`
   - `appPaletteItem`: set `shellPath: localTarget`, `showProperties` and (when `isTaskbarPinablePath(localTarget)`) `toggleTaskbarPin` closures over a module-local `toggleTaskbarPin(path)` helper that queries `isPinnedToTaskbar` and applies `setTaskbarPinned(!pinned)`.
   - `filePaletteItem`: `shellPath: entry.path`, `showProperties`, `toggleTaskbarPin` only when `!entry.isDirectory && isTaskbarPinablePath(entry.path)`.
   - `quickAccessPaletteItem`: `shellPath: entry.path`, `showProperties` only.
6. `src/features/palette/ResultContextMenu.tsx`
   - New props `onToggleTaskbarPin?` and `onShowProperties?`; render "Pin to taskbar"/"Unpin from taskbar" (`Pin`/`PinOff` icons, label resolved by querying `isPinnedToTaskbar(item.shellPath)` on open, hidden until resolved) and "Properties" (`Info` icon, last row) matching existing menuitem styling.
7. `src/features/palette/Palette.tsx`
   - Count the new actions in `openResultMenu`'s `actionCount` and open the menu when any action exists (shared helper); wire `onToggleTaskbarPin`/`onShowProperties` to close the menu, run the item action, and toast failures like `runItemAsAdmin` does.

## Acceptance Criteria
- Right-clicking an app with a local target shows "Pin to taskbar" or "Unpin from taskbar" reflecting the real taskbar state, plus "Properties".
- Local files show "Properties" always and taskbar pin only for launchable extensions; folders/Quick Access show "Properties" only.
- Clicking the pin action pins/unpins via the shell verb; clicking Properties opens the native dialog; failures surface as toasts, never unhandled rejections.
- Menu height/clamping accounts for up to four action rows; keyboard navigation covers the new items.
- All existing tests pass (`bun x vitest run` and `cargo test --manifest-path src-tauri/Cargo.toml`).
