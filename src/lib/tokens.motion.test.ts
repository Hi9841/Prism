import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { LAUNCHER_MOTION_MS } from "./launcherMotion";

const css = readFileSync(resolve("src/styles/tokens.css"), "utf8");

describe("launcher motion tokens", () => {
  it("keeps launcher and related popovers on named, interruptible transitions", () => {
    expect(css).toContain(`--duration-fast: ${LAUNCHER_MOTION_MS}ms`);
    expect(css).toContain(".launcher-stage-closing");
    expect(css).toContain("transition-property: opacity, transform");
    expect(css).not.toContain("transition: all");
    expect(css).not.toContain("scale(0)");
    expect(css).not.toContain("animation: launcher-in");
    expect(css).not.toContain("animation: power-menu-in");
    expect(css).not.toContain("animation: context-menu-in");
  });

  it("does not use ease-in on launcher or related popover motion", () => {
    const launcherBlock = css.slice(css.indexOf(".launcher-stage {"), css.indexOf(".settings-backdrop"));
    const powerBlock = css.slice(css.indexOf(".power-menu {"), css.indexOf(".reorder-drag-preview-exit"));
    expect(launcherBlock).not.toContain("ease-in");
    expect(powerBlock).not.toContain("ease-in");
  });

  it("keeps a reduced-motion opacity fade for the launcher and related popovers", () => {
    expect(css).toContain("@media (prefers-reduced-motion: reduce)");
    expect(css).toContain(".launcher-stage,");
    expect(css).toContain(".power-menu,");
    expect(css).toContain(".context-menu-enter");
    expect(css).toContain("transition-duration: 100ms");
    expect(css).toContain("*:not(.launcher-stage):not(.power-menu):not(.context-menu-enter)");
  });
});
