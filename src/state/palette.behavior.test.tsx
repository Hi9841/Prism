// @vitest-environment happy-dom

import { act, cleanup, render } from "@testing-library/react";
import { Power } from "lucide-react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as bridge from "../lib/bridge";
import { DEFAULT_SETTINGS, type FileSearchResponse, type PaletteItem } from "../lib/types";
import { PaletteProvider, usePalette } from "./palette";

vi.mock("../lib/bridge", () => ({
  existingPaths: vi.fn().mockResolvedValue([]),
  refreshApps: vi.fn().mockResolvedValue([]),
  getAppIcons: vi.fn().mockResolvedValue({}),
  getApps: vi.fn().mockResolvedValue([]),
  getFileThumbnails: vi.fn(),
  getQuickAccess: vi.fn().mockResolvedValue([]),
  hidePaletteWindow: vi.fn().mockResolvedValue(undefined),
  onAppsUpdated: vi.fn(() => () => {}),
  onFileIndexUpdated: vi.fn(),
  onWindowFocused: vi.fn(() => () => {}),
  queryPhase1: vi.fn().mockResolvedValue({
    query: "",
    recents: [],
    windows: [],
    apps: [],
    actions: [],
  }),
  queryPowerPlans: vi.fn().mockResolvedValue([]),
  acceptIntent: vi.fn().mockResolvedValue(undefined),
  setPowerPlan: vi.fn().mockResolvedValue(undefined),
  rebuildFileIndex: vi.fn().mockResolvedValue(undefined),
  searchFiles: vi.fn(),
}));

const mockApp = {
  settings: DEFAULT_SETTINGS,
  history: [],
  pushHistory: vi.fn(),
  showToast: vi.fn(),
};

vi.mock("./app", () => ({
  useApp: () => mockApp,
}));

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((next) => {
    resolve = next;
  });
  return { promise, resolve };
}

const statusResponse: FileSearchResponse = {
  items: [],
  ready: true,
  indexing: false,
  pathBrowse: false,
  volumes: [],
  totalIndexed: 1,
};

let palette: ReturnType<typeof usePalette>;
let indexUpdated: (() => void) | undefined;

function Probe() {
  palette = usePalette();
  return null;
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.useFakeTimers();
  indexUpdated = undefined;
  vi.mocked(bridge.onFileIndexUpdated).mockImplementation((callback) => {
    indexUpdated = callback;
    return () => {};
  });
  vi.mocked(bridge.searchFiles).mockImplementation(async (query) =>
    query
      ? {
          ...statusResponse,
          items: [
            {
              name: "photo.png",
              path: "C:\\Pictures\\photo.png",
              parent: "C:\\Pictures",
              isDirectory: false,
            },
          ],
        }
      : statusResponse,
  );
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("thumbnail cache invalidation", () => {
  it("allows a fresh request and ignores an older response after a catalog update", async () => {
    const oldThumbnail = deferred<(string | null)[]>();
    const freshThumbnail = deferred<(string | null)[]>();
    vi.mocked(bridge.getFileThumbnails)
      .mockReturnValueOnce(oldThumbnail.promise)
      .mockReturnValueOnce(freshThumbnail.promise);

    await act(async () => {
      render(
        <PaletteProvider>
          <Probe />
        </PaletteProvider>,
      );
    });
    await act(async () => {
      palette.setQuery("photo");
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(35);
    });
    expect(bridge.getFileThumbnails).toHaveBeenCalledTimes(1);

    await act(async () => {
      indexUpdated?.();
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(35);
    });
    expect(bridge.getFileThumbnails).toHaveBeenCalledTimes(2);

    await act(async () => {
      freshThumbnail.resolve(["data:image/png;base64,fresh"]);
      await Promise.resolve();
    });
    expect(palette.flatItems[0]?.icon).toMatchObject({
      kind: "image",
      src: "data:image/png;base64,fresh",
    });

    await act(async () => {
      oldThumbnail.resolve(["data:image/png;base64,stale"]);
      await Promise.resolve();
    });
    expect(palette.flatItems[0]?.icon).toMatchObject({
      kind: "image",
      src: "data:image/png;base64,fresh",
    });
  });
});

describe("two-phase query paint", () => {
  it("paints Phase 1 before file search runs", async () => {
    vi.mocked(bridge.queryPhase1).mockResolvedValue({
      query: "chrome",
      recents: [],
      windows: [
        {
          id: "window::9",
          kind: "window",
          title: "Chrome",
          subtitle: "Open window · chrome",
          score: 900,
          hwnd: 9,
          iconKey: "window",
        },
      ],
      apps: [],
      actions: [],
    });
    const files = deferred<FileSearchResponse>();
    vi.mocked(bridge.searchFiles).mockImplementation((query) =>
      query ? files.promise : Promise.resolve(statusResponse),
    );

    await act(async () => {
      render(
        <PaletteProvider>
          <Probe />
        </PaletteProvider>,
      );
    });
    await act(async () => {
      palette.setQuery("chrome");
    });
    await act(async () => {
      await Promise.resolve();
    });

    expect(palette.flatItems.some((item) => item.title === "Chrome")).toBe(true);
    expect(vi.mocked(bridge.searchFiles).mock.calls.some(([query]) => query === "chrome")).toBe(false);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(35);
    });
    expect(vi.mocked(bridge.searchFiles).mock.calls.some(([query]) => query === "chrome")).toBe(true);
  });

  it("does not refresh a stale power-plan query", async () => {
    const action = deferred<void>();
    await act(async () => {
      render(
        <PaletteProvider>
          <Probe />
        </PaletteProvider>,
      );
    });
    const item: PaletteItem = {
      id: "power-plan::22222222-2222-2222-2222-222222222222",
      title: "High performance",
      subtitle: "Power plan",
      icon: { kind: "tile", icon: Power, tint: "slate" },
      historyTitle: "High performance",
      powerPlanGuid: "22222222-2222-2222-2222-222222222222",
      run: () => action.promise,
    };
    let runPromise = Promise.resolve();
    await act(async () => {
      palette.setQuery("power plan");
    });
    await act(async () => {
      runPromise = palette.runItem(item);
    });
    await act(async () => {
      palette.setQuery("chrome");
    });
    await act(async () => {
      action.resolve();
      await runPromise;
    });

    expect(bridge.queryPowerPlans).not.toHaveBeenCalled();
  });

  it("refreshes the active marker and keeps the applied plan selected", async () => {
    vi.mocked(bridge.queryPhase1).mockResolvedValue({
      query: "power plan",
      recents: [],
      windows: [],
      apps: [],
      actions: [],
    });
    vi.mocked(bridge.queryPowerPlans)
      .mockResolvedValueOnce([
        {
          guid: "11111111-1111-1111-1111-111111111111",
          name: "Balanced",
          active: true,
          score: 800,
          pickerIntent: true,
        },
        {
          guid: "22222222-2222-2222-2222-222222222222",
          name: "High performance",
          active: false,
          score: 700,
          pickerIntent: true,
        },
      ])
      .mockResolvedValueOnce([
        {
          guid: "11111111-1111-1111-1111-111111111111",
          name: "Balanced",
          active: false,
          score: 700,
          pickerIntent: true,
        },
        {
          guid: "22222222-2222-2222-2222-222222222222",
          name: "High performance",
          active: true,
          score: 800,
          pickerIntent: true,
        },
      ]);

    await act(async () => {
      render(
        <PaletteProvider>
          <Probe />
        </PaletteProvider>,
      );
    });
    await act(async () => {
      palette.setQuery("power plan");
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(50);
    });
    expect(palette.sections.find((section) => section.id === "power-plans")?.items[0].subtitle).toBe(
      "Power plan · active",
    );

    const item: PaletteItem = {
      id: "power-plan::22222222-2222-2222-2222-222222222222",
      title: "High performance",
      subtitle: "Power plan",
      icon: { kind: "tile", icon: Power, tint: "slate" },
      historyTitle: "High performance",
      powerPlanGuid: "22222222-2222-2222-2222-222222222222",
      run: vi.fn().mockResolvedValue(undefined),
    };
    await act(async () => {
      await palette.runItem(item);
    });

    expect(item.run).toHaveBeenCalledOnce();
    expect(bridge.queryPowerPlans).toHaveBeenCalledTimes(2);
    expect(palette.sections.find((section) => section.id === "power-plans")?.items[1].subtitle).toBe(
      "Power plan · active",
    );
    expect(palette.flatItems[palette.selected]?.id).toBe(item.id);
  });
});

describe("installed app refresh", () => {
  it("replaces the app list when a shortcut is indexed after startup", async () => {
    let notify: (() => void) | undefined;
    vi.mocked(bridge.onAppsUpdated).mockImplementation((callback) => {
      notify = callback;
      return () => {};
    });
    vi.mocked(bridge.getApps)
      .mockResolvedValueOnce([])
      .mockResolvedValueOnce([
        {
          name: "Zeron",
          appId: "zeron",
          normalizedName: "zeron",
          source: "startMenu",
          path: "C:\\Users\\hi\\AppData\\Local\\Programs\\Zeron\\zeron.exe",
        },
      ]);

    await act(async () => {
      render(
        <PaletteProvider>
          <Probe />
        </PaletteProvider>,
      );
    });
    expect(palette.apps).toEqual([]);

    await act(async () => {
      notify?.();
      await Promise.resolve();
    });

    expect(palette.apps.map((entry) => entry.name)).toEqual(["Zeron"]);
  });
});
