# Findings

Raw data: `C:\Users\hi\prism-scratch\io\` (mon.log = 2 s samples of the owner's Prism, pid 12808,
release 1.0.25, `target/release/prism.exe` = installed exe; dump1.txt = all-thread stacks).

## F1. The spike is a scope-repair walk of C:\Users\hi (measured)
- 20:43:52-20:46:47: thread `prism-core` (tokio blocking pool) 32-66% of a core, reads 50-148 MiB/s in
  ~4 KiB ops (12-24 k ops/s, SQLite page size 4096). Writes ~0.
- Stack: `FileIndex::request_scope_repair` -> `reconcile_scope_sync` -> `scanner::repair_scope` ->
  `read_folder` (CreateFileW per folder) + `db.children_of` per folder.
- `deferred_scopes` table: `C:\Users\hi` (stamp 20:43:20, recorded by THIS run, 2.3 min after start)
  and `C:\Users\hi\AppData\Local` (20:24, previous run).
- Catalog rows under C:\Users\hi: 4,582,780 of ~4.7 M (623,687 folders). Under AppData\Local: 776,085.
  So the walk is a near-full sweep of the catalog.

## F2. Why C:\Users\hi became a hot scope (code)
- `watcher.rs` `hot_scopes`: each event counts toward every ancestor folder down to
  `min_depth = root components + 2`. For root `C:\` that excludes only `C:\` and `C:\Users`.
- The lowest ancestor that sums >= 512 unclaimed events in one flush is deferred. Diffuse churn
  (many folders, each < 512) climbs to `C:\Users\hi`.
- Nothing bounds the cost of the walk that the deferral schedules. Depth is not the cost:
  AppData\Local is deeper and holds 776 k rows.
- `DEFERRED_MAX_AGE` 5 min: under continuous churn the scope is walked every 5 min and re-deferred.

## F3. One deferral = up to 4 near-full walks (measured + code)
- The walk ended 20:46:47; an identical walk started 20:47:17 = `SCOPE_REPAIR_RETRY_FIRST` (30 s).
  `deferred_scopes` still holds both scopes, so the walk returned Incomplete.
- `mod.rs` retries an Incomplete walk after 30 s, 2 min, 8 min (`SCOPE_REPAIR_MAX_INCOMPLETE` 4).
- `scanner.rs`: `FolderRead::Denied` sets `complete = false`. A folder that is always denied makes
  every walk incomplete, so the retries cannot succeed. Walk-level `complete` has no other user.
- Evidence (denied.txt, read-only walk with the scanner's rules): 4 folders always answer
  ERROR_ACCESS_DENIED: AppData\Local\hermes\pending_messages, AppData\Local\hermes\cache\terminal-output,
  AppData\Local\ElevatedDiagnostics, .unsloth\llama.cpp. Every walk of C:\Users\hi or AppData\Local is
  therefore Incomplete: 4 walks per deferral, by design.
- Live timeline (mon.log): walks 20:43:52-20:46:47, 20:47:17-20:49:58, 20:50:02-... (2 repair slots;
  AppData\Local and C:\Users\hi both requeued/deferred).

## F4. The broad scope recurs under the owner's normal churn (simulation on real events)
- `hotsim.py` (Python port of `hot_scopes`) on 142 s of recorded C:\Users\hi events (Better-Cargo fixture
  builds, .gradle): C:\Users\hi is hot in 5 of the hidden 10 s windows, 4 of the visible 0.75 s windows.
- Hidden flush window is 10 s (`HIDDEN_FLUSH_AGE`), so 512 events = 51 events/s, not the 700/s the
  constant's comment assumes.
- Catalog rows of the scopes it picks: useful ones 0-3,829 rows (debug\build 3,778, debug\incremental 25,
  cit 0, .gradle transforms 0); harmful ones 185,390 (Better-Cargo\fixtures), 776,085 (AppData\Local),
  4,582,780 (C:\Users\hi). A 50,000-row cap separates them by >10x on both sides.
- A bounded count (LIMIT) of 185 k rows took 1.07 s cold: the check must run outside the queue lock.

