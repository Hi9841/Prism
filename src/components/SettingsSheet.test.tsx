// @vitest-environment happy-dom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setShortcut, setTaskbarAlignment } from "../lib/bridge";
import { DEFAULT_SETTINGS } from "../lib/types";
import { useApp } from "../state/app";
import { usePalette } from "../state/palette";
import { SettingsSheet } from "./SettingsSheet";

vi.mock("../state/app", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../state/app")>();
  return { ...actual, useApp: vi.fn() };
});
vi.mock("../state/palette", () => ({ usePalette: vi.fn() }));
vi.mock("./TaskbarCustomization", () => ({ TaskbarCustomization: () => null }));
vi.mock("../lib/bridge", () => ({
  getAppIcons: vi.fn().mockResolvedValue({}),
  getAppVersion: vi.fn().mockResolvedValue("0.9.38"),
  setShortcut: vi.fn().mockResolvedValue(undefined),
  setTaskbarAlignment: vi.fn().mockResolvedValue(undefined),
}));

function renderSettings(options?: { retryPersistence?: () => Promise<void>; quit?: () => Promise<void> }) {
  const retryPersistence = vi.fn(options?.retryPersistence ?? (() => Promise.resolve()));
  const quit = vi.fn(options?.quit ?? (() => Promise.resolve()));
  const updateSettings = vi.fn();
  vi.mocked(usePalette).mockReturnValue({
    query: "",
    setQuery: vi.fn(),
    sections: [],
    flatItems: [],
    apps: [],
    selected: 0,
    move: vi.fn(),
    select: vi.fn(),
    runSelected: vi.fn(),
    runItem: vi.fn(),
    runItemAsAdmin: vi.fn(),
    appsLoaded: true,
    appsError: false,
    filesBusy: false,
    filesError: false,
    fileError: null,
    fileIndexing: false,
    pathBrowsing: false,
    volumes: [],
    totalIndexed: 0,
    rebuildIndex: vi.fn(),
    retryFileSearch: vi.fn(),
    refreshApps: vi.fn(),
    reset: vi.fn(),
  });
  vi.mocked(useApp).mockReturnValue({
    ready: true,
    settings: DEFAULT_SETTINGS,
    updateSettings,
    resetSettings: vi.fn(),
    persistenceError: "Could not save settings. Check folder permissions.",
    retryPersistence,
    flushPersistence: vi.fn(),
    quit,
    openSettings: true,
    setOpenSettings: vi.fn(),
    history: [],
    pushHistory: vi.fn(),
    removeHistory: vi.fn(),
    clearHistory: vi.fn(),
    toasts: [],
    showToast: vi.fn(),
    dismissToast: vi.fn(),
  });
  render(<SettingsSheet />);
  return { retryPersistence, quit, updateSettings };
}

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("SettingsSheet persistence actions", () => {
  it.each([
    ["Ctrl + Alt + Space", setShortcut, "Shortcut not changed"],
    ["Left", setTaskbarAlignment, "Taskbar not changed"],
  ] as const)("reports a rejected %s change without persisting it", async (label, command, title) => {
    vi.mocked(command).mockRejectedValueOnce(new Error("shell unavailable"));
    const { updateSettings } = renderSettings();
    fireEvent.click(screen.getByRole("button", { name: label }));
    await waitFor(() =>
      expect(useApp().showToast).toHaveBeenCalledWith(
        title,
        expect.stringContaining("shell unavailable"),
        "error",
      ),
    );
    expect(updateSettings).not.toHaveBeenCalled();
  });
  it("keeps a save failure visible and disables Retry save while retrying", async () => {
    let resolveRetry = () => {};
    const pending = new Promise<void>((resolve) => {
      resolveRetry = resolve;
    });
    const { retryPersistence } = renderSettings({ retryPersistence: () => pending });

    expect(screen.getByRole("alert").textContent).toContain("Could not save settings");
    const retry = screen.getByRole("button", { name: "Retry save" });
    fireEvent.click(retry);

    expect(retryPersistence).toHaveBeenCalledOnce();
    expect(screen.getByRole("button", { name: "Saving..." }).getAttribute("aria-busy")).toBe("true");
    expect((screen.getByRole("button", { name: "Saving..." }) as HTMLButtonElement).disabled).toBe(true);

    resolveRetry();
    await waitFor(() =>
      expect((screen.getByRole("button", { name: "Retry save" }) as HTMLButtonElement).disabled).toBe(false),
    );
  });

  it("routes deliberate Quit through the app persistence action", () => {
    const { quit } = renderSettings();

    fireEvent.click(screen.getByRole("button", { name: "Quit" }));

    expect(quit).toHaveBeenCalledOnce();
  });

  it("focuses the close control and gives it a 44px hit area", () => {
    renderSettings();

    const closeButtons = screen.getAllByRole("button", { name: "Close settings" });
    expect(document.activeElement).toBe(closeButtons[1]);
    expect(getComputedStyle(closeButtons[1]).width).toBe("44px");
    expect(getComputedStyle(closeButtons[1]).height).toBe("44px");
  });

  it("does not create a collection or close the sheet while IME composition is active", () => {
    const { updateSettings } = renderSettings();
    const nameInput = screen.getByRole("textbox", { name: "New app collection name" });
    fireEvent.change(nameInput, { target: { value: "開発" } });

    fireEvent.keyDown(nameInput, { key: "Enter", isComposing: true });
    fireEvent.keyDown(nameInput, { key: "Escape", isComposing: true });

    expect(updateSettings).not.toHaveBeenCalled();
    expect(screen.getByRole("dialog", { name: "Settings" })).toBeTruthy();
  });
});
