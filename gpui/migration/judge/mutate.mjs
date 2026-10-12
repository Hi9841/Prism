// Runs the judge against copies of Prism with one behavior flipped at a time.
// Does not modify C:\Users\hi\Desktop\Work\Prism.
import { spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const prism = path.resolve(here, "../../../Prism");
const judge = path.join(here, "judge.mjs");
const mutations = [
  {
    id: "extension-case-sensitive",
    file: "src/features/palette/sections.ts",
    from: 'const extension = entry.name.split(".").pop()?.toLowerCase() ?? "";',
    to: 'const extension = entry.name.split(".").pop() ?? "";',
  },
  {
    id: "ts-is-text-not-video",
    // The first "ts" entry is in the video list (sections.ts:617).
    file: "src/features/palette/sections.ts",
    from: '"ts",',
    to: "",
  },
  {
    id: "settings-icon-wrong-tint",
    file: "src/features/palette/phase1.ts",
    from: 'settings: { icon: Home, tint: "azure" },',
    to: 'settings: { icon: Home, tint: "slate" },',
  },
  {
    id: "sort-byte-order",
    file: "src/lib/emoji.ts",
    from: "a.name.localeCompare(b.name, undefined, { numeric: true }),",
    to: "a.name < b.name ? -1 : a.name > b.name ? 1 : 0,",
  },
  {
    id: "dedupe-case-sensitive",
    file: "src/features/palette/sections.ts",
    from: "(entry) => !appPaths.has(entry.path.toLowerCase()),",
    to: "(entry) => !appPaths.has(entry.path),",
  },
  {
    id: "folder-contents-dropped",
    file: "src/features/palette/sections.ts",
    from: "if (filePathBrowse && fileItems.length > 0) {",
    to: "if (false && filePathBrowse && fileItems.length > 0) {",
  },
  {
    id: "unc-path-not-local",
    file: "src/features/palette/sections.ts",
    from: String.raw`|| app.path.startsWith("\\\\"))`,
    to: "|| false)",
  },
  {
    id: "group-id-limit-100",
    file: "src/state/app.tsx",
    from: "id.length > 96",
    to: "id.length > 100",
  },
  {
    id: "zoom-130-removed",
    file: "src/lib/types.ts",
    from: "70, 80, 90, 100, 110, 120, 130, 140, 150,",
    to: "70, 80, 90, 100, 110, 120, 140, 150,",
  },
  {
    // JSON.stringify turns NaN into null; the adapter's number encoding must not.
    id: "percent-nan-for-no-total",
    file: "src/features/updater/update-progress.ts",
    from: "if (!totalBytes || totalBytes <= 0) return null;",
    to: "if (!totalBytes || totalBytes <= 0) return Number.NaN;",
  },
  {
    id: "tools-become-commands",
    file: "src/features/palette/sections.ts",
    from: '.filter((hit) => hit.source === "windows-tool")',
    to: '.filter((hit) => hit.source === "windowsTool")',
  },
  {
    id: "action-runs-row-id",
    file: "src/features/palette/phase1.ts",
    from: "run: () => executeAction(actionId),",
    to: "run: () => executeAction(hit.id),",
  },
  {
    id: "window-focus-wrong-handle",
    file: "src/features/palette/phase1.ts",
    from: "focusWindow(hwnd)",
    to: "focusWindow(0)",
  },
  {
    id: "pin-flip-inverted",
    file: "src/features/palette/sections.ts",
    from: "await setTaskbarPinned(path, !pinned);",
    to: "await setTaskbarPinned(path, pinned);",
  },
  {
    id: "path-browse-shows-windows",
    file: "src/features/palette/sections.ts",
    from: "usePhase1 && !filePathBrowse\n\t\t\t\t? (phase1?.windows",
    to: "usePhase1\n\t\t\t\t? (phase1?.windows",
  },
  {
    id: "settings-reject-right-alignment",
    file: "src/state/app.tsx",
    from: '["left", "center", "right"],',
    to: '["left", "center"],',
  },
  {
    id: "recent-shows-four",
    file: "src/features/palette/sections.ts",
    from: "const RECENT_LIMIT = 5;",
    to: "const RECENT_LIMIT = 4;",
  },
  {
    id: "search-shows-seven-apps",
    file: "src/features/palette/sections.ts",
    from: "const SEARCH_APPS_LIMIT = 6;",
    to: "const SEARCH_APPS_LIMIT = 7;",
  },
  {
    id: "stale-files-shown",
    file: "src/features/palette/sections.ts",
    from: "fileResultQuery === searchQuery && dedupedFiles.length > 0",
    to: "dedupedFiles.length > 0",
  },
  {
    id: "settings-drop-wide-width",
    file: "src/state/app.tsx",
    from: "width: pick(src.width, [560, 640, 720], DEFAULT_SETTINGS.width),",
    to: "width: pick(src.width, [560, 640], DEFAULT_SETTINGS.width),",
  },
  {
    id: "history-cap-thirty",
    file: "src/state/app.tsx",
    from: "const HISTORY_CAP = 20;",
    to: "const HISTORY_CAP = 30;",
  },
  {
    id: "math-add-becomes-sub",
    file: "src/lib/math.ts",
    from: 'v = t.v === "+" ? v + rhs : v - rhs;',
    to: 'v = t.v === "+" ? v - rhs : v - rhs;',
  },
  {
    id: "fuzzy-empty-scores",
    file: "src/lib/search.ts",
    from: "if (q.length === 0) return null;",
    to: "if (q.length === 0) return 1;",
  },
  {
    id: "elevate-always-false",
    file: "src/lib/types.ts",
    from: "boundary > 0 &&\n\t\tELEVATABLE_EXTENSIONS.has(",
    to: "false &&\n\t\tELEVATABLE_EXTENSIONS.has(",
  },
  {
    id: "launcher-open-skips-preparing",
    file: "src/lib/launcherMotion.ts",
    from: ': "preparing";',
    to: ': "visible";',
  },
  {
    id: "phase1-always-matches",
    file: "src/lib/query.ts",
    from: "return phase1.query.trim().toLowerCase() === query.trim().toLowerCase();",
    to: "return true;",
  },
];

// A fresh copy of the whole UI source per mutation. node_modules is linked,
// not copied, so bare imports (react, lucide-react) resolve in the copy.
function copyTree(dest) {
  fs.rmSync(path.join(dest, "src"), { recursive: true, force: true });
  fs.cpSync(path.join(prism, "src"), path.join(dest, "src"), { recursive: true });
  const modules = path.join(dest, "node_modules");
  if (!fs.existsSync(modules)) fs.symlinkSync(path.join(prism, "node_modules"), modules, "junction");
}

function runJudge(root) {
  return new Promise((resolve) => {
    const child = spawn("bun", [judge], {
      env: { ...process.env, PRISM_ROOT: root },
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
    });
    child.on("close", (code) => resolve({ code, stdout }));
  });
}

const root = fs.mkdtempSync(path.join(os.tmpdir(), "prism-judge-"));
let caught = 0;
for (const mutation of mutations) {
  copyTree(root);
  const target = path.join(root, mutation.file);
  const source = fs.readFileSync(target, "utf8");
  if (!source.includes(mutation.from)) {
    process.stdout.write(`MISS\t${mutation.id}\tpattern not in copy\n`);
    continue;
  }
  fs.writeFileSync(target, source.replace(mutation.from, mutation.to));
  const outcome = await runJudge(root);
  const fails = outcome.stdout
    .split(/\r?\n/)
    .filter((line) => line.startsWith("FAIL\t"))
    .map((line) => line.split("\t")[1]);
  if (outcome.code !== 0 && fails.length > 0) {
    caught += 1;
    process.stdout.write(`CAUGHT\t${mutation.id}\t${fails[0]}\t${fails.length} failing\n`);
  } else {
    process.stdout.write(`MISSED\t${mutation.id}\texit ${outcome.code}\n${outcome.stdout}\n`);
  }
}
// Remove the junction on its own first, so the recursive delete can never
// walk into Prism's real node_modules.
fs.rmdirSync(path.join(root, "node_modules"));
fs.rmSync(root, { recursive: true, force: true });
process.stdout.write(`caught: ${caught}/${mutations.length}\n`);
process.exit(caught === mutations.length ? 0 : 1);
