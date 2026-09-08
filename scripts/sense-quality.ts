#!/usr/bin/env bun
/**
 * Sensor for the Prism quality-control loop.
 *
 * Measures the "panic surface" in `src-tauri/src` (non-test Rust): every
 * `.unwrap()`, `.expect()`, `panic!()`, `todo!()`, and `unimplemented!()`
 * call outside `#[cfg(test)]` modules. This is the metric the loop drives
 * toward zero. Test-only sites are counted separately and are NOT part of
 * the metric (the actuator must not touch them).
 *
 * Deterministic: no network, no compilation, no dependencies beyond Bun's
 * stdlib. Output is a single JSON document on stdout, sorted by (file, line).
 *
 * Usage:
 *   bun scripts/sense-quality.ts                    # print JSON to stdout
 *   bun scripts/sense-quality.ts > sensor-output.json
 */

import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";

const SCOPE = "src-tauri/src";

interface Site {
  file: string;
  line: number;
  column: number;
  kind: "unwrap" | "expect" | "panic" | "todo" | "unimplemented";
  category: "lock" | "parse" | "expect" | "panic" | "todo" | "unimplemented" | "bare";
  snippet: string;
}

interface SensorOutput {
  schema: string;
  generatedAt: string;
  scope: string;
  metric: number;
  counts: {
    total: number;
    nonTest: number;
    testOnly: number;
    byKind: Record<string, number>;
    byCategory: Record<string, number>;
  };
  sites: Site[];
}

const TOKEN_RE = /\.(unwrap|expect)\(|\b(panic|todo|unimplemented)!\(/g;
const LOCK_RE = /\.lock\s*\(/;
const PARSE_RE = /\bread_(?:u8|u16|u32|u64|i32|i64)\s*\(/;

function classify(kind: Site["kind"], context: string): Site["category"] {
  if (kind === "expect") return "expect";
  if (kind === "panic") return "panic";
  if (kind === "todo") return "todo";
  if (kind === "unimplemented") return "unimplemented";
  // kind === "unwrap". `context` includes the preceding lines so multi-line
  // chains like `self.x\n    .lock()\n    .unwrap()` are classified as `lock`.
  if (LOCK_RE.test(context)) return "lock";
  if (PARSE_RE.test(context)) return "parse";
  return "bare";
}

function walk(dir: string): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir)) {
    const abs = join(dir, entry);
    if (statSync(abs).isDirectory()) {
      out.push(...walk(abs));
    } else if (entry.endsWith(".rs")) {
      out.push(abs.replace(/\\/g, "/"));
    }
  }
  return out.sort();
}

function scanFile(path: string): { sites: Site[]; testOnly: number; total: number } {
  const src = readFileSync(path, "utf8");
  const lines = src.split("\n");

  const sites: Site[] = [];
  let testOnly = 0;
  let total = 0;

  let brace = 0;
  const testStack: number[] = [];
  let pendingTest: number | null = null;

  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i];
    const isCfgTest = line.includes("#[cfg(test)]");
    const opens = (line.match(/\{/g) ?? []).length;
    const closes = (line.match(/\}/g) ?? []).length;

    if (isCfgTest) {
      // `#[cfg(test)]` before a brace-less item (use/const/static/type) does not
      // open a scope; otherwise the next `{` opens a test scope.
      if (opens === 0 && /\b(use|const|static|type)\b/.test(line)) {
        pendingTest = null;
      } else {
        pendingTest = brace;
      }
    }

    const inTest = testStack.some((d) => brace >= d);
    const context = `${lines[i - 2] ?? ""}\n${lines[i - 1] ?? ""}\n${line}`;

    TOKEN_RE.lastIndex = 0;
    for (const m of line.matchAll(TOKEN_RE)) {
      total += 1;
      if (inTest) {
        testOnly += 1;
        continue;
      }
      const col = (m.index ?? 0) + 1;
      const kind = (m[1] ?? m[2]) as Site["kind"];
      const category = classify(kind, context.slice(0, context.length - (line.length - (m.index ?? 0))));
      sites.push({
        file: path,
        line: i + 1,
        column: col,
        kind,
        category,
        snippet: line.trim().slice(0, 120),
      });
    }

    if (opens > 0 && pendingTest !== null) {
      testStack.push(brace);
      pendingTest = null;
    } else if (
      opens === 0 &&
      pendingTest !== null &&
      line.trim() !== "" &&
      !line.includes("#[cfg(test)]") &&
      !line.trim().startsWith("//")
    ) {
      // Brace-less attributed item — the attribute did not open a scope.
      pendingTest = null;
    }

    brace += opens - closes;
    while (testStack.length > 0 && brace <= testStack[testStack.length - 1]) {
      testStack.pop();
    }
  }

  return { sites, testOnly, total };
}

function main(): void {
  const files = walk(SCOPE);
  const allSites: Site[] = [];
  let testOnly = 0;
  let total = 0;

  for (const path of files) {
    const r = scanFile(path);
    allSites.push(...r.sites);
    testOnly += r.testOnly;
    total += r.total;
  }

  allSites.sort((a, b) => a.file.localeCompare(b.file) || a.line - b.line);

  const byKind: Record<string, number> = {};
  const byCategory: Record<string, number> = {};
  for (const s of allSites) {
    byKind[s.kind] = (byKind[s.kind] ?? 0) + 1;
    byCategory[s.category] = (byCategory[s.category] ?? 0) + 1;
  }

  const output: SensorOutput = {
    schema: "prism-quality-loop-sensor/v1",
    generatedAt: new Date().toISOString(),
    scope: SCOPE,
    metric: allSites.length,
    counts: {
      total,
      nonTest: allSites.length,
      testOnly,
      byKind,
      byCategory,
    },
    sites: allSites,
  };

  process.stdout.write(JSON.stringify(output, null, 2) + "\n");
}

main();
