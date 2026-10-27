@C:/Users/hi/Desktop/skills/code-migration/CLAUDE.md

# Prism

The Prism launcher: a native Rust app with a GPUI interface. It replaced the original React/Tauri app
(releases 1.0.0 to 1.0.11). The port followed the migration kit at
`C:/Users/hi/Desktop/skills/code-migration` (skill `code-migration`).

## Decisions

- UI stack: upstream GPUI (`gpui` crate, vendored in `vendor/gpui` with `PRISM-PATCHES.md`), not GPUI Kit.
  Use the `build-gpui-apps` skill's direct-upstream path.
- The original React/Tauri app (the port's behavior spec) left the repo after release 1.0.11 (decided
  2026-10-27). Its behavior lives on in the judge's goldens (`migration/judge/goldens`) and in the design
  rulebook (`migration/RULEBOOK.md`, with its deviation log). Git history keeps the app.
- The repository holds the whole product: nothing in it may depend on a path outside it.

## The judge

`migration/judge` checks the port's behavior against goldens captured from the original. It talks to the
implementation only through an adapter process (JSON on stdin, JSON on stdout).

- `cargo build -p prism-judge`, then `bun migration/judge/judge.mjs` runs every scenario against the port
  (`target/debug/prism-judge.exe`; `PRISM_ADAPTER_CMD` selects another adapter).
- The goldens cannot be captured again: the original is gone. A behavior change on purpose updates the
  golden by hand and gets a line in the RULEBOOK deviation log.

## Environment

This shell cannot write to `%TEMP%`. Before cargo or bun test commands, set
`TMP`/`TEMP` to a folder inside the repo (for example `target/tmp`).
