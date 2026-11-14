"""Sampling profiler for a Windows process: where its threads spend time, by function.

  python oploop/bench/sampler.py <pid> [--seconds 10] [--interval-ms 1] [--top 30]
  python oploop/bench/sampler.py --run <exe> [args...]      (starts it, samples until it exits)

It suspends each thread of the process about every millisecond, reads the instruction pointer,
and resumes it. Functions come from dbghelp (the exe's PDB beside the exe, exports for system
DLLs). A thread inside a system call shows the ntdll stub (`NtWriteFile`, `NtCreateFile`, ...),
so kernel time is attributed to the call that entered the kernel. No admin rights needed;
it reads only processes of the same user. Python standard library only.
"""
from __future__ import annotations

import collections
import ctypes as C
import ctypes.wintypes as W
import subprocess
import sys
import time

k32 = C.WinDLL("kernel32", use_last_error=True)
dbghelp = C.WinDLL("dbghelp", use_last_error=True)

TH32CS_SNAPTHREAD = 0x4
THREAD_ALL = 0x0002 | 0x0008 | 0x0010 | 0x0040  # suspend/resume, get context, query info
PROCESS_ALL = 0x0400 | 0x0010  # query information, vm read
CONTEXT_CONTROL = 0x00100001


class THREADENTRY32(C.Structure):
    _fields_ = [("dwSize", W.DWORD), ("cntUsage", W.DWORD), ("th32ThreadID", W.DWORD),
                ("th32OwnerProcessID", W.DWORD), ("tpBasePri", W.LONG), ("tpDeltaPri", W.LONG), ("dwFlags", W.DWORD)]


class CONTEXT(C.Structure):
    # x64 CONTEXT is 1232 bytes, 16-byte aligned; Rip is at offset 0xF8.
    _fields_ = [("buf", C.c_ubyte * 1232)]


class SYMBOL_INFOW(C.Structure):
    _fields_ = [("SizeOfStruct", W.ULONG), ("TypeIndex", W.ULONG), ("Reserved", C.c_ulonglong * 2),
                ("Index", W.ULONG), ("Size", W.ULONG), ("ModBase", C.c_ulonglong), ("Flags", W.ULONG),
                ("Value", C.c_ulonglong), ("Address", C.c_ulonglong), ("Register", W.ULONG), ("Scope", W.ULONG),
                ("Tag", W.ULONG), ("NameLen", W.ULONG), ("MaxNameLen", W.ULONG), ("Name", W.WCHAR * 512)]


k32.OpenThread.restype = W.HANDLE
k32.OpenProcess.restype = W.HANDLE
k32.CreateToolhelp32Snapshot.restype = W.HANDLE


def threads(pid: int) -> list[int]:
    snap = k32.CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0)
    entry = THREADENTRY32(C.sizeof(THREADENTRY32))
    out = []
    ok = k32.Thread32First(snap, C.byref(entry))
    while ok:
        if entry.th32OwnerProcessID == pid:
            out.append(entry.th32ThreadID)
        ok = k32.Thread32Next(snap, C.byref(entry))
    k32.CloseHandle(snap)
    return out


def sample(pid: int, seconds: float, interval_ms: float, stop=lambda: False) -> collections.Counter:
    process = k32.OpenProcess(PROCESS_ALL, False, pid)
    dbghelp.SymSetOptions(0x2 | 0x10)  # undecorated names, deferred loads
    dbghelp.SymInitializeW(process, None, True)
    raw = (C.c_ubyte * (1232 + 16))()
    base = (C.addressof(raw) + 15) & ~15
    ctx = CONTEXT.from_address(base)
    handles: dict[int, int] = {}
    hits: collections.Counter = collections.Counter()
    sym = SYMBOL_INFOW()
    cache: dict[int, str] = {}

    def resolve_new() -> None:
        # Resolve while the process lives: dbghelp needs its module list.
        dbghelp.SymRefreshModuleList(process)
        for (_, rip) in list(hits):
            if rip in cache:
                continue
            sym.SizeOfStruct = C.sizeof(SYMBOL_INFOW) - 512 * 2
            sym.MaxNameLen = 511
            disp = C.c_ulonglong()
            ok = dbghelp.SymFromAddrW(process, C.c_ulonglong(rip), C.byref(disp), C.byref(sym))
            cache[rip] = sym.Name if ok else f"0x{rip:x}"

    end = time.perf_counter() + seconds
    last_scan = 0.0
    while time.perf_counter() < end and not stop():
        if time.perf_counter() - last_scan > 0.5:
            for tid in threads(pid):
                if tid not in handles:
                    h = k32.OpenThread(THREAD_ALL, False, tid)
                    if h:
                        handles[tid] = h
            resolve_new()
            last_scan = time.perf_counter()
        for tid, h in list(handles.items()):
            if k32.SuspendThread(h) == 0xFFFFFFFF:
                k32.CloseHandle(h)
                del handles[tid]
                continue
            C.memset(base, 0, 1232)
            C.c_uint32.from_address(base + 0x30).value = CONTEXT_CONTROL  # ContextFlags
            if k32.GetThreadContext(h, C.c_void_p(base)):
                hits[(tid, C.c_uint64.from_address(base + 0xF8).value)] += 1
            k32.ResumeThread(h)
        time.sleep(interval_ms / 1000)
    resolve_new()
    names: collections.Counter = collections.Counter()
    by_thread: collections.Counter = collections.Counter()
    for (tid, rip), count in hits.items():
        names[cache.get(rip, f"0x{rip:x}")] += count
        by_thread[tid] += count
    for h in handles.values():
        k32.CloseHandle(h)
    dbghelp.SymCleanup(process)
    k32.CloseHandle(process)
    names["__threads__"] = 0
    return names, by_thread


def report(names: collections.Counter, by_thread: collections.Counter, top: int) -> None:
    del names["__threads__"]
    total = sum(names.values()) or 1
    print(f"samples {total}; by thread: " + ", ".join(f"{t}:{c}" for t, c in by_thread.most_common(5)))
    for name, count in names.most_common(top):
        print(f"{100 * count / total:5.1f}%  {count:6d}  {name}")


def main() -> None:
    args = sys.argv[1:]
    top = 30
    if "--top" in args:
        i = args.index("--top")
        top = int(args[i + 1])
        del args[i:i + 2]
    if args and args[0] == "--run":
        proc = subprocess.Popen(args[1:])
        time.sleep(0.05)
        names, by_thread = sample(proc.pid, 3600, 1, stop=lambda: proc.poll() is not None)
        proc.wait()
    else:
        pid = int(args[0])
        seconds = float(args[args.index("--seconds") + 1]) if "--seconds" in args else 10
        interval = float(args[args.index("--interval-ms") + 1]) if "--interval-ms" in args else 1
        names, by_thread = sample(pid, seconds, interval)
    report(names, by_thread, top)


if __name__ == "__main__":
    main()
