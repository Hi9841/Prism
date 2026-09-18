import { describe, expect, it, vi } from "vitest";
import { executeAction, focusWindow } from "../../lib/bridge";
import type { Phase1Hit } from "../../lib/query";
import { actionPaletteItem, isCommandAction, windowPaletteItem } from "./phase1";

vi.mock("../../lib/bridge", () => ({
  executeAction: vi.fn().mockResolvedValue(undefined),
  focusWindow: vi.fn().mockResolvedValue(undefined),
  launchApp: vi.fn().mockResolvedValue(undefined),
  launchAppAsAdmin: vi.fn().mockResolvedValue(undefined),
}));

function hit(overrides: Partial<Phase1Hit> & Pick<Phase1Hit, "id" | "kind" | "title">): Phase1Hit {
  return {
    subtitle: "Windows Settings",
    score: 100,
    ...overrides,
  };
}

describe("phase1 mappers", () => {
  it("runs catalog actions through executeAction", async () => {
    const item = actionPaletteItem(
      hit({
        id: "action::audio.mute",
        kind: "action",
        title: "Mute",
        actionId: "audio.mute",
        iconKey: "mute",
      }),
    );
    await item.run();
    expect(vi.mocked(executeAction)).toHaveBeenCalledWith("audio.mute");
  });

  it("focuses an open window by hwnd", async () => {
    const item = windowPaletteItem(
      hit({
        id: "window::12",
        kind: "window",
        title: "Discord",
        hwnd: 12,
        iconKey: "window",
      }),
    );
    await item.run();
    expect(vi.mocked(focusWindow)).toHaveBeenCalledWith(12);
  });

  it("treats volume and power rows as commands, not settings pages", () => {
    expect(
      isCommandAction(
        hit({
          id: "action::audio.mute",
          kind: "action",
          title: "Mute",
          actionId: "audio.mute",
        }),
      ),
    ).toBe(true);
    expect(
      isCommandAction(
        hit({
          id: "action::settings.nightlight",
          kind: "action",
          title: "Night Light",
          actionId: "settings.nightlight",
          uri: "ms-settings:nightlight",
        }),
      ),
    ).toBe(false);
  });
});
