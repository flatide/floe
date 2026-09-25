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

Modes of renderd's FLOE_RUST_SHAPE_CUT (`current` = the variable unset):
  * (unset)  the default: since 0.12.214 (user decision 2026-09-25) the same
             as max; before it the same as min;
  * min      pages cut by max_min < cut, records by min side < cut (the
             default 0.12.173..0.12.213);
  * max      pages, child cells and child-BVH subtrees cut only when both
             sides are under the cut (the pre-0.12.173 page rule; since the
             review of 2026-09-25 also for a thin child cell), records only
             when their LARGER side is under the cut - hairlines reach the
             width-first painter;
  * off      the pre-0.12.173 rule: pages by both max sides, no per-record
             cut at all (every record of a kept page is drawn).

Per mode, layer set (--layers: all, lastN / firstN of the layers sorted by
layer/datatype - the probe's `last10`) and view (fit x zoom, thin keep, cut
3 px, 1920 x 1080, centred or --center): a FRESH worker draws the view once
(cold: nothing decoded yet - the OS file cache is not dropped) and then
--repeat times more (warm: the median and the range). Each frame reports its
wall (submit to result) and renderd's own split - plan, page read, decode,
scene, raster - and pages read, work-bin items, hierarchy cells visited,
repetition members tested / drawn, member paints, items the write-once mask
skipped, the lit share and the plan's cut counters - and, under the
diagnostic placement lattice (FLOE_RUST_PLACE_LATTICE=on, CUT_DENSITY_DESIGN
§10.8), the placement arrays' survivor walks by outcome (`place walks`:
walks / visible members per outcome, 1 / 2 = 1-D / 2-D arrays; `== type this
2 ==` keeps the three with the most members). The raster time is the
whole raster stage: the work bin, the record and member walk, the transforms,
the survival test and the pixel writes together, so a raster difference says
the stage costs more, not which of them does.

The occupancy summary is kept out with FLOE_RUST_OCCUPANCY=off (a plain
layout draws none by default anyway) and every frame is checked for
summary.layers == 0.

    .venv/bin/python tools/bench_hairline_cut.py <layout.oas> [--modes min,max] [--layers last10,all]
        [--zooms 1,4,16] [--repeat 3] [--center X,Y (um)] [--budget-mb MB]
"""
import argparse
import json
import os
import statistics
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))
from floe.cache import Cache  # noqa: E402
from floe.cachepath import vfs_cache_dir  # noqa: E402
from floe.rust_render import RustRenderWorker  # noqa: E402

BLACK = bytes((0, 0, 0, 255))
# one letter a mode on the `type this` lines (min and max share an initial)
MODE_CODE = {'current': 'd', 'min': 'n', 'max': 'x', 'off': 'o'}


def frame(w, gen, bbox_dbu, size, cut_px, visible):
    w.submit({'kind': 'render', 'gen': gen, 'scope': 'headless', 'bbox': bbox_dbu, 'view': None,
              'w': size[0], 'h': size[1], 'depth': None, 'cut_px': cut_px, 'lod': False, 'frames': False,
              'labels': False, 'abstract': False, 'visible': visible, 'frame_format': 'raw',
              'thin': 'keep', 'frame_cache': False})
    t0 = time.monotonic()
    deadline = t0 + 900
    while time.monotonic() < deadline:
        res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
        assert res.get('kind') != 'error', res
        if res.get('kind') == 'frame' and res.get('gen') == gen and not res.get('refining'):
            pixels = bytes(res.pop('rgba'))
            assert res.get('summary', {}).get('layers', 0) == 0, 'the occupancy summary drew: %s' % res.get('summary')
            return pixels, res, (time.monotonic() - t0) * 1000
    raise AssertionError('frame timeout')


def layer_sets(spec, keys):
    for name in spec.split(','):
        if name == 'all':
            yield name, None
        elif name.startswith('last'):
            yield name, keys[-int(name[4:]):]
        elif name.startswith('first'):
            yield name, keys[:int(name[5:])]
        else:
            raise SystemExit('--layers: all, lastN or firstN, not %r' % name)


def split(res):
    return {k: res.get(k) or 0 for k in ('plan_ms', 'read_ms', 'decode_ms', 'scene_ms', 'raster_ms')}


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument('source')
    ap.add_argument('--zooms', default='1,4,16')
    ap.add_argument('--layers', default='last10,all')
    ap.add_argument('--size', default='1920x1080')
    ap.add_argument('--cut-px', type=float, default=3.0)
    ap.add_argument('--modes', default='min,max')
    ap.add_argument('--repeat', type=int, default=3, help='warm frames after the cold one (default 3)')
    ap.add_argument('--center', help='view centre in um (default: the layout centre)')
    ap.add_argument('--budget-mb', type=int, help='FLOE_RUST_BUDGET_MB for the workers (raise it to keep the fit budget out of the comparison)')
    ap.add_argument('--culls', action='store_true', help='print every non-zero plan counter per frame')
    args = ap.parse_args(argv)
    os.environ.setdefault('FLOE_RUST_RETAINED_MB', '0')
    os.environ['FLOE_RUST_OCCUPANCY'] = 'off'
    if args.budget_mb:
        os.environ['FLOE_RUST_BUDGET_MB'] = str(args.budget_mb)
    ice = vfs_cache_dir(args.source)
    meta = json.loads((Path(ice) / 'meta.json').read_text())
    keys = sorted((int(l['layer']), int(l['datatype'])) for l in meta['layers'])
    x0, y0, x1, y1 = meta['bbox']
    cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
    if args.center:
        cx, cy = (float(v) / float(meta['dbu']) for v in args.center.split(','))
    w, h = (int(v) for v in args.size.split('x'))
    fit = min(w / (x1 - x0), h / (y1 - y0))          # px per dbu
    zooms = [float(v) for v in args.zooms.split(',')]
    rows = []
    for mode in args.modes.split(','):
        if mode == 'current':
            os.environ.pop('FLOE_RUST_SHAPE_CUT', None)
        else:
            os.environ['FLOE_RUST_SHAPE_CUT'] = mode
        for lname, visible in layer_sets(args.layers, keys):
            for z in zooms:
                px = fit * z
                hw, hh = w / 2 / px, h / 2 / px
                bbox = (cx - hw, cy - hh, cx + hw, cy + hh)
                cache = Cache(args.source)
                cache.load()
                worker = RustRenderWorker(cache)
                worker.start()
                try:
                    pixels, cold, cold_wall = frame(worker, 1, bbox, (w, h), args.cut_px, visible)
                    warm = [frame(worker, 2 + k, bbox, (w, h), args.cut_px, visible) for k in range(args.repeat)]
                finally:
                    worker.stop()
                if warm:
                    pixels, res = warm[-1][0], warm[-1][1]
                else:
                    res = cold
                lit = sum(1 for i in range(0, len(pixels), 4) if pixels[i:i + 4] != BLACK) / (w * h)
                walls = [f[2] for f in warm]
                rasters = [f[1].get('raster_ms') or 0 for f in warm]
                row = dict(mode=mode, layers=lname, zoom=z, cold_wall=cold_wall, cold=split(cold),
                           cold_miss=cold.get('cache_miss'),
                           warm_wall=statistics.median(walls) if walls else None, warm_range=(min(walls), max(walls)) if walls else None,
                           warm_raster=statistics.median(rasters) if rasters else None,
                           warm_raster_range=(min(rasters), max(rasters)) if rasters else None,
                           warm=split(res), warm_hit=res.get('cache_hit'), pages=res.get('tiles'),
                           bin_items=res.get('work_bin_items'), tested=res.get('rep_members_tested'),
                           drawn=res.get('rep_members_drawn'), paints=res.get('member_paints'),
                           skipped=res.get('once_items_skipped'), cells=res.get('hier_cells_visited'),
                           walks=res.get('place_walks') or {}, lit=lit, culls=res.get('plan_culls', {}))
                rows.append(row)
                c, cs = row['culls'], row['cold']
                print('%-7s %-7s x%-4g cold %8.0f ms (plan %6.0f read %6.0f decode %6.0f scene %5.0f raster %7.0f; miss %s) | '
                      'warm %8s ms [%s] raster %7s [%s] | pages %6s bin %8s cells %8s members %10s / tested %10s paints %10s '
                      'once-skipped %8s lit %.3f | shape_cut %s pages_size %s child_bvh %s children_size %s fit %s%%'
                      % (mode, lname, z, cold_wall, cs['plan_ms'], cs['read_ms'], cs['decode_ms'], cs['scene_ms'], cs['raster_ms'], row['cold_miss'],
                         '%.0f' % row['warm_wall'] if walls else '-', '%.0f..%.0f' % row['warm_range'] if walls else '-',
                         '%.0f' % row['warm_raster'] if walls else '-', '%.0f..%.0f' % row['warm_raster_range'] if walls else '-',
                         row['pages'], row['bin_items'], row['cells'], row['drawn'], row['tested'], row['paints'], row['skipped'], lit,
                         c.get('shape_cut'), c.get('pages_size'), c.get('child_bvh'), c.get('children_size'), c.get('fit_pct')), flush=True)
                if args.culls:
                    print('   culls: ' + ' '.join('%s=%s' % kv for kv in sorted(c.items()) if kv[1]), flush=True)
                if row['walks']:
                    # FLOE_RUST_PLACE_LATTICE=on: the placement arrays' survivor
                    # walks by outcome (walks / visible members; 1 / 2 = 1-D / 2-D)
                    print('   place walks: ' + ' '.join('%s %d/%d' % (k, v[0], v[1]) for k, v in
                                                        sorted(row['walks'].items(), key=lambda kv: -kv[1][1])), flush=True)
    os.environ.pop('FLOE_RUST_SHAPE_CUT', None)

    def k(v):
        return '-' if v is None else '%.0fk' % (v / 1000) if v >= 10000 else '%d' % v
    print('== type this == (mode d/n/x/o = default/min/max/off, layers zoom | cold wall decode raster | warm wall raster lo-hi | pages bin tested drawn lit fit%)')
    for r in rows:
        print('%s %s %g | %.0f %.0f %.0f | %s %s %s | %s %s %s %s %.3f %s' % (
            MODE_CODE.get(r['mode'], r['mode'][:1]), r['layers'], r['zoom'], r['cold_wall'], r['cold']['decode_ms'] + r['cold']['read_ms'], r['cold']['raster_ms'],
            '%.0f' % r['warm_wall'] if r['warm_wall'] is not None else '-',
            '%.0f' % r['warm_raster'] if r['warm_raster'] is not None else '-',
            '%.0f-%.0f' % r['warm_raster_range'] if r['warm_raster_range'] else '-',
            r['pages'], k(r['bin_items']), k(r['tested']), k(r['drawn']), r['lit'], r['culls'].get('fit_pct')))
    walked = [r for r in rows if r['walks']]
    if walked:
        print('== type this 2 == (placement walks, the 3 outcomes with the most members: outcome walks/members)')
        for r in walked:
            top = sorted(r['walks'].items(), key=lambda kv: -kv[1][1])[:3]
            print('%s %s %g | %s' % (MODE_CODE.get(r['mode'], r['mode'][:1]), r['layers'], r['zoom'], ' '.join('%s %s/%s' % (n, k(v[0]), k(v[1])) for n, v in top)))


if __name__ == '__main__':
    main()
