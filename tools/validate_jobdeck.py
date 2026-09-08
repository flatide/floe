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
        self.assertIn("no .floe cache yet; add --index", res.stdout)
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
        res = run_cli(CLI / "test.jb", "--index", "--jobs", "2", env=env,
                      ok=0)
        self.assertIn("index     : 3 built, 0 failed, 0 kept", res.stdout)
        for name in ("chipA.oas", "chipB.oas", "mark.oas"):
            c = Cache(str(CLI / name))
            self.assertTrue(c.exists(), name)
            c.load()
            self.assertTrue(c.meta.get("vfs"), name)
            self.assertFalse(c.is_stale(), name)
        res = run_cli(CLI / "test.jb", "--index", env=env, ok=0)
        self.assertIn("3 source(s) already indexed", res.stdout)
        self.assertIn("index     : 0 built, 0 failed, 3 kept", res.stdout)
        res = run_cli(CLI / "test.jb", env=env, ok=0)
        self.assertIn("3 probed, 3 ok, 3 indexed", res.stdout)


if __name__ == "__main__":
    unittest.main(verbosity=1)
