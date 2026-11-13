# Progress

- 20:43 Started `iomon.py` (2 s samples) on the owner's Prism; `stackdump dump` of all threads.
- 20:45 Read-only counts on the catalog (python sqlite3, mode=ro).
- 20:48 Started `denied.py` (read-only walk of C:\Users\hi) and `fsevents.py record` (600 s of
  C:\Users\hi name events -> home-600s.jsonl) to see the churn pattern.
- 20:52 Confirmed denied folders (denied.txt) -> F3. Simulation on real events (hotsim.py) -> F4.
- 20:53 Harness: `churn_bench --ballast N` (static files below work\hi\ballast) + `churn_ab.py --ballast`.
  Baseline exe: `C:\Users\hi\prism-scratch\io\bin\churn_A.exe` (HEAD 50ae7b60 + ballast option).
  Replay slice: `io\rec-hi-180s.jsonl` (first 180 s of home-600s.jsonl, paths prefixed `hi\`, own scratch
  and catalog paths dropped; 85,800 events; simulation: work\hi hot in 6 hidden windows).
- 20:58 Fix delegated (Sonnet 5.5 high): Change 1 remove incomplete-walk retry; Change 2 defer a scope only
  when the catalog holds <= 50,000 rows below it (count outside the queue lock).
- 20:59 Sanity run of A (ballast 100,000, settle 90 s, PRISM_CATALOG_DEBUG=1 for repair-pass lines).
- 21:01 Sanity A (ballast 100k): work\hi deferred and walked once (126,400 entries, 8.1 s); 0 differences.
- 21:02-21:26 A B B A (ballast 100k, settle 90, replay 180 s), raw: io\result-r1-*.txt, io\repairs-r1-*.txt.
  CPU ms A 4969/4875, B 5531/5922; read MiB A 257.6/257.4, B 270.1/233.7; write MiB A 196.5/195.9,
  B 206.6/182.4; converged ms A 38000/37435, B 36916/41512; differences 0 in all runs.
  A walks work\hi (126k entries) once per run; B never walks it, only scopes <= 8,980 entries.
  Reading: at 126k rows a whole-home deferral is near break-even with per-event writes (B +0.8 s CPU);
  the real home is 4.58 M rows. Next: scaling check at ballast 1,000,000 (A then B, 1 run each).
- Subagent result: gates pass (build, 44 tests, clippy no new warnings). mod.rs +30/-135.
- 21:27-21:59 Scaling check, ballast 1,000,000, 1 run per side (raw io\result-big-*.txt):
  A CPU 7906 ms, read 735.2 MiB, write 159.1 MiB, converged 47.5 s, walk of work\hi 1,035,400 entries 9.9 s.
  B CPU 7078 ms (-10%), read 500.4 MiB (-32%), write 168.0 MiB, converged 40.3 s, largest walk 8,980 entries.
  0 differences both.
- 22:10 Gates (main): cargo test -p prism-core 44/44 pass; release build of prism-app + all prism-core examples ok.
  Judge: NOT RUN on this diff. `cargo build -p prism-judge` fails at HEAD in prism-ui
  (`use gpui::SharedString` in crates/prism-ui/src/lib/types.rs, added by 89d02ea3, pre-existing, not
  touched). The judge adapter does not link prism-core, so it does not cover this change.
- 22:20-23:26 G-002 (follow-up subagent: nested records, sweep clears). Seeded startup check B vs C and
  1 M repeat A C C A; numbers in oploop/LEDGER.md session G. Decision: keep G-001 + G-002.
