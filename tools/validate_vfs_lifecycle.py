"""M3 gate: hier viewer lifecycle against a live daemon
(rust/VFS_HIER.md par.3.1/3.7) - the python VfsMosaic drives real
vfsd sessions exactly like the render service does.

  L1  10-gen pan loop: per-gen view-clipped XOR vs source == 0,
      previous gens' WC cells gone (only the current gen's remain),
      page registry consistent (every entry resolves by name)
  L2  stale drop: a response that is never applied (ack withheld) is
      rolled back; the next gen re-sends its pages - XOR green, no
      permanent blanks (the flat path's latent bug)
  L3  partial-apply faults at EVERY apply step (par.3.7 (1)-(4):
      read / link / delete-prev / evict, via the viewport fault
      hook, plus a real bad-top link failure) -> reset_all() +
      reset=1 replay on a fresh gen -> XOR green each time
  L4  eviction (budget-mb 0): pages evicted between gens and
      re-fetched on return - XOR green, no dangling registry entries
  L5  a DROPPED first response must not lose the names= table (it is
      sent once per daemon run; the client loads it before the stale
      check) - design-name resolution still works afterwards
  L7  LOD variant cycle (M7-B): wide view swaps dense pages for
      merged coverage, per-request lod=0 reverts the same view to
      exact, lod=1 restores it, and a fine px scale is exact
  L6  budgeted streaming (--stream-kb): rounds converge, their new
      pages sum to exactly the unbudgeted cold set, the final view
      XORs clean, and a dropped partial round rolls back and re-sends
      the same chunk
  L8  layers=none selects no design pages but finite depth still emits
      and applies a nonempty structural FRAME_LAYER underlay

usage: python tools/validate_vfs_lifecycle.py <src.oas> <floe_dir>
"""
import functools
import json
import math
import os
import sys

import klayout.db as db

sys.path.insert(0, os.path.dirname(os.path.dirname(
    os.path.abspath(__file__))))
from floe.vfsclient import VfsClient           # noqa: E402
from floe.viewport import VfsMosaic            # noqa: E402
from floe.service import _labels_from           # noqa: E402
from floe.render import Renderer                # noqa: E402
from floe import cache as cm                   # noqa: E402

print = functools.partial(print, flush=True)
FRAME_LAYER = (255, 0)

bad = []


def chk(cond, msg):
    if not cond:
        bad.append(msg)
        print("FAIL", msg)


def source_regions(src):
    ly = db.Layout(False)
    ly.read(src)
    top = ly.top_cell()
    bbox = top.bbox()
    regs = {}
    for li in ly.layer_indexes():
        info = ly.get_info(li)
        reg = db.Region(ly.begin_shapes(top, li))
        reg.merged_semantics = False
        regs[(info.layer, info.datatype)] = reg
    return regs, (bbox.left, bbox.bottom, bbox.right, bbox.top), ly


def xor_view(tag, sregs, mosaic, view):
    x0, y0, x1, y1 = view
    clip = db.Region(db.Box(x0, y0, x1, y1))
    ly = mosaic.ly
    vregs = {}
    for li in ly.layer_indexes():
        info = ly.get_info(li)
        key = (info.layer, info.datatype)
        if key == FRAME_LAYER:
            continue
        reg = db.Region(ly.begin_shapes(mosaic.top, li))
        reg.merged_semantics = False
        if not reg.is_empty():
            vregs[key] = reg
    for key in sorted(set(sregs) | set(vregs)):
        a = sregs.get(key, db.Region()) & clip
        b = vregs.get(key, db.Region()) & clip
        x = a ^ b
        if not x.is_empty():
            chk(False, "%s xor L%s/%s: %d polys" %
                (tag, key[0], key[1], x.count()))


def check_ledger(tag, mosaic):
    # only the current gen's WC cells may exist
    pfx = "W%d_" % mosaic.applied_gen
    for c in mosaic.ly.each_cell():
        nm = c.name
        if nm.startswith("W") and "_" in nm and not \
                nm.startswith(pfx):
            # LABELS_/FLOE_WS are not W<digit>; only real WC names
            if nm[1].isdigit():
                chk(False, "%s stale WC cell %s" % (tag, nm))
    for nm, ci in mosaic.cells.items():
        c = mosaic.ly.cell(nm)
        chk(c is not None and c.cell_index() == ci,
            "%s registry entry %s stale" % (tag, nm))
    # no ghost pages, ever: every P cell in the layout must carry
    # shapes (a partial delta may only reference MATERIALIZED pages
    # - review finding)
    for c in mosaic.ly.each_cell():
        if c.name.startswith("P") and c.name[1].isdigit():
            nsh = sum(c.shapes(li).size()
                      for li in mosaic.ly.layer_indexes())
            chk(nsh > 0, "%s ghost page cell %s" % (tag, c.name))


def check_render_stack(cache):
    """FRAME_LAYER must be first in LayoutView's actual paint stack.

    Checking Layout's numeric layer index is insufficient: KLayout's
    add_missing_layers() sorts view properties by source layer number and
    can put the runtime (max+1) frame above every design layer.
    """
    mosaic = VfsMosaic(cache)
    renderer = Renderer(mosaic.ly, mosaic.top,
                        hollow=(mosaic.FRAME_LAYER,))
    keys = [(lp.source_layer, lp.source_datatype)
            for lp in renderer.lv.each_layer()]
    fi = keys.index(mosaic.FRAME_LAYER)
    chk(all(fi < keys.index(key) for key in mosaic._layer_keys),
        "FRAME_LAYER must paint below every design layer")
    # LayoutView owns the shown Layout and releases it with the view.
    renderer.lv._destroy()


class Sess:
    """the render service's hier round, distilled"""

    def __init__(self, cache, budget_mb=1024, stream_kb=None):
        self.client = VfsClient(cache.dir, budget_mb=budget_mb,
                                stream_kb=stream_kb)
        self.m = VfsMosaic(cache)
        frame_li = self.m.ly.layer(
            db.LayerInfo(*self.m.FRAME_LAYER))
        design_lis = [self.m.ly.layer(db.LayerInfo(*key))
                      for key in self.m._layer_keys]
        chk(all(frame_li < li for li in design_lis),
            "FRAME_LAYER must be registered below every design layer")
        self.dbu = cache.meta["dbu"]

    def request(self, view, reset=False, px=1.0, lod=False,
                layers=None, depth=None, cut=0.0, frames=True,
                labels=True):
        self.m.req_gen += 1
        x0, y0, x1, y1 = view
        r = self.client.request(
            self.m.req_gen,
            (x0 * self.dbu, y0 * self.dbu,
             x1 * self.dbu, y1 * self.dbu),
            px, cut, layers, depth,
            ack=0 if reset else self.m.applied_gen,
            reset=reset, lod=lod, frames=frames, labels=labels)
        # mirror the service: names= is view-independent and sent
        # once per run, so it is consumed at REQUEST time - even a
        # response the caller then drops must not lose it
        if r["names"]:
            names_path = r["names"]
            self.m.load_names(names_path)
            chk(not os.path.exists(names_path),
                "names file not deleted after load")
        return r

    def apply(self, r, labels=None):
        return self.m.apply_hier(r["delta"], r["top"], r["evict"],
                                 labels, gen=self.m.req_gen)

    def round(self, view, reset=False):
        self.apply(self.request(view, reset))

    def stop(self):
        self.client.stop()


def main():
    src, floe_dir = sys.argv[1], sys.argv[2]
    # L1-L6 assert EXACT XOR equality and use per-request lod=False;
    # L7 below owns the explicit variant cycle.
    cache = cm.Cache(src)
    cache.dir = floe_dir
    cache.meta = json.load(open(os.path.join(floe_dir,
                                             "meta.json")))
    check_render_stack(cache)
    sregs, (bx0, by0, bx1, by1), sly = source_regions(src)
    w, h = bx1 - bx0, by1 - by0

    # ---- L1: pan loop
    s = Sess(cache)
    try:
        for i in range(10):
            x0 = bx0 + (w // 14) * i
            view = (x0, by0 + h // 4, x0 + w // 4, by1 - h // 4)
            s.round(view)
            xor_view("L1 gen%d" % s.m.req_gen, sregs, s.m, view)
            check_ledger("L1 gen%d" % s.m.req_gen, s.m)

        # ---- L2: stale drop (request consumed, never applied)
        vA = (bx0, by0, bx0 + w // 3, by0 + h // 3)
        _dropped = s.request(vA)  # noqa: F841 - deliberately unused
        vB = (bx1 - w // 3, by1 - h // 3, bx1, by1)
        s.round(vB)  # ack still points at the last APPLIED gen
        xor_view("L2 after-drop", sregs, s.m, vB)
        s.round(vA)  # the dropped view again: pages must re-arrive
        xor_view("L2 resend", sregs, s.m, vA)
        check_ledger("L2", s.m)

        # ---- L3: partial-apply faults at EVERY step (1)-(4) via the
        # viewport hook, plus a genuine bad-top link failure - each
        # one recovers through reset_all + reset=1 replay
        for step in (1, 2, 3, 4):
            r = s.request(vB)
            s.m._fault_step = step
            try:
                s.apply(r)
                chk(False, "L3 step %d did not raise" % step)
            except RuntimeError:
                pass
            s.m.reset_all()
            chk(s.m.need_reset, "L3 step %d need_reset" % step)
            s.round(vB, reset=True)
            s.m.need_reset = False
            xor_view("L3 step%d" % step, sregs, s.m, vB)
            check_ledger("L3 step%d" % step, s.m)
            # alternate views so consecutive rounds have real churn
            s.round(vA)
            xor_view("L3 step%d churn" % step, sregs, s.m, vA)
        r = s.request(vB)
        try:
            # corrupt the top name: read+clear_insts happen, link
            # fails - a genuine partial apply
            s.m.apply_hier(r["delta"], "W999999_F_0", r["evict"],
                           gen=s.m.req_gen)
            chk(False, "L3 fault did not raise")
        except RuntimeError:
            pass
        s.m.reset_all()
        chk(s.m.need_reset, "L3 need_reset unset")
        s.round(vB, reset=True)
        s.m.need_reset = False
        xor_view("L3 recovered", sregs, s.m, vB)
        check_ledger("L3", s.m)
    finally:
        s.stop()

    # ---- L8: layers=none is a real empty mask, while a finite depth
    # still emits the structural hierarchy frontier. Frames take the
    # box-size cut like geometry (rev 34): cut 0 keeps every boundary
    # box; a huge cut culls them all (second round below).
    s = Sess(cache, stream_kb=0)
    try:
        full = (bx0, by0, bx1, by1)
        r = s.request(full, px=10.0, layers=[], depth=0)
        chk(int(r.get("max_depth", -1)) >= 0,
            "L8 daemon omitted max_depth")
        chk(int(r.get("pages", -1)) == 0,
            "L8 layers=none selected design pages")
        chk(int(r.get("frame_rects", 0)) > 0,
            "L8 finite depth lost hierarchy frontier")
        labels = _labels_from(r.get("labels"), cache)
        blocks = [row for row in labels if row[6]]
        chk(blocks and all(row[5] in (0, 1) for row in blocks),
            "L8 block label orientation metadata missing")
        s.apply(r, labels)
        frame_li = s.m.ly.layer(db.LayerInfo(*s.m.FRAME_LAYER))
        it = s.m.top.begin_shapes_rec(frame_li)
        chk(not it.at_end(), "L8 FRAME_LAYER is empty after apply")
        lc = s.m.ly.cell(s.m.label_ci) if s.m.label_ci is not None else None
        centered = [] if lc is None else [
            sh.text.halign == db.Text.HAlignCenter
            and sh.text.valign == db.Text.VAlignCenter
            for sh in lc.shapes(frame_li).each() if sh.is_text()]
        chk(centered and all(centered),
            "L8 block labels are not center-aligned")
        # rev 34: a cut far above every placement footprint culls
        # ALL boundary boxes - never applied, like L5's dropped gen
        r2 = s.request(full, px=10.0, layers=[], depth=0,
                       cut=10_000_000_000.0)
        chk(int(r2.get("frame_rects", -1)) == 0,
            "L8 box size cut failed to cull sub-cut boundary boxes")
        # rev 35: the tone split survives lod=off (Sess default) -
        # the LOD kill switch must not erase the screen scale. At a
        # tiny scale every box is far under FRAME_FILL_PX: all land on
        # the smallest (dotted) band, the white dt stays empty.
        r3 = s.request(full, px=0.001, layers=[], depth=0)
        chk(int(r3.get("frame_rects", 0)) > 0,
            "L8 tiny-px round lost its frames")
        # a live recursive iterator locks the layout against read
        del it
        s.apply(r3, _labels_from(r3.get("labels"), cache))
        wit = s.m.top.begin_shapes_rec(frame_li)
        chk(wit.at_end(),
            "L8 white frames survived a sub-threshold scale")
        # sub-5px boxes fall to the dotted band (dt+3), not gray
        # outline (dt+1); some gray-tone band must carry them
        dots_li = s.m.ly.layer(db.LayerInfo(*s.m.FRAME_DOTS))
        dit = s.m.top.begin_shapes_rec(dots_li)
        chk(not dit.at_end(),
            "L8 gray frames missing at a sub-threshold scale")
    finally:
        s.stop()

    # ---- L5: dropped FIRST response keeps the names table
    s = Sess(cache)
    try:
        vA = (bx0, by0, bx0 + w // 3, by0 + h // 3)
        _dropped = s.request(vA)  # noqa: F841 - never applied
        chk(len(s.m.names) > 0, "L5 names lost on dropped first")
        s.round(vA)  # rollback + resend
        xor_view("L5", sregs, s.m, vA)
        page = next(iter(s.m.cells), None)
        chk(page is not None
            and s.m.design.get(page) is not None,
            "L5 design name unresolved for %s" % page)
    finally:
        s.stop()

    # ---- L4: eviction churn under a zero budget
    s = Sess(cache, budget_mb=0)
    try:
        vA = (bx0, by0, bx0 + w // 3, by0 + h // 3)
        vB = (bx1 - w // 3, by1 - h // 3, bx1, by1)
        evicted = 0
        for view in (vA, vB, vA):
            r = s.request(view)
            evicted += len(r["evict"])
            s.apply(r)
            xor_view("L4", sregs, s.m, view)
            check_ledger("L4", s.m)
        chk(evicted > 0, "L4 zero budget never evicted")
    finally:
        s.stop()

    # ---- L6: budgeted streaming (progressive first paint)
    view = (bx0, by0, bx0 + w // 2, by0 + h // 2)
    # cold reference MUST disable streaming outright (stream_kb=0),
    # not lean on the daemon default fitting the fixture (review)
    s = Sess(cache, stream_kb=0)
    try:
        r = s.request(view)
        cold_new = int(r["new"])
        chk(r.get("partial", "0") != "1", "L6 unbudgeted partial")
    finally:
        s.stop()
    s = Sess(cache, stream_kb=4)
    try:
        # drop the FIRST partial round: rollback must re-send the
        # same chunk (deterministic priority)
        r1 = s.request(view)
        first_new = r1["new"]
        chk(r1.get("partial") == "1", "L6 4KB budget not partial")
        r2 = s.request(view)  # ack still 0 -> r1 rolled back
        chk(r2["new"] == first_new,
            "L6 resend %s != %s" % (r2["new"], first_new))
        rounds, new_total = 0, 0
        r = r2
        while True:
            rounds += 1
            chk(rounds < 500, "L6 no convergence")
            s.apply(r)
            new_total += int(r["new"])
            if rounds == 1:
                # a partial delta must not mint deferred ghost
                # cells: exactly the sent pages exist, all filled
                pcells = [c.name for c in s.m.ly.each_cell()
                          if c.name.startswith("P")
                          and c.name[1].isdigit()]
                chk(len(pcells) == int(r["new"]),
                    "L6 round1 P cells %d != new %s"
                    % (len(pcells), r["new"]))
                check_ledger("L6 round1", s.m)
            if r.get("partial") != "1":
                break
            r = s.request(view)
        chk(rounds >= 2, "L6 single round despite budget")
        chk(new_total == cold_new,
            "L6 streamed %d != cold %d" % (new_total, cold_new))
        xor_view("L6 final", sregs, s.m, view)
        check_ledger("L6", s.m)
        # wander away MID-refinement, come back: no ghosts at any
        # stop, both views XOR clean when completed
        vB = (bx1 - w // 3, by1 - h // 3, bx1, by1)
        rp = s.request(view)  # partial round on A (fresh pages? may
        s.apply(rp)           # be complete now; either way apply)
        r = s.request(vB)
        while True:
            s.apply(r)
            check_ledger("L6 wander B", s.m)
            if r.get("partial") != "1":
                break
            r = s.request(vB)
        xor_view("L6 wander B", sregs, s.m, vB)
        r = s.request(view)
        while True:
            s.apply(r)
            if r.get("partial") != "1":
                break
            r = s.request(view)
        xor_view("L6 wander back", sregs, s.m, view)
        check_ledger("L6 wander back", s.m)
    finally:
        s.stop()
    # ---- L7: LOD variant cycle (M7-B). Wide + tiny px scale
    # fires the density gate (delta ships "...q" coverage cells);
    # the SAME view at a fine px scale must come back exact and
    # XOR clean - the in-place variant upgrade is the transition
    # the session must survive.
    s = Sess(cache)
    try:
        full = (bx0, by0, bx1, by1)
        r = s.request(full, px=0.01, lod=True)
        chk(int(r.get("lod", 0) or 0) >= 1,
            "L7 density gate never fired (lod=%s)" % r.get("lod"))
        s.apply(r)
        qres = [nm for nm in s.m.cells if nm.endswith("q")]
        chk(len(qres) >= 1, "L7 no lod page cells resident")
        check_ledger("L7 lod", s.m)
        r_off = s.request(full, px=0.01, lod=False)
        chk(int(r_off.get("lod", 0) or 0) == 0,
            "L7 per-request lod=0 still lod=%s" % r_off.get("lod"))
        s.apply(r_off)
        xor_view("L7 request-off", sregs, s.m, full)
        check_ledger("L7 request-off", s.m)
        r_on = s.request(full, px=0.01, lod=True)
        chk(int(r_on.get("lod", 0) or 0) >= 1,
            "L7 per-request lod=1 did not restore LOD")
        s.apply(r_on)
        check_ledger("L7 request-on", s.m)
        r2 = s.request(full, px=100.0)
        chk(int(r2.get("lod", 0) or 0) == 0,
            "L7 fine-px round still lod=%s" % r2.get("lod"))
        s.apply(r2)
        xor_view("L7 exact-after-lod", sregs, s.m, full)
        check_ledger("L7 exact", s.m)
    finally:
        s.stop()

    sly._destroy()

    # ---- L9 (rev 46b): the baked meta frontier IS the planner's
    # fit-view frame set - replaying its canonical parameters
    # through vfsd mode=frontier must reproduce it box for box.
    # (The view is rounded OUTWARD in um: plan seeds clip to the
    # top rbbox, so any superset view yields the identical plan.)
    fr = cache.meta.get("frontier") or {}
    chk(bool(fr.get("depths")), "L9 meta has no baked frontier")
    chk("px_per_um" in fr and "cut_px" in fr,
        "L9 baked frontier lacks canonical parameters")
    if fr.get("depths") and "px_per_um" in fr:
        s = Sess(cache, stream_kb=0)
        try:
            mb = cache.meta["bbox"]
            mdbu = cache.meta["dbu"]
            vw = (math.floor(mb[0] * mdbu) - 1,
                  math.floor(mb[1] * mdbu) - 1,
                  math.ceil(mb[2] * mdbu) + 1,
                  math.ceil(mb[3] * mdbu) + 1)
            for d, want in enumerate(fr["depths"]):
                line = ("gen=%d mode=frontier view=%s,%s,%s,%s "
                        "px=%r cut=%r depth=%d layers=all out=%s\n"
                        % (900 + d, vw[0], vw[1], vw[2], vw[3],
                           fr["px_per_um"], fr["cut_px"], d,
                           s.client.tmp))
                s.client.proc.stdin.write(line)
                s.client.proc.stdin.flush()
                resp = s.client.proc.stdout.readline()
                toks = dict(t.split("=", 1)
                            for t in resp.split() if "=" in t)
                chk("frontier" in toks,
                    "L9 depth %d: no frontier reply (%s)"
                    % (d, resp.strip()))
                if "frontier" not in toks:
                    continue
                rows = sorted(tuple(map(int, ln.split()))
                              for ln in open(toks["frontier"]))
                os.unlink(toks["frontier"])
                chk(rows == sorted(tuple(r) for r in want),
                    "L9 depth %d: bake != mode=frontier replay "
                    "(%d vs %d boxes)"
                    % (d, len(want), len(rows)))
        finally:
            s.stop()

    print("vfs-lifecycle-checked L1-L9, failures: %d" % len(bad))
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
