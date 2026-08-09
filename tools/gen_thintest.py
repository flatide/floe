"""Generate thintest.oas: thin-cell arrays for the rev 45 frame
lattice (Calibre hairline alignment), sized so the FIT VIEW already
sits inside the ladder.

Die ~4.3 x 3.1 mm. On a ~1000px viewport fit is ~0.23 px/um, so at
detail medium (3px) the cut is ~13um: the 30um-long bars are thin
(min 2um < cut <= 30um) and show DEMOTED lattice representatives
(7um = ~1.6px < 14px) right at fit; zooming IN passes the 2-per-bin
corner regime and ends with every member; zooming OUT (the viewer
allows 16x past fit, FIT_ZOOM_OUT) crosses cut > 30um where they
vanish. SHORTBAR (10x1um) is the vanish CONTRAST: already gone at
fit-medium while THINROW next to it still shows representatives.

Populated cases (all at the depth-0 boundary):

  1. sparse row   800x THINROW(30x2um) @5um X, y=0    - bins hold
     <=2 members: bound offsets keep the WHOLE row
  2. dense row    2000x VBAR(2x30um)   @2um X, y=40um - stride 4,
     corner offsets {0,3} keep 1000/2000 (demoted: 500)
  3. sparse 2D    400x12 THINROW @(10,9)um, y=90um    - both
     pitches >= 7um: kept intact
  4. vanish row   2000x SHORTBAR(10x1um) @2um X, y=220um - both
     sides under the fit-medium cut: gone at fit, thin when zoomed
  5. cluster      8x THINROW inside ONE 7um bin, y=250um -> 1
     representative + 9 spread singles (own bins, every 450um)
  6. dense column 1000x THINROW @3um Y, x=4050um      - stride 3,
     offsets {0,2} keep 667/1000 (demoted: 334)
  7. FAT 40x40um @(4200,3100)um - normal frame until cut > 40um
  8. a context plate on L20 (design geometry under the frames)

Reference runs (after `floe-index vfs data/thintest.oas`),
measured on 0.11.25 + rev 45:

  corners (cut 4um, 7um = 17.5px >= demote 14px -> bound offsets):
    floe-index plan data/thintest.oas.floe --view -10,-40,4300,3200 \
        --px-per-um 2.5 --cut-px 10 --depth 0
    -> frame_rects 11, thin_frames 6
       (rows/column/SHORTBAR 2 sub-grids each, 2D 1, all 17
        singles pack into ONE pts record -> 1 entry, FAT normal)
  fit-like demoted (cut 15um, 7um = 1.4px -> one offset per bin,
  SHORTBAR row vanished - the fit-view contrast):
    ... --px-per-um 0.2 --cut-px 3 --depth 0
    -> frame_rects 6, thin_frames 5
  nothing thin (cut 1um <= every min side):
    ... --px-per-um 3 --cut-px 3 --depth 0
    -> frame_rects 7, thin_frames 0 (all reps intact)
  rev 41 fallback (cut 4.8um so hair 2.4um > min sides):
    ... --px-per-um 2.5 --cut-px 12 --depth 0 --thin-um 0
    -> frame_rects 1 (FAT only; lattice ON at the same cut: 11)
  beyond fit, the vanish stage (cut 34um > every bar's max side;
  reachable in the viewer since FIT_ZOOM_OUT):
    ... --px-per-um 0.088 --cut-px 3 --depth 0
    -> frame_rects 1 (FAT alone survives until cut > 40um)

thin_frames counts the records that entered the lattice path;
frame_rects counts emitted entries (sub-grids/pts subsets/normals).
The viewer equivalents: detail low/medium/high + FLOE_THIN_UM.
"""

import sys

import klayout.db as db

U = 1000  # dbu per um (1nm dbu)


def main():
    out = sys.argv[1] if len(sys.argv) > 1 else "data/thintest.oas"
    ly = db.Layout()
    ly.dbu = 0.001

    l10 = ly.layer(db.LayerInfo(10, 0))
    l20 = ly.layer(db.LayerInfo(20, 0))

    thin = ly.create_cell("THINROW")  # horizontal 30x2um bar
    thin.shapes(l10).insert(db.Box(0, 0, 30 * U, 2 * U))

    vbar = ly.create_cell("VBAR")  # vertical 2x30um bar
    vbar.shapes(l10).insert(db.Box(0, 0, 2 * U, 30 * U))

    short = ly.create_cell("SHORTBAR")  # 10x1um: vanish contrast
    short.shapes(l10).insert(db.Box(0, 0, 10 * U, 1 * U))

    fat = ly.create_cell("FAT")
    fat.shapes(l10).insert(db.Box(0, 0, 40 * U, 40 * U))

    top = ly.create_cell("TOP")
    # 1. sparse row: 800 @5um X at y=0
    top.insert(db.CellInstArray(
        thin.cell_index(), db.Trans(0, False, 0, 0),
        db.Vector(5 * U, 0), db.Vector(0, 0), 800, 1))
    # 2. dense row: 2000 VBAR @2um X at y=40um (no overlap)
    top.insert(db.CellInstArray(
        vbar.cell_index(), db.Trans(0, False, 0, 40 * U),
        db.Vector(2 * U, 0), db.Vector(0, 0), 2000, 1))
    # 3. sparse 2D: 400x12 @(10,9)um at y=90um
    top.insert(db.CellInstArray(
        thin.cell_index(), db.Trans(0, False, 0, 90 * U),
        db.Vector(10 * U, 0), db.Vector(0, 9 * U), 400, 12))
    # 4. vanish row: 2000 SHORTBAR @2um X at y=220um
    top.insert(db.CellInstArray(
        short.cell_index(), db.Trans(0, False, 0, 220 * U),
        db.Vector(2 * U, 0), db.Vector(0, 0), 2000, 1))
    # 5. singles: 8 inside one 7um bin + 9 spread, y=250um
    for i in range(8):
        top.insert(db.CellInstArray(
            thin.cell_index(),
            db.Trans(0, False, i * 700, 250 * U)))
    for i in range(9):
        top.insert(db.CellInstArray(
            thin.cell_index(),
            db.Trans(0, False, (400 + 450 * i) * U, 250 * U)))
    # 6. dense column: 1000 @3um Y at x=4050um (no overlap)
    top.insert(db.CellInstArray(
        thin.cell_index(), db.Trans(0, False, 4050 * U, 0),
        db.Vector(0, 0), db.Vector(0, 3 * U), 1, 1000))
    # 7. FAT: stretches the die, normal frame until cut > 40um
    top.insert(db.CellInstArray(
        fat.cell_index(), db.Trans(0, False, 4200 * U, 3100 * U)))
    # 8. context plate under everything
    top.shapes(l20).insert(
        db.Box(0, -30 * U, 4300 * U, -10 * U))

    opts = db.SaveLayoutOptions()
    opts.format = "OASIS"
    ly.write(out, opts)
    print("wrote %s: %d cells, depth 1, 2 layers" % (out, ly.cells()))


if __name__ == "__main__":
    main()
