import { describe, expect, it } from "vitest";
import { mergeTypeaheadQuery } from "./bridge";

describe("mergeTypeaheadQuery", () => {
  it("keeps the current query when nothing was buffered", () => {
    expect(mergeTypeaheadQuery("", "r")).toBe("r");
  });

  it("puts keys typed before focus in front of keys that already landed", () => {
    expect(mergeTypeaheadQuery("ch", "")).toBe("ch");
    expect(mergeTypeaheadQuery("ch", "r")).toBe("chr");
  });
});
