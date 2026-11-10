"""Frames that the UI layer draws for a typed query (parity mode, no backend).

    python -I oploop/bench/uiframes.py --a <base prism.exe> [--b <candidate prism.exe>] [--rounds 3]
        [--query report] [--fixture FIXTURE.json] [--out results.jsonl]

Each run starts `prism.exe --parity` with `ZED_MEASUREMENTS=1`: GPUI then prints one
`frame duration` line for each frame that it builds and paints. The scenario opens the palette,
types the query one key at a time with 200 ms between the keys (longer than the file-search and
power-plan delays, so each key's replies land before the next key), captures the palette, closes
it, and opens it again (the usual open: the caches are warm). The script reports the frame count
(a count, so it does not depend on the machine load) and the time in frames, for the first open,
the typed keys and the second open, and the time of the first frame of each open. The captures of
all runs must be the same images. With `--b`, the runs alternate A B B A.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import statistics
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
FIXTURE = REPO / ".agents/skills/verify-prism/fixtures/base.json"
WORK = REPO / "target/tmp/bench/uiframes"
FRAME = re.compile(r"^frame duration: ([0-9.]+)(ns|µs|ms|s)$")
UNIT_MS = {"ns": 1e-6, "µs": 1e-3, "ms": 1.0, "s": 1000.0}
# The runner prints these marks through `{"info": true}` steps: the frames between two marks
# belong to one phase.
MARK = "parity-info"


def run_once(exe: str, fixture: Path, query: str, label: str) -> dict:
    WORK.mkdir(parents=True, exist_ok=True)
    out = WORK / label
    out.mkdir(exist_ok=True)
    steps: list[dict] = [{"info": True}, {"open": True}, {"wait": 700}, {"info": True}]
    for character in query:
        steps += [{"type": character}, {"wait": 200}]
    steps += [{"wait": 300}, {"info": True}, {"capture": "typed"}]
    # The second open is the usual one: the icon and text caches are warm.
    steps += [{"key": "escape"}, {"wait": 200}, {"key": "escape"}, {"wait": 600}, {"info": True},
              {"open": True}, {"wait": 700}, {"info": True}, {"capture": "reopened"}]
    script = WORK / f"{label}.json"
    script.write_text(json.dumps({"backdrop": "#1c1d22", "scenarios": [{"name": "type", "steps": steps}]}))
    result = subprocess.run([exe, "--parity", str(fixture), str(script), "type", str(out)],
                            capture_output=True, env={**os.environ, "ZED_MEASUREMENTS": "1"}, timeout=120)
    phases: list[list[float]] = [[]]
    for line in result.stderr.decode("utf-8", "replace").splitlines():
        line = line.strip()
        if line.startswith(MARK):
            phases.append([])
            continue
        match = FRAME.match(line)
        if match:
            phases[-1].append(float(match.group(1)) * UNIT_MS[match.group(2)])
    # phases: [before the first mark, the first open, the typed keys, the close, the second open, after]
    while len(phases) < 5:
        phases.append([])
    opened, typed, reopened = phases[1], phases[2], phases[4]
    capture = out / "typed.png"
    return {
        "exit": result.returncode,
        "open_frames": len(opened),
        "open_ms": sum(opened),
        "open_first_frame_ms": opened[0] if opened else 0.0,
        "reopen_frames": len(reopened),
        "reopen_ms": sum(reopened),
        "reopen_first_frame_ms": reopened[0] if reopened else 0.0,
        "type_frames": len(typed),
        "type_ms": sum(typed),
        "frames_per_key": len(typed) / max(len(query), 1),
        "ms_per_key": sum(typed) / max(len(query), 1),
        "capture_sha": "+".join(
            hashlib.sha256(image.read_bytes()).hexdigest()[:12] if image.exists() else "missing"
            for image in (capture, out / "reopened.png")),
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--a", required=True)
    parser.add_argument("--b")
    parser.add_argument("--rounds", type=int, default=3)
    parser.add_argument("--query", default="report")
    parser.add_argument("--fixture", default=str(FIXTURE))
    parser.add_argument("--out")
    args = parser.parse_args()
    order = ("A", "B", "B", "A") if args.b else ("A",)
    runs: dict[str, list[dict]] = {"A": [], "B": []}
    for round_number in range(args.rounds):
        for side in order:
            exe = str(Path(args.a if side == "A" else args.b).resolve())
            result = run_once(exe, Path(args.fixture), args.query, f"{side}{round_number}{len(runs[side])}")
            runs[side].append(result)
            print(f"round {round_number} {side}: open {result['open_frames']} frames {result['open_ms']:.1f} ms; "
                  f"typing {result['type_frames']} frames {result['type_ms']:.1f} ms "
                  f"({result['frames_per_key']:.1f} frames, {result['ms_per_key']:.2f} ms a key); exit {result['exit']}",
                  file=sys.stderr)
            if args.out:
                with open(args.out, "a", encoding="utf-8") as out:
                    out.write(json.dumps({"side": side, "exe": exe, "query": args.query, **result}) + "\n")
    for side, items in runs.items():
        if not items:
            continue
        print(f"{side}: n={len(items)}")
        for key in ("open_frames", "open_ms", "open_first_frame_ms", "reopen_frames", "reopen_ms",
                    "reopen_first_frame_ms", "type_frames", "type_ms", "frames_per_key", "ms_per_key"):
            values = [item[key] for item in items]
            print(f"  {key:22} {statistics.median(values):8.2f} ({min(values):.2f}-{max(values):.2f})")
        print(f"  errors: {sum(1 for item in items if item['exit'] != 0 or 'missing' in item['capture_sha'])}")
    captures = {item["capture_sha"] for items in runs.values() for item in items}
    print(f"captures (typed query + second open): {len(captures)} distinct pair(s) over all runs")


if __name__ == "__main__":
    main()
