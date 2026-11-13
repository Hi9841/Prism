# Task plan: Prism background disk/CPU spikes (2026-11-12)

## Goal
Remove the cause of the background spikes the owner saw in Task Manager (Prism hidden: 4.9% CPU,
183.7 MB/s disk, "Very high" power; normal: 0.3%, 0.9 MB/s).

## Current state
Phases 1-5 complete. Decision: keep (uncommitted, not installed). Waiting for the owner.

## Next action
Owner decides: commit + release/install; fix of the pre-existing prism-ui judge build break.

## Scope and constraints
- Files: `crates/prism-core/src/catalog/{watcher.rs,mod.rs,scanner.rs,db.rs}`.
- Subtract before add (owner, 2026-11-12). Main agent finds and measures; a Sonnet (high)
  subagent implements fixes (memory: main-finds-sonnet-fixes).
- Never `cargo clean`. TMP/TEMP inside the repo. Do not stop the owner's Prism or other processes.
- No commit, no release, no install without the owner's go-ahead.

## Completion criteria
- One hot burst can never schedule a repair walk of a scope with > 50,000 catalog rows (gate: churn replay
  with ballast shows no walk of work\hi; read ops drop).
- A walk that is incomplete because of a permanently denied folder is not repeated (gate: code + existing tests).
- `cargo test -p prism-core` passes; judge 277/277; churn replay has 0 differences.
- Before/after: churn replay CPU and read bytes, no regression > noise.

## Phases
1. Baseline measurement of the live app. Status: complete (findings.md).
2. Root cause. Status: complete.
3. Fix (subagent), smallest diff, subtraction first. Status: complete.
4. Verify: tests, judge, churn replay A/B. Status: complete.
5. Report keep/revert. Status: complete.

## Decisions
- Remove the incomplete-walk retry (subtract): denied folders are permanent, so a retry cannot succeed;
  unread folders go to the periodic sweep (the old 4th-try behavior). Err/Skipped keep the record.
- Bound deferral by catalog rows below the scope (50,000): cost is subtree size, not depth (F4).
- No new unit tests (owner rule); end-to-end gate is the churn replay with ballast.

## Blockers
- Judge gate blocked by a pre-existing prism-ui build break (progress.md 22:10).
