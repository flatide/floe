#!/usr/bin/env python3
"""docs/CUT_DENSITY_DESIGN.ko.md §10: what a density pass fed by small
per-(cell, layer) and per-page summaries would have to look up, measured on
an existing index without writing anything (`floe-index plan --density-probe 1`).

For each view it plans the way a plain layout's `thin keep` frame does (shape
cut on, thin pages not kept, no sub-cut boxes / washes / representatives,
frames off) and counts, per INSTANCE in view, what the size cut dropped:
omitted child placements (records and members, and members x visible layers =
lookups in a per-(cell, layer) table), dropped pages (and those wider than
4 px, which one value would not place), and BVH nodes cut whole (with what
lies below them). The walk time is the cost of reaching those items through
the plan's instances - the part a density pass cannot avoid without a
flattened summary. Also printed once: the (cell, layer) pairs and pages the
summaries would hold.

    .venv/bin/python tools/probe_density_queries.py data/synthetic/main01_chip_p10.oas \\
        [--layers last10,all] [--zooms 1,2,4,8,16,64] [--depth full] [--size 1920x1080]
"""
import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def cache_dir(source):
    p = Path(source)
    if p.is_dir():
        return p
    return p.parent / ('.%s.ice' % p.name)


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument('source')
    ap.add_argument('--layers', default='last10,all')
    ap.add_argument('--zooms', default='1,2,4,8,16,64')
    ap.add_argument('--depth', default='full')
    ap.add_argument('--size', default='1920x1080')
    ap.add_argument('--cut-px', type=float, default=3.0)
    ap.add_argument('--center', help='view centre "x,y" in um (default: the layout centre)')
    ap.add_argument('--bin', default=str(ROOT / 'rust/target/release/floe-index'))
    args = ap.parse_args(argv)
    ice = cache_dir(args.source)
    meta = json.loads((ice / 'meta.json').read_text())
    dbu_um = float(meta['dbu'])
    x0, y0, x1, y1 = (v * dbu_um for v in meta['bbox'])
    cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
    if args.center:
        cx, cy = (float(v) for v in args.center.split(','))
    w, h = (int(v) for v in args.size.split('x'))
    keys = sorted((int(l['layer']), int(l['datatype'])) for l in meta['layers'])
    fit = min(w / (x1 - x0), h / (y1 - y0))          # px per um
    cols = ['plan_ms', 'walk_ms', 'walk_visits', 'pages_selected', 'child_recs', 'child_members',
            'child_layers', 'thin_members', 'cut_pages', 'wide_pages', 'wide16_pages', 'wide64_pages',
            'pbvh', 'pbvh_pages',
            'cbvh', 'cbvh_masked', 'cbvh_recs', 'cbvh_members', 'allcut_cells', 'allcut_layers']
    once = None
    rows = []
    print('view          ' + ' '.join('%13s' % c for c in cols))
    for spec in args.layers.split(','):
        chosen = keys[-10:] if spec == 'last10' else keys if spec == 'all' else keys[-int(spec):]
        for z in (float(v) for v in args.zooms.split(',')):
            px = fit * z
            hw, hh = w / 2 / px, h / 2 / px
            cmd = [args.bin, 'plan', str(ice), '--view', '%f,%f,%f,%f' % (cx - hw, cy - hh, cx + hw, cy + hh),
                   '--px-per-um', repr(px), '--cut-px', repr(args.cut_px), '--depth', args.depth,
                   '--shape-cut', '1', '--page-hairline', '0', '--frames', '0', '--density-probe', '1',
                   '--density-storage', '0' if once else '1']
            if spec != 'all':
                cmd += ['--layers', ','.join('%d/%d' % k for k in chosen)]
            out = subprocess.run(cmd, capture_output=True, text=True)
            if out.returncode != 0:
                sys.exit('floe-index failed: %s\n%s' % (' '.join(cmd), out.stderr[-2000:]))
            line = next(l for l in out.stdout.splitlines() if l.startswith('density_probe\t'))
            row = dict(f.split('=', 1) for f in line.split('\t')[1:])
            once = once or row
            rows.append((spec, z, row))
            print('%-6s x%-6g ' % (spec, z) + ' '.join('%13s' % row[c] for c in cols), flush=True)
    print('\nsummaries: cells %s, (cell, layer) pairs %s (lmask unknown %s), exact pages %s, '
          'child-BVH nodes %s (%s without a mask; (node, layer) pairs %s of the masked ones, %s counting '
          'the unmasked with their cell\'s layers), page-BVH nodes %s'
          % (once['cells'], once['cell_layer_pairs'], once['lmask_unknown'], once['exact_pages'],
             once['bvh_nodes'], once['bvh_unknown'], once['bvh_masked_pairs'], once['bvh_layer_pairs'],
             once['pbvh_nodes']))
    pairs, pages = int(once['cell_layer_pairs']), int(once['exact_pages'])
    bvh, pbvh = int(once['bvh_masked_pairs']), int(once['pbvh_nodes'])
    print('   4 bytes per (cell, layer) %.1f MB + 16 bytes (4x4) per page %.1f MB = %.1f MB; '
          'with 4 bytes per (masked child-BVH node, layer) %.1f MB and 16 per page-BVH node %.1f MB: %.1f MB'
          % (pairs * 4 / 1e6, pages * 16 / 1e6, (pairs * 4 + pages * 16) / 1e6, bvh * 4 / 1e6,
             pbvh * 16 / 1e6, (pairs * 4 + pages * 16 + bvh * 4 + pbvh * 16) / 1e6))
    # what someone on a closed network types back: short numbers (k = 1e3, M = 1e6)
    def k(row, c):
        n = int(row[c])
        return str(n) if n < 10000 else '%dk' % round(n / 1e3) if n < 10**7 else '%dM' % round(n / 1e6)
    print('\n== type this == (zoom: child recs, child members, cut pages, BVH nodes, masked, '
          'records below, all-cut cells; s: cell-layer pairs, pages, masked node-layer pairs)')
    for spec, z, row in rows:
        print('%s %g %s' % (spec[0], z, ' '.join(k(row, c) for c in (
            'child_recs', 'child_members', 'cut_pages', 'cbvh', 'cbvh_masked', 'cbvh_recs', 'allcut_cells'))))
    print('s %s %s %s' % (k(once, 'cell_layer_pairs'), k(once, 'exact_pages'), k(once, 'bvh_masked_pairs')))


if __name__ == '__main__':
    main()
