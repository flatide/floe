#!/usr/bin/env python3
"""CUT_DENSITY_DESIGN §10.6 (2026-09-24): what keeping the HAIRLINES costs under
`thin keep`, detail medium (cut 3 px), on an existing index.

Today's per-shape cut (SPEC-PLANNER, 0.12.173) drops every record whose
SMALLER side is under the cut - a long thin wire goes with the specks. With
the width-first drawing (RENDERER-TESTS §3) a wire under a pixel wide is not
"all or nothing" any more: it is kept with the chance its width fills a
pixel, so hairlines could stay in the plan and thin themselves out. What
that costs is the question: the pages holding them are read and decoded and
their records walked, and the screen may fill with 1 px lines.

Three modes of renderd's FLOE_RUST_SHAPE_CUT:
  * (unset)  today: pages cut by max_min < cut, records by min side < cut;
  * max      pages cut only when both max sides are under the cut (the
             pre-0.12.173 page rule), records only when their LARGER side is
             under the cut - hairlines reach the width-first painter;
  * off      the pre-0.12.173 rule: pages by both max sides, no per-record
             cut at all (every record of a kept page is drawn).

Per view (all layers, fit / x4 / x16, thin keep, cut 3 px, 1920 x 1080,
centred): the second (warm) frame's wall and draw time, pages read, rectangle
members drawn, the share of lit pixels, and the plan's cut counters (shape
cut, pages cut by size, thin pages kept, the fit budget's percentage / over /
thin level). An occupancy summary in the index (design.ovo) replaces the
pages at wide keep views - move it aside for a page-path measurement.

    .venv/bin/python tools/bench_hairline_cut.py <layout.oas> [--zooms 1,4,16] [--size 1920x1080]
"""
import argparse
import json
import os
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))
from floe.cache import Cache  # noqa: E402
from floe.cachepath import vfs_cache_dir  # noqa: E402
from floe.rust_render import RustRenderWorker  # noqa: E402

BLACK = bytes((0, 0, 0, 255))


def frame(w, gen, bbox_dbu, size, cut_px):
    w.submit({'kind': 'render', 'gen': gen, 'scope': 'headless', 'bbox': bbox_dbu, 'view': None,
              'w': size[0], 'h': size[1], 'depth': None, 'cut_px': cut_px, 'lod': False, 'frames': False,
              'labels': False, 'abstract': False, 'visible': None, 'frame_format': 'raw',
              'thin': 'keep', 'frame_cache': False})
    t0 = time.monotonic()
    deadline = t0 + 600
    while time.monotonic() < deadline:
        res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
        assert res.get('kind') != 'error', res
        if res.get('kind') == 'frame' and res.get('gen') == gen and not res.get('refining'):
            pixels = bytes(res.pop('rgba'))
            return pixels, res, time.monotonic() - t0
    raise AssertionError('frame timeout')


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument('source')
    ap.add_argument('--zooms', default='1,4,16')
    ap.add_argument('--size', default='1920x1080')
    ap.add_argument('--cut-px', type=float, default=3.0)
    ap.add_argument('--modes', default='current,max,off')
    ap.add_argument('--budget-mb', type=int, help='FLOE_RUST_BUDGET_MB for the workers (raise it to keep the fit budget out of the comparison)')
    ap.add_argument('--culls', action='store_true', help='print every non-zero plan counter per frame')
    args = ap.parse_args(argv)
    os.environ.setdefault('FLOE_RUST_RETAINED_MB', '0')
    if args.budget_mb:
        os.environ['FLOE_RUST_BUDGET_MB'] = str(args.budget_mb)
    ice = vfs_cache_dir(args.source)
    meta = json.loads((Path(ice) / 'meta.json').read_text())
    x0, y0, x1, y1 = meta['bbox']
    cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
    w, h = (int(v) for v in args.size.split('x'))
    fit = min(w / (x1 - x0), h / (y1 - y0))          # px per dbu
    zooms = [float(v) for v in args.zooms.split(',')]
    rows = []
    for mode in args.modes.split(','):
        if mode == 'current':
            os.environ.pop('FLOE_RUST_SHAPE_CUT', None)
        else:
            os.environ['FLOE_RUST_SHAPE_CUT'] = mode
        cache = Cache(args.source)
        cache.load()
        worker = RustRenderWorker(cache)
        worker.start()
        try:
            gen = 0
            for z in zooms:
                px = fit * z
                hw, hh = w / 2 / px, h / 2 / px
                bbox = (cx - hw, cy - hh, cx + hw, cy + hh)
                gen += 1
                frame(worker, gen, bbox, (w, h), args.cut_px)          # cold
                gen += 1
                pixels, res, wall = frame(worker, gen, bbox, (w, h), args.cut_px)   # warm
                lit = sum(1 for i in range(0, len(pixels), 4) if pixels[i:i + 4] != BLACK) / (w * h)
                row = dict(mode=mode, zoom=z, wall_ms=wall * 1000, draw_ms=res.get('draw_ms'), decode_ms=res.get('decode_ms'),
                           plan_ms=res.get('plan_ms'), tiles=res.get('tiles'), members=res.get('rep_members_drawn'),
                           tested=res.get('rep_members_tested'),
                           lit=lit, culls=res.get('plan_culls', {}))
                rows.append(row)
                c = row['culls']
                print('%-8s x%-4g wall %7.0f ms (plan %6.0f decode %6.0f draw %6.0f) pages %6s members %10s / tested %10s lit %.3f | '
                      'shape_cut %s pages_size %s thin_pages %s fit %s%% over %s thin %s'
                      % (mode, z, row['wall_ms'], row['plan_ms'] or 0, row['decode_ms'] or 0, row['draw_ms'] or 0,
                         row['tiles'], row['members'], row['tested'], lit, c.get('shape_cut'), c.get('pages_size'), c.get('thin_pages'),
                         c.get('fit_pct'), c.get('fit_over'), c.get('fit_thin')), flush=True)
                if args.culls:
                    print('   culls: ' + ' '.join('%s=%s' % kv for kv in sorted(c.items()) if kv[1]), flush=True)
        finally:
            worker.stop()
    os.environ.pop('FLOE_RUST_SHAPE_CUT', None)
    print('== type this == (mode zoom wall-ms pages members tested lit fit%)')
    for r in rows:
        print('%s %g %.0f %s %s %s %.3f %s' % (r['mode'][0], r['zoom'], r['wall_ms'], r['tiles'], r['members'], r['tested'], r['lit'],
                                              r['culls'].get('fit_pct')))


if __name__ == '__main__':
    main()
