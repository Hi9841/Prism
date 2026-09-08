import { describe, expect, it } from "vitest";
import {
  isDowngrade,
  MIN_BACKGROUND_CHECK_INTERVAL_MS,
  MIN_FORCED_CHECK_INTERVAL_MS,
  shouldCheckForUpdate,
} from "./update-policy";

describe("update check policy", () => {
  it("throttles background checks", () => {
    expect(shouldCheckForUpdate(10_000, 10_000 + MIN_BACKGROUND_CHECK_INTERVAL_MS - 1, false)).toBe(false);
    expect(shouldCheckForUpdate(10_000, 10_000 + MIN_BACKGROUND_CHECK_INTERVAL_MS, false)).toBe(true);
  });

  it("allows open-driven checks after the forced floor", () => {
    expect(shouldCheckForUpdate(10_000, 10_000 + MIN_FORCED_CHECK_INTERVAL_MS - 1, true)).toBe(false);
    expect(shouldCheckForUpdate(10_000, 10_000 + MIN_FORCED_CHECK_INTERVAL_MS, true)).toBe(true);
  });
});

describe("downgrade detection", () => {
  it("detects an offered version below the installed one", () => {
    expect(isDowngrade("0.9.99", "0.9.50")).toBe(true);
    expect(isDowngrade("0.9.50", "0.9.99")).toBe(false);
    expect(isDowngrade("0.9.50", "0.10.0")).toBe(false);
    expect(isDowngrade("0.10.0", "0.9.55")).toBe(true);
  });

  it("treats an equal version and a leading v as not a downgrade", () => {
    expect(isDowngrade("0.9.50", "0.9.50")).toBe(false);
    expect(isDowngrade("v0.9.99", "0.9.50")).toBe(true);
    expect(isDowngrade("0.9.50", "v0.9.50")).toBe(false);
  });
});
