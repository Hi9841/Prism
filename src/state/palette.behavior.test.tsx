// @vitest-environment happy-dom

import { act, cleanup, render } from "@testing-library/react";
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
  onFileIndexUpdated: vi.fn(),
  onWindowFocused: vi.fn(() => () => {}),
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
  vi.mocked(bridge.getApps).mockResolvedValue([]);
  vi.mocked(bridge.getAppIcons).mockResolvedValue({});
  vi.mocked(bridge.refreshApps).mockResolvedValue([]);
  vi.mocked(bridge.getFileThumbnails).mockImplementation(async (paths) => paths.map(() => null));
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

async function mountPalette() {
  await act(async () => {
    render(
      <PaletteProvider>
        <Probe />
      </PaletteProvider>,
    );
  });
}

describe("palette recovery", () => {
  it("releases the indexing state after a rejected rebuild so the user can retry", async () => {
    await mountPalette();
    vi.mocked(bridge.rebuildFileIndex).mockRejectedValueOnce(new Error("catalog unavailable"));
    await act(async () => palette.rebuildIndex());
    expect(palette.fileIndexing).toBe(false);
    expect(mockApp.showToast).toHaveBeenCalledWith(
      "Could not rebuild file index",
      expect.stringContaining("catalog unavailable"),
      "error",
    );
    await act(async () => palette.rebuildIndex());
    expect(bridge.rebuildFileIndex).toHaveBeenCalledTimes(2);
  });

  it.each(["launch", "clipboard", "administrator"])(
    "reports %s failures as persistent errors",
    async (kind) => {
      await mountPalette();
      const fail = vi.fn().mockRejectedValue(new Error("access denied"));
      const item: PaletteItem = {
        id: kind === "clipboard" ? "calc::2+2" : "app::example",
        title: "Example",
        historyTitle: "Example",
        icon: { kind: "app", name: "Example" },
        run: fail,
        runAsAdmin: fail,
      };
      await act(async () => {
        if (kind === "administrator") await palette.runItemAsAdmin(item);
        else await palette.runItem(item);
      });
      expect(mockApp.showToast).toHaveBeenCalledWith(
        kind === "clipboard"
          ? "Could not copy to clipboard"
          : kind === "administrator"
            ? "Could not run as administrator"
            : "Could not open item",
        expect.stringContaining("access denied"),
        "error",
      );
      expect(mockApp.pushHistory).not.toHaveBeenCalled();
    },
  );

  it("requests fresh icons after a rescan and ignores the previous in-flight response", async () => {
    const apps = [{ appId: "example", name: "Example" }];
    const oldIcons = deferred<Record<string, string>>();
    const freshIcons = deferred<Record<string, string>>();
    vi.mocked(bridge.getApps).mockResolvedValueOnce(apps);
    vi.mocked(bridge.refreshApps).mockResolvedValueOnce(apps);
    vi.mocked(bridge.getAppIcons)
      .mockReturnValueOnce(oldIcons.promise)
      .mockReturnValueOnce(freshIcons.promise);
    await mountPalette();
    expect(bridge.getAppIcons).toHaveBeenCalledTimes(1);
    await act(async () => palette.refreshApps());
    expect(bridge.getAppIcons).toHaveBeenCalledTimes(2);
    await act(async () => freshIcons.resolve({ example: "fresh" }));
    expect(palette.flatItems.find((item) => item.appId === "example")?.icon).toMatchObject({ icon: "fresh" });
    await act(async () => oldIcons.resolve({ example: "stale" }));
    expect(palette.flatItems.find((item) => item.appId === "example")?.icon).toMatchObject({ icon: "fresh" });
  });
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
