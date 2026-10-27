"""Vertical offset of the port against the original per 32 px band (cross-correlation, 0.1 px steps).
Usage: python -I band_shift.py <run-dir> <capture> [x0 x1]. Positive = the port is lower."""
import sys
import numpy as np
from PIL import Image
run, name = sys.argv[1], sys.argv[2]
x0, x1 = (int(sys.argv[3]), int(sys.argv[4])) if len(sys.argv) > 4 else (300, 620)
O = np.asarray(Image.open(f"{run}/orig/{name}.png").convert("L"), float)
P = np.asarray(Image.open(f"{run}/port/{name}.png").convert("L"), float)
out = []
for y0 in range(8, O.shape[0] - 40, 16):
    a = O[y0:y0 + 16, x0:x1]
    if a.std() < 6:
        continue
    best = min(((np.abs(a - (P[np.floor(np.arange(y0, y0 + 16) + d).astype(int), x0:x1] * (1 - ((np.arange(y0, y0 + 16) + d) % 1))[:, None]
                         + P[np.floor(np.arange(y0, y0 + 16) + d).astype(int) + 1, x0:x1] * ((np.arange(y0, y0 + 16) + d) % 1)[:, None])).mean(), d)
                for d in [k / 10 for k in range(-25, 26)]))
    out.append(f"{y0}:{best[1]:+.1f}")
print(name, " ".join(out))
