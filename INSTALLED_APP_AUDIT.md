# Installed Prism audit

Date: 2026-09-06. Audited build: 0.9.53. Release version: 0.9.54.

The updated executable is installed at `C:\Users\hi\AppData\Local\Prism\prism.exe` and was launched successfully. Its SHA-256 matches the release build:

`5F0CD6B3896D181313DE9F9A1CF85A8B6798F2043D3FD4E91F8613F1C3460FC5`

## Fixed findings

| Finding | Change and verification |
| --- | --- |
| Collapsed collections hid matching applications during search. | Search now preserves ranked application matches independently of idle collection grouping. Reproduced with Notepad in the installed app; checked after installation and added a regression test. |
| A nonexistent folder ending in a separator appeared like an empty folder. | Return a corrective folder-not-found error while preserving partial-path searches. Added a Rust regression and checked native valid/missing paths. |
| Clearing a collection name immediately persisted invalid settings and caused a misleading save error. | Keep an editable draft, validate on Enter/blur, save only nonempty trimmed names, and support Escape cancellation. Mounted regression and installed-app rename checks passed. |
| The collection picker rendered and requested icons for every application immediately. | Start with 40 rows and matching icon requests; expose Show more and search across the full application list. A 385-app fixture verifies paging and finding the last app. |
| Collection picker keyboard navigation and closing lost useful focus. | Added arrow/Home/End navigation, accessible option names, and Escape/Done focus restoration. Mounted tests and native keyboard checks passed. |
| Translucent shell/settings surfaces allowed distracting content to show through. | Use opaque existing theme surfaces. Visually checked installed light and dark themes. |
| Footer hints were clipped at increased zoom. | Allow wrapping and keep individual hint groups intact. Checked native 150% zoom and multiple palette widths. |
| The light-mode calculator hint had insufficient contrast. | Use an existing readable text token. Rendered pixel contrast improved from 2.73:1 to 7.76:1. Applied the same token-based correction to the available-update control; that state was covered by component checks, not a live update install. |

## Performance evidence

- The collection-picker fixture reduced initial rows and icon requests from 385 to 40, about 90% less initial work. Single mounted-test timing samples were approximately 217 ms before and 51 ms after. These happy-dom measurements are indicative, not native end-to-end benchmarks.
- Native catalog observations used the existing index of approximately 2.66 million entries. Sampled nonempty searches took 142-229 ms; sampled no-match searches took 31-47 ms. Release compilation was active during sampling, so these are neither controlled benchmarks nor evidence of improved index latency.
- One 10-second idle sample recorded no measurable CPU time for the Prism process and approximately 57 MB working set. This excludes WebView2 processes and does not establish total application memory use.

## Verification

- Frontend: 168 tests passed across 18 files; Biome checked 46 files successfully.
- Rust: 211 tests passed, 5 ignored. Formatting passed.
- TypeScript validation, release build, and `git diff --check` passed.
- Installed checks included application search/launch, calculator clipboard copying, indexed search, direct folder browsing, pin/unpin with keyboard focus, collections, picker filtering, settings persistence, light/dark appearance, width/zoom changes, Quick Access Home opening in Explorer, power-menu dismissal, and normal quit/relaunch.
- Taskbar size, grouping, auto-hide, and icon choices were exercised and restored. The size/grouping registry values were checked after restoration.
- Original settings, pins, collections, and history were restored from the original profile while Prism was closed. Profile hashes matched before relaunch and after the final shortcut attempt. The fixed app is left running.

## Remaining limits

This is a broad local audit, not proof that every feature and environment is defect-free.

- Clippy with warnings denied still reports seven pre-existing diagnostics in untouched files: four in `apps.rs`, and one each in `catalog/ntfs/usn.rs`, `catalog/mod.rs`, and `win_key.rs`. These were not expanded into unrelated refactors.
- WebView2 did not expose a working DevTools endpoint. Automatic approval review rejected process-control/debug-relaunch commands with only "blocked by policy" as its reason. Native Windows screenshots/input and component tests were used for verification.
- Physical Win-key activation remains unverified: the native hook deliberately ignores injected keys, so the synthetic Win-key attempt cannot validate it. Ctrl+Alt+Space activation was exercised before restoring the original Win shortcut.
- Actual shutdown/restart/sleep, elevated launches, signed updater installation, multi-monitor/DPI combinations, screen-reader operation, volume changes, and native drag/reorder were not verified. Existing automated coverage does not replace those integration checks.

## Local evidence and rollback

- Original executable: `.tmp-installed-audit/original-prism.exe`.
- Original profile: `.tmp-installed-audit/original-prism.json`.
- Screenshots, native timing logs, and automation helpers: `.tmp-installed-audit/` (ignored local artifacts).
- Useful screenshots: `contrast-before.png`, `contrast-after.png`, `fixed-missing-path.png`, `fixed-zoom150.png`, `final-rename-empty.png`, `final-rename-valid.png`, and `restored-final.png`.

To roll back locally, quit Prism normally, replace the installed executable with the original executable above, and relaunch it. The original profile is already restored. No version bump, commit, publication, or signed release was performed.
