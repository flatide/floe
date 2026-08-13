"""Band-file validation: the Rust indexer vs the klayout-built cache.
Runs the full `floe-index index` build (its own grid choice must land
on the same grid as the Python cache - validate_rust_meta.py checks
that explicitly), then for every (tile, band) present on either side:

  - flatten both files to one Region per (layer, datatype)
  - XOR under merged semantics must be EMPTY (coverage-identical)

Representation differences are expected (records vs exploded members,
S-H bridge edges); coverage identity is the contract that rendering,
snap and clip rest on.

usage: python tools/validate_rust_tiles.py <src.oas> [outdir]
       (needs <src.oas>.tiles built by the Python indexer)
"""
import functools
import json
import os
import shutil
import subprocess
import sys
import time

import klayout.db as db

print = functools.partial(print, flush=True)

RUST = "rust/target/release/floe-index"


def flatten(path):
    """{(layer, dt): Region} for one band file (empty file -> {})."""
    out = {}
    if not os.path.isfile(path):
        return out
    ly = db.Layout(False)
    ly.read(path)
    tops = [c for c in ly.each_cell() if not c.parent_cells()]
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
    outdir = sys.argv[2] if len(sys.argv) > 2 else src + ".rustice"
    meta = json.load(open(src + ".tiles/meta.json"))
    g = meta["grid"]
    if os.path.isdir(outdir):   # stale grids leave orphan tiles
        shutil.rmtree(outdir)
    r = subprocess.run([RUST, "index", src, outdir],
                       capture_output=True, text=True)
    if r.returncode != 0:
        print("rust index FAILED:", r.stderr.strip())
        sys.exit(1)
    stats = json.loads(r.stdout)
    print("rust index: %s band files, grid %s in %.1fs (parse %.1fs "
          "tiles %.1fs)" % (stats["band_files"], stats["grid"],
                            stats["total_s"], stats["parse_s"],
                            stats["tiles_s"]))

    nb = len(meta["bands"]["thresholds_um"]) + 1
    bad = checked = skipped = 0
    t0 = time.perf_counter()
    for rr in range(g["ny"]):
        for cc in range(g["nx"]):
            for k in range(nb):
                kp = os.path.join(src + ".tiles", "tiles_b%d" % k,
                                  "t_%d_%d.oas" % (rr, cc))
                rp = os.path.join(outdir, "tiles_b%d" % k,
                                  "t_%d_%d.oas" % (rr, cc))
                if not os.path.isfile(kp) and not os.path.isfile(rp):
                    skipped += 1
                    continue
                ka = flatten(kp) if os.path.isfile(kp) else ({}, None)
                ra = flatten(rp) if os.path.isfile(rp) else ({}, None)
                kmap, kly = ka
                rmap, rly = ra
                keys = set(kmap) | set(rmap)
                for key in sorted(keys):
                    a = kmap.get(key)
                    b = rmap.get(key)
                    if a is None or b is None:
                        bad += 1
                        print("MISSING t_%d_%d b%d L%s: klayout=%s "
                              "rust=%s" % (rr, cc, k, key,
                                           a is not None,
                                           b is not None))
                        continue
                    x = a ^ b
                    if not x.is_empty():
                        bad += 1
                        print("XOR t_%d_%d b%d L%s: %d polys, "
                              "area %d, e.g. %s"
                              % (rr, cc, k, key, x.count(),
                                 x.area(), next(x.each()).bbox()))
                checked += 1
                if kly:
                    kly._destroy()
                if rly:
                    rly._destroy()
    print("checked %d band files (%d empty), XOR failures: %d, "
          "compare %.0fs" % (checked, skipped, bad,
                             time.perf_counter() - t0))
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
