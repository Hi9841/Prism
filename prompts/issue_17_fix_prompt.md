# Task Prompt: Fix Issue #17 - app sometimes crashes and usage becomes extremely high

## Context
In `Hi9841/Prism`, the fallback file catalog recursively watches a mounted volume in `src-tauri/src/catalog/watcher.rs`. It queues Windows directory-change notifications and periodically folds them into SQLite updates so filename search stays current while the palette is hidden.

## Problem Statement
The fallback watcher subscribes to last-write, creation-time, and size notifications across the whole volume. These content-only changes do not affect Prism's filename or path search, but high-churn browser, build, and system caches send them into an unbounded queue. `apply_queued_events` then deduplicates paths with repeated `Vec::contains` and `Vec::retain` scans, making large batches quadratic, before writing the touched rows back to SQLite.

On the reported machine, the hidden app's `prism-flush-<volume-id>` thread consumed one full CPU core continuously and the catalog WAL changed every second. The catalog contained 2,859,188 fallback entries in a 3.3 GB database. This sustained queue and database churn explains the high CPU and disk use and creates an eventual memory or crash risk.

## Objective
Keep fallback filename search current for file and directory additions, removals, and renames without processing content-only writes. Fold remaining event bursts in linear time while preserving the existing final-state semantics.

## Key Files to Modify
1. `src-tauri/src/catalog/watcher.rs`
   - Subscribe only to `FILE_NOTIFY_CHANGE_FILE_NAME` and `FILE_NOTIFY_CHANGE_DIR_NAME`.
   - Keep add, remove, rename, and overflow behavior intact.
   - Replace repeated vector membership scans with hash-set folding.
   - Add a real Windows watcher regression test proving that an existing file's content change does not enqueue a catalog update.

## Acceptance Criteria
- Content, timestamp, and size-only changes do not enter the fallback catalog queue.
- File and directory additions, removals, and renames still update search membership.
- Duplicate event folding remains correct and runs in linear expected time.
- The focused watcher tests pass.
- All existing Rust tests pass with `cargo test`.
- All frontend tests pass with `bun run test`.
- Frontend build and lint pass with `bun run build` and `bun run lint`.

Issue: https://github.com/Hi9841/Prism/issues/17
