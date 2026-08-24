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
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from floe.rust_render import (  # noqa: E402
    RustRenderWorker,
    _parse_wire_line,
    _pattern_fill,
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


class WorkerContractTests(unittest.TestCase):
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

    def test_parses_wire_fields(self):
        kind, fields = _parse_wire_line(
            "frame gen=7 png=/tmp/f.png partial=1 deferred=9")
        self.assertEqual(kind, "frame")
        self.assertEqual(fields["gen"], "7")
        self.assertEqual(fields["deferred"], "9")

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
            self.assertIn("labels=0", commands[0])
            self.assertIn("font_px=22", commands[0])
            fallback_job = dict(job, gen=8)
            fallback_job.pop("label_font_px")
            worker._submit_render(fallback_job)
            self.assertIn("font_px=18", commands[1])

            png_path = os.path.join(directory, "frame-7.png")
            with open(png_path, "wb") as frame:
                frame.write(b"\x89PNG\r\n\x1a\nfixture")
            worker._emit_frame({
                "gen": "7", "png": png_path, "partial": "0",
                "deferred": "0", "final": "1", "plan_pages": "2",
                "pages": "2", "cache_miss": "2", "plan_us": "1000",
                "read_us": "2000", "decode_us": "3000",
                "scene_us": "4000", "raster_us": "5000",
                "png_us": "6000", "publish_write_us": "7000",
                "publish_sync_us": "8000", "publish_rename_us": "9000",
                "cache_hit": "14",
                "resident_bytes": str(15 * 1024 * 1024),
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
            self.assertEqual(result["resident_mb"], 15.0)
            self.assertEqual(result["decode_workers"], 3)
            self.assertEqual(result["workers"], 4)
            self.assertEqual(result["render_tiles"], 16)
            self.assertEqual(result["tile_px"], 128)
            self.assertEqual(result["text_plan_ms"], 0.25)
            self.assertEqual(result["text_place_records"], 13)
            self.assertEqual(result["labels"], 2)
            self.assertEqual(result["label_pixel_paints"], 40)
            self.assertNotIn("labels_truncated", result)
            self.assertNotIn("drawn", result)
            self.assertNotIn("refining", result)
            self.assertFalse(os.path.exists(png_path))

            partial_job = dict(job, gen=9)
            worker._submit_render(partial_job)
            partial_path = os.path.join(
                directory, "frame-9.png.gen-9.round-1.partial.png")
            with open(partial_path, "wb") as frame:
                frame.write(b"\x89PNG\r\n\x1a\npartial")
            worker._emit_frame({
                "gen": "9", "png": partial_path, "partial": "1",
                "deferred": "0", "final": "0", "pages": "1",
            })
            partial = worker.res.get_nowait()
            self.assertEqual(partial["refining"], 1)
            self.assertIn(9, worker._jobs)
            self.assertFalse(os.path.exists(partial_path))

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
            first_png = first_frames[-1]["png"]
            self._assert_query_parity(cache, worker, bbox)
            self._assert_clip_parity(cache, worker, bbox)

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
            self.assertNotEqual(second_frames[-1]["png"], first_png)
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
            self._assert_query_parity_with_mosaic(
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
