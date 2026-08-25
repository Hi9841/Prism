# Task Prompt: Fix Issue #32 - Add 'Open location' to right-click context menu for files and folders

## Context
In repository `Hi9841/Prism`, the command palette launcher displays search results for installed applications, local files, folders, and Quick Access entries. Context menus are triggered via right-click in `src/features/palette/Palette.tsx` and rendered by `src/features/palette/ResultContextMenu.tsx`. Native filesystem operations are executed via Tauri bridge IPC to `src-tauri/src/lib.rs` and `src-tauri/src/apps.rs`.

## Problem Statement
Currently, right-clicking search results only works if `item.runAsAdmin` is defined (which is restricted to elevatable executables and scripts). File and folder search results (as well as Quick Access items) do not respond to right-click because `Palette.tsx` suppresses the context menu when `runAsAdmin` is absent. Furthermore, `ResultContextMenu.tsx` only contains a "Run as administrator" action button and lacks an "Open location" action to reveal files and folders in Windows Explorer.

## Objective
Enable right-click context menus for all file, folder, and Quick Access search results (and apps with filesystem paths) with an "Open location" action that reveals and selects the item in Windows File Explorer via `explorer.exe /select,"<path>"`. For items that support both "Run as administrator" and "Open location", the menu must cleanly display both actions with proper keyboard navigation and hit targets.

## Key Files to Modify
1. `src-tauri/src/apps.rs`
   - Implement `pub fn open_path_location(path: &Path) -> Result<(), String>` using `explorer.exe /select,"<path>"` (or direct open if root drive).
2. `src-tauri/src/lib.rs`
   - Add the Tauri command `open_path_location(path: String) -> Result<(), String>`.
   - Register `open_path_location` in the `invoke_handler` list.
3. `src/lib/bridge.ts`
   - Export `openPathLocation(path: string): Promise<void>`.
4. `src/lib/types.ts`
   - Add optional `openLocation?: () => Promise<void> | void` to `PaletteItem`.
5. `src/features/palette/sections.ts`
   - Attach `openLocation: () => openPathLocation(entry.path)` in `filePaletteItem` and `quickAccessPaletteItem`.
   - For `appPaletteItem`, attach `openLocation` when `app.location` or `app.path` is a valid filesystem path.
6. `src/features/palette/ResultContextMenu.tsx`
   - Support `onOpenLocation` callback alongside `onRunAsAdmin`.
   - Render "Open location" menuitem (with `FolderOpen` icon) and "Run as administrator" menuitem (with `ShieldCheck` icon) based on available item actions.
   - Support keyboard navigation (ArrowUp/ArrowDown/Tab/Escape) across multiple menu items and clamp menu position based on dynamic menu height.
7. `src/features/palette/Palette.tsx`
   - Update `openResultMenu` and `ResultRow.onContextMenu` to open the context menu when either `item.runAsAdmin` or `item.openLocation` is present.
   - Wire `onOpenLocation` from `ResultContextMenu` to execute `resultMenu.item.openLocation?.()`.
8. `src/features/palette/sections.test.ts`
   - Add unit tests verifying that `filePaletteItem`, `quickAccessPaletteItem`, and local `appPaletteItem` populate `openLocation`.

## Acceptance Criteria
- Right-clicking on file search results displays a context menu with "Open location".
- Right-clicking on folder search results and Quick Access entries displays a context menu with "Open location".
- Clicking "Open location" invokes `open_path_location` and reveals the item in Windows Explorer.
- Items with both admin execution and file location display both options in the context menu.
- Keyboard navigation (Arrow keys, Enter, Esc) works smoothly within the context menu.
- All existing tests pass (`bun x vitest run` and `cargo test --manifest-path src-tauri/Cargo.toml`).
