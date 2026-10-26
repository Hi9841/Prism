@C:/Users/hi/Desktop/skills/code-migration/CLAUDE.md

# prism-gpui

Port of the Prism launcher UI from React/Tauri WebView to pure GPUI (Rust).
The process is the migration kit at `C:/Users/hi/Desktop/skills/code-migration` (skill `code-migration`).

## Decisions (signed off 2026-10-09)

- Posture: redesign of the UI host. The rulebook is a design document; the bakeoff does not apply.
- UI stack: upstream GPUI (`gpui` crate), not GPUI Kit. Use the `build-gpui-apps` skill's direct-upstream path.
- Model plan: Opus for the design document, amendments, and adversarial reviewers. Sonnet for implementers and fixers.
- Old code (the executable spec): the repo root (`src/`, `src-tauri/`). Keep it runnable; the judge runs against it.
- The backend is `crates/prism-core` in this folder. The Tauri host in `../src-tauri` also uses it. Do not fork it.
- This folder is self-contained (decided 2026-10-27): nothing here may depend on a path outside `gpui/`,
  except `migration/judge`, `migration/parity`, and `scripts/extract-icons.mjs`, which read the original app on purpose.

## The judge

`migration/judge` answers "is the port done?". It talks to an implementation only through an adapter
process (JSON on stdin, JSON on stdout).

- `bun migration/judge/judge.mjs` runs every scenario against the original (`PRISM_ROOT`, default: the repo root, which holds the original React/Tauri app).
- `PRISM_ADAPTER_CMD=<path to the port's adapter exe> bun migration/judge/judge.mjs` runs it against the port.
- `bun migration/judge/capture.mjs` re-records `goldens/` from the original only. Never capture from the port.
- `bun migration/judge/mutate.mjs` proves the judge catches breakage in copies of the original.

## Environment

This shell cannot write to `%TEMP%`. Before cargo or bun test commands, set
`TMP`/`TEMP` to a folder inside the repo (for example `target/tmp`).
