#!/usr/bin/env python3
"""Layer-decode probe benchmark (docs/LAYER_DECODE_PROBE_PLAN.ko.md §9).

Replays one request through `render_probe` in every mode and reports what
each one cost, so the order's own price (baseline -> ordered) and the decode
it saves (ordered -> occlusion) stay apart. Each mode gets a worker of its
own, the modes are measured in turn so none of them rides another's warm page
cache, and each measurement is repeated - the median is reported with its
range. Frames are compared byte for byte: a mode whose pixels differ is a
failure, not a faster number.

    .venv/bin/python tools/bench_layer_decode.py <cache-or-oas> [options]
      --modes baseline,ordered      --layers 1,16,64,449
      --zooms 1,4,8                 --depth full|0|N
      --repeat 2                    --thin keep|cull
      --cut-px 3.0                  --size 1920x1080
      --json <path>
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import statistics
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from floe.cache import Cache
from floe.rust_render import RustRenderWorker
from floe import RENDERD_VERSION


def parse_args(argv):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument('source')
    ap.add_argument('--modes', default='baseline,ordered')
    ap.add_argument('--layers', default='16,449')
    ap.add_argument('--zooms', default='1,4,8')
    ap.add_argument('--depth', default='full')
    ap.add_argument('--thin', default='keep', choices=('keep', 'cull'))
    ap.add_argument('--cut-px', type=float, default=3.0)
    ap.add_argument('--size', default='1920x1080')
    ap.add_argument('--repeat', type=int, default=2)
    ap.add_argument('--json')
    args = ap.parse_args(argv)
    args.modes = args.modes.split(',')
    args.layers = [int(v) for v in args.layers.split(',')]
    args.zooms = [float(v) for v in args.zooms.split(',')]
    args.width, args.height = (int(v) for v in args.size.split('x'))
    args.depth = None if args.depth == 'full' else int(args.depth)
    return args


def make_worker(source):
    cache = Cache(source)
    cache.load()
    worker = RustRenderWorker(cache)
    worker.start()
    return worker


def run(worker, gen, args, bbox, keys, mode):
    job = {'kind': 'render_probe', 'mode': mode, 'gen': gen, 'scope': 'headless', 'bbox': bbox,
           'view': None, 'w': args.width, 'h': args.height, 'depth': args.depth,
           'cut_px': args.cut_px, 'lod': False, 'frames': False, 'labels': False,
           'abstract': False, 'visible': keys, 'frame_format': 'raw', 'thin': args.thin,
           'frame_cache': False}
    started = time.monotonic()
    worker.submit(job)
    while True:
        res = worker.res.get(timeout=3600)
        if res.get('kind') == 'error':
            raise RuntimeError(res.get('msg'))
        if res.get('kind') == 'probe_frame' and res.get('gen') == gen:
            break
    wall = time.monotonic() - started
    pixels = bytes(res.pop('rgba'))
    probe = dict(res['probe'])
    probe['wall_ms'] = wall * 1000.0
    return hashlib.sha256(pixels).hexdigest(), probe


def main(argv=None):
    args = parse_args(argv or sys.argv[1:])
    os.environ.setdefault('FLOE_INDEX_BIN', str(ROOT / 'rust/target/release/floe-index'))
    os.environ.setdefault('FLOE_RENDERD_BIN', str(ROOT / 'rust/target/release/floe-renderd'))
    probe_cache = Cache(args.source)
    probe_cache.load()
    meta = probe_cache.meta
    dbu = float(meta['dbu'])
    x0, y0, x1, y1 = (value * dbu for value in meta['bbox'])
    cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
    every = [(int(layer['layer']), int(layer['datatype'])) for layer in meta['layers']]
    cases = []
    for count in args.layers:
        keys = every[:: max(1, len(every) // count)][:count]
        for zoom in args.zooms:
            scale = max((x1 - x0) / args.width, (y1 - y0) / args.height) / zoom
            box = (cx - scale * args.width / 2, cy - scale * args.height / 2,
                   cx + scale * args.width / 2, cy + scale * args.height / 2)
            cases.append({'layers': count, 'zoom': zoom, 'keys': keys,
                          'bbox': tuple(value / dbu for value in box)})
    # one worker per mode so a mode never inherits another's warm page cache;
    # every repeat after the first is a warm measurement of the same worker
    workers = {mode: make_worker(args.source) for mode in args.modes}
    rows = []
    gen = 0
    try:
        for case in cases:
            digests, runs = {}, {}
            for repeat in range(args.repeat):
                for mode in args.modes:
                    gen += 1
                    digest, probe = run(workers[mode], gen, args, case['bbox'], case['keys'], mode)
                    digests.setdefault(mode, set()).add(digest)
                    runs.setdefault(mode, []).append(dict(probe, repeat=repeat))
            shapes = {digest for values in digests.values() for digest in values}
            if len(shapes) != 1:
                raise SystemExit('modes disagree on the pixels: layers %d zoom x%g: %s'
                                 % (case['layers'], case['zoom'], digests))
            row = {'layers': case['layers'], 'zoom': case['zoom'], 'digest': shapes.pop(),
                   'modes': {}}
            for mode, values in runs.items():
                cold, warm = values[0], values[1:]
                pick = lambda key, items: statistics.median(item[key] for item in items)
                row['modes'][mode] = {
                    'cold': cold,
                    'warm_ms': pick('wall_ms', warm) if warm else None,
                    'warm_range_ms': [min(item['wall_ms'] for item in warm),
                                      max(item['wall_ms'] for item in warm)] if warm else None,
                    'warm_decode_ms': pick('decode_ms', warm) if warm else None,
                    'warm_paint_ms': pick('paint_ms', warm) if warm else None,
                }
            rows.append(row)
            print('layers %-4d zoom x%-4g | %s' % (
                case['layers'], case['zoom'],
                '  '.join('%s cold %.0f ms (decode %.0f, paint %.0f, %d/%d pages) warm %s'
                          % (mode, values['cold']['wall_ms'], values['cold']['decode_ms'],
                             values['cold']['paint_ms'], values['cold']['decoded_pages'],
                             values['cold']['selected_pages'],
                             '-' if values['warm_ms'] is None else '%.0f ms' % values['warm_ms'])
                          for mode, values in row['modes'].items())), flush=True)
    finally:
        for worker in workers.values():
            worker.stop()
    report = {
        'source': str(Path(args.source).resolve()),
        'renderd': RENDERD_VERSION,
        'request': {'width': args.width, 'height': args.height, 'cut_px': args.cut_px,
                    'thin': args.thin, 'depth': args.depth, 'repeat': args.repeat,
                    'frames': False, 'labels': False},
        'env': {name: value for name, value in sorted(os.environ.items())
                if name.startswith('FLOE_')},
        'rows': rows,
    }
    if args.json:
        Path(args.json).write_text(json.dumps(report, indent=1))
        print('wrote', args.json)
    return report


if __name__ == '__main__':
    main()
