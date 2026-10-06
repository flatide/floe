"""Headless integration gate for the SVRF delta grouping controls.

Runs real Viewer methods with real numeric group memberships and simple
widget adapters; no GTK window or graphics context is created.

Usage: python tools/validate_drc_delta_gui.py
"""

import base64
import json
import math
import os
import sys
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import Mock, patch

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from floe import cli, drc, drc_delta as delta, fe_embed, gui, service  # noqa: E402
from validate_drc_delta import Database, area_cases, constraint, edge, rect  # noqa: E402
from validate_drc_marker_gui import viewer_fixture  # noqa: E402
from validate_drc_clusters import StatusDb, TreeStore, TreeView  # noqa: E402


class GroupStore(list):
    def append(self, row):
        index = len(self)
        super().append(row)
        return index

    def get_value(self, index, column):
        return self[index][column]


def step_entry(text):
    entry = Mock()
    entry.get_text.return_value = text
    entry.set_text.side_effect = lambda value: setattr(entry.get_text, "return_value", value)
    return entry


def fixture(errors=None, constraints=None):
    viewer = viewer_fixture()
    viewer._drc = Database(errors if errors is not None else [rect(0.8), rect(1.2)])
    constraints = constraints or [constraint(bound=3)]
    viewer._drc_rmeta = {"checks": {"R": {"constraints": constraints}}}
    viewer._drc_delta_mode = "absolute"
    viewer._drc_delta_step = 10000
    viewer._drc_delta_steps = {(0, "absolute"): 10000, (0, "percent"): 1000000}
    viewer._drc_delta_auto_steps = {}
    viewer._drc_delta_step_scope = None
    viewer._drc_delta_step_text = "0.10000"
    viewer._drc_delta_revision = 0
    viewer._drc_delta_key = None
    viewer._drc_delta_group = None
    viewer._drc_delta_groups = delta.DeltaIndex(viewer._drc, 0, constraints).measure().group(10000)
    viewer._drc_delta_page = 0
    viewer._drc_delta_busy = False
    viewer._drc_delta_error = None
    viewer._drc_delta_worker = Mock()
    viewer._drc_delta_worker.poll.return_value = None
    viewer._drc_marker_worker = Mock()
    viewer._drc_marker_key = None
    viewer._drc_marker_result = None
    viewer._drc_marker_overlay = None
    viewer._drc_marker_revision = 0
    viewer._drc_show_rule = Mock()
    viewer._drc_pos = -1
    viewer.drc_mark = None
    viewer._drc_ruler = []
    viewer.rulers = []
    viewer._drc_clusters = None
    viewer._drc_gridw = 1
    viewer._drc_cell = None
    grid = Mock()
    grid.create_pango_layout.return_value.get_pixel_size.return_value = (20, 10)
    grid.get_allocation.return_value.width = 30
    viewer._drcwin = SimpleNamespace(_gstore=[], _grid=grid,
                                    _plabel=Mock(), _pprev=Mock(), _pnext=Mock(),
                                    _delta_step=step_entry("0.10000"))
    return viewer


def picks(indices):
    return (0, list(indices), [(ei, "p", [(0, 0)]) for ei in indices],
            frozenset(indices))


def use_auto(viewer):
    viewer._drc_delta_steps.clear()
    viewer._drc_delta_auto_steps.clear()
    viewer._drc_delta_step = None
    viewer._drc_delta_step_scope = None
    viewer._drc_delta_step_text = "auto"
    viewer._drc_delta_key = None
    viewer._drc_delta_group = viewer._drc_delta_groups = None
    viewer._drcwin._delta_step.set_text("auto")
    viewer._drc_delta_worker.reset_mock()
    viewer._drc_delta_worker.poll.return_value = None
    return viewer


def reply_groups(viewer, step, key=None):
    ci = viewer._drc_open
    cons = viewer._drc_rmeta["checks"][viewer._drc.checks[ci].name]["constraints"]
    groups = delta.DeltaIndex(viewer._drc, ci, cons).measure().group(step,
        mode=viewer._drc_delta_mode)
    groups.auto_step = (ci, viewer._drc_delta_mode) not in viewer._drc_delta_steps
    viewer._drc_delta_worker.poll.return_value = (
        viewer._drc_delta_key if key is None else key, groups, None)
    viewer._drc_delta_poll()
    viewer._drc_delta_worker.poll.return_value = None
    return groups


def rule_tree_fixture():
    viewer = fixture()
    viewer._drc = StatusDb([("R", 3), ("S", 2)])
    viewer._drc.checks[0].errors = [rect(0.8), rect(1.2), rect(0.8)]
    viewer._drc.checks[1].errors = [rect(0.7), rect(1.1)]
    viewer._drc_rmeta = {"checks": {name: {"constraints": [constraint(bound=3)]}
                                    for name in ("R", "S")}}
    viewer._drc_open = None
    viewer._drc_delta_groups = None
    viewer._drc_search = ""
    viewer._drc_tfilter = "all"
    viewer._drc_rtypes = None
    viewer._drc_cum = [0, 3]
    viewer._drcwin._rstore = TreeStore()
    viewer._drcwin._rules = TreeView(viewer, viewer._drcwin._rstore)
    viewer._drcwin._detail = Mock()
    viewer._drc_fill()
    viewer._drc_select_row(0)
    groups = delta.DeltaIndex(viewer._drc, 0, [constraint(bound=3)]).measure().group(10000)
    viewer._drc_delta_worker.poll.return_value = (viewer._drc_delta_key, groups, None)
    viewer._drc_delta_poll()
    return viewer


class DeltaViewerTests(unittest.TestCase):
    def test_area_annotations_match_detail_and_group_values_without_length_rulers(self):
        for error, area in area_cases():
            with self.subTest(area=area):
                viewer = fixture([error], [constraint("area", 30)])
                viewer.dbu = 0.0025
                annotations = viewer._drc_cd_ruler(error, "R")
                self.assertEqual(len(annotations), 1)
                self.assertIsInstance(annotations[0], gui._DrcAreaLabel)
                self.assertEqual(annotations[0].text, "area %d.00000 µm²" % area)
                x0, y0, x1, y1 = annotations[0]
                self.assertEqual((x0, y0), (x1, y1))
                text = "\n".join(viewer._drc_meta_lines("R", error))
                self.assertIn("measured: %d.00000 um2" % area, text)
                self.assertEqual(viewer._drc_delta_groups.index.measured_ticks.tolist(),
                                 [area * delta.SCALE])
        error = rect(0.02, 0.1)
        viewer = fixture([error], [constraint("area", 1), constraint(bound=0.05)])
        annotations = viewer._drc_cd_ruler(error, "R")
        self.assertEqual(sum(isinstance(value, gui._DrcAreaLabel) for value in annotations), 1)
        self.assertEqual(sum(not isinstance(value, gui._DrcAreaLabel) for value in annotations), 2)

    def test_area_label_tab_visibility_and_overlay_paint_do_not_draw_dimension_lines(self):
        error = rect(3, 7)
        viewer = fixture([error], [constraint("area", 30)])
        viewer.rulers = viewer._drc_cd_ruler(error, "R")
        viewer._viewport_size = Mock(return_value=(500, 500))
        label = Mock()
        label.get_preferred_size.return_value = (None, SimpleNamespace(width=150, height=18))
        label.get_margin_start.return_value = 0
        label.get_margin_top.return_value = 0
        viewer._labels = [label]
        glib = SimpleNamespace(markup_escape_text=lambda value: value)
        for mode in (0, 1, 2, 0):
            with self.subTest(mode=mode):
                label.reset_mock()
                self.assertEqual(viewer.overlay_mode, mode)
                with patch.object(gui, "GLib", glib):
                    viewer._update_labels((0, 0, 10000, 10000), 20)
                if mode == 2:
                    label.hide.assert_called_once()
                    label.show.assert_not_called()
                else:
                    label.show.assert_called_once()
                    self.assertIn("area 21.00000 µm²", label.set_markup.call_args.args[0])
                viewer._toggle_overlays()
        viewer.overlay_mode = 1
        viewer.selections = []
        viewer._zoomdrag = viewer._band_cur = None
        disp = SimpleNamespace(get_width=lambda: 500, get_height=lambda: 500)
        with patch.object(gui, "stamp_segment") as segment, \
                patch.object(gui, "stamp_arrow") as arrow:
            viewer._draw_overlays(disp, (0, 0, 10000, 10000), 20)
            segment.assert_not_called()
            arrow.assert_not_called()
            viewer.rulers.append((0, 0, 1000, 0))
            viewer._draw_overlays(disp, (0, 0, 10000, 10000), 20)
            segment.assert_called_once()
            self.assertEqual(arrow.call_count, 2)

    def test_area_annotations_follow_error_jump_and_escape_ruler_lifecycle(self):
        errors = [error for error, _area in area_cases()[:2]]
        viewer = fixture(errors, [constraint("area", 30)])
        viewer._drc_hl = False
        viewer._drc_cum = [0]
        viewer._drc_jump_spp = None
        viewer._drc_zoom_lock = False
        viewer._pending = None
        viewer._ruler_start = None
        viewer._auto_rulers = []
        viewer.goto = Mock()
        viewer._viewport_size = Mock(return_value=(500, 500))
        viewer._drc.checks[0].desc = ""
        viewer._drcwin._detail = Mock()
        manual = (0, 0, 1000, 0)
        viewer.rulers = [manual]
        viewer._drc_jump(0, 0)
        previous = viewer._drc_ruler[0]
        self.assertIsInstance(previous, gui._DrcAreaLabel)
        self.assertEqual(previous.text, "area 21.00000 µm²")
        viewer._drc_jump(0, 1)
        self.assertEqual(viewer._drc_ruler[0].text, "area 6.00000 µm²")
        self.assertNotIn(previous, viewer.rulers)
        self.assertIn(manual, viewer.rulers, "a new error must preserve manually drawn rulers")
        self.assertEqual(len(viewer.rulers), 2)
        viewer._esc()
        self.assertEqual(viewer._drc_ruler, [])
        self.assertEqual(viewer.rulers, [])

    def test_cli_png_roundtrip_area_labels_replace_lengths_and_keep_mixed_annotations(self):
        png = base64.b64decode(
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a7X8AAAAASUVORK5CYII=")
        cases = [(error, [constraint("area", 30)], "area %d.00000 µm²" % area, 0)
                 for error, area in area_cases()]
        cases.append((rect(3, 7), [constraint("area", 30), constraint(bound=5)],
                      "area 21.00000 µm²", 2))
        with tempfile.TemporaryDirectory(prefix="floe-area-snapshot-") as tmp:
            path = os.path.join(tmp, "snapshot.png")
            for error, constraints, expected, ruler_count in cases:
                with self.subTest(expected=expected, ruler_count=ruler_count):
                    with open(path, "wb") as stream:
                        stream.write(png)
                    cli._embed_error_png(path, error, (-1, -1, 9, 9), 1000,
                                         False, "R", 1, constraints=constraints)
                    annotations, _ppu, unit, _note, _legend = fe_embed.read(path)
                    texts = [value["text"] for value in annotations if value["kind"] == "text"]
                    self.assertEqual(texts, [expected])
                    self.assertEqual(sum(value["kind"] == "ruler" for value in annotations),
                                     ruler_count)
                    self.assertEqual(unit, "um", "area annotation must not alter coordinate units")

    def test_cd_ruler_uses_explicit_rule_metadata_and_preserves_geometry_fallback(self):
        error = rect(0.02, 0.1)
        viewer = fixture([error], [constraint("notch", 0.05)])
        viewer.dbu = 0.001
        viewer._drc.checks.append(SimpleNamespace(name="S", errors=[error]))
        viewer._drc_rmeta["checks"]["S"] = {"constraints": [constraint("extension", 0.05, ">") ]}
        viewer._drc_open = 1
        axes = [tuple(value / viewer.dbu for value in segment)
                for segment in drc.cd_segments(error)]
        self.assertEqual(viewer._drc_cd_ruler(error, "R"), [axes[0]])
        self.assertEqual(viewer._drc_cd_ruler(error, "S"), [axes[1]])
        self.assertEqual(viewer._drc_cd_ruler(error), axes)
        self.assertEqual(viewer._drc_cd_ruler(error, "MISSING"), axes)

    def test_error_jump_passes_its_rule_to_cd_ruler_filter(self):
        error = rect(0.02, 0.1)
        viewer = fixture([error], [constraint("overlap", 0.05)])
        viewer.dbu = 0.001
        viewer._drc_hl = False
        viewer._drc_cum = [0]
        viewer._drc_jump_spp = None
        viewer._drc_zoom_lock = False
        viewer.spp = 1
        viewer.goto = Mock()
        viewer._viewport_size = Mock(return_value=(500, 500))
        viewer._drc_cd_ruler = Mock(return_value=[(0, 50, 20, 50)])
        viewer._drc.checks[0].desc = ""
        viewer._drcwin._detail = Mock()
        viewer._drc_jump(0, 0)
        viewer._drc_cd_ruler.assert_called_once_with(error, "R")
        self.assertEqual(viewer.rulers, [(0, 50, 20, 50)])

    def test_canvas_and_live_ruler_labels_keep_five_decimal_places(self):
        viewer = fixture()
        viewer.dbu = 1
        viewer.overlay_mode = 0
        viewer.mode = "pan"
        viewer._viewport_size = Mock(return_value=(500, 500))
        label = Mock()
        label.get_preferred_size.return_value = (None, SimpleNamespace(width=100, height=18))
        label.get_margin_start.return_value = 0
        label.get_margin_top.return_value = 0
        viewer._labels = [label]
        glib = SimpleNamespace(markup_escape_text=lambda value: value)
        for length, rendered in ((0.00001, "0.00001"), (0.123455, "0.12346"), (2, "2.00000")):
            with self.subTest(length=length):
                viewer.rulers = [(0, 0.5, length, 0.5)]
                with patch.object(gui, "GLib", glib):
                    viewer._update_labels((0, 0, 5, 5), 0.01)
                self.assertIn(rendered + " um", label.set_markup.call_args.args[0])
        viewer.mode = "ruler"
        viewer._cursor = (0.123455, 0)
        viewer._ruler_start = (0, 0)
        viewer._ruler_end_preview = Mock(return_value=viewer._cursor)
        viewer._request_snap = Mock()
        gdk = SimpleNamespace(ModifierType=SimpleNamespace(SHIFT_MASK=1))
        with patch.object(gui, "Gdk", gdk):
            viewer._hover(SimpleNamespace(state=0))
        self.assertIn("measure 0.12346 um (dx 0.12346, dy 0.00000)",
                      viewer._set_live_status.call_args.args[0])

    def test_cli_snapshot_rulers_match_gui_selected_directions_and_numeric_lengths(self):
        error = rect(0.02, 0.1)
        diagonal = drc.DrcError("e", 1, [(0, 0), (2, 0), (5, 4), (7, 4)])
        notch = constraint("notch", 0.05)
        notch["text"] = "EXTERNAL M1 < 0.05 NOTCH"
        region = constraint("space", 0.05)
        region["text"] = "EXTERNAL M1 < 0.05 REGION"
        cases = [(error, [notch], [0.02]),
                 (error, [region], [0.02, 0.1]),
                 (error, [constraint("overlap", 0.05, ">")], [0.1]),
                 (error, [constraint("extension", 0.15)], [0.02, 0.1]),
                 (error, [], [0.02, 0.1]),
                 (diagonal, [constraint("enclosure", 6)], [3, 4, 5])]
        for geometry, constraints, expected in cases:
            with self.subTest(constraints=constraints):
                viewer = fixture([geometry], constraints or [constraint()])
                viewer.dbu = 0.001
                if not constraints:
                    viewer._drc_rmeta = None
                gui_rulers = viewer._drc_cd_ruler(geometry, "R")
                with patch.object(fe_embed, "embed") as embed:
                    cli._embed_error_png("unused.png", geometry, (-1, -1, 9, 9),
                                         1000, False, "R", 1, constraints=constraints)
                annotations = [value for value in embed.call_args.args[1]
                               if value["kind"] == "ruler"]
                ppu = embed.call_args.kwargs["ppu"]
                cli_lengths = sorted(math.hypot(value["b"][0] - value["a"][0],
                                               value["b"][1] - value["a"][1]) / ppu
                                     for value in annotations)
                gui_lengths = sorted(math.hypot(x1 - x0, y1 - y0) * viewer.dbu
                                     for x0, y0, x1, y1 in gui_rulers)
                self.assertEqual(len(cli_lengths), len(expected))
                for got, wanted in zip(cli_lengths, expected):
                    self.assertAlmostEqual(got, wanted)
                for got, wanted in zip(cli_lengths, gui_lengths):
                    self.assertAlmostEqual(got, wanted)

    def test_cli_explicit_layers_still_load_rule_metadata_for_embedded_rulers(self):
        error = rect(0.02, 0.1)
        db = Database([error])
        db.checks[0].desc = ""
        constraints = [constraint("notch", 0.05)]
        png = base64.b64decode(
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a7X8AAAAASUVORK5CYII=")
        worker = Mock()
        worker.res.get.return_value = {"kind": "frame", "gen": 1, "png": png}
        cache = SimpleNamespace(meta={"dbu": 0.001},
                                resolve_layers=Mock(return_value=[(99, 0)]))
        with tempfile.TemporaryDirectory(prefix="floe-cd-snapshot-") as tmp:
            path = os.path.join(tmp, "errors.db")
            output = os.path.join(tmp, "snapshot.png")
            with open(path + ".rules.json", "w", encoding="utf-8") as stream:
                json.dump({"format": "floe-svrf-rules", "version": 1,
                           "checks": {"R": {"constraints": constraints,
                                              "source_gds": [[1, 0]]}}}, stream)
            args = SimpleNamespace(drc=path, drc_rule="R", drc_err="1", drc_cap=200,
                                   drc_rules=None, layers="99/0", depth=None,
                                   drc_frac=0.5, out=output, px="1000")
            with patch.object(drc, "load_db", return_value=db), \
                    patch.object(service, "make_render_worker", return_value=worker), \
                    patch.object(cli, "_drc_layer_legend", return_value=None), \
                    patch.object(cli, "print"):
                cli._render_drc_errors(args, cache)
            annotations, ppu, unit, _note, _legend = fe_embed.read(output)
        worker.start.assert_called_once()
        worker.stop.assert_called_once()
        cache.resolve_layers.assert_called_once_with("99/0")
        self.assertEqual(worker.submit.call_args.args[0]["visible"], [(99, 0)])
        rulers = [item for item in annotations if item["kind"] == "ruler"]
        self.assertEqual(len(rulers), 1, "explicit layers must not disable CD direction filtering")
        self.assertEqual(unit, "um")
        self.assertAlmostEqual(math.hypot(rulers[0]["b"][0] - rulers[0]["a"][0],
                                         rulers[0]["b"][1] - rulers[0]["a"][1]) / ppu, 0.02)

    def test_detail_uses_exact_five_digits_and_explicit_zero_ratio(self):
        viewer = fixture([rect(0.900005)], [constraint(bound=1.000004)])
        text = "\n".join(viewer._drc_meta_lines("R", rect(0.900005)))
        self.assertIn("estimated CD: 0.90001 um vs < 1.00000", text)
        self.assertIn("Δ 0.09999 (9.99900%)", text)
        viewer._drc_rmeta["checks"]["R"]["constraints"] = [constraint("length", 0, ">")]
        text = "\n".join(viewer._drc_meta_lines("R", edge(0.25)))
        self.assertIn("measured: 0.25000 um vs > 0.00000", text)
        self.assertIn("Δ 0.25000 (ratio unavailable)", text)

    def test_detail_and_delta_groups_use_same_directional_candidate(self):
        error = rect(0.02, 0.1)
        viewer = fixture([error], [constraint(bound=0.015, op=">")])
        text = "\n".join(viewer._drc_meta_lines("R", error))
        self.assertIn("estimated CD: 0.10000 um vs > 0.01500", text)
        self.assertIn("Δ 0.08500 (566.66667%)", text)
        self.assertIn("0.02000", text, "the alternate marker span must be disclosed")
        self.assertIn("candidate", text.lower())
        self.assertIn("max", text.lower(), "selection direction must be explained")
        groups = viewer._drc_delta_groups
        self.assertEqual(groups.index.measured_ticks.tolist(), [10000])
        self.assertEqual(groups.estimated_total, 1)
        self.assertEqual(groups[0].estimated_count, 1)
        self.assertEqual(groups[0].page(0, 10), [0])
        viewer._drc_delta_choose(groups[0])
        self.assertIs(viewer._drc_active_members(), groups[0])

    def test_detail_magnitude_matches_for_minimum_and_maximum_violations(self):
        for cd, op in ((0.8, "<"), (1.2, ">")):
            with self.subTest(cd=cd, op=op):
                viewer = fixture([edge(cd)], [constraint("length", 1, op)])
                text = "\n".join(viewer._drc_meta_lines("R", edge(cd)))
                self.assertIn("Δ 0.20000 (20.00000%)", text)
                group = viewer._drc_delta_groups[0]
                self.assertEqual(group.key, (0, 2))
                self.assertIn("[0.20000, 0.30000)", group.name)
                self.assertNotIn("+", group.name)
                self.assertNotIn("-", group.name)
        # The rule value retains its sign while the displayed difference
        # and normalized percentage are nonnegative.
        viewer = fixture([edge(0.25)], [constraint("length", -1, ">")])
        text = "\n".join(viewer._drc_meta_lines("R", edge(0.25)))
        self.assertIn("vs > -1.00000", text)
        self.assertIn("Δ 1.25000 (125.00000%)", text)

    def test_unmatched_or_conflicting_constraints_do_not_invent_detail_cd(self):
        error = rect(0.02, 0.1)
        cases = [[constraint(bound=0.01)],
                 [constraint(bound=0.15), constraint(bound=0.015, op=">")]]
        for constraints in cases:
            with self.subTest(constraints=constraints):
                viewer = fixture([error], constraints)
                text = "\n".join(viewer._drc_meta_lines("R", error))
                self.assertIn("constraint:", text)
                self.assertNotIn("measured:", text)
                self.assertNotIn("estimated CD:", text)
                self.assertEqual(viewer._drc_delta_groups[0].constraint_index, -1)
                self.assertEqual(viewer._drc_delta_groups[0].page(0, 10), [0])
                self.assertEqual(viewer._drc_delta_groups.estimated_total, 0)

    def test_group_rows_show_estimate_counts_without_changing_error_counts(self):
        viewer = fixture([rect(0.02, 0.1), rect(0.02, 0.02)],
                         [constraint(bound=0.15)])
        win = viewer._drcwin
        win._delta_store = GroupStore()
        win._delta_tree = Mock()
        win._delta_label = Mock()
        win._delta_prev, win._delta_next = Mock(), Mock()
        viewer._drc_delta_refresh()
        self.assertEqual(len(win._delta_store), 2)
        self.assertEqual(win._delta_store[1][1], "2")
        self.assertIn("estimated 1", win._delta_store[1][0].lower())
        self.assertIn("estimated", win._delta_label.set_text.call_args.args[0].lower())
        viewer._drc._status[0] = 1
        viewer._drc_delta_status_changed(0, [0])
        viewer._drc_wfilter = "waived"
        viewer._drc_delta_refresh()
        self.assertEqual(win._delta_store[1][1], "1")
        self.assertEqual(win._delta_store[1][2].estimated_count, 1)

    def test_group_rows_number_bins_independently_of_matching_condition(self):
        cases = [
            ([edge(0.7), edge(0.9)],
             [constraint("length", 0.1), constraint("length", 1)],
             ["Group #1 · condition 2 ·", "Group #2 · condition 2 ·"],
             [(1, 1), (1, 3)], [[1], [0]]),
            ([edge(0.7)],
             [constraint("length", 0.1), constraint("length", 0.2),
              constraint("length", 1)],
             ["Group #1 · condition 3 ·"], [(2, 3)], [[0]]),
            ([edge(0.7)], [constraint("length", 0.1)],
             ["Group #1 · Unmeasurable"], [(-1, 0)], [[0]]),
        ]
        for errors, constraints, prefixes, keys, memberships in cases:
            with self.subTest(prefixes=prefixes):
                viewer = fixture(errors, constraints)
                win = viewer._drcwin
                win._delta_store = GroupStore()
                win._delta_tree = Mock()
                win._delta_label = Mock()
                win._delta_prev, win._delta_next = Mock(), Mock()
                viewer._drc_delta_refresh()
                rows = win._delta_store[1:]
                self.assertEqual(len(rows), len(prefixes))
                self.assertEqual([row[2].key for row in rows], keys)
                self.assertEqual([row[2].page(0, 1000) for row in rows], memberships)
                for row, prefix in zip(rows, prefixes):
                    self.assertTrue(row[0].startswith(prefix), row[0])
                selected = rows[-1][2]
                viewer._drc_delta_choose(selected)
                self.assertIs(viewer._drc_active_members(), selected)
                self.assertEqual(viewer._drc_active_members().page(0, 1000),
                                 memberships[-1])

    def test_unresolved_alternative_explanation_is_shown_with_estimated_cd(self):
        error = drc.DrcError("e", 1, [(0, 0), (1, 0), (0, 0.05), (1, 0.05)])
        constraints = [constraint("space", 0.1),
                       {"metric": "space", "op": "<", "value": None,
                        "raw": "UNRESOLVED", "text": "EXTERNAL M2 < UNRESOLVED"}]
        viewer = fixture([error], constraints)
        pick = delta.pick_constraint(error, constraints)
        text = "\n".join(viewer._drc_meta_lines("R", error))
        self.assertIn("estimated CD: 0.05000 um vs < 0.10000", text)
        self.assertIn("Δ 0.05000 (50.00000%)", text)
        self.assertIn(pick.reason, text)
        self.assertIn("unresolved", pick.reason.lower())
        self.assertEqual(viewer._drc_delta_groups.estimated_total, 1)
        self.assertEqual(viewer._drc_delta_groups[0].estimated_count, 1)

    def test_real_rule_switch_and_status_refill_preserve_only_correct_scope(self):
        viewer = rule_tree_fixture()
        group = viewer._drc_delta_groups[1]
        viewer._drc_delta_choose(group)
        viewer._drc_set_sel(picks([0, 2]))
        group_key = viewer._drc_scope_key()
        viewer._drc_fill(preserve=True)
        self.assertIs(viewer._drc_delta_group, group)
        self.assertEqual(viewer._drc_sel[1], [0, 2])
        viewer._drc.set_status(0, 2, 1)
        viewer._drc_delta_status_changed(0, [2])
        viewer._drc_wfilter = "waived"
        viewer._drc_fill(preserve=True)
        self.assertIs(viewer._drc_delta_group, group)
        self.assertEqual(viewer._drc_sel[1], [2])
        viewer._drc_wfilter = "all"
        viewer._drc_fill(preserve=True)
        viewer._drc_select_row(1)
        self.assertIsNone(viewer._drc_delta_group)
        self.assertIsNone(viewer._drc_sel)
        self.assertEqual(viewer._drc_sels[group_key][1], [2])
        self.assertEqual(viewer._drc_delta_worker.submit.call_args.args[2], 1)

    def test_waive_import_refreshes_counts_and_preserves_active_group(self):
        viewer = rule_tree_fixture()
        group = viewer._drc_delta_groups[1]
        viewer._drc_delta_choose(group)
        viewer._drc.path = "/tmp/delta-review.tray"
        viewer._drc.waive_import = Mock(side_effect=lambda _path:
            (viewer._drc._status.__setitem__(slice(None), [1, 0, 1, 0, 0]) or 2))
        viewer.window = Mock()
        viewer._only_close_button = Mock()
        viewer._center_on_parent = Mock()
        dialog = Mock()
        dialog.run.return_value = 1
        dialog.get_filename.return_value = "/tmp/import.waive"
        gtk = SimpleNamespace(FileChooserDialog=Mock(return_value=dialog),
                              FileChooserAction=SimpleNamespace(OPEN=0),
                              ResponseType=SimpleNamespace(CANCEL=0, OK=1),
                              FileFilter=Mock())
        with patch.object(gui, "Gtk", gtk):
            viewer._drc_waive_load_dialog()
        viewer._drc.waive_import.assert_called_once_with("/tmp/import.waive")
        self.assertIs(viewer._drc_delta_group, group)
        self.assertEqual(group.status_counts(), (2, 2))

    def test_group_selection_keeps_distinct_scopes_and_resets_error_page(self):
        viewer = fixture([rect(0.8), rect(1.2), rect(0.8)])
        nearer, farther = viewer._drc_delta_groups[0], viewer._drc_delta_groups[1]
        viewer._drc_set_sel(picks([0, 1, 2]))
        all_key = viewer._drc_scope_key()
        viewer._drc_page = 9
        viewer._drc_delta_choose(farther)
        self.assertIs(viewer._drc_active_members(), farther)
        self.assertIsNone(viewer._drc_sel)
        self.assertEqual(viewer._drc_page, 0)
        farther_key = viewer._drc_scope_key()
        self.assertNotEqual(farther_key, all_key)
        viewer._drc_set_sel(picks([0, 2]))
        viewer._drc_delta_choose(nearer)
        self.assertIsNone(viewer._drc_sel)
        viewer._drc_set_sel(picks([1]))
        viewer._drc_delta_choose(farther)
        self.assertEqual(viewer._drc_sel[1], [0, 2])
        viewer._drc_delta_choose(None)
        self.assertEqual(viewer._drc_sel[1], [0, 1, 2])
        self.assertEqual(viewer._drc_sels[farther_key][1], [0, 2])

    def test_marker_query_and_grid_use_whole_selected_group(self):
        viewer = fixture([rect(0.8 if ei % 3 else 1.2) for ei in range(6003)])
        group = viewer._drc_delta_groups[1]
        viewer._drc_delta_group = group
        viewer._drc_grid_fill = gui.Viewer._drc_grid_fill.__get__(viewer)
        viewer._drc_page = 2
        viewer._drc_grid_fill(0)
        self.assertEqual(viewer._drc_grid_map, group.page(2000, 1000))
        self.assertEqual(len(viewer._drc_grid_map), 1000)
        self.assertEqual(viewer._drc_grid_base, ("cluster", group, None))
        viewer._drc_marker_request((0, 0, 1000, 1000), 10, 100, 100)
        query = viewer._drc_marker_worker.submit.call_args.args[2]
        self.assertEqual(query["members"], {0: group})
        self.assertNotIn("cap", query)

    def test_step_validation_preserves_active_membership_until_valid(self):
        viewer = fixture()
        selected = viewer._drc_delta_groups[0]
        viewer._drc_delta_group = selected
        viewer._drcwin._delta_step.get_text.return_value = "0.000001"
        viewer._on_drc_delta_step()
        self.assertEqual(viewer._drc_delta_step, 10000)
        self.assertIs(viewer._drc_delta_group, selected)
        viewer._drc_delta_worker.submit.assert_not_called()
        viewer._drcwin._delta_step.get_text.return_value = "0.025"
        viewer._on_drc_delta_step()
        self.assertEqual(viewer._drc_delta_step, 2500)
        viewer._drcwin._delta_step.set_text.assert_called_with("0.02500")
        self.assertIsNone(viewer._drc_delta_group)
        self.assertEqual(viewer._drc_page, 0)
        viewer._drc_delta_worker.submit.assert_called_once()

    def test_auto_step_starts_pending_and_uses_result_without_resubmitting(self):
        viewer = use_auto(fixture())
        self.assertTrue(viewer._drc_delta_sync())
        self.assertIsNone(viewer._drc_delta_worker.submit.call_args.args[4])
        self.assertIsNone(viewer._drc_delta_step)
        self.assertEqual(viewer._drcwin._delta_step.get_text(), "auto")
        groups = reply_groups(viewer, 20000)
        self.assertEqual(viewer._drc_delta_step, 20000)
        self.assertEqual(viewer._drc_delta_auto_steps[(0, "absolute")], 20000)
        self.assertEqual(viewer._drc_delta_steps, {})
        self.assertEqual(viewer._drcwin._delta_step.get_text(), "0.20000")
        self.assertIs(viewer._drc_delta_groups, groups)
        self.assertFalse(viewer._drc_delta_sync())
        viewer._drc_delta_worker.submit.assert_called_once()

    def test_auto_and_manual_steps_are_separate_for_each_rule_and_mode(self):
        viewer = use_auto(rule_tree_fixture())
        viewer._drc_delta_sync()
        reply_groups(viewer, 20000)
        viewer._drcwin._delta_step.get_text.return_value = "0.025"
        viewer._on_drc_delta_step()
        self.assertEqual(viewer._drc_delta_steps[(0, "absolute")], 2500)
        combo = Mock()
        combo.get_active_id.return_value = "percent"
        viewer._on_drc_delta_mode(combo)
        self.assertIsNone(viewer._drc_delta_worker.submit.call_args.args[4])
        self.assertIsNone(viewer._drc_delta_step)
        reply_groups(viewer, 500000)
        viewer._drcwin._delta_step.get_text.return_value = "12.5"
        viewer._on_drc_delta_step()
        self.assertEqual(viewer._drc_delta_steps[(0, "percent")], 1250000)
        combo.get_active_id.return_value = "absolute"
        viewer._on_drc_delta_mode(combo)
        self.assertEqual(viewer._drc_delta_step, 2500)
        self.assertEqual(viewer._drcwin._delta_step.get_text(), "0.02500")
        viewer._drc_select_row(1)
        self.assertIsNone(viewer._drc_delta_worker.submit.call_args.args[4])
        self.assertIsNone(viewer._drc_delta_step)
        reply_groups(viewer, 50000)
        self.assertEqual(viewer._drc_delta_auto_steps[(1, "absolute")], 50000)
        viewer._drc_select_row(0)
        self.assertEqual(viewer._drc_delta_step, 2500)
        self.assertEqual(viewer._drc_delta_worker.submit.call_args.args[4], 2500)
        combo.get_active_id.return_value = "percent"
        viewer._on_drc_delta_mode(combo)
        self.assertEqual(viewer._drc_delta_step, 1250000)
        self.assertEqual(viewer._drcwin._delta_step.get_text(), "12.50000")
        viewer._drc_delta_reset(drop_worker=True)
        self.assertEqual(viewer._drc_delta_steps, {})
        self.assertEqual(viewer._drc_delta_auto_steps, {})
        self.assertIsNone(viewer._drc_delta_step_scope)

    def test_entering_current_auto_value_is_manual_and_auto_or_blank_restores_auto(self):
        viewer = use_auto(fixture())
        viewer._drc_delta_sync()
        reply_groups(viewer, 20000)
        for reset_text in ("auto", ""):
            with self.subTest(reset_text=reset_text):
                viewer._drcwin._delta_step.get_text.return_value = "0.20000"
                viewer._on_drc_delta_step()
                self.assertEqual(viewer._drc_delta_steps[(0, "absolute")], 20000)
                self.assertEqual(viewer._drc_delta_worker.submit.call_args.args[4], 20000)
                reply_groups(viewer, 20000)
                viewer._drcwin._delta_step.get_text.return_value = reset_text
                viewer._on_drc_delta_step()
                self.assertNotIn((0, "absolute"), viewer._drc_delta_steps)
                self.assertIsNone(viewer._drc_delta_worker.submit.call_args.args[4])
                reply_groups(viewer, 20000)

    def test_auto_reply_preserves_entry_draft_and_invalid_input_preserves_scope(self):
        viewer = use_auto(fixture())
        viewer._drc_delta_sync()
        viewer._drcwin._delta_step.get_text.return_value = "0.075"
        reply_groups(viewer, 20000)
        self.assertEqual(viewer._drc_delta_step, 20000)
        self.assertEqual(viewer._drcwin._delta_step.get_text(), "0.075")
        viewer._on_drc_delta_step()
        self.assertEqual(viewer._drc_delta_steps[(0, "absolute")], 7500)
        groups = reply_groups(viewer, 7500)
        viewer._drc_delta_choose(groups[0])
        selected_scope = viewer._drc_scope_key()
        submissions = viewer._drc_delta_worker.submit.call_count
        viewer._drcwin._delta_step.get_text.return_value = "0.000001"
        viewer._on_drc_delta_step()
        self.assertEqual(viewer._drc_delta_step, 7500)
        self.assertEqual(viewer._drc_scope_key(), selected_scope)
        self.assertIs(viewer._drc_delta_group, groups[0])
        self.assertEqual(viewer._drc_delta_worker.submit.call_count, submissions)

    def test_stale_auto_reply_cannot_overwrite_manual_or_another_rule_step(self):
        viewer = use_auto(rule_tree_fixture())
        viewer._drc_delta_sync()
        auto_key = viewer._drc_delta_key
        viewer._drcwin._delta_step.get_text.return_value = "0.05"
        viewer._on_drc_delta_step()
        reply_groups(viewer, 20000, key=auto_key)
        self.assertEqual(viewer._drc_delta_step, 5000)
        self.assertEqual(viewer._drcwin._delta_step.get_text(), "0.05000")
        self.assertEqual(viewer._drc_delta_auto_steps, {})
        manual_groups = reply_groups(viewer, 5000)
        manual_key = viewer._drc_delta_key
        viewer._drc_select_row(1)
        current = reply_groups(viewer, 50000)
        viewer._drc_delta_worker.poll.return_value = (manual_key, manual_groups, None)
        viewer._drc_delta_poll()
        self.assertIs(viewer._drc_delta_groups, current)
        self.assertEqual(viewer._drc_delta_step, 50000)
        self.assertEqual(viewer._drcwin._delta_step.get_text(), "0.50000")
        self.assertEqual(viewer._drc_delta_auto_steps, {(1, "absolute"): 50000})

    def test_cluster_and_review_status_keep_the_whole_rule_auto_step(self):
        viewer = use_auto(fixture())
        viewer._drc_delta_sync()
        reply_groups(viewer, 20000)
        viewer._drc_cluster = Mock()
        viewer._drc_delta_sync()
        self.assertEqual(viewer._drc_delta_step, 20000)
        self.assertIsNone(viewer._drc_delta_worker.submit.call_args.args[4])
        self.assertIs(viewer._drc_delta_worker.submit.call_args.args[6], viewer._drc_cluster)
        reply_groups(viewer, 20000)
        submissions = viewer._drc_delta_worker.submit.call_count
        viewer._drc._status[0] = 1
        viewer._drc_delta_status_changed(0, [0])
        self.assertEqual(viewer._drc_delta_worker.submit.call_count, submissions)
        self.assertEqual(viewer._drc_delta_step, 20000)
        viewer._drc_delta_busy = True
        viewer._drc_delta_status_changed(0, [0])
        self.assertEqual(viewer._drc_delta_step, 20000)
        self.assertIsNone(viewer._drc_delta_worker.submit.call_args.args[4])
        self.assertEqual(viewer._drc_delta_auto_steps, {(0, "absolute"): 20000})

    def test_mode_switch_uses_independent_absolute_and_percentage_steps(self):
        viewer = fixture()
        combo = Mock()
        combo.get_active_id.return_value = "percent"
        viewer._on_drc_delta_mode(combo)
        self.assertEqual(viewer._drc_delta_mode, "percent")
        self.assertEqual(viewer._drc_delta_step, 1000000)
        viewer._drcwin._delta_step.set_text.assert_called_with("10.00000")
        viewer._drc_delta_worker.submit.assert_called_once()
        combo.get_active_id.return_value = "off"
        viewer._on_drc_delta_mode(combo)
        self.assertFalse(viewer._drc_delta_busy)
        viewer._drc_delta_worker.cancel.assert_called_once()

    def test_stale_reply_cannot_restore_old_rule_or_step_groups(self):
        viewer = fixture()
        first_groups = viewer._drc_delta_groups
        self.assertTrue(viewer._drc_delta_sync())
        first_key = viewer._drc_delta_key
        self.assertFalse(viewer._drc_delta_sync())
        viewer._drcwin._delta_step.get_text.return_value = "0.05000"
        viewer._on_drc_delta_step()
        current_key = viewer._drc_delta_key
        self.assertNotEqual(current_key, first_key)
        viewer._drc_delta_worker.poll.return_value = (first_key, first_groups, None)
        viewer._drc_delta_poll()
        self.assertIsNone(viewer._drc_delta_groups)
        self.assertTrue(viewer._drc_delta_busy)
        viewer._drc_delta_worker.poll.return_value = (current_key, first_groups, None)
        viewer._drc_delta_poll()
        self.assertIs(viewer._drc_delta_groups, first_groups)
        self.assertIsNone(viewer._drc_delta_group, "arrival must keep All errors active")
        self.assertFalse(viewer._drc_delta_busy)

    def test_group_directory_has_fifty_rows_plus_all_and_independent_paging(self):
        viewer = fixture([edge((100000 + ei) / 100000) for ei in range(153)],
                         [constraint("length", 1, ">=")])
        viewer._drc_delta_groups = delta.DeltaIndex(
            viewer._drc, 0, [constraint("length", 1, ">=")]).measure().group(1)
        viewer._drc_delta_group = viewer._drc_delta_groups[7]
        viewer._drc_delta_step = 1
        viewer._drc_delta_steps[(0, "absolute")] = 1
        viewer._drc_page = 11
        win = viewer._drcwin
        win._delta_store = GroupStore()
        win._delta_tree = Mock()
        win._delta_label = Mock()
        win._delta_prev, win._delta_next = Mock(), Mock()
        viewer._drc_delta_refresh()
        self.assertEqual(len(win._delta_store), 51)
        self.assertEqual(win._delta_store[0][0], "All errors")
        self.assertEqual([row[2].key for row in win._delta_store[1:]],
                         [(0, ei) for ei in range(50)])
        self.assertEqual([row[0].split(" · ", 1)[0] for row in win._delta_store[1:]],
                         ["Group #%d" % number for number in range(1, 51)])
        win._delta_tree.get_selection().select_iter.assert_called_with(8)
        win._delta_tree.get_selection().select_iter.reset_mock()
        viewer._drc_delta_page_step(1)
        self.assertEqual([row[2].key for row in win._delta_store[1:]],
                         [(0, ei) for ei in range(50, 100)])
        self.assertEqual([row[0].split(" · ", 1)[0] for row in win._delta_store[1:]],
                         ["Group #%d" % number for number in range(51, 101)])
        win._delta_tree.get_selection().select_iter.assert_not_called()
        self.assertEqual(viewer._drc_delta_group.key, (0, 7))
        self.assertEqual(viewer._drc_page, 11)
        viewer._drc_delta_page_step(100)
        self.assertEqual(len(win._delta_store), 4)
        self.assertEqual(viewer._drc_delta_page, 3)
        self.assertEqual([row[0].split(" · ", 1)[0] for row in win._delta_store[1:]],
                         ["Group #151", "Group #152", "Group #153"])

    def test_status_changes_update_groups_without_remeasuring_finished_index(self):
        viewer = fixture([rect(0.8), rect(1.2)])
        groups = viewer._drc_delta_groups
        viewer._drc._status[0] = 1
        viewer._drc_delta_status_changed(0, [0])
        self.assertEqual(groups[1].status_counts(), (1, 1))
        viewer._drc_delta_worker.submit.assert_not_called()
        viewer._drc_delta_busy = True
        viewer._drc_delta_status_changed(0, [0])
        self.assertEqual(viewer._drc_delta_revision, 1)
        viewer._drc_delta_worker.submit.assert_called_once()


if __name__ == "__main__":
    unittest.main(verbosity=2)
