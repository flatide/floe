"""Benchmark actual individual DRC center decoding, rasterization and delivery.

Usage: .venv/bin/python tools/bench_drc_points.py --counts 10000 1000000
       .venv/bin/python tools/bench_drc_points.py --counts 100000000

Builds real v4 packs directly: scattered blocks of four-vertex rectangles,
one distinct exact center per error. No weighted summaries or extrapolation.
Fixture creation is excluded. OS file caches are retained throughout.
"""

import argparse
import json
import math
import os
from pathlib import Path
import platform
import struct
import sys
import tempfile
import time

import numpy as np

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))


def fixture(path, count):
    from floe import drc
    blocks = (count + 63) // 64
    stride = 7919
    while math.gcd(stride, blocks) != 1:
        stride += 2
    columns = 12800
    # Fixed-length (valid non-minimal) varints make vectorized fixture
    # construction independent of coordinate magnitude. Production decoding
    # still reads every vertex of every error.
    first = bytes([8] + [128] * 4 + [0] + [128] * 4 + [0] + [6, 0, 0, 6, 5, 0])
    following = bytes([8, 16, 0, 6, 0, 0, 6, 5, 0])
    template = np.frombuffer(first + following * 63, dtype=np.uint8)
    record = len(template)
    blob_start = drc._ICE_HEADER.size
    blob_length = blocks * record - (blocks * 64 - count) * len(following)
    qbox_start = blob_start + blob_length
    status_start = qbox_start + count * 4
    waived_start = status_start + count
    block_start = waived_start + 4
    directory_start = block_start + blocks * 48
    strings_start = directory_start + drc._ICE2_CHECK.size
    strings = struct.pack('<I', 5) + b'BENCH' + struct.pack('<I', 5) + b'RECTS'
    body_end = strings_start + len(strings)
    with open(path, 'wb') as stream:
        stream.truncate(body_end + drc._ICE2_FOOTER.size)
        stream.write(drc._ICE_HEADER.pack(drc._ICE_MAGIC, 4, 1, 1000., 0, 0))
    data = np.memmap(path, mode='r+', dtype=np.uint8)
    block_dtype = np.dtype([('off', '<u8'), ('cnt', '<u4'), ('pad', '<u4'),
                           ('x0', '<i8'), ('y0', '<i8'), ('x1', '<i8'), ('y1', '<i8')])
    table = np.ndarray((blocks,), dtype=block_dtype, buffer=data, offset=block_start)
    x_max = y_max = 0
    for start in range(0, blocks, 4096):
        ids = np.arange(start, min(start + 4096, blocks), dtype=np.int64)
        locations = (ids * stride % blocks) * 64
        ends = locations % columns * 8 + 63 * 8 + 3
        if ids[-1] == blocks - 1:
            ends[-1] -= (blocks * 64 - count) * 8
        x_max = max(x_max, int(ends.max()))
        y_max = max(y_max, int((locations // columns * 8 + 3).max()))
    for start in range(0, blocks, 4096):
        stop = min(start + 4096, blocks)
        ids = np.arange(start, stop, dtype=np.int64)
        locations = (ids * stride % blocks) * 64
        x, y = (locations % columns) * 8, (locations // columns) * 8
        encoded = np.tile(template, (len(ids), 1))
        for coordinate, offset in ((x, 1), (y, 6)):
            for byte in range(5):
                encoded[:, offset + byte] = ((coordinate * 2 >> (7 * byte)) & 127) | (128 if byte < 4 else 0)
        last = min(blob_start + stop * record, qbox_start)
        data[blob_start + start * record:last] = encoded.ravel()[:last - blob_start - start * record]
        rows = table[start:stop]
        rows['off'], rows['cnt'] = blob_start + ids * record, 64
        rows['x0'], rows['y0'], rows['x1'], rows['y1'] = x, y, x + 63 * 8 + 3, y + 3
        if stop == blocks:
            rows['cnt'][-1] = count - (blocks - 1) * 64
            rows['x1'][-1] = x[-1] + (int(rows['cnt'][-1]) - 1) * 8 + 3
        # Accurate outward qboxes, even though point queries use exact
        # center coordinates rather than this coarse index.
        xs = (x[:, None] + np.arange(64) * 8).ravel()
        ys = np.repeat(y, 64)
        n = min(len(xs), count - start * 64)
        q = np.empty((n, 4), dtype=np.uint8)
        q[:, 0], q[:, 2] = xs[:n] * 255 // x_max, ((xs[:n] + 3) * 255 + x_max - 1) // x_max
        q[:, 1], q[:, 3] = ys[:n] * 255 // y_max, ((ys[:n] + 3) * 255 + y_max - 1) // y_max
        data[qbox_start + start * 256:qbox_start + start * 256 + n * 4] = q.ravel()
    directory = drc._ICE2_CHECK.pack(9, 0, 0, 0, 0, count, count, count, 0, blocks)
    data[directory_start:strings_start] = np.frombuffer(directory, dtype=np.uint8)
    data[strings_start:body_end] = np.frombuffer(strings, dtype=np.uint8)
    footer = drc._ICE2_FOOTER.pack(blob_start, blob_length, qbox_start, count * 4,
        status_start, waived_start, block_start, blocks, directory_start, 1,
        strings_start, 0, strings_start, len(strings), count, 0, 0, drc._ICE_MAGIC)
    data[body_end:] = np.frombuffer(footer, dtype=np.uint8)
    data.flush()
    del rows, table, data
    return (0., 0., (x_max + 1) / 1000, (y_max + 1) / 1000)


def run(args):
    from floe.drc import IcePack
    from floe.drc_markers import MarkerIndex
    from floe.drc_marker_worker import MarkerWorker
    from floe import drc_points
    print(json.dumps(dict(platform=platform.platform(), machine=platform.machine(),
                          python=platform.python_version(), logical_cpus=os.cpu_count(),
                          resolution=[args.width, args.height], jobs=args.jobs,
                          fixture='actual distinct four-vertex rectangles, scattered blocks',
                          note='No extrapolation; OS caches retained; fixture construction excluded.')), flush=True)
    os.environ['FLOE_DRC_JOBS'] = str(args.jobs)
    for count in args.counts:
        with tempfile.TemporaryDirectory(prefix='floe-points-benchmark-') as temporary:
            folder = Path(temporary)
            os.environ['FLOE_DRC_ANALYSIS_ROOT'] = str(folder / 'analysis')
            pack = folder / 'data.tray'
            began = time.perf_counter()
            bounds = fixture(pack, count)
            generation = time.perf_counter() - began
            db = IcePack(str(pack), review=False)
            try:
                if args.progressive:
                    progressive_run(db, bounds, args, generation, pack.stat().st_size)
                    continue
                index = MarkerIndex(db)
                began = time.perf_counter()
                centers = drc_points.prepare_rule(index, 0)
                cold = time.perf_counter() - began
                del centers
                query = dict(bounds_um=bounds, width_px=args.width, height_px=args.height,
                             checks=(0,))
                query_seconds = []
                for _ in range(2):
                    began = time.perf_counter()
                    result = index.query(**query)
                    query_seconds.append(time.perf_counter() - began)
                    assert result.visible_count == count, (result.visible_count, count)
                    assert result.occupied_count <= args.width * args.height
                value = dict(count=count, fixture_seconds=generation,
                             pack_bytes=pack.stat().st_size, center_cache_bytes=count * 16,
                             center_build_seconds=cold, query_seconds=query_seconds,
                             occupied_pixels=result.occupied_count,
                             result_bytes=len(result.rgba) + result.error_ids.nbytes + result.check_ids.nbytes)
                import gi
                gi.require_version('GdkPixbuf', '2.0')
                from gi.repository import GdkPixbuf, GLib
                disp = GdkPixbuf.Pixbuf.new(GdkPixbuf.Colorspace.RGB, False, 8, args.width, args.height)
                began = time.perf_counter()
                layer = GdkPixbuf.Pixbuf.new_from_bytes(GLib.Bytes.new(result.rgba),
                    GdkPixbuf.Colorspace.RGB, True, 8, args.width, args.height, args.width * 4)
                layer.composite(disp, 0, 0, args.width, args.height, 0, 0, 1, 1,
                                GdkPixbuf.InterpType.NEAREST, 255)
                value['gtk_layer_and_composite_seconds'] = time.perf_counter() - began
                if args.worker:
                    worker = MarkerWorker()
                    try:
                        began = time.perf_counter()
                        worker.submit('benchmark', db, query)
                        ticks = 0
                        max_tick = 0.
                        last = began
                        while True:
                            reply = worker.poll()
                            now = time.perf_counter()
                            max_tick = max(max_tick, now - last)
                            last = now
                            if reply is not None:
                                break
                            ticks += 1
                            time.sleep(.01)
                        assert reply[2] is None, reply[2]
                        assert reply[1].visible_count == count
                        value.update(worker_seconds=time.perf_counter() - began,
                                     foreground_heartbeats=ticks, max_heartbeat_gap_seconds=max_tick)
                    finally:
                        worker.close()
                        worker._thread.join(timeout=10)
                print(json.dumps(value), flush=True)
            finally:
                db.close()


def progressive_run(db, bounds, args, generation, pack_bytes):
    """Exercise the real worker/IPC path and composite every received frame."""
    import gi
    gi.require_version('GdkPixbuf', '2.0')
    from gi.repository import GdkPixbuf, GLib
    from floe.drc_marker_worker import MarkerProgress, MarkerWorker
    worker = MarkerWorker()
    disp = GdkPixbuf.Pixbuf.new(GdkPixbuf.Colorspace.RGB, False, 8, args.width, args.height)
    query = dict(bounds_um=bounds, width_px=args.width, height_px=args.height,
                 checks=(0,), progressive=True)
    try:
        for phase in ('cold', 'warm'):
            began = last_tick = time.perf_counter()
            worker.submit(phase, db, query)
            frames = []
            ticks = 0
            max_tick = 0.
            previous_processed = 0
            while True:
                reply = worker.poll()
                now = time.perf_counter()
                max_tick = max(max_tick, now - last_tick)
                last_tick = now
                if reply is None:
                    ticks += 1
                    time.sleep(.005)
                    continue
                key, result, error = reply
                assert key == phase and error is None, (key, error)
                partial = isinstance(result, MarkerProgress)
                points = result.markers if partial else result
                assert previous_processed <= points.processed_count <= db.total
                previous_processed = points.processed_count
                layer = GdkPixbuf.Pixbuf.new_from_bytes(GLib.Bytes.new(points.rgba),
                    GdkPixbuf.Colorspace.RGB, True, 8, args.width, args.height, args.width * 4)
                layer.composite(disp, 0, 0, args.width, args.height, 0, 0, 1, 1,
                                GdkPixbuf.InterpType.NEAREST, 255)
                frames.append(dict(seconds=time.perf_counter() - began,
                                   partial=partial, processed=points.processed_count,
                                   visible=points.visible_count, occupied=points.occupied_count))
                if not partial:
                    assert points.visible_count == db.total
                    assert points.processed_count == points.total_count == db.total
                    break
            print(json.dumps(dict(count=db.total, phase=phase, frames=frames,
                                  fixture_seconds=generation, pack_bytes=pack_bytes,
                                  worker_seconds=time.perf_counter() - began,
                                  foreground_heartbeats=ticks, max_heartbeat_gap_seconds=max_tick)), flush=True)
    finally:
        worker.close()
        worker._thread.join(timeout=10)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--counts', type=int, nargs='+', default=[10000, 1000000])
    parser.add_argument('--width', type=int, default=1024)
    parser.add_argument('--height', type=int, default=768)
    parser.add_argument('--jobs', type=int, default=4)
    parser.add_argument('--worker', action='store_true')
    parser.add_argument('--progressive', action='store_true',
                        help='measure cold/warm worker frames with actual 0.5s previews')
    args = parser.parse_args()
    if any(count <= 0 for count in args.counts) or min(args.width, args.height, args.jobs) < 1:
        parser.error('counts, dimensions and jobs must be positive')
    run(args)
