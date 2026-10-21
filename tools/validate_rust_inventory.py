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


TIMING = ("read_s", "inventory_s", "inventory_mb_s", "file", "load", "seg", "load_seg", "check_seg")


def inventory_json(args, what):
    r = subprocess.run([str(BIN), "inventory"] + [str(a) for a in args], capture_output=True, text=True)
    if r.returncode != 0:
        fail("%s: exit %d\n%s" % (what, r.returncode, r.stderr[-2000:]))
    try:
        return json.loads(r.stdout)
    except ValueError:
        fail("%s: no JSON\n%s" % (what, r.stdout[-800:]))


def check(path, jobs, inv_out):
    """--check + --check-cells (G1, G2) and, in the same run, design.inv
    written with --out and read back (DIRECT_READ_PLAN L1: the file's
    round trip is byte-identical) and design.seg written with --seg,
    read back and held to the Doc with --check-seg (L2: per cell and
    layer the segments' sums, boxes and feature maxima are the Doc's)"""
    seg_out = inv_out.with_suffix(".seg")
    t0 = time.perf_counter()
    r = subprocess.run([str(BIN), "inventory", str(path), "--check", "--check-cells",
                        "--jobs", str(jobs), "--out", str(inv_out), "--seg", str(seg_out), "--check-seg"],
                       capture_output=True, text=True)
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
    f = j.get("file", {})
    if not f.get("roundtrip") or not Path(inv_out).is_file() or f.get("bytes") != Path(inv_out).stat().st_size:
        fail("inventory %s (jobs %d): design.inv round trip: %s" % (path.name, jobs, f))
    if "inventory file:" not in r.stderr or "roundtrip OK" not in r.stderr:
        fail("inventory %s: no file line\n%s" % (path.name, r.stderr[-800:]))
    g = j.get("seg", {})
    if not g.get("roundtrip") or not seg_out.is_file() or g.get("bytes") != seg_out.stat().st_size:
        fail("inventory %s (jobs %d): design.seg round trip: %s" % (path.name, jobs, g))
    if g.get("segments", 0) < 1 or g["segments"] != g["cblocks"] + g["spans"] or "segment file:" not in r.stderr:
        fail("inventory %s (jobs %d): design.seg counts: %s" % (path.name, jobs, g))
    cs = j.get("check_seg", {})
    if not cs.get("ok") or cs.get("segments") != g["segments"] or cs.get("cells_skipped", 1) != 0:
        fail("inventory %s (jobs %d): segment check: %s" % (path.name, jobs, cs))
    # L3: every segment parsed with its snapshot concatenates to the cell parse
    if cs.get("stats_ok") is not True or cs.get("parse_ok") is not True or cs.get("pieces", 0) < cs["segments"]:
        fail("inventory %s (jobs %d): segment parse: %s" % (path.name, jobs, {k: cs.get(k) for k in ("stats_ok", "parse_ok", "pieces", "segments")}))
    if "parse OK" not in r.stderr:
        fail("inventory %s: no segment parse line\n%s" % (path.name, r.stderr[-800:]))
    return j, dt


def check_file(path, inv_out, jobs):
    """design.inv read back (L1): --load prints what the source gives,
    beside the source it is identical to a fresh inventory, --check holds
    the loaded inventory to the Doc, and truncated / damaged copies are
    refused; design.seg read back (L2): paired with the inventory it was
    written with, identical to a fresh directory beside the source, and
    refused when it is another file or damaged"""
    loaded = inventory_json(["--load", inv_out, "--cells"], "inventory --load %s" % inv_out.name)
    fresh = inventory_json([path, "--cells", "--jobs", jobs], "inventory %s --cells" % path.name)
    for k in TIMING:
        loaded.pop(k, None)
        fresh.pop(k, None)
    if loaded != fresh:
        diff = [k for k in set(loaded) | set(fresh) if loaded.get(k) != fresh.get(k)]
        fail("%s: --load differs from the fresh inventory in %s" % (inv_out.name, diff))
    both = inventory_json(["--load", inv_out, path, "--check", "--jobs", jobs], "inventory --load + source %s" % path.name)
    if both.get("load", {}).get("identical") is not True or not both.get("check", {}).get("ok"):
        fail("%s: load beside the source: %s / %s" % (inv_out.name, both.get("load"), both.get("check")))
    cut = inv_out.with_suffix(".cut")
    cut.write_bytes(inv_out.read_bytes()[:200])
    r = subprocess.run([str(BIN), "inventory", "--load", str(cut)], capture_output=True, text=True)
    if r.returncode == 0 or "design.inv" not in r.stderr:
        fail("%s: a truncated design.inv was not refused (exit %d)\n%s" % (inv_out.name, r.returncode, r.stderr[-500:]))
    damaged = bytearray(inv_out.read_bytes())
    damaged[len(damaged) // 2] ^= 0x55
    cut.write_bytes(bytes(damaged))
    r = subprocess.run([str(BIN), "inventory", "--load", str(cut)], capture_output=True, text=True)
    if r.returncode == 0 or "checksum" not in r.stderr:
        fail("%s: a damaged design.inv was not refused (exit %d)\n%s" % (inv_out.name, r.returncode, r.stderr[-500:]))
    # L2: design.seg read back
    seg_out = inv_out.with_suffix(".seg")
    ls = inventory_json(["--load", inv_out, "--load-seg", seg_out], "inventory --load-seg %s" % seg_out.name)
    if ls.get("load_seg", {}).get("pair_ok") is not True:
        fail("%s: not paired with %s: %s" % (seg_out.name, inv_out.name, ls.get("load_seg")))
    ls2 = inventory_json([path, "--load-seg", seg_out, "--jobs", jobs], "inventory --load-seg + source %s" % path.name)
    if ls2.get("load_seg", {}).get("identical") is not True or ls2["load_seg"].get("pair_ok") is not True:
        fail("%s: beside the source: %s" % (seg_out.name, ls2.get("load_seg")))
    r = subprocess.run([str(BIN), "inventory", "--load", str(inv_out), "--load-seg", str(inv_out)], capture_output=True, text=True)
    if r.returncode == 0 or "design.seg" not in r.stderr:
        fail("%s: a design.inv read as design.seg was not refused (exit %d)\n%s" % (seg_out.name, r.returncode, r.stderr[-500:]))
    seg_cut = seg_out.with_suffix(".segcut")
    damaged = bytearray(seg_out.read_bytes())
    damaged[len(damaged) // 2] ^= 0x55
    seg_cut.write_bytes(bytes(damaged))
    r = subprocess.run([str(BIN), "inventory", "--load", str(inv_out), "--load-seg", str(seg_cut)], capture_output=True, text=True)
    if r.returncode == 0 or "checksum" not in r.stderr:
        fail("%s: a damaged design.seg was not refused (exit %d)\n%s" % (seg_out.name, r.returncode, r.stderr[-500:]))
    return inv_out.stat().st_size


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
            inv_files = {}
            for jobs in (1, 3):
                inv_out = work / ("%s_j%d.inv" % (path.stem, jobs))
                j, dt = check(path, jobs, inv_out)
                inv_files[jobs] = inv_out
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
            # L1: one inventory, one file - the parallel scan's bytes are the
            # serial scan's; the file reads back to the same inventory
            if inv_files[1].read_bytes() != inv_files[3].read_bytes():
                fail("%s: design.inv differs between jobs 1 and 3" % name)
            if inv_files[1].with_suffix(".seg").read_bytes() != inv_files[3].with_suffix(".seg").read_bytes():
                fail("%s: design.seg differs between jobs 1 and 3" % name)
            size = check_file(path, inv_files[1], 2)
            print("OK  %-22s design.inv %d bytes, design.seg %d bytes: round trips, jobs 1 = 3, --load = fresh, --load + source identical + check, "
                  "segment statistics = Doc, segments parsed with their snapshots = cell parse, --load-seg paired + identical, truncated and damaged copies refused"
                  % (name, size, inv_files[1].with_suffix(".seg").stat().st_size))
    print("rust_inventory: ALL OK")


if __name__ == "__main__":
    main()
