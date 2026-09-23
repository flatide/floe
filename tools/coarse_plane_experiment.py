#!/usr/bin/env python3
"""ADAPTIVE_CUT_DENSITY_PLAN §6 C on the small hierarchical layout of
tools/density_planes_experiment.py (CUT_DENSITY_DESIGN §10.4, 2026-09-23):
a COARSE coverage plane per zoom octave, stored and read back, judged for
what §10.2 and §10.3 left open - empty space, cut-boundary brightness, bytes.

The layout (21,384 rects, depth 0..4, a cross-shaped empty corridor) is
rasterised at 1 DBU = 1 px, so every area is exact. Views are the sweep of
density_planes_experiment (s px/DBU from 1 down to 1/64, 16 steps an octave,
cut 3 px, so c_req = ceil(3 / s) DBU). An OCTAVE band b holds the views with
s in [2^-(b+1), 2^-b): its plane has zones of 4 px at the band's widest view
(2^(b+2) DBU) and holds the coverage of the shapes under the band's widest cut
c_b = 3 x 2^b DBU (full depth) - one plane per band, the band's own cut, as
§10.3 priced it. A view inside the band draws the originals with min side
>= c_b (the cut quantized UP to the band's widest, cheaper than continuous)
and the plane's zones where the originals did not paint.

Values: presence bit (coverage > 0) plus an 8-bit coverage with "not empty ->
at least 1" (§9: plain 8-bit rounding erased 3.3 % of the non-empty zones);
a 16-bit variant for comparison. Storage: 16 x 16-zone tiles, empty tiles
elided (a tile index, 32 presence bytes, 256 value bytes).

What it reports:
  1. store / restore: the read-back planes equal the built ones; bytes per
     band and in total, dense vs tiled, 8 vs 16 bit;
  2. empty space: every zone wholly inside the corridor is 0 in every band
     (presence keeps it); the share of empty zones that a plane marks covered
     is 0 by construction - the plane IS the union at zone resolution;
  3. brightness over the sweep, band planes vs the continuous cut with exact
     zone density (density_planes_experiment's baseline): range, largest
     step, and the mean absolute error per screen pixel of the density part
     against the exact fine coverage of the same shapes (what upper
     quantization costs at the near end of a band);
  4. originals cost: shapes and painted pixels per view against continuous;
  5. query cost: zones and bytes read per view (the whole world is in view).

    .venv/bin/python tools/coarse_plane_experiment.py [--per-octave 1|2]

--per-octave 2 splits every octave into two bands (a plane each, half-octave
upper quantization of the cut; zones are the power of two at or above 4 px
at the band's widest view, so 4..5.7 px): what a second plane per octave
buys at the near end of a band, and what it costs in bytes.
"""
import argparse
import io
import math
import struct
import sys
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
import density_planes_experiment as dpe  # noqa: E402

W, CUT_PX, CORRIDOR = dpe.W, dpe.CUT_PX, dpe.CORRIDOR
ZONE_PX = 4
TILE = 16
OCTAVES = dpe.OCTAVES          # bands 0..5: s in [1/2, 1) ... [1/64, 1/32)
STEPS = dpe.STEPS_PER_OCTAVE
PER_OCTAVE = 1                 # bands an octave (set by --per-octave)


def n_bands():
    return OCTAVES * PER_OCTAVE


def band_of(s):
    """the band of a view scale: b with s in [2^-((b+1)/k), 2^-(b/k)), k bands an octave"""
    return min(n_bands() - 1, max(0, int(math.floor(-math.log2(s) * PER_OCTAVE + 1e-9))))


def band_params(b):
    s_min = 2.0 ** -((b + 1) / PER_OCTAVE)                       # the band's widest view
    zone = 1 << int(math.ceil(math.log2(ZONE_PX / s_min) - 1e-9))  # DBU, power of two, >= 4 px
    cut = int(math.ceil(CUT_PX / s_min - 1e-9))
    return s_min, zone, cut


def quantize8(cov):
    v = np.rint(cov * 255).astype(np.uint8)
    v[(cov > 0) & (v == 0)] = 1             # not empty -> at least 1
    return v


def quantize16(cov):
    v = np.rint(cov * 65535).astype(np.uint16)
    v[(cov > 0) & (v == 0)] = 1
    return v


def store(planes):
    """tiled sparse bytes: header, then per band per non-empty tile:
    (tile index u32, presence bits 32 B, values 256 x 1 or 2 B)"""
    out = io.BytesIO()
    out.write(struct.pack('<4sII', b'CPLN', 1, len(planes)))
    per_band = {}
    for b, (values, bits) in sorted(planes.items()):
        n = values.shape[0]
        nt = -(-n // TILE)
        start = out.tell()
        out.write(struct.pack('<IIII', b, n, nt, bits))
        for tj in range(nt):
            for ti in range(nt):
                tile = values[tj * TILE:(tj + 1) * TILE, ti * TILE:(ti + 1) * TILE]
                if not tile.any():
                    continue
                full = np.zeros((TILE, TILE), dtype=values.dtype)
                full[:tile.shape[0], :tile.shape[1]] = tile
                out.write(struct.pack('<I', tj * nt + ti))
                out.write(np.packbits(full > 0).tobytes())
                out.write(full.astype('<u1' if bits == 8 else '<u2').tobytes())
        per_band[b] = out.tell() - start
    return out.getvalue(), per_band


def restore(blob):
    f = io.BytesIO(blob)
    magic, version, nb = struct.unpack('<4sII', f.read(12))
    assert magic == b'CPLN' and version == 1
    planes = {}
    for _ in range(nb):
        b, n, nt, bits = struct.unpack('<IIII', f.read(16))
        dtype = np.uint8 if bits == 8 else np.uint16
        values = np.zeros((nt * TILE, nt * TILE), dtype=dtype)
        while True:
            head = f.read(4)
            if len(head) < 4:
                break
            (k,) = struct.unpack('<I', head)
            if k >= nt * nt or (planes and k < 0):
                f.seek(-4, 1)
                break
            presence = np.unpackbits(np.frombuffer(f.read(32), dtype=np.uint8)).reshape(TILE, TILE)
            tile = np.frombuffer(f.read(TILE * TILE * (bits // 8)), dtype=dtype).reshape(TILE, TILE)
            assert ((tile > 0) == (presence > 0)).all()
            tj, ti = divmod(k, nt)
            values[tj * TILE:(tj + 1) * TILE, ti * TILE:(ti + 1) * TILE] = tile
            # the next record may be the next band's header: peek
            pos = f.tell()
            nxt = f.read(16)
            f.seek(pos)
            if len(nxt) == 16:
                b2, n2, nt2, bits2 = struct.unpack('<IIII', nxt)
                if b2 == b + 1 and bits2 == bits and n2 <= n:
                    break
        planes[b] = (values[:n, :n], bits)
    return planes


def main(argv=None):
    global PER_OCTAVE
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument('--per-octave', type=int, default=1, choices=(1, 2))
    args = ap.parse_args(argv)
    PER_OCTAVE = args.per_octave
    rects = dpe.flatten()
    minside = np.minimum(rects[:, 2] - rects[:, 0], rects[:, 3] - rects[:, 1])
    all_mask = dpe.paint(rects)
    true_area = int(all_mask.sum())
    print('layout: %d rects, union %d DBU^2; bands %d (%d an octave), zones >= %d px at each band\'s widest view, tiles %dx%d zones'
          % (len(rects), true_area, n_bands(), PER_OCTAVE, ZONE_PX, TILE, TILE))
    # ---- build the planes: per band, the coverage of shapes under c_b at zone resolution
    planes8, planes16, coverage = {}, {}, {}
    print('\n== planes')
    for b in range(n_bands()):
        s_min, zone, cut = band_params(b)
        level = int(round(math.log2(zone)))
        mask = dpe.paint(rects[minside < cut])
        cov = dpe.zone_sum(mask, level).astype(np.float64) / (zone * zone)
        coverage[b] = cov
        planes8[b] = (quantize8(cov), 8)
        planes16[b] = (quantize16(cov), 16)
        n = cov.shape[0]
        nonempty = int((cov > 0).sum())
        tiles = sum(1 for tj in range(-(-n // TILE)) for ti in range(-(-n // TILE))
                    if cov[tj * TILE:(tj + 1) * TILE, ti * TILE:(ti + 1) * TILE].any())
        print('band %d: s in [%g, %g), zone %d DBU (level %d, %.1f px at the widest), cut %d DBU, %dx%d zones, %d non-empty (%.1f%%), %d of %d tiles non-empty'
              % (b, s_min, s_min * 2 ** (1 / PER_OCTAVE), zone, level, zone * s_min, cut, n, n, nonempty, 100 * nonempty / (n * n), tiles, (-(-n // TILE)) ** 2))
    # ---- 1. store / restore
    print('\n== 1. store / restore')
    for name, planes in (('8 bit', planes8), ('16 bit', planes16)):
        blob, per_band = store(planes)
        back = restore(blob)
        for b in planes:
            assert (back[b][0] == planes[b][0]).all(), 'band %d differs after restore' % b
        dense = sum(v.shape[0] ** 2 * (bits // 8) for v, bits in planes.values())
        print('%s: %d bytes tiled (%s), dense %d bytes; read back identical'
              % (name, len(blob), ', '.join('band %d %d' % (b, n) for b, n in sorted(per_band.items())), dense))
    # ---- 2. empty space
    print('\n== 2. empty space')
    for b in range(n_bands()):
        _, zone, _ = band_params(b)
        v = planes8[b][0]
        for x0, y0, x1, y1 in CORRIDOR:
            inside = v[-(-y0 // zone):y1 // zone, -(-x0 // zone):x1 // zone]
            assert (inside == 0).all(), 'band %d: corridor zone lit' % b
        # exact: a zone is 0 iff the union is empty there
        exact_empty = dpe.zone_sum(all_mask, int(round(math.log2(zone)))) == 0
        under_cut_empty = coverage[b] == 0
        assert not ((~exact_empty) & (~under_cut_empty) & (v == 0)).any()
        assert (v[under_cut_empty] == 0).all()
    print('corridor zones 0 in every band; a zone is 0 exactly where the under-cut union is empty (presence kept by "at least 1"): OK')
    # ---- 3. brightness and density error over the sweep
    print('\n== 3. brightness (originals + density) / true area, and the density part\'s error per screen pixel')
    print('%-9s %-5s %-6s %-6s %8s %8s %8s %8s %8s %8s' % ('s', 'band', 'c_req', 'c_b', 'bright', 'cont.', 'MAE dens', 'MAE ex.', 'orig sh', 'cont sh'))
    rows = []
    prev_b = None
    for s, level, c_req in dpe.sweep():
        b = band_of(s)
        _, zone, cut = band_params(b)
        side = int(math.ceil(W * s))
        # the band's rule
        orig = dpe.paint(rects[minside >= cut], scale=s, side=side)
        n_orig = int((minside >= cut).sum())
        # density from the plane, projected to screen pixels (zone value spread over its pixels)
        v = planes8[b][0].astype(np.float64) / 255.0
        zpx = zone * s                                  # zone side in screen px (4..8)
        ys = (np.arange(side) / zpx).astype(int).clip(0, v.shape[0] - 1)
        xs = (np.arange(side) / zpx).astype(int).clip(0, v.shape[1] - 1)
        dens = v[np.ix_(ys, xs)]
        lit = orig.sum() + dens[~orig].sum()
        bright = lit / (true_area * s * s)
        # the density part's truth: the exact coverage of the shapes under the band's cut at screen pixels
        fine_under = dpe.paint(rects[minside < cut])
        px = int(round(1 / s))
        truth = fine_under[:side * px, :side * px].reshape(side, px, side, px).mean(axis=(1, 3)) if px * side <= W else None
        mae = float(np.abs(dens - truth)[~orig].mean()) if truth is not None else float('nan')
        # continuous baseline: originals >= c_req, exact zone density at this view's level (8-16 px zones)
        orig_c = dpe.paint(rects[minside >= c_req], scale=s, side=side)
        n_cont = int((minside >= c_req).sum())
        zc = 1 << level
        cov_c = dpe.zone_sum(dpe.paint(rects[minside < c_req]), level).astype(np.float64) / (zc * zc)
        zpx_c = zc * s
        ys_c = (np.arange(side) / zpx_c).astype(int).clip(0, cov_c.shape[0] - 1)
        xs_c = (np.arange(side) / zpx_c).astype(int).clip(0, cov_c.shape[1] - 1)
        dens_c = cov_c[np.ix_(ys_c, xs_c)]
        bright_c = (orig_c.sum() + dens_c[~orig_c].sum()) / (true_area * s * s)
        fine_c = dpe.paint(rects[minside < c_req])
        truth_c = fine_c[:side * px, :side * px].reshape(side, px, side, px).mean(axis=(1, 3)) if px * side <= W else None
        mae_c = float(np.abs(dens_c - truth_c)[~orig_c].mean()) if truth_c is not None else float('nan')
        rows.append((s, b, c_req, cut, bright, bright_c, mae, mae_c, n_orig, n_cont, int(orig.sum()), int(orig_c.sum()), v.shape[0] ** 2))
        if b != prev_b or True:
            print('%-9.5f %-5d %-6d %-6d %8.3f %8.3f %8.4f %8.4f %8d %8d' % (s, b, c_req, cut, bright, bright_c, mae, mae_c, n_orig, n_cont))
        prev_b = b
    brights = [r[4] for r in rows]
    brights_c = [r[5] for r in rows]
    step = max(abs(a - b) for a, b in zip(brights, brights[1:]))
    step_c = max(abs(a - b) for a, b in zip(brights_c, brights_c[1:]))
    print('band planes: brightness %.3f..%.3f, largest step %.3f; continuous: %.3f..%.3f, step %.3f'
          % (min(brights), max(brights), step, min(brights_c), max(brights_c), step_c))
    maes = [r[6] for r in rows if not math.isnan(r[6])]
    maes_c = [r[7] for r in rows if not math.isnan(r[7])]
    print('density MAE per screen px: band planes mean %.4f max %.4f; continuous mean %.4f max %.4f'
          % (sum(maes) / len(maes), max(maes), sum(maes_c) / len(maes_c), max(maes_c)))
    # ---- 4. originals cost
    print('\n== 4. originals: shapes and painted pixels, band cut vs continuous (mean over the sweep)')
    print('shapes %.2fx, painted px %.2fx (band planes / continuous)'
          % (sum(r[8] for r in rows) / max(1, sum(r[9] for r in rows)), sum(r[10] for r in rows) / max(1, sum(r[11] for r in rows))))
    # ---- 5. query cost
    print('\n== 5. query: zones read per view (whole world in view) and bytes (8 bit + presence)')
    for b in range(n_bands()):
        s_min, zone, _ = band_params(b)
        n = planes8[b][0].shape[0]
        print('band %d: %d zones, %d bytes dense, screen %d..%d px wide' % (b, n * n, n * n * 9 // 8, int(W * s_min), int(W * s_min * 2)))
    print('== type this == (band zone cut nonempty% tiles bytes8 | bright-range step mae-mean)')
    for b in range(n_bands()):
        _, zone, cut = band_params(b)
        v = planes8[b][0]
        n = v.shape[0]
        bl = [r[4] for r in rows if r[1] == b]
        ml = [r[6] for r in rows if r[1] == b and not math.isnan(r[6])]
        print('%d %d %d %.1f %d | %.3f-%.3f %.4f' % (b, zone, cut, 100 * (v > 0).mean(), sum(1 for tj in range(-(-n // TILE)) for ti in range(-(-n // TILE)) if v[tj * TILE:(tj + 1) * TILE, ti * TILE:(ti + 1) * TILE].any()),
                                              min(bl), max(bl), sum(ml) / max(1, len(ml))))


if __name__ == '__main__':
    main()
