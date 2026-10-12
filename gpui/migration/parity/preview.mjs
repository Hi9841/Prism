// Visual preview: renders parity scenarios from the working tree and shows them in a browser page.
// Usage (from gpui/):
//   bun migration/parity/preview.mjs <label> <scenario ...> [--crop x,y,w,h] [--no-build] [--no-open]
// Each run saves target/preview/<label>/<scenario>.png. The page target/preview/index.html shows
// every label side by side per scenario, so a "before" run and an "after" run compare directly.
// Captures come from the off-screen parity window: nothing opens on the desktop except the page.
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

const root = resolve(import.meta.dir, "../..");
const args = process.argv.slice(2);
const flag = (name) => args.includes(name);
const option = (name) => {
  const at = args.indexOf(name);
  return at >= 0 ? args[at + 1] : undefined;
};
const positional = args.filter((a, i) => !a.startsWith("--") && !(i > 0 && args[i - 1] === "--crop"));
const [label, ...scenarioNames] = positional;
if (!label || scenarioNames.length === 0) {
  console.error("usage: bun migration/parity/preview.mjs <label> <scenario ...> [--crop x,y,w,h] [--no-build] [--no-open]");
  process.exit(2);
}
const crop = option("--crop")?.split(",").map(Number);

const tmp = join(root, "target/tmp");
mkdirSync(tmp, { recursive: true });
const env = { ...process.env, TMP: tmp, TEMP: tmp };
const run = (cmd, argv, opts = {}) => {
  const result = spawnSync(cmd, argv, { cwd: root, env, stdio: "inherit", ...opts });
  if (result.status !== 0) {
    console.error(`${cmd} ${argv.join(" ")}: exit ${result.status}`);
    process.exit(1);
  }
  return result;
};

const fixturePath = join(root, "target/parity/fixture.json");
if (!existsSync(fixturePath)) {
  mkdirSync(join(root, "target/parity"), { recursive: true });
  run("python", ["-I", join(import.meta.dir, "make-fixture.py"), fixturePath]);
}
if (!flag("--no-build")) run("cargo", ["build", "-p", "prism-app"]);

const scriptPath = join(import.meta.dir, "scenarios.json");
const script = JSON.parse(readFileSync(scriptPath, "utf8"));
const base = JSON.parse(readFileSync(fixturePath, "utf8"));
const out = join(root, "target/preview");
const labelDir = join(out, label);
mkdirSync(join(out, "fixtures"), { recursive: true });
mkdirSync(labelDir, { recursive: true });

for (const name of scenarioNames) {
  const scenario = script.scenarios.find((s) => s.name === name);
  if (!scenario) {
    console.error(`unknown scenario: ${name}`);
    process.exit(2);
  }
  const merged = structuredClone(base);
  merged.state.settings = { ...merged.state.settings, ...(scenario.settings ?? {}) };
  const scenarioFixture = join(out, "fixtures", `${name}.json`);
  writeFileSync(scenarioFixture, JSON.stringify(merged));
  run(join(root, "target/debug/prism.exe"), ["--parity", scenarioFixture, scriptPath, name, labelDir], {
    stdio: ["ignore", "ignore", "inherit"],
    timeout: 60_000,
  });
  if (crop) {
    const png = join(labelDir, `${name}.png`);
    const [x, y, w, h] = crop;
    run("python", ["-I", "-c", `from PIL import Image;i=Image.open(r"${png}");i.crop((${x},${y},${x + w},${y + h})).save(r"${png}")`]);
  }
  console.log(`${label}/${name}.png`);
}

// The page: one row per scenario, one column per label (least recently captured label first).
const newest = (l) => Math.max(0, ...readdirSync(join(out, l)).map((f) => statSync(join(out, l, f)).mtimeMs));
const labels = readdirSync(out, { withFileTypes: true })
  .filter((d) => d.isDirectory() && d.name !== "fixtures")
  .map((d) => d.name)
  .sort((a, b) => newest(a) - newest(b));
const scenarios = [...new Set(labels.flatMap((l) => readdirSync(join(out, l)).filter((f) => f.endsWith(".png"))))].sort();
const cells = (png) =>
  labels
    .map((l) =>
      existsSync(join(out, l, png))
        ? `<figure><figcaption>${l}</figcaption><img src="${l}/${png}?${Date.now()}"></figure>`
        : `<figure><figcaption>${l}</figcaption><p>no capture</p></figure>`,
    )
    .join("");
writeFileSync(
  join(out, "index.html"),
  `<!doctype html><meta charset="utf-8"><title>Prism preview</title>
<style>
body{background:#111;color:#ddd;font:14px system-ui;margin:24px}
section{margin-bottom:40px} h2{font-size:15px;font-weight:600}
.row{display:flex;gap:24px;flex-wrap:wrap;align-items:flex-start}
figure{margin:0} figcaption{margin-bottom:6px;color:#999}
img{image-rendering:pixelated;zoom:var(--z,1);border:1px solid #333}
label{position:fixed;top:12px;right:24px}
</style>
<label>zoom <input type="range" min="1" max="4" step="0.5" value="${crop ? 3 : 1}" oninput="document.body.style.setProperty('--z',this.value)"></label>
<script>document.body.style.setProperty('--z','${crop ? 3 : 1}')</script>
${scenarios.map((png) => `<section><h2>${png.replace(".png", "")}</h2><div class="row">${cells(png)}</div></section>`).join("\n")}`,
);
console.log(join(out, "index.html"));
if (!flag("--no-open")) spawnSync("cmd", ["/c", "start", "", join(out, "index.html")]);
