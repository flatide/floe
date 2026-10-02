#!/usr/bin/env python3
"""Validate the in-tree Rust render worker contract and real daemon bridge."""

import os
import queue
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from floe import RENDERD_VERSION, __version__  # noqa: E402
from floe.rust_render import (  # noqa: E402
    CELL_QUERY_KINDS,
    RustRenderWorker,
    _parse_wire_line,
    _pattern_fill,
    _RAW_HEADER_LEN,
    _RAW_SIGNATURE,
)


class FakeCache:
    def __init__(self, directory):
        self.dir = directory
        self.src = os.path.join(directory, "source.oas")
        self.meta = {
            "dbu": 0.001,
            "layers": [
                {"layer": 2, "datatype": 0, "color": "#222222"},
                {"layer": 1, "datatype": 0, "color": "#111111"},
            ],
        }


_MARGIN_KEY = ("live", (), 999, 3.0, True, True, True, 0)


def _stub_margin_viewer(worker, frame_cache, viewport=(858, 802)):
    """A Viewer shell with just the state the margin path reads,
    holding a settled exact frame (viewport + 1 px per side)."""
    from floe.gui import Viewer

    v = Viewer.__new__(Viewer)
    v.cache = object()
    v._drag = v._pending = None
    v.worker = worker
    v.spp, v.cx, v.cy = 10.0, 100000.0, 100000.0
    v.gen = 5
    v._job_keys, v._job_depth = {}, {}
    v._render_key = lambda scope: _MARGIN_KEY
    v._depth = lambda: 999
    v._effective_cut_px = lambda: 3.0
    v.lod_on = v.frames_on = v.labels_on = True
    v.label_font_px = 14
    v.frame_cache_on = frame_cache
    v.margin_on = True
    v.abstract = False
    v._layers_arg = lambda: None
    v._viewport_size = lambda: viewport
    v._margin_pending = None

    def view_bbox():
        w, h = v._viewport_size()
        return (v.cx - w / 2 * v.spp, v.cy - h / 2 * v.spp,
                v.cx + w / 2 * v.spp, v.cy + h / 2 * v.spp)
    v.view_bbox = view_bbox
    b = view_bbox()
    # _covered wants >= 0.25 of the extra on every side
    v.last_frame = (None, (b[0] - v.spp, b[1] - v.spp,
                           b[2] + v.spp, b[3] + v.spp), v.spp, _MARGIN_KEY)
    return v


class WorkerContractTests(unittest.TestCase):
    def test_single_instance_forwards_effective_detail_and_depth(self):
        from floe import cli, instance

        def forward(detail, depth, thin=None):
            args = SimpleNamespace(
                src="/tmp/forwarded.oas", hairline=None, thin_um=None,
                goto="1,2,700", stream_kb=None, stream_target_ms=500,
                label_font_px=14, perf_baseline=False, lod="on",
                frames="on", labels="on", refinement="on",
                frame_cache="on", margin="off", render_debug=False,
                multi=False, drc=None, detail=detail, depth=depth,
                dump=False, thin=thin,
            )
            with mock.patch.object(cli.os.path, "isfile", return_value=True), \
                    mock.patch.object(cli, "_cache_ready", return_value=True), \
                    mock.patch.object(instance, "display_key",
                                      return_value=":test"), \
                    mock.patch.object(instance, "socket_address",
                                      return_value="/tmp/floe-test.sock"), \
                    mock.patch.object(instance, "try_forward",
                                      return_value=0) as try_forward:
                with self.assertRaises(SystemExit) as stopped:
                    cli.cmd_view(args)
            self.assertEqual(stopped.exception.code, 0)
            return try_forward.call_args.args[1]

        explicit = forward("high", 7)
        self.assertIn("\tdetail=high\tdepth=7\t", explicit)
        implicit = forward(None, None)
        self.assertIn("\tdetail=medium\tdepth=999\t", implicit)
        # review 2026-09-11 P2-3: an explicit --thin reaches the window,
        # auto included (a window left on keep returns to its default);
        # an unspecified one is not sent
        self.assertNotIn("thin=", implicit)
        self.assertIn("\tthin=auto", forward("high", 7, "auto"))
        self.assertIn("\tthin=keep", forward("high", 7, "keep"))

    def test_render_key_carries_the_thin_policy(self):
        """Review 2026-09-11 P1-1: toggling View > keep thin shapes must
        invalidate the displayed frame (and the margin frame, which
        shares the key) - the key differs by the effective policy."""
        from types import SimpleNamespace
        from floe import gui
        v = gui.Viewer.__new__(gui.Viewer)
        v.visible = {(1, 0)}
        v._depth_key = lambda: ("d", 3)
        v.cut_px = 1.0
        v.lod_on = True
        v.frames_on = False
        v.labels_on = False
        v._color_epoch = 5
        v.cache = SimpleNamespace(is_jobdeck=False)
        v.thin_mode = "auto"
        auto = gui.Viewer._render_key(v, "live")
        v.thin_mode = "keep"
        keep = gui.Viewer._render_key(v, "live")
        self.assertEqual(keep, auto,
                         "auto on a layout is keep (2026-09-23)")
        v.thin_mode = "cull"
        self.assertNotEqual(gui.Viewer._render_key(v, "live"), auto)
        v.thin_mode = "auto"
        v.cache = SimpleNamespace(is_jobdeck=True)
        self.assertEqual(gui.Viewer._render_key(v, "live"), keep,
                         "auto on a deck is keep")
        # the displayed frame and the margin frame are judged by the
        # same key
        import inspect
        self.assertIn("_render_key(", inspect.getsource(gui.Viewer._covered))
        self.assertIn('margin[3] != self._render_key("live")',
                      inspect.getsource(gui))

    def test_forwarded_view_options_batch_before_one_goto(self):
        from floe import gui

        class Status:
            text = None

            def set_text(self, value):
                self.text = value

        viewer = gui.Viewer.__new__(gui.Viewer)
        viewer.detail = 1
        viewer.cut_px = gui.DETAIL_PX[1]
        viewer.depth_value = 0
        viewer.lod_on = True
        viewer.frames_on = True
        viewer.labels_on = True
        viewer.label_font_px = 14
        viewer._ddlg = None
        viewer._fontdlg = None
        viewer.worker = SimpleNamespace(supports_label_font_px=True)
        viewer.dstatus = Status()
        viewer._depth_label = lambda: "forwarded state"
        viewer.redraw = mock.Mock()

        changed = gui.Viewer._forwarded_view_options(viewer, [
            "detail=high", "depth=999", "lod=off", "frames=off",
            "labels=off", "labelpx=18",
        ])
        self.assertTrue(changed)
        self.assertEqual(viewer.detail, 2)
        self.assertEqual(viewer.depth_value, 999)
        # lod= is retired from the viewer (2026-09-22): accepted, ignored
        self.assertTrue(viewer.lod_on)
        self.assertFalse(viewer.frames_on)
        self.assertFalse(viewer.labels_on)
        self.assertEqual(viewer.label_font_px, 18)
        self.assertEqual(viewer.dstatus.text, "forwarded state")
        viewer.redraw.assert_not_called()

        viewer.goto = mock.Mock()
        jumped = gui.Viewer._forwarded_goto(
            viewer, ["goto=13600,8600,700"])
        self.assertTrue(jumped)
        viewer.goto.assert_called_once_with(13600.0, 8600.0, 700.0)

    def test_cold_gui_open_redraws_without_overwriting_cli_goto(self):
        from floe import gui

        worker = object()
        calls = []
        viewer = SimpleNamespace(
            worker=worker,
            _worker_starting=True,
            _fit_after_worker_start=False,
            _did_fit=True,
            _sync_label_font_capability=lambda: None,
            _sync_abstract_capability=lambda: None,
            fit=lambda: calls.append("fit"),
            redraw=lambda immediate=False: calls.append(
                ("redraw", immediate)),
        )
        self.assertFalse(gui.Viewer._worker_start_finished(
            viewer, worker, None))
        self.assertEqual(calls, [("redraw", True)])

    def test_layout_switch_keeps_its_deferred_fit(self):
        from floe import gui

        worker = object()
        calls = []
        viewer = SimpleNamespace(
            worker=worker,
            _worker_starting=True,
            _fit_after_worker_start=True,
            _did_fit=True,
            _sync_label_font_capability=lambda: None,
            _sync_abstract_capability=lambda: None,
            fit=lambda: calls.append("fit"),
            redraw=lambda immediate=False: calls.append(
                ("redraw", immediate)),
        )
        self.assertFalse(gui.Viewer._worker_start_finished(
            viewer, worker, None))
        self.assertEqual(calls, ["fit"])
        self.assertFalse(viewer._fit_after_worker_start)

    def test_common_perf_baseline_disables_optional_render_work(self):
        from floe import cli

        args = SimpleNamespace(
            src=None, hairline=None, thin_um=None, goto=None,
            stream_kb=None, stream_target_ms=500, label_font_px=14,
            perf_baseline=True, lod="on", frames="on", labels="on",
            refinement="on", frame_cache="on", margin="on",
            render_debug=False, multi=True, drc=None, detail="high",
            depth=999, dump=False,
        )
        with mock.patch("floe.gui.run_viewer") as run_viewer:
            cli.cmd_view(args)
        options = run_viewer.call_args.kwargs
        # the viewer has no LOD toggle any more (2026-09-22)
        self.assertNotIn("lod", options)
        self.assertFalse(options["frames"])
        self.assertFalse(options["labels"])
        self.assertFalse(options["frame_cache"])
        # frame_cache off already stops the margin; the flag itself stays
        self.assertTrue(options["margin"])
        self.assertEqual(options["stream_kb"], 0)
        self.assertEqual(options["detail"], 2)
        self.assertEqual(options["depth"], 999)

    def test_refinement_off_rejects_nonzero_stable_stream_budget(self):
        from floe import cli

        args = SimpleNamespace(
            src=None, hairline=None, thin_um=None, goto=None,
            stream_kb=4096, stream_target_ms=500, label_font_px=14,
            perf_baseline=False, lod="off", frames="off", labels="off",
            refinement="off", frame_cache="off", margin="on",
            render_debug=False, multi=True, drc=None, detail="high",
            depth=999, dump=False,
        )
        with self.assertRaisesRegex(SystemExit, "conflicts"):
            cli.cmd_view(args)

    def test_margin_option_turns_the_prefetch_alone_off(self):
        """--margin off (user request 2026-09-27): the background margin
        prefetch alone stays off - retained-frame pan reuse (frame_cache)
        stays on - so a margin's landing can be told apart from the frame
        itself. It is a process option (an independent instance)."""
        from floe import cli
        from floe.gui import Viewer

        args = SimpleNamespace(
            src=None, hairline=None, thin_um=None, goto=None,
            stream_kb=None, stream_target_ms=500, label_font_px=14,
            perf_baseline=False, lod="on", frames="on", labels="on",
            refinement="on", frame_cache="on", margin="off",
            render_debug=False, multi=True, drc=None, detail="high",
            depth=999, dump=False,
        )
        with mock.patch("floe.gui.run_viewer") as run_viewer:
            cli.cmd_view(args)
        options = run_viewer.call_args.kwargs
        self.assertFalse(options["margin"])
        self.assertTrue(options["frame_cache"])
        # off is the default (user decision 2026-09-27): a launch without
        # the option still forwards to the single instance; on is the
        # process option that opens an independent one
        args.multi = False
        args.margin = "on"
        with mock.patch("floe.gui.run_viewer") as run_viewer, \
                mock.patch("floe.instance.display_key") as display_key:
            cli.cmd_view(args)
        self.assertTrue(run_viewer.call_args.kwargs["margin"])
        self.assertFalse(display_key.called)
        rust = SimpleNamespace(supports_margin_prefetch=True)
        v = _stub_margin_viewer(rust, True)
        self.assertTrue(Viewer._margin_enabled(v))
        v.margin_on = False
        self.assertFalse(Viewer._margin_enabled(v))
        self.assertTrue(v.frame_cache_on)

    def test_rust_gui_startup_does_not_import_klayout(self):
        with tempfile.TemporaryDirectory() as directory:
            binary = os.path.join(directory, "floe-renderd")
            with open(binary, "w", encoding="ascii") as script:
                script.write("#!/bin/sh\n")
            os.chmod(binary, 0o755)
            code = r'''
import builtins
import os
import sys

real_import = builtins.__import__

def guarded_import(name, *args, **kwargs):
    if name == "klayout" or name.startswith("klayout."):
        raise ImportError("KLayout intentionally unavailable")
    return real_import(name, *args, **kwargs)

builtins.__import__ = guarded_import
os.environ.pop("FLOE_RENDERER", None)

import floe2
from floe import cache
from floe import gui
from floe.service import make_render_worker

class Cache:
    src = "/tmp/source.oas"
    dir = "/tmp/.source.oas.ice"
    meta = {"dbu": 0.001, "layers": []}

worker = make_render_worker(Cache())
assert worker.__class__.__name__ == "RustRenderWorker"
assert cache.db.__class__.__name__ == "_LazyKLayoutDb"
assert all(not name.startswith("klayout") for name in sys.modules)
assert gui.live_caps({"grid": {"nx": 1, "ny": 1},
                      "src": {"size": 1}}) == (256, 1024)
'''
            environment = os.environ.copy()
            environment.update({
                "FLOE_RENDERD_BIN": binary,
                "PYTHONDONTWRITEBYTECODE": "1",
            })
            environment.pop("FLOE_RENDERER", None)
            completed = subprocess.run(
                [sys.executable, "-B", "-c", code], cwd=str(ROOT),
                env=environment, stdout=subprocess.PIPE,
                stderr=subprocess.PIPE, text=True, timeout=10)
            self.assertEqual(completed.returncode, 0, completed.stderr)

    def test_klayout_is_an_explicit_abstract_capable_rollback(self):
        from floe import service
        from floe.cli import _renderer_backend

        sentinel = object()
        with tempfile.TemporaryDirectory() as directory, \
                mock.patch.dict(os.environ, {
                    "FLOE_RENDERER": "klayout",
                }, clear=False), \
                mock.patch.object(
                    service, "RenderWorker", return_value=sentinel) as ctor:
            selected = service.make_render_worker(FakeCache(directory))
        self.assertIs(selected, sentinel)
        ctor.assert_called_once()
        self.assertTrue(service.RenderWorker.supports_abstract)
        self.assertFalse(RustRenderWorker.supports_abstract)
        with mock.patch.dict(os.environ, {
                "FLOE_PRODUCT": "floe2", "FLOE_RENDERER": ""},
                             clear=False):
            self.assertEqual(_renderer_backend(), "rust")
        with mock.patch.dict(os.environ, {"FLOE_RENDERER": "unknown"},
                             clear=False):
            with self.assertRaisesRegex(SystemExit, "klayout or rust"):
                _renderer_backend()

    def test_gui_abstract_control_follows_backend_capability(self):
        from types import SimpleNamespace
        from floe.gui import Viewer

        class Item:
            sensitive = None

            def set_sensitive(self, value):
                self.sensitive = bool(value)

        item = Item()
        viewer = SimpleNamespace(
            worker=SimpleNamespace(supports_abstract=False),
            abstract=True, _abstract_menu_item=item)
        Viewer._sync_abstract_capability(viewer)
        self.assertFalse(viewer.abstract)
        self.assertFalse(item.sensitive)
        Viewer._toggle_abstract(viewer)
        self.assertFalse(viewer.abstract)

        redraws = []
        viewer.worker = SimpleNamespace(supports_abstract=True)
        viewer._on_depth = lambda: redraws.append(True)
        Viewer._sync_abstract_capability(viewer)
        Viewer._toggle_abstract(viewer)
        self.assertTrue(viewer.abstract)
        self.assertTrue(item.sensitive)
        self.assertEqual(redraws, [True])

    def test_a_dropped_foreground_render_clears_the_pending_state(self):
        """Review 2026-09-30: the mouse waits on _pending until the pending
        generation's first frame; a `dropped` answer for that generation
        (unreachable today - a stale generation is never sent) must clear it
        rather than leave the viewer waiting for ever; another generation's
        `dropped` (a margin's) leaves it be."""
        from floe.gui import Viewer

        texts = []
        worker = SimpleNamespace(submit=lambda job: None)
        v = _stub_margin_viewer(worker, True)
        v._pending_timer = None
        v._pending = 5
        v.rstatus = SimpleNamespace(set_text=texts.append)
        v._set_cursor = lambda cursor: None
        v._idle_cursor = lambda: "idle"
        v._margin_debug = lambda message: None
        Viewer._handle_result(v, {"kind": "dropped", "gen": 4, "reason": "fit"})
        self.assertEqual(v._pending, 5, "another generation's drop")
        Viewer._handle_result(v, {"kind": "dropped", "gen": 5, "reason": "stale"})
        self.assertIsNone(v._pending)
        self.assertEqual(texts[-1], "render dropped (stale)")

    def _render_wait_viewer(self, worker, pending=5):
        """A Viewer shell in the middle of a render (gen == _pending)."""
        v = _stub_margin_viewer(worker, True)
        v.gen = v._pending = pending
        v._pending_timer = None
        v._pending_t0 = 0.0
        v._debounce = None
        v._refining = False
        v._preview_gen = None
        v.texts = []
        v.rstatus = SimpleNamespace(set_text=v.texts.append)
        v._set_cursor = lambda cursor: None
        v._idle_cursor = lambda: "idle"
        v._display = lambda: None
        v._set_live_status = v.texts.append
        v._margin_debug = lambda message: None
        v.mode = "normal"
        v.rulers = [object()]
        return v

    def test_esc_cancels_the_render_in_flight_before_the_chain(self):
        """Field 2026-09-30: Esc during "rendering…" did nothing to the
        render. Now the first Esc moves the daemon's frontier past the
        pending generation (worker.cancel(before_gen)), bumps the viewer's
        generation so a late frame of the cancelled one is not shown, and
        clears the pending state; the rulers and the rest of the chain wait
        for the next Esc. A worker without cancel still unblocks; the density
        round being drawn (refining, nothing pending) is cancelled the same
        way."""
        from floe.gui import Viewer

        cancelled = []
        v = self._render_wait_viewer(SimpleNamespace(submit=lambda job: None, cancel=cancelled.append))
        Viewer._esc(v)
        self.assertEqual(cancelled, [6], "before_gen = the cancelled generation + 1")
        self.assertEqual((v._pending, v.gen, v._refining), (None, 6, False))
        self.assertEqual(v.texts[-1], "render cancelled")
        self.assertEqual(len(v.rulers), 1, "the chain waits for the next Esc")
        # the density round being drawn: refining, nothing pending
        v = self._render_wait_viewer(SimpleNamespace(submit=lambda job: None, cancel=cancelled.append))
        v._pending = None
        v._refining = True
        Viewer._esc(v)
        self.assertEqual((cancelled[-1], v._refining, v.gen), (6, False, 6))
        # a worker without cancel (the KLayout service): the state clears anyway
        v = self._render_wait_viewer(SimpleNamespace(submit=lambda job: None))
        Viewer._esc(v)
        self.assertIsNone(v._pending)

    def test_a_wheel_zoom_during_a_render_supersedes_it(self):
        """The mouse no longer waits for the frame (2026-09-30): a wheel
        event while a render is pending reaches _zoom_at (the redraw it
        triggers submits the next generation, which the daemon runs while
        the old one stops at its next look)."""
        from floe import gui
        from floe.gui import Viewer

        # the handler reads Gdk's masks and directions: a stand-in works
        # without a display (the margin tests' shell has none)
        fake_gdk = SimpleNamespace(
            ModifierType=SimpleNamespace(BUTTON1_MASK=1, BUTTON2_MASK=2, BUTTON3_MASK=4),
            ScrollDirection=SimpleNamespace(UP="up", DOWN="down", SMOOTH="smooth"))
        zoomed = []
        v = self._render_wait_viewer(SimpleNamespace(submit=lambda job: None))
        v._drag = v._zoomdrag = None
        v._zoom_at = lambda x, y, factor: zoomed.append((x, y, factor))
        ev = SimpleNamespace(state=0, direction="up", x=10.0, y=20.0)
        with mock.patch.object(gui, "Gdk", fake_gdk):
            self.assertTrue(Viewer._on_scroll(v, None, ev))
        self.assertEqual(len(zoomed), 1)
        self.assertEqual(zoomed[0][:2], (10.0, 20.0))

    def test_a_settled_frame_of_this_view_does_not_render_again(self):
        """Field 2026-09-30 ("rendering repeats"): 0.12.251 re-rendered
        after every settled frame that _covered did not accept - and with the
        margin off _covered wants comfort around the view that a fresh
        viewport frame (<= 2 px snap slack) never has. A frame that holds the
        view tops the margin up; one the view has left renders it; a pending
        debounce or a pan in progress does neither."""
        from floe.gui import Viewer

        calls = []
        v = self._render_wait_viewer(SimpleNamespace(submit=lambda job: None))
        v._pending = None
        v._drag = None
        v.margin_on = False
        v.redraw = lambda immediate=False: calls.append("redraw")
        v._schedule_margin = lambda: calls.append("margin")
        b = v.view_bbox()
        # the exact viewport frame: the snap grows it by 2 px on one side only
        v.last_frame = (None, (b[0], b[1] - 2 * v.spp, b[2] + 2 * v.spp, b[3]), v.spp, _MARGIN_KEY)
        self.assertFalse(Viewer._covered(v, b, "live"), "the reuse test refuses it (margin off)")
        Viewer._settle_after_frame(v)
        self.assertEqual(calls, ["margin"], "a frame of this view renders nothing again")
        # the view moved away while the frame was drawn, nothing submitted
        v.cx += 500 * v.spp
        Viewer._settle_after_frame(v)
        self.assertEqual(calls[-1], "redraw")
        # a render of the new view already on its way, or a pan in progress
        n = len(calls)
        v._debounce = 17
        Viewer._settle_after_frame(v)
        v._debounce, v._drag = None, (1.0, 2.0)
        Viewer._settle_after_frame(v)
        self.assertEqual(len(calls), n)

    def test_an_older_generations_error_leaves_the_pending_render(self):
        """An error of a superseded generation (its late failure) must not
        clear the state of the one now pending; the pending one's, and an
        adapter failure without a generation, must."""
        from floe.gui import Viewer

        v = self._render_wait_viewer(SimpleNamespace(submit=lambda job: None))
        Viewer._handle_result(v, {"kind": "error", "gen": 4, "msg": "late"})
        self.assertEqual(v._pending, 5)
        Viewer._handle_result(v, {"kind": "cancelled", "gen": 4, "phase": "render"})
        self.assertEqual(v._pending, 5, "an older generation's cancellation")
        Viewer._handle_result(v, {"kind": "cancelled", "gen": 5, "phase": "queued"})
        self.assertIsNone(v._pending)
        v = self._render_wait_viewer(SimpleNamespace(submit=lambda job: None))
        Viewer._handle_result(v, {"kind": "error", "msg": "submit failed"})
        self.assertIsNone(v._pending, "an adapter failure carries no generation")

    def test_the_bar_shows_the_brief_perf_line_and_the_log_keeps_the_whole(self):
        """User 2026-10-01: "the log has it all; the bar should show only
        what is needed now, without an ellipsis". perf_status gives the
        full line - the terminal log and the bar's tooltip - and the brief
        one the lower bar shows: the frame's time and its load / draw split,
        what pass 2 lit and its plan (nodes, reads, the cells' dots; a
        probe, a fit past one pass with its floor, threads and decoded pages
        only when there are any), the work bin (with the hierarchy walk when
        it is off), the cut and its budget fit, and what the picture lacks.
        Pass 2's shapes under the cut look like dots too but are no cell's:
        a frame lights them with no cell dot (user 2026-10-01: "two draws
        and dots, yet dot items 0")."""
        from floe.gui import Viewer, perf_status

        # the field's frame of 2026-10-01 (renderd 0.12.242)
        res = {
            "tiles": 10552, "new": 9172, "ms": 4324, "load_ms": 250,
            "draw_ms": 3916, "phase_plan": 6, "phase_delta": 152,
            "phase_apply": 92, "cut_um": 7.56, "plan_ms": 6.1,
            "frame_rects": 0, "png_ms": 0.0, "publish_ms": 7.3,
            "frame_format": "raw", "raster_jobs": 4, "render_tiles": 15,
            "tile_px": 384, "frame_width": 1920, "frame_height": 1080,
            "work_bin_items": 2196, "member_paints": 686000,
            "hier_cells_visited": 2007763, "subtrees_pruned": 1735783,
            "plan_culls": {"shape_cut": 1, "shape_cut_max": 1,
                           "pages_size": 538, "child_bvh": 31601},
            "summary": {"none": "off"},
            "density_stack": {"lit": 619861}, "density_dots": {"items": 589148},
            "density_block": 4.0, "density_floor": 1.0,
            "density_us": {"plan2_us": 3494000},
            "density_plan2": {"probe_us": 0, "probes": 0, "fit_us": 3494000,
                              "passes": 1, "regions": 24, "nodes": 4000000,
                              "page_nodes": 0, "page_candidates": 19000,
                              "threads": 1, "reads": 15235408,
                              "items": 2655158},
            "density_pages": {"planned": 266, "in_hand": 266, "decoded": 0},
        }
        full, brief = perf_status(res, ", depth 3")
        self.assertEqual(
            brief,
            "4324 ms = 250 load + 3916 draw · density: lit 620k px, pass 2"
            " plan 3494 ms (nodes 4.0M, reads 15.2M, cell dots 2.7M) · bin"
            " 2196 items · cut<7.56um")
        # the log line keeps every diagnostic, as before
        for part in (
                "live [density: dots, lit 620k px, block 4 px, floor 1 px,"
                " pass 2 plan 3494 ms (probe 0 ms x0, fit 3494 ms x1 passes"
                " on 1 threads, 24 regions, nodes 4.0M, page nodes 0, pages"
                " 19k, reads 15.2M, cell dots 2.7M), 0 pages] (10552 tiles,"
                " +9172 new,"
                " 4324 ms = 250 load [6 plan+152 delta+92 apply] + 3916 draw"
                ", depth 3, cut<7.56um (larger side), plan 6.1ms/0 frontier",
                ", rust 4j 15tiles@384px 1920x1080", ", bin 2196 items",
                ", paints 686k", ", hier 2.0M/1.7M pruned",
                ", cut pages 538/", ", summary: none (off))"):
            self.assertIn(part, full)

        # what the bar adds only when it is there: the bin off with its
        # walk, a probe, a fit with its floor, threads, decoded pages, the
        # budget fit, a picture short of pages, partial labels, evictions
        res.pop("work_bin_items")
        res.update(work_bin_overflow_items=786433, other_ms=1693,
                   over_budget_pages=3, labels_truncated=True,
                   cache_evicted=1200, density_floor=15.0)
        res["plan_culls"].update(fit_pct=200, fit_over=1)
        res["density_plan2"].update(probes=1, probe_us=637000, passes=5,
                                    threads=4, probes_over=1, thinned=1)
        # where the cells' dot items came from (2026-10-02): the ones there
        # are, a chunk of a point list one item, in the log line only
        res["density_plan2"].update(by_nodes=900000, by_placements=1200000, by_arrays=55158,
                                    by_list_members=470000, by_list_chunks=20000,
                                    by_chunk_members=5120000, by_array_members=10000,
                                    by_pages=0, map_updates=12)
        # pass 2's reserve, what pass 1 left (2026-10-02): the log line's
        res["density_plan2"].update(reserve_mb=896)
        res["density_pages"].update(decoded=206, over_budget=3)
        full, brief = perf_status(res)
        self.assertIn("[density: dots, lit 620k px, block 4 px, floor 15 px, reserve 896 MB, pass 2 plan", full)
        # the chunks' members right after the chunks (0.12.268 put them last:
        # the field's `list chunks 78, array members 57k of 354 members`)
        self.assertIn(
            "reads 15.2M, cell dots 2.7M [nodes 900k, placements 1.2M, arrays"
            " 55k, list members 470k, list chunks 20k of 5.1M members, array"
            " members 10k; hash map 12]), 206 pages", full)
        # what pass 2's reserve kept out (2026-10-01): in the log line too
        self.assertIn(", 206 pages, pass 2 over budget: floor probe, thinned, 3 pages left out]", full)
        self.assertEqual(
            brief,
            "4324 ms = 250 load + 3916 draw + 1693 other · density: lit 620k"
            " px, pass 2 plan 3494 ms (probe 637 ms x1, 5 passes, floor 15 px,"
            " 4 threads, nodes 4.0M, reads 15.2M, cell dots 2.7M), 206"
            " pages decoded, pass 2 over budget: floor probe, thinned, 3 pages"
            " left out · bin off(cap@786k), hier 2.0M/1.7M pruned"
            " · cut<7.56um x2 to fit budget, STILL OVER · 3 pages over"
            " budget (not drawn) · labels partial · evict 1200")

        # the synthetic chip at medium, 694 um around (14722, 17090) um: no
        # cell under the cut, the shapes of 1-3 px drawn from 54 pages
        _, brief = perf_status({
            "tiles": 54, "ms": 69, "load_ms": 0, "draw_ms": 63,
            "cut_um": 2.08, "work_bin_items": 43000,
            "density_stack": {"lit": 83810, "top": 0, "lower": 78555,
                              "covered": 24786, "claimed": 83810},
            "density_dots": {"items": 0, "over": 0},
            "density_us": {"plan2_us": 1200},
            "density_plan2": {"probe_us": 0, "probes": 0, "fit_us": 1200,
                              "passes": 1, "regions": 15, "nodes": 3153,
                              "page_nodes": 0, "page_candidates": 54,
                              "threads": 1, "reads": 0, "items": 0},
            "density_pages": {"planned": 54, "in_hand": 18, "decoded": 36}})
        self.assertEqual(
            brief,
            "69 ms = 0 load + 63 draw · density: lit 84k px, pass 2 plan 1 ms"
            " (nodes 3153, reads 0, cell dots 0), 36 pages decoded · bin 43k"
            " items · cut<2.08um")

        # a frame without the density stack, a deck's passes
        _, brief = perf_status({"tiles": 4, "ms": 52, "load_ms": 2,
                                "draw_ms": 50, "deck": {
                                    "passes": 12, "frame_passes": 3,
                                    "passes_skipped": 1, "unique_pages": 40,
                                    "pages_summed": 90, "scene_us": 12000,
                                    "frame_raster_us": 55000,
                                    "composite_us": 3000,
                                    "pass_bytes_max": 5e7,
                                    "summary_passes": 2}})
        self.assertEqual(
            brief,
            "52 ms = 2 load + 50 draw · deck 12 passes, summary 2 passes"
            " (not pickable)")

        # the bar shows the brief line, its tooltip the whole; a state
        # without a brief one (a pan, no layers) shows as given
        shown = {}
        v = SimpleNamespace(
            dbu=0.001,
            vstatus=SimpleNamespace(set_text=lambda t: shown.update(view=t)),
            pstatus=SimpleNamespace(
                set_text=lambda t: shown.update(text=t),
                set_tooltip_text=lambda t: shown.update(tip=t)))
        Viewer._set_status(v, (0, 0, 1000, 500), full, brief)
        self.assertEqual((shown["text"], shown["tip"]), (brief, full))
        Viewer._set_status(v, (0, 0, 1000, 500), "no layers visible")
        self.assertEqual((shown["text"], shown["tip"]),
                         ("no layers visible", "no layers visible"))

        # the first frame after a load: the log line splits the load, the
        # bar says how long it took
        v._load_marks = {"t0": 100.0, "cache": 101.0, "service": 110.0,
                         "open": {"renderd_open_ms": 8800}}
        with mock.patch("floe.gui.time.monotonic", return_value=112.5):
            load, load_brief = Viewer._load_note(v, {})
        self.assertEqual(
            load, "loaded in 12.5 s (cache 1.0 s + service 9.0 s [renderd"
            " open 8.8 s] + first frame 2.50 s) · ")
        self.assertEqual(load_brief, "loaded in 12.5 s · ")
        self.assertEqual(Viewer._load_note(v, {}), ("", ""))

    def test_margin_prefetch_is_a_rust_only_reuse_capability(self):
        """P0 review (2026-09-05): the F2R-17 margin prefetch lives in
        the shared GUI and used to fire for ANY backend - stable
        floe/KLayout would have rendered ~4.8x the pixels of every
        settled view as foreground work (its service also dropped the
        bg flag). The GUI now gates on the worker capability AND on
        --frame-cache (off under --perf-baseline), and _covered() only
        crops an oversize frame while the margin is enabled."""
        from floe import service
        from floe.gui import Viewer

        self.assertTrue(RustRenderWorker.supports_margin_prefetch)
        self.assertFalse(service.RenderWorker.supports_margin_prefetch)

        key = _MARGIN_KEY
        submitted = []
        make = _stub_margin_viewer

        rust = SimpleNamespace(supports_margin_prefetch=True,
                               alive=lambda: True,
                               submit=lambda job: submitted.append(job))
        klayout = SimpleNamespace(supports_abstract=True,
                                  alive=lambda: True,
                                  submit=lambda job: submitted.append(job))

        v = make(klayout, True)
        Viewer._schedule_margin(v)
        self.assertFalse(Viewer._submit_margin(v))
        self.assertEqual(submitted, [], "KLayout must never get a margin")
        self.assertIsNone(v._margin_pending)

        v = make(rust, False)
        Viewer._schedule_margin(v)
        self.assertEqual(submitted, [], "--frame-cache off disables it")

        v = make(rust, True)
        Viewer._schedule_margin(v)
        self.assertEqual(len(submitted), 1)
        job = submitted[0]
        self.assertTrue(job["bg"])
        # ~2x2 viewports: one snapped half-step per side (exact sizes
        # pinned below)
        self.assertGreaterEqual(job["w"], 2 * 858)
        self.assertGreaterEqual(job["h"], 2 * 802)
        # §F2R-21 (user call): the margin carries the labels of its
        # own box so a pan inside it is a labelled crop
        self.assertEqual(job["labels"], v.labels_on)
        self.assertTrue(job["labels"])
        self.assertTrue(job["frames"], "hierarchy outlines are geometry")
        self.assertEqual(v._margin_pending[0], job["gen"])

        # exact fit (user call 2026-09-05): the landed margin covers
        # one snapped 50% arrow step per side to the pixel - the step
        # itself is a crop, one more 16 px period is not
        v = make(rust, True)
        Viewer._submit_margin(v)
        job = submitted[-1]
        v.last_frame = (None, tuple(job["bbox"]), v.spp, key)
        w_px, h_px = v._viewport_size()
        step_x = Viewer._snap_pan_px(v, w_px * 0.5)
        step_y = Viewer._snap_pan_px(v, h_px * 0.5)
        self.assertEqual(job["w"], w_px + 2 + 2 * step_x)
        self.assertEqual(job["h"], h_px + 2 + 2 * step_y)
        cx, cy = v.cx, v.cy
        for sx, sy in ((1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (-1, -1)):
            v.cx = cx + sx * step_x * v.spp
            v.cy = cy + sy * step_y * v.spp
            self.assertTrue(Viewer._covered(v, v.view_bbox(), "live"),
                            "one 50%% step (%d,%d) must be a crop" % (sx, sy))
            v.cx = cx + sx * (step_x + 16) * v.spp
            v.cy = cy + sy * (step_y + 16) * v.spp
            self.assertFalse(Viewer._covered(v, v.view_bbox(), "live"),
                             "a step plus one period must re-render")
        v.cx, v.cy = cx, cy

        # _covered(): an oversize (margin) frame serves a shifted view
        # only while the margin is enabled; the exact frame keeps
        # serving the unchanged view either way
        for worker, frame_cache, crops in ((rust, True, True),
                                           (rust, False, False),
                                           (klayout, True, False)):
            v = make(worker, frame_cache)
            b = v.view_bbox()
            self.assertTrue(Viewer._covered(v, b, "live"))
            vw, vh = b[2] - b[0], b[3] - b[1]
            v.last_frame = (None, (b[0] - vw, b[1] - vh,
                                   b[2] + vw, b[3] + vh), v.spp, key)
            shifted = (b[0] + 0.3 * vw, b[1], b[2] + 0.3 * vw, b[3])
            self.assertEqual(Viewer._covered(v, shifted, "live"), crops)

    def test_margin_prefetch_caps_pixels_for_large_viewports(self):
        """§F2R-20: a margin frame is bounded in pixels so renderd's
        retained set, the publish file and the GUI pixbuf stay small
        on shared hosts. Ordinary windows keep the exact one-step
        margin; a 4K window shrinks both extensions (16 px multiples)
        to fit; a window that fills the cap alone gets no margin. A
        capped margin still counts as "already margined" once landed
        and centered, so it is not topped up after every pan."""
        from floe.gui import MARGIN_MAX_MPIX, Viewer

        submitted = []
        rust = SimpleNamespace(supports_margin_prefetch=True,
                               alive=lambda: True,
                               submit=lambda job: submitted.append(job))
        cap = MARGIN_MAX_MPIX << 20

        v = _stub_margin_viewer(rust, True, viewport=(2560, 1440))
        v._margin_max_px = cap
        self.assertTrue(Viewer._submit_margin(v) is False and submitted)
        job = submitted[-1]
        self.assertEqual((job["w"], job["h"]),
                         (2560 + 2 + 2 * 1280, 1440 + 2 + 2 * 720),
                         "a QHD window keeps the full one-step margin")
        self.assertLessEqual(job["w"] * job["h"], cap)

        v = _stub_margin_viewer(rust, True, viewport=(3840, 2160))
        v._margin_max_px = cap
        Viewer._submit_margin(v)
        job = submitted[-1]
        self.assertLessEqual(job["w"] * job["h"], cap, "capped")
        ex = (job["w"] - 3842) // 2
        ey = (job["h"] - 2162) // 2
        self.assertEqual((job["w"] - 3842) % 32, 0)
        self.assertEqual((job["h"] - 2162) % 32, 0)
        self.assertGreater(ex, 0)
        self.assertGreater(ey, 0)
        self.assertLess(ex, 1920)
        self.assertLess(ey, 1080)
        # tight: one more 16 px period on both axes would break the
        # cap (each axis is floored to the period from one common
        # shrink factor, so a single axis may keep sub-period slack)
        self.assertGreater((job["w"] + 32) * (job["h"] + 32), cap)
        # landed and centered: no top-up; drift past 30% of the
        # (smaller) extension: top-up
        v.last_frame = (None, tuple(job["bbox"]), v.spp, _MARGIN_KEY)
        v._margin_pending = None
        before = len(submitted)
        Viewer._schedule_margin(v)
        self.assertEqual(len(submitted), before, "centered: no top-up")
        v.cx += 0.5 * ex * v.spp
        Viewer._schedule_margin(v)
        self.assertEqual(len(submitted), before + 1, "drifted: top-up")

        v = _stub_margin_viewer(rust, True, viewport=(3840, 2160))
        v._margin_max_px = 3842 * 2162
        before = len(submitted)
        self.assertFalse(Viewer._submit_margin(v))
        self.assertEqual(len(submitted), before,
                         "no room under the cap: no margin at all")

    def test_pan_inside_a_landed_margin_submits_without_debounce(self):
        """§F2R-21 field: with labels on the landed margin is display
        base + renderd reuse only; an arrow pan that stays inside it
        submits immediately (fast path), one that leaves it debounces."""
        from floe.gui import Viewer

        rust = SimpleNamespace(supports_margin_prefetch=True,
                               alive=lambda: True, submit=lambda job: None)
        v = _stub_margin_viewer(rust, True)
        calls = []
        v.redraw = lambda immediate=False: calls.append(immediate)
        b = v.view_bbox()
        vw, vh = b[2] - b[0], b[3] - b[1]
        # a margin 1.5 viewports wide on each side: two snapped 50%
        # steps (2 x 432 px) stay inside, the third (1296 px) leaves
        v._margin_frame = (object(), (b[0] - 1.5 * vw, b[1] - 1.5 * vh,
                                      b[2] + 1.5 * vw, b[3] + 1.5 * vh),
                           v.spp, _MARGIN_KEY)
        Viewer._pan_view(v, "Right")
        Viewer._pan_view(v, "Right")
        Viewer._pan_view(v, "Right")
        self.assertEqual(calls, [True, True, False])
        v._margin_frame = None
        Viewer._pan_view(v, "Left")
        self.assertEqual(calls[-1], False)
        # a margin of another render state or scale is no base
        v._margin_frame = (object(), (b[0] - vw, b[1] - vh,
                                      b[2] + vw, b[3] + vh),
                           v.spp * 2, _MARGIN_KEY)
        self.assertIsNone(Viewer._margin_base(v))

    def test_margin_geometry_fills_the_strip_a_pan_uncovers(self):
        """§F2R-21 field (2026-09-05): a pan used to show the incoming
        strip BLACK until the fast-path frame landed. The display now
        blits the landed margin's geometry first and the labelled frame
        over its overlap, so no black strip appears."""
        try:
            from floe import gui
            gui.import_gtk()
            GdkPixbuf = gui.GdkPixbuf
        except Exception as exc:  # pragma: no cover - headless hosts
            self.skipTest("GTK unavailable: %s" % exc)
        from floe.gui import Viewer

        def solid(size, rgb):
            pix = GdkPixbuf.Pixbuf.new(GdkPixbuf.Colorspace.RGB, False, 8,
                                       size, size)
            pix.fill((rgb[0] << 24) | (rgb[1] << 16) | (rgb[2] << 8) | 0xff)
            return pix

        shown = []
        v = _stub_margin_viewer(
            SimpleNamespace(supports_margin_prefetch=True), True,
            viewport=(64, 64))
        v.spp = 1.0
        v.visible = [(1, 0)]
        v._structure_visible = lambda: True
        v._frame_anchor = None
        v._draw_overlays = lambda disp, obox, ospp: None
        v._update_labels = lambda obox, ospp, disp: None
        v._update_note_labels = lambda obox, ospp: None
        v._update_minimap = lambda bbox: None
        v.dump = False
        v.image = SimpleNamespace(set_from_pixbuf=shown.append)
        # last (labelled) frame: green, world x 32..96; the landed
        # margin: red, world x 0..128; the view pans 16 px right of
        # the last frame, so its right 16 columns are margin-only
        v.last_frame = (solid(64, (0, 255, 0)), (32.0, 32.0, 96.0, 96.0),
                        1.0, _MARGIN_KEY)
        v._margin_frame = (solid(128, (255, 0, 0)),
                           (0.0, 0.0, 128.0, 128.0), 1.0, _MARGIN_KEY)
        v.cx, v.cy = 80.0, 64.0     # view x 48..112, y 32..96
        Viewer._display(v)
        disp = shown[-1]
        pixels, stride = disp.get_pixels(), disp.get_rowstride()

        def rgb(x, y):
            i = y * stride + x * 3
            return tuple(pixels[i:i + 3])

        self.assertEqual(rgb(0, 32), (0, 255, 0), "overlap: labelled frame")
        self.assertEqual(rgb(47, 32), (0, 255, 0))
        self.assertEqual(rgb(48, 32), (255, 0, 0), "strip: margin geometry")
        self.assertEqual(rgb(63, 32), (255, 0, 0))
        # without the margin the strip would be black
        v._margin_frame = None
        Viewer._display(v)
        self.assertEqual(rgb(63, 32), (255, 0, 0))  # previous disp object
        disp = shown[-1]
        pixels, stride = disp.get_pixels(), disp.get_rowstride()
        self.assertEqual(rgb(63, 32), (0, 0, 0))

    def test_cell_tree_first_expand_stays_open_and_asks_once(self):
        """Field 2026-09-29: the first expand of a tree row closed at
        once (later ones worked) - the placeholder child was removed
        before the children arrived, and GTK collapses a row whose
        last child goes. The children go in first; the row stays open,
        the placeholder is gone, and the row asked for them once."""
        try:
            from floe import gui
            gui.import_gtk()
        except Exception as exc:  # pragma: no cover - headless hosts
            self.skipTest("GTK unavailable: %s" % exc)
        import types
        from floe.gui import Viewer
        v = Viewer.__new__(Viewer)
        v.cache = types.SimpleNamespace(catalog=None)
        v.dbu = 0.001
        v._cellwin = None
        v._cell_seq = 0
        v._cell_pending = {}
        v._cell_sources = v._cell_sel = v._cell_hl = None
        v._cell_hl_on = True
        v._cell_hl_key = None
        v._cell_find_seq = v._cell_insts_seq = v._cell_bbox_seq = None
        v._cell_search_timer = None
        v._cell_mode = "tree"
        v._cell_nohier = set()
        sent = []
        v.worker = types.SimpleNamespace(alive=lambda: True,
                                         submit=sent.append)
        # the panel's box must stay referenced: an unparented container
        # is destroyed with its widgets when collected, which unsets the
        # tree view's model
        panel = v._build_cell_panel()
        self.addCleanup(panel.destroy)
        w = v._cellwin
        store = w._store
        v._cell_tree_load_roots()
        v._on_cell_result({
            "kind": "cell_sources", "seq": sent[-1]["seq"], "found": True,
            "sources": [{"src": 0, "placements": 1, "path": "/x/.a.ice"}]})
        v._on_cell_result({
            "kind": "cells", "seq": sent[-1]["seq"], "found": True,
            "src": 0, "cell": 6, "name": "TOP", "insts": 1, "height": 2,
            "unit": 1000.0, "bbox": [0, 0, 1, 1], "total": 2,
            "children": [
                {"cell": 4, "members": 9, "leaf": True, "name": "LEAF"},
                {"cell": 5, "members": 2, "leaf": False, "name": "MID"}]})
        root = store.get_iter_first()
        self.assertTrue(w._tree.row_expanded(store.get_path(root)))
        mid = store.iter_nth_child(root, 1)
        self.assertEqual(store[store.iter_children(mid)][5], "placeholder")
        asked = len(sent)
        w._tree.expand_row(store.get_path(mid), False)
        self.assertEqual(len(sent), asked + 1)
        self.assertEqual((sent[-1]["kind"], sent[-1]["cell"]), ("cells", 5))
        v._on_cell_result({
            "kind": "cells", "seq": sent[-1]["seq"], "found": True,
            "src": 0, "cell": 5, "name": "MID", "insts": 2, "height": 1,
            "unit": 1000.0, "bbox": [0, 0, 1, 1], "total": 1,
            "children": [
                {"cell": 3, "members": 4, "leaf": True, "name": "INV"}]})
        self.assertTrue(w._tree.row_expanded(store.get_path(mid)),
                        "the first expand closed")
        self.assertEqual([store[store.iter_nth_child(mid, i)][0]
                          for i in range(store.iter_n_children(mid))],
                         ["INV"])
        # a second open does not ask again
        w._tree.collapse_row(store.get_path(mid))
        w._tree.expand_row(store.get_path(mid), False)
        self.assertEqual(len(sent), asked + 1)

    def test_cell_page_fits_the_left_pane_at_its_start_width(self):
        """Field 2026-09-29: the left pane opened with the `cells` tab
        off screen and the DRC tab half hidden - the page's one-row
        button bar was 276 px against the pane's 196 px, and GtkPaned
        shrinks a too-wide first child by clipping its LEFT side. The
        page's minimum width must stay well under the pane's start
        width; the buttons wrap instead."""
        try:
            from floe import gui
            gui.import_gtk()
        except Exception as exc:  # pragma: no cover - headless hosts
            self.skipTest("GTK unavailable: %s" % exc)
        import types
        from floe.gui import LEFT_PANE_PX, MINIMAP_PX, Viewer
        v = Viewer.__new__(Viewer)
        v.cache = None
        v._cellwin = None
        v._cell_seq = 0
        v._cell_pending = {}
        v._cell_sources = v._cell_sel = v._cell_hl = None
        v._cell_hl_on = True
        v._cell_hl_key = None
        v._cell_find_seq = v._cell_insts_seq = v._cell_bbox_seq = None
        v._cell_search_timer = None
        v._cell_mode = "tree"
        v._cell_nohier = set()
        v._view_root = None
        v.worker = types.SimpleNamespace(alive=lambda: False)
        panel = v._build_cell_panel()
        window = gui.Gtk.OffscreenWindow()
        window.add(panel)
        window.show_all()
        self.addCleanup(window.destroy)
        minimum, _natural = panel.get_preferred_width()
        # the old floor (the minimap's) and the new start width alike
        self.assertLessEqual(minimum, MINIMAP_PX + 16 - 40, minimum)
        self.assertLess(minimum, LEFT_PANE_PX)
        # the controls sit in a wrapping row, the build button apart
        row = v._cellwin._zoom.get_parent().get_parent()
        self.assertIsInstance(row, gui.Gtk.FlowBox)
        self.assertEqual(len(row.get_children()), 4)
        self.assertFalse(v._cellwin._build.get_visible())
        # user call 2026-09-29: thirty nested levels expanded (570 px of
        # indentation) must not widen the page - the tree scrolls
        # sideways instead (hscroll AUTOMATIC), so the pane never clips
        # the page's left side with no way back
        store = v._cellwin._store
        it = None
        for depth in range(30):
            it = store.append(it, ["CELL_%02d" % depth, "", 0, depth,
                                   True, "cell"])
        v._cellwin._tree.expand_all()
        while gui.Gtk.events_pending():
            gui.Gtk.main_iteration()
        deep, _natural = panel.get_preferred_width()
        self.assertLessEqual(deep, minimum + 8, (deep, minimum))
        scroller = v._cellwin._tree.get_parent()
        self.assertEqual(scroller.get_policy()[0],
                         gui.Gtk.PolicyType.AUTOMATIC)

    def test_minimap_die_outline_keeps_a_margin_from_the_edge_and_the_view_box(self):
        """User call 2026-09-29: the die outline sat on the minimap's
        first and last pixel on its long axis (hidden at the widget
        edge) and the fit view's box, clipped to the die, lay on top of
        it. The die now fits inside a MINIMAP_PAD border, its outline
        shows on all four sides, and the fit view's box runs outside
        it; a click in the border centres on the nearest die edge."""
        try:
            from floe import gui
            gui.import_gtk()
        except Exception as exc:  # pragma: no cover - headless hosts
            self.skipTest("GTK unavailable: %s" % exc)
        import types
        from floe.gui import (MINIMAP_EDGE, MINIMAP_PAD, MINIMAP_PX,
                              MINIMAP_VIEW, Viewer)

        def rgb(color):
            return ((color >> 24) & 255, (color >> 16) & 255,
                    (color >> 8) & 255)

        for die in ([0, 0, 20000, 8000], [0, 0, 6000, 18000]):
            v = Viewer.__new__(Viewer)
            v.meta = {"bbox": die}
            v._view_root = None
            v._minimap_bases = {}
            v._frontier_depths = []
            v.depth_value = 999
            v._minimap_image = gui.Gtk.Image()
            v.cx, v.cy = (die[0] + die[2]) / 2.0, (die[1] + die[3]) / 2.0
            # the fit view: 5 % over the die on its long axis, far over
            # it on the short one (a square-ish canvas)
            span = max(die[2] - die[0], die[3] - die[1]) * 1.05
            fit = (v.cx - span / 2, v.cy - span / 2,
                   v.cx + span / 2, v.cy + span / 2)
            v._update_minimap(fit)
            pix = v._minimap_image.get_pixbuf()
            data, stride, n = (pix.get_pixels(), pix.get_rowstride(),
                               pix.get_n_channels())

            def at(x, y):
                o = int(y) * stride + int(x) * n
                return tuple(data[o:o + 3])

            _scale, x0, y0, mw, mh = v._minimap_geom()
            x1, y1 = x0 + mw - 1, y0 + mh - 1
            self.assertGreaterEqual(min(x0, y0), MINIMAP_PAD, die)
            self.assertLessEqual(max(x1, y1), MINIMAP_PX - 1 - MINIMAP_PAD,
                                 die)
            mid_x, mid_y = (x0 + x1) // 2, (y0 + y1) // 2
            for x, y in ((x0, mid_y), (x1, mid_y), (mid_x, y0),
                         (mid_x, y1)):
                self.assertEqual(at(x, y), rgb(MINIMAP_EDGE), (die, x, y))
            # the view box runs in the border, visible and off the die,
            # on all four sides
            view = rgb(MINIMAP_VIEW)
            self.assertIn(view, [at(x, mid_y) for x in range(0, x0)], die)
            self.assertIn(view, [at(x, mid_y)
                                 for x in range(x1 + 1, MINIMAP_PX)], die)
            self.assertIn(view, [at(mid_x, y) for y in range(0, y0)], die)
            self.assertIn(view, [at(mid_x, y)
                                 for y in range(y1 + 1, MINIMAP_PX)], die)
            # a click in the border lands on the nearest die edge; one
            # past it is off the map
            left = v._minimap_world_point(x0 - MINIMAP_PAD, mid_y)
            self.assertAlmostEqual(left[0], die[0])
            self.assertIsNone(v._minimap_world_point(x0 - MINIMAP_PAD - 1,
                                                     mid_y))

    def test_under_a_view_root_the_depth_counts_to_the_roots_height(self):
        """User 2026-09-30: under a view root the depth counts from the root
        (the planner always did) but the viewer kept the file top's height -
        the label read d/16 and `<` walked through levels that changed
        nothing. Now the deepest level is the root's height: the label
        shows d/h (`*` at or past h), the steps clamp to [0, h], and the
        depth itself is kept so `top` gives the view back."""
        import types
        from floe.gui import Viewer
        v = Viewer.__new__(Viewer)
        v.meta = {}
        v.abstract = False
        v.max_depth = 16
        v._view_root = None
        v._ddlg = None
        v._on_depth = lambda: None
        v.depth_value = 3
        self.assertEqual(Viewer._depth_label(v), "depth: 3/16")
        v._view_root = {"cell": 5, "name": "BLK", "bbox": [0, 0, 5000, 4000], "height": 2}
        self.assertEqual(Viewer._depth_label(v), "depth: */2", "3 >= 2 levels: the whole root")
        Viewer._depth_step(v, -1)
        self.assertEqual((v.depth_value, Viewer._depth_label(v)), (1, "depth: 1/2"))
        Viewer._depth_step(v, +1)
        Viewer._depth_step(v, +1)
        self.assertEqual(v.depth_value, 2, "clamped to the root's height")
        v.depth_value = 999
        Viewer._depth_step(v, -1)
        self.assertEqual(v.depth_value, 1, "full steps down from the root's deepest level")
        v.depth_value = 7
        v._view_root = None
        self.assertEqual(Viewer._depth_label(v), "depth: 7/16", "back at the top, the depth kept")
        titles, labels = [], []
        v._minimap_bases, v.last_frame, v._margin_frame, v._frame_anchor = {}, None, None, None
        v._clear_pending = lambda: None
        v._job_keys = {}
        v._cell_hl = v._cell_hl_key = None
        v._cellwin = None
        v._title_base = "floe - x"
        v.window = types.SimpleNamespace(set_title=titles.append)
        v.dstatus = types.SimpleNamespace(set_text=labels.append)
        v.fit = lambda: None
        v._cell_hl_query = lambda: None
        v._view_root = {"cell": 5, "name": "BLK", "bbox": [0, 0, 5000, 4000], "height": 2}
        Viewer._root_changed(v)
        self.assertEqual(labels[-1], "depth: */2", "a root change refreshes the label")

    def test_view_root_moves_the_die_the_render_state_and_the_queries(self):
        """SPEC-VIEWER §8c: the selected cell as the view root - the die
        (fit, clamp, minimap) becomes its bbox, the render state and
        every render/clip/cell query carry its index, the stale frame
        and the margin go, and `top` returns everything."""
        import types
        from floe.gui import Viewer
        v = Viewer.__new__(Viewer)
        v.cache = types.SimpleNamespace(is_jobdeck=False)
        v.meta = {"bbox": [0, 0, 20000, 12000], "dbu": 0.001}
        v.dbu = 0.001
        v.visible = {(1, 0)}
        v._depth_key = lambda: 999
        v._effective_cut_px = lambda: 3.0
        v.lod_on = v.frames_on = v.labels_on = False
        v._color_epoch = 0
        v._effective_thin = lambda: "keep"
        v._view_root = None
        v._title_base = "floe - x"
        v.window = types.SimpleNamespace(set_title=lambda t: titles.append(t))
        titles = []
        v._minimap_bases = {"stale": 1}
        v.last_frame = ("frame",)
        v._margin_frame = ("margin",)
        v._frame_anchor = (1, 2)
        v._job_keys = {3: "k"}
        v._clear_pending = lambda: None
        v._cell_hl = {"boxes": []}
        v._cell_hl_key = "k"
        v._cell_hl_on = True
        # the panel: only what the root path touches
        v._cellwin = types.SimpleNamespace(
            _top=types.SimpleNamespace(set_sensitive=lambda on: None),
            _info=types.SimpleNamespace(set_text=lambda t: None))
        v._cell_sel = (0, 5, "BLK")
        v._frontier_depths = [[[0, 0, 1, 1, 0]]]
        v.depth_value = 0
        fits, status, sent = [], [], []
        v.fit = lambda: fits.append(v._die_bbox())
        v._set_live_status = status.append
        v.view_bbox = lambda: (0.0, 0.0, 100.0, 100.0)
        v._cell_seq = 0
        v._cell_pending = {}
        v.worker = types.SimpleNamespace(alive=lambda: True,
                                         submit=sent.append)
        v._cell_insts_seq = None
        plain_key = v._render_key("live")
        self.assertIsNone(v._root_ci())
        self.assertEqual(v._die_bbox(), [0, 0, 20000, 12000])
        self.assertEqual(v._minimap_frontier_depth(), 0)
        # the `root` button asks for the selected cell; the answer applies the root
        v._cell_set_root()
        self.assertEqual((sent[-1]["kind"], sent[-1]["cell"]), ("cells", 5))
        self.assertEqual(v._cell_pending[sent[-1]["seq"]][0], "root_set")
        v._on_cell_result({
            "kind": "cells", "seq": sent[-1]["seq"], "found": True,
            "src": 0, "cell": 5, "name": "BLK", "insts": 3, "height": 2,
            "unit": 1000.0, "bbox": [0, 0, 5000, 4000], "total": 0,
            "children": []})
        self.assertEqual(v._root_ci(), 5)
        self.assertEqual(v._die_bbox(), [0.0, 0.0, 5000.0, 4000.0])
        self.assertEqual(fits, [[0.0, 0.0, 5000.0, 4000.0]])
        self.assertIsNone(v._minimap_frontier_depth())
        self.assertEqual(v._minimap_bases, {})
        self.assertIsNone(v.last_frame)
        self.assertIsNone(v._margin_frame)
        self.assertEqual(v._job_keys, {})
        self.assertNotEqual(v._render_key("live"), plain_key)
        self.assertEqual(titles[-1], "floe - x · root BLK")
        self.assertIn("view root: BLK", status[-1])
        # the highlight was re-asked under the root
        self.assertEqual(sent[-1]["kind"], "cell_insts")
        self.assertEqual(sent[-1]["root"], 5)
        # the same root again is a no-op; a shapeless cell is refused
        n = len(sent)
        v._cell_set_root()
        self.assertEqual(len(sent), n)
        v._cell_sel = (0, 7, "EMPTY")
        v._cell_set_root()
        v._on_cell_result({
            "kind": "cells", "seq": sent[-1]["seq"], "found": True,
            "src": 0, "cell": 7, "name": "EMPTY", "insts": 3, "height": 0,
            "unit": 1000.0, "bbox": None, "total": 0, "children": []})
        self.assertEqual(v._root_ci(), 5)
        self.assertIn("no shapes", status[-1])
        # back to the top
        v._cell_root_top()
        self.assertIsNone(v._root_ci())
        self.assertEqual(v._die_bbox(), [0, 0, 20000, 12000])
        self.assertEqual(v._render_key("live"), plain_key)
        self.assertEqual(titles[-1], "floe - x")
        self.assertEqual(len(fits), 2)
        # a jobdeck has no view root
        v.cache = types.SimpleNamespace(is_jobdeck=True)
        v._cell_sel = (0, 5, "BLK")
        n = len(sent)
        v._cell_set_root()
        self.assertEqual(len(sent), n)
        self.assertIn("jobdeck", status[-1])

    def test_menus_and_dialogs_hand_the_keys_back_to_the_canvas(self):
        """Field 2026-09-05: after using a menu, g and the other key
        commands stayed dead until a canvas click. _focus_view puts the
        menubar out of its active state and focuses the scroller (a
        non-entry focus widget the window key handler never yields
        to); a canvas click does the same synchronously."""
        from floe.gui import Viewer

        events = []
        v = Viewer.__new__(Viewer)
        v._menubar = SimpleNamespace(deactivate=lambda: events.append("mb"))
        v.scroller = SimpleNamespace(get_can_focus=lambda: True,
                                     grab_focus=lambda: events.append("focus"))
        v.window = SimpleNamespace(set_focus=lambda w: events.append(("set", w)))
        Viewer._focus_view(v)
        self.assertEqual(events, ["mb", "focus"])
        # a scroller that cannot take focus falls back to clearing the
        # window focus (still no entry left holding the keys)
        events.clear()
        v.scroller = SimpleNamespace(get_can_focus=lambda: False)
        Viewer._focus_view(v)
        self.assertEqual(events, ["mb", ("set", None)])

    def test_drc_grid_cells_use_one_formatter(self):
        """Field 2026-09-08: a waived error number turned green -> cyan
        once the current-cell mark moved elsewhere, because the page
        fill and the mark repaint formatted cells separately and the
        mark path kept the old cyan. One formatter now serves both."""
        import inspect
        from floe import gui
        from floe.gui import Viewer

        v = Viewer.__new__(Viewer)
        v._drc_waived = lambda db, ci, ei: ei == 1
        db = object()
        waived = Viewer._drc_cell_markup(v, db, 0, 1, False, frozenset())
        plain = Viewer._drc_cell_markup(v, db, 0, 2, False, frozenset())
        self.assertIn("#00e676", waived)
        self.assertIn("#ff5252", plain)
        self.assertIn(">2<", waived)
        noted = Viewer._drc_cell_markup(v, db, 0, 1, True, frozenset())
        self.assertIn(">*2<", noted)
        chosen = Viewer._drc_cell_markup(v, db, 0, 1, False, frozenset([1]))
        # a selected cell is gold like the canvas marker; its number
        # darkens to stay readable (user call 2026-09-08)
        self.assertIn("background='#ffd700'", chosen)
        self.assertIn("foreground='#006b3c'", chosen)
        self.assertNotIn("#00e676", chosen)
        chosen_red = Viewer._drc_cell_markup(v, db, 0, 2, False,
                                             frozenset([2]))
        self.assertIn("foreground='#b00020'", chosen_red)
        current = Viewer._drc_cell_markup(v, db, 0, 1, False, frozenset(),
                                          current=True)
        self.assertIn("background='#3465a4'", current)
        self.assertNotIn("#00ffff", inspect.getsource(gui),
                         "no second waived colour anywhere in the GUI")

    def test_panel_css_parses_and_styles_the_drc_pane(self):
        """User call 2026-09-08: the DRC pane is black with white text
        like the layer pane. The pane CSS is a module constant so a
        syntax slip cannot hide until startup: GTK must parse it, and
        the DRC selectors must be present."""
        from floe import gui

        # only the two lists are dark (user call): no pane-wide rules
        # black again (user call 2026-09-10; the deep grey of
        # 2026-09-08 is withdrawn)
        self.assertIn(b".floe-drc-list, .floe-drc-list.view "
                      b"{ background-color: #000000", gui.PANEL_CSS)
        self.assertIn(b".floe-drc-list:selected", gui.PANEL_CSS)
        # and a visible rule between the error detail and the buttons
        self.assertIn(b".floe-drc-rule { background-color: #808080; "
                      b"min-height: 1px;", gui.PANEL_CSS)
        import inspect
        src = inspect.getsource(gui.Viewer._build_drc_panel)
        self.assertIn('add_class("floe-drc-rule")', src)
        self.assertLess(src.index('add_class("floe-drc-rule")'),
                        src.index("nav = Gtk.FlowBox()"),
                        "the rule sits between the detail and the buttons")
        self.assertNotIn(b".floe-drc {", gui.PANEL_CSS)
        self.assertNotIn(b".floe-drc textview", gui.PANEL_CSS)
        self.assertNotIn(b".floe-drc entry", gui.PANEL_CSS)
        self.assertIn(b".floe-layers-frame scrollbar", gui.PANEL_CSS)
        try:
            gui.import_gtk()
            Gtk = gui.Gtk
        except Exception as exc:  # pragma: no cover - headless hosts
            self.skipTest("GTK unavailable: %s" % exc)
        provider = Gtk.CssProvider()
        provider.load_from_data(gui.PANEL_CSS)   # raises on a syntax error

    def test_drc_error_viewing_releases_in_view_keeps_zoom_and_click_steps(self):
        """User call 2026-09-08: (1) a framing jump switches the in-view
        filter off first, else the list collapses to that one error;
        (2) a zoom made while viewing sticks across n/p and list
        clicks (recenter only) until a fresh double-click; (3) a plain
        click on another number while viewing jumps like n/p."""
        from floe.gui import DRC_VIEW_FRACTION, Viewer

        class Err:
            kind, num, pts = "p", 7, [(0, 0), (1000, 0), (1000, 500), (0, 500)]

            def bbox(self):
                return (0.0, 0.0, 1.0, 0.5)

            def center(self):
                return (0.5, 0.25)

        errors = [Err(), Err(), Err()]
        db = SimpleNamespace(checks=[SimpleNamespace(name="R1", errors=errors)])
        v = Viewer.__new__(Viewer)
        v._drc, v._drc_cum, v._drc_pos = db, [0], -1
        v._drc_focus = None
        v.drc_mark = None
        v._drc_jump_spp = None
        v._drc_zoom_lock = False
        v.dbu, v.spp = 0.001, 1.0
        v._viewport_size = lambda: (800, 400)
        v.rulers, v._drc_ruler = [], []
        v._drc_cd_ruler = lambda e: []
        v._drc_waived = lambda db_, ci, ei: False
        v._drc_isolate_layers = lambda name: None
        v._drc_show_detail = lambda ci, ei: None
        v._display = lambda: None
        v._set_live_status = lambda msg: None
        v._drc_grid_ci, v._drc_grid_base, v._drc_page = 0, None, 0
        hl_calls, goto_calls, cell_calls = [], [], []
        v._drcwin = SimpleNamespace(
            _hl=SimpleNamespace(set_active=lambda on: hl_calls.append(on)))
        v._drc_goto_cell = lambda ci, ei: cell_calls.append((ci, ei))

        def goto(x, y, window_um=None):
            goto_calls.append((x, y, window_um))
            if window_um:
                v.spp = (window_um / v.dbu) / 800.0
        v.goto = goto

        # (1) in view on + double-click jump: the filter releases and
        # the mark returns to the jumped cell before the view moves
        v._drc_hl = True
        Viewer._drc_jump(v, 0, 0, isolate=True)
        self.assertEqual(hl_calls, [False])
        self.assertEqual(cell_calls, [(0, 0)])
        default_win = max(1.0 / DRC_VIEW_FRACTION,
                          0.5 / DRC_VIEW_FRACTION * 2.0)
        self.assertAlmostEqual(goto_calls[-1][2], default_win)
        spp_default = v.spp
        v._drc_hl = False

        # (2) n/p without a zoom change frames at the default again;
        # after the user zooms in 2x, n/p only recenters
        Viewer._drc_jump(v, 0, 1)
        self.assertAlmostEqual(goto_calls[-1][2], default_win)
        v.spp = spp_default * 0.5          # user wheel-zoomed in
        Viewer._drc_jump(v, 0, 2)
        self.assertIsNone(goto_calls[-1][2], "recenter only")
        self.assertAlmostEqual(v.spp, spp_default * 0.5)
        Viewer._drc_jump(v, 0, 0)
        self.assertIsNone(goto_calls[-1][2], "the zoom keeps sticking")
        # a fresh double-click frames at the default fraction again
        Viewer._drc_jump(v, 0, 1, isolate=True)
        self.assertAlmostEqual(goto_calls[-1][2], default_win)
        # Esc (mark cleared) ends the session: the next jump reframes
        v.spp = spp_default * 0.5
        v.drc_mark = None
        Viewer._drc_jump(v, 0, 2)
        self.assertAlmostEqual(goto_calls[-1][2], default_win)

        # (3) a plain click while viewing jumps like n/p; with no
        # error being viewed it only marks + details
        try:
            from floe import gui
            gui.import_gtk()
            Gdk = gui.Gdk
        except Exception as exc:  # pragma: no cover - headless hosts
            self.skipTest("GTK unavailable: %s" % exc)
        jumps = []
        v._drc_jump = lambda ci, ei, isolate=False: jumps.append((ci, ei, isolate))
        v._drc_cell_mark = lambda row, j: None
        v._drc_grid_rows, v._drc_gridw, v._drc_grid_map = 1, 3, [0, 1, 2]
        col = object()
        path = SimpleNamespace(get_indices=lambda: [0])
        tree = SimpleNamespace(get_path_at_pos=lambda x, y: (path, col, 0, 0),
                               get_columns=lambda: [None, col, None])
        ev = SimpleNamespace(button=1, type=Gdk.EventType.BUTTON_PRESS,
                             state=0, x=0, y=0)
        v.drc_mark = {"kind": "p"}
        Viewer._on_drc_grid_click(v, tree, ev)
        self.assertEqual(jumps, [(0, 1, False)])
        v.drc_mark = None
        Viewer._on_drc_grid_click(v, tree, ev)
        self.assertEqual(jumps, [(0, 1, False)], "no viewing: no jump")
        self.assertEqual(v._drc_focus[:2], (0, 1))

    def test_poll_catches_a_resize_the_signal_path_missed(self):
        """Field 2026-09-08: a title-bar double-click zoom left the view
        unrefreshed. The poll compares the live canvas allocation with
        the last one seen and runs the allocation handler on a change."""
        from floe.gui import Viewer

        v = Viewer.__new__(Viewer)
        calls = []
        v._alloc_size = (800, 600)
        v._on_allocate = lambda w, alloc, source="signal": calls.append(
            (alloc.width, alloc.height, source))
        v.scroller = SimpleNamespace(
            get_allocation=lambda: SimpleNamespace(width=800, height=600))
        self.assertFalse(Viewer._sync_allocation(v))
        v.scroller = SimpleNamespace(
            get_allocation=lambda: SimpleNamespace(width=1600, height=1000))
        self.assertTrue(Viewer._sync_allocation(v))
        self.assertEqual(calls, [(1600, 1000, "poll")])
        v.scroller = SimpleNamespace(
            get_allocation=lambda: SimpleNamespace(width=1, height=1))
        self.assertFalse(Viewer._sync_allocation(v), "unrealized: ignored")
        # the handler itself defers the redraw out of GTK's layout
        # pass (Linux: a redraw inside size-allocate did not land
        # until the next click); a repeat of the same size does
        # nothing, a superseded deferred size does nothing
        events = []
        v._alloc_size = (800, 600)
        v._did_fit = True
        v.redraw = lambda immediate=False: events.append("redraw")
        v._schedule_repaint = lambda: events.append("repaint")
        v._defer_allocation = lambda size: events.append(("defer", size))
        Viewer._on_allocate(v, None, SimpleNamespace(width=1600, height=1000))
        Viewer._on_allocate(v, None, SimpleNamespace(width=1600, height=1000))
        self.assertEqual(events, [("defer", (1600, 1000))])
        self.assertFalse(Viewer._after_allocate(v, (1600, 1000)))
        self.assertEqual(events[1:], ["redraw", "repaint"])
        self.assertFalse(Viewer._after_allocate(v, (800, 600)))
        self.assertEqual(events[1:], ["redraw", "repaint"], "superseded")

    def test_cancel_sends_the_frontier_and_a_cancelled_render_is_told(self):
        """The viewer's Esc (2026-09-30): cancel(before_gen) writes
        `cancel before_gen=N`; the daemon's `cancelled gen=N phase=...` for a
        render reaches the GUI as a result and drops the job, its
        `cancelled before_gen=F` ack (no generation) says nothing."""
        with tempfile.TemporaryDirectory() as directory:
            binary = os.path.join(directory, "floe-renderd")
            with open(binary, "w", encoding="ascii") as script:
                script.write("#!/bin/sh\n")
            os.chmod(binary, 0o755)
            with mock.patch.dict(os.environ, {"FLOE_RENDERD_BIN": binary}, clear=False):
                worker = RustRenderWorker(FakeCache(directory))
            sent = []
            worker._send = sent.append
            worker.alive = lambda: True
            worker.cancel(7)
            self.assertEqual(sent, ["cancel before_gen=7"])
            with worker._jobs_lock:
                worker._jobs[6] = {"job": {}}
            worker._handle_line("cancelled", {"gen": "6", "phase": "render"}, "")
            self.assertEqual(worker.res.get_nowait(), {"kind": "cancelled", "gen": 6, "phase": "render"})
            self.assertNotIn(6, worker._jobs)
            worker._handle_line("cancelled", {"before_gen": "7"}, "")
            self.assertTrue(worker.res.empty(), "the ack of a cancel is not a result")

    def test_parses_wire_fields(self):
        kind, fields = _parse_wire_line(
            "frame gen=7 png=/tmp/f.png partial=1 deferred=9")
        self.assertEqual(kind, "frame")
        self.assertEqual(fields["gen"], "7")
        self.assertEqual(fields["deferred"], "9")

    def test_ready_rejects_stale_renderd_before_open(self):
        with tempfile.TemporaryDirectory() as directory:
            binary = os.path.join(directory, "floe-renderd")
            with open(binary, "w", encoding="ascii") as script:
                script.write("#!/bin/sh\n")
            os.chmod(binary, 0o755)
            with mock.patch.dict(os.environ, {
                "FLOE_RENDERD_BIN": binary,
            }, clear=False):
                stale = RustRenderWorker(FakeCache(directory))
                current = RustRenderWorker(FakeCache(directory))

            stale._handle_line("ready", {"version": "0.1.0"}, "")
            self.assertFalse(stale._ready)
            self.assertIsNone(stale._renderd_version)
            self.assertIn(
                "expected %s, got 0.1.0" % RENDERD_VERSION,
                stale._startup_error)

            current._handle_line(
                "ready", {"version": RENDERD_VERSION}, "")
            self.assertTrue(current._ready)
            self.assertEqual(current._renderd_version, RENDERD_VERSION)
            self.assertIsNone(current._startup_error)
            # a bare pre-stamp ready line still yields a usable build
            self.assertEqual(current.renderd_build(), RENDERD_VERSION)

    def test_ready_build_stamp_reaches_about(self):
        with tempfile.TemporaryDirectory() as directory:
            binary = os.path.join(directory, "floe-renderd")
            with open(binary, "w", encoding="ascii") as script:
                script.write("#!/bin/sh\n")
            os.chmod(binary, 0o755)
            with mock.patch.dict(os.environ, {
                "FLOE_RENDERD_BIN": binary,
            }, clear=False):
                stamped = RustRenderWorker(FakeCache(directory))
                unstamped = RustRenderWorker(FakeCache(directory))
            stamped._handle_line("ready", {
                "version": RENDERD_VERSION, "git": "abc123+",
                "flavor": "gnu"}, "")
            self.assertEqual(stamped.renderd_build(),
                             "%s abc123+ (gnu)" % RENDERD_VERSION)
            # an unknown git hash is noise, not identity - omitted
            unstamped._handle_line("ready", {
                "version": RENDERD_VERSION, "git": "unknown",
                "flavor": "native"}, "")
            self.assertEqual(unstamped.renderd_build(),
                             "%s (native)" % RENDERD_VERSION)
            # opened carries the GUI depth cap; a pre-0.12.16 renderd
            # omits it and the display keeps its "?" fallback
            stamped._handle_line("opened", {"max_depth": "7"}, "")
            self.assertEqual(stamped._max_depth, 7)
            unstamped._handle_line("opened", {}, "")
            self.assertIsNone(unstamped._max_depth)

    def test_about_component_versions(self):
        from floe import gui
        with tempfile.TemporaryDirectory() as directory:
            index = os.path.join(directory, "floe-index")
            with open(index, "w", encoding="ascii") as script:
                script.write("#!/bin/sh\n"
                             "echo 'floe-index 9.9.9 abc (native)'\n")
            os.chmod(index, 0o755)
            renderd = os.path.join(directory, "floe-renderd")
            with open(renderd, "w", encoding="ascii") as script:
                # pre-0.12.13 shape: no --version, greets and exits
                script.write("#!/bin/sh\n"
                             "echo 'ready version=0.12.11'\n")
            os.chmod(renderd, 0o755)
            silent = os.path.join(directory, "silent")
            with open(silent, "w", encoding="ascii") as script:
                script.write("#!/bin/sh\nexit 1\n")
            os.chmod(silent, 0o755)
            # macOS scans a freshly written executable on its first
            # launch (seconds, more on a loaded host): warm both stubs
            # once so the probe under test measures the probe
            import subprocess
            for stub in (index, renderd):
                subprocess.run([stub], stdin=subprocess.DEVNULL,
                               capture_output=True, timeout=120)
            env = {"FLOE_INDEX_BIN": index, "FLOE_RENDERD_BIN": renderd}
            with mock.patch.dict(os.environ, env, clear=False):
                self.assertEqual(gui.component_versions(None), [
                    "floe-index 9.9.9 abc (native)",
                    "floe-renderd 0.12.11",
                ])
                running = SimpleNamespace(
                    renderd_build=lambda: "0.12.13 abc (gnu)")
                self.assertEqual(
                    gui.component_versions(running)[1],
                    "floe-renderd 0.12.13 abc (gnu) [running]")
            # probe failures must not break Help > About
            env = {"FLOE_INDEX_BIN": os.path.join(directory, "gone"),
                   "FLOE_RENDERD_BIN": silent}
            with mock.patch.dict(os.environ, env, clear=False):
                lines = gui.component_versions(None)
            self.assertTrue(
                lines[0].startswith("floe-index unavailable:"), lines)
            self.assertTrue(
                lines[1].startswith("floe-renderd unavailable:"), lines)

    def test_converts_patterns_and_preserves_special_fills(self):
        solid = "\n".join(["*" * 16] * 16)
        clear = "\n".join(["." * 16] * 16)
        speckle = "\n".join(
            ["*." * 8 if row % 2 == 0 else ".*" * 8
             for row in range(16)])
        self.assertEqual(_pattern_fill(solid), "solid")
        self.assertEqual(_pattern_fill(clear), "clear")
        self.assertEqual(_pattern_fill(speckle), "speckle")
        custom = ["*" + "." * 15] + ["." * 16] * 15
        self.assertEqual(
            _pattern_fill("\n".join(custom)),
            "pat:8000" + "0000" * 15)
        with self.assertRaises(ValueError):
            _pattern_fill("bad")

    def test_style_is_sorted_and_epoch_specific(self):
        with tempfile.TemporaryDirectory() as directory:
            binary = os.path.join(directory, "floe-renderd")
            with open(binary, "w", encoding="ascii") as script:
                script.write("#!/bin/sh\n")
            os.chmod(binary, 0o755)
            with mock.patch.dict(os.environ, {
                "FLOE_RENDERD_BIN": binary,
                "FLOE_RUST_JOBS": "4",
            }, clear=False):
                worker = RustRenderWorker(FakeCache(directory))
            worker._work_dir = directory
            commands = []
            worker._send = commands.append
            worker._publish_style(wait=False)
            style_path = commands[0].split("path=", 1)[1]
            with open(style_path, encoding="ascii") as style_file:
                rows = style_file.read().splitlines()
            self.assertEqual(rows, [
                "1/0 #111111 speckle 1",
                "2/0 #222222 speckle 1",
            ])
            self.assertIn("epoch=1", commands[0])
            worker._handle_line("styled", {"epoch": "1"}, "")
            self.assertFalse(os.path.exists(style_path))

    def test_render_command_and_frame_result_match_parent_schema(self):
        with tempfile.TemporaryDirectory() as directory:
            binary = os.path.join(directory, "floe-renderd")
            with open(binary, "w", encoding="ascii") as script:
                script.write("#!/bin/sh\n")
            os.chmod(binary, 0o755)
            with mock.patch.dict(os.environ, {
                "FLOE_RENDERD_BIN": binary,
                "FLOE_RUST_JOBS": "4",
                "FLOE_RUST_RASTER_JOBS": "3",
                "FLOE_RUST_LABEL_PX": "18",
            }, clear=False):
                # validate_rust.sh deliberately forces four-page rounds for its
                # progressive integration cases.  This unit instead exercises
                # the product default emitted when no override is present.
                os.environ.pop("FLOE_RUST_ROUND_PAGES", None)
                worker = RustRenderWorker(FakeCache(directory))
            worker._work_dir = directory
            worker._style_epoch = 3
            commands = []
            worker._send = commands.append
            job = {
                "kind": "render", "gen": 7,
                "bbox": (0.5, 1.5, 20.5, 11.5),
                "w": 20, "h": 10, "depth": None,
                "cut_px": 3.0, "visible": [(2, 0), (1, 0)],
                "frames": False, "labels": False, "scope": "live",
                "label_font_px": 22,
            }
            worker._submit_render(job)
            self.assertIn("depth=full", commands[0])
            self.assertIn("layers=1/0,2/0", commands[0])
            self.assertIn("style_epoch=3", commands[0])
            self.assertIn("round_paths=1", commands[0])
            self.assertIn("jobs=3 decode_jobs=4 tile_px=384", commands[0])
            self.assertIn("round_pages=%d" % (1 << 30), commands[0])
            # the view root (SPEC-VIEWER §8c) travels only when set
            self.assertNotIn(" root=", commands[0])
            worker._submit_render(dict(job, gen=98, root=17))
            rooted = commands.pop()
            self.assertTrue(rooted.endswith(" root=17"), rooted)
            self.assertIn("frame_cache=1", commands[0])
            self.assertIn("labels=0", commands[0])
            # the page hairline policy rides with every frame; a plain
            # worker's default is keep since 2026-09-23 (it was cull)
            self.assertIn("thin=keep", commands[0])
            self.assertIn("font_px=22", commands[0])
            # the interactive default skips the PNG codec on both sides
            self.assertIn("frame_format=raw", commands[0])
            fallback_job = dict(job, gen=8)
            fallback_job.pop("label_font_px")
            worker._submit_render(fallback_job)
            self.assertIn("font_px=18", commands[1])
            # headless consumers (CLI export, DRC sheets) pin real PNG
            # bytes per job over the interactive raw default
            worker._submit_render(dict(job, gen=10, frame_format="png"))
            self.assertIn("frame_format=png", commands[2])
            self.assertIn("frame-10.png", commands[2])
            with self.assertRaisesRegex(ValueError, "raw or png"):
                worker._submit_render(dict(job, gen=11, frame_format="bmp"))

            raw_pixels = b"\x12\x34\x56\xff" * (20 * 10)
            raw_path = os.path.join(directory, "frame-7.raw")
            with open(raw_path, "wb") as frame:
                frame.write(_RAW_SIGNATURE)
                frame.write((20).to_bytes(4, "little"))
                frame.write((10).to_bytes(4, "little"))
                frame.write(raw_pixels)
            # the job was submitted a second ago: what renderd's wall does
            # not account for is the client's wait (2026-09-21)
            worker._jobs[7]["started"] -= 1.0
            worker._emit_frame({
                "gen": "7", "png": raw_path, "format": "raw", "partial": "0",
                "deferred": "0", "final": "1", "plan_pages": "2",
                "pages": "2", "cache_miss": "2", "plan_us": "1000",
                "read_us": "2000", "decode_us": "3000",
                "scene_us": "4000", "raster_us": "5000",
                "png_us": "6000", "publish_write_us": "7000",
                "publish_sync_us": "8000", "publish_rename_us": "9000",
                "cache_hit": "14", "frame_cache_hit": "1",
                "bin_defer_rep": "2", "bin_defer_single": "1",
                "bin_defer_wmax": "5000",
                "resident_bytes": str(15 * 1024 * 1024),
                "retained_bytes": str(3 * 1024 * 1024),
                "decode_workers": "3", "workers": "4", "tiles": "16",
                "tile_px": "128",
                "rect_paints": "6", "polygon_paints": "7",
                "path_paints": "8", "frame_paints": "9",
                "wc_cells": "10", "inst_edges": "11",
                "frame_rects": "12",
                "text_plan_us": "250", "text_place_records": "13",
                "labels": "2", "labels_truncated": "0",
                "label_tile_paints": "3", "label_pixel_paints": "40",
                # planner verdict counters (field diagnosis 2026-09-10)
                "cull_pages": "21", "cull_pbvh": "22", "cull_cbvh": "23",
                "cull_children": "24", "cull_layer": "25", "washed": "26",
                "lod_swapped": "27", "thin_frames": "28", "thin_pages": "29",
                "sub_cut_washes": "30", "sub_cut_sparse": "31",
                "sub_cut_sparse_over": "32", "sub_cut_wash_over": "33",
                "rep_kept": "34", "rep_washed": "35", "rep_children": "36",
                "rep_page_level": "2", "rep_level": "7",
                "fit_pct": "283", "fit_cull": "1", "fit_over": "0",
                "fit_thin": "3", "fit_full_pct": "850", "fit_none_pct": "400",
                "fit_fixed": "1", "fit_redecided": "0",
                "sub_cut_boxes": "1234", "sub_cut_box_over": "5",
                "sub_cut_box_level": "1", "sub_cut_box_unsure": "2",
                "shape_cut": "4392", "shape_cut_max": "1",
                "stored_rep_points": "16384", "stored_rep_tested": "65536",
                "stored_rep_limited": "1",
                "stored_rep_nodes": "128", "stored_rep_proxies": "64",
                "stored_rep_bytes": "8192", "stored_rep_pixels": "64000",
                "stored_rep_spans": "100", "stored_rep_painted_pixels": "8000",
                "once_tiles": "5", "once_passes": "400", "once_items": "77",
                # the density stack's lit/top/lower/covered/claimed pixels, pass
                # 2's pages planned/in_hand/decoded/over_budget, its times and bins,
                # the sub-cut dots' items/over, floor and block
                "density_stack": "90/40/30/1000/200", "density_pages": "12/7/4/1",
                "density_us": "100/20/30/4/50", "density_bin": "600/1/0",
                "density_dots": "3500/2", "density_floor": "0.250",
                "density_block": "8",
                "density_plan2": "3000/4000/1/3/24/120000/900000/45000/4/700000/90000/1/2"
                                 "/30000/20000/5000/34860/40/9000/100/0/7/896",
                # 1.5 ms behind earlier commands, then 60 ms of renderd wall:
                # its phases above add up to 45.25 ms
                "queue_us": "1500", "wall_us": "60000",
            })
            result = worker.res.get_nowait()
            self.assertEqual(result["kind"], "frame")
            self.assertEqual(result["plan_culls"], {
                "pages_size": 21, "page_bvh": 22, "child_bvh": 23,
                "children_size": 24, "layer": 25, "washed": 26,
                "lod_swapped": 27, "thin_frames": 28, "thin_pages": 29,
                "sub_cut_washes": 30, "sub_cut_sparse": 31,
                "sub_cut_sparse_over": 32, "sub_cut_wash_over": 33,
                "rep_kept": 34, "rep_washed": 35, "rep_children": 36,
                "rep_page_level": 2, "rep_level": 7,
                "fit_pct": 283, "fit_cull": 1, "fit_over": 0,
                "fit_thin": 3, "fit_full_pct": 850, "fit_none_pct": 400,
                "fit_fixed": 1, "fit_redecided": 0,
                "sub_cut_boxes": 1234, "sub_cut_box_over": 5,
                "sub_cut_box_level": 1, "sub_cut_box_unsure": 2,
                "shape_cut": 4392, "shape_cut_max": 1,
                "stored_rep_points": 16384, "stored_rep_tested": 65536,
                "stored_rep_limited": 1,
                "stored_rep_nodes": 128, "stored_rep_proxies": 64,
                "stored_rep_bytes": 8192, "stored_rep_pixels": 64000,
                "stored_rep_spans": 100, "stored_rep_painted_pixels": 8000})
            self.assertEqual(result["frame_format"], "raw")
            self.assertEqual(result["rgba"], raw_pixels)
            self.assertNotIn("png", result)
            self.assertEqual(result["bbox"], job["bbox"])
            self.assertEqual(result["tiles"], 2)
            self.assertEqual(result["new"], 2)
            self.assertEqual(result["load_ms"], 10)
            self.assertEqual(result["draw_ms"], 5)
            self.assertEqual(result["read_ms"], 2.0)
            self.assertEqual(result["decode_ms"], 3.0)
            self.assertEqual(result["scene_ms"], 4.0)
            self.assertEqual(result["raster_ms"], 5.0)
            self.assertEqual(result["png_ms"], 6.0)
            self.assertEqual(result["publish_write_ms"], 7.0)
            self.assertEqual(result["publish_sync_ms"], 8.0)
            self.assertEqual(result["publish_rename_ms"], 9.0)
            self.assertEqual(result["publish_ms"], 24.0)
            self.assertGreaterEqual(result["adapter_read_ms"], 0.0)
            # the time no phase covers: renderd's own (other) and the
            # client's beyond renderd's wall (wait = queue + pipe)
            self.assertEqual((result["queue_ms"], result["wall_ms"]), (1.5, 60.0))
            self.assertEqual(result["other_ms"], 15)
            self.assertTrue(900 <= result["wait_ms"] <= 945, result["wait_ms"])
            self.assertEqual(result["cache_hit"], 14)
            self.assertEqual(result["cache_miss"], 2)
            self.assertEqual(result["frame_cache_hit"], 1)
            self.assertEqual(result["resident_mb"], 15.0)
            self.assertEqual(result["retained_mb"], 3.0)
            self.assertEqual(result["decode_workers"], 3)
            self.assertEqual(result["workers"], 4)
            self.assertEqual(result["render_tiles"], 16)
            self.assertEqual(result["tile_px"], 128)
            self.assertEqual(result["raster_jobs"], 3)
            self.assertEqual(result["frame_width"], 20)
            self.assertEqual(result["frame_height"], 10)
            self.assertEqual(result["text_plan_ms"], 0.25)
            self.assertEqual(result["text_place_records"], 13)
            self.assertEqual(result["labels"], 2)
            self.assertEqual(result["label_pixel_paints"], 40)
            self.assertEqual(result["work_bin_defer_rep"], 2)
            self.assertEqual(result["work_bin_defer_single"], 1)
            self.assertEqual(result["work_bin_defer_wmax"], 5000)
            # rect 6 + polygon 7 + path 8 + frame 9
            self.assertEqual(result["member_paints"], 30)
            self.assertEqual((result["once_full_tiles"], result["once_passes_skipped"],
                              result["once_items_skipped"]), (5, 400, 77))
            self.assertEqual(result["density_stack"], {
                "lit": 90, "top": 40, "lower": 30, "covered": 1000, "claimed": 200})
            self.assertEqual(result["density_pages"], {
                "planned": 12, "in_hand": 7, "decoded": 4, "over_budget": 1})
            self.assertEqual(result["density_us"], {
                "plan2_us": 100, "scene2_us": 20, "collect_us": 30, "regions_us": 4, "decode2_us": 50})
            self.assertEqual(result["density_bin"], {"items": 600, "deferred": 1, "overflow": 0})
            self.assertEqual(result["density_dots"], {"items": 3500, "over": 2})
            self.assertEqual(result["density_floor"], 0.25)
            self.assertEqual(result["density_block"], 8.0)
            self.assertEqual(result["density_plan2"], {
                "probe_us": 3000, "fit_us": 4000, "probes": 1, "passes": 3, "regions": 24,
                "nodes": 120000, "page_nodes": 900000, "page_candidates": 45000, "threads": 4,
                "reads": 700000, "items": 90000, "probes_over": 1, "thinned": 2,
                "by_nodes": 30000, "by_placements": 20000, "by_arrays": 5000, "by_list_members": 34860,
                "by_list_chunks": 40, "by_chunk_members": 9000, "by_array_members": 100, "by_pages": 0,
                "map_updates": 7, "reserve_mb": 896})
            self.assertNotIn("labels_truncated", result)
            self.assertNotIn("drawn", result)
            self.assertNotIn("refining", result)
            self.assertFalse(os.path.exists(raw_path))

            partial_job = dict(job, gen=9)
            worker._submit_render(partial_job)
            partial_path = os.path.join(
                directory, "frame-9.raw.gen-9.round-1.partial.raw")
            with open(partial_path, "wb") as frame:
                frame.write(_RAW_SIGNATURE)
                frame.write((20).to_bytes(4, "little"))
                frame.write((10).to_bytes(4, "little"))
                frame.write(raw_pixels)
            worker._emit_frame({
                "gen": "9", "png": partial_path, "format": "raw",
                "partial": "1",
                "deferred": "0", "final": "0", "pages": "1",
            })
            partial = worker.res.get_nowait()
            # Old/deck replies omit OVR diagnostics; keep their stable zero
            # defaults instead of carrying counters from another generation.
            for key in ("stored_rep_points", "stored_rep_tested", "stored_rep_limited",
                        "stored_rep_nodes", "stored_rep_proxies", "stored_rep_bytes",
                        "stored_rep_pixels", "stored_rep_spans", "stored_rep_painted_pixels"):
                self.assertEqual(partial["plan_culls"][key], 0)
            # a frame without the fields did not stack its density
            self.assertIsNone(partial["density_stack"])
            self.assertIsNone(partial["density_pages"])
            self.assertIsNone(partial["density_us"])
            self.assertIsNone(partial["density_bin"])
            self.assertIsNone(partial["density_dots"])
            self.assertIsNone(partial["density_floor"])
            self.assertIsNone(partial["density_block"])
            self.assertIsNone(partial["density_plan2"])
            self.assertEqual(partial["refining"], 1)
            self.assertNotIn("density_round", partial)
            self.assertIn(9, worker._jobs)
            self.assertFalse(os.path.exists(partial_path))
            # the sub-cut dots' first round (pass 1 alone) says so
            with open(partial_path, "wb") as frame:
                frame.write(_RAW_SIGNATURE)
                frame.write((20).to_bytes(4, "little"))
                frame.write((10).to_bytes(4, "little"))
                frame.write(raw_pixels)
            worker._emit_frame({
                "gen": "9", "png": partial_path, "format": "raw",
                "partial": "1", "deferred": "1", "final": "0", "density_round": "1",
            })
            first_round = worker.res.get_nowait()
            self.assertEqual((first_round["refining"], first_round["density_round"]), (1, True))
            self.assertIn(9, worker._jobs)

    def test_raw_frame_kill_switch_restores_png_frames(self):
        with tempfile.TemporaryDirectory() as directory:
            binary = os.path.join(directory, "floe-renderd")
            with open(binary, "w", encoding="ascii") as script:
                script.write("#!/bin/sh\n")
            os.chmod(binary, 0o755)
            with mock.patch.dict(os.environ, {
                "FLOE_RENDERD_BIN": binary,
                "FLOE_RUST_RAW_FRAME": "off",
            }, clear=False):
                worker = RustRenderWorker(FakeCache(directory))
            worker._work_dir = directory
            commands = []
            worker._send = commands.append
            worker._submit_render({
                "kind": "render", "gen": 7, "bbox": (0.0, 0.0, 4.0, 2.0),
                "w": 4, "h": 2, "depth": None, "cut_px": 0.0,
                "visible": None, "frames": False, "labels": False,
            })
            self.assertIn("frame_format=png", commands[0])
            png_path = os.path.join(directory, "frame-7.png")
            with open(png_path, "wb") as frame:
                frame.write(b"\x89PNG\r\n\x1a\nfixture")
            worker._emit_frame({
                "gen": "7", "png": png_path, "format": "png",
                "partial": "0", "deferred": "0", "final": "1",
                "pages": "1",
            })
            result = worker.res.get_nowait()
            self.assertEqual(result["frame_format"], "png")
            self.assertEqual(result["png"], b"\x89PNG\r\n\x1a\nfixture")
            self.assertNotIn("rgba", result)
            self.assertFalse(os.path.exists(png_path))

    def test_truncated_raw_frame_is_reported_as_error(self):
        with tempfile.TemporaryDirectory() as directory:
            binary = os.path.join(directory, "floe-renderd")
            with open(binary, "w", encoding="ascii") as script:
                script.write("#!/bin/sh\n")
            os.chmod(binary, 0o755)
            with mock.patch.dict(os.environ, {
                "FLOE_RENDERD_BIN": binary,
            }, clear=False):
                worker = RustRenderWorker(FakeCache(directory))
            worker._work_dir = directory
            worker._send = lambda command: None
            worker._submit_render({
                "kind": "render", "gen": 3, "bbox": (0.0, 0.0, 4.0, 2.0),
                "w": 4, "h": 2, "depth": None, "cut_px": 0.0,
                "visible": None, "frames": False, "labels": False,
            })
            raw_path = os.path.join(directory, "frame-3.raw")
            with open(raw_path, "wb") as frame:
                frame.write(_RAW_SIGNATURE)
                frame.write((4).to_bytes(4, "little"))
                frame.write((2).to_bytes(4, "little"))
                frame.write(b"\x00" * (4 * 2 * 4 - 1))
            worker._emit_frame({
                "gen": "3", "png": raw_path, "format": "raw",
                "partial": "0", "deferred": "0", "final": "1",
            })
            result = worker.res.get_nowait()
            self.assertEqual(result["kind"], "error")
            self.assertIn("truncated", result["msg"])
            self.assertNotIn(3, worker._jobs)
            self.assertFalse(os.path.exists(raw_path))

    def test_refinement_off_overrides_rust_round_tuning(self):
        with tempfile.TemporaryDirectory() as directory:
            binary = os.path.join(directory, "floe-renderd")
            with open(binary, "w", encoding="ascii") as script:
                script.write("#!/bin/sh\n")
            os.chmod(binary, 0o755)
            with mock.patch.dict(os.environ, {
                "FLOE_RENDERD_BIN": binary,
                "FLOE_RUST_ROUND_PAGES": "4",
            }, clear=False):
                worker = RustRenderWorker(
                    FakeCache(directory), stream_kb=0)
            self.assertEqual(worker._round_pages, 1 << 30)

    def test_clip_uses_private_wire_path_and_atomically_publishes_oasis(self):
        with tempfile.TemporaryDirectory() as directory:
            binary = os.path.join(directory, "floe-renderd")
            with open(binary, "w", encoding="ascii") as script:
                script.write("#!/bin/sh\n")
            os.chmod(binary, 0o755)
            with mock.patch.dict(os.environ, {
                "FLOE_RENDERD_BIN": binary,
                "FLOE_RUST_JOBS": "4",
            }, clear=False):
                worker = RustRenderWorker(FakeCache(directory))
            worker._work_dir = directory
            commands = []
            worker._send = commands.append
            destination = os.path.join(directory, "user clip output.oas")
            worker._submit_clip({
                "bbox": (-10, -20, 30, 40),
                "layers": [(2, 0), (1, 0)],
                "out": destination,
            })
            self.assertIn("clip seq=1 box=-10,-20,30,40", commands[0])
            self.assertIn("layers=1/0,2/0", commands[0])
            self.assertIn("cell_hex=464c4f455f434c4950", commands[0])
            self.assertNotIn(destination, commands[0])
            daemon_output = commands[0].split("out=", 1)[1]
            payload = b"%SEMI-OASIS\r\nfixture"
            with open(daemon_output, "wb") as output:
                output.write(payload)
            worker._emit_clip({"seq": "1", "size_bytes": str(len(payload)),
                               "ms": "17"})
            result = worker.res.get_nowait()
            self.assertEqual(result, {
                "kind": "clip", "path": destination,
                "size_mb": len(payload) / 1e6, "ms": 17,
            })
            with open(destination, "rb") as output:
                self.assertEqual(output.read(), payload)
            self.assertFalse(os.path.exists(daemon_output))
            worker._submit_clip({
                "bbox": (0, 0, 1, 1), "layers": [],
                "out": destination,
            })
            worker._clip_timed_out(2)
            timeout = worker.res.get_nowait()
            self.assertEqual(timeout["kind"], "error")
            self.assertIn("timed out after", timeout["msg"])
            self.assertNotIn(2, worker._clip_jobs)
            with self.assertRaisesRegex(ValueError, "reversed"):
                worker._submit_clip({
                    "bbox": (4, 0, 3, 1), "layers": [],
                    "out": destination,
                })
            self.assertNotIn(7, worker._jobs)

    def test_query_commands_and_results_match_parent_schema(self):
        with tempfile.TemporaryDirectory() as directory:
            binary = os.path.join(directory, "floe-renderd")
            with open(binary, "w", encoding="ascii") as script:
                script.write("#!/bin/sh\n")
            os.chmod(binary, 0o755)
            with mock.patch.dict(os.environ, {
                "FLOE_RENDERD_BIN": binary,
            }, clear=False):
                worker = RustRenderWorker(FakeCache(directory))
            commands = []
            worker._send = commands.append
            worker._submit_snap({
                "seq": 7, "x": 11, "y": -3, "r": 4,
                "layers": [(2, 0), (1, 0), (2, 0)],
            })
            worker._submit_pick({
                "seq": 8, "x": 5, "y": 6, "r": 0, "nth": -1,
                "layers": [],
            })
            self.assertEqual(
                commands[0],
                "snap seq=7 x=11 y=-3 r=4 layers=1/0,2/0")
            self.assertEqual(
                commands[1],
                "pick seq=8 x=5 y=6 r=1 nth=-1 layers=all")

            kind, fields = _parse_wire_line(
                "snap seq=7 found=1 x=10 y=-2 snap=vertex")
            worker._handle_line(kind, fields, "")
            self.assertEqual(worker.res.get_nowait(), {
                "kind": "snap", "seq": 7, "found": True,
                "x": 10, "y": -2, "snap": "vertex",
            })

            layer_name = "M 1/metal"
            cell_name = "TOP 한글"
            kind, fields = _parse_wire_line(
                "pick seq=8 found=1 count=2 index=1 layer=2 "
                "datatype=0 lname_hex=%s cell_hex=%s area=100 "
                "bbox=0,0,10,10 points=0,0;0,10;10,10;10,0" % (
                    layer_name.encode().hex(), cell_name.encode().hex()))
            worker._handle_line(kind, fields, "")
            self.assertEqual(worker.res.get_nowait(), {
                "kind": "pick", "seq": 8, "found": True,
                "count": 2, "index": 1, "layer": 2, "datatype": 0,
                "lname": layer_name, "cell": cell_name, "area": 100.0,
                "bbox": [0, 0, 10, 10],
                "points": [(0, 0), (0, 10), (10, 10), (10, 0)],
            })

            kind, fields = _parse_wire_line(
                "pick seq=9 found=0 count=0")
            worker._handle_line(kind, fields, "")
            self.assertEqual(worker.res.get_nowait(), {
                "kind": "pick", "seq": 9, "found": False, "count": 0,
            })

            # the cell tree's queries (docs/SPEC-VIEWER.ko.md §8c): one
            # line per kind, the answer decoded per kind
            del commands[:]
            for job in (
                    {"kind": "cell_sources", "seq": 10},
                    {"kind": "cells", "seq": 11},
                    {"kind": "cells", "seq": 12, "src": 2, "cell": 17},
                    {"kind": "cell_find", "seq": 13, "pattern": "*inv?",
                     "limit": 10},
                    {"kind": "cell_find", "seq": 14, "src": 1},
                    {"kind": "cell_bbox", "seq": 15, "cell": 9},
                    {"kind": "cell_insts", "seq": 16, "src": 1, "cell": 9,
                     "view": (0, -5, 10.5, 20), "cap": 7},
                    {"kind": "cell_bbox", "seq": 17, "cell": 9, "root": 3},
                    {"kind": "cell_insts", "seq": 18, "cell": 9,
                     "view": (0, 0, 1, 1), "root": 3}):
                self.assertIn(job["kind"], CELL_QUERY_KINDS)
                worker._submit_cell_query(job)
            self.assertEqual(commands, [
                "cell_sources seq=10",
                "cells seq=11 src=0",
                "cells seq=12 src=2 cell=17",
                "cell_find seq=13 src=-1 pat_hex=%s limit=10"
                % "*inv?".encode().hex(),
                "cell_find seq=14 src=1 limit=5000",
                "cell_bbox seq=15 src=0 cell=9",
                "cell_insts seq=16 src=1 cell=9 view=0.0,-5.0,10.5,20.0 "
                "cap=7",
                "cell_bbox seq=17 src=0 cell=9 root=3",
                "cell_insts seq=18 src=0 cell=9 view=0.0,0.0,1.0,1.0 "
                "cap=4096 root=3",
            ])
            path = "/caches/.a b.oas.ice"
            kind, fields = _parse_wire_line(
                "cell_sources seq=10 found=1 n=1 sources=0:2:%s"
                % path.encode().hex())
            worker._handle_line(kind, fields, "")
            self.assertEqual(worker.res.get_nowait(), {
                "kind": "cell_sources", "seq": 10, "found": True,
                "sources": [{"src": 0, "placements": 2, "path": path}],
            })
            kind, fields = _parse_wire_line(
                "cells seq=12 src=2 found=1 cell=17 name_hex=%s insts=6 "
                "height=3 unit=1000 bbox=0,0,10,20 n=2 total=5 "
                "children=3:12:1:%s,4:1:0:%s" % (
                    "MID 한".encode().hex(), "leaf".encode().hex(),
                    "a:b".encode().hex()))
            worker._handle_line(kind, fields, "")
            self.assertEqual(worker.res.get_nowait(), {
                "kind": "cells", "seq": 12, "found": True, "src": 2,
                "cell": 17, "name": "MID 한", "insts": 6, "height": 3,
                "unit": 1000.0, "bbox": [0.0, 0.0, 10.0, 20.0],
                "total": 5,
                "children": [
                    {"cell": 3, "members": 12, "leaf": True,
                     "name": "leaf"},
                    {"cell": 4, "members": 1, "leaf": False,
                     "name": "a:b"}],
            })
            kind, fields = _parse_wire_line(
                "cells seq=13 found=0 code=nohier err_hex=%s"
                % "no summary".encode().hex())
            worker._handle_line(kind, fields, "")
            self.assertEqual(worker.res.get_nowait(), {
                "kind": "cells", "seq": 13, "found": False,
                "code": "nohir".replace("hir", "hier"), "err": "no summary",
            })
            kind, fields = _parse_wire_line(
                "cell_find seq=14 src=-1 found=1 total=1 n=1 "
                "matches=0:5:99:%s" % "INV1".encode().hex())
            worker._handle_line(kind, fields, "")
            self.assertEqual(worker.res.get_nowait(), {
                "kind": "cell_find", "seq": 14, "found": True, "src": -1,
                "total": 1,
                "matches": [{"src": 0, "cell": 5, "insts": 99,
                             "name": "INV1"}],
            })
            kind, fields = _parse_wire_line(
                "cell_bbox seq=15 src=0 cell=9 found=1 insts=2 approx=1 "
                "bbox=-1.5,0,3,4")
            worker._handle_line(kind, fields, "")
            self.assertEqual(worker.res.get_nowait(), {
                "kind": "cell_bbox", "seq": 15, "found": True, "src": 0,
                "cell": 9, "insts": 2, "approx": True,
                "bbox": [-1.5, 0.0, 3.0, 4.0],
            })
            kind, fields = _parse_wire_line(
                "cell_bbox seq=15 src=0 cell=9 found=1 insts=0 approx=0 "
                "bbox=-")
            worker._handle_line(kind, fields, "")
            self.assertIsNone(worker.res.get_nowait()["bbox"])
            kind, fields = _parse_wire_line(
                "cell_insts seq=16 src=1 cell=9 found=1 n=2 more=1 "
                "visited=40 boxes=0,0,1,1;2,2,3.5,3")
            worker._handle_line(kind, fields, "")
            self.assertEqual(worker.res.get_nowait(), {
                "kind": "cell_insts", "seq": 16, "found": True, "src": 1,
                "cell": 9, "more": True, "visited": 40,
                "boxes": [[0.0, 0.0, 1.0, 1.0], [2.0, 2.0, 3.5, 3.0]],
            })
            kind, fields = _parse_wire_line(
                "cell_insts seq=17 src=1 cell=9 found=1 n=0 more=0 "
                "visited=1 boxes=-")
            worker._handle_line(kind, fields, "")
            self.assertEqual(worker.res.get_nowait()["boxes"], [])
            # a malformed row is an error, not a crash
            kind, fields = _parse_wire_line(
                "cells seq=18 src=0 found=1 cell=1 name_hex=41 insts=1 "
                "height=0 unit=1 bbox=- n=1 total=1 children=3:12")
            worker._handle_line(kind, fields, "")
            self.assertEqual(worker.res.get_nowait()["kind"], "error")

    def test_rust_worker_never_loads_or_composites_density_coverage(self):
        with tempfile.TemporaryDirectory() as directory:
            binary = os.path.join(directory, "floe-renderd")
            with open(binary, "w", encoding="ascii") as script:
                script.write("#!/bin/sh\n")
            os.chmod(binary, 0o755)
            # Old/shared caches may still carry the optional sidecar. The
            # floe2 Rust worker must not read it or expose a post-compositor:
            # sample09 measured 350ms -> 980ms refinement with no visible
            # change when this path ran on every progressive PNG.
            with open(os.path.join(directory, "design.ovc"), "wb") as ovc:
                ovc.write(b"must remain unread")
            with mock.patch.dict(os.environ, {
                "FLOE_RENDERD_BIN": binary,
            }, clear=False):
                worker = RustRenderWorker(FakeCache(directory))
            self.assertFalse(hasattr(worker, "_coverage"))
            self.assertFalse(hasattr(worker, "_apply_coverage"))

    def test_budget_default_stays_fixed_for_shared_hosts(self):
        # Deliberately NOT host-proportional (user call 2026-08-28):
        # shared servers make a half-the-RAM default a neighbor
        # hazard. Retention beyond 1024MB is an explicit opt-in.
        with tempfile.TemporaryDirectory() as directory:
            binary = os.path.join(directory, "floe-renderd")
            with open(binary, "w", encoding="ascii") as script:
                script.write("#!/bin/sh\n")
            os.chmod(binary, 0o755)
            environment = dict(os.environ)
            environment["FLOE_RENDERD_BIN"] = binary
            environment.pop("FLOE_RUST_BUDGET_MB", None)
            with mock.patch.dict(os.environ, environment, clear=True):
                worker = RustRenderWorker(FakeCache(directory))
            self.assertEqual(worker._budget_mb, 1024)
            environment["FLOE_RUST_BUDGET_MB"] = "8192"
            with mock.patch.dict(os.environ, environment, clear=True):
                raised = RustRenderWorker(FakeCache(directory))
            self.assertEqual(raised._budget_mb, 8192)

    def test_rejects_invalid_environment_limits(self):
        with tempfile.TemporaryDirectory() as directory:
            binary = os.path.join(directory, "floe-renderd")
            with open(binary, "w", encoding="ascii") as script:
                script.write("#!/bin/sh\n")
            os.chmod(binary, 0o755)
            with mock.patch.dict(os.environ, {
                "FLOE_RENDERD_BIN": binary,
                "FLOE_RUST_JOBS": "0",
            }, clear=False):
                with self.assertRaisesRegex(RuntimeError, "1..256"):
                    RustRenderWorker(FakeCache(directory))
            with mock.patch.dict(os.environ, {
                "FLOE_RENDERD_BIN": binary,
                "FLOE_RUST_JOBS": "4",
                "FLOE_RUST_RASTER_JOBS": "0",
            }, clear=False):
                with self.assertRaisesRegex(RuntimeError, "1..256"):
                    RustRenderWorker(FakeCache(directory))
            with mock.patch.dict(os.environ, {
                "FLOE_RENDERD_BIN": binary,
                "FLOE_RUST_LABEL_PX": "5",
            }, clear=False):
                with self.assertRaisesRegex(RuntimeError, "6..96"):
                    RustRenderWorker(FakeCache(directory))

    def test_rejects_abstract_as_intentionally_out_of_scope(self):
        with tempfile.TemporaryDirectory() as directory:
            binary = os.path.join(directory, "floe-renderd")
            with open(binary, "w", encoding="ascii") as script:
                script.write("#!/bin/sh\n")
            os.chmod(binary, 0o755)
            with mock.patch.dict(os.environ, {
                "FLOE_RENDERD_BIN": binary,
            }, clear=False):
                worker = RustRenderWorker(FakeCache(directory))
            with self.assertRaisesRegex(
                    RuntimeError, "intentionally unsupported"):
                worker._submit_render({"abstract": True})

    def test_rejects_invalid_request_label_size(self):
        with tempfile.TemporaryDirectory() as directory:
            binary = os.path.join(directory, "floe-renderd")
            with open(binary, "w", encoding="ascii") as script:
                script.write("#!/bin/sh\n")
            os.chmod(binary, 0o755)
            with mock.patch.dict(os.environ, {
                "FLOE_RENDERD_BIN": binary,
            }, clear=False):
                worker = RustRenderWorker(FakeCache(directory))
            with self.assertRaisesRegex(ValueError, "6..96"):
                worker._submit_render({
                    "gen": 1, "bbox": (0, 0, 1, 1),
                    "label_font_px": 5,
                })


class IndexOnOpenTests(unittest.TestCase):
    """Viewer._open_or_index (user call 2026-09-09): a layout or
    jobdeck without an index is ASKED about, indexed in the modal log,
    opened, and the request's options applied - from the load dialog,
    a forwarded `floe2 view`, and a fresh start alike."""

    def _shell(self, ready, answer=True):
        from floe.gui import Viewer
        v = Viewer.__new__(Viewer)
        calls = []
        v._index_ready = lambda path, ids=None: ready
        v._ask_yes_no = lambda text: (calls.append(("ask", text)), answer)[1]
        v.open_file = lambda path, ids=None: (
            calls.append(("open", path, ids)), None)[1]
        # the level question (user call 2026-09-10): the shell answers
        # "every level" unless a test installs its own
        v._jobdeck_pick_levels = lambda path, current=None, force=False: (
            calls.append(("levels", path)), None)[1]
        v._forwarded_view_options = lambda fields: (
            calls.append(("options", list(fields))), True)[1]
        v._forwarded_goto = lambda fields: (
            calls.append(("goto", list(fields))),
            any(f.startswith("goto=") for f in fields))[1]
        v.redraw = lambda immediate=False: calls.append(("redraw", immediate))
        v._present = lambda: calls.append(("present",))
        v._set_live_status = lambda msg: calls.append(("status", msg))
        v._restore_keys = lambda: calls.append(("keys",))

        # like the real helpers: index, open, then report to `after`
        def index_and_load(path, after=None):
            calls.append(("vfs-index", path))
            after(v.open_file(path))
        v._vfs_index_and_load = index_and_load

        def deck_index_and_load(path, after=None, ids=None):
            calls.append(("deck-index", path, ids))
            after(v.open_file(path, ids))
        v._jobdeck_index_and_load = deck_index_and_load
        return v, calls

    def test_ready_opens_and_applies_options(self):
        from floe.gui import Viewer
        v, calls = self._shell(ready=True)
        self.assertFalse(Viewer._open_or_index(
            v, "/x/chip.oas", ["detail=high", "depth=3"]))
        self.assertEqual([c[0] for c in calls],
                         ["open", "options", "goto", "redraw", "present"])
        self.assertEqual(calls[0][1], "/x/chip.oas")

    def test_deck_asks_levels_then_opens_them(self):
        """User call 2026-09-10 (Calibre-style load): a jobdeck asks
        which mask levels to load before anything else; a `levels=`
        field (floe2 view --level) or an explicit `ids` answers it;
        the readiness check, the index run and the open carry the
        selection; Cancel opens nothing."""
        from floe.gui import Viewer
        v, calls = self._shell(ready=True)
        Viewer._open_or_index(v, "/x/deck.jb", ["depth=999"])
        self.assertEqual(calls[0], ("levels", "/x/deck.jb"))
        self.assertEqual(calls[1], ("open", "/x/deck.jb", None),
                         "every level")
        v, calls = self._shell(ready=True)
        Viewer._open_or_index(v, "/x/deck.jb", ["levels=3,1", "depth=999"])
        self.assertEqual(calls[0], ("open", "/x/deck.jb", [1, 3]),
                         "levels= answers the question, unasked")
        self.assertEqual(calls[1], ("options", ["depth=999"]),
                         "levels= is not a view option")
        v, calls = self._shell(ready=False, answer=True)
        Viewer._open_or_index(v, "/x/deck.jb", ids=[2])
        self.assertEqual(calls[0][0], "ask")
        self.assertEqual(calls[1], ("deck-index", "/x/deck.jb", [2]))
        self.assertEqual(calls[2], ("open", "/x/deck.jb", [2]))
        v, calls = self._shell(ready=True)
        v._jobdeck_pick_levels = lambda path, current=None, force=False: False
        Viewer._open_or_index(v, "/x/deck.jb")
        self.assertEqual([c[0] for c in calls], ["status", "keys"])
        self.assertIn("cancelled", calls[0][1])
        # review 2026-09-10 (9th) P2-1: an answered "every level"
        # (ask_levels=False, ids None) is final - not asked again, not
        # replaced by the environment's list
        v, calls = self._shell(ready=True)
        os.environ["FLOE_JOBDECK_LEVELS"] = "3"
        try:
            Viewer._open_or_index(v, "/x/deck.jb", ids=None,
                                  ask_levels=False)
        finally:
            del os.environ["FLOE_JOBDECK_LEVELS"]
        self.assertEqual(calls[0], ("open", "/x/deck.jb", None))
        self.assertNotIn("levels", [c[0] for c in calls])

    def test_load_browser_hides_caches_and_sidecars(self):
        """User call 2026-09-10: the load dialog's folder listing never
        shows a layout's <src>.floe cache or a DRC db's .ice sidecar
        (nor dotfiles); folders come first, files follow the filter."""
        import tempfile
        from floe import gui
        with tempfile.TemporaryDirectory() as d:
            os.mkdir(os.path.join(d, "a.oas.floe"))
            os.mkdir(os.path.join(d, "Sub"))
            os.mkdir(os.path.join(d, ".git"))
            for name in ("a.oas", "b.GDS", "x.db", "x.db.ice", ".hidden",
                         "deck.jb", "notes.txt"):
                with open(os.path.join(d, name), "w") as fh:
                    fh.write("x")
            folders, files = gui.list_browse_entries(
                d, ("*.oas", "*.oas.gz", "*.gds", "*.gds.gz"))
            self.assertEqual(folders, ["Sub"])
            self.assertEqual([f[0] for f in files], ["a.oas", "b.GDS"])
            self.assertEqual(files[0][1], 1)
            folders, files = gui.list_browse_entries(d, ("*.jb",))
            self.assertEqual([f[0] for f in files], ["deck.jb"])
            folders, files = gui.list_browse_entries(d, ("*",))
            self.assertEqual([f[0] for f in files],
                             ["a.oas", "b.GDS", "deck.jb", "notes.txt", "x.db"],
                             "all files: still no .ice, no dotfile")
            self.assertEqual(gui.list_browse_entries(
                os.path.join(d, "nope"), ("*",)), ([], []))
        self.assertEqual(gui.fmt_bytes(1023), "1023 B")
        self.assertEqual(gui.fmt_bytes(4300), "4.2 KB")
        self.assertEqual(gui.fmt_bytes(1.3 * 1024 ** 2), "1.3 MB")
        import inspect
        src = inspect.getsource(gui.Viewer._load_layout_dialog)
        self.assertIn("_browse_file_dialog(", src)
        self.assertNotIn("FileChooserDialog", src)

    def test_load_dialog_routes_a_deck_through_the_level_question(self):
        """File > load jobdeck… (field 2026-09-10): an INDEXED deck
        opened straight away, so the level dialog never showed; every
        picked deck now goes through _open_or_index, an indexed layout
        still opens in place, an unindexed one asks."""
        from floe.gui import Viewer
        import types
        for path, ready, expect in (("/x/deck.jb", True, "open_or_index"),
                                    ("/x/deck.jb", False, "open_or_index"),
                                    ("/x/chip.oas", True, "open_file"),
                                    ("/x/chip.oas", False, "open_or_index")):
            calls = []
            v = Viewer.__new__(Viewer)
            v._index_ready = lambda p, ids=None: ready
            v._open_or_index = lambda p, fields=(), then=None, ids=None, \
                ask_levels=True: calls.append(("open_or_index", p))
            v.open_file = lambda p, ids=None: (calls.append(("open_file", p)),
                                               None)[1]
            Viewer._load_picked(v, path)
            self.assertEqual(calls, [(expect, path)], (path, ready))

    def test_reselecting_every_level_is_final(self):
        """Jobdeck > select levels to load…: the dialog's answer goes
        to the open as is - None (all levels) included - and the same
        selection again does nothing (review 2026-09-10 (9th) P2-1)."""
        from floe.gui import Viewer
        import types
        calls = []
        v = Viewer.__new__(Viewer)
        v.cache = types.SimpleNamespace(is_jobdeck=True, ids=[3],
                                        src="/x/deck.jb")
        v.cx, v.cy, v.spp = 1.0, 2.0, 3.0
        v._restore_keys = lambda: calls.append(("keys",))
        v._set_live_status = lambda msg: calls.append(("status", msg))
        v._open_or_index = lambda path, fields=(), then=None, ids=None, \
            ask_levels=True: calls.append(("open", path, ids, ask_levels))
        v._jobdeck_pick_levels = lambda path, current=None, force=False: (
            calls.append(("pick", current, force)), None)[1]
        Viewer._jobdeck_reselect_levels(v)
        self.assertEqual(calls, [("pick", [3], True),
                                 ("open", "/x/deck.jb", None, False)])
        calls.clear()
        v._jobdeck_pick_levels = lambda path, current=None, force=False: [3]
        Viewer._jobdeck_reselect_levels(v)
        self.assertEqual(calls, [("keys",)], "the same selection: no reload")

    def test_missing_index_asks_then_indexes_then_opens(self):
        from floe.gui import Viewer
        v, calls = self._shell(ready=False, answer=True)
        os.environ.pop("FLOE_INDEX_ON_OPEN", None)
        Viewer._open_or_index(v, "/x/chip.oas", ["goto=1,2,3"])
        self.assertEqual([c[0] for c in calls],
                         ["ask", "vfs-index", "open", "options", "goto",
                          "present"], "a goto owns the one redraw")
        self.assertIn("No VFS index for", calls[0][1])
        self.assertIn("chip.oas", calls[0][1])
        # a jobdeck asks which levels to load (user call 2026-09-10),
        # then about its sources, and indexes them
        v, calls = self._shell(ready=False, answer=True)
        Viewer._open_or_index(v, "/x/deck.jb")
        self.assertEqual([c[0] for c in calls[:4]],
                         ["levels", "ask", "deck-index", "open"])
        self.assertIn("Not every source of", calls[1][1])

    def test_then_runs_after_a_successful_open_only(self):
        """A --drc load must follow the (possibly indexed) layout open:
        an in-place open resets the DRC state (review 4th P2-1)."""
        from floe.gui import Viewer
        os.environ["FLOE_INDEX_ON_OPEN"] = "yes"
        try:
            v, calls = self._shell(ready=False)
            Viewer._open_or_index(v, "/x/chip.oas", [],
                                  then=lambda: calls.append(("then",)))
            # no goto: the changed options own one redraw, then the
            # follow-up runs
            self.assertEqual([c[0] for c in calls],
                             ["vfs-index", "open", "options", "goto",
                              "redraw", "present", "then"])
            v, calls = self._shell(ready=True)
            v.open_file = lambda path: "ERR no such layout"
            Viewer._open_or_index(v, "/x/chip.oas", [],
                                  then=lambda: calls.append(("then",)))
            self.assertEqual([c[0] for c in calls], ["status"])
        finally:
            del os.environ["FLOE_INDEX_ON_OPEN"]

    def test_drc_pack_shares_the_consent_policy(self):
        """DRC > open / --drc honour FLOE_INDEX_ON_OPEN like layouts
        and jobdecks (review 4th P2-2)."""
        from floe.gui import Viewer
        with tempfile.TemporaryDirectory() as td:
            db = os.path.join(td, "chip.db")
            open(db, "w").close()          # no .ice pack beside it
            for policy, asked, packed in (("yes", 0, 1), ("no", 0, 0),
                                          ("ask", 1, 1)):
                v = Viewer.__new__(Viewer)
                calls = []
                v._ask_yes_no = lambda text: (calls.append("ask"), True)[1]
                v._drc_pack_and_load = lambda path: calls.append("pack")
                v._set_live_status = lambda msg: calls.append("status")
                v.load_drc = lambda path, db=None: calls.append("load")
                os.environ["FLOE_INDEX_ON_OPEN"] = policy
                try:
                    Viewer._drc_open_db(v, db)
                finally:
                    del os.environ["FLOE_INDEX_ON_OPEN"]
                self.assertEqual(calls.count("ask"), asked, policy)
                self.assertEqual(calls.count("pack"), packed, policy)
                if not packed:
                    self.assertIn("status", calls)

    def test_declined_or_forbidden_leaves_the_file_unopened(self):
        from floe.gui import Viewer
        v, calls = self._shell(ready=False, answer=False)
        os.environ.pop("FLOE_INDEX_ON_OPEN", None)
        Viewer._open_or_index(v, "/x/chip.oas")
        self.assertEqual([c[0] for c in calls], ["ask", "status", "keys"])
        self.assertIn("index chip.oas", calls[1][1])
        # FLOE_INDEX_ON_OPEN=no never asks, =yes never asks either
        os.environ["FLOE_INDEX_ON_OPEN"] = "no"
        try:
            v, calls = self._shell(ready=False, answer=True)
            Viewer._open_or_index(v, "/x/chip.oas")
            self.assertEqual([c[0] for c in calls], ["status", "keys"])
            os.environ["FLOE_INDEX_ON_OPEN"] = "yes"
            v, calls = self._shell(ready=False, answer=False)
            Viewer._open_or_index(v, "/x/chip.oas")
            self.assertEqual([c[0] for c in calls[:2]],
                             ["vfs-index", "open"])
        finally:
            del os.environ["FLOE_INDEX_ON_OPEN"]


@unittest.skipUnless(os.environ.get("FLOE_INTEGRATION_SOURCE"),
                     "set FLOE_INTEGRATION_SOURCE for the real daemon test")
class RealDaemonIntegrationTests(unittest.TestCase):
    maxDiff = None

    def test_parent_cache_progressive_style_and_shutdown(self):
        from floe.cache import Cache
        from floe.service import make_render_worker

        source = os.path.abspath(os.environ["FLOE_INTEGRATION_SOURCE"])
        cache = Cache(source)
        cache_override = os.environ.get("FLOE_INTEGRATION_CACHE")
        if cache_override:
            cache.dir = os.path.abspath(cache_override)
        cache.load()
        worker = make_render_worker(cache)
        self.assertIsInstance(worker, RustRenderWorker)
        worker.start()
        try:
            bbox = tuple(cache.meta["bbox"])
            span_x = max(1, int(bbox[2] - bbox[0]))
            span_y = max(1, int(bbox[3] - bbox[1]))
            render_height = max(1, (400 * span_y + span_x - 1) // span_x)
            base_job = {
                "kind": "render", "gen": 1, "scope": "live",
                "bbox": bbox, "view": None, "w": 400,
                "h": render_height,
                "depth": None, "cut_px": 0.0, "visible": None,
                "frames": False, "labels": False, "abstract": False,
            }
            worker.submit(base_job)
            first_frames = self._frames_through_settled(worker, 1)
            self.assertGreaterEqual(len(first_frames), 2)
            self.assertTrue(first_frames[0].get("refining"))
            self.assertNotIn("refining", first_frames[-1])
            self.assertGreater(first_frames[-1]["tiles"], 4)
            first_payload = self._frame_payload(first_frames[-1])

            # §F2R-18: an exact revisit reuses every retained tile
            # (the payload cache is retired) - the raster collapses to
            # memcpys and the payload stays byte-identical.
            worker.submit(dict(base_job, gen=2))
            revisited = self._frames_through_settled(worker, 2)
            self.assertEqual(len(revisited), 1)
            self.assertEqual(revisited[0].get("frame_cache_hit", 0), 0)
            self.assertGreater(revisited[0].get("tiles_reused", 0), 0)
            # §F2R-20: the retained geometry is reported as a level
            self.assertGreater(revisited[0].get("retained_mb", 0.0), 0.0)
            self.assertEqual(revisited[0]["tiles_reused"],
                             revisited[0]["render_tiles"],
                             "an exact revisit reuses every tile")
            self.assertEqual(self._frame_payload(revisited[0]),
                             first_payload)
            self._assert_query_parity(cache, worker, bbox)
            self._assert_clip_parity(cache, worker, bbox)

            # Backend-neutral timing bypasses retained reuse via the
            # frame_cache flag while keeping the decoded warm set.
            worker.submit(dict(base_job, gen=3, frame_cache=False))
            uncached = self._frames_through_settled(worker, 3)
            self.assertEqual(len(uncached), 1)
            self.assertEqual(uncached[0].get("tiles_reused", 0), 0)
            self.assertGreater(uncached[0].get("raster_ms", 0.0), 0.0)
            self.assertEqual(self._frame_payload(uncached[0]),
                             first_payload)

            solid = "\n".join(["*" * 16] * 16)
            worker.submit({
                "kind": "recolor",
                "colors": [[[1, 0], "#ff0000"]],
            })
            worker.submit({
                "kind": "repattern",
                "fills": [[[1, 0], solid]],
                "widths": [[[1, 0], 2]],
            })
            worker.submit({"kind": "mono", "on": True})
            second_job = dict(
                base_job, gen=4, depth=3, frames=True, labels=True)
            worker.submit(second_job)
            second_frames = self._frames_through_settled(worker, 4)
            self.assertNotEqual(self._frame_payload(second_frames[-1]),
                                first_payload)
            self.assertIn("labels", second_frames[-1])
            self.assertNotIn("labels_truncated", second_frames[-1])
            if second_frames[-1]["labels"]:
                self.assertGreater(
                    second_frames[-1]["label_pixel_paints"], 0)

            # Model a pan/zoom burst: every request advances the strict
            # generation frontier before the previous expensive frame can
            # settle.  Only the latest generation may publish a frame.
            burst_first = 5
            burst_last = 104
            for generation in range(burst_first, burst_last + 1):
                shift = generation % 11 - 5
                shifted_bbox = (
                    bbox[0] + shift, bbox[1],
                    bbox[2] + shift, bbox[3],
                )
                worker.submit(dict(
                    base_job, gen=generation, bbox=shifted_bbox,
                    w=1000, h=700,
                ))
            burst_frames = self._frames_through_settled(
                worker, burst_last, reject_generations=range(
                    burst_first, burst_last))
            self.assertTrue(burst_frames)
            self.assertFalse(list(Path(worker._work_dir).glob(
                "*.partial.png")))
            self.assertFalse(list(Path(worker._work_dir).glob(
                "*.partial.raw")))

            # §F2R-16: a pan by an exact multiple of 16 device pixels
            # reuses the previous geometry frame's overlap and must be
            # byte-identical to a cold render of the same view (labels
            # and frames on - the on-top label pass is part of the
            # contract).
            q = max(1, int(bbox[2] - bbox[0]) // 800)
            pan_a = (int(bbox[0]), int(bbox[1]),
                     int(bbox[0]) + 800 * q, int(bbox[1]) + 768 * q)
            pan_b = (pan_a[0] + 32 * q, pan_a[1] + 32 * q,
                     pan_a[2] + 32 * q, pan_a[3] + 32 * q)
            pan_job = dict(base_job, w=800, h=768, frames=True,
                           labels=True)
            worker.submit(dict(pan_job, gen=105, bbox=pan_a))
            self._frames_through_settled(worker, 105)
            worker.submit(dict(pan_job, gen=106, bbox=pan_b))
            pan_frames = self._frames_through_settled(worker, 106)
            self.assertGreater(
                pan_frames[-1].get("tiles_reused", 0), 100,
                "the snapped pan must reuse the overlap tiles")
            cold = make_render_worker(cache)
            cold.start()
            try:
                # mirror the style state the warm worker accumulated
                cold.submit({"kind": "recolor",
                             "colors": [[[1, 0], "#ff0000"]]})
                cold.submit({"kind": "repattern",
                             "fills": [[[1, 0], solid]],
                             "widths": [[[1, 0], 2]]})
                cold.submit({"kind": "mono", "on": True})
                cold.submit(dict(pan_job, gen=1, bbox=pan_b))
                cold_frames = self._frames_through_settled(cold, 1)
            finally:
                cold.stop()
            self.assertEqual(cold_frames[-1].get("tiles_reused", 0), 0)
            self.assertEqual(self._frame_payload(pan_frames[-1]),
                             self._frame_payload(cold_frames[-1]))

            # §F2R-17/21: a margin prefetch (geometry only, as the GUI
            # submits it) reuses the viewport frame as its center, and
            # a pan fully inside the margin WITH LABELS ON reuses every
            # tile through the label re-synthesis fast path - no page
            # plan, no pages, labels planned for this viewport - and is
            # byte-identical to a cold render.
            margin_bbox = (pan_b[0] - 64 * q, pan_b[1] - 64 * q,
                           pan_b[2] + 64 * q, pan_b[3] + 64 * q)
            worker.submit(dict(pan_job, gen=107, bg=True, labels=False,
                               bbox=margin_bbox,
                               w=800 + 128, h=768 + 128))
            margin_frames = self._frames_through_settled(worker, 107)
            self.assertTrue(margin_frames[-1].get("bg"))
            self.assertGreater(
                margin_frames[-1].get("tiles_reused", 0), 0,
                "the margin must reuse the viewport as its center")
            pan_c = (pan_b[0] + 64 * q, pan_b[1] - 64 * q,
                     pan_b[2] + 64 * q, pan_b[3] - 64 * q)
            worker.submit(dict(pan_job, gen=108, bbox=pan_c))
            inside_frames = self._frames_through_settled(worker, 108)
            self.assertEqual(
                inside_frames[-1].get("tiles_reused", 0),
                inside_frames[-1]["render_tiles"],
                "a pan inside the margin must reuse every tile")
            self.assertEqual(inside_frames[-1]["tiles"], 0,
                             "full cover skips the page plan entirely")
            self.assertGreater(inside_frames[-1].get("labels", 0), 0,
                               "labels are planned for the viewport")
            # §F2R-21 review (MEDIUM): the viewport frame of that pan
            # is contained in the margin, so the margin stays retained
            # (the residency does not shrink to the viewport frame)
            self.assertEqual(inside_frames[-1]["retained_mb"],
                             margin_frames[-1]["retained_mb"])
            cold2 = make_render_worker(cache)
            cold2.start()
            try:
                cold2.submit({"kind": "recolor",
                              "colors": [[[1, 0], "#ff0000"]]})
                cold2.submit({"kind": "repattern",
                              "fills": [[[1, 0], solid]],
                              "widths": [[[1, 0], 2]]})
                cold2.submit({"kind": "mono", "on": True})
                cold2.submit(dict(pan_job, gen=1, bbox=pan_c))
                cold2_frames = self._frames_through_settled(cold2, 1)
            finally:
                cold2.stop()
            self.assertEqual(self._frame_payload(inside_frames[-1]),
                             self._frame_payload(cold2_frames[-1]))

            # §F2R-21 crop oracle: the GUI crops a margin only while
            # labels are OFF (a labelled margin's off-frame label tails
            # could overwrite in-view labels - review 2026-09-05). A
            # geometry-only crop is byte-exact under the 16 px fill
            # contract: it must equal a direct label-free render of the
            # same view exactly. The crop sits 96 px in, 16 px down in
            # the 928x896 margin.
            crop = (pan_b[0] + 32 * q, pan_b[1] + 48 * q,
                    pan_b[2] + 32 * q, pan_b[3] + 48 * q)
            worker.submit(dict(pan_job, gen=109, bbox=crop, labels=False))
            crop_frames = self._frames_through_settled(worker, 109)
            self.assertEqual(crop_frames[-1]["tiles"], 0,
                             "a second pan inside the margin stays fast")
            margin_rgba = margin_frames[-1].get("rgba")
            self.assertIsNotNone(margin_rgba, "raw transport expected")
            self.assertEqual(
                self._crop_rgba(margin_rgba, 800 + 128, 96, 16, 800, 768),
                crop_frames[-1]["rgba"],
                "a label-free margin crop must equal a direct render")

            # §F2R-21 (user call): the GUI crops a LABELLED margin too.
            # Its labels are planned over the margin box (bin-aligned,
            # same budgets as a viewport), so the crop is deterministic:
            # two labelled margin renders are byte-identical and carry
            # labels; a truncated plan would be refused by the GUI.
            worker.submit(dict(pan_job, gen=111, bg=True, bbox=margin_bbox,
                               w=800 + 128, h=768 + 128))
            labelled_a = self._frames_through_settled(worker, 111)
            worker.submit(dict(pan_job, gen=112, bg=True, bbox=margin_bbox,
                               w=800 + 128, h=768 + 128))
            labelled_b = self._frames_through_settled(worker, 112)
            self.assertGreater(labelled_a[-1].get("labels", 0), 0)
            self.assertFalse(labelled_a[-1].get("labels_truncated"))
            self.assertEqual(self._frame_payload(labelled_a[-1]),
                             self._frame_payload(labelled_b[-1]))
            self.assertNotEqual(self._frame_payload(labelled_a[-1]),
                                margin_rgba, "labels paint on the margin")

            # §F2R-20: FLOE_RUST_RETAINED_MB=0 retains nothing, so an
            # exact revisit re-rasters in full (pan reuse kill switch
            # by budget) while pixels stay identical.
            with mock.patch.dict(os.environ,
                                 {"FLOE_RUST_RETAINED_MB": "0"},
                                 clear=False):
                noretain = make_render_worker(cache)
                noretain.start()
            try:
                noretain.submit(dict(base_job, gen=1))
                self._frames_through_settled(noretain, 1)
                noretain.submit(dict(base_job, gen=2))
                again = self._frames_through_settled(noretain, 2)
                self.assertEqual(again[-1].get("tiles_reused", 0), 0)
                self.assertEqual(again[-1].get("retained_mb", 0.0), 0.0)
                self.assertEqual(self._frame_payload(again[-1]),
                                 first_payload)
            finally:
                noretain.stop()

            # §F2R-20 (review): --frame-cache off must neither clone nor
            # retain geometry - the decision is made before the raster
            # runs - so a fresh daemon under the baseline flag reports
            # retained 0 on every frame while pixels stay identical.
            baseline = make_render_worker(cache)
            baseline.start()
            try:
                baseline.submit(dict(base_job, gen=1, frame_cache=False))
                b1 = self._frames_through_settled(baseline, 1)
                baseline.submit(dict(base_job, gen=2, frame_cache=False))
                b2 = self._frames_through_settled(baseline, 2)
                for frame in (b1[-1], b2[-1]):
                    self.assertEqual(frame.get("retained_mb", 0.0), 0.0)
                    self.assertEqual(frame.get("tiles_reused", 0), 0)
                self.assertEqual(self._frame_payload(b2[-1]),
                                 first_payload)
            finally:
                baseline.stop()

            # §F2R-21 review (HIGH): a render served entirely from a
            # retained frame must not leave a query scene of a
            # DIFFERENT layer set published. All layers -> one other
            # layer -> all layers again reuses every tile and stays
            # byte-identical, and pick/snap on the first layer set must
            # still answer: the scene is republished whenever the
            # published one does not serve the request.
            worker.submit(dict(base_job, gen=200, visible=None))
            all_first = self._frames_through_settled(worker, 200)
            selected = self._assert_query_parity(cache, worker, bbox)
            other = [pair for pair in (
                (int(layer["layer"]), int(layer["datatype"]))
                for layer in cache.meta["layers"]) if pair not in selected]
            self.assertTrue(other, "the fixture needs a second layer")
            worker.submit(dict(base_job, gen=201, visible=other[:1]))
            self._frames_through_settled(worker, 201)
            worker.submit(dict(base_job, gen=202, visible=None))
            all_again = self._frames_through_settled(worker, 202)
            self.assertEqual(all_again[-1]["tiles_reused"],
                             all_again[-1]["render_tiles"])
            self.assertEqual(self._frame_payload(all_again[-1]),
                             self._frame_payload(all_first[-1]))
            self._assert_query_parity(cache, worker, bbox)
            self.assertFalse(worker._jobs)
        finally:
            worker.stop()
        self.assertFalse(worker.alive())
        self.assertEqual(worker.exitcode(), 0)
        self._assert_cli_clip(source, cache)

    def _assert_cli_clip(self, source, cache):
        import klayout.db as db

        dbu = float(cache.meta["dbu"])
        source_bbox = tuple(int(value) for value in cache.meta["bbox"])
        x1 = min(source_bbox[2], source_bbox[0] + 100_000)
        y1 = min(source_bbox[3], source_bbox[1] + 100_000)
        bbox_um = ",".join(str(value * dbu) for value in (
            source_bbox[0], source_bbox[1], x1, y1))
        layers = cache.meta["layers"][:2]
        layer_arg = ",".join("%d/%d" % (
            int(layer["layer"]), int(layer["datatype"]))
            for layer in layers)
        with tempfile.TemporaryDirectory() as directory:
            # The integration driver may place its generated VFS cache at
            # FLOE_INTEGRATION_CACHE instead of the CLI's normal hidden
            # .<source>.ice sibling (floe/cachepath.py).  Give the
            # subprocess a conventional source/cache pair while retaining
            # the same files and metadata.
            from floe.cachepath import vfs_cache_dir
            cli_source = os.path.join(directory, "CLI source.oas")
            os.symlink(source, cli_source)
            os.symlink(cache.dir, vfs_cache_dir(cli_source))
            # sitecustomize runs before `python -m floe2` and turns an
            # accidental KLayout import anywhere in the CLI startup path
            # into a hard failure.  The parent test process keeps KLayout as
            # the independent OASIS/Region oracle.
            with open(os.path.join(directory, "sitecustomize.py"),
                      "w", encoding="ascii") as startup:
                startup.write(
                    "import builtins\n"
                    "_real = builtins.__import__\n"
                    "def _guard(name, *args, **kwargs):\n"
                    "    if name == 'klayout' or "
                    "name.startswith('klayout.'):\n"
                    "        raise ImportError('KLayout unavailable')\n"
                    "    return _real(name, *args, **kwargs)\n"
                    "builtins.__import__ = _guard\n")
            child_env = os.environ.copy()
            child_env.pop("FLOE_RENDERER", None)
            child_env["PYTHONPATH"] = directory + os.pathsep + \
                child_env.get("PYTHONPATH", "")
            completed = subprocess.run(
                [sys.executable, "-B", "-m", "floe2", "info", cli_source],
                cwd=str(ROOT), env=child_env, check=True,
                stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                text=True, timeout=10)
            self.assertIn("top cell", completed.stdout)

            completed = subprocess.run(
                [sys.executable, "-B", "-m", "floe2", "probe", cli_source],
                cwd=str(ROOT), env=child_env, check=True,
                stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                text=True, timeout=30)
            self.assertIn("[probe] OK", completed.stdout)

            output = os.path.join(directory, "CLI output with spaces.oas")
            completed = subprocess.run(
                [sys.executable, "-B", "-m", "floe2", "clip", cli_source,
                 "--bbox", bbox_um, "--layers", layer_arg,
                 "--cell-name", "CLI 한글", "--out", output],
                cwd=str(ROOT), env=child_env, check=True,
                stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                text=True, timeout=30)
            self.assertIn("clip saved:", completed.stdout)
            layout = db.Layout()
            layout.read(output)
            self.assertEqual(layout.top_cell().name, "CLI 한글")
            self.assertEqual(len(list(layout.each_cell())), 1)
            self.assertTrue(any(
                layout.top_cell().shapes(li).size() > 0
                for li in layout.layer_indexes()))

            png_path = os.path.join(
                directory, "CLI rendered labels with spaces.png")
            completed = subprocess.run(
                [sys.executable, "-B", "-m", "floe2", "render",
                 cli_source, "--bbox", bbox_um, "--layers", layer_arg,
                 "--px", "257", "--depth", "999", "--labels",
                 "--label-font-px", "19", "--out", png_path],
                cwd=str(ROOT), env=child_env, check=True,
                stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                text=True, timeout=30)
            self.assertIn("rendered", completed.stdout)
            with open(png_path, "rb") as png_file:
                png = png_file.read(24)
            self.assertEqual(png[:8], b"\x89PNG\r\n\x1a\n")
            self.assertEqual(png[12:16], b"IHDR")
            self.assertEqual(int.from_bytes(png[16:20], "big"), 257)
            expected_height = max(1, round(
                257 * (y1 - source_bbox[1]) /
                max(1, x1 - source_bbox[0])))
            self.assertEqual(
                int.from_bytes(png[20:24], "big"), expected_height)
            self.assertFalse(list(Path(directory).glob(
                ".*.floe-render-*")))

    @staticmethod
    def _crop_rgba(rgba, width, x0, y0, w, h):
        """Rows [y0, y0+h) x cols [x0, x0+w) of a packed RGBA frame
        (row 0 = top), as one packed RGBA bytes object."""
        stride = width * 4
        return b"".join(
            rgba[(y0 + row) * stride + x0 * 4:
                 (y0 + row) * stride + (x0 + w) * 4]
            for row in range(h))

    def _frame_payload(self, frame):
        """Frame bytes independent of the raw/png transport default."""
        payload = frame.get("rgba")
        if payload is None:
            payload = frame.get("png")
        self.assertTrue(payload)
        return payload

    @staticmethod
    def _frames_through_settled(worker, generation,
                                reject_generations=()):
        deadline = time.monotonic() + 30.0
        frames = []
        rejected = set(reject_generations)
        while time.monotonic() < deadline:
            try:
                result = worker.res.get(timeout=0.5)
            except queue.Empty:
                continue
            if result.get("kind") == "error":
                raise AssertionError(result.get("msg"))
            if result.get("kind") != "frame":
                continue
            if result.get("gen") in rejected:
                raise AssertionError(
                    "stale generation %d published during burst" %
                    result["gen"])
            if result.get("gen") != generation:
                continue
            frames.append(result)
            if not result.get("refining"):
                return frames
        raise AssertionError("timed out waiting for generation %d" %
                             generation)

    def _assert_query_parity(self, cache, worker, bbox):
        """Use the legacy KLayout service only as a query oracle."""
        import klayout.db as db
        from floe.service import (
            _iter_global_polys,
            _svc_pick,
            _svc_snap,
        )
        from floe.vfsclient import VfsClient
        from floe.viewport import VfsMosaic

        box = db.Box(*(int(value) for value in bbox))
        cache.vfs_client = VfsClient(cache.dir)
        try:
            mosaic = VfsMosaic(cache, stream_kb=0)
            dbu = float(cache.meta["dbu"])
            view_um = tuple(float(value) * dbu for value in bbox)
            px_per_um = 400.0 / max(1e-9, view_um[2] - view_um[0])
            response = cache.vfs_client.request(
                1, view_um, px_per_um, 0.0, None, None,
                ack=0, reset=True, stream_kb=0, want_labels=False,
                lod=True, frames=False, labels=False)
            if response["names"]:
                mosaic.load_names(response["names"])
            mosaic.apply_hier(
                response["delta"], response["top"], response["evict"],
                gen=1)
            return self._assert_query_parity_with_mosaic(
                cache, worker, box, mosaic, _iter_global_polys,
                _svc_snap, _svc_pick)
        finally:
            client = cache.vfs_client
            client.stop()
            for stream in (client.proc.stdin, client.proc.stdout):
                if stream is not None:
                    stream.close()
            del cache.vfs_client

    def _assert_clip_parity(self, cache, worker, bbox):
        """Exact Rust export must be Region-identical to parent KLayout."""
        import klayout.db as db
        from floe.service import _svc_clip
        from floe.vfsclient import VfsClient

        layers = [
            (int(layer["layer"]), int(layer["datatype"]))
            for layer in cache.meta["layers"][:3]
        ]
        clip_bbox = tuple(int(value) for value in bbox)
        with tempfile.TemporaryDirectory() as directory:
            rust_j1_path = os.path.join(directory, "rust-j1.oas")
            rust_path = os.path.join(directory, "rust j8 clip output.oas")
            oracle_path = os.path.join(directory, "klayout.oas")
            original_jobs = worker._jobs_count
            try:
                worker._jobs_count = 1
                worker.submit({
                    "kind": "clip", "bbox": clip_bbox,
                    "layers": layers, "out": rust_j1_path,
                })
                self._clip_result(worker)
                worker._jobs_count = 8
                worker.submit({
                    "kind": "clip", "bbox": clip_bbox,
                    "layers": layers, "out": rust_path,
                })
                rust_result = self._clip_result(worker)
            finally:
                worker._jobs_count = original_jobs
            self.assertEqual(rust_result["path"], rust_path)
            self.assertGreater(rust_result["size_mb"], 0.0)
            with open(rust_j1_path, "rb") as j1, \
                    open(rust_path, "rb") as j8:
                self.assertEqual(j1.read(), j8.read(),
                                 "clip bytes differ for jobs=1/8")

            cache.vfs_client = VfsClient(cache.dir)
            try:
                expected_queue = queue.Queue()
                _svc_clip(cache, {
                    "kind": "clip", "bbox": clip_bbox,
                    "layers": layers, "out": oracle_path,
                }, expected_queue)
                expected_result = expected_queue.get_nowait()
                self.assertEqual(expected_result["kind"], "clip",
                                 expected_result)
            finally:
                client = cache.vfs_client
                client.stop()
                for stream in (client.proc.stdin, client.proc.stdout):
                    if stream is not None:
                        stream.close()
                del cache.vfs_client

            actual = db.Layout()
            actual.read(rust_path)
            expected = db.Layout()
            expected.read(oracle_path)
            self.assertEqual(actual.top_cell().name, "FLOE_CLIP")
            self.assertEqual(len(list(actual.each_cell())), 1)
            for layer, datatype in layers:
                actual_li = actual.find_layer(layer, datatype)
                expected_li = expected.find_layer(layer, datatype)
                actual_region = db.Region() if actual_li is None or actual_li < 0 else \
                    db.Region(actual.top_cell().begin_shapes_rec(actual_li))
                expected_region = db.Region() if expected_li is None or expected_li < 0 else \
                    db.Region(expected.top_cell().begin_shapes_rec(expected_li))
                self.assertTrue(
                    (actual_region ^ expected_region).is_empty(),
                    "clip XOR mismatch on %d/%d" % (layer, datatype))

    @staticmethod
    def _clip_result(worker):
        deadline = time.monotonic() + 30.0
        while time.monotonic() < deadline:
            try:
                result = worker.res.get(timeout=0.5)
            except queue.Empty:
                continue
            if result.get("kind") == "error":
                raise AssertionError(result.get("msg"))
            if result.get("kind") == "clip":
                return result
        raise AssertionError("timed out waiting for exact clip")

    def _assert_query_parity_with_mosaic(
            self, cache, worker, box, mosaic, iter_polys,
            svc_snap, svc_pick):
        selected = None
        anchor = None
        for poly, _text, li, _cell in iter_polys(
                mosaic, None, box):
            if poly is None:
                continue
            points = list(poly.each_point_hull())
            if points:
                info = mosaic.ly.get_info(li)
                selected = [(info.layer, info.datatype)]
                anchor = (points[0].x, points[0].y)
                break
        self.assertIsNotNone(anchor, "query oracle found no polygon")

        snap_job = {
            "kind": "snap", "seq": 700,
            "x": anchor[0], "y": anchor[1], "r": 2,
            "layers": selected,
        }
        expected_queue = queue.Queue()
        svc_snap(cache, mosaic, snap_job, expected_queue)
        expected_snap = expected_queue.get_nowait()
        worker.submit(snap_job)
        actual_snap = self._query_result(worker, "snap", 700)
        self.assertEqual(actual_snap, expected_snap)

        pick_job = {
            "kind": "pick", "seq": 701,
            "x": anchor[0], "y": anchor[1], "r": 2, "nth": 3,
            "layers": selected,
        }
        svc_pick(cache, mosaic, pick_job, expected_queue)
        expected_pick = expected_queue.get_nowait()
        worker.submit(pick_job)
        actual_pick = self._query_result(worker, "pick", 701)
        self.assertEqual(actual_pick, expected_pick)
        return selected

    @staticmethod
    def _query_result(worker, kind, sequence):
        deadline = time.monotonic() + 10.0
        while time.monotonic() < deadline:
            try:
                result = worker.res.get(timeout=0.5)
            except queue.Empty:
                continue
            if result.get("kind") == "error":
                raise AssertionError(result.get("msg"))
            if result.get("kind") == kind and result.get("seq") == sequence:
                return result
        raise AssertionError("timed out waiting for %s %d" %
                             (kind, sequence))


if __name__ == "__main__":
    unittest.main()
