# /// script
# requires-python = ">=3.12"
# dependencies = ["numpy", "pillow", "psutil"]
# ///
"""Session H journeys with the real Windows key (H1, H1t, H2, H2c). Runs ONLY on the agent desktop.

It sends a real VK_LWIN with SendInput, which goes to the whole desktop. That is safe only inside the
agent desktop (Windows Sandbox guest, user WDAGUtilityAccount); the script refuses to run anywhere else.

  uv run oploop/bench/winkey.py open   --build base=C:\\b\\prism.exe --build cand=C:\\c\\prism.exe --runs 20 --blocks 2
  uv run oploop/bench/winkey.py tail   --build base=... --delays 2 5 10 20 40
  uv run oploop/bench/winkey.py choose --build base=... --runs 10

Rows go to `oploop/results/raw/<stamp>-winkey-<mode>/rows.json`; `compare.py`-style tables come from `summary`.
"""
from __future__ import annotations

import argparse
import ctypes as C
import ctypes.wintypes as W
import json
import os
import statistics
import sys
import time
from pathlib import Path

if os.environ.get("USERNAME", "").lower() != "wdagutilityaccount":
    sys.exit("winkey.py sends a real Windows key: run it only on the agent desktop (sandbox guest).")

sys.path.insert(0, str(Path(__file__).parent))
import apps  # noqa: E402
import bench  # noqa: E402
import winapi  # noqa: E402

user32 = C.WinDLL("user32", use_last_error=True)
psapi = C.WinDLL("psapi", use_last_error=True)
kernel32 = C.WinDLL("kernel32", use_last_error=True)

VK_LWIN, VK_RETURN = 0x5B, 0x0D
KEYEVENTF_KEYUP, KEYEVENTF_EXTENDEDKEY = 0x2, 0x1


class KEYBDINPUT(C.Structure):
    _fields_ = [("wVk", W.WORD), ("wScan", W.WORD), ("dwFlags", W.DWORD), ("time", W.DWORD), ("dwExtraInfo", C.c_size_t)]


class MOUSEINPUT(C.Structure):
    _fields_ = [("dx", W.LONG), ("dy", W.LONG), ("mouseData", W.DWORD), ("dwFlags", W.DWORD), ("time", W.DWORD), ("dwExtraInfo", C.c_size_t)]


class _U(C.Union):
    _fields_ = [("ki", KEYBDINPUT), ("mi", MOUSEINPUT)]


class INPUT(C.Structure):
    _fields_ = [("type", W.DWORD), ("u", _U)]


def send_key(vk: int, extended: bool = False) -> None:
    flags = KEYEVENTF_EXTENDEDKEY if extended else 0
    down = INPUT(1, _U(ki=KEYBDINPUT(vk, 0, flags, 0, 0)))
    up = INPUT(1, _U(ki=KEYBDINPUT(vk, 0, flags | KEYEVENTF_KEYUP, 0, 0)))
    arr = (INPUT * 2)(down, up)
    if user32.SendInput(2, arr, C.sizeof(INPUT)) != 2:
        raise OSError(f"SendInput failed: {C.get_last_error()}")


def win_key() -> None:
    send_key(VK_LWIN, extended=True)


def trim(pid: int) -> None:
    """Moves the app's pages out of its working set, as Windows does under memory pressure."""
    PROCESS_SET_QUOTA, PROCESS_QUERY_INFORMATION = 0x0100, 0x0400
    handle = kernel32.OpenProcess(PROCESS_SET_QUOTA | PROCESS_QUERY_INFORMATION, False, pid)
    if not handle:
        raise OSError(f"OpenProcess failed: {C.get_last_error()}")
    try:
        if not psapi.EmptyWorkingSet(handle):
            raise OSError(f"EmptyWorkingSet failed: {C.get_last_error()}")
    finally:
        kernel32.CloseHandle(handle)


def purge_standby() -> None:
    """Drops the standby page list (admin; the guest account is one), so trimmed pages must come
    back from disk: the state after a game or a build used all the memory."""
    advapi32 = C.WinDLL("advapi32", use_last_error=True)
    ntdll = C.WinDLL("ntdll")

    class LUID(C.Structure):
        _fields_ = [("Low", W.DWORD), ("High", W.LONG)]

    class TOKEN_PRIVILEGES(C.Structure):
        _fields_ = [("Count", W.DWORD), ("Luid", LUID), ("Attributes", W.DWORD)]

    token = W.HANDLE()
    advapi32.OpenProcessToken(kernel32.GetCurrentProcess(), 0x0020 | 0x0008, C.byref(token))
    luid = LUID()
    advapi32.LookupPrivilegeValueW(None, "SeProfileSingleProcessPrivilege", C.byref(luid))
    tp = TOKEN_PRIVILEGES(1, luid, 0x2)
    advapi32.AdjustTokenPrivileges(token, False, C.byref(tp), 0, None, None)
    kernel32.CloseHandle(token)
    command = C.c_int(4)  # MemoryPurgeStandbyList
    status = ntdll.NtSetSystemInformation(80, C.byref(command), C.sizeof(command))  # SystemMemoryListInformation
    if status != 0:
        raise OSError(f"purge standby list: NTSTATUS 0x{status & 0xFFFFFFFF:08X}")


def memory_pressure(mib: int, hold_s: float = 2.0) -> None:
    """A separate process takes `mib` of RAM and touches every page, then exits: Windows trims and
    pages out the hidden app, as when a game or a build uses the memory."""
    import subprocess
    code = (f"import time; n={mib}*1048576; b=bytearray(n); b[::4096]=b'\\x01'*len(range(0,n,4096)); "
            f"time.sleep({hold_s})")
    subprocess.run([sys.executable, "-c", code], check=False)


def wait_gone(hwnd: int, t0: float, timeout_ms: float = 3000) -> float | None:
    while bench.now_ms() - t0 < timeout_ms:
        if not winapi.is_visible(hwnd):
            return bench.now_ms() - t0
        time.sleep(0.001)
    return None


def open_once(app: apps.TestApp, cap: winapi.Capturer, run: int, label: str) -> dict:
    """H1: Win key to the first painted frame; then H2c: Win key again to hidden."""
    if winapi.is_visible(app.palette):
        bench.close_palette(app)
    unix0 = time.time() * 1000
    t0 = bench.now_ms()
    win_key()
    first, settled, visible, frames, _ = bench.watch(cap, t0, None, quiet_ms=700, timeout_ms=6000)
    fg = winapi.foreground() == app.palette
    row = {"journey": "h1", "app": app.kind, "label": label, "run": run, "unix0": unix0, "visible_ms": visible, "first_ms": first,
           "settled_ms": settled, "frames": frames, "foreground": fg,
           "perf": bench.perf_window(app, unix0, unix0 + (settled or 0) + 50)}
    if first is not None:
        t1 = bench.now_ms()
        win_key()
        row["close_ms"] = wait_gone(app.palette, t1)
    time.sleep(0.6)
    return row


def cmd_open(args, out: Path, rows: list) -> None:
    for block in range(args.blocks):
        for kind in args.kinds:
            with apps.Session(kind, out, f"{kind}-b{block}") as app:
                app.find_type_target()
                bench.wait_ready(app)
                cap = winapi.Capturer(app.palette)
                try:
                    for run in range(args.runs):
                        row = open_once(app, cap, run, "warm" if run else "first")
                        row["block"] = block
                        rows.append(row)
                        apps.log(f"{kind} b{block} r{run}: first {row['first_ms']} close {row.get('close_ms')}")
                finally:
                    cap.close()


def cmd_tail(args, out: Path, rows: list) -> None:
    """H1t: one Win key at a fixed time after a start; then one after a working-set trim."""
    for block in range(args.blocks):
        for kind in args.kinds:
            for delay in args.delays:
                with apps.Session(kind, out, f"{kind}-d{delay}-b{block}") as app:
                    app.find_type_target()
                    started = psutil_create_time(app.pid)
                    cap = winapi.Capturer(app.palette)
                    try:
                        time.sleep(max(0.0, started + delay - time.time()))
                        row = open_once(app, cap, 0, f"after-start-{delay}s")
                        row["block"] = block
                        rows.append(row)
                        apps.log(f"{kind} delay {delay}s: first {row['first_ms']}")
                        if delay == args.delays[-1]:
                            bench.wait_ready(app, min_s=5)
                            trim(app.pid)
                            label = "after-trim"
                            if args.purge:
                                purge_standby()
                                label += "-purge"
                            if args.pressure:
                                memory_pressure(args.pressure)
                                label += f"-pressure{args.pressure}"
                            time.sleep(1.0)
                            row = open_once(app, cap, 1, label)
                            row["block"] = block
                            rows.append(row)
                            apps.log(f"{kind} after trim: first {row['first_ms']}")
                    finally:
                        cap.close()


def psutil_create_time(pid: int) -> float:
    import psutil
    return psutil.Process(pid).create_time()


def charmap_windows() -> set[int]:
    import psutil
    pids = {p.pid for p in psutil.process_iter(["name"]) if (p.info["name"] or "").lower() == "charmap.exe"}
    return {w.hwnd for w in winapi.top_windows(pids)} if pids else set()


def cmd_choose(args, out: Path, rows: list) -> None:
    """H2: Win key, type `charmap`, Enter; until the palette hides and Character Map shows."""
    import psutil
    for block in range(args.blocks):
        for kind in args.kinds:
            with apps.Session(kind, out, f"{kind}-choose-b{block}") as app:
                app.find_type_target()
                bench.wait_ready(app)
                cap = winapi.Capturer(app.palette)
                try:
                    for run in range(args.runs):
                        if winapi.is_visible(app.palette):
                            bench.close_palette(app)
                        t0 = bench.now_ms()
                        win_key()
                        first, _, _, _, _ = bench.watch(cap, t0, None, quiet_ms=300, timeout_ms=4000)
                        ref = bench.wait_quiet(cap, quiet_ms=300)
                        t_type = bench.now_ms()
                        app.type_text("charmap")
                        rows_done, _, _, _, _ = bench.watch(cap, t_type, ref, quiet_ms=300, timeout_ms=4000)
                        bench.wait_quiet(cap, quiet_ms=300)
                        before = charmap_windows()
                        t_enter = bench.now_ms()
                        winapi.post_key(app.type_target, VK_RETURN)
                        hidden = wait_gone(app.palette, t_enter, 4000)
                        shown = None
                        while bench.now_ms() - t_enter < 8000:
                            new = charmap_windows() - before
                            if any(winapi.is_visible(h) for h in new):
                                shown = bench.now_ms() - t_enter
                                break
                            time.sleep(0.005)
                        total = (t_enter - t0) + (shown or 0)
                        rows.append({"journey": "h2", "app": kind, "block": block, "run": run, "open_first_ms": first,
                                     "rows_done_ms": rows_done, "enter_to_hidden_ms": hidden,
                                     "enter_to_target_ms": shown, "win_to_target_ms": total})
                        apps.log(f"{kind} choose r{run}: open {first} rows {rows_done} hide {hidden} target {shown}")
                        for p in psutil.process_iter(["name"]):
                            if (p.info["name"] or "").lower() == "charmap.exe":
                                p.kill()
                        time.sleep(0.8)
                finally:
                    cap.close()


def summarize(rows: list) -> str:
    def stat(values: list[float]) -> str:
        v = sorted(x for x in values if x is not None)
        if not v:
            return "n=0"
        p95 = v[min(len(v) - 1, round(0.95 * (len(v) - 1)))]
        return f"median {statistics.median(v):.1f}  p95 {p95:.1f}  min {v[0]:.1f}  max {v[-1]:.1f}  n={len(v)}"
    lines = []
    keys = sorted({(r["journey"], r["app"], r.get("label", "")) for r in rows})
    for journey, app, label in keys:
        sel = [r for r in rows if (r["journey"], r["app"], r.get("label", "")) == (journey, app, label)]
        if journey == "h1":
            lines.append(f"{journey} {app} {label:16} first   {stat([r['first_ms'] for r in sel])}")
            lines.append(f"{journey} {app} {label:16} settled {stat([r['settled_ms'] for r in sel])}")
            lines.append(f"{journey} {app} {label:16} close   {stat([r.get('close_ms') for r in sel])}")
            lines.append(f"{journey} {app} {label:16} foreground {sum(1 for r in sel if r['foreground'])}/{len(sel)}")
        else:
            for k in ("open_first_ms", "rows_done_ms", "enter_to_hidden_ms", "enter_to_target_ms", "win_to_target_ms"):
                lines.append(f"{journey} {app} {k:20} {stat([r.get(k) for r in sel])}")
    return "\n".join(lines)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("mode", choices=["open", "tail", "choose", "summary"])
    parser.add_argument("--build", action="append", default=[], help="NAME=EXE")
    parser.add_argument("--runs", type=int, default=20)
    parser.add_argument("--blocks", type=int, default=2)
    parser.add_argument("--delays", type=float, nargs="+", default=[2, 5, 10, 20, 40])
    parser.add_argument("--rows", help="rows.json for summary")
    parser.add_argument("--purge", action="store_true", help="tail: also drop the standby list after the trim")
    parser.add_argument("--pressure", type=int, default=0, help="tail: MiB another process takes after the trim")
    args = parser.parse_args()
    if args.mode == "summary":
        print(summarize(json.loads(Path(args.rows).read_text(encoding="utf8"))))
        return
    args.kinds = []
    for spec in args.build:
        name, exe = spec.split("=", 1)
        apps.register_build(name, Path(exe))
        args.kinds.append(name)
    stamp = time.strftime("%Y%m%d-%H%M%S")
    out = Path(__file__).resolve().parents[1] / "results" / "raw" / f"{stamp}-winkey-{args.mode}"
    out.mkdir(parents=True, exist_ok=True)
    rows: list = []
    try:
        {"open": cmd_open, "tail": cmd_tail, "choose": cmd_choose}[args.mode](args, out, rows)
    finally:
        (out / "rows.json").write_text(json.dumps(rows, indent=1), encoding="utf8")
        print(f"rows: {out / 'rows.json'}")
        print(summarize(rows))


if __name__ == "__main__":
    main()
