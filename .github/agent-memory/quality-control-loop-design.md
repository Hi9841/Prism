# Design: Prism quality-control loop

An agentic control loop that drives the non-test "panic surface" of the Rust
backend toward zero — one small, reviewable, behavior-preserving change per run.

- Task slug: `quality-control-loop`
- Confirmed set point (coordinator, 2026-09-08): drive the non-test panic-surface
  count in `src-tauri/src` toward zero, one site per run, lowest-risk first.
- Repository: Prism — Tauri v2 + React 19 + Bun (Windows-only native code).

## Set point

- **Metric**: number of non-test `unwrap` / `expect` / `panic!` / `todo!` /
  `unimplemented!` sites in `src-tauri/src`. Baseline at design time: **25**
  (15 `lock`, 8 `parse`, 2 `expect`), with 356 more in test-only code (out of scope).
- **Direction**: the metric decreases by exactly 1 per run, toward 0, while every
  validation command stays green.
- **Scope**: may change `src-tauri/src/**` non-test code only. May read (never
  change) `src/`, tests, `scripts/`, CI, and build files.

## Sensor — `scripts/sense-quality.ts`

A deterministic, dependency-free Bun script. It walks `src-tauri/src/**/*.rs`,
excludes `#[cfg(test)]` modules via brace-tracking, and emits one JSON document:
`metric` (the non-test count), per-kind/per-category counts, and every site
(file, line, column, kind, category, snippet).

- Categories: `lock` (`.lock().unwrap()`), `parse` (`read_*().unwrap()`),
  `bare`, `expect`, `panic`, `todo`, `unimplemented`.
- Repeatable and cheap (no compile), so it cannot be silently disabled by an
  inline lint directive.
- Run locally: `bun scripts/sense-quality.ts`.

## Controller — `scripts/control-quality.ts`

Deterministic selection. It reads the sensor JSON, applies machine-readable
exclusions from `.github/agent-memory/quality-control-loop.md`, orders remaining
sites by category priority (`lock` → `parse` → `bare` → `expect` → `panic` →
`todo` → `unimplemented`) then by (file, line), and selects one (configurable
`--batch N`).

- Emits a markdown target brief consumed by the actuator prompt.
- The priority policy is the part to tune over time; it lives here, not in the
  skill, so feedback edits the memory file, not the code.

## Actuator — coding agent + repo-local skill

- **Agent**: Claude Code (headless), swappable for Codex/OpenCode/CodeLayer per
  `scripts/` notes and the workflow comments.
- **Skill**: `.agents/skills/quality-control-loop/SKILL.md` encodes the golden
  patterns found in the repo:
  - lock poisoning → `.lock().map_err(|e| e.to_string())?` (as in `catalog/db.rs`),
    or `.lock().unwrap_or_else(|poisoned| poisoned.into_inner())` in void contexts;
  - `Option` → `.ok_or_else(|| "...".to_string())?` (as in `ntfs/usn.rs`);
  - error context → `.map_err(|error| format!("context: {error}"))?` (as in `audio.rs`).
- **Response format**: `references/response-template.md` — the PR body.
- **Validation (must be green before commit)**:
  `bun run lint`, `bun run test`, `bun run build`,
  `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`.

## Disturbances

- Teammates' commits to `src-tauri/src` (may add or remove sites between runs).
- Dependency bumps (`bun.lock`, `Cargo.lock`, windows/webview2-com crates) that
  can shift clippy/lint results.
- Windows API churn — the code is Windows-only, so the gate must run on a
  Windows runner.
- The loop re-measures every run, so a teammate adding sites simply raises the
  metric and the loop keeps working; it does not assume a monotonic baseline.

## Dampener (offered, not wired in v1)

A PR/push check comparing `scripts/sense-quality.ts` against the metric at the
merge base, surfacing (advisory) newly introduced panic-surface sites so the
problem can't regress while the loop chips away. Not implemented in this pass;
the weekly loop plus the green gate is the current backstop.

## Cadence

- `workflow_dispatch` (manual) — the primary trigger while tuning.
- Weekly cron (Monday 13:00 UTC).
- One open PR bound: scheduled runs no-op when an open PR with label
  `agent-quality-control-loop` already exists; manual dispatch bypasses the bound.

## Memory / feedback

- `.github/agent-memory/quality-control-loop.md` is loaded before selection and
  passed to the actuator. Its Exclusions section is machine-read by the
  controller; its Guidance section is human-read.
- `/iterate` on the PR (via `scripts/agent-iteration.ts`) lets a maintainer
  update the PR and distill durable guidance back into the memory file.

## Future loop (recorded, not this one)

Option B from the set-point discussion: close remaining open findings in
`AUDIT.md` (27 deep-audit findings, v0.6.7) one per run. Deferred because many
are performance/behavior changes (not "no behavior change") and the file is
stale relative to v0.9.39. A future loop could use a sensor that parses
`AUDIT.md`/`APP_AUDIT.md` for open items and a controller that picks one
P3/low-risk item.

## Known limitation

The metric covers `unwrap`/`expect`/`panic!`/`todo!`/`unimplemented!`. Panic via
direct slice/array indexing is not counted (static detection is noisy). If the
team wants indexing on the metric later, extend `sense-quality.ts` with an
advisory indexing category — do not change the current 25-site baseline.
