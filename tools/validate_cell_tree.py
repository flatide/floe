#!/usr/bin/env python3
"""Cell tree gate (docs/SPEC-VIEWER.ko.md §8c, SPEC-FORMATS "design.ovh").

The viewer's cell tree reads the hierarchy summary design.ovh (children
with placed member counts, parents, instances under the top) and asks
renderd's hier thread for a cell's children, cells by name, a cell's
extent and its instances inside a view. This gate holds every one of
those against KLayout's reading of the same layout - a hierarchy fixture
with a rotated, a mirrored and a plain placement of a block, a grid
inside it, a cell placed both directly under the top and deep inside
the block and an empty cell (an unplaced cell would be a second top,
which the indexer refuses; the summary's unit tests cover it) - and
pins the CLI contract:

  C1  `floe2 index` writes design.ovh; `floe-index hier --check` says
      identity=ok; a file of another cache is refused (rc 1)
  C2  children: every distinct child of every cell, with its member
      count = the sum of KLayout's instance array sizes; leaves say so
  C3  instances under the top: every cell's count = KLayout top-down
      product sum (the top 1)
  C4  cell_find: substring and * ? globs, case-insensitively, sorted by
      name, with the total beside a limited row list
  C5  cell_bbox: a cell the top places directly has the exact union of
      its instances' boxes; a cell reached through a block is located
      by the blocks' extent (approx=1, a superset), a shapeless cell too
  C6  cell_insts: the boxes inside a view equal KLayout's expanded
      instance walk (rotation, mirror, grid), every member of the
      irregular repetition included; a cap reports more=1
  C7  design.ovh removed: a small cache is summarized in memory (no
      file written); with FLOE_RUST_HIER_INLINE_PLACES=0 the daemon
      answers code=nohier and, once `floe2 index --hier-only` has
      written the file, the SAME daemon answers (live pickup)
  C8  the view root (root=): a frame rooted at BLK equals byte for byte
      the frame of a layout whose top is BLK (KLayout copy_tree) over
      three views; cell_bbox / cell_insts under the root count and walk
      from BLK; a root outside the table is refused; a root that holds
      none of the visible layers is an empty picture, not an error, and
      the density stack under a root without its top plane's layer draws
      what the frame without the stack draws
  C9  a layer the file names and no cell holds, alone on, is an empty
      picture at full depth and depth 0, frames on and off, with the
      density stack too - not `invalid plan: top is missing`
  C10 the index keeps such layers (stored_shapes 0, a text alone
      counted); the viewer lists the layers something is on, as Calibre
      does (FLOE_EMPTY_LAYERS=show: every layer)

usage: python tools/validate_cell_tree.py
"""
import fnmatch
import os
import queue
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

import klayout.db as db

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
os.environ["FLOE_RENDERER"] = "rust"
BIN = ROOT / "rust" / "target" / "release" / "floe-index"
RENDERD = ROOT / "rust" / "target" / "release" / "floe-renderd"
TMP = Path(tempfile.mkdtemp(prefix="floe-celltree-"))


def run_env(**extra):
    env = dict(os.environ)
    env["FLOE_INDEX_BIN"] = str(BIN)
    env["FLOE_RENDERD_BIN"] = str(RENDERD)
    env.update(extra)
    return env


def floe2(*args, ok=0, env=None):
    res = subprocess.run([sys.executable, "-B", "-m", "floe2", *map(str, args)],
                         capture_output=True, text=True, cwd=str(ROOT),
                         env=env or run_env())
    if ok is not None and res.returncode != ok:
        raise AssertionError("floe2 %s -> rc %d\n%s\n%s" % (
            " ".join(map(str, args)), res.returncode, res.stdout, res.stderr))
    return res


def floe_index(*args, ok=0):
    res = subprocess.run([str(BIN), *map(str, args)], capture_output=True,
                         text=True, cwd=str(ROOT))
    if ok is not None and res.returncode != ok:
        raise AssertionError("floe-index %s -> rc %d\n%s\n%s" % (
            " ".join(map(str, args)), res.returncode, res.stdout, res.stderr))
    return res


def write_fixture(path):
    """TOP holds BLK three times (plain, turned 90 degrees, mirrored),
    VIA three times directly (irregular offsets) and a big rectangle;
    BLK holds a 3 x 2 grid of INV and an EMPTY cell (no shapes); INV
    holds VIA twice."""
    ly = db.Layout(True)
    ly.dbu = 0.001
    l1 = ly.layer(db.LayerInfo(1, 0))
    l2 = ly.layer(db.LayerInfo(2, 0))
    via = ly.create_cell("VIA")
    via.shapes(l1).insert(db.Box(0, 0, 100, 100))
    inv = ly.create_cell("INV")
    inv.shapes(l2).insert(db.Box(0, 0, 1000, 600))
    inv.insert(db.CellInstArray(via.cell_index(), db.Trans(db.Vector(100, 100))))
    inv.insert(db.CellInstArray(via.cell_index(), db.Trans(db.Vector(700, 300))))
    empty = ly.create_cell("EMPTY")
    blk = ly.create_cell("BLK")
    blk.shapes(l1).insert(db.Box(0, 0, 5000, 4000))
    blk.insert(db.CellInstArray(inv.cell_index(), db.Trans(db.Vector(200, 200)),
                                db.Vector(1500, 0), db.Vector(0, 1200), 3, 2))
    blk.insert(db.CellInstArray(empty.cell_index(), db.Trans(db.Vector(10, 10))))
    top = ly.create_cell("TOP")
    top.shapes(l2).insert(db.Box(0, 0, 20000, 12000))
    top.insert(db.CellInstArray(blk.cell_index(), db.Trans(db.Vector(1000, 1000))))
    top.insert(db.CellInstArray(blk.cell_index(), db.Trans(1, False, db.Vector(12000, 2000))))
    top.insert(db.CellInstArray(blk.cell_index(), db.Trans(0, True, db.Vector(1000, 11000))))
    for off in ((15000, 9000), (15300, 9000), (15000, 9700)):
        top.insert(db.CellInstArray(via.cell_index(), db.Trans(db.Vector(*off))))
    ly.write(str(path))
    return ly


class Oracle:
    """KLayout's reading of the fixture."""

    def __init__(self, ly):
        self.ly = ly
        self.top = ly.top_cell()
        self.names = {c.cell_index(): c.name for c in ly.each_cell()}

    def cell(self, name):
        return self.ly.cell(name)

    def children(self, name):
        """{child name: placed members}"""
        out = {}
        for inst in self.cell(name).each_inst():
            child = self.ly.cell(inst.cell_index).name
            out[child] = out.get(child, 0) + inst.size()
        return out

    def insts(self):
        """{name: instances under the top}"""
        total = {ci: 0 for ci in self.names}
        total[self.top.cell_index()] = 1
        for ci in self.ly.each_cell_top_down():
            mine = total[ci]
            if not mine:
                continue
            for inst in self.ly.cell(ci).each_inst():
                total[inst.cell_index] += mine * inst.size()
        return {self.names[ci]: n for ci, n in total.items()}

    def boxes(self, name, root=None):
        """Every instance of `name` under `root` (the top) as a box in
        the root's dbu coordinates."""
        target = self.cell(name).cell_index()
        out = []

        def walk(cell, trans):
            for inst in cell.each_inst():
                child = self.ly.cell(inst.cell_index)
                for t in inst.cell_inst.each_trans():
                    ct = trans * t
                    if inst.cell_index == target:
                        b = child.bbox().transformed(ct)
                        out.append((b.left, b.bottom, b.right, b.top))
                    else:
                        walk(child, ct)
        walk(self.top if root is None else self.cell(root), db.Trans())
        return out

    def union(self, boxes):
        return (min(b[0] for b in boxes), min(b[1] for b in boxes),
                max(b[2] for b in boxes), max(b[3] for b in boxes))


class Daemon:
    """The viewer's render worker over the fixture's cache."""

    def __init__(self, src, env=None):
        from floe.cache import Cache
        from floe.service import make_render_worker
        saved = dict(os.environ)
        os.environ.update(env or run_env())
        try:
            self.cache = Cache(str(src))
            self.cache.load()
            self.worker = make_render_worker(self.cache)
            self.worker.start()
        finally:
            os.environ.clear()
            os.environ.update(saved)
        self.seq = 100

    def ask(self, kind, **job):
        self.seq += 1
        self.worker.submit(dict(job, kind=kind, seq=self.seq))
        deadline = time.monotonic() + 30.0
        while time.monotonic() < deadline:
            try:
                res = self.worker.res.get(timeout=0.5)
            except queue.Empty:
                continue
            if res.get("kind") == "error":
                raise AssertionError(res.get("msg"))
            if res.get("kind") == kind and res.get("seq") == self.seq:
                return res
        raise AssertionError("no %s answer" % kind)

    def render(self, bbox, w, h, root=None, gen=None, visible=None, raw=False, depth=None, frames=True):
        """The settled frame's pixel payload for a view (dbu), through
        the viewer's own job schema; `root` = the view root cell,
        `visible` = (layer, datatype) pairs (None = all), `raw` = RGBA,
        `depth` None = full."""
        self.seq += 1
        gen = gen or self.seq
        job = {
            "kind": "render", "gen": gen, "scope": "live",
            "bbox": tuple(float(v) for v in bbox), "view": None,
            "w": w, "h": h, "depth": depth, "cut_px": 3.0,
            "visible": visible, "frames": frames, "labels": False,
            "abstract": False, "root": root}
        if raw:
            job["frame_format"] = "raw"
        self.worker.submit(job)
        deadline = time.monotonic() + 60.0
        while time.monotonic() < deadline:
            try:
                res = self.worker.res.get(timeout=0.5)
            except queue.Empty:
                continue
            if res.get("kind") == "error":
                raise AssertionError(res.get("msg"))
            if res.get("kind") != "frame" or res.get("gen") != gen:
                continue
            if res.get("refining"):
                continue
            payload = res.get("rgba") or res.get("png")
            assert payload, res
            return bytes(payload)
        raise AssertionError("no settled frame")

    def stop(self):
        self.worker.stop()


def as_int_box(b):
    return tuple(int(round(v)) for v in b)


class CellTreeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.src = TMP / "hier_fixture.oas"
        cls.ly = write_fixture(cls.src)
        cls.oracle = Oracle(cls.ly)
        floe2("index", cls.src, "--jobs", "2")
        from floe.cachepath import vfs_cache_dir
        cls.cache_dir = Path(vfs_cache_dir(str(cls.src)))
        cls.daemon = Daemon(cls.src)

    @classmethod
    def tearDownClass(cls):
        cls.daemon.stop()

    def by_name(self, kind, **job):
        res = self.daemon.ask(kind, **job)
        self.assertTrue(res.get("found"), res)
        return res

    def ci_of(self, name):
        """The daemon's index of a cell, through the search."""
        res = self.by_name("cell_find", pattern=name)
        rows = [m for m in res["matches"] if m["name"] == name]
        self.assertEqual(len(rows), 1, res)
        return rows[0]["cell"]

    def test_c1_index_writes_the_summary_and_check_binds_it(self):
        ovh = self.cache_dir / "design.ovh"
        self.assertTrue(ovh.is_file(), ovh)
        res = floe_index("hier", self.cache_dir, "--check")
        self.assertIn("identity=ok", res.stdout)
        self.assertIn("cells=5", res.stdout)
        # a summary of another cache is refused
        other = TMP / "other.oas"
        ly = db.Layout(True)
        ly.dbu = 0.001
        c = ly.create_cell("ONLY")
        c.shapes(ly.layer(db.LayerInfo(1, 0))).insert(db.Box(0, 0, 10, 10))
        ly.write(str(other))
        floe2("index", other, "--jobs", "1")
        from floe.cachepath import vfs_cache_dir
        other_dir = Path(vfs_cache_dir(str(other)))
        keep = ovh.read_bytes()
        try:
            shutil.copyfile(other_dir / "design.ovh", ovh)
            res = floe_index("hier", self.cache_dir, "--check", ok=1)
            self.assertIn("identity=none", res.stdout)
            self.assertIn("identity_error=", res.stdout)
        finally:
            ovh.write_bytes(keep)
        res = floe_index("hier", self.cache_dir, "--check")
        self.assertIn("identity=ok", res.stdout)

    def test_c2_children_match_klayout_member_sums(self):
        sources = self.by_name("cell_sources")
        self.assertEqual(len(sources["sources"]), 1)
        self.assertEqual(sources["sources"][0]["placements"], 1)
        self.assertEqual(Path(sources["sources"][0]["path"]).resolve(),
                         self.cache_dir.resolve())
        top = self.by_name("cells")
        self.assertEqual(top["name"], "TOP")
        self.assertEqual(top["insts"], 1)
        self.assertEqual(top["height"], 3)
        self.assertEqual(as_int_box(top["bbox"]), (0, 0, 20000, 12000))
        seen = {}
        stack = [(None, "TOP")]
        while stack:
            ci, name = stack.pop()
            res = self.by_name("cells", cell=ci) if ci is not None else top
            self.assertEqual(res["name"], name)
            got = {c["name"]: c["members"] for c in res["children"]}
            self.assertEqual(got, self.oracle.children(name), name)
            self.assertEqual(res["total"], len(got))
            for c in res["children"]:
                self.assertEqual(c["leaf"],
                                 not self.oracle.children(c["name"]), c)
                if c["name"] not in seen:
                    seen[c["name"]] = c["cell"]
                    stack.append((c["cell"], c["name"]))
        self.assertEqual(set(seen), {"BLK", "VIA", "INV", "EMPTY"})
        self.assertEqual(self.oracle.children("TOP"), {"BLK": 3, "VIA": 3})
        self.assertEqual(self.oracle.children("BLK"), {"INV": 6, "EMPTY": 1})
        # sorted by name, case-insensitively
        self.assertEqual([c["name"] for c in top["children"]], ["BLK", "VIA"])

    def test_c3_instances_under_the_top_match_the_product_sum(self):
        expect = self.oracle.insts()
        self.assertEqual(expect, {"TOP": 1, "BLK": 3, "INV": 18, "VIA": 39,
                                  "EMPTY": 3})
        res = self.by_name("cell_find", pattern="")
        self.assertEqual(res["total"], 5)
        got = {m["name"]: m["insts"] for m in res["matches"]}
        for name, n in expect.items():
            self.assertEqual(got[name], n, name)

    def test_c4_find_is_substring_or_glob_case_insensitively(self):
        names = [c.name for c in self.ly.each_cell()]
        for pattern in ("v", "*V*", "?NV", "b*", "*", "nothere", "In"):
            res = self.by_name("cell_find", pattern=pattern)
            if any(ch in pattern for ch in "*?"):
                expect = [n for n in names
                          if fnmatch.fnmatchcase(n.lower(), pattern.lower())]
            else:
                expect = [n for n in names if pattern.lower() in n.lower()]
            got = [m["name"] for m in res["matches"]]
            self.assertEqual(sorted(got, key=str.lower), got, pattern)
            self.assertEqual(sorted(got), sorted(expect), pattern)
            self.assertEqual(res["total"], len(expect), pattern)
        limited = self.by_name("cell_find", pattern="", limit=2)
        self.assertEqual((limited["total"], len(limited["matches"])), (5, 2))

    def test_c5_extent_is_exact_for_direct_children_and_a_superset_deeper(self):
        blk = self.by_name("cell_bbox", cell=self.ci_of("BLK"))
        self.assertEqual(blk["insts"], 3)
        self.assertFalse(blk["approx"])
        self.assertEqual(as_int_box(blk["bbox"]),
                         self.oracle.union(self.oracle.boxes("BLK")))
        via = self.by_name("cell_bbox", cell=self.ci_of("VIA"))
        self.assertEqual(via["insts"], 39)
        self.assertTrue(via["approx"])
        u = self.oracle.union(self.oracle.boxes("VIA"))
        b = as_int_box(via["bbox"])
        self.assertTrue(b[0] <= u[0] and b[1] <= u[1]
                        and b[2] >= u[2] and b[3] >= u[3], (b, u))
        # the blocks' extent joins the direct placements' own
        blocks = self.oracle.union(self.oracle.boxes("BLK") +
                                   [x for x in self.oracle.boxes("VIA")
                                    if x[0] >= 15000])
        self.assertEqual(b, blocks)
        # a shapeless cell inside the blocks: located by the blocks too
        empty = self.by_name("cell_bbox", cell=self.ci_of("EMPTY"))
        self.assertEqual((empty["insts"], empty["approx"],
                          as_int_box(empty["bbox"])),
                         (3, True, self.oracle.union(self.oracle.boxes("BLK"))))
        top = self.by_name("cell_bbox", cell=self.ci_of("TOP"))
        self.assertEqual((top["insts"], as_int_box(top["bbox"]), top["approx"]),
                         (1, (0, 0, 20000, 12000), False))

    def test_c6_instances_in_a_view_match_klayouts_walk(self):
        for name in ("VIA", "INV", "BLK"):
            ci = self.ci_of(name)
            expect_all = self.oracle.boxes(name)
            for view in ((0, 0, 20000, 12000), (12000, 2000, 16000, 7000),
                         (1000, 7000, 6000, 11000), (15200, 9000, 15400, 9100),
                         (19000, 11000, 20000, 12000)):
                res = self.by_name("cell_insts", cell=ci, view=view)
                self.assertFalse(res["more"], res)
                got = sorted(as_int_box(b) for b in res["boxes"])
                expect = sorted(b for b in expect_all
                                if b[0] <= view[2] and b[2] >= view[0]
                                and b[1] <= view[3] and b[3] >= view[1])
                self.assertEqual(got, expect, (name, view))
        # the cap stops the walk and says so
        capped = self.by_name("cell_insts", cell=self.ci_of("VIA"),
                              view=(0, 0, 20000, 12000), cap=5)
        self.assertEqual(len(capped["boxes"]), 5)
        self.assertTrue(capped["more"])
        # the top is one box, its own
        top = self.by_name("cell_insts", cell=self.ci_of("TOP"),
                           view=(0, 0, 1, 1))
        self.assertEqual([as_int_box(b) for b in top["boxes"]],
                         [(0, 0, 20000, 12000)])
        # a shapeless cell has no boxes; a view off the layout finds none
        self.assertEqual(self.by_name("cell_insts", cell=self.ci_of("EMPTY"),
                                      view=(0, 0, 20000, 12000))["boxes"], [])
        self.assertEqual(self.by_name("cell_insts", cell=self.ci_of("VIA"),
                                      view=(30000, 30000, 31000, 31000))["boxes"], [])

    def test_c8_a_view_root_draws_the_cell_as_a_layout_of_its_own(self):
        """SPEC-VIEWER §8c: rendering the fixture with root=BLK equals,
        byte for byte, rendering a layout whose top IS BLK (KLayout's
        copy_tree) over the same BLK-coordinate view; the cell queries
        under the root count and walk from BLK; a jobdeck-free contract
        the daemon refuses nothing here."""
        blk = self.ci_of("BLK")
        # the standalone BLK layout
        other = TMP / "blk_top.oas"
        ly2 = db.Layout(True)
        ly2.dbu = self.ly.dbu
        top2 = ly2.create_cell("BLK")
        top2.copy_tree(self.ly.cell("BLK"))
        ly2.write(str(other))
        floe2("index", other, "--jobs", "1")
        alone = Daemon(other)
        try:
            for view in ((-200, -200, 5200, 4200), (100, 100, 1700, 1500),
                         (4000, 3000, 5000, 4000)):
                rooted = self.daemon.render(view, 400, 320, root=blk)
                plain = alone.render(view, 400, 320)
                self.assertEqual(len(rooted), len(plain), view)
                self.assertEqual(rooted, plain, "frame differs at %s" % (view,))
            # the same view at the top is another picture
            top_view = self.daemon.render((-200, -200, 5200, 4200), 400, 320)
            self.assertNotEqual(top_view, rooted)
        finally:
            alone.stop()
        # queries under the root: INV is BLK's direct child - exact extent,
        # 6 instances; VIA's instances from BLK equal KLayout's walk from BLK
        inv = self.by_name("cell_bbox", cell=self.ci_of("INV"), root=blk)
        self.assertEqual((inv["insts"], inv["approx"]), (6, False))
        self.assertEqual(as_int_box(inv["bbox"]),
                         self.oracle.union(self.oracle.boxes("INV", "BLK")))
        via = self.by_name("cell_insts", cell=self.ci_of("VIA"), root=blk,
                           view=(0, 0, 5000, 4000))
        self.assertFalse(via["more"])
        self.assertEqual(sorted(as_int_box(b) for b in via["boxes"]),
                         sorted(self.oracle.boxes("VIA", "BLK")))
        self.assertEqual(len(via["boxes"]), 12)
        # a cell above the root is not under it; the root is itself
        top = self.by_name("cell_bbox", cell=self.ci_of("TOP"), root=blk)
        self.assertEqual((top["insts"], top["bbox"]), (0, None))
        me = self.by_name("cell_bbox", cell=blk, root=blk)
        self.assertEqual((me["insts"], as_int_box(me["bbox"])),
                         (1, (0, 0, 5000, 4000)))
        # a root outside the table is refused, not planned as the top
        bad = self.daemon.ask("cell_bbox", cell=blk, root=999)
        self.assertFalse(bad["found"])
        self.assertEqual(bad["code"], "query")

    def test_c8_a_view_root_without_the_visible_layers_is_an_empty_picture(self):
        """VIA holds layer 1/0 only. The file's top holds every layer, so
        its plan always has a working cell; a root need not (2026-09-30:
        `invalid plan: top is missing` instead of a frame). With only 2/0
        on, VIA rooted is a black frame at full depth, frames on or off;
        with 1/0 it draws. The density stack plans its top plane's layer
        (2/0) alone - under VIA that plan is empty - and the frame equals
        the one without the stack."""
        via = self.ci_of("VIA")
        view = (-20, -20, 120, 120)
        blank = self.daemon.render(view, 200, 160, root=via, visible=[(2, 0)], raw=True)
        self.assertEqual(len(blank), 200 * 160 * 4)
        self.assertEqual(set(blank[i:i + 4] for i in range(0, len(blank), 4)), {bytes((0, 0, 0, 255))})
        lit = self.daemon.render(view, 200, 160, root=via, visible=[(1, 0)], raw=True)
        self.assertNotEqual(lit, blank)
        plain = self.daemon.render(view, 200, 160, root=via, raw=True)
        stacked = Daemon(self.src, env=run_env(FLOE_RUST_DENSITY_STACK="top"))
        try:
            self.assertEqual(stacked.render(view, 200, 160, root=via, raw=True), plain)
            self.assertEqual(stacked.render(view, 200, 160, root=via, visible=[(2, 0)], raw=True), blank)
        finally:
            stacked.stop()

    def test_c9_a_named_layer_no_cell_holds_is_an_empty_picture(self):
        """3/0 is named in the file (NOTHING) and holds no shape - as the
        routing chip's BOUNDARY 100/0. The file's top does not hold it, so
        its plan had no working cell and the frame failed (user 2026-10-04:
        `invalid plan: top is missing` - at full depth, and at depth 0 with
        the frames off). Alone on it is a black frame at full depth, frames
        on and off, and at depth 0 with the frames off; at depth 0 with them
        on, the outlines of the cells past the depth alone - those of the
        frame with 1/0 on, whatever the layers. The density stack's dots
        alike; with 1/0 on the frame draws."""
        src = TMP / "named_empty.oas"
        ly = db.Layout(True)
        ly.dbu = 0.001
        l1 = ly.layer(db.LayerInfo(1, 0))
        ly.layer(db.LayerInfo(3, 0, "NOTHING"))
        leaf = ly.create_cell("LEAF")
        leaf.shapes(l1).insert(db.Box(0, 0, 400, 300))
        top = ly.create_cell("TOP")
        top.shapes(l1).insert(db.Box(0, 0, 5000, 100))
        top.insert(db.CellInstArray(leaf.cell_index(), db.Trans(db.Vector(1000, 1000)), db.Vector(800, 0), db.Vector(0, 600), 4, 3))
        options = db.SaveLayoutOptions()
        options.format = "OASIS"
        ly.write(str(src), options)
        floe2("index", src)
        view = (-100, -100, 5100, 3100)
        black = {bytes((0, 0, 0, 255))}
        for env in (run_env(), run_env(FLOE_RUST_DENSITY_STACK="top", FLOE_RUST_DENSITY_DOTS="on")):
            daemon = Daemon(src, env=env)
            try:
                self.assertIn((3, 0), [(l["layer"], l["datatype"]) for l in daemon.cache.meta["layers"]])
                for depth, frames in ((None, True), (None, False), (0, False)):
                    blank = daemon.render(view, 260, 160, visible=[(3, 0)], raw=True, depth=depth, frames=frames)
                    self.assertEqual(len(blank), 260 * 160 * 4)
                    self.assertEqual(set(blank[i:i + 4] for i in range(0, len(blank), 4)), black, (depth, frames))
                # depth 0, frames on: the outlines of LEAF past the depth, as
                # with 1/0 on (its own shapes aside)
                outlined = daemon.render(view, 260, 160, visible=[(3, 0)], raw=True, depth=0, frames=True)
                drawn = daemon.render(view, 260, 160, visible=[(1, 0)], raw=True, depth=0, frames=True)
                bare = daemon.render(view, 260, 160, visible=[(1, 0)], raw=True, depth=0, frames=False)
                outline = {i for i in range(0, len(drawn), 4) if drawn[i:i + 4] != bare[i:i + 4]}
                self.assertTrue(outline)
                self.assertEqual({i for i in range(0, len(outlined), 4) if outlined[i:i + 4] not in black}, outline)
                lit = daemon.render(view, 260, 160, visible=[(1, 0)], raw=True)
                self.assertNotEqual(set(lit[i:i + 4] for i in range(0, len(lit), 4)), black)
            finally:
                daemon.stop()

    def test_c10_the_viewer_lists_the_layers_something_is_on(self):
        """The field's EBEAM file (user 2026-10-08): Calibre listed 3.0 and
        3.300 where floe listed 3.1 and 3.2 too, nothing drawn on them -
        pairs the file's LAYERNAME table names and no shape uses (KLayout
        lists them; so does the index). The index keeps every pair, the
        named ones with stored_shapes 0, a text alone counted; the viewer
        lists the pairs that hold something (a text is drawn as a label),
        FLOE_EMPTY_LAYERS=show every pair."""
        from floe import cache as cache_mod, gui
        src = TMP / "named_list.oas"
        ly = db.Layout(True)
        ly.dbu = 0.001
        main = ly.layer(db.LayerInfo(3, 0, "MAIN"))
        for d in (1, 2):
            ly.layer(db.LayerInfo(3, d, "NAMED%d" % d))
        frame = ly.layer(db.LayerInfo(3, 300))
        label = ly.layer(db.LayerInfo(5, 0))
        leaf = ly.create_cell("LEAF")
        leaf.shapes(main).insert(db.Box(0, 0, 400, 300))
        leaf.shapes(label).insert(db.Text("L", db.Trans(db.Vector(10, 10))))
        top = ly.create_cell("TOP")
        top.shapes(frame).insert(db.Box(0, 0, 5000, 100))
        top.insert(db.CellInstArray(leaf.cell_index(), db.Trans(db.Vector(1000, 1000)), db.Vector(800, 0), db.Vector(0, 600), 4, 3))
        options = db.SaveLayoutOptions()
        options.format = "OASIS"
        ly.write(str(src), options)
        floe2("index", src)
        cache = cache_mod.Cache(str(src))
        cache.load()
        stored = {(l["layer"], l["datatype"]): l["stored_shapes"] for l in cache.meta["layers"]}
        self.assertEqual(stored, {(3, 0): 1, (3, 1): 0, (3, 2): 0, (3, 300): 1, (5, 0): 1})
        keys = lambda meta: sorted((l["layer"], l["datatype"]) for l in meta["layers"])
        saved = os.environ.pop("FLOE_EMPTY_LAYERS", None)
        try:
            self.assertEqual(keys(gui.listed_meta(cache)), [(3, 0), (3, 300), (5, 0)])
            os.environ["FLOE_EMPTY_LAYERS"] = "show"
            self.assertEqual(keys(gui.listed_meta(cache)), sorted(stored))
        finally:
            os.environ.pop("FLOE_EMPTY_LAYERS", None)
            if saved is not None:
                os.environ["FLOE_EMPTY_LAYERS"] = saved

    def test_c7_missing_summary_inline_or_refused_then_picked_up_live(self):
        ovh = self.cache_dir / "design.ovh"
        keep = ovh.read_bytes()
        ovh.unlink()
        try:
            # a small cache: summarized in memory, no file written
            small = Daemon(self.src)
            try:
                res = small.ask("cells")
                self.assertTrue(res["found"], res)
                self.assertEqual({c["name"]: c["members"] for c in res["children"]},
                                 {"BLK": 3, "VIA": 3})
            finally:
                small.stop()
            self.assertFalse(ovh.exists())
            # the inline path closed: the daemon says which index to build,
            # then finds the file the moment it is there
            strict = Daemon(self.src, env=run_env(FLOE_RUST_HIER_INLINE_PLACES="0"))
            try:
                res = strict.ask("cells")
                self.assertFalse(res["found"], res)
                self.assertEqual(res["code"], "nohier")
                self.assertIn("floe-index hier", res["err"])
                res = strict.ask("cell_find", pattern="via")
                self.assertEqual((res["found"], res["code"]), (False, "nohier"))
                floe2("index", self.src, "--hier-only")
                self.assertTrue(ovh.is_file())
                res = strict.ask("cells")
                self.assertTrue(res["found"], res)
                self.assertEqual(res["total"], 2)
            finally:
                strict.stop()
            # --hier-only refuses a source without a current cache
            floe2("index", TMP / "nothing.oas", "--hier-only", ok=None)
        finally:
            ovh.write_bytes(keep)


def main():
    for binary in (BIN, RENDERD):
        if not binary.is_file():
            print("missing %s: cd rust && cargo build --release" % binary)
            return 2
    try:
        result = unittest.main(argv=[sys.argv[0]] + sys.argv[1:], exit=False,
                               verbosity=2).result
    finally:
        shutil.rmtree(TMP, ignore_errors=True)
    if not result.wasSuccessful():
        return 1
    print("CELL TREE: ALL OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
