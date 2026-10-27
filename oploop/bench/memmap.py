"""Committed private memory of one process, by region (a small VMMap).

    python -I memmap.py <pid> [--top N]

It walks the address space with VirtualQueryEx and groups committed
MEM_PRIVATE pages by allocation base. It prints the total, a size histogram,
and the largest allocations with their protection, so a profile shows whether
private memory is heap segments, thread stacks, or large driver pools.
"""

from __future__ import annotations

import ctypes as C
import ctypes.wintypes as W
import sys
from collections import defaultdict

PROCESS_QUERY_INFORMATION, PROCESS_VM_READ = 0x0400, 0x0010
MEM_COMMIT, MEM_PRIVATE = 0x1000, 0x20000
PROTECT = {0x01: "NOACCESS", 0x02: "R", 0x04: "RW", 0x08: "WC", 0x10: "X", 0x20: "RX", 0x40: "RWX", 0x80: "WCX"}


class MBI(C.Structure):
    _fields_ = [("BaseAddress", C.c_void_p), ("AllocationBase", C.c_void_p), ("AllocationProtect", W.DWORD),
                ("PartitionId", W.WORD), ("RegionSize", C.c_size_t), ("State", W.DWORD), ("Protect", W.DWORD),
                ("Type", W.DWORD)]


def regions(pid: int):
    kernel32 = C.WinDLL("kernel32", use_last_error=True)
    kernel32.OpenProcess.restype = W.HANDLE
    kernel32.VirtualQueryEx.argtypes = [W.HANDLE, C.c_void_p, C.POINTER(MBI), C.c_size_t]
    kernel32.VirtualQueryEx.restype = C.c_size_t
    handle = kernel32.OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, False, pid)
    if not handle:
        raise OSError(C.get_last_error(), "OpenProcess")
    address = 0
    info = MBI()
    try:
        while kernel32.VirtualQueryEx(handle, C.c_void_p(address), C.byref(info), C.sizeof(info)):
            yield (info.AllocationBase or 0, info.BaseAddress or 0, info.RegionSize, info.State, info.Type,
                   info.Protect & 0xFF)
            address = (info.BaseAddress or 0) + info.RegionSize
            if address >= 0x7FFFFFFFFFFF:
                break
    finally:
        kernel32.CloseHandle(handle)


def main() -> None:
    pid = int(sys.argv[1])
    top = int(sys.argv[sys.argv.index("--top") + 1]) if "--top" in sys.argv else 25
    by_base: dict[int, int] = defaultdict(int)
    prot_of: dict[int, set] = defaultdict(set)
    for base, _, size, state, kind, protect in regions(pid):
        if state == MEM_COMMIT and kind == MEM_PRIVATE:
            by_base[base] += size
            prot_of[base].add(PROTECT.get(protect, hex(protect)))
    total = sum(by_base.values())
    print(f"committed private: {total / 2**20:.1f} MiB in {len(by_base)} allocations")
    buckets = [(0, 64 << 10), (64 << 10, 1 << 20), (1 << 20, 4 << 20), (4 << 20, 16 << 20), (16 << 20, 1 << 40)]
    for low, high in buckets:
        sizes = [s for s in by_base.values() if low <= s < high]
        print(f"  {low >> 10:>8} KiB - {high >> 10:>10} KiB: {len(sizes):5} allocations, {sum(sizes) / 2**20:7.1f} MiB")
    for base, size in sorted(by_base.items(), key=lambda kv: -kv[1])[:top]:
        print(f"  {base:#016x}  {size / 2**20:8.2f} MiB  {','.join(sorted(prot_of[base]))}")


if __name__ == "__main__":
    main()
