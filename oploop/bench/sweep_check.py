"""End-to-end correctness check of the sweep on a test tree (no unit tests: the owner's rule).

Builds a tree in target/tmp/sweep-tree, then for each step changes the tree, runs
`catalog_sweep <db> <tree> --verify` (catalog rows under the tree against a fresh disk walk),
and reports the differences. Every step must end with 0 differences.

    python -I oploop/bench/sweep_check.py <catalog_sweep.exe> <catalog copy>
"""

from __future__ import annotations

import shutil
import subprocess
import sys
import time
from pathlib import Path

TREE = Path("target/tmp/sweep-tree").resolve()


def write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def build() -> None:
    shutil.rmtree(TREE, ignore_errors=True)
    for folder in ("alpha", "beta", "José", "Gamma Case", "deep/one/two/three"):
        for number in range(12):
            write(TREE / folder / f"file-{number:02}.txt", f"{folder} {number}\n")
    write(TREE / "root-file.md", "root\n")


def change() -> None:
    for number in range(3):
        (TREE / "alpha" / f"file-{number:02}.txt").unlink()
    shutil.rmtree(TREE / "deep" / "one" / "two")
    for number in range(5):
        write(TREE / "beta" / f"new-{number}.log", "new\n")
    time.sleep(0.05)
    for number in range(4):
        write(TREE / "José" / f"file-{number:02}.txt", "changed and longer\n")
    (TREE / "beta").rename(TREE / "beta renamed")
    # A case-only folder rename: the children keep their paths but not their parent text.
    (TREE / "Gamma Case").rename(TREE / "temporary")
    (TREE / "temporary").rename(TREE / "GAMMA case")


def delete_under_renamed() -> None:
    (TREE / "GAMMA case" / "file-05.txt").unlink()
    (TREE / "José" / "file-11.txt").unlink()


def sweep(exe: str, db: str, step: str, *extra: str) -> int:
    result = subprocess.run([exe, db, str(TREE), "--verify", *extra], capture_output=True, text=True, encoding="utf-8")
    lines = result.stdout.splitlines()
    differences = [line for line in lines if line.startswith(("MISSING", "STALE", "EXTRA"))]
    summary = next((line for line in lines if line.startswith("VERIFY")), f"no VERIFY line: {result.stderr[-400:]}")
    timing = next((line for line in lines if line.startswith("{")), "")
    print(f"{step}: {summary} {timing[:40]}")
    for line in differences[:12]:
        print(f"    {line}")
    return len(differences) if summary.startswith("VERIFY") else 1


def main() -> None:
    exe, db = str(Path(sys.argv[1]).resolve()), sys.argv[2]
    build()
    failures = sweep(exe, db, "1 new tree")
    failures += sweep(exe, db, "2 idle")
    change()
    failures += sweep(exe, db, "3 changed")
    delete_under_renamed()
    failures += sweep(exe, db, "4 deleted under renamed folders")
    # Folder renames as the watcher applies them (`Database::rename_file`), one at a time.
    for old, new in (("alpha", "alpha two"), ("José", "Josefina"), ("deep", "deeper")):
        (TREE / old).rename(TREE / new)
        failures += sweep(exe, db, f"5 watcher rename {old} -> {new}", "--rename", str(TREE / old), str(TREE / new))
    shutil.rmtree(TREE, ignore_errors=True)
    print("PASS" if failures == 0 else f"FAIL: {failures} differences")
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
