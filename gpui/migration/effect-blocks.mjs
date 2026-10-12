// Lists every React effect and callback block in the original UI with its line
// range and the gap sweep sites inside it, so that one inventory row can cover
// a whole block. Brace matching skips strings, template literals, and comments.
//
//   bun migration/effect-blocks.mjs [--root ..]  -> migration/effect-blocks.tsv
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const rootArg = process.argv.includes("--root") ? process.argv[process.argv.indexOf("--root") + 1] : "..";
const prism = resolve(here, "..", rootArg);
const BLOCK_RE = /\buse(Effect|LayoutEffect|Callback|Memo)\s*\(/g;

// Index of the `)` that closes the call opened at `open` (the `(`).
function closeParen(text, open) {
  let depth = 0;
  for (let i = open; i < text.length; i += 1) {
    const c = text[i];
    if (c === "/" && text[i + 1] === "/") {
      i = text.indexOf("\n", i);
      if (i < 0) return -1;
      continue;
    }
    if (c === "/" && text[i + 1] === "*") {
      i = text.indexOf("*/", i + 2) + 1;
      continue;
    }
    if (c === '"' || c === "'" || c === "`") {
      for (i += 1; i < text.length && text[i] !== c; i += 1) if (text[i] === "\\") i += 1;
      continue;
    }
    if (c === "(" || c === "{" || c === "[") depth += 1;
    if (c === ")" || c === "}" || c === "]") {
      depth -= 1;
      if (depth === 0) return i;
    }
  }
  return -1;
}

const lineOf = (text, index) => text.slice(0, index).split("\n").length;

const sites = readFileSync(join(here, "gap-sites.tsv"), "utf8")
  .trim()
  .split(/\r?\n/)
  .slice(1)
  .map((line) => line.split("\t"))
  .map(([kind, file, ln]) => ({ kind, file, line: Number(ln) }));
const files = [...new Set(sites.map((site) => site.file))].filter((file) => /\.tsx?$/.test(file)).sort();

const rows = [];
for (const file of files) {
  const text = readFileSync(join(prism, file), "utf8").replaceAll("\r\n", "\n");
  for (const match of text.matchAll(BLOCK_RE)) {
    const open = match.index + match[0].length - 1;
    const close = closeParen(text, open);
    if (close < 0) continue;
    const start = lineOf(text, match.index);
    const end = lineOf(text, close);
    const inside = sites.filter(
      (site) => site.file === file && site.line >= start && site.line <= end && site.kind !== "react-hook",
    );
    const kinds = [...new Set(inside.map((site) => site.kind))].sort().join(",");
    rows.push(`${file}\tuse${match[1]}\t${start}\t${end}\t${inside.length}\t${kinds}`);
  }
}
writeFileSync(join(here, "effect-blocks.tsv"), `file\thook\tstart\tend\tgap_sites_inside\tkinds\n${rows.join("\n")}\n`);
process.stdout.write(`blocks=${rows.length}\n`);
