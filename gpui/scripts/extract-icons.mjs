// Writes the lucide icons Prism uses as SVG assets for the GPUI port.
// Usage: bun scripts/extract-icons.mjs [path to Prism]
// The geometry comes from the original app's lucide-react package, so the
// port draws the same strokes. GPUI tints the SVG alpha mask with text color.
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const prism = process.argv[2] ?? path.resolve(import.meta.dirname, "../../Prism");
const iconsDir = path.join(prism, "node_modules/lucide-react/dist/esm/icons");
const out = path.resolve(import.meta.dirname, "../crates/prism-ui/assets/icons");

// [file name in lucide, extra stroke widths beyond 2]
const ICONS = {
  "search": [1.75], "settings-2": [], "chevron-down": [], "chevron-up": [], "folder-sync": [],
  "grip-vertical": [], "pin": [], "pin-off": [], "refresh-cw": [], "x": [], "calculator": [],
  "clipboard-copy": [], "download": [], "file": [], "file-archive": [], "file-image": [],
  "file-text": [], "file-video": [], "folder": [], "house": [], "images": [], "monitor": [],
  "music": [], "app-window": [], "battery-charging": [], "bluetooth": [], "cpu": [],
  "hard-drive": [], "lock": [], "moon": [], "power": [], "rotate-ccw": [], "sun": [],
  "volume-2": [], "volume-x": [], "wifi": [], "wrench": [], "folder-open": [], "info": [],
  "shield-check": [], "check": [3, 2.5], "circle-alert": [2.5], "history": [], "keyboard": [],
  "log-out": [], "minus": [], "plus": [], "trash-2": [], "image-up": [], "loader-circle": [],
  "arrow-down-to-line": [],
};

fs.mkdirSync(out, { recursive: true });
const attrs = (o) =>
  Object.entries(o)
    .filter(([k]) => k !== "key")
    .map(([k, v]) => `${k.replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`)}="${v}"`)
    .join(" ");
for (const [name, widths] of Object.entries(ICONS)) {
  const file = path.join(iconsDir, `${name}.js`);
  if (!fs.existsSync(file)) throw new Error(`missing lucide icon ${name}`);
  const { __iconNode } = await import(pathToFileURL(file).href);
  const body = __iconNode.map(([tag, a]) => `<${tag} ${attrs(a)}/>`).join("");
  for (const width of [2, ...widths]) {
    const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="${width}" stroke-linecap="round" stroke-linejoin="round">${body}</svg>\n`;
    const suffix = width === 2 ? "" : `@${String(width).replace(".", "_")}`;
    fs.writeFileSync(path.join(out, `${name}${suffix}.svg`), svg);
  }
}
console.log(`wrote ${Object.keys(ICONS).length} icons to ${out}`);
