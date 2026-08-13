"""Depth-semantics validation for the hierarchical Rust tiler: in
every band file, content must sit at the same DESIGN hierarchy level
as in the klayout-built file - that is what the viewer's depth
feature renders. Compares per-(level, layer) member counts (each()
expansion, so klayout's size() corner case cannot skew it) and the
level count itself.

usage: python tools/validate_rust_depth.py <src.oas> <rust_outdir>
"""
import functools
import json
import os
import sys

import klayout.db as db

print = functools.partial(print, flush=True)


def level_counts(path):
    """{(level, layer, dt): members} + max level, from the top cell."""
    ly = db.Layout(False)
    ly.read(path)
    tops = [c for c in ly.each_cell() if not c.parent_cells()]
    assert len(tops) == 1, (path, [t.name for t in tops])
    out = {}
    level = {tops[0].cell_index(): 0}
    order = [tops[0].cell_index()]
    # BFS by first-seen level (klayout variants sit at design depth)
    i = 0
    while i < len(order):
        ci = order[i]
        i += 1
        cell = ly.cell(ci)
        lv = level[ci]
        for li in ly.layer_indexes():
            info = ly.get_info(li)
            n = sum(1 for _ in cell.shapes(li).each())
            if n:
                key = (lv, info.layer, info.datatype)
                out[key] = out.get(key, 0) + n
        for inst in cell.each_inst():
            ch = inst.cell_index
            if ch not in level:
                level[ch] = lv + 1
                order.append(ch)
    # drop pya refs before _destroy (lingering destructors touch
    # freed layout memory - flaky segfaults)
    cell = inst = None
    ly._destroy()
    return out, max(level.values())


def main():
    src = sys.argv[1]
    outdir = sys.argv[2]
    meta = json.load(open(src + ".tiles/meta.json"))
    g = meta["grid"]
    nb = len(meta["bands"]["thresholds_um"]) + 1
    bad = checked = 0
    for r in range(g["ny"]):
        for c in range(g["nx"]):
            for k in range(nb):
                kp = os.path.join(src + ".tiles", "tiles_b%d" % k,
                                  "t_%d_%d.oas" % (r, c))
                rp = os.path.join(outdir, "tiles_b%d" % k,
                                  "t_%d_%d.oas" % (r, c))
                if not (os.path.isfile(kp) and os.path.isfile(rp)):
                    continue
                ka, kd = level_counts(kp)
                ra, rd = level_counts(rp)
                checked += 1
                if kd != rd:
                    bad += 1
                    print("DEPTH t_%d_%d b%d: klayout %d levels vs "
                          "rust %d" % (r, c, k, kd, rd))
                if ka != ra:
                    bad += 1
                    keys = set(ka) | set(ra)
                    diffs = [key for key in sorted(keys)
                             if ka.get(key) != ra.get(key)]
                    print("LEVELS t_%d_%d b%d: %d differing "
                          "(level,layer) buckets, e.g. %s: "
                          "klayout %s vs rust %s"
                          % (r, c, k, len(diffs), diffs[0],
                             ka.get(diffs[0]), ra.get(diffs[0])))
    print("depth-checked %d band files, failures: %d"
          % (checked, bad))
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
