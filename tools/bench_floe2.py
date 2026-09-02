#!/usr/bin/env python3
"""Persistent-session floe2 renderer benchmark for an existing VFS cache.

The JSON report deliberately omits source names, layer identities, cell names,
and coordinates so it can be attached to performance reports without exposing
design-specific data.
"""

import argparse
import json
import math
import os
from pathlib import Path
import platform
import queue
import statistics
import subprocess
import sys
import tempfile
import threading
import time


ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

# Product/backend selection happens in main() before any worker spawns:
# --backend rust imports floe2 (Rust-only product), --backend klayout keeps
# the stable floe product so the same trace drives the KLayout service.
from floe import __version__  # noqa: E402
from floe.cache import Cache  # noqa: E402
from floe.service import DEFAULT_DETAIL, DETAIL_LEVELS, DETAIL_PX  # noqa: E402


PHASE_FIELDS = (
    "ms", "plan_ms", "read_ms", "decode_ms", "scene_ms", "raster_ms",
    "frame_format",
    "png_ms", "publish_write_ms", "publish_sync_ms", "publish_rename_ms",
    "publish_ms", "adapter_read_ms", "text_plan_ms", "cache_hit",
    "cache_miss", "frame_cache_hit", "resident_mb",
    "decode_workers", "workers", "raster_jobs", "render_tiles", "tile_px",
    "frame_width", "frame_height", "tiles",
    "wc_cells", "inst_edges", "frame_rects", "text_place_records",
    "labels", "label_tile_paints", "label_pixel_paints",
    "work_bin_defer_rep", "work_bin_defer_single", "work_bin_defer_wmax",
    "member_paints",
    # F2R diagnostics: refinement rounds, decode pool utilization and
    # stragglers, record-index build share, raster tail, traversal size
    "rounds", "decode_sum_ms", "decode_max_ms", "index_ms",
    "raster_tile_max_ms", "mask_mb", "work_bin_items",
    "work_bin_overflow_items", "rep_members_tested",
    "rep_members_drawn",
    "hier_cells_visited", "subtrees_pruned",
    # stable floe/KLayout service phases (absent from Rust results)
    "load_ms", "phase_plan", "phase_delta", "phase_apply", "draw_ms",
    "wait_ms", "new",
)


def positive_int(value):
    parsed = int(value)
    if parsed < 1:
        raise argparse.ArgumentTypeError("must be at least 1")
    return parsed


def parse_jobs(value):
    try:
        jobs = [int(item.strip()) for item in value.split(",")
                if item.strip()]
    except ValueError as exc:
        raise argparse.ArgumentTypeError(
            "jobs must be a comma-separated integer list") from exc
    if not jobs or any(item < 1 or item > 256 for item in jobs):
        raise argparse.ArgumentTypeError("jobs must be in 1..256")
    return list(dict.fromkeys(jobs))


def parse_hotspot(value):
    try:
        fields = [float(item.strip()) for item in value.split(",")]
    except ValueError as exc:
        raise argparse.ArgumentTypeError(
            "hotspot must be X_UM,Y_UM[,SPAN_UM]") from exc
    if len(fields) not in (2, 3) or not all(map(math.isfinite, fields)):
        raise argparse.ArgumentTypeError(
            "hotspot must be X_UM,Y_UM[,SPAN_UM]")
    if len(fields) == 3 and fields[2] <= 0:
        raise argparse.ArgumentTypeError("hotspot span must be positive")
    return fields


def fit_aspect(bbox, aspect):
    x0, y0, x1, y1 = map(float, bbox)
    width, height = x1 - x0, y1 - y0
    if width <= 0 or height <= 0:
        raise ValueError("cache bbox must have positive width and height")
    cx, cy = (x0 + x1) / 2.0, (y0 + y1) / 2.0
    if width / height < aspect:
        width = height * aspect
    else:
        height = width / aspect
    return (cx - width / 2.0, cy - height / 2.0,
            cx + width / 2.0, cy + height / 2.0)


def centered_window(full_bbox, fraction, aspect, center=None):
    x0, y0, x1, y1 = map(float, full_bbox)
    if center is None:
        center = ((x0 + x1) / 2.0, (y0 + y1) / 2.0)
    width = max(1e-9, (x1 - x0) * fraction)
    height = max(1e-9, (y1 - y0) * fraction)
    cx, cy = center
    return fit_aspect((cx - width / 2.0, cy - height / 2.0,
                       cx + width / 2.0, cy + height / 2.0), aspect)


def pan_windows(mid_bbox):
    x0, y0, x1, y1 = mid_bbox
    width, height = x1 - x0, y1 - y0
    cx, cy = (x0 + x1) / 2.0, (y0 + y1) / 2.0
    windows = []
    for step in (-0.25, -0.125, 0.0, 0.125, 0.25):
        shifted = cx + step * width
        windows.append((shifted - width / 2.0, cy - height / 2.0,
                        shifted + width / 2.0, cy + height / 2.0))
    return windows


def shifted_window(bbox, fraction, axis):
    x0, y0, x1, y1 = bbox
    dx = (x1 - x0) * fraction if axis == "x" else 0.0
    dy = (y1 - y0) * fraction if axis == "y" else 0.0
    return (x0 + dx, y0 + dy, x1 + dx, y1 + dy)


def build_trace(meta, width, height, hotspot, layer_spec, cache):
    full = tuple(float(value) for value in meta["bbox"])
    aspect = width / height
    fit = fit_aspect(full, aspect)
    mid = centered_window(full, 1.0 / 8.0, aspect)
    dbu = float(meta["dbu"])
    if hotspot is None:
        hot_center = None
        hot_fraction = 1.0 / 32.0
        hot = centered_window(full, hot_fraction, aspect)
        near = centered_window(full, 1.0 / 128.0, aspect)
    else:
        hot_center = (hotspot[0] / dbu, hotspot[1] / dbu)
        if len(hotspot) == 3:
            span_dbu = hotspot[2] / dbu
            hot = fit_aspect((hot_center[0] - span_dbu / 2.0,
                              hot_center[1] - span_dbu / 2.0,
                              hot_center[0] + span_dbu / 2.0,
                              hot_center[1] + span_dbu / 2.0), aspect)
            near_span = span_dbu / 4.0
            near = fit_aspect((hot_center[0] - near_span / 2.0,
                               hot_center[1] - near_span / 2.0,
                               hot_center[0] + near_span / 2.0,
                               hot_center[1] + near_span / 2.0), aspect)
        else:
            hot = centered_window(full, 1.0 / 32.0, aspect, hot_center)
            near = centered_window(full, 1.0 / 128.0, aspect, hot_center)

    if layer_spec:
        selected = cache.resolve_layers(layer_spec)
        if not selected:
            raise ValueError("--layer did not select a layer")
        single_layer = [selected[0]]
        layer_mode = "explicit"
    else:
        layers = meta.get("layers") or []
        if not layers:
            raise ValueError("cache contains no layers")
        chosen = max(layers, key=lambda item: int(
            item.get("stored_shapes", 0)))
        single_layer = [(int(chosen["layer"]), int(chosen["datatype"]))]
        layer_mode = "auto-densest"

    trace = [
        ("fit", fit, 0, None),
        ("mid_first", mid, None, None),
        ("hotspot", hot, None, None),
        ("single_layer_near", near, None, single_layer),
        ("hotspot_revisit", hot, None, None),
    ]
    trace.extend(("warm_pan_%d" % (index + 1), bbox, None, None)
                 for index, bbox in enumerate(pan_windows(mid)))
    return trace, layer_mode, hot


def pan_sweep_trace(hot):
    """F2R-10 fixed-scale adjacency sweep: shift the settled hotspot by a
    fraction of its width/height and return, at the same zoom/detail."""
    trace = []
    for fraction, tag in ((1.0 / 16.0, "16"), (1.0 / 8.0, "8"),
                          (1.0 / 4.0, "4")):
        for axis in ("x", "y"):
            trace.append(("hot_pan_%s_%s" % (axis, tag),
                          shifted_window(hot, fraction, axis), None, None))
            trace.append(("hot_pan_%s_%s_back" % (axis, tag),
                          hot, None, None))
    return trace


def read_rss_kb(pid):
    status = Path("/proc") / str(pid) / "status"
    try:
        for line in status.read_text(encoding="ascii").splitlines():
            if line.startswith("VmRSS:"):
                return int(line.split()[1])
    except (FileNotFoundError, OSError, ValueError):
        pass
    try:
        completed = subprocess.run(
            ["ps", "-o", "rss=", "-p", str(pid)],
            capture_output=True, text=True, timeout=2)
        return int(completed.stdout.strip()) if completed.returncode == 0 else 0
    except (OSError, subprocess.TimeoutExpired, ValueError):
        return 0


class RssMonitor:
    def __init__(self, pid):
        self.pid = pid
        self.peak_kb = 0
        self._stop = threading.Event()
        self._thread = threading.Thread(
            target=self._run, name="floe2-benchmark-rss", daemon=True)

    def start(self):
        self._thread.start()

    def _run(self):
        while not self._stop.is_set():
            self.peak_kb = max(self.peak_kb, read_rss_kb(self.pid))
            self._stop.wait(0.1)
        self.peak_kb = max(self.peak_kb, read_rss_kb(self.pid))

    def stop(self):
        self._stop.set()
        self._thread.join(timeout=3)


def start_worker(worker, timeout):
    if not hasattr(worker, "start_async"):
        # The KLayout RenderWorker opens its cache inside the spawned
        # process; the first frame therefore includes the cold open.
        worker.start()
        if worker._proc is None or worker._proc.pid is None:
            raise RuntimeError("render worker process failed to start")
        monitor = RssMonitor(worker._proc.pid)
        monitor.start()
        return monitor
    done = threading.Event()
    errors = []

    def callback(error):
        if error is not None:
            errors.append(error)
        done.set()

    worker.start_async(callback)
    deadline = time.monotonic() + timeout
    while worker._proc is None and not done.is_set():
        if time.monotonic() >= deadline:
            raise TimeoutError("floe-renderd process startup timed out")
        time.sleep(0.01)
    monitor = None
    if worker._proc is not None:
        monitor = RssMonitor(worker._proc.pid)
        monitor.start()
    if not done.wait(max(0.0, deadline - time.monotonic())):
        if monitor is not None:
            monitor.stop()
        raise TimeoutError("floe-renderd cache open timed out")
    if errors:
        if monitor is not None:
            monitor.stop()
        raise RuntimeError("floe-renderd startup failed: %s" % errors[0])
    return monitor


def wait_frame(worker, generation, timeout):
    deadline = time.monotonic() + timeout
    while True:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise TimeoutError("render generation %d timed out" % generation)
        try:
            result = worker.res.get(timeout=min(remaining, 1.0))
        except queue.Empty:
            if not worker.alive():
                raise RuntimeError("floe-renderd exited during benchmark")
            continue
        if result.get("kind") == "error":
            raise RuntimeError(result.get("msg", "render failed"))
        if result.get("kind") != "frame" or result.get("gen") != generation:
            continue
        if result.get("preview") or result.get("bg") or \
                result.get("refining"):
            continue
        rgba = result.pop("rgba", None)
        if rgba is not None:
            expected = int(result.get("frame_width", 0)) * \
                int(result.get("frame_height", 0)) * 4
            if len(rgba) != expected:
                raise RuntimeError("renderer returned a truncated raw frame")
            return result
        png = result.pop("png", b"")
        if not png.startswith(b"\x89PNG\r\n\x1a\n"):
            raise RuntimeError("renderer returned an invalid PNG")
        return result


def render_trace(worker, trace, args, run_number):
    results = []
    detail = DETAIL_LEVELS.index(args.detail)
    decorations = not args.perf_baseline
    frame_cache = args.frame_cache == "on" and not args.perf_baseline
    for generation, (name, bbox, depth, visible) in enumerate(trace, 1):
        worker.submit({
            "kind": "render", "gen": generation, "scope": "benchmark",
            "bbox": tuple(float(value) for value in bbox), "view": None,
            "w": args.width, "h": args.height, "depth": depth,
            "cut_px": DETAIL_PX[detail], "lod": False,
            "frames": decorations, "labels": decorations,
            "label_font_px": 14, "frame_cache": frame_cache,
            "abstract": False, "visible": visible,
        })
        result = wait_frame(worker, generation, args.timeout)
        metrics = {field: result[field] for field in PHASE_FIELDS
                   if field in result}
        metrics.update({
            "scenario": name,
            "run": run_number,
            "visibility": "single" if visible is not None else "all",
            "depth": "fit" if depth == 0 else "full",
        })
        results.append(metrics)
        if hasattr(worker, "_raster_jobs_count"):
            print("jobs=r%d/d%-3d run=%d %-18s total=%8.1f  "
                  "plan=%7.1f read=%7.1f decode=%7.1f scene=%7.1f "
                  "raster=%7.1f png=%7.1f publish=%7.1f handoff=%6.1f" % (
                      worker._raster_jobs_count, worker._jobs_count,
                      run_number, name,
                      float(metrics.get("ms", 0)),
                      float(metrics.get("plan_ms", 0)),
                      float(metrics.get("read_ms", 0)),
                      float(metrics.get("decode_ms", 0)),
                      float(metrics.get("scene_ms", 0)),
                      float(metrics.get("raster_ms", 0)),
                      float(metrics.get("png_ms", 0)),
                      float(metrics.get("publish_ms", 0)),
                      float(metrics.get("adapter_read_ms", 0))), flush=True)
        else:
            print("klayout    run=%d %-18s total=%8.1f  "
                  "load=%7.1f plan=%7.1f delta=%7.1f apply=%7.1f "
                  "draw=%7.1f" % (
                      run_number, name,
                      float(metrics.get("ms", 0)),
                      float(metrics.get("load_ms", 0)),
                      float(metrics.get("phase_plan", 0)),
                      float(metrics.get("phase_delta", 0)),
                      float(metrics.get("phase_apply", 0)),
                      float(metrics.get("draw_ms", 0))), flush=True)
    return results


def benchmark_session(cache, trace, args, jobs, run_number):
    # `--jobs` sweeps the raster workers. Without --decode-jobs the decode
    # workers follow the same value (the legacy scaling experiment); with
    # --decode-jobs they stay pinned so raster scaling and the F2R-12
    # serial profile are measured against a fixed page-load configuration.
    decode_jobs = args.decode_jobs if args.decode_jobs else jobs
    if args.backend == "klayout":
        # The stable service runs its pinned single C++ raster; the jobs
        # sweep only labels the session.
        from floe.service import make_render_worker
        worker = make_render_worker(cache)
    else:
        os.environ["FLOE_RUST_JOBS"] = str(decode_jobs)
        os.environ["FLOE_RUST_RASTER_JOBS"] = str(jobs)
        os.environ["FLOE_RUST_BUDGET_MB"] = str(args.budget_mb)
        os.environ["FLOE_RUST_ROUND_PAGES"] = str(args.round_pages)
        os.environ["FLOE_RUST_TILE_PX"] = str(args.tile_px)
        os.environ["FLOE_RUST_OPEN_TIMEOUT_S"] = str(args.timeout)
        if args.renderd:
            os.environ["FLOE_RENDERD_BIN"] = os.path.abspath(args.renderd)
        from floe.rust_render import RustRenderWorker
        worker = RustRenderWorker(cache)
    monitor = None
    try:
        monitor = start_worker(worker, args.timeout)
        results = render_trace(worker, trace, args, run_number)
        if monitor is not None:
            monitor.stop()
        peak_mb = monitor.peak_kb / 1024.0 if monitor is not None else 0.0
        monitor = None
        resident_mb = max((float(row.get("resident_mb", 0.0))
                           for row in results), default=0.0)
        return {
            "jobs": jobs,
            "decode_jobs": decode_jobs,
            "run": run_number,
            "rss_peak_mb": round(peak_mb, 3),
            "resident_peak_mb": round(resident_mb, 3),
            "results": results,
        }
    finally:
        if monitor is not None:
            monitor.stop()
        worker.stop()


def median_by_scenario(sessions, jobs, field):
    values = {}
    for session in sessions:
        if session["jobs"] != jobs:
            continue
        for result in session["results"]:
            value = float(result.get(field, 0.0))
            values.setdefault(result["scenario"], []).append(value)
    return {name: statistics.median(items)
            for name, items in values.items()}


def scaling_summary(sessions, job_counts):
    if 1 not in job_counts:
        return []
    baseline_total = median_by_scenario(sessions, 1, "ms")
    baseline_raster = median_by_scenario(sessions, 1, "raster_ms")
    summary = []
    for jobs in job_counts:
        totals = median_by_scenario(sessions, jobs, "ms")
        rasters = median_by_scenario(sessions, jobs, "raster_ms")
        for scenario in sorted(baseline_total):
            total = totals.get(scenario, 0.0)
            raster = rasters.get(scenario, 0.0)
            summary.append({
                "jobs": jobs,
                "scenario": scenario,
                "total_speedup_vs_1": (round(baseline_total[scenario] / total, 3)
                                       if total > 0 else None),
                "raster_speedup_vs_1": (
                    round(baseline_raster[scenario] / raster, 3)
                    if raster > 0 else None),
            })
    return summary


def atomic_write_json(path, report):
    destination = Path(path).resolve()
    destination.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=".%s.tmp-" % destination.name, dir=destination.parent)
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as output:
            json.dump(report, output, indent=2, sort_keys=True)
            output.write("\n")
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, destination)
    finally:
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass


def make_parser():
    parser = argparse.ArgumentParser(
        description="Measure persistent floe2 render phases and peak RSS")
    parser.add_argument("source", help="indexed OASIS source")
    parser.add_argument(
        "--hotspot", type=parse_hotspot, metavar="X_UM,Y_UM[,SPAN_UM]",
        help="private hotspot center and optional square span in micrometers")
    parser.add_argument(
        "--layer", help="single layer name or L/D for the near-view case")
    parser.add_argument(
        "--backend", choices=("rust", "klayout"), default="rust",
        help="rust drives the floe2 renderd adapter; klayout drives the "
        "stable floe render service headlessly over the same trace")
    parser.add_argument("--jobs", type=parse_jobs, default=parse_jobs("1,4,8,16"),
                        help="raster worker sweep; decode workers follow "
                        "unless --decode-jobs pins them")
    parser.add_argument("--decode-jobs", type=positive_int,
                        help="pin page-decode workers for every session "
                        "while --jobs sweeps raster workers only")
    parser.add_argument("--serial", action="store_true",
                        help="F2R-12 serial profile: raster jobs 1, decode "
                        "jobs 8 (unless --decode-jobs), one image tile "
                        "covering the framebuffer (unless --tile-px)")
    parser.add_argument("--pan-sweep", action="store_true",
                        help="append the F2R-10 hotspot adjacency sweep: "
                        "1/16, 1/8, 1/4 width/height shifts with returns "
                        "at the same zoom")
    parser.add_argument("--frame-cache", choices=("on", "off"), default="on",
                        help="exact-viewport PNG LRU; 'off' keeps warm "
                        "revisits on the full raster path")
    parser.add_argument("--perf-baseline", action="store_true",
                        help="backend-neutral measurement surface: frames, "
                        "labels, and the exact frame cache off (LOD is "
                        "always off here)")
    parser.add_argument("--runs", type=positive_int, default=1)
    parser.add_argument("--width", type=positive_int, default=1200)
    parser.add_argument("--height", type=positive_int, default=800)
    parser.add_argument(
        "--detail", choices=DETAIL_LEVELS,
        default=DETAIL_LEVELS[DEFAULT_DETAIL])
    parser.add_argument("--budget-mb", type=positive_int, default=1024)
    parser.add_argument("--round-pages", type=positive_int, default=1 << 30)
    parser.add_argument("--tile-px", type=positive_int,
                        help="image tile size (default 128; --serial "
                        "defaults to max(width, height))")
    parser.add_argument("--timeout", type=positive_int, default=600)
    parser.add_argument("--renderd", help="explicit floe-renderd binary")
    parser.add_argument("--out", help="write privacy-safe JSON report here")
    return parser


def main(argv=None):
    args = make_parser().parse_args(argv)
    if args.backend == "klayout":
        # Stay on the stable floe product and force the KLayout service;
        # both are inherited by the spawned render process.
        os.environ["FLOE_PRODUCT"] = "floe"
        os.environ["FLOE_RENDERER"] = "klayout"
    else:
        import floe2  # noqa: F401 - select the Rust-only product
    if args.serial:
        args.jobs = [1]
        if args.decode_jobs is None:
            args.decode_jobs = 8
        if args.tile_px is None:
            # One tile must cover the framebuffer so the serial baseline
            # pays no cross-tile traversal; renderd caps tiles at 4096px.
            args.tile_px = min(4096, max(args.width, args.height))
    if args.tile_px is None:
        args.tile_px = 128
    source = os.path.abspath(args.source)
    cache = Cache(source)
    if not cache.exists():
        raise SystemExit("floe2 benchmark: no VFS cache; run floe2 index first")
    cache.load()
    if cache.is_stale():
        raise SystemExit("floe2 benchmark: VFS cache is stale; rebuild it")
    try:
        trace, layer_mode, hot = build_trace(
            cache.meta, args.width, args.height, args.hotspot,
            args.layer, cache)
        if args.pan_sweep:
            trace.extend(pan_sweep_trace(hot))
    except (KeyError, TypeError, ValueError) as exc:
        raise SystemExit("floe2 benchmark: %s" % exc) from exc

    sessions = []
    started = time.time()
    try:
        for jobs in args.jobs:
            for run_number in range(1, args.runs + 1):
                sessions.append(benchmark_session(
                    cache, trace, args, jobs, run_number))
    except (OSError, RuntimeError, TimeoutError, ValueError) as exc:
        raise SystemExit("floe2 benchmark failed: %s" % exc) from exc

    report = {
        "schema": "floe2-render-benchmark-v1",
        "product": "floe2",
        "version": __version__,
        "created_unix": round(started),
        "privacy": "source names, coordinates, layer identities, and cell names omitted",
        "host": {
            "system": platform.system(),
            "machine": platform.machine(),
            "logical_cpus": os.cpu_count(),
        },
        "config": {
            "backend": args.backend,
            "jobs": args.jobs,
            "decode_jobs": args.decode_jobs,
            "serial": args.serial,
            "frame_cache": args.frame_cache,
            "perf_baseline": args.perf_baseline,
            "runs": args.runs,
            "viewport": [args.width, args.height],
            "budget_mb": args.budget_mb,
            "round_pages": args.round_pages,
            "detail": args.detail,
            "tile_px": args.tile_px,
            "hotspot_supplied": args.hotspot is not None,
            "single_layer_selection": layer_mode,
            "trace": [entry[0] for entry in trace],
        },
        "sessions": sessions,
        "scaling": scaling_summary(sessions, args.jobs),
    }
    if args.out:
        atomic_write_json(args.out, report)
        print("privacy-safe report: %s" % os.path.abspath(args.out))
    else:
        print(json.dumps(report, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
