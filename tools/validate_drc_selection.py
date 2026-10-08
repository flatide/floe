"""Uncapped DRC selection, bounded membership and async-query regressions."""

import gc
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest import mock

import numpy as np

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from floe import drc, drc_selection
from floe.drc_selection import (Selection, SelectionCancelled, SelectionProgress,
                                SelectionWorker, select_rect)

BIN = ROOT / 'rust' / 'target' / 'release' / 'floe-index'


class MembershipTests(unittest.TestCase):
    def test_sparse_ui_edit_does_not_allocate_rule_sized_bitmap(self):
        selection = Selection.from_indices(200_000_000, [199_999_999, 1, 1, 65536])
        self.assertEqual(selection.indices.nbytes, 24)
        self.assertFalse(hasattr(selection, 'bits'))
        self.assertEqual(selection.total, 3)
        self.assertEqual(selection.page(0, 1000), [1, 65536, 199999999])
        self.assertEqual(selection.rank(65536), 1)
        self.assertIsNone(selection.rank(12))
        self.assertTrue(65536 in selection)
        self.assertFalse(-1 in selection)
        self.assertEqual(selection[-1], 199999999)
        self.assertEqual(selection[:2], [1, 65536])

    def test_two_hundred_million_dense_members_use_25mb_and_bounded_pages(self):
        size = 200_000_003
        bits = np.full((size + 7) // 8, 255, dtype=np.uint8)
        bits[-1] &= (1 << (size % 8)) - 1
        bits.flags.writeable = False
        selection = Selection.from_packed(size, bits)
        self.assertEqual(selection.total, size)
        self.assertEqual(selection.bits.nbytes, 25_000_001)
        self.assertLess(selection._prefix.nbytes, 30_000)
        self.assertEqual(selection.page(size - 7, 1000), list(range(size - 7, size)))
        self.assertEqual(selection.rank(size - 1), size - 1)
        self.assertEqual(selection[65536], 65536)

    def test_bitmap_page_skips_empty_chunks_and_preserves_original_ids(self):
        size = 3 * drc_selection.CHUNK + 11
        ids = np.array([0, 7, 8, 65535, 2 * 65536, size - 1])
        raw = np.zeros((size + 7) // 8, dtype=np.uint8)
        np.bitwise_or.at(raw, ids // 8, (1 << (ids % 8)).astype(np.uint8))
        selection = Selection.from_packed(size, raw)
        raw[:] = 255
        self.assertEqual(selection.page(0, 99), ids.tolist())
        self.assertEqual(selection.page(2, 2), ids[2:4].tolist())
        self.assertEqual([selection.rank(i) for i in ids], list(range(len(ids))))
        np.testing.assert_array_equal(selection.mask(6, 5), [False, True, True, False, False])
        np.testing.assert_array_equal(selection.mask(-1, 3), [False, True, False])
        np.testing.assert_array_equal(selection.contains_many([-1, 0, 10, size-1, size]),
                                      [False, True, False, True, False])

    def test_all_combination_modes_preserve_inputs(self):
        a = Selection.from_indices(30, [0, 2, 7])
        b = Selection.from_indices(30, [2, 3, 25])
        dense = Selection.from_packed(30, np.packbits(a.mask(0, 30), bitorder='little'))
        expected = {'replace': [2, 3, 25], 'add': [0, 2, 3, 7, 25],
                    'toggle': [0, 3, 7, 25], 'remove': [0, 7], 'intersect': [2]}
        for mode, ids in expected.items():
            self.assertEqual(a.combine(b, mode).page(0, 100), ids)
            self.assertEqual(dense.combine(b, mode).page(0, 100), ids)
        self.assertEqual(a.page(0, 99), [0, 2, 7])
        self.assertFalse(Selection.empty(200_000_000))


class SelectionQueryTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix='floe-selection-tests-')
        path = Path(cls.tmp.name) / 'selection.db'
        cls.boxes = [(-1., 0., 2., 0.), (0., 0., 0., 0.), (100., 100., 100.001, 100.001)]
        cls.boxes.extend(((i % 128) / 10, (i // 128) / 10,
                          (i % 128) / 10 + .001, (i // 128) / 10 + .001)
                         for i in range(3, 8192))
        with open(path, 'w') as stream:
            stream.write('MAIN 1000\nCHECK\n8192 8192 0\n')
            for box in cls.boxes:
                coords = tuple(round(v * 1000) for v in box)
                stream.write('e 1 1\n%d %d %d %d\n' % coords)
            stream.write('EMPTY\n0 0 0\n')
        pack_path = Path(cls.tmp.name) / 'selection.tray'
        subprocess.run([str(BIN), 'drc', str(path), str(pack_path)], check=True, capture_output=True)
        cls.pack = drc.IcePack(str(pack_path))

    @classmethod
    def tearDownClass(cls):
        cls.pack.close()
        cls.tmp.cleanup()

    def tearDown(self):
        for ei in (0, 1, 2, 17, 2345):
            self.pack.set_status(0, ei, 0)

    def expected(self, bounds):
        x0, y0, x1, y1 = bounds
        return [i for i, (a, b, c, d) in enumerate(self.boxes)
                if a <= x1 and c >= x0 and b <= y1 and d >= y0]

    def test_contained_blocks_select_every_error_without_geometry_decode(self):
        with mock.patch.object(drc_selection, '_exact_boxes', side_effect=AssertionError('decode')):
            with mock.patch.object(drc.IcePack, '_block', side_effect=AssertionError('geometry objects')):
                result = select_rect(self.pack, 0, (-2, -2, 101, 101))
        self.assertEqual(result.total, 8192)
        self.assertEqual(result.page(8000, 1000), list(range(8000, 8192)))

    def test_boundary_query_uses_exact_bbox_intersection_not_marker_centers(self):
        bounds = (-.001, -.001, .001, .001)
        with mock.patch.object(drc.IcePack, '_block', side_effect=AssertionError('geometry objects')):
            result = select_rect(self.pack, 0, bounds)
        self.assertEqual(result.page(0, 9999), self.expected(bounds))
        self.assertTrue(0 in result, 'long error crosses the box despite its center being outside')
        self.assertFalse(128 in result, 'coarse qbox false positive escaped exact refinement')

    def test_arbitrary_rectangles_match_exact_original_geometry(self):
        for bounds in ((.2, .2, 2.301, 3.201), (12.7, 6.3, 12.701, 6.301),
                       (99.999, 99.999, 100.001, 100.001), (-4, -5, -3, -2)):
            with self.subTest(bounds=bounds):
                result = select_rect(self.pack, 0, bounds)
                self.assertEqual(result.page(0, 9999), self.expected(bounds))

    def test_random_varied_geometry_matches_existing_exact_query(self):
        """Independent packed-query oracle, including lattice false positives."""
        rng = np.random.default_rng(20261008)
        count = 1200
        shapes = []
        for i in range(count):
            x, y = (int(v) for v in rng.integers(-20000, 20001, size=2))
            w, h = (int(v) for v in rng.integers(0, 4001, size=2))
            if i % 6 == 0:
                kind, points = 'p', [(x, y)]
            elif i % 6 == 1:
                kind, points = 'e', [(x, y), (x + w, y)]
            elif i % 6 == 2:
                kind, points = 'e', [(x, y), (x, y + h)]
            elif i % 6 == 3:
                kind, points = 'p', [(x, y), (x + w, y), (x + w, y + h), (x, y + h)]
            elif i % 6 == 4:
                kind, points = 'e', [(x, y), (x + w, y), (x, y + h), (x + w, y + h)]
            else:
                kind, points = 'p', [(x, y), (x + w, y), (x + w, y + h // 2),
                                    (x + w // 2, y + h // 2), (x + w // 2, y + h), (x, y + h)]
            shapes.append((kind, points))
        # Far-away errors deliberately coarsen the shared qbox lattice.
        # Nearby negative/degenerate geometry must still refine exactly.
        shapes[0] = ('p', [(-1_000_000, -1_000_000)])
        shapes[1] = ('e', [(1_000_000, 1_000_000), (1_000_000, 1_000_000)])
        with tempfile.TemporaryDirectory(prefix='floe-select-varied-') as folder:
            source, packed = Path(folder) / 'varied.db', Path(folder) / 'varied.tray'
            with open(source, 'w') as stream:
                stream.write('MAIN 1000\nVARIED\n%d %d 0\n' % (count, count))
                for kind, points in shapes:
                    stream.write('%s 1 %d\n' % (kind, len(points) if kind == 'p' else len(points) // 2))
                    if kind == 'p':
                        stream.writelines('%d %d\n' % point for point in points)
                    else:
                        stream.writelines('%d %d %d %d\n' % (*points[j], *points[j+1])
                                          for j in range(0, len(points), 2))
            subprocess.run([str(BIN), 'drc', str(source), str(packed)], check=True, capture_output=True)
            db = drc.IcePack(str(packed))
            try:
                for ei in range(0, count, 5):
                    db.set_status(0, ei, drc.STATUS_WAIVED)
                member = Selection.from_indices(count, np.sort(rng.choice(count, 431, replace=False)))
                rectangles = []
                for _ in range(60):
                    x0, x1 = sorted(int(v) for v in rng.integers(-25000, 25001, size=2))
                    y0, y1 = sorted(int(v) for v in rng.integers(-25000, 25001, size=2))
                    rectangles.append((x0 / 1000, y0 / 1000, x1 / 1000, y1 / 1000))
                for i in range(2, 42):
                    points = shapes[i][1]
                    xs, ys = zip(*points)
                    # Exact point, horizontal/vertical and full-bbox boundaries.
                    if i % 3 == 0:
                        bounds = (min(xs), min(ys), min(xs), min(ys))
                    elif i % 3 == 1:
                        bounds = (min(xs), min(ys), max(xs), min(ys))
                    else:
                        bounds = (min(xs), min(ys), max(xs), max(ys))
                    rectangles.append(tuple(v / 1000 for v in bounds))
                for number, bounds in enumerate(rectangles):
                    waived = (None, False, True)[number % 3]
                    selected = member if number % 2 else None
                    with self.subTest(number=number, bounds=bounds, waived=waived):
                        expected = [ei for _ci, ei, _error in db.query_rect(
                            *bounds, cap=count, checks=[0], waived=waived,
                            members={0: selected} if selected is not None else None)]
                        result = select_rect(db, 0, bounds, selected, waived)
                        actual = []
                        for start in range(0, result.total, 37):
                            actual.extend(result.page(start, 37))
                        self.assertEqual(actual, expected)
            finally:
                db.close()

    def test_membership_and_review_filters_are_applied_before_combination(self):
        for ei in (1, 17, 2345):
            self.pack.set_status(0, ei, drc.STATUS_WAIVED)
        member = Selection.from_indices(8192, [0, 1, 2, 17, 2345])
        result = select_rect(self.pack, 0, None, membership=member, waived=True)
        self.assertEqual(result.page(0, 99), [1, 17, 2345])
        previous = Selection.from_indices(8192, [1, 2, 6000])
        result = select_rect(self.pack, 0, None, member, True, previous, 'toggle')
        self.assertEqual(result.page(0, 99), [2, 17, 2345, 6000])

    def test_none_bounds_prunes_saved_selection_without_geometry(self):
        member = Selection.from_indices(8192, [0, 1, 8191])
        self.pack.set_status(0, 1, 1)
        with mock.patch.object(drc_selection, '_packed_spatial_mask', side_effect=AssertionError('geometry')):
            result = select_rect(self.pack, 0, membership=member, waived=False)
        self.assertEqual(result.page(0, 99), [0, 8191])

    def test_plain_database_empty_rules_and_cancellation(self):
        check = drc.DrcCheck('plain')
        check.errors = [drc.DrcError('e', i+1, [(b[0], b[1]), (b[2], b[3])])
                        for i, b in enumerate(self.boxes[:10])]
        db = drc.DrcDb('', 'MAIN', 1000, [check])
        self.assertEqual(select_rect(db, 0, (-.001, -.001, .001, .001)).page(0, 100), [0, 1])
        self.assertFalse(select_rect(self.pack, 1))
        with self.assertRaises(SelectionCancelled):
            select_rect(self.pack, 0, cancelled=lambda: True)

    def test_progress_reports_numeric_counts_without_partial_selection(self):
        frames = []
        ticks = iter([0., .6])
        with mock.patch.object(drc_selection.time, 'monotonic', lambda: next(ticks)):
            result = select_rect(self.pack, 0, progress=frames.append)
        self.assertEqual(frames, [SelectionProgress(8192, 8192, 8192)])
        self.assertEqual(result.total, 8192)

    def wait_result(self, worker):
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            reply = worker.poll()
            if reply is not None and not isinstance(reply[1], SelectionProgress):
                self.assertIsNone(reply[2], reply[2])
                return reply
            time.sleep(.005)
        self.fail('selection worker did not finish')

    def test_process_result_transfers_only_mapped_membership_and_owns_its_files(self):
        from floe import drc_marker_worker
        with mock.patch.object(drc_marker_worker, 'LARGE_RULE', 1):
            worker = SelectionWorker()
            try:
                previous = Selection.from_indices(8192, [2, 4, 6000])
                member = Selection.from_indices(8192, [1, 2, 8191])
                worker.submit('first', self.pack, 0, None, member, previous=previous, mode='add')
                key, result, _error = self.wait_result(worker)
                self.assertEqual(key, 'first')
                self.assertIsInstance(result.bits, np.memmap)
                self.assertEqual(result.page(0, 99), [1, 2, 4, 6000, 8191])
                folder = result._owner.name
                self.assertTrue(os.path.exists(folder))
                # The next request can export the mapped bitmap directly.
                worker.submit('second', self.pack, 0, None, Selection.from_indices(8192, [2]),
                              previous=result, mode='remove')
                _key, second, _error = self.wait_result(worker)
                self.assertEqual(second.page(0, 99), [1, 4, 6000, 8191])
                self.assertTrue(os.path.exists(folder))
            finally:
                worker.close()
                worker._thread.join(timeout=5)
            del result
            gc.collect()
            self.assertFalse(os.path.exists(folder))

    def test_superseded_thread_query_cannot_publish_old_selection(self):
        started, release = threading.Event(), threading.Event()
        def query(db, ci, bounds, membership, waived, previous, mode, cancelled, progress):
            if bounds == (0, 0, 1, 1):
                started.set()
                release.wait(timeout=5)
                return Selection.from_indices(8192, [1])
            return Selection.from_indices(8192, [6000])
        with mock.patch.object(drc_selection, 'select_rect', query):
            worker = SelectionWorker()
            try:
                worker.submit('old', self.pack, 0, (0, 0, 1, 1))
                self.assertTrue(started.wait(timeout=5))
                worker.submit('new', self.pack, 0, (2, 2, 3, 3))
                release.set()
                key, result, _error = self.wait_result(worker)
                self.assertEqual(key, 'new')
                self.assertEqual(result.page(0, 99), [6000])
            finally:
                release.set()
                worker.close()
                worker._thread.join(timeout=5)


if __name__ == '__main__':
    unittest.main(verbosity=2)
