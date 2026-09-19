import { describe, expect, it } from "vitest";
import {
  hidePaletteInvocation,
  type LauncherEvent,
  type LauncherPhase,
  reduceLauncherPhase,
} from "./launcherMotion";

const phases: LauncherPhase[] = ["hidden", "preparing", "visible", "closing"];

describe("reduceLauncherPhase", () => {
  it("opens from rest through preparing, and interrupts a close back to visible", () => {
    expect(reduceLauncherPhase("hidden", "open")).toBe("preparing");
    expect(reduceLauncherPhase("preparing", "open")).toBe("preparing");
    expect(reduceLauncherPhase("closing", "open")).toBe("visible");
    expect(reduceLauncherPhase("visible", "open")).toBe("visible");
  });

  it("only commits presentation while preparing", () => {
    expect(reduceLauncherPhase("preparing", "presented")).toBe("visible");
    expect(reduceLauncherPhase("hidden", "presented")).toBe("hidden");
    expect(reduceLauncherPhase("closing", "presented")).toBe("closing");
    expect(reduceLauncherPhase("preparing", "present-failed")).toBe("hidden");
    expect(reduceLauncherPhase("visible", "present-failed")).toBe("hidden");
    expect(reduceLauncherPhase("closing", "present-failed")).toBe("closing");
  });

  it.each(["native-close", "web-close"] as const)(
    "%s hides a preparing window instantly and fades a visible one",
    (event: LauncherEvent) => {
      expect(reduceLauncherPhase("preparing", event)).toBe("hidden");
      expect(reduceLauncherPhase("visible", event)).toBe("closing");
      expect(reduceLauncherPhase("closing", event)).toBe("closing");
      expect(reduceLauncherPhase("hidden", event)).toBe("hidden");
    },
  );

  it("finishes an in-flight close and ignores the event on other phases", () => {
    expect(reduceLauncherPhase("closing", "exit-finished")).toBe("hidden");
    for (const phase of phases.filter((value) => value !== "closing")) {
      expect(reduceLauncherPhase(phase, "exit-finished")).toBe(phase);
    }
  });
});

describe("hidePaletteInvocation", () => {
  it("never hides from the webview on a native close; native delay owns the window", () => {
    expect(hidePaletteInvocation("native", "closing")).toBe("none");
    expect(hidePaletteInvocation("native", "hidden")).toBe("none");
  });

  it("defers Escape while fading and hides immediately if the window never appeared", () => {
    expect(hidePaletteInvocation("web", "closing")).toBe("deferred");
    expect(hidePaletteInvocation("web", "hidden")).toBe("instant");
    expect(hidePaletteInvocation("web", "visible")).toBe("none");
  });
});
