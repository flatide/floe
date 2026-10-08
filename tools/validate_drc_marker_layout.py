"""Regression gate for layout visibility with dense DRC markers.

The 100-million-error case uses the bounded weighted summaries emitted by
the spatial query, not 100 million Python geometry objects. It checks the
display policy independently of the packed-query correctness gates.
Usage: python tools/validate_drc_marker_layout.py
"""

import math
import os
import sys
import time
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from floe import drc  # noqa: E402
from floe.drc_marker_layout import compact_markers  # noqa: E402
from floe.drc_marker_style import aggregate_radius  # noqa: E402
from floe.drc_markers import Marker, MarkerIndex, MarkerQueryCancelled  # noqa: E402


def marker(ei, x, y, count=1, waived=0, bbox=None, approximate=False):
    return Marker(ei % 3, ei, x, y, count, waived,
                  (x, y, x, y) if bbox is None else bbox, approximate)


def population_markers(total=100_000_000):
    """8192 spatial summaries with exact integer population and waive totals."""
    per, extra = divmod(total, 8192)
    result = []
    for i in range(8192):
        x, y = (i % 128 + 0.5) / 128, (i // 128 + 0.5) / 64
        count = per + (i < extra)
        waived = (0, count // 3, count)[i % 3]
        result.append(marker(i, x, y, count, waived,
                             (x - 0.003, y - 0.006,
                              x + 0.003, y + 0.006), True))
    return result


def bbox_union(markers):
    return (min(m.bbox[0] for m in markers),
            min(m.bbox[1] for m in markers),
            max(m.bbox[2] for m in markers),
            max(m.bbox[3] for m in markers))


class MarkerLayoutTests(unittest.TestCase):
    def assert_population(self, original, result):
        self.assertEqual(sum(m.count for m in result),
                         sum(m.count for m in original))
        self.assertEqual(sum(m.waived for m in result),
                         sum(m.waived for m in original))
        self.assertEqual(bbox_union(result), bbox_union(original))
        representatives = {(m.ci, m.ei) for m in original}
        for m in result:
            self.assertIn((m.ci, m.ei), representatives)
            self.assertGreater(m.count, 0)
            self.assertLessEqual(0, m.waived)
            self.assertLessEqual(m.waived, m.count)
            self.assertTrue(math.isfinite(m.x) and math.isfinite(m.y))

    def assert_coverage(self, markers, width, height):
        footprint = sum(math.pi * (aggregate_radius(m.count, width, height) + 1) ** 2
                        if m.count > 1 else 25 for m in markers)
        # Below one marker's footprint a single remaining marker is allowed.
        if len(markers) != 1:
            self.assertLessEqual(footprint, width * height * 0.10 + 1e-8)
        return footprint / (width * height)

    def assert_separated(self, markers, bounds, width, height):
        x0, y0, x1, y1 = bounds
        circles = []
        for m in markers:
            x = round(min(width - 1, max(0, (m.x - x0) / (x1 - x0) * width)))
            y = round(min(height - 1, max(0, (y1 - m.y) / (y1 - y0) * height)))
            radius = (aggregate_radius(m.count, width, height) + 1
                      if m.count > 1 else math.sqrt(12.5))
            circles.append((x, y, radius))
        for i, (x, y, radius) in enumerate(circles):
            for ox, oy, other_radius in circles[i + 1:]:
                self.assertGreaterEqual(math.hypot(x - ox, y - oy) + 1e-8,
                                        radius + other_radius + 4)

    def test_hundred_million_population_retains_counts_and_layout_space(self):
        original = population_markers()
        before = list(original)
        bounds, width, height = (0, 0, 1, 1), 1024, 768
        started = time.perf_counter()
        result = compact_markers(original, bounds, width, height)
        elapsed = time.perf_counter() - started
        self.assert_population(original, result)
        self.assertEqual(original, before, "compaction mutated query summaries")
        self.assertLessEqual(len(result), 300)
        coverage = self.assert_coverage(result, width, height)
        self.assert_separated(result, bounds, width, height)
        self.assertEqual(result, compact_markers(original, bounds, width, height))
        print("100M errors: %d displayed markers, %.2f%% conservative coverage, %.3f s" %
              (len(result), coverage * 100, elapsed))

    def test_coverage_and_separation_follow_viewport_size_and_aspect(self):
        original, bounds = population_markers(), (0, 0, 1, 1)
        for width, height in ((320, 200), (1280, 720), (3840, 2160),
                              (720, 1280), (2400, 240), (32, 24), (8, 8)):
            with self.subTest(width=width, height=height):
                result = compact_markers(original, bounds, width, height)
                self.assert_population(original, result)
                self.assert_coverage(result, width, height)
                self.assert_separated(result, bounds, width, height)

    def test_markers_across_grid_corner_merge_when_circles_touch(self):
        original = [marker(i, x, y, 25_000_000, i * 1_000_000)
                    for i, (x, y) in enumerate(((31.8, 31.8), (32.2, 31.8),
                                               (31.8, 32.2), (32.2, 32.2)))]
        result = compact_markers(original, (0, 0, 512, 512), 512, 512)
        self.assert_population(original, result)
        self.assertEqual(len(result), 1)

    def test_sparse_exact_singletons_keep_identity_and_geometry(self):
        original = [marker(i, x, y, waived=i % 2,
                           bbox=(x - 0.25, y - 0.5, x + 0.25, y + 0.5))
                    for i, (x, y) in enumerate(((10, 20), (50, 50), (90, 80)))]
        result = compact_markers(original, (0, 0, 100, 100), 1000, 800)
        self.assertEqual(sorted(result), sorted(original))
        self.assertTrue(all(not m.approximate for m in result))

    def test_merge_unions_full_geometry_and_keeps_remote_singleton(self):
        original = [marker(0, 10, 10, 10, 4, (-100, -50, 20, 30), True),
                    marker(1, 10.02, 10.03, 7, 2, (-25, -70, 90, 40)),
                    marker(2, 90, 90)]
        result = compact_markers(original, (0, 0, 100, 100), 1000, 800)
        self.assert_population(original, result)
        self.assertEqual(len(result), 2)
        self.assertIn(original[2], result)
        combined = next(m for m in result if m.count == 17)
        self.assertEqual(combined.waived, 6)
        self.assertEqual(combined.bbox, (-100, -70, 90, 40))

    def test_edge_clipping_merges_long_geometry_at_visible_anchors(self):
        original = [marker(0, -1000, 50, 100, 20, (-2100, 40, 100, 60)),
                    marker(1, -500, 50.1, 200, 30, (-1100, 45, 100, 55)),
                    marker(2, 1000, 50, 300, 40, (0, 40, 2000, 60)),
                    marker(3, 500, 50.1, 400, 50, (0, 45, 1000, 55))]
        result = compact_markers(original, (0, 0, 100, 100), 1000, 800)
        self.assert_population(original, result)
        self.assertEqual(len(result), 2)
        self.assert_separated(result, (0, 0, 100, 100), 1000, 800)
        self.assertEqual(sorted(m.count for m in result), [300, 700])

    def test_cancellation_aborts_without_partial_result_and_can_retry(self):
        original = population_markers()
        with self.assertRaises(MarkerQueryCancelled):
            compact_markers(original, (0, 0, 1, 1), 1024, 768,
                            cancelled=lambda: True)
        polls = [0]

        def cancelled():
            polls[0] += 1
            return polls[0] >= 5

        with self.assertRaises(MarkerQueryCancelled):
            compact_markers(original, (0, 0, 1, 1), 1024, 768, cancelled=cancelled)
        self.assertGreaterEqual(polls[0], 5)
        self.assert_population(original, compact_markers(original, (0, 0, 1, 1), 1024, 768))

    def test_real_query_declutters_and_zoom_restores_exact_errors(self):
        class ReviewDb(drc.DrcDb):
            def get_status(self, ci, ei):
                return drc.STATUS_WAIVED if ei % 7 == 0 else drc.STATUS_NONE

        check = drc.DrcCheck("DENSE")
        check.errors = [drc.DrcError("p", y * 64 + x + 1, [(x, y)])
                        for y in range(64) for x in range(64)]
        db = ReviewDb("memory", "TOP", 1, [check])
        index = MarkerIndex(db)
        bounds = (-1, -1, 64, 64)
        raw = index.query(bounds, 512, 512, cell_px=16)
        result = index.query(bounds, 512, 512, cell_px=16, declutter=True)
        self.assert_population(raw, result)
        self.assertEqual(sum(m.count for m in result), 4096)
        self.assertEqual(sum(m.waived for m in result), 586)
        self.assertLess(len(result), len(raw))
        self.assert_coverage(result, 512, 512)
        self.assert_separated(result, bounds, 512, 512)
        waived = index.query(bounds, 512, 512, waived=True, declutter=True)
        self.assertEqual(sum(m.count for m in waived), 586)
        self.assertTrue(all(m.count == m.waived for m in waived))
        zoom = index.query((9.5, 9.5, 11.5, 11.5), 512, 512, declutter=True)
        self.assertEqual({m.ei for m in zoom}, {650, 651, 714, 715})
        for m in zoom:
            self.assertEqual(m.count, 1)
            self.assertFalse(m.approximate)
            self.assertEqual(m.bbox, check.errors[m.ei].bbox())

    def test_empty_scope_stays_empty(self):
        self.assertEqual(compact_markers([], (0, 0, 1, 1), 1024, 768), [])


if __name__ == "__main__":
    unittest.main(verbosity=2)
