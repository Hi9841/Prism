"""Crops the same region from two captures and stacks them, scaled up.

Usage: python -I crop.py <orig.png> <port.png> <x> <y> <w> <h> <scale> <out.png>
Top: original. Bottom: port. A 1px red line separates them.
"""

import sys

from PIL import Image

orig, port, x, y, w, h, scale, out = sys.argv[1:9]
x, y, w, h, scale = int(x), int(y), int(w), int(h), int(scale)
a = Image.open(orig).convert("RGB").crop((x, y, x + w, y + h)).resize((w * scale, h * scale), Image.NEAREST)
b = Image.open(port).convert("RGB").crop((x, y, x + w, y + h)).resize((w * scale, h * scale), Image.NEAREST)
canvas = Image.new("RGB", (w * scale, h * scale * 2 + 2), (255, 0, 0))
canvas.paste(a, (0, 0))
canvas.paste(b, (0, h * scale + 2))
canvas.save(out)
