import { describe, expect, it } from "vitest";
import { isElevatablePath, isPicturePath, isTaskbarPinablePath } from "./types";

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

describe("isPicturePath", () => {
  it("identifies supported image and picture extensions", () => {
    expect(isPicturePath("C:\\Photos\\vacation.png")).toBe(true);
    expect(isPicturePath("C:\\Photos\\portrait.JPG")).toBe(true);
    expect(isPicturePath("C:\\Pictures\\image.jpeg")).toBe(true);
    expect(isPicturePath("C:\\Pictures\\banner.webp")).toBe(true);
    expect(isPicturePath("C:\\Icons\\app.ico")).toBe(true);
    expect(isPicturePath("C:\\Vector\\logo.svg")).toBe(true);
    expect(isPicturePath("C:\\Raw\\sample.avif")).toBe(true);
    expect(isPicturePath("C:\\Photos\\shot.heic")).toBe(true);
    expect(isPicturePath("C:\\Graphics\\anim.gif")).toBe(true);
  });

  it("rejects non-picture files and invalid targets", () => {
    expect(isPicturePath("C:\\Docs\\report.pdf")).toBe(false);
    expect(isPicturePath("C:\\Videos\\clip.mp4")).toBe(false);
    expect(isPicturePath("C:\\Music\\song.mp3")).toBe(false);
    expect(isPicturePath("C:\\Apps\\program.exe")).toBe(false);
    expect(isPicturePath("https://example.com/photo.png")).toBe(false);
    expect(isPicturePath("")).toBe(false);
    expect(isPicturePath(undefined)).toBe(false);
  });
});
