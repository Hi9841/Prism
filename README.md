# prism-gpui

The Prism launcher with a native GPUI (Rust) user interface, in place of the React and WebView2 UI.
The goal is the same design, layout, and behavior as the current Prism (1:1), with less memory and no web view.

## Status

| Step | State |
|---|---|
| 0. Feasibility (`migration/00-feasibility-gpui.md`) | Done. Signed off by the owner on 2026-10-09. |
| 0b. Parity judge (`migration/judge`) | Built and validated. Waits for sign-off. |
| 1. Design document, dependency map, gap inventory | Not started. |

## Layout

```
migration/
  00-feasibility-gpui.md   feasibility report and verdict
  cost-log.tsv             one line per gate
  judge/                   parity judge: scenarios, goldens, adapters, mutation check
```

## Run the judge

The judge needs Bun and a checkout of the original app at `../Prism` (or set `PRISM_ROOT`).

```
bun migration/judge/judge.mjs        # every scenario against the original
bun migration/judge/mutate.mjs       # shows that the judge fails on broken copies
```
