import { Home } from "lucide-react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { isPinnedToTaskbar, setTaskbarPinned } from "../../lib/bridge";
import type { AppEntry, FileEntry, PaletteItem, QuickAccessEntry } from "../../lib/types";
import { buildSections, isClipboardKind, type PaletteSources, quickAccessPaletteItem } from "./sections";

vi.mock("../../lib/bridge", () => ({
  isPinnedToTaskbar: vi.fn().mockResolvedValue(false),
  setTaskbarPinned: vi.fn().mockResolvedValue(undefined),
  startFileDrag: vi.fn().mockResolvedValue(true),
  openPath: vi.fn().mockResolvedValue(undefined),
  openPathLocation: vi.fn().mockResolvedValue(undefined),
  runPathAsAdmin: vi.fn().mockResolvedValue(undefined),
  showPathProperties: vi.fn().mockResolvedValue(undefined),
  launchApp: vi.fn().mockResolvedValue(undefined),
  launchAppAsAdmin: vi.fn().mockResolvedValue(undefined),
  copyText: vi.fn().mockResolvedValue(undefined),
}));

function app(name: string, overrides: Partial<AppEntry> = {}): AppEntry {
  return {
    name,
    appId: `app-id-${name}`,
    normalizedName: name.toLowerCase().replace(/[^a-z0-9]/g, ""),
    ...overrides,
  };
}

function file(name: string, path: string, isDirectory = false): FileEntry {
  const boundary = Math.max(path.lastIndexOf("\\"), path.lastIndexOf("/"));
  return { name, path, parent: boundary > 0 ? path.slice(0, boundary) : path, isDirectory };
}

function item(title: string, id: string, overrides: Partial<PaletteItem> = {}): PaletteItem {
  return {
    id,
    title,
    icon: { kind: "tile", icon: Home, tint: "iris" },
    run: () => {},
    historyTitle: title,
    ...overrides,
  };
}

const DOWNLOADS_PATH = "C:\\Users\\You\\Downloads";
const DOCUMENTS_PATH = "C:\\Users\\You\\Documents";

function quickItems(): PaletteItem[] {
  return [
    item("Downloads", `file::d::${DOWNLOADS_PATH}`, { subtitle: DOWNLOADS_PATH }),
    item("Documents", `file::d::${DOCUMENTS_PATH}`, { subtitle: DOCUMENTS_PATH }),
    item("Music", `file::d::C:\\Users\\You\\Music`, {
      subtitle: "C:\\Users\\You\\Music",
      keywords: ["audio"],
    }),
  ];
}

function sources(overrides: Partial<PaletteSources> = {}): PaletteSources {
  return {
    query: "",
    apps: [],
    pinnedApps: [],
    quickItems: [],
    quickAccessCollapsed: false,
    pinnedAppIds: [],
    history: [],
    existingHistoryPaths: new Set(),
    appIcons: {},
    fileResults: [],
    fileResultQuery: "",
    filePathBrowse: false,
    filesBusy: false,
    filesError: false,
    ...overrides,
  };
}

const ids = (sections: ReturnType<typeof buildSections>["sections"]) => sections.map((s) => s.id);

describe("buildSections - idle layout (empty query)", () => {
  it("shows Pinned, Recent, Quick Access, Apps in order with caps", () => {
    const names = [
      "Alpha",
      "Bravo",
      "Charlie",
      "Delta",
      "Echo",
      "Foxtrot",
      "Golf",
      "Hotel",
      "India",
      "Juliett",
    ];
    const tenApps = names.map((name) => app(name));
    const pinned = [tenApps[0], tenApps[1]];
    // Most-recent-first history: Bravo (pinned, skipped), then Charlie..Golf
    // (5 items, hits the Recent cap) plus one entry that never gets reached.
    const history = ["Bravo", "Charlie", "Delta", "Echo", "Foxtrot", "Golf"].map((name, i) => ({
      id: `app::app-id-${name}`,
      title: name,
      ts: 10 - i,
    }));
    const result = buildSections(
      sources({
        apps: tenApps,
        pinnedApps: pinned,
        pinnedAppIds: pinned.map((a) => a.appId),
        quickItems: quickItems(),
        history,
        existingHistoryPaths: new Set([DOWNLOADS_PATH]),
      }),
    );

    expect(ids(result.sections)).toEqual(["pinned", "recent", "quick", "apps"]);
    expect(result.sections[0].items.map((i) => i.title)).toEqual(["Alpha", "Bravo"]);
    expect(result.sections[1].items.map((i) => i.title)).toEqual([
      "Charlie",
      "Delta",
      "Echo",
      "Foxtrot",
      "Golf",
    ]);
    expect(result.sections[2].items.map((i) => i.title)).toEqual(["Downloads", "Documents", "Music"]);
    // Apps section: pinned excluded, capped at 8, alphabetical.
    expect(result.sections[3].items.map((i) => i.title)).toEqual([
      "Charlie",
      "Delta",
      "Echo",
      "Foxtrot",
      "Golf",
      "Hotel",
      "India",
      "Juliett",
    ]);
  });

  it("keeps pinned apps in user order, not alphabetical", () => {
    const result = buildSections(sources({ apps: [app("Alpha")], pinnedApps: [app("Zebra"), app("Alpha")] }));
    expect(result.sections[0].items.map((i) => i.title)).toEqual(["Zebra", "Alpha"]);
  });

  it("uses the persisted section order for the idle dashboard", () => {
    const result = buildSections(
      sources({
        apps: [app("Alpha")],
        pinnedApps: [app("Pinned")],
        quickItems: quickItems(),
        sectionOrder: ["apps", "quick", "pinned", "recent"],
      }),
    );
    expect(ids(result.sections)).toEqual(["apps", "quick", "pinned"]);
    expect(result.flatItems[0]?.title).toBe("Alpha");
  });

  it("does not apply idle section order while searching", () => {
    const result = buildSections(
      sources({
        query: "alpha",
        apps: [app("Alpha")],
        quickItems: quickItems(),
        sectionOrder: ["apps", "quick", "pinned", "recent"],
      }),
    );
    expect(ids(result.sections)).toEqual(["apps"]);
  });

  it("renders configured app collections before remaining apps", () => {
    const result = buildSections(
      sources({
        apps: [app("Premiere Pro"), app("After Effects"), app("PowerShell")],
        appGroups: [
          {
            id: "creative",
            name: "Creative",
            appIds: ["app-id-Premiere Pro", "app-id-After Effects"],
            collapsed: false,
          },
        ],
      }),
    );
    expect(result.sections[0].groups?.[0]).toMatchObject({
      groupId: "creative",
      label: "Creative",
      collapsed: false,
    });
    expect(result.sections[0].groups?.[0].items.map((item) => item.title)).toEqual([
      "Premiere Pro",
      "After Effects",
    ]);
    expect(result.sections[0].items.map((item) => item.title)).toEqual(["PowerShell"]);
    expect(result.flatItems.map((item) => item.title)).toEqual([
      "Premiere Pro",
      "After Effects",
      "PowerShell",
    ]);
  });

  it("keeps collapsed collection apps out of keyboard results", () => {
    const result = buildSections(
      sources({
        apps: [app("Photoshop"), app("PowerShell")],
        appGroups: [{ id: "creative", name: "Creative", appIds: ["app-id-Photoshop"], collapsed: true }],
      }),
    );
    expect(result.sections[0].groups?.[0].items).toEqual([]);
    expect(result.flatItems.map((item) => item.title)).toEqual(["PowerShell"]);
  });

  it("rehydrates app and file history, skipping pinned and dead paths", () => {
    const vsc = app("Visual Studio Code");
    const result = buildSections(
      sources({
        apps: [vsc],
        pinnedApps: [vsc],
        pinnedAppIds: [vsc.appId],
        quickItems: quickItems(),
        history: [
          { id: `app::${vsc.appId}`, title: "Visual Studio Code", ts: 9 },
          { id: `file::d::${DOWNLOADS_PATH}`, title: "Downloads", ts: 8 },
          { id: "file::f::C:\\Users\\You\\gone.txt", title: "gone.txt", ts: 7 },
        ],
        existingHistoryPaths: new Set([DOWNLOADS_PATH]),
      }),
    );

    // Pinned history entry is deduped out of Recent and dead paths are dropped.
    // Quick Access remains stable so its complete ordered list can be managed.
    expect(ids(result.sections)).toEqual(["pinned", "recent", "quick"]);
    expect(result.sections[1].items.map((i) => i.title)).toEqual(["Downloads"]);
    expect(result.sections[2].items.map((i) => i.title)).toEqual(["Downloads", "Documents", "Music"]);
  });

  it("omits empty sections", () => {
    expect(buildSections(sources()).sections).toEqual([]);
  });

  it("keeps Quick Access available as a collapsed section without selectable rows", () => {
    const result = buildSections(sources({ quickItems: quickItems(), quickAccessCollapsed: true }));
    expect(result.sections).toMatchObject([{ id: "quick", collapsible: true, collapsed: true, items: [] }]);
    expect(result.flatItems).toEqual([]);
  });

  it("concatenates flatItems in section order", () => {
    const result = buildSections(
      sources({
        apps: [app("Alpha"), app("Bravo")],
        pinnedApps: [app("Zebra")],
        quickItems: quickItems(),
        history: [{ id: `file::d::${DOWNLOADS_PATH}`, title: "Downloads", ts: 1 }],
        existingHistoryPaths: new Set([DOWNLOADS_PATH]),
      }),
    );
    expect(result.flatItems.map((i) => i.title)).toEqual([
      "Zebra",
      "Downloads",
      "Downloads",
      "Documents",
      "Music",
      "Alpha",
      "Bravo",
    ]);
  });
});

describe("buildSections - search layout", () => {
  it("puts a math result first when the query evaluates", () => {
    const result = buildSections(sources({ query: "12 * 8" }));
    expect(ids(result.sections)).toEqual(["calc"]);
    expect(result.sections[0].items[0].id).toBe("calc::96");
    expect(result.sections[0].items[0].title).toBe("96");
  });

  it("ranks Quick Access hits before Apps hits", () => {
    const result = buildSections(
      sources({
        query: "down",
        apps: [app("Downwell"), app("Firefox")],
        quickItems: quickItems(),
      }),
    );
    expect(ids(result.sections)).toEqual(["quick", "apps"]);
    expect(result.sections[0].items[0].title).toBe("Downloads");
    expect(result.sections[1].items[0].title).toBe("Downwell");
  });

  it.each([
    ["display settings", "Display", "settings::ms-settings:display"],
    ["bluetooth", "Bluetooth", "settings::ms-settings:bluetooth"],
    ["windows update", "Windows Update", "settings::ms-settings:windowsupdate"],
    ["default apps", "Default apps", "settings::ms-settings:defaultapps"],
    ["microphone", "Microphone privacy", "settings::ms-settings:privacy-microphone"],
  ])("maps %s to the documented Windows Settings page", (query, title, id) => {
    const result = buildSections(sources({ query, fileIndexReady: true }));

    expect(result.sections).toMatchObject([{ id: "settings", label: "Settings" }]);
    expect(result.sections[0].items[0]).toMatchObject({ title, id });
  });

  it.each(["screen", "monitor"])("matches Display through its %s alias", (query) => {
    const result = buildSections(sources({ query, fileIndexReady: true }));

    expect(result.sections[0].items[0].title).toBe("Display");
  });

  it("ranks Apps before Settings and file matches in keyboard order", () => {
    const query = "dis";
    const result = buildSections(
      sources({
        query,
        apps: [app("Discord")],
        fileResults: [file("discord-notes.txt", "C:\\Users\\You\\discord-notes.txt")],
        fileResultQuery: query,
        fileIndexReady: true,
      }),
    );

    expect(ids(result.sections)).toEqual(["apps", "settings", "files"]);
    expect(result.flatItems[0]?.title).toBe("Discord");
  });

  it("does not offer the copy fallback when a Settings page matches", () => {
    const result = buildSections(
      sources({ query: "windows update", fileIndexReady: true, fileIndexing: false }),
    );

    expect(ids(result.sections)).toEqual(["settings"]);
    expect(result.flatItems.some((entry) => entry.id.startsWith("copy::"))).toBe(false);
  });

  it("matches quick access items through keywords only", () => {
    const result = buildSections(sources({ query: "audio", quickItems: quickItems() }));
    expect(result.sections[0].items[0].title).toBe("Music");
  });

  it("drops file results that belong to a stale query", () => {
    const result = buildSections(
      sources({
        query: "report",
        fileResults: [file("report.pdf", "C:\\Users\\You\\report.pdf")],
        fileResultQuery: "old", // mismatched: results are from a previous keystroke
      }),
    );
    expect(ids(result.sections)).toEqual(["fallback"]);
  });

  it("shows Folder Contents above Apps while path-browsing, never a fallback", () => {
    const query = "win";
    const result = buildSections(
      sources({
        query,
        fileResults: [file("system32", "C:\\Windows\\system32", true)],
        fileResultQuery: query,
        filePathBrowse: true,
        apps: [app("Windows Terminal")],
      }),
    );
    expect(ids(result.sections)).toEqual(["files", "apps"]);
    expect(result.sections[0].label).toBe("Folder Contents");
    expect(result.sections[0].items[0].title).toBe("system32");
    expect(result.sections[1].items[0].title).toBe("Windows Terminal");
  });

  it("shows Files & Folders below Apps for normal file hits", () => {
    const query = "brief";
    const result = buildSections(
      sources({
        query,
        apps: [app("Briefcase"), app("Firefox")],
        fileResults: [file("Project brief.docx", "C:\\Users\\You\\Documents\\Project brief.docx")],
        fileResultQuery: query,
      }),
    );
    expect(ids(result.sections)).toEqual(["apps", "files"]);
    expect(result.sections[1].label).toBe("Files & Folders");
  });

  it("uses an image thumbnail when a file result provides one", () => {
    const result = buildSections(
      sources({
        query: "darth",
        fileResults: [
          {
            ...file("darth_vader.png", "E:\\STAR WARS\\IMG assets\\darth_vader.png"),
            thumbnail: "data:image/png;base64,preview",
          },
        ],
        fileResultQuery: "darth",
      }),
    );
    expect(result.sections[0].items[0].icon).toEqual({
      kind: "image",
      src: "data:image/png;base64,preview",
      name: "darth_vader.png",
    });
  });

  it("uses a cached image thumbnail when rehydrating Recent", () => {
    const path = "C:\\Users\\You\\Pictures\\everything.png";
    const result = buildSections(
      sources({
        history: [{ id: `file::f::${path}`, title: "everything.png", ts: 1 }],
        existingHistoryPaths: new Set([path]),
        fileThumbnails: new Map([[path, "data:image/png;base64,recent-preview"]]),
      }),
    );

    expect(result.sections[0].items[0].icon).toEqual({
      kind: "image",
      src: "data:image/png;base64,recent-preview",
      name: "everything.png",
    });
  });

  it("offers the copy fallback only when nothing else matches and nothing is loading or indexing", () => {
    const base = { query: "zzzznope", fileIndexReady: true, fileIndexing: false } as const;
    expect(ids(buildSections(sources(base)).sections)).toEqual(["fallback"]);
    expect(buildSections(sources(base)).sections[0].items[0].id.startsWith("copy::")).toBe(true);
    expect(buildSections(sources({ ...base, filesBusy: true })).sections).toEqual([]);
    expect(buildSections(sources({ ...base, fileIndexing: true })).sections).toEqual([]);
    expect(buildSections(sources({ ...base, fileIndexReady: false })).sections).toEqual([]);
    expect(buildSections(sources({ ...base, filesError: true })).sections).toEqual([]);
    expect(buildSections(sources({ ...base, filePathBrowse: true })).sections).toEqual([]);
  });

  it("deduplicates file results when an app hit points to the exact same file path", () => {
    const appTarget = "C:\\Program Files\\Blender\\blender.exe";
    const blenderApp = app("Blender", { path: appTarget });
    const fileHit = file("blender.exe", appTarget);

    const result = buildSections(
      sources({
        query: "blender",
        apps: [blenderApp],
        fileResults: [fileHit],
        fileResultQuery: "blender",
      }),
    );

    // Should only have the Apps section, not a duplicate Files section for blender.exe
    expect(ids(result.sections)).toEqual(["apps"]);
    expect(result.sections[0].items[0].title).toBe("Blender");
  });
});

describe("isClipboardKind", () => {
  it("recognizes calculator and copy items", () => {
    expect(isClipboardKind("calc::96")).toBe(true);
    expect(isClipboardKind("copy::hello")).toBe(true);
    expect(isClipboardKind("app::app-id-1")).toBe(false);
    expect(isClipboardKind("file::f::C:\\x.txt")).toBe(false);
  });
});

describe("quick access kinds round-trip", () => {
  it("keeps every QuickAccessKind constructible as an item", () => {
    const kinds: QuickAccessEntry["kind"][] = [
      "home",
      "desktop",
      "downloads",
      "documents",
      "pictures",
      "music",
      "videos",
    ];
    for (const kind of kinds) {
      const result = buildSections(
        sources({ query: kind, quickItems: [item(kind, `file::d::C:\\${kind}`, { keywords: [kind] })] }),
      );
      expect(result.sections[0].items[0].title).toBe(kind);
    }
  });
});

describe("openLocation support", () => {
  it("attaches openLocation to quickAccessPaletteItem", () => {
    const qa = quickAccessPaletteItem({
      name: "Downloads",
      path: "C:\\Users\\You\\Downloads",
      kind: "downloads",
    });
    expect(typeof qa.openLocation).toBe("function");
    expect(typeof qa.run).toBe("function");
  });

  it("attaches openLocation to file search results", () => {
    const fileHit = file("report.pdf", "C:\\Users\\You\\Documents\\report.pdf");
    const result = buildSections(
      sources({
        query: "report",
        fileResults: [fileHit],
        fileResultQuery: "report",
      }),
    );
    const fileSection = result.sections.find((s) => s.id === "files");
    expect(fileSection).toBeDefined();
    expect(typeof fileSection?.items[0].openLocation).toBe("function");
  });

  it("attaches openLocation to apps with valid local paths", () => {
    const appWithLocalPath = app("ToolA", { path: "C:\\Tools\\custom.exe" });
    const appWithoutPath = app("ToolB", { path: undefined });

    const result = buildSections(
      sources({
        query: "tool",
        apps: [appWithLocalPath, appWithoutPath],
      }),
    );

    const appsSection = result.sections.find((s) => s.id === "apps");
    expect(appsSection).toBeDefined();
    const withPathItem = appsSection?.items.find((i) => i.title === "ToolA");
    const withoutPathItem = appsSection?.items.find((i) => i.title === "ToolB");
    expect(typeof withPathItem?.openLocation).toBe("function");
    expect(withoutPathItem?.openLocation).toBeUndefined();
  });
});

describe("taskbar pin & properties support", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("attaches shell actions to apps with a local launch target", () => {
    const withTarget = app("ToolA", { path: "C:\\Tools\\custom.exe" });
    const packaged = app("ToolB", { path: undefined });

    const result = buildSections(
      sources({
        query: "tool",
        apps: [withTarget, packaged],
      }),
    );

    const appsSection = result.sections.find((s) => s.id === "apps");
    const targetItem = appsSection?.items.find((i) => i.title === "ToolA");
    const packagedItem = appsSection?.items.find((i) => i.title === "ToolB");
    expect(targetItem?.shellPath).toBe("C:\\Tools\\custom.exe");
    expect(typeof targetItem?.toggleTaskbarPin).toBe("function");
    expect(typeof targetItem?.showProperties).toBe("function");
    expect(packagedItem?.shellPath).toBeUndefined();
    expect(packagedItem?.toggleTaskbarPin).toBeUndefined();
    expect(packagedItem?.showProperties).toBeUndefined();
  });

  it("does not offer taskbar pin for URL shortcuts", () => {
    const webApp = app("Site", { path: "https://example.com", location: "C:\\Links\\Site.url" });
    const result = buildSections(sources({ query: "site", apps: [webApp] }));

    const siteItem = result.sections.find((s) => s.id === "apps")?.items[0];
    expect(siteItem?.shellPath).toBe("C:\\Links\\Site.url");
    expect(siteItem?.toggleTaskbarPin).toBeUndefined();
    expect(typeof siteItem?.showProperties).toBe("function");
  });

  it("pins only launchable files but shows properties for every file result", () => {
    const installer = file("setup.exe", "C:\\Users\\You\\Downloads\\setup.exe");
    const doc = file("report.pdf", "C:\\Users\\You\\Documents\\report.pdf");
    const result = buildSections(
      sources({
        query: "s",
        fileResults: [installer, doc],
        fileResultQuery: "s",
      }),
    );

    const filesSection = result.sections.find((s) => s.id === "files");
    const exeItem = filesSection?.items.find((i) => i.title === "setup.exe");
    const pdfItem = filesSection?.items.find((i) => i.title === "report.pdf");
    expect(exeItem?.shellPath).toBe("C:\\Users\\You\\Downloads\\setup.exe");
    expect(typeof exeItem?.toggleTaskbarPin).toBe("function");
    expect(typeof pdfItem?.showProperties).toBe("function");
    expect(pdfItem?.toggleTaskbarPin).toBeUndefined();
  });

  it("attaches properties but not taskbar pin to Quick Access folders", () => {
    const qa = quickAccessPaletteItem({ name: "Downloads", path: DOWNLOADS_PATH, kind: "downloads" });
    expect(qa.shellPath).toBe(DOWNLOADS_PATH);
    expect(qa.toggleTaskbarPin).toBeUndefined();
    expect(typeof qa.showProperties).toBe("function");
  });

  it("flips the queried taskbar pin state at click time", async () => {
    const installer = file("setup.exe", "C:\\Users\\You\\Downloads\\setup.exe");
    const result = buildSections(sources({ query: "s", fileResults: [installer], fileResultQuery: "s" }));
    const exeItem = result.sections.find((s) => s.id === "files")?.items[0];

    vi.mocked(isPinnedToTaskbar).mockResolvedValue(false);
    await exeItem?.toggleTaskbarPin?.();
    expect(isPinnedToTaskbar).toHaveBeenCalledWith("C:\\Users\\You\\Downloads\\setup.exe");
    expect(setTaskbarPinned).toHaveBeenLastCalledWith("C:\\Users\\You\\Downloads\\setup.exe", true);

    vi.mocked(isPinnedToTaskbar).mockResolvedValue(true);
    await exeItem?.toggleTaskbarPin?.();
    expect(setTaskbarPinned).toHaveBeenLastCalledWith("C:\\Users\\You\\Downloads\\setup.exe", false);
  });

  it("marks pictures as draggable with isPicture and dragFile", () => {
    const photo = file("photo.png", "C:\\Users\\You\\Pictures\\photo.png");
    const doc = file("report.docx", "C:\\Users\\You\\Documents\\report.docx");
    const video = file("clip.mp4", "C:\\Users\\You\\Videos\\clip.mp4");
    const audio = file("song.mp3", "C:\\Users\\You\\Music\\song.mp3");
    const archive = file("backup.zip", "C:\\Users\\You\\Downloads\\backup.zip");
    const folder = { name: "Projects", path: "C:\\Users\\You\\Projects", parent: "C:\\Users\\You", isDirectory: true };
    const result = buildSections(
      sources({ query: "p", fileResults: [photo, doc, video, audio, archive, folder], fileResultQuery: "p" }),
    );
    const filesSection = result.sections.find((s) => s.id === "files");
    const photoItem = filesSection?.items.find((i) => i.title === "photo.png");
    const docItem = filesSection?.items.find((i) => i.title === "report.docx");
    const videoItem = filesSection?.items.find((i) => i.title === "clip.mp4");
    const audioItem = filesSection?.items.find((i) => i.title === "song.mp3");
    const archiveItem = filesSection?.items.find((i) => i.title === "backup.zip");
    const folderItem = filesSection?.items.find((i) => i.title === "Projects");

    expect(photoItem?.isPicture).toBe(true);
    expect(typeof photoItem?.dragFile).toBe("function");
    expect(docItem?.isPicture).toBe(false);
    expect(typeof docItem?.dragFile).toBe("function");
    expect(typeof videoItem?.dragFile).toBe("function");
    expect(typeof audioItem?.dragFile).toBe("function");
    expect(typeof archiveItem?.dragFile).toBe("function");
    expect(typeof folderItem?.dragFile).toBe("function");
  });

  it("marks quick access and local app targets as draggable", () => {
    const quick = quickAccessPaletteItem({
      name: "Documents",
      path: "C:\\Users\\You\\Documents",
      kind: "documents",
    });
    expect(typeof quick.dragFile).toBe("function");

    const appWithTarget = app("AppTarget", { path: "C:\\Games\\game.exe" });
    const result = buildSections(sources({ query: "app", apps: [appWithTarget] }));
    const appSection = result.sections.find((s) => s.id === "apps");
    const appItem = appSection?.items.find((i) => i.title === "AppTarget");
    expect(typeof appItem?.dragFile).toBe("function");
  });
});
