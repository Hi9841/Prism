// @vitest-environment happy-dom

import { describe, expect, it } from "vitest";
import type { AppEntry, HistoryEntry } from "../../lib/types";
import { buildMenuModel, FREQUENT_LIMIT, programGroupLabel } from "./menuModel";

function app(name: string, overrides: Partial<AppEntry> = {}): AppEntry {
  return {
    name,
    appId: name.toLowerCase(),
    location: undefined,
    ...overrides,
  };
}

const EMPTY_ICONS: Readonly<Record<string, string>> = {};

describe("programGroupLabel", () => {
  it("maps Start Menu shortcuts onto their folder", () => {
    expect(
      programGroupLabel({
        location: "C:\\ProgramData\\Microsoft\\Windows\\Start Menu\\Programs\\Accessories\\Note.lnk",
      }),
    ).toBe("Accessories");
  });

  it("joins nested folders with a separator", () => {
    expect(
      programGroupLabel({
        location: "C:\\ProgramData\\Microsoft\\Windows\\Start Menu\\Programs\\Games\\Board\\Chess.lnk",
      }),
    ).toBe("Games › Board");
  });

  it("keeps shortcuts directly under Programs in the root group", () => {
    expect(
      programGroupLabel({
        location: "C:\\Users\\hi\\AppData\\Roaming\\Microsoft\\Windows\\Start Menu\\Programs\\Tail.lnk",
      }),
    ).toBe("Programs");
  });

  it("normalizes forward slashes", () => {
    expect(programGroupLabel({ location: "C:/Start Menu/Programs/Tools/Scissors.lnk" })).toBe("Tools");
  });

  it("falls back to Other apps without a Start Menu location", () => {
    expect(programGroupLabel({ location: undefined })).toBe("Other apps");
    expect(programGroupLabel({ location: "C:\\Desktop\\Game.lnk" })).toBe("Other apps");
  });
});

describe("buildMenuModel", () => {
  it("resolves pinned tiles in the user's order", () => {
    const model = buildMenuModel({
      apps: [app("Alpha"), app("Beta")],
      pinnedApps: [app("Beta"), app("Alpha")],
      pinnedAppIds: ["beta", "alpha"],
      history: [],
      appIcons: EMPTY_ICONS,
    });
    expect(model.pinned.map((item) => item.title)).toEqual(["Beta", "Alpha"]);
  });

  it("ranks frequent apps by launch count, then recency, excluding pinned ids", () => {
    const history: HistoryEntry[] = [
      { id: "app::one", title: "One", ts: 400 },
      { id: "app::two", title: "Two", ts: 300 },
      { id: "app::one", title: "One", ts: 200 },
      { id: "app::pinned", title: "Pinned", ts: 100 },
      { id: "file::f::C:\\readme.txt", title: "readme", ts: 90 },
    ];
    const model = buildMenuModel({
      apps: [app("One"), app("Two"), app("Pinned")],
      pinnedApps: [],
      pinnedAppIds: ["pinned"],
      history,
      appIcons: EMPTY_ICONS,
    });
    expect(model.frequent.map((item) => item.appId)).toEqual(["one", "two"]);
  });

  it("caps the frequent list", () => {
    const apps = Array.from({ length: FREQUENT_LIMIT + 3 }, (_, i) => app(`App${i}`));
    const history: HistoryEntry[] = apps.map((entry, i) => ({
      id: `app::${entry.appId}`,
      title: entry.name,
      ts: i,
    }));
    const model = buildMenuModel({
      apps,
      pinnedApps: [],
      pinnedAppIds: [],
      history,
      appIcons: EMPTY_ICONS,
    });
    expect(model.frequent).toHaveLength(FREQUENT_LIMIT);
  });

  it("groups all programs by folder, sorted, with Other apps last", () => {
    const model = buildMenuModel({
      apps: [
        app("Zeta", { location: "C:\\SMP\\Programs\\Accessories\\Zeta.lnk" }),
        app("Alpha", { location: "C:\\SMP\\Programs\\Accessories\\Alpha.lnk" }),
        app("Standalone", { location: "C:\\SMP\\Programs\\Standalone.lnk" }),
        app("Mystery", { location: "C:\\Desktop\\Mystery.lnk" }),
      ],
      pinnedApps: [],
      pinnedAppIds: [],
      history: [],
      appIcons: EMPTY_ICONS,
    });
    expect(model.programGroups.map((group) => group.label)).toEqual([
      "Accessories",
      "Programs",
      "Other apps",
    ]);
    expect(model.programGroups[0].items.map((item) => item.title)).toEqual(["Alpha", "Zeta"]);
    // The flattened order matches the rendered group order.
    expect(model.programFlat.map((item) => item.title)).toEqual(["Alpha", "Zeta", "Standalone", "Mystery"]);
  });

  it("keeps Other apps rows in catalog order instead of re-sorting them", () => {
    const model = buildMenuModel({
      apps: [app("Zulu"), app("Alpha")],
      pinnedApps: [],
      pinnedAppIds: [],
      history: [],
      appIcons: EMPTY_ICONS,
    });
    expect(model.programFlat.map((item) => item.title)).toEqual(["Zulu", "Alpha"]);
  });
});
