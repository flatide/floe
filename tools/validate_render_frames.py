"""Pixel gate for hierarchy-frame outline stacking.

The viewer draws depth-boundary frames on two hollow planes: white
(FRAME_LAYER, dt 0) and gray (FRAME_GRAY, dt 1). KLayout's default
property sorting paints gray over white; the Renderer must instead
order the planes [gray, design geometry, white]: white outlines stay
readable ON TOP of dense fill (and above gray wherever they touch),
gray dust stays buried UNDER the geometry, and every frame line is a
fixed 1px with no fill. Solid fills (speckle=False) keep the
over/under-design checks deterministic; the hollow planes ignore the
speckle flag, so the stacking behavior gated here is the live
viewer's.
"""

import os
import sys

import klayout.db as db

sys.path.insert(0, os.path.dirname(os.path.dirname(
    os.path.abspath(__file__))))

from floe.render import Renderer


W = H = 100
WHITE = 0xffffffff
GRAY = 0xff808080


def pixels(renderer, visible):
    renderer.set_visible(visible)
    return renderer.lv.get_pixels_with_options(
        W, H, 1, 1, 1.0, db.DBox(0, 0, W, H))


def hits(img, argb):
    return {(x, y)
            for y in range(H)
            for x in range(W)
            if img.pixel(x, y) == argb}


def main():
    ly = db.Layout()
    ly.dbu = 1.0
    top = ly.create_cell("TOP")
    white = (99, 0)
    gray = (99, 1)
    design = (10, 0)
    colors = {white: "#ffffff", gray: "#808080", design: "#ff0000"}

    # The gray box shares its left and bottom edges with the white box:
    # exactly the collision the stacking rule is about.
    top.shapes(ly.layer(db.LayerInfo(*white))).insert(db.Box(10, 10, 90, 90))
    top.shapes(ly.layer(db.LayerInfo(*gray))).insert(db.Box(10, 10, 50, 50))
    top.shapes(ly.layer(db.LayerInfo(*design))).insert(db.Box(30, 60, 70, 80))

    renderer = Renderer(ly, top, colors, hollow=(white, gray),
                        speckle=False, above=(white,))
    try:
        # Paint-plane order: gray, design planes, white on top -
        # later entries paint over earlier ones.
        order = [(lp.source_layer, lp.source_datatype)
                 for lp in renderer.lv.each_layer()]
        if (order[0] != gray or order[-1] != white
                or design not in order[1:-1]):
            raise SystemExit("paint order wrong: %r" % order)

        white_only = hits(pixels(renderer, [white]), WHITE)
        gray_only = hits(pixels(renderer, [gray]), GRAY)
        if not (white_only & gray_only):
            raise SystemExit("fixture lost the white/gray edge collision")

        # 1px, no fill: a mid column/row crosses exactly the two edges.
        col = sum(1 for (x, y) in white_only if x == 50)
        row = sum(1 for (x, y) in white_only if y == 50)
        if col != 2 or row != 2:
            raise SystemExit("white frame not 1px hollow: col=%d row=%d"
                             % (col, row))

        both = pixels(renderer, [white, gray])
        if any(both.pixel(x, y) != WHITE for (x, y) in white_only):
            raise SystemExit("gray painted over a white frame line")
        if any(both.pixel(x, y) != GRAY for (x, y) in gray_only - white_only):
            raise SystemExit("gray-only edge lost its gray line")

        # Split stacking against design fill: an outline strictly
        # inside the solid design box is fully covered when gray but
        # fully visible when white. Each inner outline's pixels are
        # the delta against its pre-insert plane, which keeps the
        # check independent of raster/flip details.
        top.shapes(ly.layer(db.LayerInfo(*white))).insert(
            db.Box(33, 62, 48, 78))
        top.shapes(ly.layer(db.LayerInfo(*gray))).insert(
            db.Box(52, 62, 67, 78))
        renderer.refresh()
        inner_white = hits(pixels(renderer, [white]), WHITE) - white_only
        inner_gray = hits(pixels(renderer, [gray]), GRAY) - gray_only
        if not inner_white or not inner_gray:
            raise SystemExit("fixture lost the vs-design cases")
        img = pixels(renderer, [white, gray, design])
        if any(img.pixel(x, y) != WHITE for (x, y) in inner_white):
            raise SystemExit("white frame line buried by design fill")
        if any(img.pixel(x, y) == GRAY for (x, y) in inner_gray):
            raise SystemExit("gray frame line painted over design fill")
        print("render-frames: gray under design under white, 1px "
              "hollow lines, white-over-gray OK")
    finally:
        renderer.lv._destroy()


if __name__ == "__main__":
    main()
