"""M3 gate: hier viewer lifecycle against a live daemon
(rust/VFS_HIER.md par.3.1/3.7) - the python VfsMosaic drives real
vfsd sessions exactly like the render service does.

  L1  10-gen pan loop: per-gen view-clipped XOR vs source == 0,
      previous gens' WC cells gone (only the current gen's remain),
      page registry consistent (every entry resolves by name)
  L2  stale drop: a response that is never applied (ack withheld) is
      rolled back; the next gen re-sends its pages - XOR green, no
      permanent blanks (the flat path's latent bug)
  L3  partial-apply fault: apply raises mid-way -> reset_all() +
      reset=1 replay on a fresh gen -> XOR green
  L4  eviction (budget-mb 0): pages evicted between gens and
      re-fetched on return - XOR green, no dangling registry entries

usage: python tools/validate_vfs_lifecycle.py <src.oas> <floe_dir>
"""
import functools
import json
import os
import sys

import klayout.db as db

sys.path.insert(0, os.path.dirname(os.path.dirname(
    os.path.abspath(__file__))))
from floe.vfsclient import VfsClient           # noqa: E402
from floe.viewport import VfsMosaic            # noqa: E402
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


class Sess:
    """the render service's hier round, distilled"""

    def __init__(self, cache, budget_mb=1024):
        self.client = VfsClient(cache.dir, budget_mb=budget_mb)
        self.m = VfsMosaic(cache)
        self.dbu = cache.meta["dbu"]

    def request(self, view, reset=False):
        self.m.req_gen += 1
        x0, y0, x1, y1 = view
        return self.client.request(
            self.m.req_gen,
            (x0 * self.dbu, y0 * self.dbu,
             x1 * self.dbu, y1 * self.dbu),
            1.0, 0.0, None, None, hier=True,
            ack=0 if reset else self.m.applied_gen, reset=reset)

    def apply(self, r):
        if r["names"]:
            names_path = r["names"]
            self.m.load_names(names_path)
            chk(not os.path.exists(names_path),
                "names file not deleted after load")
        return self.m.apply_hier(r["delta"], r["top"], r["evict"],
                                 gen=self.m.req_gen)

    def round(self, view, reset=False):
        self.apply(self.request(view, reset))

    def stop(self):
        self.client.stop()


def main():
    src, floe_dir = sys.argv[1], sys.argv[2]
    cache = cm.Cache(src)
    cache.dir = floe_dir
    cache.meta = json.load(open(os.path.join(floe_dir,
                                             "meta.json")))
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

        # ---- L3: partial-apply fault -> reset recovery
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
    sly._destroy()

    print("vfs-lifecycle-checked L1-L4, failures: %d" % len(bad))
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
