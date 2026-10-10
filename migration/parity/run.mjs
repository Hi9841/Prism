// Visual parity run: every scenario on both sides, then the scores.
// Usage (from the repo root):
//   python -I migration/parity/make-fixture.py target/parity/fixture.json
//   bun migration/parity/run.mjs [scenario ...]
// Needs target/parity/original-dist (vite build of ../Prism) and
// target/debug/prism.exe (cargo build -p prism-app).
import { spawnSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

const root = resolve(import.meta.dir, "../..");
const work = join(root, "target/parity/run");
const fixture = JSON.parse(readFileSync(join(root, "target/parity/fixture.json"), "utf8"));
const scriptPath = join(import.meta.dir, "scenarios.json");
const script = JSON.parse(readFileSync(scriptPath, "utf8"));
const only = process.argv.slice(2);
const scenarios = script.scenarios.filter((s) => only.length === 0 || only.includes(s.name));

mkdirSync(join(work, "fixtures"), { recursive: true });
for (const scenario of scenarios) {
  const merged = structuredClone(fixture);
  merged.state.settings = { ...merged.state.settings, ...(scenario.settings ?? {}) };
  const fixturePath = join(work, "fixtures", `${scenario.name}.json`);
  writeFileSync(fixturePath, JSON.stringify(merged));
  const port = spawnSync(join(root, "target/debug/prism.exe"), ["--parity", fixturePath, scriptPath, scenario.name, join(work, "port")], {
    stdio: ["ignore", "ignore", "pipe"],
    timeout: 60_000,
  });
  if (port.status !== 0) console.log(`port ${scenario.name}: exit ${port.status} ${port.stderr}`);
  const orig = spawnSync(
    "bun",
    [join(import.meta.dir, "run-original.mjs"), fixturePath, scriptPath, scenario.name, join(work, "orig"), join(root, "target/parity/original-dist")],
    { stdio: ["ignore", "ignore", "pipe"], timeout: 60_000 },
  );
  if (orig.status !== 0) console.log(`orig ${scenario.name}: exit ${orig.status} ${orig.stderr}`);
  process.stdout.write(".");
}
console.log();
const compare = spawnSync("python", ["-I", join(import.meta.dir, "compare.py"), join(work, "orig"), join(work, "port"), join(work, "diff")], {
  encoding: "utf8",
});
process.stdout.write(compare.stdout + compare.stderr);
