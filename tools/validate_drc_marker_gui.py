"""Headless GUI regression checks for full-scope adaptive DRC markers.

Uses real Viewer methods without creating GTK windows. Worker tests use
controlled events rather than expensive geometry or arbitrary sleep delays.

Usage: python tools/validate_drc_marker_gui.py
"""

import os
import gc
import sys
import threading
import time
import unittest
import weakref
from types import SimpleNamespace
from unittest.mock import Mock, patch

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from floe import drc, gui  # noqa: E402
from floe import drc_marker_worker as worker_mod  # noqa: E402
from floe.drc_markers import Marker, MarkerQueryCancelled  # noqa: E402


def viewer_fixture():
    viewer = gui.Viewer.__new__(gui.Viewer)
    viewer._drc_open = 0
    viewer._drcwin = None
    viewer._drc_cluster = None
    viewer._drc_wfilter = "all"
    viewer._drc_sel = None
    viewer._drc_sels = {}
    viewer._drc_show_sel = False
    viewer._drc_focus = None
    viewer._drc_hl = False
    viewer._drc_hl_res = None
    viewer._drc_page = 0
    viewer._drc_page_marks = []
    viewer._drc_hits = []
    viewer._drc_group_hits = []
    viewer._drc_grid_fill = Mock()
    viewer._drc_info_refresh = Mock()
    viewer._set_live_status = Mock()
    viewer._display = Mock()
    viewer.view_bbox = Mock(return_value=(0, 0, 10000, 10000))
    viewer.dbu = 0.001
    viewer.spp = 10
    viewer.mode = "normal"
    viewer.overlay_mode = 0
    return viewer


class MarkerViewerTests(unittest.TestCase):
    def setUp(self):
        self.viewer = viewer_fixture()
        self.viewer._drc = SimpleNamespace(
            checks=[SimpleNamespace(name="R")])
        self.viewer._drc_marker_worker = Mock()
        self.viewer._drc_marker_worker.poll.return_value = None
        self.viewer._drc_marker_revision = 0
        self.viewer._drc_marker_key = None
        self.viewer._drc_marker_result = None
        self.viewer._drc_marker_overlay = None
        self.viewer._drc_marker_busy = False
        self.bounds = (0, 0, 1000, 1000)

    def request(self):
        return self.viewer._drc_marker_request(self.bounds, 10, 100, 100)

    def test_grid_page_does_not_change_marker_population_or_query(self):
        viewer = self.viewer
        self.assertEqual(self.request(), [])
        key = viewer._drc_marker_key
        markers = [Marker(0, 7093, 0.4, 0.7, 40000, 0,
                          (0.3, 0.6, 0.5, 0.8), True)]
        viewer._drc_marker_result = key, markers
        viewer._drc_page = 40
        viewer._drc_grid_map = [40107, 49019]
        self.assertIs(self.request(), markers)
        self.assertEqual(viewer._drc_marker_key, key)
        viewer._drc_marker_worker.submit.assert_called_once()
        _key, db, query = viewer._drc_marker_worker.submit.call_args.args
        self.assertIs(db, viewer._drc)
        self.assertEqual(query["bounds_um"], (0.0, 0.0, 1.0, 1.0))
        self.assertEqual(query["checks"], (0,))
        self.assertIsNone(query["members"])
        self.assertNotIn("cap", query)

    def test_scope_change_drops_cached_markers_and_stale_worker_reply(self):
        viewer = self.viewer
        self.request()
        old_key = viewer._drc_marker_key
        old = [Marker(0, 9001, 0.4, 0.7, 100, 0,
                      (0.3, 0.6, 0.5, 0.8), True)]
        viewer._drc_marker_result = old_key, old
        cluster = object()
        viewer._drc_cluster = cluster
        self.assertEqual(self.request(), [])
        self.assertNotEqual(viewer._drc_marker_key, old_key)
        self.assertEqual(viewer._drc_marker_worker.submit.call_args.args[2]
                         ["members"], {0: cluster})
        viewer._drc_marker_worker.poll.return_value = (old_key, old, None)
        viewer._drc_marker_poll()
        self.assertIsNone(viewer._drc_marker_result)
        viewer._display.assert_not_called()
        new_key = viewer._drc_marker_key
        viewer._drc_marker_worker.poll.return_value = (new_key, [], None)
        viewer._drc_marker_poll()
        self.assertEqual(viewer._drc_marker_result, (new_key, []))
        viewer._display.assert_called_once()

    def test_status_revision_invalidates_even_without_population_change(self):
        viewer = self.viewer
        self.request()
        old_key = viewer._drc_marker_key
        viewer._drc_marker_result = (old_key, ["old colors"])
        viewer._drc_hits = [(5, 5, 0, 1)]
        viewer._drc_group_hits = [(6, 6, "old group")]
        viewer._drc_marker_invalidate()
        self.assertEqual(viewer._drc_hits, [])
        self.assertEqual(viewer._drc_group_hits, [])
        viewer._drc_marker_worker.cancel.assert_called_once()
        self.assertEqual(self.request(), [])
        self.assertNotEqual(viewer._drc_marker_key, old_key)
        self.assertEqual(viewer._drc_marker_worker.submit.call_count, 2)

    def test_actual_waive_actions_refresh_same_count_spatial_swap(self):
        viewer = self.viewer
        statuses = [drc.STATUS_WAIVED, drc.STATUS_NONE]
        viewer._drc.set_status = lambda ci, ei, value: statuses.__setitem__(ei, value)
        viewer._drc_clusters = None
        viewer._drcwin = SimpleNamespace(_rstore=[])
        viewer.drc_mark = None
        self.request()
        previous_key = viewer._drc_marker_key
        viewer._drc_set_waived(0, [0], False)
        viewer._drc_set_waived(0, [1], True)
        self.assertEqual(statuses, [drc.STATUS_NONE, drc.STATUS_WAIVED])
        self.request()
        self.assertNotEqual(previous_key, viewer._drc_marker_key)
        self.assertEqual(viewer._drc_marker_worker.cancel.call_count, 2)

    def test_partially_failed_waive_batch_discards_old_marker_colors(self):
        viewer = self.viewer
        self.request()
        viewer._drc_marker_result = viewer._drc_marker_key, ["stale"]
        viewer._drc.set_status = Mock(side_effect=[None, OSError("disk full")])
        viewer._drc_clusters = None
        viewer._drc_set_waived(0, [1, 2], True)
        self.assertIsNone(viewer._drc_marker_result)
        viewer._drc_marker_worker.cancel.assert_called_once()

    def test_selected_filter_and_waive_filter_intersect_with_cluster(self):
        viewer = self.viewer
        viewer._drc_show_sel = True
        viewer._drc_wfilter = "waived"
        viewer._drc_cluster = SimpleNamespace(contains=lambda ei: ei in {7, 9017})
        viewer._drc_sel = (0, [7, 3001, 9017], [], frozenset({7, 3001, 9017}))
        self.request()
        query = viewer._drc_marker_worker.submit.call_args.args[2]
        self.assertTrue(query["waived"])
        members = query["members"][0]
        self.assertEqual(members.mask(0, 10000).nonzero()[0].tolist(), [7, 9017])
        viewer._drc_sel = None
        self.request()
        query = viewer._drc_marker_worker.submit.call_args.args[2]
        self.assertFalse(query["members"][0].mask(0, 10000).any())

    def test_viewport_change_requeries_displayed_bounds(self):
        viewer = self.viewer
        self.request()
        first_key = viewer._drc_marker_key
        viewer._drc_marker_result = first_key, ["old viewport"]
        self.bounds = (1000, 500, 2000, 1500)
        self.assertEqual(self.request(), [])
        query = viewer._drc_marker_worker.submit.call_args.args[2]
        self.assertEqual(query["bounds_um"], (1.0, 0.5, 2.0, 1.5))
        self.assertNotEqual(first_key, viewer._drc_marker_key)

    def test_resize_uses_frozen_frame_transform_with_new_canvas_size(self):
        viewer = self.viewer
        viewer._drc_marker_request((200, 300, 1200, 1300), 10, 150, 80)
        query = viewer._drc_marker_worker.submit.call_args.args[2]
        self.assertEqual(query["bounds_um"], (0.2, 0.5, 1.7, 1.3))
        self.assertEqual((query["width_px"], query["height_px"]), (150, 80))

    def test_painter_caches_layer_and_separates_exact_from_group_hits(self):
        viewer = self.viewer
        single = Marker(0, 1507, 0.2, 0.3, 1, 0, (0.2, 0.3, 0.2, 0.3), False)
        group = Marker(0, 9, 0.8, 0.5, 200000, 75000,
                       (0.7, 0.4, 0.9, 0.6), True)
        viewer._drc_marker_request = Mock(return_value=[single, group])
        viewer._drc_marker_key = ("render",)
        disp, layer = Mock(), Mock()
        disp.get_width.return_value = disp.get_height.return_value = 100
        pixbuf = SimpleNamespace(Pixbuf=SimpleNamespace(new=Mock(return_value=layer)),
                                 Colorspace=SimpleNamespace(RGB=0),
                                 InterpType=SimpleNamespace(NEAREST=0))
        with patch.object(gui, "GdkPixbuf", pixbuf), \
                patch.object(gui, "fill_rect") as fill:
            viewer._drc_stamp_markers(disp, self.bounds, 10)
            first_calls = fill.call_count
            self.assertEqual(viewer._drc_hit_at(20, 70), (0, 1507))
            self.assertIsNone(viewer._drc_hit_at(80, 50))
            self.assertIs(viewer._drc_group_at(80, 50), group)
            self.assertIsNone(viewer._drc_group_at(20, 70))
            viewer._drc_hits.clear()
            viewer._drc_group_hits.clear()
            viewer._drc_stamp_markers(disp, self.bounds, 10)
            self.assertEqual(fill.call_count, first_calls,
                             "unchanged viewport repainted all marker squares")
            self.assertEqual(len(viewer._drc_hits), 1)
            self.assertEqual(len(viewer._drc_group_hits), 1)
            self.assertEqual(layer.composite.call_count, 2)
            self.assertIn(gui.DRC_GREEN, [call.args[-1] for call in fill.call_args_list])
        self.assertIn("200000 errors (75000 waived)", viewer._drc_group_text(group))

    def test_hidden_overlays_clear_both_pick_lists(self):
        viewer = self.viewer
        viewer.overlay_mode = 2
        viewer._zoomdrag = None
        viewer._drc_hits = [(5, 5, 0, 1)]
        viewer._drc_group_hits = [(6, 6, "group")]
        viewer._draw_overlays(Mock(), self.bounds, 10)
        self.assertEqual(viewer._drc_hits, [])
        self.assertEqual(viewer._drc_group_hits, [])


class BoxSelectionTests(unittest.TestCase):
    def test_box_clicks_and_rubber_band_follow_displayed_frozen_frame(self):
        viewer = viewer_fixture()
        viewer._drc_marker_view = ((200, 300, 1200, 1300), 10, 150, 80)
        viewer.spp = 2
        viewer.mode = "esel"
        viewer._esel_start = None
        viewer._esel_apply = Mock()
        first = SimpleNamespace(x=10, y=20, state=0)
        viewer._update_cursor(first)
        self.assertEqual(viewer._cursor, (300, 1100))
        viewer._esel_click(first)
        self.assertEqual(viewer._esel_start, (300, 1100))
        gdk = SimpleNamespace(ModifierType=SimpleNamespace(CONTROL_MASK=4,
                                                         SHIFT_MASK=1))
        with patch.object(gui, "Gdk", gdk):
            viewer._esel_click(SimpleNamespace(x=30, y=50, state=0))
        viewer._esel_apply.assert_called_once_with((300, 1100), (500, 800),
                                                   "replace")
        self.assertIsNone(viewer._esel_start)
        # Normal canvas interactions continue to use the live view.
        viewer.mode = "normal"
        viewer._update_cursor(first)
        self.assertEqual(viewer._cursor, (20, 9960))

    def test_box_query_clips_to_frozen_drawn_canvas_extent(self):
        viewer = viewer_fixture()
        viewer._drc_marker_view = ((200, 300, 1200, 1300), 10, 150, 80)
        viewer._drc = SimpleNamespace(checks=[SimpleNamespace(name="R")],
                                      query_rect=Mock(return_value=[]))
        viewer._esel_apply((-10000, -10000), (20000, 20000))
        self.assertEqual(viewer._drc.query_rect.call_args.args,
                         (0.2, 0.5, 1.7, 1.3))

    def test_box_selection_reaches_errors_past_grid_page(self):
        viewer = viewer_fixture()
        cluster = object()
        viewer._drc_cluster = cluster
        viewer._drc_wfilter = "waived"
        error = drc.DrcError("p", 1, [(1.0, 2.0)])
        viewer._drc = SimpleNamespace(
            checks=[SimpleNamespace(name="R")],
            query_rect=Mock(return_value=[(0, 1307, error), (0, 9231, error)]),
            get_status=Mock(return_value=drc.STATUS_WAIVED))
        viewer._esel_apply((0, 0), (3000, 4000))
        self.assertEqual(viewer._drc_sel[1], [1307, 9231])
        args, kwargs = viewer._drc.query_rect.call_args
        self.assertEqual(args, (0.0, 0.0, 3.0, 4.0))
        self.assertEqual(kwargs["checks"], (0,))
        self.assertEqual(kwargs["members"], {0: cluster})
        self.assertTrue(kwargs["waived"])
        self.assertGreater(kwargs["cap"], 1000)
        self.assertEqual(viewer._drc_sel[2],
                         [(1307, "p", [(1000.0, 2000.0)]),
                          (9231, "p", [(1000.0, 2000.0)])])

    def test_oversized_box_preserves_entire_previous_selection(self):
        viewer = viewer_fixture()
        old = (0, [9], [(9, "p", [(4.0, 5.0)])], frozenset({9}))
        viewer._drc_sel = old
        viewer._drc_sels[(0, None)] = old
        error = drc.DrcError("p", 1, [(1.0, 2.0)])
        viewer._drc = SimpleNamespace(
            checks=[SimpleNamespace(name="R")],
            query_rect=Mock(side_effect=lambda *args, **kwargs:
                            [(0, ei, error) for ei in range(kwargs["cap"])]))
        viewer._esel_apply((0, 0), (3000, 4000), "replace")
        self.assertIs(viewer._drc_sel, old)
        self.assertIs(viewer._drc_sels[(0, None)], old)
        viewer._drc_grid_fill.assert_not_called()
        self.assertTrue(viewer._set_live_status.called)

    def test_add_overflow_also_preserves_entire_previous_selection(self):
        viewer = viewer_fixture()
        count = gui.DRC_SEL_CAP - 1
        old = (0, list(range(count)),
               [(ei, "p", [(4.0, 5.0)]) for ei in range(count)],
               frozenset(range(count)))
        viewer._drc_sel = old
        error = drc.DrcError("p", 1, [(1.0, 2.0)])
        viewer._drc = SimpleNamespace(
            checks=[SimpleNamespace(name="R")],
            query_rect=Mock(return_value=[(0, count, error), (0, count + 1, error)]))
        viewer._esel_apply((0, 0), (3000, 4000), "add")
        self.assertIs(viewer._drc_sel, old)
        viewer._drc_grid_fill.assert_not_called()

    def test_ascii_box_selection_honors_sparse_cluster_beyond_page(self):
        viewer = viewer_fixture()
        errors = [drc.DrcError("p", ei + 1, [(1.0, 2.0)]) for ei in range(1500)]
        viewer._drc_cluster = Mock()
        viewer._drc_cluster.contains.side_effect = lambda ei: ei in {7, 1409}
        viewer._drc = SimpleNamespace(checks=[SimpleNamespace(name="R", errors=errors)])
        viewer._esel_apply((0, 0), (3000, 4000))
        self.assertEqual(viewer._drc_sel[1], [7, 1409])


class MarkerWorkerTests(unittest.TestCase):
    @staticmethod
    def result(worker):
        deadline = time.monotonic() + 3.0
        while time.monotonic() < deadline:
            result = worker.poll()
            if result is not None:
                return result
            threading.Event().wait(0.002)
        raise AssertionError("marker worker did not publish its result")

    def worker(self):
        worker = worker_mod.MarkerWorker()

        def close():
            worker.close()
            worker._thread.join(3.0)
            self.assertFalse(worker._thread.is_alive(),
                             "closed marker worker remained alive")

        self.addCleanup(close)
        return worker

    def test_latest_pending_request_replaces_intermediate_view(self):
        entered, release, cancelled = (threading.Event() for _ in range(3))
        queried, created = [], []

        class Index:
            def __init__(self, database):
                created.append(database)

            def query(self, token, cancelled):
                queried.append(token)
                if token == "old":
                    entered.set()
                    if not release.wait(3.0):
                        raise AssertionError("test did not release query")
                    if cancelled():
                        cancel_seen.set()
                        raise MarkerQueryCancelled()
                return [token]

        cancel_seen = cancelled
        with patch.object(worker_mod, "MarkerIndex", Index):
            worker = self.worker()
            database = object()
            worker.submit("old-view", database, {"token": "old"})
            self.assertTrue(entered.wait(3.0))
            worker.submit("middle-view", database, {"token": "middle"})
            worker.submit("latest-view", database, {"token": "latest"})
            release.set()
            self.assertEqual(self.result(worker),
                             ("latest-view", ["latest"], None))
            self.assertTrue(cancel_seen.is_set())
            self.assertEqual(queried, ["old", "latest"])
            self.assertEqual(created, [database])
            self.assertIsNone(worker.poll(), "worker repeated a consumed result")

    def test_cancelled_scope_never_publishes_old_result(self):
        entered, release, settled = (threading.Event() for _ in range(3))

        class Index:
            def __init__(self, database):
                pass

            def query(self, cancelled):
                entered.set()
                if not release.wait(3.0):
                    raise AssertionError("test did not release query")
                # A backend may finish between cancellation checkpoints;
                # the worker must discard this successful stale return.
                return ["stale"]

        with patch.object(worker_mod, "MarkerIndex", Index):
            worker = self.worker()
            worker.submit("first-scope", object(), {})
            self.assertTrue(entered.wait(3.0))
            original_wait = worker._condition.wait

            def wait_after_query(timeout=None):
                settled.set()
                return original_wait(timeout)

            # Signal the worker's next idle wait, after result publication
            # would have happened, so the assertion cannot race its return.
            with worker._condition:
                worker._condition.wait = wait_after_query
            worker.cancel()
            release.set()
            self.assertTrue(settled.wait(3.0))
            self.assertIsNone(worker.poll())

    def test_error_then_new_database_recovers(self):
        created = []

        class Index:
            def __init__(self, database):
                created.append(database)

            def query(self, fail=False, cancelled=None):
                if fail:
                    raise ValueError("bad marker geometry")
                return ["fresh"]

        with patch.object(worker_mod, "MarkerIndex", Index):
            worker = self.worker()
            first, second = object(), object()
            worker.submit("bad", first, {"fail": True})
            self.assertEqual(self.result(worker),
                             ("bad", None, "bad marker geometry"))
            worker.submit("good", second, {})
            self.assertEqual(self.result(worker), ("good", ["fresh"], None))
            self.assertEqual(created, [first, second])

    def test_selected_members_preserve_sparse_cluster_intersection(self):
        cluster = SimpleNamespace(contains=lambda ei: ei in {2, 9, 2001})
        members = worker_mod.SelectedMembers([2001, 9, 3, 2, 9], cluster)
        self.assertEqual(members.mask(0, 12).nonzero()[0].tolist(), [2, 9])
        self.assertEqual(members.mask(2000, 3).tolist(), [False, True, False])
        self.assertEqual(members.mask(12, 0).tolist(), [])

    def test_idle_and_cancelled_queries_release_delta_membership_snapshots(self):
        class Membership:
            pass

        class Index:
            def __init__(self, database):
                self.database = database

            def query(self, members, cancelled=None):
                return []

        def collected(reference):
            deadline = time.monotonic() + 3
            while time.monotonic() < deadline:
                gc.collect()
                if reference() is None:
                    return True
                threading.Event().wait(0.002)
            return False

        with patch.object(worker_mod, "MarkerIndex", Index):
            worker = self.worker()
            database = object()
            membership = Membership()
            reference = weakref.ref(membership)
            # Both the result key and query map can retain DeltaGroups and
            # their million-row measured arrays in the real viewer.
            worker.submit(("scope", membership), database,
                          {"members": {0: membership}})
            del membership
            self.assertIsNone(self.result(worker)[2])
            self.assertTrue(collected(reference),
                            "idle worker retained consumed membership scope")
            membership = Membership()
            reference = weakref.ref(membership)
            worker.submit(("scope", membership), database,
                          {"members": {0: membership}})
            del membership
            worker.cancel()
            self.assertTrue(collected(reference),
                            "cancelled worker retained old membership scope")


if __name__ == "__main__":
    unittest.main(verbosity=2)
