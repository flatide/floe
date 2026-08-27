"""Hairline A/B: KLayout vs Rust raster at a 100um-view scale.

Field report 2026-08-27: at ~100um viewports the two products render
hairlines visibly differently. This repro builds a fixture whose
features sit at/below one device pixel (858px over 100um -> 1px ~
0.117um), renders the SAME view through the stable KLayout backend
and floe-render-cli, and reports per-layer binary-coverage diffs
(interior vs 1px-band) plus diff PNGs (magenta = rust extra, cyan =
rust missing). Measured classes and the root cause live in
docs/RENDERER-TESTS.ko.md (pixel policy section).

Usage: .venv/bin/python tools/hairline_ab.py <workdir>

Layers:
  1/0  H/V thin wires 0.03um wide + a dense 0.5um-pitch array
  2/0  diagonal thin wires (45deg and shallow) as polygons
  3/0  0.05um boxes on a 1um grid (sub-pixel both sides)
"""

import subprocess
import sys
from pathlib import Path

import numpy as np
from PIL import Image

ROOT = Path.cwd()
assert (ROOT / "floe").is_dir(), "run from the repo root"
sys.path.insert(0, str(ROOT))

SCRATCH = Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT
WORK = SCRATCH / "hairline-ab"
WORK.mkdir(exist_ok=True)

VIEW = (0.0, 0.0, 100.0, 92.0)
WIDTH, HEIGHT = 858, 789
COLORS = {(1, 0): "#ef3340", (2, 0): "#35d04f", (3, 0): "#3578ff"}


def build_fixture(path):
    import klayout.db as db

    ly = db.Layout()
    ly.dbu = 0.001
    top = ly.create_cell("HAIR")
    l1 = ly.layer(1, 0)
    l2 = ly.layer(2, 0)
    l3 = ly.layer(3, 0)

    # L1: axis-aligned thin wires, 0.03um wide, 30um long
    for i in range(8):
        y = 5_000 + i * 4_000
        top.shapes(l1).insert(db.Box(5_000, y, 35_000, y + 30))
    for i in range(8):
        x = 5_000 + i * 4_000
        top.shapes(l1).insert(db.Box(x, 40_000, x + 30, 70_000))
    # dense array: 60 vertical wires at 0.5um pitch (texture patch)
    for i in range(60):
        x = 45_000 + i * 500
        top.shapes(l1).insert(db.Box(x, 5_000, x + 30, 35_000))

    # L2: diagonal thin wires as 4-pt polygons (45deg and shallow)
    def thin_diag(x0, y0, x1, y1, half):
        pts = [db.Point(x0 - half, y0 + half), db.Point(x1 - half, y1 + half),
               db.Point(x1 + half, y1 - half), db.Point(x0 + half, y0 - half)]
        top.shapes(l2).insert(db.Polygon(pts))

    for i in range(6):
        base = 45_000 + i * 5_000
        thin_diag(base, 40_000, base + 25_000, 65_000, 15)      # 45deg
        thin_diag(base, 68_000, base + 25_000, 72_000, 15)      # shallow

    # L3: 0.05um boxes on a 1um grid
    for gy in range(15):
        for gx in range(20):
            x = 5_000 + gx * 1_000
            y = 74_000 + gy * 1_000
            top.shapes(l3).insert(db.Box(x, y, x + 50, y + 50))

    options = db.SaveLayoutOptions()
    options.format = "OASIS"
    options.oasis_compression_level = 2
    options.oasis_strict_mode = True
    ly.write(str(path), options)


def render_klayout(source, output, visible):
    import klayout.db as db

    from floe.render import Renderer

    layout = db.Layout(False)
    layout.read(str(source))
    renderer = Renderer(layout, layout.top_cell(), COLORS)
    try:
        renderer.set_fill_patterns({key: "solid" for key in COLORS})
        renderer.set_line_widths({key: 1 for key in COLORS})
        renderer.set_mono(False)
        renderer.set_visible(visible)
        renderer.lv.save_image_with_options(
            str(output), WIDTH, HEIGHT, 1, 1, 0, db.DBox(*VIEW), False)
    finally:
        renderer.lv._destroy()


def render_rust(cache, output, styles):
    command = [
        str(ROOT / "rust/target/release/floe-render-cli"), str(cache),
        "--view", ",".join(repr(v) for v in VIEW),
        "--width", str(WIDTH), "--height", str(HEIGHT),
        "--depth", "full", "--cut-px", "0",
        "--decode-pages", "1000000000", "--budget-mb", "1024",
        "--jobs", "1", "--tile-px", "1024", "--frames", "off",
        "--mono", "off", "--out", str(output),
    ]
    for style in styles:
        command.extend(("--style", style))
    subprocess.run(command, check=True, capture_output=True, text=True)


def coverage(path):
    rgb = np.asarray(Image.open(path).convert("RGB"))
    return np.any(rgb != 0, axis=2)


def dilate(mask, radius=1):
    result = mask.copy()
    for dy in (-1, 0, 1):
        for dx in (-1, 0, 1):
            shifted = np.zeros_like(mask)
            ys = slice(max(dy, 0), mask.shape[0] + min(dy, 0))
            yd = slice(max(-dy, 0), mask.shape[0] + min(-dy, 0))
            xs = slice(max(dx, 0), mask.shape[1] + min(dx, 0))
            xd = slice(max(-dx, 0), mask.shape[1] + min(-dx, 0))
            shifted[yd, xd] = mask[ys, xs]
            result |= shifted
    return result


def main():
    source = WORK / "hair.oas"
    cache = WORK / "hair.oas.floe"
    build_fixture(source)
    subprocess.run([
        str(ROOT / "rust/target/release/floe-index"), "vfs",
        str(source), str(cache)], check=True, capture_output=True)

    cases = {
        "l1-axis": ([(1, 0)], ("1/0,#ef3340,solid,1",)),
        "l2-diag": ([(2, 0)], ("2/0,#35d04f,solid,1",)),
        "l3-dots": ([(3, 0)], ("3/0,#3578ff,solid,1",)),
        "all": (list(COLORS), tuple(
            "%d/%d,%s,solid,1" % (l, d, COLORS[(l, d)])
            for (l, d) in sorted(COLORS))),
    }
    for name, (visible, styles) in cases.items():
        kl = WORK / ("kl-%s.png" % name)
        ru = WORK / ("ru-%s.png" % name)
        render_klayout(source, kl, visible)
        render_rust(cache, ru, styles)
        golden = coverage(kl)
        cand = coverage(ru)
        diff = golden ^ cand
        band = dilate(golden) & ~golden | (golden & ~dilate(~golden))
        boundary = dilate(golden) ^ golden | golden ^ (
            golden & dilate(golden))
        # interior diff = diff pixels farther than 1px from golden edge
        edge = golden ^ (golden & ~dilate(~golden))
        near_edge = dilate(dilate(golden) & ~golden | edge)
        interior = diff & ~near_edge
        extra = (cand & ~golden).sum()
        missing = (golden & ~cand).sum()
        print("%-8s golden_on=%6d rust_on=%6d diff=%6d "
              "(extra=%d missing=%d interior=%d)" % (
                  name, golden.sum(), cand.sum(), diff.sum(),
                  extra, missing, interior.sum()))
        image = np.zeros((*golden.shape, 3), dtype=np.uint8)
        image[golden] = (70, 70, 70)
        image[cand & ~golden] = (255, 0, 255)   # rust extra: magenta
        image[golden & ~cand] = (0, 255, 255)   # rust missing: cyan
        Image.fromarray(image, "RGB").save(WORK / ("diff-%s.png" % name))


if __name__ == "__main__":
    main()
