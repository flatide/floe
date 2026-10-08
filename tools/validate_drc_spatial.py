"""Persistent DRC spatial-index preparation and numeric query regression gate.

Independent packed fixtures cover postings, filtered node counts, review
changes, cache reuse/identity, cancellation and atomic cleanup. Canvas marker
rendering is tested separately by validate_drc_points.py.
"""
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import numpy as np

sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..'))
from floe import drc
from floe.drc_clusters import load_clusters
from floe.drc_markers import MarkerQueryCancelled
from floe.drc_spatial import SpatialRule, prepare_rule, rule_path, _codes
from floe.drc_marker_worker import export_members, import_members, SelectedMembers


BIN = os.path.join(os.path.dirname(__file__), '..', 'rust', 'target',
                   'release', 'floe-index')


class TrackedPack(drc.IcePack):
    def __init__(self, path):
        self.geometry_decodes = 0
        super().__init__(path)

    def _block(self, block):
        self.geometry_decodes += 1
        return super()._block(block)


class SpatialTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if not os.path.isfile(BIN):
            raise RuntimeError('build rust/target/release/floe-index first')
        cls.tmp = tempfile.TemporaryDirectory(prefix='floe-spatial-tests-')
        source = os.path.join(cls.tmp.name, 'results.db')
        packed = os.path.join(cls.tmp.name, 'results.tray')
        cls.grid_count, cls.other_count = 5000, 8
        with open(source, 'w') as stream:
            stream.write('MAIN 1000\nGRID\n5000 5000 0\n')
            # Deliberately non-spatial file order: leaves must preserve original
            # error IDs while covering every position exactly once.
            for ei in range(cls.grid_count):
                point = (ei * 7919) % cls.grid_count
                stream.write('p 1 1\n%d %d\n' %
                             ((point % 100) * 1000, (point // 100) * 1000))
            stream.write('OTHER\n8 8 0\n')
            for ei in range(cls.other_count):
                x, y = (ei - 4) * 1000, (ei % 3 - 1) * 1000
                stream.write('p 1 4\n%d %d\n%d %d\n%d %d\n%d %d\n' %
                             (x, y, x + 3, y, x + 3, y + 5, x, y + 5))
            stream.write('EMPTY\n0 0 0\n')
        subprocess.run([BIN, 'drc', source, packed], check=True,
                       capture_output=True, text=True)
        cls.pack = TrackedPack(packed)

    @classmethod
    def tearDownClass(cls):
        cls.pack.close()
        cls.tmp.cleanup()

    def setUp(self):
        self.changed = []
        self.pack.geometry_decodes = 0

    def tearDown(self):
        for ci, ei, old in reversed(self.changed):
            self.pack.set_status(ci, ei, old)
        self.assertEqual(self.pack.geometry_decodes, 0,
                         'spatial preparation decoded error geometry')

    def set_status(self, ci, ei, status):
        self.changed.append((ci, ei, self.pack.get_status(ci, ei)))
        self.pack.set_status(ci, ei, status)

    def cluster(self, body):
        path = os.path.join(self.tmp.name, 'selection.clusters')
        with open(path, 'w') as stream:
            stream.write('[GRID]\nchosen = ' + body + '\n')
        return load_clusters(path, self.pack).rules[0][0]

    def test_prepared_rule_reopens_without_population_scan(self):
        tree = prepare_rule(self.pack, 0)
        with patch('floe.drc_spatial._add', side_effect=AssertionError('rebuild')):
            reopened = prepare_rule(self.pack, 0)
            self.assertEqual(reopened.path, tree.path)
            self.assertEqual(int(reopened.nodes['count'][0]), self.grid_count)
            nodes, waived = reopened.summaries()
            self.assertIs(nodes, reopened.nodes)
            self.assertIsNone(waived)

    def test_postings_are_a_permutation_and_match_morton_leaf(self):
        tree = prepare_rule(self.pack, 0)
        self.assertEqual(sorted(tree.ids.tolist()), list(range(self.grid_count)))
        es = int(self.pack._dir_es[0])
        for leaf in np.flatnonzero(tree.nodes['count'][tree.leaf_start:]):
            node = tree.leaf_start + int(leaf)
            ids = tree.leaf_ids(node).astype(np.int64)
            self.assertEqual(len(ids), int(tree.nodes['count'][node]))
            self.assertTrue(np.all(np.diff(ids) > 0))
            qboxes = self.pack._qbox[es + ids]
            self.assertTrue(np.all(_codes(qboxes, tree.depth) == leaf))
            np.testing.assert_array_equal(tree.nodes['box'][node, :2], qboxes[:, :2].min(axis=0))
            np.testing.assert_array_equal(tree.nodes['box'][node, 2:], qboxes[:, 2:].max(axis=0))
        for level in range(tree.depth):
            first, child = (4 ** level - 1) // 3, (4 ** (level + 1) - 1) // 3
            count = 4 ** level
            np.testing.assert_array_equal(tree.nodes['count'][first:first + count],
                tree.nodes['count'][child:child + count * 4].reshape(count, 4).sum(axis=1))

    def test_filtered_tree_reused_and_same_count_status_swap_refreshed(self):
        members = self.cluster('1,3000,4001-5000')
        self.set_status(0, 0, drc.STATUS_WAIVED)
        tree = prepare_rule(self.pack, 0)
        nodes, waived = tree.summaries(members)
        self.assertEqual((int(nodes['count'][0]), int(waived[0])), (1002, 1))
        with patch('floe.drc_spatial._add', side_effect=AssertionError('rescan')):
            cached_nodes, cached_waived = tree.summaries(members)
        self.assertIs(cached_nodes, nodes)
        self.assertIs(cached_waived, waived)
        self.set_status(0, 0, drc.STATUS_NONE)
        self.set_status(0, 4999, drc.STATUS_WAIVED)
        refreshed_nodes, refreshed_waived = tree.summaries(members)
        self.assertIsNot(refreshed_nodes, nodes)
        self.assertTrue(np.any(refreshed_waived != waived))
        self.assertEqual(int(refreshed_waived[0]), 1)
        nodes, waived = tree.summaries(members, waived=True)
        self.assertEqual((int(nodes['count'][0]), int(waived[0])), (1, 1))
        self.assertEqual(int(nodes['rep'][0]), 4999)
        nodes, waived = tree.summaries(members, waived=False)
        self.assertEqual((int(nodes['count'][0]), int(waived[0])), (1001, 0))

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
        self.assertEqual(int(prepare_rule(self.pack, 1).nodes['count'][0]), self.other_count)

    def test_cancelled_filter_does_not_cache_a_partial_summary(self):
        tree = prepare_rule(self.pack, 0)
        members = self.cluster('1,3,4000-5000')
        with self.assertRaises(MarkerQueryCancelled):
            tree.summaries(members, cancelled=lambda: True)
        self.assertIsNone(tree._summary_key)
        nodes, waived = tree.summaries(members)
        self.assertEqual(int(nodes['count'][0]), 1003)
        self.assertEqual(int(waived[0]), 0)

    def test_bad_metadata_is_rebuilt(self):
        tree = prepare_rule(self.pack, 1)
        meta = os.path.join(tree.path, 'meta.json')
        for broken in ('{"version": 0}', 'null', '[]'):
            with self.subTest(manifest=broken):
                with open(meta, 'w') as stream:
                    stream.write(broken)
                with self.assertRaises(ValueError):
                    SpatialRule(tree.path, self.pack, 1)
                rebuilt = prepare_rule(self.pack, 1)
                self.assertEqual(int(rebuilt.nodes['count'][0]), self.other_count)

    def test_membership_descriptors_only_contain_scalars_and_paths(self):
        with tempfile.TemporaryDirectory() as directory:
            for member in (self.cluster('1,3,4000-5000'),
                           SelectedMembers([0, 2, 4999])):
                spec = export_members(member, directory, self.grid_count)
                self.assertNotIn('numpy', repr(spec))
                copied = import_members(spec)
                wanted = member.mask(0, self.grid_count)
                np.testing.assert_array_equal(copied.mask(0, self.grid_count), wanted)
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

    def test_failed_write_removes_staging_and_does_not_publish(self):
        path = rule_path(self.pack, 1)
        shutil.rmtree(path, ignore_errors=True)
        with patch('floe.drc_spatial.np.save', side_effect=PermissionError('read only')):
            with self.assertRaises(PermissionError):
                prepare_rule(self.pack, 1)
        self.assertFalse(os.path.exists(path))
        self.assertFalse(any(name.startswith('.spatial-build-')
                             for name in os.listdir(os.path.dirname(path))))
        self.assertEqual(int(prepare_rule(self.pack, 1).nodes['count'][0]), self.other_count)

    def test_empty_rule_has_no_postings_or_nonzero_nodes(self):
        tree = prepare_rule(self.pack, 2)
        self.assertEqual(len(tree.ids), 0)
        self.assertFalse(np.any(tree.nodes['count']))
        self.assertFalse(np.any(tree.offsets))
        self.assertEqual(len(tree.leaf_ids(tree.leaf_start)), 0)

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


if __name__ == '__main__':
    unittest.main(verbosity=2)
