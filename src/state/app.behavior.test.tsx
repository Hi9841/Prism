// @vitest-environment happy-dom
import { act, cleanup, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as bridge from "../lib/bridge";
import { DEFAULT_SETTINGS } from "../lib/types";
import { AppProvider, useApp } from "./app";

vi.mock("../lib/bridge", () => ({
  loadState: vi.fn().mockResolvedValue(null),
  saveState: vi.fn().mockResolvedValue(undefined),
  quitApp: vi.fn().mockResolvedValue(undefined),
  getSystemTheme: vi.fn().mockResolvedValue("dark"),
  onSystemThemeChange: vi.fn(() => () => {}),
  onWinModeFailed: vi.fn(() => () => {}),
  setAlwaysOnTop: vi.fn().mockResolvedValue(undefined),
  setShortcut: vi.fn().mockResolvedValue(undefined),
  setTaskbarAlignment: vi.fn().mockResolvedValue(undefined),
  setTaskbarScrollVolume: vi.fn().mockResolvedValue(undefined),
  setViewZoom: vi.fn().mockResolvedValue(undefined),
  setWindowStyle: vi.fn().mockResolvedValue(undefined),
  setWindowWidth: vi.fn().mockResolvedValue(undefined),
}));

let app: ReturnType<typeof useApp>;
function Probe() {
  app = useApp();
  return <output>{app.persistenceError}</output>;
}

async function mount() {
  await act(async () => {
    render(
      <AppProvider>
        <Probe />
      </AppProvider>,
    );
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(bridge.loadState).mockResolvedValue(null);
  vi.mocked(bridge.saveState).mockResolvedValue(undefined);
  vi.mocked(bridge.setShortcut).mockResolvedValue(undefined);
  vi.mocked(bridge.setTaskbarAlignment).mockResolvedValue(undefined);
  vi.useFakeTimers();
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("persistence lifecycle", () => {
  it("waits for initial state before flushing an early edit", async () => {
    let resolveLoad!: (state: Awaited<ReturnType<typeof bridge.loadState>>) => void;
    vi.mocked(bridge.loadState).mockReturnValueOnce(
      new Promise((resolve) => {
        resolveLoad = resolve;
      }),
    );
    await mount();
    let quitting!: Promise<void>;
    await act(async () => {
      app.updateSettings({ accent: "mint" });
      quitting = app.quit();
    });
    expect(bridge.saveState).not.toHaveBeenCalled();
    expect(bridge.quitApp).not.toHaveBeenCalled();
    await act(async () => {
      resolveLoad({ version: 3, settings: { ...DEFAULT_SETTINGS, pinnedApps: ["saved-app"] }, history: [] });
      await quitting;
    });
    expect(bridge.saveState).toHaveBeenLastCalledWith(
      expect.objectContaining({
        settings: expect.objectContaining({ pinnedApps: ["saved-app"], accent: "mint" }),
      }),
    );
  });
  it("retries a repaired file without losing saved pins or edits made during the failure", async () => {
    vi.mocked(bridge.loadState).mockRejectedValueOnce(new Error("corrupt state file"));
    await mount();
    await act(async () => {
      app.updateSettings({ accent: "mint" });
    });
    vi.mocked(bridge.loadState).mockResolvedValueOnce({
      version: 3,
      settings: {
        ...DEFAULT_SETTINGS,
        pinnedApps: ["saved-app"],
        accent: "rose",
      },
      history: [],
    });
    await act(async () => {
      await app.retryPersistence();
    });
    expect(app.settings.accent).toBe("mint");
    expect(app.settings.pinnedApps).toEqual(["saved-app"]);
    expect(app.persistenceError).toBeNull();
    expect(bridge.saveState).toHaveBeenLastCalledWith(
      expect.objectContaining({
        settings: expect.objectContaining({ pinnedApps: ["saved-app"], accent: "mint" }),
      }),
    );
  });
  it("flushes an edit before quitting inside the debounce window", async () => {
    await mount();
    await act(async () => {
      app.updateSettings({ pinnedApps: ["example-app"] });
      await app.quit();
    });
    expect(bridge.saveState).toHaveBeenCalledWith(
      expect.objectContaining({
        settings: expect.objectContaining({ pinnedApps: ["example-app"] }),
      }),
    );
    expect(vi.mocked(bridge.saveState).mock.invocationCallOrder[0]).toBeLessThan(
      vi.mocked(bridge.quitApp).mock.invocationCallOrder[0],
    );
  });

  it("keeps failed saves visible, refuses quit, and retries the latest state", async () => {
    await mount();
    vi.mocked(bridge.saveState).mockRejectedValue(new Error("disk full"));
    await act(async () => {
      app.updateSettings({ accent: "mint" });
      await vi.advanceTimersByTimeAsync(350);
      await app.quit();
    });
    expect(app.persistenceError).toContain("disk full");
    expect(app.toasts).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          title: "Settings need attention",
          kind: "error",
        }),
      ]),
    );
    expect(bridge.quitApp).not.toHaveBeenCalled();
    vi.mocked(bridge.saveState).mockResolvedValue(undefined);
    await act(async () => {
      app.updateSettings({ accent: "rose" });
      await app.retryPersistence();
    });
    expect(app.persistenceError).toBeNull();
    expect(bridge.saveState).toHaveBeenLastCalledWith(
      expect.objectContaining({
        settings: expect.objectContaining({ accent: "rose" }),
      }),
    );
  });

  it("does not overwrite an unreadable state file with defaults", async () => {
    vi.mocked(bridge.loadState).mockRejectedValue(new Error("corrupt state file"));
    await mount();
    await act(async () => {
      app.updateSettings({ accent: "mint" });
      await vi.advanceTimersByTimeAsync(350);
      await app.quit();
    });
    expect(app.persistenceError).toContain("corrupt state file");
    expect(bridge.saveState).not.toHaveBeenCalled();
    expect(bridge.quitApp).not.toHaveBeenCalled();
  });

  it("serializes writes and flushes edits made while a save is pending", async () => {
    let release: (() => void) | undefined;
    vi.mocked(bridge.saveState).mockImplementationOnce(
      () =>
        new Promise<void>((resolve) => {
          release = resolve;
        }),
    );
    await mount();
    await act(async () => {
      app.updateSettings({ accent: "mint" });
      await vi.advanceTimersByTimeAsync(350);
      app.updateSettings({ accent: "rose" });
      await vi.advanceTimersByTimeAsync(350);
    });
    expect(bridge.saveState).toHaveBeenCalledTimes(1);
    await act(async () => {
      release?.();
      await app.flushPersistence();
    });
    expect(bridge.saveState).toHaveBeenCalledTimes(2);
    expect(bridge.saveState).toHaveBeenLastCalledWith(
      expect.objectContaining({
        settings: expect.objectContaining({ accent: "rose" }),
      }),
    );
  });
});

describe("native reset", () => {
  it("rolls back shortcut registration when alignment fails", async () => {
    vi.mocked(bridge.loadState).mockResolvedValue({
      version: 3,
      settings: {
        ...DEFAULT_SETTINGS,
        shortcut: "Ctrl+Alt+Space",
        taskbarAlignment: "left",
      },
      history: [],
    });
    await mount();
    vi.mocked(bridge.setTaskbarAlignment).mockRejectedValueOnce(new Error("shell busy"));
    await act(async () => {
      await expect(app.resetSettings()).rejects.toThrow("shell busy");
    });
    expect(bridge.setShortcut).toHaveBeenNthCalledWith(1, DEFAULT_SETTINGS.shortcut);
    expect(bridge.setShortcut).toHaveBeenNthCalledWith(2, "Ctrl+Alt+Space");
    expect(bridge.setTaskbarAlignment).toHaveBeenNthCalledWith(2, "left");
    expect(app.settings.shortcut).toBe("Ctrl+Alt+Space");
    expect(app.settings.taskbarAlignment).toBe("left");
    expect(bridge.saveState).not.toHaveBeenCalled();
  });
});

describe("notification limits", () => {
  it("keeps the latest two errors when several failures arrive before a render", async () => {
    await mount();
    await act(async () => {
      for (let i = 0; i < 10; i += 1) app.showToast(`Failure ${i}`, "Retry the action", "error");
    });
    expect(app.toasts.filter((toast) => !toast.closing).map((toast) => toast.title)).toEqual([
      "Failure 8",
      "Failure 9",
    ]);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(160);
    });
    expect(app.toasts).toHaveLength(2);
  });

  it("dismisses a toast created in the same render batch", async () => {
    await mount();
    await act(async () => {
      app.showToast("Saved");
      await vi.advanceTimersByTimeAsync(2100);
    });
    expect(app.toasts).toHaveLength(0);
  });
});
