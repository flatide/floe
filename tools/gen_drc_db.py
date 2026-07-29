#!/usr/bin/env python3
"""Generate a Calibre-ASCII DRC results database (.db) for a layout.

Places synthetic violations (polygons + edges) at random positions
inside the layout bbox so the viewer's DRC browser can be exercised
without real Calibre output. Ground truth (exact um coordinates per
error) is written to <out>.manifest.json for automated verification.

    python tools/gen_drc_db.py data/api_clip_test.oas /tmp/t.db \
        --checks 4 --per 6 --seed 7
"""

import argparse
import json
import os
import random
import sys

RULES = [
    ("M1.S.1", "Metal1 spacing < 0.14"),
    ("M1.W.1", "Metal1 width < 0.10"),
    ("M2.S.1", "Metal2 spacing < 0.16"),
    ("VIA1.EN.1", "Via1 enclosure by Metal1 < 0.05\n"
                  "(two-sided rule, see section 4.2)"),
    ("M2.A.1", "Metal2 area < 0.05 um^2"),
    ("POLY.DEN.1", "Poly density < 0.15 in 50x50 window"),
]


def read_layout(path):
    import klayout.db as db
    ly = db.Layout()
    ly.read(path)
    top = ly.top_cell()
    b = top.bbox()
    return (top.name, ly.dbu,
            (b.left * ly.dbu, b.bottom * ly.dbu,
             b.right * ly.dbu, b.top * ly.dbu))


def gen_errors(rng, bbox_um, count):
    """Mixed polygon/edge violations inside the central 80% of bbox."""
    x0, y0, x1, y1 = bbox_um
    mx, my = (x1 - x0) * 0.1, (y1 - y0) * 0.1
    errs = []
    for i in range(1, count + 1):
        cx = rng.uniform(x0 + mx, x1 - mx)
        cy = rng.uniform(y0 + my, y1 - my)
        if rng.random() < 0.3:      # edge violation
            ln = rng.uniform(0.1, 2.0)
            if rng.random() < 0.5:
                pts = [(cx, cy), (cx + ln, cy)]
            else:
                pts = [(cx, cy), (cx, cy + ln)]
            errs.append({"kind": "e", "num": i, "pts": pts})
        else:                       # polygon violation (spacing box)
            w = rng.uniform(0.05, 1.5)
            h = rng.uniform(0.05, 1.5)
            pts = [(cx, cy), (cx + w, cy), (cx + w, cy + h), (cx, cy + h)]
            errs.append({"kind": "p", "num": i, "pts": pts})
    return errs


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("layout", help="OASIS/GDS file (cell name + bbox)")
    ap.add_argument("out", help="output .db path")
    ap.add_argument("--checks", type=int, default=4)
    ap.add_argument("--per", type=int, default=6,
                    help="violations per check (default 6)")
    ap.add_argument("--empty", type=int, default=1,
                    help="additional checks with 0 results (default 1)")
    ap.add_argument("--seed", type=int, default=7)
    ap.add_argument("--precision", type=float, default=None,
                    help="db units per um (default: 1/layout dbu)")
    args = ap.parse_args(argv)

    cell, dbu, bbox_um = read_layout(args.layout)
    prec = args.precision or round(1.0 / dbu)
    rng = random.Random(args.seed)
    stamp = "Mon Jul 27 12:00:00 2026"

    manifest = {"cell": cell, "precision": prec, "layout": args.layout,
                "bbox_um": bbox_um, "checks": []}
    out = ["%s %g" % (cell, prec)]
    rules = (RULES * ((args.checks + args.empty) // len(RULES) + 1))
    for ci in range(args.checks + args.empty):
        name, desc = rules[ci]
        if ci >= args.checks:
            name += ".EMPTY"
        errs = [] if ci >= args.checks \
            else gen_errors(rng, bbox_um, args.per)
        dlines = desc.split("\n")
        out.append(name)
        out.append("%d %d %d %s" % (len(errs), len(errs),
                                    len(dlines), stamp))
        out.extend(dlines)
        snap = []
        for e in errs:
            ints = [int(round(v * prec)) for xy in e["pts"] for v in xy]
            if e["kind"] == "e":
                # edge records: one line per edge, x1 y1 x2 y2
                out.append("e %d 1" % e["num"])
                out.append(" ".join(str(v) for v in ints))
            else:
                out.append("p %d %d" % (e["num"], len(e["pts"])))
                for j in range(0, len(ints), 2):
                    out.append("%d %d" % (ints[j], ints[j + 1]))
            snap.append({"kind": e["kind"], "num": e["num"],
                         "pts": [(ints[j] / prec, ints[j + 1] / prec)
                                 for j in range(0, len(ints), 2)]})
        manifest["checks"].append({"name": name, "desc": desc,
                                   "errors": snap})
    with open(args.out, "w") as f:
        f.write("\n".join(out) + "\n")
    mpath = args.out + ".manifest.json"
    with open(mpath, "w") as f:
        json.dump(manifest, f, indent=1)
    total = sum(len(c["errors"]) for c in manifest["checks"])
    print("wrote %s: cell %s, precision %g, %d checks, %d errors"
          % (args.out, cell, prec, len(manifest["checks"]), total))
    print("ground truth: %s" % mpath)


if __name__ == "__main__":
    sys.exit(main())
