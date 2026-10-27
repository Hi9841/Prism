// One adapter call: JSON on stdin, JSON on stdout. The judge never imports
// the implementation.
//
// The implementation under test is the GPUI port's adapter
// (`cargo build -p prism-judge`, then `target/debug/prism-judge.exe`).
// PRISM_ADAPTER_CMD selects another adapter. The goldens were captured from
// the original React/Tauri app, which left the repo after release 1.0.11.
import { spawn } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const defaultAdapter = path.resolve(here, "../../target/debug/prism-judge.exe");

function command() {
  return { file: process.env.PRISM_ADAPTER_CMD || defaultAdapter, args: [] };
}

export function runCall(call) {
  const { file, args } = command();
  return new Promise((resolve) => {
    const child = spawn(file, args, {
      stdio: ["pipe", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk;
    });
    child.on("error", (error) => resolve({ ok: false, error: String(error) }));
    child.on("close", (code) => {
      if (code !== 0) {
        resolve({ ok: false, error: stderr || `adapter exit ${code}` });
        return;
      }
      let result;
      try {
        result = JSON.parse(stdout);
      } catch (error) {
        resolve({ ok: false, error: `bad json: ${stdout} (${error})` });
        return;
      }
      const violation = numberContractViolation(result);
      if (violation) {
        resolve({ ok: false, error: `contract: ${violation}; encode it as {"$num": ...}` });
        return;
      }
      resolve({ ok: true, result });
    });
    child.stdin.write(JSON.stringify(call));
    child.stdin.end();
  });
}

// JSON.parse turns `-0.0` into -0 and `1e999` into Infinity; JSON has no NaN.
// The adapter contract (crates/prism-judge) carries all three as {"$num"}, so a
// raw one here means the adapter hid a value the comparison could not see.
function numberContractViolation(value, at = "$") {
  if (typeof value === "number") {
    if (Object.is(value, -0)) return `raw -0 at ${at}`;
    if (!Number.isFinite(value)) return `non-finite number at ${at}`;
    return null;
  }
  if (Array.isArray(value)) {
    for (let index = 0; index < value.length; index += 1) {
      const found = numberContractViolation(value[index], `${at}[${index}]`);
      if (found) return found;
    }
    return null;
  }
  if (value && typeof value === "object") {
    for (const [key, item] of Object.entries(value)) {
      const found = numberContractViolation(item, `${at}.${key}`);
      if (found) return found;
    }
  }
  return null;
}

/** JSON with object keys sorted at every level, so key order never decides a diff. */
export function canonical(value, indent) {
  return JSON.stringify(sortKeys(value), null, indent);
}

function sortKeys(value) {
  if (Array.isArray(value)) return value.map(sortKeys);
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.keys(value)
        .sort()
        .map((key) => [key, sortKeys(value[key])]),
    );
  }
  return value;
}
