import { describe, expect, it } from "vitest";
import { formatNumber, isMathLike, tryEvaluate } from "./math";

describe("tryEvaluate", () => {
  it("evaluates arithmetic", () => {
    expect(tryEvaluate("2 + 3")?.value).toBe(5);
    expect(tryEvaluate("12 * 8")?.value).toBe(96);
    expect(tryEvaluate("2^10")?.value).toBe(1024);
    expect(tryEvaluate("7 % 3")?.value).toBe(1);
    expect(tryEvaluate("(2 + 3) * 4")?.value).toBe(20);
  });

  it("handles unary minus and precedence", () => {
    expect(tryEvaluate("-2^2")?.value).toBe(-4);
    expect(tryEvaluate("2+3*4")?.value).toBe(14);
    expect(tryEvaluate("2*-3")?.value).toBe(-6);
    expect(tryEvaluate("10 / 4")?.value).toBe(2.5);
  });

  it("handles functions and constants", () => {
    expect(tryEvaluate("sqrt(16)")?.value).toBe(4);
    expect(tryEvaluate("abs(-5)")?.value).toBe(5);
    expect(tryEvaluate("pi * 2")?.value).toBeCloseTo(Math.PI * 2);
    expect(tryEvaluate("round(2.5)")?.value).toBe(3);
    expect(tryEvaluate("floor(2.9)")?.value).toBe(2);
  });

  it("accepts unicode operators", () => {
    expect(tryEvaluate("6 × 7")?.value).toBe(42);
    expect(tryEvaluate("8 ÷ 2")?.value).toBe(4);
    expect(tryEvaluate("√9")?.value).toBe(3);
    expect(tryEvaluate("π")?.value).toBeCloseTo(Math.PI);
  });

  it("rejects non-math input", () => {
    expect(tryEvaluate("hello world")).toBeNull();
    expect(tryEvaluate("")).toBeNull();
    expect(tryEvaluate("12*8; drop table")).toBeNull();
  });

  it("rejects malformed expressions", () => {
    expect(tryEvaluate("2 +")).toBeNull();
    expect(tryEvaluate("(2+3")).toBeNull();
    expect(tryEvaluate("2 +* 3")).toBeNull();
    expect(tryEvaluate("sqrt(16")).toBeNull();
  });

  it("formats results cleanly", () => {
    expect(formatNumber(42)).toBe("42");
    expect(formatNumber(2.5)).toBe("2.5");
    expect(formatNumber(1 / 3)).toBe("0.333333");
    expect(formatNumber(1 / 0)).toBe("Undefined");
  });
});

describe("isMathLike", () => {
  it("keeps literals and supported math tokens discoverable", () => {
    expect(isMathLike("42")).toBe(true);
    expect(isMathLike("sqrt(144)")).toBe(true);
    expect(isMathLike("pi * 2")).toBe(true);
    expect(isMathLike("6 × 7")).toBe(true);
    expect(isMathLike("√9")).toBe(true);
  });

  it("does not classify digit-bearing application names as math", () => {
    expect(isMathLike("vlc3")).toBe(false);
    expect(isMathLike("7zip")).toBe(false);
    expect(isMathLike("d3d12")).toBe(false);
  });
});
