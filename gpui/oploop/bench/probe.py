# /// script
# requires-python = ">=3.12"
# dependencies = ["numpy", "pillow", "psutil"]
# ///
"""Smoke probe: start one build, open, type `c`, clear, close; save frames.

Use it after a product change that can break the bench (window class, key
handling). It prints the windows, the capture cost, and a short timeline.

    uv run gpui/oploop/bench/probe.py gpui
"""

from __future__ import annotations

import sys
import time
from pathlib import Path

from PIL import Image

import winapi
from apps import Session, log

kind = sys.argv[1]
out = Path(__file__).resolve().parents[1] / "results" / "raw" / "probe"
out.mkdir(parents=True, exist_ok=True)


def save(frame, name):
    Image.fromarray(frame[..., [2, 1, 0, 3]], "RGBA").save(out / f"{kind}-{name}.png")


def timeline(cap, seconds, label):
    t0 = time.perf_counter()
    prev = cap.grab_small()
    events = []
    n = 0
    while time.perf_counter() - t0 < seconds:
        cur = cap.grab_small()
        n += 1
        count, width = winapi.change(prev, cur)
        if count:
            events.append((round((time.perf_counter() - t0) * 1000, 1), count, width, winapi.is_visible(cap.hwnd)))
        prev = cur
    log(f"{label}: {n} captures in {seconds}s ({seconds * 1000 / n:.1f} ms each); changes: {events[:40]}")


with Session(kind, out) as app:
    time.sleep(8)
    for w in winapi.top_windows({app.pid}):
        log(f"top {w}")
    for c in winapi.child_windows(app.palette):
        log(f"child {c}")
    cap = winapi.Capturer(app.palette)
    log(f"palette {cap.w}x{cap.h} visible={winapi.is_visible(app.palette)}")
    save(cap.grab(), "0-hidden")
    app.activate()
    timeline(cap, 1.5, "open")
    save(cap.grab(), "1-open")
    log(f"foreground is palette: {winapi.foreground() == app.palette}")
    app.find_type_target()
    app.type_text("c")
    timeline(cap, 1.5, "type c")
    save(cap.grab(), "2-typed-c")
    app.escape()
    timeline(cap, 1.0, "escape 1")
    save(cap.grab(), "3-escape1")
    app.escape()
    timeline(cap, 1.0, "escape 2")
    log(f"visible after escape 2: {winapi.is_visible(app.palette)}")
    save(cap.grab(), "4-closed")
    cap.close()
    for e in app.perf_events():
        log(f"perf {e}")
