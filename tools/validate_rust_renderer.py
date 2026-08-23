#!/usr/bin/env python3
"""Validate the in-tree Rust render worker contract and real daemon bridge."""

import os
import queue
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

    def test_render_command_and_frame_result_match_parent_schema(self):
        with tempfile.TemporaryDirectory() as directory:
            binary = os.path.join(directory, "floe-renderd")
            with open(binary, "w", encoding="ascii") as script:
                script.write("#!/bin/sh\n")
            os.chmod(binary, 0o755)
            with mock.patch.dict(os.environ, {
                "FLOE_RENDERD_BIN": binary,
                "FLOE_RUST_JOBS": "4",
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
            self.assertEqual(result["text_plan_ms"], 0.25)
            self.assertEqual(result["text_place_records"], 13)
            self.assertEqual(result["labels"], 2)
            self.assertEqual(result["label_pixel_paints"], 40)
            self.assertNotIn("labels_truncated", result)
            self.assertNotIn("drawn", result)
            self.assertNotIn("refining", result)
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
                "coverage": False,
            }
            worker.submit(base_job)
            first_frames = self._frames_through_settled(worker, 1)
            self.assertGreaterEqual(len(first_frames), 2)
            self.assertTrue(first_frames[0].get("refining"))
            self.assertNotIn("refining", first_frames[-1])
            self.assertGreater(first_frames[-1]["tiles"], 4)
            first_png = first_frames[-1]["png"]
            self._assert_query_parity(cache, worker, bbox)

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
                base_job, gen=2, depth=3, frames=True, labels=True)
            worker.submit(second_job)
            second_frames = self._frames_through_settled(worker, 2)
            self.assertNotEqual(second_frames[-1]["png"], first_png)
            self.assertIn("labels", second_frames[-1])
            self.assertNotIn("labels_truncated", second_frames[-1])
            if second_frames[-1]["labels"]:
                self.assertGreater(
                    second_frames[-1]["label_pixel_paints"], 0)

            # Model a pan/zoom burst: every request advances the strict
            # generation frontier before the previous expensive frame can
            # settle.  Only the latest generation may publish a frame.
            burst_first = 3
            burst_last = 102
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
