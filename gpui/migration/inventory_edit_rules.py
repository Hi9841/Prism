"""One-time edit: add covers= ranges to the first 20 rows and append rule rows.

Run from the repo root: python -I migration/inventory_edit_rules.py
"""
from pathlib import Path

path = Path("migration/inventory.tsv")
lines = path.read_text(encoding="utf-8").splitlines()
header, rows = lines[0], [line.split("\t") for line in lines[1:] if line.strip()]

# (file, symbol prefix) -> covered line ranges in that file
COVERS = {
    ("src/features/palette/sections.ts", "appPaletteItem"): "454-480",
    ("src/features/palette/sections.ts", "toggleTaskbarPin"): "445-450,569-572",
    ("src/features/palette/phase1.ts", "windowPaletteItem"): "85-95",
    ("src/features/palette/sections.ts", "calc and copy rows"): "441-443,488-506",
    ("src/state/palette.tsx", "file search effect"): "378-527",
    ("src/state/palette.tsx", "app icon loading"): "632-671",
    ("src/state/app.tsx", "schedulePersist"): "178-216",
    ("src/state/app.tsx", "Ctrl zoom keys"): "287-344",
    ("src/state/app.tsx", "showToast"): "427-461",
    ("src/state/app.tsx", "accent and theme"): "489-494",
    ("src/state/app.tsx", "reduced motion"): "510",
    ("src/App.tsx", "hide() and the close timer"): "83-109",
    ("src/App.tsx", "blur check"): "150-176",
    ("src/App.tsx", "prism:close"): "180,197,231-235",
    ("src/App.tsx", "focusSearch"): "113-115",
    ("src/lib/bridge.ts", "onToggleRequest"): "120-135,340-356,460-480",
    ("src/lib/bridge.ts", "searchFiles"): "247-281",
    ("src/features/palette/Palette.tsx", "reorder and category drag"): "204-232,327-363,449-471,1520-1560",
    ("src/features/palette/ResultContextMenu.tsx", "menu position"): "18-33",
    ("src/styles/tokens.css", "panel-in"): "580-583,643-652",
}

for row in rows:
    for (file, prefix), covers in COVERS.items():
        if row[0] == file and row[1].startswith(prefix) and "covers=" not in row[5]:
            row[5] = f"{row[5]}; covers={covers}"

missing = [key for key in COVERS if not any(r[0] == key[0] and r[1].startswith(key[1]) for r in rows)]
assert not missing, missing

RULE = "rule (mechanical; whole kind)"
RULES = [
    ("rule:classname", "Tailwind utility classes", "STYLE",
     "One style-builder call per utility class, with every value from theme.rs: spacing scale (1 unit = 0.25 rem), radii, colors and opacity from the token set, typography sizes and weights. Arbitrary values (`rounded-[24px_24px_8px_8px]`, `w-[196px]`) become literal px in the same builder. Pseudo states: `hover:` -> `.hover()`, `active:` -> `.active()`, `focus-visible:` -> focus-visible styling on the element's FocusHandle, `disabled:` -> a branch on the disabled flag. Custom classes defined in tokens.css (`shell`, `focus-ring`, `tile-*`, `context-menu-enter`, …) become named style helpers in theme.rs that apply the same declarations.",
     "280 sites (gap-sites.tsv kind classname); tokens.css custom classes"),
    ("rule:inline-style", "React `style={{…}}` props", "STYLE",
     "The same builder calls, with computed values. `translate3d(x, y, 0)` drag previews become absolute positioning at (x, y). `boxShadow` strings become `BoxShadow` values in theme.rs. CSS variables in values (`var(--accent)`) become reads from the Theme global.",
     "17 sites; for example ui.tsx:174, :178 (Toggle track and thumb), Palette.tsx:1225, :1261 (drag previews), SettingsSheet.tsx:845"),
    ("rule:css-token", "CSS custom properties in tokens.css", "THEME-TOKEN",
     "One field in `Theme` per token, for both themes and all five accents. OKLCH and rgb() values are converted once to linear RGBA by a const helper, and a unit test pins each converted value. Gradients become `linear_gradient` fills with the same stops and alpha.",
     "126 sites in src/styles/tokens.css (:9-210)"),
    ("rule:css-keyframes", "@keyframes", "MOTION",
     "One named animation preset in theme.rs per keyframe (fade-in/out, panel-in/out, toast-in/out, reorder-drag-pickup/out), with the same properties and the duration and easing of the rule that uses it. Used through `with_animation`. Reduced motion skips them (tokens.css:680).",
     "8 sites in src/styles/tokens.css (:633-679)"),
    ("rule:css-motion", "transition and animation declarations", "MOTION",
     "Each declaration maps to an animation on the same element with the same duration, delay, and easing (`--ease-out-soft` = cubic-bezier(0.22, 1, 0.36, 1), `--ease-in-out-soft` = cubic-bezier(0.65, 0, 0.35, 1), 110/170/260 ms). Property transitions driven by a state change (hover, selection, width) become target-driven tweens on that state.",
     "32 sites in src/styles/tokens.css; tokens.css:68-72 (easing and duration tokens)"),
    ("rule:optional-field", "optional interface fields (`name?: T`)", "OPTIONAL",
     "`Option<T>`. An absent field is `None`, never a sentinel. Judge contract: an absent optional object field is omitted from adapter output.",
     "68 sites; adapter.mjs header (output contract)"),
    ("rule:lucide", "lucide-react icon imports", "ICON",
     "Each icon is an SVG from the same lucide release, embedded as an asset under `crates/prism-ui/assets/icons/<kebab-name>.svg` and drawn with `svg().path(...)`, tinted with the text color. Icon identity in data is the lucide display name (judge `icon.name`).",
     "11 import sites; lucide-react ^0.477.0 (package.json)"),
    ("rule:tauri-import", "imports from @tauri-apps/*", "HOST-API",
     "Removed. Their capabilities move to prism-core (commands, events) and prism-app (window, clipboard, updater, process).",
     "7 sites"),
    ("rule:tauri-command", "invoke(\"command\", args)", "HOST-API (direct call)",
     "`backend.<command>(args)` as listed in migration/backend-commands.tsv (44 commands, each with its handler in src-tauri/src/lib.rs). Called on the background executor; the result is the Rust type, with no JSON.",
     "43 sites in src/lib/bridge.ts; migration/backend-commands.tsv"),
    ("rule:async@src/lib/bridge.ts", "async wrapper functions in the bridge", "HOST-API (direct call)",
     "Each wrapper becomes a `Backend` method that the UI calls through `cx.background_spawn`. The `if (!inTauri) return …` early exits are not ported: the app is always native.",
     "81 sites in src/lib/bridge.ts"),
    ("rule:host-dom@src/lib/bridge.ts", "browser-only fallbacks behind `!inTauri`", "NOT PORTED",
     "Not ported. These branches run only outside Tauri (a plain browser during development): navigator.clipboard (:77-78), CSS zoom on documentElement (:112), matchMedia color scheme (:451). The native paths they stand in for are covered by their own rows.",
     "bridge.ts:64 (inTauri), :74-80, :109-118, :449-455"),
    ("rule:react-hook/useState", "useState", "ENTITY-STATE",
     "A field on the owning view or entity. The setter becomes a method that assigns and calls `cx.notify()`. Functional updates (`set(prev => …)`) read the field.",
     "60 sites"),
    ("rule:react-hook/useRef", "useRef", "ENTITY-STATE",
     "A field that does not trigger a re-render (no `cx.notify()` on write). Timer refs become `Option<Task<()>>` (see the timer rows); DOM refs become `FocusHandle` or bounds stored by the element.",
     "68 sites"),
    ("rule:react-hook/useCallback", "useCallback", "METHOD",
     "A method on the view or entity. Dependency arrays have no equivalent: the method reads current state. The block's own logic is ported by the traced row that covers its line range.",
     "72 sites; migration/effect-blocks.tsv lists every block"),
    ("rule:react-hook/useMemo", "useMemo", "DERIVED",
     "A method that computes the value. When it is expensive (sections, dedupe), cache it on the entity keyed by a revision counter that every input mutation bumps.",
     "9 sites; for example state/palette.tsx:550-600 (visibleApps, pinnedApps, quickItems, sections)"),
    ("rule:react-hook/useContext", "useContext", "ENTITY-STATE",
     "A read of the owning entity: `self.app.read(cx)` / `self.palette.read(cx)`.",
     "2 sites"),
    ("rule:react-hook/useId", "useId", "IDENTITY",
     "A stable `ElementId` built from the component's domain id.",
     "1 site"),
    ("rule:custom-hook/useApp", "useApp()", "ENTITY-STATE",
     "The view holds `Entity<AppState>` and reads it with `.read(cx)`; it subscribes with `cx.observe` when it must re-render on change.",
     "14 sites"),
    ("rule:custom-hook/usePalette", "usePalette()", "ENTITY-STATE",
     "The view holds `Entity<PaletteState>`; same pattern as useApp.",
     "4 sites"),
    ("rule:event-handler", "JSX event props (onClick, onPointerDown, onKeyDown, …)", "INPUT",
     "onClick -> `.on_click`; onPointerDown/Move/Up -> `.on_mouse_down/move/up` (pointer capture -> the element keeps the drag); onContextMenu -> right `.on_mouse_down`; onMouseEnter/Leave -> `.on_hover`; onKeyDown -> key bindings and actions in the view's key context; onChange of an input -> the input entity's change event; onFocus/onBlur -> focus handle observers; onWheel -> `.on_scroll_wheel`. The handler body is ported with its component; block logic is covered by traced rows.",
     "116 sites"),
    ("rule:closure-field@src/lib/types.ts", "PaletteItem action fields", "ACTION-DATA",
     "The `RowActions` struct of the sections.ts appPaletteItem row: `run: RowAction` and `Option<RowAction>` for the optional ones.",
     "types.ts:19-33"),
    ("rule:closure-field@src/components/ui.tsx", "component callback props (onChange, onClick)", "CALLBACK-PROP",
     "Stateless components (RenderOnce) take `impl Fn(&V, &mut Window, &mut App) + 'static` and store it boxed; stateful views emit a typed event (`EventEmitter`).",
     "ui.tsx:161, :194, :275"),
    ("rule:closure-field@src/features/palette/PaletteSearchInput.tsx", "search input callback props", "CALLBACK-PROP",
     "PaletteSearchInput becomes a view that emits `SearchInputEvent { QueryChanged(String), Move(i8), RunSelected, OpenSelectedMenu, Dismiss, ToggleSettings }`; the palette view subscribes.",
     "PaletteSearchInput.tsx:13-18"),
    ("rule:closure-field@src/features/palette/ResultContextMenu.tsx", "context menu callback props", "CALLBACK-PROP",
     "The menu emits `ContextMenuEvent { OpenLocation, RunAsAdmin, ToggleTaskbarPin { was_pinned }, ShowProperties, Close { restore_focus } }`; optional callbacks become absent menu items.",
     "ResultContextMenu.tsx:46-50"),
]

for symbol, construct, classification, translation, evidence in RULES:
    if not any(r[1] == symbol for r in rows):
        rows.append(["*", symbol, construct, classification, translation, evidence, RULE])

path.write_text(header + "\n" + "\n".join("\t".join(r) for r in rows) + "\n", encoding="utf-8", newline="\n")
print(f"rows={len(rows)}")
