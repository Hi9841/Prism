import { existsSync, readFileSync } from "node:fs";
import { describe, expect, test } from "vitest";

const read = (name: string) => readFileSync(new URL(name, import.meta.url), "utf8");

describe("Prism landing page", () => {
  test("ships the full visitor path and factual download actions", () => {
    const html = read("index.html");

    expect(html).toContain('id="hero-title" class="command-sequence"');
    expect(html).toContain('aria-label="Find. Open. Control."');
    expect(html).toContain('href="https://github.com/Hi9841/Prism/releases/latest"');
    expect(html).toContain('href="https://github.com/Hi9841/Prism"');
    expect(html).toContain("Windows 10 and 11");
    expect(html).toContain("SmartScreen");
    expect(html).toContain("<video");
    expect(html).toContain('id="demo"');
    expect(html).toContain('id="features"');
    expect(html).toContain('id="download"');
  });

  test("uses only real Prism captures for product imagery", () => {
    const html = read("index.html");

    expect(html).toContain("assets/prism-current.webp");
    expect(html).toContain("assets/prism-settings-real.webp");
    expect(html).toContain("assets/prism-quick-access-real.webp");
    expect(html).not.toContain("assets/search.png");
    expect(html).not.toContain("assets/calculate.png");
    expect(html).not.toContain("assets/customize.png");
  });

  test("loads optimized real media in viewport order", () => {
    const html = read("index.html");

    expect(html).toContain('rel="preload" as="image" href="assets/prism-current.webp"');
    expect(html).toContain('src="assets/prism-current.webp"');
    expect(html).toContain('poster="assets/prism-demo-poster-real.webp"');
    expect(html).toContain('src="assets/prism-settings-real.webp"');
    expect(html).toContain('src="assets/prism-quick-access-real.webp"');
    expect(html).toContain('preload="none"');
    expect(html.match(/loading="lazy"/g)).toHaveLength(2);
    expect(existsSync(new URL("assets/prism-current.webp", import.meta.url))).toBe(true);
    expect(existsSync(new URL("assets/prism-demo-poster-real.webp", import.meta.url))).toBe(true);
  });

  test("promotes the command rhythm hero without prototype chrome", () => {
    const html = read("index.html");
    const css = read("styles.css");

    expect(html).toContain('<span class="command-word motion-word word-1">Find.</span>');
    expect(html).toContain('<span class="command-word motion-word word-2">Open.</span>');
    expect(html).toContain('<span class="command-word motion-word word-3">Control.</span>');
    expect(html).not.toContain("proto-picker");
    expect(css).toContain("--ease-out: cubic-bezier(0.23, 1, 0.32, 1)");
    expect(css).toContain("text-wrap: balance");
    expect(css).toContain("text-wrap: pretty");
    expect(css).toContain("@keyframes word-in");
    expect(css).not.toContain("transition: all");
    expect(css).not.toContain("scale(0)");
  });

  test("refines interaction without skipping offscreen sections", () => {
    const html = read("index.html");
    const css = read("styles.css");

    expect(css).toContain(".button:active");
    expect(css).toContain("transform: scale(0.97)");
    expect(css).toContain(".platform-note span { margin-inline: 8px; color: inherit; }");
    expect(css).not.toContain("content-visibility: auto");
    expect(css).not.toContain("contain-intrinsic-size:");
    expect(css).not.toMatch(/font-size:\s*clamp\(/);
    expect(html).not.toMatch(/>0[1-6] \/ /);
  });

  test("keeps navigation and media accessible", () => {
    const html = read("index.html");
    const css = read("styles.css");
    const responsiveCss = read("responsive.css");

    expect(html).toContain('aria-label="Primary navigation"');
    expect(html).toContain('aria-label="Play product video"');
    expect(html).toContain('class="skip-link"');
    expect(css).toContain(":focus-visible");
    expect(css).toContain(".skip-link:focus-visible");
    expect(css).toContain("clip-path: inset(50%)");
    expect(css).toContain("prefers-reduced-motion: reduce");
    expect(css).toContain(".capture img { width: 100%; height: auto;");
    expect(responsiveCss).toContain("@media (max-width: 720px)");
  });

  test("adds video controls without hiding core content behind JavaScript", () => {
    const script = read("main.js");
    const css = read("styles.css");

    expect(script).toContain("video.play()");
    expect(script.match(/video\.play\(\)/g)).toHaveLength(1);
    expect(css).not.toContain(".js .reveal { opacity: 0");
  });
});
