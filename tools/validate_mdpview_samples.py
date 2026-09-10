#!/usr/bin/env python3
"""Local source/model checks, NOT an MDPview compatibility test."""

import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

import klayout.db as db

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from floe.jobdeck.parser import parse_jobdeck
from floe.jobdeck.plan import plan_deck
from gen_mdpview_samples import generate


class SamplesTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix="mdpview-samples-")
        cls.addClassCleanup(cls.tmp.cleanup)
        cls.out = Path(cls.tmp.name) / "samples with spaces"
        cls.manifest = generate(cls.out)

    def test_inventory_hashes_and_unobserved(self):
        self.assertEqual(len(list(self.out.glob("*.jb"))), 9)
        self.assertEqual(len(list(self.out.rglob("*.oas"))), 6)
        for rel, expected in self.manifest["sha256"].items():
            self.assertEqual(hashlib.sha256((self.out / rel).read_bytes()).hexdigest(),
                             expected)
        for run in json.loads((self.out / "observations.template.json").read_text())["runs"]:
            self.assertIsNone(run["accepted"])
            self.assertIsNone(run["view_mode"])

    def test_source_geometry(self):
        for rel in self.manifest["sha256"]:
            if not rel.endswith(".oas"):
                continue
            layout = db.Layout()
            layout.read(str(self.out / rel))
            self.assertEqual(layout.dbu, 0.001)
            self.assertEqual(len(layout.top_cells()), 1)
            self.assertEqual(layout.top_cell().name, "PATTERN")
            self.assertEqual(layout.top_cell().bbox(), db.Box(0, 0, 1000000, 1000000))
            self.assertEqual(list(layout.top_cell().each_inst()), [])
            pairs = sorted((i.layer, i.datatype) for i in layout.layer_infos())
            self.assertEqual(pairs, [(7, 0), (7, 1)] if rel == "sources/m.oas"
                             else [(7, 0)])
            areas = [db.Region(layout.top_cell().begin_shapes_rec(li)).area()
                     for li in layout.layer_indexes()]
            self.assertEqual(sorted(areas), [160000000000, 160000000000]
                             if rel == "sources/m.oas" else
                             [500000000000] if "b.oas" in rel or "/right/" in rel
                             else [360000000000])

    def test_byte_identical_controls(self):
        for a, b in (("a.oas", "a_copy.oas"), ("a.oas", "left/pattern.oas"),
                     ("b.oas", "right/pattern.oas")):
            self.assertEqual((self.out / "sources" / a).read_bytes(),
                             (self.out / "sources" / b).read_bytes())

    def test_parse_and_hand_calculated_model(self):
        for case in self.manifest["cases"]:
            path = str(self.out / case["deck"])
            with self.subTest(case=case["id"]):
                deck = parse_jobdeck(path, strict=True)
                self.assertFalse(deck.errors)
                placements = plan_deck(path)[2]
                self.assertEqual(len(placements), 1 if case["id"] == "01_control" else 2)
                for p in placements:
                    self.assertEqual(p.mag, 1)
                    self.assertEqual(p.dy_um, 1500)
                    self.assertIn(p.dx_um, (1500, 3500))
                    self.assertEqual(p.ly, 7)
                    self.assertEqual(p.ry_um, 0)

    def test_model_union_controls(self):
        def regions(case):
            result = db.Region()
            for p in plan_deck(str(self.out / (case + ".jb")))[2]:
                layout = db.Layout()
                layout.read(str(self.out / p.tc))
                r = db.Region(layout.top_cell().begin_shapes_rec(layout.layer(p.ly, p.dt)))
                result += r.transformed(db.Trans(round(p.dx_um * 1000),
                                                  round(p.dy_um * 1000)))
            return result
        for a, b in (("02_rows", "03_definitions"),
                     ("03_definitions", "05_identical_copy"),
                     ("04_sources", "09_same_basename"),
                     ("07_datatypes", "08_shared_source_levels")):
            self.assertTrue((regions(a) ^ regions(b)).is_empty(), (a, b))

    def test_existing_output_is_untouched(self):
        before = {p: p.read_bytes() for p in self.out.rglob("*") if p.is_file()}
        with self.assertRaises(FileExistsError):
            generate(self.out)
        self.assertEqual(before, {p: p.read_bytes() for p in self.out.rglob("*") if p.is_file()})

    def test_level_selection_isolated_from_source_identity(self):
        for name in ("06_levels", "08_shared_source_levels"):
            path = str(self.out / (name + ".jb"))
            first = plan_deck(path, ids=[1])[2]
            second = plan_deck(path, ids=[2])[2]
            self.assertEqual(len(first), 1)
            self.assertEqual(len(second), 1)
            self.assertEqual(first[0].tc, second[0].tc)
            if name == "06_levels":
                self.assertEqual((first[0].dt, second[0].dt), (0, 0))
                self.assertEqual((first[0].dx_um, second[0].dx_um), (1500, 3500))
            else:
                self.assertEqual((first[0].dt, second[0].dt), (0, 1))
                self.assertEqual(first[0].dx_um, second[0].dx_um)

    def test_single_file_outside_repository(self):
        solo = Path(self.tmp.name) / "standalone"
        solo.mkdir()
        script = solo / "generate.py"
        shutil.copyfile(ROOT / "tools/gen_mdpview_samples.py", script)
        output = solo / "output"
        run = subprocess.run([sys.executable, "-I", str(script), str(output)],
                             cwd=str(solo), capture_output=True, text=True)
        self.assertEqual(run.returncode, 0, run.stderr)
        self.assertTrue((output / "README.ko.md").is_file())
        self.assertEqual(len(list(output.glob("*.jb"))), 9)


if __name__ == "__main__":
    unittest.main(verbosity=2)
