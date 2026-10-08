"""Headless GUI regression checks for full-scope individual DRC markers.

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
import numpy as np
from types import SimpleNamespace
from unittest.mock import Mock, patch

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from floe import drc, gui  # noqa: E402
from floe import drc_marker_worker as worker_mod  # noqa: E402
from floe.drc_marker_style import circle_rgba  # noqa: E402
from floe.drc_markers import MarkerQueryCancelled  # noqa: E402
from floe.drc_points import PointMarkers  # noqa: E402
from floe.drc_selection import Selection, SelectionProgress  # noqa: E402


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
    viewer._drc_point_hits = None
    viewer._drc_ruler = []
    viewer.rulers = []
    viewer.drc_mark = None
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


class PointViewerTests(unittest.TestCase):
    @staticmethod
    def points(width=100, height=100):
        error_ids = np.full((height, width), -1, dtype=np.int64)
        check_ids = np.full((height, width), -1, dtype=np.int32)
        error_ids[20, 30], check_ids[20, 30] = 15000, 0
        error_ids[40, 60], check_ids[40, 60] = 90000000, 2
        return PointMarkers(width, height, bytes(width * height * 4),
                            error_ids, check_ids, 100000000, 2)


    def test_keyboard_u_is_unbound_and_preserves_marker_state(self):
        viewer = viewer_fixture()
        viewer._gdlg = None
        viewer.window = Mock()
        viewer.window.get_focus.return_value = None
        viewer._command_key = Mock()
        viewer._drc_marker_result = result = (("view",), self.points())
        viewer._drc_marker_worker = Mock()
        viewer._drc_page = 27
        entry = type('Entry', (), {})
        gdk = SimpleNamespace(ModifierType=SimpleNamespace(
            CONTROL_MASK=4, SHIFT_MASK=1, MOD1_MASK=8))
        with patch.object(gui, 'Gtk', SimpleNamespace(Entry=entry)), patch.object(gui, 'Gdk', gdk):
            for name in ('u', 'U'):
                for state in (0, 1, 4, 8):
                    viewer._command_key.return_value = name
                    self.assertFalse(viewer._on_key(None, SimpleNamespace(state=state)))
            viewer.window.get_focus.return_value = entry()
            self.assertFalse(viewer._on_key(None, SimpleNamespace(state=0)))
        self.assertIs(viewer._drc_marker_result, result)
        self.assertEqual(viewer._drc_page, 27)
        viewer._drc_marker_worker.cancel.assert_not_called()
        viewer._display.assert_not_called()


    def test_point_layer_composites_once_without_per_marker_gui_stamping(self):
        viewer = viewer_fixture()
        points = self.points()
        viewer._drc_marker_key = ('individual',)
        viewer._drc_marker_request = Mock(return_value=points)
        layer = Mock()
        make = Mock(return_value=layer)
        pixbuf = SimpleNamespace(Pixbuf=SimpleNamespace(new_from_bytes=make),
                                 Colorspace=SimpleNamespace(RGB=0),
                                 InterpType=SimpleNamespace(NEAREST=0))
        disp = Mock()
        disp.get_width.return_value = disp.get_height.return_value = 100
        with patch.object(gui, 'GdkPixbuf', pixbuf), \
                patch.object(gui, 'GLib', SimpleNamespace(Bytes=SimpleNamespace(new=lambda b: b))), \
                patch.object(gui, 'stamp_drc_circle') as circle:
            viewer._drc_stamp_markers(disp, (0, 0, 1000, 1000), 10)
            viewer._drc_stamp_markers(disp, (0, 0, 1000, 1000), 10)
        make.assert_called_once()
        self.assertEqual(layer.composite.call_count, 2)
        circle.assert_not_called()
        self.assertIs(viewer._drc_point_hits, points)
        self.assertEqual(viewer._drc_hits, [])

    def test_point_picking_is_local_and_focus_retains_priority(self):
        viewer = viewer_fixture()
        viewer._drc_point_hits = self.points()
        self.assertEqual(viewer._drc_hit_at(30, 20), (0, 15000))
        self.assertEqual(viewer._drc_hit_at(65, 40), (2, 90000000))
        self.assertIsNone(viewer._drc_hit_at(67, 40))
        self.assertIsNone(viewer._drc_hit_at(-100, -100))
        viewer._drc_hits = [(30, 20, 0, 42)]
        self.assertEqual(viewer._drc_hit_at(30, 20), (0, 42))
        viewer.overlay_mode = 2
        viewer._zoomdrag = None
        viewer._draw_overlays(Mock(), (0, 0, 1000, 1000), 10)
        self.assertIsNone(viewer._drc_hit_at(30, 20))

    def test_point_info_uses_population_and_deduplicated_count(self):
        viewer = viewer_fixture()
        viewer._drcwin = SimpleNamespace(_info=Mock())
        viewer._drc = SimpleNamespace(path='points.db', cell='MAIN', checks=[object()], total=100000000)
        viewer._drc_rmeta = viewer._drc_clusters = None
        viewer._drc_shown = 1
        viewer._drc_marker_result = ('points', self.points())
        gui.Viewer._drc_info_refresh(viewer)
        text = viewer._drcwin._info.set_text.call_args.args[0]
        self.assertNotIn('(U)', text)
        self.assertNotIn('ungrouped', text.lower())
        self.assertIn('100000000 errors / 2 markers in view', text)


class MarkerCircleStyleTests(unittest.TestCase):


    def test_solid_singleton_disk_has_opaque_center_and_transparent_corners(self):
        for radius in (2, 4):
            with self.subTest(radius=radius):
                side = 2 * radius + 1
                pixels = circle_rgba(radius, gui.DRC_RED, solid=True)
                center = (radius * side + radius) * 4
                self.assertEqual(tuple(pixels[center:center + 4]),
                                 (255, 82, 82, 255))
                self.assertEqual(pixels[3], 0)
                self.assertEqual(pixels[-1], 0)
                self.assertTrue(any(0 < a < 255 for a in pixels[3::4]))

    def test_sprite_cache_reuses_identical_styles_and_clips_canvas_edges(self):
        buf, sprite = Mock(), Mock()
        buf.get_width.return_value = 100
        buf.get_height.return_value = 80
        make = Mock(return_value=sprite)
        pixbuf = SimpleNamespace(Pixbuf=SimpleNamespace(new_from_bytes=make),
                                 Colorspace=SimpleNamespace(RGB=0),
                                 InterpType=SimpleNamespace(NEAREST=0))
        glib = SimpleNamespace(Bytes=SimpleNamespace(new=lambda data: data))
        sprites = {}
        with patch.object(gui, "GdkPixbuf", pixbuf), patch.object(gui, "GLib", glib):
            gui.stamp_drc_circle(buf, 0, 0, 4, gui.DRC_RED, sprites=sprites, solid=True)
            self.assertEqual(make.call_count, 1)
            self.assertEqual(make.call_args.args[1:], (0, True, 8, 9, 9, 36))
            self.assertEqual(len(make.call_args.args[0]), 9 * 9 * 4)
            sprite.composite.assert_called_once_with(
                buf, 0, 0, 5, 5, -4, -4, 1, 1, 0, 255)
            gui.stamp_drc_circle(buf, 99, 79, 4, gui.DRC_RED, sprites=sprites, solid=True)
            self.assertEqual(make.call_count, 1, "same style rebuilt its sprite")
            self.assertEqual(sprite.composite.call_args.args,
                             (buf, 95, 75, 5, 5, 95, 75, 1, 1, 0, 255))
            gui.stamp_drc_circle(buf, 200, 200, 4, gui.DRC_RED, sprites=sprites, solid=True)
            self.assertEqual(sprite.composite.call_count, 2)
            gui.stamp_drc_circle(buf, 50, 50, 2, gui.DRC_RED, sprites=sprites, solid=True)
            gui.stamp_drc_circle(buf, 50, 50, 4, gui.DRC_GREEN, sprites=sprites, solid=True)
            self.assertEqual(make.call_count, 3,
                             "different size/review colors shared a sprite")
            self.assertEqual(len(sprites), 3)
            center = (4 * 9 + 4) * 4
            self.assertEqual(make.call_args.args[0][center + 3], 255)


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
        markers = PointViewerTests.points()
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
        self.assertTrue(query["progressive"])
        for removed in ("ungrouped", "cell_px", "declutter"):
            self.assertNotIn(removed, query)

    @staticmethod
    def progress_points(processed, total=100000000, occupied=2):
        points = PointViewerTests.points()
        return PointMarkers(points.width, points.height, points.rgba,
                            points.error_ids, points.check_ids,
                            min(processed, 1234567), occupied,
                            processed_count=processed, total_count=total)

    def test_point_progress_fields_preserve_seven_argument_constructor(self):
        points = PointViewerTests.points()
        self.assertEqual((points.processed_count, points.total_count), (0, 0))

    def test_partial_frames_repaint_same_key_until_final_and_keep_busy(self):
        viewer = self.viewer
        self.request()
        self.assertTrue(viewer._drc_marker_worker.submit.call_args.args[2]["progressive"])
        key = viewer._drc_marker_key
        frames = [self.progress_points(1000000), self.progress_points(27000000),
                  self.progress_points(100000000)]
        layers = [Mock(), Mock(), Mock()]
        create = Mock(side_effect=layers)
        pixbuf = SimpleNamespace(Pixbuf=SimpleNamespace(new_from_bytes=create),
                                 Colorspace=SimpleNamespace(RGB=0),
                                 InterpType=SimpleNamespace(NEAREST=0))
        disp = Mock()
        disp.get_width.return_value = disp.get_height.return_value = 100
        viewer._display.side_effect = lambda: viewer._drc_stamp_markers(
            disp, self.bounds, 10)
        with patch.object(gui, "GdkPixbuf", pixbuf), patch.object(
                gui, "GLib", SimpleNamespace(Bytes=SimpleNamespace(new=lambda value: value))):
            for number, frame in enumerate(frames):
                partial = number < 2
                payload = worker_mod.MarkerProgress(frame) if partial else frame
                viewer._drc_marker_worker.poll.return_value = key, payload, None
                viewer._drc_marker_poll()
                self.assertEqual(viewer._drc_marker_busy, partial)
                self.assertIs(viewer._drc_marker_result[1], frame)
                self.assertIs(viewer._drc_point_hits, frame)
                self.assertEqual(viewer._drc_marker_key, key)
                self.assertIs(viewer._drc_marker_overlay[1], layers[number])
                self.assertEqual(create.call_count, number + 1,
                                 "same-key progress reused an earlier raster")
        viewer._drc_marker_worker.submit.assert_called_once()
        self.assertEqual(viewer._display.call_count, 3)
        self.assertEqual(viewer._drc_info_refresh.call_count, 4)


    def test_partial_info_shows_processed_population_and_current_marker_count(self):
        viewer = self.viewer
        viewer._drcwin = SimpleNamespace(_info=Mock())
        viewer._drc.path, viewer._drc.cell, viewer._drc.total = "progress.db", "MAIN", 100000000
        viewer._drc_rmeta = viewer._drc_clusters = None
        viewer._drc_shown = 1
        viewer._drc_marker_busy = True
        viewer._drc_marker_result = ("progress", self.progress_points(27000000, occupied=17))
        gui.Viewer._drc_info_refresh(viewer)
        text = viewer._drcwin._info.set_text.call_args.args[0]
        compact = text.replace(",", "").replace(" ", "")
        self.assertIn("27000000/100000000", compact)
        self.assertIn("17 markers", text)
        self.assertTrue(viewer._drc_marker_busy,
                        "showing partial counts must not imply completion")

    def test_error_after_partial_clears_raster_and_picks(self):
        viewer = self.viewer
        self.request()
        key = viewer._drc_marker_key
        partial = self.progress_points(25000000)
        viewer._drc_marker_worker.poll.return_value = (
            key, worker_mod.MarkerProgress(partial), None)
        viewer._drc_marker_poll()
        viewer._drc_marker_overlay = object()
        viewer._drc_point_hits = partial
        viewer._drc_hits = [(30, 20, 0, 15000)]
        viewer._drc_marker_worker.poll.return_value = (key, None, "center scan failed")
        viewer._drc_marker_poll()
        self.assertFalse(viewer._drc_marker_busy)
        if viewer._drc_marker_result is not None:
            self.assertEqual(viewer._drc_marker_result, (key, []))
        self.assertIsNone(viewer._drc_marker_overlay)
        self.assertIsNone(viewer._drc_point_hits)
        self.assertEqual(viewer._drc_hits, [])
        self.assertIn("center scan failed", viewer._set_live_status.call_args.args[0])
        self.assertEqual(viewer._display.call_count, 2)

    def test_rule_view_and_review_changes_ignore_late_partial_frames(self):
        for change in ("rule", "view", "review"):
            with self.subTest(change=change):
                self.setUp()
                viewer = self.viewer
                self.request()
                stale_key = viewer._drc_marker_key
                old = self.progress_points(1000000)
                viewer._drc_marker_result = stale_key, old
                if change == "rule":
                    viewer._drc.checks.append(SimpleNamespace(name="R2"))
                    viewer._drc_open = 1
                elif change == "view":
                    self.bounds = (1000, 500, 2000, 1500)
                else:
                    viewer._drc_marker_invalidate()
                self.request()
                current_key = viewer._drc_marker_key
                self.assertNotEqual(current_key, stale_key)
                current_frame = self.progress_points(35000000)
                current_result = current_key, current_frame
                current_overlay = object()
                viewer._drc_marker_result = current_result
                viewer._drc_marker_overlay = current_overlay
                viewer._drc_point_hits = current_frame
                viewer._display.reset_mock()
                viewer._drc_info_refresh.reset_mock()
                viewer._drc_marker_worker.poll.return_value = (
                    stale_key, worker_mod.MarkerProgress(old), None)
                viewer._drc_marker_poll()
                self.assertIs(viewer._drc_marker_result, current_result)
                self.assertIs(viewer._drc_marker_overlay, current_overlay)
                self.assertIs(viewer._drc_point_hits, current_frame)
                self.assertTrue(viewer._drc_marker_busy)
                viewer._display.assert_not_called()
                viewer._drc_info_refresh.assert_not_called()

    def test_scope_change_drops_cached_markers_and_stale_worker_reply(self):
        viewer = self.viewer
        self.request()
        old_key = viewer._drc_marker_key
        old = PointViewerTests.points()
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
        current = PointViewerTests.points()
        viewer._drc_marker_worker.poll.return_value = (new_key, current, None)
        viewer._drc_marker_poll()
        self.assertEqual(viewer._drc_marker_result[0], new_key)
        self.assertIs(viewer._drc_marker_result[1], current)
        viewer._display.assert_called_once()

    def test_status_revision_invalidates_even_without_population_change(self):
        viewer = self.viewer
        self.request()
        old_key = viewer._drc_marker_key
        viewer._drc_marker_result = (old_key, PointViewerTests.points())
        viewer._drc_hits = [(5, 5, 0, 1)]
        viewer._drc_point_hits = viewer._drc_marker_result[1]
        viewer._drc_marker_invalidate()
        self.assertEqual(viewer._drc_hits, [])
        self.assertIsNone(viewer._drc_point_hits)
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
        viewer._drc_marker_result = viewer._drc_marker_key, PointViewerTests.points()
        viewer._drc.set_status = Mock(side_effect=[None, OSError("disk full")])
        viewer._drc_clusters = None
        viewer._drc_set_waived(0, [1, 2], True)
        self.assertIsNone(viewer._drc_marker_result)
        viewer._drc_marker_worker.cancel.assert_called_once()

    def test_selected_filter_and_waive_filter_intersect_with_cluster(self):
        viewer = self.viewer
        viewer._drc_show_sel = True
        viewer._drc_wfilter = "waived"
        viewer._drc_cluster = Selection.from_indices(10000, [7, 9017])
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
        viewer._drc_marker_result = first_key, PointViewerTests.points()
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
        self.assertTrue(query["progressive"])
        self.assertNotIn("cell_px", query)
        self.assertNotIn("declutter", query)


    def test_hidden_overlays_clear_focused_and_raster_picks(self):
        viewer = self.viewer
        viewer.overlay_mode = 2
        viewer._zoomdrag = None
        viewer._drc_hits = [(5, 5, 0, 1)]
        viewer._drc_point_hits = PointViewerTests.points()
        viewer._draw_overlays(Mock(), self.bounds, 10)
        self.assertEqual(viewer._drc_hits, [])
        self.assertIsNone(viewer._drc_point_hits)


    def test_focused_error_marker_is_a_larger_circle(self):
        viewer = self.viewer
        viewer._drc_focus = (0, 11, "p", [(100, 100)])
        viewer._drc_pos = -1
        viewer._drc_cum = []
        disp = Mock()
        marks = [(0, 11, "p", [(100, 100)]),
                 (0, 12, "p", [(200, 200)])]
        with patch.object(gui, "stamp_drc_circle") as circle, \
                patch.object(gui, "fill_rect") as fill:
            viewer._drc_stamp_errs(disp, lambda x: x, lambda y: y,
                                  marks, gui.DRC_RED)
        self.assertEqual(circle.call_count, 2)
        self.assertEqual(circle.call_args_list[0].args[:5],
                         (disp, 100, 100, 4, gui.DRC_RED))
        self.assertEqual(circle.call_args_list[1].args[:5],
                         (disp, 200, 200, gui.DRC_MARK_PX // 2, gui.DRC_RED))
        self.assertTrue(all(call.kwargs["solid"] for call in circle.call_args_list))
        fill.assert_not_called()
        self.assertEqual(viewer._drc_hits, [(100, 100, 0, 11), (200, 200, 0, 12)])


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
        viewer._drc = SimpleNamespace(checks=[SimpleNamespace(name="R", errors=range(10000))])
        viewer._drc_selection_worker = Mock()
        viewer._esel_apply((-10000, -10000), (20000, 20000))
        self.assertEqual(viewer._drc_selection_worker.submit.call_args.args[3],
                         (0.2, 0.5, 1.7, 1.3))

    def test_box_selection_reaches_errors_past_grid_page(self):
        viewer = viewer_fixture()
        cluster = Selection.from_indices(10000, [1307, 9231])
        viewer._drc_cluster = cluster
        viewer._drc_wfilter = "waived"
        viewer._drc = SimpleNamespace(
            checks=[SimpleNamespace(name="R", errors=range(10000))],
            query_rect=Mock(side_effect=AssertionError("foreground box query")),
            get_status=Mock(return_value=drc.STATUS_WAIVED))
        viewer._drc_selection_worker = worker = Mock()
        viewer._esel_apply((0, 0), (3000, 4000))
        self.assertIsNone(viewer._drc_sel)
        args, kwargs = worker.submit.call_args
        self.assertEqual(args[2:], (0, (0.0, 0.0, 3.0, 4.0)))
        self.assertIs(kwargs["membership"], cluster)
        self.assertTrue(kwargs["waived"])
        self.assertNotIn("cap", kwargs)
        worker.poll.return_value = (args[0], cluster, None)
        viewer._drc_selection_poll()
        self.assertEqual(viewer._drc_sel[1].page(0, 1000), [1307, 9231])
        self.assertEqual(viewer._drc_sel[2], (), "selection retained decoded geometry")
        self.assertIs(viewer._drc_sel[3], cluster)

    def selected_viewer(self, size=20003):
        viewer = viewer_fixture()
        previous = Selection.from_indices(size, [9])
        old = (0, previous, (), previous)
        viewer._drc_sel = old
        viewer._drc_sels[(0, None)] = old
        viewer._drc = SimpleNamespace(
            checks=[SimpleNamespace(name="R", errors=range(size))])
        viewer._drc_selection_worker = Mock()
        return viewer, old

    def test_more_than_5000_matches_replace_selection_atomically(self):
        viewer, old = self.selected_viewer()
        viewer._esel_apply((0, 0), (3000, 4000), "replace")
        worker = viewer._drc_selection_worker
        key = worker.submit.call_args.args[0]
        self.assertIs(viewer._drc_sel, old)
        worker.poll.return_value = key, SelectionProgress(10000, 20003, 10000), None
        viewer._drc_selection_poll()
        self.assertIs(viewer._drc_sel, old)
        viewer._drc_grid_fill.assert_not_called()
        selected = Selection.from_indices(20003, range(20003))
        worker.poll.return_value = key, selected, None
        viewer._drc_selection_poll()
        self.assertEqual(len(viewer._drc_sel[1]), 20003)
        self.assertIs(viewer._drc_sel[1], selected)
        self.assertEqual(viewer._drc_sel[2], ())
        viewer._drc_grid_fill.assert_called_once_with(0)

    def test_add_and_toggle_delegate_previous_membership_without_geometry(self):
        for mode in ("add", "toggle"):
            with self.subTest(mode=mode):
                viewer, old = self.selected_viewer()
                viewer._esel_apply((0, 0), (3000, 4000), mode)
                kwargs = viewer._drc_selection_worker.submit.call_args.kwargs
                self.assertIs(kwargs["previous"], old[1])
                self.assertEqual(kwargs["mode"], mode)
                self.assertIs(viewer._drc_sel, old)

    def test_failed_or_cancelled_query_preserves_previous_selection(self):
        for cancelled in (False, True):
            with self.subTest(cancelled=cancelled):
                viewer, old = self.selected_viewer()
                viewer._esel_apply((0, 0), (3000, 4000))
                worker = viewer._drc_selection_worker
                key = worker.submit.call_args.args[0]
                if cancelled:
                    viewer._drc_selection_cancel()
                    worker.poll.return_value = key, Selection.empty(20003), None
                else:
                    worker.poll.return_value = key, None, "query failed"
                viewer._drc_selection_poll()
                self.assertIs(viewer._drc_sel, old)
                self.assertIs(viewer._drc_sels[(0, None)], old)
                viewer._drc_grid_fill.assert_not_called()

    def test_scope_change_discards_late_selection_without_ui_change(self):
        for attribute, value in (("_drc_open", 1), ("_drc_wfilter", "waived")):
            with self.subTest(attribute=attribute):
                viewer, old = self.selected_viewer()
                viewer._esel_apply((0, 0), (3000, 4000))
                worker = viewer._drc_selection_worker
                key = worker.submit.call_args.args[0]
                setattr(viewer, attribute, value)
                worker.poll.return_value = key, Selection.empty(20003), None
                viewer._drc_selection_poll()
                self.assertIs(viewer._drc_sel, old)
                viewer._drc_grid_fill.assert_not_called()
                viewer._display.assert_not_called()

    def test_ascii_box_selection_honors_sparse_cluster_beyond_page(self):
        viewer = viewer_fixture()
        errors = [drc.DrcError("p", ei + 1, [(1.0, 2.0)]) for ei in range(1500)]
        viewer._drc_cluster = Selection.from_indices(1500, [7, 1409])
        viewer._drc = SimpleNamespace(checks=[SimpleNamespace(name="R", errors=errors)])
        viewer._esel_apply((0, 0), (3000, 4000))
        worker = viewer._drc_selection_worker
        self.addCleanup(worker.close)
        deadline = time.monotonic() + 3
        while viewer._drc_selection_key is not None and time.monotonic() < deadline:
            viewer._drc_selection_poll()
            threading.Event().wait(.002)
        worker.close()
        worker._thread.join(3)
        self.assertFalse(worker._thread.is_alive())
        self.assertEqual(viewer._drc_sel[1].page(0, 1000), [7, 1409])


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

    @staticmethod
    def next_idle(worker):
        """Observe publication completion without sleeping for a timer tick."""
        settled = threading.Event()
        original_wait = worker._condition.wait

        def wait_after_query(timeout=None):
            settled.set()
            return original_wait(timeout)

        with worker._condition:
            worker._condition.wait = wait_after_query
        return settled

    def test_progress_keeps_one_latest_snapshot_and_final_replaces_pending_preview(self):
        published, release = threading.Event(), threading.Event()
        references = []

        class Snapshot(list):
            pass

        class Index:
            def __init__(self, database):
                pass

            def query(self, cancelled, progress):
                for number in range(8):
                    snapshot = Snapshot([number])
                    references.append(weakref.ref(snapshot))
                    progress(snapshot)
                del snapshot
                published.set()
                if not release.wait(3.0):
                    raise AssertionError("test did not release progressive query")
                progress(["unconsumed final preview"])
                return ["complete"]

        with patch.object(worker_mod, "MarkerIndex", Index):
            worker = self.worker()
            self.addCleanup(release.set)
            worker.submit("view", object(), {"progressive": True})
            self.assertTrue(published.wait(3.0))
            result = self.result(worker)
            self.assertEqual(result[0], "view")
            self.assertIsInstance(result[1], worker_mod.MarkerProgress)
            self.assertEqual(result[1].markers, [7])
            self.assertIsNone(result[2])
            gc.collect()
            self.assertTrue(all(reference() is None for reference in references[:-1]),
                            "worker retained a queue of intermediate frames")
            self.assertIsNone(worker.poll(), "poll did not consume the latest snapshot")
            settled = self.next_idle(worker)
            release.set()
            self.assertTrue(settled.wait(3.0))
            self.assertEqual(worker.poll(), ("view", ["complete"], None),
                             "last preview overwrote or hid the completed result")
            self.assertIsNone(worker.poll())

    def test_progressive_error_replaces_the_visible_partial(self):
        published, release = threading.Event(), threading.Event()

        class Index:
            def __init__(self, database):
                pass

            def query(self, cancelled, progress):
                progress(["partial"])
                published.set()
                if not release.wait(3.0):
                    raise AssertionError("test did not release failing query")
                raise ValueError("center scan failed")

        with patch.object(worker_mod, "MarkerIndex", Index):
            worker = self.worker()
            self.addCleanup(release.set)
            worker.submit("view", object(), {"progressive": True})
            self.assertTrue(published.wait(3.0))
            self.assertIsInstance(self.result(worker)[1], worker_mod.MarkerProgress)
            settled = self.next_idle(worker)
            release.set()
            self.assertTrue(settled.wait(3.0))
            self.assertEqual(worker.poll(), ("view", None, "center scan failed"))

    def test_cancel_rejects_a_backend_that_publishes_progress_after_cancellation(self):
        published, release, attempted = (threading.Event() for _ in range(3))

        class Index:
            def __init__(self, database):
                pass

            def query(self, cancelled, progress):
                progress(["initial"])
                published.set()
                if not release.wait(3.0):
                    raise AssertionError("test did not release cancelled query")
                # Deliberately ignore cancelled(): the worker must reject it.
                try:
                    progress(["stale after cancellation"])
                finally:
                    attempted.set()
                return ["stale final"]

        with patch.object(worker_mod, "MarkerIndex", Index):
            worker = self.worker()
            self.addCleanup(release.set)
            worker.submit("old", object(), {"progressive": True})
            self.assertTrue(published.wait(3.0))
            settled = self.next_idle(worker)
            worker.cancel()
            self.assertIsNone(worker.poll(), "cancel retained an already published preview")
            release.set()
            self.assertTrue(attempted.wait(3.0))
            self.assertTrue(settled.wait(3.0))
            self.assertIsNone(worker.poll(), "cancelled progress escaped its generation")

    def test_replacement_scope_rejects_old_progress_and_keeps_the_new_query(self):
        entered, release_old, new_entered, release_new = (threading.Event() for _ in range(4))
        attempted = threading.Event()

        class Index:
            def __init__(self, database):
                pass

            def query(self, token, cancelled, progress):
                if token == "old":
                    progress(["old initial"])
                    entered.set()
                    if not release_old.wait(3.0):
                        raise AssertionError("test did not release old query")
                    try:
                        progress(["old late preview"])
                    finally:
                        attempted.set()
                    return ["old final"]
                new_entered.set()
                if not release_new.wait(3.0):
                    raise AssertionError("test did not release new query")
                progress(["new preview"])
                return ["new final"]

        with patch.object(worker_mod, "MarkerIndex", Index):
            worker = self.worker()
            self.addCleanup(release_old.set)
            self.addCleanup(release_new.set)
            database = object()
            worker.submit("old-view", database, {"token": "old", "progressive": True})
            self.assertTrue(entered.wait(3.0))
            worker.submit("new-view", database, {"token": "new", "progressive": True})
            release_old.set()
            self.assertTrue(attempted.wait(3.0))
            self.assertTrue(new_entered.wait(3.0))
            self.assertIsNone(worker.poll(), "obsolete preview appeared in the replacement scope")
            settled = self.next_idle(worker)
            release_new.set()
            self.assertTrue(settled.wait(3.0))
            self.assertEqual(worker.poll(), ("new-view", ["new final"], None))

    def test_explicit_nonprogressive_query_preserves_existing_backend_signature(self):
        class Index:
            def __init__(self, database):
                pass

            def query(self, cancelled):
                return ["complete"]

        with patch.object(worker_mod, "MarkerIndex", Index):
            worker = self.worker()
            worker.submit("view", object(), {"progressive": False})
            self.assertEqual(self.result(worker), ("view", ["complete"], None))

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
        cluster = Selection.from_indices(3000, [2, 9, 2001])
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
