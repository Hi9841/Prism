// @vitest-environment happy-dom

import { act, cleanup, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as bridge from "../lib/bridge";
import { DEFAULT_SETTINGS, type FileSearchResponse } from "../lib/types";
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
