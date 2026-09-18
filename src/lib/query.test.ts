import { describe, expect, it } from "vitest";
import { EMPTY_PHASE1, phase1MatchesQuery } from "./query";

describe("phase1MatchesQuery", () => {
  it("requires the backend query to match the typed query", () => {
    expect(phase1MatchesQuery({ ...EMPTY_PHASE1, query: "Night Light" }, "night light")).toBe(true);
    expect(phase1MatchesQuery({ ...EMPTY_PHASE1, query: "old" }, "night light")).toBe(false);
    expect(phase1MatchesQuery(null, "night light")).toBe(false);
  });
});
