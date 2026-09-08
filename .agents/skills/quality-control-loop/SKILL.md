---
name: quality-control-loop
description: remove one measured panic-surface site (unwrap/expect/panic) in src-tauri/src to drive the non-test panic-surface count toward zero, preserving behavior; follow the repo's existing Result<String>/map_err/ok_or_else style instead of inventing a new one
---

# Quality control loop: remove one panic-surface site

Use this skill when the recurring `Agent: Prism quality control loop` workflow (or a
manual run of `scripts/control-quality.ts`) selects a target for you to fix.

The goal: remove exactly one non-test `unwrap` / `expect` / `panic` site from
`src-tauri/src` without changing behavior, so the measured panic-surface count
decreases by one each run.

## Core Requirements

- Only edit the single site named in "Selected work" (the controller output).
  Do not fix adjacent sites in the same run.
- Only touch files under `src-tauri/src/`. Never edit tests, `src/`, build
  scripts, or CI files.
- Behavior-preserving: the normal (non-panicking) code path must stay
  identical in effect. Convert a panic into a handled error or a
  poison-recovering lock — never into new logic.
- Follow the repo's existing error style (`Result<T, String>`, `.map_err(...)`,
  `.ok_or_else(...)`). Do not introduce new error types or crates.
- The change must make `bun scripts/sense-quality.ts` report the metric one
  lower than before and remove the edited line from its site list.

## Golden patterns (source of truth)

The repo already contains the patterns to follow. Read these files before editing:

- `src-tauri/src/catalog/db.rs` — lock poisoning: `.lock().map_err(|e| e.to_string())?` inside `Result`-returning functions.
- `src-tauri/src/audio.rs` — error context: `.map_err(|error| format!("context: {error}"))?`.
- `src-tauri/src/catalog/ntfs/usn.rs` — `Option` → error: `.ok_or_else(|| "...".to_string())?`. Note `read_u16`/`read_u32`/`read_u64`/`read_i64` return `Option`.

For each category, use the matching replacement:

| Category | Replacement |
| --- | --- |
| `lock` | Enclosing fn returns `Result`: `self.x.lock().map_err(|e| e.to_string())?`. Enclosing fn cannot return an error: `self.x.lock().unwrap_or_else(|poisoned| poisoned.into_inner())`. |
| `parse` (`read_*().unwrap()`) | `read_u64(bytes, 8).ok_or_else(|| "USN v2 record too short for FRN".to_string())?` — the enclosing fn already returns `Result<_, String>`. Use a message specific to the field being read. |
| `bare` | Inspect the enclosing fn. Prefer `?`, `ok_or_else`, or `match`. If the value is genuinely invariant, propagate the error anyway rather than keeping a panic site. |
| `expect` / `panic` | Only touch if the controller selected it. A startup `expect("...")` with a clear message in `main` is usually acceptable and can be excluded via the memory file rather than rewritten. |

Do **not** "fix" a site by replacing `.unwrap()` with `.expect("...")`: that
leaves the panic-surface count unchanged. The goal is to remove the site.

## Workflow

### 1. Read the selected target

Read the controller output (passed in the prompt as "Selected work"). Note the
file, line, kind, and category.

Completion criterion: you can state the exact site and its category.

### 2. Inspect the source of truth

Open the file at the target line plus the golden-pattern files listed above.
Determine whether the enclosing function returns `Result` — this decides the
`lock` replacement.

Completion criterion: you know the enclosing function's return type and which
golden pattern applies.

### 3. Make the smallest safe change

Edit only the one site, matching the replacement from the table. For `parse`
sites, keep the error message specific to the field being read. Do not reorder,
rename, or refactor anything else.

Completion criterion: `bun scripts/sense-quality.ts` reports the metric
decreased by exactly 1 and the edited line is gone from the site list.

### 4. Validate

Run the full gate and confirm it stays green:

```powershell
bun run test
bun run build
cargo fmt --manifest-path src-tauri\Cargo.toml --all -- --check
cargo clippy --manifest-path src-tauri\Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri\Cargo.toml
```

Do **not** run `bun run lint`: it is broken at HEAD (no `biome.json` in the repo, plus
Windows CRLF) and is intentionally excluded from the gate. Do not try to fix it here.

Completion criterion: every command passes. If one fails, fix only what your
change broke and re-run; if it still fails, name the exact blocker in your
final response.

### 5. Format the response

Read `references/response-template.md` and format your final answer exactly as
it specifies. This becomes the PR body.

Completion criterion: the response follows the template and includes the
before/after metric count.

## Review Checklist

- Exactly one site was edited; no adjacent cleanup.
- The replacement follows a golden pattern from this repo.
- The normal code path is behavior-identical.
- `scripts/sense-quality.ts` metric decreased by exactly 1.
- The full validation gate passes.
- The final response follows the response template.
