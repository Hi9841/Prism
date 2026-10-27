const fuzzyPool = [
  { id: "a", title: "Calculator" },
  { id: "b", title: "Calendar" },
  { id: "c", title: "Clock" },
  { id: "d", title: "Chrome" },
  { id: "e", title: "Terminal" },
];

const apps = [
  {
    name: "Fortnite",
    appId: "fortnite",
    normalizedName: "fortnite",
    keywords: ["fortniteclient-win64-shipping", "documents"],
  },
  { name: "Fortress", appId: "fortress", normalizedName: "fortress" },
  {
    name: "Visual Studio Code",
    appId: "vsc",
    normalizedName: "visualstudiocode",
    keywords: ["code.exe"],
  },
  { name: "Steam", appId: "steam", normalizedName: "steam" },
  { name: "Discord", appId: "discord", normalizedName: "discord" },
];

const wezAliases = [
  { name: "WezTerm", appId: "wezterm-shortcut", normalizedName: "wezterm", source: "startMenu" },
  { name: "WezTerm", appId: "org.wezfurlong.wezterm", normalizedName: "wezterm", source: "appsFolder" },
  { name: "wezterm", appId: "wezterm-cli", normalizedName: "wezterm", source: "programs" },
  {
    name: "Windows Performance Analyzer",
    appId: "wpa",
    normalizedName: "windowsperformanceanalyzer",
    source: "startMenu",
  },
];

function row(id, batch, source, call, expect) {
  return { id, batch, source, call, expect };
}

const mathValues = [
  ["2 + 3", 5],
  ["12 * 8", 96],
  ["2^10", 1024],
  ["7 % 3", 1],
  ["(2 + 3) * 4", 20],
  ["-2^2", -4],
  ["2+3*4", 14],
  ["2*-3", -6],
  ["10 / 4", 2.5],
  ["sqrt(16)", 4],
  ["abs(-5)", 5],
  ["round(2.5)", 3],
  ["floor(2.9)", 2],
  ["6 × 7", 42],
  ["8 ÷ 2", 4],
  ["√9", 3],
];

const mathNulls = ["hello world", "", "12*8; drop table", "2 +", "(2+3", "2 +* 3", "sqrt(16"];

const mathLikeTrue = ["42", "sqrt(144)", "pi * 2", "6 × 7", "√9"];
const mathLikeFalse = ["vlc3", "7zip", "d3d12"];

const launcherReduce = [
  ["hidden", "open", "preparing"],
  ["preparing", "open", "preparing"],
  ["closing", "open", "visible"],
  ["visible", "open", "visible"],
  ["preparing", "presented", "visible"],
  ["hidden", "presented", "hidden"],
  ["closing", "presented", "closing"],
  ["preparing", "present-failed", "hidden"],
  ["visible", "present-failed", "hidden"],
  ["closing", "present-failed", "closing"],
  ["preparing", "native-close", "hidden"],
  ["visible", "native-close", "closing"],
  ["closing", "native-close", "closing"],
  ["hidden", "native-close", "hidden"],
  ["preparing", "web-close", "hidden"],
  ["visible", "web-close", "closing"],
  ["closing", "web-close", "closing"],
  ["hidden", "web-close", "hidden"],
  ["closing", "exit-finished", "hidden"],
  ["hidden", "exit-finished", "hidden"],
  ["preparing", "exit-finished", "preparing"],
  ["visible", "exit-finished", "visible"],
];

const launcherHide = [
  ["native", "closing", "none"],
  ["native", "hidden", "none"],
  ["web", "closing", "deferred"],
  ["web", "hidden", "instant"],
  ["web", "visible", "none"],
];

const pinableTrue = [
  "C:\\Apps\\Tool.exe",
  "C:\\Users\\You\\AppData\\Local\\lnk.lnk",
  "C:\\Windows\\System32\\dsa.msc",
  "C:\\Scripts\\job.CMD",
];
const pinableFalse = [
  "C:\\Users\\You\\Documents\\report.pdf",
  "C:\\Apps\\no-extension",
  "https://example.com",
  "shell:AppsFolder\\12345!App",
  null,
  "",
];
const pictureTrue = [
  "C:\\Photos\\vacation.png",
  "C:\\Photos\\portrait.JPG",
  "C:\\Pictures\\image.jpeg",
  "C:\\Pictures\\banner.webp",
  "C:\\Icons\\app.ico",
  "C:\\Vector\\logo.svg",
  "C:\\Raw\\sample.avif",
  "C:\\Photos\\shot.heic",
  "C:\\Graphics\\anim.gif",
];
const pictureFalse = [
  "C:\\Docs\\report.pdf",
  "C:\\Videos\\clip.mp4",
  "C:\\Music\\song.mp3",
  "C:\\Apps\\program.exe",
  "https://example.com/photo.png",
  "",
  null,
];
const elevatableTrue = [
  "C:\\Apps\\Tool.exe",
  "C:\\Tools\\tool.exe",
  "C:\\Tools\\tool.COM",
  "C:\\Tools\\task.bat",
  "C:\\Tools\\task.cmd",
  "C:\\Tools\\task.ps1",
  "C:\\Tools\\task.vbs",
  "C:\\Tools\\task.js",
  "C:\\Tools\\task.wsf",
];
const elevatableFalse = [
  "C:\\Apps\\Tool.exe.txt",
  "C:\\Windows\\System32\\dsa.msc",
  null,
  "",
  "C:\\Tools",
  "tool.lnk",
  "notes.txt",
  "https://example.com/tool.exe/",
];

export const scenarios = [
  ...mathValues.map(([input, value]) =>
    row(
      `math.eval.${input}`,
      "a",
      "src/lib/math.test.ts",
      { op: "math.tryEvaluate", input },
      { kind: "eq", value: { returned: "value", value } },
    ),
  ),
  ...mathNulls.map((input) =>
    row(
      `math.reject.${JSON.stringify(input)}`,
      "a",
      "src/lib/math.test.ts",
      { op: "math.tryEvaluate", input },
      { kind: "eq", value: { returned: null } },
    ),
  ),
  row(
    "math.eval.pi*2",
    "a",
    "src/lib/math.test.ts:22",
    { op: "math.tryEvaluate", input: "pi * 2" },
    { kind: "closeTo", token: "PI*2", digits: 2 },
  ),
  row(
    "math.eval.pi",
    "a",
    "src/lib/math.test.ts:33",
    { op: "math.tryEvaluate", input: "π" },
    { kind: "closeTo", token: "PI", digits: 2 },
  ),
  row("math.format.42", "a", "src/lib/math.test.ts:49", { op: "math.formatNumber", input: 42 }, {
    kind: "eq",
    value: { text: "42" },
  }),
  row("math.format.2.5", "a", "src/lib/math.test.ts:50", { op: "math.formatNumber", input: 2.5 }, {
    kind: "eq",
    value: { text: "2.5" },
  }),
  row(
    "math.format.third",
    "a",
    "src/lib/math.test.ts:51",
    { op: "math.formatNumber", input: 1 / 3 },
    { kind: "eq", value: { text: "0.333333" } },
  ),
  row("math.format.inf", "a", "src/lib/math.test.ts:52", { op: "math.formatNumber", input: "inf" }, {
    kind: "eq",
    value: { text: "Undefined" },
  }),
  ...mathLikeTrue.map((input) =>
    row(
      `math.like.yes.${input}`,
      "a",
      "src/lib/math.test.ts:57-62",
      { op: "math.isMathLike", input },
      { kind: "eq", value: { value: true } },
    ),
  ),
  ...mathLikeFalse.map((input) =>
    row(
      `math.like.no.${input}`,
      "a",
      "src/lib/math.test.ts:65-68",
      { op: "math.isMathLike", input },
      { kind: "eq", value: { value: false } },
    ),
  ),
  ...launcherReduce.map(([phase, event, next]) =>
    row(
      `launcher.reduce.${phase}.${event}`,
      "a",
      "src/lib/launcherMotion.test.ts",
      { op: "launcher.reduce", phase, event },
      { kind: "eq", value: { phase: next } },
    ),
  ),
  ...launcherHide.map(([origin, phase, invocation]) =>
    row(
      `launcher.hide.${origin}.${phase}`,
      "a",
      "src/lib/launcherMotion.test.ts:46-56",
      { op: "launcher.hide", origin, phase },
      { kind: "eq", value: { invocation } },
    ),
  ),
  row(
    "query.phase1.case",
    "a",
    "src/lib/query.test.ts:6",
    { op: "query.phase1Matches", phase1Query: "Night Light", typed: "night light", nullPhase1: false },
    { kind: "eq", value: { value: true } },
  ),
  row(
    "query.phase1.stale",
    "a",
    "src/lib/query.test.ts:7",
    { op: "query.phase1Matches", phase1Query: "old", typed: "night light", nullPhase1: false },
    { kind: "eq", value: { value: false } },
  ),
  row(
    "query.phase1.null",
    "a",
    "src/lib/query.test.ts:8",
    { op: "query.phase1Matches", phase1Query: "", typed: "night light", nullPhase1: true },
    { kind: "eq", value: { value: false } },
  ),
  row(
    "update.background.before",
    "a",
    "src/features/updater/update-policy.test.ts:10",
    { op: "update.shouldCheck", last: 10000, offsetMs: -1, force: false },
    { kind: "eq", value: { value: false } },
  ),
  row(
    "update.background.at",
    "a",
    "src/features/updater/update-policy.test.ts:11",
    { op: "update.shouldCheck", last: 10000, offsetMs: 0, force: false },
    { kind: "eq", value: { value: true } },
  ),
  row(
    "update.forced.before",
    "a",
    "src/features/updater/update-policy.test.ts:15",
    { op: "update.shouldCheck", last: 10000, offsetMs: -1, force: true },
    { kind: "eq", value: { value: false } },
  ),
  row(
    "update.forced.at",
    "a",
    "src/features/updater/update-policy.test.ts:16",
    { op: "update.shouldCheck", last: 10000, offsetMs: 0, force: true },
    { kind: "eq", value: { value: true } },
  ),
  row(
    "update.percent.half",
    "a",
    "src/features/updater/update-progress.test.ts:6",
    { op: "update.percent", downloaded: 512, total: 1024 },
    { kind: "eq", value: { value: 50 } },
  ),
  row(
    "update.percent.third",
    "a",
    "src/features/updater/update-progress.test.ts:7",
    { op: "update.percent", downloaded: 1, total: 3 },
    { kind: "eq", value: { value: 33 } },
  ),
  row(
    "update.percent.negative",
    "a",
    "src/features/updater/update-progress.test.ts:11",
    { op: "update.percent", downloaded: -1, total: 100 },
    { kind: "eq", value: { value: 0 } },
  ),
  row(
    "update.percent.over",
    "a",
    "src/features/updater/update-progress.test.ts:12",
    { op: "update.percent", downloaded: 150, total: 100 },
    { kind: "eq", value: { value: 100 } },
  ),
  row(
    "update.percent.missing-total",
    "a",
    "src/features/updater/update-progress.test.ts:13",
    { op: "update.percent", downloaded: 50 },
    { kind: "eq", value: { value: null } },
  ),
  row(
    "update.percent.zero-total",
    "a",
    "src/features/updater/update-progress.test.ts:14",
    { op: "update.percent", downloaded: 50, total: 0 },
    { kind: "eq", value: { value: null } },
  ),
  row(
    "search.score.prefix",
    "b",
    "src/lib/search.test.ts:6-11",
    { op: "search.fuzzyScore", pairs: [["cal", "Calendar"], ["cal", "Coalesce"]] },
    { kind: "scoreOrder", nonNull: true },
  ),
  row(
    "search.score.subsequence",
    "b",
    "src/lib/search.test.ts:14-17",
    {
      op: "search.fuzzyScore",
      pairs: [
        ["clndr", "Calendar"],
        ["vsc", "Visual Studio Code"],
        ["gogl", "Google"],
      ],
    },
    { kind: "allNonNull" },
  ),
  row(
    "search.score.word-start",
    "b",
    "src/lib/search.test.ts:20-25",
    { op: "search.fuzzyScore", pairs: [["st", "Studio"], ["st", "Fast"]] },
    { kind: "scoreOrder", nonNull: true },
  ),
  row(
    "search.score.reject",
    "b",
    "src/lib/search.test.ts:28-32",
    { op: "search.fuzzyScore", pairs: [["xyz", "abc"], ["", "abc"], ["abcef", "abc"]] },
    { kind: "eq", value: { scores: [null, null, null] } },
  ),
  row(
    "search.fuzzy.prefix",
    "b",
    "src/lib/search.test.ts:44-48",
    { op: "search.fuzzy", pool: fuzzyPool, query: "cal", limit: 5 },
    { kind: "prefix", field: "ids", value: ["b", "a"] },
  ),
  row(
    "search.fuzzy.subsequence",
    "b",
    "src/lib/search.test.ts:50-53",
    { op: "search.fuzzy", pool: fuzzyPool, query: "clndr", limit: 5 },
    { kind: "prefix", field: "ids", value: ["b"] },
  ),
  row(
    "search.fuzzy.empty",
    "b",
    "src/lib/search.test.ts:55-57",
    { op: "search.fuzzy", pool: fuzzyPool, query: "", limit: 5 },
    { kind: "eq", value: { ids: [] } },
  ),
  row(
    "search.fuzzy.limit",
    "b",
    "src/lib/search.test.ts:59-61",
    { op: "search.fuzzy", pool: fuzzyPool, query: "c", limit: 2 },
    { kind: "length", field: "ids", value: 2 },
  ),
  row(
    "search.fuzzy.keyword",
    "b",
    "src/lib/search.test.ts:63-74",
    {
      op: "search.fuzzy",
      pool: [
        { id: "x", title: "Prism", keywords: ["shortcut", "palette"] },
        { id: "y", title: "Something Else", keywords: [] },
      ],
      query: "shortcut",
      limit: 2,
    },
    { kind: "prefix", field: "ids", value: ["x"] },
  ),
  row(
    "search.apps.prefix",
    "b",
    "src/lib/search.test.ts:91-95",
    { op: "search.fuzzyApps", apps, query: "fort", limit: 8 },
    { kind: "prefix", field: "names", value: ["Fortnite", "Fortress"] },
  ),
  row(
    "search.apps.case",
    "b",
    "src/lib/search.test.ts:97-99",
    { op: "search.fuzzyApps", apps, query: "FORTNITE", limit: 8 },
    { kind: "prefix", field: "names", value: ["Fortnite"] },
  ),
  row(
    "search.apps.keyword",
    "b",
    "src/lib/search.test.ts:101-104",
    { op: "search.fuzzyApps", apps, query: "shipping", limit: 8 },
    { kind: "contains", field: "names", value: "Fortnite" },
  ),
  row(
    "search.apps.words",
    "b",
    "src/lib/search.test.ts:106-109",
    { op: "search.fuzzyApps", apps, query: "studio code", limit: 8 },
    { kind: "prefix", field: "names", value: ["Visual Studio Code"] },
  ),
  row(
    "search.apps.normalized",
    "b",
    "src/lib/search.test.ts:111-114",
    { op: "search.fuzzyApps", apps, query: "visualstudiocode", limit: 8 },
    { kind: "prefix", field: "names", value: ["Visual Studio Code"] },
  ),
  row(
    "search.apps.prefix-beats-fuzzy",
    "b",
    "src/lib/search.test.ts:116-119",
    { op: "search.fuzzyApps", apps, query: "ste", limit: 8 },
    { kind: "prefix", field: "names", value: ["Steam"] },
  ),
  row(
    "search.apps.empty",
    "b",
    "src/lib/search.test.ts:121-123",
    { op: "search.fuzzyApps", apps, query: "", limit: 8 },
    { kind: "eq", value: { names: [], appIds: [] } },
  ),
  row(
    "search.apps.limit",
    "b",
    "src/lib/search.test.ts:125-127",
    { op: "search.fuzzyApps", apps, query: "e", limit: 2 },
    { kind: "length", field: "names", value: 2 },
  ),
  row(
    "search.apps.alias",
    "b",
    "src/lib/search.test.ts:157-160",
    { op: "search.fuzzyApps", apps: wezAliases, query: "wez", limit: 8 },
    [
      { kind: "length", field: "appIds", value: 1 },
      { kind: "prefix", field: "appIds", value: ["wezterm-shortcut"] },
    ],
  ),
  row(
    "search.dedupe.source",
    "b",
    "src/lib/search.test.ts:162-168",
    {
      op: "search.dedupeApps",
      apps: [
        { name: "Prism", appId: "aumid", normalizedName: "prism", source: "appsFolder" },
        { name: "Prism", appId: "shortcut", normalizedName: "prism", source: "startMenu" },
      ],
    },
    { kind: "eq", value: { appIds: ["shortcut"] } },
  ),
  ...pinableTrue.map((path) =>
    row(
      `types.pinable.yes.${path}`,
      "b",
      "src/lib/types.test.ts:5-9",
      { op: "types.pathFlags", path },
      { kind: "flag", field: "pinable", value: true },
    ),
  ),
  ...pinableFalse.map((path) =>
    row(
      `types.pinable.no.${String(path)}`,
      "b",
      "src/lib/types.test.ts:12-18",
      { op: "types.pathFlags", path },
      { kind: "flag", field: "pinable", value: false },
    ),
  ),
  ...pictureTrue.map((path) =>
    row(
      `types.picture.yes.${path}`,
      "b",
      "src/lib/types.test.ts:29-38",
      { op: "types.pathFlags", path },
      { kind: "flag", field: "picture", value: true },
    ),
  ),
  ...pictureFalse.map((path) =>
    row(
      `types.picture.no.${String(path)}`,
      "b",
      "src/lib/types.test.ts:41-48",
      { op: "types.pathFlags", path },
      { kind: "flag", field: "picture", value: false },
    ),
  ),
  ...elevatableTrue.map((path) =>
    row(
      `types.elevate.yes.${path}`,
      "b",
      "src/lib/types.test.ts:22 src/state/app.test.ts:187-199",
      { op: "types.pathFlags", path },
      { kind: "flag", field: "elevatable", value: true },
    ),
  ),
  ...elevatableFalse.map((path) =>
    row(
      `types.elevate.no.${String(path)}`,
      "b",
      "src/lib/types.test.ts:23-24 src/state/app.test.ts:202-213",
      { op: "types.pathFlags", path },
      { kind: "flag", field: "elevatable", value: false },
    ),
  ),
  row("types.zoom.up", "b", "src/state/app.test.ts:30", { op: "types.stepViewZoom", current: 100, direction: 1 }, {
    kind: "eq",
    value: { value: 110 },
  }),
  row("types.zoom.down", "b", "src/state/app.test.ts:31", { op: "types.stepViewZoom", current: 100, direction: -1 }, {
    kind: "eq",
    value: { value: 90 },
  }),
  row("types.zoom.max", "b", "src/state/app.test.ts:32", { op: "types.stepViewZoom", current: 150, direction: 1 }, {
    kind: "eq",
    value: { value: 150 },
  }),
  row("types.zoom.min", "b", "src/state/app.test.ts:33", { op: "types.stepViewZoom", current: 70, direction: -1 }, {
    kind: "eq",
    value: { value: 70 },
  }),
  row(
    "types.reorder.pinned.down",
    "b",
    "src/state/app.test.ts:151-154",
    { op: "types.reorderPinned", ids: ["app-a", "app-b", "app-c"], source: "app-a", target: "app-b" },
    { kind: "eq", value: { result: ["app-b", "app-a", "app-c"], input: ["app-a", "app-b", "app-c"] } },
  ),
  row(
    "types.reorder.pinned.up",
    "b",
    "src/state/app.test.ts:155-159",
    { op: "types.reorderPinned", ids: ["app-a", "app-b", "app-c"], source: "app-c", target: "app-a" },
    { kind: "eq", value: { result: ["app-c", "app-a", "app-b"], input: ["app-a", "app-b", "app-c"] } },
  ),
  row(
    "types.reorder.pinned.missing",
    "b",
    "src/state/app.test.ts:163-167",
    { op: "types.reorderPinned", ids: ["app-a", "app-b"], source: "missing", target: "app-b" },
    { kind: "eq", value: { result: ["app-a", "app-b"], input: ["app-a", "app-b"] } },
  ),
  row(
    "types.reorder.pinned.same",
    "b",
    "src/state/app.test.ts:165",
    { op: "types.reorderPinned", ids: ["app-a", "app-b"], source: "app-a", target: "app-a" },
    { kind: "eq", value: { result: ["app-a", "app-b"], input: ["app-a", "app-b"] } },
  ),
  row(
    "types.reorder.quick",
    "b",
    "src/state/app.test.ts:173-177",
    { op: "types.reorderQuick", ids: ["home", "desktop", "downloads"], source: "home", target: "downloads" },
    { kind: "eq", value: { result: ["desktop", "downloads", "home"], input: ["home", "desktop", "downloads"] } },
  ),
  row(
    "types.reorder.quick.same",
    "b",
    "src/state/app.test.ts:181-183",
    { op: "types.reorderQuick", ids: ["home", "desktop"], source: "home", target: "home" },
    { kind: "eq", value: { result: ["home", "desktop"], input: ["home", "desktop"] } },
  ),
  row(
    "types.reorder.sections",
    "b",
    "src/state/app.test.ts:97-100",
    { op: "types.reorderSections", ids: ["pinned", "recent", "quick", "apps"], source: "apps", target: "recent" },
    {
      kind: "eq",
      value: {
        result: ["pinned", "apps", "recent", "quick"],
        input: ["pinned", "recent", "quick", "apps"],
      },
    },
  ),
  row(
    "types.reorder.groups",
    "b",
    "src/state/app.test.ts:102-111",
    {
      op: "types.reorderGroups",
      groups: [
        { id: "creative", name: "Creative", appIds: [], collapsed: false },
        { id: "developer", name: "Developer", appIds: [], collapsed: false },
      ],
      source: "developer",
      target: "creative",
    },
    { kind: "eq", value: { ids: ["developer", "creative"], inputIds: ["creative", "developer"] } },
  ),
  row(
    "icons.wait.blocked",
    "b",
    "src/lib/iconLoading.test.ts:15",
    { op: "icons.select", ids: ["app-a"], state: { appsLoaded: false } },
    { kind: "eq", value: { ids: [] } },
  ),
  row(
    "icons.wait.ready",
    "b",
    "src/lib/iconLoading.test.ts:16",
    { op: "icons.select", ids: ["app-a"], state: { appsLoaded: true } },
    { kind: "eq", value: { ids: ["app-a"] } },
  ),
  row(
    "icons.skip",
    "b",
    "src/lib/iconLoading.test.ts:19-29",
    {
      op: "icons.select",
      ids: ["loaded", "iconless", "loading", "missing"],
      state: {
        appsLoaded: true,
        icons: { loaded: "data:image/png;base64,icon" },
        settled: ["iconless"],
        inFlight: ["loading"],
      },
    },
    { kind: "eq", value: { ids: ["missing"] } },
  ),
  row("icons.delay.1", "b", "src/lib/iconLoading.test.ts:33", { op: "icons.delay", attempt: 1 }, {
    kind: "eq",
    value: { delay: 200 },
  }),
  row("icons.delay.2", "b", "src/lib/iconLoading.test.ts:34", { op: "icons.delay", attempt: 2 }, {
    kind: "eq",
    value: { delay: 800 },
  }),
  row("icons.delay.3", "b", "src/lib/iconLoading.test.ts:35", { op: "icons.delay", attempt: 3 }, {
    kind: "eq",
    value: { delay: null },
  }),
  row(
    "icons.retry.continue",
    "b",
    "src/lib/iconLoading.test.ts:36-38",
    { op: "icons.select", ids: ["app-a"], state: { appsLoaded: true, attempts: [["app-a", 2]] } },
    { kind: "eq", value: { ids: ["app-a"] } },
  ),
  row(
    "icons.retry.stop",
    "b",
    "src/lib/iconLoading.test.ts:39",
    { op: "icons.select", ids: ["app-a"], state: { appsLoaded: true, attempts: [["app-a", 3]] } },
    { kind: "eq", value: { ids: [] } },
  ),

  // Absolute times pin the intervals themselves (update-policy.ts:1, :7):
  // 5 min in the background, 1 min when forced. The adapter does not derive
  // `now` from its own constants here.
  ...[
    ["update.at.background.before", false, 299_999, false],
    ["update.at.background.at", false, 300_000, true],
    ["update.at.forced.before", true, 59_999, false],
    ["update.at.forced.at", true, 60_000, true],
  ].map(([id, force, elapsed, value]) =>
    row(
      id,
      "b",
      "src/features/updater/update-policy.ts:1-7",
      { op: "update.shouldCheckAt", last: 1_700_000_000_000, now: 1_700_000_000_000 + elapsed, force },
      { kind: "eq", value: { value } },
    ),
  ),
  ...[
    ["calc::96", true],
    ["copy::hello", true],
    ["app::app-id-1", false],
    ["file::f::C:\\x.txt", false],
  ].map(([id, value]) =>
    row(`clipboard.kind.${id}`, "b", "src/features/palette/sections.test.ts:775-778", { op: "clipboard.kind", id }, {
      kind: "eq",
      value: { value },
    }),
  ),
  // Number-contract canaries: the original's real results for these inputs
  // are NaN, +/-Infinity, and -0 (probed 2026-10-09). A port adapter that
  // writes null or 0 instead fails here, so the {"$num"} contract is proven.
  ...[
    ["0/0", "NaN"],
    ["1/0", "Infinity"],
    ["-1/0", "-Infinity"],
    ["-0", "-0"],
    ["0*-1", "-0"],
  ].map(([input, num]) =>
    row(`math.canary.${input}`, "b", "src/lib/math.ts (tryEvaluate)", { op: "math.tryEvaluate", input }, {
      kind: "eq",
      value: { returned: "value", value: { $num: num } },
    }),
  ),
];
