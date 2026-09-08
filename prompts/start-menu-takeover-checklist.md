# Start-menu takeover manual checklist

Run on the target machine after the takeover build. Each item is one bounded
action; record pass/fail per row.

## Button and glyph

| # | Action | Expected | Pass |
|---|---|---|---|
| 1 | Check the taskbar Start position | Prism's violet glyph, no native logo visible | |
| 2 | Click the glyph once (palette closed) | Prism opens | |
| 3 | Click the glyph again | Prism closes | |
| 4 | Switch taskbar alignment (Settings > Taskbar > Center/Left/Right) | Glyph follows within ~1s, no dead button at the old spot | |
| 5 | Settings > Start button icon > System | Native button returns without restart | |
| 6 | Settings > Start button icon > Prism | Glyph returns | |
| 7 | Custom PNG selected | Custom icon renders, correct aspect, no bleed | |
| 8 | Restart Windows Explorer (Task Manager > Restart) | Overlay and glyph reappear within ~2s, no ghost or stacked glyph | |
| 9 | Kill Prism from Task Manager | Overlay disappears within one heartbeat tick; native button returns | |
| 10 | Relaunch Prism | Glyph returns; no stale overlays | |

## Palette behavior

| # | Action | Expected | Pass |
|---|---|---|---|
| 11 | Click glyph | Palette opens anchored above the taskbar at the button | |
| 12 | Press Escape | Palette closes | |
| 13 | Click the glyph while open | Palette closes, no reopen (no double-fire) | |
| 14 | Click anywhere else on the taskbar | Palette closes | |
| 15 | Win key | Toggles Prism; native Start never appears | |
| 16 | Win+E, Win+R, Win+S | Native behavior, Prism never toggles | |
| 17 | Ctrl+Esc | Opens Prism, never the native menu | |
| 18 | Win key with an elevated app focused (e.g. admin terminal) | Opens Prism | |
| 19 | Search box click | Opens Prism with the search input focused | |
| 20 | Type immediately after search-box open | Characters land in the search input | |
| 21 | Second monitor | Palette anchors to the primary button; secondary taskbar untouched | |
| 22 | Auto-hide taskbar | Overlay hides with the bar, returns on peek, click works | |

## Stability

| # | Action | Expected | Pass |
|---|---|---|---|
| 23 | 5 minutes of normal use | No explorer crashes (check Event Viewer for explorer.exe 1000s) | |
| 24 | Open a topmost app (Discord popout) over the taskbar | Glyph restores on the next refresh; palette renders above | |
| 25 | Quit Prism normally | Native Start menu and button return; taskbar settings untouched | |

## Known limits

- Win key on this machine is disabled at the OS level (Prism owns it). The
  native Start menu is only reachable through the button, which the overlay
  covers; the backstop watcher hides the native menu if a leak path shows it.
- Windows 10: keyboard and rect capture paths carry the takeover; the glyph
  overlay is best effort (no reliable UIA button rectangle on many builds).
- The three launcher window classes watched are
  `XamlExplorerHostIslandWindow` (Win11), `ImmersiveLauncher` and
  `ModeInputWnd` (Win10). The Win11 class is shared with other explorer
  XAML surfaces, so only tall wide silhouettes are hidden.
