# Prism performance report

Measured on Windows in the optimized Tauri release build on 2026-08-08.

## Method

- `scripts/profile-prism.ps1` samples every 100 ms.
- Resource totals include Prism and its descendant `msedgewebview2.exe` runtime processes.
- Apps launched by Prism, including Explorer and WezTerm, are excluded from Prism's resource totals.
- Native timings are written only when `PRISM_PERF_LOG` is set. Normal app use does not write performance logs.
- `before-resources.csv` is the pre-optimization baseline. `final-resources.csv`, `final-timings.csv`, and `final-native.jsonl` are the final release run.

## Feature outcome

- Typing two or more characters returns indexed files and folders alongside applications.
- Absolute paths are browsed directly even when the local index is still warming up.
- Quick Access and Recent sections provide frequently used folders and applications.
- Duplicate Windows app aliases, including the duplicate WezTerm result, collapse to the best launch source.
- Web search and the obsolete "actions" interface wording are removed.
- Standalone Win presses toggle Prism without opening native Start, StartAllBack, Open-Shell, or Classic Shell menus; Win+key system shortcuts remain available.
- Indexed results verify that their paths still exist, and a background reconciliation adds/removes files every 60 seconds.
- Settings segmented controls use a sliding selection surface, and the native width animation stays centered while moving through bounded intermediate sizes.

## Resource results

| Phase | Working set before | Working set final | Change | Private memory before | Private memory final | Change |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Startup, hidden | 344.54 MB | 267.48 MB | -22.4% | 229.88 MB | 149.48 MB | -35.0% |
| Palette visible | 451.25 MB | 391.72 MB | -13.2% | 290.43 MB | 231.18 MB | -20.4% |
| After Explorer | 471.39 MB | 424.16 MB | -10.0% | 299.56 MB | 248.77 MB | -17.0% |
| After WezTerm | 473.69 MB | 425.96 MB | -10.1% | 303.28 MB | 247.69 MB | -18.3% |
| Final, hidden | 472.56 MB | 425.13 MB | -10.0% | 301.81 MB | 246.33 MB | -18.4% |

A second optimized run recorded 407.77 MB working set and 220.66 MB private memory in the final hidden phase. The post-optimization observed range is therefore 407.77-425.13 MB working set and 220.66-246.33 MB private memory. Fully initialized Prism still uses seven processes because WebView2 uses a multi-process browser architecture.

The release executable is 3,747,840 bytes (3.57 MiB).

## Timing results

| Operation | Final time | Notes |
| --- | ---: | --- |
| Startup setup | 286.49 ms | Native setup through Tauri initialization |
| Installed app scan | 3.72 ms | 355 applications |
| Palette reveal | 15.07 ms | Global shortcut to Prism foreground |
| Exact path lookup | 0.18 ms | `C:\Users\hi\Documents\Marvel` |
| Indexed file search | 5.03 ms | Query `wez`, eight file results |
| Explorer native dispatch | 22.15 ms | `ShellExecuteW`; Explorer reused an existing window |
| WezTerm native dispatch | 12.49 ms | Direct resolved executable launch |
| WezTerm visible window | 508.16 ms | Enter key to a new visible WezTerm window |

The cold direct WezTerm dispatch was 24.29 ms. Before the direct executable optimization, launching its Start Menu shortcut took 326.28 ms, so cold dispatch improved by about 92.6%. Warm direct dispatch measured 5.50-12.49 ms.

`final-timings.csv` contains a 9.77 ms Explorer foreground value. That is the time for Prism to hide and reveal the already-open Explorer underneath it, not a cold Explorer window launch, so the native 22.15 ms dispatch trace is the authoritative measurement for that run.

## File index

- Cache format reduced from 39,464,328 bytes to 12,088,913 bytes, a 69.4% reduction.
- In-memory entries now retain one compact path plus a lowercase filename instead of duplicate path, parent, name, and lowercase-path strings.
- Generated dependency/build directories are excluded from indexing.
- A cache up to six hours old provides fast startup, then the bounded 100,000-entry index reconciles every 60 seconds.
- A live refresh test wrote a newly created marker to the cache after 61.3 seconds; after deletion, its exact search returned zero results immediately without waiting for the next scan.
- Absolute path browsing remains independent of the index.

## Reproduce

```powershell
.\scripts\profile-prism.ps1 `
  -ExePath .\artifacts\Prism.exe `
  -Label local-run `
  -EnableNativeLog
```

This creates `performance/local-run-resources.csv`, `performance/local-run-timings.csv`, and `performance/local-run-native.jsonl`.

## Verification

- Rust: 35 tests passed.
- Frontend: 28 tests passed.
- Rust Clippy with warnings denied: clean.
- Biome lint: clean.
- TypeScript and Vite production build: clean.
- Tauri optimized release build: clean.
- Win-key runtime matrix: four standalone presses alternated open/closed, zero replacement Start windows were visible across all samples, and Win+E opened Explorer without toggling Prism.
- Native width runtime sample: 560, 597, 638, 675, 710, and 720 px while remaining centered.

The remaining large memory floor is primarily the WebView2 runtime. A major reduction below the measured range would require replacing the webview UI with a native renderer; small React or Rust changes cannot remove the browser process model.
