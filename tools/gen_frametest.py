"""Generate frametest.oas: a tiny frame-stacking test asset.

Six layers, each with a few LARGE plates (horizontal bands spanning
the die, one L-shaped polygon among them), and block cells placed as
tall columns that CROSS every band - so each depth's frame boxes and
block names run over every layer's speckled fill. Depth is 3
(TOP -> BLK -> SUB -> LEAF), plus dust and a thin strip for the gray
tone / hairline interplay, and a design text on every band.

Checks it serves: white frame/name solid over any fill, gray buried
under fills, tone split (big=white, small/thin=gray), per-layer
toggling with few layers.
"""

import sys

import klayout.db as db

U = 1000  # dbu per um (1nm dbu)


def main():
    out = sys.argv[1] if len(sys.argv) > 1 else "data/frametest.oas"
    ly = db.Layout()
    ly.dbu = 0.001

    L = {n: ly.layer(db.LayerInfo(n, 0)) for n in
         (10, 20, 30, 38, 39, 40)}

    # ---- LEAF (depth 3): 5um of small boxes
    leaf = ly.create_cell("LEAF")
    for i in range(40):
        x = (i * 397) % 4500
        y = (i * 811) % 4500
        leaf.shapes(L[10]).insert(db.Box(x, y, x + 400, y + 400))

    # ---- SUB (depth 2): 20x30um, own plate + 2 LEAF placements
    sub = ly.create_cell("SUB")
    sub.shapes(L[20]).insert(db.Box(0, 0, 20 * U, 8 * U))
    sub.shapes(L[30]).insert(db.Box(0, 22 * U, 20 * U, 30 * U))
    for k, (x, y) in enumerate(((2 * U, 10 * U), (12 * U, 14 * U))):
        sub.insert(db.CellInstArray(
            leaf.cell_index(), db.Trans(k & 3, False, x, y)))
    sub.shapes(L[20]).insert(db.Text(
        "SUB_NET_A", db.Trans(db.Vector(10 * U, 4 * U))))

    # SMALLSUB (depth 2): 4um -> gray-tone frame at wide zoom
    ssub = ly.create_cell("SMALLSUB")
    ssub.shapes(L[10]).insert(db.Box(0, 0, 4 * U, 4 * U))

    # DUST (depth 1 leafs): 200nm - sub-cut material
    dust = ly.create_cell("DUST")
    dust.shapes(L[10]).insert(db.Box(0, 0, 200, 200))

    # STRIP (depth 1): 40x2um thin - hairline/gray interplay
    strip = ly.create_cell("STRIP")
    strip.shapes(L[30]).insert(db.Box(0, 0, 40 * U, 2 * U))

    # ---- BLK (depth 1): 50x120um columns, cross every TOP band
    blks = []
    for bi in range(3):
        b = ly.create_cell("BLK_%c" % (65 + bi))
        # own plates on the "report" layers 38/39
        b.shapes(L[38]).insert(db.Box(2 * U, 5 * U, 48 * U, 40 * U))
        b.shapes(L[39]).insert(
            db.Box(2 * U, 45 * U, 48 * U, 75 * U))
        b.shapes(L[40]).insert(
            db.Box(2 * U, 80 * U, 48 * U, 115 * U))
        for k, (x, y) in enumerate(
                ((4 * U, 8 * U), (26 * U, 42 * U), (8 * U, 84 * U))):
            b.insert(db.CellInstArray(
                sub.cell_index(), db.Trans(k & 3, False, x, y)))
        b.insert(db.CellInstArray(
            ssub.cell_index(), db.Trans(0, False, 40 * U, 60 * U)))
        for k in range(4):
            b.insert(db.CellInstArray(
                dust.cell_index(),
                db.Trans(0, False, (6 + 10 * k) * U, 118 * U)))
        b.insert(db.CellInstArray(
            strip.cell_index(), db.Trans(0, False, 5 * U, 2 * U)))
        b.shapes(L[39]).insert(db.Text(
            "BLK_PIN_%d" % bi, db.Trans(db.Vector(25 * U, 77 * U))))
        blks.append(b)

    # ---- TOP: 200x200um, six full-width plates (one per layer,
    # horizontal bands), an L-polygon, three BLK columns across
    top = ly.create_cell("TOP")
    bands = [(10, 5), (20, 35), (30, 65), (38, 95), (39, 125),
             (40, 155)]
    for (ln, y0) in bands:
        top.shapes(L[ln]).insert(
            db.Box(5 * U, y0 * U, 195 * U, (y0 + 25) * U))
        top.shapes(L[ln]).insert(db.Text(
            "PLATE_%d" % ln,
            db.Trans(db.Vector(100 * U, (y0 + 12) * U))))
    # one big L-shaped polygon (poly path, not a rect)
    pts = [db.Point(150 * U, 10 * U), db.Point(190 * U, 10 * U),
           db.Point(190 * U, 180 * U), db.Point(170 * U, 180 * U),
           db.Point(170 * U, 30 * U), db.Point(150 * U, 30 * U)]
    top.shapes(L[20]).insert(db.Polygon(pts))
    # BLK columns cross every band vertically
    for bi, x in enumerate((15, 75, 135)):
        top.insert(db.CellInstArray(
            blks[bi].cell_index(),
            db.Trans(0, False, x * U, 30 * U)))

    opts = db.SaveLayoutOptions()
    opts.format = "OASIS"
    ly.write(out, opts)
    print("wrote %s: %d cells, depth 3, 6 layers" % (out, ly.cells()))


if __name__ == "__main__":
    main()
