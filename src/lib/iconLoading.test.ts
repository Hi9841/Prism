import { describe, expect, it } from "vitest";
import { appIconRetryDelay, selectAppIconRequestIds } from "./iconLoading";

const state = (overrides: Partial<Parameters<typeof selectAppIconRequestIds>[1]> = {}) => ({
  appsLoaded: true,
  icons: {},
  settled: new Set<string>(),
  inFlight: new Set<string>(),
  attempts: new Map<string, number>(),
  ...overrides,
});

describe("app icon request policy", () => {
  it("waits for the native app index before requesting persisted pins", () => {
    expect(selectAppIconRequestIds(["app-a"], state({ appsLoaded: false }))).toEqual([]);
    expect(selectAppIconRequestIds(["app-a"], state())).toEqual(["app-a"]);
  });

  it("skips loaded, iconless, and in-flight apps", () => {
    expect(
      selectAppIconRequestIds(
        ["loaded", "iconless", "loading", "missing"],
        state({
          icons: { loaded: "data:image/png;base64,icon" },
          settled: new Set(["iconless"]),
          inFlight: new Set(["loading"]),
        }),
      ),
    ).toEqual(["missing"]);
  });

  it("bounds retries and supplies increasing delays", () => {
    expect(appIconRetryDelay(1)).toBe(200);
    expect(appIconRetryDelay(2)).toBe(800);
    expect(appIconRetryDelay(3)).toBeNull();
    expect(selectAppIconRequestIds(["app-a"], state({ attempts: new Map([["app-a", 2]]) }))).toEqual([
      "app-a",
    ]);
    expect(selectAppIconRequestIds(["app-a"], state({ attempts: new Map([["app-a", 3]]) }))).toEqual([]);
  });
});
