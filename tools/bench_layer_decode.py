#!/usr/bin/env python3
"""Layer-decode probe benchmark (docs/LAYER_DECODE_PROBE_PLAN.ko.md §9).

Replays one request through `render_probe` in every mode and reports what
each one cost, so the order's own price (baseline -> ordered) and the decode
it saves (ordered -> occlusion) stay apart. Frames are compared byte for byte:
a mode whose pixels differ is a failure, not a faster number.

Two rules the numbers depend on (review 2026-09-20):

  * cold means a worker that has rendered nothing. Every cold measurement
    gets a worker of its own, so a view never inherits the page cache of the
    view before it, and `--repeat` repeats the COLD measurement (a fresh
    worker each time) while `--warm` adds re-runs on the last of them. The
    order of the modes is rotated between repeats, so a mode is not always
    the one that finds the OS file cache warm. Reported: the median and the
    range - differences of a few ms are not readable from one run.
  * the raster is `prepare_ms + paint_ms` in every mode. The normal path
    collects the work bin and builds the tiles inside its one render call, so
    it reports that as paint with prepare 0; a session splits them. Comparing
    the paint columns alone compares different things. What a layered run
    spends on reading (`decode_ms`), on asking what to read (`demand_ms`) and
    on the decode pool itself (`pool_ms`) is taken out of its paint.

    .venv/bin/python tools/bench_layer_decode.py <cache-or-oas> [options]
      --modes baseline,ordered:1,ordered:4   --layers 16,449 or all
      --zooms 1,4,8                 --depth full|0|N
      --repeat 3   --warm 1        --thin keep|cull
      --cut-px 3.0                  --size 1920x1080
      --json <path>

A mode is `baseline` or `ordered[:BLOCK]` / `occlusion[:BLOCK]`, where BLOCK
is how many consecutive layers a raster worker paints into a tile before the
workers meet and the driver looks at the masks again (1 = stop at every
layer). Pair an `occlusion:N` with the `ordered:N` of the same block size:
only that pair separates the decode it skips from what the stops cost. A
BLOCK past the layer count is one block: the tiles then run in the normal
render's order, which is the floor of what the stops can cost (the metadata
scene and the session are still there, so it is not the normal render).

On a real chip (MAIN01, MAIN09) the run to report is, per representative view:

    .venv/bin/python tools/bench_layer_decode.py <cache> \
        --modes baseline,ordered:8,occlusion:8,occlusion:1000 \
        --layers <all> --zooms 1,4,8 --depth full --repeat 3 --warm 2 \
        --center <x,y in um> --json main01-full.json

and the same with `--depth 0`. What matters is whether the pages the coverage
leaves out survive the real shared instances and deferred arrays: the JSON's
`demand_unsure` (deferred edges, whose layer is read whole) and
`demand_occluded` against `selected_pages` say that directly.
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
    ap.add_argument('--modes', default='baseline,ordered:1')
    ap.add_argument('--layers', default='16,449')
    ap.add_argument('--zooms', default='1,4,8')
    ap.add_argument('--depth', default='full')
    ap.add_argument('--thin', default='keep', choices=('keep', 'cull'))
    ap.add_argument('--cut-px', type=float, default=3.0)
    ap.add_argument('--size', default='1920x1080')
    ap.add_argument('--repeat', type=int, default=3)
    ap.add_argument('--warm', type=int, default=1)
    ap.add_argument('--center', help='view centre "x,y" in um (default: the layout centre)')
    ap.add_argument('--json')
    args = ap.parse_args(argv)
    specs, args.modes = args.modes.split(','), []
    for spec in specs:
        mode, _, block = spec.partition(':')
        if mode not in ('baseline', 'ordered', 'occlusion'):
            ap.error('unknown probe mode: %s' % spec)
        args.modes.append((spec, mode, int(block) if block else 1))
    # 'all' = every layer of the source, resolved once the cache is open
    args.layers = [v if v == 'all' else int(v) for v in args.layers.split(',')]
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


def run(worker, gen, args, bbox, keys, mode, block):
    job = {'kind': 'render_probe', 'mode': mode, 'block': block, 'gen': gen, 'scope': 'headless', 'bbox': bbox,
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
    # the whole raster, comparable across modes (see the module docstring)
    probe['raster_ms'] = probe['prepare_ms'] + probe['paint_ms']
    probe['other_ms'] = probe['scene_ms'] + probe['pool_ms']
    return hashlib.sha256(pixels).hexdigest(), probe


def main(argv=None):
    args = parse_args(argv or sys.argv[1:])
    os.environ.setdefault('FLOE_INDEX_BIN', str(ROOT / 'rust/target/release/floe-index'))
    os.environ.setdefault('FLOE_RENDERD_BIN', str(ROOT / 'rust/target/release/floe-renderd'))
    source_cache = Cache(args.source)
    source_cache.load()
    meta = source_cache.meta
    dbu = float(meta['dbu'])
    x0, y0, x1, y1 = (value * dbu for value in meta['bbox'])
    cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
    if args.center:
        cx, cy = (float(value) for value in args.center.split(','))
    every = [(int(layer['layer']), int(layer['datatype'])) for layer in meta['layers']]
    cases = []
    for count in args.layers:
        if count == 'all':
            count, keys = len(every), list(every)
        else:
            keys = every[:: max(1, len(every) // count)][:count]
        for zoom in args.zooms:
            scale = max((x1 - x0) / args.width, (y1 - y0) / args.height) / zoom
            box = (cx - scale * args.width / 2, cy - scale * args.height / 2,
                   cx + scale * args.width / 2, cy + scale * args.height / 2)
            cases.append({'layers': count, 'zoom': zoom, 'keys': keys,
                          'bbox': tuple(value / dbu for value in box)})
    # the first process launch also pays for the binary and the OS file cache
    # of the index; that is not what "cold" is about here (plan §9)
    warmup = make_worker(args.source)
    try:
        run(warmup, 0, args, cases[0]['bbox'], cases[0]['keys'], 'baseline', 1)
    finally:
        warmup.stop()
    rows = []
    gen = 0
    for case in cases:
        digests, cold_runs, warm_runs = {}, {}, {}
        for repeat in range(args.repeat):
            # rotate the modes so one of them is not always first
            order = args.modes[repeat % len(args.modes):] + args.modes[:repeat % len(args.modes)]
            for spec, mode, block in order:
                # a worker of its own: cold is a worker that has rendered nothing
                worker = make_worker(args.source)
                try:
                    gen += 1
                    digest, probe = run(worker, gen, args, case['bbox'], case['keys'], mode, block)
                    digests.setdefault(spec, set()).add(digest)
                    cold_runs.setdefault(spec, []).append(dict(probe, repeat=repeat))
                    if repeat + 1 == args.repeat:
                        for warm in range(args.warm):
                            gen += 1
                            digest, probe = run(worker, gen, args, case['bbox'],
                                                case['keys'], mode, block)
                            digests[spec].add(digest)
                            warm_runs.setdefault(spec, []).append(dict(probe, warm=warm))
                finally:
                    worker.stop()
        shapes = {digest for values in digests.values() for digest in values}
        if len(shapes) != 1:
            raise SystemExit('modes disagree on the pixels: layers %d zoom x%g: %s'
                             % (case['layers'], case['zoom'], digests))
        row = {'layers': case['layers'], 'zoom': case['zoom'], 'digest': shapes.pop(),
               'modes': {}}
        for spec, cold in cold_runs.items():
            warm = warm_runs.get(spec, [])
            mid = lambda items, key: statistics.median(item[key] for item in items)
            span = lambda items, key: [min(item[key] for item in items),
                                       max(item[key] for item in items)]
            row['modes'][spec] = {
                # the median cold run, and the spread of the repeats
                'cold': min(cold, key=lambda item: abs(item['wall_ms'] - mid(cold, 'wall_ms'))),
                'cold_ms': mid(cold, 'wall_ms'),
                'cold_range_ms': span(cold, 'wall_ms'),
                'cold_decode_ms': mid(cold, 'decode_ms'),
                'cold_raster_ms': mid(cold, 'raster_ms'),
                'cold_demand_ms': mid(cold, 'demand_ms'),
            'cold_other_ms': mid(cold, 'other_ms'),
                'repeats': len(cold),
                'warm_ms': mid(warm, 'wall_ms') if warm else None,
                'warm_range_ms': span(warm, 'wall_ms') if warm else None,
                'warm_decode_ms': mid(warm, 'decode_ms') if warm else None,
                'warm_raster_ms': mid(warm, 'raster_ms') if warm else None,
            }
        rows.append(row)
        for spec, values in row['modes'].items():
            cold = values['cold']
            print('layers %-4d zoom x%-5g %-12s | cold %5.0f ms [%.0f-%.0f] decode %5.0f '
                  'raster %5.0f demand %4.1f other %4.1f | pages %5d/%-5d mem %5.1f MB skipped %5.1f MB src'
                  % (case['layers'], case['zoom'], spec, values['cold_ms'],
                     values['cold_range_ms'][0], values['cold_range_ms'][1],
                     values['cold_decode_ms'], values['cold_raster_ms'], values['cold_demand_ms'],
                     values['cold_other_ms'],
                     cold['decoded_pages'], cold['selected_pages'],
                     cold['decoded_bytes'] / 1e6, cold['skipped_bytes'] / 1e6), flush=True)
    report = {
        'source': str(Path(args.source).resolve()),
        'renderd': RENDERD_VERSION,
        'request': {'width': args.width, 'height': args.height, 'cut_px': args.cut_px,
                    'thin': args.thin, 'depth': args.depth, 'repeat': args.repeat,
                    'warm': args.warm, 'frames': False, 'labels': False},
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
