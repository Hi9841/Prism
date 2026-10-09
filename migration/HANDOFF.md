# Handoff: Prism → GPUI port, Step 3 (translation), in progress

Paste this into a new session. Write in Simplified Technical English.

## Owner rules (binding)

- **No subagents.** Usage is limited. Do all work inline in one session.
- **No unit tests.** Verify with `cargo check`, the parity judge, and by running the real app.
- Keep behavior and UI 1:1 with the original. The original is the spec.
- Headless checks only while the owner uses the machine. Never send global input (`SendInput`) and never
  open or focus a window on the desktop without asking first. In run 1 of Step 2 this happened while the owner played a game.
- No agent co-authors in commits.
- Shell quirks: set `TMP` and `TEMP` to a repo-local folder (`target/tmp`) before cargo or bun.
  The `sqlite3` on PATH is 32-bit; use Python. In the Bash tool, heredocs that contain an unbalanced
  `'` fail to parse ("unexpected EOF"). Write files with the Write tool, or run a Python script from a file.
  Python `open(p, 'w')` writes the Windows code page: always pass `encoding='utf8'` (a `§` broke a build once).

## Where things are

- Port repo: `C:\Users\hi\Desktop\Work\prism-gpui`. Read `CLAUDE.md`, `README.md`, `migration/RULEBOOK.md` (draft 3)
  first. Process: migration kit at `C:\Users\hi\Desktop\skills\migration-kit` (README, CLAUDE.md, `prompts/04`).
- Original app (spec, do not edit): `C:\Users\hi\Desktop\Work\Prism`. The owner has uncommitted work there
  (not in `src/`: `src/` equals HEAD `4bc2404`).
- Backend extraction target: git worktree `C:\Users\hi\Desktop\Work\Prism-core`, branch `gpui/prism-core`
  (created from Prism `4bc2404`, still unchanged).
- Work queue: `migration/manifest.tsv` (44 units). Status:
  `bun C:/Users/hi/Desktop/skills/migration-kit/scripts/queue_runner.mjs status` (12/44 at handoff).
- Judge: `bun migration/judge/judge.mjs` (original). Port: `cargo build -p prism-judge`, then
  `PRISM_ADAPTER_CMD=target/debug/prism-judge.exe bun migration/judge/judge.mjs`.

## State at handoff (2026-10-10)

Subsystems 1 and 2 are done and committed. **The judge passes 277/277 against the port** (and 277/277 against
the original). A deliberate break (`RECENT_LIMIT` 5 → 4) made the port fail 7 scenarios, so the judge sees the port.

- `crates/prism-ui` (crate root `src/prism_ui.rs`; feature `gpui` is on by default, but no GPUI code exists yet):
  - `js.rs`: JS semantics (trim set, UTF-16 length, number printing, `toPrecision`, `Math.round`, `**`, min/max, truthiness).
  - `lib/`: `math`, `search`, `types`, `query`, `icon_loading`, `launcher_motion`, `emoji` (`icu_collator`, Windows UI
    language through `sys-locale`, numeric on).
  - `lib/types.rs`: `RowAction::execute(&mut impl Invoke)` is the one copy of the action semantics (stop at the first
    rejected call; `TogglePin` reads the pin state, a rejection counts as false, then sets `!truthy(reply)`).
    The UI must run actions through it (implement `Invoke` on the backend handle); the judge's recorder does.
  - `features/palette/{phase1,sections}.rs`, `features/updater/{update_policy,update_progress}.rs`.
  - `state/app.rs`: the option lists and the sanitizers only. It has a `TODO(port)` for the `AppState` entity;
    the queue already counts the file as done.
- `crates/prism-judge/src/main.rs`: every op of `adapter.mjs`. Its `palette_sections` mirrors the provider the
  way `paletteSources` in `adapter.mjs` does. When `state/palette.rs` exists, move that derivation into it and
  call it from the judge, so the judge checks the real provider code.
- Known `BUG(port)` notes (source defects not reproduced): Object.prototype keys in `id in icons` and in the
  `ICONS` lookup; half surrogate pairs in monograms. `search.rs` keeps one from before.

## Next actions, in order

1. Subsystem 0 in the worktree `../Prism-core` (RULEBOOK §1 A4, §5 item 0, `migration/backend-units.tsv`,
   `migration/backend-commands.tsv`): create `crates/prism-core` (git mv the Tauri-free modules), a `Host` trait
   (emit an event with a JSON payload, app data dir; the runtime goes to a prism-core tokio runtime), port the 10
   coupled modules (catalog/mod, apps, theme, audio, audio_hook, win_key, start_menu, windows_tools,
   taskbar_icon_overlay, taskbar_customization), move the `lib.rs` command bodies to `backend.rs`, and keep the
   Tauri app building (`cargo check` in `Prism-core/src-tauri`). The original judge must still pass on the worktree
   (`PRISM_ROOT=../Prism-core bun migration/judge/judge.mjs`).
2. Then subsystems 3–6 (theme, primitives, palette view, settings, app shell) per RULEBOOK §5,
   with the §2 rules from Step 2 (A5 intercept_keystrokes, A6 pointer drags, A7 threads, A8 window, A9
   on_next_frame, A10 reveal_nearest, A11 rem zoom + ListState::reset). Subsystem 3 adds `AppState` to `state/app.rs`.
3. After each subsystem: append a line to `migration/cost-log.tsv`, rerun the port judge, and commit.

## Facts to remember

- Incremental `cargo check` takes about 1 s; a cold gpui build takes about 90 s. `cargo check -p prism-ui
  --no-default-features` skips GPUI.
- GPUI 0.2.2: `PopUp` windows are tool windows but not topmost; there is no show API (use `ShowWindow` through Host);
  `activate_window` injects an Alt key; `request_animation_frame` panics outside paint.
- Chromium wheel: `-lines × 100/3 ÷ (zoom/100)` CSS px. Reduced motion = `!SPI_GETCLIENTAREAANIMATION`.
- D3 (shipping) is still open; it is the owner's decision.
