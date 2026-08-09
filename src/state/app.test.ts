import { describe, expect, it } from "vitest";
import { DEFAULT_SETTINGS, stepViewZoom } from "../lib/types";
import { SHORTCUT_OPTIONS, sanitizeHistory, sanitizeSettings } from "./app";

describe("default shortcut", () => {
  it("uses standalone Win for a fresh or invalid state", () => {
    expect(DEFAULT_SETTINGS.shortcut).toBe("Win");
    expect(SHORTCUT_OPTIONS[0]?.value).toBe("Win");
    expect(sanitizeSettings({}).shortcut).toBe("Win");
    expect(sanitizeSettings({ shortcut: "invalid" }).shortcut).toBe("Win");
  });

  it("preserves a valid shortcut selected by an existing user", () => {
    expect(sanitizeSettings({ shortcut: "Ctrl+Alt+Space" }).shortcut).toBe("Ctrl+Alt+Space");
  });
});

describe("view zoom", () => {
  it("steps within the supported range", () => {
    expect(stepViewZoom(100, 1)).toBe(110);
    expect(stepViewZoom(100, -1)).toBe(90);
    expect(stepViewZoom(150, 1)).toBe(150);
    expect(stepViewZoom(70, -1)).toBe(70);
  });

  it("sanitizes persisted zoom values and defaults older state", () => {
    expect(sanitizeSettings({ viewZoom: 130 }).viewZoom).toBe(130);
    expect(sanitizeSettings({ viewZoom: 135 }).viewZoom).toBe(DEFAULT_SETTINGS.viewZoom);
    expect(sanitizeSettings({}).viewZoom).toBe(DEFAULT_SETTINGS.viewZoom);
  });
});

describe("sanitizeHistory", () => {
  it("keeps valid unique entries and rejects malformed state", () => {
    expect(
      sanitizeHistory([
        { id: "file::d::C:\\Docs", title: "Docs", ts: 2 },
        { id: "file::d::C:\\Docs", title: "Duplicate", ts: 1 },
        { id: 42, title: "Invalid", ts: 0 },
        null,
      ]),
    ).toEqual([{ id: "file::d::C:\\Docs", title: "Docs", ts: 2 }]);
  });

  it("caps persisted history", () => {
    const entries = Array.from({ length: 30 }, (_, index) => ({
      id: `app::${index}`,
      title: `App ${index}`,
      ts: index,
    }));
    expect(sanitizeHistory(entries)).toHaveLength(20);
  });
});
