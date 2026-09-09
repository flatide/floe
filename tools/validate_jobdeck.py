#!/usr/bin/env python3
"""Jobdeck (Calibre MDPView .jb) gate: parser, placement, colours, header
probe, CLI and batch index.

Fixtures are generated here with KLayout (a development-only dependency,
like every other generator): three tiny OASIS sources plus a deck that
exercises every documented trap (missing identifiers per CHIP, an
identifier whose AD differs between CHIPs, a different TC/SF for one
identifier, a source with another dbu, an undocumented $ field, an
MTITLE never placed, `CHIP ID, * tail` lines). The expected placements in
tools/jobdeck_expected.json were computed BY HAND from the coordinate
model, not by the code, so this gate proves the port against the model
and not against itself.

Nothing is written outside a temporary directory; real jobdecks and mask
data never enter the repository.
"""

import gzip
import itertools
import json
import struct
import zlib
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from floe import jobdeck as jd                       # noqa: E402
from floe.jobdeck import color as jcolor             # noqa: E402
from floe.jobdeck import geom as jgeom               # noqa: E402
from floe.jobdeck.sources import file_header         # noqa: E402
from floe.cache import Cache                         # noqa: E402
from floe.jobdeck import render as jrender           # noqa: E402

EXPECTED = json.loads((ROOT / "tools" / "jobdeck_expected.json").read_text())

DECK = """SLICE 1,17
RETICLE
* test.jb
OPTION PA, AA=0.0200, BA=0.002000, SA=80
MTITLE 1,METAL1
MTITLE 2,VIA1
MTITLE 3,ALIGN
MTITLE 4,NEVER_PLACED
*PLACE-INFO
*
CHIP ID001, * MAIN 1.0000
*
$ (1, METAL1, AD=0.00020, SF=1, TC=chipA.oas, LY={123}, DT={43}, BX=0.0, BY=0.0, UX=2000.0, UY=2550.0 )
$ (2, VIA1, AD=0.00020, SF=1, TC=chipA.oas, LY={456}, DT={0}, BX=0.0, BY=0.0, UX=2000.0, UY=2550.0 )
$ (3, ALIGN, AD=0.00020, SF=1, TC=mark.oas, LY={999}, DT={0}, BX=0.0, BY=0.0, UX=100.0, UY=100.0 )
ROWS 85120.0/45020.0
ROWS 85120.0/55020.0
*
CHIP ID002, * MAIN2 1.0000
*
$ (1, METAL1, AD=0.00020, SF=1, TC=chipA.oas, LY={123}, DT={43}, BX=0.0, BY=0.0, UX=2000.0, UY=2550.0 )
$ (2, VIA1, AD=0.00020, SF=0.5, TC=chipB.oas, LY={456}, DT={0}, BX=0.0, BY=0.0, UX=1000.0, UY=1000.0 )
$ (5, GUARD, AD=0.00020, SF=1, TC=chipB.oas, LY={7}, DT={2}, BX=0.0, BY=0.0, UX=1000.0, UY=1000.0 )
ROWS 90000.0/45020.0
ROWS 90000.0/55020.0
ROWS 90000.0/65020.0
*
CHIP ID003, * MAIN3 1.0000
*
$ (1, METAL1, AD=0.00040, SF=1, TC=chipA.oas, LY={123}, DT={43}, BX=0.0, BY=0.0, UX=2000.0, UY=2550.0 )
$ (3, ALIGN, AD=0.00020, SF=1, TC=mark.oas, LY={999}, DT={0}, ZZ=7, BX=0.0, BY=0.0, UX=100.0, UY=100.0 )
ROWS 95000.0/45020.0
*END-PLACE
END
"""

# the same CHIPA geometry in every container the probe must classify,
# plus a junk file and a missing one for the skip ledger
FORMAT_DECK = """SLICE 1,17
RETICLE
* test_formats.jb
MTITLE 1,METAL1
*PLACE-INFO
CHIP ID001, * MAIN 1.0000
$ (1, METAL1, AD=0.00020, SF=1, TC=chipA.oas, LY={123}, DT={43}, BX=0.0, BY=0.0, UX=2000.0, UY=2550.0 )
$ (2, GDS, AD=0.00020, SF=1, TC=chipA.gds, LY={123}, DT={43}, BX=0.0, BY=0.0, UX=2000.0, UY=2550.0 )
$ (3, OASGZ, AD=0.00020, SF=1, TC=chipA.oas.gz, LY={123}, DT={43}, BX=0.0, BY=0.0, UX=2000.0, UY=2550.0 )
$ (4, GDSGZ, AD=0.00020, SF=1, TC=chipA.gds.gz, LY={123}, DT={43}, BX=0.0, BY=0.0, UX=2000.0, UY=2550.0 )
$ (6, JUNK, AD=0.00020, SF=1, TC=junk.bin, LY={123}, DT={43}, BX=0.0, BY=0.0, UX=2000.0, UY=2550.0 )
ROWS 85120.0/45020.0
CHIP ID002, * MAIN2 1.0000
$ (8, ABSENT, AD=0.00020, SF=1, TC=absent.oas, LY={123}, DT={43}, BX=0.0, BY=0.0, UX=2000.0, UY=2550.0 )
ROWS 90000.0/45020.0
*END-PLACE
END
"""

# review 2026-09-09 P1-1: level 2 names a layer chipA does not have
MISSING_LAYER_DECK = """* test_missing_layer.jb
MTITLE 1,METAL1
MTITLE 2,GHOST
*PLACE-INFO
CHIP ID001, * MAIN 1.0000
$ (1, METAL1, AD=0.00020, SF=1, TC=chipA.oas, LY={123}, DT={43}, BX=0.0, BY=0.0, UX=2000.0, UY=2550.0 )
$ (2, GHOST, AD=0.00020, SF=1, TC=chipA.oas, LY={987}, DT={43}, BX=0.0, BY=0.0, UX=2000.0, UY=2550.0 )
ROWS 85120.0/45020.0
*END-PLACE
END
"""

# review 2026-09-09 P1-2: a source whose one pass decodes past 1 MiB
DENSE_DECK = """* dense.jb
MTITLE 1,DENSE
*PLACE-INFO
CHIP D1
$ (1, DENSE, AD=0.00020, SF=1, TC=dense.oas, LY={1}, DT={0}, BX=0.0, BY=0.0, UX=2003.0, UY=2003.0 )
ROWS 1000.0/1000.0
*END-PLACE
END
"""

# step 4 (2026-09-10): a source whose content is all under the size
# cut at a full-deck view - a page of 1 um boxes (level 1) and an
# array of a 1 um child cell (level 2)
TINY_DECK = """* tiny.jb
MTITLE 1,DOTS
MTITLE 2,BITS
*PLACE-INFO
CHIP T1
$ (1, DOTS, AD=0.00020, SF=1, TC=tiny.oas, LY={1}, DT={0}, BX=0.0, BY=0.0, UX=2000.0, UY=2000.0 )
$ (2, BITS, AD=0.00020, SF=1, TC=tiny.oas, LY={2}, DT={0}, BX=0.0, BY=0.0, UX=2000.0, UY=2000.0 )
ROWS 0.0/0.0
*END-PLACE
END
"""

# review 2026-09-09 (2nd) P1-1: two overlapping placements of a source
# that has a top-level box AND a child cell - at depth 0 the child is
# a white frame that the later placement's box must not bury
FRAMES_DECK = """* frames.jb
MTITLE 1,A
MTITLE 2,B
*PLACE-INFO
CHIP A
$ (1, A, AD=0.00020, SF=1, TC=hier2.oas, LY={1}, DT={0}, BX=0.0, BY=0.0, UX=2000.0, UY=2000.0 )
ROWS 0.0/0.0
CHIP B
$ (2, B, AD=0.00020, SF=1, TC=hier2.oas, LY={1}, DT={0}, BX=0.0, BY=0.0, UX=2000.0, UY=2000.0 )
ROWS 0.0/1600.0
*END-PLACE
END
"""

# review 2026-09-09 (3rd) P2-2: one LY with datatypes 0 and 1 - in the
# source-layer view DT0 is a real layer, not a group head
DT_DECK = """* dt.jb
MTITLE 1,DT
*PLACE-INFO
CHIP D
$ (1, DT, AD=0.00020, SF=1, TC=dt.oas, LY={7}, DT={0,1}, BX=0.0, BY=0.0, UX=2000.0, UY=2000.0 )
ROWS 0.0/0.0
*END-PLACE
END
"""

# review 2026-09-09 P1-3: the shapes live in a child cell only
HIER_DECK = """* hier.jb
MTITLE 1,KID
*PLACE-INFO
CHIP H1
$ (1, KID, AD=0.00020, SF=1, TC=hier.oas, LY={1}, DT={0}, BX=0.0, BY=0.0, UX=2000.0, UY=2000.0 )
ROWS 1000.0/1000.0
*END-PLACE
END
"""

BROKEN_DECK = """* broken.jb
*PLACE-INFO
CHIP ID001
$ (1, METAL1, AD=0.00020, SF=1, TC=chipA.oas, LY={123}, DT={43},
BX=0.0, BY=0.0, UX=2000.0, UY=2550.0 )
ROWS 85120.0/45020.0
*END-PLACE
END
"""


def build_oas(path, dbu, w_um, h_um, layers, cellname):
    import klayout.db as db
    ly = db.Layout()
    ly.dbu = dbu
    top = ly.create_cell(cellname)
    w = int(round(w_um / dbu))
    h = int(round(h_um / dbu))
    for i, (l, d) in enumerate(layers):
        li = ly.layer(l, d)
        if i == 0:
            top.shapes(li).insert(db.Box(0, 0, w, h))
        else:
            m = int(w * 0.1)
            top.shapes(li).insert(db.Box(m, m, w - m, h - m))
    ly.write(str(path))


def build_fixtures(d: Path):
    build_oas(d / "chipA.oas", 0.00005, 2000.0, 2550.0,
              [(123, 43), (456, 0)], "CHIPA")
    build_oas(d / "chipB.oas", 0.00005, 1000.0, 1000.0,
              [(456, 0), (7, 2)], "CHIPB")
    build_oas(d / "mark.oas", 0.001, 100.0, 100.0, [(999, 0)], "MARK")
    build_oas(d / "chipA.gds", 0.00005, 2000.0, 2550.0,
              [(123, 43), (456, 0)], "CHIPA")
    for src, dst in (("chipA.oas", "chipA.oas.gz"),
                     ("chipA.gds", "chipA.gds.gz")):
        with open(d / src, "rb") as fi, gzip.open(d / dst, "wb") as fo:
            fo.write(fi.read())
    (d / "junk.bin").write_bytes(bytes(range(256)) * 4)
    (d / "test.jb").write_text(DECK)
    (d / "test_formats.jb").write_text(FORMAT_DECK)
    (d / "broken.jb").write_text(BROKEN_DECK)
    (d / "test_missing_layer.jb").write_text(MISSING_LAYER_DECK)
    (d / "dense.jb").write_text(DENSE_DECK)
    (d / "hier.jb").write_text(HIER_DECK)
    (d / "frames.jb").write_text(FRAMES_DECK)
    (d / "dt.jb").write_text(DT_DECK)
    (d / "tiny.jb").write_text(TINY_DECK)
    build_oas(d / "dt.oas", 0.00005, 2000.0, 2000.0, [(7, 0), (7, 1)], "DT")
    build_dense_oas(d / "dense.oas")
    build_tiny_oas(d / "tiny.oas")
    build_hier_oas(d / "hier.oas")
    build_hier2_oas(d / "hier2.oas")


def build_hier2_oas(path, dbu=0.00005):
    """TOP: a 2000 um box on 1/0 and an instance of KID (a 200 um box
    on 1/0 at 500,500) - depth 0 draws the box and KID's frame."""
    import klayout.db as db
    ly = db.Layout()
    ly.dbu = dbu
    li = ly.layer(1, 0)
    kid = ly.create_cell("KID")
    k = int(round(200.0 / dbu))
    kid.shapes(li).insert(db.Box(0, 0, k, k))
    top = ly.create_cell("TOP")
    w = int(round(2000.0 / dbu))
    top.shapes(li).insert(db.Box(0, 0, w, w))
    off = int(round(500.0 / dbu))
    top.insert(db.CellInstArray(kid.cell_index(), db.Trans(db.Vector(off, off))))
    ly.write(str(path))


def build_dense_oas(path, dbu=0.00005, count=60000, extent_um=2000.0,
                    cells=4):
    """`count` rectangles at pseudo-random positions and sizes (a
    deterministic LCG) spread over `cells` child cells of TOP: nothing
    regular enough for the OASIS writer or the indexer to fold into
    repetitions, so the decode really holds 60k rectangles - well over
    the 1 MiB budget the review's memory test used - and the index has
    several pages, so a budget-capped pass has pages left to defer."""
    import klayout.db as db
    ly = db.Layout()
    ly.dbu = dbu
    top = ly.create_cell("DENSE")
    li = ly.layer(1, 0)
    unit = int(round(1.0 / dbu))
    span = int(round(extent_um * unit))
    state = 0x2545F491
    def rnd():
        nonlocal state
        state = (state * 1103515245 + 12345) & 0x7fffffff
        return state
    # D0 also holds a grandchild K (a 100 um box): at depth 1 the D
    # cells' pages stream while K is a hierarchy frame (StreamTests)
    grand = ly.create_cell("K")
    grand.shapes(li).insert(db.Box(0, 0, 100 * unit, 100 * unit))
    for c in range(cells):
        kid = ly.create_cell("D%d" % c)
        shapes = kid.shapes(li)
        for _ in range(count // cells):
            x = rnd() % span
            y = rnd() % span
            w = unit + rnd() % (3 * unit)
            h = unit + rnd() % (3 * unit)
            shapes.insert(db.Box(x, y, x + w, y + h))
        if c == 0:
            kid.insert(db.CellInstArray(grand.cell_index(),
                                        db.Trans(span // 2, span // 2)))
        top.insert(db.CellInstArray(kid.cell_index(), db.Trans()))
    ly.write(str(path))


def build_tiny_oas(path, dbu=0.00005, pitch_um=10.0, n=200, box_um=1.0):
    """TINY: layer 1/0 holds an n x n field of 1 um boxes (own page),
    layer 2/0 the same field as an array of a 1 um child cell BIT -
    both entirely under a full-deck view's size cut."""
    import klayout.db as db
    ly = db.Layout()
    ly.dbu = dbu
    top = ly.create_cell("TINY")
    bit = ly.create_cell("BIT")
    unit = int(round(1.0 / dbu))
    b = int(round(box_um * unit))
    p = int(round(pitch_um * unit))
    l1 = ly.layer(1, 0)
    l2 = ly.layer(2, 0)
    dots = top.shapes(l1)
    for i in range(n):
        for j in range(n):
            dots.insert(db.Box(i * p, j * p, i * p + b, j * p + b))
    bit.shapes(l2).insert(db.Box(0, 0, b, b))
    top.insert(db.CellInstArray(bit.cell_index(), db.Trans(),
                                db.Vector(p, 0), db.Vector(0, p), n, n))
    ly.write(str(path))


def build_hier_oas(path, dbu=0.00005):
    """TOP holds only an instance of KID; KID holds the box."""
    import klayout.db as db
    ly = db.Layout()
    ly.dbu = dbu
    kid = ly.create_cell("KID")
    li = ly.layer(1, 0)
    w = int(round(2000.0 / dbu))
    kid.shapes(li).insert(db.Box(0, 0, w, w))
    top = ly.create_cell("TOP")
    top.insert(db.CellInstArray(kid.cell_index(), db.Trans()))
    ly.write(str(path))


TMP = None      # library tests: never indexed
CLI = None      # CLI tests: a second copy, so --index leaks nowhere


def setUpModule():
    global TMP, CLI
    base = os.environ.get("TMPDIR") or None
    TMP = Path(tempfile.mkdtemp(prefix="floe-jobdeck-", dir=base))
    build_fixtures(TMP)
    CLI = TMP / "cli"
    shutil.copytree(TMP, CLI)


def tearDownModule():
    if TMP is not None and not os.environ.get("FLOE_JOBDECK_KEEP"):
        shutil.rmtree(TMP, ignore_errors=True)


def run_cli(*args, env=None, ok=None):
    e = os.environ.copy()
    e.update({"PYTHONDONTWRITEBYTECODE": "1", "PYTHONPATH": str(ROOT)})
    if env:
        e.update(env)
    res = subprocess.run(
        [sys.executable, "-B", "-m", "floe2", "jobdeck", *map(str, args)],
        cwd=ROOT, env=e, capture_output=True, text=True)
    if ok is not None and res.returncode != ok:
        raise AssertionError(
            "floe2 jobdeck %s: exit %d, wanted %d\nstdout:\n%s\nstderr:\n%s"
            % (args, res.returncode, ok, res.stdout, res.stderr))
    return res


class ParserTests(unittest.TestCase):
    def setUp(self):
        self.deck = jd.parse_jobdeck(str(TMP / "test.jb"))

    def test_structure(self):
        d = self.deck
        self.assertEqual([c.id for c in d.chips], ["ID001", "ID002", "ID003"])
        self.assertEqual(d.chips[0].tail, "* MAIN 1.0000")
        self.assertEqual(d.identifiers(), EXPECTED["identifiers"])
        self.assertEqual(sum(1 for _ in d.instances()),
                         EXPECTED["instance_count"])
        self.assertEqual(d.jb_name, "test.jb")
        self.assertEqual(d.header["SLICE"], "1,17")
        self.assertIn("RETICLE", d.header)
        self.assertEqual(d.option_flags, ["PA"])
        self.assertEqual(d.options["SA"], 80)
        self.assertEqual(d.mtitles[4], "NEVER_PLACED")
        # ROWS are y/x: Y FIRST
        self.assertEqual(d.chips[0].rows, [(85120.0, 45020.0),
                                           (85120.0, 55020.0)])
        self.assertEqual(d.errors, [])
        self.assertEqual(d.unknown, [])

    def test_entry_fields(self):
        e = self.deck.chips[2].entry(3)
        self.assertEqual(e.extra, {"ZZ": "7"})
        self.assertEqual(self.deck.extras()["ZZ"][0]["chip"], "ID003")
        e1 = self.deck.chips[2].entry(1)
        self.assertEqual(e1.ad, 0.0004)
        self.assertEqual(e1.ly, [123])
        self.assertEqual(e1.dt, [43])
        self.assertEqual(self.deck.chips[1].entry(2).sf, 0.5)
        self.assertEqual(self.deck.chips[1].entry(2).tc, "chipB.oas")

    def test_coverage_variation_titles(self):
        cov = {r["chip"]: r for r in self.deck.coverage()["chips"]}
        for cid, want in EXPECTED["coverage_expected"].items():
            self.assertEqual(cov[cid]["present"], want["present"], cid)
            self.assertEqual(cov[cid]["missing"], want["missing"], cid)
        self.assertEqual(sorted(self.deck.variation_report()),
                         EXPECTED["variation_expected"])
        tc = self.deck.title_check()
        want = EXPECTED["title_check_expected"]
        self.assertEqual(tc["missing_title"], want["missing_title"])
        self.assertEqual(tc["unused_title"], want["unused_title"])

    def test_placed_widths(self):
        """expected widths are on the deck: entry width times mag."""
        cat = jd.SourceCatalog(str(TMP))
        cat.probe_all(self.deck.sources())
        pl, _ = jgeom.plan(self.deck, cat.dbus())
        for key, want in EXPECTED["widths_um"].items():
            cid, idx = key.split("/")
            p = next(p for p in pl if p.chip == cid and p.idx == int(idx))
            self.assertAlmostEqual(p.bbox_um[2] - p.bbox_um[0], want,
                                   places=9, msg=key)

    def test_wrapped_entry_is_refused(self):
        with self.assertRaises(ValueError) as cm:
            jd.parse_jobdeck(str(TMP / "broken.jb"))
        self.assertIn("unterminated", str(cm.exception))
        self.assertIn("orphan", str(cm.exception))
        lenient = jd.parse_jobdeck(str(TMP / "broken.jb"), strict=False)
        self.assertEqual(len(lenient.errors), 2)
        self.assertEqual(lenient.chips[0].entries, [])

    def test_split_helpers(self):
        self.assertEqual(jd.parser.split_top_level("a, {1,2}, (3,4), b"),
                         ["a", "{1,2}", "(3,4)", "b"])
        self.assertEqual(jd.parser.parse_braced_list("{1,2}"), [1, 2])
        self.assertEqual(jd.parser.parse_braced_list("3"), [3])
        self.assertEqual(jd.parser.parse_braced_list("{}"), [])


class ProbeTests(unittest.TestCase):
    def test_header_dbu(self):
        for name, want in EXPECTED["source_dbu"].items():
            fmt, dbu, version, gz = file_header(str(TMP / name))
            self.assertEqual(fmt, "oasis", name)
            self.assertFalse(gz)
            self.assertAlmostEqual(dbu, want, places=12, msg=name)
        fmt, dbu, _, gz = file_header(str(TMP / "chipA.gds"))
        self.assertEqual((fmt, gz), ("gds", False))
        self.assertAlmostEqual(dbu, 5e-05, places=12)
        fmt, dbu, _, gz = file_header(str(TMP / "chipA.oas.gz"))
        self.assertEqual((fmt, gz), ("oasis", True))
        self.assertAlmostEqual(dbu, 5e-05, places=12)
        fmt, dbu, _, gz = file_header(str(TMP / "chipA.gds.gz"))
        self.assertEqual((fmt, gz), ("gds", True))
        self.assertAlmostEqual(dbu, 5e-05, places=12)
        self.assertEqual(file_header(str(TMP / "junk.bin"))[:2],
                         (None, None))

    def test_catalog_statuses(self):
        deck = jd.parse_jobdeck(str(TMP / "test_formats.jb"))
        cat = jd.SourceCatalog(str(TMP))
        cat.probe_all(deck.sources())
        st = {tc: i.status for tc, i in cat.infos.items()}
        # GDS and gzip containers probe (dbu known) but floe-index
        # reads plain OASIS only: unsupported, a skipped placement
        self.assertEqual(st, {
            "chipA.oas": "ok", "chipA.gds": "unsupported",
            "chipA.oas.gz": "unsupported", "chipA.gds.gz": "unsupported",
            "junk.bin": "unknown_format", "absent.oas": "missing"})
        self.assertAlmostEqual(cat.infos["chipA.gds"].dbu, 5e-05, places=12)
        self.assertEqual(sorted(cat.dbus()), ["chipA.oas"])
        self.assertEqual(sorted(cat.bad()), ["absent.oas", "chipA.gds",
                                             "chipA.gds.gz", "chipA.oas.gz",
                                             "junk.bin"])
        self.assertFalse(any(i.indexed for i in cat.infos.values()))


class PlacementTests(unittest.TestCase):
    def setUp(self):
        self.deck = jd.parse_jobdeck(str(TMP / "test.jb"))
        self.cat = jd.SourceCatalog(str(TMP))
        self.cat.probe_all(self.deck.sources())
        self.pl, self.st = jgeom.plan(self.deck, self.cat.dbus())

    def test_against_hand_computed(self):
        want = EXPECTED["placements"]
        self.assertEqual(len(self.pl), len(want))
        self.assertEqual(self.st["instances"], EXPECTED["instance_count"])
        got = {(p.chip, p.idx, p.row): p for p in self.pl}
        for w in want:
            p = got[(w["chip"], w["idx"], w["row"])]
            for k in ("mag", "dx", "dy"):
                have = getattr(p, k if k == "mag" else k + "_um")
                self.assertAlmostEqual(have, w[k], delta=1e-9,
                                       msg="%s %s" % (w, k))
        # dbu: min(5e-5, 1e-3, 2e-4, 4e-4)/2, no coarsening on i64
        self.assertAlmostEqual(self.st["dbu"], 2.5e-05, places=15)
        self.assertEqual(self.st["dbu_choice"]["doublings"], 0)
        self.assertEqual(self.st["residual_nonzero"], 0)
        self.assertEqual(self.st["residual_over_half_dbu"], 0)
        self.assertEqual(self.st["mags"], [0.2, 2.0, 4.0, 8.0])
        self.assertEqual(self.st["skipped"], [])
        for p in self.pl:
            self.assertEqual(p.ix * self.st["dbu"], p.dx_um)
            self.assertEqual(p.iy * self.st["dbu"], p.dy_um)

    def test_bbox_and_layer_table(self):
        x0, y0, x1, y1 = self.st["bbox_um"]
        # ID001 $1 row 0: dx 41020 + 4*[0,2000] -> 41020..49020
        self.assertAlmostEqual(x0, 37020.0)          # ID003 $1 mag 8
        self.assertAlmostEqual(x1, 61020.0 + 4.0 * 2000.0)  # ID002 $1 row 2
        self.assertAlmostEqual(y0, 80020.0)
        self.assertAlmostEqual(y1, 84800.0 + 8.0 * 2550.0)  # ID003 $1
        table = self.st["layer_table"]
        self.assertEqual([(r["idx"], r["ly"], r["dt"]) for r in table],
                         [(1, 123, 43), (2, 456, 0), (3, 999, 0), (5, 7, 2)])
        self.assertEqual([r["out"] for r in table], [0, 1, 2, 3])
        self.assertEqual(table[0]["title"], "METAL1")
        self.assertEqual(table[3]["title"], "")

    def test_selection_keeps_the_grid(self):
        pl, st = jgeom.plan(self.deck, self.cat.dbus(), ids=[3])
        self.assertEqual(st["dbu"], self.st["dbu"])
        self.assertEqual(st["instances"], 3)
        self.assertEqual(st["instances_total"], EXPECTED["instance_count"])
        self.assertEqual({p.idx for p in pl}, {3})
        self.assertEqual(st["selection"], [3])

    def test_missing_source_ledger(self):
        deck = jd.parse_jobdeck(str(TMP / "test_formats.jb"))
        cat = jd.SourceCatalog(str(TMP))
        cat.probe_all(deck.sources())
        with self.assertRaises(KeyError):
            jgeom.plan(deck, cat.dbus(), bad=cat.bad())
        pl, st = jgeom.plan(deck, cat.dbus(), missing=jgeom.MISSING_SKIP,
                            bad=cat.bad())
        self.assertEqual(len(pl), 1)
        self.assertEqual(st["skip_counts"],
                         {"missing": 1, "unknown_format": 1,
                          "unsupported": 3})
        reasons = {(r["chip"], r["idx"]): r["reason"] for r in st["skipped"]}
        self.assertEqual(reasons, {("ID001", 2): "unsupported",
                                   ("ID001", 3): "unsupported",
                                   ("ID001", 4): "unsupported",
                                   ("ID001", 6): "unknown_format",
                                   ("ID002", 8): "missing"})
        self.assertEqual(st["skipped"][0]["anchors"], [[45020.0, 85120.0]])
        # outside the selection a bad source is information, not a skip
        pl, st = jgeom.plan(deck, cat.dbus(), ids=[1], bad=cat.bad())
        self.assertEqual(len(pl), 1)
        self.assertEqual(st["skipped"], [])
        self.assertEqual(st["deck_issue_counts"],
                         {"missing": 1, "unknown_format": 1,
                          "unsupported": 3})

    def test_choose_dbu_coarsens_only_past_the_limit(self):
        dbu, why = jgeom.choose_dbu([5e-5], [2e-4], extent_um=1e6)
        self.assertEqual((dbu, why["doublings"]), (2.5e-5, 0))
        dbu, why = jgeom.choose_dbu([5e-5], [2e-4], extent_um=1e6,
                                    coord_max=2 ** 31 - 1)
        self.assertGreater(why["doublings"], 0)
        self.assertLessEqual(1e6 / dbu, (2 ** 31 - 1) * 0.9)
        with self.assertRaises(ValueError):
            jgeom.choose_dbu([], [])


class ColorTests(unittest.TestCase):
    def setUp(self):
        self.deck = jd.parse_jobdeck(str(TMP / "test.jb"))

    def test_identifier_order(self):
        s = jd.ColorScheme(mode=jd.MODE_IDENTIFIER)
        self.assertEqual(s.order(self.deck), ["$1", "$2", "$3", "$5"])
        cm = s.build(self.deck)
        self.assertEqual(cm, {1: "#0000ff", 2: "#ffff00", 3: "#ff0000",
                              5: "#ffc0cb"})
        # a selection never moves identifier colours
        self.assertEqual(s.build(self.deck, ids=[3]), cm)
        rows = s.order_table(self.deck)
        self.assertEqual([r["name"] for r in rows],
                         ["blue", "yellow", "red", "pink"])

    def test_chip_order_splices_the_selection(self):
        self.assertEqual(jcolor.chip_order(["C1", "C2", "C3", "C4"], [2]),
                         ["CHIP:C1", "$2", "CHIP:C2", "CHIP:C3", "CHIP:C4"])
        self.assertEqual(jcolor.chip_order(["C1", "C2", "C3"], [1, 3]),
                         ["$1", "CHIP:C1", "CHIP:C2", "$3", "CHIP:C3"])
        s = jd.ColorScheme(mode=jd.MODE_CHIP)
        self.assertEqual(s.build(self.deck, ids=[2]),
                         {"ID001": "#0000ff", "ID002": "#ff0000",
                          "ID003": "#ffc0cb"})
        # every identifier selected: $1 C1 $2 C2 $3 C3 $5 -> C3 is 6th
        self.assertEqual(s.order(self.deck),
                         ["$1", "CHIP:ID001", "$2", "CHIP:ID002", "$3",
                          "CHIP:ID003", "$5"])
        self.assertEqual(s.build(self.deck),
                         {"ID001": "#ffff00", "ID002": "#ffc0cb",
                          "ID003": "#ffffff"})

    def test_layer_mode_and_pins(self):
        s = jd.ColorScheme(mode=jd.MODE_LAYER)
        self.assertEqual(s.order(self.deck), ["LY7", "LY123", "LY456",
                                              "LY999"])
        cm = s.build(self.deck)
        self.assertEqual(cm[(123, 43)], "#ffff00")
        s.pin_layer_datatype(123, 43, "#123456")
        self.assertEqual(s.build(self.deck)[(123, 43)], "#123456")
        self.assertEqual(s.order_table(self.deck)[1]["source"], "palette")
        s.pin_layer(456, "#abcdef")
        self.assertEqual(s.order_table(self.deck)[2]["source"], "pinned")
        self.assertEqual(jcolor.palette_color(jd.JOBDECK_PALETTE, 10),
                         "#0000ff")

    def test_scheme_roundtrip(self):
        s = jd.ColorScheme(mode=jd.MODE_CHIP, overrides={"ID002": "#000000"})
        p = TMP / "scheme.json"
        s.save(str(p))
        t = jd.ColorScheme.load(str(p))
        self.assertEqual((t.mode, t.overrides), (s.mode, s.overrides))
        (TMP / "bad_scheme.json").write_text('{"mode": "nope"}')
        with self.assertRaises(ValueError):
            jd.ColorScheme.load(str(TMP / "bad_scheme.json"))


class CliTests(unittest.TestCase):
    def test_1_plan_and_report(self):
        rep = CLI / "report.json"
        res = run_cli(CLI / "test.jb", "--report", rep, "--placements",
                      "--mode", "chip", "--id", "2", ok=0)
        self.assertIn("[jobdeck] instances : 5 placed of 17 (selection [2])",
                      res.stdout)
        self.assertIn("CHIP:ID001 blue, $2 yellow, CHIP:ID002 red",
                      res.stdout)
        self.assertIn("no .floe cache yet; run: floe2 index", res.stdout)
        r = json.loads(rep.read_text())
        self.assertEqual(r["plan"]["instances"], 5)
        self.assertEqual(len(r["placements"]), 5)
        self.assertEqual(r["deck"]["undocumented_entry_fields"]["ZZ"][0]
                         ["value"], "7")
        self.assertEqual(r["plan"]["sources"]["probed"], 3)
        self.assertEqual(r["plan"]["colors"]["order"][1]["key"], "$2")
        self.assertEqual(r["plan"]["layer_table_by_chip"], True)

    def test_2_missing_source_exit_codes(self):
        res = run_cli(CLI / "test_formats.jb", ok=3)
        self.assertIn("skipped   : CHIP ID002 $8 absent.oas: missing",
                      res.stdout)
        self.assertIn("junk.bin: unknown_format", res.stdout)
        self.assertIn("chipA.gds: unsupported", res.stdout)
        run_cli(CLI / "test_formats.jb", "--on-missing", "fail", ok=2)
        run_cli(CLI / "test_formats.jb", "--id", "1", ok=0)
        res = run_cli(CLI / "broken.jb", ok=1)
        self.assertIn("structural error", res.stderr)

    def test_3_batch_index(self):
        """runs last: it leaves .floe caches in the CLI copy."""
        binary = ROOT / "rust" / "target" / "release" / "floe-index"
        self.assertTrue(binary.is_file(), "release floe-index is not built")
        env = {"FLOE_INDEX_BIN": str(binary)}
        res = run_floe2("index", CLI / "test.jb", "--jobs", "2", env=env,
                        ok=0)
        self.assertIn("index     : 3 built, 0 failed, 0 kept", res.stdout)
        for name in ("chipA.oas", "chipB.oas", "mark.oas"):
            c = Cache(str(CLI / name))
            self.assertTrue(c.exists(), name)
            c.load()
            self.assertTrue(c.meta.get("vfs"), name)
            self.assertFalse(c.is_stale(), name)
        res = run_floe2("index", CLI / "test.jb", env=env, ok=0)
        self.assertIn("3 source(s) already indexed", res.stdout)
        self.assertIn("index     : 0 built, 0 failed, 3 kept", res.stdout)
        res = run_cli(CLI / "test.jb", env=env, ok=0)
        self.assertIn("3 probed, 3 ok, 3 indexed", res.stdout)


# ---------------------------------------------------------------------
# M2: the composite through renderd
# ---------------------------------------------------------------------

# the fixture geometry per (source, ly, dt) in source um: a full-extent
# box on the first layer, a 10% inset box on the others (build_oas)
FIXTURE_BOXES = {
    ("chipA.oas", 123, 43): (0.0, 0.0, 2000.0, 2550.0),
    ("chipA.oas", 456, 0): (200.0, 200.0, 1800.0, 2350.0),
    ("chipB.oas", 456, 0): (0.0, 0.0, 1000.0, 1000.0),
    ("chipB.oas", 7, 2): (100.0, 100.0, 900.0, 900.0),
    ("mark.oas", 999, 0): (0.0, 0.0, 100.0, 100.0),
}


def _rgb(hexcolor):
    h = hexcolor.lstrip("#")
    return tuple(int(h[i:i + 2], 16) for i in (0, 2, 4))


_GEN = itertools.count(1)


SPECKLE_ROWS = None      # the viewer's default: no repattern
PATTERN_ROWS = "\n".join(("*.*." * 4, ".*.*" * 4, "**.." * 4, "..**" * 4) * 4)


def _render_raw(worker, bbox_dbu, width, height, visible=None,
                depth=None, cut_px=0.0, frames=False, with_result=False,
                fill="solid", width_px=1):
    """One settled raw frame through a started worker: RGBA bytes.
    Generations must increase per daemon: a repeated one is dropped.
    `fill`: "solid" (archival), "speckle" (the viewer's default) or a
    16x16 pattern rows string."""
    gen = next(_GEN)
    solid = "\n".join(["*" * 16] * 16)
    keys = [(int(l["layer"]), int(l["datatype"]))
            for l in worker.cache.meta["layers"]]
    rows = solid if fill == "solid" else (None if fill == "speckle"
                                          else fill)
    # speckle is the worker's default fill: only the widths are sent
    worker.submit({"kind": "repattern",
                   "fills": [] if rows is None else [(k, rows) for k in keys],
                   "widths": [(k, width_px) for k in keys]})
    worker.submit({
        "kind": "render", "gen": gen, "scope": "headless",
        "bbox": tuple(float(v) for v in bbox_dbu), "view": None,
        "w": width, "h": height, "depth": depth, "cut_px": cut_px,
        "lod": False, "frames": frames, "labels": False,
        "abstract": False, "visible": visible, "frame_format": "raw",
    })
    while True:
        result = worker.res.get(timeout=300)
        if result.get("kind") == "error":
            raise AssertionError("renderd: %s" % result.get("msg"))
        if result.get("kind") != "frame" or result.get("gen") != gen:
            continue
        if result.get("refining"):
            continue
        rgba = result["rgba"]
        if len(rgba) != width * height * 4:
            raise AssertionError("raw frame size mismatch")
        return (rgba, result) if with_result else rgba


class CompositeTests(unittest.TestCase):
    """renderd `open deck=`: the composite of the three fixture caches
    against (1) the boxes placed by hand and (2) a KLayout-flattened
    single layout rendered through the ordinary single-cache path."""

    W, H = 1024, 800
    # fractional um offsets so no box edge sits on a device boundary,
    # where the two f64 mappings could legitimately round apart
    BBOX_UM = (36000.37, 79000.61, 70000.37, 106000.61)

    @classmethod
    def setUpClass(cls):
        cls.binary = ROOT / "rust" / "target" / "release" / "floe-renderd"
        if not cls.binary.is_file():
            raise unittest.SkipTest("release floe-renderd is not built")
        cls.env = {"FLOE_INDEX_BIN": str(ROOT / "rust" / "target" /
                                         "release" / "floe-index"),
                   "FLOE_RENDERD_BIN": str(cls.binary)}
        os.environ["FLOE_RENDERD_BIN"] = str(cls.binary)
        run_floe2("index", CLI / "test.jb", "--jobs", "2", env=cls.env,
                  ok=0)
        deck, cat, pl, st, scheme, cm = jd.plan_deck(str(CLI / "test.jb"))
        cls.deck, cls.catalog, cls.placements = deck, cat, pl
        cls.stats, cls.scheme, cls.colormap = st, scheme, cm
        cls.dbu = float(st["dbu"])
        cls.bbox_um = jrender.fit_bbox_to_pixels(cls.BBOX_UM, cls.W, cls.H)
        cls.bbox_dbu = tuple(v / cls.dbu for v in cls.bbox_um)
        cls.spec = CLI / "deck.spec"
        cls.ledger = jrender.write_deck_spec(
            str(cls.spec), deck, pl, st, scheme, cm, cat)
        cls.layers = jrender.deck_layers_meta(deck, st, scheme, cm, pl)
        cls.rows = jrender.view_layers(deck, st, scheme, cm)
        cls.out_of = staticmethod(jrender.view_out_of(cls.rows, scheme))
        worker = jrender.DeckRenderWorker(jrender._DeckCacheShim(
            str(cls.spec), str(CLI / "test.jb"), cls.dbu, cls.layers))
        worker.start()
        try:
            cls.composite = _render_raw(worker, cls.bbox_dbu, cls.W, cls.H)
            # level view keys rows by level number: level 2 ($2) alone
            cls.only_2 = _render_raw(worker, cls.bbox_dbu, cls.W, cls.H,
                                     visible=[(2, 0)])
            # the viewer's defaults (depth 0, detail medium = 3px cut):
            # the 20 um mark is below the cut on a full-deck view, so
            # the planner drops that source - an empty pass, not an
            # error (field: the first GUI frame failed on this)
            cls.gui_defaults = _render_raw(worker, cls.bbox_dbu, cls.W,
                                           cls.H, depth=0, cut_px=3.0)
        finally:
            worker.stop()

    def _expected_boxes(self):
        """(out, deck box um, rgb) per placement, in paint order."""
        rows = []
        for p in self.placements:
            sx0, sy0, sx1, sy1 = FIXTURE_BOXES[(p.tc, p.ly, p.dt)]
            box = (p.mag * sx0 + p.dx_um, p.mag * sy0 + p.dy_um,
                   p.mag * sx1 + p.dx_um, p.mag * sy1 + p.dy_um)
            rows.append((self.out_of(p), box, _rgb(self.colormap[p.idx])))
        rows.sort(key=lambda r: r[0])
        return rows

    def test_1_spec_and_ledger(self):
        self.assertEqual(self.ledger, [])
        text = self.spec.read_text()
        self.assertEqual(text.count("\nsource "), 3)
        self.assertEqual(text.count("\nplacement "), 17)
        self.assertEqual(text.count("\nlayer "), 4)
        # scale = mag * source_dbu / deck_dbu: $1 in ID001 is 4 * 2 = 8
        self.assertIn("layer=123/43 out=0 scale=8.0 dx=1640800000 "
                      "dy=3200800000 order=0", text)
        self.assertIn("layer=999/0 out=2 scale=8.0", text)   # mag 0.2 * 40
        self.assertIn("color=#0000ff", text)

    def test_2_composite_matches_hand_placed_boxes(self):
        x0, y0, x1, y1 = self.bbox_um
        sx = (x1 - x0) / self.W
        sy = (y1 - y0) / self.H
        boxes = self._expected_boxes()
        self.assertEqual(len(boxes), 17)
        checked = 0
        mismatches = []
        px = self.composite
        for j in range(self.H):
            y = y1 - (j + 0.5) * sy
            for i in range(self.W):
                x = x0 + (i + 0.5) * sx
                want = (0, 0, 0)
                near_edge = False
                for _out, (bx0, by0, bx1, by1), rgb in boxes:
                    if (abs(x - bx0) < sx or abs(x - bx1) < sx or
                            abs(y - by0) < sy or abs(y - by1) < sy):
                        if by0 - sy <= y <= by1 + sy and \
                                bx0 - sx <= x <= bx1 + sx:
                            near_edge = True
                            break
                    if bx0 <= x <= bx1 and by0 <= y <= by1:
                        want = rgb          # later out paints over
                if near_edge:
                    continue
                o = (j * self.W + i) * 4
                got = (px[o], px[o + 1], px[o + 2])
                checked += 1
                if got != want and len(mismatches) < 5:
                    mismatches.append((i, j, got, want))
        self.assertGreater(checked, self.W * self.H // 2)
        self.assertEqual(mismatches, [], "first mismatching pixels")
        # something was actually drawn, in more than one colour
        colours = {tuple(px[o:o + 3]) for o in range(0, len(px), 4 * 97)}
        self.assertGreaterEqual(len(colours), 4)

    def test_3_visible_layers_cull_placements(self):
        # only level 2 ($2): yellow and black, nothing else
        colours = {tuple(self.only_2[o:o + 3])
                   for o in range(0, len(self.only_2), 4)}
        self.assertEqual(colours, {(0, 0, 0), (255, 255, 0)})

    def test_3b_viewer_defaults_keep_sub_cut_sources_as_washes(self):
        # the sub-cut mark used to vanish quietly (an empty pass);
        # since step 4 (2026-09-10) the jobdeck wide-view policy keeps
        # its existence as a footprint wash in its own colour - the
        # off-switch case is WideViewTests
        colours = {tuple(self.gui_defaults[o:o + 3])
                   for o in range(0, len(self.gui_defaults), 4)}
        self.assertIn((0, 0, 255), colours)      # $1 chips are drawn
        self.assertIn((255, 255, 0), colours)    # $2
        self.assertIn((255, 192, 203), colours)  # $5
        self.assertIn((255, 0, 0), colours)      # the sub-cut mark too

    def test_4_composite_equals_flattened_single_cache_render(self):
        import klayout.db as db
        flat = CLI / "deck_flat.oas"
        # KLayout holds int32 coordinates: at the deck's 2.5e-5 um grid
        # the deck (105 mm) overflows, so the oracle layout uses 1e-4 um
        # (every fixture edge is a multiple of it). The renderer maps
        # um the same way at either grid, so the pixels must agree.
        oracle_dbu = 1e-4
        lay = db.Layout()
        lay.dbu = oracle_dbu
        top = lay.create_cell("DECK")
        srcs = {}
        for p in self.placements:
            if p.tc not in srcs:
                s = db.Layout()
                s.read(str(CLI / p.tc))
                srcs[p.tc] = s
            s = srcs[p.tc]
            dst = lay.layer(1000 + self.out_of(p), 0)
            cell = s.top_cell()
            for sh in cell.shapes(s.layer(p.ly, p.dt)).each():
                b = sh.dbbox()          # source um
                top.shapes(dst).insert(db.DBox(
                    p.mag * b.left + p.dx_um, p.mag * b.bottom + p.dy_um,
                    p.mag * b.right + p.dx_um, p.mag * b.top + p.dy_um))
        lay.write(str(flat))
        res = subprocess.run(
            [sys.executable, "-B", "-m", "floe2", "index", str(flat),
             "--jobs", "2"], cwd=ROOT, capture_output=True, text=True,
            env={**os.environ, **self.env, "PYTHONPATH": str(ROOT)})
        self.assertEqual(res.returncode, 0, res.stderr)
        from floe.rust_render import RustRenderWorker
        c = Cache(str(flat))
        self.assertTrue(c.exists())
        c.load()
        worker = RustRenderWorker(c)
        worker.start()
        try:
            colours = [((1000 + row["out"], 0), row["color"])
                       for row in self.rows]
            worker.submit({"kind": "recolor", "colors": colours})
            oracle = _render_raw(worker,
                                 tuple(v / oracle_dbu for v in self.bbox_um),
                                 self.W, self.H)
        finally:
            worker.stop()
        if oracle != self.composite:
            diff = sum(1 for o in range(0, len(oracle), 4)
                       if oracle[o:o + 4] != self.composite[o:o + 4])
            self.fail("composite differs from the flattened oracle in %d "
                      "of %d pixels" % (diff, self.W * self.H))

    def test_5_ordinary_commands_take_a_deck(self):
        # floe2 render deck.jb: the standard --bbox (um) / --px / --out
        out = CLI / "deck.png"
        res = run_floe2("render", CLI / "test.jb", "--bbox",
                        "40000,80000,60000,95000", "--px", "300",
                        "--layers", "$1 METAL1,$3 ALIGN", "--out", out,
                        env=self.env, ok=0)
        self.assertIn("rendered", res.stdout)
        data = out.read_bytes()
        self.assertTrue(data.startswith(b"\x89PNG\r\n\x1a\n"))
        import struct
        w, h = struct.unpack(">II", data[16:24])
        self.assertEqual((w, h), (300, 225))
        # floe2 info deck.jb
        res = run_floe2("info", CLI / "test.jb", env=self.env, ok=0)
        self.assertIn("[jobdeck] chips     : 3", res.stdout)
        self.assertIn("$1 METAL1", res.stdout)
        self.assertIn("bbox       : (37020.0, 80020.0)", res.stdout)
        # floe2 index deck.jb: every source, current caches kept
        res = run_floe2("index", CLI / "test.jb", "--jobs", "2",
                        env=self.env, ok=0)
        self.assertIn("index     : 0 built, 0 failed, 3 kept", res.stdout)
        # a deck naming an unindexed source: exit 3 with the ledger, and
        # the spec still lists what could be drawn
        res = run_cli(CLI / "test_formats.jb", "--id", "1,2", "--spec",
                      CLI / "formats.spec", env=self.env, ok=3)
        self.assertIn("chipA.gds: unsupported", res.stdout)
        self.assertIn("1 placement(s)", res.stdout)
        # a deck with missing / unsupported sources OPENS once its
        # OASIS sources are indexed (field 2026-09-09: three 'file not
        # found' sources kept a fully indexed deck closed)
        from floe.jobdeck.viewer import DeckCache, deck_ready
        self.assertTrue(deck_ready(str(CLI / "test_formats.jb")))
        c = DeckCache(str(CLI / "test_formats.jb"))
        self.assertEqual(c.unindexed(), [])
        c.load()
        try:
            self.assertTrue(c.incomplete)
            self.assertEqual(sorted(r["reason"] for r in c.skipped),
                             ["missing", "unknown_format", "unsupported",
                              "unsupported", "unsupported"])
            self.assertEqual(c.meta["jobdeck"]["placements"], 1)
        finally:
            c.close()
        res = run_floe2("info", CLI / "test_formats.jb", env=self.env, ok=0)
        self.assertIn("INCOMPLETE: 5 placement(s) will not be drawn",
                      res.stdout)
        # (`floe2 view` on a deck without an index starts the viewer
        # and asks - a GUI path, checked by IndexOnOpenSmokeTests under
        # its display guard; review 2026-09-09 (4th) P2-3)


class ViewerCacheTests(unittest.TestCase):
    """floe.jobdeck.viewer.DeckCache: the Cache the viewer opens."""

    @classmethod
    def setUpClass(cls):
        cls.env = {"FLOE_INDEX_BIN": str(ROOT / "rust" / "target" /
                                         "release" / "floe-index")}
        run_floe2("index", CLI / "test.jb", "--jobs", "2", env=cls.env,
                  ok=0)

    def test_meta_and_modes(self):
        from floe.jobdeck.viewer import DeckCache, deck_ready, is_deck_path
        self.assertTrue(is_deck_path("a/b.JB"))
        self.assertFalse(is_deck_path("a/b.oas"))
        self.assertTrue(deck_ready(str(CLI / "test.jb")))
        # formats: its one OASIS source is indexed (CliTests), the rest
        # are skips - ready
        self.assertTrue(deck_ready(str(CLI / "test_formats.jb")))
        c = DeckCache(str(CLI / "test.jb"))
        self.assertTrue(c.is_jobdeck)
        self.assertEqual(c.unindexed(), [])
        try:
            meta = c.load()
            self.assertEqual(meta["dbu"], 2.5e-05)
            self.assertEqual(meta["bbox"], [int(37020 / 2.5e-5),
                                            int(80020 / 2.5e-5),
                                            int(69020 / 2.5e-5),
                                            int(105200 / 2.5e-5)])
            self.assertEqual([l["name"] for l in meta["layers"]],
                             ["$1 METAL1", "$2 VIA1", "$3 ALIGN", "$5"])
            self.assertEqual([l["color"] for l in meta["layers"]],
                             ["#0000ff", "#ffff00", "#ff0000", "#ffc0cb"])
            self.assertEqual([l["stored_shapes"] for l in meta["layers"]],
                             [6, 5, 3, 3])
            self.assertEqual(meta["grid"]["nx"], 1)
            self.assertTrue(meta["vfs"])
            self.assertEqual(meta["jobdeck"]["placements"], 17)
            self.assertEqual(meta["jobdeck"]["skipped"], [])
            self.assertTrue(os.path.isfile(c.dir))
            self.assertFalse(c.is_stale())
            self.assertEqual(c.resolve_layers("$2 VIA1,3/0"),
                             [(2, 0), (3, 0)])
            self.assertIsNone(c.resolve_layers("all"))
            with self.assertRaises(ValueError):
                c.resolve_layers("nope")
            self.assertEqual(c.mode, "level")
            self.assertEqual([(l["layer"], l["datatype"])
                              for l in meta["layers"]],
                             [(1, 0), (2, 0), (3, 0), (5, 0)])
            # chip view: the CHIPs in deck order, each expanding into
            # the levels it places (<pos>/<level>), MDPView chip colours
            meta = c.set_mode("chip")
            self.assertEqual([l["name"] for l in meta["layers"]],
                             ["CHIP ID001", "$1 METAL1", "$2 VIA1",
                              "$3 ALIGN", "CHIP ID002", "$1 METAL1",
                              "$2 VIA1", "$5", "CHIP ID003", "$1 METAL1",
                              "$3 ALIGN"])
            self.assertEqual([(l["layer"], l["datatype"])
                              for l in meta["layers"]],
                             [(1, 0), (1, 1), (1, 2), (1, 3), (2, 0), (2, 1),
                              (2, 2), (2, 5), (3, 0), (3, 1), (3, 3)])
            self.assertEqual([l["color"] for l in meta["layers"]],
                             ["#ffff00"] * 4 + ["#ffc0cb"] * 4
                             + ["#ffffff"] * 3)
            self.assertEqual([l["stored_shapes"] for l in meta["layers"]],
                             [0, 2, 2, 2, 0, 3, 3, 3, 0, 1, 1])
            self.assertTrue(c.dir.endswith("deck-chip.spec"))
            self.assertEqual(c.resolve_layers("CHIP ID002,3/1"),
                             [(2, 0), (2, 1), (2, 2), (2, 5), (3, 1)])
            with open(c.dir) as fh:
                spec = fh.read()
            self.assertIn("layer out=0 key=1/0 name_hex=", spec)
            self.assertEqual(spec.count("\nlayer "), 11)
            # our source-layer view: LY groups with DT children
            meta = c.set_mode("layer")
            self.assertEqual([l["name"] for l in meta["layers"]],
                             ["LY7.DT2", "LY123.DT43", "LY456.DT0",
                              "LY999.DT0"])
            self.assertEqual([(l["layer"], l["datatype"])
                              for l in meta["layers"]],
                             [(7, 2), (123, 43), (456, 0), (999, 0)])
            # the old name still opens the level view
            meta = c.set_mode("identifier")
            self.assertEqual(c.mode, "level")
            with self.assertRaises(ValueError):
                c.set_mode("rainbow")
        finally:
            c.close()
        self.assertFalse(os.path.exists(c.dir))

    def test_worker_factory_routes_a_deck(self):
        from floe.jobdeck.viewer import DeckCache
        from floe.service import make_render_worker
        c = DeckCache(str(CLI / "test.jb"))
        c.load()
        try:
            os.environ["FLOE_RENDERER"] = "rust"
            worker = make_render_worker(c)
            self.assertFalse(worker.supports_margin_prefetch)
            self.assertIn("open deck=", worker._open_command())
        finally:
            c.close()


class GuiSmokeTests(unittest.TestCase):
    """`floe2 view deck.jb` really opens: GTK start, deck worker open,
    first composite frame displayed (FLOE_GUI_SMOKE_MS)."""

    def test_view_smoke(self):
        try:
            import gi
            gi.require_version("Gtk", "3.0")
            from gi.repository import Gtk  # noqa: F401
        except (ImportError, ValueError):
            raise unittest.SkipTest("PyGObject/GTK is not importable")
        if sys.platform.startswith("linux") and not os.environ.get(
                "DISPLAY") and not os.environ.get("WAYLAND_DISPLAY"):
            raise unittest.SkipTest("no display")
        env = {"FLOE_INDEX_BIN": str(ROOT / "rust" / "target" / "release" /
                                     "floe-index"),
               "FLOE_RENDERD_BIN": str(ROOT / "rust" / "target" /
                                       "release" / "floe-renderd"),
               "FLOE_GUI_SMOKE_MS": "8000"}
        run_floe2("index", CLI / "test.jb", "--jobs", "2", env=env, ok=0)
        res = run_floe2("view", "--multi", CLI / "test.jb", env=env, ok=0,
                        timeout=120)
        self.assertNotIn("no GUI frame", res.stderr + res.stdout)


class IndexOnOpenSmokeTests(unittest.TestCase):
    """`floe2 view <file>` without an index starts the viewer, asks
    (FLOE_INDEX_ON_OPEN answers for the gate), indexes in the modal
    log, opens and shows a frame - for a layout and for a jobdeck."""

    def _env(self, policy):
        return {"FLOE_INDEX_BIN": str(ROOT / "rust" / "target" / "release" /
                                      "floe-index"),
                "FLOE_RENDERD_BIN": str(ROOT / "rust" / "target" /
                                        "release" / "floe-renderd"),
                "FLOE_GUI_SMOKE_MS": "20000",
                "FLOE_INDEX_ON_OPEN": policy}

    def _gtk(self):
        try:
            import gi
            gi.require_version("Gtk", "3.0")
            from gi.repository import Gtk  # noqa: F401
        except (ImportError, ValueError):
            raise unittest.SkipTest("PyGObject/GTK is not importable")
        if sys.platform.startswith("linux") and not os.environ.get(
                "DISPLAY") and not os.environ.get("WAYLAND_DISPLAY"):
            raise unittest.SkipTest("no display")

    def test_unindexed_layout_and_deck_open_after_indexing(self):
        self._gtk()
        fresh = CLI / "fresh"
        fresh.mkdir(exist_ok=True)
        for name in ("chipA.oas", "chipB.oas", "mark.oas", "test.jb",
                     "test_formats.jb", "chipA.gds"):
            shutil.copy2(CLI / name, fresh / name)
        self.assertFalse((fresh / "chipA.oas.floe").exists())
        # declined (policy no): the viewer stays empty, the smoke says so
        res = run_floe2("view", "--multi", fresh / "chipA.oas",
                        env=self._env("no"), ok=1, timeout=120)
        self.assertIn("pending open never landed", res.stderr + res.stdout)
        self.assertFalse((fresh / "chipA.oas.floe").exists())
        # the same for a deck whose OASIS source lacks an index (it
        # used to be refused in the terminal)
        res = run_floe2("view", "--multi", fresh / "test_formats.jb",
                        env=dict(self._env("no"), FLOE_GUI_SMOKE_MS="3000"),
                        ok=1, timeout=120)
        self.assertIn("pending open never landed", res.stderr + res.stdout)
        # once that source is indexed the deck opens although three of
        # its sources are unsupported containers and one is missing
        run_floe2("index", fresh / "test_formats.jb", "--jobs", "2",
                  env=self._env("no"), ok=0)
        run_floe2("view", "--multi", fresh / "test_formats.jb",
                  env=dict(self._env("no"), FLOE_GUI_SMOKE_MS="8000"),
                  ok=0, timeout=120)
        # accepted, with a --drc db beside an unindexed layout: indexed,
        # opened, a frame shown, and the DRC results still loaded after
        # the open (review 2026-09-09 (4th) P2-1: they were reset by it)
        db = fresh / "chipA.db"
        res = subprocess.run(
            [sys.executable, "-B", str(ROOT / "tools" / "gen_drc_db.py"),
             str(fresh / "chipA.oas"), str(db), "--checks", "2", "--per",
             "3"], cwd=ROOT, capture_output=True, text=True,
            env={**os.environ, "PYTHONPATH": str(ROOT)})
        self.assertEqual(res.returncode, 0, res.stderr)
        run_floe2("view", "--multi", fresh / "chipA.oas", "--goto",
                  "1000,1000,500", "--drc", db, env=self._env("yes"),
                  ok=0, timeout=180)
        self.assertTrue((fresh / "chipA.oas.floe" / "meta.json").is_file())
        self.assertTrue((fresh / "chipA.db.ice").exists(),
                        "the DRC pack was built without asking (policy yes)")
        # a jobdeck: every source indexed through `floe2 index deck.jb`
        run_floe2("view", "--multi", fresh / "test.jb", env=self._env("yes"),
                  ok=0, timeout=180)
        for name in ("chipB.oas", "mark.oas"):
            self.assertTrue((fresh / (name + ".floe") / "meta.json").is_file(),
                            name)


class ShotTests(unittest.TestCase):
    """floe.shots (jobdeck M4, generic for any source): units, anchors,
    aspect fitting, mosaic seams, batch files, and `floe2 render` with
    the new region forms on a deck and on a plain layout."""

    @classmethod
    def setUpClass(cls):
        cls.env = {"FLOE_INDEX_BIN": str(ROOT / "rust" / "target" /
                                         "release" / "floe-index"),
                   "FLOE_RENDERD_BIN": str(ROOT / "rust" / "target" /
                                           "release" / "floe-renderd")}
        run_floe2("index", CLI / "test.jb", "--jobs", "2", env=cls.env,
                  ok=0)

    def test_units_regions_and_aspect(self):
        from floe import shots as sh
        self.assertEqual(sh.parse_length("8mm"), 8000.0)
        self.assertEqual(sh.parse_length("500nm"), 0.5)
        self.assertEqual(sh.parse_length("32µm"), 32.0)
        self.assertEqual(sh.parse_length("32μm"), 32.0)
        self.assertEqual(sh.parse_length("1cm"), 10000.0)
        self.assertEqual(sh.parse_length(" 7 "), 7.0)
        self.assertEqual(sh.parse_lengths("53.02mm,92.61mm", 2, "at"),
                         (53020.0, 92610.0))
        self.assertEqual(sh.region_from((100, 200), (40, 20)),
                         (80.0, 190.0, 120.0, 210.0))
        self.assertEqual(sh.region_from((100, 200), (40, 20), "lb"),
                         (100.0, 200.0, 140.0, 220.0))
        # expand, never stretch: centre held / lb corner held
        self.assertEqual(sh.fit_aspect((0, 0, 100, 100), 200, 100),
                         (-50.0, 0.0, 150.0, 100.0))
        self.assertEqual(sh.fit_aspect((0, 0, 100, 100), 200, 100, "lb"),
                         (0.0, 0.0, 200.0, 100.0))
        self.assertEqual(sh.fit_aspect((0, 0, 100, 100), 100, 200),
                         (0.0, -50.0, 100.0, 150.0))
        self.assertEqual(sh.fit_aspect((0, 0, 100, 100), 200, 100,
                                       stretch=True),
                         (0.0, 0.0, 100.0, 100.0))
        self.assertEqual(sh.pixel_size((0, 0, 400, 300), 800, None),
                         (800, 600))
        self.assertEqual(sh.parse_pixel("1200x900"), (1200, 900))
        self.assertEqual(sh.parse_pixel("640"), (640, None))
        with self.assertRaises(ValueError):
            sh.parse_points("1,2;3,4")
        # corners: the region's four W,H corner rectangles, inside it
        # (review 2026-09-09 P2-5: tiles centred on the corners shot
        # half outside the region; the reference tool's tl of
        # corners=0,0,100,100 size=20,10 is (0,90,20,100))
        shot = sh.Shot("m", corners=(0, 0, 100, 100), size=(20, 10),
                       px=(20, 10))
        boxes, (w, h) = shot.tile_boxes((0, 0, 1, 1))
        self.assertEqual((w, h), (20, 10))
        self.assertEqual(boxes, [(0.0, 90.0, 20.0, 100.0),    # tl
                                 (80.0, 90.0, 100.0, 100.0),  # tr
                                 (0.0, 0.0, 20.0, 10.0),      # bl
                                 (80.0, 0.0, 100.0, 10.0)])   # br
        # mosaic-at: clockwise input tl, tr, br, bl -> canvas rows
        # tl, tr / bl, br (review 2026-09-09 P2-4: the bottom row was
        # swapped)
        shot = sh.Shot("m", mosaic=[(10, 90), (90, 90), (90, 10), (10, 10)],
                       size=(20, 10), px=(20, 10))
        boxes, _ = shot.tile_boxes((0, 0, 1, 1))
        self.assertEqual(boxes[2], (0.0, 5.0, 20.0, 15.0))    # bl = 4th point
        self.assertEqual(boxes[3], (80.0, 5.0, 100.0, 15.0))  # br = 3rd point
        with self.assertRaises(ValueError):
            sh.Shot("x", bbox=(0, 0, 1, 1), at=(0, 0), size=(1, 1))
        with self.assertRaises(ValueError):
            sh.Shot("x", at=(0, 0))

    def test_mosaic_lines_and_png(self):
        from floe import shots as sh
        self.assertEqual(sh.line_spans(100, 2, 200), [(99, 1.0), (100, 1.0)])
        self.assertEqual(sh.line_spans(100, 3, 200),
                         [(99, 1.0), (100, 1.0), (98, 0.5), (101, 0.5)])
        self.assertEqual(sh.line_spans(100, 0, 200), [])
        self.assertEqual(sh.line_spans(1, 4, 3), [(0, 1.0), (1, 1.0),
                                                   (2, 1.0)])
        red = bytes([255, 0, 0, 255]) * 4          # 2x2 tiles
        blue = bytes([0, 0, 255, 255]) * 4
        canvas, info = sh.compose_mosaic([red, blue, blue, red], 2, 2,
                                         line=1.0, color="#00ff00")
        self.assertEqual((info["width"], info["height"]), (4, 4))
        self.assertEqual(info["line_pixels"], "1 at 50% on each side")
        px = lambda x, y: tuple(canvas[(y * 4 + x) * 4:(y * 4 + x) * 4 + 3])
        self.assertEqual(px(0, 0), (255, 0, 0))
        self.assertEqual(px(3, 0), (0, 0, 255))
        self.assertEqual(px(3, 3), (255, 0, 0))
        # seam columns 1 and 2 blend 50% green over the tiles; row 1/2 too
        self.assertEqual(px(1, 0), (128, 128, 0))
        self.assertEqual(px(2, 0), (0, 128, 128))
        self.assertEqual(px(0, 1), (128, 128, 0))
        png = sh.png_encode(4, 4, canvas)
        self.assertTrue(png.startswith(b"\x89PNG\r\n\x1a\n"))
        import struct
        self.assertEqual(struct.unpack(">II", png[16:24]), (4, 4))
        idat = png.index(b"IDAT")
        length = struct.unpack(">I", png[idat - 4:idat])[0]
        raw = zlib.decompress(png[idat + 4:idat + 4 + length])
        self.assertEqual(len(raw), 4 * (1 + 16))
        self.assertEqual(raw[1:5], bytes([255, 0, 0, 255]))

    def test_batch_parsing(self):
        from floe import shots as sh
        text = """
        # comment
        left  at=40mm,85mm size=8000,6000 px=400x300 layers="$1 METAL1"
        right at=60mm,85mm size=8000,6000 anchor=lb depth=0
        quad  corners=40000,80000,60000,95000 size=4000,3000 line=3 keep_tiles=1
        """
        shots = sh.parse_batch(text, {"px": "200", "bbox": "1,2,3,4"})
        self.assertEqual([x.name for x in shots], ["left", "right", "quad"])
        self.assertEqual(shots[0].px, (400, 300))
        self.assertEqual(shots[0].layers, "$1 METAL1")
        self.assertIsNone(shots[0].bbox, "a line's own form drops bbox")
        self.assertEqual(shots[1].anchor, "lb")
        self.assertEqual(shots[1].depth, 0)
        self.assertEqual(shots[1].px, (200, None))
        self.assertTrue(shots[2].is_mosaic)
        self.assertTrue(shots[2].keep_tiles)
        self.assertEqual(shots[2].line, 3.0)
        with self.assertRaises(ValueError):
            sh.parse_batch("a bbox=1,2,3,4\na bbox=1,2,3,4")
        with self.assertRaises(ValueError):
            sh.parse_batch("a rot=1")
        with self.assertRaises(ValueError):
            sh.parse_batch("")

    def _png_size(self, path):
        import struct
        data = Path(path).read_bytes()
        self.assertTrue(data.startswith(b"\x89PNG\r\n\x1a\n"), path)
        return struct.unpack(">II", data[16:24])

    def test_cli_region_forms_on_a_deck(self):
        out = CLI / "at.png"
        res = run_floe2("render", CLI / "test.jb", "--at", "53.02mm,92.61mm",
                        "--size", "20mm,10mm", "--px", "400x200", "--out",
                        out, env=self.env, ok=0)
        self.assertEqual(self._png_size(out), (400, 200))
        self.assertIn("43020.0000,87610.0000,63020.0000,97610.0000 um",
                      res.stdout)
        # lb anchor + expansion keeps the corner: 100x100 um in 200x100 px
        res = run_floe2("render", CLI / "test.jb", "--at", "41020,80020",
                        "--size", "100,100", "--anchor", "lb", "--px",
                        "200x100", "--out", out, env=self.env, ok=0)
        self.assertIn("41020.0000,80020.0000,41220.0000,80120.0000 um",
                      res.stdout)
        # no region: the whole deck at the bbox aspect
        res = run_floe2("render", CLI / "test.jb", "--px", "320", "--out",
                        out, env=self.env, ok=0)
        w, h = self._png_size(out)
        self.assertEqual(w, 320)
        self.assertEqual(h, round(320 * (105200 - 80020) / (69020 - 37020)))
        # mosaic: four corners of a region, seams over the tile edges
        out = CLI / "quad.png"
        res = run_floe2("render", CLI / "test.jb", "--corners",
                        "40000,80000,60000,95000", "--size", "4000,3000",
                        "--px", "200x150", "--line", "3", "--keep-tiles",
                        "--out", out, "--report", CLI / "quad.json",
                        env=self.env, ok=0)
        self.assertEqual(self._png_size(out), (400, 300))
        for tag in ("tl", "tr", "bl", "br"):
            self.assertEqual(self._png_size(CLI / ("quad_%s.png" % tag)),
                             (200, 150))
        rep = json.loads((CLI / "quad.json").read_text())
        self.assertEqual(rep["shots"][0]["mosaic"]["line_pixels"],
                         "1 solid + 1 at 50% on each side")
        self.assertEqual(rep["shots"][0]["tiles"]["tl"],
                         [40000.0, 92000.0, 44000.0, 95000.0])
        self.assertEqual(rep["shots"][0]["tiles"]["br"],
                         [56000.0, 80000.0, 60000.0, 83000.0])
        # --mosaic-at: the third point is the bottom-RIGHT tile
        res = run_floe2("render", CLI / "test.jb", "--mosaic-at",
                        "42000,94000;58000,94000;58000,82000;42000,82000",
                        "--size", "4000,3000", "--px", "40x30",
                        "--keep-tiles", "--out", CLI / "quad2.png",
                        "--report", CLI / "quad2.json", env=self.env, ok=0)
        rep = json.loads((CLI / "quad2.json").read_text())
        self.assertEqual(rep["shots"][0]["tiles"]["br"],
                         [56000.0, 80500.0, 60000.0, 83500.0])
        self.assertEqual(rep["shots"][0]["tiles"]["bl"],
                         [40000.0, 80500.0, 44000.0, 83500.0])
        self.assertEqual(rep["jobdeck"]["complete"], True)
        # exclusive forms and a bad layer are refused
        run_floe2("render", CLI / "test.jb", "--bbox", "1,2,3,4", "--at",
                  "1,2", "--size", "1,1", env=self.env, ok=1)
        run_floe2("render", CLI / "test.jb", "--layers", "nope", "--out",
                  out, env=self.env, ok=1)

    def test_cli_batch_and_plain_layout(self):
        batch = CLI / "shots.txt"
        batch.write_text(
            "left  at=45020,86000 size=8mm,6mm layers=\"$1 METAL1\"\n"
            "mark  bbox=45000,85100,45040,85140 px=64x64\n"
            "quad  corners=40000,80000,60000,95000 size=4mm,3mm\n")
        outdir = CLI / "shots"
        res = run_floe2("render", CLI / "test.jb", "--batch", batch, "--px",
                        "200x150", "--out", outdir, "--report",
                        outdir / "report.json", env=self.env, ok=0)
        self.assertEqual(self._png_size(outdir / "left.png"), (200, 150))
        self.assertEqual(self._png_size(outdir / "mark.png"), (64, 64))
        self.assertEqual(self._png_size(outdir / "quad.png"), (400, 300))
        rep = json.loads((outdir / "report.json").read_text())
        self.assertEqual([r["name"] for r in rep["shots"]],
                         ["left", "mark", "quad"])
        self.assertEqual(rep["shots"][0]["layers"], "$1 METAL1")
        self.assertIn("3 shot(s)", res.stdout)
        run_floe2("render", CLI / "test.jb", "--batch", batch, "--out",
                  CLI / "x.png", env=self.env, ok=1)
        # a plain layout renders through the same path (the floe rule:
        # height from the bbox aspect)
        out = CLI / "chipA.png"
        run_floe2("render", CLI / "chipA.oas", "--bbox", "0,0,2000,2550",
                  "--px", "200", "--layers", "123/43", "--out", out,
                  env=self.env, ok=0)
        self.assertEqual(self._png_size(out), (200, 255))


class ReviewFixTests(unittest.TestCase):
    """Review 2026-09-09 (six findings): each one pinned."""

    @classmethod
    def setUpClass(cls):
        cls.env = {"FLOE_INDEX_BIN": str(ROOT / "rust" / "target" /
                                         "release" / "floe-index"),
                   "FLOE_RENDERD_BIN": str(ROOT / "rust" / "target" /
                                           "release" / "floe-renderd")}
        os.environ["FLOE_RENDERD_BIN"] = cls.env["FLOE_RENDERD_BIN"]
        for deck in ("test.jb", "dense.jb", "hier.jb"):
            run_floe2("index", CLI / deck, "--jobs", "2", env=cls.env, ok=0)

    def test_p1_1_incomplete_render_is_said_and_exits_3(self):
        out = CLI / "ghost.png"
        rep = CLI / "ghost.json"
        res = run_floe2("render", CLI / "test_missing_layer.jb", "--px",
                        "100", "--out", out, "--report", rep, env=self.env,
                        ok=3)
        self.assertIn("skipped   : CHIP ID001 $2 chipA.oas: empty_layer",
                      res.stdout)
        self.assertIn("WARNING: 1 jobdeck placement(s) not drawn",
                      res.stdout)
        self.assertIn("rendered incomplete - 1 jobdeck placement(s) missing",
                      res.stderr)
        self.assertTrue(out.is_file(), "the partial PNG is still written")
        doc = json.loads(rep.read_text())
        self.assertFalse(doc["jobdeck"]["complete"])
        # the shot row agrees with the report (review 2026-09-09: a
        # per-shot reader saw complete=true under an incomplete report)
        self.assertFalse(doc["complete"])
        self.assertFalse(doc["shots"][0]["complete"])
        self.assertEqual(doc["shots"][0]["skipped_placements"], 1)
        self.assertIn("INCOMPLETE", res.stdout)
        self.assertEqual(doc["jobdeck"]["skipped"][0]["reason"],
                         "empty_layer")
        self.assertEqual(doc["jobdeck"]["skipped"][0]["ly"], 987)
        res = run_floe2("info", CLI / "test_missing_layer.jb", env=self.env,
                        ok=0)
        self.assertIn("INCOMPLETE: 1 placement(s) will not be drawn",
                      res.stdout)
        # a complete deck: exit 0 and complete=true
        res = run_floe2("render", CLI / "test.jb", "--px", "50", "--out",
                        out, "--report", rep, env=self.env, ok=0)
        self.assertNotIn("WARNING", res.stdout)
        self.assertTrue(json.loads(rep.read_text())["jobdeck"]["complete"])
        from floe.jobdeck.viewer import DeckCache
        c = DeckCache(str(CLI / "test_missing_layer.jb"))
        c.load()
        try:
            self.assertTrue(c.incomplete)
            self.assertEqual(len(c.skipped), 1)
            self.assertEqual(len(c.meta["jobdeck"]["skipped"]), 1)
        finally:
            c.close()

    def test_p1_2_deck_pass_is_charged_against_the_page_budget(self):
        from floe.rust_render import RustRenderWorker
        from floe.jobdeck.viewer import DeckCache
        os.environ["FLOE_RUST_BUDGET_MB"] = "1"
        try:
            # the plain layout is refused at 1 MiB ...
            c = Cache(str(CLI / "dense.oas"))
            c.load()
            worker = RustRenderWorker(c)
            worker.start()
            try:
                with self.assertRaises(AssertionError) as cm:
                    _render_raw(worker, (0, 0, 2003 / 5e-5, 2003 / 5e-5),
                                200, 200)
                self.assertIn("decoded generation budget exceeded",
                              str(cm.exception))
            finally:
                worker.stop()
            # ... the deck placing it is charged the same way, but
            # (step 3, 2026-09-10) a pass over the slice limit is no
            # longer cut off: it streams its pages slice by slice and
            # the frame is complete (field 2026-09-09: a mid-zoom pass
            # wanted 2 GB and the refusal left the viewer with an
            # error; the partial frame that replaced it drew a part)
            d = DeckCache(str(CLI / "dense.jb"))
            d.load()
            try:
                worker = jrender.DeckRenderWorker(d)
                worker.start()
                try:
                    bb = d.meta["bbox"]
                    small, result = _render_raw(worker, tuple(bb), 200, 200,
                                                with_result=True)
                    self.assertEqual(result.get("over_budget_pages", 0), 0)
                    self.assertGreater(result["deck"]["streamed_passes"], 0)
                    self.assertGreater(result["deck"]["slices"], 1)
                    self.assertFalse(result.get("refining"))
                finally:
                    worker.stop()
            finally:
                d.close()
        finally:
            del os.environ["FLOE_RUST_BUDGET_MB"]
        # with the normal budget the deck renders everything in one
        # scene - the same pixels
        c = DeckCache(str(CLI / "dense.jb"))
        c.load()
        try:
            worker = jrender.DeckRenderWorker(c)
            worker.start()
            try:
                rgba, result = _render_raw(worker, tuple(c.meta["bbox"]),
                                           200, 200, with_result=True)
            finally:
                worker.stop()
        finally:
            c.close()
        self.assertEqual(result.get("over_budget_pages", 0), 0)
        self.assertEqual(result["deck"]["streamed_passes"], 0)
        self.assertTrue(any(rgba[o:o + 3] != b"\0\0\0"
                            for o in range(0, len(rgba), 4)))
        self.assertEqual(small, rgba, "streamed slices = the whole scene")

    def test_p1_3_hierarchical_source_shows_at_the_defaults(self):
        from floe.jobdeck.viewer import DeckCache
        c = DeckCache(str(CLI / "hier.jb"))
        c.load()
        try:
            worker = jrender.DeckRenderWorker(c)
            worker.start()
            try:
                bb = tuple(c.meta["bbox"])

                def lit(rgba):
                    return sum(1 for o in range(0, len(rgba), 4)
                               if rgba[o:o + 3] != b"\0\0\0")
                # full depth (the render default, and now the deck's view
                # default): the child's box is drawn
                self.assertGreater(lit(_render_raw(worker, bb, 64, 64)), 1000)
                # depth 0 without frames was the black screen ...
                self.assertEqual(lit(_render_raw(worker, bb, 64, 64,
                                                 depth=0)), 0)
                # ... and with the viewer's frames on, depth 0 now shows
                # the child cell's hierarchy frame instead of nothing
                worker.submit({"kind": "render", "gen": 999, "bbox": bb,
                               "view": None, "w": 64, "h": 64, "depth": 0,
                               "cut_px": 0.0, "lod": False, "frames": True,
                               "labels": False, "abstract": False,
                               "visible": None, "frame_format": "raw",
                               "scope": "headless"})
                while True:
                    r = worker.res.get(timeout=120)
                    if r.get("kind") == "error":
                        self.fail(r.get("msg"))
                    if r.get("kind") == "frame" and r.get("gen") == 999 \
                            and not r.get("refining"):
                        break
                self.assertGreater(lit(r["rgba"]), 0)
            finally:
                worker.stop()
        finally:
            c.close()
        # `floe2 render hier.jb` (depth default full) is not black -
        # decoded, RGB only (review 2026-09-09 (2nd) P2-4: the raw IDAT
        # check counted alpha bytes and passed an all-black PNG)
        out = CLI / "hier.png"
        run_floe2("render", CLI / "hier.jb", "--px", "32", "--out", out,
                  env=self.env, ok=0)
        self.assertGreater(_png_lit_pixels(out), 0, "the PNG is all black")

    def test_p2_6_one_line_batch_writes_into_a_directory(self):
        batch = CLI / "one.txt"
        batch.write_text("only bbox=45000,85000,46000,86000 px=16x16\n")
        outdir = CLI / "one-shot-dir"
        self.assertFalse(outdir.exists())
        run_floe2("render", CLI / "test.jb", "--batch", batch, "--out",
                  outdir, env=self.env, ok=0)
        self.assertTrue(outdir.is_dir())
        self.assertTrue((outdir / "only.png").is_file())


def _png_lit_pixels(path):
    """Non-black RGB pixels of a PNG (Pillow; the battery's dev env)."""
    from PIL import Image
    img = Image.open(path).convert("RGB")
    return sum(1 for px in img.getdata() if px != (0, 0, 0))


def _lit(rgba):
    return sum(1 for o in range(0, len(rgba), 4) if rgba[o:o + 3] != b"\0\0\0")


def _white(rgba):
    return sum(1 for o in range(0, len(rgba), 4)
               if rgba[o:o + 3] == b"\xff\xff\xff")


class ReviewFixTests2(unittest.TestCase):
    """Review 2026-09-09, second pass (five findings): each pinned."""

    @classmethod
    def setUpClass(cls):
        cls.env = {"FLOE_INDEX_BIN": str(ROOT / "rust" / "target" /
                                         "release" / "floe-index"),
                   "FLOE_RENDERD_BIN": str(ROOT / "rust" / "target" /
                                           "release" / "floe-renderd")}
        os.environ["FLOE_RENDERD_BIN"] = cls.env["FLOE_RENDERD_BIN"]
        for deck in ("test.jb", "frames.jb"):
            run_floe2("index", CLI / deck, "--jobs", "2", env=cls.env, ok=0)

    def test_p1_1_frame_order_is_kept_across_placements(self):
        from floe.jobdeck.viewer import DeckCache
        c = DeckCache(str(CLI / "frames.jb"))
        c.load()
        try:
            worker = jrender.DeckRenderWorker(c)
            worker.start()
            try:
                bb = tuple(c.meta["bbox"])
                # placement A alone: its child's white frame at depth 0
                a_only = _render_raw(worker, bb, 400, 400, depth=0,
                                     frames=True, visible=[(1, 0)])
                both = _render_raw(worker, bb, 400, 400, depth=0,
                                   frames=True)
                no_frames = _render_raw(worker, bb, 400, 400, depth=0)
            finally:
                worker.stop()
        finally:
            c.close()
        self.assertGreater(_white(a_only), 0)
        self.assertEqual(_white(no_frames), 0)
        # B's box covers A's child region: A's white frame must still be
        # on top (every white pixel of A survives, B adds its own)
        self.assertGreaterEqual(_white(both), _white(a_only),
                                "a later placement's geometry buried an "
                                "earlier placement's white frame")
        self.assertGreater(_lit(both), _lit(a_only))

    def test_p2_2_group_names_select_their_levels(self):
        from floe.jobdeck.viewer import DeckCache
        c = DeckCache(str(CLI / "test.jb"), mode="chip")
        c.load()
        try:
            # a CHIP row stands for its levels ...
            self.assertEqual(c.resolve_layers("CHIP ID002"),
                             [(2, 0), (2, 1), (2, 2), (2, 5)])
            self.assertEqual(c.resolve_layers("2/0"),
                             [(2, 0), (2, 1), (2, 2), (2, 5)])
            # ... a level name selects it in EVERY CHIP that places it
            self.assertEqual(c.resolve_layers("$1 METAL1"),
                             [(1, 1), (2, 1), (3, 1)])
            self.assertEqual(c.resolve_layers("$5"), [(2, 5)])
            with self.assertRaises(ValueError):
                c.resolve_layers("9/9")
            worker = jrender.DeckRenderWorker(c)
            worker.start()
            try:
                bb = tuple(c.meta["bbox"])
                head = _render_raw(worker, bb, 200, 160,
                                   visible=c.resolve_layers("CHIP ID002"))
            finally:
                worker.stop()
            self.assertGreater(_lit(head), 1000, "CHIP ID002 drew nothing")
        finally:
            c.close()
        # the level view: unique names, no groups
        c = DeckCache(str(CLI / "test.jb"))
        c.load()
        try:
            self.assertEqual(c.resolve_layers("$1 METAL1,$5"),
                             [(1, 0), (5, 0)])
        finally:
            c.close()

    def test_p2_3_saved_colours_come_back_per_view(self):
        from floe import fillpat
        from floe.jobdeck.viewer import DeckCache
        from floe.rust_render import RustRenderWorker
        props = CLI / "test.jb.layerprops"
        props.write_text(fillpat.format_layerprops(
            [((1, 0), "#123456", "solid", "$1 METAL1", "1", "3")]))
        try:
            c = DeckCache(str(CLI / "test.jb"))
            c.load()
            try:
                self.assertEqual(c.props_src, str(CLI / "test.jb"))
                colours = {(l["layer"], l["datatype"]): l["color"]
                           for l in c.meta["layers"]}
                self.assertEqual(colours[(1, 0)], "#123456")
                self.assertEqual(colours[(2, 0)], "#ffff00")
                # the worker keys its styles on the same meta: colour
                # and width both come back
                worker = jrender.DeckRenderWorker(c)
                self.assertEqual(worker._colors[(1, 0)], "#123456")
                self.assertEqual(worker._widths[(1, 0)], 3)
                # chip view keeps its own key space and file
                c.set_mode("chip")
                self.assertEqual(c.props_src, str(CLI / "test.chip.jb"))
                colours = {(l["layer"], l["datatype"]): l["color"]
                           for l in c.meta["layers"]}
                self.assertEqual(colours[(1, 0)], "#ffff00",
                                 "the level view's file leaked into "
                                 "the chip view")
            finally:
                c.close()
        finally:
            props.unlink()

    def test_p3_5_render_deck_png_keys_every_row(self):
        from PIL import Image
        from floe.jobdeck.viewer import DeckCache
        c = DeckCache(str(CLI / "test.jb"), mode="chip")
        c.load()
        try:
            bb = tuple(c.meta["bbox"])
            out = CLI / "chip-helper.png"
            jrender.render_deck_png(c.dir, c.src, c.meta["dbu"],
                                    c.meta["layers"], bb, 200, 160, str(out))
            worker = jrender.DeckRenderWorker(c)
            worker.start()
            try:
                raw = _render_raw(worker, bb, 200, 160)
            finally:
                worker.stop()
        finally:
            c.close()
        png = Image.open(out).convert("RGB").tobytes()
        rgb = bytes(b for o in range(0, len(raw), 4) for b in raw[o:o + 3])
        self.assertEqual(png, rgb, "the helper's fills differ from the "
                                   "solid archival render")


def _gray(rgba):
    return sum(1 for o in range(0, len(rgba), 4)
               if rgba[o:o + 3] == b"\x80\x80\x80")


class ReviewFixTests3(unittest.TestCase):
    """Review 2026-09-09, third pass (two findings)."""

    @classmethod
    def setUpClass(cls):
        cls.env = {"FLOE_INDEX_BIN": str(ROOT / "rust" / "target" /
                                         "release" / "floe-index"),
                   "FLOE_RENDERD_BIN": str(ROOT / "rust" / "target" /
                                           "release" / "floe-renderd")}
        os.environ["FLOE_RENDERD_BIN"] = cls.env["FLOE_RENDERD_BIN"]
        for deck in ("frames.jb", "dt.jb", "hier.jb"):
            run_floe2("index", CLI / deck, "--jobs", "2", env=cls.env, ok=0)

    def _gray_scale_view(self, cache, child_um, px=400, child_px=13.0):
        """A viewport where a child frame of `child_um` (deck um) is
        ~13 px on screen: the gray outline band (9..25 px), not the
        white one (>= 25 px)."""
        bb = cache.meta["bbox"]
        dbu = cache.meta["dbu"]
        span_um = child_um * px / child_px
        cx = (bb[0] + bb[2]) / 2.0
        cy = (bb[1] + bb[3]) / 2.0
        half = span_um / dbu / 2.0
        return (cx - half, cy - half, cx + half, cy + half), px

    def test_p2_1_black_design_covers_the_gray_frame(self):
        """Review 2026-09-09 (4th) P3-4: at the earlier scale the child
        frame was white, so the check proved nothing about gray. Here
        it is a gray band: with no design over it the gray shows
        (hier.jb: the child alone), with BLACK design over it the gray
        must be hidden exactly as a coloured design hides it."""
        from floe.jobdeck.viewer import DeckCache
        c = DeckCache(str(CLI / "hier.jb"))
        c.load()
        try:
            # hier.oas: KID is a 2000 um box, x4 on the deck
            view, px = self._gray_scale_view(c, 8000.0)
            worker = jrender.DeckRenderWorker(c)
            worker.start()
            try:
                bare = _render_raw(worker, view, px, px, depth=0, frames=True)
            finally:
                worker.stop()
        finally:
            c.close()
        self.assertGreater(_gray(bare), 0, "no gray band at this scale")
        self.assertEqual(_white(bare), 0)
        c = DeckCache(str(CLI / "frames.jb"))
        c.load()
        try:
            # hier2.oas: KID is a 200 um box, x4 on the deck, under the
            # top-level box of each placement
            view, px = self._gray_scale_view(c, 800.0)
            worker = jrender.DeckRenderWorker(c)
            worker.start()
            try:
                coloured = _render_raw(worker, view, px, px, depth=0,
                                       frames=True)
                worker.submit({"kind": "recolor",
                               "colors": [[[1, 0], "#000000"],
                                          [[2, 0], "#000000"]]})
                black = _render_raw(worker, view, px, px, depth=0,
                                    frames=True)
            finally:
                worker.stop()
        finally:
            c.close()
        self.assertGreater(_lit(coloured), 0)
        self.assertEqual(_gray(coloured), 0, "a coloured box hides the gray")
        self.assertEqual(_gray(black), 0, "a black box must hide it too")

    def test_p2_2_source_layer_dt0_is_a_layer_not_a_head(self):
        from floe.jobdeck.viewer import DeckCache
        c = DeckCache(str(CLI / "dt.jb"), mode="layer")
        c.load()
        try:
            self.assertEqual([(l["layer"], l["datatype"], l["jobdeck_head"])
                              for l in c.meta["layers"]],
                             [(7, 0, False), (7, 1, False)])
            self.assertEqual(c.resolve_layers("7/0"), [(7, 0)])
            self.assertEqual(c.resolve_layers("LY7.DT0"), [(7, 0)])
            self.assertEqual(c.resolve_layers("LY7.DT1"), [(7, 1)])
            worker = jrender.DeckRenderWorker(c)
            worker.start()
            try:
                bb = tuple(c.meta["bbox"])
                only_dt0 = _render_raw(worker, bb, 100, 100, visible=[(7, 0)])
                both = _render_raw(worker, bb, 100, 100)
            finally:
                worker.stop()
        finally:
            c.close()
        # DT0 is the full box, DT1 the 10% inset: painted over DT0 in
        # its own colour... both share LY7's colour here, so compare
        # the two selections by the layer-1 inset being absent/present
        self.assertGreater(_lit(only_dt0), 0)
        self.assertEqual(_lit(both), _lit(only_dt0),
                         "DT1 lies inside DT0: selecting DT0 alone must "
                         "not add DT1")
        # and a CHIP head in chip view still expands
        c = DeckCache(str(CLI / "dt.jb"), mode="chip")
        c.load()
        try:
            self.assertEqual([l["jobdeck_head"] for l in c.meta["layers"]],
                             [True, False])
            self.assertEqual(c.resolve_layers("CHIP D"), [(1, 0), (1, 1)])
        finally:
            c.close()


class PerfAnalysisTests(unittest.TestCase):
    """Analysis 2026-09-09 step 1 (pixel-neutral): empty frame passes
    are skipped, the deck's per-pass costs are reported, and a capture
    that stopped at the page budget is never a complete result."""

    @classmethod
    def setUpClass(cls):
        cls.env = {"FLOE_INDEX_BIN": str(ROOT / "rust" / "target" /
                                         "release" / "floe-index"),
                   "FLOE_RENDERD_BIN": str(ROOT / "rust" / "target" /
                                           "release" / "floe-renderd")}
        os.environ["FLOE_RENDERD_BIN"] = cls.env["FLOE_RENDERD_BIN"]
        for deck in ("test.jb", "hier.jb", "dense.jb"):
            run_floe2("index", CLI / deck, "--jobs", "2", env=cls.env, ok=0)

    def _deck_counters(self, deck, depth, frames):
        from floe.jobdeck.viewer import DeckCache
        c = DeckCache(str(CLI / deck))
        c.load()
        try:
            worker = jrender.DeckRenderWorker(c)
            worker.start()
            try:
                _, result = _render_raw(worker, tuple(c.meta["bbox"]), 200,
                                        200, depth=depth, frames=frames,
                                        with_result=True)
            finally:
                worker.stop()
        finally:
            c.close()
        return result["deck"]

    def test_frame_passes_only_where_frames_exist(self):
        flat = self._deck_counters("test.jb", 0, True)
        self.assertEqual(flat["passes"], 17)
        self.assertEqual(flat["frame_passes"], 0,
                         "flat sources have no hierarchy frame to raster")
        # 17 passes over three one-page caches: the summed count is
        # per pass, the unique count is what was really decoded
        # (pages are per cell and layer: the 17 passes share a handful)
        self.assertGreater(flat["pages_summed"], flat["unique_pages"])
        self.assertGreaterEqual(flat["unique_pages"], 3)
        self.assertGreater(flat["pass_bytes_max"], 0)
        hier = self._deck_counters("hier.jb", 0, True)
        self.assertEqual(hier["frame_passes"], 1)
        off = self._deck_counters("hier.jb", 0, False)
        self.assertEqual(off["frame_passes"], 0)

    def test_over_budget_capture_is_complete_by_streaming(self):
        # step 1 pinned "a capture that stopped at the page budget is
        # incomplete (exit 3)"; since step 3 (2026-09-10) a pass over
        # the slice limit streams instead of stopping, so the same
        # capture at a 1 MiB budget is complete and identical to the
        # default-budget one. The incomplete path stays for skipped
        # placements (ReviewFixTests).
        out = CLI / "dense-budget.png"
        rep = CLI / "dense-budget.json"
        res = run_floe2("render", CLI / "dense.jb", "--px", "120", "--out",
                        out, "--report", rep,
                        env=dict(self.env, FLOE_RUST_BUDGET_MB="1"), ok=0)
        self.assertNotIn("stopped at the page budget", res.stdout)
        doc = json.loads(rep.read_text())
        self.assertTrue(doc["complete"])
        self.assertTrue(doc["jobdeck"]["complete"])
        self.assertEqual(doc["shots"][0]["over_budget_pages"], 0)
        streamed = out.read_bytes()
        # the default budget: complete, exit 0
        res = run_floe2("render", CLI / "dense.jb", "--px", "120", "--out",
                        out, "--report", rep, env=self.env, ok=0)
        doc = json.loads(rep.read_text())
        self.assertTrue(doc["complete"])
        self.assertEqual(doc["shots"][0]["over_budget_pages"], 0)
        self.assertEqual(out.read_bytes(), streamed,
                         "the streamed capture is the whole-scene one")


class SubwindowTests(unittest.TestCase):
    """Step 2 (analysis 2026-09-09): every placement is rastered and
    composited only on the phase-aligned sub-window it can touch. The
    pixels must equal the full-frame path (FLOE_RUST_DECK_SUBWINDOW=off)
    byte for byte - solid, the viewer's speckle and a 16x16 pattern,
    outline width 3, frames on at depth 0 - on the odd-sized frames the
    phase rule depends on."""

    @classmethod
    def setUpClass(cls):
        cls.env = {"FLOE_INDEX_BIN": str(ROOT / "rust" / "target" /
                                         "release" / "floe-index"),
                   "FLOE_RENDERD_BIN": str(ROOT / "rust" / "target" /
                                           "release" / "floe-renderd")}
        os.environ["FLOE_RENDERD_BIN"] = cls.env["FLOE_RENDERD_BIN"]
        for deck in ("test.jb", "frames.jb"):
            run_floe2("index", CLI / deck, "--jobs", "2", env=cls.env, ok=0)

    def _frames(self, deck, switch, size, fill, width_px, frames, depth,
                bbox=None):
        from floe.jobdeck.viewer import DeckCache
        os.environ["FLOE_RUST_DECK_SUBWINDOW"] = switch
        try:
            c = DeckCache(str(CLI / deck))
            c.load()
            try:
                worker = jrender.DeckRenderWorker(c)
                worker.start()
                try:
                    bb = bbox or tuple(c.meta["bbox"])
                    rgba, result = _render_raw(
                        worker, bb, size[0], size[1], depth=depth,
                        frames=frames, with_result=True, fill=fill,
                        width_px=width_px)
                finally:
                    worker.stop()
            finally:
                c.close()
        finally:
            del os.environ["FLOE_RUST_DECK_SUBWINDOW"]
        return rgba, result

    def test_subwindow_pixels_equal_the_full_frame(self):
        cases = [
            ("test.jb", (301, 237), "solid", 1, False, None),
            ("test.jb", (301, 237), "speckle", 1, False, None),
            ("test.jb", (263, 301), PATTERN_ROWS, 3, False, None),
            ("frames.jb", (333, 211), "speckle", 1, True, 0),
            ("frames.jb", (211, 333), PATTERN_ROWS, 2, True, 0),
        ]
        for deck, size, fill, width_px, frames, depth in cases:
            full, _ = self._frames(deck, "off", size, fill, width_px,
                                   frames, depth)
            sub, result = self._frames(deck, "on", size, fill, width_px,
                                       frames, depth)
            self.assertEqual(sub, full, "%s %s fill=%s w=%d frames=%s"
                             % (deck, size, fill[:8], width_px, frames))
            self.assertGreater(_lit(sub), 0)
        # a zoomed view where placements lie partly outside the frame
        from floe.jobdeck.viewer import DeckCache
        c = DeckCache(str(CLI / "test.jb"))
        c.load()
        bb = c.meta["bbox"]
        c.close()
        cx, cy = (bb[0] + bb[2]) / 2, (bb[1] + bb[3]) / 2
        w4, h4 = (bb[2] - bb[0]) / 4, (bb[3] - bb[1]) / 4
        zoom = (cx - w4 * 0.7, cy - h4 * 0.3, cx + w4 * 1.1, cy + h4 * 0.9)
        full, _ = self._frames("test.jb", "off", (317, 251), "speckle", 1,
                               False, None, bbox=zoom)
        sub, result = self._frames("test.jb", "on", (317, 251), "speckle", 1,
                                   False, None, bbox=zoom)
        self.assertEqual(sub, full, "zoomed view")
        self.assertGreater(result["deck"]["passes_skipped"], 0,
                           "placements outside the frame are skipped")


class ReuseAndBatchTests(unittest.TestCase):
    """Step 2b: placements of the same source at the same scale share
    one plan and scene within a frame, and prepared passes are rastered
    in parallel batches - pixels unchanged (1 vs 4 raster jobs, and
    against the full-frame path)."""

    @classmethod
    def setUpClass(cls):
        cls.env = {"FLOE_INDEX_BIN": str(ROOT / "rust" / "target" /
                                         "release" / "floe-index"),
                   "FLOE_RENDERD_BIN": str(ROOT / "rust" / "target" /
                                           "release" / "floe-renderd")}
        os.environ["FLOE_RENDERD_BIN"] = cls.env["FLOE_RENDERD_BIN"]
        for deck in ("test.jb", "frames.jb"):
            run_floe2("index", CLI / deck, "--jobs", "2", env=cls.env, ok=0)

    def _render(self, deck, size, jobs, fill="speckle", frames=False,
                depth=None, subwindow="on", bbox=None):
        from floe.jobdeck.viewer import DeckCache
        os.environ["FLOE_RUST_RASTER_JOBS"] = str(jobs)
        os.environ["FLOE_RUST_JOBS"] = str(jobs)
        os.environ["FLOE_RUST_DECK_SUBWINDOW"] = subwindow
        try:
            c = DeckCache(str(CLI / deck))
            c.load()
            try:
                worker = jrender.DeckRenderWorker(c)
                worker.start()
                try:
                    bb = bbox or tuple(c.meta["bbox"])
                    return _render_raw(worker, bb, size[0], size[1],
                                       depth=depth, frames=frames,
                                       with_result=True, fill=fill)
                finally:
                    worker.stop()
            finally:
                c.close()
        finally:
            for k in ("FLOE_RUST_RASTER_JOBS", "FLOE_RUST_JOBS",
                      "FLOE_RUST_DECK_SUBWINDOW"):
                os.environ.pop(k, None)

    def test_same_source_placements_share_a_scene(self):
        rgba, result = self._render("test.jb", (301, 237), 4)
        d = result["deck"]
        # chipA at x4 appears in five placements of level 1 and two of
        # level 2 fully inside the view, chipB in six: one plan each
        self.assertGreaterEqual(d["scene_reuses"], 8, d)
        self.assertEqual(d["passes"], 17)

    def test_parallel_batches_and_reuse_keep_the_pixels(self):
        for deck, size, fill, frames, depth in (
                ("test.jb", (301, 237), "speckle", False, None),
                ("test.jb", (263, 301), PATTERN_ROWS, False, None),
                ("frames.jb", (333, 211), "speckle", True, 0)):
            serial, _ = self._render(deck, size, 1, fill, frames, depth)
            parallel, _ = self._render(deck, size, 4, fill, frames, depth)
            full, _ = self._render(deck, size, 4, fill, frames, depth,
                                   subwindow="off")
            self.assertEqual(parallel, serial, "%s jobs 1 vs 4" % deck)
            self.assertEqual(full, serial, "%s full-frame path" % deck)
            self.assertGreater(_lit(serial), 0)


class StreamTests(unittest.TestCase):
    """Step 3 (2026-09-10): a pass over the slice limit is rastered
    slice by slice on its window - no page is dropped, and the union
    of the slices is the whole-scene raster byte for byte (every pass
    paints one layer in one colour). Frames come from the plan and
    are rastered once on a page-less scene."""

    @classmethod
    def setUpClass(cls):
        cls.env = {"FLOE_INDEX_BIN": str(ROOT / "rust" / "target" /
                                         "release" / "floe-index"),
                   "FLOE_RENDERD_BIN": str(ROOT / "rust" / "target" /
                                           "release" / "floe-renderd")}
        os.environ["FLOE_RENDERD_BIN"] = cls.env["FLOE_RENDERD_BIN"]
        run_floe2("index", CLI / "dense.jb", "--jobs", "2", env=cls.env,
                  ok=0)

    def _render(self, env, size, bbox=None, **kw):
        from floe.jobdeck.viewer import DeckCache
        for k, v in env.items():
            os.environ[k] = v
        try:
            c = DeckCache(str(CLI / "dense.jb"))
            c.load()
            try:
                worker = jrender.DeckRenderWorker(c)
                worker.start()
                try:
                    return _render_raw(worker, bbox or tuple(c.meta["bbox"]),
                                       size[0], size[1], with_result=True,
                                       **kw)
                finally:
                    worker.stop()
            finally:
                c.close()
        finally:
            for k in env:
                os.environ.pop(k, None)

    def test_streamed_pass_equals_the_whole_scene(self):
        # (case, does the geometry stream): at depth 0 dense.jb's four
        # child cells are frames and TOP owns no page - a frames-only
        # pass on a page-less scene, nothing to stream
        cases = [
            (dict(depth=None, frames=False, fill="speckle"), True),
            (dict(depth=None, frames=True, fill="speckle"), True),
            (dict(depth=0, frames=True, fill="speckle"), False),
            # depth 1: the D cells' pages stream while D0's grandchild
            # K is a hierarchy frame (review 2026-09-10 (8th) P2-2:
            # the frames raster is part of the streamed wall-clock)
            (dict(depth=1, frames=True, fill="speckle"), True),
            (dict(depth=None, frames=False, fill=PATTERN_ROWS, width_px=2),
             True),
        ]
        for kw, streams in cases:
            whole, r0 = self._render({}, (301, 237), **kw)
            self.assertEqual(r0["deck"]["streamed_passes"], 0, kw)
            streamed, r1 = self._render({"FLOE_RUST_BUDGET_MB": "1"},
                                        (301, 237), **kw)
            d = r1["deck"]
            self.assertEqual(d["streamed_passes"], 1 if streams else 0, d)
            if streams:
                self.assertGreater(d["slices"], 1, d)
            self.assertEqual(r1.get("over_budget_pages", 0), 0)
            self.assertEqual(d["frame_passes"], r0["deck"]["frame_passes"])
            if kw["depth"] == 1:
                self.assertEqual(d["frame_passes"], 1, d)
                self.assertGreater(d["frame_raster_us"], 0, d)
            # one pass, so every raster (geometry slices and the frames
            # pass) ran serially: the wall-clock covers their SUM (the
            # line's raster_us), not only the frames or only the
            # geometry (review 2026-09-10: wall >= frame alone let the
            # geometry-only wall of 11.97 ms pass a 13.84 ms sum)
            self.assertEqual(d["passes"], 1, d)
            self.assertGreaterEqual(d["raster_us"], d["frame_raster_us"], d)
            self.assertGreaterEqual(d["raster_wall_us"], d["raster_us"], d)
            self.assertEqual(streamed, whole, kw)
            self.assertGreater(_lit(whole), 0)
        # a zoomed view: the pass streams on its sub-window
        from floe.jobdeck.viewer import DeckCache
        c = DeckCache(str(CLI / "dense.jb"))
        c.load()
        bb = c.meta["bbox"]
        c.close()
        zoom = (bb[0] + (bb[2] - bb[0]) * 0.3, bb[1] - (bb[3] - bb[1]) * 0.2,
                bb[2] + (bb[2] - bb[0]) * 0.1, bb[1] + (bb[3] - bb[1]) * 0.6)
        whole, _ = self._render({}, (263, 301), bbox=zoom)
        streamed, r1 = self._render({"FLOE_RUST_BUDGET_MB": "1"},
                                    (263, 301), bbox=zoom)
        self.assertEqual(streamed, whole, "zoomed")
        self.assertEqual(r1["deck"]["streamed_passes"], 1)
        # the kill switch restores the partial frame
        partial, r2 = self._render({"FLOE_RUST_BUDGET_MB": "1",
                                    "FLOE_RUST_DECK_STREAM": "off"},
                                   (301, 237))
        self.assertGreater(r2.get("over_budget_pages", 0), 0)
        self.assertEqual(r2["deck"]["streamed_passes"], 0)
        self.assertNotEqual(partial, whole)


class WideViewTests(unittest.TestCase):
    """Step 4 (2026-09-10): the jobdeck wide-view policy. What the
    size cut would drop keeps its on-screen existence as a footprint
    wash on its own layer: a page of sub-cut shapes as the page's
    bbox, an array of sub-cut child cells as the placement footprint
    on the child's visible layers. Kill switch FLOE_RUST_DECK_WIDE=off
    restores the silent omission."""

    @classmethod
    def setUpClass(cls):
        cls.env = {"FLOE_INDEX_BIN": str(ROOT / "rust" / "target" /
                                         "release" / "floe-index"),
                   "FLOE_RENDERD_BIN": str(ROOT / "rust" / "target" /
                                           "release" / "floe-renderd")}
        os.environ["FLOE_RENDERD_BIN"] = cls.env["FLOE_RENDERD_BIN"]
        for deck in ("tiny.jb", "test.jb"):
            run_floe2("index", CLI / deck, "--jobs", "2", env=cls.env,
                      ok=0)

    def _render(self, deck, env, visible, cut_px, size=(200, 200)):
        from floe.jobdeck.viewer import DeckCache
        for k, v in env.items():
            os.environ[k] = v
        try:
            c = DeckCache(str(CLI / deck))
            c.load()
            try:
                worker = jrender.DeckRenderWorker(c)
                worker.start()
                try:
                    return _render_raw(worker, tuple(c.meta["bbox"]),
                                       size[0], size[1], visible=visible,
                                       cut_px=cut_px, with_result=True)
                finally:
                    worker.stop()
            finally:
                c.close()
        finally:
            for k in env:
                os.environ.pop(k, None)

    def test_sub_cut_pages_and_children_keep_their_existence(self):
        # 2000 um on 200 px, cut 3 px = 30 um: the 1 um dots (level 1,
        # an own page) and the 1 um BIT array (level 2, sub-cut child
        # placements) are both below the cut
        for level in (1, 2):
            gone, r_off = self._render("tiny.jb", {"FLOE_RUST_DECK_WIDE": "off"},
                                       [(level, 0)], 3.0)
            self.assertEqual(_lit(gone), 0, "level %d is culled" % level)
            self.assertEqual(r_off["deck"]["wide_washes"], 0)
            kept, r_on = self._render("tiny.jb", {}, [(level, 0)], 3.0)
            self.assertGreater(_lit(kept), 0, "level %d washed" % level)
            self.assertGreater(r_on["deck"]["wide_washes"], 0)
            # the wash covers the field: its corners are lit
            w = 200
            corners = [(2, 2), (2, w - 3), (w - 3, 2), (w - 3, w - 3)]
            for x, y in corners:
                at = (y * w + x) * 4
                self.assertNotEqual(kept[at:at + 3], b"\0\0\0",
                                    "level %d corner %d,%d" % (level, x, y))
            # exact (no cut) draws the real geometry in the same colour
            exact, _ = self._render("tiny.jb", {}, [(level, 0)], 0.0)
            self.assertGreater(_lit(exact), 0)
            lit_colours = {tuple(kept[o:o + 3])
                           for o in range(0, len(kept), 4)
                           if kept[o:o + 3] != b"\0\0\0"}
            exact_colours = {tuple(exact[o:o + 3])
                             for o in range(0, len(exact), 4)
                             if exact[o:o + 3] != b"\0\0\0"}
            self.assertEqual(lit_colours, exact_colours)

    def test_wide_policy_is_a_no_op_above_the_cut(self):
        # test.jb's chips (levels 1 and 2) are far above the cut: the
        # policy adds no wash and changes no pixel there; the whole
        # deck holds exactly one sub-cut source, the 0.2x mark
        for visible in ([(1, 0)], [(2, 0)]):
            on, r_on = self._render("test.jb", {}, visible, 3.0, (301, 237))
            off, r_off = self._render("test.jb", {"FLOE_RUST_DECK_WIDE": "off"},
                                      visible, 3.0, (301, 237))
            self.assertEqual(r_on["deck"]["wide_washes"], 0)
            self.assertEqual(on, off)
            self.assertGreater(_lit(on), 0)
        _, r_all = self._render("test.jb", {}, None, 3.0, (301, 237))
        self.assertEqual(r_all["deck"]["wide_washes"], 1)


class ReviewFixTests8(unittest.TestCase):
    """Review 2026-09-10 (8th, after steps 3 and 4): pages the request's
    decode_pages limit leaves out are reported as missing (deferred,
    partial) whether the pass streams or not."""

    @classmethod
    def setUpClass(cls):
        cls.env = {"FLOE_INDEX_BIN": str(ROOT / "rust" / "target" /
                                         "release" / "floe-index"),
                   "FLOE_RENDERD_BIN": str(ROOT / "rust" / "target" /
                                           "release" / "floe-renderd")}
        os.environ["FLOE_RENDERD_BIN"] = cls.env["FLOE_RENDERD_BIN"]
        run_floe2("index", CLI / "dense.jb", "--jobs", "2", env=cls.env,
                  ok=0)

    def _render(self, env, decode_pages):
        from floe.jobdeck.viewer import DeckCache
        for k, v in env.items():
            os.environ[k] = v
        try:
            c = DeckCache(str(CLI / "dense.jb"))
            c.load()
            try:
                worker = jrender.DeckRenderWorker(c)
                worker.start()
                if decode_pages is not None:
                    # the protocol's page limit, which the viewer's
                    # command never sets: spliced into the render line
                    send = worker._send

                    def limited(command):
                        if command.startswith("render "):
                            command = command.replace(
                                " out=", " decode_pages=%d out=" % decode_pages,
                                1)
                        return send(command)
                    worker._send = limited
                try:
                    return _render_raw(worker, tuple(c.meta["bbox"]), 200,
                                       200, with_result=True)
                finally:
                    worker.stop()
            finally:
                c.close()
        finally:
            for k in env:
                os.environ.pop(k, None)

    def test_p2_1_decode_pages_limit_is_reported_when_streaming(self):
        whole, r_all = self._render({}, None)
        self.assertEqual(r_all.get("over_budget_pages", 0), 0)
        total = r_all["deck"]["pages_summed"]
        self.assertGreater(total, 2)
        streamed_cases = 0
        for env in ({}, {"FLOE_RUST_BUDGET_MB": "1"}):
            for limit in (2, total - 1):
                cut, r = self._render(env, limit)
                d = r["deck"]
                if not env:
                    self.assertEqual(d["streamed_passes"], 0, d)
                streamed_cases += d["streamed_passes"]
                self.assertEqual(r.get("over_budget_pages", 0), total - limit,
                                 "excluded pages are missing pages: %s" % d)
                self.assertNotEqual(cut, whole)
                self.assertGreater(_lit(cut), 0)
        self.assertGreater(streamed_cases, 0,
                           "a limited pass streamed at 1 MiB too")


class ReviewFixTests5(unittest.TestCase):
    """Review 2026-09-09 (5th, after step 2): a batch's window images
    are charged to its budget; the sub-window is computed in source
    units like the raster (a huge deck offset no longer hides a
    placement); the frame line separates the batches' wall-clock from
    the per-pass sum and reports the pass parallelism."""

    @classmethod
    def setUpClass(cls):
        cls.env = {"FLOE_INDEX_BIN": str(ROOT / "rust" / "target" /
                                         "release" / "floe-index"),
                   "FLOE_RENDERD_BIN": str(ROOT / "rust" / "target" /
                                           "release" / "floe-renderd")}
        os.environ["FLOE_RENDERD_BIN"] = cls.env["FLOE_RENDERD_BIN"]
        run_floe2("index", CLI / "test.jb", "--jobs", "2", env=cls.env,
                  ok=0)

    def _render(self, cache, bbox, size, env, **kw):
        for k, v in env.items():
            os.environ[k] = v
        try:
            worker = jrender.DeckRenderWorker(cache)
            worker.start()
            try:
                return _render_raw(worker, bbox, size[0], size[1],
                                   with_result=True, **kw)
            finally:
                worker.stop()
        finally:
            for k in env:
                os.environ.pop(k, None)

    def _deck(self):
        from floe.jobdeck.viewer import DeckCache
        c = DeckCache(str(CLI / "test.jb"))
        c.load()
        self.addCleanup(c.close)
        return c

    def test_p1_1_pass_images_bound_the_batch(self):
        # 17 full-frame passes of a 1024x1024 frame: 4 MiB of image
        # each. At a 1 MiB budget (half of it per batch) every pass is
        # a batch of its own; at the default budget they fit one batch
        # (the review measured 306 MB RSS for 64 such passes at 1 MiB)
        c = self._deck()
        bb = tuple(c.meta["bbox"])
        env = {"FLOE_RUST_BUDGET_MB": "1", "FLOE_RUST_DECK_SUBWINDOW": "off",
               "FLOE_RUST_JOBS": "4", "FLOE_RUST_RASTER_JOBS": "4"}
        small, result = self._render(c, bb, (1024, 1024), env)
        d = result["deck"]
        self.assertEqual(d["batches"], d["passes"], d)
        self.assertEqual(d["passes"], 17)
        self.assertLessEqual(d["batch_bytes_max"], 4 * 2 ** 20 + 2 ** 20,
                             "a lone pass at most")
        self.assertGreaterEqual(d["batch_bytes_max"], 1024 * 1024 * 4)
        env.pop("FLOE_RUST_BUDGET_MB")
        big, result = self._render(c, bb, (1024, 1024), env)
        d = result["deck"]
        self.assertEqual(d["batches"], 1, d)
        self.assertGreaterEqual(d["batch_bytes_max"], 17 * 1024 * 1024 * 4)
        self.assertEqual(small, big, "batching never changes the pixels")
        self.assertGreater(_lit(big), 0)
        # with sub-windows the charge is the window, not the frame
        env["FLOE_RUST_DECK_SUBWINDOW"] = "on"
        env["FLOE_RUST_BUDGET_MB"] = "1"
        sub, result = self._render(c, bb, (1024, 1024), env)
        self.assertEqual(sub, big)
        self.assertLess(result["deck"]["batch_bytes_max"], 4 * 2 ** 20,
                        "window images are smaller than the frame")

    def test_p2_2_huge_offset_keeps_the_placement(self):
        # The review's reproduction: dx = 1e16. chipB's top cell is
        # 0..2e7 dbu; at scale 4.5e-8 its right edge is deck 1e16 + 0.9,
        # which f64 rounds to 1e16 - in the view 1e16-10 .. 1e16+10 on
        # 2000 px (100 px per deck unit) the deck-space window ended at
        # column 1009 (1000 + slack) while the raster, mapping source
        # dbu through the source view (-10/4.5e-8 .. +10/4.5e-8), draws
        # the 7/2 box (2e6..1.8e7 dbu) on columns 1009..1081 and rows
        # 918..990 (73 x 73 px): the sub-window path drew NONE of it
        # (0 lit pixels). The window is now computed in source units
        # like the raster.
        spec = CLI / "huge-offset.spec"
        cache = str(CLI / "chipB.oas.floe")
        hexs = lambda t: t.encode().hex()
        dx = 1e16
        spec.write_text(
            "deck unit=1e-06\n"
            "source path_hex=%s\n"
            "layer out=0 name_hex=%s color=#ffffff fill=solid width=1\n"
            "placement source=0 layer=7/2 out=0 scale=4.5e-08 dx=%r dy=%r "
            "order=0\n" % (hexs(cache), hexs("$1 X"), dx, dx))
        layers = [{"layer": 0, "datatype": 0, "name": "$1 X",
                   "color": "#ffffff", "stored_shapes": 1,
                   "jobdeck_head": False}]
        shim = jrender._DeckCacheShim(str(spec), str(CLI / "test.jb"),
                                      1e-6, layers)
        view = (1e16 - 10.0, 1e16 - 10.0, 1e16 + 10.0, 1e16 + 10.0)
        full, _ = self._render(shim, view, (2000, 2000),
                               {"FLOE_RUST_DECK_SUBWINDOW": "off"})
        sub, result = self._render(shim, view, (2000, 2000),
                                   {"FLOE_RUST_DECK_SUBWINDOW": "on"})
        self.assertEqual(result["deck"]["passes"], 1)
        black, white = b"\0\0\0\xff", b"\xff\xff\xff\xff"

        def px(frame, col, row):
            at = (row * 2000 + col) * 4
            return frame[at:at + 4]
        for col, want in ((1008, black), (1009, white), (1081, white),
                          (1082, black)):
            self.assertEqual(px(full, col, 950), want, "column %d" % col)
        for row, want in ((917, black), (918, white), (990, white),
                          (991, black)):
            self.assertEqual(px(full, 1040, row), want, "row %d" % row)
        self.assertEqual(_lit(full), 73 * 73)
        self.assertEqual(sub, full, "the sub-window path draws the same")

    def test_p2_4_offscreen_placement_does_not_fail_the_frame(self):
        # Review 2026-09-09 (6th): chipB placed normally plus the same
        # source at dx = 1e16, scale 0.001 - outside the frame. The
        # off-screen placement's source view (-1e19 dbu) overflowed the
        # integer conversion and the whole frame failed with
        # "coordinate overflow: source view x0"; it must be an empty
        # pass, and the frame equals the deck without it.
        hexs = lambda t: t.encode().hex()
        cache = str(CLI / "chipB.oas.floe")
        head = ("deck unit=1e-06\n"
                "source path_hex=%s\n"
                "layer out=0 name_hex=%s color=#ffffff fill=solid width=1\n"
                "placement source=0 layer=7/2 out=0 scale=0.05 dx=0 dy=0 "
                "order=0\n" % (hexs(cache), hexs("$1 X")))
        extra = ("placement source=0 layer=7/2 out=0 scale=0.001 "
                 "dx=1e16 dy=1e16 order=1\n")
        layers = [{"layer": 0, "datatype": 0, "name": "$1 X",
                   "color": "#ffffff", "stored_shapes": 2,
                   "jobdeck_head": False}]
        # chipB's 7/2 box (2e6..1.8e7 dbu) at scale 0.05 is deck
        # 1e5..9e5: the view holds the whole placement
        view = (-1e5, -1e5, 1.1e6, 1.1e6)
        frames = []
        for name, text in (("plain.spec", head),
                           ("offscreen.spec", head + extra)):
            spec = CLI / name
            spec.write_text(text)
            shim = jrender._DeckCacheShim(str(spec), str(CLI / "test.jb"),
                                          1e-6, layers)
            rgba, result = self._render(shim, view, (300, 300), {})
            frames.append((rgba, result["deck"]))
        (plain, d0), (with_extra, d1) = frames
        self.assertGreater(_lit(plain), 0)
        self.assertEqual(with_extra, plain)
        self.assertEqual((d0["passes"], d0["passes_skipped"]), (1, 0))
        self.assertEqual((d1["passes"], d1["passes_skipped"]), (1, 1))

    def test_p2_3_wall_clock_and_pass_parallelism(self):
        c = self._deck()
        bb = tuple(c.meta["bbox"])
        for jobs, expect in ((1, 1), (4, 4)):
            env = {"FLOE_RUST_JOBS": str(jobs),
                   "FLOE_RUST_RASTER_JOBS": str(jobs)}
            rgba, result = self._render(c, bb, (301, 237), env)
            d = result["deck"]
            self.assertEqual(d["pass_workers"], expect, d)
            self.assertGreaterEqual(d["batches"], 1)
            self.assertGreater(d["raster_wall_us"], 0)
            # the summed per-pass time (draw_ms) is not the wall time:
            # with 4 passes at once the wall is at most the sum (plus
            # the spawn), never presented as it
            if jobs == 4:
                self.assertLessEqual(d["raster_wall_us"],
                                     result["draw_ms"] * 1000 + 20000)


class KLayoutOracleTests(unittest.TestCase):
    """M5: KLayout as the independent oracle of the composite. The deck
    is built the reference tool's way - every source cell copied into
    one KLayout layout on an int32-safe grid and instantiated with a
    magnifying ICplxTrans - and drawn by KLayout's own LayoutView through
    the frozen shell's Renderer; floe2's composite of the same viewport
    must match under the battery's pixel policy (validate_render_goldens
    P-a/b/c per colour: differences only inside the 1px edge band, no
    vanished component, bounded area drift)."""

    @classmethod
    def setUpClass(cls):
        try:
            import klayout.db  # noqa: F401
            import numpy  # noqa: F401
            from PIL import Image  # noqa: F401
        except ImportError as exc:
            raise unittest.SkipTest("oracle needs klayout, numpy, Pillow: "
                                    "%s" % exc)
        cls.env = {"FLOE_INDEX_BIN": str(ROOT / "rust" / "target" /
                                         "release" / "floe-index"),
                   "FLOE_RENDERD_BIN": str(ROOT / "rust" / "target" /
                                           "release" / "floe-renderd")}
        os.environ["FLOE_RENDERD_BIN"] = cls.env["FLOE_RENDERD_BIN"]
        run_floe2("index", CLI / "test.jb", "--jobs", "2", env=cls.env,
                  ok=0)

    def _goldens_module(self):
        import importlib.util
        path = ROOT / "tools" / "validate_render_goldens.py"
        spec = importlib.util.spec_from_file_location("floe_goldens", path)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        return module

    def test_composite_matches_klayout_layoutview(self):
        import klayout.db as db
        import numpy as np
        from PIL import Image
        from floe.render import Renderer
        from floe.jobdeck.viewer import DeckCache
        goldens = self._goldens_module()

        cache = DeckCache(str(CLI / "test.jb"))
        cache.load()
        try:
            rows = cache.view_rows()
            out_of = jrender.view_out_of(rows, cache.scheme)
            # --- the KLayout deck: int32-safe 1e-4 um grid, source cells
            # copied verbatim (their own dbu), instantiated with the
            # magnification deck-dbu-per-source-dbu and the deck offset
            oracle_dbu = 1e-4
            lay = db.Layout()
            lay.dbu = oracle_dbu
            top = lay.create_cell("DECK")
            srcs = {}
            cells = {}
            colors = {}
            for p in cache.placements:
                info = cache.catalog.infos[p.tc]
                if p.tc not in srcs:
                    s = db.Layout()
                    s.read(str(CLI / p.tc))
                    srcs[p.tc] = s
                s = srcs[p.tc]
                out = out_of(p)
                key = (p.tc, p.ly, p.dt, out)
                if key not in cells:
                    cell = lay.create_cell("P%d" % len(cells))
                    dst = lay.layer(1000 + out, 0)
                    src_cell = s.top_cell()
                    for sh in src_cell.shapes(s.layer(p.ly, p.dt)).each():
                        cell.shapes(dst).insert(sh)
                    cells[key] = cell.cell_index()
                    colors[(1000 + out, 0)] = rows[out]["color"]
                scale = p.mag * float(info.dbu) / oracle_dbu
                disp = db.Vector(int(round(p.dx_um / oracle_dbu)),
                                 int(round(p.dy_um / oracle_dbu)))
                top.insert(db.CellInstArray(
                    cells[key], db.ICplxTrans(scale, 0.0, False, disp)))
            bbox_um = jrender.fit_bbox_to_pixels(
                (36000.37, 79000.61, 70000.37, 106000.61), 1024, 800)
            golden_png = CLI / "oracle-klayout.png"
            renderer = Renderer(lay, top, colors, speckle=False)
            try:
                renderer.set_line_widths({k: 1 for k in colors})
                renderer.render_png(
                    str(golden_png), *(v / oracle_dbu for v in bbox_um),
                    1024, 800, visible=None, depth=None)
            finally:
                renderer.lv._destroy()
            golden = np.asarray(Image.open(golden_png).convert("RGB"))
            # --- floe2's composite of the same viewport
            worker = jrender.DeckRenderWorker(cache)
            worker.start()
            try:
                rgba = _render_raw(worker, tuple(v / cache.meta["dbu"]
                                                 for v in bbox_um),
                                   1024, 800)
            finally:
                worker.stop()
            candidate = np.frombuffer(rgba, dtype=np.uint8).reshape(
                800, 1024, 4)[:, :, :3]
        finally:
            cache.close()
        self.assertEqual(golden.shape, candidate.shape)
        # per colour: the battery's mask policy (P-a/b/c)
        band = goldens._edge_band(np.any(golden != 0, axis=2))[0]
        for row in rows:
            rgb = np.array(_rgb(row["color"]), dtype=np.uint8)
            g = np.all(golden == rgb, axis=2)
            c = np.all(candidate == rgb, axis=2)
            self.assertGreater(int(g.sum()), 0, row["name"])
            ok, reason = goldens.compare(g, c)
            self.assertTrue(ok, "%s: %s" % (row["name"], reason))
            # a colour's edge is a boundary too where it meets another
            # colour (a mark over a chip), not only where it meets black
            band |= goldens._edge_band(g)[0]
        # and nothing but edge-band pixels may differ at all
        bad = np.any(golden != candidate, axis=2) & ~band
        if bad.any():
            y, x = np.argwhere(bad)[0]
            Image.fromarray(candidate, "RGB").save(CLI / "oracle-floe2.png")
            self.fail("%d px differ outside the 1px edge bands (first y=%d "
                      "x=%d); see %s" % (int(bad.sum()), y, x, CLI))


def run_floe2(*args, env=None, ok=None, timeout=600):
    e = os.environ.copy()
    e.update({"PYTHONDONTWRITEBYTECODE": "1", "PYTHONPATH": str(ROOT)})
    if env:
        e.update(env)
    res = subprocess.run(
        [sys.executable, "-B", "-m", "floe2", *map(str, args)],
        cwd=ROOT, env=e, capture_output=True, text=True, timeout=timeout)
    if ok is not None and res.returncode != ok:
        raise AssertionError(
            "floe2 %s: exit %d, wanted %d\nstdout:\n%s\nstderr:\n%s"
            % (args, res.returncode, ok, res.stdout, res.stderr))
    return res


if __name__ == "__main__":
    unittest.main(verbosity=1)
