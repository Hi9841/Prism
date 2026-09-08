// @vitest-environment happy-dom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { act, createRef } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_SETTINGS } from "../../lib/types";
import { useApp } from "../../state/app";
import { usePalette } from "../../state/palette";
import { Palette } from "./Palette";
import { StartMenu, type StartMenuHandle } from "./StartMenu";

vi.mock("../../state/app", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../state/app")>();
  return { ...actual, useApp: vi.fn() };
});

vi.mock("../../state/palette", () => ({ usePalette: vi.fn() }));
vi.mock("../../components/PowerMenu", () => ({ PowerMenu: () => null }));
vi.mock("../updater/UpdateControl", () => ({ UpdateControl: () => null }));

const apps = [
  { name: "Zeta", appId: "zeta", location: "C:\\SMP\\Programs\\Accessories\\Zeta.lnk" },
  { name: "Alpha", appId: "alpha", location: "C:\\SMP\\Programs\\Accessories\\Alpha.lnk" },
  { name: "Standalone", appId: "standalone", location: "C:\\SMP\\Programs\\Standalone.lnk" },
  { name: "Mystery", appId: "mystery", location: "C:\\Desktop\\Mystery.lnk" },
];

function mockContexts(options?: {
  startView?: "palette" | "menu";
  pinnedApps?: string[];
  history?: { id: string; title: string; ts: number }[];
}) {
  const runItem = vi.fn();
  const updateSettings = vi.fn();
  const paletteValue: ReturnType<typeof usePalette> = {
    query: "",
    setQuery: vi.fn(),
    sections: [],
    flatItems: [],
    apps,
    quickItems: [],
    appIcons: {},
    selected: 0,
    move: vi.fn(),
    select: vi.fn(),
    runSelected: vi.fn(),
    runItem,
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
  };
  vi.mocked(usePalette).mockReturnValue(paletteValue);
  vi.mocked(useApp).mockReturnValue({
    ready: true,
    settings: {
      ...DEFAULT_SETTINGS,
      startView: options?.startView ?? "menu",
      pinnedApps: options?.pinnedApps ?? ["alpha"],
    },
    updateSettings,
    resetSettings: vi.fn(),
    openSettings: false,
    setOpenSettings: vi.fn(),
    history: options?.history ?? [],
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
  return { runItem, updateSettings, paletteValue };
}

function renderStartMenu(options?: {
  pinnedApps?: string[];
  history?: { id: string; title: string; ts: number }[];
}) {
  const mocks = mockContexts(options);
  const ref = createRef<StartMenuHandle>();
  const view = render(<StartMenu ref={ref} />);
  return { ...view, ...mocks, ref };
}

describe("StartMenu", () => {
  beforeEach(() => {
    Element.prototype.scrollIntoView = vi.fn();
  });

  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  it("renders pinned tiles and the All apps folder groups", () => {
    renderStartMenu();

    expect(screen.getByRole("button", { name: /Alpha/ })).toBeTruthy();
    expect(screen.getByText("Accessories")).toBeTruthy();
    expect(screen.getByText("Programs")).toBeTruthy();
    expect(screen.getByText("Other apps")).toBeTruthy();
  });

  it("runs an app when its row is clicked", () => {
    const { runItem } = renderStartMenu();

    fireEvent.click(screen.getByRole("option", { name: /Standalone/ }));

    expect(runItem).toHaveBeenCalledOnce();
    expect(runItem.mock.calls[0][0].appId).toBe("standalone");
  });

  it("surfaces frequent apps from history, excluding pinned ids", () => {
    renderStartMenu({
      history: [
        { id: "app::standalone", title: "Standalone", ts: 10 },
        { id: "app::alpha", title: "Alpha", ts: 5 },
        { id: "app::zeta", title: "Zeta", ts: 1 },
      ],
    });

    // Zeta appears once in Frequent and once in All apps; pinned Alpha only
    // appears in its pinned tile and All apps, never as a frequent row twice.
    expect(screen.getAllByText("Zeta").length).toBe(2);
  });

  it("navigates and runs through the imperative handle", () => {
    const { ref, runItem } = renderStartMenu();

    act(() => ref.current?.move(1));
    act(() => ref.current?.runSelected());
    // Flattened order: Alpha, Zeta (Accessories), Standalone (Programs), Mystery (Other).
    expect(runItem.mock.calls[0][0].appId).toBe("zeta");

    act(() => ref.current?.jump("last"));
    act(() => ref.current?.runSelected());
    expect(runItem.mock.calls[1][0].appId).toBe("mystery");
  });

  it("pins an app from its context menu", () => {
    const { updateSettings } = renderStartMenu();

    const row = screen.getByRole("option", { name: /Standalone/ });
    fireEvent.contextMenu(row);
    fireEvent.click(screen.getByRole("menuitem", { name: "Pin" }));

    expect(updateSettings).toHaveBeenCalledWith({ pinnedApps: ["alpha", "standalone"] });
  });
});

describe("Palette start-menu integration", () => {
  beforeEach(() => {
    Element.prototype.scrollIntoView = vi.fn();
  });

  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  function renderPaletteInView(startView: "palette" | "menu") {
    const { updateSettings } = mockContexts({ startView, pinnedApps: [] });
    const view = render(<Palette />);
    return { ...view, updateSettings };
  }

  it("shows the menu view instead of results when startView is menu", () => {
    renderPaletteInView("menu");
    expect(screen.getByTestId("start-menu")).toBeTruthy();
    expect(screen.queryByRole("grid", { name: "Search results" })).toBeNull();
  });

  it("keeps the results grid when startView is palette", () => {
    renderPaletteInView("palette");
    expect(screen.queryByTestId("start-menu")).toBeNull();
  });

  it("toggles the start view from the footer", () => {
    const { updateSettings } = renderPaletteInView("palette");

    fireEvent.click(screen.getByRole("button", { name: "Switch to menu view" }));
    expect(updateSettings).toHaveBeenCalledWith({ startView: "menu" });

    cleanup();
    const rerendered = renderPaletteInView("menu");
    fireEvent.click(screen.getByRole("button", { name: "Switch to list view" }));
    expect(rerendered.updateSettings).toHaveBeenCalledWith({ startView: "palette" });
  });
});
