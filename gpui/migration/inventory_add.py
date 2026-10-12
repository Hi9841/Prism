"""Append inventory rows from a TSV fragment (same 7 columns, no header).

A row whose (file, symbol) already exists is replaced, so re-running a fragment
is idempotent. Run from the repo root:

    python -I migration/inventory_add.py migration/fragments/<name>.tsv
"""
import sys
from pathlib import Path

path = Path("migration/inventory.tsv")
lines = path.read_text(encoding="utf-8").splitlines()
header, rows = lines[0], [line.split("\t") for line in lines[1:] if line.strip()]
width = len(header.split("\t"))

added = replaced = 0
for fragment in sys.argv[1:]:
    for number, line in enumerate(Path(fragment).read_text(encoding="utf-8").splitlines(), 1):
        if not line.strip() or line.startswith("#"):
            continue
        row = line.split("\t")
        if len(row) != width:
            sys.exit(f"{fragment}:{number}: expected {width} columns, got {len(row)}")
        for index, existing in enumerate(rows):
            if existing[0] == row[0] and existing[1] == row[1]:
                rows[index] = row
                replaced += 1
                break
        else:
            rows.append(row)
            added += 1

path.write_text(header + "\n" + "\n".join("\t".join(r) for r in rows) + "\n", encoding="utf-8", newline="\n")
print(f"added={added} replaced={replaced} rows={len(rows)}")
