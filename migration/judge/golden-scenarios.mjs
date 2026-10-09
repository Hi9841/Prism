// Real-world scenarios whose expected output is the original app's own output
// (the old code is the executable spec). `capture.mjs` records the goldens from
// the original; `judge.mjs` diffs any implementation against them.
//
// Inputs are plain JSON so a non-TypeScript adapter can answer the same calls.
// Palette inputs use the shapes the real provider holds: raw persisted settings
// (sanitized by the adapter, as the app does on load), the backend's Quick
// Access list, history, the last file search response, and Phase 1 payloads
// shaped like src-tauri/src/query/mod.rs, catalog.rs, power.rs and
// windows_tools.rs build them.

const HOME = "C:\\Users\\You";
const DATA = "data:image/png;base64,";

const quickAccess = [
  { name: "Home", path: HOME, kind: "home" },
  { name: "Desktop", path: `${HOME}\\Desktop`, kind: "desktop" },
  { name: "Downloads", path: `${HOME}\\Downloads`, kind: "downloads" },
  { name: "Documents", path: `${HOME}\\Documents`, kind: "documents" },
  { name: "Pictures", path: `${HOME}\\Pictures`, kind: "pictures" },
  { name: "Music", path: `${HOME}\\Music`, kind: "music" },
  { name: "Videos", path: `${HOME}\\Videos`, kind: "videos" },
];

function app(name, appId, extra = {}) {
  return {
    name,
    appId,
    normalizedName: name.toLowerCase().replace(/[^a-z0-9]/g, ""),
    source: "startMenu",
    ...extra,
  };
}

const CHROME_EXE = "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe";

const apps = [
  app("Google Chrome", "chrome", {
    path: CHROME_EXE,
    location: `${HOME}\\AppData\\Roaming\\Microsoft\\Windows\\Start Menu\\Programs\\Google Chrome.lnk`,
    keywords: ["chrome.exe", "browser"],
  }),
  app("Calculator", "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App", {
    aumid: "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App",
    source: "appsFolder",
  }),
  app("Visual Studio Code", "vscode", {
    path: `${HOME}\\AppData\\Local\\Programs\\Microsoft VS Code\\Code.exe`,
    keywords: ["code.exe"],
  }),
  // Same app found twice: dedupe keeps one by source priority.
  app("Visual Studio Code", "vscode-appsfolder", { source: "appsFolder" }),
  app("Steam", "steam", { path: "C:\\Program Files (x86)\\Steam\\steam.exe" }),
  app("Discord", "discord", { path: `${HOME}\\AppData\\Local\\Discord\\Update.exe`, args: "--processStart Discord.exe" }),
  app("Notepad", "notepad", { path: "C:\\Windows\\System32\\notepad.exe" }),
  app("Terminal", "Microsoft.WindowsTerminal_8wekyb3d8bbwe!App", { source: "appsFolder" }),
  app("Spotify", "spotify", { path: "spotify:" }),
  app("Build Tools", "build-tools", { path: "C:\\Tools\\build.bat" }),
  app("Company Portal", "portal", { path: "https://portal.example.com/" }),
  app("Intranet", "intranet", {
    path: "https://intranet.example.com/",
    location: `${HOME}\\Desktop\\Intranet.url`,
  }),
  // UNC path (local target) and a path with no drive or UNC prefix (none).
  app("Share Tool", "share-tool", { path: "\\\\server\\share\\tool.exe" }),
  app("Relative Tool", "relative-tool", { path: "relative-tool.exe" }),
  app("Calendar", "calendar", { source: "appsFolder" }),
  app("Clock", "clock", { source: "appsFolder" }),
  app("Paint", "paint", { path: "C:\\Windows\\System32\\mspaint.exe" }),
  app("Photos", "photos", { source: "appsFolder" }),
  app("Snipping Tool", "snipping", { source: "appsFolder" }),
  app("Task Manager", "taskmgr", { path: "C:\\Windows\\System32\\Taskmgr.exe" }),
  app("Fortnite", "fortnite", { keywords: ["fortniteclient-win64-shipping"] }),
  app("App 10", "app-10", { path: "C:\\Apps\\app10.exe" }),
  app("App 2", "app-2", { path: "C:\\Apps\\app2.exe" }),
  app("obsidian", "obsidian", { path: `${HOME}\\AppData\\Local\\Obsidian\\Obsidian.exe` }),
];

const appIcons = { chrome: `${DATA}Q0hST01F`, vscode: `${DATA}VlND` };

function file(name, path, isDirectory = false, thumbnail) {
  const boundary = path.lastIndexOf("\\");
  const entry = { name, path, parent: boundary > 0 ? path.slice(0, boundary) : path, isDirectory };
  if (thumbnail) entry.thumbnail = thumbnail;
  return entry;
}

const reportFiles = [
  file("report.pdf", `${HOME}\\Documents\\report.pdf`),
  file("report-final.docx", `${HOME}\\Documents\\report-final.docx`),
  file("Reports", `${HOME}\\Documents\\Reports`, true),
  file("report.png", `${HOME}\\Pictures\\report.png`, false, `${DATA}UkVQ`),
];

// Array order is the authority for Recent (sections.ts:166), not ts: Notepad
// has the newest ts but sits fourth.
const history = [
  { id: "app::chrome", title: "Google Chrome", ts: 1_700_000_500 },
  { id: `file::f::${HOME}\\Documents\\report.pdf`, title: "report.pdf", ts: 1_700_000_400 },
  { id: `file::f::${HOME}\\Gone\\deleted.txt`, title: "deleted.txt", ts: 1_700_000_300 },
  { id: "app::notepad", title: "Notepad", ts: 1_700_000_900 },
  { id: `file::d::${HOME}\\Projects`, title: "Projects", ts: 1_700_000_100 },
  { id: `file::f::${HOME}\\Pictures\\cat.jpg`, title: "cat.jpg", ts: 1_700_000_050 },
  { id: "app::steam", title: "Steam", ts: 1_700_000_010 },
  { id: `file::f::${HOME}\\Gone\\report-old.pdf`, title: "report-old.pdf", ts: 1_700_000_005 },
];

const existingHistoryPaths = [
  `${HOME}\\Documents\\report.pdf`,
  `${HOME}\\Projects`,
  `${HOME}\\Pictures\\cat.jpg`,
];

function hit(fields) {
  return { subtitle: "", score: 100, ...fields };
}

function phase1(query, fields = {}) {
  return { query, recents: [], windows: [], apps: [], actions: [], ...fields };
}

// Phase 1 hits as the backend builds them.
// query/mod.rs app_hit: subtitle "Application", appId, path, source.
function appHit(entry, score = 500) {
  const fields = {
    id: `app::${entry.appId}`,
    kind: "app",
    title: entry.name,
    subtitle: "Application",
    appId: entry.appId,
    source: entry.source,
    score,
  };
  if (entry.path) fields.path = entry.path;
  return hit(fields);
}

// query/mod.rs window_hit: no appId.
function windowHit(hwnd, title, process) {
  return hit({ id: `window::${hwnd}`, kind: "window", title, subtitle: `Open window · ${process}`, hwnd, iconKey: "window", score: 800 });
}

// query/mod.rs recent_hit: the history id, no appId or path.
function recentHit(entry, score = 900) {
  return hit({ id: entry.id, kind: "recent", title: entry.title, subtitle: "Recent", score });
}

// query/mod.rs tool_hit: windows_tools.rs subtitles are "Windows tool" or
// "Control Panel"; only tools carry an icon.
function toolHit(id, title, iconKey, icon, subtitle = "Windows tool") {
  const fields = { id: `action::${id}`, kind: "action", title, subtitle, actionId: id, iconKey, source: "windows-tool", score: 700 };
  if (icon) fields.icon = icon;
  return hit(fields);
}

// catalog.rs settings pages carry a ms-settings uri.
function settingsHit(id, title, uri, iconKey, score = 600) {
  return hit({ id: `action::${id}`, kind: "action", title, subtitle: "Windows Settings", actionId: id, uri, iconKey, score });
}

// catalog.rs commands: no uri; subtitles "Power" or "Volume".
function commandHit(id, title, iconKey, subtitle) {
  return hit({ id: `action::${id}`, kind: "action", title, subtitle, actionId: id, iconKey, score: 650 });
}

// power.rs: pickerIntent is always a boolean.
function plan(guid, name, active, score, pickerIntent) {
  return { guid, name, active, score, pickerIntent };
}

const BALANCED = "381b4222-f694-41f0-9685-ff5bb260df2e";
const SAVER = "a1841308-3541-4fab-bc81-f71556f20b4a";
const HIGH = "8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c";

const byId = Object.fromEntries(apps.map((entry) => [entry.appId, entry]));

// The persisted settings an existing user has; the adapter sanitizes them.
const baseSettings = { pinnedApps: ["vscode", "chrome", "removed-app"] };

// The last file search response plus provider state (src/state/palette.tsx).
function sources(fields = {}) {
  return {
    query: "",
    apps,
    settings: baseSettings,
    quickAccess,
    history,
    existingHistoryPaths,
    fileThumbnails: [
      [`${HOME}\\Pictures\\cat.jpg`, `${DATA}Q0FU`],
      [`${HOME}\\Pictures\\broken.png`, null],
    ],
    appIcons,
    fileResults: [],
    fileResultQuery: "",
    filePathBrowse: false,
    filesSearching: false,
    responseError: false,
    fileIndexing: false,
    fileIndexReady: true,
    volumes: [],
    phase1: null,
    powerPlans: [],
    replies: {},
    ...fields,
  };
}

// A search whose file results have arrived for this exact query.
function settled(query, fields = {}) {
  return sources({ query, fileResultQuery: query.trim().toLowerCase(), ...fields });
}

const kitchenSinkFiles = [
  file("setup.exe", `${HOME}\\Downloads\\setup.exe`),
  file("install.bat", `${HOME}\\Downloads\\install.bat`),
  file("holiday.mp4", `${HOME}\\Videos\\holiday.mp4`),
  file("song.mp3", `${HOME}\\Music\\song.mp3`),
  file("backup.zip", `${HOME}\\Documents\\backup.zip`),
  file("data.xyz", `${HOME}\\Documents\\data.xyz`),
  file("notes.txt", `${HOME}\\Documents\\notes.txt`),
  file("broken.png", `${HOME}\\Pictures\\broken.png`),
  // Upper-case extension, and ".ts" (both a video and a text extension).
  file("PHOTO.JPG", `${HOME}\\Pictures\\PHOTO.JPG`),
  file("clip.ts", `${HOME}\\Videos\\clip.ts`),
];

const devGroups = [
  { id: "group-dev", name: "Dev", appIds: ["vscode", "notepad", "build-tools", "taskmgr", "obsidian", "app-10", "app-2", "paint", "discord", "steam"], collapsed: false },
  { id: "group-tools", name: "Tools", appIds: ["notepad", "paint", "snipping"], collapsed: false },
  { id: "group-games", name: "Games", appIds: ["steam", "fortnite"], collapsed: true },
];

// Every icon key in phase1.ts ICONS, plus one unknown key (falls back to
// "settings"). Rendered as Settings rows so each icon and tint is visible.
const ICON_KEYS = [
  "nightlight", "display", "hdr", "bluetooth", "sound", "storage", "power", "update", "network",
  "mute", "unmute", "lock", "sleep", "shutdown", "restart", "window", "settings", "control-panel",
  "device", "performance", "power-plan", "tool", "mystery-key",
];

// Collation: case, digits, and accents. The goldens record the original's
// localeCompare with the capture host's default locale (en-US here).
const collationApps = [
  app("zeta", "zeta"),
  app("Alpha", "alpha"),
  app("beta", "beta"),
  app("App 10", "app-10b"),
  app("App 2", "app-2b"),
  app("Éclair", "eclair"),
  app("Ångström", "angstrom"),
  app("delta", "delta"),
  app("Delta", "delta-upper"),
  app("app 3", "app-3"),
];

const sectionCases = [
  // Idle dashboard.
  ["idle.default", sources()],
  ["idle.no-pins", sources({ settings: { pinnedApps: [] } })],
  ["idle.no-history", sources({ history: [] })],
  ["idle.quick-collapsed", sources({ settings: { ...baseSettings, quickAccessCollapsed: true } })],
  ["idle.custom-order", sources({ settings: { ...baseSettings, sectionOrder: ["apps", "quick", "recent", "pinned"] } })],
  ["idle.partial-order", sources({ settings: { ...baseSettings, sectionOrder: ["recent"] } })],
  [
    "idle.quick-access-order",
    sources({ settings: { ...baseSettings, quickAccess: ["videos", "music", "downloads"] }, quickAccess: quickAccess.filter((entry) => entry.kind !== "music") }),
  ],
  ["idle.collections", sources({ settings: { ...baseSettings, appGroups: devGroups } })],
  ["idle.sort-collation", sources({ apps: collationApps, settings: { pinnedApps: [] }, history: [] })],
  ["idle.empty-everything", sources({ apps: [], settings: { pinnedApps: [] }, quickAccess: [], history: [] })],

  // Math and copy.
  ["search.math", settled("2+3*4")],
  ["search.math-function", settled("sqrt(16) + 2^3")],
  ["search.copy-fallback", settled("zzqx")],
  ["search.copy-fallback-padded", settled("   zzqx   ")],

  // Phase 1 and the section order while searching.
  [
    "search.chrome",
    settled("chrome", {
      phase1: phase1("chrome", {
        recents: [recentHit(history[0])],
        windows: [windowHit(42, "Inbox - Google Chrome", "chrome")],
        apps: [appHit(byId.chrome)],
      }),
    }),
  ],
  [
    "search.recent-files",
    settled("report", {
      phase1: phase1("report", { recents: [recentHit(history[1]), recentHit(history[7], 800)] }),
      fileResults: reportFiles,
    }),
  ],
  // calc, recent, and windows together.
  [
    "search.calc-recent-windows",
    settled("1+1", {
      phase1: phase1("1+1", { recents: [recentHit(history[3])], windows: [windowHit(5, "1+1 - Notepad", "notepad")] }),
    }),
  ],
  // recent, windows, picker power plans, and Quick Access together.
  [
    "search.recent-windows-plans-quick",
    settled("do", {
      phase1: phase1("do", { recents: [recentHit(history[1])], windows: [windowHit(6, "Documents", "explorer")] }),
      powerPlans: [plan(BALANCED, "Balanced", true, 900, true), plan(SAVER, "Power saver", false, 700, true)],
    }),
  ],
  ["search.chrome-phase1-pending", sources({ query: "chrome", filesSearching: true })],
  [
    "search.stale-phase1",
    settled("chrome", { phase1: phase1("chr", { actions: [settingsHit("settings.display", "Display", "ms-settings:display", "display")] }) }),
  ],
  ["search.phase1-empty-apps", settled("steam", { phase1: phase1("steam") })],
  [
    "search.phase1-app-order",
    settled("t", {
      phase1: phase1("t", {
        apps: [
          appHit(byId.taskmgr, 900),
          appHit(byId.steam, 880),
          appHit(byId["Microsoft.WindowsTerminal_8wekyb3d8bbwe!App"], 870),
          appHit(byId.snipping, 860),
          appHit(byId.notepad, 850),
          appHit(byId["build-tools"], 840),
          appHit(byId.portal, 830),
          // Not in the local app list: becomes a fallback row.
          appHit(app("Tool Box", "unknown-tool", { path: "C:\\Tools\\toolbox.exe" }), 820),
        ],
      }),
    }),
  ],
  [
    "search.phase1-unknown-app",
    settled("toolbox", { phase1: phase1("toolbox", { apps: [appHit(app("Tool Box", "unknown-tool", { path: "C:\\Tools\\toolbox.exe" }), 820)] }) }),
  ],
  [
    "search.phase1-path-removes-file",
    settled("notepad", {
      phase1: phase1("notepad", { apps: [appHit(byId.notepad)] }),
      fileResults: [file("NOTEPAD.EXE", "C:\\WINDOWS\\system32\\NOTEPAD.EXE"), file("notepad-notes.txt", `${HOME}\\notepad-notes.txt`)],
    }),
  ],
  [
    "search.settings-intent",
    settled("my eyes hurt", {
      phase1: phase1("my eyes hurt", {
        actions: [hit({ id: "action::settings.nightlight", kind: "action", title: "Night Light", subtitle: "Windows Settings · from your wording", actionId: "settings.nightlight", uri: "ms-settings:nightlight", iconKey: "nightlight", score: 10_000 })],
      }),
    }),
  ],
  [
    "search.windows-and-actions",
    sources({
      query: "notes",
      filesSearching: true,
      phase1: phase1("notes", { windows: [windowHit(1, "Notes", "notepad")], actions: [commandHit("power.lock", "Lock", "lock", "Power")] }),
    }),
  ],
  [
    "search.tools",
    settled("device", {
      phase1: phase1("device", {
        actions: [
          toolHit("windows.device-manager", "Device Manager", "device", `${DATA}REVW`),
          toolHit("windows.control-panel", "Control Panel", "control-panel", undefined, "Control Panel"),
          toolHit("windows.computer-management", "Computer Management", "tool"),
        ],
      }),
    }),
  ],
  [
    "search.icon-keys",
    settled("icons", {
      phase1: phase1("icons", {
        actions: ICON_KEYS.map((key, index) => settingsHit(`settings.icon-${key}`, `Icon ${key}`, `ms-settings:icon-${key}`, key, 900 - index)),
      }),
    }),
  ],
  // Apps, tools, commands, settings and files in one result: their order.
  [
    "search.every-section",
    settled("se", {
      phase1: phase1("se", {
        windows: [windowHit(7, "Steam", "steam")],
        apps: [appHit(byId.steam), appHit(byId.snipping, 480)],
        actions: [
          settingsHit("settings.sound", "Sound", "ms-settings:sound", "sound"),
          toolHit("windows.services", "Services", "tool"),
          commandHit("power.sleep", "Sleep", "sleep", "Power"),
          commandHit("audio.mute", "Mute", "mute", "Volume"),
        ],
      }),
      fileResults: [file("settings.json", `${HOME}\\settings.json`), file("Screenshots", `${HOME}\\Pictures\\Screenshots`, true)],
      powerPlans: [plan(SAVER, "Power saver", false, 300, false)],
    }),
  ],
  ["search.power-plans", settled("power plan", { powerPlans: [plan(BALANCED, "Balanced", true, 900, true), plan(HIGH, "High performance", false, 800, true)] })],
  // Picker power plans next to Quick Access and Apps hits.
  ["search.power-plans-with-quick-and-apps", settled("p", { powerPlans: [plan(BALANCED, "Balanced", true, 900, true), plan(SAVER, "Power saver", false, 700, true)] })],
  [
    "search.plan-name-no-intent",
    settled("balanced", {
      apps: [...apps, app("Balanced Breakfast", "breakfast", { path: "C:\\Games\\breakfast.exe" })],
      powerPlans: [plan(BALANCED, "Balanced", true, 900, false)],
    }),
  ],

  // Quick Access and Apps.
  ["search.quick-access", settled("down")],
  ["search.quick-access-videos", settled("vid", { settings: { ...baseSettings, quickAccess: ["videos", "home"] } })],
  ["search.keyword-only", settled("browser")],
  ["search.multi-token", settled("visual code")],
  ["search.whitespace", settled("   chrome   ")],
  // More app matches than the search cap, so the cap itself is observable.
  ["search.broad-apps", settled("e")],
  ["search.broad-apps-two", settled("ca")],
  ["search.url-shortcut", settled("intranet")],
  ["search.unc-and-relative-paths", settled("tool")],
  ["search.collections", settled("o", { settings: { ...baseSettings, appGroups: devGroups } })],

  // Files.
  ["search.files", settled("report", { fileResults: reportFiles })],
  ["search.file-kinds", settled("file", { fileResults: kitchenSinkFiles, replies: { is_pinned_to_taskbar: true } })],
  ["search.file-pin-query-fails", settled("setup", { fileResults: kitchenSinkFiles.slice(0, 1), replies: { is_pinned_to_taskbar: { $reject: "access denied" } } })],
  // Real stale state: the new query is still searching (palette.tsx:417-418).
  ["search.files-stale", sources({ query: "report", fileResultQuery: "rep", fileResults: reportFiles, filesSearching: true })],
  ["search.files-searching", sources({ query: "report", filesSearching: true })],
  ["search.files-error", settled("zzqx", { responseError: true })],
  ["search.indexing-not-ready", settled("zzqx", { fileIndexing: true, fileIndexReady: false })],
  ["search.indexing-ready", settled("zzqx", { fileIndexing: true, fileIndexReady: true })],
  ["search.not-ready-not-indexing", settled("zzqx", { fileIndexing: false, fileIndexReady: false })],
  [
    "search.volume-ready-overrides",
    settled("zzqx", { fileIndexing: false, fileIndexReady: false, volumes: [{ drive: "D:", state: "ready", indexedCount: 10 }] }),
  ],
  [
    "search.volume-indexing-overrides",
    settled("zzqx", { fileIndexing: false, fileIndexReady: true, volumes: [{ drive: "E:", state: "indexing", indexedCount: 3, totalProgress: 0.4 }] }),
  ],
  ["search.short-query-hides-files", settled("r", { fileResults: reportFiles })],
  [
    "search.thumbnails",
    settled("cat", { fileResults: [file("cat.jpg", `${HOME}\\Pictures\\cat.jpg`), file("catalog.txt", `${HOME}\\catalog.txt`)] }),
  ],
  // Case-insensitive app/file dedupe (sections.ts:232, 236, 292).
  [
    "search.app-file-dedupe",
    settled("notepad", { fileResults: [file("NOTEPAD.EXE", "C:\\WINDOWS\\system32\\NOTEPAD.EXE"), file("notepad-notes.txt", `${HOME}\\notepad-notes.txt`)] }),
  ],

  // Path browsing hides recents, windows, settings, tools, commands and power
  // plans; Folder Contents stays above Apps; no copy fallback.
  [
    "search.path-browse",
    settled("C:\\Users\\You\\Doc", {
      filePathBrowse: true,
      fileResults: [file("Documents", `${HOME}\\Documents`, true), file("Docs.txt", `${HOME}\\Docs.txt`)],
    }),
  ],
  // The original test's case (sections.test.ts:648-663).
  [
    "search.path-browse-above-apps",
    settled("win", {
      filePathBrowse: true,
      fileResults: [file("system32", "C:\\Windows\\system32", true)],
      apps: [...apps, app("Windows Terminal", "wt", { source: "appsFolder" })],
    }),
  ],
  [
    "search.path-browse-everything-else",
    settled("C:\\Tools\\build", {
      filePathBrowse: true,
      fileResults: [file("build.bat", "C:\\Tools\\build.bat"), file("build-log.txt", "C:\\Tools\\build-log.txt")],
      phase1: phase1("C:\\Tools\\build", {
        recents: [recentHit(history[0])],
        windows: [windowHit(9, "build", "cmd")],
        actions: [settingsHit("settings.display", "Display", "ms-settings:display", "display"), commandHit("power.lock", "Lock", "lock", "Power"), toolHit("windows.services", "Services", "tool")],
      }),
      powerPlans: [plan(BALANCED, "Balanced", true, 900, true)],
    }),
  ],
  ["search.path-browse-empty", settled("C:\\Nowhere\\", { filePathBrowse: true, fileResults: [] })],
];

const SHORTCUTS = [
  "Win",
  "Ctrl+Alt+Space",
  "Ctrl+Alt+S",
  "Ctrl+Alt+Shift+S",
  "Ctrl+Alt+P",
  "Ctrl+Alt+Shift+P",
  "Ctrl+Alt+Enter",
  "Ctrl+Alt+Shift+Enter",
];
const ACCENTS = ["iris", "azure", "mint", "amber", "rose"];
const ZOOMS = [70, 80, 90, 100, 110, 120, 130, 140, 150];

const settingsCases = [
  ["settings.fresh", undefined],
  ["settings.null", null],
  ["settings.empty", {}],
  ["settings.string", "hello"],
  ["settings.number", 42],
  ["settings.array", [1, 2, 3]],
  [
    "settings.full-valid",
    {
      accent: "mint",
      width: 720,
      viewZoom: 110,
      shortcut: "Ctrl+Alt+Space",
      alwaysOnTop: false,
      taskbarAlignment: "left",
      taskbarScrollVolume: false,
      theme: "dark",
      quickAccess: ["downloads", "documents", "videos"],
      quickAccessCollapsed: true,
      pinnedApps: ["chrome", "vscode"],
      appGroups: [{ id: "group-dev", name: "Dev", appIds: ["vscode", "notepad"], collapsed: false }],
      sectionOrder: ["apps", "recent", "quick", "pinned"],
    },
  ],
  ["settings.align-right", { taskbarAlignment: "right", accent: "rose", width: 560, theme: "light" }],
  ...SHORTCUTS.map((shortcut) => [`settings.shortcut.${shortcut}`, { shortcut }]),
  ...ACCENTS.map((accent) => [`settings.accent.${accent}`, { accent }]),
  ...ZOOMS.map((viewZoom) => [`settings.zoom.${viewZoom}`, { viewZoom }]),
  ["settings.zoom-off-step", { viewZoom: 135 }],
  ["settings.quick-access-cap", { quickAccess: ["videos", "home", "desktop", "downloads", "documents", "pictures", "music"] }],
  [
    "settings.invalid-values",
    {
      accent: "neon",
      width: 999,
      viewZoom: 7,
      shortcut: "Ctrl+Nope",
      alwaysOnTop: "yes",
      taskbarAlignment: "diagonal",
      taskbarScrollVolume: 1,
      theme: "sepia",
      quickAccess: ["downloads", "downloads", "nowhere", 4],
      quickAccessCollapsed: "true",
      pinnedApps: ["chrome", "chrome", "", 7, null],
      appGroups: "not-a-list",
      sectionOrder: ["apps", "apps", "unknown"],
    },
  ],
  // An own "__proto__" key, as JSON.parse of a persisted file produces it.
  ["settings.unknown-keys", JSON.parse('{"accent":"rose","extra":1,"nested":{"a":1},"__proto__":{"polluted":true}}')],
  ["settings.too-many-pins", { pinnedApps: Array.from({ length: 80 }, (_, index) => `app-${index}`) }],
  ["settings.pin-id-limits", { pinnedApps: ["a".repeat(4096), "b".repeat(4097), "ok"] }],
  [
    "settings.groups-messy",
    {
      appGroups: [
        { id: "a", name: "  Work  ", appIds: ["x", "x", "y"], collapsed: true },
        { id: "a", name: "Duplicate id", appIds: ["z"], collapsed: false },
        { id: "", name: "No id", appIds: [] },
        { name: "Missing id", appIds: ["q"] },
        { id: "b", name: "", appIds: ["r"] },
        { id: "c", name: "Collapsed text", appIds: ["s"], collapsed: "yes" },
        ...Array.from({ length: 20 }, (_, index) => ({ id: `g${index}`, name: `Group ${index}`, appIds: [`app-${index}`], collapsed: false })),
      ],
    },
  ],
  // Exact boundaries after trimming (app.tsx:727-729): id <= 96, name <= 64.
  [
    "settings.group-limits",
    {
      appGroups: [
        { id: "i".repeat(96), name: "Id at limit", appIds: ["a"], collapsed: false },
        { id: "j".repeat(97), name: "Id over limit", appIds: ["a"], collapsed: false },
        { id: `  ${"k".repeat(96)}  `, name: "Padded id at limit", appIds: ["a"], collapsed: false },
        { id: "  padded  ", name: "Padded id", appIds: ["a"], collapsed: false },
        { id: "name-64", name: "n".repeat(64), appIds: ["a"], collapsed: false },
        { id: "name-65", name: "m".repeat(65), appIds: ["a"], collapsed: false },
        { id: "many-apps", name: "Many", appIds: Array.from({ length: 70 }, (_, index) => `app-${index}`), collapsed: false },
      ],
    },
  ],
  ["settings.empty-lists", { quickAccess: [], pinnedApps: [], appGroups: [], sectionOrder: [] }],
  ["settings.string-numbers", { viewZoom: "110", width: "720" }],
];

const historyCases = [
  ["history.null", null],
  ["history.valid", history],
  [
    "history.messy",
    [
      { id: "app::chrome", title: "Chrome", ts: 5 },
      { id: "app::chrome", title: "Chrome again", ts: 6 },
      { id: "", title: "No id", ts: 1 },
      { id: "app::x", title: 3, ts: 1 },
      { id: "app::y", title: "No ts" },
      { id: "app::z", title: "String ts", ts: "5" },
      { id: "app::empty-title", title: "", ts: 1 },
      { id: 42, title: "Numeric id", ts: 2 },
      { id: "app::extra", title: "Extra fields", ts: 3, pinned: true, path: "C:\\x.exe" },
      "garbage",
      null,
    ],
  ],
  ["history.too-long", Array.from({ length: 40 }, (_, index) => ({ id: `app::a${index}`, title: `A${index}`, ts: index }))],
  // Exact boundaries (app.tsx:797-800): id <= 4096, title <= 512.
  [
    "history.limits",
    [
      { id: `app::${"i".repeat(4091)}`, title: "Id at limit", ts: 1 },
      { id: `app::${"j".repeat(4092)}`, title: "Id over limit", ts: 2 },
      { id: "app::title-512", title: "t".repeat(512), ts: 3 },
      { id: "app::title-513", title: "u".repeat(513), ts: 4 },
    ],
  ],
];

export const goldenScenarios = [
  ...sectionCases.map(([id, input]) => ({ id: `sections.${id}`, call: { op: "palette.sections", sources: input } })),
  ...settingsCases.map(([id, raw]) => ({ id, call: { op: "settings.sanitize", raw: raw === undefined ? { undefined: true } : { value: raw } } })),
  ...historyCases.map(([id, raw]) => ({ id, call: { op: "history.sanitize", raw: { value: raw } } })),
];
