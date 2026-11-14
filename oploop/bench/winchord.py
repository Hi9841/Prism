# /// script
# requires-python = ">=3.12"
# dependencies = ["numpy", "pillow", "psutil"]
# ///
"""Win-key chord rules, end to end, with real key events. Runs ONLY on the agent desktop.

  uv run oploop/bench/winchord.py --build base=C:\\b\\prism.exe --build cand=C:\\c\\prism.exe [--reps 3]

For each case it sends a key sequence with SendInput and records whether the palette toggled and
whether an Explorer window opened (Win+E). Every build must give the same outcome for every case.
"""
from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import winkey  # noqa: E402  (refuses to run outside the guest)
import apps  # noqa: E402
import bench  # noqa: E402
import winapi  # noqa: E402

VK_LWIN, VK_E, VK_A, VK_CONTROL = 0x5B, 0x45, 0x41, 0x11
UP = winkey.KEYEVENTF_KEYUP


def key(vk: int, up: bool = False) -> winkey.INPUT:
    flags = (UP if up else 0) | (winkey.KEYEVENTF_EXTENDEDKEY if vk == VK_LWIN else 0)
    return winkey.INPUT(1, winkey._U(ki=winkey.KEYBDINPUT(vk, 0, flags, 0, 0)))


def send(*events) -> None:
    arr = (winkey.INPUT * len(events))(*events)
    winkey.user32.SendInput(len(events), arr, winkey.C.sizeof(winkey.INPUT))


def sequence(case: str) -> None:
    """Each step is a SendInput batch; numbers are sleeps in ms between batches."""
    steps = {
        "tap": [[key(VK_LWIN)], [key(VK_LWIN, True)]],
        "win+e": [[key(VK_LWIN)], [key(VK_E)], [key(VK_E, True)], [key(VK_LWIN, True)]],
        "win-up-first-0ms": [[key(VK_LWIN)], [key(VK_E)], [key(VK_LWIN, True), key(VK_E, True)]],
        "win-up-first-5ms": [[key(VK_LWIN)], [key(VK_E)], [key(VK_LWIN, True)], 5, [key(VK_E, True)]],
        "win-up-first-15ms": [[key(VK_LWIN)], [key(VK_E)], [key(VK_LWIN, True)], 15, [key(VK_E, True)]],
        "win-up-first-40ms": [[key(VK_LWIN)], [key(VK_E)], [key(VK_LWIN, True)], 40, [key(VK_E, True)]],
        "ctrl+win": [[key(VK_CONTROL)], [key(VK_LWIN)], [key(VK_LWIN, True)], [key(VK_CONTROL, True)]],
        "tap-then-a-5ms": [[key(VK_LWIN)], [key(VK_LWIN, True)], 5, [key(VK_A)], [key(VK_A, True)]],
    }[case]
    for step in steps:
        if isinstance(step, int):
            time.sleep(step / 1000)
        else:
            send(*step)
            time.sleep(0.002)


def explorer_windows() -> set[int]:
    import psutil
    pids = {p.pid for p in psutil.process_iter(["name"]) if (p.info["name"] or "").lower() == "explorer.exe"}
    return {w.hwnd for w in winapi.top_windows(pids) if w.cls == "CabinetWClass"}


def close_explorer(new: set[int]) -> None:
    for hwnd in new:
        winapi.user32.PostMessageW(hwnd, 0x0010, 0, 0)  # WM_CLOSE


CASES = ["tap", "win+e", "win-up-first-0ms", "win-up-first-5ms", "win-up-first-15ms", "win-up-first-40ms",
         "ctrl+win", "tap-then-a-5ms"]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--build", action="append", default=[])
    parser.add_argument("--reps", type=int, default=3)
    args = parser.parse_args()
    kinds = []
    for spec in args.build:
        name, exe = spec.split("=", 1)
        apps.register_build(name, Path(exe))
        kinds.append(name)
    out = Path(__file__).resolve().parents[1] / "results" / "raw" / f"{time.strftime('%Y%m%d-%H%M%S')}-winchord"
    out.mkdir(parents=True, exist_ok=True)
    results: dict[str, dict[str, list[str]]] = {}
    for kind in kinds:
        with apps.Session(kind, out, kind) as app:
            app.find_type_target()
            bench.wait_ready(app)
            for case in CASES:
                for rep in range(args.reps):
                    if winapi.is_visible(app.palette):
                        bench.close_palette(app)
                    before = explorer_windows()
                    time.sleep(0.3)
                    sequence(case)
                    time.sleep(0.8)
                    toggled = winapi.is_visible(app.palette)
                    new = explorer_windows() - before
                    outcome = ("palette" if toggled else "no-palette") + ("+explorer" if new else "")
                    results.setdefault(case, {}).setdefault(kind, []).append(outcome)
                    apps.log(f"{kind} {case} r{rep}: {outcome}")
                    close_explorer(new)
                    if toggled:
                        bench.close_palette(app)
                    time.sleep(0.5)
    (out / "results.json").write_text(json.dumps(results, indent=1), encoding="utf8")
    same = True
    for case in CASES:
        row = results.get(case, {})
        outcomes = {k: sorted(set(v)) for k, v in row.items()}
        agree = len({tuple(v) for v in outcomes.values()}) == 1
        same &= agree
        print(f"{case:20} {'same' if agree else 'DIFFERENT'}  {outcomes}")
    print("ALL SAME" if same else "DIFFERENCES FOUND")
    sys.exit(0 if same else 1)


if __name__ == "__main__":
    main()
