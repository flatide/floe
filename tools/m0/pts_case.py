#!/usr/bin/env python
"""M0 Pts materialization case (VFS_HIER.md par.5 M0 / par.2.3).

One matrix row: N-member type-10 file. Measures what klayout does
with an irregular repetition of that size, then verifies the par.2.3
rebase (selection 0 / 1 / >=2 subset files) against the full file by
region XOR inside the selection window.

Usage: pts_case.py <dir> <N> [--no-draw]
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

CHIP_LAYERS = [(1, 0), (2, 0)]
CHIP_AREA = {(1, 0): 100 * 100, (2, 0): 10 * 10}


def rss_mb():
    out = subprocess.run(["ps", "-o", "rss=", "-p", str(os.getpid())],
                         capture_output=True, text=True).stdout.strip()
    return int(out) / 1024.0 if out else -1.0


def peak_mb():
    v = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    # darwin: bytes, linux: KB
    return v / (1024.0 * 1024.0) if sys.platform == "darwin" else v / 1024.0


def layer_index(ly, l, dt):
    for li in ly.layer_indexes():
        inf = ly.get_info(li)
        if inf.layer == l and inf.datatype == dt:
            return li
    return None


def window_region(ly, cell, l, dt, box):
    li = layer_index(ly, l, dt)
    if li is None:
        return db.Region()
    r = db.Region(db.RecursiveShapeIterator(ly, cell, li, box))
    return r & db.Region(box)


def main():
    d, n = sys.argv[1], int(sys.argv[2])
    draw = "--no-draw" not in sys.argv
    tmpdir = os.environ.get("M0_TMP", "/tmp")

    cases = []
    with open(os.path.join(d, "pts_manifest.tsv")) as f:
        for line in f:
            v = line.rstrip("\n").split("\t")
            if int(v[0]) == n:
                cases.append(dict(case=int(v[1]),
                                  win=tuple(map(int, v[2:6])),
                                  expected=int(v[6]),
                                  full=v[7], sub=v[8]))
    assert len(cases) == 3, cases

    res = dict(n=n,
               klayout=getattr(__import__("klayout"), "__version__", "?"))

    ly = db.Layout(False)
    rss0 = rss_mb()
    t0 = time.perf_counter()
    ly.read(os.path.join(d, cases[0]["full"]))
    res["read_s"] = round(time.perf_counter() - t0, 3)
    res["rss_after_read_mb"] = round(rss_mb(), 1)
    res["rss_delta_read_mb"] = round(rss_mb() - rss0, 1)

    top = ly.cell("TOP")
    t0 = time.perf_counter()
    records = 0
    members = 0
    props = set()
    for inst in top.each_inst():
        records += 1
        members += inst.size()
        if records <= 3:
            props.add((inst.size(), str(inst.trans)))
    res["inst_iter_s"] = round(time.perf_counter() - t0, 3)
    res["inst_records"] = records
    res["inst_members"] = members
    res["expanded"] = (records == n)
    res["sample_insts"] = sorted(props)

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
        lv.save_image(os.path.join(tmpdir, f"pts_{n}_wide.png"), 800, 600)
        res["draw_wide_s"] = round(time.perf_counter() - t0, 3)
        w = cases[1]["win"]
        t0 = time.perf_counter()
        lv.zoom_box(db.DBox(w[0] * dbu, w[1] * dbu,
                            w[2] * dbu, w[3] * dbu))
        lv.save_image(os.path.join(tmpdir, f"pts_{n}_narrow.png"), 800, 600)
        res["draw_narrow_s"] = round(time.perf_counter() - t0, 3)

    xor = []
    for c in cases:
        ly2 = db.Layout(False)
        ly2.read(os.path.join(d, c["sub"]))
        top2 = ly2.cell("TOP")
        box = db.Box(*c["win"])
        ok = True
        detail = {}
        for (l, dt) in CHIP_LAYERS:
            rf = window_region(ly, top, l, dt, box)
            rs = window_region(ly2, top2, l, dt, box)
            same = (rf ^ rs).is_empty()
            area_ok = (rf.area() == c["expected"] * CHIP_AREA[(l, dt)])
            ok = ok and same and area_ok
            detail[f"L{l}"] = dict(xor_empty=same,
                                   full_area=rf.area(),
                                   sub_area=rs.area(),
                                   expected_area=c["expected"] *
                                   CHIP_AREA[(l, dt)])
        xor.append(dict(case=c["case"], expected=c["expected"],
                        ok=ok, detail=detail))
    res["xor"] = xor
    res["xor_all_ok"] = all(x["ok"] for x in xor)
    res["peak_mb"] = round(peak_mb(), 1)
    print(json.dumps(res, indent=1))


if __name__ == "__main__":
    main()
