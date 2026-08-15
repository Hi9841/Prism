import { Home } from "lucide-react";
import { describe, expect, it } from "vitest";
import type { AppEntry, FileEntry, PaletteItem, QuickAccessEntry } from "../../lib/types";
import { buildSections, isClipboardKind, type PaletteSources } from "./sections";

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
