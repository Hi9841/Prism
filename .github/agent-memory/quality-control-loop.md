# Agent Memory: Prism quality control loop

Standing feedback for future `Agent: Prism quality control loop` runs. This is the
human-on-the-loop steering channel: it is loaded before selection on every run and
passed to the actuator. Deterministic exclusions below are consumed by
`scripts/control-quality.ts`; edits here change future behavior, not just one PR.
Keep durable guidance only — not one-off instructions or single-run logs.

## Guidance

- Set point: drive the non-test panic-surface count in `src-tauri/src`
  (`unwrap` / `expect` / `panic!`) toward zero. Currently 25 sites.
- Scope is `src-tauri/src` non-test code only. Never touch test modules, `src/`,
  or CI files.
- One site per run. Behavior-preserving changes only. The full gate
  (`bun run lint` / `test` / `build` + `cargo fmt --check` / `clippy -D warnings` / `test`)
  must stay green before commit.
- Selection priority (controller): `lock` → `parse` → `bare` → `expect`.
- The two startup `.expect("...")` sites (`src-tauri/src/lib.rs:274`,
  `src-tauri/src/power.rs:130`) are documented, unrecoverable-startup panics and
  are the last things the controller will select. If the team decides they are
  acceptable, exclude them below rather than rewriting them.
- Do not "fix" a site by replacing `.unwrap()` with `.expect("...")` — that does
  not reduce the metric.

## Exclusions

Machine-readable. `scripts/control-quality.ts` consumes only active bullet
lines with these forms (the controller does not strip HTML comments, so keep
examples inline in prose rather than as bullets):

- `- exclude: <file>:<line>` — skip one site
- `- exclude: <file>` — skip a whole file
- `- exclude: kind=<kind>` — skip a whole kind (unwrap/expect/panic/todo/unimplemented)
- `- exclude-pattern: <regex>` — skip sites whose `file:line` or snippet matches

To make an exclusion active, add a bullet at the start of a line, e.g. add the
line `- exclude: src-tauri/src/lib.rs:274` to accept the startup `expect` in
`main`. There are currently no active exclusions.
