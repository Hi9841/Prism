"""Offset (dx, dy) of the port against the original for each text line in a column range.
Usage: python -I line_shift.py <run-dir> <capture> [x0 x1]. Positive dx = the port is right, dy = lower.
A line is a run of rows with ink in the original; the offset is the best of a 0.25 px grid search."""
import sys

import numpy as np
from PIL import Image


def shifted(img, dy, dx):
    """Bilinear sample of img moved by (dy, dx)."""
    h, w = img.shape
    ys, xs = np.arange(h) + dy, np.arange(w) + dx
    y0, x0 = np.clip(np.floor(ys).astype(int), 0, h - 2), np.clip(np.floor(xs).astype(int), 0, w - 2)
    fy, fx = (ys - np.floor(ys))[:, None], (xs - np.floor(xs))[None, :]
    a, b = img[y0][:, x0], img[y0][:, x0 + 1]
    c, d = img[y0 + 1][:, x0], img[y0 + 1][:, x0 + 1]
    return (a * (1 - fx) + b * fx) * (1 - fy) + (c * (1 - fx) + d * fx) * fy


def lines(img, x0, x1):
    ink = img[:, x0:x1].std(axis=1) > 8
    out, start = [], None
    for y, on in enumerate(ink):
        if on and start is None:
            start = y
        if not on and start is not None:
            if y - start >= 6:
                out.append((start, y))
            start = None
    return out


def main():
    run, name = sys.argv[1], sys.argv[2]
    x0, x1 = (int(sys.argv[3]), int(sys.argv[4])) if len(sys.argv) > 4 else (300, 620)
    orig = np.asarray(Image.open(f"{run}/orig/{name}.png").convert("L"), float)
    port = np.asarray(Image.open(f"{run}/port/{name}.png").convert("L"), float)
    grid = [k / 4 for k in range(-12, 13)]
    for top, bottom in lines(orig, x0, x1):
        y0, y1 = max(top - 3, 4), min(bottom + 3, orig.shape[0] - 4)
        a = orig[y0:y1, x0:x1]
        # The search moves at most 3 px, so a 4 px margin around the line is enough.
        m0, n0 = y0 - 4, max(x0 - 4, 0)
        window = port[m0 : y1 + 4, n0 : x1 + 4]
        best = min(
            (np.abs(a - shifted(window, dy, dx)[y0 - m0 : y1 - m0, x0 - n0 : x1 - n0]).mean(), dx, dy)
            for dy in grid
            for dx in grid
        )
        print(f"y {top}-{bottom}: dx {best[1]:+.2f} dy {best[2]:+.2f} (err {best[0]:.2f})")


if __name__ == "__main__":
    main()
