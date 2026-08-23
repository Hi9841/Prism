# Task Prompt: Fix Issue #17 Recurrence - removed paths scan the full catalog

## Context
In `Hi9841/Prism`, the fallback file catalog recursively watches a mounted volume in `src-tauri/src/catalog/watcher.rs`. It queues Windows directory-change notifications and periodically folds them into SQLite updates so filename search stays current while the palette is hidden.

## Problem Statement
The earlier fix stopped content-only watcher events, but watcher removals and startup scan pruning still delete directory subtrees with `normalized_path LIKE ? || '%'`. SQLite can only use the `volume_id` part of the `(volume_id, normalized_path)` index, so each removed or already-missing filename scans the entire volume catalog.

On the reported machine, Prism crashed and was reopened. The replacement process immediately consumed one full CPU core while hidden and read about 2.8 GB every three seconds. The catalog contains 2,874,343 entries in a 3.35 GB database. A missing-path subtree probe took about 790 ms, while the exact composite-index probe took 0.1 ms.

## Objective
Keep file and directory removal semantics while making watcher updates and startup pruning use the complete composite index, so removal batches cannot pin a CPU core or repeatedly read the full catalog.

## Key Files to Modify
1. `src-tauri/src/catalog/db.rs`
   - Replace the non-indexable subtree `LIKE` delete with exact and bounded descendant-range deletes.
   - Share the indexed delete across watcher updates and scan pruning.
   - Add regressions that fail when either removal path scales with total catalog rows.
2. `src-tauri/src/catalog/watcher.rs`
   - Preserve existing add, remove, rename, overflow, and content-write filtering behavior.

## Acceptance Criteria
- Missing and ordinary file removals do not scan all rows for a volume.
- Startup pruning does not scan all rows once per missing directory.
- Directory removal still deletes the directory and every descendant.
- File additions, removals, renames, overflow repair, and content-write filtering keep working.
- The focused watcher tests pass.
- All existing Rust tests pass with `cargo test`.
- All frontend tests pass with `bun run test`.
- Frontend build and lint pass with `bun run build` and `bun run lint`.

Issue: https://github.com/Hi9841/Prism/issues/17
