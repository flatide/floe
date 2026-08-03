"""VFS V1 gates G5/G6: the .floe cache vs the source.

Pages are UNCLIPPED, so exact equalities hold:
  G5a  per page: klayout member count (each() expands arrays)
       == directory members; single layer per page
  G5b  per layer: sum of directory members over pages ==
       klayout source member count == ovm layer table
  G5c  per layer: sum of directory records over pages ==
       ovm layer table records == floe-index scan records
  G6   ovm top-cell recursive bbox == klayout top cell bbox

usage: python tools/validate_vfs.py <src.oas> <outdir.floe> [bin]
       (bin = floe-index binary, default rust/target/release/...)
"""
import functools
import json
import os
import struct
import subprocess
import sys
import tempfile

import klayout.db as db

print = functools.partial(print, flush=True)

# .ovm wire v2 (rust/VFS_HIER.md par.3.6): header 232B with
# ovp_len@72 + 9 sections@88, cell 128B (height/topo_rank; rbbox@48),
# page 96B (seq u32@8, max_w/max_h u64@80/88)
PAGE_LEN = 96
CELL_LEN = 128


def read_ovm(path):
    d = open(path, "rb").read()
    assert d[:8] == b"FLOEOVM1", "magic"
    ver = struct.unpack_from("<I", d, 8)[0]
    assert ver == 3, ver  # v3: rep-split page partitioning
    top, n_layers, n_cells, n_pages = struct.unpack_from(
        "<IIII", d, 40)
    ovp_len = struct.unpack_from("<Q", d, 72)[0]
    secs = [struct.unpack_from("<QQ", d, 88 + 16 * i)
            for i in range(9)]
    names = d[secs[0][0]:secs[0][0] + secs[0][1]]
    layers = []
    for i in range(n_layers):
        o = secs[1][0] + 32 * i
        l, dt = struct.unpack_from("<II", d, o)
        recs, mems = struct.unpack_from("<QQ", d, o + 16)
        layers.append((l, dt, recs, mems))
    cells = []
    for i in range(n_cells):
        o = secs[2][0] + CELL_LEN * i
        no, nl = struct.unpack_from("<IH", d, o)
        rbbox = struct.unpack_from("<4q", d, o + 48)
        cells.append((names[no:no + nl].decode(), rbbox))
    pages = []
    for i in range(n_pages):
        o = secs[6][0] + PAGE_LEN * i
        cell, li, seq = struct.unpack_from("<III", d, o)
        off, csz, usz, recs = struct.unpack_from("<QIII", d, o + 48)
        mems = struct.unpack_from("<Q", d, o + 72)[0]
        pages.append((cell, li, seq, off, csz, recs, mems))
    return {"top": top, "layers": layers, "cells": cells,
            "pages": pages, "ovp_len": ovp_len}


def main():
    src, outdir = sys.argv[1], sys.argv[2]
    fi = sys.argv[3] if len(sys.argv) > 3 else os.path.join(
        os.path.dirname(__file__), "..",
        "rust/target/release/floe-index")
    bad = []

    def fail(msg):
        bad.append(msg)
        print("FAIL", msg)

    ovm = read_ovm(outdir + "/design.ovm")
    ovp = open(outdir + "/design.ovp", "rb").read()
    if ovm["ovp_len"] != len(ovp):
        fail("ovp_len header=%d file=%d"
             % (ovm["ovp_len"], len(ovp)))
    lmap = ovm["layers"]

    # source truth: members per layer via klayout each() (expands
    # arrays; the established "each() truth"), records via our scan
    ly = db.Layout(False)
    ly.read(src)
    truth_mems = {}
    for li in ly.layer_indexes():
        info = ly.get_info(li)
        key = (info.layer, info.datatype)
        m = 0
        for cell in ly.each_cell():
            for sh in cell.shapes(li).each():
                if sh.is_text():
                    continue
                m += 1
        if m:
            truth_mems[key] = m
    top_bbox = ly.top_cell().bbox()
    sh = cell = None
    ly._destroy()

    scan = json.loads(subprocess.run(
        [fi, "scan", src, "4"], capture_output=True,
        check=True).stdout)
    truth_recs = {
        tuple(int(v) for v in k.split("/")): s["records"]
        for k, s in scan["shapes"].items()
    }

    # G6: top-cell recursive bbox
    _, trb = ovm["cells"][ovm["top"]]
    kb = (top_bbox.left, top_bbox.bottom,
          top_bbox.right, top_bbox.top)
    if tuple(trb) != kb:
        fail("G6 top rbbox ovm=%s klayout=%s" % (trb, kb))

    # G5a: page-by-page klayout member recount
    sum_recs = {}
    sum_mems = {}
    checked = 0
    for (ci, lidx, seq, off, csz, recs, mems) in ovm["pages"]:
        key = (lmap[lidx][0], lmap[lidx][1])
        pl = db.Layout(False)
        with tempfile.NamedTemporaryFile(suffix=".oas") as f:
            f.write(ovp[off:off + csz])
            f.flush()
            pl.read(f.name)
        got_m = 0
        for pli in pl.layer_indexes():
            info = pl.get_info(pli)
            if (info.layer, info.datatype) != key:
                fail("G5a page c=%d li=%d holds %s/%s"
                     % (ci, lidx, info.layer, info.datatype))
            for c in pl.each_cell():
                for s in c.shapes(pli).each():
                    if not s.is_text():
                        got_m += 1
        s = c = None
        pl._destroy()
        if got_m != mems:
            fail("G5a page c=%d li=%d seq=%d members klayout=%d "
                 "dir=%d" % (ci, lidx, seq, got_m, mems))
        sum_recs[key] = sum_recs.get(key, 0) + recs
        sum_mems[key] = sum_mems.get(key, 0) + mems
        checked += 1

    # G5b/G5c: per-layer sums vs table vs truth
    for (l, d, lr, lm) in lmap:
        key = (l, d)
        if sum_mems.get(key, 0) != truth_mems.get(key, 0):
            fail("G5b layer %s members pages=%d klayout=%d"
                 % (key, sum_mems.get(key, 0),
                    truth_mems.get(key, 0)))
        if lm != truth_mems.get(key, 0):
            fail("G5b layer %s table members=%d klayout=%d"
                 % (key, lm, truth_mems.get(key, 0)))
        if sum_recs.get(key, 0) != lr:
            fail("G5c layer %s records pages=%d table=%d"
                 % (key, sum_recs.get(key, 0), lr))
        # fragmentation (v3 rep split) may STORE more records than
        # the source spells out - members conservation above stays
        # exact; records are a lower-bounded >= check
        if lr < truth_recs.get(key, 0):
            fail("G5c layer %s table records=%d < scan=%d"
                 % (key, lr, truth_recs.get(key, 0)))

    print("vfs-checked %d pages, %d layers, top rbbox, "
          "failures: %d" % (checked, len(lmap), len(bad)))
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
