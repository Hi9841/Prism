# Prism audit fixes and verification

Date: 2026-09-05. Release version: 0.9.39. Audited base commit: `3b1f74c` at version 0.9.38.

This report tracks the implementation and local review of [the original audit](APP_AUDIT.md). Fixes are in the working tree. The final review and corrections were performed locally after the request to stop using subagents.

## Finding status

"Implemented" means the source correction and applicable local checks are complete. It does not imply that the installed Windows app was exercised interactively.

| ID | Status | Change and evidence |
| --- | --- | --- |
| A01 | Implemented | Audio requests use one COM worker and a bounded 64-entry queue. Same-target, same-direction events coalesce in bounded batches. Full queues pass the wheel event through. Enable generations invalidate queued requests after toggling the feature. Deterministic tests cover request handling. |
| A02 | Implemented | Failed or ambiguous hover identification produces an unknown target. Only positively recognized taskbar or volume-tray targets select master volume. Unknown targets do not mutate audio. |
| A03 | Implemented | Application targeting uses exact normalized executable stems instead of broad substring matching. Ambiguous application identifiers are rejected. Tests cover normalization and matching. |
| A04 | Implemented | A COM apartment guard pairs successful initialization with cleanup on the audio worker. |
| A05 | Implemented; native check pending | The volume display uses the pointer monitor's work area and scales using the target window DPI. Positioning tests cover negative coordinates, taskbar edges, and 150% scaling. Actual rendering on mixed-DPI monitors remains unverified. |
| A06 | Implemented; native check pending | Display state is stored before window creation. Publishing the window posts the pending update. Initialization failures reset startup state and allow retries, including requests arriving during failed creation. |
| A07 | Implemented | Replaced tests that queried or changed live application audio with deterministic tests. The suite no longer requires Discord, Fortnite, or a particular master-volume state. |
| A08 | Implemented | Corrected lint, Rust formatting, and Clippy diagnostics. Simplified drag PIDL handling and removed redundant casts. All listed validation gates pass. |
| A09 | Implemented | Saves are serialized and drain edits made during an in-flight write. Quit and updater installation await persistence. Initial reads finish before flushing. Read failures pause writes to preserve the existing file. Settings show a persistent error and retry action. Deferred-promise tests cover ordering, recovery, and failure. |
| A10 | Implemented | Reset waits for initial state, applies native settings before committing defaults, and attempts alignment and shortcut rollback on failure. Rollback errors explain partial state. A behavior test covers rollback. |
| A11 | Implemented | Directory and index-query failures cross IPC as typed errors. Directory enumeration errors propagate instead of disappearing. The launcher offers Retry or Rebuild, including when other result categories still have matches. |
| A12 | Implemented with bounded coverage | Added bounded subsequence candidate probes to fallback and NTFS searches, enabling matches such as `rpt` to `report.txt`. Tests cover mixed tokens and Unicode. Dense candidate ranges can still omit matches beyond the scan cap. |
| A13 | Implemented with cooperative cancellation | Search generations are assigned before blocking work is queued. Obsolete searches stop at stage boundaries, including after acquiring the database mutex. Already-running SQL and filesystem calls are not interrupted. |
| A14 | Implemented | Removed the repeated two-character fallback prefix query. |
| A15 | Implemented | Index events invalidate thumbnail caches, clear visible stale previews, and advance an epoch so late responses cannot repopulate stale thumbnails. Mounted behavior tests cover invalidation. |
| A16 | Implemented; assistive-technology check pending | Search input exposes combobox semantics linked to the results grid and active row. Selected results have stable IDs and selection semantics. Mounted tests cover focus and active-result changes. |
| A17 | Implemented; native IME check pending | Composition guards prevent search Enter and related Settings/global keyboard shortcuts from acting during IME composition. Mounted keyboard tests cover composition events. |
| A18 | Implemented for audited controls | Settings close, power actions, and update controls now meet the 44px target rule. Footer groups wrap to accommodate the larger controls. Mounted tests check relevant control sizing; live layout inspection remains pending. |
| A19 | Implemented | Install generations reject late update checks, duplicate installation is guarded, and active update resources stay alive until the operation settles. Download and install are separate so persistence can flush again after a long download. Deferred-promise tests cover late checks, save failures, and resource cleanup. |
| A20 | Coverage expanded; live check pending | Added mounted React behavior tests for launcher semantics, IME handling, Settings actions, persistence, search errors, thumbnails, updater races, and power-menu focus. These tests mock native boundaries and do not replace WebView2 or installer tests. |
| A21 | Measured; no refactor | Synthetic section-building measurements did not justify extra caching or state machinery. Results below describe the pure function only. |
| A22 | Focused extraction complete | Moved search input focus, keyboard handling, and ARIA ownership into `PaletteSearchInput.tsx`, with mounted behavior coverage. Larger settings and palette modules remain candidates for later focused extraction when behavior changes require it. |
| A23 | Implemented | README now describes the supported theme and accent choices and no longer promises unavailable material selection. |

## Verification

| Command | Final result |
| --- | --- |
| `bun run test` | Passed: 148 tests across 16 files. |
| `bun run lint` | Passed: 43 files checked. |
| `bunx tsc --noEmit` | Passed. |
| `cargo test --manifest-path src-tauri/Cargo.toml --quiet` | Passed: 174 tests, zero failures, five ignored. Binary and doc-test targets also passed. |
| `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings` | Passed. |
| `cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check` | Passed. |
| `scripts/check-version.ps1` | Passed: 0.9.38 is consistent. |
| `git diff --check` | Passed. |
| Browser at `http://localhost:1420` | Unavailable: `ERR_CONNECTION_REFUSED`. No dev server was started. |

No production frontend build or installer build was run. Cargo compiled the targets required for tests and Clippy. The five ignored Rust tests were not executed.

## Section-building measurements

Local synthetic benchmark of `buildSections`, with 20 warmup calls and alternating icon maps. Other result categories were empty. These timings exclude React rendering, IPC, disk access, and native presentation, and are not end-to-end latency measurements.

| Applications | Query | Samples | p50 | p95 |
| --- | --- | --- | --- | --- |
| 500 | `app 49` | 500 | 0.0678 ms | 0.1082 ms |
| 2,000 | `app 199` | 300 | 0.1752 ms | 0.1939 ms |
| 10,000 | `app 999` | 100 | 0.8289 ms | 0.9432 ms |
| 10,000 | Empty | 100 | 0.9331 ms | 2.4318 ms |

No before/after performance improvement is claimed for A21. The measurement supports leaving section construction simple until profiling shows a user-visible cost.

## Remaining verification and limits

1. Exercise real taskbar audio targeting, rapid enable/disable changes, first-show recovery, and OSD placement across monitors with different DPI settings. Ambiguous packaged-app identifiers intentionally produce no adjustment.
2. Check launcher selection with Narrator or NVDA, real IME composition in WebView2, and the larger controls at narrow window widths. Mounted DOM tests cannot establish visual polish or assistive-technology compatibility.
3. Exercise native update download and installation with pending settings changes. Tests cover ordering and failures through mocks; no installer was launched.
4. Evaluate fuzzy recall on a representative large catalog. Candidate scans are bounded, and cancellation is cooperative between stages. Neither exhaustive subsequence retrieval nor immediate interruption of every obsolete search is guaranteed.

## Workspace notes

The new mounted tests require `happy-dom` and `@testing-library/react`; Vitest now includes `.test.tsx` files. The updated root `package.json`, `bun.lock`, and `vitest.config.ts` are excluded by the existing local `.git/info/exclude` configuration and are not tracked at HEAD. They exist in this workspace but will not be included by ordinary staging. A portable patch must account for those dependency and configuration changes.

Existing deletions of the landing page, video project, and older reports were preserved. The original audit's landing-page test results describe its earlier snapshot and are not part of the final app test count.
