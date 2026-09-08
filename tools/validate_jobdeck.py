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
        self.assertEqual(st, {
            "chipA.oas": "ok", "chipA.gds": "ok", "chipA.oas.gz": "ok",
            "chipA.gds.gz": "ok", "junk.bin": "unknown_format",
            "absent.oas": "missing"})
        self.assertEqual(sorted(cat.dbus()), [
            "chipA.gds", "chipA.gds.gz", "chipA.oas", "chipA.oas.gz"])
        self.assertEqual(sorted(cat.bad()), ["absent.oas", "junk.bin"])
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
        self.assertEqual(len(pl), 4)
        self.assertEqual(st["skip_counts"],
                         {"missing": 1, "unknown_format": 1})
        reasons = {(r["chip"], r["idx"]): r["reason"] for r in st["skipped"]}
        self.assertEqual(reasons, {("ID001", 6): "unknown_format",
                                   ("ID002", 8): "missing"})
        self.assertEqual(st["skipped"][0]["anchors"], [[45020.0, 85120.0]])
        # outside the selection a bad source is information, not a skip
        pl, st = jgeom.plan(deck, cat.dbus(), ids=[1, 2], bad=cat.bad())
        self.assertEqual(len(pl), 2)
        self.assertEqual(st["skipped"], [])
        self.assertEqual(st["deck_issue_counts"],
                         {"missing": 1, "unknown_format": 1})
        # the four containers of the same geometry place identically
        by = {p.idx: p for p in jgeom.plan(deck, cat.dbus(),
                                            missing=jgeom.MISSING_SKIP,
                                            bad=cat.bad())[0]}
        for i in (2, 3, 4):
            self.assertEqual((by[i].mag, by[i].dx_um, by[i].dy_um),
                             (by[1].mag, by[1].dx_um, by[1].dy_um))

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
        run_cli(CLI / "test_formats.jb", "--on-missing", "fail", ok=2)
        run_cli(CLI / "test_formats.jb", "--id", "1,2", ok=0)
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


def _render_raw(worker, bbox_dbu, width, height, visible=None,
                depth=None, cut_px=0.0):
    """One settled raw frame through a started worker: RGBA bytes.
    Generations must increase per daemon: a repeated one is dropped."""
    gen = next(_GEN)
    solid = "\n".join(["*" * 16] * 16)
    keys = [(int(l["layer"]), int(l["datatype"]))
            for l in worker.cache.meta["layers"]]
    worker.submit({"kind": "repattern",
                   "fills": [(k, solid) for k in keys],
                   "widths": [(k, 1) for k in keys]})
    worker.submit({
        "kind": "render", "gen": gen, "scope": "headless",
        "bbox": tuple(float(v) for v in bbox_dbu), "view": None,
        "w": width, "h": height, "depth": depth, "cut_px": cut_px,
        "lod": False, "frames": False, "labels": False,
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
        return rgba


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
            cls.only_2 = _render_raw(worker, cls.bbox_dbu, cls.W, cls.H,
                                     visible=[(1, 0)])
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
        # only deck layer 1 ($2): yellow and black, nothing else
        colours = {tuple(self.only_2[o:o + 3])
                   for o in range(0, len(self.only_2), 4)}
        self.assertEqual(colours, {(0, 0, 0), (255, 255, 0)})

    def test_3b_viewer_defaults_drop_sub_cut_sources_quietly(self):
        colours = {tuple(self.gui_defaults[o:o + 3])
                   for o in range(0, len(self.gui_defaults), 4)}
        self.assertIn((0, 0, 255), colours)      # $1 chips are drawn
        self.assertIn((255, 255, 0), colours)    # $2
        self.assertIn((255, 192, 203), colours)  # $5
        self.assertNotIn((255, 0, 0), colours)   # the sub-cut mark is not

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
        self.assertIn("chipA.gds: not_indexed", res.stdout)
        self.assertIn("1 placement(s), 1 skipped", res.stdout)
        res = run_floe2("view", CLI / "test_formats.jb", "--multi",
                        env=self.env, ok=1)
        self.assertIn("no VFS cache for", res.stderr)


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
        self.assertFalse(deck_ready(str(CLI / "test_formats.jb")))
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
                             [(1, 0), (3, 0)])
            self.assertIsNone(c.resolve_layers("all"))
            with self.assertRaises(ValueError):
                c.resolve_layers("nope")
            # chip mode: one row per CHIP in deck order, MDPView colours
            meta = c.set_mode("chip")
            self.assertEqual([l["name"] for l in meta["layers"]],
                             ["CHIP ID001", "CHIP ID002", "CHIP ID003"])
            self.assertEqual([l["color"] for l in meta["layers"]],
                             ["#ffff00", "#ffc0cb", "#ffffff"])
            self.assertEqual([l["stored_shapes"] for l in meta["layers"]],
                             [6, 9, 2])
            self.assertTrue(c.dir.endswith("deck-chip.spec"))
            meta = c.set_mode("layer")
            self.assertEqual([l["name"] for l in meta["layers"]],
                             ["LY7.DT2", "LY123.DT43", "LY456.DT0",
                              "LY999.DT0"])
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
