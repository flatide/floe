"""Persistent spatial tree correctness, reuse, cancellation and size gate.

All existing packed marker regressions are repeated through the tree path.
Optional --benchmark-10m compares full-scan overview with a warmed tree on
10 million errors in deliberately non-spatial file order.
"""
import os
import shutil
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

import numpy as np

sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..'))
import validate_drc_markers as regression
from floe import drc
from floe.drc_markers import MarkerIndex, MarkerQueryCancelled
from floe.drc_spatial import SpatialRule, prepare_rule, rule_path, _codes
from floe.drc_marker_worker import export_members, import_members, SelectedMembers


class SpatialTests(regression.MarkerTests):
    def setUp(self):
        super().setUp()
        self.index = regression.CountingIndex(self.pack, cache_bytes=4 * 1024 * 1024,
                                              spatial=True)

    def test_prepared_rule_reopens_without_population_scan(self):
        tree = prepare_rule(self.pack, 0)
        with patch('floe.drc_spatial._add', side_effect=AssertionError('rebuild')):
            reopened = prepare_rule(self.pack, 0)
            self.assertEqual(reopened.path, tree.path)
            self.assert_counts(self.index.query((-1, -1, 100, 50), 400, 200,
                                                checks=[0]), 5000)
        self.assertEqual(self.index.decoded, [])

    def test_postings_are_a_permutation_and_match_morton_leaf(self):
        tree = prepare_rule(self.pack, 0)
        self.assertEqual(sorted(tree.ids.tolist()), list(range(5000)))
        es = int(self.pack._dir_es[0])
        for leaf in np.flatnonzero(tree.nodes['count'][tree.leaf_start:]):
            ids = tree.leaf_ids(tree.leaf_start + int(leaf)).astype(np.int64)
            self.assertTrue(np.all(_codes(self.pack._qbox[es + ids], tree.depth) == leaf))

    def test_filtered_tree_reused_and_same_count_status_swap_refreshed(self):
        members = self.cluster('1-5000')
        self.waive(0, [0])
        bounds = (-1, -1, 100, 50)
        self.assert_counts(self.index.query(bounds, 400, 200, checks=[0],
                                           members={0: members}), 5000, 1)
        with patch('floe.drc_spatial._add', side_effect=AssertionError('rescan')):
            self.assert_counts(self.index.query(bounds, 500, 300, checks=[0],
                                               members={0: members}), 5000, 1)
        self.pack.set_status(0, 0, drc.STATUS_NONE)
        self.waive(0, [4999])
        markers = self.index.query(bounds, 400, 200, checks=[0],
                                   members={0: members}, waived=True)
        self.assert_counts(markers, 1, 1)
        self.assertEqual(markers[0].ei, 4999)

    def test_cancelled_build_is_not_published_and_temporary_files_removed(self):
        path = rule_path(self.pack, 1)
        shutil.rmtree(path, ignore_errors=True)
        calls = [0]

        def cancelled():
            calls[0] += 1
            return calls[0] >= 3
        with self.assertRaises(MarkerQueryCancelled):
            prepare_rule(self.pack, 1, cancelled=cancelled)
        self.assertFalse(os.path.exists(path))
        self.assertFalse(any(name.startswith('.spatial-build-')
                             for name in os.listdir(os.path.dirname(path))))
        self.assertEqual(int(prepare_rule(self.pack, 1).nodes['count'][0]),
                         len(self.edge_boxes))

    def test_bad_metadata_is_rebuilt_and_stale_identity_is_rejected(self):
        tree = prepare_rule(self.pack, 1)
        meta = os.path.join(tree.path, 'meta.json')
        for broken in ('{"version": 0}', 'null', '[]'):
            with self.subTest(manifest=broken):
                with open(meta, 'w') as stream:
                    stream.write(broken)
                with self.assertRaises(ValueError):
                    SpatialRule(tree.path, self.pack, 1)
                rebuilt = prepare_rule(self.pack, 1)
                self.assertEqual(int(rebuilt.nodes['count'][0]), len(self.edge_boxes))

    def test_membership_descriptors_only_contain_scalars_and_paths(self):
        with tempfile.TemporaryDirectory() as directory:
            for member in (self.cluster('1,3,4000-5000'),
                           SelectedMembers([0, 2, 4999])):
                spec = export_members(member, directory, 5000)
                self.assertNotIn('numpy', repr(spec))
                copied = import_members(spec)
                np.testing.assert_array_equal(copied.mask(0, 5000), member.mask(0, 5000))
                wanted = member.mask(0, 5000)
                ids = np.array([4999, 1, 0, 3999, 10])
                np.testing.assert_array_equal(copied.contains_many(ids), wanted[ids])

    def test_pack_replacement_rejects_open_snapshot_and_gets_new_cache(self):
        with tempfile.TemporaryDirectory() as directory:
            path = os.path.join(directory, 'copy.tray')
            shutil.copyfile(self.pack.path, path)
            old = drc.IcePack(path, review=False)
            try:
                before = prepare_rule(old, 0).path
                stat = os.stat(path)
                os.utime(path, ns=(stat.st_atime_ns, stat.st_mtime_ns + 1_000_000))
                with self.assertRaisesRegex(ValueError, 'pack changed'):
                    prepare_rule(old, 0)
                new = drc.IcePack(path, review=False)
                try:
                    self.assertNotEqual(before, prepare_rule(new, 0).path)
                finally:
                    new.close()
            finally:
                old.close()

    def test_unwritable_cache_keeps_complete_markers_with_scan_fallback(self):
        with patch('floe.drc_spatial.prepare_rule', side_effect=PermissionError('read only')) as prepare:
            for _ in range(2):
                self.assert_counts(self.index.query((-1, -1, 100, 50), 400, 200,
                                                    checks=[0]), 5000)
            prepare.assert_called_once()

    def test_delta_descriptor_maps_a_sliced_array_without_copy_or_wrong_offset(self):
        from types import SimpleNamespace
        with tempfile.TemporaryDirectory() as directory:
            rows = np.memmap(os.path.join(directory, 'rows.bin'), mode='w+',
                             dtype='<i4', shape=(20,))
            rows[:] = np.arange(20) % 3
            rows.flush()
            group = SimpleNamespace(_owner=SimpleNamespace(_row_for_error=rows[3:15]), _row=1)
            spec = export_members(group, directory)
            self.assertEqual(spec['array']['offset'], 12)
            self.assertEqual(len(os.listdir(directory)), 1)
            copied = import_members(spec)
            np.testing.assert_array_equal(copied.mask(0, 12), rows[3:15] == 1)


def benchmark_10m():
    """Real pack; shuffled affine order avoids file-block spatial locality."""
    import subprocess
    n = 10_000_000
    with tempfile.TemporaryDirectory(prefix='floe-spatial-10m-') as directory:
        path, pack = os.path.join(directory, 'ten.db'), os.path.join(directory, 'ten.tray')
        with open(path, 'w') as stream:
            stream.write('TOP 1\nSHUFFLED\n%d %d 0\n' % (n, n))
            for start in range(0, n, 65536):
                ids = (np.arange(start, min(start+65536, n), dtype=np.int64) * 7_919) % n
                stream.write(''.join('p 1 1\n%d %d\n' % (ei % 4000, ei // 4000) for ei in ids))
        subprocess.run([regression.BIN, 'drc', path, pack], check=True, capture_output=True)
        db = drc.IcePack(pack)
        try:
            bounds = (-1, -1, 4000, 2500)
            began = time.perf_counter()
            old = MarkerIndex(db).query(bounds, 512, 512, checks=[0])
            before = time.perf_counter() - began
            import tracemalloc
            tracemalloc.start()
            began = time.perf_counter()
            tree = prepare_rule(db, 0)
            build = time.perf_counter() - began
            _current, peak = tracemalloc.get_traced_memory()
            tracemalloc.stop()
            index = MarkerIndex(db, spatial=True)
            began = time.perf_counter()
            new = index.query(bounds, 512, 512, checks=[0])
            first = time.perf_counter() - began
            began = time.perf_counter()
            newer = index.query(bounds, 1024, 768, checks=[0])
            second = time.perf_counter() - began
            assert sum(m.count for m in old) == sum(m.count for m in new) == sum(m.count for m in newer) == n
            size = sum(os.path.getsize(os.path.join(tree.path, name)) for name in os.listdir(tree.path))
            print('10M packed random-order overview: scan %.3fs, prepare %.3fs, tree %.3fs, resize %.3fs; index %.2f MiB, prepare heap peak %.2f MiB; %d bins' %
                  (before, build, first, second, size / 2**20, peak / 2**20, len(new)))
        finally:
            db.close()


if __name__ == '__main__':
    if '--benchmark-10m' in sys.argv:
        benchmark_10m()
    else:
        unittest.main(verbosity=2)
