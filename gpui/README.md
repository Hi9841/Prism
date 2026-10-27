# prism-gpui

The Prism launcher with a native GPUI (Rust) user interface, in place of the React and WebView2 UI.
The goal is the same design, layout, and behavior as the current Prism (1:1), with less memory and no web view.

## Status

| Step | State |
|---|---|
| 0. Feasibility (`migration/00-feasibility-gpui.md`) | Done. Signed off by the owner on 2026-10-09. |
| 0b. Parity judge (`migration/judge`) | Done. 277 scenarios, 26/26 mutations caught. |
| 1. Dependency map (`migration/depmap`, `manifest.tsv`) | Done. 50 files, 127 edges, 0 cycles; 44 units (14 backend + 30 UI). |
| 1. Design document (`migration/RULEBOOK.md`) | Draft 3. D1, D2, D4, D5 decided; D3 open. |
| 1. Gap inventory (`migration/inventory.tsv`) | Done. 1,428/1,428 sites covered by 118 rows. Spike rows settled at Step 2 (row 112 waits for Step 5). |
| 2. Design review and spike (`migration/stress-test/step2-report.md`) | Done 2026-10-09. 7 of 8 spike rows settled (1:1 with headless Chromium baselines); OLE drag moved to Step 5. Rulebook draft 3 (A1, A2, A4-A11). Manifest: 44 units. |
| 3. Translation | Done 2026-10-10. Queue 44/44 (`queue_runner.mjs status`). |
| 4-5. Compile, run | Done. Shipped as Prism 1.0.0 on 2026-10-12; current release 1.0.10. |
| 6. Behavior match | Done. Judge 277/277 against the original and the port. UI parity mean 9.90/10, min 9.67 (9.91 without the two menu captures that differ on purpose); UX 37/37 (`migration/parity`). Re-checked 2026-10-27. Port-only changes are in the RULEBOOK deviation log; the optimization loop is in `oploop/LEDGER.md`. |

## Layout

This folder is self-contained: it builds with nothing outside it. The original React/Tauri app at the
repo root (the parity spec) depends on `crates/prism-core` here, not the other way around.

```
crates/
  prism-core/              backend (catalog, apps, query, power, audio, taskbar, Win key); no Tauri
  prism-ui/                views and state (folders mirror the original's src/)
  prism-app/               the prism.exe shell: window, hotkey, tray, updater host
  prism-judge/             JSON adapter that the parity judge drives
packaging/                 build-installer.ps1 and the NSIS script
vendor/gpui/               gpui 0.2.2 with the patches in PRISM-PATCHES.md
migration/
  00-feasibility-gpui.md   feasibility report and verdict
  RULEBOOK.md              design document and deviation log
  cost-log.tsv             one line per gate
  judge/                   parity judge: scenarios, goldens, adapters, mutation check
  parity/                  visual and UX parity against the original in headless Chrome
```

## Build

```
cargo build -p prism-app                 # target/debug/prism.exe
bun install                              # Tauri CLI, used only to sign the installer
.\packaging\build-installer.ps1          # release exe, NSIS installer, signature
```

## Run the judge

The judge needs Bun and the original app, which is the repo root (or set `PRISM_ROOT`).

```
bun migration/judge/judge.mjs        # every scenario against the original
bun migration/judge/mutate.mjs       # shows that the judge fails on broken copies
```

To run it against the port, build the adapter and point the judge at it:

```
cargo build -p prism-judge
PRISM_ADAPTER_CMD=target/debug/prism-judge.exe bun migration/judge/judge.mjs
```
