# Parity coverage of user-facing features

This table maps each user-facing feature of the original (`src/`, the spec) to the parity scenarios that
check it. `ui:` names are captures in `scenarios.json` (look only). `ux-` names are scenarios in
`ux-scenarios.json` (recorded backend calls and look). Run them with `bun migration/parity/run.mjs [--ux]`
from `gpui/`.

Status:

- pass: the scenario passes (calls match, each capture >= 9.4).
- deviation: the port differs on purpose (RULEBOOK "Deviation log").
- harness: parity mode cannot check it (see "Not testable in parity mode").

Scenario steps can patch the fixture for one scenario (`"fixture": {...}`, merged into
`target/parity/fixture.json` by `run.mjs`). The patches hold neutral paths (`D:\Pictures`), not
owner data.

Five rows are views with no user action to record (the idle palette, the settings scroll views,
About, the light theme, the zoom levels). UI captures check them; they have no UX scenario.

| Area | Feature | Original source | Scenario(s) | Status |
|---|---|---|---|---|
| Open | Open shows the idle palette (Pinned, Recent, Quick Access, Apps) | `src/App.tsx:123` | `ui:idle`, `ui:idle-scrolled`, `ui:idle-bottom` | pass |
| Search | Apps | `src/features/palette/sections.ts:452` | `ui:search-app`, `ui:search-broad`, `ux-enter-launch` | pass |
| Search | Files and folders | `src/features/palette/sections.ts:547` | `ui:search-files`, `ux-file-open` | pass |
| Search | Windows Settings pages | `src/features/palette/phase1.ts:69` | `ui:search-settings`, `ux-action-settings` | pass |
| Search | Commands (power words: lock) | `src/features/palette/phase1.ts:69` | `ui:search-command`, `ux-action-command` | pass |
| Search | Power plans | `src/features/palette/phase1.ts:73` | `ui:search-power`, `ux-power-plan` | pass |
| Search | Open windows | `src/features/palette/phase1.ts:85` | `ux-window-focus` | pass |
| Search | Calculator (copy result) | `src/features/palette/sections.ts:484` | `ui:search-calc`, `ux-enter-calc` | pass |
| Search | No match (copy the query) | `src/features/palette/sections.ts:497` | `ui:search-nomatch`, `ux-nomatch-copy` | pass |
| Search | Path browse, with items and empty | `src/features/palette/Palette.tsx:1369` | `ux-path-browse`, `ux-path-empty` | pass |
| Search | Picture thumbnails | `src/state/palette.tsx:282` | `ux-thumbnail` | pass |
| Search | File index in progress (footer) | `src/features/palette/Palette.tsx:1155` | `ux-indexing` | pass |
| Keys | Up/Down move the selection, clamped at the ends | `src/state/palette.tsx:687` | `ui:keyboard-nav`, `ui:keyboard-deep`, `ux-nav-top`, `ux-down-enter` | pass |
| Keys | Enter runs the selection | `src/features/palette/PaletteSearchInput.tsx:58` | `ux-enter-launch`, `ux-down-enter`, `ux-quick-open` | pass |
| Keys | Ctrl+Enter and Shift+Enter run it the same way | `src/features/palette/PaletteSearchInput.tsx:58` | `ux-ctrl-enter`, `ux-shift-enter` | pass |
| Keys | Escape: close settings, else clear the query, else hide | `src/features/palette/Palette.tsx:717` | `ux-escape-settings`, `ux-escape-clears`, `ux-escape-hide` | pass |
| Keys | Ctrl+W dismisses like Escape | `src/features/palette/PaletteSearchInput.tsx:72` | `ux-ctrl-w` | pass |
| Keys | Ctrl+K hides the palette | `src/features/palette/PaletteSearchInput.tsx:66` | `ux-ctrl-k` | pass |
| Keys | Ctrl+Backspace clears the query | `src/features/palette/PaletteSearchInput.tsx:78` | `ux-ctrl-backspace` | pass |
| Keys | Menu key and Shift+F10 open the result menu | `src/features/palette/PaletteSearchInput.tsx:41` | `ux-context-keys`, `ux-context-shift-f10` | pass (deviation: port menu order, RULEBOOK 2026-10-11) |
| Keys | Ctrl+= / Ctrl+- / Ctrl+Up / Ctrl+Down / Ctrl+0 zoom | `src/state/app.tsx:297` | `ux-zoom-keys`, `ux-zoom-out-keys`, `ux-zoom-reset` | pass |
| Keys | Ctrl+wheel zooms | `src/state/app.tsx:317` | `ux-zoom-wheel` | pass |
| Keys | Tab order, focus rings, Enter on a focused button | `src/features/palette/Palette.tsx:1652` | `ux-tab-trigger`, `ux-tab-grip`, `ux-tab-footer-enter` | pass |
| Mouse | Hover selects, click runs | `src/features/palette/Palette.tsx:1516` | `ui:idle-hover`, `ux-hover-select`, `ux-click-row` | pass |
| Context menu | Right-click opens, Escape / Tab / click away close | `src/features/palette/ResultContextMenu.tsx:159` | `ui:context-menu`, `ux-context-escape`, `ux-context-tab`, `ux-context-clickaway` | pass |
| Context menu | Open location | `src/features/palette/ResultContextMenu.tsx:194` | `ux-menu-location` | pass |
| Context menu | Run as administrator (file and app) | `src/features/palette/ResultContextMenu.tsx:205` | `ux-menu-admin`, `ux-menu-app-admin` | pass |
| Context menu | Pin to taskbar (toast) | `src/features/palette/ResultContextMenu.tsx:221` | `ux-menu-taskbar-pin` | pass |
| Context menu | Properties | `src/features/palette/ResultContextMenu.tsx:233` | `ux-menu-properties` | pass |
| Context menu | Up/Down/Home/End wrap focus | `src/features/palette/ResultContextMenu.tsx:129` | `ux-context-keys`, `ux-menu-taskbar-pin` | pass (deviation: port menu order, RULEBOOK 2026-10-11) |
| Power menu | Open, arrow keys, Escape returns focus | `src/components/PowerMenu.tsx:120` | `ui:power-menu`, `ux-power-keys` | pass |
| Power menu | Lock | `src/components/PowerMenu.tsx:10` | `ux-power-lock` | pass |
| Power menu | Sleep | `src/components/PowerMenu.tsx:11` | `ux-power-sleep` | pass |
| Power menu | Shut down | `src/components/PowerMenu.tsx:12` | `ux-power-shutdown` | pass |
| Power menu | Restart (End key) | `src/components/PowerMenu.tsx:13` | `ux-power-restart` | pass |
| Power menu | Click away closes | `src/components/PowerMenu.tsx:85` | `ux-power-click-away` | pass |
| Footer | Check for updates (no update) | `src/features/updater/UpdateControl.tsx:227` | `ux-update-check` | pass |
| Footer | Rebuild file index | `src/features/palette/Palette.tsx:1177` | `ux-rebuild-index` | pass |
| Footer | Refresh apps | `src/features/palette/Palette.tsx:1184` | `ux-refresh-apps` | pass |
| Quick Access | Open a folder | `src/features/palette/sections.ts:508` | `ux-quick-open` | pass |
| Quick Access | Collapse the section | `src/features/palette/Palette.tsx:827` | `ui:quick-collapsed`, `ux-collapse-quick` | pass |
| Collections | Collapse a collection in the palette | `src/features/palette/Palette.tsx:931` | `ux-collapse-group` | pass |
| Pins | Pin (toast), unpin (toast) | `src/features/palette/Palette.tsx:152` | `ui:pin-toast`, `ux-pin-toast`, `ux-pin-recent`, `ux-unpin-toast` | pass |
| Pins | Reorder by drag (grip) and by keys | `src/features/palette/Palette.tsx:433` | `ux-reorder-drag`, `ux-reorder-grip`, `ux-tab-grip-move` | pass |
| Pins | Escape during a drag | `src/features/palette/Palette.tsx:672` | `ux-drag-escape` | pass |
| Sections | Reorder a section by its grip | `src/features/palette/Palette.tsx:302` | `ux-section-drag` | pass |
| Recent | Remove one item | `src/features/palette/Palette.tsx:1738` | `ux-remove-recent` | pass |
| Files | Drag a file out (start_file_drag) | `src/features/palette/Palette.tsx:566` | `ux-file-drag` | pass |
| Files | Click on the file grip opens the file | `src/features/palette/Palette.tsx:1798` | `ux-file-grip-click` | pass |
| Toasts | Success toast, at most two at a time | `src/state/app.tsx:452` | `ux-pin-toast`, `ux-toast-stack`, `ux-nomatch-copy` | pass |
| Settings | Open, close (X, backdrop, Escape), focus trap | `src/components/SettingsSheet.tsx:691` | `ui:settings`, `ux-close-x`, `ux-close-backdrop`, `ux-settings-trap`, `ux-settings-close-focus` | pass |
| Settings | Appearance: System / Dark / Light | `src/components/SettingsSheet.tsx:928` | `ux-theme-system`, `ux-theme-dark`, `ux-theme-light`, `ux-settings-segment` | pass |
| Settings | Accent colors (5) | `src/components/SettingsSheet.tsx:941` | `ux-accent-mint`, `ux-accents`, `ui:accent-amber-settings` | pass |
| Settings | Window width S / M / L | `src/components/SettingsSheet.tsx:968` | `ux-width-large`, `ux-width-small`, `ui:width-560`, `ui:width-720` | pass |
| Settings | View zoom buttons (out, in, reset) | `src/components/SettingsSheet.tsx:167` | `ux-zoom-in-button`, `ux-zoom-out-button`, `ux-zoom-reset-button` | pass |
| Settings | Taskbar alignment | `src/components/SettingsSheet.tsx:125` | `ux-taskbar-align` | pass |
| Settings | Scroll volume toggle | `src/components/SettingsSheet.tsx:992` | `ux-scroll-volume` | pass |
| Settings | Taskbar density | `src/components/TaskbarCustomization.tsx:79` | `ux-taskbar-density` | pass |
| Settings | Auto-hide taskbar | `src/components/TaskbarCustomization.tsx:102` | `ux-taskbar-autohide` | pass |
| Settings | Combine buttons | `src/components/TaskbarCustomization.tsx:116` | `ux-taskbar-combine` | pass |
| Settings | Start icon: System / Gem / Diamond | `src/components/TaskbarCustomization.tsx:147` | `ux-start-icons` | pass |
| Settings | Start icon: use and remove a custom icon | `src/components/TaskbarCustomization.tsx:268` | `ux-start-custom-use`, `ux-start-custom-remove` | pass |
| Settings | Global shortcut options | `src/components/SettingsSheet.tsx:63` | `ux-shortcut`, `ux-shortcut-pick`, `ux-shortcut-last` | pass |
| Settings | Keep on top | `src/components/SettingsSheet.tsx:1011` | `ux-keep-on-top` | pass |
| Settings | Quick Access folder choice (limit 6) | `src/components/SettingsSheet.tsx:236` | `ux-quick-folders` | pass |
| Settings | Collections: create | `src/components/SettingsSheet.tsx:346` | `ux-collection-create` | pass |
| Settings | Collections: rename | `src/components/SettingsSheet.tsx:457` | `ux-collection-rename` | pass |
| Settings | Collections: move | `src/components/SettingsSheet.tsx:466` | `ux-collection-move` | pass |
| Settings | Collections: delete | `src/components/SettingsSheet.tsx:489` | `ux-collection-delete` | pass |
| Settings | Collections: add apps, filter the picker | `src/components/SettingsSheet.tsx:504` | `ux-collection-add-app`, `ux-collection-filter` | pass |
| Settings | Collections: remove an app | `src/components/SettingsSheet.tsx:670` | `ux-collection-remove-app` | pass |
| Settings | Recent items: Clear | `src/components/SettingsSheet.tsx:1038` | `ux-recent-clear` | pass |
| Settings | Reset: confirm, Reset now, Cancel | `src/components/SettingsSheet.tsx:882` | `ui:settings-reset`, `ux-reset-cancel`, `ux-reset-now`, `ux-reset-cancel-click` | pass |
| Settings | About: version | `src/components/SettingsSheet.tsx:1046` | `ui:settings-bottom` | pass |
| Settings | Quit | `src/components/SettingsSheet.tsx:1056` | `ux-quit` | pass |
| Settings | Scrolled views | `src/components/SettingsSheet.tsx:927` | `ui:settings-scroll-1`, `ui:settings-scroll-2`, `ui:settings-scroll-3` | pass |
| Theme | Light theme views | `src/state/app.tsx:492` | `ui:light-idle`, `ui:light-search`, `ui:light-settings` | pass |
| Zoom | Views at 80 % and 120 % | `src/state/app.tsx:281` | `ui:zoom-80`, `ui:zoom-120` | pass |

## Not testable in parity mode

These features need the real host (tray, real window, Win key, real launches) or a failure that
the fixture cannot inject. Test them on the desktop build.

| Feature | Original source | Reason |
|---|---|---|
| Tray icon and its menu | `src-tauri` | host-only: no tray in parity mode |
| Real window: show, hide, focus, click-away hide, always-on-top effect | `src/App.tsx:154` | host-only: the parity window is parked and never activated |
| Win key and global shortcut press | `src-tauri` | host-only: needs global input |
| Real launches, power actions, Explorer verbs, taskbar writes, clipboard | `src/lib/bridge.ts` | host-only: parity mode records the call and runs nothing |
| Native file drag (OLE drop target) | `src/features/palette/Palette.tsx:542` | host-only: the mock returns at once; `ux-file-drag` checks the call only |
| Add a custom Start PNG (file picker) | `src/components/TaskbarCustomization.tsx:246` | host-only: opens a native file dialog |
| Update available, download, install, retry | `src/features/updater/UpdateControl.tsx:271` | harness: the fixture answers "no update" |
| Error toasts and their Dismiss button | `src/components/Toast.tsx:46` | harness: the fixture cannot make a call fail |
| Apps failed to load, file search errors, Retry / Rebuild index notices | `src/features/palette/Palette.tsx:1286` | harness: needs a failing backend |
| Settings save failure and Retry save | `src/components/SettingsSheet.tsx:919` | harness: needs a failing save_state |
| Pin limit (64), collection limits (16 collections, 64 apps) | `src/lib/types.ts:109` | not covered: needs 64 real app ids in a committed scenario |
| Tooltips (`title`) | `src/components/ui.tsx` | harness: headless Chrome cannot capture native tooltips; parity mode hides them |
| Screen reader names and roles | `-` | deviation: gpui 0.2.2 has no UI Automation provider (RULEBOOK 2026-10-27 (8)) |
| Port-only menu entries "Open" and "Copy as path" | `-` | deviation: RULEBOOK 2026-10-11; the original has no such entries |
