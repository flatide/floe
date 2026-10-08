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
import numpy as np
from types import SimpleNamespace
from unittest.mock import Mock, patch

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from floe import drc, gui  # noqa: E402
from floe import drc_marker_worker as worker_mod  # noqa: E402
from floe.drc_marker_style import aggregate_radius, circle_rgba, marker_cell_px  # noqa: E402
from floe.drc_markers import Marker, MarkerQueryCancelled  # noqa: E402
from floe.drc_points import PointMarkers  # noqa: E402


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


class UngroupedViewerTests(unittest.TestCase):
    @staticmethod
    def points(width=100, height=100):
        error_ids = np.full((height, width), -1, dtype=np.int64)
        check_ids = np.full((height, width), -1, dtype=np.int32)
        error_ids[20, 30], check_ids[20, 30] = 15000, 0
        error_ids[40, 60], check_ids[40, 60] = 90000000, 2
        return PointMarkers(width, height, bytes(width * height * 4),
                            error_ids, check_ids, 100000000, 2)

    def test_toggle_cancels_pending_query_without_changing_list_scope(self):
        viewer = viewer_fixture()
        viewer._drc_marker_worker = Mock()
        viewer._drc_cluster = scope = object()
        viewer._drc_page = 27
        viewer._drc_marker_result = ('old', [])
        viewer._drc_marker_overlay = object()
        viewer._drc_point_hits = self.points()
        viewer._drc_group_selected = object()
        viewer._drc_toggle_grouping()
        self.assertTrue(viewer._drc_ungrouped)
        self.assertIsNone(viewer._drc_marker_result)
        self.assertIsNone(viewer._drc_marker_overlay)
        self.assertIsNone(viewer._drc_point_hits)
        self.assertIsNone(viewer._drc_group_selected)
        self.assertIs(viewer._drc_cluster, scope)
        self.assertEqual(viewer._drc_page, 27)
        viewer._drc_grid_fill.assert_not_called()
        viewer._drc_marker_worker.cancel.assert_called_once()
        viewer._drc_toggle_grouping()
        self.assertFalse(viewer._drc_ungrouped)

    def test_keyboard_u_preserves_typing_and_modifier_chords(self):
        viewer = viewer_fixture()
        viewer._gdlg = None
        viewer.window = Mock()
        viewer.window.get_focus.return_value = None
        viewer._drc_toggle_grouping = Mock()
        viewer._command_key = Mock()
        entry = type('Entry', (), {})
        gdk = SimpleNamespace(ModifierType=SimpleNamespace(
            CONTROL_MASK=4, SHIFT_MASK=1, MOD1_MASK=8))
        with patch.object(gui, 'Gtk', SimpleNamespace(Entry=entry)), patch.object(gui, 'Gdk', gdk):
            for name, state in (('u', 0), ('U', 1)):
                viewer._command_key.return_value = name
                self.assertTrue(viewer._on_key(None, SimpleNamespace(state=state)))
            self.assertEqual(viewer._drc_toggle_grouping.call_count, 2)
            for state in (4, 8):
                self.assertFalse(viewer._on_key(None, SimpleNamespace(state=state)))
            viewer.window.get_focus.return_value = entry()
            self.assertFalse(viewer._on_key(None, SimpleNamespace(state=0)))
            self.assertEqual(viewer._drc_toggle_grouping.call_count, 2)

    def test_mode_is_query_identity_and_discards_old_group_reply(self):
        viewer = viewer_fixture()
        viewer._drc = object()
        viewer._drc_marker_worker = Mock()
        viewer._drc_marker_request((0, 0, 1000, 1000), 10, 100, 100)
        old_key, _, old_query = viewer._drc_marker_worker.submit.call_args.args
        self.assertFalse(old_query['ungrouped'])
        viewer._drc_toggle_grouping()
        viewer._drc_marker_request((0, 0, 1000, 1000), 10, 100, 100)
        key, _, query = viewer._drc_marker_worker.submit.call_args.args
        self.assertNotEqual(key, old_key)
        self.assertTrue(query['ungrouped'])
        self.assertEqual(query['checks'], (0,))
        self.assertNotIn('cap', query)
        viewer._drc_marker_worker.poll.return_value = (old_key, [], None)
        viewer._drc_marker_poll()
        self.assertIsNone(viewer._drc_marker_result)

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
        self.assertEqual(viewer._drc_group_hits, [])

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
        viewer._drc_ungrouped = True
        viewer._drc_marker_result = ('points', self.points())
        gui.Viewer._drc_info_refresh(viewer)
        text = viewer._drcwin._info.set_text.call_args.args[0]
        self.assertIn('ungrouped (U)', text)
        self.assertIn('100000000 errors / 2 markers in view', text)


class MarkerCircleStyleTests(unittest.TestCase):
    def test_count_bands_have_distinct_sizes_and_no_within_band_jitter(self):
        bands = ((2, 9, 4), (10, 49, 5), (50, 99, 6),
                 (100, 999, 7), (1000, 9999, 8),
                 (10000, 99999, 9), (100000, 999999, 10),
                 (1000000, 9999999, 11))
        for first, last, radius in bands:
            with self.subTest(band=(first, last)):
                self.assertEqual(aggregate_radius(first, 1200, 800), radius)
                self.assertEqual(aggregate_radius(last, 1200, 800), radius)

    def test_viewport_scale_uses_short_side_and_keeps_size_bounded(self):
        radius = aggregate_radius(1000, 1200, 800)
        self.assertEqual(radius, aggregate_radius(1000, 800, 1200))
        self.assertEqual(radius, aggregate_radius(1000, 12000, 800))
        self.assertLess(aggregate_radius(1000, 600, 600), radius)
        self.assertEqual(aggregate_radius(1000, 600, 600),
                         aggregate_radius(1000, 60, 60))
        self.assertEqual(aggregate_radius(1000, 1200, 1200), 12)
        self.assertEqual(aggregate_radius(1000, 1200, 1200),
                         aggregate_radius(1000, 12000, 12000))
        self.assertEqual(aggregate_radius(10 ** 50, 12000, 12000), 18)
        for count in (2, 10, 50, 100, 1000, 10000, 10 ** 12):
            for width, height in ((1, 1), (800, 800), (10000, 10000)):
                with self.subTest(count=count, viewport=(width, height)):
                    value = aggregate_radius(count, width, height)
                    self.assertIsInstance(value, int)
                    self.assertGreater(value, 0)
                    self.assertLessEqual(value, 18)

    def test_initial_grouping_pitch_scales_with_canvas_without_dense_16px_grid(self):
        self.assertEqual(marker_cell_px(1200, 800), 50)
        self.assertEqual(marker_cell_px(800, 1200), 50)
        self.assertEqual(marker_cell_px(160, 100), 32)
        self.assertEqual(marker_cell_px(7680, 4320), 64)

    def test_circle_is_filled_translucent_with_antialiased_outline(self):
        radius = 12
        side = 2 * radius + 1
        pixels = circle_rgba(radius, gui.DRC_RED)
        self.assertEqual(len(pixels), side * side * 4)

        def pixel(x, y):
            start = (y * side + x) * 4
            return tuple(pixels[start:start + 4])

        center = pixel(radius, radius)
        self.assertEqual(center[:3], (255, 82, 82))
        self.assertGreater(center[3], 0, "aggregate center was hollow")
        self.assertLessEqual(center[3], 80, "dense fill obscured underlying layout")
        for x, y in ((0, 0), (0, side - 1), (side - 1, 0),
                     (side - 1, side - 1)):
            self.assertEqual(pixel(x, y)[3], 0)
        alpha = pixels[3::4]
        self.assertGreater(max(alpha), center[3], "outline is not visible")
        self.assertTrue(any(0 < a < center[3] for a in alpha),
                        "edge lacks antialias coverage")
        # Geometry and opacity must remain symmetric around the center.
        for y in range(side):
            for x in range(side):
                self.assertEqual(pixel(x, y)[3], pixel(side - 1 - x, y)[3])
                self.assertEqual(pixel(x, y)[3], pixel(x, side - 1 - y)[3])

    def test_mixed_waiver_fill_has_both_colors_without_hollow_center(self):
        radius = 10
        side = 2 * radius + 1
        pixels = circle_rgba(radius, gui.DRC_RED, gui.DRC_GREEN)
        offset = radius * side * 4
        left = tuple(pixels[offset + (radius - 2) * 4:
                            offset + (radius - 2) * 4 + 4])
        right = tuple(pixels[offset + (radius + 2) * 4:
                             offset + (radius + 2) * 4 + 4])
        self.assertEqual(left[:3], (255, 82, 82))
        self.assertEqual(right[:3], (0, 230, 118))
        self.assertGreater(left[3], 0)
        self.assertEqual(left[3], right[3])
        self.assertLess(left[3], 255)

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
            gui.stamp_drc_circle(buf, 0, 0, 8, gui.DRC_RED, sprites=sprites)
            self.assertEqual(make.call_count, 1)
            self.assertEqual(make.call_args.args[1:], (0, True, 8, 17, 17, 68))
            self.assertEqual(len(make.call_args.args[0]), 17 * 17 * 4)
            sprite.composite.assert_called_once_with(
                buf, 0, 0, 9, 9, -8, -8, 1, 1, 0, 255)
            gui.stamp_drc_circle(buf, 99, 79, 8, gui.DRC_RED, sprites=sprites)
            self.assertEqual(make.call_count, 1, "same style rebuilt its sprite")
            self.assertEqual(sprite.composite.call_args.args,
                             (buf, 91, 71, 9, 9, 91, 71, 1, 1, 0, 255))
            gui.stamp_drc_circle(buf, 200, 200, 8, gui.DRC_RED, sprites=sprites)
            self.assertEqual(sprite.composite.call_count, 2)
            gui.stamp_drc_circle(buf, 50, 50, 10, gui.DRC_RED, sprites=sprites)
            gui.stamp_drc_circle(buf, 50, 50, 8, gui.DRC_GREEN, sprites=sprites)
            gui.stamp_drc_circle(buf, 50, 50, 8, gui.DRC_RED,
                                gui.DRC_GREEN, sprites=sprites)
            self.assertEqual(make.call_count, 4,
                             "different size/review colors shared a sprite")
            self.assertEqual(len(sprites), 4)
            gui.stamp_drc_circle(buf, 50, 50, 8, gui.DRC_RED,
                                 sprites=sprites, solid=True)
            self.assertEqual(make.call_count, 5,
                             "solid singleton reused translucent sprite")
            center = (8 * 17 + 8) * 4
            self.assertEqual(make.call_args.args[0][center + 3], 255)

    def test_group_outline_preserves_empty_interior_and_mixed_review_colors(self):
        pixels = {}

        def fill(_buf, x, y, width, height, color):
            for row in range(int(y), int(y + height)):
                for column in range(int(x), int(x + width)):
                    pixels[column, row] = color

        with patch.object(gui, "fill_rect", side_effect=fill):
            gui.stamp_drc_group_box(Mock(), (10, 20, 30, 40),
                                    gui.DRC_RED, gui.DRC_GREEN)
        self.assertNotIn((20, 30), pixels, "aggregate filled its interior")
        self.assertEqual(pixels[10, 30], gui.DRC_RED)
        self.assertEqual(pixels[30, 30], gui.DRC_GREEN)
        self.assertEqual(pixels[12, 20], gui.DRC_RED)
        self.assertEqual(pixels[28, 40], gui.DRC_GREEN)
        self.assertEqual(len(pixels), 80)

    def test_group_box_excludes_geometry_wholly_outside_canvas(self):
        for bbox in ((1.01, 0.4, 1.2, 0.6), (0.4, -0.2, 0.6, -0.01),
                     (-0.2, 0.4, -0.01, 0.6), (0.4, 1.01, 0.6, 1.2)):
            with self.subTest(bbox=bbox):
                self.assertIsNone(gui.drc_group_screen_box(
                    bbox, 0.001, (0, 0, 1000, 1000), 10, 100, 100))

    def test_group_box_on_closed_viewport_boundary_keeps_valid_visible_bounds(self):
        for bbox, axis in (((1, 0.4, 1.2, 0.6), 0),
                           ((0.4, -0.2, 0.6, 0), 1)):
            with self.subTest(bbox=bbox):
                box = gui.drc_group_screen_box(
                    bbox, 0.001, (0, 0, 1000, 1000), 10, 100, 100)
                self.assertIsNotNone(box)
                self.assertEqual(box[axis], 99)
                self.assertEqual(box[axis + 2], 99)
                self.assertLessEqual(box[0], box[2])
                self.assertLessEqual(box[1], box[3])


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
        self.assertTrue(query["declutter"], "viewer omitted the worker coverage budget")
        self.assertEqual(query["cell_px"], marker_cell_px(100, 100))

    @staticmethod
    def progress_points(processed, total=100000000, occupied=2):
        points = UngroupedViewerTests.points()
        return PointMarkers(points.width, points.height, points.rgba,
                            points.error_ids, points.check_ids,
                            min(processed, 1234567), occupied,
                            processed_count=processed, total_count=total)

    def test_point_progress_fields_preserve_seven_argument_constructor(self):
        points = UngroupedViewerTests.points()
        self.assertEqual((points.processed_count, points.total_count), (0, 0))

    def test_partial_frames_repaint_same_key_until_final_and_keep_busy(self):
        viewer = self.viewer
        viewer._drc_ungrouped = True
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

    def test_grouped_partial_list_is_drawable_without_finishing_query(self):
        self.request()
        key = self.viewer._drc_marker_key
        markers = [Marker(0, 15, 0.4, 0.7, 500, 0,
                          (0.3, 0.6, 0.5, 0.8), True)]
        self.viewer._drc_marker_overlay = object()
        self.viewer._drc_marker_worker.poll.return_value = (
            key, worker_mod.MarkerProgress(markers), None)
        self.viewer._drc_marker_poll()
        self.assertTrue(self.viewer._drc_marker_busy)
        self.assertIsNone(self.viewer._drc_marker_overlay)
        self.assertIs(self.request(), markers)
        self.viewer._display.assert_called_once()

    def test_partial_info_shows_processed_population_and_current_marker_count(self):
        viewer = self.viewer
        viewer._drcwin = SimpleNamespace(_info=Mock())
        viewer._drc.path, viewer._drc.cell, viewer._drc.total = "progress.db", "MAIN", 100000000
        viewer._drc_rmeta = viewer._drc_clusters = None
        viewer._drc_shown = 1
        viewer._drc_ungrouped = True
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
        viewer._drc_group_hits = [(10, 10, 20, 20, object())]
        viewer._drc_marker_worker.poll.return_value = (key, None, "center scan failed")
        viewer._drc_marker_poll()
        self.assertFalse(viewer._drc_marker_busy)
        if viewer._drc_marker_result is not None:
            self.assertEqual(viewer._drc_marker_result, (key, []))
        self.assertIsNone(viewer._drc_marker_overlay)
        self.assertIsNone(viewer._drc_point_hits)
        self.assertEqual(viewer._drc_hits, [])
        self.assertEqual(viewer._drc_group_hits, [])
        self.assertIn("center scan failed", viewer._set_live_status.call_args.args[0])
        self.assertEqual(viewer._display.call_count, 2)

    def test_rule_view_and_u_changes_ignore_late_partial_frames(self):
        for change in ("rule", "view", "U"):
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
                    viewer._drc_toggle_grouping()
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
        viewer._drc_group_hits = [(6, 6, 8, "old group")]
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
        self.assertTrue(query["declutter"])
        self.assertEqual(query["cell_px"], marker_cell_px(150, 80))

    @staticmethod
    def canvas(width=100, height=100):
        disp, layer = Mock(), Mock()
        disp.get_width.return_value = width
        disp.get_height.return_value = height
        pixbuf = SimpleNamespace(Pixbuf=SimpleNamespace(new=Mock(return_value=layer)),
                                 Colorspace=SimpleNamespace(RGB=0),
                                 InterpType=SimpleNamespace(NEAREST=0))
        return disp, layer, pixbuf

    def test_painter_caches_layer_and_separates_exact_from_group_hits(self):
        viewer = self.viewer
        single = Marker(0, 1507, 0.2, 0.3, 1, 0, (0.2, 0.3, 0.2, 0.3), False)
        group = Marker(0, 9, 0.8, 0.5, 200000, 75000,
                       (0.75, 0.4, 0.9, 0.6), True)
        viewer._drc_marker_request = Mock(return_value=[single, group])
        viewer._drc_marker_key = ("render",)
        disp, layer, pixbuf = self.canvas()
        with patch.object(gui, "GdkPixbuf", pixbuf), \
                patch.object(gui, "stamp_drc_group_box") as box, \
                patch.object(gui, "stamp_drc_circle") as circle:
            viewer._drc_stamp_markers(disp, self.bounds, 10)
            circle.assert_called_once()
            self.assertEqual(circle.call_args.args,
                             (layer, 20, 70, 2, gui.DRC_RED))
            self.assertTrue(circle.call_args.kwargs["solid"])
            box.assert_called_once_with(layer, (75, 40, 90, 60),
                                        gui.DRC_RED, gui.DRC_GREEN)
            self.assertEqual(viewer._drc_hit_at(20, 70), (0, 1507))
            self.assertIsNone(viewer._drc_hit_at(80, 50))
            self.assertIs(viewer._drc_group_at(80, 50), group)
            self.assertIsNone(viewer._drc_group_at(20, 70))
            viewer._drc_hits.clear()
            viewer._drc_group_hits.clear()
            viewer._drc_stamp_markers(disp, self.bounds, 10)
            self.assertEqual(circle.call_count, 1,
                             "unchanged viewport repainted singleton circles")
            self.assertEqual(box.call_count, 1,
                             "unchanged viewport repainted aggregate boxes")
            self.assertEqual(len(viewer._drc_hits), 1)
            self.assertEqual(viewer._drc_group_hits,
                             [(75, 40, 90, 60, group)])
            self.assertEqual(layer.composite.call_count, 2)
            # Picking adds a highlight to the display, not to the reusable
            # cached layer, so changing selection never rerenders all groups.
            viewer._drc_group_pick(viewer._drc_group_at(80, 50, cycle=True))
            box.reset_mock()
            viewer._drc_hits.clear()
            viewer._drc_group_hits.clear()
            viewer._drc_stamp_markers(disp, self.bounds, 10)
            self.assertEqual(circle.call_count, 1)
            self.assertTrue(box.called)
            self.assertTrue(all(call.args[0] is disp for call in box.call_args_list))
            self.assertEqual(layer.composite.call_count, 3)
            pixbuf.Pixbuf.new.assert_called_once()
        self.assertIn("200000 errors (75000 waived)", viewer._drc_group_text(group))

    def test_group_boxes_follow_geometry_not_count_and_round_outward(self):
        viewer = self.viewer
        groups = [
            Marker(0, 0, 0.5, 0.5, 10, 0, (0.101, 0.201, 0.899, 0.799), True),
            Marker(0, 1, 0.5, 0.5, 100000000, 0,
                   (0.101, 0.201, 0.899, 0.799), True),
            Marker(0, 2, 0.5, 0.5, 20, 0, (-1.0, 0.45, 2.0, 0.55), True),
            Marker(0, 3, 0.5, 0.5, 30, 0, (0.5, 0.5, 0.5, 0.5), True),
        ]
        viewer._drc_marker_request = Mock(return_value=groups)
        viewer._drc_marker_key = ("geometry",)
        disp, _layer, pixbuf = self.canvas()
        with patch.object(gui, "GdkPixbuf", pixbuf), \
                patch.object(gui, "stamp_drc_group_box"), \
                patch.object(gui, "stamp_drc_circle") as circle:
            viewer._drc_stamp_markers(disp, self.bounds, 10)
        boxes = {hit[-1].ei: hit[:4] for hit in viewer._drc_group_hits}
        self.assertEqual(boxes[0], (10, 20, 90, 80))
        self.assertEqual(boxes[1], boxes[0], "count resized a geometry bbox")
        self.assertEqual(boxes[2][0], 0)
        self.assertEqual(boxes[2][2], 99)
        self.assertEqual(boxes[2][1:4:2], (45, 55))
        left, top, right, bottom = boxes[3]
        self.assertLessEqual(left, 50)
        self.assertLessEqual(top, 50)
        self.assertGreaterEqual(right, 50)
        self.assertGreaterEqual(bottom, 50)
        self.assertGreaterEqual(right - left, 4)
        self.assertGreaterEqual(bottom - top, 4)
        self.assertEqual(len(boxes), len(groups), "renderer changed group count")
        circle.assert_not_called()

    def test_group_hit_uses_rectangle_and_cycles_overlaps_without_hover_advance(self):
        viewer = self.viewer
        large, small = object(), object()
        viewer._drc_marker_key = ("cycle",)
        viewer._drc_group_hits = [(10, 10, 50, 50, large),
                                 (24, 24, 36, 36, small)]
        self.assertIs(viewer._drc_group_at(49, 49), large,
                      "bbox corner should be selectable")
        self.assertIsNone(viewer._drc_group_at(53.01, 30))
        self.assertIs(viewer._drc_group_at(30, 30), small)
        self.assertIs(viewer._drc_group_at(30, 30, cycle=True), small)
        self.assertIs(viewer._drc_group_at(30, 30, cycle=True), large)
        for _ in range(3):
            self.assertIs(viewer._drc_group_at(30, 30), large,
                          "hover/double-click changed chosen overlap")
        self.assertIs(viewer._drc_group_at(31, 31, cycle=True), small,
                      "nearby click did not wrap the overlap cycle")
        # Movement to a different candidate set, then back, starts at top.
        self.assertIs(viewer._drc_group_at(48, 48, cycle=True), large)
        self.assertIs(viewer._drc_group_at(30, 30, cycle=True), small)
        self.assertIs(viewer._drc_group_at(30, 30, cycle=True), large)
        viewer._drc_marker_key = ("new viewport",)
        self.assertIs(viewer._drc_group_at(30, 30, cycle=True), small)
        # Same location and viewport but different candidates also resets.
        third = object()
        viewer._drc_group_hits.append((25, 25, 35, 35, third))
        self.assertIs(viewer._drc_group_at(30, 30, cycle=True), third)

    def test_dense_groups_paint_first_and_singleton_picks_remain_visible(self):
        viewer = self.viewer
        single = Marker(0, 9071, 0.5, 0.5, 1, 0, (0.5, 0.5, 0.5, 0.5), False)
        small = Marker(0, 13, 0.5, 0.5, 10, 10, (0.4, 0.4, 0.6, 0.6), True)
        large = Marker(0, 17, 0.5, 0.5, 10000, 0, (0.1, 0.1, 0.9, 0.9), True)
        viewer._drc_marker_request = Mock(return_value=[small, single, large])
        viewer._drc_marker_key = ("overlap",)
        disp, layer, pixbuf = self.canvas()
        paints = Mock()
        with patch.object(gui, "GdkPixbuf", pixbuf), \
                patch.object(gui, "stamp_drc_circle", paints.circle), \
                patch.object(gui, "stamp_drc_group_box", paints.box):
            viewer._drc_stamp_markers(disp, self.bounds, 10)
        self.assertEqual([entry[0] for entry in paints.mock_calls],
                         ["box", "box", "circle"])
        self.assertEqual(paints.box.call_args_list[0].args[1], (10, 10, 90, 90))
        self.assertEqual(paints.box.call_args_list[1].args[1], (40, 40, 60, 60))
        self.assertEqual(paints.box.call_args_list[1].args[2:],
                         (gui.DRC_GREEN, None))
        self.assertTrue(paints.circle.call_args.kwargs["solid"])
        self.assertEqual([hit[-1] for hit in viewer._drc_group_hits], [large, small])
        self.assertIs(viewer._drc_group_at(50, 50), small)
        self.assertEqual(viewer._drc_hit_at(50, 50), (0, 9071))

    def test_plain_click_cycles_and_double_click_zooms_current_overlap(self):
        viewer = self.viewer
        a = Marker(0, 1, 0.5, 0.5, 20, 0, (0.1, 0.1, 0.9, 0.9), True)
        b = Marker(0, 2, 0.5, 0.5, 10, 0, (0.2, 0.2, 0.8, 0.8), True)
        viewer._drc_group_hits = [(10, 10, 90, 90, a), (20, 20, 80, 80, b)]
        viewer._drc_marker_key = ("click",)
        viewer._drcwin = SimpleNamespace(_detail=Mock())
        viewer._update_cursor = Mock()
        viewer._cursor = (50, 50)
        viewer._drc_group_zoom = Mock()
        viewer._focus_view = Mock()
        viewer._set_cursor = Mock()
        viewer._idle_cursor = Mock()
        viewer.cache = object()
        gdk = SimpleNamespace(ModifierType=SimpleNamespace(CONTROL_MASK=4,
                                                         SHIFT_MASK=1),
                              EventType=SimpleNamespace(DOUBLE_BUTTON_PRESS=5))
        event = SimpleNamespace(x=50, y=50, state=0, button=1, type=5)
        with patch.object(gui, "Gdk", gdk):
            viewer._pick_click(event)
            self.assertIs(viewer._drc_group_at(50, 50), b)
            self.assertIn("overlap 1/2", viewer._drcwin._detail.set_text.call_args.args[0])
            viewer._pick_click(event)
            self.assertIs(viewer._drc_group_at(50, 50), a)
            self.assertIn("overlap 2/2", viewer._drcwin._detail.set_text.call_args.args[0])
            self.assertTrue(viewer._on_press(None, event))
            viewer._drc_group_zoom.assert_called_once_with(a)
            self.assertIs(viewer._drc_group_at(50, 50), a)
            viewer._pick_click(event)
            self.assertIs(viewer._drc_group_at(50, 50), b)
            viewer._drc_hits = [(50, 50, 0, 99)]
            viewer._drc.checks[0].errors = [None] * 99 + [
                SimpleNamespace(kind="p", pts=[(0.5, 0.5)])]
            viewer._drc_goto_cell = Mock()
            viewer._drc_show_detail = Mock()
            viewer._pick_click(event)
            self.assertEqual(viewer._drc_focus[:2], (0, 99))
            viewer._drc_show_detail.assert_called_once_with(0, 99)
            viewer._drc_hits.clear()
            viewer._pick_click(event)
            self.assertIs(viewer._drc_group_at(50, 50), b,
                          "single error click did not reset group cycle")

    def test_hidden_overlays_clear_both_pick_lists(self):
        viewer = self.viewer
        viewer.overlay_mode = 2
        viewer._zoomdrag = None
        viewer._drc_hits = [(5, 5, 0, 1)]
        viewer._drc_group_hits = [(6, 6, 8, "group")]
        viewer._draw_overlays(Mock(), self.bounds, 10)
        self.assertEqual(viewer._drc_hits, [])
        self.assertEqual(viewer._drc_group_hits, [])

    def test_group_cycle_restarts_after_scope_status_viewport_or_hide_reset(self):
        viewer = self.viewer
        a, b = object(), object()
        hits = [(10, 10, 90, 90, a), (20, 20, 80, 80, b)]
        self.request()
        for reset in ("scope", "status", "viewport", "hide"):
            with self.subTest(reset=reset):
                viewer._drc_group_hits = list(hits)
                self.assertIs(viewer._drc_group_at(50, 50, cycle=True), b)
                self.assertIs(viewer._drc_group_at(50, 50, cycle=True), a)
                if reset == "scope":
                    viewer._drc_cluster = object()
                    self.request()
                elif reset == "status":
                    viewer._drc_marker_invalidate()
                    self.request()
                elif reset == "viewport":
                    self.bounds = (1000, 0, 2000, 1000)
                    self.request()
                else:
                    viewer.overlay_mode = 2
                    viewer._zoomdrag = None
                    viewer._draw_overlays(Mock(), self.bounds, 10)
                viewer._drc_group_hits = list(hits)
                self.assertIs(viewer._drc_group_at(50, 50, cycle=True), b)
                # No saved second-position cycle leaks into the next case.
                viewer._drc_marker_invalidate()
                self.request()

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

    def test_outside_click_and_escape_clear_group_selection_and_cycle(self):
        viewer = self.viewer
        a = Marker(0, 1, 0.5, 0.5, 20, 0, (0.1, 0.1, 0.9, 0.9), True)
        b = Marker(0, 2, 0.5, 0.5, 10, 0, (0.2, 0.2, 0.8, 0.8), True)
        viewer._drc_group_hits = [(10, 10, 90, 90, a), (20, 20, 80, 80, b)]
        viewer._drc_marker_key = ("reset",)
        viewer._pending = None
        viewer._ruler_start = None
        viewer.selection = None
        viewer._cell_hl = None
        viewer._drc_lyr_saved = None
        viewer._update_cursor = Mock()
        viewer._cursor = (1000, 1000)
        viewer._pick_px = None
        viewer.tiles_spanned = Mock(return_value=5)
        gdk = SimpleNamespace(ModifierType=SimpleNamespace(CONTROL_MASK=4,
                                                         SHIFT_MASK=1))
        for reset in ("outside click", "escape"):
            with self.subTest(reset=reset):
                viewer._drc_group_at(50, 50, cycle=True)
                selected = viewer._drc_group_at(50, 50, cycle=True)
                self.assertIs(selected, a)
                viewer._drc_group_pick(selected)
                if reset == "escape":
                    viewer._esc()
                else:
                    with patch.object(gui, "Gdk", gdk):
                        viewer._pick_click(SimpleNamespace(x=1000, y=1000, state=0))
                self.assertIs(viewer._drc_group_at(50, 50), b)
                self.assertIs(viewer._drc_group_at(50, 50, cycle=True), b)
                viewer._drc_marker_invalidate()
                viewer._drc_marker_key = ("reset",)
                viewer._drc_group_hits = [(10, 10, 90, 90, a),
                                         (20, 20, 80, 80, b)]

    def test_grid_step_and_jump_clear_prior_group_selection_and_cycle(self):
        a = Marker(0, 1, 0.5, 0.5, 20, 0, (0.1, 0.1, 0.9, 0.9), True)
        b = Marker(0, 2, 0.5, 0.5, 10, 0, (0.2, 0.2, 0.8, 0.8), True)
        for action in ("grid", "step", "jump"):
            with self.subTest(action=action):
                viewer = viewer_fixture()
                error = drc.DrcError("p", 1, [(0.4, 0.4), (0.6, 0.6)])
                viewer._drc = SimpleNamespace(
                    checks=[SimpleNamespace(name="R", errors=[error])])
                viewer._drc_marker_key = ("navigation",)
                viewer._drc_group_hits = [(10, 10, 90, 90, a),
                                         (20, 20, 80, 80, b)]
                viewer._drc_group_at(50, 50, cycle=True)
                viewer._drc_group_pick(viewer._drc_group_at(50, 50, cycle=True))
                self.assertIs(viewer._drc_group_at(50, 50), a)
                viewer._drc_grid_ci = 0
                viewer._drc_grid_base = None
                viewer._drc_cum = [0]
                viewer._drc_jump_spp = None
                viewer._drc_show_detail = Mock()
                viewer._viewport_size = Mock(return_value=(100, 100))
                viewer._drc_cd_ruler = Mock(return_value=[])
                viewer.goto = Mock()
                if action == "grid":
                    viewer._drc_grid_rows = viewer._drc_gridw = 1
                    viewer._drc_grid_map = [0]
                    viewer._drc_cell_mark = Mock()
                    column = object()
                    tree = Mock()
                    tree.get_path_at_pos.return_value = (
                        SimpleNamespace(get_indices=lambda: [0]), column, 0, 0)
                    tree.get_columns.return_value = [column]
                    gdk = SimpleNamespace(
                        ModifierType=SimpleNamespace(CONTROL_MASK=4, SHIFT_MASK=1),
                        EventType=SimpleNamespace(BUTTON_PRESS=1, DOUBLE_BUTTON_PRESS=5))
                    with patch.object(gui, "Gdk", gdk):
                        viewer._on_drc_grid_click(
                            tree, SimpleNamespace(x=0, y=0, button=1, state=0, type=1))
                    viewer._drc_cell_mark.assert_called_once_with(0, 0)
                elif action == "step":
                    viewer._drc_step(1)
                else:
                    viewer._drc_jump(0, 0)
                    self.assertIsNotNone(viewer.drc_mark)
                    viewer.goto.assert_called_once()
                viewer._drc_show_detail.assert_called_once_with(0, 0)
                if action != "jump":
                    self.assertEqual(viewer._drc_focus[:2], (0, 0))
                self.assertIs(viewer._drc_group_at(50, 50), b,
                              "individual navigation retained prior group selection")
                self.assertIs(viewer._drc_group_at(50, 50, cycle=True), b,
                              "individual navigation retained prior overlap cycle")


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
