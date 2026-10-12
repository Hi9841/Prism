// M3 (zoom part): is WheelEvent.deltaY divided by the browser zoom?
// Headless Chrome, scratch profile with a default page zoom, a wheel event
// dispatched over CDP, and the page reports deltaY and devicePixelRatio.
import { mkdirSync, writeFileSync, readFileSync, existsSync, rmSync } from "node:fs";
import { join } from "node:path";
import { spawn } from "node:child_process";

const here = import.meta.dir;
const chrome = "C:/Program Files/Google/Chrome/Application/chrome.exe";
const page = `data:text/html,<script>window.seen=[];addEventListener('wheel',e=>{e.preventDefault();seen.push({deltaY:e.deltaY,deltaMode:e.deltaMode,dpr:devicePixelRatio})},{passive:false})</script><body style="height:3000px">x`;

async function run(zoomPercent) {
	const profile = join(here, `hl-zoom-${zoomPercent}`);
	rmSync(profile, { recursive: true, force: true });
	mkdirSync(join(profile, "Default"), { recursive: true });
	const level = Math.log(zoomPercent / 100) / Math.log(1.2); // Chromium zoom level
	writeFileSync(join(profile, "Default", "Preferences"),
		JSON.stringify({ partition: { default_zoom_level: { x: level } } }));
	const proc = spawn(chrome, ["--headless=new", "--disable-gpu", "--no-first-run",
		`--user-data-dir=${profile}`, "--remote-debugging-port=0", "about:blank"], { stdio: "ignore" });
	const portFile = join(profile, "DevToolsActivePort");
	for (let i = 0; i < 100 && !existsSync(portFile); i++) await Bun.sleep(100);
	const port = readFileSync(portFile, "utf8").split("\n")[0];
	const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
	const ws = new WebSocket(targets.find((t) => t.type === "page").webSocketDebuggerUrl);
	await new Promise((r) => (ws.onopen = r));
	let id = 0;
	const pending = new Map();
	ws.onmessage = (m) => { const d = JSON.parse(m.data); pending.get(d.id)?.(d); };
	const cdp = (method, params = {}) => new Promise((r) => { pending.set(++id, r); ws.send(JSON.stringify({ id, method, params })); });
	await cdp("Page.enable");
	await cdp("Page.navigate", { url: page });
	await Bun.sleep(800);
	await cdp("Input.dispatchMouseEvent", { type: "mouseWheel", x: 50, y: 50, deltaX: 0, deltaY: 100 });
	await Bun.sleep(300);
	const res = await cdp("Runtime.evaluate", { expression: "JSON.stringify({seen, dpr: devicePixelRatio})", returnByValue: true });
	ws.close();
	proc.kill();
	return { zoomPercent, ...JSON.parse(res.result.result.value) };
}

for (const z of [100, 150, 70]) console.log(JSON.stringify(await run(z)));
