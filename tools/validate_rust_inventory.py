#!/usr/bin/env python3
"""G1 and G2 of the streaming index plan (docs/STREAMING_INDEX_PLAN.ko.md
§6): the inventory pass equals the in-memory build's Doc pre-passes, cell
by cell (G1), and the cell parser - every byte range through
parse_cell_range + normalize_cells - equals the whole-Doc parse's cells,
record by record (G2, `--check-cells`).

`floe-index inventory FILE --check` parses the Doc itself, runs build()'s
pre-passes (rust/cli/src/prepass.rs PrePass::from_doc) and compares them
with the inventory (floe_oasis::inventory): the cell table and its order,
the top, the layer order / names / aliases, per cell the own boxes with
and without texts, the layer and text masks, the own members, the edges
(children in first-placement order, member sums, per-orientation offset
boxes, placement counts), the recursive boxes with and without texts,
the topological order, rank, height, recursive members and masks, and
the per-layer record and member sums. Exit 1 and a list of the
differences otherwise.

This gate runs it, at jobs 1 and jobs 3 (the parallel split merges the
workers' parts), on:
  - the battery fixture (valmini: rotated and mirrored placements, three
    levels, texts, paths, non-manhattan polygons, KLayout's writer);
  - valmini re-written by KLayout without CBLOCKs and in strict mode
    (name tables at the end, cells by reference);
  - tools/gen_route_chip.py --scale 0.05 (by-reference cells, modal
    records, count-2 repetitions, million-member point lists);
  - tools/gen_main01_like.py --scale 0.001 (a deep hierarchy with
    hundreds of layers, placement-heavy).
The hand-encoded streams (trapezoids, CTRAPEZOID implied dimensions,
circles, XYRELATIVE, every repetition type, late name tables, CBLOCK
bodies, an undefined placement target) are the crates' unit tests
(floe_oasis::inventory fixtures; the `unit` gate).

usage: python tools/validate_rust_inventory.py valmini.oas
"""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
BIN = ROOT / "rust" / "target" / "release" / "floe-index"


def fail(msg):
    print("FAIL: %s" % msg)
    sys.exit(1)


def rewrite_with_klayout(src, out, cblocks, strict):
    import klayout.db as db
    ly = db.Layout()
    ly.read(str(src))
    opts = db.SaveLayoutOptions()
    opts.format = "OASIS"
    opts.oasis_write_cblocks = cblocks
    opts.oasis_strict_mode = strict
    ly.write(str(out), opts)
    ly._destroy()


def generate(script, out, *args):
    cmd = [sys.executable, "-B", str(ROOT / "tools" / script), str(out)] + list(args)
    r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True)
    if r.returncode != 0 or not Path(out).is_file():
        fail("%s: %s\n%s" % (script, r.stdout[-800:], r.stderr[-1200:]))


def check(path, jobs):
    t0 = time.perf_counter()
    r = subprocess.run([str(BIN), "inventory", str(path), "--check", "--check-cells",
                        "--jobs", str(jobs)], capture_output=True, text=True)
    dt = time.perf_counter() - t0
    if r.returncode != 0:
        fail("inventory --check %s (jobs %d) exit %d\n%s" % (path.name, jobs, r.returncode, r.stderr[-3000:]))
    try:
        j = json.loads(r.stdout)
    except ValueError:
        fail("inventory %s: no JSON\n%s" % (path.name, r.stdout[-800:]))
    if not j.get("check", {}).get("ok"):
        fail("inventory %s (jobs %d): check not ok: %s" % (path.name, jobs, j.get("check")))
    if not j.get("check_cells", {}).get("ok"):
        fail("inventory %s (jobs %d): cell check not ok: %s" % (path.name, jobs, j.get("check_cells")))
    if j["cells"] < 1 or j["layers"] < 1 or j["bbox"] is None:
        fail("inventory %s: empty result %s" % (path.name, {k: j[k] for k in ("cells", "layers", "bbox")}))
    return j, dt


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(2)
    src = Path(sys.argv[1]).resolve()
    if not BIN.is_file():
        fail("release floe-index is not built")
    if not src.is_file():
        fail("source missing: %s" % src)
    with tempfile.TemporaryDirectory(prefix="floe-inventory-") as td:
        work = Path(td)
        files = [("valmini", src)]
        plain = work / "valmini_nocblock.oas"
        rewrite_with_klayout(src, plain, cblocks=False, strict=False)
        files.append(("valmini no-cblock", plain))
        strict = work / "valmini_strict.oas"
        rewrite_with_klayout(src, strict, cblocks=True, strict=True)
        files.append(("valmini strict+cblock", strict))
        route = work / "route_chip.oas"
        generate("gen_route_chip.py", route, "--scale", "0.05", "--jobs", "2")
        files.append(("route_chip 0.05", route))
        main01 = work / "main01_like.oas"
        generate("gen_main01_like.py", main01, "--scale", "0.001", "--jobs", "2")
        files.append(("main01_like 0.001", main01))
        first = None
        for name, path in files:
            for jobs in (1, 3):
                j, dt = check(path, jobs)
                key = (j["cells"], j["layers"], j["top"], j["bbox"], j["height"], j["top_members"])
                if jobs == 1:
                    first = key
                elif key != first:
                    fail("%s: jobs 1 vs 3 disagree: %s vs %s" % (name, first, key))
                print("OK  %-22s jobs %d: %d cells, %d layers, height %d, bbox %s; "
                      "inventory %.2fs, doc pre-pass %.2fs, cell parser %d pieces %.2fs (%.1fs wall)"
                      % (name, jobs, j["cells"], j["layers"], j["height"], j["bbox"],
                         j["inventory_s"], j["check"]["doc_s"], j["check_cells"]["pieces"],
                         j["check_cells"]["parse_s"], dt))
    print("rust_inventory: ALL OK")


if __name__ == "__main__":
    main()
