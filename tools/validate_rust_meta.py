"""M2 validation: the Rust `index` cache-completeness outputs vs the
klayout-built cache.

  - meta.json scalars: version / dbu / top_cell / bbox / grid /
    src(size, mtime) / layers (ORDER, names, colors, stored_shapes) /
    bands / lod(cap, tiles)
  - density: compared EXACTLY against an oracle recomputed from the
    RUST band files (meta must describe the cache it ships with - the
    contract the viewer relies on), and DIRECTIONALLY against the
    Python meta (rust <= py element-wise, same layers, same depths).
    Exact parity with klayout's numbers is impossible by design:
    klayout's clip carries each_overlapping's conservative
    approximation into the tile - its band files really contain
    duplicate boundary sub-arrays coinciding with members of the main
    block (valmini MEGAFILL tile 0,2: the 1x425 column at x=302440
    duplicates the last column of the 388x425 block; coverage XOR
    cannot see that, instance counts do). The Rust counts are the
    exact distinct-content counts.
  - tiles_lod: existence for every non-empty tile + coverage XOR per
    (layer, datatype) INCLUDING the ghost layer 254/0 - a wrong depth
    cut or ghost bbox shows up there - and cut depths (lod.tiles)
    equal to the Python meta.

usage: python tools/validate_rust_meta.py <src.oas> <rust_outdir>
       (needs <src.oas>.ice built by the Python indexer and
       <rust_outdir> built by `floe-index index`)
"""
import functools
import json
import os
import sys
import time

import klayout.db as db

print = functools.partial(print, flush=True)

GHOST = (254, 0)


def fail(msgs, text):
    msgs.append(text)
    print(text)


# --------------------------------------------------- density oracle

def _base(name, k):
    """Design-level cell name of a band-file cell (bands mirror the
    same tile tree: NAME[$v]__b<k>, root TILE_r_c_b<k> -> 'TILE')."""
    suf = "__b%d" % k
    return name[:-len(suf)] if name.endswith(suf) else "TILE"


def density_from_bands(bdir, r, c, nb, max_levels):
    """Recompute the per-tile density table from a cache's band files:
    the same tile tree is mirrored into every band, so shapes ADD
    across bands while instance edges DEDUPE (per-edge count = max
    over bands; identity includes the full member-offset multiset -
    klayout holds OASIS point-list repetitions as IRREGULAR arrays
    where is_regular_array() is False but size() counts members).
    Returns the {"l/dt": [...], "cells": [...]} dict or None."""
    shapes = {}  # base -> {"l/dt": members}
    edges = {}   # base -> {edge_key: [child, members, count]}
    found = False
    for k in range(nb):
        p = os.path.join(bdir, "tiles_b%d" % k,
                         "t_%d_%d.oas" % (r, c))
        if not os.path.isfile(p):
            continue
        found = True
        ly = db.Layout(False)
        ly.read(p)
        infos = {li: "%d/%d" % (ly.get_info(li).layer,
                                ly.get_info(li).datatype)
                 for li in ly.layer_indexes()}
        for cell in ly.each_cell():
            base = _base(cell.name, k)
            sh = shapes.setdefault(base, {})
            for li, key in infos.items():
                n = sum(1 for _ in cell.shapes(li).each())
                if n:
                    sh[key] = sh.get(key, 0) + n
            cnt = {}
            for inst in cell.each_inst():
                ia = inst.cell_inst
                child = _base(ly.cell(inst.cell_index).name, k)
                if ia.is_regular_array():
                    ek = (child, str(ia.trans), ia.na, ia.nb,
                          str(ia.a), str(ia.b))
                else:  # iterated array: exact member multiset
                    ek = (child, tuple(sorted(
                        str(t) for t in ia.each_trans())))
                members = ia.size()
                e = cnt.setdefault(ek, [child, members, 0])
                e[2] += 1
            ed = edges.setdefault(base, {})
            for ek, (child, members, n) in cnt.items():
                cur = ed.get(ek)
                if cur is None or n > cur[2]:
                    ed[ek] = [child, members, n]
        ly._destroy()
    if not found:
        return None
    keys = sorted({key for sh in shapes.values() for key in sh})
    totals = dict.fromkeys(keys, 0)
    arrs = {key: [] for key in keys}
    cells_arr = []
    level, depth = {"TILE": 1}, 0
    while level:
        if depth <= max_levels:
            cells_arr.append(sum(level.values()))
        nxt = {}
        for base, mult in level.items():
            for key, n in shapes.get(base, {}).items():
                totals[key] += n * mult
            for child, members, n in edges.get(base, {}).values():
                nxt[child] = nxt.get(child, 0) + members * n * mult
        if depth < max_levels:
            for key in keys:
                arrs[key].append(totals[key])
        level, depth = nxt, depth + 1
    for key, arr in arrs.items():
        arr[-1] = totals[key]
    out = {key: arr for key, arr in arrs.items() if arr[-1]}
    if out:
        out["cells"] = cells_arr
    return out or None


# -------------------------------------------------------- lod XOR

def flatten(path):
    """{(layer, dt): Region} for one file, or None if missing."""
    if not os.path.isfile(path):
        return None
    ly = db.Layout(False)
    ly.read(path)
    tops = [c for c in ly.each_cell() if not c.parent_cells()]
    out = {}
    for li in ly.layer_indexes():
        info = ly.get_info(li)
        reg = db.Region()
        for top in tops:
            reg += db.Region(ly.begin_shapes(top, li))
        if not reg.is_empty():
            out[(info.layer, info.datatype)] = reg
    return out, ly


def main():
    src = sys.argv[1]
    outdir = sys.argv[2]
    icedir = src + ".ice"
    py = json.load(open(os.path.join(icedir, "meta.json")))
    rs = json.load(open(os.path.join(outdir, "meta.json")))
    bad = []

    for key in ("version", "dbu", "top_cell", "bbox", "grid"):
        if py[key] != rs[key]:
            fail(bad, "META %s: py=%r rust=%r" % (key, py[key], rs[key]))
    for key in ("size", "mtime"):
        if py["src"][key] != rs["src"][key]:
            fail(bad, "META src.%s: py=%r rust=%r"
                 % (key, py["src"][key], rs["src"][key]))
    if py["bands"]["thresholds_um"] != rs["bands"]["thresholds_um"]:
        fail(bad, "META bands: py=%r rust=%r"
             % (py["bands"], rs["bands"]))
    if len(py["layers"]) != len(rs["layers"]):
        fail(bad, "META layers: %d vs %d entries"
             % (len(py["layers"]), len(rs["layers"])))
    recount = None
    for a, b in zip(py["layers"], rs["layers"]):
        if a == b:
            continue
        akey = {k: v for k, v in a.items() if k != "stored_shapes"}
        bkey = {k: v for k, v in b.items() if k != "stored_shapes"}
        if akey == bkey:
            # klayout Shapes.size() does not expand TEXT arrays (a
            # 12-member text repetition counts 1) and is off-by-one
            # on irregular shape arrays; the rust count is the each()
            # truth - recount the source to confirm
            if recount is None:
                recount = {}
                ly = db.Layout(False)
                ly.read(src)
                for li in ly.layer_indexes():
                    info = ly.get_info(li)
                    n = sum(sum(1 for _ in cell.shapes(li).each())
                            for cell in ly.each_cell())
                    recount[(info.layer, info.datatype)] = n
                ly._destroy()
            truth = recount.get((a["layer"], a["datatype"]))
            if truth == b["stored_shapes"]:
                continue
            fail(bad, "META layer %d/%d stored_shapes: py=%s rust=%s "
                 "each()=%s" % (a["layer"], a["datatype"],
                                a["stored_shapes"], b["stored_shapes"],
                                truth))
        else:
            fail(bad, "META layer: py=%r rust=%r" % (a, b))
    if py["lod"] != rs["lod"]:
        fail(bad, "META lod: py=%r rust=%r" % (py["lod"], rs["lod"]))

    g = py["grid"]
    nb = len(py["bands"]["thresholds_um"]) + 1
    levels = py["density"]["levels"]
    if rs["density"]["levels"] != levels:
        fail(bad, "META density.levels: %r vs %r"
             % (levels, rs["density"]["levels"]))

    t0 = time.perf_counter()
    dens_checked = lod_checked = 0
    for r in range(g["ny"]):
        for c in range(g["nx"]):
            rc = "%d,%d" % (r, c)
            oracle = density_from_bands(outdir, r, c, nb, levels)
            got = rs["density"]["tiles"].get(rc)
            pyd = py["density"]["tiles"].get(rc)
            if oracle is not None or got is not None:
                dens_checked += 1
            if oracle != got:
                keys = set(oracle or {}) | set(got or {})
                diffs = [k for k in sorted(keys)
                         if (oracle or {}).get(k) != (got or {}).get(k)]
                fail(bad, "DENSITY %s: %d differing keys, e.g. %s: "
                     "oracle=%s rust=%s"
                     % (rc, len(diffs), diffs[0],
                        (oracle or {}).get(diffs[0]),
                        (got or {}).get(diffs[0])))
            # directional cross-check vs the klayout meta: same layer
            # set, same depth structure, klayout counts never below
            # rust (its clip approximation only ever ADDS members)
            if (pyd is None) != (got is None):
                fail(bad, "DENSITY %s: py=%s rust=%s"
                     % (rc, pyd is not None, got is not None))
            elif pyd is not None:
                if set(pyd) != set(got):
                    fail(bad, "DENSITY %s: key sets differ py=%s "
                         "rust=%s" % (rc, sorted(pyd), sorted(got)))
                else:
                    for key in pyd:
                        a, b = pyd[key], got[key]
                        if len(a) != len(b) or any(
                                x < y for x, y in zip(a, b)):
                            fail(bad, "DENSITY %s %s: py=%s below "
                                 "rust=%s" % (rc, key, a, b))
            kp = os.path.join(icedir, "tiles_lod",
                              "t_%d_%d.oas" % (r, c))
            rp = os.path.join(outdir, "tiles_lod",
                              "t_%d_%d.oas" % (r, c))
            ka = flatten(kp)
            ra = flatten(rp)
            if (ka is None) != (ra is None):
                fail(bad, "LOD t_%s: klayout=%s rust=%s"
                     % (rc, ka is not None, ra is not None))
                continue
            if ka is None:
                continue
            lod_checked += 1
            kmap, kly = ka
            rmap, rly = ra
            for key in sorted(set(kmap) | set(rmap)):
                a, b = kmap.get(key), rmap.get(key)
                if a is None or b is None:
                    fail(bad, "LOD t_%s L%s: klayout=%s rust=%s"
                         % (rc, key, a is not None, b is not None))
                    continue
                x = a ^ b
                if not x.is_empty():
                    fail(bad, "LOD XOR t_%s L%s: %d polys, area %d, "
                         "e.g. %s" % (rc, key, x.count(), x.area(),
                                      next(x.each()).bbox()))
            kly._destroy()
            rly._destroy()
    print("meta-checked %d density tiles + %d lod files "
          "(ghost layer %s included), failures: %d, %.0fs"
          % (dens_checked, lod_checked, "%d/%d" % GHOST, len(bad),
             time.perf_counter() - t0))
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
