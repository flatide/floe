"""Independent reviewer snapshots, aggregate conflicts and safe import.

All packs, reviewer state and exported files are synthetic and confined to
temporary folders. No existing sample or personal reviewer files are used.
"""

import os
from contextlib import closing
from pathlib import Path
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from types import SimpleNamespace
from unittest.mock import patch

import numpy as np

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from floe import drc, drc_team as team  # noqa: E402
from floe.drc_review import ReviewStore  # noqa: E402
from floe.drc_selection import Selection  # noqa: E402
from floe.drc_marker_worker import export_members, import_members  # noqa: E402

BIN = ROOT / "rust" / "target" / "release" / "floe-index"
SIZES = (4113, 17, 0)


@unittest.skipUnless(BIN.is_file(), "build rust/target/release/floe-index first")
class TeamReviewTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.fixture = tempfile.TemporaryDirectory(prefix="floe-team-fixture-")
        source = Path(cls.fixture.name) / "synthetic.db"
        cls.packed = Path(cls.fixture.name) / "synthetic.tray"
        with source.open("w", encoding="utf-8") as stream:
            stream.write("TEAM_TEST 1000\n")
            for name, count in zip(("FIRST", "SECOND", "EMPTY"), SIZES):
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
        temporary = tempfile.TemporaryDirectory(prefix="floe-team-test-")
        self.addCleanup(temporary.cleanup)
        self.folder = Path(temporary.name)
        self.path = self.folder / "results.tray"
        shutil.copy2(self.packed, self.path)
        env = patch.dict(os.environ, {"FLOE_REVIEWER": "alice"})
        env.start()
        self.addCleanup(env.stop)
        self.own = self.database("alice")
        self.peer = self.database("bob")

    def database(self, reviewer, path=None):
        with patch.dict(os.environ, {"FLOE_REVIEWER": reviewer}):
            db = drc.IcePack(str(path or self.path))
            db._review_flags_store()  # Capture this reviewer's lazy store path.
        self.addCleanup(db.close)
        return db

    def state(self, db, ci, ei, *, visited=False, waived=False, not_required=False):
        if visited:
            db.mark_visited(ci, ei)
        db.set_status(ci, ei, drc.STATUS_WAIVED if waived else drc.STATUS_NONE)
        db.set_review_members(ci, Selection.from_indices(SIZES[ci], [ei]), not_required)

    def export(self, db, name, reviewer):
        path = self.folder / name
        source = team.export_review(db, str(path), reviewer=reviewer)
        self.addCleanup(source.close)
        return path, source

    def aggregate(self, sources, include_own=False):
        result = team.aggregate_reviews(self.own, sources, include_own=include_own)
        self.addCleanup(result.close)
        return result

    def unpaired_review(self):
        # Raw review files normally find a sibling waive automatically. Put
        # only this file in a separate folder to exercise genuinely unknown W.
        directory = self.folder / "unpaired"
        directory.mkdir(exist_ok=True)
        path = directory / "bob.sqlite3"
        shutil.copy2(self.peer.review_path, path)
        source = team.open_review(self.own, str(path), reviewer="bob")
        self.addCleanup(source.close)
        return source

    def assert_state(self, record, visited, waived, not_required, conflict=None):
        self.assertEqual((bool(record.visited), bool(record.waived),
                          bool(record.not_required)), (visited, waived, not_required))
        if conflict is not None:
            self.assertEqual(bool(record.conflict), conflict)

    def test_snapshot_roundtrip_is_independent_and_readonly(self):
        self.state(self.peer, 0, 7, visited=True, waived=True)
        self.state(self.peer, 0, 4096, not_required=True)
        self.state(self.peer, 1, 16, visited=True, not_required=True)
        with patch.object(drc.IcePack, "_block", side_effect=AssertionError("geometry decoded")):
            path, source = self.export(self.peer, "bob.review", "bob")
        original = path.read_bytes()
        self.assert_state(source.get(0, 7), True, True, False)
        self.assert_state(source.get(0, 4096), False, False, True)
        self.assertEqual(source.counts()["visited"], 2)
        self.assertEqual(source.counts()["work"], 3)
        self.state(self.peer, 0, 7, not_required=True)
        self.assert_state(source.get(0, 7), True, True, False)
        source.close()
        path.chmod(0o444)
        try:
            reopened = team.open_review(self.own, str(path))
            self.addCleanup(reopened.close)
            self.assert_state(reopened.get(0, 7), True, True, False)
            self.assert_state(reopened.get(1, 16), True, False, True)
            self.assertEqual(path.read_bytes(), original)
        finally:
            path.chmod(0o644)

    def test_all_four_visited_judgements_conflict_only_when_tuples_differ(self):
        for left in range(4):
            for right in range(4):
                ei = left * 4 + right
                self.state(self.own, 0, ei, visited=True,
                           waived=bool(left & 1), not_required=bool(left & 2))
                self.state(self.peer, 0, ei, visited=True,
                           waived=bool(right & 1), not_required=bool(right & 2))
        _, alice = self.export(self.own, "alice.review", "alice")
        _, bob = self.export(self.peer, "bob.review", "bob")
        result = self.aggregate([alice, bob])
        self.assertEqual(result.counts(0)["visited"], 16)
        self.assertEqual(result.counts(0)["conflicts"], 12)
        self.assertEqual(result.counts()["reviewers"], 2)
        self.assertEqual(result.counts(2)["total"], 0)
        wanted = []
        for left in range(4):
            for right in range(4):
                ei = left * 4 + right
                conflict = left != right
                self.assert_state(result.get(0, ei), True,
                    bool(left & 1) if not conflict else False,
                    bool(left & 2) if not conflict else False, conflict)
                if conflict:
                    wanted.append(ei)
        self.assertEqual(result.page(0, 0, 1000, kind="conflict"), wanted)

    def test_unvisited_defaults_and_legacy_judgements_are_not_conflict_votes(self):
        # Only one reviewer has visited these errors. A second reviewer's
        # default or preexisting non-visited judgement is not another vote.
        self.state(self.peer, 0, 0, visited=True, waived=True, not_required=True)
        self.state(self.own, 0, 1, visited=True)
        self.state(self.peer, 0, 1, waived=True, not_required=True)
        self.state(self.own, 0, 2, waived=True)
        self.state(self.peer, 0, 2, not_required=True)
        _, alice = self.export(self.own, "alice.review", "alice")
        _, bob = self.export(self.peer, "bob.review", "bob")
        result = self.aggregate([alice, bob])
        self.assertEqual(result.counts()["conflicts"], 0)
        self.assertEqual(result.counts()["visited"], 2)
        self.assertEqual(result.counts()["waived"], 3)
        self.assertEqual(result.counts()["not_required"], 3)
        self.assertEqual(result.counts()["pending"], sum(SIZES) - 3)
        self.assert_state(result.get(0, 2), False, True, True, False)

    def test_duplicate_reviewer_sources_deduplicate_and_latest_snapshot_wins(self):
        self.state(self.peer, 0, 7, visited=True, waived=True)
        first_path, first = self.export(self.peer, "bob-first.review", "bob")
        copied_path = self.folder / "bob-copy.review"
        shutil.copy2(first_path, copied_path)
        copied = team.open_review(self.own, str(copied_path))
        self.addCleanup(copied.close)
        repeated = self.aggregate([first, copied, first])
        self.assertEqual(repeated.counts()["reviewers"], 1)
        self.assertEqual(repeated.counts()["visited"], 1)
        self.assertEqual(repeated.counts()["conflicts"], 0)
        self.state(self.peer, 0, 7, not_required=True)
        _, latest = self.export(self.peer, "bob-latest.review", "bob")
        for sources in ([first, latest], [latest, copied, first]):
            with self.subTest(order=[id(source) for source in sources]):
                result = self.aggregate(sources)
                self.assertEqual(result.counts()["reviewers"], 1)
                self.assertEqual(result.counts()["visited"], 1)
                self.assert_state(result.get(0, 7), True, False, True, False)

    def test_current_own_state_overrides_older_snapshot_with_same_reviewer(self):
        self.state(self.own, 0, 7, waived=True)
        _, old = self.export(self.own, "alice-old.review", "alice")
        self.state(self.own, 0, 7)
        result = self.aggregate([old], include_own=True)
        self.assertEqual(result.counts()["reviewers"], 1)
        self.assertEqual(result.counts()["work"], 0)
        self.assert_state(result.get(0, 7), False, False, False, False)

    def test_duplicate_reviewer_equal_timestamp_tie_is_order_independent(self):
        self.state(self.peer, 0, 7, visited=True, waived=True)
        with patch.object(team.time, "time_ns", return_value=123456789):
            _, first = self.export(self.peer, "bob-tie-first.review", "bob")
            self.state(self.peer, 0, 7, not_required=True)
            _, second = self.export(self.peer, "bob-tie-second.review", "bob")
        winner = max((first, second), key=lambda source: source.snapshot_id)
        expected = winner.get(0, 7)
        for sources in ([first, second], [second, first]):
            result = self.aggregate(sources)
            self.assertEqual(result.counts()["reviewers"], 1)
            self.assert_state(result.get(0, 7), expected.visited,
                              expected.waived, expected.not_required, False)

    def test_safe_import_preserves_own_visits_and_skips_different_visited_judgements(self):
        self.state(self.own, 0, 0, visited=True)
        self.state(self.peer, 0, 0, visited=True, waived=True)
        self.state(self.own, 0, 1, not_required=True)
        self.state(self.peer, 0, 1, visited=True, waived=True)
        self.state(self.own, 0, 2, waived=True)
        self.state(self.peer, 0, 2, visited=True)
        self.state(self.own, 0, 3, visited=True, not_required=True)
        self.state(self.peer, 0, 3, visited=True, not_required=True)
        path, source = self.export(self.peer, "bob.review", "bob")
        original = path.read_bytes()
        with patch.object(drc.IcePack, "_block", side_effect=AssertionError("geometry decoded")):
            report = team.apply_review(self.own, source)
        self.assertEqual(report["conflicts_skipped"], 1)
        self.assertEqual(report["changed_waived"], 2)
        self.assertEqual(report["changed_not_required"], 1)
        self.assertEqual(self.own.review_flags(0, 0), 1)
        self.assertEqual(self.own.get_status(0, 0), 0)
        self.assertEqual(self.own.review_flags(0, 1), 0,
                         "import copied another reviewer's visit into my history")
        self.assertEqual(self.own.get_status(0, 1), drc.STATUS_WAIVED)
        self.assertEqual(self.own.get_status(0, 2), 0)
        self.assertEqual(self.own.review_flags(0, 3), 3)
        self.assertEqual(path.read_bytes(), original, "import modified peer snapshot")
        second = team.apply_review(self.own, source)
        self.assertEqual(second["conflicts_skipped"], 1)
        self.assertEqual(second["changed_waived"], 0)
        self.assertEqual(second["changed_not_required"], 0)

    def test_explicit_force_applies_selected_conflicts_and_preserves_own_visits(self):
        for ei in (3, 4):
            self.state(self.own, 0, ei, visited=True, waived=True, not_required=True)
            self.state(self.peer, 0, ei, visited=True)
        _, source = self.export(self.peer, "bob.review", "bob")
        selected = Selection.from_indices(SIZES[0], [3])
        report = team.apply_review(self.own, source, ci=0, membership=selected, force=True)
        self.assertEqual(report["changed_waived"], 1)
        self.assertEqual(report["changed_not_required"], 1)
        self.assertEqual(report["conflicts_skipped"], 0)
        self.assertEqual(self.own.review_flags(0, 3), 1)
        self.assertEqual(self.own.get_status(0, 3), 0)
        self.assertEqual(self.own.review_flags(0, 4), 3)
        self.assertEqual(self.own.get_status(0, 4), drc.STATUS_WAIVED)

    def test_unvisited_peer_judgement_is_not_vote_but_cannot_overwrite_my_visit(self):
        self.state(self.own, 0, 7, visited=True)
        self.state(self.peer, 0, 7, waived=True)
        _, source = self.export(self.peer, "bob-legacy-judgement.review", "bob")
        aggregate = self.aggregate([source], include_own=True)
        self.assertEqual(aggregate.counts()["conflicts"], 0,
                         "unvisited peer judgement became a conflict vote")
        self.assertEqual(aggregate.counts()["visited"], 1)
        report = team.apply_review(self.own, source)
        self.assertEqual(report["conflicts_skipped"], 1,
                         "safe import allowed an unvisited peer to overwrite my reviewed judgement")
        self.assertEqual(report["changed_waived"], 0)
        self.assertEqual(self.own.review_flags(0, 7), 1)
        self.assertEqual(self.own.get_status(0, 7), drc.STATUS_NONE)
        conflicts = team.import_conflicts(self.own, source)
        self.addCleanup(conflicts.close)
        self.assertEqual(conflicts.counts()["conflicts"], 1)
        self.assertEqual(conflicts.page(0, 0, 1000, kind="conflict"), [7])

    def test_legacy_waive_and_unpaired_review_import_preserve_unknown_dimensions(self):
        self.state(self.own, 0, 7, not_required=True)
        self.state(self.peer, 0, 7, waived=True)
        legacy = team.open_review(self.own, self.peer._waive_path, reviewer="bob")
        self.addCleanup(legacy.close)
        self.assertFalse(legacy.visited_known)
        self.assertFalse(legacy.not_required_known)
        team.apply_review(self.own, legacy)
        self.assertEqual(self.own.get_status(0, 7), drc.STATUS_WAIVED)
        self.assertEqual(self.own.review_flags(0, 7), 2,
                         "legacy waive import cleared an unknown NR dimension")
        self.state(self.peer, 0, 7, visited=True)
        raw = self.unpaired_review()
        self.assertFalse(raw.waive_known)
        self.assertTrue(raw.visited_known)
        self.assertTrue(raw.not_required_known)
        team.apply_review(self.own, raw)
        self.assertEqual(self.own.get_status(0, 7), drc.STATUS_WAIVED,
                         "unpaired review import cleared an unknown waive dimension")
        self.assertEqual(self.own.review_flags(0, 7), 0)

    def test_raw_review_votes_only_known_nr_and_pairing_adds_waive_vote(self):
        self.state(self.own, 0, 7, visited=True)
        self.state(self.peer, 0, 7, visited=True, waived=True)
        raw = self.unpaired_review()
        paired = team.open_review(self.own, self.peer.review_path,
                                  waive_path=self.peer._waive_path, reviewer="bob")
        self.addCleanup(paired.close)
        self.assertEqual(self.aggregate([raw], include_own=True).counts()["conflicts"], 0)
        self.assertEqual(self.aggregate([paired], include_own=True).counts()["conflicts"], 1)
        self.state(self.own, 0, 7, visited=True, not_required=True)
        self.assertEqual(self.aggregate([raw], include_own=True).counts()["conflicts"], 1,
                         "known NR disagreement was ignored because W is unknown")

    def test_work_paging_masks_rank_and_conflicts_keep_original_ids(self):
        ids = list(range(1003)) + [4095, 4096, 4112]
        for ei in ids:
            self.peer.mark_visited(0, ei)
        for ei in (999, 1000, 4095, 4096):
            self.state(self.own, 0, ei, visited=True, waived=True)
        _, alice = self.export(self.own, "alice.review", "alice")
        _, bob = self.export(self.peer, "bob.review", "bob")
        result = self.aggregate([alice, bob])
        self.assertEqual(result.page(0, 0, 1000), ids[:1000])
        self.assertEqual(result.page(0, 1000, 1000), ids[1000:])
        self.assertEqual(result.page(0, 2000, 1000), [])
        members = result.members(0, kind="work")
        self.assertEqual(members.count(), len(ids))
        self.assertEqual(members.page(995, 10), ids[995:1005])
        self.assertEqual(members.rank(4096), ids.index(4096))
        self.assertIsNone(members.rank(4000))
        self.assertTrue(members.contains(4095))
        self.assertFalse(members.contains(4000))
        self.assertEqual(list(members.mask(4094, 5)), [False, True, True, False, False])
        self.assertEqual(result.page(0, 0, 1000, kind="conflict"), [999, 1000, 4095, 4096])

    def test_snapshot_rejects_different_pack_and_failed_export_preserves_destination(self):
        self.peer.mark_visited(0, 7)
        path, source = self.export(self.peer, "bob.review", "bob")
        original = path.read_bytes()
        changed_path = self.folder / "changed.tray"
        shutil.copy2(self.path, changed_path)
        stat = changed_path.stat()
        os.utime(changed_path, ns=(stat.st_atime_ns, stat.st_mtime_ns + 1_000_000))
        changed_db = self.database("charlie", changed_path)
        with self.assertRaises(ValueError):
            team.open_review(changed_db, str(path))
        with self.assertRaises(ValueError):
            team.aggregate_reviews(changed_db, [source], include_own=False)
        with self.assertRaises(ValueError):
            team.apply_review(changed_db, source)
        with self.assertRaises(InterruptedError):
            team.export_review(self.peer, str(path), reviewer="bob", cancelled=lambda: True)
        self.assertEqual(path.read_bytes(), original)
        self.assert_state(source.get(0, 7), True, False, False)
        with patch.object(team.os, "replace", side_effect=OSError("injected export replace")):
            with self.assertRaisesRegex(OSError, "injected export"):
                team.export_review(self.peer, str(path), reviewer="bob")
        self.assertEqual(path.read_bytes(), original)
        self.assertEqual(list(self.folder.glob(".review-snapshot-*")), [])

    def test_derived_aggregate_cannot_be_reused_as_a_reviewer_source(self):
        self.peer.mark_visited(0, 7)
        _, source = self.export(self.peer, "bob.review", "bob")
        result = self.aggregate([source])
        path = Path(result.path)
        original = path.read_bytes()
        with self.assertRaises(ValueError):
            team.open_review(self.own, str(path))
        with self.assertRaises(ValueError):
            team.aggregate_reviews(self.own, [result], include_own=False)
        with self.assertRaises(ValueError):
            team.apply_review(self.own, result)
        self.assertEqual(path.read_bytes(), original,
                         "rejected aggregate source was modified")
        self.assertEqual(self.own.review_counts()["visited"], 0)

    def test_corrupt_snapshot_counters_are_rejected_instead_of_returning_zero(self):
        self.peer.mark_visited(0, 7)
        path, source = self.export(self.peer, "bob.review", "bob")
        original = path.read_bytes()
        for name, assignments in (
                ("zeroed", "v=0,w=0,n=0,c=0,a=0,done=0"),
                ("contradictory", "v=1,w=0,n=0,c=0,a=0,done=1")):
            with self.subTest(corruption=name):
                damaged = self.folder / (name + ".review")
                shutil.copy2(path, damaged)
                with closing(sqlite3.connect(str(damaged))) as connection, connection:
                    connection.execute("UPDATE rules SET " + assignments + " WHERE ci=0")
                damaged_bytes = damaged.read_bytes()
                with self.assertRaises(ValueError):
                    team.open_review(self.own, str(damaged))
                self.assertEqual(damaged.read_bytes(), damaged_bytes,
                                 "reader rewrote rejected snapshot")
                self.assertEqual(path.read_bytes(), original)
                self.assertEqual(source.counts()["visited"], 1)


class ConflictSelectionTests(unittest.TestCase):
    """Direct sparse conflict fixtures isolate paging/lifetime from voting."""

    def aggregate(self, ids, size=131_083):
        owner = tempfile.TemporaryDirectory(prefix="floe-conflict-fixture-")
        self.addCleanup(owner.cleanup)
        status_path = Path(owner.name) / "status.bin"
        with status_path.open("wb") as stream:
            stream.truncate(size)
        statuses = np.memmap(status_path, mode="r+", dtype=np.uint8, shape=(size,))
        self.addCleanup(statuses._mmap.close)
        db = SimpleNamespace(_review_fingerprint="conflict-fixture",
                             _ecnt=np.array([size]), _dir_es=np.array([0]),
                             _status=statuses, _status_generation=0)
        path = Path(owner.name) / "aggregate.review"
        writer = team._Writer(db, path, "team", metadata={"aggregate": True})
        chunks = {}
        for ei in ids:
            chunks[ei // 4096] = chunks.get(ei // 4096, 0) | (1 << (ei % 4096))
        try:
            for k, bits in sorted(chunks.items()):
                writer.add(0, k, bits, 0, 0, bits, bits)
            writer.finish()
        finally:
            writer.close()
        source = team.ReviewAggregate(db, path, owner=owner)
        self.addCleanup(source.close)
        return db, source

    def prepare(self, db, source, **kwargs):
        result = team.prepare_conflict_selection(db, source, 0, **kwargs)
        if result._owner is not None:
            self.addCleanup(result._owner.cleanup)
        return result

    def test_thousand_item_pages_and_worker_protocol_preserve_gaps(self):
        ids = list(range(0, 3003, 3)) + [4095, 4096, 65535, 65536, 131082]
        db, source = self.aggregate(ids)
        result = self.prepare(db, source)
        self.assertEqual(result.count(), len(ids))
        self.assertEqual(result.page(0, 1000), ids[:1000])
        self.assertEqual(result.page(1000, 1000), ids[1000:])
        self.assertEqual(result.page(2000, 1000), [])
        self.assertEqual(result.rank(65536), ids.index(65536))
        self.assertIsNone(result.rank(5000))
        np.testing.assert_array_equal(result.mask(4094, 4), [False, True, True, False])
        with tempfile.TemporaryDirectory() as directory, \
                patch.object(np, "save", side_effect=AssertionError("membership copied")), \
                patch.object(np.lib.format, "open_memmap", side_effect=AssertionError("full mask allocated")):
            spec = export_members(result, directory, count=result.size)
            self.assertEqual(spec["kind"], "packed-bits")
            self.assertTrue(os.path.samefile(spec["array"]["path"], result.bits.filename))
            mapped = import_members(spec)
            np.testing.assert_array_equal(mapped.mask(65534, 5), result.mask(65534, 5))
            self.assertTrue(mapped.contains(131082))

    def test_cluster_and_current_waive_filters_intersect_conflicts(self):
        db, source = self.aggregate([3, 4, 4095, 4096, 65536, 131082])
        db._status[[4, 4096, 131082]] = drc.STATUS_WAIVED
        member = Selection.from_indices(131083, [3, 4, 4000, 4096, 131082])
        self.assertEqual(self.prepare(db, source, membership=member).page(0, 1000),
                         [3, 4, 4096, 131082])
        self.assertEqual(self.prepare(db, source, membership=member, waived=True).page(0, 1000),
                         [4, 4096, 131082])
        self.assertEqual(self.prepare(db, source, membership=member, waived=False).page(0, 1000), [3])
        self.assertEqual(self.prepare(db, source, waived=False).page(0, 1000), [3, 4095, 65536])

    def test_empty_conflicts_do_not_open_sql_or_allocate_rule_bitmap(self):
        db, source = self.aggregate([], size=100_000_003)
        progress = []
        with patch.object(team, "_connect", side_effect=AssertionError("empty SQL read")), \
                patch.object(team.tempfile, "TemporaryDirectory", side_effect=AssertionError("empty output file")), \
                patch.object(np.lib.format, "open_memmap", side_effect=AssertionError("empty bitmap")):
            lease = source.lease()
            try:
                result = self.prepare(db, lease, progress=lambda *value: progress.append(value))
            finally:
                lease.close()
        self.assertFalse(result)
        self.assertIsNone(result._owner)
        self.assertEqual(progress, [(100_000_003, 100_000_003)])

    def test_lease_survives_source_close_and_selection_survives_lease_close(self):
        db, source = self.aggregate([7, 4096, 131082])
        path = Path(source.path)
        with patch.object(team, "_connect", side_effect=AssertionError("lease opened SQL")):
            lease = source.lease()
        source.close()
        self.assertTrue(path.exists())
        result = self.prepare(db, lease)
        lease.close()
        self.assertFalse(path.exists())
        self.assertEqual(result.page(0, 1000), [7, 4096, 131082])
        with self.assertRaises(OSError):
            source.lease()
        with self.assertRaises(OSError):
            lease.lease()

    def test_source_close_does_not_wait_for_background_mask_work(self):
        db, source = self.aggregate([7, 4096])
        entered, release = threading.Event(), threading.Event()
        outcomes = []

        def mask(start, count):
            entered.set()
            if not release.wait(2):
                raise AssertionError("foreground close waited for worker")
            return np.ones(count, dtype=bool)

        def run():
            try:
                outcomes.append(team.prepare_conflict_selection(
                    db, source, 0, membership=SimpleNamespace(size=131083, mask=mask)))
            except Exception as exc:
                outcomes.append(exc)

        worker = threading.Thread(target=run)
        worker.start()
        try:
            self.assertTrue(entered.wait(2))
            started = time.perf_counter()
            source.close()
            self.assertLess(time.perf_counter() - started, 0.2)
            self.assertTrue(Path(source.path).exists())
        finally:
            release.set()
            worker.join(3)
        self.assertFalse(worker.is_alive())
        self.assertEqual(len(outcomes), 1)
        result = outcomes[0]
        self.assertIsInstance(result, Selection)
        self.addCleanup(result._owner.cleanup)
        self.assertFalse(Path(source.path).exists())
        self.assertEqual(result.page(0, 1000), [7, 4096])

    def test_cancel_mask_error_and_progress_error_clean_partial_files(self):
        db, source = self.aggregate([7, 4096, 65536])
        factory = tempfile.TemporaryDirectory
        for mode in ("cancel", "mask", "progress"):
            with self.subTest(mode=mode):
                paths, stop = [], [False]

                def temporary(*args, **kwargs):
                    owner = factory(*args, **kwargs)
                    paths.append(Path(owner.name))
                    return owner

                def mask(start, count):
                    stop[0] = True
                    return np.ones(count - (mode == "mask"), dtype=bool)

                def progress(*_args):
                    if mode == "progress":
                        raise RuntimeError("callback failed")

                expected = {"cancel": InterruptedError, "mask": ValueError, "progress": RuntimeError}[mode]
                with patch.object(team.tempfile, "TemporaryDirectory", side_effect=temporary):
                    with self.assertRaises(expected):
                        team.prepare_conflict_selection(db, source, 0,
                            membership=SimpleNamespace(size=131083, mask=mask),
                            cancelled=lambda: mode == "cancel" and stop[0], progress=progress)
                self.assertTrue(paths)
                self.assertTrue(all(not path.exists() for path in paths))
                self.assertEqual(source.counts(0)["conflicts"], 3)

    def test_status_change_rejects_mixed_waive_filter(self):
        db, source = self.aggregate([7, 4096])

        def mask(start, count):
            db._status_generation += 2
            return np.ones(count, dtype=bool)

        with self.assertRaisesRegex(InterruptedError, "waive status changed"):
            self.prepare(db, source, waived=False,
                         membership=SimpleNamespace(size=131083, mask=mask))
        db._status_generation = 1
        with patch.object(team.tempfile, "TemporaryDirectory", side_effect=AssertionError("busy allocated")):
            with self.assertRaisesRegex(InterruptedError, "waive status is changing"):
                self.prepare(db, source, waived=True)

    def test_hundred_million_scope_reads_only_sparse_conflict_chunks(self):
        size = 100_000_003
        db, source = self.aggregate([7, 4096, size - 1], size=size)
        calls = []

        def mask(start, count):
            calls.append((start, count))
            self.assertLessEqual(count, 4096)
            return np.ones(count, dtype=bool)

        started = time.perf_counter()
        with patch.object(np, "arange", side_effect=AssertionError("enumerated all IDs")), \
                patch.object(drc.IcePack, "_block", side_effect=AssertionError("geometry decoded")):
            result = self.prepare(db, source, membership=SimpleNamespace(size=size, mask=mask))
        elapsed = time.perf_counter() - started
        self.assertEqual(len(calls), 3)
        self.assertEqual(result.page(0, 1000), [7, 4096, size - 1])
        self.assertEqual(result.bits.nbytes, (size + 7) // 8)
        self.assertLess(Path(result.bits.filename).stat().st_size, 12_501_000)
        print("\n100M conflict preparation: %.4fs, %d conflict chunks, %d logical bytes" %
              (elapsed, len(calls), result.bits.nbytes), flush=True)


class LargeVirtualTeamTests(unittest.TestCase):
    def test_hundred_million_sparse_rule_uses_bounded_arrays_and_no_geometry(self):
        with tempfile.TemporaryDirectory(prefix="floe-team-large-") as folder:
            folder = Path(folder)
            size = 100_000_003
            status_path = folder / "sparse-status.bin"
            with status_path.open("wb") as stream:
                stream.truncate(size)
            statuses = np.memmap(status_path, mode="r+", dtype=np.uint8, shape=(size,))
            statuses[size - 1] = drc.STATUS_WAIVED
            store = ReviewStore(str(folder / "virtual.tray"), (size,), "virtual-fixture",
                                path=str(folder / "virtual-review.sqlite3"), reviewer="virtual")
            try:
                store.mark_visited(0, 7)
                store.mark_visited(0, size - 1)
                store.set_members(0, Selection.from_indices(size, [4096]), True)
                db = SimpleNamespace(path=str(folder / "virtual.tray"),
                    _review_fingerprint="virtual-fixture", _ecnt=np.array([size]),
                    _dir_es=np.array([0]), _status=statuses, _status_lock=threading.RLock(),
                    total=size, _review_flags_store=lambda: store, _reviewer="virtual")
                source_path = folder / "virtual-snapshot.review"
                original_arange = np.arange

                def bounded_arange(*args, **kwargs):
                    if len(args) == 1:
                        length = int(args[0])
                    elif len(args) >= 2:
                        length = max(0, int(args[1]) - int(args[0]))
                    else:
                        length = 0
                    if length > 65536:
                        raise AssertionError("team operation enumerated all error IDs")
                    return original_arange(*args, **kwargs)

                with patch.object(np, "arange", side_effect=bounded_arange), \
                        patch.object(drc.IcePack, "_block", side_effect=AssertionError("geometry decoded")):
                    source = team.export_review(db, str(source_path), reviewer="virtual")
                    try:
                        result = team.aggregate_reviews(db, [source], include_own=False)
                        try:
                            self.assertEqual(result.counts()["total"], size)
                            self.assertEqual(result.counts()["visited"], 2)
                            self.assertEqual(result.counts()["work"], 3)
                            self.assertEqual(result.counts()["conflicts"], 0)
                            self.assertEqual(result.page(0, 0, 1000), [7, 4096, size - 1])
                            self.assertEqual(result.members(0).rank(size - 1), 2)
                        finally:
                            result.close()
                        # Exercise the importer on the same virtual universe.
                        # Only bounded touched spans may reach the status writer.
                        statuses[size - 1] = drc.STATUS_NONE
                        store.set_members(0, Selection.from_indices(size, [4096]), False)
                        spans = []

                        def write_span(ci, start, before, after):
                            self.assertEqual(ci, 0)
                            self.assertLessEqual(len(after), 4096)
                            np.testing.assert_array_equal(statuses[start:start + len(before)], before)
                            statuses[start:start + len(after)] = after
                            spans.append((start, len(after)))

                        db._write_status_span = write_span
                        applied = team.apply_review(db, source, force=True)
                        self.assertEqual(applied["changed_waived"], 1)
                        self.assertEqual(applied["changed_not_required"], 1)
                        self.assertEqual(applied["conflicts_skipped"], 0)
                        self.assertEqual(len(spans), 1)
                        self.assertEqual(store.counts()["visited"], 2)
                        self.assertEqual(int(statuses[size - 1]), drc.STATUS_WAIVED)
                    finally:
                        source.close()
                self.assertLess(source_path.stat().st_size, 2_000_000,
                                "three reviewed errors created a rule-sized snapshot")
            finally:
                store.close()
                statuses._mmap.close()


if __name__ == "__main__":
    unittest.main(verbosity=2)
