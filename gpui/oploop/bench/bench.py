# /// script
# requires-python = ">=3.12"
# dependencies = ["numpy", "pillow", "psutil"]
# ///
"""End-to-end bench for the real Prism: GPUI build against the Tauri build.

    uv run gpui/oploop/bench/bench.py run [--j2-runs 20] [--j3-reps 20] [--blocks 2]
    uv run gpui/oploop/bench/bench.py j4 [--runs 3]
    uv run gpui/oploop/bench/bench.py restore

`run` measures J2 (open) and J3 (type) in interleaved blocks
(gpui, tauri, gpui, tauri, ...) and keeps the settled J3 frames for J5.
`j4` measures resources. Raw data goes to gpui/oploop/results/raw/<stamp>/.
`report.py` turns raw data into the results table. See JOURNEYS.md.
"""

from __future__ import annotations

import argparse
import json
import time
from pathlib import Path

import numpy as np
import psutil
from PIL import Image

import winapi
from apps import Session, TestApp, check_prism_json, log, prism_processes, start_installed, kill_tree

RESULTS = Path(__file__).resolve().parents[1] / "results" / "raw"

QUERIES = [
    "c", "ch", "chrome", "notepad", "uac", "settings", "calc", "2+2*3", "crhome",
    "HyperType-snippets.json", "apply_rose_pine_moon_accent.ps1", "problem solving practice.excalidraw",
    "C:\\Windows\\",
]

# A change counts when its changed columns span at least this many
# half-resolution pixels: the blinking caret is 1 px wide, a glyph is wider.
MIN_CHANGE_WIDTH = 4
MIN_CHANGE_PIXELS = 12


def significant(a: np.ndarray, b: np.ndarray) -> bool:
    count, width = winapi.change(a, b)
    return count >= MIN_CHANGE_PIXELS and width >= MIN_CHANGE_WIDTH


def now_ms() -> float:
    return time.perf_counter_ns() / 1e6


def watch(cap: winapi.Capturer, t0: float, reference: np.ndarray | None, quiet_ms: float, timeout_ms: float):
    """Captures until nothing changed for `quiet_ms` after the first change.

    Returns (first, settled, visible, frames, changes): `first` is the time of the
    first frame that differs from `reference` (or, with no reference, the
    first non-blank frame); `settled` is the time of the last frame that
    differed from the frame before it. Times are ms after `t0`.
    """
    first = settled = visible = None
    prev = None
    frames = 0
    changes: list[tuple] = []
    while True:
        t = now_ms() - t0
        if visible is None and winapi.is_visible(cap.hwnd):
            visible = t
        frame = cap.grab_small()
        t = now_ms() - t0
        frames += 1
        if first is None:
            changed = not winapi.is_blank(frame) if reference is None else significant(frame, reference)
            if changed:
                first = settled = t
        else:
            count, width, y0, y1 = winapi.change_box(frame, prev)
            if count >= MIN_CHANGE_PIXELS and width >= MIN_CHANGE_WIDTH:
                settled = t
                if len(changes) < 60:
                    # Rows in full-resolution pixels (frames are half size).
                    changes.append((round(t, 1), count, y0 * 2, y1 * 2))
        prev = frame
        if first is not None and t - settled >= quiet_ms:
            break
        if t >= timeout_ms:
            break
    return first, settled, visible, frames, changes


def wait_quiet(cap: winapi.Capturer, quiet_ms: float = 400, timeout_ms: float = 3000) -> np.ndarray:
    """Waits until the window content stops changing; returns the last frame."""
    start = now_ms()
    prev = cap.grab_small()
    last_change = now_ms()
    while now_ms() - last_change < quiet_ms and now_ms() - start < timeout_ms:
        frame = cap.grab_small()
        if significant(frame, prev):
            last_change = now_ms()
        prev = frame
    return prev


def wait_hidden(app: TestApp, timeout_s: float = 3.0) -> bool:
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        if not winapi.is_visible(app.palette):
            return True
        time.sleep(0.01)
    return False


def close_palette(app: TestApp) -> None:
    """Escape clears a query first, then closes. Never Enter."""
    for _ in range(3):
        if not winapi.is_visible(app.palette):
            break
        app.escape()
        if wait_hidden(app, 1.0):
            break
    time.sleep(0.6)  # let the exit animation and the hide timer end


def wait_ready(app: TestApp, min_s: float = 20, max_s: float = 120) -> float:
    """After start: wait until the app tree uses < 3% of one core for 3 s."""
    start = time.time()
    procs = app.processes()
    while time.time() - start < max_s:
        before = sum(p.cpu_times().user + p.cpu_times().system for p in procs if p.is_running())
        time.sleep(3)
        procs = app.processes()
        after = sum(p.cpu_times().user + p.cpu_times().system for p in procs if p.is_running())
        if time.time() - start >= min_s and (after - before) / 3 < 0.03:
            break
    return time.time() - start


def perf_window(app: TestApp, start_unix_ms: float, end_unix_ms: float) -> dict:
    """Sums the app's own backend timings that fell inside one measurement."""
    sums: dict[str, float] = {}
    for event in app.perf_events():
        if start_unix_ms - 5 <= event["unixMs"] <= end_unix_ms + 5:
            sums[event["event"]] = sums.get(event["event"], 0.0) + event["durationMs"]
    return sums


def j2_open(app: TestApp, cap: winapi.Capturer, run: int) -> dict:
    if winapi.is_visible(app.palette):
        close_palette(app)
    unix0 = time.time() * 1000
    t0 = now_ms()
    app.activate()
    first, settled, visible, frames, changes = watch(cap, t0, None, quiet_ms=700, timeout_ms=4000)
    took_foreground = winapi.foreground() == app.palette
    result = {
        "journey": "j2", "app": app.kind, "run": run, "visible_ms": visible, "first_ms": first,
        "settled_ms": settled, "frames": frames, "foreground": took_foreground, "changes": changes,
        "perf": perf_window(app, unix0, unix0 + (settled or 0) + 50),
    }
    return result


def j3_type(app: TestApp, cap: winapi.Capturer, query: str, rep: int, out: Path, save_frame: bool) -> dict:
    reference = wait_quiet(cap)
    unix0 = time.time() * 1000
    t0 = now_ms()
    app.type_text(query)
    first, settled, _, frames, changes = watch(cap, t0, reference, quiet_ms=1000, timeout_ms=5000)
    result = {
        "journey": "j3", "app": app.kind, "query": query, "rep": rep, "first_ms": first,
        "settled_ms": settled, "frames": frames, "changes": changes,
        "perf": perf_window(app, unix0, unix0 + (settled or 0) + 50),
    }
    if save_frame:
        frame = cap.grab()
        name = f"{app.kind}-q{QUERIES.index(query):02d}-r{rep:02d}.png"
        Image.fromarray(frame[..., [2, 1, 0, 3]], "RGBA").save(out / "frames" / name)
        result["frame"] = name
    # Clear the query (the first Escape) and keep the palette open.
    app.escape()
    wait_quiet(cap)
    return result


def block(kind: str, out: Path, tag: str, j2_runs: int, j3_reps: int, rep_offset: int, rows: list) -> None:
    with Session(kind, out, tag) as app:
        ready = wait_ready(app)
        log(f"{tag}: ready after {ready:.0f} s")
        cap = winapi.Capturer(app.palette)
        app.find_type_target()
        for run in range(j2_runs):
            load = psutil.cpu_percent(None)
            row = j2_open(app, cap, rep_offset + run)
            row["cold"] = run == 0
            row["machine_cpu_pct"] = load
            row["block"] = tag
            rows.append(row)
            log(f"{tag} j2 #{run}: first={row['first_ms']} settled={row['settled_ms']} fg={row['foreground']}")
            close_palette(app)
        # J3 starts from an open palette with an empty query.
        app.activate()
        wait_quiet(cap, quiet_ms=700)
        for rep in range(j3_reps):
            for query in QUERIES:
                if not winapi.is_visible(app.palette):
                    app.activate()
                    wait_quiet(cap, quiet_ms=700)
                load = psutil.cpu_percent(None)
                row = j3_type(app, cap, query, rep_offset + rep, out, save_frame=rep < 2)
                row["block"] = tag
                row["machine_cpu_pct"] = load
                rows.append(row)
            log(f"{tag} j3 rep {rep}: " + ", ".join(f"{r['query']}={r['settled_ms']:.0f}" for r in rows[-len(QUERIES):] if r["settled_ms"]))
        close_palette(app)
        cap.close()


def cmd_run(args) -> None:
    out = RESULTS / time.strftime("%Y%m%d-%H%M%S")
    (out / "frames").mkdir(parents=True)
    rows: list[dict] = []
    per_block_j2 = max(1, args.j2_runs // args.blocks)
    per_block_j3 = max(1, args.j3_reps // args.blocks)
    meta = {"queries": QUERIES, "blocks": args.blocks, "j2_runs": args.j2_runs, "j3_reps": args.j3_reps,
            "started": time.strftime("%Y-%m-%d %H:%M:%S")}
    try:
        for b in range(args.blocks):
            for kind in args.apps:
                block(kind, out, f"{kind}-b{b}", per_block_j2, per_block_j3, b * 1000, rows)
                (out / "rows.json").write_text(json.dumps(rows, indent=1), encoding="utf8")
    finally:
        meta["ended"] = time.strftime("%Y-%m-%d %H:%M:%S")
        (out / "meta.json").write_text(json.dumps(meta, indent=1), encoding="utf8")
        (out / "rows.json").write_text(json.dumps(rows, indent=1), encoding="utf8")
        log(f"raw data: {out}")


def tree_sample(app: TestApp) -> dict:
    procs = []
    for proc in app.processes():
        try:
            mem = proc.memory_info()
            cpu = proc.cpu_times()
            procs.append({
                "name": proc.name(), "pid": proc.pid, "private": mem.private, "wset": mem.rss,
                "handles": proc.num_handles(), "threads": proc.num_threads(), "cpu": cpu.user + cpu.system,
            })
        except psutil.NoSuchProcess:
            continue
    total = {k: sum(p[k] for p in procs) for k in ("private", "wset", "handles", "threads", "cpu")}
    total["processes"] = len(procs)
    total["detail"] = procs
    return total


def j4_once(kind: str, out: Path, run: int, idle_s: float, cycles: int) -> dict:
    with Session(kind, out, f"{kind}-j4-{run}") as app:
        start = time.time()
        time.sleep(idle_s)
        after_start = tree_sample(app)
        log(f"{kind} j4 #{run}: after {idle_s:.0f} s idle: private={after_start['private'] / 2**20:.0f} MiB cpu={after_start['cpu']:.2f} s")
        time.sleep(idle_s)
        after_idle2 = tree_sample(app)
        cap = winapi.Capturer(app.palette)
        app.find_type_target()
        for _ in range(cycles):
            app.activate()
            wait_quiet(cap, quiet_ms=300)
            app.type_text("chrome")
            wait_quiet(cap, quiet_ms=500)
            close_palette(app)
        cap.close()
        after_cycles = tree_sample(app)
        time.sleep(idle_s)
        after_cycles_idle = tree_sample(app)
        log(f"{kind} j4 #{run}: after {cycles} cycles + {idle_s:.0f} s idle: private={after_cycles_idle['private'] / 2**20:.0f} MiB")
        return {
            "journey": "j4", "app": kind, "run": run, "idle_s": idle_s, "cycles": cycles,
            "start_unix": start, "after_start": after_start, "after_idle2": after_idle2,
            "after_cycles": after_cycles, "after_cycles_idle": after_cycles_idle,
        }


def cmd_j4(args) -> None:
    out = RESULTS / (time.strftime("%Y%m%d-%H%M%S") + "-j4")
    out.mkdir(parents=True)
    rows = []
    try:
        for run in range(args.runs):
            for kind in args.apps:
                rows.append(j4_once(kind, out, run, args.idle, args.cycles))
                (out / "rows.json").write_text(json.dumps(rows, indent=1), encoding="utf8")
    finally:
        (out / "rows.json").write_text(json.dumps(rows, indent=1), encoding="utf8")
        log(f"raw data: {out}")


def cmd_restore(_args) -> None:
    """Emergency, right after a failed run only: stop every test build, start the installed
    Prism, and restore prism.json from that run's backup if a test build changed it."""
    for proc in prism_processes():
        if "PrismGpuiE2E" in (proc.exe() or ""):
            kill_tree(proc.pid)
    start_installed()
    log(f"prism.json unchanged: {check_prism_json()}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="cmd", required=True)
    run = sub.add_parser("run", help="J2 + J3 (+ J5 frames), interleaved blocks")
    run.add_argument("--j2-runs", type=int, default=20, help="open runs per app (split over blocks)")
    run.add_argument("--j3-reps", type=int, default=20, help="reps per query per app (split over blocks)")
    run.add_argument("--blocks", type=int, default=2)
    run.add_argument("--apps", nargs="+", default=["gpui", "tauri"])
    run.set_defaults(func=cmd_run)
    j4 = sub.add_parser("j4", help="resources: idle after start, then after open/type/close cycles")
    j4.add_argument("--runs", type=int, default=3)
    j4.add_argument("--idle", type=float, default=60)
    j4.add_argument("--cycles", type=int, default=20)
    j4.add_argument("--apps", nargs="+", default=["gpui", "tauri"])
    j4.set_defaults(func=cmd_j4)
    sub.add_parser("restore", help="restore the owner's setup").set_defaults(func=cmd_restore)
    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
