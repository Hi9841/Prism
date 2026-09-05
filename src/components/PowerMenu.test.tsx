// @vitest-environment happy-dom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { performPowerAction } from "../lib/bridge";
import { useApp } from "../state/app";
import { PowerMenu } from "./PowerMenu";

vi.mock("../state/app", () => ({ useApp: vi.fn() }));
vi.mock("../lib/bridge", () => ({ performPowerAction: vi.fn() }));

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("PowerMenu", () => {
  it("keeps a rejected power action visible as an error", async () => {
    const showToast = vi.fn();
    vi.mocked(useApp).mockReturnValue({ openSettings: false, showToast } as unknown as ReturnType<
      typeof useApp
    >);
    vi.mocked(performPowerAction).mockRejectedValueOnce(new Error("access denied"));
    render(<PowerMenu />);
    fireEvent.click(screen.getByRole("button", { name: "Power options" }));
    fireEvent.click(screen.getByRole("menuitem", { name: /lock/i }));
    await waitFor(() =>
      expect(showToast).toHaveBeenCalledWith("Could not lock", "Error: access denied", "error"),
    );
    expect(screen.getByRole("menu")).toBeTruthy();
  });
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
});
