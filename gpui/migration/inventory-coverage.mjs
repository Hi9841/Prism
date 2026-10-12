// Coverage check for the gap inventory (prompts/02: the inventory is complete
// when every site in gap-sites.tsv has a row, and not before).
//
// A site is covered by:
// - a traced row for the same file whose evidence has `covers=<a>-<b>[,<a>-<b>]`
//   with the site's line inside a range; or
// - a rule row: file `*`, symbol `rule:<kind>[/<subpattern>][@<file>]`.
//   <subpattern> must appear in the site's text; @<file> limits it to one file.
//
//   bun migration/inventory-coverage.mjs [--list]
// Prints covered/total per kind; --list prints every uncovered site.
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const read = (name) =>
  readFileSync(join(here, name), "utf8")
    .trim()
    .split(/\r?\n/)
    .slice(1)
    .filter((line) => line.trim() && !line.startsWith("#"))
    .map((line) => line.split("\t"));

const sites = read("gap-sites.tsv").map(([kind, file, line, text]) => ({ kind, file, line: Number(line), text }));
const rows = read("inventory.tsv").map(([file, symbol, , , , evidence]) => ({ file, symbol, evidence: evidence ?? "" }));

const ranges = rows
  .filter((row) => row.file !== "*")
  .flatMap((row) =>
    [...row.evidence.matchAll(/covers=([\d\-,]+)/g)].flatMap((match) =>
      match[1].split(",").filter(Boolean).map((part) => {
        const [a, b] = part.split("-").map(Number);
        return { file: row.file, a, b: b || a };
      }),
    ),
  );

const rules = rows
  .filter((row) => row.file === "*" && row.symbol.startsWith("rule:"))
  .map((row) => {
    const spec = row.symbol.slice("rule:".length);
    const [kindPart, file] = spec.split("@");
    const [kind, sub] = kindPart.split("/");
    return { kind, sub, file };
  });

const covered = (site) =>
  ranges.some((r) => r.file === site.file && site.line >= r.a && site.line <= r.b) ||
  rules.some((r) => r.kind === site.kind && (!r.file || r.file === site.file) && (!r.sub || site.text.includes(r.sub)));

const byKind = new Map();
const missing = [];
for (const site of sites) {
  const entry = byKind.get(site.kind) ?? { total: 0, done: 0 };
  entry.total += 1;
  if (covered(site)) entry.done += 1;
  else missing.push(site);
  byKind.set(site.kind, entry);
}
for (const [kind, { total, done }] of [...byKind.entries()].sort()) {
  process.stdout.write(`${kind}\t${done}/${total}\n`);
}
process.stdout.write(`covered ${sites.length - missing.length}/${sites.length}\n`);
if (process.argv.includes("--list")) {
  for (const site of missing) process.stdout.write(`MISSING\t${site.kind}\t${site.file}:${site.line}\t${site.text}\n`);
}
process.exit(missing.length === 0 ? 0 : 1);
