#!/usr/bin/env bun
/**
 * Controller for the Prism quality-control loop.
 *
 * Turns the sensor's measurement into exactly one (or `--batch N`) next
 * target, sized to stay low-risk and reviewable. Selection policy:
 *
 *   1. Apply machine-readable exclusions from the memory file
 *      (`- exclude: <file>:<line>` | `- exclude: <file>` | `- exclude: kind=<kind>`
 *       | `- exclude-pattern: <regex>`).
 *   2. Order remaining sites by category priority — lock, parse, bare,
 *      expect, panic, todo, unimplemented — then by (file, line).
 *   3. Take the first N (default 1).
 *
 * The controller decides WHAT to change; the actuator skill decides HOW.
 * Deterministic, no network, no dependencies beyond Bun's stdlib.
 *
 * Usage:
 *   bun scripts/control-quality.ts sensor-output.json \
 *     --feedback .github/agent-memory/quality-control-loop.md \
 *     --batch 1
 *   # or read sensor JSON from stdin:
 *   bun scripts/sense-quality.ts | bun scripts/control-quality.ts - --feedback <memory>
 */

import { readFileSync } from "node:fs";

interface Site {
  file: string;
  line: number;
  column: number;
  kind: string;
  category: string;
  snippet: string;
}

interface SensorOutput {
  metric: number;
  sites: Site[];
}

const CATEGORY_PRIORITY: Record<string, number> = {
  lock: 0,
  parse: 1,
  bare: 2,
  expect: 3,
  panic: 4,
  todo: 5,
  unimplemented: 6,
};

function parseArgs(argv: string[]): {
  sensorPath: string;
  feedbackPath: string | null;
  batch: number;
} {
  let sensorPath: string | null = null;
  let feedbackPath: string | null = null;
  let batch = 1;

  for (let i = 0; i < argv.length; i += 1) {
    const a = argv[i];
    if (a === "--feedback") {
      feedbackPath = argv[++i] ?? null;
    } else if (a === "--batch") {
      batch = Number.parseInt(argv[++i] ?? "1", 10);
    } else if (!a.startsWith("--") && sensorPath === null) {
      sensorPath = a;
    }
  }

  if (sensorPath === null) throw new Error("sensor JSON path (or '-' for stdin) is required");
  return { sensorPath, feedbackPath, batch: Number.isFinite(batch) && batch > 0 ? batch : 1 };
}

function readSensor(sensorPath: string): SensorOutput {
  const text = sensorPath === "-" ? readFileSync(0, "utf8") : readFileSync(sensorPath, "utf8");
  return JSON.parse(text) as SensorOutput;
}

interface Exclusions {
  files: Set<string>;
  sites: Set<string>; // "file:line"
  kinds: Set<string>;
  patterns: RegExp[];
}

function readExclusions(feedbackPath: string | null): Exclusions {
  const ex: Exclusions = { files: new Set(), sites: new Set(), kinds: new Set(), patterns: [] };
  if (feedbackPath === null) return ex;

  let text: string;
  try {
    text = readFileSync(feedbackPath, "utf8");
  } catch {
    return ex; // missing memory file => no exclusions
  }

  for (const raw of text.split("\n")) {
    const line = raw.trim();
    const siteMatch = line.match(/^-\s*exclude:\s*([^#]+)/);
    const patternMatch = line.match(/^-\s*exclude-pattern:\s*([^#]+)/);
    if (patternMatch) {
      try {
        ex.patterns.push(new RegExp(patternMatch[1].trim()));
      } catch {
        // ignore invalid regex in memory
      }
      continue;
    }
    if (siteMatch) {
      const value = siteMatch[1].trim();
      if (value.startsWith("kind=")) {
        ex.kinds.add(value.slice(5).trim());
      } else if (value.includes(":")) {
        ex.sites.add(value);
      } else {
        ex.files.add(value);
      }
    }
  }
  return ex;
}

function isExcluded(site: Site, ex: Exclusions): boolean {
  const key = `${site.file}:${site.line}`;
  if (ex.sites.has(key)) return true;
  if (ex.files.has(site.file)) return true;
  if (ex.kinds.has(site.kind)) return true;
  return ex.patterns.some((p) => p.test(key) || p.test(site.snippet));
}

function main(): void {
  const { sensorPath, feedbackPath, batch } = parseArgs(Bun.argv.slice(2));
  const sensor = readSensor(sensorPath);
  const ex = readExclusions(feedbackPath);

  const candidates = sensor.sites
    .filter((s) => !isExcluded(s, ex))
    .sort((a, b) => {
      const pa = CATEGORY_PRIORITY[a.category] ?? 99;
      const pb = CATEGORY_PRIORITY[b.category] ?? 99;
      if (pa !== pb) return pa - pb;
      if (a.file !== b.file) return a.file.localeCompare(b.file);
      return a.line - b.line;
    });

  const selected = candidates.slice(0, batch);

  const lines: string[] = [];
  lines.push("# Quality-control target");
  lines.push("");
  lines.push(`- **Metric before this run**: ${sensor.metric} non-test panic-surface sites in \`src-tauri/src\``);
  lines.push(`- **Batch size**: ${batch} (selected ${selected.length})`);
  lines.push(`- **Exclusions applied**: ${ex.sites.size + ex.files.size + ex.kinds.size + ex.patterns.length}`);
  lines.push("");

  if (selected.length === 0) {
    lines.push("## No target");
    lines.push("");
    lines.push("All measured sites are excluded by the memory file, or the metric is already zero.");
    lines.push("Nothing to change this run. The actuator should no-op.");
    process.stdout.write(lines.join("\n") + "\n");
    return;
  }

  for (let i = 0; i < selected.length; i += 1) {
    const s = selected[i];
    lines.push(`## Target ${i + 1}`);
    lines.push("");
    lines.push(`- **File**: \`${s.file}\``);
    lines.push(`- **Line**: ${s.line}`);
    lines.push(`- **Kind**: \`${s.kind}\` · **Category**: \`${s.category}\``);
    lines.push(`- **Snippet**: \`${s.snippet}\``);
    lines.push(`- **Risk**: low (behavior-preserving; one site)`);
    lines.push("");
  }

  lines.push("## Guidance");
  lines.push("");
  lines.push("Read and follow the `quality-control-loop` skill (`.agents/skills/quality-control-loop/SKILL.md`).");
  lines.push("Fix only the selected site(s), preserve behavior, and keep every validation command green");
  lines.push("before committing. Format the final answer per the skill's `references/response-template.md`.");
  lines.push("");

  process.stdout.write(lines.join("\n") + "\n");
}

main();
