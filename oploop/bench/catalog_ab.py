# /// script
# requires-python = ">=3.10"
# dependencies = ["psutil>=5.9"]
# ///
"""Interleaved A/B of `catalog_bench` builds on one catalog copy (offline, no desktop).

Each build runs the same harness (`crates/prism-core/examples/catalog_bench.rs`): the name index
load, then the search queries. The runs alternate A B B A, so cache and drift effects fall on both
sides. The script samples private bytes at the harness's MARK points (index memory = after the
load minus before it) and compares the result lists of every run against the first A run.

    uv run oploop/bench/catalog_ab.py [--cold] --a target/oploop/bench-base.exe --b target/release/examples/catalog_bench.exe \
        --db target/tmp/bench/catalog.db --rounds 4
"""

from __future__ import annotations

import argparse
import json
import math
import statistics
import subprocess
import sys
import tempfile
from pathlib import Path

import psutil


def purge_file_cache(path: str) -> None:
    """Drops the file from the Windows file cache: a non-cached open purges its cached pages."""
    import ctypes
    from ctypes import wintypes

    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel32.CreateFileW.restype = wintypes.HANDLE
    kernel32.CreateFileW.argtypes = [
        wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD, wintypes.LPVOID,
        wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE,
    ]
    generic_read, share_all, open_existing, no_buffering = 0x80000000, 7, 3, 0x20000000
    handle = kernel32.CreateFileW(str(Path(path).resolve()), generic_read, share_all, None, open_existing, no_buffering, None)
    if handle in (None, wintypes.HANDLE(-1).value):
        raise SystemExit(f"cannot purge {path}: error {ctypes.get_last_error()}")
    kernel32.CloseHandle(handle)


def run_once(exe: str, db: str, results: Path) -> dict:
    proc = subprocess.Popen(
        [str(Path(exe).resolve()), db, str(results)],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        text=True,
    )
    handle = psutil.Process(proc.pid)
    private = {}
    summary = None
    for line in proc.stdout:
        line = line.strip()
        if line.startswith("MARK "):
            private[line[5:]] = handle.memory_info().private
            proc.stdin.write("\n")
            proc.stdin.flush()
        elif line.startswith("{"):
            summary = json.loads(line)
    proc.wait()
    if proc.returncode != 0 or summary is None:
        raise SystemExit(f"{exe} failed with exit code {proc.returncode}")
    summary["index_mib"] = (private["loaded"] - private["start"]) / 1048576
    summary["after_search_mib"] = private["searched"] / 1048576
    return summary


def rank_sum_p(a: list[float], b: list[float]) -> float:
    """Two-sided Mann-Whitney U p value (normal approximation, tie-corrected)."""
    values = sorted((v, side) for side, xs in ((0, a), (1, b)) for v in xs)
    ranks: list[float] = [0.0] * len(values)
    i = 0
    ties = 0.0
    while i < len(values):
        j = i
        while j + 1 < len(values) and values[j + 1][0] == values[i][0]:
            j += 1
        for k in range(i, j + 1):
            ranks[k] = (i + j) / 2 + 1
        t = j - i + 1
        ties += t**3 - t
        i = j + 1
    n1, n2 = len(a), len(b)
    r1 = sum(r for r, (_, side) in zip(ranks, values) if side == 0)
    u = r1 - n1 * (n1 + 1) / 2
    n = n1 + n2
    sigma = math.sqrt(n1 * n2 / 12 * ((n + 1) - ties / (n * (n - 1))))
    if sigma == 0:
        return 1.0
    z = (abs(u - n1 * n2 / 2) - 0.5) / sigma
    return math.erfc(max(z, 0) / math.sqrt(2))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--a", required=True, help="baseline catalog_bench.exe")
    parser.add_argument("--b", required=True, help="candidate catalog_bench.exe")
    parser.add_argument("--db", required=True, help="catalog copy")
    parser.add_argument("--rounds", type=int, default=4, help="A B B A rounds (2 runs per side each)")
    parser.add_argument("--out", help="append the raw runs as JSON lines")
    parser.add_argument("--cold", action="store_true", help="purge the catalog from the file cache before each run")
    args = parser.parse_args()

    tmp = Path(tempfile.mkdtemp(prefix="catalog-ab-", dir=Path("target/tmp")))
    runs: dict[str, list[dict]] = {"A": [], "B": []}
    reference: str | None = None
    differences = 0
    for round_number in range(args.rounds):
        for side in ("A", "B", "B", "A"):
            results = tmp / f"{side}-{round_number}-{len(runs[side])}.tsv"
            if args.cold:
                purge_file_cache(args.db)
            summary = run_once(args.a if side == "A" else args.b, args.db, results)
            runs[side].append(summary)
            text = results.read_text(encoding="utf-8")
            if reference is None:
                reference = text
            elif text != reference:
                differences += 1
                old = dict(line.split("\t", 1) if "\t" in line else (line, "") for line in reference.splitlines())
                for line in text.splitlines():
                    query, _, paths = line.partition("\t")
                    if old.get(query) != paths:
                        print(f"  results differ ({side}, round {round_number}): {query!r}", file=sys.stderr)
            print(
                f"round {round_number} {side}: load {summary['load_ms']:.0f} ms, index {summary['index_mib']:.1f} MiB, "
                f"first {summary['first_total_ms']:.1f} ms, warm {summary['search_total_ms']:.1f} ms",
                file=sys.stderr,
            )
    if args.out:
        with open(args.out, "a", encoding="utf-8") as out:
            for side, items in runs.items():
                for item in items:
                    out.write(json.dumps({"side": side, **item}) + "\n")

    def column(side: str, key: str, query: str | None = None) -> list[float]:
        return [run[key][query] if query else run[key] for run in runs[side]]

    rows = [(key, None) for key in ("load_ms", "index_mib", "first_total_ms", "search_total_ms")]
    rows += [("per_query_ms", query) for query in runs["A"][0]["per_query_ms"]]
    print(f"{'metric':42} {'A median':>10} {'B median':>10} {'change':>8} {'A range':>17} {'B range':>17} {'p':>7}")
    for key, query in rows:
        a, b = column("A", key, query), column("B", key, query)
        ma, mb = statistics.median(a), statistics.median(b)
        change = (mb - ma) / ma * 100 if ma else 0.0
        p = rank_sum_p(a, b)
        name = f"{key}[{query}]" if query else key
        flag = " significant" if p < 0.05 else ""
        print(
            f"{name:42} {ma:10.2f} {mb:10.2f} {change:+7.1f}% {min(a):8.2f}-{max(a):<8.2f} {min(b):8.2f}-{max(b):<8.2f} {p:7.4f}{flag}"
        )
    print(f"runs per side: {len(runs['A'])}; result lists different from the first A run: {differences}")


if __name__ == "__main__":
    main()
