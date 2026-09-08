// @vitest-environment happy-dom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { Calculator } from "lucide-react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { FileSearchError, PaletteItem } from "../../lib/types";
import { DEFAULT_SETTINGS } from "../../lib/types";
import { useApp } from "../../state/app";
import { usePalette } from "../../state/palette";
import { Palette } from "./Palette";

vi.mock("../../state/app", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../state/app")>();
  return { ...actual, useApp: vi.fn() };
});

vi.mock("../../state/palette", () => ({ usePalette: vi.fn() }));
vi.mock("../../components/PowerMenu", () => ({ PowerMenu: () => null }));
vi.mock("../updater/UpdateControl", () => ({ UpdateControl: () => null }));

const firstItem: PaletteItem = {
  id: "app::calculator",
  title: "Calculator",
  subtitle: "Application",
  icon: { kind: "tile", icon: Calculator, tint: "iris" },
  run: vi.fn(),
  openLocation: vi.fn(),
  historyTitle: "Calculator",
  appId: "calculator",
};

const secondItem: PaletteItem = {
  id: "app::calendar",
  title: "Calendar",
  subtitle: "Application",
  icon: { kind: "tile", icon: Calculator, tint: "azure" },
  run: vi.fn(),
  historyTitle: "Calendar",
  appId: "calendar",
};

function renderPalette(options?: { fileError?: FileSearchError; keepItemsOnError?: boolean }) {
  const move = vi.fn();
  const runSelected = vi.fn();
  const setQuery = vi.fn();
  const rebuildIndex = vi.fn();
  const retryFileSearch = vi.fn();
  const items = options?.fileError && !options.keepItemsOnError ? [] : [firstItem, secondItem];
  const paletteValue: ReturnType<typeof usePalette> = {
    query: "cal",
    setQuery,
    sections: items.length > 0 ? [{ id: "apps", label: "Applications", items }] : [],
    flatItems: items,
    apps: [],
    selected: 0,
    move,
    select: vi.fn(),
    runSelected,
    runItem: vi.fn(),
    runItemAsAdmin: vi.fn(),
    appsLoaded: true,
    appsError: false,
    filesBusy: false,
    filesError: Boolean(options?.fileError),
    fileError: options?.fileError ?? null,
    fileIndexing: false,
    pathBrowsing: false,
    volumes: [],
    totalIndexed: 0,
    rebuildIndex,
    retryFileSearch,
    refreshApps: vi.fn(),
    reset: vi.fn(),
  };
  vi.mocked(usePalette).mockReturnValue(paletteValue);
  vi.mocked(useApp).mockReturnValue({
    ready: true,
    settings: DEFAULT_SETTINGS,
    updateSettings: vi.fn(),
    resetSettings: vi.fn(),
    openSettings: false,
    setOpenSettings: vi.fn(),
    history: [],
    pushHistory: vi.fn(),
    removeHistory: vi.fn(),
    clearHistory: vi.fn(),
    toasts: [],
    showToast: vi.fn(),
    dismissToast: vi.fn(),
    persistenceError: null,
    retryPersistence: vi.fn(),
    flushPersistence: vi.fn(),
    quit: vi.fn(),
  });

  const view = render(<Palette />);
  return { ...view, paletteValue, move, runSelected, setQuery, rebuildIndex, retryFileSearch };
}

describe("Palette keyboard and accessibility behavior", () => {
  beforeEach(() => {
    Element.prototype.scrollIntoView = vi.fn();
  });

  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  it("focuses the combobox and exposes its active result in the grid", () => {
    const { move, paletteValue, rerender } = renderPalette();

    const input = screen.getByRole("combobox", { name: /search files/i });
    const activeRow = screen.getByRole("row", { name: "Calculator, Application" });

    expect(document.activeElement).toBe(input);
    expect(input.getAttribute("aria-controls")).toBe("prism-results");
    expect(input.getAttribute("aria-activedescendant")).toBe(activeRow.id);
    expect(activeRow.getAttribute("aria-selected")).toBe("true");
    expect(screen.getByRole("grid", { name: "Search results" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Pin Calculator" })).toBeTruthy();

    fireEvent.keyDown(input, { key: "ArrowDown" });
    expect(move).toHaveBeenCalledWith(1);
    vi.mocked(usePalette).mockReturnValue({ ...paletteValue, selected: 1 });
    rerender(<Palette />);
    expect(input.getAttribute("aria-activedescendant")).toBe("prism-opt-1");
    expect(screen.getByRole("row", { name: "Calendar, Application" }).getAttribute("aria-selected")).toBe(
      "true",
    );
  });

  it("ignores launcher commands while IME composition is active", () => {
    const { move, runSelected, setQuery } = renderPalette();
    const input = screen.getByRole("combobox", { name: /search files/i });

    fireEvent.keyDown(input, { key: "Enter", isComposing: true });
    fireEvent.keyDown(input, { key: "ArrowDown", isComposing: true });
    fireEvent.keyDown(input, { key: "Escape", isComposing: true });

    expect(runSelected).not.toHaveBeenCalled();
    expect(move).not.toHaveBeenCalled();
    expect(setQuery).not.toHaveBeenCalled();

    fireEvent.keyDown(input, { key: "Enter" });
    expect(runSelected).toHaveBeenCalledOnce();
  });

  it("opens the selected result's named action menu from the combobox", async () => {
    renderPalette();
    const input = screen.getByRole("combobox", { name: /search files/i });

    fireEvent.keyDown(input, { key: "F10", shiftKey: true });

    expect(await screen.findByRole("menu", { name: "Calculator actions" })).toBeTruthy();
    expect(screen.getByRole("menuitem", { name: "Open location" })).toBeTruthy();
  });

  it("offers the matching recovery for each file-search failure", () => {
    const directory = renderPalette({ fileError: { kind: "directoryAccess", message: "denied" } });
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(directory.retryFileSearch).toHaveBeenCalledOnce();
    cleanup();

    const index = renderPalette({ fileError: { kind: "indexQuery", message: "query failed" } });
    fireEvent.click(screen.getByRole("button", { name: "Rebuild index" }));
    expect(index.rebuildIndex).toHaveBeenCalledOnce();
  });

  it("keeps file-search recovery visible when app matches remain", () => {
    const { rebuildIndex } = renderPalette({
      fileError: { kind: "indexQuery", message: "query failed" },
      keepItemsOnError: true,
    });

    expect(screen.getByRole("alert").textContent).toContain("File index search failed");
    fireEvent.click(screen.getByRole("button", { name: "Rebuild index" }));
    expect(rebuildIndex).toHaveBeenCalledOnce();
    expect(screen.getByRole("row", { name: "Calculator, Application" })).toBeTruthy();
  });
});
