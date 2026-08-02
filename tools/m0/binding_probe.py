#!/usr/bin/env python
"""Dump per-cell-index state around one gen cycle (debug aid)."""

import sys
import klayout.db as db

d = sys.argv[1]


def dump(ly, tag):
    print(f"--- {tag}: cells()={ly.cells()}")
    for c in ly.each_cell():
        nsh = sum(c.shapes(li).size() for li in ly.layer_indexes())
        ninst = sum(1 for _ in c.each_inst())
        nparents = sum(1 for _ in c.each_parent_cell())
        try:
            ghost = c.is_ghost_cell()
        except AttributeError:
            ghost = "?"
        print(f"  idx={c.cell_index():3d} name={c.name:12s} "
              f"ghost={ghost} shapes={nsh} insts={ninst} "
              f"parents={nparents}")


ly = db.Layout(False)
ws = ly.create_cell("FLOE_WS")
ly.read(f"{d}/gen1.oas")
dump(ly, "after gen1 read")

top = ly.cell("W1_F_0")
ws.insert(db.CellInstArray(top.cell_index(), db.Trans(0, 0)))

ly.read(f"{d}/gen2.oas")
dump(ly, "after gen2 read")

print("ly.cell('P0_1_0') ->", ly.cell("P0_1_0").cell_index())

ws.clear_insts()
top2 = ly.cell("W2_F_0")
ws.insert(db.CellInstArray(top2.cell_index(), db.Trans(0, 0)))
idxs = [ly.cell(w).cell_index() for w in ("W1_F_0", "W1_F_1")]
print("deleting", idxs)
ly.delete_cells(idxs)
dump(ly, "after delete gen1 WCs")

ly.read(f"{d}/gen3.oas")
dump(ly, "after gen3 read")
