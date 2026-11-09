"""Threads of one process: name, start module, and CPU time (a read-only census).

    python -I threads.py <pid> [--watch SECONDS] [--top N]

It lists each thread with its description (Rust sets it from the thread name),
the module that holds its start address, and its user and kernel CPU time.
With `--watch`, it takes a second sample after SECONDS and prints the CPU time
and the context switches of the process in between: an idle profile by thread.
It only queries the process; it changes nothing in it.
"""

from __future__ import annotations

import ctypes as C
import ctypes.wintypes as W
import sys
import time
from collections import defaultdict

TH32CS_SNAPTHREAD, TH32CS_SNAPMODULE, TH32CS_SNAPMODULE32 = 0x4, 0x8, 0x10
THREAD_QUERY_INFORMATION, THREAD_QUERY_LIMITED_INFORMATION = 0x0040, 0x0800
PROCESS_QUERY_LIMITED_INFORMATION = 0x1000

kernel32 = C.WinDLL("kernel32", use_last_error=True)
ntdll = C.WinDLL("ntdll")


class THREADENTRY32(C.Structure):
    _fields_ = [("dwSize", W.DWORD), ("cntUsage", W.DWORD), ("th32ThreadID", W.DWORD),
                ("th32OwnerProcessID", W.DWORD), ("tpBasePri", W.LONG), ("tpDeltaPri", W.LONG), ("dwFlags", W.DWORD)]


class MODULEENTRY32W(C.Structure):
    _fields_ = [("dwSize", W.DWORD), ("th32ModuleID", W.DWORD), ("th32ProcessID", W.DWORD),
                ("GlblcntUsage", W.DWORD), ("ProccntUsage", W.DWORD), ("modBaseAddr", C.c_void_p),
                ("modBaseSize", W.DWORD), ("hModule", W.HMODULE), ("szModule", W.WCHAR * 256),
                ("szExePath", W.WCHAR * 260)]


class IO_COUNTERS(C.Structure):
    _fields_ = [(name, C.c_ulonglong) for name in ("ReadOps", "WriteOps", "OtherOps", "ReadBytes", "WriteBytes",
                                                   "OtherBytes")]


kernel32.CreateToolhelp32Snapshot.restype = W.HANDLE
kernel32.OpenThread.restype = W.HANDLE
kernel32.OpenProcess.restype = W.HANDLE
kernel32.GetThreadDescription.argtypes = [W.HANDLE, C.POINTER(C.c_wchar_p)]
kernel32.GetThreadTimes.argtypes = [W.HANDLE] + [C.POINTER(W.FILETIME)] * 4
kernel32.GetProcessTimes.argtypes = [W.HANDLE] + [C.POINTER(W.FILETIME)] * 4
kernel32.GetProcessIoCounters.argtypes = [W.HANDLE, C.POINTER(IO_COUNTERS)]
kernel32.LocalFree.argtypes = [C.c_void_p]
kernel32.QueryThreadCycleTime.argtypes = [W.HANDLE, C.POINTER(C.c_ulonglong)]
kernel32.QueryProcessCycleTime.argtypes = [W.HANDLE, C.POINTER(C.c_ulonglong)]


def cycles_per_second() -> float:
    """The nominal CPU frequency (registry `~MHz`): the rate of the cycle counters."""
    import winreg

    with winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, r"HARDWARE\DESCRIPTION\System\CentralProcessor\0") as key:
        return winreg.QueryValueEx(key, "~MHz")[0] * 1e6


def thread_cycles(pid: int, mods: list[tuple[int, int, str]] | None = None) -> dict[int, tuple[str, int]]:
    """Thread id -> (label, CPU cycles). Thread times move only in 15.6 ms clock ticks; cycles do not.

    The label is the thread description, or `<module>` of the start address when `mods` is given.
    """
    snapshot = kernel32.CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0)
    entry = THREADENTRY32()
    entry.dwSize = C.sizeof(entry)
    found = {}
    more = kernel32.Thread32First(W.HANDLE(snapshot), C.byref(entry))
    while more:
        if entry.th32OwnerProcessID == pid:
            handle = kernel32.OpenThread(THREAD_QUERY_INFORMATION, False, entry.th32ThreadID) or kernel32.OpenThread(
                THREAD_QUERY_LIMITED_INFORMATION, False, entry.th32ThreadID)
            if handle:
                handle = W.HANDLE(handle)
                text = C.c_wchar_p()
                name = ""
                if kernel32.GetThreadDescription(handle, C.byref(text)) >= 0 and text.value is not None:
                    name = text.value
                    kernel32.LocalFree(C.cast(text, C.c_void_p))
                if not name and mods is not None:
                    start = C.c_void_p()
                    name = "<?>"
                    if ntdll.NtQueryInformationThread(handle, 9, C.byref(start), C.sizeof(start), None) == 0:
                        address = start.value or 0
                        name = next((f"<{label}>" for base, size, label in mods if base <= address < base + size), "<?>")
                cycles = C.c_ulonglong()
                kernel32.QueryThreadCycleTime(handle, C.byref(cycles))
                found[entry.th32ThreadID] = (name, cycles.value)
                kernel32.CloseHandle(handle)
        more = kernel32.Thread32Next(W.HANDLE(snapshot), C.byref(entry))
    kernel32.CloseHandle(W.HANDLE(snapshot))
    return found


def process_cycles(pid: int) -> int:
    handle = W.HANDLE(kernel32.OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, False, pid))
    cycles = C.c_ulonglong()
    kernel32.QueryProcessCycleTime(handle, C.byref(cycles))
    kernel32.CloseHandle(handle)
    return cycles.value


def filetime_seconds(value: W.FILETIME) -> float:
    return ((value.dwHighDateTime << 32) | value.dwLowDateTime) / 1e7


def modules(pid: int) -> list[tuple[int, int, str]]:
    snapshot = kernel32.CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid)
    found = []
    entry = MODULEENTRY32W()
    entry.dwSize = C.sizeof(entry)
    if kernel32.Module32FirstW(W.HANDLE(snapshot), C.byref(entry)):
        while True:
            found.append((entry.modBaseAddr or 0, entry.modBaseSize, entry.szModule))
            if not kernel32.Module32NextW(W.HANDLE(snapshot), C.byref(entry)):
                break
    kernel32.CloseHandle(W.HANDLE(snapshot))
    return found


def sample(pid: int, mods: list[tuple[int, int, str]]) -> dict[int, tuple[str, str, float, float]]:
    snapshot = kernel32.CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0)
    entry = THREADENTRY32()
    entry.dwSize = C.sizeof(entry)
    threads = {}
    more = kernel32.Thread32First(W.HANDLE(snapshot), C.byref(entry))
    while more:
        if entry.th32OwnerProcessID == pid:
            handle = kernel32.OpenThread(THREAD_QUERY_INFORMATION, False, entry.th32ThreadID) or kernel32.OpenThread(
                THREAD_QUERY_LIMITED_INFORMATION, False, entry.th32ThreadID)
            if handle:
                handle = W.HANDLE(handle)
                text = C.c_wchar_p()
                name = ""
                if kernel32.GetThreadDescription(handle, C.byref(text)) >= 0 and text.value is not None:
                    name = text.value
                    kernel32.LocalFree(C.cast(text, C.c_void_p))
                start = C.c_void_p()
                module = "?"
                # ThreadQuerySetWin32StartAddress = 9
                if ntdll.NtQueryInformationThread(handle, 9, C.byref(start), C.sizeof(start), None) == 0:
                    address = start.value or 0
                    for base, size, label in mods:
                        if base <= address < base + size:
                            module = label
                            break
                created, exited, kernel, user = W.FILETIME(), W.FILETIME(), W.FILETIME(), W.FILETIME()
                kernel32.GetThreadTimes(handle, C.byref(created), C.byref(exited), C.byref(kernel), C.byref(user))
                threads[entry.th32ThreadID] = (name, module, filetime_seconds(user), filetime_seconds(kernel))
                kernel32.CloseHandle(handle)
        more = kernel32.Thread32Next(W.HANDLE(snapshot), C.byref(entry))
    kernel32.CloseHandle(W.HANDLE(snapshot))
    return threads


def process_counters(pid: int) -> tuple[float, IO_COUNTERS]:
    handle = W.HANDLE(kernel32.OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, False, pid))
    created, exited, kernel, user = W.FILETIME(), W.FILETIME(), W.FILETIME(), W.FILETIME()
    kernel32.GetProcessTimes(handle, C.byref(created), C.byref(exited), C.byref(kernel), C.byref(user))
    io = IO_COUNTERS()
    kernel32.GetProcessIoCounters(handle, C.byref(io))
    kernel32.CloseHandle(handle)
    return filetime_seconds(kernel) + filetime_seconds(user), io


def label(name: str, module: str) -> str:
    return name or f"<{module}>"


def main() -> None:
    pid = int(sys.argv[1])
    watch = float(sys.argv[sys.argv.index("--watch") + 1]) if "--watch" in sys.argv else 0.0
    top = int(sys.argv[sys.argv.index("--top") + 1]) if "--top" in sys.argv else 40
    mods = modules(pid)
    first = sample(pid, mods)
    groups: dict[str, list[float]] = defaultdict(lambda: [0, 0.0])
    for name, module, user, kernel in first.values():
        group = groups[label(name, module)]
        group[0] += 1
        group[1] += user + kernel
    print(f"{len(first)} threads; by name (count, CPU s since thread start):")
    for key, (count, cpu) in sorted(groups.items(), key=lambda item: (-item[1][1], item[0]))[:top]:
        print(f"  {count:4}  {cpu:9.2f}  {key}")
    if not watch:
        return
    cpu_before, io_before = process_counters(pid)
    time.sleep(watch)
    second = sample(pid, mods)
    cpu_after, io_after = process_counters(pid)
    print(f"\n{watch:.0f} s later: process CPU {cpu_after - cpu_before:.3f} s, "
          f"read {(io_after.ReadBytes - io_before.ReadBytes) / 2**20:.2f} MiB in {io_after.ReadOps - io_before.ReadOps} ops, "
          f"written {(io_after.WriteBytes - io_before.WriteBytes) / 2**20:.2f} MiB in {io_after.WriteOps - io_before.WriteOps} ops, "
          f"other ops {io_after.OtherOps - io_before.OtherOps}; "
          f"threads {len(first)} -> {len(second)} (new {len(set(second) - set(first))}, gone {len(set(first) - set(second))})")
    deltas: dict[str, list[float]] = defaultdict(lambda: [0, 0.0, 0.0])
    for tid, (name, module, user, kernel) in second.items():
        before = first.get(tid)
        base_user, base_kernel = (before[2], before[3]) if before else (0.0, 0.0)
        group = deltas[label(name, module) + ("" if before else " (new)")]
        group[0] += 1
        group[1] += user - base_user
        group[2] += kernel - base_kernel
    print("  count    user s  kernel s  thread")
    for key, (count, user, kernel) in sorted(deltas.items(), key=lambda item: -(item[1][1] + item[1][2]))[:top]:
        if user + kernel > 0:
            print(f"  {count:5}  {user:8.3f}  {kernel:8.3f}  {key}")


if __name__ == "__main__":
    main()
