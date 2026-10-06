"""Delta grouping gate: measurements, decimal bins, compact membership.

Exercises the public grouping and paging contracts without GTK or a pack
builder. Rule errors use arbitrary file-order identities throughout.

Usage: python tools/validate_drc_delta.py
"""

import gc
import os
import subprocess
import sys
import tempfile
import tracemalloc
import unittest
import weakref
from types import SimpleNamespace

import numpy as np

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from floe import drc, drc_delta as delta, gui  # noqa: E402
from floe.drc_clusters import Cluster  # noqa: E402

BIN = os.path.join(os.path.dirname(__file__), "..", "rust", "target",
                   "release", "floe-index")


def rect(width, height=2.0):
    return drc.DrcError("p", 1, [(0, 0), (width, 0),
                                 (width, height), (0, height)])


def edge(length):
    return drc.DrcError("e", 1, [(0, 0), (length, 0)])


def area_cases():
    return [(rect(3, 7), 21),
            (drc.DrcError("p", 1, [(0, 0), (4, 0), (0, 3)]), 6),
            (drc.DrcError("p", 1, [(0, 0), (3, 0), (3, 1),
                                    (1, 1), (1, 3), (0, 3)]), 5)]


def constraint(metric="width", bound=1.0, op="<"):
    return {"metric": metric, "value": bound, "op": op,
            "text": "%s %s %s" % (metric, op, bound)}


class Database:
    def __init__(self, errors):
        self.checks = [SimpleNamespace(name="R", errors=errors)]
        self._dir_es = np.asarray([0], dtype=np.int64)
        self._status = np.zeros(len(errors), dtype=np.uint8)

    def get_status(self, ci, ei):
        return int(self._status[ei])

    def status_counts(self, ci):
        return int(np.count_nonzero(self._status == 1)), len(self._status)


class RepeatedErrors:
    """Large input without per-error Python state or retained geometry."""

    def __init__(self, count, error):
        self.count, self.error, self.reads = count, error, 0

    def __len__(self):
        return self.count

    def __getitem__(self, index):
        if not 0 <= index < self.count:
            raise IndexError(index)
        self.reads += 1
        return self.error


def groups_for(errors, constraints=None, step="0.10000", mode="absolute",
               cluster=None):
    db = Database(errors)
    index = delta.DeltaIndex(db, 0, constraints or [constraint()]).measure()
    groups = index.group(delta.parse_step(step), cluster=cluster, mode=mode)
    return db, index, groups


def group_members(groups):
    return {group.key: group.page(0, group.total + 1)
            for group in groups.page(0, len(groups) + 1)}


class MeasurementTests(unittest.TestCase):
    def test_area_labels_match_polygon_measurements_detail_ticks_and_groups(self):
        constraints = [constraint("area", 30)]
        _db, index, groups = groups_for([error for error, _area in area_cases()],
                                        constraints, step="1")
        self.assertEqual(index.measured_ticks.tolist(), [2100000, 600000, 500000])
        self.assertEqual(group_members(groups), {(0, 9): [0], (0, 24): [1], (0, 25): [2]})
        for error, area in area_cases():
            for points in (error.pts, list(reversed(error.pts)), error.pts + [error.pts[0]]):
                polygon = drc.DrcError("p", 1, points)
                with self.subTest(area=area, points=points):
                    label = delta.area_label(polygon, constraints)
                    self.assertIsNotNone(label)
                    self.assertEqual(label[2], "area %d.00000 µm²" % area)
                    self.assertEqual(delta.ruler_segments(polygon, constraints), [])
                    pick = delta.pick_constraint(polygon, constraints)
                    self.assertEqual(delta.measurement_ticks(pick)[0], area * delta.SCALE)

    def test_area_label_policy_preserves_geometry_when_area_is_not_the_only_metric(self):
        error = rect(0.02, 0.1)
        area = constraint("area", 0.001)
        # A marker's area remains useful even when its listed predicate
        # does not match or its bound cannot be resolved.
        for con in (area, {"metric": "area", "op": "<", "value": None,
                           "raw": "UNKNOWN", "text": "AREA M1 < UNKNOWN"}):
            with self.subTest(constraint=con):
                self.assertEqual(delta.area_label(error, [con])[2], "area 0.00200 µm²")
                self.assertEqual(delta.ruler_segments(error, [con]), [])
                self.assertIsNone(delta.area_label(edge(1), [con]))
                self.assertEqual(delta.ruler_segments(edge(1), [con]), [])
        mixed = [area, constraint(bound=0.05)]
        self.assertEqual(delta.area_label(error, mixed)[2], "area 0.00200 µm²")
        self.assertEqual(delta.ruler_segments(error, mixed), drc.cd_segments(error))
        for constraints in ([], [constraint(bound=0.05)]):
            self.assertIsNone(delta.area_label(error, constraints))
        self.assertEqual(delta.ruler_segments(error), drc.cd_segments(error))
        invalid = drc.DrcError("p", 1, [(0, 0), (1, 0)])
        self.assertIsNone(delta.area_label(invalid, [area]))

    def test_candidates_keep_rectangle_spans_and_deduplicate_squares(self):
        for metric in ("width", "space", "notch", "enclosure", "overlap", "extension"):
            with self.subTest(metric=metric):
                self.assertEqual(delta.measurement_candidates(rect(0.1, 0.02), metric),
                                 (0.02, 0.1))
                self.assertEqual(delta.measurement_candidates(rect(0.02, 0.02), metric),
                                 (0.02,))
                self.assertEqual(delta.measured(rect(0.1, 0.02), metric), 0.02,
                                 "legacy scalar ruler must remain the minimum")
        self.assertEqual(delta.measurement_candidates(rect(3, 7), "area"), (21,))
        self.assertEqual(delta.measurement_candidates(edge(3), "length"), (3,))
        triangle = drc.DrcError("p", 1, [(0, 0), (4, 0), (0, 3)])
        self.assertEqual(delta.measurement_candidates(triangle, "width"), ())
        self.assertEqual(delta.measurement_candidates(edge(3), "width"), ())

    def test_additional_cd_metrics_reuse_geometry_and_absolute_delta_groups(self):
        gap = drc.DrcError("e", 1, [(0, 0), (1, 0), (0, 0.02), (1, 0.02)])
        diagonal = drc.DrcError("e", 1, [(0, 0), (2, 0), (5, 4), (7, 4)])
        triangle = drc.DrcError("p", 1, [(0, 0), (1, 0), (0, 1)])
        for metric in ("notch", "enclosure", "overlap", "extension"):
            with self.subTest(metric=metric):
                self.assertEqual(delta.measurement_candidates(gap, metric), (0.02,))
                self.assertEqual(delta.measurement_candidates(diagonal, metric), (5,))
                self.assertEqual(delta.measurement_candidates(triangle, metric), ())
                self.assertEqual(delta.measurement_candidates(edge(1), metric), ())
                _db, index, groups = groups_for([rect(0.02, 0.1), gap, triangle],
                                                [constraint(metric, 0.05)], step="0.01")
                self.assertEqual(index.measured_ticks.tolist(), [2000, 2000, 0])
                self.assertEqual(group_members(groups), {(0, 3): [0, 1], (-1, 0): [2]})
                self.assertEqual(groups[0].metric, metric)
                self.assertEqual(groups[0].unit, "um")

    def test_rectangle_rulers_keep_only_uniquely_matching_directions(self):
        for width, height, short_axis in ((0.02, 0.1, 0), (0.1, 0.02, 1)):
            error = rect(width, height)
            axes = drc.cd_segments(error)
            for metric in ("width", "space", "notch", "enclosure", "overlap", "extension"):
                for op, axis in (("<", short_axis), (">", 1 - short_axis)):
                    with self.subTest(metric=metric, op=op, width=width):
                        self.assertEqual(delta.ruler_segments(error, [constraint(metric, 0.05, op)]),
                                         [axes[axis]])
            for op, bound in (("<", 0.15), (">", 0.015)):
                with self.subTest(both_match=(op, bound), width=width):
                    self.assertEqual(delta.ruler_segments(error, [constraint(bound=bound, op=op)]),
                                     axes, "CD selection must not discard another violating direction")
        square = rect(0.02, 0.02)
        self.assertEqual(delta.ruler_segments(square, [constraint(bound=0.05)]),
                         drc.cd_segments(square), "equal lengths still represent two directions")

    def test_rectangle_ruler_chains_and_ambiguous_metadata_fallback(self):
        error = rect(0.02, 0.1)
        axes = drc.cd_segments(error)
        low, high = constraint(bound=0.05, op=">"), constraint(bound=0.15)
        low["text"] = "INTERNAL M1 > 0.05"
        high["text"] = "INTERNAL M1 > 0.05 < 0.15"
        self.assertEqual(delta.ruler_segments(error, [low, high]), [axes[1]])
        unknown = {"metric": "space", "op": "<", "value": None,
                   "raw": "UNKNOWN", "text": "EXTERNAL M2 < UNKNOWN"}
        for constraints in ([], [constraint(bound=0.01)],
                            [constraint(bound=0.05), constraint(bound=0.05, op=">")],
                            [constraint(bound=0.05), unknown],
                            [constraint(bound=0.05), constraint("area", 1)],
                            [constraint(bound=0.05), constraint("angle", 90)]):
            with self.subTest(constraints=constraints):
                self.assertEqual(delta.ruler_segments(error, constraints), axes)

    def test_ruler_filters_known_external_selectors_but_keeps_unknown_options(self):
        error = rect(0.02, 0.1)
        axes = drc.cd_segments(error)
        gap = drc.DrcError("e", 1, [(0, 0), (1, 0), (0, 0.02), (1, 0.02)])
        for option, metric in (("NOTCH", "notch"), ("SPACE", "space")):
            with self.subTest(selector=option):
                con = constraint(metric, 0.05)
                con["text"] = "EXTERNAL M1 < 0.05 " + option
                self.assertEqual(delta.ruler_segments(error, [con]), [axes[0]])
                self.assertTrue(delta.pick_constraint(gap, [con]).estimated,
                                "ruler selector support does not establish exact measurement provenance")
        for option in ("ANGLED", "REGION", "OPPOSITE EXTENDED < 0.005",
                       "NOTCH ANGLED", "SPACE REGION"):
            with self.subTest(unevaluated_option=option):
                con = constraint("space", 0.05)
                con["text"] = "EXTERNAL M1 < 0.05 " + option
                self.assertEqual(delta.ruler_segments(error, [con]), axes)

    def test_rectangle_ruler_predicates_share_coordinate_roundoff_handling(self):
        for origin in (0, 1_000_000):
            error = drc.DrcError("p", 1, [(origin + 0.2, 0), (origin + 0.3, 0),
                                        (origin + 0.3, 0.2), (origin + 0.2, 0.2)])
            axes = drc.cd_segments(error)
            for op, expected in (("<=", [axes[0]]), (">", [axes[1]]),
                                 ("==", [axes[0]]), ("<", axes)):
                with self.subTest(origin=origin, op=op):
                    self.assertEqual(delta.ruler_segments(error, [constraint(bound=0.1, op=op)]),
                                     expected)
        error = rect(0.100004, 0.2)
        self.assertEqual(delta.ruler_segments(error, [constraint(bound=0.1, op=">")]),
                         drc.cd_segments(error))
        diagonal = drc.DrcError("e", 1, [(0, 0), (2, 0), (5, 4), (7, 4)])
        self.assertEqual(delta.ruler_segments(diagonal, [constraint("extension", 6)]),
                         drc.cd_segments(diagonal), "edge gap and its XY rulers must remain intact")

    def test_edgepair_axis_helpers_are_not_measurement_candidates(self):
        error = drc.DrcError("e", 1, [(0, 0), (2, 0), (5, 4), (7, 4)])
        self.assertEqual(len(drc.cd_segments(error)), 3)
        self.assertEqual(delta.measurement_candidates(error, "space"), (5,))
        pick = delta.pick_constraint(error, [constraint("space", 6)])
        self.assertEqual(tuple(pick), (0, 6, 5))
        self.assertEqual(pick.candidates, (5,))
        self.assertFalse(pick.estimated)

    def test_error_predicate_direction_selects_minimum_or_maximum_candidate(self):
        error = rect(0.02, 0.1)
        for op, bound, expected in (("<", 0.15, 0.02), ("<=", 0.15, 0.02),
                                    (">", 0.015, 0.1), (">=", 0.015, 0.1),
                                    ("<", 0.05, 0.02), (">", 0.05, 0.1)):
            with self.subTest(op=op, bound=bound):
                pick = delta.pick_constraint(error, [constraint(bound=bound, op=op)])
                self.assertEqual(tuple(pick), (0, bound, expected))
                self.assertEqual(pick.candidates, (0.02, 0.1))
                self.assertTrue(pick.estimated,
                                "two-span marker does not identify the original CD")
                self.assertIsInstance(pick.reason, str)
                self.assertTrue(pick.reason)

    def test_matching_constraint_wins_and_unmatched_errors_remain_unmeasurable(self):
        error = rect(0.02, 0.1)
        constraints = [constraint(bound=0.01), constraint(bound=0.05, op=">")]
        self.assertEqual(tuple(delta.pick_constraint(error, constraints)), (1, 0.05, 0.1))
        for op, bound in (("<", 0.01), (">", 0.15)):
            with self.subTest(op=op):
                pick = delta.pick_constraint(error, [constraint(bound=bound, op=op)])
                self.assertIsNone(pick)
        square = rect(0.02, 0.02)
        self.assertFalse(delta.pick_constraint(square, [constraint(bound=0.03)]).estimated)
        self.assertIsNone(delta.pick_constraint(square, [constraint(bound=0.01)]))

    def test_same_statement_chain_filters_all_bounds_before_choosing_cd(self):
        error = rect(0.02, 0.1)
        texts = [
            ("INTERNAL M1 > 0.05 < 0.15", "INTERNAL M1 > 0.05 < 0.15"),
            ("INTERNAL M1 > 0.05", "INTERNAL M1 > 0.05 < 0.15"),
        ]
        for low_text, high_text in texts:
            with self.subTest(texts=(low_text, high_text)):
                low, high = constraint(bound=0.05, op=">"), constraint(bound=0.15)
                low["text"], high["text"] = low_text, high_text
                pick = delta.pick_constraint(error, [low, high])
                self.assertEqual(tuple(pick), (1, 0.15, 0.1))
                self.assertTrue(pick.estimated)
        low, high = constraint(bound=0.05, op=">"), constraint(bound=0.08)
        low["text"] = high["text"] = "INTERNAL M1 > 0.05 < 0.08"
        self.assertIsNone(delta.pick_constraint(error, [low, high]),
                          "each bound matches one span, but their conjunction matches neither")

    def test_unrelated_matching_statements_do_not_form_a_chain(self):
        # A shared text prefix must not combine checks on M1 and M10.
        low, high = constraint(bound=0.05, op=">"), constraint(bound=0.15)
        low["text"], high["text"] = "INTERNAL M1 > 0.05", "INTERNAL M10 < 0.15"
        pick = delta.pick_constraint(rect(0.02, 0.1), [low, high])
        self.assertIsNone(pick)
        # A unique span cannot identify which unrelated criterion emitted it.
        pick = delta.pick_constraint(rect(0.1, 0.1), [low, high])
        self.assertIsNone(pick)
        duplicate = delta.pick_constraint(rect(0.1, 0.1), [high, dict(high)])
        self.assertEqual(tuple(duplicate), (0, 0.15, 0.1))
        self.assertFalse(duplicate.estimated)

    def test_strict_predicate_boundaries_are_not_rounded_into_matches(self):
        error = rect(0.1, 0.1)
        for op in ("<", ">", "<=", ">="):
            with self.subTest(op=op):
                pick = delta.pick_constraint(error, [constraint(bound=0.1, op=op)])
                if op in ("<", ">"):
                    self.assertIsNone(pick)
                else:
                    self.assertEqual(tuple(pick), (0, 0.1, 0.1))
                    self.assertFalse(pick.estimated)
        pick = delta.pick_constraint(rect(0.100004, 0.100004),
                                     [constraint(bound=0.1, op=">")])
        self.assertIsNotNone(pick)
        self.assertEqual(delta.measurement_ticks(pick), (10000, 10000, 0))

    def test_coordinate_roundoff_respects_strict_inclusive_and_equality_predicates(self):
        expected = {"<": False, ">": False, "<=": True, ">=": True,
                    "==": True, "!=": False}
        for origin in (0, 1_000_000):
            error = drc.DrcError("e", 1, [(origin + 0.2, 0), (origin + 0.3, 0)])
            for op, matches in expected.items():
                with self.subTest(origin=origin, op=op):
                    pick = delta.pick_constraint(error, [constraint("length", 0.1, op)])
                    self.assertEqual(pick is not None, matches)
                    if pick is not None:
                        self.assertEqual(delta.measurement_ticks(pick), (10000, 10000, 0))
                        if op == "==":
                            self.assertTrue(pick.estimated)
                            self.assertIn("roundoff", pick.reason.lower())
        # Four millionths is a real difference, despite the same five-digit
        # displayed value; coordinate ulps must not become display rounding.
        for op, matches in {"<": False, ">": True, "<=": False, ">=": True,
                            "==": False, "!=": True}.items():
            with self.subTest(real_difference_op=op):
                pick = delta.pick_constraint(edge(0.100004),
                                             [constraint("length", 0.1, op)])
                self.assertEqual(pick is not None, matches)

    def test_unresolved_alternative_marks_supported_candidate_as_estimated(self):
        error = drc.DrcError("e", 1, [(0, 0), (1, 0), (0, 0.05), (1, 0.05)])
        known = constraint("space", 0.1)
        known["text"] = "EXTERNAL M1 < 0.1"
        unresolved = {"metric": "space", "op": "<", "value": None,
                      "raw": "UNRESOLVED", "text": "EXTERNAL M2 < UNRESOLVED"}
        self.assertFalse(delta.pick_constraint(error, [known]).estimated)
        for constraints, expected_index in (([known, unresolved], 0),
                                             ([unresolved, known], 1)):
            with self.subTest(order=expected_index):
                pick = delta.pick_constraint(error, constraints)
                self.assertEqual(tuple(pick), (expected_index, 0.1, 0.05))
                self.assertTrue(pick.estimated)
                self.assertIn("unresolved", pick.reason.lower())
                _db, index, groups = groups_for([error], constraints)
                self.assertEqual(index.estimated_flags.tolist(), [True])
                self.assertEqual(groups.estimated_total, 1)
                self.assertEqual(groups[0].page(0, 10), [0])

    def test_supported_measurements_agree_with_existing_cd_rulers(self):
        cases = [
            (rect(3, 7), "width", 3),
            (rect(3, 7), "area", 21),
            (drc.DrcError("p", 1, [(0, 0), (4, 0), (0, 3)]), "area", 6),
            (drc.DrcError("p", 1, [(0, 0), (4, 0), (0, 3)]), "width", None),
            (drc.DrcError("e", 1, [(0, 0), (3, 4)]), "length", 5),
            (drc.DrcError("e", 1, [(0, 0), (5, 0), (0, 2), (5, 2)]),
             "space", 2),
            (drc.DrcError("e", 1, [(0, 0), (2, 0), (5, 4), (7, 4)]),
             "enclosure", 5),
            (drc.DrcError("e", 1, [(0, 0), (2, 2), (0, 2), (2, 0)]),
             "space", None),
            (rect(3, 7), "angle", None),
            (edge(3), "area", None),
        ]
        viewer = gui.Viewer.__new__(gui.Viewer)
        viewer.dbu = 0.0025
        for error, metric, expected in cases:
            with self.subTest(metric=metric, points=error.pts):
                got = delta.measured(error, metric)
                if expected is None:
                    self.assertIsNone(got)
                else:
                    self.assertAlmostEqual(got, expected)
                    self.assertAlmostEqual(got, viewer._drc_measured(error, metric))

    def test_constraint_upper_bound_precedes_zero_lower_bound(self):
        constraints = [constraint(bound=0, op=">"), constraint(bound=1, op="<")]
        for con in constraints:
            con["text"] = "INTERNAL M1 > 0 < 1"
        picked = delta.pick_constraint(rect(0.75), constraints)
        self.assertEqual(picked[0], 1)
        self.assertEqual(picked[1:], (1, 0.75))
        self.assertIsNone(delta.pick_constraint(edge(3), constraints))
        # Fall back to a supported lower bound when there is no upper one.
        picked = delta.pick_constraint(rect(0.75), constraints[:1])
        self.assertEqual(picked[0], 0)

    def test_step_validation_and_five_decimal_format(self):
        self.assertEqual(delta.parse_step("0.10000"), 10000)
        self.assertEqual(delta.parse_step("0.00001"), 1)
        self.assertEqual(delta.parse_step("1"), 100000)
        self.assertEqual(delta.parse_step("0.1000000"), 10000)
        for text in ("0", "-0.1", "NaN", "inf", "hello", "0.000001",
                     "0.000006", "0.123456",
                     "0.100000000000000000000000000000000001",
                     "1e100000000"):
            with self.subTest(value=text), self.assertRaises(ValueError):
                delta.parse_step(text)
        self.assertEqual(delta.format_ticks(12345), "0.12345")
        self.assertEqual(delta.format_ticks(-1), "-0.00001")
        self.assertEqual(delta.format_ticks(1, signed=True), "+0.00001")

    def test_rounding_uses_displayed_values_and_exact_percent_ties(self):
        self.assertEqual(delta.value_ticks(0.900005), 90001)
        self.assertEqual(delta.value_ticks(-0.000005), -1)
        self.assertEqual(delta.measurement_ticks((0, 1.000004, 0.900005)),
                         (90001, 100000, 9999))
        self.assertEqual(delta.measurement_ticks((0, 0.900005, 1.000004)),
                         (100000, 90001, 9999))
        self.assertEqual(delta.percent_ticks(-100000, 300000), 3333333)
        self.assertEqual(delta.percent_ticks(1, 20000000), 1)
        self.assertEqual(delta.percent_ticks(-1, 20000000), 1)
        self.assertIsNone(delta.percent_ticks(1, 0))
        with self.assertRaises(ValueError):
            delta.value_ticks("1e100000000")
        self.assertEqual(delta.value_ticks("1e-100000000"), 0)

    def test_magnitude_percent_handles_both_signs_zero_and_overflow(self):
        for difference in (-100000, 100000):
            for bound in (-300000, 300000):
                with self.subTest(difference=difference, bound=bound):
                    self.assertEqual(delta.percent_ticks(difference, bound), 3333333)
        for difference in (-1, 1):
            self.assertEqual(delta.percent_ticks(difference, -20000000), 1)
        self.assertEqual(delta.percent_ticks(0, -1), 0)
        self.assertIsNone(delta.percent_ticks(0, 0))
        self.assertEqual(delta.measurement_ticks((0, 1, 1)), (100000, 100000, 0))
        limit = int(np.iinfo(np.int64).max)
        self.assertEqual(delta.percent_ticks(limit, -limit), 10000000)
        self.assertIsNone(delta.percent_ticks(limit, -1))
        with self.assertRaises(ValueError):
            delta.measurement_ticks((0, -50000000000000, 50000000000000))

    def test_small_polygon_area_is_stable_at_large_layout_coordinates(self):
        for origin in (0, 1_000_000, 10_000_000):
            error = drc.DrcError("p", 1, [(origin, origin),
                (origin + 0.01, origin), (origin + 0.01, origin + 0.01),
                (origin, origin + 0.01)])
            with self.subTest(origin=origin):
                self.assertEqual(delta.value_ticks(delta.measured(error, "area")), 10)
                self.assertEqual(delta.area_label(error, [constraint("area", 1)])[2],
                                 "area 0.00010 µm²")


class DeltaGroupingTests(unittest.TestCase):
    def test_selected_candidate_is_reused_by_absolute_and_percent_groups(self):
        errors = [rect(0.02, 0.1), rect(0.06, 0.08), rect(0.06, 0.06)]
        constraints = [constraint(bound=0.05, op=">")]
        _db, index, groups = groups_for(errors, constraints, step="0.01000")
        np.testing.assert_array_equal(index.measured_ticks, [10000, 8000, 6000])
        self.assertEqual(group_members(groups), {(0, 1): [2], (0, 3): [1], (0, 5): [0]})
        percent = index.group(delta.parse_step("10"), mode="percent")
        self.assertEqual(group_members(percent), {(0, 2): [2], (0, 6): [1], (0, 10): [0]})
        for ei, error in enumerate(errors):
            pick = delta.pick_constraint(error, constraints)
            self.assertEqual(index.measured_ticks[ei], delta.measurement_ticks(pick)[0])

    def test_estimate_counts_follow_group_and_sparse_cluster_membership(self):
        errors = [rect(0.02, 0.1), rect(0.02, 0.02),
                  rect(0.03, 0.1), edge(0.1)]
        db, index, groups = groups_for(errors, [constraint(bound=0.15)], step="1")
        self.assertEqual(index.estimated_flags.dtype, np.dtype("bool"))
        np.testing.assert_array_equal(index.estimated_flags, [True, False, True, False])
        self.assertFalse(index.estimated_flags.flags.writeable)
        self.assertEqual(groups.estimated_total, 2)
        self.assertEqual(groups[0].estimated_count, 2)
        self.assertEqual(groups[0].total, 3)
        self.assertEqual(groups[-1].constraint_index, -1)
        self.assertEqual(groups[-1].estimated_count, 0)
        self.assertIn("estimated", groups[0].name.lower())
        cluster = Cluster("scattered", db, 0, np.array([0, 3]), np.array([2, 4]))
        chosen = index.group(delta.parse_step("1"), cluster=cluster)
        self.assertEqual(chosen[0].page(0, 10), [0, 1])
        self.assertEqual(chosen[0].estimated_count, 1)
        self.assertEqual(chosen.estimated_total, 1)
        db._status[0] = 1
        chosen.status_changed([0])
        self.assertEqual(chosen[0].estimated_count, 1,
                         "review status must not change measurement provenance")

    def test_exact_five_decimal_rounding_and_magnitude_bin_boundaries(self):
        _db, _index, groups = groups_for(
            [rect(value) for value in (0.9, 0.89999, 0.899999, 0.90001)])
        self.assertEqual(group_members(groups), {(0, 0): [3], (0, 1): [0, 1, 2]})
        self.assertEqual((groups[0].low_ticks, groups[0].high_ticks), (0, 10000))
        self.assertEqual((groups[1].low_ticks, groups[1].high_ticks), (10000, 20000))
        self.assertIn("[0.10000, 0.20000)", groups[1].name)
        self.assertNotIn("+", groups[1].name)
        # Decimal HALF_UP is applied to both measured CD and rule value.
        _db, _index, groups = groups_for([rect(0.900005)],
                                        [constraint(bound=1.000005)], step="0.1")
        self.assertEqual(group_members(groups), {(0, 1): [0]})

    def test_minimum_and_maximum_violations_share_magnitude_but_not_criterion(self):
        errors = [edge(0.8), edge(1.0), edge(1.2)]
        db = Database(errors)
        index = delta.DeltaIndex(db, 0, [constraint("length", 1),
                                       constraint("length", 1, ">=")]).measure()
        absolute = index.group(delta.parse_step("0.10000"), mode="absolute")
        percent = index.group(delta.parse_step("10.00000"), mode="percent")
        self.assertEqual(group_members(absolute),
                         {(0, 2): [0], (1, 0): [1], (1, 2): [2]})
        self.assertEqual(group_members(percent),
                         {(0, 2): [0], (1, 0): [1], (1, 2): [2]})
        self.assertEqual(percent[0].low_ticks, 2000000)
        self.assertNotEqual(absolute[0].unit, percent[0].unit)

    def test_negative_bound_ratios_match_vector_and_large_integer_paths(self):
        errors = [edge(0.25), edge(200000000)]
        _db, index, groups = groups_for(errors, [constraint("length", -1, ">")],
                                        step="1", mode="percent")
        # 1.25 / |-1| * 100 = 125; the second numerator requires Python
        # integers rather than int64 multiplication by 100 * SCALE.
        self.assertEqual(group_members(groups),
                         {(0, 125): [0], (0, 20000000100): [1]})
        self.assertEqual(groups[0].low_ticks, 12500000)
        self.assertEqual(groups[1].low_ticks, 2000000010000000)
        self.assertTrue(all(group.low_ticks >= 0 for group in groups.page(0, 10)))
        self.assertEqual(index.measured_ticks.tolist(), [25000, 20000000000000])
        # A valid absolute CD can have a percentage beyond int64 capacity.
        _db, _index, overflow = groups_for([edge(100000000)],
            [constraint("length", -0.00001, ">")], mode="percent")
        self.assertEqual(group_members(overflow), {(-1, 0): [0]})

    def test_zero_bound_and_unsupported_geometry_have_unknown_group(self):
        constraints = [constraint("length", 0, ">")]
        db, index, groups = groups_for([edge(0.25), rect(0.5)], constraints,
                                      mode="percent")
        self.assertEqual(len(groups), 1)
        unknown = groups[0]
        self.assertEqual(unknown.constraint_index, -1)
        self.assertEqual(unknown.page(0, 10), [0, 1])
        absolute = index.group(delta.parse_step("0.1"), mode="absolute")
        self.assertEqual(absolute[0].page(0, 10), [0])
        self.assertEqual(absolute[-1].constraint_index, -1)
        self.assertEqual(absolute[-1].page(0, 10), [1])

    def test_mixed_constraint_units_are_never_combined(self):
        _db, _index, groups = groups_for([rect(3, 3), edge(9)],
            [constraint("area", 10), constraint("length", 10)], step="1")
        self.assertEqual(len(groups), 2)
        self.assertEqual(group_members(groups), {(0, 1): [0], (1, 1): [1]})
        self.assertEqual({group.metric for group in groups.page(0, 10)},
                         {"area", "length"})
        self.assertEqual(len({group.unit for group in groups.page(0, 10)}), 2)

    def test_sparse_cluster_membership_and_error_pages_are_exact(self):
        n = 6003
        db = Database([edge(0.8 if ei % 3 else 1.2) for ei in range(n)])
        db._status[::7] = 1
        selected = np.asarray([ei for ei in range(n) if ei % 5 != 1], dtype=np.int64)
        # File loading canonicalizes adjacent intervals before constructing
        # Cluster; mirror that input contract rather than overlapping events.
        breaks = np.flatnonzero(np.diff(selected) > 1)
        starts = selected[np.r_[0, breaks + 1]]
        stops = selected[np.r_[breaks, len(selected) - 1]] + 1
        cluster = Cluster("scatter", db, 0, starts, stops)
        index = delta.DeltaIndex(db, 0, [constraint("length", 1),
                                       constraint("length", 1, ">")]).measure()
        groups = index.group(delta.parse_step("0.1"), cluster=cluster)
        for group in groups.page(0, 100):
            expected = [ei for ei in selected.tolist()
                        if (ei % 3 == 0) == (group.constraint_index == 1)]
            self.assertEqual(group.total, len(expected))
            self.assertEqual(group.page(0, 1000), expected[:1000])
            self.assertEqual(group.page(1000, 1000), expected[1000:2000])
            for waived in (None, False, True):
                filtered = [ei for ei in expected if waived is None
                            or (db.get_status(0, ei) == 1) == waived]
                self.assertEqual(group.count(waived), len(filtered))
                self.assertEqual(group.page(100, 35, waived), filtered[100:135])
                for ei in (0, 1, 1007, 5903, n - 1):
                    rank = filtered.index(ei) if ei in filtered else None
                    self.assertEqual(group.rank(ei, waived), rank)
            self.assertEqual(group.mask(989, 73).tolist(),
                             [ei in expected for ei in range(989, 1062)])
            for ei in (1, 1007, 5903):
                self.assertEqual(group.contains(ei), ei in expected)

    def test_status_updates_and_same_count_swaps_refresh_group_pages(self):
        db, _index, groups = groups_for([rect(0.8), rect(1.2), rect(0.8)],
                                       [constraint(bound=3)])
        first, second = groups[groups.find((0, 22))], groups[groups.find((0, 18))]
        db._status[0] = 1
        groups.status_changed([0])
        self.assertEqual(first.page(0, 10, True), [0])
        self.assertEqual(second.count(True), 0)
        db._status[[0, 1]] = [0, 1]
        groups.status_changed([0, 1])
        self.assertEqual(first.status_counts(), (0, 2))
        self.assertEqual(second.status_counts(), (1, 1))
        self.assertEqual([group.key for group in groups.page(0, 10, True)],
                         [second.key])
        db._status[:] = [0, 0, 1]
        groups.reset_status()
        self.assertEqual(first.page(0, 10, True), [2])
        self.assertEqual(second.page(0, 10, True), [])

    def test_group_descriptor_does_not_keep_old_numeric_scope_alive_in_cycle(self):
        was_enabled = gc.isenabled()
        gc.disable()
        try:
            _db, _index, groups = groups_for([rect(0.8), rect(1.2)])
            group = groups[0]
            owner_ref = weakref.ref(groups)
            del groups
            self.assertIsNotNone(owner_ref(), "live group must retain its member arrays")
            del group
            self.assertIsNone(owner_ref(), "replaced grouping retained a reference cycle")
        finally:
            if was_enabled:
                gc.enable()

    def test_thousands_of_distinct_groups_support_bounded_row_pages(self):
        n = 5003
        _db, _index, groups = groups_for(
            [edge((100000 + ei) / 100000) for ei in range(n)],
            [constraint("length", 1, ">=")], step="0.00001")
        self.assertEqual(len(groups), n)
        rows = groups.page(999, 5)
        self.assertEqual([group.key for group in rows],
                         [(0, ei) for ei in range(999, 1004)])
        self.assertEqual([group.page(0, 1000) for group in rows],
                         [[ei] for ei in range(999, 1004)])
        self.assertEqual(groups.find((0, n - 1)), n - 1)
        self.assertIsNone(groups.find((0, n)))
        self.assertEqual(groups.page(n, 10), [])

    def test_large_input_retains_numeric_state_and_reuses_measurements(self):
        n = 40003
        errors = RepeatedErrors(n, rect(0.8))
        db = Database(errors)
        gc.collect()
        tracemalloc.start()
        try:
            index = delta.DeltaIndex(db, 0, [constraint()]).measure()
            groups = index.group(delta.parse_step("0.1"))
            _current, peak = tracemalloc.get_traced_memory()
        finally:
            tracemalloc.stop()
        self.assertEqual(index.measured_ticks.dtype, np.dtype("int64"))
        self.assertEqual(index.constraint_indices.dtype, np.dtype("int32"))
        self.assertEqual(len(index.measured_ticks), n)
        self.assertEqual(len(groups), 1)
        self.assertEqual(groups[0].page(n - 3, 10), [n - 3, n - 2, n - 1])
        self.assertLess(peak, 160 * n + 3_000_000,
                        "numeric grouping retained disproportionate working memory")
        reads = errors.reads
        index.group(delta.parse_step("0.01"), mode="percent")
        self.assertEqual(errors.reads, reads, "changing units redecoded all geometry")
        # The result may own large NumPy arrays; it must never retain a
        # Python integer/member/geometry container for every error.
        pending, seen = [index, groups], {id(db)}
        while pending:
            value = pending.pop()
            if id(value) in seen:
                continue
            seen.add(id(value))
            if isinstance(value, np.ndarray):
                self.assertNotEqual(value.dtype, np.dtype("object"))
                continue
            if isinstance(value, dict):
                self.assertLess(len(value), 1024)
                pending.extend(value.values())
            elif isinstance(value, (list, tuple, set, frozenset)):
                self.assertLess(len(value), 1024)
                pending.extend(value)
            elif type(value).__module__ == delta.__name__:
                pending.extend(vars(value).values())

    def test_cancelled_measurement_is_reusable_and_group_cancel_is_atomic(self):
        errors = RepeatedErrors(9003, rect(0.8))
        db = Database(errors)
        index = delta.DeltaIndex(db, 0, [constraint()])
        calls = [0]

        def cancel_after_work():
            calls[0] += 1
            return calls[0] > 2

        with self.assertRaises(delta.DeltaQueryCancelled):
            index.measure(cancelled=cancel_after_work)
        self.assertIsNone(index.measured_ticks)
        self.assertIsNone(index.constraint_indices)
        index.measure()
        with self.assertRaises(delta.DeltaQueryCancelled):
            index.group(delta.parse_step("0.1"), cancelled=lambda: True)
        groups = index.group(delta.parse_step("0.1"))
        self.assertEqual(groups[0].total, len(errors))

    @unittest.skipUnless(os.path.isfile(BIN), "packed parity requires floe-index")
    def test_packed_measurements_match_ascii_without_touching_gui_block_cache(self):
        class WorkerPack(drc.IcePack):
            def _block(self, block):
                raise AssertionError("delta worker touched the GUI geometry cache")

        with tempfile.TemporaryDirectory(prefix="floe-delta-") as tmp:
            path = os.path.join(tmp, "results.db")
            packed_path = os.path.join(tmp, "results.tray")
            count = 773
            with open(path, "w", encoding="utf-8") as stream:
                stream.write("MAIN 100000\nBEFORE\n1 1 0\ne 1 1\n0 0 3 4\n")
                stream.write("EMPTY\n0 0 0\nR\n%d %d 0\n" % (count, count))
                for ei in range(count):
                    width = 50000 + ei % 11 * 10000
                    # Negative origins and non-monotonic widths stress the
                    # block's first-point deltas and local error identities.
                    x, y = -ei * 13, ei * 7
                    if ei % 2:
                        stream.write("e %d 1\n%d %d %d %d\n" %
                                     (ei + 1, x, y, x + width, y))
                    else:
                        stream.write("p %d 4\n%d %d\n%d %d\n%d %d\n%d %d\n" %
                                     (ei + 1, x, y, x + width, y,
                                      x + width, y + 200000, x, y + 200000))
            subprocess.run([BIN, "drc", path, packed_path], check=True,
                           capture_output=True, text=True)
            ascii_db = drc.load_ascii(path)
            packed = WorkerPack(packed_path)
            try:
                constraints = [constraint(), constraint("length", 1)]
                ascii_index = delta.DeltaIndex(ascii_db, 2, constraints).measure()
                packed_index = delta.DeltaIndex(packed, 2, constraints).measure()
                np.testing.assert_array_equal(ascii_index.measured_ticks,
                                              packed_index.measured_ticks)
                np.testing.assert_array_equal(ascii_index.constraint_indices,
                                              packed_index.constraint_indices)
                for mode in ("absolute", "percent"):
                    ascii_groups = ascii_index.group(delta.parse_step("0.1"), mode=mode)
                    packed_groups = packed_index.group(delta.parse_step("0.1"), mode=mode)
                    self.assertEqual(group_members(ascii_groups), group_members(packed_groups))
                self.assertEqual(packed._cache, {})
            finally:
                packed.close()


class AutomaticStepTests(unittest.TestCase):
    @staticmethod
    def numeric_index(values, constraints=None, choices=None):
        """Seed exact ticks for integer-limit and bounded-memory cases.

        These exercise grouping independently of float geometry precision;
        real geometry and decoder reuse are covered by the tests below too.
        """
        values = np.asarray(values, dtype=np.int64)
        constraints = constraints or [constraint("length", 0, ">=")]
        index = delta.DeltaIndex(Database(range(len(values))), 0, constraints)
        index.measured_ticks = values
        index.constraint_indices = np.asarray(
            np.zeros(len(values), dtype=np.int32) if choices is None else choices,
            dtype=np.int32)
        index.estimated_flags = np.zeros(len(values), dtype=bool)
        return index

    def test_default_step_targets_ten_groups_and_manual_override_stays_exact(self):
        db = Database([edge((100000 + ei) / delta.SCALE) for ei in range(100)])
        index = delta.DeltaIndex(db, 0, [constraint("length", 1, ">=")])
        groups = index.group()
        self.assertTrue(groups.auto_step)
        self.assertEqual(groups.step_ticks, 10)
        self.assertEqual(len(groups), 10)
        self.assertEqual([group.total for group in groups.page(0, 20)], [10] * 10)
        manual = index.group(1)
        self.assertFalse(manual.auto_step)
        self.assertEqual(manual.step_ticks, 1)
        self.assertEqual(len(manual), 100)
        self.assertEqual(index.group(None).step_ticks, 10)

    def test_zero_anchored_endpoint_extra_interval_is_corrected(self):
        index = self.numeric_index(np.arange(99, 199))
        groups = index.group(None)
        self.assertEqual(groups.step_ticks, 11)
        self.assertEqual(len(groups), 10)
        self.assertEqual(groups[0].low_ticks, 99)
        self.assertEqual(groups[-1].high_ticks, 209)
        self.assertEqual(sum(group.total for group in groups.page(0, 20)), 100)

    def test_abs_and_ratio_have_independent_whole_rule_steps(self):
        errors = [edge((200000 + ei) / delta.SCALE) for ei in range(100)]
        index = delta.DeltaIndex(Database(errors), 0, [constraint("length", 2, ">=")])
        absolute = index.group(None)
        percent = index.group(None, mode="percent")
        self.assertEqual((absolute.step_ticks, percent.step_ticks), (10, 496))
        self.assertEqual((len(absolute), len(percent)), (10, 10))
        self.assertEqual(index.group(None).step_ticks, 10)
        self.assertTrue(percent.auto_step)

    def test_cluster_and_waives_do_not_change_auto_range_or_decode_again(self):
        index = self.numeric_index(np.arange(1000))
        db = index.db
        empty = Cluster("empty", db, 0, np.array([], dtype=np.int64),
                        np.array([], dtype=np.int64))
        one = Cluster("one", db, 0, np.array([17]), np.array([18]))
        # The first query itself can have no members: still use all errors.
        groups = index.group(None, cluster=empty)
        self.assertEqual((groups.step_ticks, groups.total), (100, 0))
        original = index._difference_chunk
        scanned = []

        def track(ids, mode, cancelled):
            scanned.append(len(ids))
            return original(ids, mode, cancelled)

        index._difference_chunk = track
        db._status[:] = 1
        groups = index.group(None, cluster=one)
        self.assertEqual((groups.step_ticks, groups.total, groups[0].count(True)),
                         (100, 1, 1))
        self.assertEqual(scanned, [1], "cached auto step rescanned the full rule")
        db._status[:] = 0
        groups.reset_status()
        self.assertEqual(groups.step_ticks, 100)
        self.assertEqual(groups[0].count(False), 1)

    def test_auto_step_reuses_measured_geometry_across_bases(self):
        errors = RepeatedErrors(1003, edge(1.25))
        index = delta.DeltaIndex(Database(errors), 0,
                                [constraint("length", 1, ">")])
        self.assertEqual(index.group(None).step_ticks, 1)
        reads = errors.reads
        self.assertEqual(index.group(None, mode="percent").step_ticks, 1)
        self.assertEqual(index.group(100).step_ticks, 100)
        self.assertEqual(errors.reads, reads)
        self.assertEqual(index._auto_steps, {"absolute": 1, "percent": 1})

    def test_constant_tiny_empty_and_unmeasurable_ranges_use_one_tick(self):
        for values, want_count in (([0, 1, 2], 3), ([9000000] * 3, 1), ([], 0)):
            with self.subTest(values=values):
                groups = self.numeric_index(values).group(None)
                self.assertEqual((groups.step_ticks, len(groups)), (1, want_count))
        unknown = self.numeric_index([0, 999999], choices=[-1, -1]).group(None)
        self.assertEqual((unknown.step_ticks, len(unknown)), (1, 1))
        self.assertEqual(unknown[0].constraint_index, -1)

    def test_invalid_ratios_are_excluded_from_range_but_kept_as_unknown(self):
        constraints = [constraint("length", 0, ">"),
                       constraint("length", 0.00001, ">"),
                       constraint("length", 1, ">=")]
        values = [1, 10**13, *range(100000, 100010), 0]
        choices = [0, 1, *([2] * 10), -1]
        groups = self.numeric_index(values, constraints, choices).group(None, mode="percent")
        self.assertEqual(groups.step_ticks, 91)  # Valid ratio range 0..900 ticks.
        self.assertEqual(groups[-1].page(0, 10), [0, 1, 12])
        self.assertEqual(groups[-1].constraint_index, -1)
        all_invalid = self.numeric_index(values[:2], constraints, [0, 1]).group(
            None, mode="percent")
        self.assertEqual((all_invalid.step_ticks, len(all_invalid)), (1, 1))

    def test_criterion_ranges_do_not_merge_units_or_distant_offsets(self):
        constraints = [constraint("area", 0, ">="), constraint("length", 0, ">=")]
        values = np.r_[np.arange(100), np.arange(1000000, 1000100)]
        choices = np.r_[np.zeros(100, dtype=np.int32), np.ones(100, dtype=np.int32)]
        groups = self.numeric_index(values, constraints, choices).group(None)
        self.assertEqual(groups.step_ticks, 10)
        self.assertEqual(len(groups), 20, "unrelated criteria were combined")
        self.assertEqual({g.unit for g in groups.page(0, 30)}, {"um", "um2"})
        for ci in (0, 1):
            self.assertEqual(sum(g.constraint_index == ci for g in groups.page(0, 30)), 10)
        constants = self.numeric_index([1, 1000000], constraints, [0, 1]).group(None)
        self.assertEqual(constants.step_ticks, 1)
        self.assertEqual(len(constants), 2)

    def test_int64_endpoints_do_not_overflow_step_arithmetic(self):
        limit = int(np.iinfo(np.int64).max)
        for values in ([0, limit], [limit - 99, limit], [limit, limit]):
            with self.subTest(values=values):
                groups = self.numeric_index(values).group(None)
                self.assertIsInstance(groups.step_ticks, int)
                self.assertGreaterEqual(groups.step_ticks, 1)
                lo, hi = min(values), max(values)
                self.assertLessEqual(hi // groups.step_ticks - lo // groups.step_ticks + 1, 10)
                self.assertEqual(groups.total, len(values))

    def test_cancelled_range_scan_never_publishes_partial_auto_cache(self):
        index = self.numeric_index(np.arange(delta._CHUNK + 3))
        original = index._difference_chunk
        chunks = []

        def track(ids, mode, cancelled):
            result = original(ids, mode, cancelled)
            chunks.append(len(ids))
            return result

        index._difference_chunk = track
        with self.assertRaises(delta.DeltaQueryCancelled):
            index.group(None, cancelled=lambda: bool(chunks))
        self.assertEqual(chunks, [delta._CHUNK])
        self.assertEqual(index._auto_steps, {})
        groups = index.group(None)
        self.assertEqual(groups.total, delta._CHUNK + 3)
        self.assertLessEqual(len(groups), 10)
        self.assertEqual(index._auto_steps, {"absolute": groups.step_ticks})


if __name__ == "__main__":
    unittest.main(verbosity=2)
