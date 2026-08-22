# Task Prompt: Fix Issue #26 - Search and open Windows Settings pages

## Context
In repository `Hi9841/Prism`, `src/features/palette/sections.ts` builds the searchable result sections from apps, files, Quick Access entries, and calculations. Result actions cross the Tauri bridge in `src/lib/bridge.ts`, and Rust commands in `src-tauri/src/lib.rs` validate and launch native targets.

## Problem Statement
Windows Settings destinations are absent from the palette's searchable data. Queries such as `display settings` therefore cannot produce a Settings result and may fall through to the copy action after file search completes. Prism also has no validated native command for launching a documented `ms-settings:` URI.

## Objective
Add a built-in catalog of common Windows Settings pages, rank matching pages in a Settings section ahead of apps and files, and launch each result through a restricted Tauri command without disrupting keyboard navigation or error handling.

## Key Files to Modify
1. `src/features/palette/windowsSettings.ts`
   - Define common Windows Settings pages with titles, documented `ms-settings:` URIs, and useful search aliases.
   - Build ranked `PaletteItem` results and cap the section to a practical number of rows.
2. `src/features/palette/windowsSettings.test.ts`
   - Verify the initial issue mappings and title-versus-keyword ranking.
3. `src/features/palette/sections.ts`
   - Add matching results to a Settings section before app and file results.
   - Count Settings matches when deciding whether to show the no-match copy action.
4. `src/features/palette/sections.test.ts`
   - Reproduce the missing result, assert section ordering, keyboard item ordering, and fallback suppression.
5. `src/lib/bridge.ts`
   - Add a typed wrapper for the native Settings launch command.
6. `src-tauri/src/lib.rs`
   - Register a command that accepts only cataloged `ms-settings:` targets and reports launch failures without crashing Prism.
7. `src-tauri/src/windows_settings.rs`
   - Add the native URI launcher and unit-test the allowlist used at the command boundary.

## Acceptance Criteria
- Common Windows Settings pages are included in Prism's searchable data.
- `display settings`, `bluetooth`, `windows update`, and `default apps` map to their documented `ms-settings:` URIs.
- Page-title matches appear in a Settings section before weaker app and file matches and remain in `flatItems` keyboard order.
- Settings aliases such as `screen` and `monitor` find Display.
- A Settings match suppresses the no-match copy action.
- The native bridge rejects targets outside Prism's built-in Settings URI allowlist and returns launch errors to the existing toast handler.
- All existing tests pass (`bun run test`, `bun run build`, `bun run lint`, and `cargo test --manifest-path src-tauri/Cargo.toml`).
