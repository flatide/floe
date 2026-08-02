#!/usr/bin/env python
"""M0 pya-fallback expansion cost (VFS_HIER.md par.5 M0 / par.3.3).

Prices the hier.tsv fallback / flat-path baseline: inserting k single
CellInstArray placements through pya into a viewer-mode Layout,
i.e. what a Pts rep costs when the viewer has to expand it itself.

Usage: pts_fallback.py <k> [--no-draw]
Prints one JSON object.
"""

import json
import os
import subprocess
import sys
import time
import resource

import klayout.db as db
import klayout.lay as klay


def rss_mb():
    out = subprocess.run(["ps", "-o", "rss=", "-p", str(os.getpid())],
                         capture_output=True, text=True).stdout.strip()
    return int(out) / 1024.0 if out else -1.0


def main():
    k = int(sys.argv[1])
    draw = "--no-draw" not in sys.argv
    tmpdir = os.environ.get("M0_TMP", "/tmp")

    ly = db.Layout(False)
    ly.dbu = 0.001
    chip = ly.create_cell("CHIP")
    l1 = ly.layer(1, 0)
    l2 = ly.layer(2, 0)
    chip.shapes(l1).insert(db.Box(0, 0, 100, 100))
    chip.shapes(l2).insert(db.Box(45, 45, 55, 55))
    top = ly.create_cell("TOP")
    ci = chip.cell_index()

    # same LCG as m0_gen for a comparable spatial distribution
    state = 0x5EEDF10E
    def nxt():
        nonlocal state
        state = (state * 6364136223846793005 + 1442695040888963407) \
            % (1 << 64)
        return state >> 33

    rss0 = rss_mb()
    t0 = time.perf_counter()
    for _ in range(k):
        x = nxt() % 1048576
        y = nxt() % 1048576
        top.insert(db.CellInstArray(ci, db.Trans(db.Vector(x, y))))
    t_insert = time.perf_counter() - t0

    res = dict(k=k,
               insert_s=round(t_insert, 3),
               inserts_per_s=int(k / t_insert) if t_insert > 0 else -1,
               rss_delta_mb=round(rss_mb() - rss0, 1))

    if draw:
        lv = klay.LayoutView()
        lv.show_layout(ly, False)
        lv.cellview(0).cell = top
        lv.add_missing_layers()
        lv.max_hier()
        dbu = ly.dbu
        b = top.bbox()
        t0 = time.perf_counter()
        lv.zoom_box(db.DBox(b.left * dbu, b.bottom * dbu,
                            b.right * dbu, b.top * dbu))
        lv.save_image(os.path.join(tmpdir, f"fb_{k}_wide.png"), 800, 600)
        res["draw_wide_s"] = round(time.perf_counter() - t0, 3)

    v = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    res["peak_mb"] = round(v / (1024.0 * 1024.0)
                           if sys.platform == "darwin" else v / 1024.0, 1)
    print(json.dumps(res, indent=1))


if __name__ == "__main__":
    main()
