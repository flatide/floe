#!/usr/bin/env python
"""Generate a repetition-heavy OASIS stress file that mimics the
closed-host pathology: a small file (--file-mb) that EXPANDS to a huge
in-memory layout (--expand-gb) when klayout reads it in editable mode,
because OASIS shape repetitions are materialized one shape at a time.

    python tools/gen_stress.py stress30.oas --file-mb 30 --expand-gb 15

Two independent knobs:
  --file-mb    file size, made of truly-random discrete rectangles
               (~9 B each after cblocks; randomness defeats deflate)
  --expand-gb  editable-mode RAM, made of flattened atom grids that the
               OASIS writer collapses into repetition records
               (~46 B per materialized shape at read time)

Generation RAM stays bounded (~2-3 GB): each unique block is built in
its own editable Layout (C++ flatten trick, ~19M shapes/s), written to
a temp .oas, then all temps are merged in a VIEWER-mode Layout which
keeps the shape arrays compact and writes them back out as repetitions.

Reading the result:
  viewer mode   - instant, tiny RAM (what floe picks via its
                  shapes-per-byte heuristic)
  editable mode - materializes everything: ~46 B/shape, minutes on old
                  hosts (--layout-mode editable to force in floe;
                  `floe index` reads the source in viewer mode by
                  default since 0.4.3 - --read-mode editable restores
                  the full-expansion behavior)
"""
import argparse
import os
import random
import shutil
import sys
import time

import klayout.db as db

DISC_B = 9.1        # file bytes per random discrete rect (cblocks on)
RAM_B = 46          # editable-mode bytes per materialized shape
ATOM_N = 1000       # distinct boxes per atom -> repetition records
ATOM_COLS = 40      # atom internal grid: 40 x 25
ATOM_PITCH = (120, 180)
BLOCK = 1_800_000   # block extent in dbu (1.8 mm at 1 nm)


def save_opts():
    opt = db.SaveLayoutOptions()
    opt.format = "OASIS"
    opt.oasis_write_cblocks = True
    opt.oasis_compression_level = 2
    opt.write_context_info = False
    return opt


def build_block(path, u, layers, monster_layers, members, discretes,
                seed):
    """One unique block def -> temp .oas with repetitions + discretes."""
    rng = random.Random(seed * 1000 + u)
    ly = db.Layout()
    ly.dbu = 0.001
    blk = ly.create_cell("BLK_%d" % u)
    lis = [ly.layer(db.LayerInfo(i + 1, 0, "M%d" % (i + 1)))
           for i in range(layers)]

    # --- repetition monsters: atom grid -> flatten -> writer collapses
    mlis = [lis[(u + k * 3) % layers] for k in range(monster_layers)]
    per_layer = members // len(mlis)
    for li in mlis:
        atom = ly.create_cell("A_%d_%d" % (u, li))
        sh = atom.shapes(li)
        for i in range(ATOM_N):
            x = (i % ATOM_COLS) * ATOM_PITCH[0]
            y = (i // ATOM_COLS) * ATOM_PITCH[1]
            w = rng.randrange(40, 110)
            h = rng.randrange(50, 160)
            sh.insert(db.Box(x, y, x + w, y + h))
        reps = max(1, per_layer // ATOM_N)
        nx = max(1, int(reps ** 0.5))
        ny = max(1, (reps + nx - 1) // nx)
        blk.insert(db.CellInstArray(
            atom.cell_index(), db.Trans(0, 0),
            db.Vector(BLOCK // nx, 0), db.Vector(0, BLOCK // ny),
            nx, ny))
        blk.flatten(1, True)

    # --- discrete filler: truly random rects = incompressible file bulk
    per_li = discretes // layers
    for li in lis:
        sh = blk.shapes(li)
        for _ in range(per_li):
            w = rng.randrange(40, 440)
            h = rng.randrange(50, 650)
            x = rng.randrange(0, BLOCK - 700)
            y = rng.randrange(0, BLOCK - 700)
            sh.insert(db.Box(x, y, x + w, y + h))

    got = sum(blk.shapes(li).size() for li in lis)
    ly.write(path, save_opts())
    ly._destroy()
    return got


def main():
    ap = argparse.ArgumentParser(
        description="generate a small OASIS file with a huge "
                    "editable-mode expansion")
    ap.add_argument("out")
    ap.add_argument("--file-mb", type=float, default=30.0,
                    help="target file size in MB (default 30)")
    ap.add_argument("--expand-gb", type=float, default=15.0,
                    help="target editable-mode RAM in GB (default 15)")
    ap.add_argument("--layers", type=int, default=12)
    ap.add_argument("--grid", default="6x6",
                    help="top-level block grid, e.g. 6x6")
    ap.add_argument("--unique", type=int, default=8,
                    help="unique block definitions (default 8)")
    ap.add_argument("--monster-layers", type=int, default=4,
                    help="repetition-heavy layers per block")
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--scale", type=float, default=1.0,
                    help="scale both budgets (0.1 = quick test)")
    ap.add_argument("--keep-temps", action="store_true")
    a = ap.parse_args()

    bx, by = (int(v) for v in a.grid.lower().split("x"))
    members = int(a.expand_gb * a.scale * 2 ** 30 / RAM_B)
    discretes = int(a.file_mb * a.scale * 1e6 * 0.95 / DISC_B)
    u_n = min(a.unique, bx * by)
    tmpdir = a.out + ".tmp"
    os.makedirs(tmpdir, exist_ok=True)

    print("[gen] %d unique blocks on %dx%d grid, %d layers" %
          (u_n, bx, by, a.layers))
    print("[gen] repetition members %s (editable RAM ~%.1f GB), "
          "discrete rects %s (file ~%.0f MB)"
          % (format(members, ","), members * RAM_B / 2 ** 30,
             format(discretes, ","), discretes * DISC_B / 1e6))

    t0 = time.perf_counter()
    temps = []
    for u in range(u_n):
        p = os.path.join(tmpdir, "blk_%d.oas" % u)
        got = build_block(p, u, a.layers, a.monster_layers,
                          members // u_n, discretes // u_n, a.seed)
        temps.append(p)
        print("[gen] block %d/%d: %s shapes, %.1f MB (%.0fs)"
              % (u + 1, u_n, format(got, ","),
                 os.path.getsize(p) / 1e6, time.perf_counter() - t0),
              flush=True)

    # --- merge in viewer mode: shape arrays stay compact in RAM
    mv = db.Layout(False)
    for p in temps:
        mv.read(p)
    top = mv.create_cell("TOP")
    b = mv.layer(db.LayerInfo(0, 0, "BOUNDARY"))
    top.shapes(b).insert(db.Box(0, 0, bx * BLOCK, by * BLOCK))
    for j in range(by):
        for i in range(bx):
            u = (i + j * bx) % u_n
            ci = mv.cell("BLK_%d" % u)
            top.insert(db.CellInstArray(
                ci.cell_index(), db.Trans(i * BLOCK, j * BLOCK)))
    mv.write(a.out, save_opts())
    stored = sum(c.shapes(li).size() for c in mv.each_cell()
                 for li in mv.layer_indexes())
    mv._destroy()
    if not a.keep_temps:
        shutil.rmtree(tmpdir, ignore_errors=True)

    sz = os.path.getsize(a.out)
    print("[gen] wrote %s: %.1f MB, %s def-level shapes when expanded"
          % (a.out, sz / 1e6, format(stored, ",")))
    print("[gen] editable read needs ~%.1f GB RAM (%.0f B/shape); "
          "viewer read is instant" % (stored * RAM_B / 2 ** 30, RAM_B))
    print("[gen] %.1f shapes/byte -> floe picks viewer mode; force the "
          "pathology with --layout-mode editable" % (stored / sz))
    print("[gen] NOTE: floe index reads the source in viewer mode by "
          "default (--read-mode editable needs the RAM above)")
    print("[gen] done in %.0fs" % (time.perf_counter() - t0))


if __name__ == "__main__":
    main()
