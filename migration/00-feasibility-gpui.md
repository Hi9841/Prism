# Feasibility report: Prism UI from React/Tauri WebView to pure GPUI (Rust)

Date: 2026-10-09. Kit: `C:/Users/hi/Desktop/skills/migration-kit`, prompt `00-feasibility.md`.
Status: read-only survey. No migration step has started.

## The six steps (kit README)

1. Create the map and the rules: dependency map, gap inventory, and rulebook (a design document for a redesign).
2. Stress-test the rules: for a redesign, an adversarial review of the design document. The bakeoff is not valid.
3. Translate everything: one implementer, two adversarial reviewers, and one fixer for each unit, from a queue on disk.
4. Compile: one survey build makes the error queue. Fixers work without compiler access.
5. Run it: hello world, then smoke tests.
6. Match behavior: a judge runs the same scenarios against the old app and the new app.

## Case for leaving (measured on this machine)

- **Memory.** Prism uses 370 MB of working set and 455 MB private memory in total (`Get-CimInstance`, 2026-10-09).
  The six `msedgewebview2.exe` processes use 126 MB of working set and 238 MB private. A GPUI UI removes these processes.
- **IPC boundary.** The UI calls 45 Tauri commands (`src-tauri/src/lib.rs` `generate_handler!`) through `src/lib/bridge.ts` (511 lines).
  Each keystroke crosses IPC at least twice (`query_phase1` and `search_files`) and serializes JSON both ways.
- **Two type universes.** `src/lib/types.ts` (329 lines) copies the Rust serde structs by hand. Drift between them is not checked by a compiler.
- **Search speed is not a reason to leave.** File search was the slow path. It is now fixed in Rust: 1.4–7 ms per query on 4.3 M files
  (`src-tauri/src/catalog/name_index.rs`, `live_catalog_benchmark`). The UI framework did not cause that latency.
- **Not measured:** palette show latency and keystroke-to-paint latency. `PRISM_PERF_LOG` was not set for the running app. See the verdict.

## Three calls

### 1. Structure-preserving or redesign? — Redesign.

React hooks, JSX, Tailwind classes, and CSS motion have no one-to-one equivalent in GPUI.
GPUI uses retained entities, `Render` implementations, and style builder methods in Rust.
Every UI file changes its structure, not only its syntax.

- State lives in React context and hooks: `src/state/app.tsx` (812 lines), `src/state/palette.tsx` (864 lines).
- Styling is Tailwind: 280 `className` lines, plus `src/styles/tokens.css` (703 lines) and 6 `backdrop-filter` sites.
- Motion is CSS and React driven: `src/lib/launcherMotion.ts`, `src/App.tsx`.
- Icons are `lucide-react` components.

The earlier GPUIX attempt reached the same call (`C:/Users/hi/Desktop/Work/GPUIX/Prism-ReWrite/migration/RULEBOOK.md` §0: "This is a redesign of the UI host").

**Committed:** redesign, forced by `src/state/app.tsx`, `src/state/palette.tsx`, `src/styles/tokens.css`, `src/features/palette/Palette.tsx`, `src/components/SettingsSheet.tsx`.

### 2. What does verification cost?

Baseline (measured):

| Referee | Time |
|---|---|
| `bunx tsc --noEmit` | 1.6 s |
| `vite build` (inside `bun run build`) | 19.8 s |
| `cargo build` after `touch src/lib.rs` (debug, `prism` crate) | 7.2 s |
| `cargo test --release` build (warm dependencies) | 2 min 16 s |

Target: every UI edit becomes a Rust compile. Assumption, not measured: a cold `gpui` dependency build takes several minutes,
and an incremental UI-crate build takes tens of seconds. Scaling assumption: the incremental time grows with the size of the crate
that changes. Put the UI in its own crate (`prism-ui`), so that a UI edit does not recompile the 27.7 k-line backend.
The typecheck referee (`cargo check` on the UI crate) is cheaper than a full build. Decide at the Step 3 gate if Step 4 dissolves into Step 3.

**Committed:** verification gets about 5–20 times slower per UI edit (assumption), forced by `src-tauri/Cargo.toml` (one 27.7 k-line crate today) and the `gpui` dependency.

### 3. Do the tests survive? — Too few. Build the judge first.

- Frontend: 155 test cases in 20 files (3,422 lines). None of them can run against a Rust UI.
  - 7 files render React through Testing Library and die. Examples: `src/features/palette/Palette.test.tsx`, `src/components/SettingsSheet.test.tsx`, `src/App.motion.test.tsx`.
  - 13 files test pure TypeScript logic. Their assertions are portable as a specification, but not as code. Examples: `src/features/palette/sections.test.ts`, `src/lib/math.test.ts`, `src/lib/search.test.ts`.
- Rust: 246 tests cover the backend, which stays. They all survive. Examples: `catalog::name_index`, `catalog::search`, `query::jev`.
- No judge hits the UI through its public surface (window, keyboard, painted rows).

**Committed:** build the judge first (`prompts/00b-judge-setup.md`), forced by `src/features/palette/Palette.test.tsx`, `src/state/palette.behavior.test.tsx`, `src/components/SettingsSheet.test.tsx`.

Parity scenario seed for 00b:

- The 13 scenarios in `GPUIX/Prism-ReWrite/migration/judge/scenarios.mjs` (fuzzy ranking, app de-duplication, aliases). They call TypeScript modules today and need a JSON-in/JSON-out adapter to a Rust harness.
- Query to ordered visible rows, for each section: apps, open windows, settings, tools, power plans, math, files.
- Keyboard: arrows, Enter, Escape, Tab, the context menu, and Win-key toggle.
- Settings persistence: change a setting, restart, read `prism.json`.
- Visual parity: screenshots of the palette, settings, and power menu at fixed sizes in the light and dark themes.

## The six steps for this repo

| Step | Meaning here | Prompt |
|---|---|---|
| 0b | Build the parity judge (scenarios above), validated against the current app and against deliberately broken code | `prompts/00b-judge-setup.md` |
| 1 | Design document: GPUI Kit or upstream GPUI (the `build-gpui-apps` skill requires a user decision), state ownership, theme tokens from `tokens.css`, motion, window model. Dependency map with `GPUIX/Prism-ReWrite/migration/depmap_ts.mjs`. Gap inventory: start from the 563-row GPUIX inventory, and add the Tauri plugins (updater, single-instance, global shortcut, clipboard) | `prompts/01`, `prompts/02` |
| 2 | Adversarial review of the design document. One required spike: a transparent, borderless, always-on-top, no-taskbar GPUI window with acrylic or blur, shown by the Win key | `prompts/03` (bakeoff replaced) |
| 3 | About 10 units (subsystems): window and shell glue, app state, palette state, ranking logic (`sections.ts`, `search.ts`, `query.ts`, `phase1.ts`, `math.ts`), palette view, settings sheet, power menu, taskbar customization, updater, theme and motion | `prompts/04` |
| 4 | Survey build of `prism-ui`, then the machine queue | `prompts/05` |
| 5 | Hello world: the window opens and closes on Win. Smoke: type `chrome`, see the app row, press Enter to start it | none |
| 6 | Judge burndown against the old app | `prompts/06` after the gate |

## Cost and duration

Drivers: 10 units × 4 agent passes per unit (implementer, two reviewers, fixer) × a moderate referee price (Rust compile).
Plus the judge, the design document, the inventory, and survey-build rounds.

- Tokens: **tens of millions** (band 10 M–100 M). A precise number is not given.
- Wall clock: weeks, not days.
- Your attention: about 1–3 hours at each of about 8 gates, plus the visual parity review, which only you can sign off.

## Model plan (your decision)

| Phase | Tier | Why |
|---|---|---|
| Design document and amendments | Largest | Every error copies into every unit |
| Skeptic reviewers | Largest | 1:1 visual and motion fidelity is the hard gap |
| Implementers | Mid | Two reviewers and the compiler stand behind them |
| Fixers | Mid | The compiler is the real referee |

## Verdict: later — not now

The goal is a faster Prism. The measured bottleneck was file search, and that is now fixed in Rust (2–7 ms instead of 5–250 ms).
A GPUI port removes about 240 MB of private WebView2 memory and the IPC hops. But it is a full UI redesign,
it needs a judge that does not exist, and 1:1 parity with CSS blur, Tailwind tokens, and CSS motion is not guaranteed in GPUI.

**The fact that changes the verdict:** the latency from Win-key press to first paint, and from keystroke to painted row, in the current WebView.
If either is above about 50 ms and the WebView causes it, migrate. If both are below, do not migrate.

**How to check it in less than one day:** run Prism with `PRISM_PERF_LOG` set and read the `toggle_palette` events.
Add one `performance.now()` mark in `src/state/palette.tsx` from `setQuery` to the next painted frame (`requestAnimationFrame`).
Take 50 samples of each.

## What I read and what I ran

Read: `package.json`, `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`, `src/state/palette.tsx` (search effects),
the catalog modules, the GPUIX attempt (`RULEBOOK.md`, `design-review.md`, `cost-log.tsv`, `judge/scenarios.mjs`),
and the `build-gpui-apps` skill index. Sampled, not read in full: `Palette.tsx`, `SettingsSheet.tsx`, `tokens.css`.

Ran: line and test counts (`git ls-files | wc -l`, `grep -c`), `bunx tsc --noEmit`, `cargo build` (incremental),
and a process memory probe (`Get-CimInstance Win32_Process`).
