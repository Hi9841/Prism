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
| 3. Translation | In progress. Subsystems 1-2 (pure logic, palette model, `prism-judge`) done 2026-10-10: the judge passes 277/277 against the port. Next: subsystem 0 (`prism-core`). |

## Layout

```
migration/
  00-feasibility-gpui.md   feasibility report and verdict
  cost-log.tsv             one line per gate
  judge/                   parity judge: scenarios, goldens, adapters, mutation check
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
