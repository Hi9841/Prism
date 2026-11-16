# Catalog v2: in-memory file index, NTFS change journal, snapshot file

Owner request 2026-11-16: "fix all those issues" after the resource checkup (memory note
`prism-checkup-space`). This spec replaces the SQLite file catalog. **Test only in the agent-desktop
sandbox** (owner rule): never run a test build, a bench, or file churn on the owner's machine.

## Why (measured on 1.0.28, owner machine, 2026-11-16)

| Problem | Measurement | Cause |
|---|---|---|
| Disk | `catalog.db` 6.26 GB for 7.46 M rows (~840 B/row) | full path stored 3x per row + 2 path indexes; 36-char volume id text in every row and index |
| Phantom results | 2.09 M rows (28%) below deleted folders | sweep prunes only children of visited folders; no orphan cleanup |
| I/O | 251 GB read, 52.8 GB written in 21.7 h; ~4.6 KB written per re-indexed entry | SQLite page writes per row + 3 indexes; name-index reloads read the whole table |
| CPU / disk spikes | full `C:\` sweep every 6 h (3.5-7 min); 3 whole-home repairs (4.7 M entries, 4-7 min) on one day | `ReadDirectoryChangesW` overflow and queue collapse merge dirty scopes up to `C:\Users\hi` |
| Memory | 401 MB working set, 523 MB private | name index + SQLite mmap/cache |

Spike (2026-11-16, non-admin, `C:\Users\hi\prism-scratch\checkup\usn_spike3.py`, `usn_spike4.py`):
- A handle to the directory `C:\` (access 0 or `FILE_READ_ATTRIBUTES`, `FILE_FLAG_BACKUP_SEMANTICS`)
  accepts `FSCTL_QUERY_USN_JOURNAL`.
- `FSCTL_READ_UNPRIVILEGED_USN_JOURNAL` (0x000903AB) works with `READ_USN_JOURNAL_DATA_V0` or `_V1`
  **when `StartUsn` is a record boundary** (0, `FirstUsn`, `NextUsn`, or a `next_usn` the read
  returned). An arbitrary `StartUsn` returns `ERROR_INVALID_PARAMETER` (87).
- Records carry FRN, parent FRN, reason, attributes, **but no file name** (`FileNameLength` = 0).
- `OpenFileById(volume hint = the C:\ handle, FileId = FRN, access FILE_READ_ATTRIBUTES,
  FILE_FLAG_BACKUP_SEMANTICS)` + `GetFinalPathNameByHandleW` resolved 172 of 200 recent FRNs
  (the rest were deleted already).
- The C: journal holds only ~39 MB (minutes under heavy churn): Prism must read it continuously
  and must rebuild when its checkpoint falls behind `FirstUsn`.
- `FSCTL_ENUM_USN_DATA` (MFT enumeration) and `FSCTL_READ_USN_JOURNAL` need admin: error 5.

## Design

### One store, in memory (the truth while Prism runs)

`catalog/store.rs`, evolved from `name_index.rs`'s `Names` (keep its ranking code: `top_hits`,
`compare`, typo pass, `Closeness`, `char_mask`, the scorer calls). Per entry (index `u32`):

| Array | Type | Meaning |
|---|---|---|
| `parent` | `u32` | entry index of the parent folder; `ROOT` for children of a volume root (a per-volume root entry is fine too) |
| `name` | `u32` | display-name number (exact case); a per-display-name table maps it to the lowercase name number that the search scans |
| `flags` | `u16` | directory, dead, excluded-folder, penalty / `PENALTY_STEP` (as now) |
| `places` | `u32` | depth (high 16) and display-path byte length (low 16), as now |
| `seq` | `u16` | NTFS sequence number (FRN high 16 bits), to reject a reused MFT record |
| `next` | `u32` | next older entry with the same lowercase name (as now) |

Per volume: `by_record: Vec<u32>` indexed by MFT record number (FRN low 48 bits) -> entry index, or
`NONE`. Walk-only volumes (not NTFS, or no journal) get synthetic record numbers from the walk.

No SQLite row ids, no `display_path` text, no size or mtime (nothing reads them: `FileEntry` has
name, path, parent, is_directory). The display path is built from the parent chain when a result
is returned. Target: under 20 B per entry + names; about 150 MB RAM for 5.3 M entries.

Mutations (all under the store's write lock, short):
- `upsert(volume, frn, parent_frn, name, is_dir)`: insert, or move/rename in place (a folder
  rename changes one entry; its children keep their parent index). A rename that changes the
  penalty or the depth of descendants leaves their `places`/penalty stale until compaction
  (tie-breaks only); compaction recomputes them.
- `remove(volume, frn)`: mark dead. Children of a dead or excluded folder are dead by ancestry:
  path building and result selection walk the chain and drop such entries. NTFS removes a folder
  only when it is empty, so journal order normally deletes children first; a move to an unindexed
  place (recycle bin, an excluded folder, another volume) relies on the ancestry rule.
- Compaction: when dead entries pass a share (as `COMPACT_SHARE` now), build new arrays without
  dead entries and without entries that have a dead ancestor; recompute penalty and places from
  the chain; swap. Searches run during the build (slices under the read lock, as now).
- No orphans by construction: an entry is reachable only through live ancestors.

### Sources of truth on disk

- **NTFS volume with a journal** (`C:`, `D:`): initial build = a directory walk with
  `FileIdFullDirectoryInfo` (or `FileIdBothDirectoryInfo`): each entry's `FileId` is its FRN; the
  parent FRN is the walked folder's FRN (root FRN from `GetFileInformationByHandleEx(FileIdInfo)` or
  `BY_HANDLE_FILE_INFORMATION` on the root). Same reparse rules as `scanner.rs` (junctions and
  symlinks are leaves; cloud placeholders are descended). Record the journal id and `NextUsn`
  **before** the walk; after the walk, replay the journal from that USN (changes during the walk).
- Changes: one thread per NTFS volume reads the journal (unprivileged read, `ReasonMask` without
  `USN_REASON_CLOSE` only records, `BytesToWaitFor` = 1 with a timeout so it blocks instead of
  polling; or poll every 1 s). Fold one read batch per FRN to its final state:
  - last reason has `FILE_DELETE` -> remove;
  - `FILE_CREATE` or `RENAME_NEW_NAME` -> resolve now with `OpenFileById` +
    `GetFileInformationByHandleEx(FileNameInfo)` (path below the volume root; take the last
    component as the name) + attributes (`FileBasicInfo`) -> upsert with the record's parent FRN.
    Open fails (gone, access denied) -> skip; a later delete record removes it.
  - `HARD_LINK_CHANGE` -> resolve like a rename, with the parent read from the current path.
  - `RENAME_OLD_NAME` alone -> nothing (the new-name record carries the move).
  - One entry for each file record: extra hard links of a file are not indexed.
  - A record whose parent FRN is not a live, non-excluded entry -> skip without any open (this is
    what keeps excluded build folders free).
- Journal continuity: keep `journal_continuity` (`ntfs/usn.rs`). `Rebuild` (id changed, cursor
  truncated, cursor ahead) -> new walk in the background; the old entries stay searchable until the
  new arrays swap in.
- **Other volumes** (FAT, exFAT, ReFS, NTFS without a journal): walk at mount, then again every
  30 min while mounted and when the palette opens after more than 30 min. No watcher.
- `ReadDirectoryChangesW` (`watcher.rs`), the scope repairs, the deferred scopes, the 6 h sweep and
  the startup sweep all go away.

### Snapshot file (persistence)

`%APPDATA%\app.prism.launcher\file-index.bin`: header (magic, format version, per volume: id, mount
path, backend, journal id, next USN, entry range), then the arrays and the name text, each section
with a checksum (crc32 or xxh3). Written to `file-index.bin.tmp`, flushed, renamed over the old file.
Save after a build, then at most every 10 minutes while dirty, and at exit. Load at start (one read,
no parsing per entry beyond the checksum), then replay each journal from its checkpoint: the journal
is the write-ahead log. A bad checksum or an unknown version -> rebuild. Target size: about 100-150
MB for 5.3 M entries (from 6.26 GB).

### Exclusions (owner approved 2026-11-16)

Default list, matched on the lowercase folder name or a path below a volume root; folders matched are
kept as one excluded entry (so the journal filter can see them) and not descended:
`$recycle.bin`, `system volume information`, `node_modules`, `.git`, `__pycache__`, `.gradle`,
`<home>\.cargo\registry`, `<home>\.rustup\toolchains`, `<home>\.nuget\packages`,
`<home>\appdata\local\temp`, `c:\windows\winsxs`, `c:\windows\installer`,
`c:\programdata\microsoft\windows\containers`, Prism's own app data folder.
`file-index-exclusions.txt` in the app data folder (one folder name or absolute path per line, `#`
comments) replaces the default list when it exists. Prism writes the default list to that file on first
start, so the owner can edit it. A changed list takes effect at the next start (full re-walk). This ends the "full coverage" policy: log it in `migration/RULEBOOK.md` deviations.

### Migration

First start of v2: no `file-index.bin` -> build by walk (searches use the partial store and report
`indexing: true`; the UI already shows that state). When every volume is built and the first
snapshot is saved, delete `catalog.db`, `catalog.db-wal`, `catalog.db-shm`.

### What stays, what goes

- Stays: `search.rs` (scorer, `path_penalty`, `browse_path_with_errors`, `result_order`),
  `volume.rs` (volume discovery), `types.rs` public types, `ntfs/usn.rs` continuity logic and record
  parser (adapted), `FileIndex` public API used by `prism-app`/`prism-ui` (`init`, `begin_search`,
  `search_with_generation`, `rebuild`, `warm`, `sync_volume` or its replacement, status/coverage),
  `quick_access`, thumbnails.
- Goes: `db.rs` (file catalog parts), `scanner.rs` (replaced by the FileId walk), `watcher.rs`,
  the SQLite sync machinery of `name_index.rs` (catch-up, reload, deleted-id queues, by-id fetch),
  `ntfs/mft.rs`, `ntfs_nodes`/`ntfs_fts`. `rusqlite` stays (used by `query/jev.rs`).

## Gates

1. Unit tests on the host (pure, no real user folders): store mutations, ancestry rule, compaction,
   snapshot round trip + corruption, journal record parsing and fold.
2. Ranking parity on the host: the same entry set ranked by the old `NameIndex` and by the v2 store
   gives the same top results for a query list (`examples/rank_audit.rs` queries + random names).
3. Sandbox only: real walk + journal on the guest's `C:` (create, rename folder, move to recycle bin,
   delete tree, burst of 100 k files), then compare the store against a fresh walk: zero
   differences. Restart (snapshot + replay) and compare again. Journal wrap forced by a tiny
   checkpoint -> rebuild path.
4. Sandbox only: app run, palette file search works, `indexing` state during the first build.
5. Numbers (sandbox, synthetic tree of ~1 M files): snapshot size, RAM, build time, CPU per 10 k
   journal records; compared with 1.0.28 on the same tree.
