// Gap sweep for the React/TypeScript -> Rust/GPUI port (prompts/02).
// Enumerates every production site where Rust or GPUI demands a decision that
// TypeScript and React let the source leave implicit. One row per matching
// line and kind. Deterministic: sorted walk, fixed patterns, no judgment.
// Tests are not translated units (RULEBOOK.md section 4), so they are skipped.
//
//   bun migration/gap-sweep.mjs [--root ../Prism]
// writes migration/gap-sites.tsv and prints the per-kind counts.
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const rootArg = process.argv.includes("--root") ? process.argv[process.argv.indexOf("--root") + 1] : "../Prism";
const prism = resolve(here, "..", rootArg);
const out = join(here, "gap-sites.tsv");

const BUILTIN_HOOKS =
  "State|Reducer|Effect|LayoutEffect|InsertionEffect|Memo|Callback|Ref|Context|Id|Transition|DeferredValue|SyncExternalStore|ImperativeHandle";

// [kind, pattern, applies to (ts = .ts/.tsx, css = .css)]
const PATTERNS = [
  // Ownership and lifetime: where state lives and who cancels what.
  ["react-hook", new RegExp(`\\buse(?:${BUILTIN_HOOKS})\\s*[<(]`), "ts"],
  ["custom-hook", new RegExp(`\\buse(?!(?:${BUILTIN_HOOKS})\\b)[A-Z]\\w*\\s*\\(`), "ts"],
  ["timer", /\b(?:setTimeout|setInterval|clearTimeout|clearInterval|requestAnimationFrame|cancelAnimationFrame|queueMicrotask)\s*\(/, "ts"],
  ["async", /\basync\b|\bawait\b|\.then\(|\.catch\(|\.finally\(|\bnew Promise\b/, "ts"],
  // Behavior stored in data: a Rust row is data, so these become enum values.
  // Any field with an action name, whatever its value: a direct arrow, a
  // conditional (`cond ? () => x : undefined`), or a value on the next line.
  ["closure-field", /^\s*(?:run|runAsAdmin|openLocation|toggleTaskbarPin|showProperties|dragFile|on[A-Z]\w*)\??\s*:/, "ts"],
  ["event-handler", /\bon[A-Z][A-Za-z]*=\{/, "ts"],
  // Host boundary: the WebView and Tauri go away.
  // `[^(]*` spans nested generics such as invoke<Record<string, string>>(.
  ["tauri-command", /\binvoke\b[^(\n]*\(\s*["'][a-z_0-9]+["']/, "ts"],
  ["tauri-import", /from\s+["']@tauri-apps\//, "ts"],
  ["host-dom", /\b(?:document|navigator)\.|\bwindow\.(?!__TAURI)|\b(?:localStorage|sessionStorage)\b|getBoundingClientRect|elementFromPoint|scrollIntoView|\.focus\(|\.blur\(|ResizeObserver|matchMedia|addEventListener|removeEventListener|getComputedStyle/, "ts"],
  // Nullability: optional fields in shared data shapes.
  ["optional-field", /^\s*(?:readonly\s+)?\w+\?\s*:/, "ts"],
  // Presentation: Tailwind, inline styles, icons, tokens, motion.
  ["classname", /\bclassName=/, "ts"],
  ["inline-style", /\bstyle=\{\{/, "ts"],
  ["lucide", /from\s+["']lucide-react["']/, "ts"],
  ["css-token", /^\s*--[\w-]+\s*:/, "css"],
  ["css-keyframes", /@keyframes\b/, "css"],
  ["css-motion", /^\s*(?:transition|animation)(?:-[\w-]+)?\s*:/, "css"],
];

function walk(dir, acc) {
  const entries = readdirSync(dir, { withFileTypes: true }).sort((a, b) =>
    a.name < b.name ? -1 : a.name > b.name ? 1 : 0,
  );
  for (const entry of entries) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) walk(full, acc);
    else if (/\.(ts|tsx|css)$/.test(entry.name) && !/\.test\.(ts|tsx)$/.test(entry.name) && !entry.name.endsWith(".d.ts")) {
      acc.push(full);
    }
  }
  return acc;
}

const rows = [];
const counts = new Map();
for (const file of walk(join(prism, "src"), [])) {
  const rel = `src/${file.slice(join(prism, "src").length + 1).replaceAll("\\", "/")}`;
  const kind = file.endsWith(".css") ? "css" : "ts";
  const lines = readFileSync(file, "utf8").split(/\r?\n/);
  lines.forEach((line, index) => {
    const trimmed = line.trim();
    if (trimmed.startsWith("//") || trimmed.startsWith("*") || trimmed.startsWith("/*")) return;
    for (const [gap, re, applies] of PATTERNS) {
      if (applies !== kind || !re.test(line)) continue;
      rows.push(`${gap}\t${rel}\t${index + 1}\t${trimmed.slice(0, 160).replaceAll("\t", " ")}`);
      counts.set(gap, (counts.get(gap) || 0) + 1);
    }
  });
}

writeFileSync(out, `kind\tfile\tline\ttext\n${rows.join("\n")}\n`);
const summary = [...counts.entries()].sort().map(([gap, n]) => `${gap}\t${n}`).join("\n");
process.stdout.write(`sites=${rows.length}\n${summary}\n`);
