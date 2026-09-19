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
    assert ver == 8, ver  # v8: bvh subtree layer masks
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


BVH_LEN = 56       # v8: +lmask_rec@48, +lmask_direct@52
PLACE_LEN = 64
LMASK_UNKNOWN = 0xFFFFFFFF


def check_bvh_masks(path):
    """v8: every annotated instance-BVH node holds exactly the union of the
    layer masks of the cells placed below it (recursive @48, own shapes @52).
    Returns (nodes, annotated, problems)."""
    d = open(path, "rb").read()
    n_layers, n_cells = struct.unpack_from("<II", d, 44)
    secs = [struct.unpack_from("<QQ", d, 88 + 16 * i) for i in range(14)]
    width = max(1, (n_layers + 7) // 8)
    bits_off, bits_len = secs[4]
    n_sets = bits_len // width

    def mask(idx):
        return int.from_bytes(d[bits_off + idx * width:bits_off + (idx + 1) * width], "little")

    cells = []
    for i in range(n_cells):
        o = secs[2][0] + CELL_LEN * i
        cells.append(struct.unpack_from("<II", d, o + 104))      # (direct, recursive)
    places_off = secs[3][0]
    bvh_off, bvh_len = secs[5]
    n_nodes = bvh_len // BVH_LEN
    assert bvh_len % BVH_LEN == 0, "bvh stride"
    truth = [None] * n_nodes            # (recursive, direct) unions, children follow parents
    problems, annotated = [], 0
    for i in range(n_nodes - 1, -1, -1):
        o = bvh_off + BVH_LEN * i
        first, count, leaf = struct.unpack_from("<IHH", d, o + 32)
        rec = own = 0
        for k in range(first, first + count):
            if leaf:
                child = struct.unpack_from("<I", d, places_off + PLACE_LEN * k)[0]
                rec |= mask(cells[child][1])
                own |= mask(cells[child][0])
            else:
                rec |= truth[k][0]
                own |= truth[k][1]
        truth[i] = (rec, own)
        m_rec, m_own = struct.unpack_from("<II", d, o + 48)
        if m_rec == LMASK_UNKNOWN and m_own == LMASK_UNKNOWN:
            continue
        annotated += 1
        if m_rec >= n_sets or m_own >= n_sets:
            problems.append("bvh node %d mask index out of the pool" % i)
        elif (mask(m_rec), mask(m_own)) != (rec, own):
            problems.append("bvh node %d masks %x/%x, the placements below hold %x/%x"
                            % (i, mask(m_rec), mask(m_own), rec, own))
    return n_nodes, annotated, problems


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

    # minimap frontier (rev 46b): baked through the real planner at
    # the canonical fit scale. Schema/sanity here; the equivalence
    # oracle (bake == vfsd mode=frontier replay at the canonical
    # parameters) is L9 in validate_vfs_lifecycle.py.
    fr = vmeta.get("frontier") or {}
    keep = fr.get("keep", 0)
    depths = fr.get("depths")
    if keep < 1 or not isinstance(depths, list) \
            or "px_per_um" not in fr or "cut_px" not in fr:
        fail("frontier meta missing/malformed: %r"
             % (sorted(fr.keys()),))
        depths = []
    ly3 = db.Layout(False)
    ly3.read(src)
    die = ly3.top_cell().bbox()
    for d, boxes in enumerate(depths):
        if len(boxes) > keep:
            fail("frontier depth %d has %d > keep %d"
                 % (d, len(boxes), keep))
        for b in boxes:
            if len(b) != 5 or not (0 <= b[4] <= 3):
                fail("frontier depth %d row malformed: %r" % (d, b))
                break
            if b[0] < die.left or b[1] < die.bottom \
                    or b[2] > die.right or b[3] > die.top:
                fail("frontier depth %d box outside die: %r" % (d, b))
    if depths and not depths[0]:
        fail("frontier depth 0 is empty on a populated die")
    ly3._destroy()

    print("vfs-checked %d pages (%d lod), %d layers, %d text "
          "members indexed, top rbbox, failures: %d"
          % (checked, n_lod, len(lmap), tm.get("members", 0),
             len(bad)))
    # v8 node layer masks: the battery cache (nodes under 64 placements stay
    # unknown) and a rebuild that annotates EVERY node, both checked against
    # the placements; the same bytes at 1 and 4 jobs
    nodes, annotated, problems = check_bvh_masks(outdir + "/design.ovm")
    for msg in problems:
        fail(msg)
    with tempfile.TemporaryDirectory(prefix="floe-ovm8-") as temp:
        built = []
        for jobs in ("1", "4"):
            out = os.path.join(temp, "j" + jobs)
            env = dict(os.environ, FLOE_INDEX_BVH_MASK_MIN="1")
            run = subprocess.run([fi, "vfs", src, out, "--jobs", jobs], env=env,
                                 capture_output=True, text=True)
            if run.returncode != 0:
                fail("v8 rebuild failed: " + run.stderr[-300:])
                break
            built.append(open(out + "/design.ovm", "rb").read())
        if len(built) == 2:
            if built[0] != built[1]:
                fail("design.ovm differs between --jobs 1 and 4 with every node annotated")
            all_nodes, all_annotated, problems = check_bvh_masks(os.path.join(temp, "j1", "design.ovm"))
            for msg in problems:
                fail(msg)
            if all_nodes and all_annotated != all_nodes:
                fail("FLOE_INDEX_BVH_MASK_MIN=1 annotated %d of %d nodes" % (all_annotated, all_nodes))
            print("bvh layer masks: %d nodes (%d annotated at the default threshold, %d of %d with "
                  "threshold 1), all equal to the placements below" % (nodes, annotated, all_annotated, all_nodes))
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
