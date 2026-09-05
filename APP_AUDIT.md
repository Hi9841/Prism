# Prism app audit

Fixes and current verification: [APP_AUDIT_FIXES.md](APP_AUDIT_FIXES.md). The findings and command results below record the original audit before implementation.

Audit date: 2026-09-05. Version: 0.9.38. Reviewed commit: `3b1f74c`.

Start with the audio event path, search keyboard handling, and failing validation gates. These affect user control or the ability to ship a verified release.

## Scope and evidence

This is a source audit with local verification, covering the React launcher, settings, persistence, updater, file search and catalog queries, taskbar audio integration, native volume display, configuration, and CI. The landing page received a limited source and test review. The video production project, installer execution, and exhaustive review of the Windows shell-hook and NTFS implementations were outside this pass.

The working tree was clean before the audit. This report is the only file created or modified by this audit. No application fixes were applied. During the final check, concurrent deletions appeared for the landing page, video project, and older reports. Those changes were left untouched. Landing-page observations and passing test results below describe the files as read earlier in this audit; they do not describe the final working tree. If removal of the landing page is intentional, its test-integration recommendation in A20 no longer applies.

Browser verification through `npx -y chrome-devtools-axi open http://localhost:1420` returned `ERR_CONNECTION_REFUSED`. No dev server was started. The installed native app was not exercised interactively. Consequently, this report makes no measured claims about visual contrast, frame rates, memory growth, input latency, or production exploitability.

Evidence labels:

- **Observed**: a local verification command demonstrated the issue.
- **Source-confirmed**: the implementation contains the described behavior. Native or browser reproduction remains pending.
- **Optimization candidate**: the code does avoidable work, but its runtime cost needs measurement.

The installed Impeccable skill lacked its referenced audit playbook and context script. The UI review used the available instructions and project source directly.

## Verification results

| Check | Result |
| --- | --- |
| `bun run test` | Passed: 122 tests across 10 files. |
| `bunx tsc --noEmit` | Passed. |
| `bun run lint` | Failed: six formatting errors and one optional-chain warning. |
| `bunx vitest run --config landing/vitest.config.ts` | Passed: seven tests. These are source-content checks. |
| `cargo test --manifest-path src-tauri/Cargo.toml --lib --quiet` | Failed: 154 passed, one failed, five ignored. |
| `cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check` | Failed. Formatting differences include audio, drag, apps, lib, and Win-key code. |
| `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings` | Failed: 14 library diagnostics; 15 for the library test target. |
| `scripts/check-version.ps1` | Passed: version 0.9.38 is consistent. |
| Browser at the configured development URL | Unavailable: connection refused. |

No frontend production build or installer build was run. Cargo compiled the targets needed for the test and Clippy checks.

## Priority index

P1 means fix before the next release. P2 means schedule a focused correction. P3 means improve after correctness work or after profiling justifies it. No P0 issue was established.

| ID | Priority | Finding | Evidence |
| --- | --- | --- | --- |
| A01 | P1 | Audio scroll workers race and have no concurrency bound | Source-confirmed |
| A02 | P1 | Failed hover identification falls back to master volume | Source-confirmed |
| A03 | P2 | Broad process-name matching can adjust multiple applications | Source-confirmed |
| A04 | P2 | Audio COM initialization has no matching cleanup | Source-confirmed |
| A05 | P2 | Native volume display uses primary-monitor coordinates | Source-confirmed |
| A06 | P2 | First volume display request can be lost during startup | Source-confirmed |
| A07 | P1 | Audio tests depend on live applications and can unmute audio | Observed failure; source-confirmed side effect |
| A08 | P1 | Formatting and Clippy failures block the configured CI gates | Observed |
| A09 | P2 | Persistence failures are silent and pending saves are not flushed | Source-confirmed |
| A10 | P2 | Reset can leave native settings partially changed | Source-confirmed |
| A11 | P2 | Search failures are reported as successful empty results | Source-confirmed |
| A12 | P2 | File candidate retrieval prevents some advertised fuzzy matches | Source-confirmed |
| A13 | P2 | Superseded searches still perform serialized database work | Optimization candidate |
| A14 | P3 | Two-character fallback searches repeat the same prefix query | Source-confirmed |
| A15 | P2 | Thumbnail cache does not invalidate changed files | Source-confirmed |
| A16 | P1 | Keyboard result selection is absent from accessibility semantics | Source-confirmed |
| A17 | P1 | Enter during IME composition can launch a result | Source-confirmed |
| A18 | P2 | Several controls fall below the project's 44px target rule | Source-confirmed |
| A19 | P2 | Update checks can overwrite the state of an active installation | Source-confirmed |
| A20 | P2 | Integration behavior is largely outside automated coverage | Source-confirmed |
| A21 | P3 | Icon and thumbnail updates rebuild search sections | Optimization candidate |
| A22 | P3 | Large UI modules combine too many interaction responsibilities | Source-confirmed |
| A23 | P3 | README advertises appearance choices absent from the UI | Source-confirmed |

## Audio and native feedback

### A01. Serialize volume changes and bound worker concurrency

**P1. Source-confirmed.** References: [audio_hook.rs](src-tauri/src/audio_hook.rs), lines 100-117; [audio.rs](src-tauri/src/audio.rs), lines 72-81 and 166-174.

Every wheel message starts a new OS thread. Each thread resolves the hovered target, enumerates audio objects, reads volume, adds a delta, and writes volume. Two workers can read the same starting value and overwrite each other's changes. Completion order can also reverse the volume display updates. Fast scrolling creates an unbounded number of simultaneous workers if UI Automation or audio calls slow down.

Use one dedicated audio worker with bounded pending work. Accumulate wheel deltas for the same target, initialize COM once on that worker, and serialize volume mutations. Preserve target changes when coalescing. Avoid adding a general job framework.

Validate with a rapid wheel burst and alternating direction. Compare expected accumulated deltas against final volume and display order. Record maximum worker count and p95 event-to-feedback latency.

### A02. Treat failed target identification as unknown

**P1. Source-confirmed.** Reference: [audio.rs](src-tauri/src/audio.rs), lines 194-201, 240-256, and 354-366.

`inspect_element_at` returns `None` when COM creation or `ElementFromPoint` fails. `identify_taskbar_target_at` turns this into empty strings through `unwrap_or_default`. Empty name and automation ID mean "empty taskbar", which selects master volume. A failed lookup over an application button can therefore change system-wide volume instead.

Preserve an explicit unknown/error result. Change master volume only after positively identifying the taskbar background or intended tray control. Leave volume unchanged on an unresolved target.

Validate by injecting an inspection failure while the pointer is over an app button. Master volume must remain unchanged.

### A03. Resolve an application identity before changing sessions

**P2. Source-confirmed.** Reference: [audio.rs](src-tauri/src/audio.rs), lines 151-178 and 270-294.

The matcher accepts a session when any title or AppID token contains its process stem, or vice versa. It then adjusts every matching session across every active endpoint. Display words such as "Music" can match several different processes. This is a plausible wrong-target path; it was not reproduced with live applications during this audit.

Prefer an exact resolved executable, process identity, or explicit AppID mapping. Retain multiple sessions only when they belong to that resolved application. Report an unresolved target rather than relying on arbitrary title substrings.

Validate against synthetic sessions with overlapping names, such as `music` and `musicbee`, and two different applications playing audio at once.

### A04. Balance COM initialization and release

**P2. Source-confirmed.** Reference: [audio.rs](src-tauri/src/audio.rs), lines 62, 99, and 196.

Three functions call `CoInitializeEx`, ignore the result, and never call `CoUninitialize`. A normal scroll worker can initialize COM in both inspection and adjustment before exiting. Microsoft requires each successful initialization, including repeated successful initialization, to have matching cleanup. See [Microsoft's COM initialization guidance](https://learn.microsoft.com/en-us/windows/win32/learnwin32/initializing-the-com-library).

Own COM initialization at the audio worker boundary with a small RAII guard. Handle initialization failure and release interface objects before the guard uninitializes COM. Pair this with A01.

Validate repeated events and all early-return paths. Resource growth was not measured in this audit.

### A05. Position the volume display on the pointer's monitor

**P2. Source-confirmed.** Reference: [audio_osd.rs](src-tauri/src/audio_osd.rs), lines 33-34 and 155-182.

The display clamps horizontal coordinates to `SM_CXSCREEN`, with a fixed lower bound of 12. These are primary-screen bounds, while the hook accepts secondary taskbars and the pointer uses desktop coordinates. A monitor to the left has negative coordinates that are forced onto the primary monitor. A monitor to the right is also clamped to the primary screen. Width and height are fixed physical pixels.

Resolve the monitor containing the pointer, clamp to its work area, and scale display dimensions and offsets for that monitor's DPI.

Validate monitors on all four sides of the primary display, mixed DPI, and secondary-taskbar scrolling.

### A06. Deliver the pending volume display state after initialization

**P2. Source-confirmed.** Reference: [audio_osd.rs](src-tauri/src/audio_osd.rs), lines 52-78 and 81-124.

`show` starts the display thread, stores the latest state, and posts an update only when `OSD_HWND` is already nonzero. Window creation runs asynchronously. If it finishes after the first `show`, no message is posted for the pending state, so the first scroll can adjust volume without feedback. A failed window creation also leaves `OSD_THREAD_INITIALIZED` true, preventing a retry.

After publishing the window handle, process any pending state. Reset initialization state on creation failure or thread exit.

Validate a single wheel event after cold startup and a simulated first window-creation failure.

## Release checks and persistence

### A07. Make default audio tests independent of the desktop

**P1. Observed failure; source-confirmed side effect.** Reference: [audio.rs](src-tauri/src/audio.rs), lines 421-445, with mutation paths at 77-81 and 169-171.

`test_adjust_app_volume_isolation` assumes Discord and Fortnite both have active audio sessions. The local run failed at line 434 because Discord returned `Ok(None)`. A fresh CI runner does not provide these applications or sessions. The master-volume test also requires a working audio endpoint.

Both tests call mutation functions with delta zero as if they were read-only queries. Those functions still set volume and unmute whenever the resulting volume is above zero. Running the default suite can therefore unmute the developer's machine. This audit ran the suite; whether mute state actually changed was not measured because no baseline was captured.

Extract pure session-selection tests. Put device-dependent tests behind an explicit opt-in and restore any state they change. Provide actual read-only volume queries for inspection.

Validate the default suite on a clean Windows runner without Discord, Fortnite, or an audio device.

### A08. Restore the configured validation gates

**P1. Observed.** References: [.github/workflows/ci.yml](.github/workflows/ci.yml); [audio.rs](src-tauri/src/audio.rs), line 320; [audio_osd.rs](src-tauri/src/audio_osd.rs), lines 262-264, 291-294, 394-415, and 457; [drag.rs](src-tauri/src/drag.rs), lines 97 and 268.

Frontend lint reports formatting errors in `Palette.tsx`, `sections.test.ts`, `sections.ts`, `bridge.ts`, `types.test.ts`, and `types.ts`. It also reports `useOptionalChain` at `Palette.tsx:435`.

Rust formatting fails. Clippy with warnings denied reports unnecessary casts and mutable references, manual range checks, a full iterator traversal where `next_back` suffices, an excessive argument count, and a test clone that can use `slice::from_ref`.

The committed CI invokes these checks and requires its validation jobs before the installer job. These failures already exist on the reviewed commit; they were not introduced by this report. Remote CI status was not queried.

Apply narrowly scoped formatting and lint corrections, fix A07, then rerun the existing checks. Do not suppress warnings globally.

### A09. Surface save failures and flush deliberate exits

**P2. Source-confirmed.** References: [app.tsx](src/state/app.tsx), lines 101-108 and 125-140; [SettingsSheet.tsx](src/components/SettingsSheet.tsx), line 899; [lib.rs](src-tauri/src/lib.rs), lines 1486-1500 and 1704-1719.

State loads and saves swallow errors. Disk-full or permission errors leave the UI behaving as though pins and settings were saved. A 350ms timer delays persistence; provider cleanup cancels the timer, and the Quit action does not explicitly flush pending state. Quick process exit can lose the most recent edit. Native teardown sometimes delays exit, but it is not a persistence guarantee.

Keep the existing atomic replacement on disk. Track dirty state, surface failed writes with retry, and await a pending save before deliberate Quit or update installation. Distinguish a missing state file from a corrupt or unreadable file so a load failure does not silently become an unrelated overwrite later.

Validate save rejection, corrupt input, and edit-then-quit within 350ms using an isolated state directory.

### A10. Handle partial failure during reset

**P2. Source-confirmed.** Reference: [app.tsx](src/state/app.tsx), lines 254-269.

Reset applies the default shortcut first, then taskbar alignment, and only then updates React state. If alignment fails after shortcut registration succeeds, the shortcut has changed but the UI and persisted settings still describe the previous shortcut.

Capture the previous native values and roll back an earlier successful change when a later operation fails. If rollback fails, reload authoritative values and explain the partial result. Use the same explicit failure handling for native width, zoom, always-on-top, and scroll-volume effects currently swallowed at lines 326-365.

Validate shortcut success followed by alignment failure. The UI and native state must agree afterward.

## Search correctness and performance

### A11. Preserve search and directory-access errors

**P2. Source-confirmed.** Reference: [catalog/search.rs](src-tauri/src/catalog/search.rs), lines 59-70 and 324-327; [palette.tsx](src/state/palette.tsx), line 275.

Database candidate errors become an empty response with the existing ready/indexing flags. With a ready index, the frontend cannot distinguish this from a successful search with no matches. Direct browsing similarly maps `read_dir` failure to an empty directory listing.

Return an explicit error through the command boundary or add a typed failure field to the search response. Keep "no matches", "cannot read this folder", and "index query failed" distinct. Offer Retry or Rebuild only for the corresponding failure.

Validate a database query error and an unreadable directory. Neither should display the normal empty-results state.

### A12. Align file candidate retrieval with fuzzy scoring

**P2. Source-confirmed.** References: [catalog/db.rs](src-tauri/src/catalog/db.rs), lines 1560-1745 and 2024-2035; [catalog/search.rs](src-tauri/src/catalog/search.rs), `target_score`.

The scorer supports noncontiguous subsequences, but candidate retrieval uses exact, prefix, and contiguous trigram matches. Searching `rpt` cannot retrieve an otherwise isolated `report.txt`: it is neither an exact nor a prefix match, and it contains no contiguous `rpt`. The subsequence scorer never receives the file. Queries composed entirely of short tokens also produce no FTS expression.

Choose a bounded candidate strategy that supports the promised fuzzy behavior, or explicitly narrow the product's file-search promise. Avoid fixing this with a full catalog scan on every keystroke.

Validate indexed searches for `rpt` against `report.txt`, mixed short tokens, and non-Latin names. Include both NTFS and fallback catalogs.

### A13. Cancel obsolete work before expensive search stages

**P2. Optimization candidate.** References: [catalog/search.rs](src-tauri/src/catalog/search.rs), lines 56-73 and 116-122; [catalog/db.rs](src-tauri/src/catalog/db.rs), line 1565; [lib.rs](src-tauri/src/lib.rs), lines 1040-1050.

The generation check happens after `search_candidates` completes. Every request first waits for the same reader mutex and runs candidate SQL. The frontend discards obsolete responses, but that does not stop their backend work. The later disk-existence loop has no generation check either. Older requests can spend time querying or checking unavailable paths while the current query waits or competes for workers.

Propagate request identity early enough to skip superseded requests before database execution. Check cancellation between expensive stages. Measure before adding extra readers or a more complex scheduler; SQLite interruption must not cancel a newer query sharing the connection.

Benchmark rapid typing during indexing and with a temporarily unavailable volume. Record current-query p50/p95, reader wait time, and obsolete queries actually executed.

### A14. Remove the duplicate two-character prefix pass

**P3. Source-confirmed.** Reference: [catalog/db.rs](src-tauri/src/catalog/db.rs), lines 1645-1675 and 1711-1743.

The unconditional prefix stage already runs the range query. For a two-character query, the later `else if query_len == 2` repeats the same fallback query and parameters. Deduplication discards the repeated rows only after SQL execution and row materialization.

Remove the duplicate branch after checking result equivalence for two-character inputs. Measure query count and latency on a large fallback catalog. This is a small optimization with a clear mechanism, not a demonstrated user-visible speedup.

### A15. Invalidate cached thumbnails when files change

**P2. Source-confirmed.** Reference: [palette.tsx](src/state/palette.tsx), lines 58-68, 188-198, 207-214, and 260-294.

Thumbnail cache keys contain only the path. Both images and null results remain cached until eviction. Index-update notifications refresh results and history existence but do not invalidate thumbnails. Saving a new image at the same path can keep showing the old preview; replacing an unreadable image with a valid one can retain the null result.

Use a versioned cache key based on file modification metadata or explicit invalidation on relevant file changes. Give transient null results a retry policy. Keep the existing 128-entry retention bound.

Validate overwriting an image at the same path and replacing an initially invalid image with a valid one while Prism stays open.

## Keyboard interaction, accessibility, and updates

### A16. Expose the active search result to assistive technology

**P1. Source-confirmed.** Reference: [Palette.tsx](src/features/palette/Palette.tsx), lines 575-585, 637-647, 918-920, and 1159-1178.

Arrow keys change selection while DOM focus stays in the input. The input has `aria-controls`, but no `aria-activedescendant` or composite-widget role. Rows express selection through `data-selected` and styling. The live region announces reordering only. The selected launch target is therefore not represented through the active-descendant mechanism or an equivalent selection announcement.

Choose an accessible composite pattern suited to rows with secondary actions. Wire the active row identity, selected state, and keyboard behavior together; do not blindly place interactive buttons inside a listbox option. The [WAI-ARIA combobox pattern](https://www.w3.org/WAI/ARIA/apg/patterns/combobox/) explains active-descendant focus management and alternative popup patterns.

Validate with NVDA or Narrator: type a query, arrow between results, hear the active result, open its action menu, and launch it. Confirm names and actions remain available when sections update.

### A17. Ignore launcher commands while an IME composition is active

**P1. Source-confirmed.** Reference: [Palette.tsx](src/features/palette/Palette.tsx), lines 558-610.

The input key handler unconditionally consumes Enter and calls `runSelected`. There is no composition guard. Enter used to confirm a Japanese, Chinese, or Korean IME candidate can trigger a launch, and arrows or Escape can interfere with candidate selection.

Return early while the native keyboard event is composing. Verify any WebView2-specific composition-end ordering before adding compatibility handling.

Validate real IME input in WebView2. Confirming a composition must not launch a result; the following deliberate Enter should launch it.

### A18. Extend hit areas on remaining small controls

**P2. Source-confirmed against the project rule.** References: [SettingsSheet.tsx](src/components/SettingsSheet.tsx), lines 751-756; [PowerMenu.tsx](src/components/PowerMenu.tsx), line 137; [UpdateControl.tsx](src/features/updater/UpdateControl.tsx), lines 160 and 196.

The settings close button is 28px square, power rows are 36px tall, and update controls are 32px tall, with no local hit-area expansion. The shared `IconButton` already uses an expanded pseudo-element; these separate controls do not. This finding concerns the project's explicit 44px rule, not a blanket assertion that every control violates WCAG.

Reuse a consistent expanded hit area or increase control height without overlapping neighboring targets. Verify actual geometry at the supported zoom levels in the running app.

### A19. Prevent an earlier update check from resetting install state

**P2. Source-confirmed.** Reference: [UpdateControl.tsx](src/features/updater/UpdateControl.tsx), lines 30-64 and 98-114.

The check entry point refuses new checks while installing. However, an already-running check can resolve after installation starts from a previously available update. Its completion handler does not recheck `installInFlightRef`; it can replace or close `updateRef.current` and set the view back to available or hidden while the download continues.

Keep installation ownership stable until completion. Discard or defer check results that arrive during installation, and guard installation entry with the ref as well as rendered state. Close only resources that are no longer owned by an active operation.

Validate with deferred promises: retain an available update, begin another check, start installation, then resolve that check. The control must remain busy and must not permit a second install. Native resource-close consequences require integration verification.

## Coverage and maintainability

### A20. Add coverage at the component and native workflow boundaries

**P2. Source-confirmed.** References: [vitest.config.ts](vitest.config.ts), `landing/vitest.config.ts` as read before its concurrent deletion, [.github/workflows/ci.yml](.github/workflows/ci.yml), and existing `src/**/*.test.ts` files.

The default frontend suite runs in a Node environment and covers helpers and policies. It does not mount the main UI or exercise actual keyboard, composition, focus, persistence, or updater request lifecycles. This leaves A09, A16, A17, and A19 outside the current tests. The seven landing tests pass when invoked separately, but the default test include excludes them and CI does not invoke their config.

Add a small set of behavior tests around the affected components with controllable bridge promises, then a Windows smoke workflow for show/search/launch, settings focus, and taskbar behavior. Run landing checks in CI if the site is part of the supported release. Keep live desktop mutation tests explicitly opt-in, as described in A07.

Acceptance should cover actual outcomes rather than source-string assertions. No new general testing framework is needed merely to increase a coverage number.

### A21. Separate result ranking from icon hydration if profiling warrants it

**P3. Optimization candidate.** References: [palette.tsx](src/state/palette.tsx), lines 367-413; [sections.ts](src/features/palette/sections.ts), lines 136 and 188.

The section-building memo depends on app icons, thumbnails, file results, and volume state. Icon batches therefore rebuild sections and rerun app ranking for an unchanged search query. In the idle view, section rebuilds sort the installed app list again. Fresh item and callback identities can also reduce the benefit of memoized result rows.

Profile a large app list while icons load. If this cost is material, memoize app ranking and idle sorting on app metadata/query changes, then attach icons independently. Preserve the existing prepared-app metadata cache and batched icon requests.

Use React render timings and query-to-paint measurements to justify the change. Do not virtualize or introduce a state library without evidence.

### A22. Split interaction ownership when touching the large UI modules

**P3. Source-confirmed.** References: [Palette.tsx](src/features/palette/Palette.tsx), 1,397 lines; [SettingsSheet.tsx](src/components/SettingsSheet.tsx), 918 lines; [palette.tsx](src/state/palette.tsx), 591 lines; [app.tsx](src/state/app.tsx), 563 lines.

The palette combines result rendering, several drag modes, category reordering, context menus, and keyboard routing. Settings combines native operations, collection management, picker behavior, and modal lifecycle. This makes interaction changes harder to review and test independently.

After adding behavior coverage, extract one responsibility at a time, starting with drag/reorder state or the app-collection picker. Keep ownership and call chains shallow. File length alone does not justify a broad rewrite during a bug fix.

### A23. Update the appearance promise in the README

**P3. Source-confirmed.** References: [README.md](README.md), line 15; [SettingsSheet.tsx](src/components/SettingsSheet.tsx), Appearance section; [app.tsx](src/state/app.tsx), lines 305-318; [lib.rs](src-tauri/src/lib.rs), lines 789-792.

The README advertises acrylic, mica, and solid appearance choices. The current UI exposes theme and accent controls, and the bridge's `setWindowStyle` accepts only light/dark. The native implementation clears window effects. Legacy validation accepting an `effect` field does not provide a working user choice.

Rewrite the documentation around the current controls. If restoring material selection is intended, treat it as a separate feature with visible controls and native behavior tests.

## Strengths to preserve

- The app already has a restrictive production CSP, a configured updater public key, and scoped main-window plugin permissions. No exploitable security issue was established in this pass; this is not a dependency-vulnerability or penetration-test attestation.
- State persistence uses validation and atomic file replacement. Fix error reporting and lifecycle handling without replacing that design.
- Search work already runs outside the main native thread. The catalog has indexes, transactional operations, and meaningful regression tests.
- Icons are requested in batches, thumbnail dimensions and batch sizes are bounded, and the frontend thumbnail cache has a retention cap.
- Shared controls include focus treatment and expanded targets; global CSS respects reduced motion. Existing settings focus management and menu keyboard handling provide a useful base.

## Recommended work order

1. Make tests safe and repeatable, then restore lint, formatting, and Clippy gates: A07-A08.
2. Fix audio target identification and serialized mutation; correct display delivery and monitor placement: A01-A06.
3. Fix keyboard selection semantics and composition handling: A16-A18.
4. Correct persistence, reset, search errors, candidate matching, thumbnail invalidation, and updater lifecycle: A09-A12, A15, A19.
5. Add focused workflow coverage, benchmark search/rendering, and make only justified optimizations and small extractions: A13-A14, A20-A23.

Before claiming the next release is verified, complete a native Windows pass with real IME input, Narrator or NVDA, rapid taskbar scrolling, mixed-DPI monitors, an unavailable volume, and an isolated save-failure scenario. Record measured latency and memory baselines before assigning numerical performance targets.
