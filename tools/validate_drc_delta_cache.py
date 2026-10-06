"""Persistent delta gate: real subprocesses, cache reuse and paged review.

All packs, review files and derived caches stay inside temporary directories.
Run with a Python environment containing NumPy and a built floe-index.
"""

import gc
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import unittest
from concurrent.futures import ThreadPoolExecutor
from unittest.mock import patch

import numpy as np

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from floe import cachepath, drc, drc_delta as delta  # noqa: E402
from floe import drc_delta_cache as cache  # noqa: E402
from floe.drc_clusters import Cluster  # noqa: E402

BIN = os.path.join(os.path.dirname(__file__), "..", "rust", "target",
                   "release", "floe-index")
CONSTRAINTS = [{"metric": "width", "op": "<", "value": 0.05,
                "text": "INT M1 < 0.05"}]


def members(groups):
    return {group.key: group.page(0, group.total + 1)
            for group in groups.page(0, len(groups) + 1)}


@unittest.skipUnless(os.path.isfile(BIN), "requires built floe-index")
class PersistentDeltaTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.fixture = tempfile.TemporaryDirectory(prefix="floe-delta-fixture-")
        cls.count = 8209
        cls.source = os.path.join(cls.fixture.name, "source.db")
        cls.pack = os.path.join(cls.fixture.name, "source.tray")
        with open(cls.source, "w", encoding="utf-8") as stream:
            stream.write("MAIN 100000\nR\n%d %d 0\n" % (cls.count, cls.count))
            for ei in range(cls.count):
                width, x = 2000 + (ei % 5) * 101, -ei * 13
                stream.write("p %d 4\n%d 0\n%d 0\n%d 9000\n%d 9000\n" %
                             (ei + 1, x, x + width, x + width, x))
            stream.write("EMPTY\n0 0 0\n")
        subprocess.run([BIN, "drc", cls.source, cls.pack], check=True,
                       capture_output=True)

    @classmethod
    def tearDownClass(cls):
        cls.fixture.cleanup()

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="floe-delta-cache-")
        self.addCleanup(self.temporary.cleanup)
        environment = patch.dict(os.environ,
                                  {"FLOE_DRC_ANALYSIS_ROOT": self.temporary.name})
        environment.start()
        self.addCleanup(environment.stop)
        path = os.path.join(self.temporary.name, "results.tray")
        shutil.copyfile(self.pack, path)
        self.db = drc.IcePack(path)
        self.addCleanup(self.db.close)

    def index(self, constraints=CONSTRAINTS, ci=0):
        return delta.DeltaIndex(self.db, ci, constraints)

    def test_real_child_parity_both_modes_and_no_parent_measurement(self):
        reference = self.index().measure()
        index = self.index()
        pids = []
        with patch.object(delta.DeltaIndex, "measure",
                          side_effect=AssertionError("geometry touched in parent")):
            cache.process_measure(index, process_callback=pids.append)
        self.assertTrue(any(isinstance(pid, int) and pid != os.getpid() for pid in pids))
        np.testing.assert_array_equal(index.measured_ticks, reference.measured_ticks)
        np.testing.assert_array_equal(index.constraint_indices, reference.constraint_indices)
        np.testing.assert_array_equal(index.estimated_flags, reference.estimated_flags)
        self.assertIsInstance(index.measured_ticks, np.memmap)
        self.assertFalse(index.measured_ticks.flags.writeable)
        for mode in ("absolute", "percent"):
            with self.subTest(mode=mode):
                expected = reference.group(mode=mode)
                with patch.object(delta.DeltaGroups, "__init__",
                                  side_effect=AssertionError("O(N) parent restore")):
                    actual = cache.process_group(index, mode=mode)
                self.assertEqual(actual.step_ticks, expected.step_ticks)
                self.assertEqual(members(actual), members(expected))
                self.assertEqual(actual.estimated_total, expected.estimated_total)
                self.assertTrue(actual._persistent)
                self.assertIsInstance(actual._row_for_error, np.memmap)

    def test_restart_and_rule_round_trip_reuse_immutable_files(self):
        index = cache.process_measure(self.index())
        first = cache.process_group(index)
        measurement = index.measured_ticks.filename
        membership = first._ids.filename
        stamps = [os.stat(path).st_mtime_ns for path in (measurement, membership)]
        reopened = drc.IcePack(self.db.path, review=False,
                               review_path=self.db._waive_path)
        self.addCleanup(reopened.close)
        index = delta.DeltaIndex(reopened, 0, CONSTRAINTS)
        with patch.object(cache, "_run_child", side_effect=AssertionError("CD child restarted")):
            cache.process_measure(index)
        second = cache.process_group(index)
        self.assertEqual(second._ids.filename, membership)
        self.assertEqual([os.stat(path).st_mtime_ns for path in (measurement, membership)], stamps)
        self.assertEqual(members(first), members(second))

    def test_cluster_empty_unknown_and_noncontiguous_membership(self):
        index = cache.process_measure(self.index())
        cluster = Cluster("scattered", self.db, 0, [0, 17, 4093, 8100],
                           [3, 23, 4099, 8107])
        for mode in ("absolute", "percent"):
            expected = index.group(137, mode=mode, cluster=cluster)
            actual = cache.process_group(index, 137, mode=mode, cluster=cluster)
            self.assertEqual(members(actual), members(expected))
            self.assertEqual(actual.total, cluster.total)
        empty = Cluster("empty", self.db, 0, [], [])
        groups = cache.process_group(index, cluster=empty)
        self.assertEqual((len(groups), groups.total, groups._offsets.tolist()), (0, 0, [0]))
        unknown = cache.process_measure(self.index([]))
        groups = cache.process_group(unknown)
        self.assertEqual(groups[0].key, (-1, 0))
        self.assertEqual(groups[0].page(self.count - 2, 10), [self.count - 2, self.count - 1])
        no_errors = cache.process_measure(self.index(ci=1))
        self.assertEqual(len(cache.process_group(no_errors)), 0)

    def test_empty_rule_measure_and_groups_need_no_child_cache_or_temporary_files(self):
        forbidden = AssertionError("empty rule touched preprocessing or cache I/O")
        pids = []
        with patch.object(cache, "_run_child", side_effect=forbidden), \
                patch.object(cache, "load_measurements", side_effect=forbidden), \
                patch.object(cachepath, "drc_analysis_dir", side_effect=forbidden), \
                patch.object(cache.tempfile, "mkdtemp", side_effect=forbidden), \
                patch.object(np, "memmap", side_effect=forbidden):
            index = self.index(ci=1)
            self.assertIs(cache.process_measure(index, process_callback=pids.append), index)
            self.assertEqual(index.measured_ticks.tolist(), [])
            self.assertEqual(index.constraint_indices.tolist(), [])
            self.assertEqual(index.estimated_flags.tolist(), [])
            self.assertEqual(cache.prepare_group_cache(index), 0)
            for mode in ("absolute", "percent"):
                for step in (None, 123):
                    with self.subTest(mode=mode, step=step):
                        groups = cache.process_group(self.index(ci=1), step, mode=mode,
                                                     process_callback=pids.append)
                        self.assertEqual((len(groups), groups.total), (0, 0))
                        self.assertEqual(groups.page(0, 1000), [])
                        self.assertEqual(groups._offsets.tolist(), [0])
                        self.assertEqual(groups.step_ticks, 1 if step is None else step)
                        self.assertEqual(groups.auto_step, step is None)
                        self.assertEqual(groups.mode, mode)
            with self.assertRaises(delta.DeltaQueryCancelled):
                cache.process_measure(self.index(ci=1), cancelled=lambda: True)
            with self.assertRaises(delta.DeltaQueryCancelled):
                cache.process_group(self.index(ci=1), cancelled=lambda: True)
            for options in ({"step": 0}, {"step": -1}, {"mode": "invalid"}):
                with self.subTest(options=options), self.assertRaises(ValueError):
                    cache.process_group(self.index(ci=1), **options)
        self.assertEqual(pids, [])

    def test_status_pages_ranks_updates_and_private_snapshots(self):
        original = [0, 1, 4095, 4096, 4097, 8191, 8208]
        for ei in original:
            self.db.set_status(0, ei, drc.STATUS_WAIVED)
        index = cache.process_measure(self.index())
        # A broad manual bin puts every member in one group across 4096-row pages.
        first = cache.process_group(index, 100000)
        second = cache.process_group(index, 100000)
        group = first[0]
        self.assertEqual(group.page(0, 100, True), original)
        self.assertEqual(group.page(3, 3, True), original[3:6])
        self.assertEqual(group.rank(8191, True), 5)
        for ei, state in [(1, 0), (4096, 0), (4098, 1), (8000, 1)]:
            self.db.set_status(0, ei, state)
        first.status_changed([1, 4096, 4098, 8000])
        expected = [ei for ei in range(self.count)
                    if self.db.get_status(0, ei) == drc.STATUS_WAIVED]
        self.assertEqual(group.page(0, 100, True), expected)
        self.assertEqual(group.count(True), len(expected))
        self.assertEqual(group.rank(8000, True), expected.index(8000))
        self.assertEqual(second[0].page(0, 100, True), original)
        refreshed = cache.process_group(index, 100000)
        self.assertEqual(refreshed[0].page(0, 100, True), expected)
        scratch = os.path.dirname(first._statuses.filename)
        del group, first
        gc.collect()
        self.assertFalse(os.path.exists(scratch), "released groups leaked a review snapshot")

    def test_constraint_version_and_corrupt_size_invalidate_measurements(self):
        first = cache.process_measure(self.index())
        other_constraints = [dict(CONSTRAINTS[0], value=0.06)]
        self.assertFalse(cache.load_measurements(self.index(other_constraints)))
        with patch.object(cache, "VERSION", cache.VERSION + 1):
            self.assertFalse(cache.load_measurements(self.index()))
        path = first.measured_ticks.filename
        # Avoid truncating an in-use map; only a fresh loader handles this cache.
        del first
        gc.collect()
        os.truncate(path, 8)
        self.assertFalse(cache.load_measurements(self.index()))
        repaired = cache.process_measure(self.index())
        self.assertEqual(len(repaired.measured_ticks), self.count)
        manifest = os.path.join(os.path.dirname(repaired.measured_ticks.filename),
                                 "complete.json")
        for invalid in ("null", "[]"):
            with self.subTest(manifest=invalid):
                with open(manifest, "w", encoding="utf-8") as stream:
                    stream.write(invalid)
                self.assertFalse(cache.load_measurements(self.index()))
                repaired = cache.process_measure(self.index())
                self.assertEqual(len(repaired.measured_ticks), self.count)

    def test_cancellation_cleans_child_and_unpublished_cache(self):
        cancel = threading.Event()
        pids = []

        def progress(message):
            if message.startswith("CD measurement "):
                cancel.set()

        with self.assertRaises(delta.DeltaQueryCancelled):
            cache.process_measure(self.index(), cancelled=cancel.is_set,
                                   progress=progress, process_callback=pids.append)
        self.assertIsNone(pids[-1])
        for parent, folders, _files in os.walk(cachepath.drc_analysis_dir(self.db.path)):
            self.assertFalse(any(name.startswith(".measure-") for name in folders), parent)

    def test_concurrent_same_key_builds_publish_readable_results(self):
        with ThreadPoolExecutor(max_workers=2) as pool:
            results = list(pool.map(lambda _: cache.process_measure(self.index()), range(2)))
        self.assertEqual(results[0].measured_ticks.filename, results[1].measured_ticks.filename)
        np.testing.assert_array_equal(results[0].measured_ticks, results[1].measured_ticks)

    def test_group_builder_uses_disk_sort_without_lexsort(self):
        index = cache.process_measure(self.index())
        key, _automatic = cache._group_key(index, 1, "absolute", "all")
        with patch.object(np, "lexsort", side_effect=AssertionError("full-rule lexsort")):
            _folder, (meta, _arrays) = cache._build_group(index, key, None, {})
        self.assertEqual(meta["total"], self.count)
        self.assertEqual(meta["groups"], 5)

    def test_scatter_and_external_sort_match_at_chunk_boundaries(self):
        index = cache.process_measure(self.index())
        expected = index.group(137)
        key, _automatic = cache._group_key(index, 137, "absolute", "all")
        with patch.object(cache, "_SCATTER_LIMIT", 0), patch.object(cache, "_CHUNK", 257):
            _folder, (meta, arrays) = cache._build_group(index, key, None, {})
        self.assertEqual(meta["groups"], len(expected))
        np.testing.assert_array_equal(arrays["ids"], expected._ids)
        np.testing.assert_array_equal(arrays["row_for_error"], expected._row_for_error)
        np.testing.assert_array_equal(arrays["estimated_counts"], expected._estimated_counts)
        # Distinct fixed-precision bins exercise the automatically selected
        # fallback, including directory continuation across output chunks.
        index.measured_ticks = 5000 + np.arange(self.count, dtype=np.int64)
        index.constraint_indices = np.zeros(self.count, dtype=np.int32)
        index.estimated_flags = np.arange(self.count) % 3 == 0
        expected = index.group(1)
        key, _automatic = cache._group_key(index, 1, "absolute", "all")
        with patch.object(cache, "_CHUNK", 257):
            _folder, (meta, arrays) = cache._build_group(index, key, None, {})
        self.assertEqual(meta["groups"], self.count)
        np.testing.assert_array_equal(arrays["ids"], expected._ids)
        np.testing.assert_array_equal(arrays["bins"], expected._bins)
        np.testing.assert_array_equal(arrays["estimated_counts"], expected._estimated_counts)


if __name__ == "__main__":
    unittest.main(verbosity=2)
