import { describe, expect, it } from "vitest";
import {
  DEFAULT_QUICK_ACCESS,
  DEFAULT_SETTINGS,
  isElevatablePath,
  reorderPinnedApps,
  reorderQuickAccess,
  stepViewZoom,
} from "../lib/types";
import { SHORTCUT_OPTIONS, sanitizeHistory, sanitizeSettings } from "./app";

describe("default shortcut", () => {
  it("uses standalone Win for a fresh or invalid state", () => {
    expect(DEFAULT_SETTINGS.shortcut).toBe("Win");
    expect(SHORTCUT_OPTIONS[0]?.value).toBe("Win");
    expect(sanitizeSettings({}).shortcut).toBe("Win");
    expect(sanitizeSettings({ shortcut: "invalid" }).shortcut).toBe("Win");
  });

  it("preserves a valid shortcut selected by an existing user", () => {
    expect(sanitizeSettings({ shortcut: "Ctrl+Alt+Space" }).shortcut).toBe("Ctrl+Alt+Space");
  });
});

describe("view zoom", () => {
  it("steps within the supported range", () => {
    expect(stepViewZoom(100, 1)).toBe(110);
    expect(stepViewZoom(100, -1)).toBe(90);
    expect(stepViewZoom(150, 1)).toBe(150);
    expect(stepViewZoom(70, -1)).toBe(70);
  });

  it("sanitizes persisted zoom values and defaults older state", () => {
    expect(sanitizeSettings({ viewZoom: 130 }).viewZoom).toBe(130);
    expect(sanitizeSettings({ viewZoom: 135 }).viewZoom).toBe(DEFAULT_SETTINGS.viewZoom);
    expect(sanitizeSettings({}).viewZoom).toBe(DEFAULT_SETTINGS.viewZoom);
  });
});

describe("taskbar alignment", () => {
  it("defaults older state to center alignment and preserves valid choices", () => {
    expect(sanitizeSettings({}).taskbarAlignment).toBe("center");
    expect(sanitizeSettings({ taskbarAlignment: "center" }).taskbarAlignment).toBe("center");
    expect(sanitizeSettings({ taskbarAlignment: "right" }).taskbarAlignment).toBe("right");
    expect(sanitizeSettings({ taskbarAlignment: "invalid" }).taskbarAlignment).toBe("center");
  });
});

describe("quick access settings", () => {
  it("keeps the current pins as the default for older state", () => {
    expect(sanitizeSettings({}).quickAccess).toEqual(DEFAULT_QUICK_ACCESS);
  });

  it("allows an empty list and filters duplicates, unknown values, and excess pins", () => {
    expect(sanitizeSettings({ quickAccess: [] }).quickAccess).toEqual([]);
    expect(
      sanitizeSettings({
        quickAccess: [
          "videos",
          "home",
          "videos",
          "invalid",
          "desktop",
          "downloads",
          "documents",
          "pictures",
          "music",
        ],
      }).quickAccess,
    ).toEqual(["videos", "home", "desktop", "downloads", "documents", "pictures"]);
  });

  it("persists the collapsed state and defaults older settings to expanded", () => {
    expect(sanitizeSettings({}).quickAccessCollapsed).toBe(false);
    expect(sanitizeSettings({ quickAccessCollapsed: true }).quickAccessCollapsed).toBe(true);
    expect(sanitizeSettings({ quickAccessCollapsed: "yes" }).quickAccessCollapsed).toBe(false);
  });
});

describe("pinned app settings", () => {
  it("defaults older state to no pins", () => {
    expect(sanitizeSettings({}).pinnedApps).toEqual([]);
  });

  it("keeps valid unique app ids and rejects malformed ids", () => {
    expect(sanitizeSettings({ pinnedApps: ["app-a", "app-b", "app-a", "", 4] }).pinnedApps).toEqual([
      "app-a",
      "app-b",
    ]);
  });

  it("caps persisted pins", () => {
    const pinnedApps = Array.from({ length: 80 }, (_, index) => `app-${index}`);
    expect(sanitizeSettings({ pinnedApps }).pinnedApps).toHaveLength(64);
  });
});

describe("pinned app ordering", () => {
  it("moves an app up or down to the target position", () => {
    expect(reorderPinnedApps(["app-a", "app-b", "app-c"], "app-a", "app-b")).toEqual([
      "app-b",
      "app-a",
      "app-c",
    ]);
    expect(reorderPinnedApps(["app-a", "app-b", "app-c"], "app-c", "app-a")).toEqual([
      "app-c",
      "app-a",
      "app-b",
    ]);
  });

  it("leaves the input untouched when a move is invalid", () => {
    const pinnedApps = ["app-a", "app-b"];
    expect(reorderPinnedApps(pinnedApps, "missing", "app-b")).toEqual(pinnedApps);
    expect(reorderPinnedApps(pinnedApps, "app-a", "app-a")).toEqual(pinnedApps);
    expect(pinnedApps).toEqual(["app-a", "app-b"]);
  });
});

describe("quick access ordering", () => {
  it("moves a folder to the selected target position", () => {
    expect(reorderQuickAccess(["home", "desktop", "downloads"], "home", "downloads")).toEqual([
      "desktop",
      "downloads",
      "home",
    ]);
  });

  it("leaves invalid moves untouched", () => {
    const quickAccess = ["home", "desktop"] as const;
    expect(reorderQuickAccess(quickAccess, "home", "home")).toEqual(quickAccess);
  });
});

describe("administrator launch eligibility", () => {
  it("allows desktop applications and supported Windows scripts", () => {
    for (const path of [
      "tool.exe",
      "tool.COM",
      "task.bat",
      "task.cmd",
      "task.ps1",
      "task.vbs",
      "task.js",
      "task.wsf",
    ]) {
      expect(isElevatablePath(`C:\\Tools\\${path}`)).toBe(true);
    }
  });

  it("rejects folders, shortcuts, documents, URLs, and missing targets", () => {
    for (const path of [
      undefined,
      "",
      "C:\\Tools",
      "tool.lnk",
      "notes.txt",
      "https://example.com/tool.exe/",
    ]) {
      expect(isElevatablePath(path)).toBe(false);
    }
  });
});

describe("sanitizeHistory", () => {
  it("keeps valid unique entries and rejects malformed state", () => {
    expect(
      sanitizeHistory([
        { id: "file::d::C:\\Docs", title: "Docs", ts: 2 },
        { id: "file::d::C:\\Docs", title: "Duplicate", ts: 1 },
        { id: 42, title: "Invalid", ts: 0 },
        null,
      ]),
    ).toEqual([{ id: "file::d::C:\\Docs", title: "Docs", ts: 2 }]);
  });

  it("caps persisted history", () => {
    const entries = Array.from({ length: 30 }, (_, index) => ({
      id: `app::${index}`,
      title: `App ${index}`,
      ts: index,
    }));
    expect(sanitizeHistory(entries)).toHaveLength(20);
  });
});
