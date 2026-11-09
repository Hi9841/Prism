# /// script
# requires-python = ">=3.10"
# dependencies = ["psutil>=5.9"]
# ///
"""Idle cost of the UI layer alone: CPU and wakeups while the palette is open and while it is hidden.

    uv run oploop/bench/uiidle.py --a <base prism.exe> [--b <candidate prism.exe>] [--rounds 3]
        [--visible 6] [--hidden 20] [--out results.jsonl]

Each run starts `prism.exe --parity` (fixture data, a parked window, no backend and no hooks) on a
scenario that opens the palette, leaves it open, hides it with Escape, leaves it hidden, opens
it again, and types a query at once. The script watches the window from outside. In each quiet period it reads the CPU time
and the context switches of the process and the CPU time of each thread. A context switch is a
wakeup: a count that does not depend on the machine load. The last capture (120 ms after the typed
query) proves that the palette paints the result rows after the hidden period: all runs must give
the same image. With `--b`, the runs alternate A B B A.
"""

from __future__ import annotations

import argparse
import ctypes as C
import ctypes.wintypes as W
import hashlib
import json
import statistics
import subprocess
import sys
import time
from pathlib import Path

import psutil

sys.path.insert(0, str(Path(__file__).parent))
import threads as thread_census  # noqa: E402

REPO = Path(__file__).resolve().parents[2]
FIXTURE = REPO / ".agents/skills/verify-prism/fixtures/base.json"
WORK = REPO / "target/tmp/bench/uiidle"
user32 = C.WinDLL("user32", use_last_error=True)
ENUM_PROC = C.WINFUNCTYPE(W.BOOL, W.HWND, W.LPARAM)


def palette_window(pid: int) -> int | None:
    found: list[int] = []

    def visit(hwnd, _):
        owner = W.DWORD()
        user32.GetWindowThreadProcessId(hwnd, C.byref(owner))
        if owner.value == pid:
            name = C.create_unicode_buffer(64)
            user32.GetClassNameW(hwnd, name, 64)
            if name.value == "Zed::Window":
                found.append(hwnd)
        return True

    user32.EnumWindows(ENUM_PROC(visit), 0)
    return found[0] if found else None


def wait_for(condition, timeout: float, what: str) -> None:
    deadline = time.perf_counter() + timeout
    while time.perf_counter() < deadline:
        if condition():
            return
        time.sleep(0.01)
    raise SystemExit(f"timeout: {what}")


def quiet_period(process: psutil.Process, seconds: float) -> dict:
    """CPU (from the cycle counters, in ms at the nominal frequency) and context switches of a period."""
    rate = thread_census.cycles_per_second()
    mods = thread_census.modules(process.pid)
    threads_before = thread_census.thread_cycles(process.pid, mods)
    cycles_before, switches_before = thread_census.process_cycles(process.pid), process.num_ctx_switches().voluntary
    started = time.perf_counter()
    time.sleep(seconds)
    elapsed = time.perf_counter() - started
    cycles_after, switches_after = thread_census.process_cycles(process.pid), process.num_ctx_switches().voluntary
    threads_after = thread_census.thread_cycles(process.pid, mods)
    by_thread: dict[str, float] = {}
    for tid, (name, cycles) in threads_after.items():
        delta = cycles - threads_before.get(tid, ("", 0))[1]
        if delta > 0:
            label = name or "<unnamed>"
            by_thread[label] = by_thread.get(label, 0.0) + delta / rate * 1000 / elapsed
    return {
        "cpu_ms_per_s": (cycles_after - cycles_before) / rate * 1000 / elapsed,
        "switches_per_s": (switches_after - switches_before) / elapsed,
        "threads": len(threads_after),
        "by_thread_ms_per_s": dict(sorted(by_thread.items())),
    }


def run_once(exe: str, visible: float, hidden: float, label: str) -> dict:
    WORK.mkdir(parents=True, exist_ok=True)
    out = WORK / label
    out.mkdir(exist_ok=True)
    capture = out / "reopened.png"
    capture.unlink(missing_ok=True)
    script = WORK / f"{label}.json"
    steps = [
        {"open": True}, {"wait": int(visible * 1000) + 1500},
        {"key": "escape"}, {"wait": int(hidden * 1000) + 2500},
        # Typed at once after the show: the result rows arrive later and need a vsync frame.
        {"open": True}, {"type": "note"}, {"capture": "reopened"},
    ]
    script.write_text(json.dumps({"backdrop": "#1c1d22", "scenarios": [{"name": "idle", "steps": steps}]}))
    proc = subprocess.Popen([exe, "--parity", str(FIXTURE), str(script), "idle", str(out)],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        process = psutil.Process(proc.pid)
        window: list[int] = []
        wait_for(lambda: window.append(palette_window(proc.pid) or 0) or window[-1], 10, "palette window")
        hwnd = window[-1]
        wait_for(lambda: user32.IsWindowVisible(hwnd), 10, "palette visible")
        time.sleep(1.0)
        open_idle = quiet_period(process, visible)
        wait_for(lambda: not user32.IsWindowVisible(hwnd), 10, "palette hidden")
        time.sleep(1.0)
        hidden_idle = quiet_period(process, hidden)
        memory, handles = process.memory_full_info(), process.num_handles()
        wait_for(lambda: user32.IsWindowVisible(hwnd), 10, "palette visible again")
        shown = time.perf_counter()
        code = proc.wait(30)
    finally:
        if proc.poll() is None:
            proc.kill()
    return {
        "exit": code,
        "capture_bytes": capture.stat().st_size if capture.exists() else 0,
        "capture_sha": hashlib.sha256(capture.read_bytes()).hexdigest()[:16] if capture.exists() else "",
        "visible": open_idle,
        "hidden": hidden_idle,
        "private_mib": memory.private / 2**20,
        "wset_mib": memory.wset / 2**20,
        "handles": handles,
        "after_show_s": time.perf_counter() - shown,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--a", required=True)
    parser.add_argument("--b")
    parser.add_argument("--rounds", type=int, default=3)
    parser.add_argument("--visible", type=float, default=6.0)
    parser.add_argument("--hidden", type=float, default=20.0)
    parser.add_argument("--out")
    args = parser.parse_args()
    order = ("A", "B", "B", "A") if args.b else ("A",)
    runs: dict[str, list[dict]] = {"A": [], "B": []}
    for round_number in range(args.rounds):
        for side in order:
            exe = str(Path(args.a if side == "A" else args.b).resolve())
            result = run_once(exe, args.visible, args.hidden, f"{side}{round_number}{len(runs[side])}")
            runs[side].append(result)
            print(f"round {round_number} {side}: visible {result['visible']['cpu_ms_per_s']:.2f} ms/s, "
                  f"{result['visible']['switches_per_s']:.0f} switches/s; hidden {result['hidden']['cpu_ms_per_s']:.2f} ms/s, "
                  f"{result['hidden']['switches_per_s']:.0f} switches/s; threads {result['hidden']['threads']}; "
                  f"private {result['private_mib']:.1f} MiB; handles {result['handles']}; exit {result['exit']}, capture {result['capture_bytes']} B",
                  file=sys.stderr)
            if args.out:
                with open(args.out, "a", encoding="utf-8") as out:
                    out.write(json.dumps({"side": side, "exe": exe, **result}) + "\n")
    for side, items in runs.items():
        if not items:
            continue
        print(f"{side}: n={len(items)}")
        for phase in ("visible", "hidden"):
            cpu = [item[phase]["cpu_ms_per_s"] for item in items]
            switches = [item[phase]["switches_per_s"] for item in items]
            print(f"  {phase:8} CPU {statistics.median(cpu):7.2f} ms/s ({min(cpu):.2f}-{max(cpu):.2f}), "
                  f"switches {statistics.median(switches):7.0f} /s ({min(switches):.0f}-{max(switches):.0f})")
            names = sorted({name for item in items for name in item[phase]["by_thread_ms_per_s"]})
            for name in names:
                values = [item[phase]["by_thread_ms_per_s"].get(name, 0.0) for item in items]
                if statistics.median(values) >= 0.005:
                    print(f"           {statistics.median(values):7.3f} ms/s  {name}")
        errors = sum(1 for item in items if item["exit"] != 0 or item["capture_bytes"] < 2000)
        print(f"  errors (exit code or missing capture after the hidden period): {errors}")
    captures = {item["capture_sha"] for items in runs.values() for item in items}
    print(f"captures after the hidden period (open, type, 120 ms): {len(captures)} distinct image(s) over all runs")


if __name__ == "__main__":
    main()
