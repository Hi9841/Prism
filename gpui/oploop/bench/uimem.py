"""Memory and threads of the port's UI layer alone (no file index, no SQLite).

    python -I oploop/bench/uimem.py <prism.exe> [runs]

Runs `prism.exe --parity` (fixture data, a parked window) on one UI capture
scenario, and samples the process 1.3 s after start, when the window has
painted. Prints the median committed private memory (psutil `private`),
working set and thread count over the runs. Run it from `gpui/`.
"""

from __future__ import annotations

import os
import statistics
import subprocess
import sys
import time

import psutil

FIXTURE = r"target\parity\run-i026\fixtures\idle.json"
SCRIPT = r"migration\parity\scenarios.json"


def once(exe: str) -> tuple[float, float, int]:
    proc = subprocess.Popen([exe, "--parity", FIXTURE, SCRIPT, "idle-scrolled", r"target\tmp\memprobe"],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(1.3)
    info = psutil.Process(proc.pid)
    memory = info.memory_full_info()
    sample = (memory.private / 2**20, memory.wset / 2**20, info.num_threads())
    proc.wait(30)
    return sample


def main() -> None:
    exe = os.path.abspath(sys.argv[1])
    runs = int(sys.argv[2]) if len(sys.argv) > 2 else 5
    samples = [once(exe) for _ in range(runs)]
    private, wset, threads = (sorted(column) for column in zip(*samples))
    print(f"{exe}: private {statistics.median(private):.1f} MiB ({private[0]:.1f}-{private[-1]:.1f}), "
          f"working set {statistics.median(wset):.1f} MiB, threads {statistics.median(threads):.0f} "
          f"({threads[0]}-{threads[-1]}), n={runs}")


if __name__ == "__main__":
    main()
