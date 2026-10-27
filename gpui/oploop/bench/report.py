# /// script
# requires-python = ">=3.12"
# dependencies = [
#   "numpy", "pillow",
#   "winrt-runtime", "winrt-Windows.Foundation", "winrt-Windows.Foundation.Collections",
#   "winrt-Windows.Globalization", "winrt-Windows.Graphics.Imaging", "winrt-Windows.Media.Ocr",
#   "winrt-Windows.Storage", "winrt-Windows.Storage.Streams",
# ]
# ///
"""Summarize raw bench data into Markdown tables.

    uv run gpui/oploop/bench/report.py <raw run dir> [<raw j4 dir>]

Writes `summary.md` and `j5-rows.json` into the run dir, and side-by-side
PNGs (`frames/pair-qNN.png`) for every query that has a J5 difference.
J5 reads the visible rows with Windows OCR (Windows.Media.Ocr) from the
settled PrintWindow frames that `bench.py run` kept.
"""

from __future__ import annotations

import asyncio
import difflib
import json
import math
import re
import sys
from collections import defaultdict
from pathlib import Path

import numpy as np
from PIL import Image, ImageOps

APPS = ("gpui", "tauri")


# ---------- statistics ----------

def stats(values: list[float]) -> dict:
    v = sorted(x for x in values if x is not None)
    if not v:
        return {"n": 0}
    p95 = v[max(0, math.ceil(0.95 * len(v)) - 1)]
    return {"n": len(v), "median": float(np.median(v)), "p95": p95, "min": v[0], "max": v[-1]}


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


def fmt(s: dict, unit: str = "") -> str:
    if not s.get("n"):
        return "n/a"
    return f"{s['median']:.0f}{unit} (p95 {s['p95']:.0f}, {s['min']:.0f}-{s['max']:.0f}, n={s['n']})"


def block_spread(rows: list[dict], key: str) -> str:
    """Range of the per-block medians: the drift between sessions."""
    by_block = defaultdict(list)
    for r in rows:
        if r.get(key) is not None:
            by_block[r["block"]].append(r[key])
    medians = [float(np.median(v)) for v in by_block.values() if v]
    if not medians:
        return "n/a"
    return f"{min(medians):.0f}-{max(medians):.0f}"


# ---------- J2 and J3 ----------

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


def latency_tables(rows: list[dict]) -> list[str]:
    out = []
    j2 = [r for r in rows if r["journey"] == "j2"]
    out.append("### J2 open latency (ms after the activation signal)\n")
    out.append("| Measure | GPUI | Tauri (original) | p (rank-sum) |")
    out.append("|---|---|---|---|")
    for label, key, warm in [
        ("Window visible (warm)", "visible_ms", True),
        ("First painted frame (warm)", "first_ms", True),
        ("Settled frame (warm)", "settled_ms", True),
        ("First painted frame (first open after start)", "first_ms", False),
        ("Settled frame (first open after start)", "settled_ms", False),
    ]:
        sel = {a: [r[key] for r in j2 if r["app"] == a and r.get("cold") != warm] for a in APPS}
        p = rank_sum_p(sel["gpui"], sel["tauri"])
        out.append(f"| {label} | {fmt(stats(sel['gpui']))} | {fmt(stats(sel['tauri']))} | {p:.2g} |")
    for a in APPS:
        warm = [r for r in j2 if r["app"] == a and not r.get("cold")]
        fg = sum(1 for r in j2 if r["app"] == a and r["foreground"])
        out.append("")
        out.append(f"- {a}: per-session median of the warm first frame: {block_spread(warm, 'first_ms')} ms; "
                   f"palette took the foreground in {fg}/{sum(1 for r in j2 if r['app'] == a)} opens.")
    out.append("")

    j3 = [r for r in rows if r["journey"] == "j3"]
    queries = list(dict.fromkeys(r["query"] for r in j3))
    for r in j3:
        r["rows_ms"] = rows_done(r)
    for key, title in [("first_ms", "first changed frame (the query echo)"),
                       ("rows_ms", "rows done (last change before the first 300 ms quiet gap)"),
                       ("settled_ms", "settled frame (last change of all, limit 5 s)")]:
        out.append(f"### J3 type latency: {title}, ms after the posted keys\n")
        out.append("| Query | GPUI | Tauri (original) | GPUI / Tauri | p |")
        out.append("|---|---|---|---|---|")
        all_g, all_t = [], []
        for q in queries:
            g = [r[key] for r in j3 if r["app"] == "gpui" and r["query"] == q]
            t = [r[key] for r in j3 if r["app"] == "tauri" and r["query"] == q]
            all_g += g
            all_t += t
            sg, st = stats(g), stats(t)
            ratio = f"{sg['median'] / st['median']:.2f}" if sg.get("n") and st.get("n") and st["median"] else "n/a"
            out.append(f"| `{q}` | {fmt(sg)} | {fmt(st)} | {ratio} | {rank_sum_p(g, t):.2g} |")
        sg, st = stats(all_g), stats(all_t)
        out.append(f"| all queries | {fmt(sg)} | {fmt(st)} | {sg['median'] / st['median']:.2f} | |")
        out.append("")
        for a in APPS:
            sel = [r for r in j3 if r["app"] == a]
            out.append(f"- {a}: per-session median over all queries: {block_spread(sel, key)} ms.")
        out.append("")
    # Backend share from the apps' own PRISM_PERF_LOG events.
    out.append("### J3 backend time inside each measurement (PRISM_PERF_LOG, ms, median)\n")
    out.append("| Query | GPUI query_phase1 | Tauri query_phase1 | GPUI search_files | Tauri search_files |")
    out.append("|---|---|---|---|---|")
    for q in queries:
        cells = []
        for event in ("query_phase1", "search_files"):
            for a in APPS:
                v = [r["perf"].get(event, 0.0) for r in j3 if r["app"] == a and r["query"] == q]
                cells.append(f"{np.median(v):.1f}" if v else "n/a")
        out.append(f"| `{q}` | {cells[0]} | {cells[1]} | {cells[2]} | {cells[3]} |")
    out.append("")
    out.append(f"### J3 late changes (a change more than {QUIET_GAP_MS} ms after the rows were done)\n")
    out.append("| App | Query | Runs with late changes | Late change times (ms) | Changed rows (y px) |")
    out.append("|---|---|---|---|---|")
    any_late = False
    for a in APPS:
        for q in queries:
            runs = [r for r in j3 if r["app"] == a and r["query"] == q]
            late = [r for r in runs if r["rows_ms"] is not None and r["settled_ms"] - r["rows_ms"] > QUIET_GAP_MS]
            if not late:
                continue
            any_late = True
            times = sorted(c[0] for r in late for c in r["changes"] if c[0] > r["rows_ms"])
            ys = {(c[2], c[3]) for r in late for c in r["changes"] if c[0] > r["rows_ms"]}
            out.append(f"| {a} | `{q}` | {len(late)}/{len(runs)} | {times[0]:.0f}-{times[-1]:.0f} | "
                       + ", ".join(f"{y0}-{y1}" for y0, y1 in sorted(ys)[:4]) + " |")
    if not any_late:
        out.append("| none | | | | |")
    out.append("")
    loads = [r.get("machine_cpu_pct") for r in rows if r.get("machine_cpu_pct") is not None]
    if loads:
        out.append(f"Machine CPU load before each measurement: median {np.median(loads):.0f}%, max {max(loads):.0f}%.")
        out.append("")
    return out


# ---------- J4 ----------

def j4_table(rows: list[dict]) -> list[str]:
    out = ["### J4 resources (whole process tree; Tauri includes its WebView2 processes)\n"]
    out.append("| Measure | GPUI | Tauri (original) |")
    out.append("|---|---|---|")
    mib = 2**20

    def col(app, phase, key, scale=1.0):
        return stats([r[phase][key] / scale for r in rows if r["app"] == app])

    def cell(s, digits=0):
        if not s.get("n"):
            return "n/a"
        f = f"{{:.{digits}f}}"
        return f"{f.format(s['median'])} ({f.format(s['min'])}-{f.format(s['max'])}, n={s['n']})"

    idle = rows[0]["idle_s"] if rows else 60
    cycles = rows[0]["cycles"] if rows else 20
    for phase, label in [("after_start", f"{idle:.0f} s after start (hidden)"),
                         ("after_cycles_idle", f"after {cycles} open/type/close cycles + {idle:.0f} s idle")]:
        for key, name, scale, digits in [("private", "private bytes, MiB", mib, 0), ("wset", "working set, MiB", mib, 0),
                                         ("handles", "handles", 1, 0), ("threads", "threads", 1, 0),
                                         ("processes", "processes", 1, 0)]:
            out.append(f"| {name}, {label} | {cell(col('gpui', phase, key, scale), digits)} | "
                       f"{cell(col('tauri', phase, key, scale), digits)} |")
    for a_phase, b_phase, label in [
        (None, "after_start", f"CPU time, start to {idle:.0f} s (s)"),
        ("after_start", "after_idle2", f"CPU time, {idle:.0f} s hidden idle after that (s)"),
        ("after_cycles", "after_cycles_idle", f"CPU time, {idle:.0f} s hidden idle after the cycles (s)"),
    ]:
        cells = []
        for app in APPS:
            v = [r[b_phase]["cpu"] - (r[a_phase]["cpu"] if a_phase else 0) for r in rows if r["app"] == app]
            cells.append(cell(stats(v), 2))
        out.append(f"| {label} | {cells[0]} | {cells[1]} |")
    for app in APPS:
        deltas = [(r["after_cycles_idle"]["private"] - r["after_idle2"]["private"]) / mib for r in rows if r["app"] == app]
        hdeltas = [r["after_cycles_idle"]["handles"] - r["after_idle2"]["handles"] for r in rows if r["app"] == app]
        if deltas:
            out.append("")
            out.append(f"- {app}: private bytes change across the {cycles} cycles: "
                       + ", ".join(f"{d:+.1f} MiB" for d in deltas)
                       + "; handles change: " + ", ".join(f"{d:+d}" for d in hdeltas))
    out.append("")
    return out


# ---------- J5 ----------

async def _ocr(path: Path):
    from winrt.windows.graphics.imaging import BitmapDecoder
    from winrt.windows.media.ocr import OcrEngine
    from winrt.windows.storage import FileAccessMode, StorageFile

    engine = OcrEngine.try_create_from_user_profile_languages()
    file = await StorageFile.get_file_from_path_async(str(path))
    stream = await file.open_async(FileAccessMode.READ)
    decoder = await BitmapDecoder.create_async(stream)
    bitmap = await decoder.get_software_bitmap_async()
    result = await engine.recognize_async(bitmap)
    lines = []
    for line in result.lines:
        words = list(line.words)
        x = min(w.bounding_rect.x for w in words)
        y = min(w.bounding_rect.y for w in words)
        lines.append({"text": line.text, "x": x, "y": y})
    return lines


SCALE = 3
TEXT_LEVEL = 85  # grey level above which a pixel is text on the dark palette


def text_bands(gray: np.ndarray) -> list[tuple[int, int]]:
    """Horizontal bands that hold text. Icons sit left of x=64, so the band
    search skips that column; a row's title and subtitle become two bands."""
    h, w = gray.shape
    hits = (gray[:, 64:w - 12] > TEXT_LEVEL).sum(axis=1) > 0
    bands, start = [], None
    for y, hit in enumerate(hits):
        if hit and start is None:
            start = y
        elif not hit and start is not None:
            if y - start >= 5:
                bands.append((start, y))
            start = None
    if start is not None and h - start >= 5:
        bands.append((start, h))
    return bands


def segments(gray: np.ndarray, top: int, bottom: int, x_from: int, x_to: int, gap: int = 16) -> list[tuple[int, int]]:
    """Column runs of text in one band, split where no text runs for `gap` px."""
    cols = (gray[top:bottom, x_from:x_to] > TEXT_LEVEL).sum(axis=0) > 0
    runs, start, last = [], None, None
    for x, hit in enumerate(cols):
        if hit:
            if start is None:
                start = x
            elif x - last > gap:
                runs.append((start + x_from, last + x_from + 1))
                start = x
            last = x
    if start is not None:
        runs.append((start + x_from, last + x_from + 1))
    return runs


def load_gray(png: Path) -> Image.Image:
    image = Image.open(png).convert("RGBA")
    flat = Image.new("RGB", image.size, (0, 0, 0))
    flat.paste(image, mask=image.split()[3])
    return ImageOps.grayscale(flat)


def regions(grays: list[Image.Image]) -> list[tuple[int, int, int, int]]:
    """Text boxes (top, bottom, left, right) of the union of the frames.

    Every frame is then read in the same boxes, so two apps (or two runs)
    are compared on the same screen region. Icons sit left of x=64: that
    column is split off, so an icon never joins a text box.
    """
    w = min(g.width for g in grays)
    h = min(g.height for g in grays)
    union = np.maximum.reduce([np.asarray(g)[:h, :w] for g in grays])
    boxes = []
    for top, bottom in text_bands(union):
        for x_from, x_to in ((12, 64), (64, w - 12)):
            for left, right in segments(union, top, bottom, x_from, x_to):
                boxes.append((top, bottom, left, right))
    return boxes


def ocr_box(gray: Image.Image, box: tuple[int, int, int, int], target: Path) -> str:
    top, bottom, left, right = box
    strip = gray.crop((max(0, left - 4), max(0, top - 5), min(gray.width, right + 4), min(gray.height, bottom + 5)))
    # Dark text on a light page, three times the size: Windows OCR reads it best.
    strip = ImageOps.invert(strip).resize((strip.width * SCALE, strip.height * SCALE), Image.LANCZOS)
    page = Image.new("L", (strip.width + 80, strip.height + 40), 255)
    page.paste(strip, (40, 20))
    page.save(target)
    return " ".join(line["text"] for line in asyncio.run(_ocr(target.resolve()))).strip()


def norm(text: str) -> str:
    text = text.lower().replace("…", "").replace("...", "").replace("|", "")
    return re.sub(r"\s+", " ", text).strip(" .")


def same_text(a: str, b: str) -> str:
    """'same', 'ocr-noise' (90% alike), 'truncated' (one is a prefix of the
    other), or 'different'."""
    na, nb = norm(a), norm(b)
    if na == nb:
        return "same"
    short, long = sorted((na, nb), key=len)
    if short and len(short) >= 8 and long.startswith(short[: max(8, len(short) - 2)]):
        return "truncated"
    if difflib.SequenceMatcher(None, na, nb).ratio() >= 0.9:
        return "ocr-noise"
    return "different"


# The search box only echoes the query (J3 measures the echo). Its blinking
# caret reads as "l", "1", or "d" at the end of the query, so J5 skips it.
SEARCH_BOX_BOTTOM = 80


def is_glyph_noise(a: str, b: str) -> bool:
    """Icons and key caps read as one or two random characters."""
    return len(norm(a)) <= 2 and len(norm(b)) <= 2


def j5(run_dir: Path, queries: list[str]) -> tuple[list[str], dict]:
    frames = run_dir / "frames"
    target = frames / "ocr-tmp.png"
    data: dict = {}
    out = ["### J5 visible rows: GPUI against Tauri (Windows OCR of the settled frames)\n"]
    for qi, query in enumerate(queries):
        pngs = {app: sorted(frames.glob(f"{app}-q{qi:02d}-r*.png")) for app in APPS}
        if not pngs["gpui"] or not pngs["tauri"]:
            out.append(f"- `{query}`: no frames")
            continue
        grays = {p: load_gray(p) for app in APPS for p in pngs[app]}
        boxes = regions(list(grays.values()))
        texts = {p.name: [ocr_box(g, box, target) for box in boxes] for p, g in grays.items()}
        g0, t0 = pngs["gpui"][0].name, pngs["tauri"][0].name
        rows = []
        for i, (top, _, left, _) in enumerate(boxes):
            g, t = texts[g0][i], texts[t0][i]
            if (not g and not t) or is_glyph_noise(g, t) or top < SEARCH_BOX_BOTTOM:
                continue
            rows.append({"y": top, "x": left, "gpui": g, "tauri": t, "kind": same_text(g, t) if g and t else
                         ("only gpui" if g else "only tauri")})
        diffs = [r for r in rows if r["kind"] not in ("same", "ocr-noise")]
        stable = {}
        for app in APPS:
            first = texts[pngs[app][0].name]
            same_frames = 0
            for p in pngs[app]:
                kinds = [same_text(a, b) for box, a, b in zip(boxes, first, texts[p.name])
                         if (a or b) and not is_glyph_noise(a, b) and box[0] >= SEARCH_BOX_BOTTOM]
                same_frames += all(k in ("same", "ocr-noise") for k in kinds)
            stable[app] = f"{same_frames}/{len(pngs[app])}"
        data[query] = {"frames": {k: v for k, v in texts.items()}, "boxes": boxes, "rows": rows}
        kinds = defaultdict(int)
        for r in diffs:
            kinds[r["kind"]] += 1
        summary = ", ".join(f"{k}: {v}" for k, v in sorted(kinds.items())) or "none"
        out.append(f"#### `{query}`: {len(rows)} text boxes, differences: {summary} "
                   f"(frames that read the same as the app's first frame: GPUI {stable['gpui']}, "
                   f"Tauri {stable['tauri']})\n")
        if diffs:
            out.append("| y | x | GPUI | Tauri | kind |")
            out.append("|---|---|---|---|---|")
            for r in diffs:
                g = r["gpui"].replace("|", "\\|")
                t = r["tauri"].replace("|", "\\|")
                out.append(f"| {r['y']} | {r['x']} | {g} | {t} | {r['kind']} |")
            out.append("")
        g_img = Image.open(pngs["gpui"][0])
        t_img = Image.open(pngs["tauri"][0])
        pair = Image.new("RGBA", (g_img.width + t_img.width + 10, max(g_img.height, t_img.height)), (255, 255, 255, 255))
        pair.paste(g_img, (0, 0))
        pair.paste(t_img, (g_img.width + 10, 0))
        pair.save(frames / f"pair-q{qi:02d}.png")
        out.append(f"Side by side (GPUI left, Tauri right): `frames/pair-q{qi:02d}.png`\n")
    target.unlink(missing_ok=True)
    return out, data


def main() -> None:
    run_dir = Path(sys.argv[1])
    lines = [f"# Bench summary: {run_dir.name}\n"]
    rows = json.loads((run_dir / "rows.json").read_text(encoding="utf8"))
    meta = json.loads((run_dir / "meta.json").read_text(encoding="utf8"))
    lines += latency_tables(rows)
    if len(sys.argv) > 2:
        j4_dir = Path(sys.argv[2])
        lines += j4_table(json.loads((j4_dir / "rows.json").read_text(encoding="utf8")))
    j5_lines, j5_data = j5(run_dir, meta["queries"])
    lines += j5_lines
    (run_dir / "j5-rows.json").write_text(json.dumps(j5_data, indent=1, ensure_ascii=False), encoding="utf8")
    (run_dir / "summary.md").write_text("\n".join(lines) + "\n", encoding="utf8")
    print("\n".join(lines))


if __name__ == "__main__":
    main()
