// @vitest-environment happy-dom

import { act, cleanup, render } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";
import * as bridge from "./lib/bridge";
import { LAUNCHER_MOTION_MS } from "./lib/launcherMotion";

let toggleHandler: ((request: { open: boolean }) => void) | undefined;
let focusHandler: ((focused: boolean) => void) | undefined;

vi.mock("./lib/bridge", () => ({
  inTauri: true,
  getAppVersion: vi.fn().mockResolvedValue("0.11.1"),
  hidePaletteWindow: vi.fn().mockResolvedValue(undefined),
  presentPaletteWindow: vi.fn().mockResolvedValue(true),
  isWindowVisible: vi.fn().mockResolvedValue(false),
  onToggleRequest: vi.fn((callback: (request: { open: boolean }) => void) => {
    toggleHandler = callback;
    return () => {
      toggleHandler = undefined;
    };
  }),
  onWindowFocused: vi.fn((callback: (focused: boolean) => void) => {
    focusHandler = callback;
    return () => {
      focusHandler = undefined;
    };
  }),
}));

vi.mock("./state/app", () => ({
  AppProvider: ({ children }: { children: ReactNode }) => children,
  useApp: () => ({
    settings: { width: 720, viewZoom: 100 },
    setOpenSettings: vi.fn(),
  }),
}));

vi.mock("./state/palette", () => ({
  PaletteProvider: ({ children }: { children: ReactNode }) => children,
  usePalette: () => ({ reset: vi.fn() }),
}));

vi.mock("./features/palette/Palette", () => ({
  Palette: () => <div data-testid="palette" />,
}));
vi.mock("./components/SettingsSheet", () => ({ SettingsSheet: () => null }));
vi.mock("./components/Toast", () => ({ ToastStack: () => null }));

function stage(): HTMLElement {
  const node = document.querySelector(".launcher-stage");
  if (!(node instanceof HTMLElement)) throw new Error("launcher stage missing");
  return node;
}

async function openLauncher() {
  await act(async () => {
    toggleHandler?.({ open: true });
  });
  await act(async () => {
    await Promise.resolve();
  });
}

beforeEach(() => {
  toggleHandler = undefined;
  focusHandler = undefined;
  vi.mocked(bridge.hidePaletteWindow).mockClear();
  vi.mocked(bridge.presentPaletteWindow).mockReset();
  vi.mocked(bridge.presentPaletteWindow).mockResolvedValue(true);
  vi.mocked(bridge.isWindowVisible).mockResolvedValue(false);
  vi.useFakeTimers();
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
    callback(0);
    return 1;
  });
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("launcher open/close motion", () => {
  it("starts hidden in Tauri and fades in only after present succeeds", async () => {
    render(<App />);
    expect(stage().className).toContain("launcher-stage-hidden");
    expect(stage().getAttribute("aria-hidden")).toBe("true");

    await openLauncher();

    expect(bridge.presentPaletteWindow).toHaveBeenCalledOnce();
    expect(stage().className).toContain("launcher-stage-visible");
    expect(stage().getAttribute("aria-hidden")).toBe("false");
  });

  it("plays a close transition on native hide without calling hide_palette", async () => {
    render(<App />);
    await openLauncher();

    await act(async () => {
      toggleHandler?.({ open: false });
    });

    expect(stage().className).toContain("launcher-stage-closing");
    expect(stage().getAttribute("aria-hidden")).toBe("true");
    expect(bridge.hidePaletteWindow).not.toHaveBeenCalled();

    await act(async () => {
      vi.advanceTimersByTime(LAUNCHER_MOTION_MS);
    });
    expect(stage().className).toContain("launcher-stage-hidden");
    expect(bridge.hidePaletteWindow).not.toHaveBeenCalled();
  });

  it("defers hide_palette on Escape so the fade can finish", async () => {
    render(<App />);
    await openLauncher();

    await act(async () => {
      document.dispatchEvent(new Event("prism:close"));
    });

    expect(stage().className).toContain("launcher-stage-closing");
    expect(bridge.hidePaletteWindow).toHaveBeenCalledTimes(1);
    expect(bridge.hidePaletteWindow).toHaveBeenCalledWith(true);

    await act(async () => {
      vi.advanceTimersByTime(LAUNCHER_MOTION_MS);
    });
    expect(stage().className).toContain("launcher-stage-hidden");
  });

  it("reopens from an in-flight close without waiting for the exit timer", async () => {
    render(<App />);
    await openLauncher();
    await act(async () => {
      toggleHandler?.({ open: false });
    });
    expect(stage().className).toContain("launcher-stage-closing");

    await openLauncher();

    expect(stage().className).toContain("launcher-stage-visible");
    expect(bridge.hidePaletteWindow).not.toHaveBeenCalled();
    await act(async () => {
      vi.advanceTimersByTime(LAUNCHER_MOTION_MS);
    });
    expect(stage().className).toContain("launcher-stage-visible");
  });

  it("plays the close transition on click-away while the native window is still visible", async () => {
    vi.mocked(bridge.isWindowVisible).mockResolvedValue(true);
    render(<App />);
    await openLauncher();

    await act(async () => {
      focusHandler?.(false);
    });
    await act(async () => {
      vi.advanceTimersByTime(60);
      await Promise.resolve();
    });

    expect(stage().className).toContain("launcher-stage-closing");
    expect(bridge.hidePaletteWindow).not.toHaveBeenCalled();
  });

  it("snaps hidden when focus is lost after an instant native hide", async () => {
    render(<App />);
    await openLauncher();
    vi.mocked(bridge.isWindowVisible).mockResolvedValue(false);

    await act(async () => {
      focusHandler?.(false);
    });
    await act(async () => {
      vi.advanceTimersByTime(60);
      await Promise.resolve();
    });

    expect(stage().className).toContain("launcher-stage-hidden");
    expect(bridge.hidePaletteWindow).not.toHaveBeenCalled();
  });

  it("hides immediately if native close arrives before the window is presented", async () => {
    let resolvePresent!: (value: boolean) => void;
    vi.mocked(bridge.presentPaletteWindow).mockReturnValue(
      new Promise((resolve) => {
        resolvePresent = resolve;
      }),
    );
    render(<App />);
    await act(async () => {
      toggleHandler?.({ open: true });
    });
    expect(stage().className).toContain("launcher-stage-preparing");

    await act(async () => {
      toggleHandler?.({ open: false });
    });
    expect(stage().className).toContain("launcher-stage-hidden");
    expect(bridge.hidePaletteWindow).not.toHaveBeenCalled();

    await act(async () => {
      resolvePresent(true);
      await Promise.resolve();
    });
    expect(stage().className).toContain("launcher-stage-hidden");
  });
});
