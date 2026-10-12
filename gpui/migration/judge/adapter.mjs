// Original-side binding for the parity judge.
// stdin: one JSON call. stdout: one JSON result. No expected values live here.
//
// Output contract (every adapter, including the port's; call.mjs enforces the
// number rule):
// - Numbers that JSON cannot carry are objects: NaN -> {"$num":"NaN"},
//   Infinity -> {"$num":"Infinity"}, -Infinity -> {"$num":"-Infinity"},
//   -0 -> {"$num":"-0"}. A raw -0 or a non-finite literal is a contract error.
// - A function that returns "no value" (TS null, Rust None) writes `null`.
// - An optional object field that is absent is omitted, never `null`.
// - A row action is the list of backend calls it makes: [{cmd, args}], named by
//   the original Tauri command (or "clipboard.writeText"), plus `threw` if it
//   failed.
//
// Input markers (JSON cannot carry these either):
// - math.formatNumber input "inf" means Infinity.
// - types.pathFlags path null means "no path" (undefined in TS).
// - query.phase1Matches nullPhase1 true means "no Phase 1 payload".
// - settings.sanitize raw {"undefined": true} means "nothing persisted".
// - palette.sections replies[cmd] {"$reject": message} makes that backend
//   call fail with that message.
import path from "node:path";
import { pathToFileURL } from "node:url";

const root = process.env.PRISM_ROOT;
if (!root) {
  process.stderr.write("PRISM_ROOT is required\n");
  process.exit(2);
}

// Records every backend call instead of reaching Tauri. Registered before any
// Prism module loads, so src/lib/bridge.ts binds to these stubs.
globalThis.__prismCalls = [];
globalThis.__prismReplies = {};
// bridge.ts short-circuits every command unless it sees the Tauri marker
// (`inTauri`, src/lib/bridge.ts:64), so present it.
globalThis.window = { __TAURI_INTERNALS__: {} };
// Matched by resolved file path: Bun runtime plugins do not apply onResolve to
// bare package specifiers, but onLoad sees every file it loads.
Bun.plugin({
  name: "prism-backend-recorder",
  setup(build) {
    build.onLoad({ filter: /[\\/]@tauri-apps[\\/]api[\\/]core\.js$/ }, () => ({
      loader: "js",
      contents: `
        export function invoke(cmd, args) {
          globalThis.__prismCalls.push({ cmd, args: args ?? {} });
          const reply = globalThis.__prismReplies[cmd];
          if (reply && typeof reply === "object" && "$reject" in reply) {
            return Promise.reject(new Error(reply.$reject));
          }
          return Promise.resolve(reply);
        }
        export function convertFileSrc(path) { return path; }
        export function transformCallback() { return 0; }
        export class Channel { constructor() { this.onmessage = () => {}; } }
        export class Resource { constructor(rid) { this.rid = rid; } }
        export class PluginListener {}
        export async function addPluginListener() { return new PluginListener(); }
        export function isTauri() { return false; }
        export async function checkPermissions() { return {}; }
        export async function requestPermissions() { return {}; }
        export const SERIALIZE_TO_IPC_FN = "__TAURI_TO_IPC_KEY__";
      `,
    }));
    build.onLoad({ filter: /[\\/]plugin-clipboard-manager[\\/]dist-js[\\/]index\.js$/ }, () => ({
      loader: "js",
      contents: `
        export function writeText(text) {
          globalThis.__prismCalls.push({ cmd: "clipboard.writeText", args: { text } });
          return Promise.resolve();
        }
      `,
    }));
  },
});

function load(rel) {
  return import(pathToFileURL(path.join(root, rel)).href);
}

function readStdin() {
  return new Promise((resolve, reject) => {
    const chunks = [];
    process.stdin.on("data", (chunk) => chunks.push(chunk));
    process.stdin.on("end", () => resolve(Buffer.concat(chunks).toString("utf8")));
    process.stdin.on("error", reject);
  });
}

function iconState(raw) {
  return {
    appsLoaded: raw.appsLoaded,
    icons: raw.icons ?? {},
    settled: new Set(raw.settled ?? []),
    inFlight: new Set(raw.inFlight ?? []),
    attempts: new Map(raw.attempts ?? []),
  };
}

function encodeNumbers(_key, value) {
  if (typeof value !== "number") return value;
  if (Number.isNaN(value)) return { $num: "NaN" };
  if (value === Number.POSITIVE_INFINITY) return { $num: "Infinity" };
  if (value === Number.NEGATIVE_INFINITY) return { $num: "-Infinity" };
  if (Object.is(value, -0)) return { $num: "-0" };
  return value;
}

const stdin = await readStdin();
const call = JSON.parse(stdin);
let result;

switch (call.op) {
  case "math.tryEvaluate": {
    const { tryEvaluate } = await load("src/lib/math.ts");
    const evaluated = tryEvaluate(call.input);
    result = evaluated === null ? { returned: null } : { returned: "value", value: evaluated.value };
    break;
  }
  case "math.formatNumber": {
    const { formatNumber } = await load("src/lib/math.ts");
    const input = call.input === "inf" ? 1 / 0 : call.input;
    result = { text: formatNumber(input) };
    break;
  }
  case "math.isMathLike": {
    const { isMathLike } = await load("src/lib/math.ts");
    result = { value: isMathLike(call.input) };
    break;
  }
  case "search.fuzzyScore": {
    const { fuzzyScore } = await load("src/lib/search.ts");
    result = {
      scores: call.pairs.map(([query, target]) => fuzzyScore(query, target)),
    };
    break;
  }
  case "search.fuzzy": {
    const { fuzzy } = await load("src/lib/search.ts");
    const hits = fuzzy(call.pool, call.query, { limit: call.limit });
    result = { ids: hits.map((hit) => hit.item.id) };
    break;
  }
  case "search.fuzzyApps": {
    const { fuzzyApps } = await load("src/lib/search.ts");
    const hits = fuzzyApps(call.apps, call.query, call.limit);
    result = { names: hits.map((hit) => hit.name), appIds: hits.map((hit) => hit.appId) };
    break;
  }
  case "search.dedupeApps": {
    const { dedupeApps } = await load("src/lib/search.ts");
    result = { appIds: dedupeApps(call.apps).map((app) => app.appId) };
    break;
  }
  case "types.pathFlags": {
    const { isElevatablePath, isPicturePath, isTaskbarPinablePath } = await load("src/lib/types.ts");
    const pathValue = call.path === null ? undefined : call.path;
    result = {
      elevatable: isElevatablePath(pathValue),
      pinable: isTaskbarPinablePath(pathValue),
      picture: isPicturePath(pathValue),
    };
    break;
  }
  case "types.stepViewZoom": {
    const { stepViewZoom } = await load("src/lib/types.ts");
    result = { value: stepViewZoom(call.current, call.direction) };
    break;
  }
  case "types.reorderPinned": {
    const { reorderPinnedApps } = await load("src/lib/types.ts");
    const input = [...call.ids];
    result = { result: reorderPinnedApps(input, call.source, call.target), input };
    break;
  }
  case "types.reorderQuick": {
    const { reorderQuickAccess } = await load("src/lib/types.ts");
    const input = [...call.ids];
    result = { result: reorderQuickAccess(input, call.source, call.target), input };
    break;
  }
  case "types.reorderSections": {
    const { reorderSections } = await load("src/lib/types.ts");
    const input = [...call.ids];
    result = { result: reorderSections(input, call.source, call.target), input };
    break;
  }
  case "types.reorderGroups": {
    const { reorderAppGroups } = await load("src/lib/types.ts");
    const input = call.groups.map((group) => ({ ...group, appIds: [...group.appIds] }));
    const reordered = reorderAppGroups(input, call.source, call.target);
    result = {
      ids: reordered.map((group) => group.id),
      inputIds: input.map((group) => group.id),
    };
    break;
  }
  case "query.phase1Matches": {
    const { EMPTY_PHASE1, phase1MatchesQuery } = await load("src/lib/query.ts");
    const phase1 = call.nullPhase1 ? null : { ...EMPTY_PHASE1, query: call.phase1Query };
    result = { value: phase1MatchesQuery(phase1, call.typed) };
    break;
  }
  case "launcher.reduce": {
    const { reduceLauncherPhase } = await load("src/lib/launcherMotion.ts");
    result = { phase: reduceLauncherPhase(call.phase, call.event) };
    break;
  }
  case "launcher.hide": {
    const { hidePaletteInvocation } = await load("src/lib/launcherMotion.ts");
    result = { invocation: hidePaletteInvocation(call.origin, call.phase) };
    break;
  }
  case "icons.select": {
    const { selectAppIconRequestIds } = await load("src/lib/iconLoading.ts");
    result = { ids: selectAppIconRequestIds(call.ids, iconState(call.state)) };
    break;
  }
  case "icons.delay": {
    const { appIconRetryDelay } = await load("src/lib/iconLoading.ts");
    result = { delay: appIconRetryDelay(call.attempt) };
    break;
  }
  case "update.shouldCheck": {
    const policy = await load("src/features/updater/update-policy.ts");
    const interval = call.force
      ? policy.MIN_FORCED_CHECK_INTERVAL_MS
      : policy.MIN_BACKGROUND_CHECK_INTERVAL_MS;
    const now = call.last + interval + call.offsetMs;
    result = { value: policy.shouldCheckForUpdate(call.last, now, call.force) };
    break;
  }
  case "update.shouldCheckAt": {
    // Absolute times: the scenario, not the implementation, fixes the interval.
    const { shouldCheckForUpdate } = await load("src/features/updater/update-policy.ts");
    result = { value: shouldCheckForUpdate(call.last, call.now, call.force) };
    break;
  }
  case "update.percent": {
    const { updatePercent } = await load("src/features/updater/update-progress.ts");
    result = { value: updatePercent(call.downloaded, call.total) };
    break;
  }
  case "clipboard.kind": {
    const { isClipboardKind } = await load("src/features/palette/sections.ts");
    result = { value: isClipboardKind(call.id) };
    break;
  }
  case "palette.sections": {
    const { buildSections, quickAccessPaletteItem } = await load("src/features/palette/sections.ts");
    const { dedupeApps } = await load("src/lib/search.ts");
    const { sanitizeSettings } = await load("src/state/app.tsx");
    globalThis.__prismReplies = call.sources.replies ?? {};
    const sources = paletteSources(call.sources, { dedupeApps, quickAccessPaletteItem, sanitizeSettings });
    result = await serializeSections(buildSections(sources));
    break;
  }
  case "settings.sanitize": {
    const { sanitizeSettings } = await load("src/state/app.tsx");
    result = { settings: sanitizeSettings(call.raw.undefined ? undefined : call.raw.value) };
    break;
  }
  case "history.sanitize": {
    const { sanitizeHistory } = await load("src/state/app.tsx");
    result = { history: sanitizeHistory(call.raw.value) };
    break;
  }
  default:
    process.stderr.write(`unknown op ${call.op}\n`);
    process.exit(2);
}

process.stdout.write(`${JSON.stringify(result, encodeNumbers)}\n`);

// Mirrors the provider (src/state/palette.tsx): settings are sanitized, apps
// de-duplicated, pins resolve in user order, Quick Access rows follow
// settings.quickAccess (550-566). File state comes from the last search
// response: queries under 2 characters clear it (383-389), a pending search
// clears the error (417-418), cached thumbnails are copied onto results
// (431-434), and filesError, filesBusy, and the volume-aware index flags are
// derived (449-452, 567-568, 590-592).
function paletteSources(raw, { dedupeApps, quickAccessPaletteItem, sanitizeSettings }) {
  const settings = sanitizeSettings(raw.settings);
  const apps = dedupeApps(raw.apps);
  const appsById = new Map(apps.map((entry) => [entry.appId, entry]));
  const entriesByKind = new Map(raw.quickAccess.map((entry) => [entry.kind, entry]));
  const thumbnails = new Map(raw.fileThumbnails);
  const normalized = raw.query.trim().toLowerCase();
  const short = normalized.length < 2;
  const filesSearching = !short && raw.filesSearching;
  const filePathBrowse = !short && raw.filePathBrowse;
  const fileResults = short ? [] : raw.fileResults;
  const filesError =
    short || filesSearching
      ? false
      : Boolean(raw.responseError) || (!filePathBrowse && !raw.fileIndexReady && !raw.fileIndexing);
  const volumes = raw.volumes ?? [];
  return {
    query: raw.query,
    apps,
    pinnedApps: settings.pinnedApps.flatMap((appId) => (appsById.has(appId) ? [appsById.get(appId)] : [])),
    quickItems: settings.quickAccess.flatMap((kind) =>
      entriesByKind.has(kind) ? [quickAccessPaletteItem(entriesByKind.get(kind))] : [],
    ),
    quickAccessCollapsed: settings.quickAccessCollapsed,
    appGroups: settings.appGroups,
    sectionOrder: settings.sectionOrder,
    pinnedAppIds: settings.pinnedApps,
    history: raw.history,
    existingHistoryPaths: new Set(raw.existingHistoryPaths),
    fileThumbnails: thumbnails,
    appIcons: raw.appIcons,
    fileResults: fileResults.map((entry) => {
      const thumbnail = thumbnails.get(entry.path);
      return thumbnail ? { ...entry, thumbnail } : entry;
    }),
    fileResultQuery: short ? normalized : raw.fileResultQuery,
    filePathBrowse,
    filesBusy: filesSearching || (!filePathBrowse && !raw.fileIndexReady && raw.fileIndexing),
    filesError,
    fileIndexing: raw.fileIndexing || volumes.some((volume) => volume.state === "indexing"),
    fileIndexReady: raw.fileIndexReady || volumes.some((volume) => volume.state === "ready"),
    phase1: raw.phase1,
    powerPlans: raw.powerPlans,
  };
}

// Runs one row action against the recorder: the backend calls it makes, in
// order, and whether it threw.
async function recordAction(action) {
  globalThis.__prismCalls = [];
  let threw;
  try {
    await action();
  } catch (error) {
    threw = String(error?.message ?? error);
  }
  const record = { calls: globalThis.__prismCalls };
  if (threw !== undefined) record.threw = threw;
  return record;
}

// Everything a row shows or offers. Actions become their recorded backend
// calls; icons are recorded by kind and name.
async function serializeItem(item) {
  const out = {};
  for (const key of Object.keys(item).sort()) {
    const value = item[key];
    if (value === undefined) continue;
    if (typeof value === "function") out[key] = await recordAction(value);
    else if (key === "icon") out.icon = serializeIcon(value);
    else out[key] = value;
  }
  return out;
}

function serializeIcon(icon) {
  if (icon.kind !== "tile") return icon;
  return { kind: "tile", name: icon.icon.displayName ?? icon.icon.name, tint: icon.tint };
}

async function serializeItems(items) {
  const out = [];
  for (const item of items) out.push(await serializeItem(item));
  return out;
}

async function serializeSection(section) {
  const out = {
    id: section.id,
    label: section.label,
    collapsible: section.collapsible ?? false,
    collapsed: section.collapsed ?? false,
    items: await serializeItems(section.items),
  };
  if (section.groups) {
    out.groups = [];
    for (const group of section.groups) out.groups.push({ ...group, items: await serializeItems(group.items) });
  }
  return out;
}

async function serializeSections({ sections, flatItems }) {
  const out = [];
  for (const section of sections) out.push(await serializeSection(section));
  return { sections: out, flatIds: flatItems.map((item) => item.id) };
}
