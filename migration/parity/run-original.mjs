// Captures one parity scenario from the original UI: the built frontend in
// headless Chrome 154 (the Chromium of WebView2 154) with the mocked Tauri
// layer answering from the fixture.
// Usage: bun run-original.mjs <fixture.json> <script.json> <scenario> <out-dir> <dist-dir>
import { spawn } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

const [fixturePath, scriptPath, scenarioName, outDir, distDir] = process.argv.slice(2);
const here = import.meta.dir;
const chrome = "C:/Program Files/Google/Chrome/Application/chrome.exe";
const fixture = JSON.parse(readFileSync(fixturePath, "utf8"));
const script = JSON.parse(readFileSync(scriptPath, "utf8"));
const scenario = script.scenarios.find((s) => s.name === scenarioName);
if (!scenario) throw new Error(`no scenario ${scenarioName}`);
mkdirSync(outDir, { recursive: true });

const backdrop = script.backdrop ?? "#1c1d22";
const hex = Number.parseInt(backdrop.slice(1), 16);
const color = { r: (hex >> 16) & 255, g: (hex >> 8) & 255, b: hex & 255, a: 1 };

// Static server for the built frontend.
const server = Bun.serve({
  port: 0,
  fetch(request) {
    const path = new URL(request.url).pathname;
    const file = join(distDir, path === "/" ? "index.html" : path);
    if (!existsSync(file)) return new Response("not found", { status: 404 });
    return new Response(Bun.file(file));
  },
});

const profile = resolve(here, "../../target/parity/chrome-profile", scenarioName);
rmSync(profile, { recursive: true, force: true });
mkdirSync(profile, { recursive: true });
const proc = spawn(
  chrome,
  ["--headless=new", "--no-first-run", `--user-data-dir=${profile}`, "--remote-debugging-port=0", ...(process.env.PARITY_CHROME_FLAGS ?? "").split(" ").filter(Boolean), "about:blank"],
  { stdio: "ignore" },
);
const portFile = join(profile, "DevToolsActivePort");
let port;
for (let i = 0; i < 200 && !port; i++) {
  await Bun.sleep(50);
  try {
    // Chrome may still hold the file open while it writes it.
    port = existsSync(portFile) ? readFileSync(portFile, "utf8").split("\n")[0] : undefined;
  } catch {
    port = undefined;
  }
}
const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
const ws = new WebSocket(targets.find((t) => t.type === "page").webSocketDebuggerUrl);
await new Promise((r) => (ws.onopen = r));
let id = 0;
const pending = new Map();
ws.onmessage = (m) => {
  const d = JSON.parse(m.data);
  if (d.id && pending.has(d.id)) {
    pending.get(d.id)(d);
    pending.delete(d.id);
  }
};
const cdp = (method, params = {}) =>
  new Promise((r) => {
    pending.set(++id, r);
    ws.send(JSON.stringify({ id, method, params }));
  });
const evaluate = async (expression) =>
  (await cdp("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true })).result?.result?.value;

const settings = fixture.state?.settings ?? {};
let width = [560, 640, 720].includes(settings.width) ? settings.width : 640;
let zoom = 1;
async function applyMetrics() {
  await cdp("Emulation.setDeviceMetricsOverride", {
    width: Math.round(width / zoom),
    height: Math.round(620 / zoom),
    deviceScaleFactor: zoom,
    mobile: false,
  });
}

await cdp("Page.enable");
await cdp("Runtime.enable");
await cdp("Emulation.setFocusEmulationEnabled", { enabled: true });
await cdp("Emulation.setDefaultBackgroundColorOverride", { color });
await applyMetrics();
const mock = readFileSync(join(here, "mock-tauri.js"), "utf8");
await cdp("Page.addScriptToEvaluateOnNewDocument", {
  source: `window.__PRISM_FIXTURE__ = ${JSON.stringify(fixture)};\n${mock}
document.addEventListener("DOMContentLoaded", () => {
  const style = document.createElement("style");
  style.textContent = "input { caret-color: transparent !important; }";
  document.head.appendChild(style);
});
${process.env.PARITY_DEBUG ? `for (const t of ["pointerdown","pointermove","pointerup","gotpointercapture","lostpointercapture","pointercancel"]) window.addEventListener(t, (e) => __prismMock.calls.push({ cmd: "#" + t, args: { x: e.clientX, y: e.clientY, b: e.buttons, id: e.pointerId, target: e.target.className?.toString().slice(0, 40) } }), true);` : ""}`,
});
await cdp("Page.navigate", { url: `http://127.0.0.1:${server.port}/` });
await Bun.sleep(400);

const KEYS = {
  up: ["ArrowUp", "ArrowUp", 38],
  down: ["ArrowDown", "ArrowDown", 40],
  left: ["ArrowLeft", "ArrowLeft", 37],
  right: ["ArrowRight", "ArrowRight", 39],
  enter: ["Enter", "Enter", 13],
  escape: ["Escape", "Escape", 27],
  backspace: ["Backspace", "Backspace", 8],
  tab: ["Tab", "Tab", 9],
  home: ["Home", "Home", 36],
  end: ["End", "End", 35],
  "=": ["=", "Equal", 187],
  "-": ["-", "Minus", 189],
  "0": ["0", "Digit0", 48],
  k: ["k", "KeyK", 75],
  w: ["w", "KeyW", 87],
  menu: ["ContextMenu", "ContextMenu", 93],
  space: [" ", "Space", 32],
};

async function key(spec) {
  const parts = spec.split("-");
  const name = parts.pop() || "-";
  let modifiers = 0;
  if (parts.includes("alt")) modifiers |= 1;
  if (parts.includes("ctrl")) modifiers |= 2;
  if (parts.includes("shift")) modifiers |= 8;
  const [k, code, vk] = KEYS[name] ?? [name, `Key${name.toUpperCase()}`, name.toUpperCase().charCodeAt(0)];
  // Enter and Space carry text so a focused button activates, as with a real key press.
  const text = modifiers === 0 ? { enter: "\r", space: " " }[name] : undefined;
  await cdp("Input.dispatchKeyEvent", text
    ? { type: "keyDown", key: k, code, windowsVirtualKeyCode: vk, modifiers, text, unmodifiedText: text }
    : { type: "rawKeyDown", key: k, code, windowsVirtualKeyCode: vk, modifiers });
  await cdp("Input.dispatchKeyEvent", { type: "keyUp", key: k, code, windowsVirtualKeyCode: vk, modifiers });
}

// A WebView zoom or width change resizes the native window.
async function syncMetrics() {
  const state = await evaluate("({ zoom: __prismMock.zoom, width: __prismMock.width })");
  if (state && (state.zoom !== zoom || (state.width && state.width !== width))) {
    zoom = state.zoom;
    if (state.width) width = state.width;
    await applyMetrics();
  }
}

let pressed = false;
for (const step of scenario.steps) {
  if (step.wait !== undefined) {
    // Follow native resizes while the UI animates.
    for (let left = step.wait; left > 0; left -= 50) {
      await Bun.sleep(Math.min(50, left));
      await syncMetrics();
    }
    continue;
  }
  if (step.open) {
    await evaluate(`__prismEmit("prism-toggle", { open: true, source: "winKey", anchor: null, generation: 1 })`);
  } else if (step.type !== undefined) {
    for (const ch of step.type) await cdp("Input.insertText", { text: ch });
  } else if (step.key) {
    await key(step.key);
  } else if (step.move) {
    await cdp("Input.dispatchMouseEvent", { type: "mouseMoved", x: step.move[0] / zoom, y: step.move[1] / zoom, buttons: pressed ? 1 : 0, button: pressed ? "left" : "none" });
  } else if (step.down) {
    pressed = true;
    await cdp("Input.dispatchMouseEvent", { type: "mouseMoved", x: step.down[0] / zoom, y: step.down[1] / zoom });
    await cdp("Input.dispatchMouseEvent", { type: "mousePressed", x: step.down[0] / zoom, y: step.down[1] / zoom, button: "left", buttons: 1, clickCount: 1 });
  } else if (step.up) {
    pressed = false;
    await cdp("Input.dispatchMouseEvent", { type: "mouseReleased", x: step.up[0] / zoom, y: step.up[1] / zoom, button: "left", buttons: 0, clickCount: 1 });
  } else if (step.click) {
    const button = step.button === "right" ? "right" : "left";
    const [x, y] = [step.click[0] / zoom, step.click[1] / zoom];
    await cdp("Input.dispatchMouseEvent", { type: "mouseMoved", x, y });
    await cdp("Input.dispatchMouseEvent", { type: "mousePressed", x, y, button, clickCount: 1, buttons: button === "right" ? 2 : 1 });
    await Bun.sleep(16);
    await cdp("Input.dispatchMouseEvent", { type: "mouseReleased", x, y, button, clickCount: 1 });
  } else if (step.wheel) {
    const [x, y, deltaY] = step.wheel;
    await cdp("Input.dispatchMouseEvent", {
      type: "mouseWheel",
      x: x / zoom,
      y: y / zoom,
      deltaX: 0,
      deltaY: deltaY / zoom,
      modifiers: step.ctrl ? 2 : 0,
    });
  } else if (step.capture) {
    await Bun.sleep(120);
    await syncMetrics();
    await Bun.sleep(60);
    const shot = await cdp("Page.captureScreenshot", { format: "png" });
    writeFileSync(join(outDir, `${step.capture}.png`), Buffer.from(shot.result.data, "base64"));
  } else if (step.eval) {
    // Debug probe: the value lands in the calls file as `#eval`.
    const value = await evaluate(step.eval);
    await evaluate(`__prismMock.calls.push({ cmd: "#eval", args: ${JSON.stringify(value ?? null)} })`);
  }
  await syncMetrics();
  await Bun.sleep(30);
}

const calls = await evaluate("JSON.stringify(__prismMock.calls)");
writeFileSync(join(outDir, `${scenarioName}.calls.json`), calls ?? "[]");
ws.close();
proc.kill();
server.stop(true);
