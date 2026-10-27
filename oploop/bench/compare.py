# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Compares any builds of one bench run (`bench.py run --apps a b c`).

    uv run oploop/bench/compare.py <raw run dir> <app> <app> [<app> ...] [--j4 <raw j4 dir>]

For each pair (the first app against each other app) it prints J2 (warm and
first open) and J3 (query echo, rows done, settled; per query and over all
queries) with median, p95, n and a rank-sum p value, and, with `--j4`, the
resource samples.
"""

from __future__ import annotations

import json
import math
import statistics
import sys
from pathlib import Path



def rank_sum_p(a: list[float], b: list[float]) -> float:
    """Two-sided Mann-Whitney U p-value (normal approximation, tie-corrected)."""
    a = [x for x in a if x is not None]
    b = [x for x in b if x is not None]
    if len(a) < 3 or len(b) < 3:
        return float("nan")
    data = sorted([(x, 0) for x in a] + [(x, 1) for x in b])
    ranks = [0.0] * len(data)
    i = 0
    ties = 0.0
    while i < len(data):
        j = i
        while j + 1 < len(data) and data[j + 1][0] == data[i][0]:
            j += 1
        for k in range(i, j + 1):
            ranks[k] = (i + j) / 2 + 1
        t = j - i + 1
        ties += t**3 - t
        i = j + 1
    n1, n2 = len(a), len(b)
    r1 = sum(r for r, (_, g) in zip(ranks, data) if g == 0)
    u = r1 - n1 * (n1 + 1) / 2
    mean = n1 * n2 / 2
    n = n1 + n2
    sd = math.sqrt(n1 * n2 / 12 * ((n + 1) - ties / (n * (n - 1))))
    if sd == 0:
        return 1.0
    z = (abs(u - mean) - 0.5) / sd
    return math.erfc(z / math.sqrt(2))


QUIET_GAP_MS = 300


def rows_done(row: dict) -> float | None:
    """The last change before the first quiet gap of QUIET_GAP_MS.

    A late change after a gap (for example a thumbnail that paints again
    seconds later) moves `settled_ms`, but not this time.
    """
    done = row.get("first_ms")
    if done is None:
        return None
    for t, *_ in row.get("changes", []):
        if t - done > QUIET_GAP_MS:
            break
        done = t
    return done


def summary(values: list[float]) -> str:
    if not values:
        return "-"
    ordered = sorted(values)
    p95 = ordered[min(len(ordered) - 1, round(0.95 * (len(ordered) - 1)))]
    return f"{statistics.median(ordered):.0f} (p95 {p95:.0f}, n={len(ordered)})"


def table(title: str, apps: list[str], pick) -> None:
    print(f"\n### {title}\n")
    print("| Measure | " + " | ".join(apps) + " | " + " | ".join(f"p {apps[0]} vs {a}" for a in apps[1:]) + " |")
    print("|---" * (1 + len(apps) + len(apps) - 1) + "|")
    for label, values in pick():
        cells = [summary(values[a]) for a in apps]
        ps = [f"{rank_sum_p(values[apps[0]], values[a]):.2g}" if values[apps[0]] and values[a] else "-" for a in apps[1:]]
        print(f"| {label} | " + " | ".join(cells) + " | " + " | ".join(ps) + " |")


def main() -> None:
    args = sys.argv[1:]
    j4_dir = None
    if "--j4" in args:
        index = args.index("--j4")
        j4_dir = Path(args[index + 1])
        del args[index:index + 2]
    run, apps = Path(args[0]), args[1:]
    rows = json.loads((run / "rows.json").read_text(encoding="utf8"))
    j2 = [r for r in rows if r["journey"] == "j2"]
    j3 = [r for r in rows if r["journey"] == "j3"]

    def j2_pick():
        for label, key, cold in (("first frame, warm", "first_ms", False), ("settled, warm", "settled_ms", False),
                                 ("first frame, first open", "first_ms", True), ("settled, first open", "settled_ms", True)):
            yield label, {a: [r[key] for r in j2 if r["app"] == a and bool(r.get("cold")) == cold and r[key] is not None]
                          for a in apps}

    def j3_pick(measure):
        queries = list(dict.fromkeys(r["query"] for r in j3))
        for query in queries + ["all queries"]:
            values = {}
            for a in apps:
                chosen = [r for r in j3 if r["app"] == a and (query == "all queries" or r["query"] == query)]
                values[a] = [v for v in (measure(r) for r in chosen) if v is not None]
            yield f"`{query}`" if query != "all queries" else "**all queries**", values

    table("J2 open latency (ms after the activation signal)", apps, j2_pick)
    table("J3 query echo (first changed frame, ms)", apps, lambda: j3_pick(lambda r: r["first_ms"]))
    table("J3 rows done (ms)", apps, lambda: j3_pick(rows_done))
    table("J3 settled (ms)", apps, lambda: j3_pick(lambda r: r["settled_ms"]))

    if j4_dir:
        j4 = json.loads((j4_dir / "rows.json").read_text(encoding="utf8"))
        print("\n### J4 resources (median over runs)\n")
        print("| Measure | " + " | ".join(apps) + " |")
        print("|---" * (1 + len(apps)) + "|")
        for stage in ("after_start", "after_cycles_idle"):
            for key, unit, scale in (("private", "MiB", 2**20), ("wset", "MiB", 2**20), ("cpu", "s", 1),
                                     ("threads", "", 1), ("handles", "", 1), ("processes", "", 1)):
                cells = []
                for a in apps:
                    values = [r[stage][key] / scale for r in j4 if r["app"] == a]
                    cells.append(f"{statistics.median(values):.1f} {unit}".strip() if values else "-")
                print(f"| {stage} {key} | " + " | ".join(cells) + " |")


if __name__ == "__main__":
    main()
