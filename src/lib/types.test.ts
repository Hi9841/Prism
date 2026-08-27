import { describe, expect, it } from "vitest";
import { isElevatablePath, isTaskbarPinablePath } from "./types";

describe("isTaskbarPinablePath", () => {
  it("accepts launchable shortcuts and executables", () => {
    expect(isTaskbarPinablePath("C:\\Apps\\Tool.exe")).toBe(true);
    expect(isTaskbarPinablePath("C:\\Users\\You\\AppData\\Local\\lnk.lnk")).toBe(true);
    expect(isTaskbarPinablePath("C:\\Windows\\System32\\dsa.msc")).toBe(true);
    expect(isTaskbarPinablePath("C:\\Scripts\\job.CMD")).toBe(true);
  });

  it("rejects documents, folders, and non-filesystem targets", () => {
    expect(isTaskbarPinablePath("C:\\Users\\You\\Documents\\report.pdf")).toBe(false);
    expect(isTaskbarPinablePath("C:\\Apps\\no-extension")).toBe(false);
    expect(isTaskbarPinablePath("https://example.com")).toBe(false);
    expect(isTaskbarPinablePath("shell:AppsFolder\\12345!App")).toBe(false);
    expect(isTaskbarPinablePath(undefined)).toBe(false);
    expect(isTaskbarPinablePath("")).toBe(false);
  });

  it("leaves elevation eligibility untouched", () => {
    expect(isElevatablePath("C:\\Apps\\Tool.exe")).toBe(true);
    expect(isElevatablePath("C:\\Apps\\Tool.exe.txt")).toBe(false);
    expect(isElevatablePath("C:\\Windows\\System32\\dsa.msc")).toBe(false);
  });
});
