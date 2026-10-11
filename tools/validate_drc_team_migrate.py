"""Synthetic-only rerun review migration gates; no personal files are read."""

from contextlib import closing
import os
from pathlib import Path
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import threading
import unittest
from types import SimpleNamespace
from unittest.mock import patch

import numpy as np

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from floe import drc, drc_migrate as migrate, drc_team as team  # noqa: E402
from floe.drc_review import ReviewStore, VISITED, NOT_REQUIRED  # noqa: E402
from floe.drc_selection import Selection  # noqa: E402

BIN = ROOT / "rust/target/release/floe-index"


def build_pack(folder, name, rules):
    source = Path(folder) / (name + ".db")
    pack = Path(folder) / ("." + name + ".db.tray")
    with source.open("w") as stream:
        stream.write("MIGRATE 1000\n")
        for rule, coords in rules:
            stream.write("%s\n%d %d 0\n" % (rule, len(coords), len(coords)))
            for ei, x in enumerate(coords):
                stream.write("e %d 1\n%d -7 %d 13\n" % (ei + 1, x, x + 6))
    subprocess.run([str(BIN), "drc", str(source), str(pack)], capture_output=True, check=True)
    return source, pack


@unittest.skipUnless(BIN.is_file(), "build floe-index first")
class MigrationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.owner = tempfile.TemporaryDirectory(prefix="floe-migrate-fixtures-")
        old_x = [ei * 20 for ei in range(1105)]
        old_x[4], old_x[6] = old_x[3], old_x[5]
        new_x = list(reversed(old_x)) + [999999]
        new_x[1104 - 2] += 1  # Bounding/coordinate-near is not exact.
        cls.old_db, cls.old_pack = build_pack(cls.owner.name, "old", [("A", old_x), ("B", [77777])])
        cls.new_db, cls.new_pack = build_pack(cls.owner.name, "new", [("B", [77777]), ("A", new_x)])

    @classmethod
    def tearDownClass(cls):
        cls.owner.cleanup()

    def setUp(self):
        owner = tempfile.TemporaryDirectory(prefix="floe-migrate-test-")
        self.addCleanup(owner.cleanup)
        self.folder = Path(owner.name)
        env = patch.dict(os.environ, {"FLOE_REVIEWER": "migration-test"})
        env.start()
        self.addCleanup(env.stop)
        for source in (self.old_db, self.old_pack, self.new_db, self.new_pack):
            shutil.copy2(source, self.folder / source.name)
        self.old_path, self.new_path = self.folder / self.old_pack.name, self.folder / self.new_pack.name
        self.old = drc.IcePack(str(self.old_path), cache_write=False)
        self.new = drc.IcePack(str(self.new_path), cache_write=False)
        self.addCleanup(self.old.close)
        self.addCleanup(self.new.close)
        for ei in (0, 1, 2, 3, 4, 7):
            self.old.mark_visited(0, ei)
        for ei in (0, 3, 5):
            self.old.set_status(0, ei, drc.STATUS_WAIVED)
        self.old.set_review_members(0, Selection.from_indices(1105, [1, 8]), True)

    def fake_match(self, old, target, pairs, states, folder, cancelled, report):
        self.assertEqual(pairs, [[0, 1]])
        flags = np.memmap(states, mode="r", dtype=np.uint8)
        try:
            np.testing.assert_array_equal(flags[:9], [3, 5, 1, 3, 1, 2, 0, 1, 4])
        finally:
            flags._mmap.close()
        rows = [(1, 4, 0, 0, 0, 1104 - 8), (1, 1, 0, 0, 0, 1104 - 7),
                (1, 0, 0, 2, 0, 1104 - 6), (1, 0, 0, 2, 0, 1104 - 5),
                (1, 1, 0, 2, 0, 1104 - 4), (1, 1, 0, 2, 0, 1104 - 3),
                (1, 5, 0, 0, 0, 1104 - 1), (1, 3, 0, 0, 0, 1104)]
        path = Path(folder) / "matches.bin"
        np.array(rows, dtype=migrate._RECORD).tofile(path)
        return str(path), {"matched_target": 8, "ambiguous_target": 4, "partial_target": 4}

    def prepare(self, native=False, **kwargs):
        if native:
            if not migrate._compatible(str(BIN), *self.binary_stamp()):
                self.skipTest("build floe-index with drc-match protocol 2")
            plan = migrate.prepare_migration(self.new, str(self.folder / self.old_db.name), **kwargs)
        else:
            with patch.object(migrate, "_match", side_effect=self.fake_match):
                plan = migrate.prepare_migration(self.new, str(self.folder / self.old_db.name), **kwargs)
        self.addCleanup(plan.close)
        return plan

    def binary_stamp(self):
        st = BIN.stat()
        return st.st_size, st.st_mtime_ns, st.st_ctime_ns

    def test_prepare_reorders_rules_errors_and_preserves_original_files(self):
        paths = (self.old_path, Path(self.old._waive_path), Path(self.old.review_path))
        before = {path: path.read_bytes() for path in paths}
        plan = self.prepare()
        self.assertEqual(plan.summary["matched"], 8)
        self.assertEqual(plan.summary["ambiguous"], 4)
        self.assertEqual(plan.summary["partial"], 4)
        self.assertEqual(plan.rule_counts(0)["matched"], 0)
        self.assertEqual(plan.page(1, 0, 1000), [1096, 1097, 1098, 1099, 1100, 1101, 1103, 1104])
        self.assertEqual(plan.page(1, 0, 1000, "ambiguous"), [1098, 1099, 1100, 1101])
        self.assertEqual(plan.get(1, 1098), team.ReviewState(False, None, False))
        self.assertEqual(plan.get(1, 1100), team.ReviewState(True, None, False))
        self.assertEqual(plan.get(1, 1104), team.ReviewState(True, True, False))
        for path, original in before.items():
            self.assertEqual(path.read_bytes(), original)
        self.assertFalse((self.folder / ".old.db.analysis").exists())
        self.assertEqual(self.new.review_counts()["visited"], 0)
        self.assertFalse((Path(plan._owner.name) / "old-states.u8").exists())

    def test_apply_transfers_visits_without_erasing_live_review_conflicts(self):
        self.new.mark_visited(1, 1104)  # Own W=0 differs from prior W=1.
        plan = self.prepare()
        self.assertEqual(plan.page(1, 0, 1000, "conflict"), [1098, 1099, 1100, 1101, 1104])
        # A visit after preview is protected too.
        self.new.mark_visited(1, 1103)
        result = migrate.apply_migration(self.new, plan)
        self.assertEqual(result["conflicts_skipped"], 2)
        self.assertEqual(result["changed_visited"], 3)
        self.assertEqual(result["changed_not_required"], 1)
        self.assertEqual(result["changed_waived"], 0)
        self.assertEqual(self.new.review_flags(1, 1097), 1)
        self.assertEqual(self.new.review_flags(1, 1096), 2)  # NR without prior visit.
        self.assertEqual(int(self.new._status[int(self.new._dir_es[1]) + 1104]), drc.STATUS_NONE)
        self.assertEqual(plan.rule_counts(1)["protected"], 2)
        again = migrate.apply_migration(self.new, plan)
        self.assertEqual(again["changed_visited"], 0)
        self.assertEqual(again["changed_not_required"], 0)

    def test_successful_transfer_has_correct_v_nr_intersection_counters(self):
        plan = self.prepare()
        result = migrate.apply_migration(self.new, plan)
        self.assertEqual(result["changed_visited"], 5)
        self.assertEqual(result["changed_waived"], 1)
        self.assertEqual(result["changed_not_required"], 2)
        counts = self.new.review_counts()
        self.assertEqual((counts["visited"], counts["not_required"], counts["visited_not_required"]), (5, 2, 1))
        self.assertEqual(self.new.review_flags(1, 1098), 0)
        self.assertEqual(self.new.status_counts(1)[0], 1)
        # Existing ordinary peer import must still never copy V.
        untouched = drc.IcePack(str(self.new_path), review=False, cache_write=False)
        self.addCleanup(untouched.close)
        self.assertFalse(untouched._review)

    def test_explicit_snapshot_is_frozen_before_original_path_replacement(self):
        path = self.folder / "previous.floereview"
        snapshot = team.export_review(self.old, str(path))
        snapshot.close()
        original = path.read_bytes()

        def replace_then_match(*args):
            path.write_bytes(b"replaced while matching")
            return self.fake_match(*args)

        with patch.object(migrate, "_match", side_effect=replace_then_match):
            plan = migrate.prepare_migration(self.new, str(self.old_path), str(path))
        self.addCleanup(plan.close)
        self.assertEqual(plan.get(1, 1104), team.ReviewState(True, True, False))
        self.assertEqual(plan.state_path, str(path))
        self.assertNotEqual(path.read_bytes(), original)

    def test_same_result_and_foreign_state_reject_without_mutation(self):
        with self.assertRaisesRegex(ValueError, "currently open"):
            migrate.prepare_migration(self.new, str(self.new_path))
        path = self.folder / "wrong.floereview"
        snapshot = team.export_review(self.new, str(path))
        snapshot.close()
        before = path.read_bytes()
        with self.assertRaises(ValueError):
            migrate.prepare_migration(self.new, str(self.old_path), str(path))
        self.assertEqual(path.read_bytes(), before)

    def test_cancel_and_native_error_remove_owned_temporary_artifacts(self):
        factory, paths = tempfile.TemporaryDirectory, []

        def temporary(*args, **kwargs):
            owner = factory(*args, **kwargs)
            if kwargs.get("prefix") == "floe-review-migration-":
                paths.append(Path(owner.name))
            return owner

        with patch.object(migrate.tempfile, "TemporaryDirectory", side_effect=temporary), \
                patch.object(migrate, "_match", side_effect=RuntimeError("injected native failure")):
            with self.assertRaisesRegex(RuntimeError, "injected native"):
                migrate.prepare_migration(self.new, str(self.old_path))
        self.assertTrue(paths)
        self.assertTrue(all(not path.exists() for path in paths))
        with self.assertRaises(InterruptedError):
            migrate.prepare_migration(self.new, str(self.old_path), cancelled=lambda: True)

    def test_apply_failure_does_not_mark_unsaved_decision_visited(self):
        plan = self.prepare()
        with patch.object(drc.IcePack, "_write_status_span", side_effect=OSError("injected status write")):
            with self.assertRaisesRegex(OSError, "injected status"):
                migrate.apply_migration(self.new, plan)
        self.assertEqual(self.new.review_counts()["visited"], 0)
        self.assertEqual(self.new.review_counts()["not_required"], 0)

    def test_exclusion_write_failure_keeps_visits_unset_after_waive_commit(self):
        plan = self.prepare()
        store = self.new._review_flags_store()
        with patch.object(store, "_change_many", side_effect=OSError("injected flags write")):
            with self.assertRaisesRegex(OSError, "injected flags"):
                migrate.apply_migration(self.new, plan)
        self.assertEqual(self.new.status_counts(1)[0], 1)
        self.assertEqual(self.new.review_counts()["visited"], 0)
        self.assertEqual(self.new.review_counts()["not_required"], 0)

    def test_invalid_existing_personal_flags_are_not_replaced_by_empty_fallback(self):
        path = Path(self.old.review_path)
        with closing(sqlite3.connect(path)) as connection, connection:
            row = connection.execute("SELECT value FROM metadata WHERE id=1").fetchone()[0]
            connection.execute("UPDATE metadata SET value=? WHERE id=1", (row.replace(self.old._review_fingerprint, "wrong"),))
        original = path.read_bytes()
        with patch.object(migrate, "_match", side_effect=AssertionError("invalid source matched")):
            with self.assertRaises(ValueError):
                migrate.prepare_migration(self.new, str(self.old_path))
        self.assertEqual(path.read_bytes(), original)

    def test_legacy_waive_only_preserves_unknown_visit_and_exclusion(self):
        self.new.set_review_members(1, Selection.from_indices(1106, [1104]), True)

        def match_waive(old, target, pairs, states, folder, cancelled, report):
            with open(states, "rb") as stream:
                self.assertEqual(stream.read(9), bytes([2, 0, 0, 2, 0, 2, 0, 0, 0]))
            path = Path(folder) / "matches.bin"
            np.array([(1, 2, 0, 0, 0, 1104)], dtype=migrate._RECORD).tofile(path)
            return str(path), {"matched_target": 1, "ambiguous_target": 0}

        with patch.object(migrate, "_match", side_effect=match_waive):
            plan = migrate.prepare_migration(self.new, str(self.old_path), self.old._waive_path)
        self.addCleanup(plan.close)
        self.assertEqual(plan.get(1, 1104), team.ReviewState(None, True, None))
        result = migrate.apply_migration(self.new, plan)
        self.assertEqual(result["changed_visited"], 0)
        self.assertEqual(result["changed_waived"], 1)
        self.assertEqual(self.new.review_flags(1, 1104), 2)

    def test_unpaired_review_preserves_unknown_waive(self):
        raw = self.folder / "unpaired.sqlite3"
        shutil.copy2(self.old.review_path, raw)
        self.new.set_status(1, 1104, drc.STATUS_WAIVED)

        def match_flags(old, target, pairs, states, folder, cancelled, report):
            with open(states, "rb") as stream:
                self.assertEqual(stream.read(9), bytes([1, 5, 1, 1, 1, 0, 0, 1, 4]))
            path = Path(folder) / "matches.bin"
            np.array([(1, 1, 0, 0, 0, 1104)], dtype=migrate._RECORD).tofile(path)
            return str(path), {"matched_target": 1, "ambiguous_target": 0}

        with patch.object(migrate, "_match", side_effect=match_flags):
            plan = migrate.prepare_migration(self.new, str(self.old_path), str(raw))
        self.addCleanup(plan.close)
        self.assertEqual(plan.get(1, 1104), team.ReviewState(True, None, False))
        result = migrate.apply_migration(self.new, plan)
        self.assertEqual(result["changed_visited"], 1)
        self.assertEqual(self.new.status_counts(1)[0], 1)

    def test_native_exact_match_pipeline_and_ambiguous_zero_duplicate(self):
        plan = self.prepare(native=True)
        self.assertEqual(plan.page(1, 0, 1000), [1096, 1097, 1098, 1099, 1100, 1101, 1103, 1104])
        self.assertEqual(plan.page(1, 0, 1000, "ambiguous"), [1098, 1099, 1100, 1101])
        self.assertFalse(plan.members(1).contains(1102), "one-tick changed geometry transferred")
        result = migrate.apply_migration(self.new, plan)
        self.assertEqual(result["changed_visited"], 5)


class SparseMappedTests(unittest.TestCase):
    def test_independent_consensus_matrix_zero_flags_and_protected_overlap(self):
        # Global unknown legacy dimensions intersect with the per-error
        # disagreement mask; neither can manufacture a false decision.
        for known_w, known_n in ((True, True), (False, True), (True, False)):
            with self.subTest(known_w=known_w, known_n=known_n), \
                    tempfile.TemporaryDirectory(prefix="floe-migrate-matrix-") as folder:
                folder = Path(folder)
                pack = folder / "virtual.tray"
                pack.write_bytes(b"immutable matrix fixture")
                cases = []
                for unknown in (0, 2, 4, 6):
                    for flags in range(8):
                        if flags & unknown or not flags | unknown:
                            continue
                        for own_v in (False, True):
                            for own_w in (False, True):
                                for own_n in (False, True):
                                    incoming_v, incoming_w, incoming_n = (bool(flags & bit) for bit in (1, 2, 4))
                                    use_w, use_n = known_w and not unknown & 2, known_n and not unknown & 4
                                    protect = own_v and ((use_w and own_w != incoming_w) or
                                                         (use_n and own_n != incoming_n))
                                    expected = ((own_v, own_w, own_n) if protect else
                                        (own_v or incoming_v, incoming_w if use_w else own_w,
                                         incoming_n if use_n else own_n))
                                    cases.append((flags, unknown, own_v, own_w, own_n, protect, expected))
                size = len(cases) + 1
                statuses = np.array([drc.STATUS_WAIVED if c[3] else drc.STATUS_NONE for c in cases] + [drc.STATUS_WAIVED], dtype=np.uint8)
                store = ReviewStore(str(pack), (size,), "matrix", path=str(folder / "review.sqlite3"))
                source = SimpleNamespace(visited_known=True, waive_known=known_w, not_required_known=known_n,
                                         fingerprint="prior", snapshot_id="prior", reviewer="prior")
                target = SimpleNamespace(path=str(pack), _review_fingerprint="matrix",
                    _ecnt=np.array([size]), _dir_es=np.array([0]), _status=statuses,
                    _status_lock=threading.RLock(), _review_flags_store=lambda: store,
                    _require_review=lambda: None,
                    _write_status_span=lambda ci, start, before, after: statuses.__setitem__(slice(start, start + len(after)), after))
                store._change_many(0, [(0, sum(1 << ei for ei, c in enumerate(cases) if c[2]))], VISITED, True)
                store._change_many(0, [(0, sum(1 << ei for ei, c in enumerate(cases) if c[4]))], NOT_REQUIRED, True)
                records = folder / "records.bin"
                rows = [(0, c[0], 0, c[1], 0, ei) for ei, c in enumerate(cases)]
                rows.append((0, 0, 1, 0, 0, len(cases)))  # No proven exact geometry.
                np.array(rows, dtype=migrate._RECORD).tofile(records)
                owner = tempfile.TemporaryDirectory(prefix="floe-matrix-plan-")
                plan = None
                try:
                    mapped, ambiguous, eligibility = migrate._mapped_snapshots(
                        target, source, str(records), owner.name, None, lambda *_: None)
                    partial = [ei for ei, c in enumerate(cases) if c[1]]
                    protected = [ei for ei, c in enumerate(cases) if c[5]]
                    plan = migrate.MigrationPlan(target, owner, source, mapped, ambiguous, eligibility,
                        dict(source_path="prior", state_path="prior", ambiguous=len(partial) + 1))
                    plan.refresh_conflicts(target)
                    counts = plan.rule_counts(0)
                    self.assertEqual(counts["matched"], len(cases))
                    self.assertEqual(counts["partial"], len(partial))
                    self.assertEqual(counts["ambiguous"], len(partial) + 1)
                    self.assertEqual(counts["unmatched"], 0)
                    self.assertEqual(counts["protected"], len(protected))
                    self.assertEqual(counts["conflict"], len(set(partial + protected + [len(cases)])))
                    self.assertEqual(plan.page(0, 0, 1000, "protected"), protected)
                    self.assertEqual(plan.page(0, 0, 1000, "partial"), partial)
                    self.assertIsNone(plan.get(0, len(cases)))
                    for ei, case in enumerate(cases):
                        flags, unknown = case[:2]
                        self.assertEqual(plan.get(0, ei), team.ReviewState(bool(flags & 1),
                            bool(flags & 2) if known_w and not unknown & 2 else None,
                            bool(flags & 4) if known_n and not unknown & 4 else None))
                    original = Path(mapped.path).read_bytes()
                    result = migrate.apply_migration(target, plan)
                    self.assertEqual(result["conflicts_skipped"], len(protected))
                    self.assertEqual(plan.page(0, 0, 1000, "protected"), protected)
                    for ei, case in enumerate(cases):
                        flags = store.flags(0, ei)
                        self.assertEqual((bool(flags & VISITED), bool(statuses[ei] == drc.STATUS_WAIVED),
                                          bool(flags & NOT_REQUIRED)), case[-1], (ei, case))
                    self.assertEqual(statuses[-1], drc.STATUS_WAIVED)
                    self.assertEqual(store.flags(0, size - 1), 0)
                    self.assertEqual(Path(mapped.path).read_bytes(), original)
                finally:
                    if plan is not None:
                        plan.close()
                    else:
                        owner.cleanup()
                    store.close()

    def test_large_target_mapping_paging_and_batched_visit_application(self):
        with tempfile.TemporaryDirectory(prefix="floe-migrate-large-") as folder:
            folder = Path(folder)
            size = 100_000_003
            pack = folder / "virtual.tray"
            pack.write_bytes(b"virtual immutable identity")
            statuses_path = folder / "statuses.u8"
            with statuses_path.open("wb") as stream:
                stream.truncate(size)
            statuses = np.memmap(statuses_path, mode="r+", dtype=np.uint8, shape=(size,))
            store = ReviewStore(str(pack), (size,), "large-fixture", path=str(folder / "review.sqlite3"))
            spans = []

            def write_span(ci, start, before, after):
                self.assertLessEqual(len(after), 4096)
                spans.append((ci, start, len(after)))
                statuses[start:start + len(after)] = after

            target = SimpleNamespace(path=str(pack), _review_fingerprint="large-fixture",
                _ecnt=np.array([size]), _dir_es=np.array([0]), _status=statuses,
                _status_lock=threading.RLock(), _review_flags_store=lambda: store,
                _require_review=lambda: None, _write_status_span=write_span)
            source = SimpleNamespace(visited_known=True, waive_known=True, not_required_known=True,
                                     fingerprint="prior", snapshot_id="prior-id", reviewer="prior")
            ids = list(range(1003)) + [4095, 4096, 65536, size - 1]
            records = folder / "records.bin"
            np.array([(0, 7 if ei == size - 1 else 1, 0, 0, 0, ei) for ei in ids],
                     dtype=migrate._RECORD).tofile(records)
            owner = tempfile.TemporaryDirectory(prefix="floe-migration-plan-")
            plan = None
            try:
                with patch.object(drc.IcePack, "_block", side_effect=AssertionError("geometry decoded")):
                    mapped, ambiguous, eligibility = migrate._mapped_snapshots(target, source, str(records),
                        owner.name, None, lambda *_: None)
                    plan = migrate.MigrationPlan(target, owner, source, mapped, ambiguous, eligibility,
                        dict(source_path="prior", state_path="prior-review", ambiguous=0))
                    plan.refresh_conflicts(target)
                    self.assertEqual(plan.page(0, 0, 1000), ids[:1000])
                    self.assertEqual(plan.page(0, 1000, 1000), ids[1000:])
                    self.assertEqual(plan.members(0).rank(size - 1), len(ids) - 1)
                    with patch.object(store, "_change_many", wraps=store._change_many) as writes:
                        result = migrate.apply_migration(target, plan)
                    visits = [call for call in writes.call_args_list if call.args[2] == VISITED]
                    self.assertEqual(len(visits), 1, "one SQLite visit transaction per error")
                    self.assertLessEqual(len(visits[0].args[1]), 16)
                self.assertEqual(result["changed_visited"], len(ids))
                self.assertEqual(result["changed_waived"], 1)
                self.assertEqual(store.counts()["visited"], len(ids))
                self.assertEqual(store.counts()["visited_not_required"], 1)
                self.assertEqual(len(spans), 1)
                self.assertLess(Path(mapped.path).stat().st_size, 200_000)
            finally:
                if plan is not None:
                    plan.close()
                else:
                    owner.cleanup()
                store.close()
                statuses._mmap.close()

    def test_unsorted_and_duplicate_native_records_are_rejected(self):
        with tempfile.TemporaryDirectory() as folder:
            target = SimpleNamespace(path=os.path.join(folder, "target.tray"),
                                     _ecnt=np.array([10]), _review_fingerprint="f")
            source = SimpleNamespace(visited_known=True, waive_known=True, not_required_known=True,
                                     fingerprint="old", snapshot_id="id", reviewer="old")
            for values in ([4, 3], [4, 4]):
                path = Path(folder) / "records.bin"
                np.array([(0, 1, 0, 0, 0, ei) for ei in values], dtype=migrate._RECORD).tofile(path)
                with self.assertRaisesRegex(ValueError, "unique and sorted"):
                    migrate._mapped_snapshots(target, source, str(path), folder, None, lambda *_: None)


if __name__ == "__main__":
    unittest.main(verbosity=2)
