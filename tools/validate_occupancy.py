#!/usr/bin/env python3
"""Occupancy pyramid gate (docs/OCCUPANCY_PLAN.ko.md M1): gate 1 and
the generation half of gate 5.

Gate 1 - generation against an INDEPENDENT shape-intersection oracle:
for every level-0 cell of every layer, KLayout answers "does the open
cell box meet the layer's geometry with positive area" through the
recursive touching-shape iterator (arrays expanded, paths hulled by
KLayout) and `Region & Region(box)`; the design.ovo bit must equal it
on every cell. Fixtures: an L with an empty corner, a ring with a
hole (written as a cut polygon), diagonal band and triangle, paths
with extensions and a diagonal path, boxes on and off grid lines,
zero-area shapes, an empty layer, rotated/mirrored/nested placements,
axis arrays with gaps narrower and wider than a cell, a diagonal-
vector array (the review's counterexample), two far-apart clusters,
and the valmini asset at a coarse cell. Upper levels must be the OR
pool of the level below. A bbox oracle would pass a builder that
fills empty space; this one does not.

Gate 5 (generation part) - `floe-index occupancy` listing and dump,
identity mismatch and truncation refused, --occupancy-only leaves the
cache's other files byte-identical and honours --occupancy-um,
limits recorded as none:<reason>, --kill-at occupancy-tmp keeps the
previous file, the wrapper discards a leftover tmp, --occupancy-um
implies --occupancy, and the jobdeck wrapper forwards the options
(index with summary / occupancy-only on indexed sources / nothing to
do when the summary exists).

usage: python tools/validate_occupancy.py [src.oas]
"""
import hashlib
import os
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import klayout.db as db

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from floe.cachepath import vfs_cache_dir  # noqa: E402
BIN = ROOT / "rust" / "target" / "release" / "floe-index"
TMP = Path(tempfile.mkdtemp(prefix="floe-occ-"))
UM = 1000  # dbu per micron in the fixtures (dbu 0.001)

DECK = """SLICE 1,17
RETICLE
* occ.jb
OPTION PA, AA=0.0200, BA=0.002000, SA=80
MTITLE 1,METAL1
MTITLE 2,VIA1
*PLACE-INFO
*
CHIP ID001, * MAIN 1.0000
*
$ (1, METAL1, AD=0.00020, SF=1, TC=chipA.oas, LY={1}, DT={0}, BX=0.0, BY=0.0, UX=30.0, UY=30.0 )
$ (2, VIA1, AD=0.00020, SF=1, TC=chipB.oas, LY={1}, DT={0}, BX=0.0, BY=0.0, UX=20.0, UY=20.0 )
ROWS 100.0/100.0
*END-PLACE
END
"""


def run_env():
    e = os.environ.copy()
    e.update({"PYTHONDONTWRITEBYTECODE": "1", "PYTHONPATH": str(ROOT),
              "FLOE_INDEX_BIN": str(BIN)})
    return e


def floe_index(*args, ok=0, env=None):
    res = subprocess.run([str(BIN), *map(str, args)], capture_output=True,
                         text=True, env=env or run_env())
    if ok is not None and res.returncode != ok:
        raise AssertionError("floe-index %s: exit %d, wanted %d\n%s\n%s"
                             % (args, res.returncode, ok, res.stdout,
                                res.stderr))
    return res


def floe2(*args, ok=0, env=None):
    res = subprocess.run([sys.executable, "-B", "-m", "floe2",
                          *map(str, args)], cwd=ROOT, env=env or run_env(),
                         capture_output=True, text=True)
    if ok is not None and res.returncode != ok:
        raise AssertionError("floe2 %s: exit %d, wanted %d\n%s\n%s"
                             % (args, res.returncode, ok, res.stdout,
                                res.stderr))
    return res


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


# ------------------------------------------------------------ design.ovo

HDR = "<8sIdQQq4qII"     # magic version unit size mtime cell bbox levels layers
LAYER = "<IIBQB"          # v2: layer dt status work n_planes
LAYER_V1 = "<IIBQ"        # v1: layer dt status work (one flattened plane)
LEVEL = "<IIQQ"           # w h off len
DEPTH_ALL = "all"         # a version-1 plane: every depth flattened
STATUS = {0: "ok", 1: "none:cells", 2: "none:work", 3: "none:size",
          4: "none:unsupported", 5: "empty"}


def or_levels(level_lists, nlv):
    """the OR of several planes' levels; no plane = the zero entries a
    none/empty layer carries"""
    if not level_lists:
        return [(0, 0, b"")] * nlv
    out = []
    for lv in range(nlv):
        w, h, b = level_lists[0][lv]
        acc = bytearray(b)
        for other in level_lists[1:]:
            ow, oh, ob = other[lv]
            assert (ow, oh) == (w, h), ((ow, oh), (w, h))
            for i, v in enumerate(ob):
                acc[i] |= v
        out.append((w, h, bytes(acc)))
    return out


def levels_at_depth(layer, depth, nlv):
    """the levels a request depth draws: the OR of the planes at or
    above it (a version-1 `all` plane only by the unlimited depth)"""
    drawn = [p["levels"] for p in layer["planes"]
             if depth is None or (p["depth"] != DEPTH_ALL
                                  and p["depth"] <= depth)]
    return or_levels(drawn, nlv)


def read_ovo(path):
    """The file as dicts: per layer its `planes` (placement depth +
    levels; version 2, 2026-09-16) and the flattened `levels` (the OR
    of every plane - the version-1 view). A version-1 file reads as
    one plane of depth `all`."""
    d = Path(path).read_bytes()
    (magic, ver, unit, size, mtime, cell, x0, y0, x1, y1, nlv,
     nl) = struct.unpack_from(HDR, d, 0)
    assert (magic, ver) in ((b"FLOEOVO2", 2), (b"FLOEOVO1", 1)), (magic, ver)
    v1 = ver == 1
    o = struct.calcsize(HDR)
    top_len = struct.unpack_from("<H", d, o)[0]
    o += 2
    top = d[o:o + top_len].decode("utf-8")
    o += top_len
    layers = []
    for _ in range(nl):
        if v1:
            layer, dt, status, work = struct.unpack_from(LAYER_V1, d, o)
            o += struct.calcsize(LAYER_V1)
            n_planes = 1
        else:
            layer, dt, status, work, n_planes = struct.unpack_from(LAYER, d, o)
            o += struct.calcsize(LAYER)
        planes = []
        for _ in range(n_planes):
            if v1:
                depth = DEPTH_ALL
            else:
                depth = d[o]
                o += 1
            levels = []
            for _ in range(nlv):
                w, h, off, ln = struct.unpack_from(LEVEL, d, o)
                o += struct.calcsize(LEVEL)
                levels.append((w, h, d[off:off + ln]))
            planes.append(dict(depth=depth, levels=levels))
        if status != 0:
            planes = []   # a version-1 none/empty layer carries zero entries
        layers.append(dict(key=(layer, dt), status=STATUS.get(status),
                           work=work, planes=planes,
                           levels=or_levels([p["levels"] for p in planes],
                                            nlv)))
    return dict(version=ver, unit=unit, src_size=size, src_mtime=mtime,
                cell=cell, bbox=(x0, y0, x1, y1), n_levels=nlv, top=top,
                layers=layers)


def bit(level, i, j):
    w, h, b = level
    if i >= w or j >= h:
        return False
    rb = (w + 7) // 8
    return (b[j * rb + i // 8] >> (i % 8)) & 1 == 1


def pool(level):
    w, h, b = level
    w2, h2 = (w + 1) // 2, (h + 1) // 2
    rb2 = (w2 + 7) // 8
    out = bytearray(rb2 * h2)
    for j in range(h2):
        for i in range(w2):
            if (bit(level, 2 * i, 2 * j) or bit(level, 2 * i + 1, 2 * j)
                    or bit(level, 2 * i, 2 * j + 1)
                    or bit(level, 2 * i + 1, 2 * j + 1)):
                out[j * rb2 + i // 8] |= 1 << (i % 8)
    return (w2, h2, bytes(out))


def oracle_level0(src, key, ovo, depth=None):
    """set of (i, j) whose open cell box meets the layer with area > 0;
    `depth` restricts the shapes to exactly that placement depth (the
    plane of that depth), None takes every depth (the flattening)"""
    ly = db.Layout()
    ly.read(str(src))
    top = ly.top_cell()
    li = ly.find_layer(key[0], key[1])
    x0, y0, x1, y1 = ovo["bbox"]
    c = ovo["cell"]
    w = -(-(x1 - x0) // c)
    h = -(-(y1 - y0) // c)
    lit = set()
    if li is not None:
        for j in range(h):
            for i in range(w):
                box = db.Box(x0 + i * c, y0 + j * c, x0 + (i + 1) * c,
                             y0 + (j + 1) * c)
                it = ly.begin_shapes_touching(top, li, box)
                if depth is not None:
                    it.min_depth = depth
                    it.max_depth = depth
                reg = db.Region(it)
                if reg.is_empty():
                    continue
                if (reg & db.Region(box)).area() > 0:
                    lit.add((i, j))
    ly._destroy()
    return w, h, lit


# -------------------------------------------------------------- fixtures

def P(x, y):
    return db.Point(x, y)


def write_shapes(path):
    ly = db.Layout()
    ly.dbu = 0.001
    top = ly.create_cell("SHAPES")
    L = {n: ly.layer(n, 0) for n in range(1, 7)}
    # L shape with a 21 x 22 um empty corner
    top.shapes(L[1]).insert(db.Polygon([
        P(0, 0), P(30 * UM, 0), P(30 * UM, 8 * UM), P(9 * UM, 8 * UM),
        P(9 * UM, 30 * UM), P(0, 30 * UM)]))
    # ring with a hole: KLayout writes the hole as a cut polygon
    ring = db.Polygon(db.Box(2 * UM, 2 * UM, 28 * UM, 28 * UM))
    ring.insert_hole(db.Box(6 * UM, 6 * UM, 24 * UM, 24 * UM))
    top.shapes(L[2]).insert(ring)
    # diagonal band and a triangle whose vertices sit on grid lines
    top.shapes(L[3]).insert(db.Polygon([
        P(0, 0), P(3 * UM, 0), P(30 * UM, 27 * UM), P(30 * UM, 30 * UM),
        P(27 * UM, 30 * UM), P(0, 3 * UM)]))
    top.shapes(L[3]).insert(db.Polygon([P(20 * UM, 2 * UM), P(28 * UM, 2 * UM),
                                        P(20 * UM, 10 * UM)]))
    # manhattan path with extensions, a diagonal path, a U route
    top.shapes(L[4]).insert(db.Path([P(2 * UM, 3 * UM), P(20 * UM, 3 * UM),
                                     P(20 * UM, 25 * UM)], 1500, 500, 300))
    top.shapes(L[4]).insert(db.Path([P(3 * UM, 27 * UM), P(27 * UM, 5 * UM)],
                                    700))
    top.shapes(L[4]).insert(db.Path([P(24 * UM, 27 * UM), P(28 * UM, 27 * UM),
                                     P(28 * UM, 22 * UM), P(24 * UM, 22 * UM)],
                                    900))
    # boxes on grid lines and straddling them
    top.shapes(L[5]).insert(db.Box(10 * UM, 10 * UM, 12 * UM, 12 * UM))
    top.shapes(L[5]).insert(db.Box(20500, 20300, 25200, 21100))
    top.shapes(L[5]).insert(db.Box(1 * UM, 15 * UM, 29 * UM, 15 * UM + 80))
    # zero-area shapes mark nothing
    top.shapes(L[6]).insert(db.Box(5 * UM, 5 * UM, 5 * UM, 9 * UM))
    top.shapes(L[6]).insert(db.Path([P(7 * UM, 7 * UM), P(12 * UM, 7 * UM)], 0))
    top.shapes(L[6]).insert(db.Box(14 * UM, 14 * UM, 16 * UM, 16 * UM))
    # a layer without any positive-area shape: recorded as `empty`
    L7 = ly.layer(7, 0)
    top.shapes(L7).insert(db.Box(3 * UM, 3 * UM, 3 * UM, 20 * UM))
    top.shapes(L7).insert(db.Path([P(2 * UM, 2 * UM), P(9 * UM, 2 * UM)], 0))
    ly.write(str(path))
    ly._destroy()


def write_reps(path):
    ly = db.Layout()
    ly.dbu = 0.001
    top = ly.create_cell("REPS")
    child = ly.create_cell("C")
    grand = ly.create_cell("G")
    tiny = ly.create_cell("T")
    l1, l2, l3 = ly.layer(1, 0), ly.layer(2, 0), ly.layer(3, 0)
    grand.shapes(l1).insert(db.Box(0, 0, 600, 600))
    tiny.shapes(l2).insert(db.Box(0, 0, 100, 100))
    child.shapes(l1).insert(db.Polygon([
        P(0, 0), P(3 * UM, 0), P(3 * UM, 800), P(800, 800), P(800, 3 * UM),
        P(0, 3 * UM)]))
    child.shapes(l3).insert(db.Path([P(200, 2500), P(2800, 2500)], 300))
    child.insert(db.CellInstArray(grand.cell_index(),
                                  db.Trans(db.Vector(1500, 1500))))
    # axis arrays: gap 0.2 um (< cell) and 1.9 um (>= cell)
    top.insert(db.CellInstArray(grand.cell_index(),
                                db.Trans(db.Vector(1000, 1000)),
                                db.Vector(800, 0), db.Vector(0, 800), 12, 6))
    top.insert(db.CellInstArray(grand.cell_index(),
                                db.Trans(db.Vector(1000, 12000)),
                                db.Vector(2500, 0), db.Vector(0, 2500), 8, 4))
    # the review's counterexample: a diagonal-vector array of tiny boxes
    top.insert(db.CellInstArray(tiny.cell_index(),
                                db.Trans(db.Vector(2000, 22000)),
                                db.Vector(400, 400), db.Vector(400, -400),
                                60, 2))
    # rotated / mirrored placements of the L child (nested grand)
    for k, t in enumerate([db.Trans(0, False, 24000, 1000),
                           db.Trans(1, False, 30000, 1000),
                           db.Trans(2, True, 30000, 8000),
                           db.Trans(3, True, 24000, 8000)]):
        top.insert(db.CellInstArray(child.cell_index(), t))
    # two far-apart clusters of identical boxes (a Pts repetition when
    # the writer compresses them; the oracle does not care)
    for cx, cy in ((30000, 14000), (30000, 26000)):
        for k in range(5):
            top.shapes(l3).insert(db.Box(cx + k * 700, cy,
                                         cx + k * 700 + 300, cy + 300))
    opt = db.SaveLayoutOptions()
    opt.format = "OASIS"
    opt.oasis_compression_level = 10
    opt.oasis_recompress = True
    ly.write(str(path), opt)
    ly._destroy()


def write_deep(path):
    """three placement depths (per-depth planes, 2026-09-16): 1/0 at
    depth 0 (the top's own box), 1 (A, placed twice) and 2 (B inside
    A); 2/0 only at depth 1, 3/0 only at depth 2"""
    ly = db.Layout()
    ly.dbu = 0.001
    top = ly.create_cell("DEEP")
    a = ly.create_cell("A")
    b = ly.create_cell("B")
    l1, l2, l3 = ly.layer(1, 0), ly.layer(2, 0), ly.layer(3, 0)
    top.shapes(l1).insert(db.Box(0, 0, 3 * UM, 3 * UM))
    a.shapes(l1).insert(db.Box(0, 0, 2 * UM, 2 * UM))
    a.shapes(l2).insert(db.Box(3 * UM, 0, 5 * UM, 2 * UM))
    b.shapes(l1).insert(db.Box(0, 0, 2 * UM, 2 * UM))
    b.shapes(l3).insert(db.Box(3 * UM, 0, 5 * UM, 2 * UM))
    a.insert(db.CellInstArray(b.cell_index(), db.Trans(db.Vector(0, 5 * UM))))
    for x in (10, 20):
        top.insert(db.CellInstArray(a.cell_index(),
                                    db.Trans(db.Vector(x * UM, 0))))
    ly.write(str(path))
    ly._destroy()


def write_uturn(path):
    """1/0 holds a U-turn path the hull refuses (the raster refuses it
    too) beside a box; 2/0 a plain box."""
    ly = db.Layout()
    ly.dbu = 0.001
    top = ly.create_cell("UTURN")
    l1, l2 = ly.layer(1, 0), ly.layer(2, 0)
    top.shapes(l1).insert(db.Path([P(2 * UM, 2 * UM), P(20 * UM, 2 * UM),
                                   P(2 * UM, 2 * UM)], 1000))
    top.shapes(l1).insert(db.Box(2 * UM, 10 * UM, 20 * UM, 20 * UM))
    top.shapes(l2).insert(db.Box(2 * UM, 2 * UM, 20 * UM, 20 * UM))
    ly.write(str(path))
    ly._destroy()


def write_gaps(path):
    """2000 x 2000 um, 1/0: columns of 100 um boxes whose gaps are 2, 3,
    4 and 5 px at 10 um/px, at three sub-pixel phases; rows of the same
    along y (the empty-run gate)."""
    ly = db.Layout()
    ly.dbu = 0.001
    top = ly.create_cell("GAPS")
    l1 = ly.layer(1, 0)
    y = 100
    for phase in (0, 3, 7):
        x = 100 + phase
        for gap in (20, 30, 40, 50, 30, 20):
            top.shapes(l1).insert(db.Box(x * UM, y * UM, (x + 100) * UM,
                                         (y + 150) * UM))
            x += 100 + gap
        y += 250
    x = 1300
    for phase in (0, 4, 9):
        yy = 100 + phase
        for gap in (20, 30, 40, 50, 30, 20):
            top.shapes(l1).insert(db.Box(x * UM, yy * UM, (x + 150) * UM,
                                         (yy + 100) * UM))
            yy += 100 + gap
        x += 200
    ly.write(str(path))
    ly._destroy()


def write_chip(path, cellname, w_um, h_um):
    ly = db.Layout()
    ly.dbu = 0.001
    top = ly.create_cell(cellname)
    li = ly.layer(1, 0)
    top.shapes(li).insert(db.Box(0, 0, w_um * UM, h_um * UM))
    top.shapes(li).insert(db.Box(2 * UM, 2 * UM, 4 * UM, 4 * UM))
    ly.write(str(path))
    ly._destroy()


def index_with_occupancy(src, um):
    out = vfs_cache_dir(src)
    shutil.rmtree(out, ignore_errors=True)
    floe_index("vfs", src, out, "--occupancy", "--occupancy-um", um,
               "--no-lod", "--slow-cell-s", "999", "--jobs", "2")
    return Path(out)


# ------------------------------------------------------------------ gates

class GenerationOracleTests(unittest.TestCase):
    """gate 1: every level-0 bit equals KLayout's shape intersection;
    upper levels are the OR pool of the level below"""

    @classmethod
    def setUpClass(cls):
        cls.cases = []
        shapes = TMP / "shapes.oas"
        reps = TMP / "reps.oas"
        write_shapes(shapes)
        write_reps(reps)
        cls.cases.append((shapes, index_with_occupancy(shapes, 1)))
        cls.cases.append((reps, index_with_occupancy(reps, 1)))
        deep = TMP / "deep.oas"
        write_deep(deep)
        cls.cases.append((deep, index_with_occupancy(deep, 1)))
        src = sys.argv[1] if len(sys.argv) > 1 else None
        if src and Path(src).is_file():
            real = TMP / Path(src).name
            shutil.copy2(src, real)
            cls.cases.append((real, index_with_occupancy(real, 4)))

    def check_case(self, src, cache):
        ovo = read_ovo(cache / "design.ovo")
        checked = 0
        for layer in ovo["layers"]:
            w, h, lit = oracle_level0(src, layer["key"], ovo)
            if layer["status"] == "empty":
                # no bitmap at all, and the oracle agrees there is nothing
                self.assertEqual(lit, set(), layer["key"])
                self.assertTrue(all(lv == (0, 0, b"") for lv in layer["levels"]))
                checked += 1
                continue
            self.assertEqual(layer["status"], "ok", layer)
            l0 = layer["levels"][0]
            self.assertEqual((l0[0], l0[1]), (w, h), layer["key"])
            mine = {(i, j) for j in range(h) for i in range(w)
                    if bit(l0, i, j)}
            missing = sorted(lit - mine)[:10]
            extra = sorted(mine - lit)[:10]
            self.assertEqual(
                (missing, extra), ([], []),
                "%s layer %s: %d oracle-lit cells missing (%s), %d extra "
                "(%s), oracle %d cells" % (src.name, layer["key"],
                                           len(lit - mine), missing,
                                           len(mine - lit), extra,
                                           len(lit)))
            checked += 1
            for below, above in zip(layer["levels"], layer["levels"][1:]):
                self.assertEqual(pool(below), above,
                                 "%s %s pyramid" % (src.name, layer["key"]))
            self.assertLessEqual(max(layer["levels"][-1][:2]), 64)
            # every plane holds exactly the shapes of its placement
            # depth (KLayout's iterator limited to that depth) and is
            # its own pyramid; planes ascend by depth
            depths = [p["depth"] for p in layer["planes"]]
            self.assertEqual(depths, sorted(depths), layer["key"])
            self.assertGreater(len(depths), 0, layer["key"])
            for plane in layer["planes"]:
                _, _, lit_d = oracle_level0(src, layer["key"], ovo,
                                            depth=plane["depth"])
                p0 = plane["levels"][0]
                mine_d = {(i, j) for j in range(h) for i in range(w)
                          if bit(p0, i, j)}
                self.assertEqual(
                    (sorted(lit_d - mine_d)[:10], sorted(mine_d - lit_d)[:10]),
                    ([], []),
                    "%s layer %s depth %d: %d missing, %d extra" % (
                        src.name, layer["key"], plane["depth"],
                        len(lit_d - mine_d), len(mine_d - lit_d)))
                for below, above in zip(plane["levels"], plane["levels"][1:]):
                    self.assertEqual(pool(below), above,
                                     "%s %s depth %d pyramid"
                                     % (src.name, layer["key"], plane["depth"]))
        self.assertGreater(checked, 0)

    def test_planes_follow_the_placement_depth(self):
        deep = read_ovo(self.cases[2][1] / "design.ovo")
        self.assertEqual(deep["version"], 2)
        by_key = {l["key"]: l for l in deep["layers"]}
        planes = lambda key: [p["depth"] for p in by_key[key]["planes"]]
        self.assertEqual((planes((1, 0)), planes((2, 0)), planes((3, 0))),
                         ([0, 1, 2], [1], [2]))
        nlv = deep["n_levels"]
        # 1 um cells: the top's box covers cells 0..2, A at x=10 um
        # covers 10..11, B (inside A, y=5) covers rows 5..6
        l0 = levels_at_depth(by_key[(1, 0)], 0, nlv)[0]
        self.assertTrue(bit(l0, 1, 1) and not bit(l0, 11, 1))
        l1 = levels_at_depth(by_key[(1, 0)], 1, nlv)[0]
        self.assertTrue(bit(l1, 1, 1) and bit(l1, 11, 1) and bit(l1, 21, 1)
                        and not bit(l1, 11, 6))
        l2 = levels_at_depth(by_key[(1, 0)], 2, nlv)[0]
        self.assertTrue(bit(l2, 11, 6))
        self.assertEqual(l2, by_key[(1, 0)]["levels"][0])
        # nothing of 3/0 above depth 2
        self.assertEqual(levels_at_depth(by_key[(3, 0)], 1, nlv),
                         [(0, 0, b"")] * nlv)

    def test_fixtures_and_asset_match_the_oracle_on_every_cell(self):
        for src, cache in self.cases:
            with self.subTest(src=src.name):
                self.check_case(src, cache)

    def test_the_fixtures_exercise_empty_space_and_reps(self):
        # the L's empty corner and the ring's hole stay empty; the
        # diagonal array stays sparse (a bbox fill would light hundreds)
        shapes = read_ovo(self.cases[0][1] / "design.ovo")
        by_key = {l["key"]: l for l in shapes["layers"]}
        l_shape = by_key[(1, 0)]["levels"][0]
        self.assertFalse(bit(l_shape, 20, 20))
        self.assertTrue(bit(l_shape, 0, 20) and bit(l_shape, 20, 0))
        ring = by_key[(2, 0)]["levels"][0]
        self.assertFalse(bit(ring, 15, 15))
        self.assertTrue(bit(ring, 15, 3))
        zero = by_key[(6, 0)]["levels"][0]
        self.assertEqual(sum(bin(b).count("1") for b in zero[2]), 4)
        reps = read_ovo(self.cases[1][1] / "design.ovo")
        by_key = {l["key"]: l for l in reps["layers"]}
        diag = by_key[(2, 0)]["levels"][0]
        lit = sum(bin(b).count("1") for b in diag[2])
        self.assertGreater(lit, 20)
        self.assertLess(lit, 120)


class GenerationContractTests(unittest.TestCase):
    """gate 5 (generation part): listing/dump, refusals, additive runs,
    limits, kill point, wrapper cleanup, jobdeck forwarding"""

    @classmethod
    def setUpClass(cls):
        cls.src = TMP / "contract.oas"
        write_shapes(cls.src)
        cls.cache = index_with_occupancy(cls.src, 1)
        cls.other = TMP / "other.oas"
        write_reps(cls.other)
        cls.other_cache = index_with_occupancy(cls.other, 1)

    def test_an_empty_layer_costs_no_bitmap_and_threads_write_the_same_file(self):
        # field 2026-09-14: the deck-wide build wrote 9.8 GB, half of it
        # full-size zero pyramids of layers without a shape, on one
        # thread per layer. A layer without a positive-area shape is
        # `empty` without bitmaps (the file holds exactly the ok
        # layers' pyramids), and the marking of a layer is split over
        # --jobs threads whose merge is byte-identical to one thread
        ovo = read_ovo(self.cache / "design.ovo")
        by_key = {l["key"]: l for l in ovo["layers"]}
        self.assertEqual(by_key[(7, 0)]["status"], "empty")
        self.assertTrue(all(lv == (0, 0, b"") for lv in by_key[(7, 0)]["levels"]))
        rows = self.listing(self.cache).stdout.splitlines()
        self.assertTrue(any("ld=7/0 status=empty work=0" in r for r in rows),
                        rows)
        # version 2: a plane count per layer, a depth byte per plane, and
        # one pyramid per plane (only the ok layers have planes)
        table = struct.calcsize(HDR) + 2 + len(ovo["top"]) + sum(
            struct.calcsize(LAYER)
            + len(l["planes"]) * (1 + ovo["n_levels"] * struct.calcsize(LEVEL))
            for l in ovo["layers"])
        bitmaps = sum(len(lv[2]) for l in ovo["layers"]
                      for p in l["planes"] for lv in p["levels"])
        self.assertEqual(os.path.getsize(self.cache / "design.ovo"),
                         table + bitmaps)
        outs = []
        for jobs in (1, 4):
            out = TMP / ("reps_jobs%d.floe" % jobs)
            shutil.rmtree(out, ignore_errors=True)
            res = floe_index("vfs", self.other, out, "--occupancy",
                             "--occupancy-um", 1, "--no-lod", "--slow-cell-s",
                             "999", "--jobs", jobs)
            self.assertIn(" jobs=%d " % jobs, res.stderr)
            outs.append(sha(out / "design.ovo"))
        self.assertEqual(outs[0], outs[1])

    def test_an_unknown_option_is_refused_instead_of_becoming_the_outdir(self):
        # field 2026-09-14: `floe-index index file.oas --occupancy-only`
        # (the legacy tile indexer knows no such option) took the option
        # as the output directory and built a tile index under a folder
        # named --occupancy-only; every subcommand's positional arm did
        # the same. An argument starting with -- that is not an option
        # of the subcommand exits 2 before anything touches the file
        # system (vfsd would otherwise start serving on stdin).
        cases = (("index", self.src, "--occupancy-only"),
                 ("vfs", self.src, "--occupancy_only"),
                 ("tile", self.src, "--grids"),
                 ("occupancy", self.cache, "--layers"),
                 ("vfsd", self.cache, "--budget"))
        for sub, target, opt in cases:
            with self.subTest(sub=sub, opt=opt):
                cwd = TMP / ("unknown_option_" + sub)
                shutil.rmtree(cwd, ignore_errors=True)
                cwd.mkdir()
                res = subprocess.run(
                    [str(BIN), sub, str(target), opt], cwd=cwd,
                    capture_output=True, text=True, env=run_env(),
                    stdin=subprocess.DEVNULL, timeout=120)
                self.assertEqual(res.returncode, 2, res.stderr)
                self.assertIn("floe-index %s: unknown option %s" % (sub, opt),
                              res.stderr)
                self.assertEqual(sorted(p.name for p in cwd.iterdir()), [],
                                 "the option must not become a directory")

    def listing(self, cache, ok=0):
        return floe_index("occupancy", cache, ok=ok)

    def test_the_base_cell_follows_the_chip_size_unless_given(self):
        # 2026-09-16: no --occupancy-um -> the coarsest of 4/2/1/0.5/0.25
        # um whose longer side reaches 2,048 cells (a 10 x 8 um test chip
        # floors at 0.25; a 3 x 2 mm one gets 1 um); an explicit cell
        # is taken as given
        small = TMP / "auto_small.oas"
        write_chip(small, "SMALL", 10, 8)
        big = TMP / "auto_big.oas"
        write_chip(big, "BIG", 3000, 2000)
        for src, extra, base, cell in ((small, (), 0.25, 250),
                                       (big, (), 1.0, 1000),
                                       (small, ("--occupancy-um", "4"), 4.0, 4000)):
            cache = Path(vfs_cache_dir(src))
            shutil.rmtree(cache, ignore_errors=True)
            res = floe_index("vfs", src, cache, "--occupancy", *extra,
                             "--no-lod", "--slow-cell-s", "999", "--jobs", "2")
            self.assertIn("occupancy cell=%gum (%d dbu%s)"
                          % (base, cell, "" if extra else ", auto"),
                          res.stderr)
            head = self.listing(cache).stdout.splitlines()[0]
            self.assertIn("cell_dbu=%d base_um=%g " % (cell, base), head)
            self.assertEqual(read_ovo(cache / "design.ovo")["cell"], cell)

    def test_a_reader_closing_the_pipe_early_does_not_panic(self):
        # field 2026-09-16: `floe-index occupancy … | head -1` printed
        # "failed printing to stdout: Broken pipe" from a panic; the
        # process now dies quietly on the broken pipe like a C program
        p = subprocess.Popen([str(BIN), "occupancy", str(self.cache)],
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        first = p.stdout.readline()
        p.stdout.close()
        err = p.stderr.read()
        p.wait(timeout=60)
        self.assertTrue(first.startswith(b"occupancy file="), first)
        self.assertNotIn(b"panicked", err, err)
        self.assertNotIn(b"Broken pipe", err, err)
        self.assertIn(p.returncode, (0, -13), (p.returncode, err))

    def test_listing_and_dump_match_the_file(self):
        res = self.listing(self.cache)
        head = res.stdout.splitlines()[0]
        self.assertIn("identity=ok", head)
        self.assertIn("top=SHAPES", head)
        ovo = read_ovo(self.cache / "design.ovo")
        self.assertIn("cell_dbu=%d" % ovo["cell"], head)
        self.assertIn("grid=%dx%d" % ovo["layers"][0]["levels"][0][:2], head)
        self.assertEqual(ovo["top"], "SHAPES")
        st = os.stat(self.src)
        self.assertEqual((ovo["src_size"], ovo["src_mtime"]),
                         (st.st_size, int(st.st_mtime)))
        rows = [l for l in res.stdout.splitlines() if l.startswith("layer ")]
        self.assertEqual(len(rows), len(ovo["layers"]))
        for row, layer in zip(rows, ovo["layers"]):
            self.assertIn("ld=%d/%d" % layer["key"], row)
            self.assertIn("status=empty" if layer["key"] == (7, 0)
                          else "status=ok", row)
            counts = [sum(bin(b).count("1") for b in lv[2])
                      for lv in layer["levels"]]
            self.assertIn("set=" + ",".join(map(str, counts)), row)
            # a flat source: one plane, depth 0, with the level-0 count
            self.assertIn("planes=-" if layer["key"] == (7, 0)
                          else "planes=0:%d" % counts[0], row)
        levels = next(l for l in ovo["layers"] if l["key"] == (1, 0))["levels"]
        top_level = len(levels) - 1
        dump = floe_index("occupancy", self.cache, "--layer", "1/0",
                          "--level", top_level, "--dump").stdout.splitlines()
        start = next(i for i, l in enumerate(dump) if l.startswith("dump "))
        level = levels[top_level]
        self.assertEqual(dump[start], "dump ld=1/0 level=%d w=%d h=%d"
                         % (top_level, level[0], level[1]))
        floe_index("occupancy", self.cache, "--layer", "1/0", "--level",
                   top_level + 1, "--dump", ok=1)
        for j, row in enumerate(dump[start + 1:start + 1 + level[1]]):
            self.assertEqual(row, "".join("1" if bit(level, i, j) else "0"
                                          for i in range(level[0])),
                             "row %d" % j)
        # --depth N dumps the planes at or above N (the flat source's
        # depth 0 is everything)
        dump0 = floe_index("occupancy", self.cache, "--layer", "1/0",
                           "--level", top_level, "--depth", "0",
                           "--dump").stdout.splitlines()
        start0 = next(i for i, l in enumerate(dump0) if l.startswith("dump "))
        self.assertEqual(dump0[start0], "dump ld=1/0 level=%d depth=0 w=%d h=%d"
                         % (top_level, level[0], level[1]))
        self.assertEqual(dump0[start0 + 1:start0 + 1 + level[1]],
                         dump[start + 1:start + 1 + level[1]])

    def test_identity_mismatch_and_truncation_are_refused(self):
        ovo = self.cache / "design.ovo"
        keep = ovo.read_bytes()
        try:
            shutil.copy2(self.other_cache / "design.ovo", ovo)
            res = self.listing(self.cache, ok=1)
            self.assertIn("identity=mismatch", res.stdout)
            self.assertIn("identity_error=", res.stdout)
            ovo.write_bytes(keep[:-7])
            res = self.listing(self.cache, ok=1)
            self.assertIn("truncated", res.stderr)
            ovo.write_bytes(keep[:30])
            res = self.listing(self.cache, ok=1)
            self.assertIn("truncated", res.stderr)
            bad = bytearray(keep)
            bad[0] = ord("X")
            ovo.write_bytes(bytes(bad))
            res = self.listing(self.cache, ok=1)
            self.assertIn("magic", res.stderr)
        finally:
            ovo.write_bytes(keep)
        self.listing(self.cache)

    def test_occupancy_only_replaces_the_summary_and_nothing_else(self):
        before = {f: sha(self.cache / f) for f in
                  ("design.ovm", "design.ovp", "meta.json")}
        res = floe2("index", self.src, "--occupancy-only", "--occupancy-um",
                    "2")
        self.assertIn("--occupancy-only", res.stdout)
        self.assertIn("--occupancy-um 2.0", res.stdout)
        after = {f: sha(self.cache / f) for f in before}
        self.assertEqual(before, after)
        ovo = read_ovo(self.cache / "design.ovo")
        self.assertEqual(ovo["cell"], 2000)
        self.assertFalse((self.cache / "design.ovo.tmp").exists())
        # a current cache with the summary: --occupancy is a no-op
        res = floe2("index", self.src, "--occupancy")
        self.assertIn("occupancy already present", res.stdout)
        self.assertEqual(read_ovo(self.cache / "design.ovo")["cell"], 2000)
        # a current cache WITHOUT it: --occupancy adds it additively
        (self.cache / "design.ovo").unlink()
        res = floe2("index", self.src, "--occupancy")
        self.assertIn("--occupancy-only", res.stdout)
        # no --occupancy-um: the automatic cell (2026-09-16), 0.25 um on
        # this tens-of-microns fixture
        self.assertEqual(read_ovo(self.cache / "design.ovo")["cell"], 250)
        self.assertEqual({f: sha(self.cache / f) for f in before}, before)
        # --occupancy-only on a stale/missing cache is refused
        res = floe2("index", TMP / "missing.oas", "--occupancy-only", ok=1)
        self.assertIn("source not found", res.stderr)
        floe2("index", self.src, "--occupancy-only", "--occupancy-um", "1")
        self.assertEqual(read_ovo(self.cache / "design.ovo")["cell"], 1000)

    def test_a_refused_path_leaves_the_layer_without_a_summary(self):
        # review 2026-09-11 (2nd) P1-2: the U-turn path was dropped and
        # the layer published ok; now the layer is none:unsupported and
        # the page path draws it (or refuses it loudly)
        src = TMP / "uturn.oas"
        write_uturn(src)
        cache = index_with_occupancy(src, 1)
        rows = [l for l in self.listing(cache).stdout.splitlines()
                if l.startswith("layer ")]
        self.assertTrue(any("ld=1/0 status=none:unsupported" in r for r in rows), rows)
        self.assertTrue(any("ld=2/0 status=ok" in r for r in rows), rows)
        ovo = read_ovo(cache / "design.ovo")
        by_key = {l["key"]: l for l in ovo["layers"]}
        self.assertEqual(by_key[(1, 0)]["status"], "none:unsupported")
        self.assertTrue(all(lv[2] == b"" for lv in by_key[(1, 0)]["levels"]))

    def test_corrupt_bitmap_offsets_are_refused(self):
        # review 2026-09-11 (2nd) P2-4: an offset into the header read as
        # a plausible bitmap (identity=ok, 3,275 wrong pixels)
        src = TMP / "offsets.oas"
        write_thinwide(src)
        cache = index_with_occupancy(src, 4)
        ovo = cache / "design.ovo"
        keep = ovo.read_bytes()
        hdr = struct.calcsize(HDR)
        top_len = struct.unpack_from("<H", keep, hdr)[0]
        first_entry = hdr + 2 + top_len + struct.calcsize(LAYER)
        off0 = first_entry + 8
        off1 = first_entry + struct.calcsize(LEVEL) + 8
        level0_off = struct.unpack_from("<Q", keep, off0)[0]
        try:
            bad = bytearray(keep)
            struct.pack_into("<Q", bad, off0, 0)
            ovo.write_bytes(bytes(bad))
            res = self.listing(cache, ok=1)
            self.assertIn("inside the header", res.stderr)
            bad = bytearray(keep)
            struct.pack_into("<Q", bad, off1, level0_off)
            ovo.write_bytes(bytes(bad))
            res = self.listing(cache, ok=1)
            self.assertIn("overlaps", res.stderr)
        finally:
            ovo.write_bytes(keep)
        self.listing(cache)

    def test_the_synthetic_mask_chip_reproduces_the_field_symptom(self):
        # tools/gen_maskchip.py: the 35.8 x 34.6 mm stand-in for the
        # field source. Under the plain cull policy the corner cell
        # (a thick record) survives while its connected hairline
        # neighbours vanish; under keep they stay (exact pixels); the
        # whole-chip fit view summarizes every layer.
        from PIL import Image
        d = TMP / "maskchip"
        d.mkdir()
        src = d / "chip.oas"
        res = subprocess.run([sys.executable, "-B", str(ROOT / "tools" /
                              "gen_maskchip.py"), str(src), "--cells", "6",
                              "--pitch", "2.0", "--clusters", "1", "--jb"],
                             capture_output=True, text=True, cwd=ROOT)
        self.assertEqual(res.returncode, 0, res.stderr)
        self.assertTrue((d / "chip.jb").is_file())
        cache = Path(vfs_cache_dir(src))
        floe_index("vfs", src, cache, "--occupancy", "--no-lod",
                   "--slow-cell-s", "999", "--jobs", "2")
        ovo = read_ovo(cache / "design.ovo")
        self.assertEqual({l["key"] for l in ovo["layers"]},
                         {(1, 0), (3, 0), (3, 300), (4, 0)})
        self.assertTrue(all(l["status"] == "ok" for l in ovo["layers"]))
        x0, y0, x1, y1 = ovo["bbox"]
        self.assertAlmostEqual((x1 - x0) * 0.0001, 35838.4, places=3)
        self.assertAlmostEqual((y1 - y0) * 0.0001, 34617.6, places=3)

        def lit(detail, thin, env=None):
            out = d / ("corner-%s-%s.png" % (detail, thin))
            floe2("render", src, "--bbox", "17300,-17309,17919,-16700",
                  "--px", "300", "--detail", detail, "--thin", thin,
                  "--layers", "3/0", "--out", out, env=env)
            im = Image.open(out).convert("RGB")
            return sum(1 for p in im.getdata() if p != (0, 0, 0))
        exact = lit("exact", "keep")
        cull = lit("high", "cull")
        keep = lit("high", "keep")
        self.assertGreater(exact, 10000)
        self.assertEqual(keep, exact)
        # under cull the dense hairline neighbours' pages are cut and
        # dropped: the field symptom is the default (the page frontier
        # is deactivated since 2026-09-17; FLOE_RUST_PAGE_REPS=on keeps
        # representatives of them, the sub-cut rules
        # FLOE_RUST_SUB_CUT_WASH=on wash them all as blocks)
        self.assertGreater(cull, 0)
        self.assertLess(cull, exact // 2)
        reps = lit("high", "cull",
                   env=dict(os.environ, FLOE_RUST_PAGE_REPS="on"))
        self.assertGreater(reps, cull)
        washed = lit("high", "cull",
                     env=dict(os.environ, FLOE_RUST_SUB_CUT_WASH="on"))
        self.assertGreater(washed, exact // 2)

    def test_limits_are_recorded_as_none_never_approximated(self):
        floe_index("vfs", self.src, self.cache, "--occupancy-only",
                   "--occupancy-max-cells", "10")
        res = self.listing(self.cache)
        rows = [l for l in res.stdout.splitlines() if l.startswith("layer ")]
        self.assertTrue(rows and all("status=none:cells" in r for r in rows))
        ovo = read_ovo(self.cache / "design.ovo")
        self.assertTrue(all(lv[2] == b"" for l in ovo["layers"]
                            for lv in l["levels"]))
        floe_index("vfs", self.src, self.cache, "--occupancy-only",
                   "--occupancy-max-work", "0")
        rows = [l for l in self.listing(self.cache).stdout.splitlines()
                if l.startswith("layer ")]
        # the layer without a positive-area shape charges nothing: it
        # stays `empty` under a zero budget
        self.assertTrue(all("status=none:work" in r
                            or "ld=7/0 status=empty" in r for r in rows), rows)
        res = floe_index("vfs", self.src, self.cache, "--occupancy-only",
                         "--occupancy-max-bytes", "1")
        self.assertIn("none:size", res.stderr)
        floe_index("vfs", self.src, self.cache, "--occupancy-only",
                   "--occupancy-um", "1")
        rows = [l for l in self.listing(self.cache).stdout.splitlines()
                if l.startswith("layer ")]
        self.assertTrue(all("status=ok" in r or "ld=7/0 status=empty" in r
                            for r in rows), rows)

    def test_kill_point_keeps_the_previous_file_and_the_wrapper_cleans_up(self):
        before = sha(self.cache / "design.ovo")
        floe_index("vfs", self.src, self.cache, "--occupancy-only",
                   "--occupancy-um", "3", "--kill-at", "occupancy-tmp", ok=9)
        self.assertEqual(sha(self.cache / "design.ovo"), before)
        self.assertTrue((self.cache / "design.ovo.tmp").exists())
        floe2("index", self.src, "--occupancy-only", "--occupancy-um", "3")
        self.assertFalse((self.cache / "design.ovo.tmp").exists())
        self.assertEqual(read_ovo(self.cache / "design.ovo")["cell"], 3000)
        # a child that dies after writing the tmp: the wrapper removes it
        fake = TMP / "fake-index"
        fake.write_text("""#!/usr/bin/env python3
import pathlib, sys
args = sys.argv[1:]
if args and args[0] == "vfsd":
    sys.exit(0)
out = pathlib.Path(args[2])
(out / "design.ovo.tmp").write_bytes(b"half")
sys.exit(9)
""")
        fake.chmod(0o755)
        env = run_env()
        env["FLOE_INDEX_BIN"] = str(fake)
        res = floe2("index", self.src, "--occupancy-only", env=env, ok=9)
        self.assertIn("discarded", res.stderr)
        self.assertFalse((self.cache / "design.ovo.tmp").exists())
        self.assertEqual(read_ovo(self.cache / "design.ovo")["cell"], 3000)
        floe2("index", self.src, "--occupancy-only", "--occupancy-um", "1")

    def test_occupancy_um_implies_a_summary_on_a_fresh_index(self):
        src = TMP / "fresh.oas"
        write_chip(src, "FRESH", 10, 8)
        res = floe2("index", src, "--occupancy-um", "2", "--jobs", "2")
        self.assertIn("--occupancy --occupancy-um 2.0", res.stdout)
        ovo = read_ovo(Path(vfs_cache_dir(src)) / "design.ovo")
        self.assertEqual((ovo["cell"], ovo["top"]), (2000, "FRESH"))
        # a LAYOUT indexes without the summary (2026-09-16: the deck
        # default is on, the layout default off); --occupancy makes it
        # and adds it to a cache without one
        plain = TMP / "plain.oas"
        write_chip(plain, "PLAIN", 10, 8)
        floe2("index", plain, "--jobs", "2")
        self.assertFalse((Path(vfs_cache_dir(plain)) / "design.ovo").exists())
        res = floe2("index", plain, "--jobs", "2")
        self.assertIn("cache up to date", res.stdout)
        self.assertNotIn("--occupancy-only", res.stdout)
        bare = TMP / "bare.oas"
        write_chip(bare, "BARE", 10, 8)
        floe2("index", bare, "--no-occupancy", "--jobs", "2")
        self.assertFalse((Path(vfs_cache_dir(bare)) / "design.ovo").exists())
        res = floe2("index", bare, "--occupancy", "--jobs", "2")
        self.assertIn("--occupancy-only", res.stdout)
        self.assertTrue((Path(vfs_cache_dir(bare)) / "design.ovo").exists())
        res = floe2("index", bare, "--occupancy", "--jobs", "2")
        self.assertIn("cache up to date", res.stdout)
        self.assertIn("occupancy already present", res.stdout)

    def test_a_deck_indexes_its_sources_with_the_summary_by_default(self):
        # the deck default is ON (2026-09-16): a mask deck's wide view
        # needs the summary; --no-occupancy still turns it off
        for tag, extra, want in (("on", (), True),
                                 ("off", ("--no-occupancy",), False)):
            deck_dir = TMP / ("deck_default_" + tag)
            deck_dir.mkdir()
            write_chip(deck_dir / "chipA.oas", "CHIPA", 30, 30)
            write_chip(deck_dir / "chipB.oas", "CHIPB", 20, 20)
            (deck_dir / "occ.jb").write_text(DECK)
            res = floe2("index", deck_dir / "occ.jb", *extra, "--jobs", "2")
            self.assertIn("2 built, 0 failed, 0 kept", res.stdout)
            for name in ("chipA.oas", "chipB.oas"):
                ovo = Path(vfs_cache_dir(deck_dir / name)) / "design.ovo"
                self.assertEqual(ovo.exists(), want, (tag, name))

    def test_jobdeck_wrapper_forwards_the_occupancy_options(self):
        deck_dir = TMP / "deck"
        deck_dir.mkdir()
        write_chip(deck_dir / "chipA.oas", "CHIPA", 30, 30)
        write_chip(deck_dir / "chipB.oas", "CHIPB", 20, 20)
        (deck_dir / "occ.jb").write_text(DECK)
        caches = [Path(vfs_cache_dir(deck_dir / "chipA.oas")),
                  Path(vfs_cache_dir(deck_dir / "chipB.oas"))]
        res = floe2("index", deck_dir / "occ.jb", "--occupancy",
                    "--occupancy-um", "5", "--jobs", "2")
        self.assertIn("2 built, 0 failed, 0 kept", res.stdout)
        for c in caches:
            self.assertEqual(read_ovo(c / "design.ovo")["cell"], 5000)
        before = {c: (sha(c / "design.ovm"), sha(c / "design.ovp"))
                  for c in caches}
        res = floe2("index", deck_dir / "occ.jb", "--occupancy-only",
                    "--occupancy-um", "10", "--jobs", "2")
        self.assertIn("[jobdeck] occupancy : (1/2)", res.stdout)
        # the closing line repeats the position and carries the
        # elapsed / remaining estimate (field 2026-09-15, 667 sources)
        self.assertRegex(res.stdout, r"\[jobdeck\] occupancy : \(2/2\) ok "
                                     r"\S+ \(\d+\.\ds; \d+:\d\d elapsed, "
                                     r"~0:00 left\)")
        self.assertIn("2 built, 0 failed, 0 kept", res.stdout)
        for c in caches:
            self.assertEqual(read_ovo(c / "design.ovo")["cell"], 10000)
            self.assertEqual((sha(c / "design.ovm"), sha(c / "design.ovp")),
                             before[c])
        res = floe2("index", deck_dir / "occ.jb", "--occupancy", "--jobs", "2")
        self.assertIn("2 source(s) already indexed", res.stdout)
        self.assertIn("0 built, 0 failed, 2 kept", res.stdout)
        # a source without the summary gets it; the other is kept
        (caches[1] / "design.ovo").unlink()
        res = floe2("index", deck_dir / "occ.jb", "--occupancy", "--jobs", "2")
        self.assertIn("1 built, 0 failed, 1 kept", res.stdout)
        # rebuilt without --occupancy-um: the automatic cell (0.25 um on
        # a 20 um source)
        self.assertEqual(read_ovo(caches[1] / "design.ovo")["cell"], 250)



# ------------------------------------------------------- M2: rendering

def write_thinwide(path):
    """2000 x 2000 um: 1/0 all-thin lines (0.1 x 40 um on a 100 um
    grid), 2/0 a solid block and an L with a 70 um empty corner, 3/0
    one 1500 um square (over the gate's work limit -> none:work, so it
    keeps the page path and sits ABOVE the summary layers)."""
    ly = db.Layout()
    ly.dbu = 0.001
    top = ly.create_cell("THINWIDE")
    l1, l2, l3 = ly.layer(1, 0), ly.layer(2, 0), ly.layer(3, 0)
    for x in range(50, 2000, 100):
        for y in range(50, 2000, 100):
            top.shapes(l1).insert(db.Box(x * UM, y * UM, x * UM + 100,
                                         (y + 40) * UM))
    top.shapes(l2).insert(db.Box(1200 * UM, 1200 * UM, 1400 * UM, 1400 * UM))
    top.shapes(l2).insert(db.Polygon([
        P(200 * UM, 1200 * UM), P(300 * UM, 1200 * UM), P(300 * UM, 1230 * UM),
        P(230 * UM, 1230 * UM), P(230 * UM, 1300 * UM), P(200 * UM, 1300 * UM)]))
    top.shapes(l3).insert(db.Box(100 * UM, 100 * UM, 1600 * UM, 1600 * UM))
    # one child (hierarchy height 1) with a 2/0 box inside the top's
    # 2/0 block, so every depth draws the same pixels: 2/0's pages
    # reach depth 1, 1/0's stay at depth 0 - the summary's per-layer
    # depth condition is tested on both (2026-09-15)
    deep = ly.create_cell("DEEP")
    deep.shapes(l2).insert(db.Box(0, 0, 50 * UM, 50 * UM))
    top.insert(db.CellInstArray(deep.cell_index(),
                                db.Trans(db.Vector(1250 * UM, 1250 * UM))))
    # a second child whose 2/0 box sits in empty space (600..650 x
    # 1600..1650 um): the per-depth planes (2026-09-16) leave it out
    # at depth 0 and draw it from depth 1
    far = ly.create_cell("FAR")
    far.shapes(l2).insert(db.Box(0, 0, 50 * UM, 50 * UM))
    top.insert(db.CellInstArray(far.cell_index(),
                                db.Trans(db.Vector(600 * UM, 1600 * UM))))
    ly.write(str(path))
    ly._destroy()


def expected_mask(level, bbox_dbu, width, height, halo=0):
    """Pixels holding the centre of an occupied cell of `level` under
    the render's view (the summary path's projection, mirrored
    operation by operation); `halo` admits pixels just outside the
    frame for boundary tests."""
    import math
    w, h, bits = level["w"], level["h"], level["bits"]
    cell, x0, y0 = level["cell"], level["x0"], level["y0"]
    vx0, vy0, vx1, vy1 = (float(v) for v in bbox_dbu)
    span_x, span_y = vx1 - vx0, vy1 - vy0
    lit = set()
    rb = (w + 7) // 8
    for j in range(h):
        row = bits[j * rb:(j + 1) * rb]
        if not any(row):
            continue
        for i in range(w):
            if not (row[i // 8] >> (i % 8)) & 1:
                continue
            mx = x0 + (i + 0.5) * cell
            my = y0 + (j + 0.5) * cell
            pc = math.floor((mx - vx0) * width / span_x)
            pr = math.floor((vy1 - my) * height / span_y)
            if -halo <= pc < width + halo and -halo <= pr < height + halo:
                lit.add((pc, pr))
    return lit


def ovo_level(ovo, key, lv, depth=None):
    layer = next(l for l in ovo["layers"] if l["key"] == key)
    levels = (layer["levels"] if depth is None
              else levels_at_depth(layer, depth, ovo["n_levels"]))
    w, h, bits = levels[lv]
    return dict(w=w, h=h, bits=bits, cell=ovo["cell"] << lv,
                x0=ovo["bbox"][0], y0=ovo["bbox"][1])


def lit_pixels(rgba, width, height):
    return {(i % width, i // width) for i in range(width * height)
            if rgba[4 * i] or rgba[4 * i + 1] or rgba[4 * i + 2]}


def near(pixel, pixels, d):
    x, y = pixel
    return any((x + dx, y + dy) in pixels for dx in range(-d, d + 1)
               for dy in range(-d, d + 1))


class RenderTests(unittest.TestCase):
    """M2 (docs/OCCUPANCY_PLAN.ko.md gates 2, 3 and the render half of
    5): the wide-view mask equals the projected level at every pan
    phase, the level follows the pixel size, the mask is conservative
    against the exact render and keeps the empty space, cull / exact /
    limited-depth requests are untouched, the kill switch works, a
    none:work layer keeps the page path in its own slot above the
    summary, and a rebuilt design.ovo is picked up by a running daemon
    (no stale retained frame)."""

    @classmethod
    def setUpClass(cls):
        cls.src = TMP / "thinwide.oas"
        write_thinwide(cls.src)
        cls.cache = Path(vfs_cache_dir(cls.src))
        floe_index("vfs", cls.src, cls.cache, "--occupancy", "--occupancy-um",
                   "4", "--occupancy-max-work", "100000", "--no-lod",
                   "--slow-cell-s", "999", "--jobs", "2")
        rows = [l for l in floe_index("occupancy", cls.cache).stdout.splitlines()
                if l.startswith("layer ")]
        assert any("ld=3/0 status=none:work" in r for r in rows), rows
        assert sum("status=ok" in r for r in rows) == 2, rows
        os.environ["FLOE_RENDERD_BIN"] = str(
            ROOT / "rust" / "target" / "release" / "floe-renderd")
        os.environ.pop("FLOE_RUST_OCCUPANCY", None)
        cls.worker = cls._start_worker()
        os.environ["FLOE_RUST_OCCUPANCY"] = "off"
        cls.worker_off = cls._start_worker()
        del os.environ["FLOE_RUST_OCCUPANCY"]
        # the per-depth planes' kill switch: a version-2 file used like
        # a version-1 one (only at a depth that draws the layer whole)
        os.environ["FLOE_RUST_OCCUPANCY_DEPTH"] = "off"
        cls.worker_nodepth = cls._start_worker()
        del os.environ["FLOE_RUST_OCCUPANCY_DEPTH"]
        cls.gen = 100

    @classmethod
    def tearDownClass(cls):
        for w in (cls.worker, cls.worker_off, cls.worker_nodepth):
            try:
                w.stop()
            except Exception:
                pass

    @classmethod
    def _start_worker(cls):
        sys.path.insert(0, str(ROOT))
        from floe.cache import Cache
        from floe.rust_render import RustRenderWorker
        cache = Cache(str(cls.src))
        cache.load()
        worker = RustRenderWorker(cache)
        worker.start()
        return worker

    @classmethod
    def _render(cls, worker, bbox_um=(0, 0, 2000, 2000), px=200, cut_px=1.0,
                thin="keep", visible=None, depth=None, fill="solid",
                width_px=1):
        cls.gen += 1
        gen = cls.gen
        solid = "\n".join(["*" * 16] * 16)
        keys = [(int(l["layer"]), int(l["datatype"]))
                for l in worker.cache.meta["layers"]]
        # speckle is the worker's default fill: only the widths are sent
        worker.submit({"kind": "repattern",
                       "fills": [] if fill == "speckle"
                       else [(k, solid) for k in keys],
                       "widths": [(k, width_px) for k in keys]})
        bbox = tuple(float(v * UM) for v in bbox_um)
        job = {"kind": "render", "gen": gen, "scope": "headless",
               "bbox": bbox, "view": None, "w": px, "h": px, "depth": depth,
               "cut_px": cut_px, "lod": False, "frames": False,
               "labels": False, "abstract": False, "visible": visible,
               "frame_format": "raw", "thin": thin}
        worker.submit(job)
        while True:
            res = worker.res.get(timeout=60)
            if res.get("kind") == "error":
                raise AssertionError("renderd: %s" % res.get("msg"))
            if res.get("kind") != "frame" or res.get("gen") != gen:
                continue
            if res.get("refining"):
                continue
            return lit_pixels(res["rgba"], px, px), res["summary"], bbox

    def test_the_wide_view_mask_equals_the_projected_level_at_every_pan_phase(self):
        ovo = read_ovo(self.cache / "design.ovo")
        # 10 um/px: 4 um cells are 0.4 px, 8 um 0.8 px, 16 um 1.6 px -> level 1
        for shift in (0.0, 2.5, 5.0, 7.5):
            bbox_um = (shift, shift, 2000 + shift, 2000 + shift)
            lit, summ, bbox = self._render(self.worker, bbox_um=bbox_um,
                                           visible=[(1, 0)])
            self.assertEqual((summ["layers"], summ["level"], summ["cell_um"],
                              summ["none"]), (1, 1, 8.0, "-"), summ)
            self.assertGreater(summ["cells"], 0)
            self.assertEqual(summ["pixels"], len(lit))
            expect = expected_mask(ovo_level(ovo, (1, 0), 1), bbox, 200, 200)
            self.assertEqual(lit, expect, "pan phase %s um" % shift)

    def test_the_level_follows_the_pixel_size(self):
        ovo = read_ovo(self.cache / "design.ovo")
        for px, level, cell_um in ((250, 1, 8.0), (260, 0, 4.0)):
            lit, summ, bbox = self._render(self.worker, px=px, visible=[(1, 0)])
            self.assertEqual((summ["level"], summ["cell_um"], summ["none"]),
                             (level, cell_um, "-"), (px, summ))
            self.assertEqual(lit, expected_mask(ovo_level(ovo, (1, 0), level),
                                                bbox, px, px), px)
        # 3.33 um/px: even level 0 is 1.2 px -> near view, the page path
        lit, summ, _ = self._render(self.worker, px=600, visible=[(1, 0)])
        self.assertEqual((summ["layers"], summ["none"]), (0, "near"), summ)
        off, _, _ = self._render(self.worker_off, px=600, visible=[(1, 0)])
        self.assertEqual(lit, off)

    def test_the_summary_is_conservative_against_exact_and_keeps_empty_space(self):
        exact, s_exact, _ = self._render(self.worker, cut_px=0.0, visible=[(1, 0)])
        self.assertEqual(s_exact["none"], "exact")
        lit, summ, _ = self._render(self.worker, visible=[(1, 0)])
        self.assertEqual(summ["layers"], 1)
        # every exact pixel has a summary pixel within one pixel, every
        # summary pixel an exact pixel within two (cells <= 1 px)
        missed_far = [p for p in exact if not near(p, lit, 1)]
        extra_far = [p for p in lit if not near(p, exact, 2)]
        self.assertEqual((missed_far[:5], extra_far[:5]), ([], []),
                         "missed %d extra %d" % (len(missed_far), len(extra_far)))
        # the L's 70 um (7 px) empty corner stays empty away from the edges
        lit2, summ2, _ = self._render(self.worker, visible=[(2, 0)])
        self.assertEqual(summ2["layers"], 1)
        corner = {(x, y) for x in range(24, 29) for y in range(71, 76)}
        self.assertEqual(corner & lit2, set())
        self.assertTrue((21, 78) in lit2 and (28, 79) in lit2, "the L's arms")

    def test_cull_exact_and_limited_depth_requests_are_untouched(self):
        for kw, vis, reason in (({"thin": "cull"}, [(1, 0)], "policy"),
                                ({"cut_px": 0.0}, [(1, 0)], "exact")):
            lit, summ, _ = self._render(self.worker, visible=vis, **kw)
            off, s_off, _ = self._render(self.worker_off, visible=vis, **kw)
            self.assertEqual((summ["layers"], summ["none"]), (0, reason), kw)
            self.assertEqual(lit, off, kw)
        # a limited depth draws the planes at or above it (per-depth
        # planes, 2026-09-16; user: keep at any depth): 2/0's FAR child
        # box (depth 1, at 600..650 x 1600..1650 um = pixels 60..64 x
        # 35..39 at 10 um/px) is left out at depth 0 and drawn from
        # depth 1; the depth-0 summary stays within a pixel of the
        # depth-0 page path, as the full one does of the full path
        far = {(x, y) for x in range(58, 68) for y in range(33, 43)}
        d0, s_d0, _ = self._render(self.worker, visible=[(2, 0)], depth=0)
        self.assertEqual((s_d0["layers"], s_d0["none"]), (1, "-"), s_d0)
        self.assertEqual(d0 & far, set())
        page0, s_page0, _ = self._render(self.worker_off, visible=[(2, 0)], depth=0)
        self.assertEqual(s_page0["none"], "off")
        self.assertEqual(page0 & far, set())
        self.assertEqual(([p for p in page0 if not near(p, d0, 1)][:5],
                          [p for p in d0 if not near(p, page0, 2)][:5]),
                         ([], []), "depth-0 summary vs depth-0 page path")
        full2, s_full2, _ = self._render(self.worker, visible=[(2, 0)])
        d1, s_d1, _ = self._render(self.worker, visible=[(2, 0)], depth=1)
        self.assertEqual((s_full2["layers"], s_d1["layers"]), (1, 1), s_d1)
        self.assertTrue(d1 & far, "the FAR box is drawn from depth 1")
        self.assertEqual(d1, full2)
        # 1/0's pages all sit in the top: depth 0 equals the unlimited
        # depth; both visible at depth 0: both summarized
        full1, s_full1, _ = self._render(self.worker, visible=[(1, 0)])
        d0_1, s_d0_1, _ = self._render(self.worker, visible=[(1, 0)], depth=0)
        self.assertEqual((s_full1["layers"], s_d0_1["layers"], s_d0_1["none"]), (1, 1, "-"), s_d0_1)
        self.assertEqual(d0_1, full1)
        _, s_both, _ = self._render(self.worker, visible=[(1, 0), (2, 0)], depth=0)
        self.assertEqual((s_both["layers"], s_both["none"]), (2, "-"), s_both)
        # FLOE_RUST_OCCUPANCY_DEPTH=off: the 2026-09-15 rule - 2/0 (pages
        # down to depth 1) has no summary at depth 0 and the page path
        # draws it; 1/0 (pages in the top only) keeps its summary
        nd, s_nd, _ = self._render(self.worker_nodepth, visible=[(2, 0)], depth=0)
        self.assertEqual((s_nd["layers"], s_nd["none"]), (0, "depth"), s_nd)
        self.assertEqual(nd, page0)
        _, s_nd1, _ = self._render(self.worker_nodepth, visible=[(1, 0)], depth=0)
        self.assertEqual((s_nd1["layers"], s_nd1["none"]), (1, "-"), s_nd1)
        # the kill switch: same pixels as a cache without the file
        off, s_off, _ = self._render(self.worker_off, visible=[(1, 0)])
        self.assertEqual(s_off["none"], "off")
        ovo = self.cache / "design.ovo"
        keep = ovo.read_bytes()
        try:
            ovo.unlink()
            nofile, s_no, _ = self._render(self.worker, visible=[(1, 0)])
            self.assertEqual(s_no["none"], "nofile")
            self.assertEqual(nofile, off)
            ovo.write_bytes(keep[:-9])
            _, s_bad, _ = self._render(self.worker, visible=[(1, 0)])
            self.assertEqual(s_bad["none"], "invalid")
        finally:
            ovo.write_bytes(keep)
        _, s_back, _ = self._render(self.worker, visible=[(1, 0)])
        self.assertEqual(s_back["none"], "-")

    def test_a_none_layer_keeps_the_page_path_in_its_own_slot(self):
        # 1/0 and 2/0 summarized, 3/0 (none:work) exact; 3/0 is later
        # in the paint order, so its solid square covers the lines
        lit, summ, _ = self._render(self.worker, visible=None)
        self.assertEqual(summ["layers"], 2)
        # a pixel with only the 3/0 square vs one where a 1/0 line
        # also lies: same colour (the square is on top)
        RenderTests.gen += 1
        gen = RenderTests.gen
        px = 200
        worker = self.worker
        worker.submit({"kind": "render", "gen": gen, "scope": "headless",
                       "bbox": (0.0, 0.0, 2000.0 * UM, 2000.0 * UM),
                       "view": None, "w": px, "h": px, "depth": None,
                       "cut_px": 1.0, "lod": False, "frames": False,
                       "labels": False, "abstract": False, "visible": None,
                       "frame_format": "raw", "thin": "keep"})
        while True:
            res = worker.res.get(timeout=60)
            if res.get("kind") == "frame" and res.get("gen") == gen \
                    and not res.get("refining"):
                break
        rgba = res["rgba"]

        def rgb(x, y):
            i = 4 * (y * px + x)
            return tuple(rgba[i:i + 3])
        only_square = rgb(70, 100)
        square_over_line = rgb(25, 173)
        self.assertNotEqual(only_square, (0, 0, 0))
        self.assertEqual(square_over_line, only_square)
        # a line outside the square shows the 1/0 colour
        line_only = rgb(185, 23)
        self.assertNotEqual(line_only, (0, 0, 0))
        self.assertNotEqual(line_only, only_square)

    def test_the_summary_stays_within_one_pixel_of_exact_in_every_direction(self):
        # review 2026-09-11 (2nd) P1-3: the shape -> cell and cell ->
        # pixel projections each widen by up to a pixel, so "2 px empty
        # runs survive" was false. The contract that holds (plan §3):
        # every summary pixel lies within the 8-neighbourhood of an
        # exact pixel - an empty region keeps every pixel two or more
        # pixels away from exact geometry, a gap of g px keeps g - 2,
        # 3 px gaps always keep one. Checked pixel by pixel on a gap
        # fixture at four pan phases (the grid origin shifts against
        # the pixels with the view).
        src = TMP / "gaps.oas"
        write_gaps(src)
        cache = index_with_occupancy(src, 4)
        os.environ.pop("FLOE_RUST_OCCUPANCY", None)
        sys.path.insert(0, str(ROOT))
        from floe.cache import Cache
        from floe.rust_render import RustRenderWorker
        c = Cache(str(src))
        c.load()
        worker = RustRenderWorker(c)
        worker.start()
        try:
            widened = 0
            for shift in (0.0, 2.5, 5.0, 7.5):
                bbox = tuple(float(v * UM) for v in
                             (shift, shift, 2000 + shift, 2000 + shift))
                RenderTests.gen += 1
                exact, sres = render_settled(worker, RenderTests.gen, bbox,
                                             200, cut_px=0.0,
                                             visible=[(1, 0)])
                self.assertEqual(sres["summary"]["none"], "exact")
                RenderTests.gen += 1
                summ, res = render_settled(worker, RenderTests.gen, bbox,
                                           200, visible=[(1, 0)])
                self.assertEqual(res["summary"]["layers"], 1)
                far = sorted(p for p in summ if not near(p, exact, 1))
                self.assertEqual(far[:8], [], "shift %s: %d summary pixels "
                                 "farther than 1 px from exact" % (shift, len(far)))
                missed = sorted(p for p in exact if not near(p, summ, 1))
                self.assertEqual(missed[:8], [], "shift %s: %d exact pixels "
                                 "without a summary pixel within 1 px" % (shift, len(missed)))
                widened += len(summ - exact)
                if shift == 0.0:
                    # the 3 px gap between the first row's boxes at x
                    # 320..350 um keeps its middle pixel (33) clear
                    self.assertTrue((32, 180) in exact or (31, 180) in exact)
                    self.assertNotIn((33, 180), summ)
                    self.assertNotIn((33, 180), exact)
            self.assertGreater(widened, 0)
        finally:
            worker.stop()

    def test_the_mask_is_styled_boundary_solid_and_interior_by_the_fill(self):
        # the viewer's speckle fill: interior pixels follow the
        # checkerboard, a lit pixel with an unlit 4-neighbour is solid;
        # the stroke width does not widen the boundary (fixed 1 px)
        ovo = read_ovo(self.cache / "design.ovo")
        for width_px in (1, 3):
            lit, summ, bbox = self._render(self.worker, visible=[(2, 0)],
                                           fill="speckle", width_px=width_px)
            self.assertEqual(summ["layers"], 1)
            mask = expected_mask(ovo_level(ovo, (2, 0), 1), bbox, 200, 200,
                                 halo=1)
            expect = set()
            for (x, y) in mask:
                if not (0 <= x < 200 and 0 <= y < 200):
                    continue
                boundary = any((x + dx, y + dy) not in mask
                               for dx, dy in ((-1, 0), (1, 0), (0, -1), (0, 1)))
                if boundary or (x + y) % 2 == 0:
                    expect.add((x, y))
            self.assertEqual(lit, expect, "stroke width %d" % width_px)
            # the block's interior really is speckled (not solid)
            inside = {(x, y) for x in range(125, 135) for y in range(65, 75)}
            self.assertEqual(len(inside & lit), 50)

    def test_a_rebuilt_file_reaches_a_running_daemon(self):
        _, summ, _ = self._render(self.worker, visible=[(1, 0)])
        self.assertEqual(summ["cell_um"], 8.0)
        floe_index("vfs", self.src, self.cache, "--occupancy-only",
                   "--occupancy-um", "3", "--occupancy-max-work", "100000")
        # the same view again: no stale retained frame, the new file
        _, summ, _ = self._render(self.worker, visible=[(1, 0)])
        self.assertEqual((summ["level"], summ["cell_um"]), (1, 6.0), summ)
        floe_index("vfs", self.src, self.cache, "--occupancy-only",
                   "--occupancy-um", "4", "--occupancy-max-work", "100000")
        _, summ, _ = self._render(self.worker, visible=[(1, 0)])
        self.assertEqual(summ["cell_um"], 8.0)



WIDE_DECK = """SLICE 1,17
RETICLE
* wide.jb
OPTION PA, AA=0.0200, BA=0.002000, SA=80
MTITLE 1,THIN
*PLACE-INFO
*
CHIP ID001, * MAIN 1.0000
*
$ (1, THIN, AD=0.00020, SF=1, TC=thinwide.oas, LY={1}, DT={0}, BX=0.0, BY=0.0, UX=2000.0, UY=2000.0 )
ROWS 100.0/100.0
*END-PLACE
END
"""


def render_settled(worker, gen, bbox_dbu, px, cut_px=1.0, thin="keep",
                   visible=None, depth=None, frames=False):
    solid = "\n".join(["*" * 16] * 16)
    keys = [(int(l["layer"]), int(l["datatype"]))
            for l in worker.cache.meta["layers"]]
    worker.submit({"kind": "repattern", "fills": [(k, solid) for k in keys],
                   "widths": [(k, 1) for k in keys]})
    worker.submit({"kind": "render", "gen": gen, "scope": "headless",
                   "bbox": tuple(float(v) for v in bbox_dbu), "view": None,
                   "w": px, "h": px, "depth": depth, "cut_px": cut_px,
                   "lod": False, "frames": frames, "labels": False,
                   "abstract": False, "visible": visible,
                   "frame_format": "raw", "thin": thin})
    while True:
        res = worker.res.get(timeout=60)
        if res.get("kind") == "error":
            raise AssertionError("renderd: %s" % res.get("msg"))
        if res.get("kind") != "frame" or res.get("gen") != gen:
            continue
        if res.get("refining"):
            continue
        return lit_pixels(res["rgba"], px, px), res


def write_subcut(path):
    """a design layout whose sub-cut pages must not vanish (field
    2026-09-16: a 9.8 GB design layout showed far less than Calibre at
    detail high). 4/0: a 200 x 200 array of 0.2 um boxes on a 1 um
    pitch over 200..400 um, written as one repetition record (a dense
    page every shape of which is below a 1 px cut at 10 um/px); 7/0:
    the same array as placements of a DOT cell; 5/0: four 0.2 um boxes
    on a diagonal 120 um apart from (1500, 1400) um (a sparse page:
    four pixels in a 36 x 36 px footprint, under the 1/256 wash rule);
    8/0: 200 hairlines 0.1 x 190 um on a 1 um pitch over 200..400 um
    (a dense hairline page: washed under cull); 9/0: three such lines
    400 um apart (3.75 % of their footprint, under the 1/8 hairline
    rule: kept and drawn as lines); 10/0: 40,000 plain placements of
    a 0.2 um DOT2 cell at jittered positions over 1000..1200 um (no
    array the writer could fold: a child BVH of thousands of leaves,
    for the page frontier's node dots); 6/0: a frame so the top spans
    0..2000 um"""
    ly = db.Layout()
    ly.dbu = 0.001
    top = ly.create_cell("SUBCUT")
    l4, l5, l6, l7 = ly.layer(4, 0), ly.layer(5, 0), ly.layer(6, 0), ly.layer(7, 0)
    for j in range(200):
        for i in range(200):
            x, y = 200 * UM + i * UM, 200 * UM + j * UM
            top.shapes(l4).insert(db.Box(x, y, x + 200, y + 200))
    dot = ly.create_cell("DOT")
    dot.shapes(l7).insert(db.Box(0, 0, 200, 200))
    top.insert(db.CellInstArray(dot.cell_index(),
                                db.Trans(db.Vector(600 * UM, 200 * UM)),
                                db.Vector(UM, 0), db.Vector(0, UM), 200, 200))
    for k in range(4):
        x, y = 1500 * UM + k * 120 * UM, 1400 * UM + k * 120 * UM
        top.shapes(l5).insert(db.Box(x, y, x + 200, y + 200))
    l8, l9 = ly.layer(8, 0), ly.layer(9, 0)
    for i in range(200):
        x = 200 * UM + i * UM
        top.shapes(l8).insert(db.Box(x, 1600 * UM, x + 100, 1790 * UM))
    for k in range(3):
        x = 1000 * UM + k * 400 * UM
        top.shapes(l9).insert(db.Box(x, 1500 * UM, x + 100, 1690 * UM))
    l10 = ly.layer(10, 0)
    dot2 = ly.create_cell("DOT2")
    dot2.shapes(l10).insert(db.Box(0, 0, 200, 200))
    for j in range(200):
        for i in range(200):
            x = 1000 * UM + i * UM + (i * 7 + j * 3) % 5 * 100
            y = 1000 * UM + j * UM + (i * 3 + j * 11) % 7 * 100
            top.insert(db.CellInstArray(dot2.cell_index(),
                                        db.Trans(db.Vector(x, y))))
    top.shapes(l6).insert(db.Box(0, 0, 2000 * UM, 2000 * UM))
    opt = db.SaveLayoutOptions()
    opt.format = "OASIS"
    opt.oasis_compression_level = 10
    opt.oasis_recompress = True
    ly.write(str(path), opt)
    ly._destroy()


def write_giant(path):
    """record repetitions too big for one marking unit (2026-09-16):
    1/0 a 300 x 300 array of 0.2 um boxes on a 2 um pitch, written as
    one repetition record (compression); 2/0 the same array as the
    record of a cell placed as a 2 x 2 array (rotated); 3/0 a 100 x
    100 array of triangles"""
    ly = db.Layout()
    ly.dbu = 0.001
    top = ly.create_cell("GIANT")
    l1, l2, l3 = ly.layer(1, 0), ly.layer(2, 0), ly.layer(3, 0)
    for j in range(300):
        for i in range(300):
            x, y = i * 2 * UM, j * 2 * UM
            top.shapes(l1).insert(db.Box(x, y, x + 200, y + 200))
    arr = ly.create_cell("ARR")
    for j in range(200):
        for i in range(200):
            x, y = i * 2 * UM, j * 2 * UM
            arr.shapes(l2).insert(db.Box(x, y, x + 200, y + 200))
    top.insert(db.CellInstArray(arr.cell_index(),
                                db.Trans(1, False, db.Vector(700 * UM, 0)),
                                db.Vector(500 * UM, 0), db.Vector(0, 500 * UM),
                                2, 2))
    for j in range(100):
        for i in range(100):
            x, y = 1300 * UM + i * 2 * UM, 700 * UM + j * 2 * UM
            top.shapes(l3).insert(db.Polygon([db.Point(x, y), db.Point(x + 200, y),
                                              db.Point(x, y + 200)]))
    opt = db.SaveLayoutOptions()
    opt.format = "OASIS"
    opt.oasis_compression_level = 10
    ly.write(str(path), opt)


def write_frontier(path):
    """the page frontier's fixture: 1,210,000 hairlines 1 um wide and
    40..41 um tall (1,000 distinct heights, written uncompressed, so
    every line is its own record) on a 1.8 um lattice over 0..2000 um
    on 1/0 - about 19 MB of records, so the layer spreads over 19 or
    so 1 MB pages and a page BVH; on 2/0 an L of two hairlines - a
    1 x 1000 um and a 1000 x 1 um line sharing a corner - whose page
    bbox is 1000 x 1000 um (review 2026-09-17: the old ink estimate
    members x max_w x max_h called this 200 % dense and washed the
    whole square); on 3/0 a sparser field of the same lines on an
    18 um lattice (12,100 lines in one page, for the record thinning
    of the raster to show as a density); a frame on 6/0 spans the top"""
    ly = db.Layout()
    ly.dbu = 0.001
    top = ly.create_cell("FRONTIER")
    l1, l6 = ly.layer(1, 0), ly.layer(6, 0)
    shapes = top.shapes(l1)
    pitch = 1800
    for j in range(1100):
        y = j * pitch
        for i in range(1100):
            x = i * pitch
            h = 40 * UM + ((i * 31 + j * 17) % 1000)
            shapes.insert(db.Box(x, y, x + UM, y + h))
    l2 = ly.layer(2, 0)
    top.shapes(l2).insert(db.Box(100 * UM, 500 * UM, 1100 * UM, 501 * UM))
    top.shapes(l2).insert(db.Box(100 * UM, 500 * UM, 101 * UM, 1500 * UM))
    l3 = ly.layer(3, 0)
    for j in range(110):
        for i in range(110):
            x, y = i * 18 * UM, j * 18 * UM
            top.shapes(l3).insert(db.Box(x, y, x + UM, y + 40 * UM + (i * 31 + j * 17) % 1000))
    top.shapes(l6).insert(db.Box(0, 0, 2000 * UM, 2000 * UM))
    opt = db.SaveLayoutOptions()
    opt.format = "OASIS"
    opt.oasis_compression_level = 0
    ly.write(str(path), opt)


class PageFrontierTests(unittest.TestCase):
    """The page frontier (user design 2026-09-17): what the cut drops
    is thinned to representatives - a cut page k octaves below its cut
    survives when its index in its run is a multiple of 4^k - so the
    count in view stays what it was at the cut and the survivors are
    nested across zooms. Deactivated in the viewer since 2026-09-17
    (the answer moves to representative data built at index time);
    FLOE_RUST_PAGE_REPS=on switches the planner-side frontier on."""

    gen = 900

    @classmethod
    def setUpClass(cls):
        cls.src = TMP / "frontier.oas"
        write_frontier(cls.src)
        cls.cache = Path(vfs_cache_dir(cls.src))
        floe_index("vfs", cls.src, cls.cache, "--no-lod", "--slow-cell-s",
                   "999", "--jobs", "2")
        os.environ["FLOE_RENDERD_BIN"] = str(
            ROOT / "rust" / "target" / "release" / "floe-renderd")
        sys.path.insert(0, str(ROOT))
        # the page frontier is opt-in (deactivated 2026-09-17): the
        # worker under test switches it on, worker_off is the default
        os.environ["FLOE_RUST_PAGE_REPS"] = "on"
        cls.worker = SubCutTests._worker.__func__(cls)
        del os.environ["FLOE_RUST_PAGE_REPS"]
        cls.worker_off = SubCutTests._worker.__func__(cls)

    @classmethod
    def tearDownClass(cls):
        for w in (cls.worker, cls.worker_off):
            try:
                w.stop()
            except Exception:
                pass

    def _frame(self, worker, px, layer=(1, 0)):
        PageFrontierTests.gen += 1
        box = (0.0, 0.0, 2000.0 * UM, 2000.0 * UM)
        return render_settled(worker, PageFrontierTests.gen, box, px,
                              cut_px=1.0, thin="cull", visible=[layer])

    def _plan(self, px_per_um, view="0,0,2000,2000", layer="1/0", env=None):
        """(representative page ids, plan stats) of a plan with the
        page frontier on, from `floe-index plan --explain 1`"""
        res = floe_index("plan", self.cache, "--view", view,
                         "--px-per-um", px_per_um, "--cut-px", "1",
                         "--page-hairline", "1", "--page-reps", "1",
                         "--layers", layer, "--explain", "1", env=env)
        rows = [l.split("\t") for l in res.stdout.splitlines()
                if l.startswith("explain\t")]
        reps = {int(r[5]) for r in rows
                if r[1] == "page" and r[2] in ("rep_keep", "rep_wash")}
        stats = {k: int(v) for k, v in
                 re.findall(r'"(\w+)": (\d+)', res.stdout)}
        return reps, stats

    def test_every_cut_page_in_view_is_kept_and_levels_follow_the_screen_density(self):
        # the 1 um lines are hairline-cut once 1 um < 0.5 px. Every cut
        # page in view is a representative at every zoom (the run's
        # ~8.5 MB decoded is within the decode budget: page level 0);
        # each page's level is its ink against its box on screen, so it
        # climbs as the view widens and the survivors thin to the
        # density's dot pattern
        s200, st200 = self._plan(0.1)
        s400, st400 = self._plan(0.2)
        s800, st800 = self._plan(0.4)
        n = len(s800)
        self.assertGreaterEqual(n, 16, n)
        lo = min(s800)
        self.assertEqual(s800, set(range(lo, lo + n)), "one contiguous run")
        self.assertEqual(s400, s800)
        self.assertEqual(s200, s800)
        for st in (st200, st400, st800):
            self.assertEqual(st["rep_page_level"], 0, st)
            self.assertEqual(st["rep_replans"], 0, st)
            self.assertEqual(st["rep_pages_kept"], n, st)
            self.assertEqual(st["rep_items"], 1_210_000, st)
            self.assertGreater(st["rep_decode_bytes"], n * 100_000, st)
        self.assertGreater(st200["rep_level"], 0, st200)
        self.assertGreaterEqual(st200["rep_level"], st400["rep_level"], (st200, st400))
        self.assertGreaterEqual(st400["rep_level"], st800["rep_level"], (st400, st800))
        # the frames: the sparse layer (12,100 lines in one page)
        # lights every quadrant at every zoom at about the same pixel
        # density - the survivors halve as the pixels do - and never
        # solid; the kill switch shows the old cull
        dens = {}
        for px in (200, 400, 800):
            lit, res = self._frame(self.worker, px, (3, 0))
            culls = res["plan_culls"]
            self.assertEqual(culls["rep_kept"], 1, (px, culls))
            self.assertEqual(culls["rep_page_level"], 0, (px, culls))
            half = px // 2
            for qx, qy in ((0, 0), (1, 0), (0, 1), (1, 1)):
                quad = sum(1 for x, y in lit
                           if (x >= half) == bool(qx) and (y >= half) == bool(qy))
                self.assertGreater(quad, 0, (px, qx, qy))
            dens[px] = len(lit) / (px * px)
            self.assertLess(dens[px], 0.5, (px, dens[px]))
            gone, res = self._frame(self.worker_off, px, (3, 0))
            self.assertEqual(gone, set(), px)
            self.assertEqual(res["plan_culls"]["rep_kept"], 0)
        lo_d, hi_d = min(dens.values()), max(dens.values())
        self.assertLess(hi_d, lo_d * 3.0, dens)
        # the dense layer: every page kept, thinned inside, a dot field
        # that is neither empty nor solid
        lit_thin, res = self._frame(self.worker, 800, (1, 0))
        self.assertGreaterEqual(res["plan_culls"]["rep_level"], 1, res["plan_culls"])
        self.assertEqual(res["plan_culls"]["rep_kept"], n, res["plan_culls"])
        self.assertTrue(30_000 < len(lit_thin) < 500_000, len(lit_thin))

    def test_a_lower_density_raises_the_level_and_thins_the_records(self):
        # FLOE_RUST_REP_DENSITY=0.03125 (an eighth of the default): every
        # level climbs by three, the frame lights about an eighth of the
        # pixels the default does, and what it draws the default drew
        _, base = self._plan(0.4, layer="3/0")
        _, low = self._plan(0.4, layer="3/0", env=dict(os.environ, FLOE_RUST_REP_DENSITY="0.03125"))
        self.assertEqual(low["rep_level"], base["rep_level"] + 3, (base, low))
        os.environ["FLOE_RUST_REP_DENSITY"] = "0.03125"
        os.environ["FLOE_RUST_PAGE_REPS"] = "on"
        try:
            tight = SubCutTests._worker.__func__(self)
        finally:
            del os.environ["FLOE_RUST_REP_DENSITY"]
            del os.environ["FLOE_RUST_PAGE_REPS"]
        try:
            lit_tight, res = self._frame(tight, 800, (3, 0))
        finally:
            tight.stop()
        self.assertEqual(res["plan_culls"]["rep_level"], base["rep_level"] + 3, res["plan_culls"])
        lit_all, _ = self._frame(self.worker, 800, (3, 0))
        self.assertTrue(len(lit_all) / 16 <= len(lit_tight) <= len(lit_all) / 3,
                        (len(lit_tight), len(lit_all)))
        self.assertTrue(lit_tight <= lit_all, len(lit_tight - lit_all))

    def test_the_decode_budget_thins_the_pages_by_index_and_replans(self):
        # FLOE_RUST_REP_DECODE_MB=1 against the run's ~8.5 MB decoded
        # (32 pages of ~265 KB): the plan is redone with the pages one
        # in 2^Lp by index, Lp the smallest that fits (16), so the kept
        # set is the run's every 16th page from its first, and the leaf
        # runs holding none are pruned
        s_all, _ = self._plan(0.1)
        n, lo = len(s_all), min(s_all)
        s, st = self._plan(0.1, env=dict(os.environ, FLOE_RUST_REP_DECODE_MB="1"))
        self.assertEqual(st["rep_replans"], 1, st)
        level = st["rep_page_level"]
        self.assertGreaterEqual(level, 4, st)
        self.assertEqual(s, {lo + i for i in range(0, n, 1 << level)}, (level, sorted(s)))
        self.assertEqual(st["rep_pages_kept"], len(s), st)
        self.assertLessEqual(st["rep_decode_bytes"], 1 << 20, st)
        self.assertGreaterEqual(st["rep_pruned"], 1, st)
        self.assertLess(st["page_candidates"], n, st)

    def test_an_l_of_two_hairlines_is_drawn_as_lines_not_as_its_square(self):
        # review 2026-09-17: a representative is drawn, never washed -
        # the L page (2 records, within the budget: level 0) shows its
        # two 100 px lines, never the 100 x 100 px square
        lit, res = self._frame(self.worker, 200, (2, 0))
        culls = res["plan_culls"]
        self.assertEqual((culls["rep_kept"], culls["rep_washed"]), (1, 0), culls)
        self.assertTrue(150 <= len(lit) <= 450, len(lit))
        xs = sorted({x for x, _ in lit})
        ys = sorted({y for _, y in lit})
        self.assertGreaterEqual(max(sum(1 for x, _ in lit if x == c) for c in xs), 80)
        self.assertGreaterEqual(max(sum(1 for _, y in lit if y == r) for r in ys), 80)
        reps, _ = self._plan(0.1, layer="2/0")
        self.assertEqual(len(reps), 1)


class GiantRepetitionTests(unittest.TestCase):
    """2026-09-16: a record's own repetition is marked by member-range
    units under the balanced split, so a giant array record no longer
    runs on one thread; the file is byte-identical across thread
    counts and with the count-based split (`--occupancy-balance 0`,
    the kill switch, also reachable through `floe2 index`)."""

    @classmethod
    def setUpClass(cls):
        cls.src = TMP / "giant.oas"
        write_giant(cls.src)

    def test_thread_counts_and_the_kill_switch_write_the_same_file(self):
        shas = {}
        for tag, extra in (("j1", ["--jobs", 1]), ("j4", ["--jobs", 4]),
                           ("count", ["--jobs", 4, "--occupancy-balance", 0])):
            out = TMP / ("giant_%s.floe" % tag)
            shutil.rmtree(out, ignore_errors=True)
            res = floe_index("vfs", self.src, out, "--occupancy",
                             "--occupancy-um", 1, "--no-lod", "--slow-cell-s",
                             "999", *extra)
            self.assertIn(" ok=3 ", res.stderr)
            shas[tag] = sha(out / "design.ovo")
        self.assertEqual(len(set(shas.values())), 1, shas)
        # floe2 index passes the switch through to floe-index
        cache = Path(vfs_cache_dir(self.src))
        shutil.rmtree(cache, ignore_errors=True)
        floe2("index", self.src, "--occupancy", "--occupancy-um", "1",
              "--occupancy-balance", "0", "--jobs", "2")
        self.assertEqual(sha(cache / "design.ovo"), shas["j1"])


class SubCutTests(unittest.TestCase):
    """The sub-cut rules of a plain layout (2026-09-16): a dense page
    whose shapes are all below the cut is a footprint wash, a sparse
    one is drawn as pixels, on both thin policies. As a blanket rule
    they are OFF (user decision of the same day: slower mid-zoom draws,
    still not everything visible), FLOE_RUST_SUB_CUT_WASH=on being the
    diagnostic; since 2026-09-17 the page frontier applies them to
    REPRESENTATIVES only - one page in 4^k, k octaves below the cut -
    which for this fixture's single-page layers (index 0 of every run)
    means the same picture. Both are opt-in since 2026-09-17: the
    default worker is the plain cull, FLOE_RUST_PAGE_REPS=on the
    frontier."""

    gen = 500

    @classmethod
    def setUpClass(cls):
        cls.src = TMP / "subcut.oas"
        write_subcut(cls.src)
        cls.cache = Path(vfs_cache_dir(cls.src))
        floe_index("vfs", cls.src, cls.cache, "--no-lod", "--slow-cell-s",
                   "999", "--jobs", "2")
        os.environ["FLOE_RENDERD_BIN"] = str(
            ROOT / "rust" / "target" / "release" / "floe-renderd")
        sys.path.insert(0, str(ROOT))
        os.environ.pop("FLOE_RUST_SUB_CUT_WASH", None)
        os.environ.pop("FLOE_RUST_PAGE_REPS", None)
        # the default worker: the plain cull (both the sub-cut rules
        # and the page frontier are off); worker_reps switches the
        # frontier on, worker_on the sub-cut rules
        cls.worker = cls._worker()
        os.environ["FLOE_RUST_PAGE_REPS"] = "on"
        cls.worker_reps = cls._worker()
        del os.environ["FLOE_RUST_PAGE_REPS"]
        os.environ["FLOE_RUST_SUB_CUT_WASH"] = "on"
        cls.worker_on = cls._worker()
        # tight per-plan budgets: 10 px of sparse ink, 100 px of wash
        os.environ["FLOE_RUST_SUB_CUT_SPARSE_MPX"] = "0.00001"
        os.environ["FLOE_RUST_SUB_CUT_WASH_MPX"] = "0.0001"
        cls.worker_tight = cls._worker()
        del os.environ["FLOE_RUST_SUB_CUT_SPARSE_MPX"]
        del os.environ["FLOE_RUST_SUB_CUT_WASH_MPX"]
        del os.environ["FLOE_RUST_SUB_CUT_WASH"]

    @classmethod
    def tearDownClass(cls):
        for w in (cls.worker, cls.worker_reps, cls.worker_on, cls.worker_tight):
            try:
                w.stop()
            except Exception:
                pass

    @classmethod
    def _worker(cls):
        from floe.cache import Cache
        from floe.rust_render import RustRenderWorker
        cache = Cache(str(cls.src))
        cache.load()
        worker = RustRenderWorker(cache)
        worker.start()
        return worker

    def _lit(self, worker, layer, thin="cull"):
        return self._frame(worker, layer, thin)[0]

    def _frame(self, worker, layer, thin="cull"):
        SubCutTests.gen += 1
        box = (0.0, 0.0, 2000.0 * UM, 2000.0 * UM)
        return render_settled(worker, SubCutTests.gen, box, 200,
                              cut_px=1.0, thin=thin, visible=[layer])

    def test_the_frontier_switched_on_keeps_representatives_and_the_default_drops_all(self):
        # every layer here is one page (or one placement array) - the
        # first of its run, a representative at any zoom - so the
        # frontier (FLOE_RUST_PAGE_REPS=on) draws what the sub-cut rules
        # draw thinned to the density and counts it as reps, not as
        # sub-cut verdicts; the default is the pre-2026-09-16 cull:
        # nothing lit, nothing counted
        placed = {(x, y) for x in range(58, 82) for y in range(158, 182)}
        for layer in ((4, 0), (5, 0), (7, 0), (8, 0), (9, 0)):
            lit, res = self._frame(self.worker_reps, layer)
            lit_on, _ = self._frame(self.worker_on, layer)
            self.assertTrue(lit, layer)
            # a representative is geometry thinned to the screen density:
            # the dense array page, the placed array and the dense line
            # page thin to a dot / line pattern inside the block the
            # diagnostic rules wash; the sparse page and the three
            # lines are within the density and draw whole
            self.assertTrue(lit <= lit_on, (layer, sorted(lit - lit_on)[:10]))
            if layer in ((4, 0), (7, 0), (8, 0)):
                self.assertLess(len(lit), len(lit_on) * 3 // 4, (layer, len(lit), len(lit_on)))
                self.assertGreaterEqual(res["plan_culls"]["rep_level"], 1, (layer, res["plan_culls"]))
            else:
                self.assertEqual(lit, lit_on, layer)
                self.assertEqual(res["plan_culls"]["rep_level"], 0, (layer, res["plan_culls"]))
            if layer == (7, 0):
                # the placed array: a dot per kept member on the child's
                # layer, no page of the child decoded, nothing walked
                # below the placement
                self.assertGreaterEqual(res["plan_culls"]["rep_children"], 1, res["plan_culls"])
                self.assertEqual(res["tiles"], 0, res)  # plan_pages on the wire
                self.assertEqual(res["plan_culls"]["rep_kept"], 0, res["plan_culls"])
            if layer == (7, 0):
                self.assertTrue(lit <= placed, sorted(lit - placed)[:10])
            culls = res["plan_culls"]
            self.assertGreaterEqual(culls["rep_kept"] + culls["rep_washed"]
                                    + culls["rep_children"], 1, (layer, culls))
            self.assertEqual((culls["sub_cut_washes"], culls["sub_cut_sparse"]),
                             (0, 0), (layer, culls))
            gone, res = self._frame(self.worker, layer)
            self.assertEqual(gone, set(), layer)
            culls = res["plan_culls"]
            self.assertEqual((culls["rep_kept"], culls["rep_washed"], culls["rep_children"],
                              culls["sub_cut_washes"], culls["sub_cut_sparse"]),
                             (0, 0, 0, 0, 0), (layer, culls))
        for layer in ((4, 0), (5, 0), (7, 0)):
            self.assertEqual(self._lit(self.worker, layer, "keep"), set(), layer)

    def test_a_field_of_plain_cut_placements_is_node_dots_and_a_tiny_page_is_not_a_square(self):
        # 10/0: 40,000 plain placements of a 0.2 um cell over a 200 um
        # square (20 x 20 px at 10 um/px): cut subtrees within the
        # density's 2 px pitch become one pixel each at their centre,
        # at most one per 2 x 2 px lattice cell - up to a quarter of the
        # block, never solid, no placement below them visited, nothing
        # decoded, and only on the layer the dotted child holds (the
        # 4/0 and 9/0 frames above saw no dot from this field); the
        # kill switch culls
        block = {(x, y) for x in range(98, 122) for y in range(78, 102)}
        lit, res = self._frame(self.worker_reps, (10, 0))
        self.assertTrue(lit <= block, sorted(lit - block)[:10])
        self.assertTrue(40 <= len(lit) <= 150, len(lit))
        self.assertEqual(res["tiles"], 0, res)
        self.assertGreaterEqual(res["plan_culls"]["rep_children"], 40, res["plan_culls"])
        res = floe_index("plan", self.cache, "--view", "0,0,2000,2000",
                         "--px-per-um", "0.1", "--cut-px", "1",
                         "--page-hairline", "1", "--page-reps", "1",
                         "--layers", "10/0")
        stats = {k: int(v) for k, v in re.findall(r'"(\w+)": (\d+)', res.stdout)}
        # (the OASIS writer folded the scattered instances into point
        # sets whose boxes exceed the pitch, so here the lattice caps
        # the member dots; a compact subtree ends in one node dot)
        self.assertGreaterEqual(stats["rep_dots"] + stats["rep_node_dots"], 40, stats)
        self.assertLessEqual(stats["rep_dots"] + stats["rep_node_dots"], 150, stats)
        self.assertLess(stats["visited_bvh"], 40_000, stats)
        self.assertEqual(self._lit(self.worker, (10, 0)), set())
        # a representative page that is one or two pixels on screen is
        # drawn thinned (one dot), never as the M7-C bbox square:
        # 4/0's 200 um array page at 100 um/px
        SubCutTests.gen += 1
        box = (0.0, 0.0, 2000.0 * UM, 2000.0 * UM)
        lit, res = render_settled(self.worker_reps, SubCutTests.gen, box, 20,
                                  cut_px=1.0, thin="cull", visible=[(4, 0)])
        # (its two kept members straddle pixel edges: up to the page's
        # 2 x 2 px, but as geometry - no M7-C wash, one representative)
        self.assertTrue(1 <= len(lit) <= 4, sorted(lit))
        self.assertEqual(res["plan_culls"]["washed"], 0, res["plan_culls"])
        self.assertEqual(res["plan_culls"]["rep_kept"], 1, res["plan_culls"])
        SubCutTests.gen += 1
        gone, _ = render_settled(self.worker, SubCutTests.gen, box, 20,
                                 cut_px=1.0, thin="cull", visible=[(4, 0)])
        self.assertEqual(gone, set())

    def test_dense_sub_cut_pages_are_washed_and_sparse_ones_drawn(self):
        # with the rules on: 10 um/px, cut 1 px = 10 um, every 0.2 um
        # box is below the cut
        block = {(x, y) for x in range(20, 40) for y in range(160, 180)}
        placed = {(x, y) for x in range(60, 80) for y in range(160, 180)}
        for thin in ("cull", "keep"):
            lit4 = self._lit(self.worker_on, (4, 0), thin)
            self.assertGreaterEqual(len(lit4 & block), 300, (thin, len(lit4)))
            self.assertLessEqual(len(lit4 - block), 90, (thin, sorted(lit4 - block)[:10]))
            lit7 = self._lit(self.worker_on, (7, 0), thin)
            self.assertGreaterEqual(len(lit7 & placed), 300, (thin, len(lit7)))
            self.assertLessEqual(len(lit7 - placed), 90, (thin, sorted(lit7 - placed)[:10]))
            # the sparse page: a few pixels at the boxes, nothing else
            # (a footprint wash would light a 36 x 36 block)
            lit5 = self._lit(self.worker_on, (5, 0), thin)
            self.assertTrue(2 <= len(lit5) <= 12, (thin, sorted(lit5)))
            spots = [(150 + 12 * k, 60 - 12 * k) for k in range(4)]
            self.assertTrue(all(any(abs(x - sx) <= 2 and abs(y - sy) <= 2
                                    for sx, sy in spots) for x, y in lit5),
                            (thin, sorted(lit5)))
        # the perf counters name the verdicts (the field reads the
        # cost of a slow mid-zoom draw from them, 2026-09-16)
        _, dense_res = self._frame(self.worker_on, (4, 0))
        self.assertGreaterEqual(dense_res["plan_culls"]["sub_cut_washes"], 1, dense_res["plan_culls"])
        _, sparse_res = self._frame(self.worker_on, (5, 0))
        self.assertGreaterEqual(sparse_res["plan_culls"]["sub_cut_sparse"], 1, sparse_res["plan_culls"])

    def test_hairline_pages_under_cull_are_washed_when_dense_and_kept_when_sparse(self):
        # with the rules on: 10 um/px, cut 1 px, the 0.1 um lines are
        # hairline-cut (max_min 0.1 um < 5 um). Under cull the dense
        # page (200 lines) is a footprint block, the sparse one (3
        # lines, 3.75 %) draws its lines; under keep both draw exactly
        block = {(x, y) for x in range(20, 40) for y in range(21, 40)}
        dense = self._lit(self.worker_on, (8, 0), "cull")
        self.assertGreaterEqual(len(dense & block), 250, len(dense))
        self.assertLessEqual(len(dense - block), 90, sorted(dense - block)[:10])
        sparse = self._lit(self.worker_on, (9, 0), "cull")
        self.assertTrue(30 <= len(sparse) <= 90, sorted(sparse))
        self.assertTrue(all(any(abs(x - cx) <= 1 for cx in (100, 140, 180))
                            and 30 <= y <= 51 for x, y in sparse), sorted(sparse))
        keep_sparse = self._lit(self.worker_on, (9, 0), "keep")
        self.assertEqual(keep_sparse, sparse)
        keep_dense = self._lit(self.worker_on, (8, 0), "keep")
        self.assertGreaterEqual(len(keep_dense & block), 250, len(keep_dense))
        # keep never culls a hairline page: the default worker draws
        # both exactly as well
        self.assertEqual(self._lit(self.worker, (9, 0), "keep"), sparse)
        self.assertGreaterEqual(len(self._lit(self.worker, (8, 0), "keep") & block), 250)

    def test_the_per_plan_budgets_bound_what_the_sub_cut_rules_add(self):
        # field 2026-09-16: a 150 MB chip at thin:cull detail medium
        # drew over 6 s at mid zoom with the rules on. The tight worker
        # has 10 px of sparse ink and 100 px of wash area per plan: the
        # 4-box page (4 px of ink) still fits and is kept; the 3-line
        # page (3 x 21 px) and every 20 x 20 px footprint wash exceed
        # their budget and are dropped as the cull always did - never
        # washed - and counted; the default budgets drop nothing here
        lit5, res5 = self._frame(self.worker_tight, (5, 0))
        self.assertEqual(lit5, self._lit(self.worker_on, (5, 0)))
        self.assertEqual(res5["plan_culls"]["sub_cut_sparse_over"], 0, res5["plan_culls"])
        lit9, res9 = self._frame(self.worker_tight, (9, 0))
        self.assertEqual(lit9, set())
        self.assertGreaterEqual(res9["plan_culls"]["sub_cut_sparse_over"], 1, res9["plan_culls"])
        self.assertEqual(res9["plan_culls"]["sub_cut_washes"], 0, res9["plan_culls"])
        for layer in ((4, 0), (7, 0), (8, 0)):
            lit, res = self._frame(self.worker_tight, layer)
            self.assertEqual(lit, set(), layer)
            self.assertGreaterEqual(res["plan_culls"]["sub_cut_wash_over"], 1, (layer, res["plan_culls"]))
            self.assertEqual(res["plan_culls"]["sub_cut_washes"], 0, (layer, res["plan_culls"]))
        for layer in ((4, 0), (5, 0), (7, 0), (8, 0), (9, 0)):
            _, res = self._frame(self.worker_on, layer)
            self.assertEqual((res["plan_culls"]["sub_cut_sparse_over"],
                              res["plan_culls"]["sub_cut_wash_over"]), (0, 0),
                             (layer, res["plan_culls"]))


class DeckRenderTests(unittest.TestCase):
    """M4 (gate 4): a deck pass under the keep policy draws its
    source's summary on the source view, composites in pass order,
    counts its passes and cells on the frame line; the kill switch and
    a limited depth take the page path; frames still come."""

    gen = 500

    @classmethod
    def setUpClass(cls):
        cls.dir = TMP / "deckwide"
        cls.dir.mkdir()
        write_thinwide(cls.dir / "thinwide.oas")
        (cls.dir / "wide.jb").write_text(WIDE_DECK)
        floe2("index", cls.dir / "wide.jb", "--occupancy", "--occupancy-um",
              "4", "--jobs", "2")
        os.environ["FLOE_RENDERD_BIN"] = str(
            ROOT / "rust" / "target" / "release" / "floe-renderd")
        sys.path.insert(0, str(ROOT))
        os.environ.pop("FLOE_RUST_OCCUPANCY", None)
        cls.deck, cls.single = cls._workers()
        os.environ["FLOE_RUST_OCCUPANCY"] = "off"
        cls.deck_off, cls.single_off = cls._workers()
        del os.environ["FLOE_RUST_OCCUPANCY"]
        cls.bbox = tuple(cls.deck.cache.meta["bbox"])
        cls.dbu = float(cls.deck.cache.meta["dbu"])

    @classmethod
    def _workers(cls):
        from floe.jobdeck.viewer import DeckCache
        from floe.jobdeck import render as jrender
        from floe.cache import Cache
        from floe.rust_render import RustRenderWorker
        dc = DeckCache(str(cls.dir / "wide.jb"), mode="level")
        dc.load()
        deck = jrender.DeckRenderWorker(dc)
        deck.start()
        c = Cache(str(cls.dir / "thinwide.oas"))
        c.load()
        single = RustRenderWorker(c)
        single.start()
        return deck, single

    @classmethod
    def tearDownClass(cls):
        for w in (cls.deck, cls.single, cls.deck_off, cls.single_off):
            try:
                w.stop()
            except Exception:
                pass

    def _next(self):
        DeckRenderTests.gen += 1
        return DeckRenderTests.gen

    def test_a_keep_pass_draws_the_summary_on_the_source_view(self):
        # the placement is the 2000 um source at mag 0.2 (a 400 um deck
        # box): the source view of the deck fit is the source's 0..2000
        # um box at the same pixel size, so the deck's pixels are the
        # single cache's summary pixels of that box (level chosen on
        # the source view, scale included - plan §7)
        x0, y0, x1, y1 = self.bbox
        self.assertAlmostEqual((x1 - x0) * self.dbu, 400.0, places=3)
        lit, res = render_settled(self.deck, self._next(), self.bbox, 200)
        d = res["deck"]
        self.assertEqual((d["summary_passes"], d["summary_none_passes"]),
                         (1, 0), d)
        self.assertGreater(d["summary_cells"], 0)
        src_box = (0.0, 0.0, 2000.0 * UM, 2000.0 * UM)
        single, sres = render_settled(self.single, self._next(), src_box,
                                      200, visible=[(1, 0)])
        self.assertEqual(sres["summary"]["layers"], 1)
        self.assertEqual(lit, single)
        # frames wanted: the summary still draws (no pruning), frames come
        lit_f, res_f = render_settled(self.deck, self._next(), self.bbox,
                                      200, frames=True)
        self.assertEqual(res_f["deck"]["summary_passes"], 1)
        self.assertTrue(lit <= lit_f)

    def test_the_kill_switch_and_a_limited_depth_take_the_page_path(self):
        off, res = render_settled(self.deck_off, self._next(), self.bbox, 200)
        self.assertEqual(res["deck"]["summary_passes"], 0)
        src_box = (0.0, 0.0, 2000.0 * UM, 2000.0 * UM)
        single_off, _ = render_settled(self.single_off, self._next(), src_box,
                                       200, visible=[(1, 0)])
        self.assertEqual(off, single_off)
        on, _ = render_settled(self.deck, self._next(), self.bbox, 200)
        self.assertNotEqual(on, off)
        # the deck places 1/0, whose pages all sit in the top: depth 0
        # draws that layer whole, so its pass is summarized even there
        # (per-layer depth condition, 2026-09-15); depth 1 is the
        # source's hierarchy height
        for depth in (0, 1):
            at, res_d = render_settled(self.deck, self._next(), self.bbox,
                                       200, depth=depth)
            self.assertEqual(res_d["deck"]["summary_passes"], 1, (depth, res_d["deck"]))
            self.assertEqual(at, on, depth)

    def test_a_source_without_the_file_counts_as_none(self):
        ovo = Path(vfs_cache_dir(self.dir / "thinwide.oas")) / "design.ovo"
        keep = ovo.read_bytes()
        try:
            ovo.unlink()
            _, res = render_settled(self.deck, self._next(), self.bbox, 200)
            self.assertEqual((res["deck"]["summary_passes"],
                              res["deck"]["summary_none_passes"]), (0, 1))
        finally:
            ovo.write_bytes(keep)
        _, res = render_settled(self.deck, self._next(), self.bbox, 200)
        self.assertEqual(res["deck"]["summary_passes"], 1)


class PlanCliTests(unittest.TestCase):
    """M3: `floe-index plan --summary-layers` skips the pages (verdict
    `summary` under --explain) and `--prune-summary 1` drops the
    subtrees that hold nothing else."""

    @classmethod
    def setUpClass(cls):
        cls.src = TMP / "plancli.oas"
        write_thinwide(cls.src)
        cls.cache = index_with_occupancy(cls.src, 4)

    def plan(self, *extra):
        res = floe_index("plan", self.cache, "--view", "0,0,2000,2000",
                         "--px-per-um", "0.1", "--cut-px", "1",
                         "--page-hairline", "0", *extra)
        return res.stdout

    def test_summary_layers_skip_pages_and_explain_names_the_verdict(self):
        plain = self.plan("--layers", "1/0")
        self.assertIn('"pages": ', plain)
        self.assertNotIn('"pages": 0,', plain)
        skipped = self.plan("--layers", "1/0", "--summary-layers", "1/0",
                            "--explain", "1")
        self.assertIn('"pages": 0,', skipped)
        rows = [l for l in skipped.splitlines() if l.startswith("explain\t")]
        verdicts = {l.split("\t")[2] for l in rows if l.split("\t")[1] == "page"}
        self.assertIn("summary", verdicts)
        self.assertNotIn("exact", verdicts)

    def test_prune_summary_drops_the_summary_only_subtrees(self):
        import re
        kept = self.plan("--layers", "1/0", "--summary-layers", "1/0")
        pruned = self.plan("--layers", "1/0", "--summary-layers", "1/0",
                           "--prune-summary", "1")
        wc = lambda s: int(re.search(r'"wc_cells": (\d+)', s).group(1))
        self.assertGreater(wc(kept), 0)
        self.assertEqual(wc(pruned), 0)
        # another visible layer keeps the walk alive
        both = self.plan("--layers", "1/0,2/0", "--summary-layers", "1/0",
                         "--prune-summary", "1")
        self.assertGreater(wc(both), 0)


def main():
    if not BIN.is_file():
        print("FAIL: release floe-index is not built")
        sys.exit(1)
    argv = [sys.argv[0]]
    suite = unittest.defaultTestLoader.loadTestsFromModule(sys.modules[__name__])
    result = unittest.TextTestRunner(verbosity=1).run(suite)
    shutil.rmtree(TMP, ignore_errors=True)
    print("OCCUPANCY: %s (%d tests, %d failures, %d errors)" % (
        "ALL OK" if result.wasSuccessful() else "FAIL",
        result.testsRun, len(result.failures), len(result.errors)))
    sys.exit(0 if result.wasSuccessful() else 1)


if __name__ == "__main__":
    main()
