#!/usr/bin/env python3
"""CUT_DENSITY_DESIGN §10.3 (2026-09-23): the storage of COARSE coverage planes
per zoom band - what §10.2 says a density summary has to be (a spatial grid
at zone resolution, not a value on a node) - recomputed from an index without
writing any plane.

A plane is one value per zone per layer; a band is a zoom range whose zones
project to about `--zone-px` screen pixels at its widest view. Bands here:
fit, x4, x16 (zone side = zone_px / (fit px per um x band)). What is
multiplied:

  * zones of the chip's box at that zone side (a pyramid over the band's
    octaves adds 4/3 when the same plane serves x1..x2 within the band);
  * layers: all of the index's, or only those with a record under the
    band's cut (`probe_density_queries.py --meta`, "type this 4");
  * planes per layer: 1 (the band's own cut, full depth) or §9's joint
    F[k, d] set (4.5 size planes x 6 depth planes = 27);
  * bits per zone: 1 (presence only - keeps empty space, no density), 8 or
    16 (coverage; §9 found 8-bit linear values erase 3.3 % of the non-empty
    zones);
  * sparsity: with an occupancy pyramid of the same index (`floe2 index
    --occupancy-only --occupancy-um F`, then `floe-index occupancy <ice>`),
    the set-cell share per layer per level gives the share of non-empty
    zones, and 16 x 16-zone tiles that are entirely empty cost nothing in a
    tiled layout (the estimate takes the set share as the tile share, an
    upper bound on what tiling saves).

    .venv/bin/python tools/coarse_plane_storage.py <layout.oas> [--probe-log LOG]
        [--occupancy-log LOG] [--zone-px 4] [--size 1920x1080]
"""
import argparse
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))
from floe.cachepath import vfs_cache_dir  # noqa: E402

BANDS = ((1, 'fit'), (4, 'x4'), (16, 'x16'))


def fmt(b):
    for unit, name in ((1 << 30, 'GB'), (1 << 20, 'MB'), (1 << 10, 'KB')):
        if b >= unit:
            return '%.1f %s' % (b / unit, name)
    return '%d B' % b


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument('source')
    ap.add_argument('--probe-log', help='output of probe_density_queries.py --meta (layers with sub-cut records per band)')
    ap.add_argument('--occupancy-log', help='output of floe-index occupancy <ice> (set cells per layer per level)')
    ap.add_argument('--zone-px', type=float, default=4.0)
    ap.add_argument('--size', default='1920x1080')
    args = ap.parse_args(argv)
    ice = vfs_cache_dir(args.source)
    meta = json.loads((Path(ice) / 'meta.json').read_text())
    dbu_um = float(meta['dbu'])
    x0, y0, x1, y1 = (v * dbu_um for v in meta['bbox'])
    n_layers = len(meta['layers'])
    w, h = (int(v) for v in args.size.split('x'))
    fit = min(w / (x1 - x0), h / (y1 - y0))
    # layers with sub-cut records per band ("type this 4": zoom layers records)
    layers_sub = {}
    if args.probe_log:
        block = False
        for line in Path(args.probe_log).read_text().splitlines():
            if line.startswith('== type this 4 =='):
                block = True
                continue
            if block:
                f = line.split()
                if len(f) == 3 and re.match(r'^[0-9.]+$', f[0]):
                    layers_sub[float(f[0])] = int(f[1])
                else:
                    block = False
    # occupancy: per level, the set-cell share averaged over layers (layer lines: "layer L/D ... set=a,b,c ...")
    occ_share = {}
    if args.occupancy_log:
        text = Path(args.occupancy_log).read_text()
        head = re.search(r'cell_dbu=(\d+) .*?grid=(\d+)x(\d+) levels=(\d+)', text)
        if head:
            cell_dbu, gw, gh, n_levels = (int(v) for v in head.groups())
            cell_um = cell_dbu * dbu_um
            per_level = [[] for _ in range(n_levels)]
            for m in re.finditer(r'set=([0-9,]+)', text):
                counts = [int(v) for v in m.group(1).split(',')]
                for lv, c in enumerate(counts[:n_levels]):
                    cells = (max(1, gw >> lv)) * (max(1, gh >> lv))
                    per_level[lv].append(c / cells)
            for lv in range(n_levels):
                if per_level[lv]:
                    occ_share[cell_um * (1 << lv)] = (sum(per_level[lv]) / len(per_level[lv]), len(per_level[lv]))
            built = len(re.findall(r'status=ok', text))
            print('occupancy report: %d of %d layers built (the rest hit the file size cap: none:size) - the set shares are '
                  'a mean over the built layers only' % (built, len(re.findall(r'^layer ', text, re.M))))
    print('chip %.1f x %.1f mm, %d layers, %dx%d fit %.4f px/um; zones %g px' % ((x1 - x0) / 1e3, (y1 - y0) / 1e3, n_layers, w, h, fit, args.zone_px))
    if occ_share:
        print('occupancy set-cell share by level (mean over layers): ' + ', '.join('%g um %.3f' % (s, v[0]) for s, v in sorted(occ_share.items())))
    print('%-5s %9s %12s %7s %7s | %11s %11s %11s | %11s %11s | %11s %11s' % (
        'band', 'zone um', 'zones', 'layers', 'sparse', '1 bit/plane', '8 bit/plane', '16 bit', '27 pl x 16b', '27 pl sparse',
        '1bit sparse', '8bit sparse'))
    total = {}
    for band, name in BANDS:
        px = fit * band
        zone_um = args.zone_px / px
        nz = ((x1 - x0) / zone_um) * ((y1 - y0) / zone_um) * 4 / 3   # pyramid within the band
        layers = layers_sub.get(float(band), n_layers)
        # sparsity: the occupancy level nearest the zone side
        share = None
        if occ_share:
            side = min(occ_share, key=lambda s: abs(s - zone_um))
            share = occ_share[side][0]
        one_bit = nz * layers / 8
        eight = nz * layers
        sixteen = nz * layers * 2
        planes27 = nz * layers * 27 * 2
        sparse27 = planes27 * share if share is not None else None
        sparse1 = one_bit * share if share is not None else None
        sparse8 = eight * share if share is not None else None
        print('%-5s %9.1f %12.3g %7d %7s | %11s %11s %11s | %11s %11s | %11s %11s' % (
            name, zone_um, nz, layers, ('%.3f' % share) if share is not None else '-',
            fmt(one_bit), fmt(eight), fmt(sixteen), fmt(planes27), fmt(sparse27) if sparse27 is not None else '-',
            fmt(sparse1) if sparse1 is not None else '-', fmt(sparse8) if sparse8 is not None else '-'))
        for key, val in (('1bit', one_bit), ('8bit', eight), ('16bit', sixteen), ('27x16', planes27), ('27sparse', sparse27 or 0),
                         ('1sparse', sparse1 or 0), ('8sparse', sparse8 or 0)):
            total[key] = total.get(key, 0) + val
    print('all bands: 1 bit %s, 8 bit %s, 16 bit %s, 27 planes x 16 bit %s%s' % (
        fmt(total['1bit']), fmt(total['8bit']), fmt(total['16bit']), fmt(total['27x16']),
        (', sparse: 27 planes %s, 1 bit %s, 8 bit %s' % (fmt(total['27sparse']), fmt(total['1sparse']), fmt(total['8sparse']))) if occ_share else ''))
    print('== type this == (band zones layers sparse-share)')
    for band, name in BANDS:
        px = fit * band
        zone_um = args.zone_px / px
        nz = ((x1 - x0) / zone_um) * ((y1 - y0) / zone_um) * 4 / 3
        layers = layers_sub.get(float(band), n_layers)
        share = '-'
        if occ_share:
            side = min(occ_share, key=lambda s: abs(s - zone_um))
            share = '%.3f' % occ_share[side][0]
        print('%s %.3g %d %s' % (name, nz, layers, share))


if __name__ == '__main__':
    main()
