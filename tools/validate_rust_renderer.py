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

from floe import __version__  # noqa: E402
from floe.rust_render import (  # noqa: E402
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

        def forward(detail, depth):
            args = SimpleNamespace(
                src="/tmp/forwarded.oas", hairline=None, thin_um=None,
                goto="1,2,700", stream_kb=None, stream_target_ms=500,
                label_font_px=14, perf_baseline=False, lod="on",
                frames="on", labels="on", refinement="on",
                frame_cache="on", render_debug=False, multi=False,
                drc=None, detail=detail, depth=depth, dump=False,
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
        self.assertFalse(viewer.lod_on)
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
            refinement="on", frame_cache="on", render_debug=False,
            multi=True, drc=None, detail="high", depth=999, dump=False,
        )
        with mock.patch("floe.gui.run_viewer") as run_viewer:
            cli.cmd_view(args)
        options = run_viewer.call_args.kwargs
        self.assertFalse(options["lod"])
        self.assertFalse(options["frames"])
        self.assertFalse(options["labels"])
        self.assertFalse(options["frame_cache"])
        self.assertEqual(options["stream_kb"], 0)
        self.assertEqual(options["detail"], 2)
        self.assertEqual(options["depth"], 999)

    def test_refinement_off_rejects_nonzero_stable_stream_budget(self):
        from floe import cli

        args = SimpleNamespace(
            src=None, hairline=None, thin_um=None, goto=None,
            stream_kb=4096, stream_target_ms=500, label_font_px=14,
            perf_baseline=False, lod="off", frames="off", labels="off",
            refinement="off", frame_cache="off", render_debug=False,
            multi=True, drc=None, detail="high", depth=999, dump=False,
        )
        with self.assertRaisesRegex(SystemExit, "conflicts"):
            cli.cmd_view(args)

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
    dir = "/tmp/source.oas.floe"
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
        # §F2R-21: the margin is geometry only - its labels would be
        # planned for the enlarged box and could overwrite in-view
        # labels when cropped; pans re-synthesize labels instead
        self.assertFalse(job["labels"])
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
                "expected %s, got 0.1.0" % __version__,
                stale._startup_error)

            current._handle_line(
                "ready", {"version": __version__}, "")
            self.assertTrue(current._ready)
            self.assertEqual(current._renderd_version, __version__)
            self.assertIsNone(current._startup_error)
            # a bare pre-stamp ready line still yields a usable build
            self.assertEqual(current.renderd_build(), __version__)

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
                "version": __version__, "git": "abc123+",
                "flavor": "gnu"}, "")
            self.assertEqual(stamped.renderd_build(),
                             "%s abc123+ (gnu)" % __version__)
            # an unknown git hash is noise, not identity - omitted
            unstamped._handle_line("ready", {
                "version": __version__, "git": "unknown",
                "flavor": "native"}, "")
            self.assertEqual(unstamped.renderd_build(),
                             "%s (native)" % __version__)
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
            self.assertIn("frame_cache=1", commands[0])
            self.assertIn("labels=0", commands[0])
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
            })
            result = worker.res.get_nowait()
            self.assertEqual(result["kind"], "frame")
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
            self.assertEqual(partial["refining"], 1)
            self.assertIn(9, worker._jobs)
            self.assertFalse(os.path.exists(partial_path))

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
            # FLOE_INTEGRATION_CACHE instead of the CLI's normal
            # <source>.floe location.  Give the subprocess a conventional
            # source/cache pair while retaining the same files and metadata.
            cli_source = os.path.join(directory, "CLI source.oas")
            os.symlink(source, cli_source)
            os.symlink(cache.dir, cli_source + ".floe")
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
