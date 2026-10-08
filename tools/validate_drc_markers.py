"""Headless regression gate for viewport-wide DRC marker aggregation.

Real packed fixtures cover complete rule/cluster counts, review filters,
exact viewport edges and zoom refinement. A million-point numeric fixture
checks bounded marker output and working memory without GTK or geometry
objects. Usage: python tools/validate_drc_markers.py [floe-index-bin]
"""

import math
import os
import subprocess
import sys
import tempfile
import time
import tracemalloc
import unittest

import numpy as np

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from floe import drc  # noqa: E402
from floe.drc_clusters import load_clusters  # noqa: E402
from floe.drc_markers import MarkerIndex, MarkerQueryCancelled  # noqa: E402


BIN = os.path.join(os.path.dirname(__file__), "..", "rust", "target",
                   "release", "floe-index")
if len(sys.argv) > 1 and not sys.argv[1].startswith("-"):
    BIN = sys.argv.pop(1)


class TrackedPack(drc.IcePack):
    def __init__(self, path):
        self.geometry_decodes = 0
        super().__init__(path)

    def _block(self, bi):
        self.geometry_decodes += 1
        return super()._block(bi)


class CountingIndex(MarkerIndex):
    def __init__(self, *args, **kwargs):
        self.decoded = []
        super().__init__(*args, **kwargs)

    def _decode_block(self, *args, **kwargs):
        self.decoded.append(args)
        return super()._decode_block(*args, **kwargs)


class MarkerTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if not os.path.isfile(BIN):
            raise RuntimeError("build floe-index or pass its binary path")
        cls.tmp = tempfile.TemporaryDirectory(prefix="floe-markers-")
        cls.db_path = os.path.join(cls.tmp.name, "results.db")
        pack_path = os.path.join(cls.tmp.name, "results.tray")
        cls.grid = [(x, y) for y in range(50) for x in range(100)]
        cls.edge_boxes = [(-10, -10, -10, -10), (-5, -2, -1, -2),
                          (-2, -5, -2, -1), (-3, -3, 3, 3),
                          (0, 0, 0, 0), (5, 5, 5, 5),
                          (10, 10, 10, 10), (5, -5, 9, -1)]
        with open(cls.db_path, "w") as stream:
            stream.write("MAIN 1000\nGRID\n5000 5000 0\n")
            for x, y in cls.grid:
                stream.write("p 1 1\n%d %d\n" % (x * 1000, y * 1000))
            stream.write("EDGES\n%d %d 0\n" %
                         (len(cls.edge_boxes), len(cls.edge_boxes)))
            for i, bbox in enumerate(cls.edge_boxes):
                x0, y0, x1, y1 = (v * 1000 for v in bbox)
                if i == 3:
                    stream.write("p 1 4\n%d %d\n%d %d\n%d %d\n%d %d\n"
                                 % (x0, y0, x1, y0, x1, y1, x0, y1))
                elif i == 7:
                    stream.write("e 1 2\n%d %d %d %d\n%d %d %d %d\n"
                                 % (x0, y0, x1, y1, x0, y1, x1, y0))
                else:
                    stream.write("e 1 1\n%d %d %d %d\n"
                                 % (x0, y0, x1, y1))
            stream.write("EMPTY\n0 0 0\nMILLION\n1000000 1000000 0\n")
            for y in range(1000):
                stream.write("".join("p 1 1\n%d %d\n" % (x * 1000, y * 1000)
                                     for x in range(1000)))
        subprocess.run([BIN, "drc", cls.db_path, pack_path], check=True,
                       capture_output=True, text=True)
        cls.pack = TrackedPack(pack_path)

    @classmethod
    def tearDownClass(cls):
        cls.pack.close()
        cls.tmp.cleanup()

    def setUp(self):
        self.index = CountingIndex(self.pack, cache_bytes=4 * 1024 * 1024)
        self.pack.geometry_decodes = 0
        self.changed = []

    def tearDown(self):
        for ci, ei, old in self.changed:
            self.pack.set_status(ci, ei, old)

    def waive(self, ci, eis):
        for ei in eis:
            self.changed.append((ci, ei, self.pack.get_status(ci, ei)))
            self.pack.set_status(ci, ei, drc.STATUS_WAIVED)

    def cluster(self, body):
        path = os.path.join(self.tmp.name, "selection.clusters")
        with open(path, "w") as stream:
            stream.write("[GRID]\nchosen = " + body + "\n")
        return load_clusters(path, self.pack).rules[0][0]

    def assert_counts(self, markers, count, waived=0):
        self.assertEqual(sum(m.count for m in markers), count)
        self.assertEqual(sum(m.waived for m in markers), waived)
        for marker in markers:
            self.assertGreater(marker.count, 0)
            self.assertGreaterEqual(marker.waived, 0)
            self.assertLessEqual(marker.waived, marker.count)
            self.assertTrue(math.isfinite(marker.x))
            self.assertTrue(math.isfinite(marker.y))
            self.assertEqual(len(marker.bbox), 4)
            self.assertLessEqual(marker.bbox[0], marker.bbox[2])
            self.assertLessEqual(marker.bbox[1], marker.bbox[3])

    def test_overview_counts_all_errors_without_page_cap_or_decode(self):
        markers = self.index.query((-1, -1, 100, 50), 400, 200,
                                   checks=[0], cell_px=16)
        self.assert_counts(markers, 5000)
        self.assertLessEqual(len(markers), 25 * 13)
        self.assertTrue(all(m.ci == 0 for m in markers))
        self.assertTrue(any(m.approximate for m in markers))
        self.assertEqual(self.index.decoded, [],
                         "overview decoded full-resolution geometry")
        self.assertEqual(self.pack.geometry_decodes, 0)
        marker = markers[0]
        with self.assertRaises((AttributeError, TypeError)):
            marker.count = 0

    def test_coarse_cluster_bbox_contains_every_member_without_decode(self):
        # The arbitrary members do not touch the rule bbox extremes. This
        # exercises outward lattice bounds, rather than just the exact
        # rule-wide bbox that happens to contain every error already.
        indices = (101, 202, 1707, 2243, 3566)
        group = self.cluster(",".join(str(ei + 1) for ei in indices))
        markers = self.index.query((-1, -1, 100, 50), 400, 200,
                                   checks=[0], members={0: group}, cell_px=400)
        self.assert_counts(markers, len(indices))
        self.assertEqual(len(markers), 1)
        marker = markers[0]
        self.assertTrue(marker.approximate)
        x0, y0, x1, y1 = marker.bbox
        for ei in indices:
            x, y = self.grid[ei]
            self.assertLessEqual(x0, x)
            self.assertLessEqual(y0, y)
            self.assertGreaterEqual(x1, x)
            self.assertGreaterEqual(y1, y)
        self.assertEqual(self.index.decoded, [])
        self.assertEqual(self.pack.geometry_decodes, 0)

    def test_coarse_bbox_contains_negative_degenerate_and_extended_geometry(self):
        # Includes a point, horizontal/vertical segments, a polygon, and a
        # two-edge record. Containment covers full geometry, not its center.
        markers = self.index.query((-20, -20, 20, 20), 400, 400,
                                   checks=[1], cell_px=400)
        self.assert_counts(markers, len(self.edge_boxes))
        self.assertEqual(len(markers), 1)
        self.assertTrue(markers[0].approximate)
        x0, y0, x1, y1 = markers[0].bbox
        for a, b, c, d in self.edge_boxes:
            self.assertLessEqual(x0, a)
            self.assertLessEqual(y0, b)
            self.assertGreaterEqual(x1, c)
            self.assertGreaterEqual(y1, d)
        self.assertEqual(self.index.decoded, [])
        self.assertEqual(self.pack.geometry_decodes, 0)

    def test_compacted_overview_bboxes_keep_full_population_coverage(self):
        bounds = (-1, -1, 100, 50)
        raw = self.index.query(bounds, 100, 50, checks=[0], cell_px=16)
        markers = self.index.query(bounds, 100, 50, checks=[0], cell_px=16,
                                   declutter=True)
        self.assert_counts(raw, len(self.grid))
        self.assert_counts(markers, len(self.grid))
        self.assertLess(len(markers), len(raw))

        def union(items):
            return (min(m.bbox[0] for m in items),
                    min(m.bbox[1] for m in items),
                    max(m.bbox[2] for m in items),
                    max(m.bbox[3] for m in items))

        self.assertEqual(union(markers), union(raw))
        points = np.asarray(self.grid)
        covered = np.zeros(len(points), dtype=bool)
        for marker in markers:
            x0, y0, x1, y1 = marker.bbox
            covered |= ((x0 <= points[:, 0]) & (points[:, 0] <= x1) &
                        (y0 <= points[:, 1]) & (points[:, 1] <= y1))
        self.assertTrue(np.all(covered), "compacted boxes omit error locations")
        self.assertEqual(self.index.decoded, [])
        self.assertEqual(self.pack.geometry_decodes, 0)

    def test_arbitrary_cluster_and_review_filters_are_complete(self):
        group = self.cluster("1,3000,4001-5000")
        chosen = {0, 2999} | set(range(4000, 5000))
        waived = {0, 4100, 4999}
        self.waive(0, waived)
        for status in (None, False, True):
            want = chosen if status is None else (
                chosen & waived if status else chosen - waived)
            markers = self.index.query((-1, -1, 100, 50), 400, 200,
                                       checks=[0], members={0: group},
                                       waived=status)
            self.assert_counts(markers, len(want), len(want & waived))
            self.assertTrue(all(m.ci == 0 and m.ei in want for m in markers))
        self.assertGreater(len(chosen), 1000)

    def test_zoom_resolves_exact_singletons_and_updates_status(self):
        bounds = (10.5, 10.5, 13.5, 12.5)
        want = {y * 100 + x for y in (11, 12) for x in (11, 12, 13)}
        markers = self.index.query(bounds, 600, 400, checks=[0])
        self.assert_counts(markers, len(want))
        self.assertEqual({m.ei for m in markers}, want)
        for marker in markers:
            self.assertEqual(marker.count, 1)
            self.assertFalse(marker.approximate)
            x, y = self.grid[marker.ei]
            self.assertEqual((marker.x, marker.y), (x, y))
            self.assertEqual(tuple(marker.bbox), (x, y, x, y))
        target = min(want)
        self.waive(0, [target])
        changed = self.index.query(bounds, 600, 400, checks=[0], waived=True)
        self.assert_counts(changed, 1, 1)
        self.assertEqual(changed[0].ei, target)
        self.assertEqual(self.pack.geometry_decodes, 0,
                         "numeric refinement created DrcError objects")

    def test_exact_boundary_counts_negative_and_degenerate_geometry(self):
        bounds_list = [(-3, -3, 0, 0), (-10, -10, -9, -9),
                       (-2, -6, -1.9, 0), (-100, -100, -50, -50),
                       (5, -5, 9, -1), (-20, -20, 20, 20)]
        for bounds in bounds_list:
            x0, y0, x1, y1 = bounds
            want = {ei for ei, (a, b, c, d) in enumerate(self.edge_boxes)
                    if a <= x1 and c >= x0 and b <= y1 and d >= y0}
            markers = self.index.query(bounds, 300, 300, checks=[1])
            self.assert_counts(markers, len(want))
            self.assertTrue(all(m.ei in want and m.ci == 1 for m in markers))

    def test_invalid_or_empty_viewports_do_not_create_markers(self):
        for bounds, width, height in (((1, 1, 1, 1), 300, 300),
                                      ((2, 0, 1, 2), 300, 300),
                                      ((0, 0, 1, 1), 0, 300),
                                      ((0, 0, 1, 1), 300, -1)):
            self.assertEqual(self.index.query(bounds, width, height), [])
        for bounds in ((0, 0, float("inf"), 1),
                       (0, float("nan"), 1, 1)):
            with self.assertRaises(ValueError):
                self.index.query(bounds, 300, 300)

    def test_partial_overview_has_exact_intersection_counts(self):
        for bounds in ((17.15, 13.25, 77.1, 37.7),
                       (0, 0, 99, 49), (99, 49, 101, 51),
                       (40.1, 20.1, 40.9, 20.9)):
            x0, y0, x1, y1 = bounds
            want = {ei for ei, (x, y) in enumerate(self.grid)
                    if x0 <= x <= x1 and y0 <= y <= y1}
            markers = self.index.query(bounds, 160, 100, checks=[0])
            self.assert_counts(markers, len(want))
            self.assertTrue(all(m.ei in want for m in markers))

    def test_rule_filter_empty_rules_and_missing_members_entry(self):
        group = self.cluster("1-100")
        markers = self.index.query((-20, -20, 110, 60), 400, 250,
                                   checks=[0, 1, 2], members={0: group})
        self.assert_counts(markers, 100 + len(self.edge_boxes))
        self.assertEqual(self.index.query((-20, -20, 110, 60), 400, 250,
                                          checks=[2]), [])
        self.assertEqual(self.index.query((-20, -20, 110, 60), 400, 250,
                                          checks=[]), [])

    def test_million_errors_have_bounded_output_and_working_memory(self):
        tracemalloc.start()
        started = time.perf_counter()
        try:
            markers = self.index.query((-1, -1, 1000, 1000), 512, 512,
                                       checks=[3], cell_px=16)
            _current, peak = tracemalloc.get_traced_memory()
        finally:
            tracemalloc.stop()
        self.assert_counts(markers, 1_000_000)
        self.assertLessEqual(len(markers), 32 * 32)
        self.assertLess(peak, 48 * 1024 * 1024,
                        "overview allocated a full Python object per error")
        self.assertEqual(self.index.decoded, [])
        self.assertEqual(self.pack.geometry_decodes, 0)
        print("million markers: %d bins, %.3f s, %.2f MiB peak" %
              (len(markers), time.perf_counter() - started, peak / 2**20))
        # Extreme display resolution still changes aggregation density,
        # never the count, and cannot produce unbounded canvas objects.
        large = self.index.query((-1, -1, 1000, 1000), 8192, 8192,
                                 checks=[3], cell_px=1)
        self.assert_counts(large, 1_000_000)
        self.assertLessEqual(len(large), 8192)

    def test_cancellation_stops_query_and_followup_recovers(self):
        with self.assertRaises(MarkerQueryCancelled):
            self.index.query((-1, -1, 1000, 1000), 512, 512,
                             checks=[3], cancelled=lambda: True)
        polls = [0]

        def cancelled():
            polls[0] += 1
            return polls[0] >= 6

        with self.assertRaises(MarkerQueryCancelled):
            self.index.query((-1, -1, 1000, 1000), 512, 512,
                             checks=[3], cancelled=cancelled)
        self.assertGreaterEqual(polls[0], 6)
        markers = self.index.query((10.5, 10.5, 11.5, 11.5), 100, 100,
                                   checks=[0])
        self.assert_counts(markers, 1)
        self.assertEqual(markers[0].ei, 1111)

    def test_exact_numeric_cache_obeys_budget_and_reuses_recent_blocks(self):
        budget = 4096
        index = CountingIndex(self.pack, cache_bytes=budget)
        markers = index.query((-1, -1, 100, 50), 3000, 1500, checks=[0])
        self.assert_counts(markers, 5000)
        self.assertGreater(len(index.decoded), 2)
        self.assertLessEqual(index._cache_size, budget)
        self.assertEqual(index._cache_size,
                         sum(boxes.nbytes for boxes in index._cache.values()))
        before = len(index.decoded)
        markers = index.query((98.5, 48.5, 99.5, 49.5), 100, 100, checks=[0])
        self.assert_counts(markers, 1)
        self.assertEqual(markers[0].ei, 4999)
        self.assertEqual(len(index.decoded), before,
                         "recent numeric bbox cache was not reused")
        self.assertLessEqual(index._cache_size, budget)
        self.assertEqual(self.pack.geometry_decodes, 0)

    def test_ascii_fallback_counts_and_exact_geometry(self):
        errors = [drc.DrcError("p", i + 1, [(x, y)])
                  for i, (x, y) in enumerate(((-1, -1), (0, 0), (1, 1)))]
        check = drc.DrcCheck("ASCII")
        check.errors = errors
        db = drc.DrcDb("memory", "TOP", 1, [check])
        index = MarkerIndex(db)
        markers = index.query((-2, -2, 2, 2), 400, 400)
        self.assert_counts(markers, 3)
        self.assertTrue(all(m.count == 1 and not m.approximate for m in markers))
        self.assertEqual(index.query((-2, -2, 2, 2), 400, 400,
                                     waived=True), [])


if __name__ == "__main__":
    unittest.main(verbosity=2)
