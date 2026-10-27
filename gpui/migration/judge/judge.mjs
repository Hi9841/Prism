// Parity judge. Talks only to an adapter process. Does not import Prism.
// Literal scenarios (scenarios.mjs) carry their expected values; golden
// scenarios (golden-scenarios.mjs) compare against goldens/, captured from the
// original React/Tauri app (removed after release 1.0.11; the goldens stay).
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { canonical, runCall } from "./call.mjs";
import { goldenScenarios } from "./golden-scenarios.mjs";
import { scenarios } from "./scenarios.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const batch = process.argv.find((arg) => arg.startsWith("--batch="))?.slice("--batch=".length);

// Key order never decides a diff: a Rust adapter may write sorted keys.
function deepEqual(left, right) {
  return canonical(left) === canonical(right);
}

const isNumber = (value) => typeof value === "number";

function closeEnough(actual, expected, digits) {
  const tolerance = 10 ** -digits / 2;
  return Math.abs(actual - expected) < tolerance;
}

function expectedClose(token) {
  if (token === "PI") return Math.PI;
  if (token === "PI*2") return Math.PI * 2;
  throw new Error(`unknown closeTo token ${token}`);
}

function listOf(result, field) {
  if (field) return result[field];
  return result.ids ?? result.names ?? result.appIds;
}

function matchOne(expect, result, call) {
  if (expect.kind === "eq") return deepEqual(result, expect.value);
  if (expect.kind === "golden") return canonical(result) === canonical(golden(expect.id));
  if (expect.kind === "closeTo") {
    return (
      result.returned === "value" &&
      isNumber(result.value) &&
      closeEnough(result.value, expectedClose(expect.token), expect.digits)
    );
  }
  // Vitest's toBeGreaterThan accepts numbers only; so does this.
  if (expect.kind === "scoreOrder") {
    const [first, second] = result.scores;
    if (expect.nonNull && (first === null || second === null)) return false;
    return isNumber(first) && isNumber(second) && first > second;
  }
  // One numeric score per input pair: an empty or short list must not pass.
  if (expect.kind === "allNonNull") {
    return (
      Array.isArray(result.scores) &&
      result.scores.length === call.pairs.length &&
      result.scores.every(isNumber)
    );
  }
  // List matchers need a real array: a string has .includes, .length, and [i].
  if (expect.kind === "contains") {
    const list = listOf(result, expect.field);
    return Array.isArray(list) && list.includes(expect.value);
  }
  if (expect.kind === "length") {
    const list = listOf(result, expect.field);
    return Array.isArray(list) && list.length === expect.value;
  }
  if (expect.kind === "prefix") {
    const list = listOf(result, expect.field);
    return Array.isArray(list) && expect.value.every((item, index) => deepEqual(list[index], item));
  }
  if (expect.kind === "flag") return result[expect.field] === expect.value;
  throw new Error(`unknown expect kind ${expect.kind}`);
}

function matches(expect, result, call) {
  const checks = Array.isArray(expect) ? expect : [expect];
  return checks.every((check) => matchOne(check, result, call));
}

function golden(id) {
  return JSON.parse(fs.readFileSync(path.join(here, "goldens", `${id}.json`), "utf8"));
}

const all = [
  ...scenarios,
  ...goldenScenarios.map((scenario) => ({ ...scenario, batch: "golden", expect: { kind: "golden", id: scenario.id } })),
];
const selected = batch ? all.filter((scenario) => scenario.batch === batch) : all;
if (selected.length === 0) {
  process.stdout.write(`FAIL\tno scenarios selected (batch ${JSON.stringify(batch)})\n`);
  process.exit(1);
}
let failed = 0;
for (const scenario of selected) {
  const outcome = await runCall(scenario.call);
  let pass = false;
  let detail = outcome.ok ? JSON.stringify(outcome.result) : outcome.error.trim();
  if (outcome.ok) {
    // A malformed result (missing field, wrong type) is a failed scenario,
    // never a crash that hides every later result.
    try {
      pass = matches(scenario.expect, outcome.result, scenario.call);
    } catch (error) {
      detail = `matcher error: ${error.message} in ${detail}`;
    }
  }
  if (!pass) {
    failed += 1;
    process.stdout.write(`FAIL\t${scenario.id}\t${detail}\n`);
  }
}
const passed = selected.length - failed;
process.stdout.write(`pass: ${passed}/${selected.length}\n`);
process.exit(failed === 0 ? 0 : 1);
