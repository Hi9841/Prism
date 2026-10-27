"""Parity profiler: where do the parity points go?

Usage: python -I hotspots.py <run-dir> [--top N] [--crop OUT-DIR]
<run-dir> holds orig/ and port/ (target/parity/run or target/parity/ux).

The score of one capture is 10 * (0.5 * ssim + 0.5 * close) (migration/parity/compare.py).
Each pixel has a share of the loss: (1 - ssim_map) / pixels * 5 + (diff > 24) / pixels * 5.
This script sums that loss over 16 px cells, joins neighboring hot cells into regions,
and prints the regions that cost the most points over all captures. It also splits the
loss into "edge" pixels (text, icons, borders: high local contrast in the original) and
"flat" pixels (fills and gradients), because the two need different fixes.
"""
import json
import sys
from pathlib import Path

import numpy as np
from PIL import Image

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "migration" / "parity"))
from compare import blur, gaussian_kernel, luminance  # noqa: E402

CELL = 16


def ssim_map(a, b):
    k = gaussian_kernel()
    c1, c2 = (0.01 * 255) ** 2, (0.03 * 255) ** 2
    mu_a, mu_b = blur(a, k), blur(b, k)
    aa, bb, ab = blur(a * a, k) - mu_a**2, blur(b * b, k) - mu_b**2, blur(a * b, k) - mu_a * mu_b
    return ((2 * mu_a * mu_b + c1) * (2 * ab + c2)) / ((mu_a**2 + mu_b**2 + c1) * (aa + bb + c2))


def loss_maps(orig, port):
    a = np.asarray(Image.open(orig).convert("RGB"), dtype=np.float64)
    b = np.asarray(Image.open(port).convert("RGB"), dtype=np.float64)
    h, w = min(a.shape[0], b.shape[0]), min(a.shape[1], b.shape[1])
    a, b = a[:h, :w], b[:h, :w]
    n = h * w
    la, lb = luminance(a), luminance(b)
    s_loss = (1 - ssim_map(la, lb)) * 5 / n
    c_loss = (np.abs(a - b).max(axis=2) > 24) * 5 / n
    # Local contrast of the original: text and icon edges are high, fills are low.
    k = gaussian_kernel()
    var = blur(la * la, k) - blur(la, k) ** 2
    edge = var > 40.0
    return s_loss, c_loss, edge


def regions(cell_loss, threshold):
    """Joins 4-connected hot cells into regions; returns (loss, y0, x0, y1, x1) in cells."""
    hot = cell_loss > threshold
    seen = np.zeros_like(hot)
    out = []
    for y, x in zip(*np.nonzero(hot)):
        if seen[y, x]:
            continue
        stack, cells = [(y, x)], []
        seen[y, x] = True
        while stack:
            cy, cx = stack.pop()
            cells.append((cy, cx))
            for ny, nx in ((cy + 1, cx), (cy - 1, cx), (cy, cx + 1), (cy, cx - 1)):
                if 0 <= ny < hot.shape[0] and 0 <= nx < hot.shape[1] and hot[ny, nx] and not seen[ny, nx]:
                    seen[ny, nx] = True
                    stack.append((ny, nx))
        ys, xs = zip(*cells)
        out.append((float(sum(cell_loss[c] for c in cells)), min(ys), min(xs), max(ys) + 1, max(xs) + 1))
    return out


def main():
    run = Path(sys.argv[1])
    top = int(sys.argv[sys.argv.index("--top") + 1]) if "--top" in sys.argv else 25
    crop_dir = Path(sys.argv[sys.argv.index("--crop") + 1]) if "--crop" in sys.argv else None
    totals = {"ssim_edge": 0.0, "ssim_flat": 0.0, "close_edge": 0.0, "close_flat": 0.0}
    found = []
    captures = sorted((run / "orig").glob("*.png"))
    for orig in captures:
        port = run / "port" / orig.name
        if not port.exists():
            continue
        s_loss, c_loss, edge = loss_maps(orig, port)
        totals["ssim_edge"] += float(s_loss[edge].sum())
        totals["ssim_flat"] += float(s_loss[~edge].sum())
        totals["close_edge"] += float(c_loss[edge].sum())
        totals["close_flat"] += float(c_loss[~edge].sum())
        loss = s_loss + c_loss
        h, w = loss.shape
        gh, gw = h // CELL, w // CELL
        cells = loss[: gh * CELL, : gw * CELL].reshape(gh, CELL, gw, CELL).sum(axis=(1, 3))
        for value, y0, x0, y1, x1 in regions(cells, threshold=0.0015):
            found.append((value, orig.stem, y0 * CELL, x0 * CELL, y1 * CELL, x1 * CELL))
    count = max(len(captures), 1)
    print(json.dumps({"captures": len(captures), "mean_loss_per_capture": {k: round(v / count, 4) for k, v in totals.items()}}))
    found.sort(reverse=True)
    for value, name, y0, x0, y1, x1 in found[:top]:
        print(f"{value:.4f}  {name:26s} y {y0}-{y1}  x {x0}-{x1}")
        if crop_dir:
            crop_dir.mkdir(parents=True, exist_ok=True)
            box = (max(x0 - 8, 0), max(y0 - 8, 0), x1 + 8, y1 + 8)
            o = Image.open(run / "orig" / f"{name}.png").convert("RGB").crop(box)
            p = Image.open(run / "port" / f"{name}.png").convert("RGB").crop(box)
            sheet = Image.new("RGB", (o.width * 2 + 4, o.height), (255, 0, 255))
            sheet.paste(o, (0, 0))
            sheet.paste(p, (o.width + 4, 0))
            scale = 3 if sheet.width < 500 else 1
            sheet = sheet.resize((sheet.width * scale, sheet.height * scale), Image.NEAREST)
            sheet.save(crop_dir / f"{value:.4f}-{name}-{y0}-{x0}.png")


if __name__ == "__main__":
    main()
