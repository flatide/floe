"""Bounded bulk review writes, cancellation and failed-write consistency.

Uses only synthetic status sidecars inside temporary directories; no sample
pack, reviewer file or geometry is opened.
"""

import os
from pathlib import Path
import sys
import tempfile
import threading
import unittest
from unittest.mock import patch

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from floe import drc  # noqa: E402


class MaskMembers:
    """Selection contract that explicitly forbids enumerating all IDs."""

    def __init__(self, size, modulo=3):
        self.size = size
        self.modulo = modulo
        self.calls = []

    def __iter__(self):
        raise AssertionError("bulk operation enumerated selected IDs")

    def mask(self, start, count):
        self.calls.append((start, count))
        if count > drc._STATUS_WRITE_CHUNK:
            raise AssertionError("unbounded selection mask")
        return np.arange(start, start + count) % self.modulo == 0


class BulkStatusTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="floe-bulk-status-")
        self.addCleanup(self.temp.cleanup)
        self.size = 3 * drc._STATUS_WRITE_CHUNK + 17
        self.initial = (np.arange(self.size + 23) % 4).astype(np.uint8)
        self.path = Path(self.temp.name) / "status.waive"
        offset = 64
        counts = np.array([np.count_nonzero(self.initial[:self.size] == 1),
                           np.count_nonzero(self.initial[self.size:] == 1)],
                          dtype="<u4")
        self.path.write_bytes(bytes(offset) + self.initial.tobytes() + counts.tobytes())
        db = drc.IcePack.__new__(drc.IcePack)
        db._review = True
        db._map = None
        db._wfd = None
        db._status_lock = threading.RLock()
        db._status_cache_lock = threading.Lock()
        db._status_generation = 0
        db._waive_path = str(self.path)
        db.total = len(self.initial)
        db._ecnt = np.array([self.size, 23], dtype=np.int64)
        db._dir_es = np.array([0, self.size], dtype=np.int64)
        db._status_off = offset
        db._wcount_off = offset + db.total
        db._status = np.memmap(self.path, mode="r", dtype=np.uint8,
                               offset=offset, shape=(db.total,))
        db._wcount = np.memmap(self.path, mode="r", dtype="<u4",
                               offset=db._wcount_off, shape=(2,))
        db._wchunk = {}
        self.db = db
        self.addCleanup(db.close)

    def assert_consistent(self, expected, persisted=True):
        db = self.db
        np.testing.assert_array_equal(db._status, expected)
        for ci, (start, size) in enumerate(zip(db._dir_es, db._ecnt)):
            count = int(np.count_nonzero(expected[start:start + size] == 1))
            self.assertEqual(db.status_counts(ci), (count, size))
            chunks = db._wf_chunks(ci)
            wanted = [np.count_nonzero(expected[start + a:start + min(a + drc._STATUS_CHUNK, size)] == 1)
                      for a in range(0, int(size), drc._STATUS_CHUNK)]
            np.testing.assert_array_equal(chunks, wanted)
            if persisted:
                stored = np.frombuffer(self.path.read_bytes(), dtype="<u4", count=2,
                                       offset=db._wcount_off)
                self.assertEqual(int(stored[ci]), count)

    def test_sparse_bounded_writes_preserve_other_statuses_rules_and_cached_pages(self):
        members = MaskMembers(self.size)
        expected = self.initial.copy()
        chosen = np.arange(self.size) % 3 == 0
        changed = np.count_nonzero(expected[:self.size][chosen] != 1)
        expected[:self.size][chosen] = 1
        progress = []
        original_write = os.pwrite
        with patch.object(drc, "_STATUS_CHUNK", 8192):
            self.db._wf_chunks(0)
            with patch.object(os, "pwrite", wraps=original_write) as write:
                result = self.db.set_status_members(0, members, 1,
                    progress=lambda done, total: progress.append((done, total)))
            self.assertEqual(result, changed)
            self.assertLessEqual(write.call_count, 2 * len(members.calls))
            self.assertEqual(len(members.calls), 4)
            self.assert_consistent(expected)
            want = np.flatnonzero(expected[:self.size] == 1).tolist()
            self.assertEqual(self.db.status_page(0, True, 1000, 1000), want[1000:2000])
            self.assertEqual(self.db.status_rank(0, True, want[-1]), len(want) - 1)
        self.assertEqual(progress[-1], (self.size, self.size))

    def test_idempotent_edit_does_not_write_and_nonwaived_values_keep_counts(self):
        members = MaskMembers(self.size, modulo=5)
        self.db.set_status_members(0, members, 255)
        with patch.object(os, "pwrite", side_effect=AssertionError("unchanged data written")):
            self.assertEqual(self.db.set_status_members(0, members, 255), 0)
        expected = self.initial.copy()
        expected[:self.size:5] = 255
        self.assert_consistent(expected)

    def test_cancel_keeps_only_completed_chunks_and_consistent_counts(self):
        stop = threading.Event()
        progress = []
        def updated(done, total):
            progress.append((done, total))
            stop.set()
        with self.assertRaises(InterruptedError):
            self.db.set_status_members(0, None, 1, cancelled=stop.is_set,
                                       progress=updated)
        expected = self.initial.copy()
        expected[:drc._STATUS_WRITE_CHUNK] = 1
        self.assert_consistent(expected)
        self.assertEqual(progress, [(drc._STATUS_WRITE_CHUNK, self.size)])

    def test_initial_cancel_empty_selection_and_readonly_are_no_write(self):
        with patch.object(os, "pwrite", side_effect=AssertionError("unexpected write")):
            with self.assertRaises(InterruptedError):
                self.db.set_status_members(0, None, 1, cancelled=lambda: True)
            members = MaskMembers(self.size)
            members.total = 0
            self.assertEqual(self.db.set_status_members(0, members, 1), 0)
            self.assertEqual(members.calls, [])
            self.db._review = False
            with self.assertRaisesRegex(OSError, "read-only"):
                self.db.set_status_members(0, None, 1)

    def test_short_and_interrupted_writes_are_completed(self):
        original_write = os.pwrite
        calls = 0
        def short(fd, data, offset):
            nonlocal calls
            calls += 1
            if calls == 1:
                raise InterruptedError()
            return original_write(fd, data[:max(1, len(data) // 3)], offset)
        with patch.object(os, "pwrite", side_effect=short):
            self.db.set_status_members(1, None, 1)
        expected = self.initial.copy()
        expected[self.size:] = 1
        self.assert_consistent(expected)

    def test_partial_status_failure_rolls_back_current_chunk(self):
        original_write = os.pwrite
        failed = False
        def interrupted(fd, data, offset):
            nonlocal failed
            if not failed and offset >= self.db._status_off + drc._STATUS_WRITE_CHUNK \
                    and offset < self.db._wcount_off:
                failed = True
                original_write(fd, data[:23], offset)
                raise OSError("injected partial status write")
            return original_write(fd, data, offset)
        with patch.object(os, "pwrite", side_effect=interrupted):
            with self.assertRaisesRegex(OSError, "injected partial"):
                self.db.set_status_members(0, None, 1)
        expected = self.initial.copy()
        expected[:drc._STATUS_WRITE_CHUNK] = 1
        self.assert_consistent(expected)

    def test_partial_counter_failure_restores_status_and_counter(self):
        original_write = os.pwrite
        failed = False
        def interrupted(fd, data, offset):
            nonlocal failed
            if not failed and offset == self.db._wcount_off:
                failed = True
                original_write(fd, data[:1], offset)
                raise OSError("injected partial counter write")
            return original_write(fd, data, offset)
        with patch.object(os, "pwrite", side_effect=interrupted):
            with self.assertRaisesRegex(OSError, "injected partial"):
                self.db.set_status_members(0, None, 1)
        self.assert_consistent(self.initial)

    def test_persistent_io_failure_reports_counter_persistence_and_keeps_live_counts(self):
        original_write = os.pwrite
        writes = 0
        def broken(fd, data, offset):
            nonlocal writes
            writes += 1
            if writes == 1:
                original_write(fd, data[:23], offset)
            raise OSError("device unavailable")
        with patch.object(os, "pwrite", side_effect=broken):
            with self.assertRaisesRegex(OSError, "counter could not be persisted"):
                self.db.set_status_members(0, None, 1)
        expected = self.initial.copy()
        expected[:23] = 1
        self.assert_consistent(expected, persisted=False)
        # A subsequent successful update repairs persisted counts too.
        self.db.set_status_members(0, None, 1)
        expected[:self.size] = 1
        self.assert_consistent(expected)

    def test_bad_rule_size_and_mask_are_rejected_before_any_writes(self):
        with patch.object(os, "pwrite", side_effect=AssertionError("invalid edit wrote data")):
            with self.assertRaises(IndexError):
                self.db.set_status_members(-1, None, 1)
            with self.assertRaises(ValueError):
                self.db.set_status_members(0, MaskMembers(2), 1)
            members = MaskMembers(self.size)
            members.mask = lambda _start, _count: np.zeros(1, dtype=bool)
            with self.assertRaises(ValueError):
                self.db.set_status_members(0, members, 1)
            with self.assertRaises(IndexError):
                self.db.set_status(0, self.size, 1)

    def test_single_edits_share_failure_handling_and_reject_cross_rule_index(self):
        self.db._wf_chunks(0)
        self.db.set_status(0, 0, 1)
        self.db.set_status(0, 1, 2)
        expected = self.initial.copy()
        expected[0], expected[1] = 1, 2
        self.assert_consistent(expected)

    def test_note_reader_tolerates_concurrent_note_detach(self):
        self.db._note_of = {0: 7}
        self.db._notes = {}
        self.assertIsNone(self.db.get_note_gid(0))
        self.db._notes[7] = {"text": "review"}
        self.assertEqual(self.db.get_note_gid(0), "review")

    def test_cache_scan_started_before_bulk_write_cannot_publish_after_commit(self):
        captured, release = threading.Event(), threading.Event()
        failures, results = [], []
        class PausedScan(np.ndarray):
            def sum(array, *args, **kwargs):
                result = super().sum(*args, **kwargs)
                if threading.current_thread().name == "status-cache-reader" \
                        and not captured.is_set():
                    # The old count is already computed, but _wf_chunks has
                    # not published its array. Reproduce that exact race.
                    captured.set()
                    if not release.wait(3):
                        raise AssertionError("cache scan was not released")
                return result
        self.db._status = self.db._status.view(PausedScan)
        def read():
            try:
                results.append(self.db._wf_chunks(0))
            except Exception as exc:
                failures.append(exc)
        reader = threading.Thread(target=read, name="status-cache-reader")
        reader.start()
        try:
            self.assertTrue(captured.wait(3))
            self.db.set_status_members(0, None, 1)
            self.assertNotIn(0, self.db._wchunk)
        finally:
            release.set()
            reader.join(3)
        self.assertFalse(reader.is_alive())
        self.assertEqual(failures, [])
        self.assertEqual(int(results[0].sum()), np.count_nonzero(self.initial[:self.size] == 1))
        self.assertNotIn(0, self.db._wchunk, "late reader published pre-write counts")
        expected = self.initial.copy()
        expected[:self.size] = 1
        self.assert_consistent(expected)

    def test_cache_reader_during_blocked_writer_returns_without_wait_or_publication(self):
        entered, release, read_done = threading.Event(), threading.Event(), threading.Event()
        failures = []
        original_write = os.pwrite
        def blocked_write(fd, data, offset):
            written = original_write(fd, data, offset)
            if offset == self.db._status_off and not entered.is_set():
                entered.set()
                if not release.wait(3):
                    raise AssertionError("status writer was not released")
            return written
        def write():
            try:
                self.db.set_status_members(0, None, 1)
            except Exception as exc:
                failures.append(exc)
        def read():
            try:
                self.db._wf_chunks(0)
            except Exception as exc:
                failures.append(exc)
            finally:
                read_done.set()
        with patch.object(os, "pwrite", side_effect=blocked_write):
            writer = threading.Thread(target=write)
            reader = threading.Thread(target=read)
            writer.start()
            try:
                self.assertTrue(entered.wait(3))
                reader.start()
                self.assertTrue(read_done.wait(1), "cache read waited for full bulk-write lock")
                self.assertNotIn(0, self.db._wchunk,
                                 "in-flight write allowed unstable cache publication")
            finally:
                release.set()
                writer.join(3)
                if reader.ident is not None:
                    reader.join(3)
        self.assertFalse(writer.is_alive())
        self.assertFalse(reader.is_alive())
        self.assertEqual(failures, [])
        self.assertEqual(self.db._status_generation % 2, 0)
        expected = self.initial.copy()
        expected[:self.size] = 1
        self.assert_consistent(expected)


if __name__ == "__main__":
    unittest.main()
