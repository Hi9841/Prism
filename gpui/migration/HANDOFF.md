# Handoff: Prism → GPUI port, Step 3 done, parity measured

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
- Backend: git worktree `C:\Users\hi\Desktop\Work\Prism-core`, branch `gpui/prism-core`, commit `0573388`:
  `crates/prism-core` (Tauri-free, `Host` trait, tokio runtime, `Backend` command bodies). Its `src-tauri` is a
  thin Tauri host over prism-core and still builds (it needs `dist/`, copied from `../Prism/dist`, untracked).
- Work queue: `migration/manifest.tsv` (44 units). Status:
  `bun C:/Users/hi/Desktop/skills/migration-kit/scripts/queue_runner.mjs status` (12/44 at handoff).
- Judge: `bun migration/judge/judge.mjs` (original). Port: `cargo build -p prism-judge`, then
  `PRISM_ADAPTER_CMD=target/debug/prism-judge.exe bun migration/judge/judge.mjs`.

## State (2026-10-10)

The port is complete: backend (prism-core), every view, and the app shell (`crates/prism-app`). Scores:

- Judge: 277/277 (`cargo build -p prism-judge`, then `PRISM_ADAPTER_CMD=target/debug/prism-judge.exe bun migration/judge/judge.mjs`).
- UI parity: mean 9.80/10 over 33 captures, min 9.51 (zoom-120: text at 16.2 px). `bun migration/parity/run.mjs`.
- UX parity: 37/37 scenarios (10.0); their captures have a mean of 9.78. `bun migration/parity/run.mjs --ux`.
- 2026-10-11: thin Chromium scrollbars added (gutter only on overflow, arrows, thumb drag, track click).
  The reference now draws scrollbars too (`--hide-scrollbars=false` hid them: Chrome only checks that a
  switch is present). With it: UI mean 9.78, min 9.49 (zoom-120); UX 36/37 (`ux-context-keys` differs
  on purpose: Open is the first menu item); judge 277/277. The review findings of 2026-10-11 are fixed,
  except: `hooks.nsh` deletes app data on any non-update uninstall (inherited from Tauri; owner decision),
  `repost_size` is a call-site workaround for the patch-6 root cause, and commit `32a03a7` is large.
- Installer tests: copy the installer out of the repo first (for example to a new folder in `%LOCALAPPDATA%`).
  The repo's Low integrity label makes it run at Low integrity: it cannot write TEMP, cannot see Prism,
  and cannot replace the exe. From a normal folder, `/P /R /UPDATE /ARGS --autostart` stopped Prism,
  replaced the exe, and relaunched it (2026-10-11). `Start-Process -Wait` also waits for the relaunched
  Prism; check the processes instead.
- Parity setup: `python -I migration/parity/make-fixture.py target/parity/fixture.json` (local data, never commit),
  `target/parity/original-dist` (vite build of `../Prism`), `cargo build -p prism-app`. Scripts: `run-original.mjs`
  (headless Chrome + `mock-tauri.js`; `PARITY_DEBUG=1` logs pointer events; an `eval` step probes the DOM),
  `prism.exe --parity` (parked window, never activated; an `info` step prints the focused bounds).
- Vendored gpui patches: `vendor/gpui/PRISM-PATCHES.md` (7). Deviation log: `migration/RULEBOOK.md`.

## Update 2026-10-12 (prism-core `gpui/prism-core` at `66c8b81`, local only)

- FFF decision (owner): keep Prism's full-drive catalog; use FFF's matcher (`neo_frizbee`) for typos.
  FFF's `fff-search` as-is indexed 253 k of 4.44 M files on C: (its walker always skips hidden folders,
  `.gitignore`d files, most of Program Files), had no folders, and ranked `ChromeSetup.exe` for `chrome.exe`.
- Typo pass (`catalog/name_index.rs`): runs only when no name is an exact/prefix/substring match.
  Names starting within 2 edits of the query rank above scattered subsequence matches. Queries with a strong
  match are unchanged. 11-23 ms on typo queries, 1-6 ms otherwise. `PRISM_CATALOG_DEBUG` logs its timings.
- `file_fts` dropped (schema v7, -413 MB); `reclaim_space` now really shortens the file (3,927 MB);
  `Database::open` moves the catalog aside only on corruption; a test no longer opens File Explorer
  (it caused "explorer.exe 0xc0000142" dialogs when run from the Low-labelled repo).
- Resources (release build, idle, hidden): 359 MB private (204 MiB is the name index), idle CPU about 0.5%
  of one core-second per 10 s. 69 of 107 threads are NVIDIA's D3D11 driver pool for GPUI's single device
  (one burst at device creation; skipping GPUI's probe device did not change it, so no patch).
- Start test builds with WMI (`Invoke-CimMethod Win32_Process Create`) from `%LOCALAPPDATA%\PrismGpui`.
- Judge 277/277 after the change. Not done: a typed real-app test (needs the owner's go-ahead), the 1.0.0
  installer rebuild, and the open owner decisions below.

## Next actions

1. Real app: run it only from `%LOCALAPPDATA%\PrismGpui\prism.exe` (a copy). The repo folder has an
   inherited Low integrity label, so an exe started from `target/` runs at Low integrity and the catalog,
   the Win-key observer, and the Explorer bridge fail. Quit the installed Tauri Prism first (both own the
   Win key). Tested 2026-10-10: Win toggle, Start-button click, search, launch, Escape, click-away,
   settings, width, Tab focus ring. Test scripts: `target/tmp/scripts/` (`keys.ps1` refuses input unless
   the palette is in the foreground). Trace: `%TEMP%\Prism\semantic-debug.log` (debug builds).
2. Release (the next public Prism release can be this build): bump `version` in `Cargo.toml`, run
   `packaging/build-installer.ps1` (release exe, NSIS installer from Tauri's generated script, signed with
   `~/.prism/signing/updater.key`), then Prism's `scripts/publish-release.ps1` with `-ArtifactsDirectory`
   pointing at `prism-gpui/artifacts` (it writes `latest.json`). Old Tauri builds update in place: same
   endpoint, key, install folder, registry keys, hooks, and `/P /R /UPDATE /ARGS`.
   Tested 2026-10-10 on the owner's PC: Tauri 0.11.9 -> port install as the Tauri updater runs it; in-app
   update 0.90.0 -> 0.90.1 from a local channel (`-UpdateEndpoint`/`-UpdatePublicKey`/`-Version`, test key);
   a tampered signature is refused; silent uninstall; fresh passive install and first run; search, Win key,
   Start-button click, click-away. Also tested: a real Tauri build (0.11.8, `tauri build --config` with the local endpoint and test key)
   updated itself to the port through its own Update button (download, signature, install, restart).
   Performance (PRISM_CATALOG_DEBUG=1 traces catalog work): idle hidden 0.4% of one core; a 40k-file
   burst costs ~16 s CPU and ~1.1 GB reads (was 47 s / 3.3 GB). Remaining cost: trigram FTS upkeep per
   changed name (the FTS serves only the fallback search path).
3. Optional polish: zoom captures (fractional font sizes), launcher-stage scale animation, hover color transitions.
4. D3 (shipping) is the owner's decision.

## Facts to remember

- Incremental `cargo build -p prism-app` takes about 10 s; a cold gpui build takes about 90 s.
- GPUI key listeners belong to elements on the focus path; a canvas's `on_key_event` sits on its own node.
- GPUI focusable elements take focus on mouse down unless a listener calls `window.prevent_default()`.
- Chromium: wheel `-lines x 100/3 / zoom`; scroll offsets on whole pixels; focus reveal is center-if-needed,
  the selected row uses nearest. Reduced motion = `!SPI_GETCLIENTAREAANIMATION`.
