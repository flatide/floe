"""Description-derived DRC constraints: units, predicates and refusal cases."""

import sys
import unittest
from pathlib import Path
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from floe import drc, drc_delta  # noqa: E402
from floe.drc_description import description_constraints, constraints_for  # noqa: E402


class DescriptionTests(unittest.TestCase):
    def condition(self, description, *, name="M1.WIDTH", metric="width", op="<", value=0.05):
        result = description_constraints(name, description)
        self.assertEqual(len(result), 1, description)
        condition = result[0]
        self.assertEqual((condition["metric"], condition["op"]), (metric, op))
        self.assertAlmostEqual(condition["value"], value, places=12)
        self.assertEqual(condition["source"], "description")
        self.assertIn(condition["description"], description)
        self.assertFalse(drc_delta._has_options(condition["text"]))
        return condition

    def test_minimum_and_maximum_requirements_and_strict_boundaries(self):
        for description, op in (
                ("Minimum width 0.05000 um", "<"),
                ("Width minimum = 0.05 um", "<"),
                ("Minimum width >= 0.05 um", "<"),
                ("Minimum width > 0.05 um", "<="),
                ("Maximum width 0.05 um", ">"),
                ("Width 0.05 um maximum", ">"),
                ("Maximum width <= 0.05 um", ">"),
                ("Maximum width < 0.05 um", ">="),
                ("Width must be >= 0.05 um", "<"),
                ("Width must be > 0.05 um", "<="),
                ("Width shall be <= 0.05 um", ">"),
                ("Width should be < 0.05 um", ">="),
                ("Width must be at least 0.05 um", "<"),
                ("Width must be less than 0.05 um", ">=")):
            with self.subTest(description=description):
                self.condition(description, op=op)

    def test_bare_error_comparators_keep_their_boundaries(self):
        for op in ("<", "<=", ">", ">="):
            self.condition("Width %s 0.05 um" % op, op=op)
        self.condition("Width ≤ 0.05 µm", op="<=")
        self.condition("Width ≥ 0.05 μm", op=">=")

    def test_units_and_all_supported_metrics(self):
        for description, metric in (
                ("Minimum width 50 nm", "width"),
                ("Min. spacing 50 nm", "space"),
                ("Notch < 5e1 nm", "notch"),
                ("Enclosure < .05 um", "enclosure"),
                ("Two-layer INTERNAL overlap < 0.05 um", "overlap"),
                ("ENCLOSURE extension < 0.05 um", "extension"),
                ("Minimum length 50 nanometers", "length")):
            with self.subTest(metric=metric):
                self.condition(description, metric=metric)
        self.condition("Area < 10000 nm²", metric="area", value=0.01)
        self.condition("Maximum area 0.01 um^2", metric="area", op=">", value=0.01)
        self.condition("Area < 0.01 μm2", metric="area", value=0.01)

    def test_korean_minimum_maximum_and_requirement_suffixes(self):
        for description, metric, op in (
                ("최소 폭 50 nm", "width", "<"),
                ("최대 선폭 0.05 um", "width", ">"),
                ("간격은 최소 0.05 um", "space", "<"),
                ("폭은 0.05 um 이상이어야 함", "width", "<"),
                ("폭이 0.05 um 초과여야 한다", "width", "<="),
                ("길이는 0.05 um 이하이어야 한다", "length", ">"),
                ("폭 0.05 um 미만", "width", "<"),
                ("겹침 < 50 나노미터", "overlap", "<")):
            with self.subTest(description=description):
                self.condition(description, metric=metric, op=op)

    def test_rule_name_only_completes_missing_metric(self):
        self.condition("Minimum 0.05 um", name="M1.WIDTH.MIN")
        self.condition("CD must be >= 0.05 um", name="M1.WIDTH.MIN")
        self.condition("M1.WIDTH: Minimum width 0.05 um")
        self.assertEqual(description_constraints("R1", "Minimum 0.05 um"), [])
        self.assertEqual(description_constraints("M1.WIDTH.SPACE", "Minimum 0.05 um"), [])
        # An unambiguous explicit physical dimension takes precedence over
        # heuristic rule-name words (e.g. ENCLOSURE extension).
        self.condition("Extension < 0.05 um", name="R.ENCLOSURE", metric="extension")

    def test_notes_file_metadata_and_sample_values_are_not_bounds(self):
        description = ("Rule File Pathname: /Minimum width 0.01 um/deck.cal\n"
                       "Rule File Title: Maximum width 0.02 um\n"
                       "Waiver: Width < 0.03 um\n"
                       "Minimum width 0.05 um; CD 0.02 and 0.03 um.\n"
                       "Selected CD: 0.01 um.\nExpected deltas: 0.03 um.")
        condition = self.condition(description)
        self.assertEqual(condition["description"], "Minimum width 0.05 um")
        for description in ("CD sequence repeats 0.01, 0.02, 0.03 um.",
                            "Selected CD is 0.02000 um.",
                            "Expected area deltas: 0.00600 um^2.",
                            "Rule File Title: Minimum width 0.05 um"):
            self.assertEqual(description_constraints("M1.WIDTH", description), [])

    def test_ambiguous_and_incomplete_conditions_are_withheld(self):
        descriptions = (
            "Minimum width 0.05", "Width < MIN_WIDTH um", "Width < 0.05 * SCALE um",
            "Width < 0.05 um or Width > 0.08 um", "Width < 0.05 um and length > 0.1 um",
            "Minimum notch 0.05 um if wire width > 0.10 um", "Width < 0.05 um except corners",
            "Width between 0.05 um and 0.1 um", "Width < 0.05 um; if wire is wide",
            "Minimum width 0.05 um; Maximum width 0.08 um",
            "Minimum width 0.05 um\nMaximum length 0.1 um",
            "Minimum width < 0.05 um", "Maximum width > 0.05 um",
            "Minimum maximum width 0.05 um", "Width = 0.05 um", "Width != 0.05 um",
            "Width < 0.05 um²", "Area < 0.05 um", "Width < 0.05 mm",
            "Width < -0.05 um", "Width < NaN um", "Width < 1e999 um", "Width < 1e-999 um",
            "Minimum width 1e999999999 nm", "Minimum width 1e-999999999 nm",
            "Width < 0.05 um3", "Width < 0.05 um^3", "Width < 0.05 um for wide wires",
            "Width < 0.05 um\nEnclosure > UNKNOWN", "Width < 0.05 um; limit 0.08 um",
            "최소 폭 0.05 um 또는 최대 길이 0.1 um", "Minimum width 0.05 um (see exceptions)",
            "Notch < 0.05 um; applies only inside macro",
            "Notch < 0.05 um\nInside the macro", "Notch < 0.05 um; 배선 영역",
        )
        for description in descriptions:
            with self.subTest(description=description):
                self.assertEqual(description_constraints("M1.WIDTH", description), [])

    def test_equivalent_repeated_conditions_are_not_ambiguous(self):
        self.condition("Minimum width 0.05 um\nWidth < 50 nm")

    def test_authoritative_metadata_is_preserved_even_if_unsupported(self):
        check = SimpleNamespace(name="M1.WIDTH", desc="Minimum width 0.05 um")
        explicit = [{"metric": "density", "op": "<", "value": "UNRESOLVED"}]
        metadata = {"checks": {check.name: {"constraints": explicit}}}
        self.assertIs(constraints_for(check, metadata), explicit)
        self.assertEqual(constraints_for(check, {"checks": {}}), constraints_for(check))
        self.assertEqual(constraints_for(SimpleNamespace(name="unknown")), [])

    def test_sample_geometry_measurement_and_rulers_without_sidecars(self):
        db = drc.load_ascii(str(ROOT / "sample.db"))
        expected = {
            "M1.WIDTH.MIN": [2000, 2000, 2000],
            "M1.WIDTH.MAX": [10000, 10000, 12000],
            "M1.NOTCH": [2000, 2000, 2345],
            "VIA1.ENCLOSURE": [2000, 2500],
            "M1.M2.OVERLAP": [2000, 1500],
            "M1.EXTENSION": [2000, 3000],
            "M1.AREA": [400, 600, 360],
        }
        for ci, check in enumerate(db.checks):
            conditions = constraints_for(check)
            if check.name not in expected:
                self.assertEqual(conditions, [], check.name)
                continue
            with self.subTest(rule=check.name):
                index = drc_delta.DeltaIndex(db, ci, conditions).measure()
                self.assertEqual(index.measured_ticks.tolist(), expected[check.name])
                self.assertTrue(index.estimated_flags.all())
                self.assertGreater(len(index.group(1000, mode="absolute")), 0)
                if check.name == "M1.WIDTH.MIN":
                    self.assertEqual(len(drc_delta.ruler_segments(check.errors[0], conditions)), 1)
                    self.assertEqual(len(drc_delta.ruler_segments(check.errors[2], conditions)), 2)
                if check.name == "M1.AREA":
                    self.assertEqual(drc_delta.ruler_segments(check.errors[0], conditions), [])
                    self.assertIsNotNone(drc_delta.area_label(check.errors[0], conditions))


if __name__ == "__main__":
    unittest.main(verbosity=2)
