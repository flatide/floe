"""Persistent DRC visit/not-required flags and bounded review statistics.

Every pack and reviewer sidecar is synthetic and stays in a temporary folder.
Run after building floe-index: python tools/validate_drc_review.py
"""

import os
from pathlib import Path
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

import numpy as np

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from floe import drc  # noqa: E402
from floe import drc_review as review  # noqa: E402
from floe.drc_selection import Selection  # noqa: E402

BIN = ROOT / "rust" / "target" / "release" / "floe-index"


def expected_counts(total, visited=0, not_required=0, both=0):
    return dict(total=total, visited=visited, not_required=not_required,
                visited_not_required=both,
                pending=total - visited - not_required + both)


@unittest.skipUnless(BIN.is_file(), "build rust/target/release/floe-index first")
class PackedReviewTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.fixture = tempfile.TemporaryDirectory(prefix="floe-review-fixture-")
        source = Path(cls.fixture.name) / "synthetic.db"
        cls.packed = Path(cls.fixture.name) / "synthetic.tray"
        with source.open("w", encoding="utf-8") as stream:
            stream.write("REVIEW_TEST 1000\n")
            for name, count in (("FIRST", 129), ("SECOND", 5), ("EMPTY", 0)):
                stream.write("%s\n%d %d 0\n" % (name, count, count))
                for ei in range(count):
                    stream.write("e %d 1\n%d 0 %d 10\n" %
                                 (ei + 1, ei * 10, ei * 10 + 5))
        subprocess.run([str(BIN), "drc", str(source), str(cls.packed)],
                       check=True, capture_output=True)

    @classmethod
    def tearDownClass(cls):
        cls.fixture.cleanup()

    def setUp(self):
        folder = tempfile.TemporaryDirectory(prefix="floe-review-test-")
        self.addCleanup(folder.cleanup)
        self.folder = Path(folder.name)
        self.path = self.folder / "review.tray"
        shutil.copyfile(self.packed, self.path)
        self.db = self.open()

    def open(self, **kwargs):
        db = drc.IcePack(str(self.path), **kwargs)
        self.addCleanup(db.close)
        return db

    def test_flags_counters_and_bulk_overlap_keep_pending_union_correct(self):
        db = self.db
        with patch.object(drc.IcePack, "_block", side_effect=AssertionError("geometry decoded")):
            self.assertEqual(db.review_counts(), expected_counts(134))
            self.assertEqual(db.review_counts(0), expected_counts(129))
            self.assertEqual(db.review_counts(2), expected_counts(0))
            db.mark_visited(0, 0)
            db.mark_visited(0, 2)
            db.mark_visited(1, 4)
            selected = Selection.from_indices(129, [1, 2, 2, 128])
            self.assertEqual(db.set_review_members(0, selected, True), 3)
            self.assertEqual(db.review_flags(0, 0), 1)
            self.assertEqual(db.review_flags(0, 1), 2)
            self.assertEqual(db.review_flags(0, 2), 3)
            self.assertEqual(db.review_flags(0, 127), 0)
            self.assertEqual(db.review_flags(0, 128), 2)
            self.assertEqual(db.review_counts(0), expected_counts(129, 2, 3, 1))
            self.assertEqual(db.review_counts(1), expected_counts(5, 1))
            self.assertEqual(db.review_counts(), expected_counts(134, 3, 3, 1))

    def test_constructor_path_failure_is_backed_off_without_losing_fallback_history(self):
        db = self.db
        fallback = self.folder / "durable-user-data" / "review.sqlite3"
        saved = review.ReviewStore(str(self.path), (129, 5, 0),
                                  db._review_fingerprint, path=fallback)
        self.addCleanup(saved.close)
        saved.mark_visited(0, 7)
        saved.close()
        exists = review._exists

        def denied_fallback(path):
            if os.fspath(path) == str(fallback):
                raise PermissionError("review fallback path access denied")
            return exists(path)

        with patch.dict(os.environ, {"FLOE_DRC_REVIEW_ROOT": ""}), \
                patch.object(review, "_fallback_path", return_value=str(fallback)), \
                patch.object(review.time, "monotonic", return_value=100.0) as clock:
            with patch.object(review, "_exists", side_effect=denied_fallback) as probe:
                for ei in range(1000):
                    with self.assertRaisesRegex(PermissionError, "fallback path"):
                        db.review_flags(0, ei % 129)
                with self.assertRaises(PermissionError):
                    db.review_counts()
                with self.assertRaises(PermissionError):
                    _ = db.review_path
                self.assertEqual(probe.call_count, 2,
                                 "constructor retried filesystem discovery per row")
                self.assertIsNone(db._review_store)
            clock.return_value = 102.0
            self.assertEqual(db.review_flags(0, 7), review.VISITED)
            self.assertEqual(db.review_counts(), expected_counts(134, 1))
            self.assertEqual(db.review_path, str(fallback))

    def test_idempotent_visit_set_and_unset_preserve_visited_bit(self):
        db = self.db
        db.mark_visited(0, 7)
        db.mark_visited(0, 7)
        selected = Selection.from_indices(129, [7, 8])
        self.assertEqual(db.set_review_members(0, selected, True), 2)
        self.assertEqual(db.set_review_members(0, selected, True), 0)
        self.assertEqual(db.set_review_members(0, selected, False), 2)
        self.assertEqual(db.set_review_members(0, selected, False), 0)
        self.assertEqual(db.review_flags(0, 7), 1)
        self.assertEqual(db.review_flags(0, 8), 0)
        self.assertEqual(db.review_counts(), expected_counts(134, 1))

    def test_reopen_preserves_review_and_never_changes_pack_or_waives(self):
        db = self.db
        initial_pack = self.path.read_bytes()
        db.set_status(0, 7, drc.STATUS_WAIVED)
        waive_path = Path(db._waive_path)
        initial_waives = waive_path.read_bytes()
        db.mark_visited(0, 128)
        db.set_review_members(1, Selection.from_indices(5, [0, 4]), True)
        self.assertEqual(waive_path.read_bytes(), initial_waives)
        self.assertEqual(self.path.read_bytes(), initial_pack)
        db.close()
        reopened = self.open()
        self.assertEqual(reopened.review_flags(0, 128), 1)
        self.assertEqual(reopened.review_flags(1, 0), 2)
        self.assertEqual(reopened.review_counts(), expected_counts(134, 1, 2))
        self.assertEqual(reopened.get_status(0, 7), drc.STATUS_WAIVED)
        reopened.set_status(0, 128, drc.STATUS_WAIVED)
        self.assertEqual(reopened.review_flags(0, 128), 1)
        self.assertEqual(reopened.review_counts(), expected_counts(134, 1, 2))

    def test_reviewers_have_independent_flags_and_statistics(self):
        self.db.close()
        with patch.object(drc, "_waive_user", return_value="review-alice"):
            alice = self.open()
            alice.mark_visited(0, 7)
            alice.set_review_members(1, Selection.from_indices(5, [2]), True)
            alice.close()
        with patch.object(drc, "_waive_user", return_value="review-bob"):
            bob = self.open()
            self.assertEqual(bob.review_counts(), expected_counts(134))
            self.assertEqual(bob.review_flags(0, 7), 0)
            bob.mark_visited(0, 8)
            bob.close()
        with patch.object(drc, "_waive_user", return_value="review-alice"):
            alice = self.open()
            self.assertEqual(alice.review_flags(0, 7), 1)
            self.assertEqual(alice.review_flags(0, 8), 0)
            self.assertEqual(alice.review_counts(), expected_counts(134, 1, 1))

    def test_lazy_review_store_retains_reviewer_captured_when_pack_opened(self):
        with patch.object(drc, "_waive_user", return_value="captured-alice"):
            alice = self.open()
        with patch.object(drc, "_waive_user", return_value="captured-bob"):
            bob = self.open()
            alice.mark_visited(0, 7)
            alice.set_status(0, 7, drc.STATUS_WAIVED)
            self.assertEqual(alice._reviewer, "captured-alice")
            self.assertIn("captured-alice", alice.review_path)
            self.assertEqual(bob.review_flags(0, 7), 0)
            self.assertEqual(bob.get_status(0, 7), drc.STATUS_NONE)

    def test_missing_personal_waive_storage_keeps_shared_pack_read_only(self):
        original = self.path.read_bytes()
        with patch.object(drc.IcePack, "_ensure_waive_autosave",
                          side_effect=PermissionError("reviewer storage denied")):
            readonly = self.open()
        self.assertEqual(readonly.get_status(0, 7), drc.STATUS_NONE)
        with self.assertRaisesRegex(OSError, "shared pack is read-only"):
            readonly.set_status(0, 7, drc.STATUS_WAIVED)
        self.assertEqual(self.path.read_bytes(), original)

    def test_empty_rule_and_empty_selection_are_noops(self):
        db = self.db
        self.assertEqual(db.set_review_members(2, None, True), 0)
        self.assertEqual(db.set_review_members(0, Selection.empty(129), True), 0)
        self.assertEqual(db.review_counts(2), expected_counts(0))
        self.assertEqual(db.review_counts(), expected_counts(134))

    def test_dense_packed_membership_preserves_unselected_error_flags(self):
        db = self.db
        ids = [0, 7, 8, 63, 64, 127, 128]
        mask = np.zeros(129, dtype=bool)
        mask[ids] = True
        selected = Selection.from_packed(129, np.packbits(mask, bitorder="little"))
        db.mark_visited(0, 64)
        self.assertEqual(db.set_review_members(0, selected, True), len(ids))
        self.assertEqual([db.review_flags(0, ei) for ei in range(129)],
                         [(2 if ei in ids else 0) | (1 if ei == 64 else 0)
                          for ei in range(129)])
        self.assertEqual(db.review_counts(0), expected_counts(129, 1, 7, 1))

    def test_invalid_rule_indices_error_indices_and_membership_size_do_not_mutate(self):
        db = self.db
        for ci, ei in ((-1, 0), (3, 0), (0, -1), (0, 129), (2, 0)):
            with self.subTest(ci=ci, ei=ei):
                with self.assertRaises(IndexError):
                    db.mark_visited(ci, ei)
                with self.assertRaises(IndexError):
                    db.review_flags(ci, ei)
        with self.assertRaises(ValueError):
            db.set_review_members(0, Selection.from_indices(5, [0]), True)
        self.assertEqual(db.review_counts(), expected_counts(134))

    def test_open_analysis_view_never_opens_or_changes_review_sidecar(self):
        self.db.mark_visited(0, 7)
        self.db.set_review_members(0, Selection.from_indices(129, [8]), True)
        self.db.close()
        before = {path.name: (path.read_bytes(), path.stat().st_mtime_ns)
                  for path in self.folder.iterdir() if path.is_file()}
        analysis = self.open(review=False)
        with patch.object(drc.IcePack, "_block", side_effect=AssertionError("geometry decoded")):
            self.assertEqual(analysis.review_flags(0, 7), 0)
            self.assertEqual(analysis.review_counts(), expected_counts(134))
            with self.assertRaises(OSError):
                analysis.mark_visited(0, 7)
            with self.assertRaises(OSError):
                analysis.set_review_members(0, None, True)
        analysis.close()
        after = {path.name: (path.read_bytes(), path.stat().st_mtime_ns)
                 for path in self.folder.iterdir() if path.is_file()}
        self.assertEqual(after, before)

    def test_concurrent_open_handles_do_not_lose_visits_or_counter_updates(self):
        other = self.open()
        barrier = threading.Barrier(2)
        failures = []

        def visit(db, ids):
            try:
                barrier.wait(timeout=5)
                for ei in ids:
                    db.mark_visited(0, ei)
            except BaseException as exc:
                failures.append(exc)

        threads = [threading.Thread(target=visit, args=(self.db, range(0, 80))),
                   threading.Thread(target=visit, args=(other, range(40, 120)))]
        for thread in threads:
            thread.start()
        for thread in threads:
            thread.join(timeout=15)
            self.assertFalse(thread.is_alive(), "review write deadlocked")
        self.assertEqual(failures, [])
        self.assertEqual(self.db.review_counts(0), expected_counts(129, 120))
        self.assertEqual(other.review_counts(0), expected_counts(129, 120))
        self.assertEqual([self.db.review_flags(0, ei) for ei in range(129)],
                         [1] * 120 + [0] * 9)


class MaskMembers:
    """Membership that rejects allocation or enumeration of all selected IDs."""

    def __init__(self, size, modulo=3):
        self.size = size
        self.modulo = modulo
        self.calls = []

    def __iter__(self):
        raise AssertionError("review operation enumerated every selected ID")

    def mask(self, start, count):
        if count > 65536:
            raise AssertionError("review operation allocated an unbounded mask")
        self.calls.append((start, count))
        return np.arange(start, start + count) % self.modulo == 0


class ReviewStoreTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="floe-review-store-")
        self.addCleanup(self.temp.cleanup)
        self.folder = Path(self.temp.name)
        self.path = self.folder / "state.sqlite"
        self.pack = self.folder / "synthetic.tray"

    def store(self, sizes=(9003, 5, 0), fingerprint="fixture-v1", **kwargs):
        store = review.ReviewStore(str(self.pack), sizes, fingerprint,
                                   path=str(self.path), **kwargs)
        self.addCleanup(store.close)
        return store

    def test_hundred_million_rule_single_visits_use_sparse_storage(self):
        size = 100_000_003
        store = self.store((size, 5, 0))
        for ei in (0, 4095, 4096, size - 1):
            store.mark_visited(0, ei)
        self.assertEqual(store.set_members(
            0, Selection.from_indices(size, [0, size - 1]), True), 2)
        self.assertEqual(store.counts(0), expected_counts(size, 4, 2, 2))
        self.assertEqual(store.counts(), expected_counts(size + 5, 4, 2, 2))
        self.assertEqual(store.flags(0, size - 1), review.VISITED | review.NOT_REQUIRED)
        self.assertEqual(store.flags(0, size - 2), 0)
        store.close()
        stored_bytes = sum(path.stat().st_size for path in self.folder.iterdir()
                           if path.is_file())
        self.assertLess(stored_bytes, 1_000_000,
                        "four visits allocated rule-sized review storage")
        reopened = self.store((size, 5, 0))
        self.assertEqual(reopened.counts(), expected_counts(size + 5, 4, 2, 2))

    def test_mask_only_bulk_write_is_bounded_and_reports_monotonic_progress(self):
        size = 9003
        store = self.store()
        members = MaskMembers(size)
        store.mark_visited(0, 0)
        store.mark_visited(0, 3)
        store.mark_visited(0, 8)
        progress = []
        changed = store.set_members(0, members, True,
                                    progress=lambda *values: progress.append(values))
        expected = (size + 2) // 3
        self.assertEqual(changed, expected)
        self.assertEqual(store.counts(), expected_counts(size + 5, 3, expected, 2))
        self.assertEqual(progress[-1], (size, size, expected))
        self.assertEqual([step[0] for step in progress],
                         sorted(step[0] for step in progress))
        self.assertEqual(sum(count for _start, count in members.calls), size)
        self.assertEqual(store.flags(0, 0), 3)
        self.assertEqual(store.flags(0, 8), 1)
        self.assertEqual(store.flags(0, 9000), 2)
        self.assertEqual(store.flags(1, 0), 0)

    def test_cancelled_bulk_retains_only_completed_chunks_and_exact_counters(self):
        size = 3 * 65536 + 17
        store = self.store((size, 5, 0))
        store.mark_visited(0, 0)
        store.mark_visited(0, size - 1)
        progress = []
        stop = threading.Event()

        def completed(processed, total, changed):
            progress.append((processed, total, changed))
            stop.set()

        with self.assertRaises(review.ReviewCancelled) as raised:
            store.set_members(0, None, True, cancelled=stop.is_set, progress=completed)
        done, total, changed = progress[-1]
        self.assertEqual((raised.exception.processed, raised.exception.total,
                          raised.exception.changed), (done, total, changed))
        self.assertEqual(total, size)
        self.assertEqual(done, changed)
        self.assertGreater(done, 0)
        self.assertLess(done, size)
        expected = expected_counts(size, 2, done, 1)
        self.assertEqual(store.counts(0), expected)
        self.assertEqual(store.flags(0, done - 1), 2)
        self.assertEqual(store.flags(0, done), 0)
        self.assertEqual(store.flags(0, size - 1), 1)
        store.close()
        reopened = self.store((size, 5, 0))
        self.assertEqual(reopened.counts(0), expected)
        self.assertEqual(reopened.set_members(0, None, True), size - done)
        self.assertEqual(reopened.counts(0), expected_counts(size, 2, size, 2))

    def test_initial_cancellation_makes_no_changes(self):
        store = self.store()
        with self.assertRaises(review.ReviewCancelled):
            store.set_members(0, None, True, cancelled=lambda: True)
        self.assertEqual(store.counts(), expected_counts(9008))

    def test_mismatched_pack_identity_rejects_existing_state_without_overwriting(self):
        store = self.store()
        store.mark_visited(0, 9002)
        store.close()
        before = self.path.read_bytes()
        with self.assertRaises(review.ReviewIdentityError):
            self.store(fingerprint="a-different-pack").counts()
        self.assertEqual(self.path.read_bytes(), before)
        with self.assertRaises(review.ReviewIdentityError):
            self.store(sizes=(9004, 4, 0)).counts()
        self.assertEqual(self.path.read_bytes(), before)
        reopened = self.store()
        self.assertEqual(reopened.counts(), expected_counts(9008, 1))
        self.assertEqual(reopened.flags(0, 9002), 1)

    def test_readonly_store_never_reads_existing_review_database(self):
        self.path.write_bytes(b"not a sqlite file: analysis must not open this")
        before = self.path.read_bytes()
        store = self.store(writable=False)
        self.assertEqual(store.counts(), expected_counts(9008))
        self.assertEqual(store.flags(0, 1), 0)
        with self.assertRaises(OSError):
            store.mark_visited(0, 1)
        with self.assertRaises(OSError):
            store.set_members(0, None, True)
        self.assertEqual(self.path.read_bytes(), before)

    def test_invalid_membership_mask_keeps_flags_and_counters_unchanged(self):
        store = self.store()
        members = MaskMembers(9003)
        members.mask = lambda _start, count: np.zeros(count + 1, dtype=bool)
        with self.assertRaises(ValueError):
            store.set_members(0, members, True)
        self.assertEqual(store.counts(), expected_counts(9008))

    def test_random_visit_and_not_required_edits_match_independent_bitset_oracle(self):
        size = 4113
        store = self.store((size, 5, 0))
        rng = np.random.default_rng(20261009)
        visited, skipped = set(), set()
        for step in range(80):
            if step % 3 == 0:
                ei = int(rng.integers(size))
                store.mark_visited(0, ei)
                visited.add(ei)
            else:
                chosen = set(int(ei) for ei in rng.integers(size, size=70))
                selected = Selection.from_indices(size, sorted(chosen))
                on = step % 4 != 0
                changed = len(chosen - skipped if on else chosen & skipped)
                self.assertEqual(store.set_members(0, selected, on), changed)
                skipped = skipped | chosen if on else skipped - chosen
            self.assertEqual(store.counts(0), expected_counts(
                size, len(visited), len(skipped), len(visited & skipped)))
            self.assertEqual(store.counts(), expected_counts(
                size + 5, len(visited), len(skipped), len(visited & skipped)))
        self.assertEqual([store.flags(0, ei) for ei in range(size)],
                         [int(ei in visited) | (2 * int(ei in skipped))
                          for ei in range(size)])

    def test_failed_write_preserves_agreement_between_flags_counts_and_reopen(self):
        store = self.store()
        store.mark_visited(0, 0)
        # Simulate the database rejecting a later row of a bulk transaction.
        # Inspect behavior, not transaction size: committed work may remain,
        # but cached flags, persisted flags and counters must always agree.
        store._conn.execute("""
            CREATE TRIGGER fail_review_chunk BEFORE INSERT ON chunks
            WHEN NEW.chunk = 1
            BEGIN SELECT RAISE(ABORT, 'injected review write failure'); END
        """)
        with self.assertRaisesRegex(sqlite3.DatabaseError, "injected review"):
            store.set_members(0, None, True)
        live = [store.flags(0, ei) for ei in range(9003)]
        self.assertEqual(live[0] & review.VISITED, review.VISITED)
        visited = sum(bool(flags & review.VISITED) for flags in live)
        skipped = sum(bool(flags & review.NOT_REQUIRED) for flags in live)
        both = sum(flags == (review.VISITED | review.NOT_REQUIRED) for flags in live)
        expected = expected_counts(9003, visited, skipped, both)
        self.assertEqual(store.counts(0), expected)
        store.close()
        reopened = self.store()
        self.assertEqual(reopened.counts(0), expected)
        self.assertEqual([reopened.flags(0, ei) for ei in range(9003)], live)

    def test_unwritable_primary_uses_and_reopens_durable_fallback(self):
        primary = self.folder / "primary" / "review.sqlite3"
        fallback = self.folder / "user-data" / "review.sqlite3"
        connect = sqlite3.connect

        def restricted_connect(database, *args, **kwargs):
            if str(primary) in database:
                raise sqlite3.OperationalError("unable to open database file")
            return connect(database, *args, **kwargs)

        with patch.dict(os.environ, {"FLOE_DRC_REVIEW_ROOT": ""}), \
                patch.object(review, "review_path", return_value=str(primary)), \
                patch.object(review, "_fallback_path", return_value=str(fallback)), \
                patch.object(review.sqlite3, "connect", side_effect=restricted_connect):
            store = review.ReviewStore(str(self.pack), (10,), "fallback-fixture")
            self.addCleanup(store.close)
            store.mark_visited(0, 7)
            self.assertEqual(store.path, str(fallback))
            self.assertEqual(store.counts(), expected_counts(10, 1))
            store.close()
            reopened = review.ReviewStore(str(self.pack), (10,), "fallback-fixture")
            self.addCleanup(reopened.close)
            self.assertEqual(reopened.path, str(fallback))
            self.assertEqual(reopened.flags(0, 7), 1)
            self.assertEqual(reopened.counts(), expected_counts(10, 1))

    def test_existing_corrupt_primary_is_not_bypassed_by_empty_fallback(self):
        primary, fallback = self.path, self.folder / "fallback.sqlite3"
        corrupt = b"existing user review data must be preserved"
        primary.write_bytes(corrupt)
        with patch.dict(os.environ, {"FLOE_DRC_REVIEW_ROOT": ""}), \
                patch.object(review, "review_path", return_value=str(primary)), \
                patch.object(review, "_fallback_path", return_value=str(fallback)):
            store = review.ReviewStore(str(self.pack), (10,), "corrupt-fixture")
            self.addCleanup(store.close)
            with self.assertRaises(sqlite3.DatabaseError):
                store.mark_visited(0, 7)
        self.assertEqual(primary.read_bytes(), corrupt)
        self.assertFalse(fallback.exists())

    def test_page_of_unvisited_flags_does_not_repeat_missing_file_probes(self):
        store = self.store()
        with patch.object(review, "_exists", wraps=review._exists) as exists, \
                patch.object(review.time, "monotonic", return_value=10.0), \
                patch.object(review.sqlite3, "connect",
                             side_effect=AssertionError("empty reads opened SQLite")):
            self.assertEqual([store.flags(0, ei) for ei in range(1000)], [0] * 1000)
            self.assertLessEqual(exists.call_count, 2,
                                 "grid rendering probed the sidecar per row")
        self.assertFalse(self.path.exists())

    def test_page_of_existing_flags_reuses_chunk_and_counter_queries(self):
        store = self.store()
        store.mark_visited(0, 7)
        statements = []
        store._conn.set_trace_callback(statements.append)
        with patch.object(review.time, "monotonic", return_value=10.0):
            flags = [store.flags(0, ei) for ei in range(1000)]
        self.assertEqual(flags, [int(ei == 7) for ei in range(1000)])
        self.assertLessEqual(len(statements), 6,
                             "grid rendering queried SQLite for each row")

    def test_reader_remains_available_while_writer_waits_for_another_process(self):
        store = self.store()
        store.mark_visited(0, 0)
        writer_started, reader_finished = threading.Event(), threading.Event()
        failures, read_results = [], []
        store._writer.set_trace_callback(
            lambda statement: writer_started.set()
            if statement == "BEGIN IMMEDIATE" else None)
        blocker = sqlite3.connect(str(self.path), isolation_level=None)
        self.addCleanup(blocker.close)
        blocker.execute("BEGIN IMMEDIATE")

        def write():
            try:
                store.mark_visited(0, 1)
            except BaseException as exc:
                failures.append(exc)

        def read():
            try:
                read_results.extend((store.flags(0, 0), store.counts(0)))
            except BaseException as exc:
                failures.append(exc)
            finally:
                reader_finished.set()

        writer = threading.Thread(target=write)
        reader = threading.Thread(target=read)
        try:
            writer.start()
            self.assertTrue(writer_started.wait(3), "writer never attempted transaction")
            reader.start()
            self.assertTrue(reader_finished.wait(1),
                            "statistics/grid reads waited for the blocked writer")
            self.assertEqual(read_results, [1, expected_counts(9003, 1)])
            self.assertTrue(writer.is_alive(), "external lock did not block writer")
        finally:
            blocker.execute("ROLLBACK")
            writer.join(5)
            if reader.ident is not None:
                reader.join(5)
        self.assertFalse(writer.is_alive())
        self.assertFalse(reader.is_alive())
        self.assertEqual(failures, [])
        self.assertEqual(store.counts(0), expected_counts(9003, 2))

    def test_missing_metadata_errors_are_backed_off_for_an_entire_grid_page(self):
        with sqlite3.connect(str(self.path)) as connection:
            connection.execute("PRAGMA user_version=7")
        original = self.path.read_bytes()
        store = self.store()
        connect = sqlite3.connect
        with patch.object(review.time, "monotonic", return_value=100.0), \
                patch.object(review.time, "sleep",
                             side_effect=AssertionError("foreground metadata read retried synchronously")), \
                patch.object(review.sqlite3, "connect", wraps=connect) as opened:
            started = time.perf_counter()
            for ei in range(1000):
                with self.assertRaises(sqlite3.DatabaseError):
                    store.flags(0, ei)
            elapsed = time.perf_counter() - started
            self.assertLess(elapsed, 1.0,
                            "missing review metadata repeatedly blocked grid rendering")
            self.assertEqual(opened.call_count, 1,
                             "failed review database reopened for each grid cell")
            with self.assertRaises(sqlite3.DatabaseError):
                store.counts()
            self.assertEqual(opened.call_count, 1)
        self.assertEqual(self.path.read_bytes(), original,
                         "invalid reviewer database was overwritten")

    def test_fresh_reader_locked_database_fails_fast_then_recovers_after_backoff(self):
        initial = self.store()
        initial.mark_visited(0, 7)
        initial.close()
        blocker = sqlite3.connect(str(self.path), isolation_level=None)
        self.addCleanup(blocker.close)
        blocker.execute("BEGIN EXCLUSIVE")
        fresh = self.store()
        connect = sqlite3.connect
        try:
            with patch.object(review.time, "monotonic", return_value=100.0) as clock, \
                    patch.object(review.sqlite3, "connect", wraps=connect) as opened:
                started = time.perf_counter()
                for ei in range(1000):
                    with self.assertRaises(sqlite3.DatabaseError):
                        fresh.flags(0, ei)
                elapsed = time.perf_counter() - started
                self.assertLess(elapsed, 1.0,
                                "fresh reader inherited the background writer's long timeout")
                self.assertEqual(opened.call_count, 1)
                blocker.execute("ROLLBACK")
                with self.assertRaises(sqlite3.DatabaseError):
                    fresh.flags(0, 7)
                clock.return_value = 102.0
                self.assertEqual(fresh.flags(0, 7), review.VISITED)
                self.assertEqual(fresh.counts(0), expected_counts(9003, 1))
                self.assertEqual(opened.call_count, 2)
        finally:
            if blocker.in_transaction:
                blocker.execute("ROLLBACK")

    def test_identity_errors_remain_errors_without_reopening_for_each_grid_cell(self):
        initial = self.store()
        initial.mark_visited(0, 7)
        initial.close()
        original = self.path.read_bytes()
        wrong = self.store(fingerprint="different-result")
        connect = sqlite3.connect
        with patch.object(review.time, "monotonic", return_value=100.0), \
                patch.object(review.sqlite3, "connect", wraps=connect) as opened:
            started = time.perf_counter()
            for ei in range(1000):
                with self.assertRaises(review.ReviewIdentityError):
                    wrong.flags(0, ei)
            self.assertLess(time.perf_counter() - started, 1.0)
            self.assertEqual(opened.call_count, 1)
        self.assertEqual(self.path.read_bytes(), original)

    def test_initialization_mutex_cannot_block_an_entire_grid_page(self):
        store = self.store()
        acquired, release = threading.Event(), threading.Event()

        def initialize():
            with store._lock:
                acquired.set()
                release.wait(5)

        initializer = threading.Thread(target=initialize)
        initializer.start()
        try:
            self.assertTrue(acquired.wait(3), "initializer did not acquire store mutex")
            with patch.object(review.time, "monotonic", return_value=100.0) as clock:
                started = time.perf_counter()
                for ei in range(1000):
                    with self.assertRaisesRegex(OSError, "initialization"):
                        store.flags(0, ei)
                self.assertLess(time.perf_counter() - started, 1.0,
                                "grid rendering waited for background initialization")
                with self.assertRaises(OSError):
                    store.counts()
                release.set()
                initializer.join(3)
                self.assertFalse(initializer.is_alive())
                with self.assertRaises(OSError):
                    store.flags(0, 0)
                clock.return_value = 102.0
                self.assertEqual(store.flags(0, 0), 0)
                self.assertEqual(store.counts(), expected_counts(9008))
        finally:
            release.set()
            initializer.join(5)
        self.assertFalse(self.path.exists(), "read retry created an empty sidecar")


if __name__ == "__main__":
    unittest.main(verbosity=2)
