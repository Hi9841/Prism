import { describe, expect, it } from "vitest";
import { MIN_BACKGROUND_CHECK_INTERVAL_MS, shouldCheckForUpdate } from "./update-policy";

describe("update check policy", () => {
  it("throttles background checks", () => {
    expect(shouldCheckForUpdate(10_000, 10_000 + MIN_BACKGROUND_CHECK_INTERVAL_MS - 1, false)).toBe(false);
    expect(shouldCheckForUpdate(10_000, 10_000 + MIN_BACKGROUND_CHECK_INTERVAL_MS, false)).toBe(true);
  });

  it("always checks when the palette is explicitly opened", () => {
    expect(shouldCheckForUpdate(10_000, 10_000, true)).toBe(true);
  });
});
