"""Review flags follow displayed individual geometry, never marker traffic.

Headless tests exercise the real overlay and review handlers. No native
window, real user review file, or large geometry allocation is needed.
"""

import contextlib
import csv
import io
import os
import sys
import tempfile
import threading
import time
import unittest
from types import SimpleNamespace
from unittest.mock import Mock, patch

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from floe import gui  # noqa: E402
from floe.drc_selection import Selection  # noqa: E402
from validate_drc_marker_gui import viewer_fixture  # noqa: E402


def display_fixture():
    viewer = viewer_fixture()
    viewer._drc = SimpleNamespace(mark_visited=Mock())
    viewer._drc_visit_worker = Mock()
    viewer._drc_marker_invalidate = Mock()
    viewer._drc_delta_status_changed = Mock()
    viewer._drc_stamp_markers = Mock()
    viewer._drc_fill_translucent = Mock()
    viewer.selections = []
    viewer._cell_hl = None
    viewer._zoomdrag = None
    viewer.overlay_mode = 1
    disp = Mock()
    disp.get_width.return_value = disp.get_height.return_value = 100
    return viewer, disp


def mark(viewer, points, kind="e", ei=7):
    return dict(kind=kind, pts=points, color=gui.DRC_RED,
                review_target=(viewer._drc, 0, ei))


def paint(viewer, disp, box=(0, 0, 100, 100)):
    with patch.object(gui, "stamp_segment"), \
            patch.object(gui, "stamp_drc_circle"):
        viewer._draw_overlays(disp, box, 1)


class DisplayVisitTests(unittest.TestCase):
    def test_visible_individual_edge_is_queued_once_after_draw(self):
        viewer, disp = display_fixture()
        viewer.drc_mark = mark(viewer, [(20, 20), (80, 80)])
        calls = []
        viewer._drc_visit_worker.submit.side_effect = lambda *_: calls.append("save")
        with patch.object(gui, "stamp_segment", side_effect=lambda *_: calls.append("draw")):
            viewer._draw_overlays(disp, (0, 0, 100, 100), 1)
        self.assertLess(calls.index("draw"), calls.index("save"))
        paint(viewer, disp)
        viewer._drc_visit_worker.submit.assert_called_once_with(viewer._drc, 0, 7)
        viewer._drc.mark_visited.assert_not_called()
        viewer._drc_marker_invalidate.assert_not_called()

    def test_hidden_overlay_is_not_a_visit_but_showing_it_is(self):
        viewer, disp = display_fixture()
        viewer.drc_mark = mark(viewer, [(20, 20), (80, 80)])
        viewer.overlay_mode = 2
        paint(viewer, disp)
        viewer._drc_visit_worker.submit.assert_not_called()
        viewer.overlay_mode = 1
        paint(viewer, disp)
        viewer._drc_visit_worker.submit.assert_called_once()

    def test_collapsed_circle_does_not_visit_until_geometry_is_visible(self):
        viewer, disp = display_fixture()
        viewer.drc_mark = mark(viewer, [(20, 20), (22, 22)])
        paint(viewer, disp)
        viewer._drc_visit_worker.submit.assert_not_called()
        with patch.object(gui, "stamp_segment"):
            viewer._draw_overlays(disp, (19, 19, 29, 29), 0.1)
        viewer._drc_visit_worker.submit.assert_called_once()

    def test_offscreen_previous_frame_waits_for_the_jump_frame(self):
        viewer, disp = display_fixture()
        viewer.drc_mark = mark(viewer, [(200, 200), (250, 250)])
        paint(viewer, disp)
        viewer._drc_visit_worker.submit.assert_not_called()
        paint(viewer, disp, (190, 190, 290, 290))
        viewer._drc_visit_worker.submit.assert_called_once()

    def test_disjoint_edge_pairs_do_not_visit_for_bbox_overlap_alone(self):
        viewer, disp = display_fixture()
        viewer.drc_mark = mark(viewer,
                               [(-10, -10), (-10, 110), (110, -10), (110, 110)])
        paint(viewer, disp)
        viewer._drc_visit_worker.submit.assert_not_called()

    def test_segment_crossing_the_view_and_polygon_surrounding_it_are_visible(self):
        for points, kind in (([(-10, 50), (110, 50)], "e"),
                             ([(-10, -10), (110, -10), (110, 110), (-10, 110)], "p")):
            with self.subTest(kind=kind):
                viewer, disp = display_fixture()
                viewer.drc_mark = mark(viewer, points, kind)
                paint(viewer, disp)
                viewer._drc_visit_worker.submit.assert_called_once()

    def test_marker_raster_focus_circle_and_selection_do_not_visit(self):
        viewer, disp = display_fixture()
        viewer.overlay_mode = 0
        viewer._drc_focus = (0, 7, "e", [(20, 20), (80, 80)])
        viewer._drc_sel = (0, Selection.from_indices(100, [7, 8]), (), {7, 8})
        viewer._drc_stamp_errs = Mock()
        paint(viewer, disp)
        viewer._drc_stamp_markers.assert_called_once()
        viewer._drc_stamp_errs.assert_called_once()
        viewer._drc_visit_worker.submit.assert_not_called()

    def test_geometry_from_a_replaced_db_cannot_visit_the_new_db(self):
        viewer, disp = display_fixture()
        viewer.drc_mark = mark(viewer, [(20, 20), (80, 80)])
        previous = viewer._drc
        viewer._drc = SimpleNamespace(mark_visited=Mock())
        paint(viewer, disp)
        viewer._drc_visit_worker.submit.assert_not_called()
        previous.mark_visited.assert_not_called()

    def test_extreme_diagonal_only_visits_when_the_bounded_painter_hits_pixels(self):
        # The diagonal painter samples at most 8,001 dabs. At a locked
        # extreme zoom an intersecting line can have no actual visible dab.
        for offset, visible in ((50, False), (0, True)):
            with self.subTest(offset=offset):
                viewer, disp = display_fixture()
                viewer.drc_mark = mark(viewer,
                    [(-1_000_000 - offset, 1_000_100 + offset),
                     (1_000_000 - offset, -999_900 + offset)])
                viewer._draw_overlays(disp, (0, 0, 100, 100), 1)
                self.assertEqual(disp.new_subpixbuf.called, visible)
                self.assertEqual(viewer._drc_visit_worker.submit.called, visible)


class VisitWorkerTests(unittest.TestCase):
    def test_visit_writes_run_off_the_caller_and_survive_db_replacement(self):
        worker = gui._DrcVisitWorker()
        started, release, second_done = threading.Event(), threading.Event(), threading.Event()
        caller = threading.get_ident()
        threads, calls = [], []

        def first_write(ci, ei):
            threads.append(threading.get_ident())
            started.set()
            if not release.wait(5):
                raise RuntimeError("test did not release worker")
            calls.append(("first", ci, ei))

        def second_write(ci, ei):
            threads.append(threading.get_ident())
            calls.append(("second", ci, ei))
            second_done.set()

        first = SimpleNamespace(mark_visited=first_write)
        second = SimpleNamespace(mark_visited=second_write)
        try:
            worker.submit(first, 0, 7)
            self.assertTrue(started.wait(5))
            worker.submit(second, 4, 1)
            self.assertEqual(calls, [])
        finally:
            release.set()
        self.assertTrue(second_done.wait(5))
        deadline = time.monotonic() + 5
        results = []
        while len(results) < 2 and time.monotonic() < deadline:
            result = worker.poll()
            if result is not None:
                results.append(result)
            else:
                time.sleep(0.005)
        self.assertEqual(calls, [("first", 0, 7), ("second", 4, 1)])
        self.assertTrue(all(thread != caller for thread in threads))
        self.assertEqual(results, [(first, 0, 7, None), (second, 4, 1, None)])

    def test_write_error_is_reported_without_losing_the_next_visit(self):
        worker = gui._DrcVisitWorker()
        bad = SimpleNamespace(mark_visited=Mock(side_effect=OSError("disk full")))
        good = SimpleNamespace(mark_visited=Mock())
        with contextlib.redirect_stderr(io.StringIO()):
            worker.submit(bad, 0, 0)
            worker.submit(good, 0, 1)
            deadline = time.monotonic() + 5
            results = []
            while len(results) < 2 and time.monotonic() < deadline:
                result = worker.poll()
                if result is not None:
                    results.append(result)
                else:
                    time.sleep(0.005)
        self.assertEqual(len(results), 2)
        self.assertIn("disk full", results[0][3])
        self.assertIsNone(results[1][3])
        good.mark_visited.assert_called_once_with(0, 1)


def finish_job(viewer):
    job = viewer._drc_review_job
    deadline = time.monotonic() + 5
    while not job["done"] and time.monotonic() < deadline:
        time.sleep(0.005)
    if not job["done"]:
        raise AssertionError("review job did not finish")
    viewer._drc_review_poll()
    return job


class ReviewActionTests(unittest.TestCase):
    def test_exclusion_uses_background_immutable_selection_without_marker_rescan(self):
        viewer = viewer_fixture()
        entered, release = threading.Event(), threading.Event()
        foreground = threading.get_ident()
        observed = []
        selected = Selection.from_indices(100_000_000, [0, 99_999_999])

        def update(ci, targets, on, **kwargs):
            observed.append((threading.get_ident(), ci, targets, on))
            entered.set()
            if not release.wait(5):
                raise RuntimeError("test did not release edit")
            kwargs["progress"](targets.size, targets.size, 2)

        viewer._drc = SimpleNamespace(checks=[SimpleNamespace(errors=range(selected.size))],
                                     set_review_members=update)
        viewer._drc_marker_invalidate = Mock()
        viewer._drc_review_flags_refresh = Mock()
        viewer._drc_sel = (0, selected, (), selected)
        viewer._drc_page = 99999
        try:
            viewer._drc_set_not_required(0, selected, True)
            self.assertTrue(entered.wait(5))
            self.assertFalse(viewer._drc_review_job["done"])
            self.assertIs(observed[0][2], selected)
            self.assertNotEqual(observed[0][0], foreground)
        finally:
            release.set()
        job = finish_job(viewer)
        self.assertIsNone(job["error"])
        self.assertEqual(viewer._drc_page, 99999)
        self.assertIs(viewer._drc_sel[1], selected)
        viewer._drc_marker_invalidate.assert_not_called()
        viewer._drc_grid_fill.assert_not_called()
        viewer._drc_review_flags_refresh.assert_called_once_with(0)

    def test_v_and_x_do_not_replace_waive_or_selection_colours(self):
        viewer = viewer_fixture()
        db = SimpleNamespace(review_flags=lambda *_: 3, get_status=lambda *_: 1)
        plain = viewer._drc_cell_markup(db, 0, 4, True, ())
        selected = viewer._drc_cell_markup(db, 0, 4, True, {4})
        current = viewer._drc_cell_markup(db, 0, 4, True, {4}, current=True)
        for text in (plain, selected, current):
            self.assertIn("VX *5", text)
        self.assertIn(viewer._rgb_hex(gui.DRC_GREEN), plain)
        self.assertIn(viewer._rgb_hex(gui.DRC_GOLD), selected)
        self.assertIn(viewer._rgb_hex(gui.DRC_GREEN_ON_GOLD), selected)
        self.assertIn("#3465a4", current)

    def test_unavailable_flags_are_shown_as_unknown_once_not_as_unvisited(self):
        viewer = viewer_fixture()
        db = SimpleNamespace(review_flags=Mock(side_effect=ValueError("another DRC result")))
        for _ in range(3):
            text = viewer._drc_cell_markup(db, 0, 0, False, ())
            self.assertIn("? 1", text)
        viewer._set_live_status.assert_called_once()

    def test_stats_export_waits_for_prior_visit_and_preserves_rule_and_total_counts(self):
        viewer = viewer_fixture()
        viewer._drc_visit_worker = worker = gui._DrcVisitWorker()
        entered, release = threading.Event(), threading.Event()
        visited = [0]

        def write(*_args):
            entered.set()
            if not release.wait(5):
                raise RuntimeError("test did not release visit")
            visited[0] = 1

        def counts(ci):
            return dict(total=3, visited=visited[0] if ci == 0 else 0,
                        not_required=1, visited_not_required=0,
                        pending=2 - (visited[0] if ci == 0 else 0))

        db = viewer._drc = SimpleNamespace(
            checks=[SimpleNamespace(name='M1,"space"'), SimpleNamespace(name="둘째 룰")],
            mark_visited=write, review_counts=counts)
        worker.submit(db, 0, 0)
        self.assertTrue(entered.wait(5))
        with tempfile.TemporaryDirectory(prefix="floe-review-export-") as folder:
            path = os.path.join(folder, "stats.csv")
            try:
                viewer._drc_review_export_start(path)
                self.assertFalse(viewer._drc_review_job["done"])
                self.assertFalse(os.path.exists(path))
            finally:
                release.set()
            viewer._drc_review_flags_refresh = Mock()
            job = finish_job(viewer)
            self.assertIsNone(job["error"])
            with open(path, encoding="utf-8", newline="") as stream:
                rows = list(csv.DictReader(stream))
        self.assertEqual([row["scope"] for row in rows], ["database", "rule", "rule"])
        self.assertEqual(rows[1]["rule"], 'M1,"space"')
        self.assertEqual(rows[2]["rule"], "둘째 룰")
        self.assertEqual(int(rows[0]["visited"]), 1)
        for field in ("total", "visited", "not_required", "visited_not_required", "unvisited_targets"):
            self.assertEqual(int(rows[0][field]), sum(int(row[field]) for row in rows[1:]))

    def test_stats_read_failure_preserves_an_existing_export(self):
        viewer = viewer_fixture()
        viewer._drc = SimpleNamespace(checks=[SimpleNamespace(name="R")],
            review_counts=Mock(side_effect=ValueError("review fingerprint mismatch")))
        with tempfile.TemporaryDirectory(prefix="floe-review-export-") as folder:
            path = os.path.join(folder, "stats.csv")
            with open(path, "w", encoding="utf-8") as stream:
                stream.write("previous statistics")
            with contextlib.redirect_stderr(io.StringIO()):
                viewer._drc_review_export_start(path)
                job = finish_job(viewer)
            self.assertIn("fingerprint mismatch", job["error"])
            with open(path, encoding="utf-8") as stream:
                self.assertEqual(stream.read(), "previous statistics")
            self.assertEqual(os.listdir(folder), ["stats.csv"])


if __name__ == "__main__":
    unittest.main(verbosity=2)
