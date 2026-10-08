"""Exact-center cache and bounded point marker regression gate.

Usage: python tools/validate_drc_points.py [floe-index-binary]
"""

import os
import pickle
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest import mock
from types import SimpleNamespace

import numpy as np

sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..'))
from floe import drc, drc_points
from floe.drc_marker_style import circle_rgba
from floe.drc_marker_worker import (MarkerWorker, SelectedMembers,
                                    export_members, import_members)
from floe.drc_markers import MarkerIndex, MarkerQueryCancelled


BIN = os.path.join(os.path.dirname(__file__), '..', 'rust', 'target',
                   'release', 'floe-index')
if len(sys.argv) > 1 and not sys.argv[1].startswith('-'):
    BIN = sys.argv.pop(1)


class PointTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix='floe-points-')
        path = os.path.join(cls.tmp.name, 'points.db')
        cls.boxes = [(-11, -7, -2, -4), (-9, -9, -4, -2),
                     (-8, -8, -5, -3), (-6, -5, -6, -5),
                     (0, 0, 0, 0), (10, 10, 10, 10),
                     (-1000, 0, 2000, 0)]
        with open(path, 'w') as stream:
            stream.write('MAIN 1000\nONE\n7 7 0\n')
            for x0, y0, x1, y1 in cls.boxes:
                stream.write('e 1 1\n%d %d %d %d\n' % (x0, y0, x1, y1))
            stream.write('TWO\n1 1 0\np 1 1\n0 0\nEMPTY\n0 0 0\n')
        pack = os.path.join(cls.tmp.name, 'points.tray')
        subprocess.run([BIN, 'drc', path, pack], check=True, capture_output=True)
        cls.db = drc.IcePack(pack)

    @classmethod
    def tearDownClass(cls):
        cls.db.close()
        cls.tmp.cleanup()

    def setUp(self):
        self.cache = tempfile.TemporaryDirectory(prefix='floe-point-cache-')
        self.env = mock.patch.dict(os.environ, {'FLOE_DRC_ANALYSIS_ROOT': self.cache.name})
        self.env.start()
        self.index = MarkerIndex(self.db)
        for ci, check in enumerate(self.db.checks):
            for ei in range(len(check.errors)):
                self.db.set_status(ci, ei, 0)

    def tearDown(self):
        self.env.stop()
        self.cache.cleanup()

    def query(self, **kwargs):
        return self.index.query((-.01, -.01, .01, .01), 100, 100,
                                checks=[0], **kwargs)

    @staticmethod
    def fake_clock(step=.51):
        now = [-step]
        def monotonic():
            now[0] += step
            return now[0]
        return monotonic

    def test_integer_cache_preserves_negative_half_grid_centers(self):
        with mock.patch.object(drc_points, 'find_binary', return_value=None):
            rule = drc_points.prepare_rule(self.index, 0)
        expected = np.asarray([(b[0] + b[2], b[1] + b[3]) for b in self.boxes],
                              dtype=np.int64)
        np.testing.assert_array_equal(rule.centers, expected)
        self.assertEqual(rule.scale, 2000)
        self.assertEqual(tuple(rule.centers[0]), (-13, -11))
        self.assertEqual(rule.centers.dtype, np.dtype('<i8'))
        self.assertIsInstance(rule.centers, np.memmap)

    def test_native_and_python_centers_match(self):
        if drc_points.find_binary() is None:
            self.skipTest('native center protocol is not built')
        native = drc_points.prepare_rule(self.index, 0).centers.copy()
        with tempfile.TemporaryDirectory(prefix='floe-point-python-') as folder:
            with mock.patch.dict(os.environ, {'FLOE_DRC_ANALYSIS_ROOT': folder}):
                with mock.patch.object(drc_points, 'find_binary', return_value=None):
                    python = drc_points.prepare_rule(self.index, 0).centers.copy()
        np.testing.assert_array_equal(native, python)

    def test_exact_duplicates_and_red_priority_keep_one_representative(self):
        self.db.set_status(0, 0, drc.STATUS_WAIVED)
        self.db.set_status(0, 1, drc.STATUS_WAIVED)
        result = self.query()
        self.assertEqual(result.visible_count, 6)
        self.assertEqual(result.occupied_count, 4)
        self.assertEqual(result.error_ids[78, 18], 2)
        pixels = np.frombuffer(result.rgba, np.uint8).reshape(100, 100, 4)
        np.testing.assert_array_equal(pixels[78, 18], (255, 82, 82, 255))
        self.assertEqual(set(result.error_ids[result.error_ids >= 0]), {2, 3, 4, 5})

    def test_all_waived_duplicate_uses_first_and_current_review_is_read(self):
        before = self.query()
        self.assertEqual(before.error_ids[78, 18], 0)
        for ei in (0, 1, 2):
            self.db.set_status(0, ei, drc.STATUS_WAIVED)
        after = self.query()
        self.assertEqual(after.error_ids[78, 18], 0)
        pixel = np.frombuffer(after.rgba, np.uint8).reshape(100, 100, 4)[78, 18]
        np.testing.assert_array_equal(pixel, (0, 230, 118, 255))

    def test_selected_duplicates_are_gold_and_take_picking_priority(self):
        self.db.set_status(0, 1, drc.STATUS_WAIVED)
        result = self.query(selected={0: SelectedMembers([1])})
        self.assertEqual(result.error_ids[78, 18], 1,
                         'selected waived error must beat unselected unwaived')
        pixels = np.frombuffer(result.rgba, np.uint8).reshape(100, 100, 4)
        np.testing.assert_array_equal(pixels[78, 18], (255, 215, 0, 255))
        self.assertEqual((result.visible_count, result.occupied_count), (6, 4))
        result = self.query(selected={0: SelectedMembers([1, 2])})
        self.assertEqual(result.error_ids[78, 18], 2,
                         'unwaived wins within selected duplicates')
        result = self.query(selected={0: SelectedMembers([0, 1, 2])})
        self.assertEqual(result.error_ids[78, 18], 0,
                         'original order resolves equally selected statuses')

    def test_gold_selection_obeys_scope_review_and_multirule_filters(self):
        self.db.set_status(0, 1, drc.STATUS_WAIVED)
        selection = {0: SelectedMembers([1, 4, 6])}
        member = {0: SelectedMembers([0, 1, 2])}
        result = self.query(members=member, selected=selection, waived=False)
        self.assertEqual((result.visible_count, result.occupied_count), (2, 1))
        self.assertEqual(result.error_ids[78, 18], 0)
        pixels = np.frombuffer(result.rgba, np.uint8).reshape(100, 100, 4)
        np.testing.assert_array_equal(pixels[78, 18], (255, 82, 82, 255))
        result = self.query(members=member, selected=selection, waived=True)
        self.assertEqual((result.visible_count, result.occupied_count), (1, 1))
        self.assertEqual(result.error_ids[78, 18], 1)
        pixels = np.frombuffer(result.rgba, np.uint8).reshape(100, 100, 4)
        np.testing.assert_array_equal(pixels[78, 18], (255, 215, 0, 255))
        result = self.index.query((-.01, -.01, .01, .01), 100, 100,
                                  selected={1: SelectedMembers([0])})
        self.assertEqual((result.check_ids[50, 50], result.error_ids[50, 50]), (1, 0))
        self.assertEqual(result.visible_count, 7)

    def test_selected_late_chunk_replaces_red_without_mutating_prior_progress(self):
        drc_points.prepare_rule(self.index, 0)
        selection = {0: SelectedMembers([2])}
        frames = []
        with mock.patch.object(drc_points, 'CHUNK', 2):
            with mock.patch.object(drc_points.time, 'monotonic', self.fake_clock()):
                result = self.query(selected=selection, progress=frames.append)
        self.assertEqual(frames[0].error_ids[78, 18], 0)
        self.assertEqual(frames[1].error_ids[78, 18], 2)
        first = np.frombuffer(frames[0].rgba, np.uint8).reshape(100, 100, 4)
        second = np.frombuffer(frames[1].rgba, np.uint8).reshape(100, 100, 4)
        np.testing.assert_array_equal(first[78, 18], (255, 82, 82, 255))
        np.testing.assert_array_equal(second[78, 18], (255, 215, 0, 255))
        self.assertEqual(result.rgba, self.query(selected=selection).rgba)
        self.assertEqual((result.processed_count, result.visible_count), (7, 6))

    def test_large_packed_selection_is_lazy_and_exports_bits_and_intersection(self):
        class PackedSelection:
            size = 10_000_003
            def __init__(self):
                self.bits = np.zeros((self.size + 7) // 8, dtype=np.uint8)
                self.ids = np.array([0, 7, 8, 6000, 9_999_999])
                np.bitwise_or.at(self.bits, self.ids // 8,
                                 (1 << (self.ids % 8)).astype(np.uint8))
                self.bits.flags.writeable = False
            def __iter__(self):
                raise AssertionError('large selection enumerated')
            def mask(self, start, count):
                ids = np.arange(start, start + count)
                out = np.zeros(count, dtype=bool)
                valid = (ids >= 0) & (ids < self.size)
                ids = ids[valid]
                out[valid] = ((self.bits[ids // 8] >> (ids % 8)) & 1) != 0
                return out
        source = PackedSelection()
        selected = SelectedMembers(source, SelectedMembers([8, 6000, 9_999_999]))
        self.assertIs(selected._source, source)
        self.assertFalse(hasattr(selected, 'indices'))
        with tempfile.TemporaryDirectory() as directory:
            spec = export_members(selected, directory, source.size)
            self.assertEqual(spec['kind'], 'intersection')
            self.assertEqual(spec['parts'][0]['kind'], 'packed-bits')
            copied = import_members(spec)
            for start, count in ((-2, 20), (5997, 12), (9_999_995, 15)):
                np.testing.assert_array_equal(copied.mask(start, count), selected.mask(start, count))
            ids = np.array([-1, 0, 7, 8, 6000, 9_999_999, source.size])
            np.testing.assert_array_equal(copied.contains_many(ids),
                                          [False, False, False, True, True, True, False])
            self.assertLess(sum(os.path.getsize(os.path.join(directory, f))
                                for f in os.listdir(directory)), len(source.bits) + 1024)

    def test_marker_process_transfers_lazy_selected_memberships(self):
        import time
        worker = MarkerWorker()
        selection = SelectedMembers(SelectedMembers([1, 2]), SelectedMembers([1]))
        query = dict(bounds_um=(-.01, -.01, .01, .01), width_px=100, height_px=100,
                     checks=[0], selected={0: selection})
        self.db.set_status(0, 1, drc.STATUS_WAIVED)
        try:
            with mock.patch('floe.drc_marker_worker.LARGE_RULE', 0):
                worker.submit('selected', self.db, query)
                deadline = time.monotonic() + 10
                reply = None
                while time.monotonic() < deadline and reply is None:
                    reply = worker.poll()
                    time.sleep(.01)
            self.assertIsNotNone(reply)
            self.assertIsNone(reply[2])
            self.assertEqual(reply[1].error_ids[78, 18], 1)
            self.assertEqual(reply[1].rgba, self.query(selected={0: selection}).rgba)
        finally:
            worker.close()
            worker._thread.join(timeout=5)

    def test_scope_filters_original_rule_ids_and_waived_status(self):
        member = SelectedMembers([1, 3, 5, 6])
        self.db.set_status(0, 1, drc.STATUS_WAIVED)
        result = self.query(members={0: member}, waived=False)
        self.assertEqual(result.visible_count, 2)
        self.assertEqual(set(result.error_ids[result.error_ids >= 0]), {3, 5})
        waived = self.query(members={0: member}, waived=True)
        self.assertEqual(waived.visible_count, 1)
        self.assertEqual(set(waived.error_ids[waived.error_ids >= 0]), {1})

    def test_multirule_priority_and_deterministic_original_ids(self):
        self.db.set_status(0, 4, drc.STATUS_WAIVED)
        result = self.index.query((-.01, -.01, .01, .01), 100, 100,
                                  checks=[1, 0, 1, -1, 99, 2])
        self.assertEqual(result.visible_count, 7)
        self.assertEqual(result.occupied_count, 4)
        self.assertEqual((result.check_ids[50, 50], result.error_ids[50, 50]), (1, 0))
        self.db.set_status(0, 4, 0)
        result = self.index.query((-.01, -.01, .01, .01), 100, 100)
        self.assertEqual((result.check_ids[50, 50], result.error_ids[50, 50]), (0, 4))

    def test_closed_boundaries_and_outside_center_is_not_dragged_to_edge(self):
        result = self.query()
        self.assertEqual(result.error_ids[0, 99], 5)
        self.assertNotIn(6, result.error_ids)
        self.assertEqual(result.visible_count, 6)

    def test_subpixel_locations_separate_when_zoomed(self):
        member = SelectedMembers([0, 3])
        coarse = self.index.query((-1, -1, 1, 1), 100, 100, checks=[0],
                                  members={0: member})
        fine = self.query(members={0: member})
        self.assertEqual(coarse.visible_count, 2)
        self.assertEqual(coarse.occupied_count, 1)
        self.assertEqual(fine.occupied_count, 2)

    def test_raster_uses_solid_antialiased_circle_without_per_error_sprite(self):
        result = self.query(members={0: SelectedMembers([4])})
        pixels = np.frombuffer(result.rgba, np.uint8).reshape(100, 100, 4)
        expected = np.frombuffer(circle_rgba(2, drc_points.RED, solid=True),
                                 np.uint8).reshape(5, 5, 4)
        np.testing.assert_array_equal(pixels[48:53, 48:53], expected)
        self.assertEqual(np.count_nonzero(pixels[:, :, 3]), np.count_nonzero(expected[:, :, 3]))
        self.assertEqual(result.error_ids.shape, (100, 100))
        self.assertEqual(result.check_ids.dtype, np.dtype('int32'))
        copied = pickle.loads(pickle.dumps(result))
        self.assertEqual(copied.rgba, result.rgba)
        np.testing.assert_array_equal(copied.error_ids, result.error_ids)

    def test_persistent_cache_reuse_and_invalid_cache_replacement(self):
        with mock.patch.object(drc_points, 'find_binary', return_value=None):
            rule = drc_points.prepare_rule(self.index, 0)
        path = rule.path
        del rule
        with mock.patch.object(drc_points, '_decode_centers', side_effect=AssertionError('decoded again')):
            with mock.patch.object(drc_points, '_native', side_effect=AssertionError('native repeated')):
                self.query()
        with open(os.path.join(path, 'centers.bin'), 'wb') as stream:
            stream.write(b'broken')
        with mock.patch.object(drc_points, 'find_binary', return_value=None):
            result = self.query()
        self.assertEqual(result.visible_count, 6)
        self.assertEqual(os.path.getsize(os.path.join(path, 'centers.bin')), 7 * 16)

    def test_cancelled_build_does_not_publish_partial_cache(self):
        calls = [0]
        def cancelled():
            calls[0] += 1
            return calls[0] > 3
        with mock.patch.object(drc_points, 'find_binary', return_value=None):
            with self.assertRaises(MarkerQueryCancelled):
                self.query(cancelled=cancelled)
        path = drc_points.rule_path(self.db, 0)
        self.assertFalse(os.path.exists(path))
        if os.path.isdir(os.path.dirname(path)):
            self.assertEqual(os.listdir(os.path.dirname(path)), [])

    def test_cancellation_after_cached_query_starts_returns_no_partial_raster(self):
        self.query()
        calls = [0]
        def cancelled():
            calls[0] += 1
            return calls[0] > 5
        with self.assertRaises(MarkerQueryCancelled):
            self.query(cancelled=cancelled)

    def test_readonly_cache_falls_back_to_exact_chunk_decode(self):
        with mock.patch.object(drc_points, 'prepare_rule', side_effect=PermissionError()):
            result = self.query()
        self.assertEqual(result.visible_count, 6)
        self.assertEqual(result.occupied_count, 4)

    def test_inmemory_database_uses_point_markers_by_default(self):
        check = drc.DrcCheck('INMEMORY')
        check.errors = [drc.DrcError('p', 1, [(-.0065, -.0055)]),
                        drc.DrcError('p', 2, [(-.0065, -.0055)])]
        index = MarkerIndex(drc.DrcDb('', 'MAIN', 1000, [check]))
        result = index.query((-.01, -.01, .01, .01), 100, 100)
        self.assertIsInstance(result, drc_points.PointMarkers)
        self.assertEqual(result.visible_count, 2)
        self.assertEqual(result.occupied_count, 1)

    def test_empty_rule_and_small_viewport(self):
        empty = self.index.query((0, 0, 1, 1), 1, 1, checks=[2])
        self.assertEqual(empty.visible_count, 0)
        self.assertEqual(empty.rgba, b'\0\0\0\0')
        point = self.index.query((0, 0, 1, 1), 1, 1, checks=[1])
        self.assertEqual(point.error_ids[0, 0], 0)
        self.assertEqual(point.rgba, bytes((255, 82, 82, 255)))

    def test_red_priority_survives_chunk_boundaries(self):
        drc_points.prepare_rule(self.index, 0)
        self.db.set_status(0, 0, drc.STATUS_WAIVED)
        self.db.set_status(0, 1, drc.STATUS_WAIVED)
        with mock.patch.object(drc_points, 'CHUNK', 2):
            result = self.query()
        self.assertEqual(result.error_ids[78, 18], 2)
        self.assertEqual(result.visible_count, 6)

    def test_changed_pack_identity_is_rejected(self):
        path = os.path.join(self.cache.name, 'changed.tray')
        shutil.copy2(self.db.path, path)
        db = drc.IcePack(path, review=False)
        try:
            stat = os.stat(path)
            os.utime(path, ns=(stat.st_atime_ns, stat.st_mtime_ns + 1000000))
            with self.assertRaisesRegex(ValueError, 'pack changed'):
                drc_points.prepare_rule(MarkerIndex(db), 0)
        finally:
            db.close()

    def test_forced_worker_stop_terminates_native_child(self):
        child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(60)'])
        worker = MarkerWorker()
        try:
            worker._process_native_pid = SimpleNamespace(value=child.pid)
            worker._stop_process()
            self.assertLess(child.wait(timeout=5), 0)
            self.assertIsNone(worker._process_native_pid)
        finally:
            worker.close()
            worker._thread.join(timeout=5)
            if child.poll() is None:
                child.terminate()
                child.wait(timeout=5)

    def test_progress_is_cumulative_and_snapshots_are_independent(self):
        drc_points.prepare_rule(self.index, 0)
        self.db.set_status(0, 0, drc.STATUS_WAIVED)
        self.db.set_status(0, 1, drc.STATUS_WAIVED)
        frames, frozen = [], []
        def progress(frame):
            frames.append(frame)
            frozen.append((frame.rgba, frame.error_ids.copy(), frame.check_ids.copy()))
        with mock.patch.object(drc_points, 'CHUNK', 2):
            with mock.patch.object(drc_points.time, 'monotonic', self.fake_clock()):
                result = self.query(progress=progress)
        self.assertEqual([f.processed_count for f in frames], [2, 4, 6, 7])
        self.assertEqual([f.visible_count for f in frames], [2, 4, 6, 6])
        self.assertEqual([f.occupied_count for f in frames], [1, 2, 4, 4])
        self.assertEqual(frames[0].error_ids[78, 18], 0)
        self.assertEqual(frames[1].error_ids[78, 18], 2)
        self.assertTrue(all(f.total_count == 7 for f in frames))
        for frame, (rgba, errors, checks) in zip(frames, frozen):
            self.assertEqual(frame.rgba, rgba)
            np.testing.assert_array_equal(frame.error_ids, errors)
            np.testing.assert_array_equal(frame.check_ids, checks)
            self.assertFalse(frame.error_ids.flags.writeable)
        final = self.query()
        self.assertEqual((result.processed_count, result.total_count), (7, 7))
        self.assertEqual(result.rgba, final.rgba)
        np.testing.assert_array_equal(result.error_ids, final.error_ids)

    def test_progress_interval_skips_queries_faster_than_half_second(self):
        drc_points.prepare_rule(self.index, 0)
        frames = []
        with mock.patch.object(drc_points, 'CHUNK', 2):
            with mock.patch.object(drc_points.time, 'monotonic', self.fake_clock(.01)):
                result = self.query(progress=frames.append)
        self.assertEqual(frames, [])
        self.assertEqual(result.processed_count, 7)

    def test_progress_interval_includes_snapshot_and_callback_cost(self):
        drc_points.prepare_rule(self.index, 0)
        now, events = [0], []
        status = self.index._status
        def read_status(ci, start, count):
            now[0] += 500 if start == 0 else 300
            return status(ci, start, count)
        def progress(frame):
            events.append((now[0], frame.processed_count))
            now[0] += 200  # Simulated raster/IPC work belongs within the tick.
        with mock.patch.object(drc_points, 'CHUNK', 2):
            with mock.patch.object(drc_points.time, 'monotonic', lambda: now[0] / 1000):
                with mock.patch.object(self.index, '_status', read_status):
                    self.query(progress=progress)
        self.assertEqual(events, [(500, 2), (1000, 4), (1500, 6), (2000, 7)])

    def test_progress_counts_scanned_errors_before_membership_and_view_filter(self):
        drc_points.prepare_rule(self.index, 0)
        frames = []
        with mock.patch.object(drc_points, 'CHUNK', 2):
            with mock.patch.object(drc_points.time, 'monotonic', self.fake_clock()):
                result = self.query(members={0: SelectedMembers([5, 6])}, progress=frames.append)
        self.assertEqual([f.processed_count for f in frames], [2, 4, 6, 7])
        self.assertEqual([f.visible_count for f in frames], [0, 0, 1, 1])
        self.assertEqual(result.visible_count, 1)

    def test_python_cold_build_emits_real_data_before_cache_publication(self):
        frames = []
        path = drc_points.rule_path(self.db, 0)
        def progress(frame):
            self.assertFalse(os.path.exists(path))
            frames.append(frame)
        with mock.patch.object(drc_points, 'find_binary', return_value=None):
            with mock.patch.object(drc_points.time, 'monotonic', self.fake_clock()):
                result = self.query(progress=progress)
        self.assertGreater(len(frames), 0)
        self.assertEqual(frames[0].visible_count, 6)
        self.assertEqual(result.processed_count, 7)
        self.assertEqual(result.visible_count, 6, 'new cache was scanned twice')
        self.assertTrue(os.path.isdir(path))

    def fake_native_process(self, command, **kwargs):
        """Complete tasks out of order; the remaining preallocated rows are zero."""
        path = command[command.index('--out') + 1]
        ready = command[command.index('--ready') + 1]
        expected = np.asarray([(b[0] + b[2], b[1] + b[3]) for b in self.boxes], '<i8')
        with open(path, 'wb') as stream:
            stream.truncate(len(expected) * 16)
        with open(ready, 'wb') as stream:
            stream.write(bytes(4))
        class Process:
            pid = 123456789
            returncode = None
            stage = 0
            terminated = False
            def poll(self):
                if self.returncode is not None:
                    return self.returncode
                tasks = ((2,), (0,), (), (1, 3))[self.stage]
                with open(path, 'r+b') as stream, open(ready, 'r+b') as flags:
                    for task in tasks:
                        start = task * 2
                        stream.seek(start * 16)
                        stream.write(expected[start:start + 2].tobytes())
                        stream.flush()
                        flags.seek(task)
                        flags.write(b'\1')
                self.stage += 1
                if self.stage == 4:
                    self.returncode = 0
                return self.returncode
            def terminate(self):
                self.terminated = True
                self.returncode = -15
            def kill(self):
                self.returncode = -9
            def wait(self, timeout=None):
                return self.returncode
        self.native_process = Process()
        return self.native_process

    def test_native_ready_flags_emit_only_completed_chunks_and_do_not_rescan(self):
        frames = []
        path = drc_points.rule_path(self.db, 0)
        def progress(frame):
            self.assertFalse(os.path.exists(path))
            frames.append(frame)
        with mock.patch.object(drc_points, 'find_binary', return_value='fake-index'):
            with mock.patch.object(drc_points.subprocess, 'Popen', self.fake_native_process):
                with mock.patch.object(drc_points, 'NATIVE_TASK', 2):
                    with mock.patch.object(drc_points.time, 'monotonic', self.fake_clock()):
                        with mock.patch.object(drc_points.time, 'sleep'):
                            result = self.query(progress=progress)
        self.assertEqual([f.processed_count for f in frames], [2, 4, 6, 7])
        self.assertEqual(set(frames[0].error_ids[frames[0].error_ids >= 0]), {4, 5})
        self.assertEqual(result.processed_count, 7)
        self.assertEqual(result.visible_count, 6)
        self.assertEqual(result.rgba, self.query().rgba)
        self.assertFalse(os.path.exists(os.path.join(path, 'centers.bin.ready')))

    def test_cancel_after_native_partial_cleans_unpublished_cache(self):
        frames = []
        def progress(frame):
            frames.append(frame)
        with mock.patch.object(drc_points, 'find_binary', return_value='fake-index'):
            with mock.patch.object(drc_points.subprocess, 'Popen', self.fake_native_process):
                with mock.patch.object(drc_points, 'NATIVE_TASK', 2):
                    with mock.patch.object(drc_points.time, 'monotonic', self.fake_clock()):
                        with mock.patch.object(drc_points.time, 'sleep'):
                            with self.assertRaises(MarkerQueryCancelled):
                                self.query(progress=progress, cancelled=lambda: bool(frames))
        self.assertEqual(len(frames), 1)
        self.assertEqual(frames[0].processed_count, 2)
        self.assertTrue(self.native_process.terminated)
        path = drc_points.rule_path(self.db, 0)
        self.assertFalse(os.path.exists(path))
        self.assertEqual(os.listdir(os.path.dirname(path)), [])

    def test_real_native_cold_build_stream_matches_final_cache(self):
        if drc_points.find_binary() is None:
            self.skipTest('native center protocol is not built')
        frames = []
        with mock.patch.object(drc_points.time, 'monotonic', self.fake_clock()):
            result = self.query(progress=frames.append)
        self.assertGreater(len(frames), 0)
        self.assertEqual(result.visible_count, 6)
        self.assertEqual(result.processed_count, 7)
        self.assertEqual(result.rgba, self.query().rgba)


if __name__ == '__main__':
    unittest.main(verbosity=2)
