#!/usr/bin/env node
// Adapted from the migration kit's depmap_ts.mjs for Prism's UI program.
// Relative import/export/require only. Bare specifiers are not edges.
// Walks --root but keeps only src/**/*.{ts,tsx,css,svg} plus any other
// in-repo file those imports resolve to (package.json). Skips website,
// landing, src-tauri, node_modules, dist, and other non-UI trees.

import { existsSync, mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, relative, resolve, sep } from "node:path";

const EXTS = [".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs", ".css", ".svg"];
const SKIP_DIRS = new Set([
  "node_modules",
  "dist",
  "build",
  ".git",
  "coverage",
  "target",
  "website",
  "landing",
  "prism-video",
  "artifacts",
  "src-tauri",
  "plans",
  "prompts",
  "scripts",
]);

function args() {
  const out = {};
  const argv = process.argv.slice(2);
  for (let i = 0; i < argv.length; i += 2) out[argv[i].replace(/^--/, "")] = argv[i + 1];
  if (!out.root || !out.out) {
    process.stderr.write("usage: --root <dir> --out <dir>\n");
    process.exit(1);
  }
  return out;
}

function walk(dir, acc) {
  const entries = readdirSync(dir, { withFileTypes: true }).sort((a, b) =>
    a.name < b.name ? -1 : a.name > b.name ? 1 : 0,
  );
  for (const entry of entries) {
    if (entry.name.startsWith(".") || SKIP_DIRS.has(entry.name)) continue;
    if (entry.isSymbolicLink()) continue;
    const full = join(dir, entry.name);
    if (entry.isDirectory()) walk(full, acc);
    else if (entry.isFile() && EXTS.some((ext) => entry.name.endsWith(ext)) && !entry.name.endsWith(".d.ts")) {
      acc.push(full);
    }
  }
  return acc;
}

// A mocked module path (vi.mock / vi.doMock / vi.importActual / vi.importMock)
// is a dependency of the test that mocks it, even with no import statement.
const SPEC_RE =
  /(?:from\s+|import\s*\(\s*|require\s*\(\s*|import\s+|\bvi\.(?:mock|doMock|unmock|importActual|importMock)\s*(?:<[^>]*>)?\(\s*)["']([^"']+)["']/g;

const FILE_READ_RE = /\breadFileSync\(\s*(?:(?:path\.)?(?:resolve|join)\(\s*)?["']([^"']+)["']/g;

function resolveRootPath(literal, root) {
  const candidate = resolve(root, literal);
  const fromRoot = relative(root, candidate);
  if (fromRoot.startsWith("..") || fromRoot === "") return null;
  return existsSync(candidate) && statSync(candidate).isFile() ? candidate : null;
}

function resolveSpecifier(spec, fromFile, root) {
  if (!spec.startsWith(".")) return null;
  const base = resolve(dirname(fromFile), spec.split("?")[0]);
  const candidates = [base, ...EXTS.map((ext) => base + ext), ...EXTS.map((ext) => join(base, `index${ext}`))];
  for (const candidate of candidates) {
    const fromRoot = relative(root, candidate);
    if (fromRoot.startsWith("..") || fromRoot === "") continue;
    if (existsSync(candidate) && statSync(candidate).isFile()) return candidate;
  }
  return null;
}

function moduleOf(relPath) {
  if (relPath === "package.json") return "package-meta";
  const parts = relPath.split("/");
  if (parts[0] !== "src") return parts[0];
  const head = parts[1];
  if (!head || head.includes(".")) return "src";
  if (head === "features" && parts[2] && !parts[2].includes(".")) return `src/features/${parts[2]}`;
  return `src/${head}`;
}

function tarjan(nodes, adj) {
  let counter = 0;
  const index = new Map();
  const low = new Map();
  const onStack = new Set();
  const stack = [];
  const sccs = [];
  for (const start of nodes) {
    if (index.has(start)) continue;
    const work = [[start, 0]];
    while (work.length) {
      const frame = work[work.length - 1];
      const [node, pi] = frame;
      if (pi === 0) {
        index.set(node, counter);
        low.set(node, counter);
        counter += 1;
        stack.push(node);
        onStack.add(node);
      }
      let recursed = false;
      const succs = adj.get(node) || [];
      for (let i = pi; i < succs.length; i += 1) {
        const next = succs[i];
        if (!index.has(next)) {
          frame[1] = i + 1;
          work.push([next, 0]);
          recursed = true;
          break;
        } else if (onStack.has(next)) low.set(node, Math.min(low.get(node), index.get(next)));
      }
      if (recursed) continue;
      if (low.get(node) === index.get(node)) {
        const scc = [];
        for (;;) {
          const item = stack.pop();
          onStack.delete(item);
          scc.push(item);
          if (item === node) break;
        }
        sccs.push(scc.sort());
      }
      work.pop();
      if (work.length) {
        const parent = work[work.length - 1][0];
        low.set(parent, Math.min(low.get(parent), low.get(node)));
      }
    }
  }
  return sccs;
}

function condense(nodes, edges, groupOf) {
  const groups = [...new Set(nodes.map(groupOf))].sort();
  const adj = new Map(groups.map((group) => [group, []]));
  const seen = new Set();
  for (const edge of edges) {
    const [from, to] = edge.split("\t");
    const left = groupOf(from);
    const right = groupOf(to);
    if (left === right) continue;
    const key = `${left}\t${right}`;
    if (seen.has(key)) continue;
    seen.add(key);
    adj.get(left).push(right);
  }
  for (const value of adj.values()) value.sort();
  const sccs = tarjan(groups, adj);
  return { groups, edges: [...seen].sort(), cycles: sccs.filter((scc) => scc.length > 1) };
}

const { root: rootArg, out: outArg } = args();
const root = resolve(rootArg);
const rel = (file) => relative(root, file).split(sep).join("/");
const files = walk(root, []).filter((file) => rel(file).startsWith("src/"));
const edges = new Set();
const extra = new Set();
for (const file of files) {
  const text = readFileSync(file, "utf8");
  const deps = [...text.matchAll(SPEC_RE)].map((match) => resolveSpecifier(match[1], file, root));
  // Files a test reads from disk. Vitest runs from the repo root, so the
  // literal path is root-relative (e.g. readFileSync(resolve("src/x.css"))).
  for (const match of text.matchAll(FILE_READ_RE)) deps.push(resolveRootPath(match[1], root));
  for (const dep of deps) {
    if (!dep || dep === file) continue;
    const depRel = rel(dep);
    if (!depRel.startsWith("src/")) extra.add(dep);
    edges.add(`${rel(file)}\t${depRel}`);
  }
}
const allFiles = [...files, ...extra].sort((a, b) => (rel(a) < rel(b) ? -1 : rel(a) > rel(b) ? 1 : 0));
const sortedEdges = [...edges].sort();
const nodes = allFiles.map(rel);
const adj = new Map(nodes.map((node) => [node, []]));
for (const edge of sortedEdges) {
  const [from, to] = edge.split("\t");
  adj.get(from).push(to);
}
for (const value of adj.values()) value.sort();

const sccs = tarjan(nodes, adj);
const cycles = sccs.filter((scc) => scc.length > 1);
const modules = condense(nodes, sortedEdges, moduleOf);
const packages = condense(nodes, sortedEdges, () => "prism-ui");

mkdirSync(outArg, { recursive: true });
writeFileSync(join(outArg, "edges.tsv"), `from\tto\n${sortedEdges.map((edge) => `${edge}\n`).join("")}`);
writeFileSync(
  join(outArg, "order.txt"),
  `# migration order: one batch per line, dependencies first\n${sccs.map((scc) => `${scc.join("\t")}\n`).join("")}`,
);
writeFileSync(
  join(outArg, "cycles.txt"),
  `# strongly connected components > 1 file: ${cycles.length}\n${cycles.map((scc) => `${scc.join("\t")}\n`).join("")}`,
);
writeFileSync(
  join(outArg, "module-cycles.txt"),
  `# directory modules > 1: ${modules.cycles.length}\n${modules.cycles.map((scc) => `${scc.join("\t")}\n`).join("")}`,
);
writeFileSync(
  join(outArg, "package-cycles.txt"),
  `# compiler packages > 1: ${packages.cycles.length}\n# prism-ui is the single tsc program (tsconfig include: src)\n${packages.cycles.map((scc) => `${scc.join("\t")}\n`).join("")}`,
);
process.stdout.write(
  `files=${allFiles.length} edges=${sortedEdges.length} batches=${sccs.length} cycles=${cycles.length} module_cycles=${modules.cycles.length} package_cycles=${packages.cycles.length}\n`,
);
if (allFiles.length > 1 && sortedEdges.length === 0) {
  process.stderr.write("WARN: 0 edges across multiple files\n");
}
