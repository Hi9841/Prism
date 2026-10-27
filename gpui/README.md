# prism-gpui

The Prism launcher with a native GPUI (Rust) user interface. It replaced the original React and WebView2
(Tauri) app with the same design, layout, and behavior, with less memory and no web view.

## Status

| Step | State |
|---|---|
| Migration | Done. Feasibility signed off 2026-10-09; translated 2026-10-10; shipped as Prism 1.0.0 on 2026-10-12. |
| Behavior match | Judge 277/277 against goldens captured from the original. Last visual parity check against the original (2026-10-27): mean 9.90/10, min 9.67; 107/107 interaction scenarios. Port-only changes are in the RULEBOOK deviation log. |
| Current release | 1.0.11 (2026-10-27). The original React/Tauri app left the repo after this release. |
| Performance | `oploop/LEDGER.md`: every measured change, kept or reverted, with its numbers. |

## Layout

This folder holds the whole product: it builds with nothing outside it.

```
crates/
  prism-core/              backend (catalog, apps, query, power, audio, taskbar, Win key)
  prism-ui/                views and state
  prism-app/               the prism.exe shell: window, hotkey, tray, updater host
  prism-judge/             JSON adapter that the behavior judge drives
packaging/                 build-installer.ps1, the NSIS script, check-version.ps1, publish-release.ps1
vendor/gpui/               gpui 0.2.2 with the patches in PRISM-PATCHES.md
migration/
  00-feasibility-gpui.md   feasibility report and verdict
  HANDOFF.md               facts and procedures from the migration
  RULEBOOK.md              design document and deviation log
  cost-log.tsv             one line per gate
  judge/                   behavior judge: scenarios, goldens (captured from the original)
oploop/                    performance ledger, journeys, E2E bench, functional test of the real app
```

## Build

```
cargo build -p prism-app                 # target/debug/prism.exe
bun install                              # Tauri CLI, used only to sign the installer
.\packaging\build-installer.ps1          # release exe, NSIS installer, signature
```

## Run the judge

```
cargo build -p prism-judge
bun migration/judge/judge.mjs            # every scenario against the port's adapter
```

`PRISM_ADAPTER_CMD=<exe>` runs the judge against another adapter.
