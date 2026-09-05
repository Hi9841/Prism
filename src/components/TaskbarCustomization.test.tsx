// @vitest-environment happy-dom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { getTaskbarSettings } from "../lib/bridge";
import { TaskbarCustomization } from "./TaskbarCustomization";

const showToast = vi.hoisted(() => vi.fn());
vi.mock("../state/app", () => ({ useApp: () => ({ showToast }) }));
vi.mock("../lib/bridge", async (original) => ({
  ...(await original<typeof import("../lib/bridge")>()),
  getTaskbarSettings: vi.fn(),
}));

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

it("replaces a failed load with an accessible retry and restores settings after retry", async () => {
  vi.mocked(getTaskbarSettings).mockRejectedValueOnce(new Error("shell unavailable")).mockResolvedValueOnce({
    thickness: "default",
    autoHide: false,
    combineButtons: "always",
    startIcon: "system",
    selectedCustomIcon: null,
    customStartIcons: [],
  });
  render(<TaskbarCustomization />);
  const retry = await screen.findByRole("button", { name: "Retry taskbar settings" });
  expect(screen.getByRole("alert").textContent).toContain("Could not load taskbar settings");
  expect(screen.queryByText("Loading taskbar settings")).toBeNull();
  expect(showToast).toHaveBeenCalledWith(
    "Taskbar settings unavailable",
    expect.stringContaining("shell unavailable"),
    "error",
  );
  fireEvent.click(retry);
  await waitFor(() => expect(screen.getByText("Taskbar density")).toBeTruthy());
  expect(getTaskbarSettings).toHaveBeenCalledTimes(2);
  expect(screen.queryByRole("alert")).toBeNull();
});
