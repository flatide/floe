"""Headless contracts for reviewer comparison, protected application and paging."""

import os
from pathlib import Path
import sys
import tempfile
import threading
import time
import unittest
from types import SimpleNamespace
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from floe import gui, drc_team as team  # noqa: E402
from floe.drc_selection import Selection  # noqa: E402


def fixture():
    db = SimpleNamespace(checks=[SimpleNamespace(name="RULE", errors=range(100_000_000))],
                         review_flags=Mock(return_value=1), get_status=Mock(return_value=0))
    viewer = SimpleNamespace(_drc=db, _drc_visit_worker=None, _drc_team_write=None,
        _drc_clusters=None, _drc_jump=Mock(), _drc_team_changed=Mock(),
        _set_live_status=Mock(), _drc_review_pending=lambda: False)
    window = gui._DrcTeamWindow.__new__(gui._DrcTeamWindow)
    window.viewer, window.db = viewer, db
    window.sources, window.aggregate = [], None
    window.import_conflicts, window.import_source = None, None
    window.closed, window.job, window.page, window._guard = False, None, 0, False
    window.controls = [Mock(), Mock()]
    for name in ("tree", "status", "stats", "peer", "rule", "mode", "prev", "next",
                 "page_label", "source_label", "window"):
        setattr(window, name, Mock())
    window.peer.get_active.return_value = 0
    window.rule.get_active_id.return_value = "0"
    window.mode.get_active_id.return_value = "work"
    window.store = []
    window.refreshed_at = "12:34:56"
    viewer._drc_team = window
    return window


def finish(window):
    deadline = time.monotonic() + 3
    while window.job is not None and not window.job["done"]:
        if time.monotonic() >= deadline:
            raise AssertionError("review worker did not finish")
        time.sleep(0.002)
    window._poll()


def counts(total=100_000_000, work=1001):
    return dict(total=total, visited=work, waived=0, not_required=0,
                pending=total-work, conflicts=0, work=work, reviewers=2)


def conflict_fixture():
    from validate_drc_delta_gui import fixture as grid_fixture
    viewer = grid_fixture()
    viewer._drc = SimpleNamespace(checks=[SimpleNamespace(name="RULE", errors=range(100_000_000))],
                                  get_status=Mock(return_value=0))
    viewer._drc_delta_group = viewer._drc_delta_groups = None
    viewer._drc_rmeta = None
    viewer._drc_team = None
    viewer._drc_team_dialog = Mock(return_value=None)
    viewer._drcwin._conflict = Mock()
    viewer._drcwin._conflict_info = Mock()
    viewer._drc_conflict_only = False
    viewer._drc_conflict_source = None
    viewer._drc_conflict_members = None
    viewer._drc_conflict_key = None
    viewer._drc_conflict_job = None
    viewer._drc_conflict_revision = 0
    viewer._drc_conflict_error = None
    viewer._drc_conflict_stamp = None
    viewer._drc_grid_fill = gui.Viewer._drc_grid_fill.__get__(viewer)
    return viewer


class ConflictFilterTests(unittest.TestCase):
    def setUp(self):
        self.glib = patch.object(gui, "GLib", SimpleNamespace(timeout_add=Mock(),
            idle_add=Mock(), markup_escape_text=lambda value: value))
        self.glib.start()
        self.addCleanup(self.glib.stop)

    def ready(self, ids, *, waived="all"):
        viewer = conflict_fixture()
        viewer._drc_conflict_only = True
        viewer._drc_wfilter = waived
        viewer._drc_conflict_members = Selection.from_indices(100_000_000, ids)
        # Exercise actual consumers with one already published scope. Async
        # generation/publish contracts are checked separately below.
        viewer._drc_conflict_sync = Mock(return_value=viewer._drc_conflict_members)
        return viewer

    def test_main_grid_keeps_original_ids_and_1000_pages_without_geometry(self):
        ids = list(range(3, 2003, 2)) + [99_999_999]
        viewer = self.ready(ids)
        viewer._drc_grid_fill(0)
        self.assertEqual(viewer._drc_grid_map, ids[:1000])
        viewer._drcwin._pnext.set_sensitive.assert_called_with(True)
        viewer._drc_page_step(1)
        self.assertEqual(viewer._drc_grid_map, [99_999_999])
        self.assertEqual(viewer._drc_page, 1)
        viewer._drcwin._pnext.set_sensitive.assert_called_with(False)
        self.assertIn("100000000", viewer._drcwin._gstore[0][0])
        self.assertEqual(viewer._drc_step_ei(viewer._drc, 0, viewer._drc_grid_base,
                                           99_999_999, 1), 3)
        self.assertEqual(viewer._drc_step_ei(viewer._drc, 0, viewer._drc_grid_base,
                                           3, -1), 99_999_999)

    def test_resolved_waive_scope_pages_without_reapplying_selection_status(self):
        viewer = self.ready([3, 9, 99_999_999], waived="waived")
        viewer._drc.get_status.return_value = 1
        viewer._drc_grid_fill(0)
        self.assertEqual(viewer._drc_grid_map, [3, 9, 99_999_999])
        self.assertIsNone(viewer._drc_grid_base[2])
        self.assertEqual(viewer._drc_step_ei(viewer._drc, 0, viewer._drc_grid_base, 9, 1),
                         99_999_999)

    def test_marker_and_box_selection_receive_the_same_conflict_membership(self):
        viewer = self.ready([7, 99_999_999])
        viewer._drc_marker_request((0, 0, 10000, 10000), 10, 100, 100)
        query = viewer._drc_marker_worker.submit.call_args.args[2]
        self.assertIs(query["members"][0], viewer._drc_conflict_members)
        viewer._drc_display_view = Mock(return_value=((0, 0, 10000, 10000), 10))
        viewer._drc_selection_submit = Mock()
        viewer._esel_apply((0, 0), (100, 100))
        self.assertIs(viewer._drc_selection_submit.call_args.args[2],
                      viewer._drc_conflict_members)
        # Panning must reuse the membership rather than copy/scan it again.
        viewer._drc_marker_request((100, 0, 10100, 10000), 10, 100, 100)
        self.assertIs(viewer._drc_marker_worker.submit.call_args.args[2]["members"][0],
                      query["members"][0])

    def test_in_view_query_receives_conflicts_before_cap_and_selected_intersects(self):
        viewer = self.ready([5, 17, 99_999_999])
        error = SimpleNamespace(kind="e", pts=[(0, 0), (10, 10)])
        viewer._drc.query_rect = Mock(return_value=[(0, 5, error), (0, 99_999_999, error)])
        viewer._drc_hl = True
        selected = Selection.from_indices(100_000_000, [17, 99_999_999])
        viewer._drc_sel = (0, selected, (), selected)
        viewer._drc_show_sel = True
        viewer._drc_grid_fill(0)
        self.assertEqual(viewer._drc_grid_map, [99_999_999])
        kwargs = viewer._drc.query_rect.call_args.kwargs
        self.assertIs(kwargs["members"][0], viewer._drc_conflict_members)
        self.assertEqual(kwargs["cap"], gui.DRC_HL_CAP)

    @staticmethod
    def wait_job(job):
        deadline = time.monotonic() + 3
        while not job["done"]:
            if time.monotonic() >= deadline:
                raise AssertionError("conflict preparation did not finish")
            time.sleep(0.002)

    def test_enabling_without_snapshot_shows_empty_and_off_restores_full_page(self):
        viewer = conflict_fixture()
        viewer._drc_waive_load_dialog = Mock()
        selected = Selection.from_indices(100_000_000, [11, 99_999_999])
        viewer._drc_sel = (0, selected, (), selected)
        viewer._drc_sels = {viewer._drc_scope_key(): viewer._drc_sel}
        viewer._drc_page = 4
        button = Mock()
        button.get_active.return_value = True
        viewer._on_drc_conflict(button)
        self.assertEqual(viewer._drc_grid_map, [])
        self.assertEqual(viewer._drc_page, 0)
        self.assertIsNone(viewer._drc_sel)
        self.assertEqual(viewer._drc_sels, {})
        viewer._drc_waive_load_dialog.assert_called_once()
        self.assertIn("load peer work", viewer._drcwin._conflict_info.set_text.call_args.args[0])
        button.get_active.return_value = False
        viewer._on_drc_conflict(button)
        self.assertEqual(viewer._drc_grid_map, list(range(1000)))

    def test_scope_switch_clears_previous_error_geometry_and_preserves_manual_ruler(self):
        viewer = conflict_fixture()
        viewer._drc_waive_load_dialog = Mock()
        viewer.drc_mark = dict(kind="e", pts=[(0, 0), (1, 1)])
        viewer._drc_pos = 73
        cd, manual = object(), object()
        viewer._drc_ruler, viewer.rulers = [cd], [cd, manual]
        button = Mock()
        button.get_active.return_value = True
        viewer._on_drc_conflict(button)
        self.assertIsNone(viewer.drc_mark)
        self.assertEqual(viewer._drc_pos, -1)
        self.assertEqual(viewer._drc_ruler, [])
        self.assertEqual(viewer.rulers, [manual])

    def test_preparation_is_async_empty_while_pending_and_reused_for_paging(self):
        viewer = conflict_fixture()
        viewer._drc_conflict_only = True
        lease = SimpleNamespace(close=Mock())
        viewer._drc_conflict_source = SimpleNamespace(lease=lambda: lease)
        entered, release = threading.Event(), threading.Event()
        prepared = Selection.from_indices(100_000_000, list(range(1000)) + [99_999_999])
        caller = threading.get_ident()
        def work(*args, **kwargs):
            self.assertNotEqual(threading.get_ident(), caller)
            entered.set()
            release.wait(2)
            return prepared
        with patch.object(team, "prepare_conflict_selection", side_effect=work) as build:
            try:
                viewer._drc_grid_fill(0)
                self.assertTrue(entered.wait(1))
                job = viewer._drc_conflict_job
                self.assertEqual(viewer._drc_grid_map, [])
                viewer._drc_marker_request((0, 0, 10000, 10000), 10, 100, 100)
                viewer._drc_marker_worker.submit.assert_not_called()
            finally:
                release.set()
            self.wait_job(job)
            viewer._drc_conflict_poll()
            self.assertEqual(viewer._drc_grid_map, list(range(1000)))
            viewer._drc_page_step(1)
            self.assertEqual(viewer._drc_grid_map, [99_999_999])
            self.assertIs(viewer._drc_active_members(), prepared)
            self.assertEqual(build.call_count, 1)
        lease.close.assert_called_once()

    def test_filter_change_rejects_late_old_result_and_clears_old_selection(self):
        viewer = conflict_fixture()
        viewer._drc_conflict_only = True
        viewer._drc_conflict_source = SimpleNamespace(lease=lambda: SimpleNamespace(close=Mock()))
        entered, release = threading.Event(), threading.Event()
        def work(*args, **kwargs):
            if kwargs["waived"] is None:
                entered.set()
                release.wait(2)
                return Selection.from_indices(100_000_000, [5])
            return Selection.from_indices(100_000_000, [17])
        with patch.object(team, "prepare_conflict_selection", side_effect=work):
            try:
                viewer._drc_grid_fill(0)
                self.assertTrue(entered.wait(1))
                old_job = viewer._drc_conflict_job
                selected = Selection.from_indices(100_000_000, [5, 99_999_999])
                viewer._drc_sel = (0, selected, (), selected)
                viewer._drc_show_sel = True
                viewer._drc_wfilter = "waived"
                viewer._drc.get_status.return_value = 1
                viewer._drc_grid_fill(0)
                self.assertTrue(old_job["cancel"].is_set())
                self.assertIsNone(viewer._drc_sel)
                self.assertEqual(viewer._drc_grid_map, [])
                new_job = viewer._drc_conflict_job
                self.wait_job(new_job)
                viewer._drc_conflict_poll()
            finally:
                release.set()
            self.wait_job(old_job)
            viewer._drc_conflict_poll()
            self.assertEqual(viewer._drc_active_members().page(0, 1000), [17])
            self.assertEqual(viewer._drc_grid_map, [], "selected-only has no retained picks")
            viewer._drc_show_sel = False
            viewer._drc_grid_fill(0)
            self.assertEqual(viewer._drc_grid_map, [17])

    def test_failure_stays_empty_is_reported_and_does_not_retry_every_frame(self):
        viewer = conflict_fixture()
        viewer._drc_conflict_only = True
        viewer._drc_conflict_source = SimpleNamespace(lease=lambda: SimpleNamespace(close=Mock()))
        with patch.object(team, "prepare_conflict_selection", side_effect=OSError("read failed")) as build:
            viewer._drc_grid_fill(0)
            self.wait_job(viewer._drc_conflict_job)
            viewer._drc_conflict_poll()
            for _ in range(3):
                self.assertEqual(len(viewer._drc_active_members()), 0)
            self.assertEqual(viewer._drc_grid_map, [])
            self.assertIn("read failed", viewer._drcwin._conflict_info.set_text.call_args.args[0])
            self.assertEqual(build.call_count, 1)

    def test_database_reset_cancels_without_waiting_and_drops_snapshot(self):
        viewer = conflict_fixture()
        viewer._drc_conflict_only = True
        source = SimpleNamespace(close=Mock(), lease=lambda: SimpleNamespace(close=Mock()))
        viewer._drc_conflict_source = source
        entered, release = threading.Event(), threading.Event()
        def work(*args, **kwargs):
            entered.set()
            release.wait(2)
            return Selection.from_indices(100_000_000, [5])
        with patch.object(team, "prepare_conflict_selection", side_effect=work):
            try:
                viewer._drc_grid_fill(0)
                self.assertTrue(entered.wait(1))
                old_job = viewer._drc_conflict_job
                viewer._drc_conflict_reset(drop_source=True)
                self.assertTrue(old_job["cancel"].is_set())
                self.assertFalse(old_job["done"], "reset must not join the running worker")
                self.assertFalse(viewer._drc_conflict_only)
                self.assertIsNone(viewer._drc_conflict_source)
                source.close.assert_called_once()
            finally:
                release.set()
            self.wait_job(old_job)
            viewer._drc_conflict_poll()
            self.assertIsNone(viewer._drc_conflict_members)


class ReviewWindowTests(unittest.TestCase):
    def setUp(self):
        self.glib = patch.object(gui, "GLib", SimpleNamespace(timeout_add=Mock()))
        self.glib.start()
        self.addCleanup(self.glib.stop)

    def test_worker_waits_for_accepted_visits_without_blocking_caller(self):
        window = fixture()
        barrier, entered = threading.Event(), threading.Event()
        window.viewer._drc_visit_worker = SimpleNamespace(barrier=lambda: barrier)
        called = []
        def work(cancelled, progress):
            called.append(threading.get_ident())
            entered.set()
            return 7
        complete = Mock()
        try:
            self.assertTrue(window._start("apply", work, complete, write=True))
            self.assertFalse(entered.wait(0.03))
            self.assertIsNotNone(window.viewer._drc_team_write)
        finally:
            barrier.set()
        finish(window)
        self.assertNotEqual(called, [threading.get_ident()])
        complete.assert_called_once_with(7)
        self.assertIsNone(window.viewer._drc_team_write)

    def test_replaced_database_discards_and_closes_read_results(self):
        window = fixture()
        release, entered = threading.Event(), threading.Event()
        result, complete = Mock(), Mock()
        def work(cancelled, progress):
            entered.set()
            release.wait(2)
            return result
        window._start("read", work, complete)
        self.assertTrue(entered.wait(1))
        old_job = window.job
        window.viewer._drc = object()
        window._poll()
        self.assertTrue(old_job["cancel"].is_set())
        release.set()
        finish(window)
        complete.assert_not_called()
        result.close.assert_called_once()
        window.viewer._drc_team_changed.assert_not_called()

    def test_accepted_write_finishes_after_window_closes(self):
        window = fixture()
        release, entered = threading.Event(), threading.Event()
        complete = Mock()
        def work(cancelled, progress):
            entered.set()
            release.wait(2)
            self.assertFalse(cancelled())
        window._start("write", work, complete, write=True, changed="flags")
        self.assertTrue(entered.wait(1))
        window._destroy()
        release.set()
        finish(window)
        complete.assert_not_called()
        window.viewer._drc_team_changed.assert_called_once_with(window.db, waived=False)
        self.assertIsNone(window.viewer._drc_team_write)

    def test_jump_uses_own_database_without_directly_recording_peer_visit(self):
        window = fixture()
        original = window.db
        window.store = [(0, 73, "#74", "no", "yes", "")]
        window._jump(None, 0, None)
        window.viewer._drc_jump.assert_called_once_with(0, 73, isolate=True)
        self.assertIs(window.viewer._drc, original)
        window.viewer._drc_jump.reset_mock()
        window.viewer._drc = object()
        window._jump(None, 0, None)
        window.viewer._drc_jump.assert_not_called()

    def test_safe_and_explicit_selected_application_have_distinct_scope(self):
        window = fixture()
        source = SimpleNamespace(reviewer="bob")
        window.sources = [source]
        selected = Selection.from_indices(100_000_000, [2, 99_999_999])
        window._selection = Mock(return_value=(0, selected))
        window._start = Mock()
        with patch.object(team, "apply_review", return_value={}, create=True) as apply:
            window._apply()
            call = window._start.call_args
            self.assertTrue(call.kwargs["write"])
            call.args[1](lambda: False, Mock())
            self.assertIs(apply.call_args.args[0], window.db)
            self.assertIs(apply.call_args.args[1], source)
            self.assertFalse(apply.call_args.kwargs["force"])
            self.assertIsNone(apply.call_args.kwargs["membership"])
            window._apply(True)
            window._start.call_args.args[1](lambda: False, Mock())
            self.assertTrue(apply.call_args.kwargs["force"])
            self.assertIs(apply.call_args.kwargs["membership"], selected)
            self.assertEqual(apply.call_args.kwargs["ci"], 0)

    def test_empty_explicit_selection_does_not_apply_whole_rule(self):
        window = fixture()
        window.sources = [SimpleNamespace(reviewer="bob")]
        window._selection = Mock(return_value=(0, Selection.empty(100_000_000)))
        window._start = Mock()
        window._apply(True)
        window._start.assert_not_called()

    def test_manual_exclusion_preserves_immutable_selection_and_flag_refresh(self):
        window = fixture()
        selected = Selection.from_indices(100_000_000, [7, 99_999_999])
        window._selection = Mock(return_value=(0, selected))
        window._start = Mock()
        window.db.set_review_members = Mock()
        window._manual("not-required", False)
        call = window._start.call_args
        self.assertEqual(call.kwargs["changed"], "flags")
        call.args[1](lambda: False, Mock())
        args = window.db.set_review_members.call_args.args
        self.assertEqual(args[0], 0)
        self.assertIs(args[1], selected)
        self.assertFalse(args[2])

    def test_page_reads_at_most_1000_and_marks_import_protection_separately(self):
        window = fixture()
        peer = SimpleNamespace(reviewer="bob", path="synthetic", visited_known=False,
            waive_known=True, not_required_known=False,
            members=Mock(return_value=SimpleNamespace(count=lambda: 1001)),
            page=Mock(return_value=list(range(1000))),
            get=Mock(return_value=SimpleNamespace(visited=False, waived=True, not_required=False)))
        aggregate = SimpleNamespace(counts=lambda ci=None: counts(),
            get=Mock(return_value=SimpleNamespace(conflict=False)))
        window.sources, window.aggregate = [peer], aggregate
        window.import_source = peer
        window._start = Mock()
        window._page_load()
        call = window._start.call_args
        result = call.args[1](lambda: False, Mock())
        call.args[2](result)
        peer.page.assert_called_once_with(0, 0, 1000, kind="work")
        self.assertEqual(len(window.store), 1000)
        self.assertEqual(window.store[0][3], "yes / no / no")
        self.assertEqual(window.store[0][4], "— / yes / —")
        self.assertIn("import protected", window.store[0][5])
        self.assertNotIn("reviewers", window.store[0][5])
        self.assertEqual(window.db.review_flags.call_count, 1000)
        window.next.set_sensitive.assert_called_with(True)

    def test_last_page_and_conflict_view_use_aggregate_membership(self):
        window = fixture()
        aggregate = SimpleNamespace(counts=lambda ci=None: counts(),
            get=lambda ci, ei: SimpleNamespace(conflict=True),
            members=Mock(return_value=SimpleNamespace(count=lambda: 1001)),
            page=Mock(return_value=[99999]))
        window.aggregate = aggregate
        window.mode.get_active_id.return_value = "conflict"
        window.page = 99
        window._start = Mock()
        window._page_load()
        call = window._start.call_args
        result = call.args[1](lambda: False, Mock())
        call.args[2](result)
        aggregate.page.assert_called_once_with(0, 1000, 1000, kind="conflict")
        self.assertEqual(window.page, 1)
        self.assertEqual(window.store[0][5], "reviewers")
        window.next.set_sensitive.assert_called_with(False)

    def test_legacy_load_menu_opens_comparison_instead_of_replacing_my_state(self):
        viewer = gui.Viewer.__new__(gui.Viewer)
        viewer._drc = SimpleNamespace(waive_import=Mock())
        window = SimpleNamespace(job=None, _load_dialog=Mock())
        viewer._drc_team_dialog = Mock(return_value=window)
        viewer._drc_waive_load_dialog()
        window._load_dialog.assert_called_once()
        viewer._drc.waive_import.assert_not_called()

    def test_same_reviewer_latest_snapshot_and_equal_time_tie_are_deterministic(self):
        for old_key, new_key, replace in (((5, "a"), (4, "z"), False),
                                           ((5, "a"), (5, "b"), True),
                                           ((5, "b"), (5, "a"), False)):
            with self.subTest(old=old_key, new=new_key):
                window = fixture()
                old = SimpleNamespace(reviewer="bob", exported_ns=old_key[0],
                                      snapshot_id=old_key[1], close=Mock())
                new = SimpleNamespace(reviewer="bob", exported_ns=new_key[0],
                                      snapshot_id=new_key[1], close=Mock())
                window.sources = [old]
                window._start, window._refresh = Mock(), Mock()
                window._load_paths(["not-opened-by-this-completion-test"])
                window._start.call_args.args[2](([new], []))
                self.assertEqual(window.sources, [new if replace else old])
                (old if replace else new).close.assert_called_once()
                (new if replace else old).close.assert_not_called()

    def test_output_cannot_replace_personal_or_loaded_source_files(self):
        window = fixture()
        with tempfile.TemporaryDirectory(prefix="floe-team-output-") as folder:
            paths = [str(Path(folder) / name) for name in
                     ("pack", "mine.waive", "mine.review", "peer.snapshot", "peer.original")]
            window.db.path, window.db._waive_path, window.db.review_path = paths[:3]
            companion = str(Path(folder) / "peer.waive")
            window.sources = [SimpleNamespace(path=paths[3], original_path=paths[4],
                                              metadata={"source_waive_path": companion})]
            for path in paths:
                self.assertFalse(window._output_allowed(path))
            self.assertFalse(window._output_allowed(companion))
            self.assertTrue(window._output_allowed(str(Path(folder) / "fresh.csv")))
            Path(paths[4]).write_text("peer data", encoding="utf-8")
            alias = Path(folder) / "alias"
            alias.symlink_to(paths[4])
            self.assertFalse(window._output_allowed(str(alias)))
            self.assertEqual(Path(paths[4]).read_text(encoding="utf-8"), "peer data")

    def test_failed_csv_preserves_existing_file_and_closes_aggregate(self):
        window = fixture()
        aggregate = SimpleNamespace(counts=Mock(side_effect=[counts(), OSError("read failed")]),
                                    close=Mock())
        window._start = Mock()
        window._output_allowed = Mock(return_value=True)
        with tempfile.TemporaryDirectory(prefix="floe-team-csv-") as folder:
            path = Path(folder) / "reviewers.csv"
            path.write_text("previous report", encoding="utf-8")
            window._choose = Mock(return_value=str(path))
            window._csv()
            with patch.object(team, "aggregate_reviews", return_value=aggregate):
                with self.assertRaisesRegex(OSError, "read failed"):
                    window._start.call_args.args[1](lambda: False, Mock())
            self.assertEqual(path.read_text(encoding="utf-8"), "previous report")
            self.assertEqual(list(Path(folder).iterdir()), [path])
            aggregate.close.assert_called_once()

    def test_read_job_blocks_competing_manual_writes_until_completion(self):
        window = fixture()
        release = threading.Event()
        window._start("snapshot", lambda *_: release.wait(1), Mock())
        self.assertIsNotNone(window.viewer._drc_team_busy)
        viewer = gui.Viewer.__new__(gui.Viewer)
        viewer._drc_team_busy = window.viewer._drc_team_busy
        viewer._set_live_status = Mock()
        self.assertTrue(viewer._drc_review_pending())
        release.set()
        finish(window)
        self.assertIsNone(window.viewer._drc_team_busy)

    def test_applying_judgement_keeps_open_error_and_cd_rulers_visible(self):
        viewer = gui.Viewer.__new__(gui.Viewer)
        viewer._drc = db = object()
        viewer._drc_open, viewer._drc_pos, viewer._drc_cum = None, 7, [0, 100]
        viewer._drc_sel, viewer._drc_wfilter, viewer._drcwin = None, "all", object()
        mark = dict(color=gui.DRC_RED, review_target=(db, 0, 7), review_queued=True)
        cd, manual = object(), object()
        viewer.drc_mark, viewer._drc_ruler, viewer.rulers = mark, [cd], [cd, manual]
        for name in ("_drc_marker_invalidate", "_drc_delta_status_changed", "_drc_show_detail",
                     "_drc_review_stats_refresh", "_drc_review_flags_refresh", "_display"):
            setattr(viewer, name, Mock())
        viewer._drc_waived = Mock(return_value=True)
        def rebuild_rules(**kwargs):
            viewer.drc_mark, viewer._drc_pos, viewer._drc_ruler = None, -1, []
            viewer.rulers = [manual]
        viewer._drc_fill = Mock(side_effect=rebuild_rules)
        viewer._drc_team_changed(db)
        self.assertIs(viewer.drc_mark, mark)
        self.assertEqual(mark["color"], gui.DRC_GREEN)
        self.assertTrue(mark["review_queued"])
        self.assertEqual(viewer._drc_pos, 7)
        self.assertEqual(viewer._drc_ruler, [cd])
        self.assertCountEqual(viewer.rulers, [cd, manual])
        viewer._drc_show_detail.assert_called_once_with(0, 7)


def migration_fixture():
    original = fixture()
    window = gui._DrcMigrationWindow.__new__(gui._DrcMigrationWindow)
    window.__dict__.update(original.__dict__)
    window.plan, window._applied = None, None
    for name in ("previous", "review_file", "apply_button", "cancel_button"):
        setattr(window, name, Mock())
    window.previous.get_text.return_value = "  prior.db  "
    window.review_file.get_text.return_value = ""
    window.mode.get_active_id.return_value = "matched"
    window.viewer._drc_team = None
    window.viewer._drc_migration = window
    return window


def migration_plan(ids=(3, 99_999_999), ambiguous=(), conflicts=(), states=None, protected=None):
    size = 100_000_000
    partial = set(ids) & set(ambiguous)
    if protected is None:
        protected = set(conflicts) - set(ambiguous)
    scopes = {kind: Selection.from_indices(size, values) for kind, values in
              (("matched", ids), ("ambiguous", ambiguous), ("conflict", conflicts),
               ("partial", partial), ("protected", protected))}
    summary = dict(matched=len(ids), unmatched=size-len(set(ids) | set(ambiguous)),
                   ambiguous=len(ambiguous), conflict=len(conflicts), partial=len(partial))
    plan = SimpleNamespace(summary=summary, close=Mock())
    plan.members = Mock(side_effect=lambda ci, kind: scopes[kind])
    plan.page = Mock(side_effect=lambda ci, start, limit, kind="matched":
                     scopes[kind].page(start, limit))
    plan.rule_counts = Mock(return_value=summary)
    states = states or {}
    plan.get = Mock(side_effect=lambda ci, ei: states.get(ei,
                    team.ReviewState(True, False, False)))
    return plan


class MigrationWindowTests(unittest.TestCase):
    def setUp(self):
        self.glib = patch.object(gui, "GLib", SimpleNamespace(timeout_add=Mock()))
        self.glib.start()
        self.addCleanup(self.glib.stop)

    def test_prepare_captures_previous_and_optional_review_paths_without_applying(self):
        from floe import drc_migrate
        for entry, expected in (("", None), ("  prior.floereview  ", "prior.floereview")):
            with self.subTest(review_file=entry):
                window = migration_fixture()
                window.review_file.get_text.return_value = entry
                window._start, window._page_load = Mock(), Mock()
                old, result = migration_plan(), migration_plan()
                window.plan = old
                with patch.object(drc_migrate, "prepare_migration", return_value=result) as prepare, \
                        patch.object(drc_migrate, "apply_migration") as apply:
                    window._prepare()
                    call = window._start.call_args
                    self.assertFalse(call.kwargs.get("write", False))
                    self.assertFalse(call.kwargs.get("changed", False))
                    window.previous.get_text.return_value = "different.db"
                    window.review_file.get_text.return_value = "different.review"
                    cancelled, progress = Mock(return_value=False), Mock()
                    self.assertIs(call.args[1](cancelled, progress), result)
                    self.assertEqual(prepare.call_args.args, (window.db, "prior.db"))
                    self.assertEqual(prepare.call_args.kwargs["state_path"], expected)
                    self.assertIs(prepare.call_args.kwargs["cancelled"], cancelled)
                    prepare.call_args.kwargs["progress"](7, 20, "geometry")
                    progress.assert_called_once_with(7, 20)
                    apply.assert_not_called()
                    old.close.assert_not_called()
                    call.args[2](result)
                    old.close.assert_called_once()
                    self.assertIs(window.plan, result)
                    window._page_load.assert_called_once()

    def test_browse_cancel_preserves_entry_and_empty_previous_does_not_prepare(self):
        window = migration_fixture()
        window._choose = Mock(side_effect=[None, "chosen.db"])
        window._browse(window.previous, "Choose previous")
        window.previous.set_text.assert_not_called()
        window._browse(window.previous, "Choose previous")
        window.previous.set_text.assert_called_once_with("chosen.db")
        window.previous.get_text.return_value = "   "
        window._start = Mock()
        window._prepare()
        window._start.assert_not_called()
        self.assertIn("previous", window.status.set_text.call_args.args[0])

    def test_apply_captures_plan_targets_new_database_and_marks_accepted_write(self):
        from floe import drc_migrate
        window = migration_fixture()
        window.plan = plan = migration_plan()
        window._start, window._page_load = Mock(), Mock()
        result = dict(changed_visited=3, changed_waived=2,
                      changed_not_required=1, conflicts_skipped=4)
        with patch.object(drc_migrate, "apply_migration", return_value=result) as apply:
            window._apply()
            call = window._start.call_args
            self.assertTrue(call.kwargs["write"])
            self.assertTrue(call.kwargs["changed"])
            window.plan = migration_plan(ids=(77,))
            window.viewer._drc = object()
            cancel, progress = Mock(return_value=False), Mock()
            self.assertIs(call.args[1](cancel, progress), result)
            self.assertEqual(apply.call_args.args, (window.db, plan))
            self.assertIs(apply.call_args.kwargs["cancelled"], cancel)
            apply.call_args.kwargs["progress"](4, 8, "apply")
            progress.assert_called_once_with(4, 8)
            call.args[2](result)
            self.assertIs(window._applied, result)
            window._page_load.assert_called_once()

    def test_input_changes_close_plan_clear_preview_and_block_unprepared_apply(self):
        from floe import drc_migrate
        window = migration_fixture()
        window.plan = plan = migration_plan()
        window._applied = {"changed_visited": 1}
        window.store = [(0, 3, "#4", "", "", "exact match")]
        window._inputs_changed()
        plan.close.assert_called_once()
        self.assertIsNone(window.plan)
        self.assertIsNone(window._applied)
        self.assertEqual(window.store, [])
        window.apply_button.set_sensitive.assert_called_with(False)
        window.prev.set_sensitive.assert_called_with(False)
        window.next.set_sensitive.assert_called_with(False)
        with patch.object(drc_migrate, "apply_migration") as apply:
            window._apply()
            apply.assert_not_called()
        self.assertIn("Compare geometry first", window.status.set_text.call_args.args[0])

    def test_match_pages_are_bounded_and_keep_sparse_new_error_numbers(self):
        window = migration_fixture()
        ids = list(range(7, 2007, 2)) + [99_999_999]
        window.plan = plan = migration_plan(ids=ids)
        window._page_load()
        finish(window)
        self.assertEqual([row[1] for row in window.store], ids[:1000])
        self.assertEqual(len(window.store), 1000)
        self.assertEqual(window.store[0][2], "#8")
        plan.page.assert_called_with(0, 0, 1000, kind="matched")
        window.next.set_sensitive.assert_called_with(True)
        window._turn(1)
        finish(window)
        self.assertEqual([row[1] for row in window.store], [99_999_999])
        self.assertEqual(window.store[0][2], "#100000000")
        plan.page.assert_called_with(0, 1000, 1000, kind="matched")
        window.next.set_sensitive.assert_called_with(False)
        window.prev.set_sensitive.assert_called_with(True)
        window._applied = dict(changed_visited=4, changed_waived=3,
                               changed_not_required=2, conflicts_skipped=1)
        window._page_load()
        finish(window)
        text = window.stats.set_text.call_args.args[0]
        self.assertIn("Whole new DB", text)
        self.assertIn("visited 4 · waived 3 · not for review 2 · protected 1", text)

    def test_unknown_previous_flags_ambiguity_and_protected_decisions_are_distinct(self):
        window = migration_fixture()
        window.plan = plan = migration_plan(ids=(4, 9), ambiguous=(77,), conflicts=(9,),
            states={4: team.ReviewState(None, True, None),
                    9: team.ReviewState(True, True, False), 77: None})
        window._page_load()
        finish(window)
        self.assertEqual(window.store[0][4:], ("— / yes / —", "exact match"))
        self.assertEqual(window.store[1][5], "current judgement protected")
        window.mode.get_active_id.return_value = "ambiguous"
        window._changed()
        finish(window)
        self.assertEqual(window.store, [(0, 77, "#78", "yes / no / no",
                                        "ambiguous", "geometry match unverified")])
        self.assertIn((0, 77), [call.args for call in plan.get.call_args_list])
        window.mode.get_active_id.return_value = "conflict"
        window._changed()
        finish(window)
        self.assertEqual([row[1] for row in window.store], [9])
        plan.page.assert_called_with(0, 0, 1000, kind="conflict")

    def test_partial_duplicate_consensus_shows_union_visits_and_only_disputed_fields_unknown(self):
        window = migration_fixture()
        window.plan = migration_plan(ids=(4, 9, 12), ambiguous=(4, 9, 12), conflicts=(4, 9, 12),
            protected=(9,), states={4: team.ReviewState(True, None, False),
                9: team.ReviewState(True, True, None), 12: team.ReviewState(False, None, None)})
        for mode in ("matched", "ambiguous", "conflict"):
            with self.subTest(mode=mode):
                window.mode.get_active_id.return_value = mode
                window._page_load()
                finish(window)
                self.assertEqual([row[1] for row in window.store], [4, 9, 12])
                self.assertEqual(window.store[0][4:],
                    ("yes / — / no", "partial consensus; differing fields retained"))
                self.assertEqual(window.store[1][4:],
                    ("yes / yes / —", "current judgement protected; partial prior consensus"))
                self.assertEqual(window.store[2][4:],
                    ("no / — / —", "partial consensus; differing fields retained"))
                self.assertIn("matched review 3 (partial 3)", window.stats.set_text.call_args.args[0])

    def test_double_click_only_jumps_in_captured_new_database_when_idle(self):
        window = migration_fixture()
        window.store = [(0, 99_999_999, "#100000000", "", "", "exact match")]
        window._jump(None, 0, None)
        window.viewer._drc_jump.assert_called_once_with(0, 99_999_999, isolate=True)
        window.viewer._drc_jump.reset_mock()
        window.job = {}
        window._jump(None, 0, None)
        window.viewer._drc_jump.assert_not_called()
        window.job = None
        window.viewer._drc = object()
        window._jump(None, 0, None)
        window.viewer._drc_jump.assert_not_called()

    def test_read_cancel_and_database_switch_discard_late_plan_and_close_sources(self):
        from floe import drc_migrate
        window = migration_fixture()
        window.plan, result = migration_plan(), migration_plan()
        previous = window.plan
        entered, release = threading.Event(), threading.Event()
        def prepare(*args, **kwargs):
            entered.set()
            release.wait(2)
            return result
        with patch.object(drc_migrate, "prepare_migration", side_effect=prepare):
            try:
                window._prepare()
                self.assertTrue(entered.wait(1))
                job = window.job
                self.assertFalse(job["write"])
                window._cancel_operation()
                self.assertTrue(job["cancel"].is_set())
                window.cancel_button.set_sensitive.assert_called_with(False)
                window.viewer._drc = object()
                window._destroy()
                self.assertIsNone(window.viewer._drc_migration)
                previous.close.assert_not_called()
                self.assertFalse(job["done"], "window destruction must not wait for geometry matching")
            finally:
                release.set()
            finish(window)
        self.assertIsNone(window.plan)
        previous.close.assert_called_once()
        result.close.assert_called_once()
        window.viewer._drc_team_changed.assert_not_called()

    def test_accepted_apply_finishes_after_close_before_plan_is_released(self):
        from floe import drc_migrate
        window = migration_fixture()
        window.plan = plan = migration_plan()
        entered, release = threading.Event(), threading.Event()
        def apply(db, captured, **kwargs):
            self.assertIs(db, window.db)
            self.assertIs(captured, plan)
            entered.set()
            release.wait(2)
            self.assertFalse(kwargs["cancelled"]())
            plan.close.assert_not_called()
            return dict(changed_visited=1, changed_waived=0, changed_not_required=0,
                        conflicts_skipped=0)
        with patch.object(drc_migrate, "apply_migration", side_effect=apply):
            try:
                window._apply()
                self.assertTrue(entered.wait(1))
                job = window.job
                self.assertTrue(job["write"])
                self.assertIs(window.viewer._drc_team_write, job)
                window._destroy()
                self.assertFalse(job["cancel"].is_set())
                plan.close.assert_not_called()
            finally:
                release.set()
            finish(window)
        self.assertIsNone(window.plan)
        plan.close.assert_called_once()
        self.assertIsNone(window.viewer._drc_team_write)
        window.viewer._drc_team_changed.assert_called_once_with(window.db, waived=False)


if __name__ == "__main__":
    unittest.main(verbosity=2)
