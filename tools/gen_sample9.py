"""Generate sample9.oas: a dense depth-9 hierarchy stress asset.

Purpose: one file that exercises every frame/label behavior at once -
depth 0..9 boundaries (plus side branches bottoming at 3/5/7 so each
depth has its own frontier), frame boxes of every size class (large
white, medium, long-thin gray, sub-cut dust), dense-pitch arrays
(rev 39 per-member frames), tens of layers, and text labels at every
tier. Sized by two budgets: unique records (file bytes) and expanded
shapes (memory when flattened).

Budgeting: the tier table fixes the placement plan; expanded instance
counts E_t follow from it, so per-cell box counts are derived as
flat_budget_t / E_t and record counts pick the number of distinct
cells. --scale shrinks both budgets for calibration runs.

Deterministic (seed 42): regenerating produces the identical file.
"""

import argparse
import random
import sys
import time

import klayout.db as db


# tier table, top to bottom:
# name, N distinct cells, extent um, per-parent child placements P,
# record budget (fraction of total), flat budget (fraction)
TIERS = [
    # name   N    ext_um     P  rec%   (records weighted toward the
    # low-E mid tiers so the expanded total lands near --flat; the
    # placement plan sets E, ~x3.3 overall)
    ("TOP",    1, 30_000.0,  4, 0.007),
    ("D1",     4, 14_000.0,  4, 0.015),
    ("D2",    15,  6_000.0,  2, 0.050),
    ("B3",    19,  2_500.0,  2, 0.190),
    ("U4",    29,  1_000.0,  2, 0.190),
    ("U5",    58,    450.0,  2, 0.120),
    ("U6",    96,    200.0,  2, 0.110),
    ("U7",   190,     90.0,  2, 0.100),
    ("U8",   240,     40.0,  2, 0.100),
    ("L9",   530,     10.0,  0, 0.118),
]
# layer bands per tier (design geometry), 40 layers total
BANDS = {
    "L9": (1, 12), "U8": (13, 18), "U7": (19, 24), "U6": (25, 28),
    "U5": (29, 32), "U4": (33, 35), "B3": (36, 37), "D2": (38, 38),
    "D1": (39, 39), "TOP": (40, 40),
}
TEXT_LAYERS = (5, 15, 25, 35)
DBU_PER_UM = 1000  # dbu = 1nm


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("out")
    ap.add_argument("--records", type=float, default=13.5e6,
                    help="unique shape record budget (file size)")
    ap.add_argument("--flat", type=float, default=50e6,
                    help="expanded shape budget (flat memory)")
    ap.add_argument("--scale", type=float, default=1.0,
                    help="shrink both budgets (calibration)")
    args = ap.parse_args()
    records_budget = args.records * args.scale
    flat_budget = args.flat * args.scale

    rnd = random.Random(42)
    ly = db.Layout()
    ly.dbu = 1.0 / DBU_PER_UM

    layers = {n: ly.layer(db.LayerInfo(n, 0)) for n in range(1, 41)}

    # Arrays: half of the U8 cells carry one 3x3 leaf array; three B3
    # showcase macros carry one dense 60x60 dust array (dust cells
    # are cheap, so the big arrays cost flat little but exercise
    # rev 39 per-member frames hard).
    stats = {"records": 0, "flat": 0, "texts": 0, "cells": 0}
    tier_cells = {}
    box_per_cell = {}

    def fill_cell(cell, ext_dbu, nboxes, band, thin=False):
        lo, hi = band
        for _ in range(nboxes):
            l = layers[rnd.randint(lo, hi)]
            if thin or rnd.random() < 0.25:
                # wire look: long and thin
                w = rnd.randint(40, 120)
                h = rnd.randint(ext_dbu // 4, max(ext_dbu // 2, 200))
                if rnd.random() < 0.5:
                    w, h = h, w
            else:
                w = rnd.randint(50, 500)
                h = rnd.randint(50, 500)
            x = rnd.randint(0, max(ext_dbu - w, 1))
            y = rnd.randint(0, max(ext_dbu - h, 1))
            cell.shapes(l).insert(db.Box(x, y, x + w, y + h))
        stats["records"] += nboxes

    def add_texts(cell, ext_dbu, count, prefix):
        for i in range(count):
            l = layers[rnd.choice(TEXT_LAYERS)]
            t = db.Text("%s_%d" % (prefix, i),
                        db.Trans(db.Vector(rnd.randint(0, ext_dbu),
                                           rnd.randint(0, ext_dbu))))
            cell.shapes(l).insert(t)
            stats["texts"] += 1

    # ---- pass 2: build bottom-up
    t0 = time.time()
    for (name, n, ext_um, p, rec_frac) in reversed(TIERS):
        ext = int(ext_um * DBU_PER_UM)
        n_records = rec_frac * records_budget
        # per-cell boxes come from the record budget; the flat budget
        # is steered by the placement plan (P and the array shares),
        # verified against the analyzer after generation
        b = max(30, int(n_records / n))
        n_eff = max(1 if name == "TOP" else 2, int(n_records / b))
        if name == "TOP":
            n_eff = 1
        cells = []
        for i in range(n_eff):
            long_tail = (name == "U4" and i < 2)
            cname = ("%s_VERY_LONG_BLOCK_NAME_FOR_ELLIPSIS_%03d"
                     % (name, i)) if long_tail else "%s_%03d" % (name, i)
            c = ly.create_cell(cname)
            # size variety inside the tier: 0.5x..2x, plus two
            # 20:1 thin strips per tier (gray-tone material)
            if i % max(n_eff // 2, 1) == 1 and name != "TOP":
                w = int(ext * 2)
                h = max(int(ext * 0.1), 400)
                fill_cell(c, h, b, BANDS[name], thin=True)
                c.shapes(layers[BANDS[name][0]]).insert(
                    db.Box(0, 0, w, h))
                stats["records"] += 1
            else:
                f = 0.5 + 1.5 * rnd.random()
                fill_cell(c, int(ext * f), b, BANDS[name])
            add_texts(c, ext, rnd.randint(1, 3), name)
            cells.append(c)
        tier_cells[name] = cells
        box_per_cell[name] = b
        stats["cells"] += n_eff
        print("tier %-3s cells %4d x %6d boxes  (%.1fs)"
              % (name, n_eff, b, time.time() - t0), flush=True)

    # dust cells: sub-cut material placed at several depths
    dust = []
    for i in range(6):
        c = ly.create_cell("DUST_%02d" % i)
        e = rnd.randint(60, 300)  # 60..300nm
        c.shapes(layers[rnd.randint(1, 12)]).insert(db.Box(0, 0, e, e))
        stats["records"] += 1
        dust.append(c)

    # ---- placements, top-down; round-robin coverage of every child
    def place(parent, child, x, y, rot=0):
        parent.insert(db.CellInstArray(
            child.cell_index(), db.Trans(rot, False, x, y)))

    order = [t[0] for t in TIERS]
    for ti, (name, n, ext_um, p, _rb) in enumerate(TIERS):
        if p == 0:
            continue
        ext = int(ext_um * DBU_PER_UM)
        child_name = order[ti + 1]
        children = tier_cells[child_name]
        cext = int(TIERS[ti + 1][2] * DBU_PER_UM)
        ci = 0
        for pc_i, pc in enumerate(tier_cells[name]):
            placements = max(p, -(-len(children) //
                                  len(tier_cells[name])))
            for k in range(placements):
                child = children[ci % len(children)]
                ci += 1
                x = rnd.randint(0, max(ext - cext, 1))
                y = rnd.randint(0, max(ext - cext, 1))
                rot = rnd.choice((0, 0, 0, 1))
                place(pc, child, x, y, rot)
            # half of the U8 cells: 3x3 array of one leaf kind
            if name == "U8" and pc_i % 2 == 0:
                child = children[ci % len(children)]
                ci += 1
                pitch = int(cext * 1.05)
                pc.insert(db.CellInstArray(
                    child.cell_index(), db.Trans(0, False, 0, 0),
                    db.Vector(pitch, 0), db.Vector(0, pitch), 3, 3))
            # dust at every tier below D2
            if name in ("B3", "U4", "U5", "U6", "U7", "U8"):
                for _ in range(2):
                    d = dust[rnd.randrange(len(dust))]
                    place(pc, d, rnd.randint(0, ext),
                          rnd.randint(0, ext))

    # three B3 showcase macros: dense 60x60 dust array (per-member
    # frames under rev 39; pitch crosses any px threshold with zoom)
    for i, pc in enumerate(tier_cells["B3"][:3]):
        d = dust[i % len(dust)]
        pitch = 420  # dbu; dust is 60..300nm -> tight pitch
        pc.insert(db.CellInstArray(
            d.cell_index(), db.Trans(0, False, 10_000, 10_000),
            db.Vector(pitch, 0), db.Vector(0, pitch), 60, 60))

    # declutter stress: a grid of texts on two D2 cells
    for pc in tier_cells["D2"][:2]:
        ext = int(6_000.0 * DBU_PER_UM)
        step = ext // 100
        for gx in range(100):
            for gy in range(100):
                l = layers[rnd.choice(TEXT_LAYERS)]
                pc.shapes(l).insert(db.Text(
                    "NET_%d_%d" % (gx, gy),
                    db.Trans(db.Vector(gx * step, gy * step))))
                stats["texts"] += 1

    # side branches that bottom out early: place three L9 leaves and
    # one U7 directly under D2/B3 so depths 3..5 own real frontiers
    for i, pc in enumerate(tier_cells["D2"][2:6]):
        place(pc, tier_cells["L9"][i], 50_000, 50_000)
    for i, pc in enumerate(tier_cells["B3"][3:7]):
        place(pc, tier_cells["U7"][i], 20_000, 20_000)

    top = tier_cells["TOP"][0]
    print("cells %d, records %d, texts %d  (%.1fs)"
          % (stats["cells"], stats["records"], stats["texts"],
             time.time() - t0), flush=True)

    opts = db.SaveLayoutOptions()
    opts.format = "OASIS"
    ly.write(args.out, opts)
    print("wrote %s (%.1fs)" % (args.out, time.time() - t0),
          flush=True)


if __name__ == "__main__":
    main()
