# /// script
# requires-python = ">=3.10"
# dependencies = ["psutil>=5.9"]
# ///
"""Interleaved A/B of `churn_bench` builds: the catalog's cost while files change (offline).

Each build runs the same harness (`crates/prism-core/examples/churn_bench.rs`): the real watcher on
a test tree, and a child process that changes files as a build, a package install and a clean do.
The generator is always the same exe for both sides (`--generator`, default: the B exe). The runs
alternate A B B A. Between the harness's MARK points this script samples the thread count, the
handle count and the private bytes from outside (ten times a second).

    uv run oploop/bench/churn_ab.py --a target/oploop/e/base/churn_bench.exe \
        --b target/release/examples/churn_bench.exe --rounds 2 [--out results.jsonl]

A run with catalog rows that differ from the disk at the end counts as an error. With
`--recording`, the child replays recorded file events (a real workload) and not the built-in
script.
"""

from __future__ import annotations

import argparse
import json
import statistics
import subprocess
import sys
import threading
import time
from pathlib import Path

import psutil

sys.path.insert(0, str(Path(__file__).parent))
from catalog_ab import rank_sum_p  # noqa: E402

REPO = Path(__file__).resolve().parents[2]


def run_once(exe: str, generator: str, data: str, tree: str, settle: float, recording: str | None = None) -> dict:
    proc = subprocess.Popen(
        [exe, data, tree, "--settle", str(settle), "--generator", generator]
        + (["--recording", recording] if recording else []),
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
    )
    handle = psutil.Process(proc.pid)
    peaks = {"threads": 0, "handles": 0, "private": 0}
    stop = threading.Event()

    def sample() -> None:
        while not stop.is_set():
            try:
                peaks["threads"] = max(peaks["threads"], handle.num_threads())
                peaks["handles"] = max(peaks["handles"], handle.num_handles())
                peaks["private"] = max(peaks["private"], handle.memory_info().private)
            except psutil.Error:
                return
            time.sleep(0.1)

    sampler = None
    summary = None
    base = {}
    end = {}
    for line in proc.stdout:
        line = line.strip()
        if line == "MARK ready":
            base = {"threads": handle.num_threads(), "handles": handle.num_handles(),
                    "private": handle.memory_info().private}
            sampler = threading.Thread(target=sample, daemon=True)
            sampler.start()
            proc.stdin.write("\n")
            proc.stdin.flush()
        elif line == "MARK settled":
            stop.set()
            end = {"threads": handle.num_threads(), "handles": handle.num_handles(),
                   "private": handle.memory_info().private}
            proc.stdin.write("\n")
            proc.stdin.flush()
        elif line.startswith("{"):
            summary = json.loads(line)
    errors = proc.stderr.read()
    proc.wait()
    if summary is None:
        raise SystemExit(f"{exe} gave no summary (exit code {proc.returncode})\n{errors}")
    summary["cpu_ms"] = summary["kernel_ms"] + summary["user_ms"]
    summary["threads_base"] = base["threads"]
    summary["threads_peak"] = peaks["threads"]
    summary["threads_end"] = end["threads"]
    summary["handles_peak"] = peaks["handles"]
    summary["private_base_mib"] = base["private"] / 2**20
    summary["private_peak_mib"] = peaks["private"] / 2**20
    summary["private_end_mib"] = end["private"] / 2**20
    summary["exit"] = proc.returncode
    summary["first_differences"] = errors.strip().splitlines()[:3]
    return summary


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--a", required=True, help="baseline churn_bench.exe")
    parser.add_argument("--b", help="candidate churn_bench.exe (omit for an A-only noise run)")
    parser.add_argument("--generator", help="the exe that changes the files, for both sides (default: B, else A)")
    parser.add_argument("--data", default=str(REPO / "target/tmp/bench/data"))
    parser.add_argument("--tree", default=str(REPO / "target/tmp/bench/churn-tree"))
    parser.add_argument("--settle", type=float, default=30.0)
    parser.add_argument("--recording", help="replay this slice of an `fsevents.py record` file, not the built-in script")
    parser.add_argument("--rounds", type=int, default=2, help="A B B A rounds (2 runs per side each)")
    parser.add_argument("--out", help="append the raw runs as JSON lines")
    args = parser.parse_args()

    generator = str(Path(args.generator or args.b or args.a).resolve())
    order = ("A", "B", "B", "A") if args.b else ("A",)
    runs: dict[str, list[dict]] = {"A": [], "B": []}
    for round_number in range(args.rounds):
        for side in order:
            exe = str(Path(args.a if side == "A" else args.b).resolve())
            summary = run_once(exe, generator, args.data, args.tree, args.settle, args.recording)
            runs[side].append(summary)
            converged = summary["converged_ms"]
            print(
                f"round {round_number} {side}: CPU {summary['cpu_ms']:.0f} ms, written {summary['write_mib']:.1f} MiB in "
                f"{summary['write_ops']} ops, threads {summary['threads_base']} -> peak {summary['threads_peak']}, "
                f"private peak {summary['private_peak_mib']:.0f} MiB, fresh after "
                f"{'never' if converged is None else f'{converged / 1000:.1f} s'}, differences {summary['differences']}",
                file=sys.stderr,
            )
            if args.out:
                with open(args.out, "a", encoding="utf-8") as out:
                    out.write(json.dumps({"side": side, "exe": exe, **summary}) + "\n")

    keys = ("cpu_ms", "kernel_ms", "user_ms", "write_mib", "write_ops", "read_mib", "read_ops", "threads_peak",
            "handles_peak", "private_peak_mib", "private_end_mib", "generator_ms")
    print(f"{'metric':18} {'A median':>10} {'B median':>10} {'change':>8} {'A range':>19} {'B range':>19} {'p':>7}")
    for key in keys:
        a = [run[key] for run in runs["A"]]
        b = [run[key] for run in runs["B"]] or a
        ma, mb = statistics.median(a), statistics.median(b)
        change = (mb - ma) / ma * 100 if ma else 0.0
        p = rank_sum_p(a, b) if runs["B"] else 1.0
        flag = " significant" if p < 0.05 else ""
        print(f"{key:18} {ma:10.1f} {mb:10.1f} {change:+7.1f}% {min(a):9.1f}-{max(a):<9.1f} "
              f"{min(b):9.1f}-{max(b):<9.1f} {p:7.4f}{flag}")
    for side, items in runs.items():
        if not items:
            continue
        fresh = [item["converged_ms"] for item in items]
        print(f"{side}: n={len(items)}; catalog equal to disk after (ms): {fresh}; "
              f"differences at the end: {[item['differences'] for item in items]}; "
              f"errors: {sum(1 for item in items if item['differences'] != 0)}")


if __name__ == "__main__":
    main()
