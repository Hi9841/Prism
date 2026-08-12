# Prism Deep Audit - Findings

Audit date: 2025-08-12 (v0.6.7). Issues only - no fixes applied.

Severity: **[H]** high (visible/measurable impact) | **[M]** medium (real, situational) | **[L]** low (minor).

---

## 1. Steady-state background load (runs forever, even when idle)

### 1.1 [H] Taskbar alignment watcher polls at 50 ms forever
`src-tauri/src/taskbar_alignment.rs:31` (`ALIGNMENT_WATCH_INTERVAL = 50ms`), `:222-236`

`start_alignment_watcher()` spawns a thread that loops every 50 ms (20x/sec) and calls `apply_classic_taskbars()` unconditionally. Each iteration:

- `taskbar_windows()`: full `EnumWindows` over **every top-level window in the session**, with `GetClassNameW` per window
- `EnumChildWindows` + `GetClassNameW` + `IsWindowVisible` per taskbar child
- `GetWindowRect` x3 per taskbar

This runs 24/7 once any alignment was set or any palette presentation happened (`reapply_with_companion` also calls `start_alignment_watcher`). It never stops and is not gated on state changes - only the SetWindowPos/RedrawWindow part is skipped when geometry is unchanged. Constant cross-process user32 churn on the shell, measurable CPU even when Prism is hidden. It also takes `ALIGNMENT_APPLY_LOCK` every iteration, which the palette-open path needs (see 2.1).

### 1.2 [H] Start-button rect UIA query every ~1 second
`src-tauri/src/win_key.rs:431-432`, `:789-830`

The raw-input pump loop wakes at least every 1 s (`MsgWaitForMultipleObjectsEx(..., 1000, ...)`) and calls `refresh_start_rect()` on every wake. `refresh_start_rect` runs `start_button_locator.rect()`, which does a UIA `FindFirst(TreeScope_Descendants, ...)` over the **entire taskbar element subtree** plus `CurrentBoundingRectangle()`. UIA calls are marshaled into Explorer - this is a permanent, ~1 Hz UIA traversal of the shell tree for the lifetime of the app (whenever Win-key observation is active, which is the default "Win" shortcut). Should be event-driven or throttled to seconds, and skipped when the rect is already known and the taskbar hasn't changed.

### 1.3 [M] File index: full recursive rescan + multi-MB cache write every 15 minutes
`src-tauri/src/files.rs:104-108`, `:393-401`

`warm()` loops `refresh_index()` every `REFRESH_INTERVAL` (15 min) forever. Each refresh walks all user folders (up to 100k entries), then serializes the **entire cache to JSON (~10-20 MB)** and atomically replaces `files.json`. The 6 h `CACHE_TTL_SECONDS` is only consulted at startup - the periodic refresh ignores it entirely and rescans whether or not the snapshot is stale. On a laptop this wakes the disk every 15 min indefinitely. The in-code comment acknowledges "sustained disk I/O" as a design choice, but it never backs off when the index is fresh.

### 1.4 [M] Update check hits GitHub on every palette open
`src/features/updater/UpdateControl.tsx:65`

`onToggleRequest` → `checkForUpdate()` fires an HTTP request to the GitHub release endpoint (15 s timeout) every time the palette opens, plus at startup and hourly. There is no cooldown/min-interval between checks (only an in-flight dedup). Win-key spamming or a flaky network means repeated network I/O on the launcher's most common interaction. README documents the behavior, but a minimum-interval cache (e.g. check at most every N minutes) would keep the guarantee without the per-open cost.

### 1.5 [L] `RegisterWindowMessageW` on every keyboard event
`src-tauri/src/win_key.rs:1156`

`raw_input_window_proc` calls `shell_bridge_message()` (which calls `RegisterWindowMessageW` - a system-wide registration call) for **every message**, including every WM_INPUT key event, to compare against the bridge message id. The bridge id is a fixed string; it should be cached in a `OnceLock` (the shell-hook DLL already does exactly this - `src-tauri/shell-hook/src/lib.rs:236` - the Rust side forgot). Small per-event cost on the global keyboard path.

---

## 2. Palette open / presentation latency

### 2.1 [H] Every Win-key press does 2-3 full desktop window enumerations on the main thread
`src-tauri/src/lib.rs:635` (`present_palette`) → `src-tauri/src/taskbar_alignment.rs:154` (`reapply_with_companion`)

`present_palette` is a sync command that runs on the main thread and calls `reconcile_palette_position` on every activation - even when the palette is already open. That calls `reapply_with_companion`, which:

1. takes `ALIGNMENT_APPLY_LOCK` (contended with the 50 ms watcher from 1.1),
2. runs `classic_taskbar_count()` - full `EnumWindows` + `EnumChildWindows`,
3. runs `apply_classic_taskbars()` - another full `EnumWindows` + per-taskbar `EnumChildWindows` + `GetWindowRect`,
4. possibly `SetWindowPos` + `RedrawWindow(RDW_UPDATENOW)` on Explorer's taskbar HWNDs.

This all happens **before** `window.show()`/`raise_palette`, so it adds directly to Win-key → visible latency on every open. On a loaded desktop (hundreds of top-level windows), each `EnumWindows` pass with per-window class-name queries is a few ms; with lock contention it can be much worse. The taskbar only needs re-reconciling when the palette *opens* (not when already visible), and the enumeration should be off the main thread.

### 2.2 [M] Shortcut change / startup shortcut apply blocks the main thread up to ~2 s
`src-tauri/src/win_key.rs` (`set_enabled`: `ready_rx.recv_timeout(Duration::from_secs(2))`, `:116-161`) and `src-tauri/src/lib.rs:1040-1060` (`apply_startup_shortcut`)

`set_shortcut` (sync command) and the startup shortcut application run on the main thread and can block on:
- Explorer bridge install: `find_shell_window` retry loop (20 x 50 ms), `SetWindowsHookExW`, `wait_for_ack` with 1 s timeouts (x2)
- `start_menu::enable`: registry writes + watchdog spawn + `SendMessageTimeoutW`

Worst case the UI thread is frozen for multiple seconds during shortcut changes or startup. This is inherent to the Win-key design but worth knowing: the default "Win" path installs a shell bridge **on the main thread at every startup**.

---

## 3. Keystroke / search hot path

### 3.1 [M] File search: redundant syscalls per keystroke
`src-tauri/src/files.rs:417-419` (`indexed_file_entry`), `:201-230` (`browse_path`), `:268` (`path_entry`)

- Every search result (up to 20) triggers `std::fs::metadata()` - a fresh stat syscall per result - even though `SearchEntry` already stores `is_directory`; the stat only re-verifies existence (a test requires it, but it's 20 syscalls per keystroke).
- `browse_path` does `is_dir()` + `is_file()` (two stats) for the query, then `list_directory` calls `path_entry()` on **every child**, each doing `path.is_dir()` - another stat. Typing a path into a large directory (e.g. `C:\Windows\System32`, thousands of entries) = thousands of stat syscalls per keystroke. `DirEntry::file_type()` comes free from the enumeration and would replace all of these.
- Combined with the full 100k-entry score scan (below), each keystroke pays several milliseconds of blocking I/O (off-thread via `spawn_blocking`, so the UI survives, but results latency and disk churn are real).

### 3.2 [M] File search is a full linear scan with no index on every keystroke
`src-tauri/src/files.rs:140-158`

Every search iterates all up-to-100k entries, runs `entry_score` (subsequence matching over the lowercased name), and maintains a sorted top-N with `Vec::insert` + `truncate` per hit (O(limit) shifts x 100k entries worst case). No prefix/token cache, no incremental scoring across keystrokes, no trigram index. It's off-thread and probably acceptable at 100k, but it's the single hottest algorithm in the app and the first thing to break at larger indexes. Also the `RwLock` read guard is held for the whole scan, blocking the 15-min refresh's `replace()`.

### 3.3 [L] Frontend: redundant dedupe + eager sort per keystroke
`src/lib/search.ts:97` (`fuzzyApps` calls `dedupeApps` internally), `src/state/palette.tsx:232-233`

`fuzzyApps(visibleApps, ...)` re-dedupes an already-deduped list (a HashMap build + `normalizeText` on every app) on every keystroke. `sortedApps` (a full `localeCompare` sort of all apps) is computed via `useMemo` even when the query is non-empty and the value is unused. ~350 apps makes both cheap, but it's pure waste on the per-keystroke path.

### 3.4 [L] Status IPC fires on every empty/short query
`src/state/palette.tsx:186-194`

Whenever the query drops below 2 chars (backspacing through, or every palette open via `reset()`), the effect calls `searchFiles("", 1)` - a full IPC round trip - to refresh `ready/indexing` status, even if the status was already known milliseconds ago.

---

## 4. Sync commands doing slow work on the main thread

Tauri 2 sync commands execute on the main thread. These all do filesystem or shell work:

| Command | File:line | Work |
|---|---|---|
| `existing_paths` | `src-tauri/src/lib.rs:869` | Up to 64 `Path::exists()` syscalls; called on **every window focus**, every file-index update, and mount (`src/state/palette.tsx:157-169`). Network/unc or cold drives can stall the UI thread for each call. |
| `open_path` | `src-tauri/src/lib.rs:818` | `exists()` + `is_dir()` + `ShellExecuteW` - ShellExecuteW can block for seconds when Explorer is busy. |
| `launch_app` | `src-tauri/src/lib.rs:790` | **Holds the `apps_cache` std Mutex while calling `ShellExecuteW`** - `get_apps`/`refresh_apps` block for the whole launch. `launch_app_as_admin` correctly clones the entry and releases the lock first (`:806`); `launch_app` doesn't - inconsistent. |
| `set_start_icon` / `select_custom_start_icon` / `remove_custom_start_icon` | `src-tauri/src/lib.rs:958-980`, `src-tauri/src/taskbar_customization.rs:197-260` | PNG decode of up to 2 MB + resize (Lanczos) + encode on the main thread. |
| `get_taskbar_settings` | `src-tauri/src/lib.rs:955` | Reads the registry + reads up to 12 icon preview files from disk, every time the settings panel opens and after every taskbar change (`src/components/TaskbarCustomization.tsx` refresh/apply). |

`existing_paths` is the worst offender because it's on the **focus path of every single open**.

---

## 5. Startup

### 5.1 [M] App cache write is non-atomic; corrupt cache forces a full rescan
`src-tauri/src/apps.rs:350-356` (`write_cache`)

`std::fs::write(path, text)` writes `apps.json` in place. A crash or power loss mid-write corrupts the cache; `load_cache` then fails to parse and the **entire scan (shortcut resolution + icon extraction for ~355 apps) runs again**. `files.rs` (`save_cache`) and `taskbar_customization.rs` use temp-file + `MoveFileExW` - apps.rs should do the same.

### 5.2 [L] `load_cache` clones the entire app payload
`src-tauri/src/apps.rs:340-348`

`serde_json::from_value(v.clone())` clones the full apps JSON (several MB with base64 icons) before deserializing - transient double allocation of the whole cache at every startup and refresh.

### 5.3 [L] First cold start runs the full scan + icon extraction lazily
`src-tauri/src/lib.rs:739-766`

The app-index scan only starts when the frontend calls `get_apps` (webview load). With a cold icon cache, ~355 apps x (SHGetFileInfoW + GDI conversion + PNG encode) takes tens of seconds. It's in `spawn_blocking` so the UI survives, but the first open shows "indexing apps" for a long time; the file index (by contrast) is warmed eagerly in `setup` (`files::warm`). The apps scan could be warmed the same way.

---

## 6. IPC payload design

### 6.1 [M] Custom Start icon upload serializes a 2 MB PNG as a JSON array of numbers
`src/lib/bridge.ts:273` (`{ png: Array.from(png) }`)

`Array.from(Uint8Array)` creates a 2M-element JS array; Tauri v2 serializes it as JSON - roughly 8+ MB of JSON for a max-size icon, plus the same on the Rust side. Tauri v2 supports raw binary bodies (`invoke` with `Uint8Array`/`ArrayBuffer` arguments are handled more efficiently when passed directly, and raw request bodies entirely). The read path has the same shape: `CustomStartIcon.preview: Vec<u8>` is serialized as a `number[]` per icon (`src-tauri/src/taskbar_customization.rs` `CustomStartIcon`), re-created as a Blob in `TaskbarCustomization.tsx:133`.

### 6.2 [L] Every app entry carries a base64 data-URL icon
`src-tauri/src/apps.rs` (`icon: Option<String>`), `src/lib/bridge.ts:64-67`

~355 icons x 2-8 KB base64 = a few MB held in the Rust cache, transferred over IPC, parsed by the frontend, and retained in React state for the whole session. Fine once per session, but `refresh_apps` re-sends the entire payload, and `launch_app`'s cache lock (4.1) makes a concurrent `get_apps` wait on the whole transfer.

---

## 7. Reliability & consistency

### 7.1 [M] Recent-items rows flicker on every window focus
`src/state/palette.tsx:137-169`

`validateHistoryPaths` runs on mount, every file-index update, and **every window focus**. It first `setExistingHistoryPaths(new Set())` - unmounting every history row - then re-populates after an IPC round trip. Every open therefore clears and re-renders the whole Recent section, with a visible gap on slow drives. It also re-validates the same unchanged paths on every focus - no caching keyed on history identity.

### 7.2 [L] Toast timers are untracked
`src/state/app.tsx:298-304`

`showToast` slices the toast list to the last 3, but the dismissed (sliced-away) toast's `setTimeout` still fires later, calling `dismissToast` → a `setToasts` with a fresh array identity → spurious re-render. Timers are also never cleared on unmount.

### 7.3 [L] Alignment marker file written non-atomically
`src-tauri/src/taskbar_alignment.rs:246-256` (`write_shared_alignment` uses plain `fs::write`)

Small file, but a crash mid-write can leave a truncated marker that fails `Alignment::parse` - handled gracefully (falls back), so low impact.

### 7.4 [L] Two competing background enumerations plus the presentation path contend on one lock
`ALIGNMENT_APPLY_LOCK` is taken by the 50 ms watcher (1.1), `reapply_with_companion` (2.1), and `set_alignment` - the palette-open path can be delayed by the watcher's enumeration at any moment.

---

## 8. Frontend rendering

### 8.1 [L] No memoization on result rows
`src/features/palette/Palette.tsx` (`ResultRow`)

Every keystroke re-renders every row with 12+ props and inline arrow closures; ~20 rows so it's fine today, but it's the first place to optimize if the palette ever grows (sections, group headers, more items). `ResultRow` is a pure function of its props and would memoize cleanly.

### 8.2 [L] Width animation: ~15 IPC round trips per resize
`src/state/app.tsx:169-190`

The 240 ms window-width animation calls `setWindowWidth` (a sync command that also re-positions the window) per rAF frame. Works, but each call is a main-thread Win32 round trip on the palette's most visually active moment.

### 8.3 [L] Hidden stage keeps the full React tree mounted
`src/styles/tokens.css:455-471` (`opacity: 0` + `pointer-events: none` + `contain`)

Deliberate (keeps state, avoids remount cost) and mitigated by `contain` and WebView2 memory-target LOW on hide. Fine as-is; just noting the trade-off: all toast/settings state and the whole app list stay resident while hidden.

---

## 9. CI & test suite

### 9.1 [M] `cargo test` runs a full real app scan with cold icon extraction
`src-tauri/src/apps.rs` - `scan_produces_valid_cached_entries` and the `hang_debug` module

Both tests perform a **real scan of the runner's machine** - shortcut roots, `shell:AppsFolder`, registry, Program Files - and the `hang_debug` test uses a **fresh empty icon cache**, forcing cold icon extraction (SHGetFileInfoW + GDI + PNG encode) for every discovered app. On a clean CI runner this can take minutes and flaky-fails on machines with unusual Explorer states. `hang_debug` is a profiling harness with no assertions, shipped inside `cargo test`. These belong behind an opt-in feature/flag.

### 9.2 [L] Frontend tests run in the browser-less Vitest environment with mocked IPC - fine, but `existing_paths`-style behavior (flicker) is untested
`src/state/app.test.ts`, `src/lib/search.test.ts`

Behavioral tests for the focus-validation path (7.1) and the search debounce/race handling (`fileRequest` counter) are absent.

---

## 10. Security hardening

### 10.1 [M] Production CSP allows local dev servers
`src-tauri/tauri.conf.json` (`connect-src ... http://localhost:1420 ws://localhost:1420`)

The same CSP ships in production builds. If any content injection ever happened, the webview could reach local dev servers / websockets. The dev endpoints should be injected only in dev builds (e.g. via a build-time flag in `vite.config.ts`/`tauri.conf.json` templating).

### 10.2 [L] No explicit `script-src` nonce/hash; `style-src 'unsafe-inline'`
Standard Tauri trade-off (inline styles are used throughout for dynamic values); acceptable, but combined with 10.1 there's no injection hardening story at all.

---

## Quick ranking (what to fix first)

1. **1.1 + 2.1** - 50 ms taskbar watcher + per-open desktop enumerations: steady CPU and per-open latency, both in the same lock/machinery.
2. **1.2** - 1 Hz UIA Start-button traversal for the app's whole lifetime.
3. **4.1** - `existing_paths` on the focus path, sync on the main thread; plus `launch_app` holding the cache lock through `ShellExecuteW`.
4. **1.3** - 15-min full rescan + multi-MB cache write forever.
5. **1.4** - update check network call on every open.
6. **3.1** - per-keystroke stat syscalls in file search (metadata per result + per-child stats in path browse).
7. **7.1** - Recent-items flicker on every focus.
8. **5.1/5.2** - non-atomic apps cache write; full-payload clone on load.
9. **6.1** - PNG upload as a 2M-element JSON array.
10. **9.1** - full machine scan inside `cargo test`.
