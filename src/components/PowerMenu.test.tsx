// @vitest-environment happy-dom

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { POPOVER_MOTION_MS } from "../lib/launcherMotion";
import { useApp } from "../state/app";
import { PowerMenu } from "./PowerMenu";

vi.mock("../state/app", () => ({ useApp: vi.fn() }));
vi.mock("../lib/bridge", () => ({ performPowerAction: vi.fn() }));

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  vi.useRealTimers();
});

describe("PowerMenu", () => {
  it("focuses the first action and exposes 44px menu targets", async () => {
    vi.mocked(useApp).mockReturnValue({
      openSettings: false,
      showToast: vi.fn(),
    } as unknown as ReturnType<typeof useApp>);
    render(<PowerMenu />);

    fireEvent.click(screen.getByRole("button", { name: "Power options" }));

    const actions = await screen.findAllByRole("menuitem");
    await waitFor(() => expect(document.activeElement).toBe(actions[0]));
    for (const action of actions) {
      expect(getComputedStyle(action).minHeight).toBe("44px");
    }
  });

  it("reverses an in-flight close when the trigger is pressed again", async () => {
    vi.mocked(useApp).mockReturnValue({
      openSettings: false,
      showToast: vi.fn(),
    } as unknown as ReturnType<typeof useApp>);
    render(<PowerMenu />);

    fireEvent.click(screen.getByRole("button", { name: "Power options" }));
    const menu = await screen.findByRole("menu");
    vi.useFakeTimers();
    fireEvent.click(screen.getByRole("button", { name: "Power options" }));
    expect(menu.className).toContain("power-menu-exit");
    expect(screen.getByRole("button", { name: "Power options" }).getAttribute("aria-expanded")).toBe(
      "false",
    );

    fireEvent.click(screen.getByRole("button", { name: "Power options" }));
    expect(screen.getByRole("menu").className).not.toContain("power-menu-exit");
    expect(screen.getByRole("button", { name: "Power options" }).getAttribute("aria-expanded")).toBe(
      "true",
    );

    await act(async () => {
      vi.advanceTimersByTime(POPOVER_MOTION_MS);
    });
    expect(screen.getByRole("menu")).toBeTruthy();
    vi.useRealTimers();
  });
});
