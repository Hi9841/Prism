"""J6: Enter on an app row to the app's window, through the running Prism.

usage: python launch_e2e.py <query> <window match> [reps] [--out FILE] [--kill IMAGE]

Each rep: Win key (SendInput) opens the palette, the query is typed, and after
the rows settle the bench sends Enter. It then polls every 2 ms for:
  hiddenMs   the palette window is hidden
  windowMs   the first new visible, titled, unowned top-level window whose
             process name or title contains <window match>
  focusMs    that window is the foreground window
  prismCpuMs / prismReadMiB / prismWriteMiB
             prism.exe CPU time and I/O from Enter to 3 s after windowMs
It closes the app window with WM_CLOSE (with --kill, it ends every IMAGE
process instead) and waits before the next rep.
The bench drives the real desktop: do not use the mouse or keyboard while it runs.
"""

import ctypes
import ctypes.wintypes as wt
import json
import sys
import time

user32 = ctypes.WinDLL("user32", use_last_error=True)
kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
dwmapi = ctypes.WinDLL("dwmapi")

ULONG_PTR = ctypes.c_size_t
SETTLE = float(__import__("os").environ.get("LAUNCH_SETTLE", "0.8"))


class KEYBDINPUT(ctypes.Structure):
    _fields_ = [("wVk", wt.WORD), ("wScan", wt.WORD), ("dwFlags", wt.DWORD), ("time", wt.DWORD), ("dwExtraInfo", ULONG_PTR)]


class MOUSEINPUT(ctypes.Structure):
    _fields_ = [("dx", wt.LONG), ("dy", wt.LONG), ("mouseData", wt.DWORD), ("dwFlags", wt.DWORD), ("time", wt.DWORD), ("dwExtraInfo", ULONG_PTR)]


class INPUTUNION(ctypes.Union):
    _fields_ = [("ki", KEYBDINPUT), ("mi", MOUSEINPUT)]


class INPUT(ctypes.Structure):
    _fields_ = [("type", wt.DWORD), ("u", INPUTUNION)]


KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE = 1, 2, 4
VK_RETURN, VK_LWIN, VK_ESCAPE = 0x0D, 0x5B, 0x1B
WM_CLOSE = 0x0010
DWMWA_CLOAKED = 14
GW_OWNER = 4
PROCESS_QUERY_LIMITED_INFORMATION = 0x1000

EnumWindowsProc = ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)
user32.EnumWindows.argtypes = [EnumWindowsProc, wt.LPARAM]
user32.GetWindow.restype = wt.HWND
user32.GetForegroundWindow.restype = wt.HWND
user32.PostMessageW.argtypes = [wt.HWND, wt.UINT, wt.WPARAM, wt.LPARAM]
kernel32.OpenProcess.restype = wt.HANDLE


class IO_COUNTERS(ctypes.Structure):
    _fields_ = [(name, ctypes.c_ulonglong) for name in ("ro", "wo", "oo", "rb", "wb", "ob")]


def prism_pid():
    import subprocess

    rows = subprocess.run(["tasklist", "/fi", "imagename eq prism.exe", "/fo", "csv", "/nh"], capture_output=True, text=True).stdout
    for row in rows.splitlines():
        cells = row.strip('"').split('","')
        if len(cells) > 1 and cells[0].lower() == "prism.exe":
            return int(cells[1])
    return None


def usage(pid):
    """(cpu ms, read bytes, write bytes) of one process."""
    handle = kernel32.OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, False, pid)
    if not handle:
        return (0.0, 0, 0)
    times = [wt.FILETIME() for _ in range(4)]
    kernel32.GetProcessTimes(handle, *[ctypes.byref(t) for t in times])
    io = IO_COUNTERS()
    kernel32.GetProcessIoCounters(handle, ctypes.byref(io))
    kernel32.CloseHandle(handle)
    ticks = lambda t: (t.dwHighDateTime << 32) | t.dwLowDateTime
    return ((ticks(times[2]) + ticks(times[3])) / 10_000, io.rb, io.wb)


def send_key(vk, up=False):
    flags = KEYEVENTF_KEYUP if up else 0
    if vk == VK_LWIN:
        flags |= KEYEVENTF_EXTENDEDKEY
    event = INPUT(type=1)
    event.u.ki = KEYBDINPUT(vk, 0, flags, 0, 0)
    user32.SendInput(1, ctypes.byref(event), ctypes.sizeof(INPUT))


def tap(vk):
    send_key(vk)
    send_key(vk, up=True)


def type_text(text):
    for char in text:
        for up in (False, True):
            event = INPUT(type=1)
            event.u.ki = KEYBDINPUT(0, ord(char), KEYEVENTF_UNICODE | (KEYEVENTF_KEYUP if up else 0), 0, 0)
            user32.SendInput(1, ctypes.byref(event), ctypes.sizeof(INPUT))
        time.sleep(0.015)


def pid_of(hwnd):
    pid = wt.DWORD()
    user32.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
    return pid.value


def process_name(pid):
    handle = kernel32.OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, False, pid)
    if not handle:
        return ""
    buffer = ctypes.create_unicode_buffer(1024)
    size = wt.DWORD(1024)
    ok = kernel32.QueryFullProcessImageNameW(handle, 0, buffer, ctypes.byref(size))
    kernel32.CloseHandle(handle)
    return buffer.value.rsplit("\\", 1)[-1].lower() if ok else ""


def title(hwnd):
    buffer = ctypes.create_unicode_buffer(256)
    user32.GetWindowTextW(hwnd, buffer, 256)
    return buffer.value


def cloaked(hwnd):
    value = wt.DWORD()
    dwmapi.DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, ctypes.byref(value), 4)
    return value.value != 0


def top_windows():
    found = []

    def collect(hwnd, _):
        if user32.IsWindowVisible(hwnd) and not cloaked(hwnd) and not user32.GetWindow(hwnd, GW_OWNER) and title(hwnd):
            found.append(hwnd)
        return True

    user32.EnumWindows(EnumWindowsProc(collect), 0)
    return found


def palette_visible():
    found = []

    def collect(hwnd, _):
        if user32.IsWindowVisible(hwnd) and not cloaked(hwnd) and process_name(pid_of(hwnd)) == "prism.exe":
            found.append(hwnd)
            return False
        return True

    user32.EnumWindows(EnumWindowsProc(collect), 0)
    return bool(found)


def wait(predicate, timeout):
    deadline = time.perf_counter() + timeout
    while time.perf_counter() < deadline:
        if predicate():
            return True
        time.sleep(0.005)
    return False


def close(hwnd):
    user32.PostMessageW(hwnd, WM_CLOSE, 0, 0)
    wait(lambda: not user32.IsWindow(hwnd), 5)


def run(query, matcher, reps, out, kill):
    import subprocess

    matcher = matcher.lower()
    prism = prism_pid()
    for rep in range(reps):
        if palette_visible():
            tap(VK_ESCAPE)
            tap(VK_ESCAPE)
            wait(lambda: not palette_visible(), 2)
        tap(VK_LWIN)
        if not wait(palette_visible, 3):
            print(json.dumps({"rep": rep, "error": "palette did not open"}), flush=True)
            continue
        time.sleep(0.4)
        type_text(query)
        time.sleep(SETTLE)
        before = set(top_windows())
        before_usage = usage(prism)
        started = time.perf_counter()
        tap(VK_RETURN)
        hidden = window = focus = None
        target = None
        while time.perf_counter() - started < 20:
            now = time.perf_counter() - started
            if hidden is None and not palette_visible():
                hidden = now
            if target is None:
                for hwnd in top_windows():
                    if hwnd not in before and (matcher in process_name(pid_of(hwnd)) or matcher in title(hwnd).lower()):
                        target, window = hwnd, now
                        break
            if target is not None and focus is None and user32.GetForegroundWindow() == target:
                focus = now
            if target is not None and hidden is not None and (focus is not None or now - window > 3):
                break
            time.sleep(0.002)
        if window is not None:
            time.sleep(max(0.0, window + 3 - (time.perf_counter() - started)))
        after_usage = usage(prism)
        ms = lambda value: None if value is None else round(value * 1000, 1)
        mib = lambda value: round(value / 1048576, 2)
        record = {
            "rep": rep,
            "query": query,
            "hiddenMs": ms(hidden),
            "windowMs": ms(window),
            "focusMs": ms(focus),
            "prismCpuMs": round(after_usage[0] - before_usage[0], 1),
            "prismReadMiB": mib(after_usage[1] - before_usage[1]),
            "prismWriteMiB": mib(after_usage[2] - before_usage[2]),
        }
        print(json.dumps(record), flush=True)
        if out:
            with open(out, "a", encoding="utf-8") as handle:
                handle.write(json.dumps(record) + "\n")
        if kill:
            subprocess.run(["taskkill", "/f", "/t", "/im", kill], capture_output=True)
        elif target is not None:
            close(target)
        time.sleep(3 if kill else 1.5)


if __name__ == "__main__":
    args = sys.argv[1:]
    options = {}
    for flag in ("--out", "--kill"):
        if flag in args:
            index = args.index(flag)
            options[flag] = args[index + 1]
            del args[index : index + 2]
    run(args[0], args[1], int(args[2]) if len(args) > 2 else 5, options.get("--out"), options.get("--kill"))
