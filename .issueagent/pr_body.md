## Summary
Resolves #32.

### Root Cause
Context menu interactions were restricted exclusively to elevatable executables and scripts (checking only `runAsAdmin`). File and folder search results, Quick Access items, and local application entries lacked an 'Open location' option and context menu support.

### Key Changes
- `src-tauri/src/apps.rs` & `src-tauri/src/lib.rs`: Added `open_path_location` Tauri command to reveal items in Windows File Explorer via `explorer.exe /select,"<path>"`.
- `src/lib/bridge.ts`: Added `openPathLocation` IPC bridge function.
- `src/lib/types.ts`: Added `openLocation` function to `PaletteItem` interface.
- `src/features/palette/sections.ts`: Attached `openLocation` to `filePaletteItem`, `quickAccessPaletteItem`, and local `appPaletteItem`s.
- `src/features/palette/ResultContextMenu.tsx`: Added 'Open location' menu action with `FolderOpen` icon, dynamic height calculation, and accessible multi-item keyboard navigation (`ArrowUp`/`ArrowDown`/`Home`/`End`).
- `src/features/palette/Palette.tsx`: Enabled right-click and keyboard shortcut (`ContextMenu` / `Shift+F10`) for items with `openLocation` or `runAsAdmin`.
- `src/features/palette/sections.test.ts`: Added unit tests verifying `openLocation` is attached to files, Quick Access, and local app items.

### Verification
- [x] Full frontend test suite passed: `bun x vitest run` (110 passed)
- [x] Full backend test suite passed: `cargo test` (138 passed)
- [x] Independent subagent code review passed (Verdict: PASS)

Fixes #32
