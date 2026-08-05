"""Pixel gate for the Calibre-style common-phase speckle fill.

Checks KLayout's real compositor, including a hollow FRAME-like underlay:
all design layers must use one common 50% checkerboard phase, hiding an
intermediate layer must not move that phase, and overlapping fills must
leave exactly half of the background pixels open without alpha blending.
Also gates the coverage composite against the speckle: density tint may
only land on true background, never inside a speckled polygon (the
per-pixel darkness test once flooded 50% of every drawn interior).
"""

import io
import os
import sys
import tempfile

import klayout.db as db

sys.path.insert(0, os.path.dirname(os.path.dirname(
    os.path.abspath(__file__))))

from floe.render import Renderer


W = H = 100
BLACK = 0xff000000


def image(renderer, visible=None):
    renderer.set_visible(visible)
    return renderer.lv.get_pixels_with_options(
        W, H, 1, 1, 1.0, db.DBox(0, 0, W, H))


def occupied_phase(pixels, x0, y0, x1, y1):
    return {(x & 1, y & 1)
            for y in range(y0, y1)
            for x in range(x0, x1)
            if pixels.pixel(x, y) != BLACK}


def main():
    ly = db.Layout()
    ly.dbu = 1.0
    top = ly.create_cell("TOP")
    frame = (99, 0)
    designs = [(10, 0), (20, 0), (30, 0)]
    colors = {frame: "#808080",
              designs[0]: "#ff0000",
              designs[1]: "#00ff00",
              designs[2]: "#0000ff"}

    # The frame source sorts last; Renderer must first move it under the
    # design planes, then compensate phase using the resulting paint order.
    top.shapes(ly.layer(db.LayerInfo(*frame))).insert(
        db.Box(1, 1, 99, 99))
    for i, key in enumerate(designs):
        li = ly.layer(db.LayerInfo(*key))
        top.shapes(li).insert(db.Box(5 + i * 25, 10,
                                     20 + i * 25, 90))
        top.shapes(li).insert(db.Box(20, 30, 80, 70))

    renderer = Renderer(ly, top, colors, hollow=(frame,))
    try:
        pixels = image(renderer, designs)
        phases = [occupied_phase(pixels, 8 + i * 25, 12,
                                 17 + i * 25, 25)
                  for i in range(3)]
        expected = {(0, 0), (1, 1)}
        if not all(phase == expected for phase in phases):
            raise SystemExit("speckle phase mismatch: %r" % phases)

        # Visibility changes must not renumber the surviving paint planes.
        pixels = image(renderer, (designs[0], designs[2]))
        phases = [occupied_phase(pixels, 8, 12, 17, 25),
                  occupied_phase(pixels, 58, 12, 67, 25)]
        if not all(phase == expected for phase in phases):
            raise SystemExit("visibility shifted speckle: %r" % phases)

        # In the three-layer overlap, precisely one parity is colored and
        # the other remains black. Non-black pixels must be the upper blue,
        # never an alpha/additive mixture.
        pixels = image(renderer, designs)
        # Stay away from the three diagnostic strip borders: their
        # continuous frames intentionally add other layer colors.
        overlap = [pixels.pixel(x, y)
                   for y in range(40, 50) for x in range(46, 54)]
        colored = [p for p in overlap if p != BLACK]
        if len(colored) != 40 or set(colored) != {0xff0000ff}:
            raise SystemExit("overlap is not opaque 50%% speckle: %r" %
                             sorted(set(overlap)))

        # coverage composite vs the speckle: tint fills background
        # only - a speckled interior has near-black pixels at every
        # other position, and none of them may be repainted
        import numpy as np
        from PIL import Image
        from floe.coverage import composite
        renderer.set_visible(designs)
        with tempfile.NamedTemporaryFile(suffix=".png") as f:
            renderer.render_png(f.name, 0, 0, W, H, W, H,
                                visible=designs, depth=None)
            before = np.asarray(
                Image.open(f.name).convert("RGB")).copy()
            tint = np.full((H, W, 3), (7, 99, 55), dtype=np.uint8)
            after = np.asarray(Image.open(io.BytesIO(
                composite(f.name, tint))).convert("RGB"))
        changed = (after != before).any(axis=2)
        # interior of the first strip (render_png flips y vs the
        # get_pixels path; the strip spans y 10..90, safely inside)
        if changed[40:60, 9:16].any():
            raise SystemExit(
                "coverage tint flooded a speckled interior")
        # true background well away from any drawn frame
        if not changed[2:5, 2:5].all():
            raise SystemExit("coverage tint missed real background")
        print("render-speckle: common phase, visibility, opaque "
              "overlap, coverage-composite containment OK")
    finally:
        renderer.lv._destroy()


if __name__ == "__main__":
    main()
