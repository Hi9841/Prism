import {
  Accessibility,
  Bluetooth,
  Clock,
  Gamepad2,
  Home,
  LayoutGrid,
  Monitor,
  Paintbrush,
  RefreshCw,
  Shield,
  UserRound,
  Wifi,
} from "lucide-react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { openWindowsSettings } from "../../lib/bridge";
import { searchWindowsSettings } from "./windowsSettings";

vi.mock("../../lib/bridge", () => ({
  openWindowsSettings: vi.fn(),
}));

const openWindowsSettingsMock = vi.mocked(openWindowsSettings);

describe("Windows Settings actions", () => {
  beforeEach(() => openWindowsSettingsMock.mockClear());

  it.each([
    ["display settings", "ms-settings:display"],
    ["bluetooth", "ms-settings:bluetooth"],
    ["windows update", "ms-settings:windowsupdate"],
    ["default apps", "ms-settings:defaultapps"],
    ["microphone", "ms-settings:privacy-microphone"],
  ])("opens the documented target for %s", async (query, uri) => {
    const result = searchWindowsSettings(query)[0];

    await result.run();

    expect(openWindowsSettingsMock).toHaveBeenCalledOnce();
    expect(openWindowsSettingsMock).toHaveBeenCalledWith(uri);
  });

  it.each([
    ["settings", Home],
    ["display", Monitor],
    ["bluetooth", Bluetooth],
    ["network & internet", Wifi],
    ["background", Paintbrush],
    ["installed apps", LayoutGrid],
    ["your info", UserRound],
    ["date & time", Clock],
    ["game mode", Gamepad2],
    ["narrator", Accessibility],
    ["privacy & security", Shield],
    ["windows update", RefreshCw],
  ])("uses the Windows category icon for %s", (query, expectedIcon) => {
    const result = searchWindowsSettings(query)[0];

    expect(result.icon).toMatchObject({ kind: "tile", icon: expectedIcon, tint: "azure" });
  });
});
