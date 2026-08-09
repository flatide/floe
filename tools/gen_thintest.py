"""Generate thintest.oas: thin-cell arrays for the rev 45 frame
lattice (Calibre hairline alignment).

Every populated case uses a cell whose box is THIN at the reference
cut (min side < cut <= max side), so the depth-0 boundary frames
enter the 7um lattice path instead of vanishing:

  1. sparse row   60x THINROW(30x2um) @5um X   - pitch < 7um but
     bins hold <=2 members: the bound offsets keep the WHOLE row
  2. dense row    150x VBAR(2x30um)   @2um X   - stride 4, corner
     offsets {0,3} keep 75/150 (demoted: 38)
  3. sparse 2D    20x12 THINROW @(10,9)um      - both pitches >=
     7um: kept intact (already sparser than the lattice)
  4. cluster      8x THINROW inside ONE 7um bin -> 1 representative
     + 3 spread singles (own bins)             -> 4 members total
  5. dense column 70x THINROW @3um Y           - stride 3, offsets
     {0,2} keep 47/70 (demoted: 24)
  6. FAT 40x40um - never thin at these cuts, always a normal frame
  7. a context plate on L20 (design geometry under the frames)

Reference runs (after `floe-index vfs data/thintest.oas`),
measured on 0.11.25:

  corners (cut 4um, 7um = 17.5px >= demote 14px -> bound offsets):
    floe-index plan data/thintest.oas.floe --view -10,-40,650,400 \
        --px-per-um 2.5 --cut-px 10 --depth 0
    -> frame_rects 9, thin_frames 5
       (rows/column 2 sub-grids each, 2D + pts 1 each, FAT normal)
  demoted (cut 15um, 7um = 1.4px < 14px -> one offset per bin):
    ... --px-per-um 0.2 --cut-px 3 --depth 0
    -> frame_rects 6, thin_frames 5
  nothing thin (cut 1um < every min side):
    ... --px-per-um 3 --cut-px 3 --depth 0
    -> frame_rects 6, thin_frames 0 (all reps intact)
  rev 41 fallback (cut 4.8um so hair 2.4um > min side 2um):
    ... --px-per-um 2.5 --cut-px 12 --depth 0 --thin-um 0
    -> frame_rects 1 (FAT only; lattice ON at the same cut: 9)

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

    fat = ly.create_cell("FAT")
    fat.shapes(l10).insert(db.Box(0, 0, 40 * U, 40 * U))

    top = ly.create_cell("TOP")
    # 1. sparse row: 60 @5um X at y=0
    top.insert(db.CellInstArray(
        thin.cell_index(), db.Trans(0, False, 0, 0),
        db.Vector(5 * U, 0), db.Vector(0, 0), 60, 1))
    # 2. dense row: 150 VBAR @2um X at y=40um (no overlap)
    top.insert(db.CellInstArray(
        vbar.cell_index(), db.Trans(0, False, 0, 40 * U),
        db.Vector(2 * U, 0), db.Vector(0, 0), 150, 1))
    # 3. sparse 2D: 20x12 @(10,9)um at y=90um
    top.insert(db.CellInstArray(
        thin.cell_index(), db.Trans(0, False, 0, 90 * U),
        db.Vector(10 * U, 0), db.Vector(0, 9 * U), 20, 12))
    # 4. singles: 8 inside one 7um bin + 3 spread, y=210um
    for i in range(8):
        top.insert(db.CellInstArray(
            thin.cell_index(),
            db.Trans(0, False, i * 700, 210 * U)))
    for x in (400 * U, 450 * U, 500 * U):
        top.insert(db.CellInstArray(
            thin.cell_index(), db.Trans(0, False, x, 210 * U)))
    # 5. dense column: 70 @3um Y at x=560um (no overlap)
    top.insert(db.CellInstArray(
        thin.cell_index(), db.Trans(0, False, 560 * U, 0),
        db.Vector(0, 0), db.Vector(0, 3 * U), 1, 70))
    # 6. FAT: never thin at the reference cuts
    top.insert(db.CellInstArray(
        fat.cell_index(), db.Trans(0, False, 300 * U, 300 * U)))
    # 7. context plate under everything
    top.shapes(l20).insert(
        db.Box(0, -30 * U, 600 * U, -10 * U))

    opts = db.SaveLayoutOptions()
    opts.format = "OASIS"
    ly.write(out, opts)
    print("wrote %s: %d cells, depth 1, 2 layers" % (out, ly.cells()))


if __name__ == "__main__":
    main()
