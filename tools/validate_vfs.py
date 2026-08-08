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

# .ovm wire v5 (rust/VFS_HIER.md par.3.6 + VFS_TEXT_PLAN.md):
# header 312B with ovp_len@72, ovt_len@80 + 14 sections@88, cell
# 144B (height/topo_rank; rbbox@48; trange range@128, tmask@136),
# page 96B (seq u32@8, max_w/max_h u64@80/88)
PAGE_LEN = 104
CELL_LEN = 144


def read_ovm(path):
    d = open(path, "rb").read()
    assert d[:8] == b"FLOEOVM1", "magic"
    ver = struct.unpack_from("<I", d, 8)[0]
    assert ver == 6, ver  # v6: page max_min (hairline cut)
    top, n_layers, n_cells, n_pages = struct.unpack_from(
        "<IIII", d, 40)
    ovp_len = struct.unpack_from("<Q", d, 72)[0]
    secs = [struct.unpack_from("<QQ", d, 88 + 16 * i)
            for i in range(14)]
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
        lod = d[o + 12]
        off, csz, usz, recs = struct.unpack_from("<QIII", d, o + 48)
        mems = struct.unpack_from("<Q", d, o + 72)[0]
        max_w, max_h, max_min = struct.unpack_from("<QQQ", d, o + 80)
        if not (max_min <= min(max_w, max_h)):
            raise SystemExit("page %d max_min %d exceeds min(max_w "
                             "%d, max_h %d)" % (i, max_min, max_w,
                                                max_h))
        pages.append((cell, li, seq, off, csz, recs, mems, lod))
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
    n_lod = 0
    for (ci, lidx, seq, off, csz, recs, mems, lod) in ovm["pages"]:
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
                 "dir=%d lod=%d" % (ci, lidx, seq, got_m, mems, lod))
        if lod:
            # LOD variants are derived coverage (M7): payload must
            # parse and match its directory counts (above), but the
            # layer conservation sums are an EXACT-page contract
            n_lod += 1
        else:
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

    # v5 text index (T4, VFS_TEXT_PLAN.md): the sidecars are GONE -
    # labels are request-scoped daemon responses; meta carries the
    # text-index tallies and design.ovt holds strings/pts pools
    with open(os.path.join(outdir, "meta.json")) as f:
        vmeta = json.load(f)
    for legacy in ("labels.tsv", "texts.tsv", "skeleton.oas"):
        if os.path.isfile(os.path.join(outdir, legacy)):
            fail("legacy sidecar %s still produced" % legacy)
    tm = vmeta.get("texts") or {}
    # klayout text truth: expanded members per layer
    ly2 = db.Layout(False)
    ly2.read(src)
    want_texts = 0
    for li in ly2.layer_indexes():
        for cell in ly2.each_cell():
            for s in cell.shapes(li).each():
                if s.is_text():
                    want_texts += 1
    ly2._destroy()
    if tm.get("members", -1) != want_texts:
        fail("meta texts members=%s klayout=%d"
             % (tm.get("members"), want_texts))
    ovt_path = os.path.join(outdir, "design.ovt")
    ovt_size = os.path.getsize(ovt_path) \
        if os.path.isfile(ovt_path) else 0
    ovm_ovt_len = struct.unpack_from(
        "<Q", open(outdir + "/design.ovm", "rb").read(88), 80)[0]
    if ovt_size != ovm_ovt_len:
        fail("ovt_len header=%d file=%d" % (ovm_ovt_len, ovt_size))

    # minimap frontier (rev 30): per-depth structural boxes vs a
    # klayout oracle - depth 0 exact (top member expansion), depth 1
    # spot (two-level expansion); min filter and cap respected
    fr = vmeta.get("frontier") or {}
    fmin = fr.get("min", 0)
    keep = fr.get("keep", 0)
    depths = fr.get("depths")
    if fmin < 1 or keep < 1 or not isinstance(depths, list):
        fail("frontier meta missing/malformed: %r" % (fr,))
        depths = []
    ly3 = db.Layout(False)
    ly3.read(src)
    die = ly3.top_cell().bbox()

    def expand(cell):
        """placed member boxes of cell's direct children (all)"""
        out = []
        for inst in cell.each_inst():
            cb = inst.cell.bbox()
            for tr in inst.cell_inst.each_trans():
                out.append(cb.transformed(tr))
            # complex transforms (magnification) don't occur in the
            # gate assets; each_trans covers rot/mirror arrays
        return out

    for d, boxes in enumerate(depths):
        if len(boxes) > keep:
            fail("frontier depth %d has %d > keep %d"
                 % (d, len(boxes), keep))
        for b in boxes:
            if max(b[2] - b[0], b[3] - b[1]) < fmin:
                fail("frontier depth %d box under min: %r" % (d, b))
            if b[0] < die.left or b[1] < die.bottom \
                    or b[2] > die.right or b[3] > die.top:
                fail("frontier depth %d box outside die: %r" % (d, b))
    trunc = fr.get("truncated") or []
    for d, want_boxes in ((0, None), (1, None)):
        if d >= len(depths) or (d < len(trunc) and trunc[d]) \
                or len(depths[d]) >= keep:
            continue
        if d == 0:
            oracle_boxes = expand(ly3.top_cell())
        else:
            oracle_boxes = []
            for inst in ly3.top_cell().each_inst():
                sub = expand(inst.cell)
                for tr in inst.cell_inst.each_trans():
                    oracle_boxes.extend(
                        b.transformed(tr) for b in sub)
        want = sorted(
            (b.left, b.bottom, b.right, b.top)
            for b in oracle_boxes
            if max(b.width(), b.height()) >= fmin)
        got = sorted(tuple(b) for b in depths[d])
        if got != want:
            fail("frontier depth %d: %d boxes vs oracle %d "
                 "(miss %s / extra %s)"
                 % (d, len(got), len(want),
                    [t for t in want if t not in got][:2],
                    [t for t in got if t not in want][:2]))
    ly3._destroy()

    print("vfs-checked %d pages (%d lod), %d layers, %d text "
          "members indexed, top rbbox, failures: %d"
          % (checked, n_lod, len(lmap), tm.get("members", 0),
             len(bad)))
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
