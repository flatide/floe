"""Generate valmini.oas: a SMALL adversarial validation asset.

The big assets (stress30 class) cost ~1h of oracle time per XOR
sweep; valmini keeps every characteristic that has caught bugs -
dense repetition arrays, tile-straddling arrays, mixed random
scatter, all four size bands - at ~1M expanded members, and adds
what the old assets never exercised:

  - ROTATED and MIRRORED placements (rot 90/180/270, flip)
  - three hierarchy levels (TOP -> MID -> LEAF*)
  - non-manhattan polygons (triangles, L-shapes)
  - texts (marker spray + labels) for the sidecar milestones

Die 400x400 um, dbu 1 nm -> the Python indexer makes a 4x4 grid of
100 um tiles; instances deliberately straddle the 100 um lines.

usage: python tools/gen_valmini.py <out.oas>
"""
import random
import sys

import klayout.db as db

U = 1000  # dbu per um


def main(out):
    rng = random.Random(20260731)
    ly = db.Layout(True)
    ly.dbu = 0.001

    L = {i: ly.layer(db.LayerInfo(i, 0)) for i in range(1, 8)}
    LM = ly.layer(db.LayerInfo(63, 63, "MARKER"))

    # LEAF1: dense fine grid (b3) + medium grid (b2) + b0 bar
    leaf1 = ly.create_cell("LEAF1")
    sh = leaf1.shapes(L[1])
    for i in range(100):
        for j in range(100):
            x, y = i * 100, j * 100
            sh.insert(db.Box(x, y, x + 50, y + 50))     # 0.05um -> b3
    sh = leaf1.shapes(L[2])
    for i in range(30):
        for j in range(30):
            x, y = i * 600, j * 600
            sh.insert(db.Box(x, y, x + 300, y + 300))   # 0.3um -> b2
    leaf1.shapes(L[3]).insert(db.Box(0, 0, 18_000, 3_000))  # 18um -> b0

    # LEAF2: sparse scatter + triangles (non-manhattan)
    leaf2 = ly.create_cell("LEAF2")
    for _ in range(50):
        x = rng.randrange(0, 15_000)
        y = rng.randrange(0, 15_000)
        s = rng.choice((80, 300, 900, 2_500))
        leaf2.shapes(L[4]).insert(db.Box(x, y, x + s, y + s))
    for _ in range(20):
        x = rng.randrange(0, 15_000)
        y = rng.randrange(0, 15_000)
        s = rng.randrange(200, 1_500)
        tri = db.Polygon([db.Point(x, y), db.Point(x + s, y),
                          db.Point(x, y + s)])
        leaf2.shapes(L[5]).insert(tri)

    # MID: LEAF1 3x3 array + rotated/mirrored LEAF2 + own L-shape (b1)
    mid = ly.create_cell("MID")
    mid.insert(db.CellInstArray(
        leaf1.cell_index(), db.Trans(db.Vector(0, 0)),
        db.Vector(20_000, 0), db.Vector(0, 20_000), 3, 3))
    for k, (rot, mirr) in enumerate(
            ((1, False), (2, False), (3, False), (0, True), (2, True))):
        mid.insert(db.CellInstArray(
            leaf2.cell_index(),
            db.Trans(rot, mirr, db.Vector(62_000 + k * 1_000,
                                          8_000 + k * 9_000))))
    lsh = db.Polygon([
        db.Point(64_000, 52_000), db.Point(65_500, 52_000),
        db.Point(65_500, 53_500), db.Point(64_600, 53_500),
        db.Point(64_600, 52_600), db.Point(64_000, 52_600)])
    mid.shapes(L[6]).insert(lsh)                        # 1.5um -> b1

    # TOP: MID instances straddling the 100um tile lines, rotated
    # copies, random scatter across bands, texts
    top = ly.create_cell("VALMINI_TOP")
    top.insert(db.CellInstArray(
        mid.cell_index(), db.Trans(db.Vector(30_000, 30_000)),
        db.Vector(110_000, 0), db.Vector(0, 110_000), 3, 3))
    top.insert(db.CellInstArray(
        mid.cell_index(), db.Trans(1, False, db.Vector(390_000, 5_000))))
    top.insert(db.CellInstArray(
        mid.cell_index(), db.Trans(2, True, db.Vector(90_000, 385_000))))
    sh = top.shapes(L[7])
    for _ in range(2_000):
        x = rng.randrange(0, 396_000)
        y = rng.randrange(0, 396_000)
        s = rng.choice((60, 200, 700, 1_800, 4_000, 12_000))
        sh.insert(db.Box(x, y, x + s, y + s))
    shm = top.shapes(LM)
    for i in range(500):
        x = rng.randrange(0, 399_000)
        y = rng.randrange(0, 399_000)
        shm.insert(db.Text("M%d" % i, db.Trans(db.Vector(x, y))))
    for i, (x, y) in enumerate(((50_000, 50_000), (200_000, 200_000),
                                (350_000, 350_000))):
        shm.insert(db.Text("BLOCK_%d" % i, db.Trans(db.Vector(x, y))))

    opt = db.SaveLayoutOptions()
    opt.format = "OASIS"
    opt.oasis_write_cblocks = True
    opt.oasis_compression_level = 1
    opt.write_context_info = False
    ly.write(out, opt)
    n = sum(c.shapes(li).size() for c in ly.each_cell()
            for li in ly.layer_indexes())
    print("wrote %s: %d cells, %d stored members (expanded ~1M)"
          % (out, sum(1 for _ in ly.each_cell()), n))


if __name__ == "__main__":
    main(sys.argv[1])
