#!/usr/bin/env python3
"""Generate a mask-like OASIS that stands in for the field chip.

The real source (JOBDECK.ko.md §10, 2026-09-10/11) is a 35838.4 x
34617.6 um mask layer set whose problem region is made of thousands of
`ICV_nnnn` cells, each 167.7 x 535 um of hairlines: pages with ~29k
members of 11.38 x 0.0806 um lines (page 928), a variant with lines up
to 118.6 um long, 0.1244 um wide and 4.3 um bars (page 930), the same
geometry again on datatype 300, cells stacked in pairs and clustered
so that 7.4 % of the 26 x 33 mm region is occupied at 4 um cells,
11 % at 32 um and 15.8 % at 130 um. One cell at the bottom-right
corner holds a thicker record and therefore survives the plain
layout's page hairline cull while its connected neighbours vanish
from ~229 um views - the field symptom.

This script reproduces those numbers with synthetic geometry: the
same overall size (a 0.5 um boundary ring on 1/0 fixes the extent),
data only inside --region (default: the 26 x 33 mm bottom-right
region), --cells distinct ICV cells (every one a different page) each
of ~29k members at --pitch 0.255 um, placed once in 2 x 3-pair
clusters on a jittered lattice, the datatype-300 twin, a few marks on
4/0, and the ICV_BR cell at the corner. Cost scales with --cells x
members: the default 700 cells x 29k lines x 2 datatypes is ~41M
boxes in KLayout (about 1 GB, one to three minutes); use --cells 60
--pitch 1.0 for a quick file.

usage: python tools/gen_maskchip.py OUT.oas [--cells N] [--pitch UM]
           [--region X0,Y0,X1,Y1] [--seed S] [--jb] [--dbu UM]
"""
import argparse
import math
import os
import random
import sys
import time

import klayout.db as db

CHIP_W_UM = 35838.4
CHIP_H_UM = 34617.6
CELL_W_UM = 167.7
CELL_H_UM = 535.0
BR_W_UM = 167.7
BR_H_UM = 437.0
LINE_LEN_UM = 11.38          # page 928: max_w 11.3839
LINE_W_UM = 0.0806           # page 928/929/931: max_min 0.0806
LINE_W_B_UM = 0.1244         # page 930: max_min 0.1244
BAR_H_UM = 4.299             # page 930: max_h 4.2990
LONG_LEN_UM = 118.6442       # page 930: max_w 118.6442
LINE_GAP_UM = 0.6
BR_BAR_W_UM = 2.0            # thick enough to survive the page hairline cull
CLUSTER_COLS = 2
CLUSTER_PAIRS = 3

DECK = """SLICE 1,17
RETICLE
* {name}.jb
OPTION PA, AA=0.0200, BA=0.002000, SA=80
MTITLE 1,ICV
MTITLE 2,ICV300
*PLACE-INFO
*
CHIP ID001, * MAIN 1.0000
*
$ (1, ICV, AD={ad:.6f}, SF=1, TC={oas}, LY={{3}}, DT={{0}}, BX=0.0, BY=0.0, UX={ux:.1f}, UY={uy:.1f} )
$ (2, ICV300, AD={ad:.6f}, SF=1, TC={oas}, LY={{3}}, DT={{300}}, BX=0.0, BY=0.0, UX={ux:.1f}, UY={uy:.1f} )
ROWS 0.0/0.0
*END-PLACE
END
"""


def parse_region(text, chip):
    x0, y0, x1, y1 = (float(v) for v in text.split(","))
    cx0, cy0, cx1, cy1 = chip
    if not (cx0 <= x0 < x1 <= cx1 and cy0 <= y0 < y1 <= cy1):
        raise SystemExit("--region must lie inside the chip %s" % (chip,))
    return x0, y0, x1, y1


def make_icv(ly, name, layers, rng, dbu, pitch_um, variant, brand=False):
    """One ICV cell: rows of hairlines at `pitch_um`, a per-cell phase
    so every cell is a distinct page. Variant B carries the long lines,
    the wider width and the 4.3 um bars of page 930; the corner cell
    (brand) adds 2 um bars so its page is not all-thin."""
    cell = ly.create_cell(name)
    l3, _l3b = layers
    shapes = cell.shapes(l3)
    to = lambda um: int(round(um / dbu))
    w = to(CELL_W_UM)
    h = to(BR_H_UM if brand else CELL_H_UM)
    line_w = to(LINE_W_B_UM if variant else LINE_W_UM)
    pitch = to(pitch_um)
    rows = h // pitch
    phase_x = to(rng.uniform(0.0, LINE_GAP_UM))
    phase_y = to(rng.uniform(0.0, pitch_um * 0.5))
    seg = to(LINE_LEN_UM)
    gap = to(LINE_GAP_UM)
    step = seg + gap
    long_len = to(LONG_LEN_UM)
    bar_h = to(BAR_H_UM)
    br_bar_w = to(BR_BAR_W_UM)
    members = 0
    for r in range(rows):
        y = phase_y + r * pitch
        if y + line_w > h:
            break
        if variant and r % 8 == 0:
            # page 930: one long line and the rest short ones
            x = phase_x
            shapes.insert(db.Box(x, y, x + long_len, y + line_w))
            members += 1
            x += long_len + gap
            while x + seg <= w:
                shapes.insert(db.Box(x, y, x + seg, y + line_w))
                members += 1
                x += step
        else:
            x = phase_x
            while x + seg <= w:
                shapes.insert(db.Box(x, y, x + seg, y + line_w))
                members += 1
                x += step
        if variant and r % 40 == 20 and y + bar_h <= h:
            # a 0.1244 x 4.3 um bar (page 930's max_h)
            x = to(rng.uniform(0.0, CELL_W_UM - 1.0))
            shapes.insert(db.Box(x, y, x + line_w, y + bar_h))
            members += 1
        if brand and r % 100 == 50 and y + bar_h <= h:
            # the corner cell's thick record: min side 2 um
            x = to(rng.uniform(0.0, CELL_W_UM - 3.0))
            shapes.insert(db.Box(x, y, x + br_bar_w, y + bar_h))
            members += 1
    return cell, members


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("out")
    ap.add_argument("--cells", type=int, default=700,
                    help="distinct ICV cells (default 700; ~29k lines "
                         "each at the default pitch)")
    ap.add_argument("--pitch", type=float, default=0.255,
                    help="row pitch of the hairlines in um (default "
                         "0.255 -> ~29k members per cell)")
    ap.add_argument("--region", default=None,
                    help="X0,Y0,X1,Y1 in um (chip centred at 0,0); default "
                         "the 26 x 33 mm bottom-right region")
    ap.add_argument("--clusters", type=int, default=None,
                    help="cluster count (default: cells / 12, one instance "
                         "per cell)")
    ap.add_argument("--seed", type=int, default=20260911)
    ap.add_argument("--dbu", type=float, default=0.0001,
                    help="database unit in um (default 0.0001)")
    ap.add_argument("--jb", action="store_true",
                    help="also write OUT.jb: a two-level jobdeck (3/0, "
                         "3/300) placing the file at mag 1")
    args = ap.parse_args()
    if args.cells < 1 or args.pitch <= 0:
        raise SystemExit("--cells and --pitch must be positive")
    dbu = args.dbu
    to = lambda um: int(round(um / dbu))
    chip = (-CHIP_W_UM / 2, -CHIP_H_UM / 2, CHIP_W_UM / 2, CHIP_H_UM / 2)
    if args.region:
        region = parse_region(args.region, chip)
    else:
        region = (chip[2] - 26000.0, chip[1], chip[2], chip[1] + 33000.0)
    rng = random.Random(args.seed)
    t0 = time.time()

    ly = db.Layout(True)
    ly.dbu = dbu
    top = ly.create_cell("CHIP")
    lb = ly.layer(db.LayerInfo(1, 0, "BND"))
    l3 = ly.layer(db.LayerInfo(3, 0, "ICV"))
    l3b = ly.layer(db.LayerInfo(3, 300, "ICV300"))
    l4 = ly.layer(db.LayerInfo(4, 0, "MARK"))

    # the extent: a 0.5 um boundary ring (a hairline itself at wide views)
    ring = db.Polygon(db.Box(to(chip[0]), to(chip[1]), to(chip[2]), to(chip[3])))
    ring.insert_hole(db.Box(to(chip[0] + 0.5), to(chip[1] + 0.5),
                            to(chip[2] - 0.5), to(chip[3] - 0.5)))
    top.shapes(lb).insert(ring)

    # distinct ICV cells (every cell its own page on 3/0 and 3/300)
    cells = []
    members_total = 0
    for k in range(args.cells):
        cell, members = make_icv(ly, "ICV_%04d" % (k + 1), (l3, l3b), rng,
                                 dbu, args.pitch, variant=(k % 2 == 1))
        cell.shapes(l3b).insert(cell.shapes(l3))
        cells.append(cell)
        members_total += members
        if (k + 1) % 100 == 0 or k + 1 == args.cells:
            print("[maskchip] cells %d/%d (%.0fs, %d members so far)" % (
                k + 1, args.cells, time.time() - t0, members_total),
                  flush=True)
    br, br_members = make_icv(ly, "ICV_BR", (l3, l3b), rng, dbu, args.pitch,
                              variant=False, brand=True)
    br.shapes(l3b).insert(br.shapes(l3))

    # clusters of 2 columns x 3 stacked pairs on a jittered lattice
    per_cluster = CLUSTER_COLS * CLUSTER_PAIRS * 2
    n_clusters = args.clusters or max(1, math.ceil(args.cells / per_cluster))
    cw = CLUSTER_COLS * CELL_W_UM
    ch = CLUSTER_PAIRS * 2 * CELL_H_UM
    rx0, ry0, rx1, ry1 = region
    nx = max(1, int(math.sqrt(n_clusters * (rx1 - rx0) / (ry1 - ry0))))
    ny = max(1, math.ceil(n_clusters / nx))
    px = (rx1 - rx0) / nx
    py = (ry1 - ry0) / ny
    if px < cw or py < ch:
        raise SystemExit("region too small for %d clusters of %.0f x %.0f um"
                         % (n_clusters, cw, ch))
    sites = [(i, j) for j in range(ny) for i in range(nx)]
    rng.shuffle(sites)
    instances = 0
    next_cell = 0
    for (i, j) in sites[:n_clusters]:
        ox = rx0 + i * px + rng.uniform(0.0, max(0.0, px - cw))
        oy = ry0 + j * py + rng.uniform(0.0, max(0.0, py - ch))
        for col in range(CLUSTER_COLS):
            for pair in range(CLUSTER_PAIRS):
                for half in range(2):
                    cell = cells[next_cell % len(cells)]
                    next_cell += 1
                    x = ox + col * CELL_W_UM
                    y = oy + (pair * 2 + half) * CELL_H_UM
                    top.insert(db.CellInstArray(cell.cell_index(),
                                                db.Trans(db.Vector(to(x), to(y)))))
                    instances += 1
    # the corner: ICV_BR (survives the hairline cull) with two connected
    # ordinary cells beside and above it (they vanish under cull)
    bx = rx1 - BR_W_UM - 10.0
    by = ry0 + 10.0
    top.insert(db.CellInstArray(br.cell_index(), db.Trans(db.Vector(to(bx), to(by)))))
    top.insert(db.CellInstArray(cells[0].cell_index(),
                                db.Trans(db.Vector(to(bx - CELL_W_UM), to(by)))))
    top.insert(db.CellInstArray(cells[1 % len(cells)].cell_index(),
                                db.Trans(db.Vector(to(bx), to(by + BR_H_UM)))))
    instances += 3
    # a few 50 um marks on 4/0 at the region corners
    for (mx, my) in ((rx0 + 200, ry0 + 200), (rx1 - 250, ry1 - 250),
                     (rx0 + 200, ry1 - 250)):
        top.shapes(l4).insert(db.Box(to(mx), to(my), to(mx + 50), to(my + 50)))

    opt = db.SaveLayoutOptions()
    opt.format = "OASIS"
    opt.oasis_write_cblocks = True
    opt.oasis_compression_level = 2
    t1 = time.time()
    ly.write(args.out, opt)
    t2 = time.time()
    size = os.path.getsize(args.out)
    occupied = n_clusters * cw * ch
    area = (rx1 - rx0) * (ry1 - ry0)
    print("[maskchip] %s: %.1f MB in %.0fs build + %.0fs write" % (
        args.out, size / 1e6, t1 - t0, t2 - t1))
    print("[maskchip] chip %.1f x %.1f um, region %.0f,%.0f..%.0f,%.0f um "
          "(%.0f x %.0f mm)" % (CHIP_W_UM, CHIP_H_UM, rx0, ry0, rx1, ry1,
                                 (rx1 - rx0) / 1000, (ry1 - ry0) / 1000))
    print("[maskchip] %d distinct cells x ~%d members (+ICV_BR %d), %d "
          "instances in %d clusters; cluster fill of the region %.1f%% "
          "(the real region: 7.4%% at 4 um cells)" % (
              args.cells, members_total // max(1, args.cells), br_members,
              instances, n_clusters, 100.0 * occupied / area))
    if args.jb:
        name = os.path.splitext(os.path.basename(args.out))[0]
        jb = os.path.join(os.path.dirname(os.path.abspath(args.out)),
                          name + ".jb")
        with open(jb, "w", encoding="utf-8") as f:
            f.write(DECK.format(name=name, ad=dbu,
                                oas=os.path.basename(args.out),
                                ux=CHIP_W_UM, uy=CHIP_H_UM))
        print("[maskchip] deck %s (levels 1 = 3/0, 2 = 3/300, mag 1)" % jb)
    return 0


if __name__ == "__main__":
    sys.exit(main())
