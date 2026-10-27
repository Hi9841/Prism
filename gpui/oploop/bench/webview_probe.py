# /// script
# requires-python = ">=3.12"
# dependencies = ["numpy", "psutil", "websockets"]
# ///
"""Reads layout values from the real Tauri build's WebView2 (read-only CDP).

The harness runs the original in headless Chrome; this probe checks a value
against the engine that ships (WebView2). The test build starts hidden
(`--autostart`), so no window opens; the session restores the installed Prism.

    uv run gpui/oploop/bench/webview_probe.py [zoom]

With a zoom (for example 1.2), the probe first sets the WebView zoom the way
the app does (`set_webview_zoom`, not saved in the settings).
"""

from __future__ import annotations

import asyncio
import json
import sys
import time
import urllib.request
from pathlib import Path

import websockets

import apps

PORT = 9333
PROBE = r"""(() => {
  const d = document.createElement('div');
  d.className = 'scroll-thin';
  d.style.cssText = 'position:absolute;left:-9999px;top:0;width:100px;height:50px;overflow-y:scroll';
  d.innerHTML = '<div style="height:200px"></div>';
  document.body.appendChild(d);
  const thin = d.offsetWidth - d.clientWidth;
  d.className = '';
  const normal = d.offsetWidth - d.clientWidth;
  d.remove();
  return { thin, normal, dpr: devicePixelRatio, inner: [innerWidth, innerHeight], ua: navigator.userAgent };
})()"""


ZOOM = r"""window.__TAURI_INTERNALS__.invoke('plugin:webview|set_webview_zoom',
  { label: window.__TAURI_INTERNALS__.metadata.currentWebview.label, value: %s })
  .then(() => new Promise((done) => setTimeout(() => done('ok'), 500)))"""


async def evaluate(ws_url: str, expression: str) -> object:
    async with websockets.connect(ws_url, max_size=None) as ws:
        await ws.send(json.dumps({"id": 1, "method": "Runtime.evaluate",
                                  "params": {"expression": expression, "returnByValue": True,
                                             "awaitPromise": True}}))
        while True:
            reply = json.loads(await ws.recv())
            if reply.get("id") == 1:
                return reply["result"]["result"].get("value")


def page_target() -> str:
    for _ in range(100):
        try:
            with urllib.request.urlopen(f"http://127.0.0.1:{PORT}/json/list", timeout=1) as r:
                pages = [t for t in json.load(r) if t.get("type") == "page"]
            if pages:
                return pages[0]["webSocketDebuggerUrl"]
        except OSError:
            pass
        time.sleep(0.2)
    raise RuntimeError("no WebView2 page on the debugging port")


def main() -> None:
    base_env = apps.default_environment

    def with_debug_port(cache: Path) -> dict[str, str]:
        env = base_env(cache)
        env["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = f"--remote-debugging-port={PORT}"
        return env

    apps.default_environment = with_debug_port
    out = apps.E2E_DIR / "webview-probe"
    zoom = float(sys.argv[1]) if len(sys.argv) > 1 else None
    with apps.Session("tauri", out):
        target = page_target()
        if zoom is not None:
            print("zoom", asyncio.run(evaluate(target, ZOOM % zoom)))
        print(json.dumps(asyncio.run(evaluate(target, PROBE))))


if __name__ == "__main__":
    main()
