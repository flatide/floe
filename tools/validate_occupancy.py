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
import shutil
import struct
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import klayout.db as db

ROOT = Path(__file__).resolve().parents[1]
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
LAYER = "<IIBQ"           # layer dt status work
LEVEL = "<IIQQ"           # w h off len
STATUS = {0: "ok", 1: "none:cells", 2: "none:work", 3: "none:size"}


def read_ovo(path):
    d = Path(path).read_bytes()
    (magic, ver, unit, size, mtime, cell, x0, y0, x1, y1, nlv,
     nl) = struct.unpack_from(HDR, d, 0)
    assert magic == b"FLOEOVO1" and ver == 1, (magic, ver)
    o = struct.calcsize(HDR)
    top_len = struct.unpack_from("<H", d, o)[0]
    o += 2
    top = d[o:o + top_len].decode("utf-8")
    o += top_len
    layers = []
    for _ in range(nl):
        layer, dt, status, work = struct.unpack_from(LAYER, d, o)
        o += struct.calcsize(LAYER)
        levels = []
        for _ in range(nlv):
            w, h, off, ln = struct.unpack_from(LEVEL, d, o)
            o += struct.calcsize(LEVEL)
            levels.append((w, h, d[off:off + ln]))
        layers.append(dict(key=(layer, dt), status=STATUS.get(status),
                           work=work, levels=levels))
    return dict(unit=unit, src_size=size, src_mtime=mtime, cell=cell,
                bbox=(x0, y0, x1, y1), n_levels=nlv, top=top,
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


def oracle_level0(src, key, ovo):
    """set of (i, j) whose open cell box meets the layer with area > 0"""
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
                reg = db.Region(ly.begin_shapes_touching(top, li, box))
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
    out = str(src) + ".floe"
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
        src = sys.argv[1] if len(sys.argv) > 1 else None
        if src and Path(src).is_file():
            real = TMP / Path(src).name
            shutil.copy2(src, real)
            cls.cases.append((real, index_with_occupancy(real, 4)))

    def check_case(self, src, cache):
        ovo = read_ovo(cache / "design.ovo")
        checked = 0
        for layer in ovo["layers"]:
            self.assertEqual(layer["status"], "ok", layer)
            w, h, lit = oracle_level0(src, layer["key"], ovo)
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
        self.assertGreater(checked, 0)

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

    def listing(self, cache, ok=0):
        return floe_index("occupancy", cache, ok=ok)

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
            self.assertIn("status=ok", row)
            counts = [sum(bin(b).count("1") for b in lv[2])
                      for lv in layer["levels"]]
            self.assertIn("set=" + ",".join(map(str, counts)), row)
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
        self.assertEqual(read_ovo(self.cache / "design.ovo")["cell"], 4000)
        self.assertEqual({f: sha(self.cache / f) for f in before}, before)
        # --occupancy-only on a stale/missing cache is refused
        res = floe2("index", TMP / "missing.oas", "--occupancy-only", ok=1)
        self.assertIn("source not found", res.stderr)
        floe2("index", self.src, "--occupancy-only", "--occupancy-um", "1")
        self.assertEqual(read_ovo(self.cache / "design.ovo")["cell"], 1000)

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
        self.assertTrue(all("status=none:work" in r for r in rows))
        res = floe_index("vfs", self.src, self.cache, "--occupancy-only",
                         "--occupancy-max-bytes", "1")
        self.assertIn("none:size", res.stderr)
        floe_index("vfs", self.src, self.cache, "--occupancy-only",
                   "--occupancy-um", "1")
        rows = [l for l in self.listing(self.cache).stdout.splitlines()
                if l.startswith("layer ")]
        self.assertTrue(all("status=ok" in r for r in rows))

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
        ovo = read_ovo(Path(str(src) + ".floe") / "design.ovo")
        self.assertEqual((ovo["cell"], ovo["top"]), (2000, "FRESH"))
        # a plain index makes no summary (opt-in)
        plain = TMP / "plain.oas"
        write_chip(plain, "PLAIN", 10, 8)
        floe2("index", plain, "--jobs", "2")
        self.assertFalse((Path(str(plain) + ".floe") / "design.ovo").exists())

    def test_jobdeck_wrapper_forwards_the_occupancy_options(self):
        deck_dir = TMP / "deck"
        deck_dir.mkdir()
        write_chip(deck_dir / "chipA.oas", "CHIPA", 30, 30)
        write_chip(deck_dir / "chipB.oas", "CHIPB", 20, 20)
        (deck_dir / "occ.jb").write_text(DECK)
        caches = [deck_dir / "chipA.oas.floe", deck_dir / "chipB.oas.floe"]
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
        self.assertEqual(read_ovo(caches[1] / "design.ovo")["cell"], 4000)


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
