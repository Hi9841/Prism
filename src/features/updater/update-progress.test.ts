import { describe, expect, it } from "vitest";
import { updatePercent } from "./update-progress";

describe("updatePercent", () => {
  it("reports rounded download progress", () => {
    expect(updatePercent(512, 1024)).toBe(50);
    expect(updatePercent(1, 3)).toBe(33);
  });

  it("clamps invalid and oversized progress", () => {
    expect(updatePercent(-1, 100)).toBe(0);
    expect(updatePercent(150, 100)).toBe(100);
    expect(updatePercent(50)).toBeNull();
    expect(updatePercent(50, 0)).toBeNull();
  });
});
