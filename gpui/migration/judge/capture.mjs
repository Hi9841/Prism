// Records the golden outputs from the ORIGINAL app (the executable spec).
// Run only against the original: goldens captured from the port would make
// the judge compare the port with itself.
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { canonical, runCall } from "./call.mjs";
import { goldenScenarios } from "./golden-scenarios.mjs";

if (process.env.PRISM_ADAPTER_CMD) {
  process.stderr.write("capture.mjs records the original only; unset PRISM_ADAPTER_CMD\n");
  process.exit(2);
}

const here = path.dirname(fileURLToPath(import.meta.url));
const dir = path.join(here, "goldens");
fs.mkdirSync(dir, { recursive: true });

let failed = 0;
for (const scenario of goldenScenarios) {
  const outcome = await runCall(scenario.call);
  if (!outcome.ok) {
    failed += 1;
    process.stdout.write(`ERROR\t${scenario.id}\t${outcome.error.trim()}\n`);
    continue;
  }
  fs.writeFileSync(path.join(dir, `${scenario.id}.json`), `${canonical(outcome.result, 2)}\n`);
}
process.stdout.write(`captured: ${goldenScenarios.length - failed}/${goldenScenarios.length}\n`);
process.exit(failed === 0 ? 0 : 1);
