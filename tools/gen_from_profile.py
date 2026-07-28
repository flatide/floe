#!/usr/bin/env python3
"""Synthesize a render-performance lookalike OASIS from a `floe profile`
JSON (structure-only: counts, grid, density tables - no real geometry).

The generator reproduces what drives floe's rendering cost:
  - the same die/grid and per-tile, per-layer, per-DEPTH-LEVEL shape
    counts (the density table), so auto depth picks the same levels and
    rasterization draws comparable shape volumes;
  - the same per-level instance counts ("cells"), so outline-frame costs
    and hierarchy traversal match;
  - array-heavy instancing (CellInstArray grids), mirroring how bitcell/
    fill arrays explode when loaded;
  - a shape-type mix (box vs polygon, vertex counts) from the sampled
    tiles.
Shape POSITIONS are random within their level cell - layouts look like
noise, but load/render timing tracks the original.

    floe profile chip.oas --out prof.json          # on the closed host
    python tools/gen_from_profile.py prof.json --out sample.oas
    floe index sample.oas && floe view sample.oas  # reproduce, optimize
"""

import argparse
import functools
import json
import math
import random

print = functools.partial(print, flush=True)

import klayout.db as db

FAST = False


def save_opts():
    opt = db.SaveLayoutOptions()
    opt.format = "OASIS"
    opt.oasis_write_cblocks = True
    opt.oasis_compression_level = 2
    opt.write_context_info = False
    return opt


def global_mix(prof):
    """Per-layer shape-type mix averaged over the sampled tiles."""
    mix = {}
    for s in prof.get("samples") or []:
        for key, m in (s.get("shape_mix") or {}).items():
            mix.setdefault(key, []).append(m)
    out = {}
    for key, ms in mix.items():
        out[key] = {
            "polygon": sum(m["polygon"] for m in ms) / len(ms),
            "poly_pts": max(4, round(sum(m["poly_pts_p50"] for m in ms)
                                     / len(ms))),
        }
    return out


def stair_polygon(x, y, w, h, pts, rnd):
    """Rectilinear staircase polygon with ~pts vertices inside (w, h)."""
    steps = max(1, (pts - 2) // 4)
    sw, sh = max(1, w // (steps + 1)), max(1, h // (steps + 1))
    ring = [db.Point(x, y)]
    cx, cy = x, y
    for _ in range(steps):
        cx += sw
        ring.append(db.Point(cx, cy))
        cy += sh
        ring.append(db.Point(cx, cy))
    ring.append(db.Point(x + w, cy))
    ring.append(db.Point(x + w, y + h))
    ring.append(db.Point(x, y + h))
    return db.Polygon(ring)


def fill_shapes(cell, li, n, ext_w, ext_h, mix, rnd, cap, dropped):
    """n pseudo-random shapes inside (0,0)-(ext_w,ext_h) of cell.
    Kept cheap (~1us/shape): flat-heavy profiles insert many millions
    python-side, so sizes cycle through 32 precomputed variants and
    positions use two raw random() calls."""
    if n > cap:
        dropped[0] += n - cap
        n = cap
    poly_frac = mix.get("polygon", 0.0) if mix else 0.0
    pts = mix.get("poly_pts", 6) if mix else 6
    shapes = cell.shapes(li)
    lo_w, hi_w = max(1, ext_w // 200), max(2, ext_w // 40)
    lo_h, hi_h = max(1, ext_h // 200), max(2, ext_h // 40)
    # every shape gets DISTINCT dims (prime-stride pseudo-random, no
    # python-random cost): identical boxes would collapse into OASIS
    # repetition records, making tile files unrealistically small and
    # fast to parse - the opposite of what the clone must reproduce
    span_w = max(1, hi_w - lo_w)
    span_h = max(1, hi_h - lo_h)
    off = rnd.randrange(1 << 16)
    rr = rnd.random
    n_poly = int(n * poly_frac)
    insert = shapes.insert
    Box = db.Box
    for i in range(n):
        w = lo_w + (i * 7919 + off) % span_w
        h = lo_h + (i * 104729 + off) % span_h
        x = int(rr() * (ext_w - w)) if ext_w > w else 0
        y = int(rr() * (ext_h - h)) if ext_h > h else 0
        if i < n_poly:
            insert(stair_polygon(x, y, w, h, pts, rnd))
        else:
            insert(Box(x, y, x + w, y + h))


def bulk_fill(ly, cell, li, n, ext_w, ext_h, mix, rnd, atoms):
    """Materialize n STORED shapes in cell fast: python-insert a ~1000-
    shape atom cell, place it as a CellInstArray, then C++ flatten() it
    into real records (~19M shapes/s measured). Small n inserts directly.
    The caller must flatten the cell (one level) afterwards; atom cells
    are collected in `atoms` for deletion at the end."""
    # Discrete records are the point: the atom+flatten shortcut clones
    # identical shapes that OASIS re-collapses into repetition records,
    # making tiles unrealistically small/fast to parse. Direct insertion
    # at ~1us/shape costs a few minutes for 10^8 shapes - acceptable for
    # a one-time fidelity clone. (Set --fast for the old shortcut.)
    if not FAST or n <= 4000:
        fill_shapes(cell, li, n, ext_w, ext_h, mix, rnd, n + 1, [0])
        return
    atom_n = 1000
    reps = n // atom_n
    nx = max(1, int(math.ceil(math.sqrt(reps))))
    ny = max(1, int(math.ceil(reps / nx)))
    aw, ah = max(4, ext_w // nx), max(4, ext_h // ny)
    atom = ly.create_cell("ATOM")
    atoms.append(atom.cell_index())
    fill_shapes(atom, li, atom_n, aw, ah, mix, rnd, atom_n + 1, [0])
    cell.insert(db.CellInstArray(atom.cell_index(), db.Trans(0, 0),
                                 db.Vector(aw, 0), db.Vector(0, ah),
                                 nx, ny))
    rest = n - reps * atom_n
    if rest:
        fill_shapes(cell, li, rest, ext_w, ext_h, mix, rnd, rest + 1, [0])


def build_tile(ly, lmap, table, tile_w, tile_h, rnd, mix, tag, cap,
               dropped, scale=1.0, flat_ratio=None):
    """One tile cell reproducing the density table with two chains:

    ARRAY chain - shared level cells multiplied by CellInstArray grids so
    cells[k] instances enter level k; shapes there are stored once and
    expand at load/render (bitcell/array behavior).
    FLAT chain - one single-instance cell per level (multiplicity 1);
    shapes there are materialized as real stored records (fill behavior:
    stored == expanded, dominating tile parse time).

    flat_ratio[key] (stored/expanded per layer, from the profile) splits
    each level delta between the chains."""
    flat_ratio = flat_ratio or {}
    cells_arr = [max(1, v) for v in table.get("cells", [1])]
    K = len(cells_arr) - 1
    lvl = [ly.create_cell("T%s_L%d" % (tag, k)) for k in range(K + 1)]
    flat = [lvl[0]] + [ly.create_cell("T%s_F%d" % (tag, k))
                       for k in range(1, K + 1)]
    ext = [(tile_w, tile_h)]
    grids = []
    for k in range(1, K + 1):
        want = max(1, round(cells_arr[k] / cells_arr[k - 1]))
        nx = max(1, int(math.ceil(math.sqrt(want))))
        ny = max(1, int(math.ceil(want / nx)))
        pw, ph = ext[k - 1]
        cw, ch = max(4, pw // nx), max(4, ph // ny)
        grids.append((nx, ny, cw, ch))
        ext.append((cw, ch))
    atoms = []
    for key, arr in table.items():
        if key == "cells":
            continue
        li = lmap.get(key)
        if li is None:
            continue
        fr = flat_ratio.get(key, 0.0)
        prev = 0
        for k, cum in enumerate(arr):
            delta, prev = cum - prev, cum
            delta = int(delta * scale)
            if delta <= 0:
                continue
            k2 = min(k, K)
            n_flat = int(delta * fr)
            n_mult = delta - n_flat
            if n_mult:
                per = max(1, n_mult // cells_arr[k2])
                fill_shapes(lvl[k2], li, per, ext[k2][0], ext[k2][1],
                            mix.get(key), rnd, cap, dropped)
            if n_flat:
                bulk_fill(ly, flat[k2], li, min(n_flat, cap * 50),
                          tile_w, tile_h, mix.get(key), rnd, atoms)
                if n_flat > cap * 50:
                    dropped[0] += n_flat - cap * 50
    # materialize the flat chain, then wire both chains
    for k in range(K + 1):
        if flat[k].child_instances():
            flat[k].flatten(1, True)
    for k in range(1, K + 1):
        nx, ny, cw, ch = grids[k - 1]
        lvl[k - 1].insert(db.CellInstArray(
            lvl[k].cell_index(), db.Trans(0, 0),
            db.Vector(cw, 0), db.Vector(0, ch), nx, ny))
        # flat chain: single instance per level keeps multiplicity 1
        flat_parent = lvl[0] if k == 1 else flat[k - 1]
        flat_parent.insert(db.CellInstArray(flat[k].cell_index(),
                                            db.Trans(0, 0)))
    live = [ci for ci in atoms if ly.is_valid_cell_index(ci)
            and ly.cell(ci) is not None]
    if live:  # flatten(prune=True) usually removed them already
        ly.delete_cells(live)
    return lvl[0]


def main():
    ap = argparse.ArgumentParser(
        description="build a lookalike OASIS from a `floe profile` JSON")
    ap.add_argument("profile")
    ap.add_argument("--out", default="sample.oas")
    ap.add_argument("--seed", type=int, default=7)
    ap.add_argument("--fast", action="store_true",
                    help="atom+flatten shortcut: much faster + tiny "
                         "files, but tile parse cost is unrealistic")
    ap.add_argument("--scale", type=float, default=1.0,
                    help="multiply all shape counts (e.g. 0.1 for a "
                         "quick small clone)")
    ap.add_argument("--max-shapes-per-cell", type=int, default=200_000,
                    help="python-side insert cap per (cell, layer, level) "
                         "- exceeding counts are dropped and reported")
    args = ap.parse_args()
    global FAST
    FAST = args.fast
    prof = json.load(open(args.profile))
    if prof.get("profile_version") != 1:
        raise SystemExit("unsupported profile version")
    g = prof["grid"]
    rnd = random.Random(args.seed)
    ly = db.Layout()
    ly.dbu = prof["dbu"]
    top = ly.create_cell("PROFILE_TOP")
    lmap = {}
    for l in prof["layers"]:
        key = "%d/%d" % (l["layer"], l["datatype"])
        lmap[key] = ly.layer(db.LayerInfo(l["layer"], l["datatype"],
                                          l["name"]))
    bb = prof["bbox"]
    if "0/0" in lmap:  # die boundary helps fit/skeleton look right
        top.shapes(lmap["0/0"]).insert(db.Box(bb[0], bb[1], bb[2], bb[3]))
    dens = (prof.get("density") or {}).get("tiles", {})
    mix = global_mix(prof)
    # stored/expanded per layer: 1.0 = flat fills (every record parsed
    # at load), ~0 = heavy arrays (records expand at load/render)
    expanded = {}
    for table in dens.values():
        for key, arr in table.items():
            if key != "cells":
                expanded[key] = expanded.get(key, 0) + arr[-1]
    flat_ratio = {}
    for l in prof["layers"]:
        key = "%d/%d" % (l["layer"], l["datatype"])
        exp = expanded.get(key, 0)
        if exp:
            flat_ratio[key] = max(0.0, min(1.0, l["stored_shapes"] / exp))
    dropped = [0]
    done = 0
    for rc in sorted(dens):
        r, c = (int(v) for v in rc.split(","))
        tag = "%d_%d" % (r, c)
        tcell = build_tile(ly, lmap, dens[rc], g["tile_w"], g["tile_h"],
                           rnd, mix, tag, args.max_shapes_per_cell,
                           dropped, args.scale, flat_ratio)
        top.insert(db.CellInstArray(
            tcell.cell_index(),
            db.Trans(g["x0"] + c * g["tile_w"],
                     g["y0"] + r * g["tile_h"])))
        done += 1
        if done % 32 == 0 or done == len(dens):
            print(f"[gen] tiles {done}/{len(dens)}")
    ly.write(args.out, save_opts())
    import os
    size = os.path.getsize(args.out)
    print(f"[gen] wrote {args.out} ({size / 1e6:.1f} MB, "
          f"{ly.cells()} cells)")
    nx = g["nx"]
    tile_mb = max(0.01, size / (nx * nx) / 1e6)
    print(f"[gen] index with the ORIGINAL grid ({nx}x{nx}):")
    print(f"    floe index {args.out} --tile-mb {tile_mb:.3f}")
    if dropped[0]:
        print(f"[gen][warn] shape cap dropped {dropped[0]:,} python-side "
              f"inserts (raise --max-shapes-per-cell for more fidelity)")
    # target-vs-generated per-layer totals (multiplicity-expanded counts
    # differ from stored counts; compare the flat expansion estimate)
    want = {("%d/%d" % (l["layer"], l["datatype"])): l["stored_shapes"]
            for l in prof["layers"]}
    print("[gen] per-layer stored-shape targets (from profile):")
    for key in sorted(want, key=lambda k: -want[k])[:8]:
        print(f"    {key:>8}  target {want[key]:>14,}")


if __name__ == "__main__":
    main()
