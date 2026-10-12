"""Compares parity captures: original (headless Chrome) vs port (GPUI).

Usage: python -I compare.py <orig-dir> <port-dir> <out-dir>
Writes <out-dir>/<name>.png (original | port | difference heat map) and
prints one JSON line per capture with the scores, then a summary line.

Scores (per capture):
- ssim: structural similarity on luminance (7x7 Gaussian windows), 0..1.
- close: share of pixels whose largest channel difference is <= 24 of 255.
- score: 10 * (0.5 * ssim + 0.5 * close), the UI parity score out of 10.
"""

import json
import sys
from pathlib import Path

import numpy as np
from PIL import Image


def gaussian_kernel(size=7, sigma=1.5):
    ax = np.arange(size) - size // 2
    k = np.exp(-(ax**2) / (2 * sigma**2))
    return k / k.sum()


def blur(img, k):
    # Separable convolution with edge padding.
    pad = len(k) // 2
    padded = np.pad(img, pad, mode="edge")
    tmp = np.zeros_like(img)
    for i, w in enumerate(k):
        tmp += w * padded[pad:-pad or None, i : i + img.shape[1]]
    out = np.zeros_like(img)
    padded = np.pad(tmp, pad, mode="edge")
    for i, w in enumerate(k):
        out += w * padded[i : i + img.shape[0], pad:-pad or None]
    return out


def ssim(a, b):
    k = gaussian_kernel()
    c1, c2 = (0.01 * 255) ** 2, (0.03 * 255) ** 2
    mu_a, mu_b = blur(a, k), blur(b, k)
    aa, bb, ab = blur(a * a, k) - mu_a**2, blur(b * b, k) - mu_b**2, blur(a * b, k) - mu_a * mu_b
    s = ((2 * mu_a * mu_b + c1) * (2 * ab + c2)) / ((mu_a**2 + mu_b**2 + c1) * (aa + bb + c2))
    return float(s.mean())


def luminance(rgb):
    return 0.299 * rgb[..., 0] + 0.587 * rgb[..., 1] + 0.114 * rgb[..., 2]


def compare(orig_path, port_path, out_path):
    a = np.asarray(Image.open(orig_path).convert("RGB"), dtype=np.float64)
    b = np.asarray(Image.open(port_path).convert("RGB"), dtype=np.float64)
    if a.shape != b.shape:
        h, w = min(a.shape[0], b.shape[0]), min(a.shape[1], b.shape[1])
        size_note = f"size {a.shape[1]}x{a.shape[0]} vs {b.shape[1]}x{b.shape[0]}"
        a, b = a[:h, :w], b[:h, :w]
    else:
        size_note = ""
    diff = np.abs(a - b).max(axis=2)
    close = float((diff <= 24).mean())
    s = ssim(luminance(a), luminance(b))
    heat = np.zeros_like(a)
    heat[..., 0] = np.clip(diff * 3, 0, 255)
    heat[..., 1] = np.clip(255 - diff * 3, 0, 255) * (diff > 24)
    heat = np.where((diff <= 24)[..., None], a * 0.25, heat)
    strip = np.concatenate([a, b, heat], axis=1).astype(np.uint8)
    Image.fromarray(strip).save(out_path)
    return {"ssim": round(s, 4), "close": round(close, 4), "score": round(10 * (0.5 * s + 0.5 * close), 2), "note": size_note}


def main():
    orig_dir, port_dir, out_dir = map(Path, sys.argv[1:4])
    out_dir.mkdir(parents=True, exist_ok=True)
    scores = []
    for orig in sorted(orig_dir.glob("*.png")):
        port = port_dir / orig.name
        if not port.exists():
            print(json.dumps({"capture": orig.stem, "error": "missing port capture"}))
            scores.append(0.0)
            continue
        result = compare(orig, port, out_dir / orig.name)
        scores.append(result["score"])
        print(json.dumps({"capture": orig.stem, **result}))
    if scores:
        print(json.dumps({"summary": True, "captures": len(scores), "mean": round(sum(scores) / len(scores), 2), "min": min(scores)}))


if __name__ == "__main__":
    main()
