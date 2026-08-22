# File index freshness plan

## Implementation status (2026-08-23)

The normal-user directory backend is implemented and verified:

- committed filename batches emit `file-index-updated`, so an open query refreshes without editing it;
- each volume uses a nonrecursive topology watcher plus recursive top-level shards;
- watcher queues are bounded and collapse overload into deduplicated dirty-shard markers;
- overflow and populated-directory additions trigger coalesced subtree repair, never an immediate whole-volume scan;
- the scoped scanner cannot prune rows outside its requested subtree;
- new top-level directories attach a watcher without restarting Prism;
- volume workers schedule independently behind a two-scan resource limit, so one slow source does not hold back discovery and maintenance for another;
- mapped SMB drives use the same notification shards, capped handle-reopen backoff, and automatic periodic reconciliation path;
- the regression test creates a file under a recursive `Users\...\Documents` shard and verifies both the SQLite result and the live search-refresh callback.

The privileged NTFS service is not implemented in this change. Prism is still a per-user installation, and the repository has no authenticated named-pipe protocol, client impersonation policy, or tested rule for filtering MFT metadata to the connected user. Shipping a SYSTEM service before those boundaries exist would create a local filename-disclosure risk. The sharded fallback now provides automatic freshness without elevation; phases 3, 4, and the service-backed portion of phase 7 remain security-gated follow-up work.

Unmapped UNC roots are also not auto-discovered because Prism has no configured-source model yet. Mapped NAS drives are covered. Adding arbitrary UNC roots requires a persisted source configuration and explicit credential/offline behavior rather than guessing network paths.

## Goal

New, renamed, moved, and deleted files should reach Prism search without a manual reindex. This applies to every indexed folder on local drives, removable drives, mapped drives, and configured NAS shares.

The target behavior is:

- Local NTFS changes normally appear within two seconds.
- Filesystems with working directory notifications normally update within two seconds.
- Missed notifications, journal gaps, disconnects, and restarts recover automatically.
- Search remains available from the persisted catalog during catch-up or repair.
- Content-only writes do not create filename-index work.
- A single missed event never starts a whole-volume scan.

For a NAS that does not report changes, no Windows client can promise a fixed discovery delay without enumerating the share. Prism should continuously crawl such shares under a resource budget. The worst-case delay is one crawl cycle, and recovery must not require user action.

## Observed failure

The affected Prism 0.9.17 catalog used the fallback backend for `C:` and contained 2,854,346 entries. The missing file appeared after Prism restarted and began another fallback scan.

The current fallback backend opens one recursive `ReadDirectoryChangesW` watcher for the entire volume. Its synchronous notification buffer is 64 KB. Windows discards the detailed records when that buffer overflows and tells the caller to enumerate the watched subtree. Prism records an overflow marker, then uses a guarded whole-volume scan to recover. Those guards protect CPU and disk use, but they also leave a freshness gap after a lost event.

There is a second issue. Ordinary watcher batches update SQLite without emitting `file-index-updated`. An unchanged query can remain stale even when ingestion succeeded.

The exact missed notification was not persisted, so overflow cannot be proven for this specific save. The catalog backend, restart behavior, and watcher design confine the fault to the live fallback path. The plan adds enough telemetry to identify the exact path if it happens again.

Relevant code:

- `src-tauri/src/catalog/watcher.rs`: whole-volume notification reader, event folding, and overflow callback.
- `src-tauri/src/catalog/mod.rs`: backend selection, global scan loop, NTFS polling, and update events.
- `src-tauri/src/catalog/ntfs/volume.rs`: raw NTFS transport boundary.
- `src-tauri/src/catalog/scanner.rs`: recursive fallback reconciliation.
- `src-tauri/src/catalog/db.rs`: fallback and NTFS storage plus FTS maintenance.
- `src/state/palette.tsx`: refreshes the current query after `file-index-updated`.
- `src-tauri/nsis/hooks.nsh`: installer and uninstaller hooks.

## Selected architecture

Use one catalog supervisor with an independent worker for every volume or share.

```text
Catalog supervisor
|-- local NTFS ---- privileged service ---- MFT baseline + USN change journal
|-- other disks --- notification shards --- scoped automatic reconciliation
`-- NAS / SMB ----- notification shards --- reconnect + continuous fallback crawl
                                             |
                                             v
                                      SQLite commit generation
                                             |
                                             v
                                      refresh current query
```

The public application path stays small:

```rust
let catalog = CatalogSupervisor::start(app_data_dir, app_handle);
let response = catalog.search(query, limit);
```

`search_files` should not know which backend owns a volume. The supervisor selects and runs the source, applies committed changes, tracks health, and emits one generation event when visible search membership changes.

A source should expose one conceptual operation: maintain this volume from its saved checkpoint. Backend-specific polling, buffering, repair, and retries stay private.

```rust
enum ChangeSource {
    NtfsService(NtfsServiceClient),
    DirectoryShards(DirectoryShardSet),
}

enum CatalogChange {
    Upsert(ScannedItem),
    Remove(PathKey),
    RenameSubtree { from: PathKey, to: ScannedItem },
    DirtyShard(ShardId),
}

struct AppliedBatch {
    changed_entries: u64,
    checkpoint: SourceCheckpoint,
}
```

These are design shapes, not required final names. Do not expose service messages, Win32 handles, watcher buffers, or scan generations to the search command.

## Phase 1: lock down the regression

Write failing Windows integration tests before changing the implementation.

1. Complete an initial fallback scan.
2. Create a file while Prism remains running.
3. Keep the same query open.
4. Assert that the file appears without editing the query, restarting, or rebuilding.
5. Repeat for atomic save, rename, delete, directory rename, and a populated directory moved into the watched tree.

Add deterministic tests for an injected notification overflow. The expected result is a dirty scope and automatic convergence, not a volume rebuild.

Add catalog health data per worker:

- selected backend;
- last notification time;
- last committed catalog generation;
- saved checkpoint;
- queue depth and coalesced-event count;
- overflow, reconnect, and repair counts;
- current state: live, catching up, repairing, degraded, or offline.

Persist only what helps restart recovery. Keep detailed diagnostics behind `PRISM_CATALOG_DEBUG`.

## Phase 2: introduce the catalog supervisor

Move volume scheduling out of the single `scan_all_volumes` lifecycle.

- Discover volumes and configured UNC roots.
- Start one worker per volume or share.
- Stop and replace only the worker whose mount identity changed.
- Let local drives update while a NAS is slow or offline.
- Bound every event queue. When a queue reaches its limit, replace queued detail with one `DirtyShard` marker.
- Return an `AppliedBatch` from every database commit.
- Emit `file-index-updated` only when `changed_entries` is greater than zero.
- Include a monotonic catalog generation so the frontend can discard duplicate events.

This phase also fixes the current missing update event for successful fallback watcher batches.

## Phase 3: add the NTFS indexing service

Local NTFS should use the persistent USN change journal instead of directory notifications. Windows restricts change-journal operations to administrators, so install a small Windows service once and keep the Prism UI non-elevated.

The repository already has the right boundary. `NtfsTransport` supports journal queries, MFT enumeration, and journal reads. Add a named-pipe implementation and let the existing `NtfsBackend` synchronization code consume it.

Suggested modules:

```text
src-tauri/src/catalog/supervisor.rs
src-tauri/src/catalog/source/mod.rs
src-tauri/src/catalog/source/ntfs_service.rs
src-tauri/src/catalog/source/directory.rs
src-tauri/src/indexer_service/mod.rs
src-tauri/src/indexer_service/pipe.rs
src-tauri/src/bin/prism-indexer-service.rs
```

The service protocol should support only:

- protocol handshake and capability query;
- validated local NTFS volume discovery;
- journal metadata query;
- streamed MFT enumeration;
- journal reads from a validated checkpoint;
- a blocking wait for the next journal change;
- cancellation and clean shutdown.

Do not expose generic `DeviceIoControl`, arbitrary device names, filesystem writes, process launch, or arbitrary paths.

The client stores the journal ID and next USN in its existing per-user catalog. A continuous wait wakes on a journal change, applies the batch transactionally, advances the cursor in the same transaction, and emits the new catalog generation. If the journal ID changes or the cursor falls behind retained history, Prism starts an automatic MFT rebuild and keeps serving the last valid generation until the replacement is ready.

## Phase 4: secure and package the service

The service binary must live in an administrator-protected directory such as `Program Files`. Never register a SYSTEM service whose executable is under `%LOCALAPPDATA%` or another user-writable directory.

Use an explicit named-pipe ACL:

- allow LocalSystem and the intended local interactive user;
- deny network access;
- authenticate the client process and session;
- reject clients using an incompatible protocol version;
- cap message and stream sizes;
- disconnect after malformed input;
- never continue a privileged request if impersonation or access validation fails.

Before implementation, complete a threat model for MFT metadata. The current elevated backend can see filenames outside the user's normal directory traversal. The service must not make that metadata available to unrelated local users. Filter records under the connected user's security context or document and test an equally strong boundary.

Update `tauri.conf.json`, the release scripts, and `src-tauri/nsis/hooks.nsh` to package, register, start, stop, upgrade, and remove the service. A per-machine installer is the simplest secure route. The migration from the current per-user installation must rewrite the autostart path instead of preserving the old `%LOCALAPPDATA%` executable. Service installation and service-binary updates may require an administrator prompt.

## Phase 5: replace whole-volume fallback recovery

Use adaptive notification shards for local non-NTFS filesystems, removable drives, mapped drives, and configured UNC roots.

During the baseline crawl, choose shard roots using observed entry count and scan time. Each shard owns a separate `ReadDirectoryChangesW` handle and buffer. Ancestor watchers monitor only direct child topology so Prism can add, remove, or split shard roots without overlapping recursive event streams.

Shard behavior:

1. Start the watcher in buffering mode.
2. Reconcile the shard subtree.
3. Replay buffered events over the baseline.
4. Switch to live event application.
5. On overflow, mark only that shard dirty.
6. Reconcile the dirty shard and replay events received during repair.
7. Split a shard when its event rate or repair duration repeatedly exceeds its budget.

Add a scoped scanner that walks a subtree without changing the volume-wide generation or pruning rows outside that subtree. Reuse the existing per-parent `prune_removed_children` behavior. A completed enumeration may prune missing children; a stalled or partial enumeration must not.

Directory additions need special handling. If a newly visible directory may already contain children, enqueue a subtree reconciliation instead of indexing only the directory row.

## Phase 6: handle NAS behavior

Use SMB change notifications when the server supports them. Keep buffers at or below 64 KB for network handles.

On disconnect or notification failure:

- mark the affected shards dirty;
- preserve their last valid search generation;
- avoid synchronous network `exists` checks on the search path while the share is offline;
- reconnect with capped exponential backoff and jitter;
- repair dirty shards after reconnect;
- resume notifications only after buffered repair events are applied.

If change notifications are unsupported, continuously cycle through persisted shards under a network budget. Prioritize newly mounted shares, dirty shards, and shards whose previous scan was fast. Report the measured crawl cycle as freshness status instead of promising an artificial fixed delay.

A full share crawl remains necessary for first-time indexing and after failures that provide no narrower recovery point. Prism starts it automatically and keeps other volumes live.

## Phase 7: migrate existing catalogs

Do not clear the current database on upgrade.

For an existing local NTFS fallback catalog:

1. Serve its persisted rows immediately.
2. Ask the service for journal cursor A.
3. Build a new NTFS generation in staging.
4. Read and apply changes from cursor A.
5. Atomically switch the volume to the new generation.
6. Remove superseded fallback rows after the switch commits.

For non-NTFS and network sources, build the shard map from the existing catalog, attach watchers before repair starts, then reconcile shards in the background.

Keep the manual rebuild command only as a diagnostic repair tool. It must not be part of normal freshness or recovery.

## Test and verification matrix

### Unit tests

- event folding preserves final state;
- queues remain bounded and collapse overload to `DirtyShard`;
- catalog generation increments only after changed commits;
- journal checkpoint and catalog changes commit atomically;
- journal discontinuity selects automatic rebuild;
- shard repair cannot prune outside its root;
- incomplete enumeration cannot prune live rows;
- cross-shard directory rename becomes a remove plus scoped add or an equivalent atomic operation;
- reconnect backoff is capped and resets after success;
- protocol parser rejects invalid versions, lengths, commands, and volume identifiers.

### Windows integration tests

- create, rename, move, and delete become searchable without query edits;
- atomic-save patterns from common editors converge;
- a burst larger than one notification buffer repairs automatically;
- content-only writes cause no catalog membership writes;
- service restart resumes from the saved USN;
- app restart catches up without a volume scan;
- journal truncation rebuilds in the background;
- one slow volume does not delay another;
- populated directory moves preserve descendants;
- offline mapped drives do not stall local search.

### SMB tests

- a local Windows SMB share exercises the network code path;
- create, rename, and delete arrive through notifications;
- forced disconnect and reconnect repair missed changes;
- injected overflow repairs one shard;
- a mock server without change notifications uses continuous crawl mode;
- offline shares preserve status without blocking queries.

### Installer and security tests

- clean install registers and starts the service;
- upgrade stops and replaces it safely;
- uninstall removes the service and binary;
- migration removes stale per-user executable and autostart paths;
- a remote pipe client cannot connect;
- an unrelated local process cannot issue privileged requests;
- malformed requests never reach raw volume operations;
- the service executable and its parent directories are not user-writable.

### Performance checks

- an idle catalog produces no sustained worker CPU or WAL growth;
- content churn without name changes produces no index churn;
- burst processing is linear in unique paths;
- queue memory stays within its configured cap;
- local search latency does not depend on NAS response time;
- scoped repair does not rebuild FTS when no searchable row changed.

Run the existing gates after each phase:

```powershell
bun run lint
bun run test
bun run build
Push-Location src-tauri
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
Pop-Location
```

Add elevated service and SMB integration jobs to CI rather than weakening tests when the ordinary test process lacks the required Windows privileges.

## Acceptance criteria

- Local NTFS changes normally become searchable within two seconds.
- Working directory notifications normally update search within two seconds.
- The currently displayed query refreshes after a committed catalog change.
- Overflow, journal gaps, disconnects, and restarts converge without manual reindexing.
- One overflow never triggers a whole-volume scan.
- A slow or offline source never blocks another source or the search command.
- Content-only changes create no filename-index work.
- Event queues and memory use stay bounded during bursts.
- Search continues using the last valid generation during repair.
- The installer places privileged code only in an administrator-protected directory.
- Service IPC is local, authenticated, versioned, bounded, and read-only.
- NAS sources that cannot notify remain visibly degraded and continuously self-repair.

## Alternatives considered

### Sharded watchers everywhere

This avoids a service but cannot provide lossless local NTFS history. It reduces overflow frequency and repair cost, yet a missed notification still needs enumeration. It remains the right fallback for filesystems and servers without a change journal, not the primary local NTFS design.

### One privileged machine-wide search database

This centralizes service work but changes database ownership, complicates multi-user access control, duplicates search ranking across the service boundary, and risks exposing filenames that a user cannot normally enumerate. Keeping the catalog per user preserves the current search path and limits the service to metadata transport.

### Windows Search or Everything as a required dependency

Both can provide fast results when installed and configured, but Prism would inherit an external indexer's coverage, exclusions, lifecycle, and availability. They may be optional adapters later. They should not define correctness for Prism's core index.

## Constraints to preserve

- Preserve the name-only notification filter introduced for issue 17. Content, timestamp, and size writes must not re-enter the fallback queue.
- Preserve persisted search results during startup and repair.
- Preserve atomic NTFS generation swaps and journal cursor consistency.
- Preserve path existence and access checks before showing a result.
- Keep filesystem enumeration off Tauri runtime and UI threads.
- Do not let one volume's lifecycle own or block another volume.
- Do not put a privileged executable in a user-writable location.

## References

- Microsoft, `ReadDirectoryChangesW`: <https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-readdirectorychangesw>
- Microsoft, change journal identifier and privileges: <https://learn.microsoft.com/en-us/windows/win32/fileio/using-the-change-journal-identifier>
- Microsoft, named pipe security: <https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-security-and-access-rights>
- Microsoft, named pipes and remote access: <https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipes>
- Tauri, Windows installer modes and hooks: <https://v2.tauri.app/distribute/windows-installer/>
- Tauri, external binaries: <https://v2.tauri.app/reference/config/#bundleconfig>
- Prism PR 18, watcher CPU and disk-use fix: <https://github.com/Hi9841/Prism/pull/18>
