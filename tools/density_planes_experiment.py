#!/usr/bin/env python3
"""Step 2 of docs/CUT_DENSITY_DESIGN.ko.md §7: the joint cumulative planes
F[k,d] and the common quantized cut on a small HIERARCHICAL synthetic layout.
NOT the index, the renderer or a file format.

The layout is built in memory (cells, placements, arrays, rotations, mirrors,
an exact duplicate placement, a big shape over small deep ones, a cross-shaped
empty corridor) and flattened. Every coordinate is an integer DBU and the fine
raster is 1 DBU per pixel, so every union area below is exact:

  F[k,d](zone) = area(zone ∩ union{shape : min side < b[k], depth <= d}) / area(zone)

Zones are the world-anchored dyadic cells of step 1 (§8): level L has side
2^L DBU and a view of scale s px/DBU reads the level whose zone projects to
[8, 16) px. The cut follows renderd: c_req = ceil(3 px / s) DBU (detail
medium), and the common quantized cut c_eff = max{b[k] <= c_req} goes to the
original path (min side >= c_eff) and to the density path (min side < c_eff,
the plane F[k_eff, d]). The band table b is integer DBU, 1/4 or 1/2 octave.

What it measures (each section of the output says how):
  1. checks: F is monotone in k and in d; at every zoom step the original and
     the density sets partition the shapes and their union is the layout's;
     the corridor stays empty; a duplicate placement adds nothing.
  2. what a cheaper storage would get wrong: bands stored apart and summed,
     and separate size-cumulative / depth-cumulative planes combined.
  3. a zoom sweep, 16 steps per octave: brightness (lit pixels / true area) of
     original + density, its largest step-to-step jump, and the original cost
     (shapes, pixel paints) of the quantized cut against the continuous one.
  4. storage of the planes a view can ask for: which (level, band) pairs,
     non-empty zones and tiles, identical tiles shared, 8- vs 16-bit values.
  5. what a bottom-up (composable) build would get instead of the exact
     union: summed shape areas, the exact union of each TOP child summed
     (instances overlap: the rail over the arrays, the duplicate block, the
     big shape over deep ones), and OR-ed occupancy bits of zone/16.

    .venv/bin/python tools/density_planes_experiment.py
"""
import math

import numpy as np

W = 2048                 # world side, DBU (= fine raster pixels)
CUT_PX = 3.0             # detail medium
PITCH_PX = 8             # zone projects to [8, 16) px (step 1)
STEPS_PER_OCTAVE = 16
S_MAX, OCTAVES = 1.0, 6  # sweep s = 1 .. 1/64 px/DBU
TILE_ZONES = 16          # sparse storage tile: 16 x 16 zones
CORRIDOR = ((896, 0, 1152, W), (0, 896, W, 1152))   # cross, x0 y0 x1 y1


# ------------------------------------------------------------------ layout

def R(x, y, w, h):
    return ('r', x, y, w, h)


def P(cell, x, y, orient=0, nx=1, ny=1, px=0, py=0):
    """orient 0..3 = rotation by 90 degrees, +4 = mirrored in y first."""
    return ('p', cell, x, y, orient, nx, ny, px, py)


CELLS = {
    'SPECK': [R(0, 0, 1, 1), R(3, 0, 2, 2)],
    'VIA': [R(x * 8 + 2, y * 8 + 2, 4, 4) for y in range(4) for x in range(4)],
    'BAR': [R(2 + i * 6, 2, 2, 24) for i in range(5)] + [R(2 + i * 6, 28, 3, 3) for i in range(5)],
    'FILL': [R(0, 0, 12, 12)],
    'WIREBUS': [R(0, i * 8, 200, 2) for i in range(16)],
    # 64 x 64: the rotated and mirrored copies land in the upper half
    'STD': [P('BAR', 0, 0), P('VIA', 32, 0), P('BAR', 32, 32, 1), P('VIA', 32, 64, 4)],
    'ARR': [P('STD', 0, 0, 0, 7, 7, 64, 64)],
    # 896 x 896 blocks: A dense (a rail over its arrays), B sparse
    'BLOCK_A': [P('ARR', 0, 0, 0, 2, 2, 448, 448), R(430, 0, 24, 896)],
    'BLOCK_B': [P('FILL', 20, 20, 0, 10, 10, 86, 86), P('SPECK', 63, 63, 0, 10, 10, 86, 86),
                P('WIREBUS', 100, 600), P('WIREBUS', 700, 100, 1)],
    'TOP': [P('BLOCK_A', 0, 0), P('BLOCK_B', 1152, 0), P('BLOCK_A', 0, 2048, 4),
            P('BLOCK_B', 1152, 1152), P('BLOCK_B', 1152, 1152),     # exact duplicate
            R(100, 100, 600, 600),                                   # big, over depth-4 smalls
            R(0, 1500, 896, 1)],                                     # top hairline
}


def orient_xy(o, x, y):
    if o & 4:
        y = -y
    for _ in range(o & 3):
        x, y = -y, x
    return x, y


def flatten():
    """Rects as numpy columns x0 y0 x1 y1 depth top; depth = placements from
    TOP, top = which item (placement member or own shape) of TOP holds it."""
    out = []
    top = [0]

    def walk(cell, xf, depth):
        o, tx, ty = xf
        for item in CELLS[cell]:
            if item[0] == 'r':
                _, x, y, w, h = item
                ax, ay = orient_xy(o, x, y)
                bx, by = orient_xy(o, x + w, y + h)
                out.append((min(ax, bx) + tx, min(ay, by) + ty, max(ax, bx) + tx, max(ay, by) + ty,
                            depth, top[0]))
                if depth == 0:
                    top[0] += 1
                continue
            _, child, x, y, co, nx, ny, px, py = item
            for j in range(ny):
                for i in range(nx):
                    dx, dy = orient_xy(o, x + i * px, y + j * py)
                    walk(child, (compose(o, co), tx + dx, ty + dy), depth + 1)
                    if depth == 0:
                        top[0] += 1

    walk('TOP', (0, 0, 0), 0)
    a = np.array(out, dtype=np.int64)
    assert a[:, :4].min() >= 0 and a[:, :4].max() <= W, "layout leaves the world"
    return a


def compose(outer, inner):
    """The orientation of applying `inner` first, then `outer`."""
    probe = [(1, 0), (0, 1)]
    target = [orient_xy(outer, *orient_xy(inner, *v)) for v in probe]
    for o in range(8):
        if [orient_xy(o, *v) for v in probe] == target:
            return o
    raise AssertionError("orientations are not closed")


# ------------------------------------------------------------------ rasters

def paint(rects, scale=1.0, side=W):
    """Coverage of rects on a side x side grid; at scale != 1 a rect covers
    every pixel it touches (the original path's at-least-a-pixel rule)."""
    grid = np.zeros((side + 1, side + 1), dtype=np.int32)
    if len(rects):
        x0 = np.floor(rects[:, 0] * scale).astype(np.int64)
        y0 = np.floor(rects[:, 1] * scale).astype(np.int64)
        x1 = np.minimum(np.ceil(rects[:, 2] * scale).astype(np.int64), side)
        y1 = np.minimum(np.ceil(rects[:, 3] * scale).astype(np.int64), side)
        x1, y1 = np.maximum(x1, x0 + 1), np.maximum(y1, y0 + 1)
        np.add.at(grid, (y0, x0), 1)
        np.add.at(grid, (y0, x1), -1)
        np.add.at(grid, (y1, x0), -1)
        np.add.at(grid, (y1, x1), 1)
    return grid.cumsum(0).cumsum(1)[:side, :side] > 0


def zone_sum(mask, level):
    z = 1 << level
    n = W // z
    return mask.reshape(n, z, n, z).sum(axis=(1, 3), dtype=np.int32)


# ------------------------------------------------------------------ bands and views

def band_table(octave):
    """Monotone integer DBU boundaries, duplicates merged (the index's table)."""
    steps = round(1 / octave)
    out = sorted({max(1, round(2 ** (k / steps))) for k in range(0, steps * 12 + 1)})
    return [b for b in out if b <= W]


def sweep():
    for i in range(OCTAVES * STEPS_PER_OCTAVE + 1):
        s = S_MAX * 2 ** (-i / STEPS_PER_OCTAVE)
        level = math.ceil(math.log2(PITCH_PX / s) - 1e-12)
        yield s, level, math.ceil(CUT_PX / s - 1e-9)


def effective(table, c_req):
    below = [b for b in table if b <= c_req]
    return below[-1] if below else 0


# ------------------------------------------------------------------ main

def main():
    rects = flatten()
    minside = np.minimum(rects[:, 2] - rects[:, 0], rects[:, 3] - rects[:, 1])
    depth = rects[:, 4]
    dmax = int(depth.max())
    all_mask = paint(rects)
    union = int(all_mask.sum())
    summed = int(((rects[:, 2] - rects[:, 0]) * (rects[:, 3] - rects[:, 1])).sum())
    unique = np.unique(rects[:, :5], axis=0)
    print("layout: %d rects (%d unique), union %d DBU^2 of summed %d, depths %s"
          % (len(rects), len(unique), union, summed,
             dict(zip(*[v.tolist() for v in np.unique(depth, return_counts=True)]))))
    print("min sides:", dict(zip(*[v.tolist() for v in np.unique(minside, return_counts=True)])))

    tables = {'1/4': band_table(0.25), '1/2': band_table(0.5)}
    views = list(sweep())
    cuts = sorted({c for t in tables.values() for c in t} | {c for _, _, c in views} | {W + 1})

    # cumulative fine masks per (cut, depth), built once in increasing cut order;
    # only zone sums at the levels a view reads are kept (and every level for the
    # table cuts, for the storage section)
    levels = sorted({lv for _, lv, _ in views})
    area = {}           # (cut, depth, level) -> zone areas
    fine = {}           # (cut, depth) -> packed fine mask: the sweep's cuts, depth 1 and full
    sweep_cuts = {c for _, _, c in views} | {effective(t, c) for t in tables.values() for _, _, c in views}
    running = [np.zeros((W, W), dtype=bool) for _ in range(dmax + 1)]
    previous = 0
    for cut in cuts:
        new = (minside >= previous) & (minside < cut)
        run = np.zeros((W, W), dtype=bool)
        for d in range(dmax + 1):
            run |= paint(rects[new & (depth == d)])
            running[d] |= run
            for lv in levels:
                area[cut, d, lv] = zone_sum(running[d], lv)
            if cut in sweep_cuts and d in (1, dmax):
                fine[cut, d] = np.packbits(running[d])
        previous = cut
    full = W + 1

    def fine_mask(cut, d):
        return np.unpackbits(fine[cut, d], count=W * W).reshape(W, W).astype(bool)

    band_cache = {}

    def band_area(a, b, lv):
        # a band stored apart = the union of ITS shapes, not the cumulative difference
        if (a, b, lv) not in band_cache:
            band_cache[a, b, lv] = zone_sum(paint(rects[(minside >= a) & (minside < b)]), lv)
        return band_cache[a, b, lv]

    # ---- 1. checks
    print("\n== 1. checks")
    mono = all((area[a, d, lv] <= area[b, d, lv]).all()
               for a, b in zip(cuts, cuts[1:]) for d in range(dmax + 1) for lv in levels)
    mono_d = all((area[c, d, lv] <= area[c, d + 1, lv]).all()
                 for c in cuts for d in range(dmax) for lv in levels)
    assert mono and mono_d
    print("F monotone in the cut and in the depth: OK (%d cuts x %d depths x %d levels)"
          % (len(cuts), dmax + 1, len(levels)))
    for x0, y0, x1, y1 in CORRIDOR:
        assert not all_mask[y0:y1, x0:x1].any()
    for lv in levels:
        z = 1 << lv
        for x0, y0, x1, y1 in CORRIDOR:
            inside = area[full, dmax, lv][-(-y0 // z):y1 // z, -(-x0 // z):x1 // z]
            assert (inside == 0).all()
    print("corridor zones (cross, 256 wide) are 0 at every level wholly inside it: OK")
    dup = paint(unique)
    assert (dup == all_mask).all()
    print("the duplicate placement adds %d DBU^2 to the union (summed area +%d): OK"
          % (int(all_mask.sum() - dup.sum()), summed - int(((unique[:, 2] - unique[:, 0])
                                                           * (unique[:, 3] - unique[:, 1])).sum())))
    checked = 0
    every = {d: paint(rects[depth <= d]) for d in (dmax, 1)}
    for name, table in tables.items():
        for s, lv, c_req in views:
            c = effective(table, c_req)
            for d in (dmax, 1):
                keep = depth <= d
                orig = paint(rects[keep & (minside >= c)])
                dens = fine_mask(c, d) if c else np.zeros((W, W), dtype=bool)
                assert ((orig | dens) == every[d]).all()
                assert not (keep & (minside >= c) & (minside < c)).any()
                checked += 1
    print("original (min side >= c_eff) | F[c_eff, d] == the layout at depth d, at %d (step, "
          "table, depth) cases: OK" % checked)

    # ---- 2. cheaper storage
    print("\n== 2. what a cheaper storage gets wrong (full depth unless said)")
    t = tables['1/4']
    for lv in (levels[0], levels[len(levels) // 2], levels[-1]):
        z2 = float(1 << (2 * lv))
        for c in (5, 32, 256):
            cum = area[effective(t, c), dmax, lv]
            bands = [b for b in t if b <= effective(t, c)]
            apart = sum(band_area(a, b, lv) for a, b in zip([0] + bands, bands))
            over = apart.sum() / max(1, cum.sum())
            clamp = np.minimum(apart, z2)
            print("L%-2d c<%-3d bands %2d | summed bands / union %.3f  (worst zone %+.3f of its area, "
                  "clamped %.3f)" % (lv, c, len(bands), over,
                                     ((apart - cum) / z2).max(), clamp.sum() / max(1, cum.sum())))
    for lv in (levels[0], levels[-1]):
        z2 = float(1 << (2 * lv))
        worst = leak = 0.0
        zones = 0
        for c in t:
            for d in range(dmax):
                true = area[c, d, lv]
                size_only, depth_only = area[c, dmax, lv], area[full, d, lv]
                est = np.minimum(size_only, depth_only)
                worst = max(worst, ((est - true) / z2).max())
                bad = (est > 0) & (true == 0)
                leak = max(leak, bad.mean())
                zones += int(bad.sum())
        print("L%-2d separate planes min(F_size[k], F_depth[d]) vs F[k,d]: worst zone +%.3f, "
              "zones lit where F[k,d] = 0: %d (at most %.1f %% of one plane)"
              % (lv, worst, zones, 100 * leak))

    # ---- 3. sweep
    print("\n== 3. zoom sweep: s = %g .. %g px/DBU, %d steps per octave, depth full"
          % (S_MAX, S_MAX / 2 ** OCTAVES, STEPS_PER_OCTAVE))
    variants = [('continuous', None)] + list(tables.items())
    series = {}
    for name, table in variants:
        rows = []
        for s, lv, c_req in views:
            c = c_req if table is None else effective(table, c_req)
            side = math.ceil(W * s)
            orig_rects = rects[minside >= c]
            img = paint(orig_rects, s, side)
            lit = int(img.sum())
            # a density dot lands inside a density shape; it adds a pixel only
            # where the original has not lit that pixel (write-once)
            dens = fine_mask(c, dmax)
            idx = np.minimum((np.arange(W) * s).astype(np.int64), side - 1)
            under = img[np.ix_(idx, idx)]
            visible = int((dens & ~under).sum())
            ref = union * s * s
            paints = int(((np.ceil(orig_rects[:, 2] * s) - np.floor(orig_rects[:, 0] * s)).clip(1)
                          * (np.ceil(orig_rects[:, 3] * s) - np.floor(orig_rects[:, 1] * s)).clip(1)).sum())
            rows.append({'s': s, 'c': c, 'bright': (lit + visible * s * s) / ref,
                         'orig': len(orig_rects), 'paints': paints})
        series[name] = rows
    base = series['continuous']
    print("variant      | brightness min..max  largest jump  steps >2%% jump | original shapes / continuous: "
          "max mean | pixel paints: max mean | cut changes")
    for name, _ in variants:
        rows = series[name]
        br = [r['bright'] for r in rows]
        jumps = [abs(b - a) / a for a, b in zip(br, br[1:])]
        so = [r['orig'] / max(1, q['orig']) for r, q in zip(rows, base)]
        sp = [r['paints'] / max(1, q['paints']) for r, q in zip(rows, base)]
        changes = sum(1 for a, b in zip(rows, rows[1:]) if a['c'] != b['c'])
        print("%-12s | %.3f..%.3f  %6.1f %%  %3d           | %.2f %.3f | %.2f %.3f | %d"
              % (name, min(br), max(br), 100 * max(jumps), sum(1 for j in jumps if j > 0.02),
                 max(so), sum(so) / len(so), max(sp), sum(sp) / len(sp), changes))
    for name in tables:
        worst = max(range(len(base)), key=lambda i: series[name][i]['orig'] - base[i]['orig'])
        print("most extra originals, %s: s=%.4g c_req=%d c_eff=%d: %d original shapes against %d "
              "(pixel paints %d against %d)"
              % (name, base[worst]['s'], base[worst]['c'], series[name][worst]['c'],
                 series[name][worst]['orig'], base[worst]['orig'],
                 series[name][worst]['paints'], base[worst]['paints']))

    # ---- 4. storage
    print("\n== 4. storage of the planes the views read (depth planes 0..%d)" % dmax)
    for name, table in tables.items():
        need = sorted({(lv, effective(table, c_req)) for _, lv, c_req in views if effective(table, c_req)})
        per_level = {}
        for lv, c in need:
            per_level.setdefault(lv, []).append(c)
        dense = zones_nz = tiles = shared = lost8 = lost16 = 0
        for lv, cs in per_level.items():
            n = W >> lv
            z2 = float(1 << (2 * lv))
            prev_c = None
            for c in cs:
                for d in range(dmax + 1):
                    a = area[c, d, lv]
                    dense += n * n
                    zones_nz += int((a > 0).sum())
                    q8 = np.rint(a / z2 * 255)
                    lost8 += int(((a > 0) & (q8 == 0)).sum())
                    lost16 += int(((a > 0) & (np.rint(a / z2 * 65535) == 0)).sum())
                    tn = max(1, n // TILE_ZONES)
                    tz = n // tn
                    for ty in range(tn):
                        for tx in range(tn):
                            cell = a[ty * tz:(ty + 1) * tz, tx * tz:(tx + 1) * tz]
                            if not cell.any():
                                continue
                            same = (d and (cell == area[c, d - 1, lv][ty * tz:(ty + 1) * tz,
                                                                      tx * tz:(tx + 1) * tz]).all())
                            same = same or (prev_c is not None and (
                                cell == area[prev_c, d, lv][ty * tz:(ty + 1) * tz,
                                                            tx * tz:(tx + 1) * tz]).all())
                            if same:
                                shared += 1
                            else:
                                tiles += 1
                prev_c = c
        print("%s octave: (level, band) planes %s" % (name, {lv: len(cs) for lv, cs in per_level.items()}))
        print("   bands the views read per level: %s" % {lv: cs for lv, cs in per_level.items()})
        print("   zones: dense %d, non-empty %d (%.1f %%); %dx%d tiles stored %d, shared with the plane"
              " below in depth or band %d; 8-bit values turn %d non-empty zones to 0, 16-bit %d"
              % (dense, zones_nz, 100 * zones_nz / dense, TILE_ZONES, TILE_ZONES, tiles, shared,
                 lost8, lost16))
    all_bands = len(tables['1/4'])
    print("every band at every level instead: %d bands x %d levels x %d depths" % (all_bands, len(levels), dmax + 1))

    # ---- 5. composable builds
    print("\n== 5. bottom-up builds against the exact union (full depth, the widest band each level reads, 1/4)")
    print("level band | summed shapes  per-child union  bits zone/16  min(child, bits) | worst zone error"
          " of those four (fraction of the zone)")
    t = tables['1/4']
    tops = np.unique(rects[:, 5])
    for lv in levels:
        c = max(effective(t, c_req) for _, level, c_req in views if level == lv)
        z = 1 << lv
        z2 = float(z * z)
        exact = area[c, dmax, lv].astype(np.float64)
        sel = rects[minside < c]
        summed_z = np.zeros_like(exact)
        for r in sel:
            x0, y0, x1, y1 = (int(v) for v in r[:4])
            for gy in range(y0 // z, (y1 - 1) // z + 1):
                for gx in range(x0 // z, (x1 - 1) // z + 1):
                    summed_z[gy, gx] += ((min(x1, (gx + 1) * z) - max(x0, gx * z))
                                         * (min(y1, (gy + 1) * z) - max(y0, gy * z)))
        child = sum(zone_sum(paint(sel[sel[:, 5] == k]), lv).astype(np.float64) for k in tops)
        sub = max(1, z // 16)
        m = fine_mask(c, dmax)
        bits = m.reshape(W // sub, sub, W // sub, sub).any(axis=(1, 3))
        bits_z = bits.reshape(W // z, z // sub, W // z, z // sub).sum(axis=(1, 3)) * float(sub * sub)
        est = [np.minimum(summed_z, z2), np.minimum(child, z2), bits_z, np.minimum(np.minimum(child, z2), bits_z)]
        total = max(1.0, exact.sum())
        print("L%-2d c<%-4d | %6.3f %6.3f %6.3f %6.3f | %+.3f %+.3f %+.3f %+.3f"
              % ((lv, c) + tuple(e.sum() / total for e in est)
                 + tuple(((e - exact) / z2).max() for e in est)))


if __name__ == "__main__":
    main()
