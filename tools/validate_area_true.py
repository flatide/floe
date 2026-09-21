#!/usr/bin/env python3
"""Area-true drawing gate (floe_render_core::GeometryRasterRequest::area_true,
user decision 2026-09-22).

The KLayout rule grew every drawn shape by about a pixel per axis - fill by
two sampling phases plus an edge stroke on the pixel holding each edge - so
gaps up to ~1.5 px closed (3.8 px bars 1.2 px apart drew as one block) and a
shape under a pixel lit a whole one (0.1 px wires 1 px apart lit every
column, 10x their area). Under area-true a shape lights the pixels whose
centres it covers, its outline is the rim of those pixels, and a shape under a
pixel on a side is kept with the chance its area fills its pixels, ranked by
its world box.

One layout written with klayout.db: fields of vertical bars, width / gap given
in pixels at the 0.1 um/px view (the field's lines keep one phase).

  * bars at least a pixel wide: the mean drawn width is within 0.6 px of the
    true one and every gap of a pixel or more stays open between each pair of
    bars; under the kill switch FLOE_RUST_AREA_TRUE=off the 1.2 and 1.5 px
    gaps close, as they did;
  * bars under a pixel: the lit share is within 0.5..1.6 of the covered share
    (the kill switch lights 1.0 of every field);
  * the same view twice gives the same pixels, and a view moved by a whole
    number of pixels gives the same pixels where the two overlap;
  * a view moved by a quarter, a half and three quarters of a pixel: each bar
    grating keeps its gaps, and its lit column share averaged over the four
    phases is within 0.08 of the covered share - one phase alone is NOT (1.5 px
    bars 1.5 px apart light 2 of 3 columns at one phase, 1 of 3 at another:
    the pixel-centre rule rounds each edge, so widths and gaps are kept on
    average over positions, not at every one);
  * with the fill cleared, a polygon and a rectangle that run off every side
    of the view light no pixel in it (review 2026-09-22: the rim took the row
    over the top edge for a border), at whole and fractional pans;
  * 900 triangles and 900 squares of the same 0.8 x 0.8 px box, 3 px apart:
    the triangles light about half as many pixels (review 2026-09-22: a
    sub-pixel polygon was kept by its box's area).

    .venv/bin/python tools/validate_area_true.py
"""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from floe.cache import Cache
from floe.rust_render import RustRenderWorker

W, H = 1000, 400
PX_UM = 0.1                                   # the view: 0.1 um a pixel
FIELD_W, FIELD_H, PAD = 16.0, 6.0, 4.0        # um
LAYER = (1, 0)
POLY, RECT, TRI, SQUARE = (2, 0), (3, 0), (4, 0), (5, 0)
BIG = (60.0, 40.0)            # um: the shapes that run off the edges, around this point
SMALL = (120.0, 5.0)          # um: the triangle and square fields' corner
CLEAR = '\n'.join(['.' * 16] * 16)
BLACK = bytes((0, 0, 0, 255))              # the frame background
# (bar width px, gap px); the first six are at least a pixel wide
WIDE = [(3.8, 3.8), (3.8, 1.2), (5.2, 2.8), (7.6, 2.4), (1.5, 1.5), (2.0, 2.0)]
THIN = [(0.1, 0.9), (0.25, 0.75), (0.5, 1.5), (0.5, 0.5)]
FIELDS = WIDE + THIN
PER_ROW = 5


def origin(n):
    return (PAD + (n % PER_ROW) * (FIELD_W + PAD), PAD + (n // PER_ROW) * (FIELD_H + PAD))


def layout(path):
    import klayout.db as kdb
    ly = kdb.Layout()
    ly.dbu = 0.001
    top = ly.create_cell('TOP')
    li = ly.layer(*LAYER)
    for n, (w, g) in enumerate(FIELDS):
        fx, fy = origin(n)
        x = fx
        while x + w * PX_UM <= fx + FIELD_W + 1e-9:
            top.shapes(li).insert(kdb.DBox(x, fy, x + w * PX_UM, fy + FIELD_H))
            x += (w + g) * PX_UM
    cx, cy = BIG
    octagon = [(-30, -12), (-12, -31), (13, -29), (31, -11), (29, 12), (11, 30), (-12, 31), (-31, 13)]
    top.shapes(ly.layer(*POLY)).insert(kdb.DPolygon([kdb.DPoint(cx + x, cy + y) for x, y in octagon]))
    top.shapes(ly.layer(*RECT)).insert(kdb.DBox(cx - 27.3, cy - 26.1, cx + 28.7, cy + 25.9))
    tri, sq = ly.layer(*TRI), ly.layer(*SQUARE)
    side = 0.8 * PX_UM
    for j in range(30):
        for i in range(30):
            x = SMALL[0] + i * 3 * PX_UM + ((j * 7) % 11) * 0.01 * PX_UM
            y = SMALL[1] + j * 3 * PX_UM + ((i * 5) % 13) * 0.01 * PX_UM
            top.shapes(tri).insert(kdb.DPolygon([kdb.DPoint(x, y), kdb.DPoint(x + side, y), kdb.DPoint(x, y + side)]))
            top.shapes(sq).insert(kdb.DBox(x, y, x + side, y + side))
    ly.write(str(path))


def worker(src, on):
    if on:
        os.environ.pop('FLOE_RUST_AREA_TRUE', None)
    else:
        os.environ['FLOE_RUST_AREA_TRUE'] = 'off'
    cache = Cache(str(src))
    cache.load()
    w = RustRenderWorker(cache)
    w.start()
    os.environ.pop('FLOE_RUST_AREA_TRUE', None)
    return w


def frame(w, gen, view_um, cut_px=0.0, visible=(LAYER,), size=(None, None)):
    dbu = float(w.cache.meta['dbu'])
    w.submit({'kind': 'render', 'gen': gen, 'scope': 'headless', 'bbox': tuple(v / dbu for v in view_um), 'view': None,
              'w': size[0] or W, 'h': size[1] or H, 'depth': None, 'cut_px': cut_px, 'lod': False, 'frames': False,
              'labels': False, 'abstract': False, 'visible': list(visible), 'frame_format': 'raw',
              'thin': 'keep', 'frame_cache': False})
    deadline = time.monotonic() + 300
    while time.monotonic() < deadline:
        res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
        assert res.get('kind') != 'error', res
        if res.get('kind') == 'frame' and res.get('gen') == gen and not res.get('refining'):
            return bytes(res.pop('rgba'))
    raise AssertionError('area-true frame timeout')


def columns(pixels, view, n):
    """Per column of field n's interior (2 px in): whether any pixel is lit,
    and the lit share of the interior."""
    fx, fy = origin(n)
    c0 = int(round((fx - view[0]) / PX_UM)) + 2
    c1 = int(round((fx + FIELD_W - view[0]) / PX_UM)) - 2
    r0 = int(round((view[3] - (fy + FIELD_H)) / PX_UM)) + 2
    r1 = int(round((view[3] - fy) / PX_UM)) - 2
    background = pixels[:4]
    flags, lit = [], 0
    for c in range(c0, c1):
        k = sum(pixels[(r * W + c) * 4:(r * W + c) * 4 + 4] != background for r in range(r0, r1))
        flags.append(k > 0)
        lit += k
    return flags, lit / ((c1 - c0) * (r1 - r0))


def runs(flags):
    out = {True: [], False: []}
    cur, n = flags[0], 0
    for f in flags:
        if f == cur:
            n += 1
        else:
            out[cur].append(n)
            cur, n = f, 1
    out[cur].append(n)
    return out[True][1:-1] if len(out[True]) > 2 else out[True], out[False]


def main():
    os.environ['FLOE_INDEX_BIN'] = str(ROOT / 'rust/target/release/floe-index')
    os.environ['FLOE_RENDERD_BIN'] = str(ROOT / 'rust/target/release/floe-renderd')
    os.environ['FLOE_RUST_RETAINED_MB'] = '0'
    with tempfile.TemporaryDirectory(prefix='floe-area-true-') as temp:
        src = Path(temp) / 'bars.oas'
        layout(src)
        done = subprocess.run([sys.executable, '-B', '-m', 'floe2', 'index', str(src)],
                              cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
        assert done.returncode == 0, done.stdout + done.stderr
        on, off = worker(src, True), worker(src, False)
        try:
            view = (0.0, 0.0, W * PX_UM, H * PX_UM)
            now, was = frame(on, 1, view), frame(off, 1, view)
            for n, (w, g) in enumerate(WIDE):
                flags, _ = columns(now, view, n)
                bars, gaps = runs(flags)
                mean = sum(bars) / len(bars)
                assert abs(mean - w) <= 0.6, 'bars %g/%g px: drawn %.2f px wide' % (w, g, mean)
                assert len(gaps) >= len(bars) and min(gaps) >= 1, \
                    'bars %g/%g px: a gap closed (%d bars, gaps %s)' % (w, g, len(bars), gaps[:8])
                print('area-true bars %4g / %4g px: drawn %.2f px, gaps %.2f px'
                      % (w, g, mean, sum(gaps) / len(gaps)))
            for n in (WIDE.index((3.8, 1.2)), WIDE.index((1.5, 1.5))):
                flags, _ = columns(was, view, n)
                assert all(flags), 'kill switch: the %g/%g px gaps should close as before' % WIDE[n]
            for k, (w, g) in enumerate(THIN):
                n = len(WIDE) + k
                cover = w / (w + g)
                _, lit = columns(now, view, n)
                _, before = columns(was, view, n)
                assert 0.5 <= lit / cover <= 1.6, 'bars %g/%g px: lit %.3f of covered %.3f' % (w, g, lit, cover)
                assert before > 0.99, 'kill switch: bars %g/%g px lit %.3f, expected every pixel' % (w, g, before)
                print('area-true bars %4g / %4g px: lit %.3f for %.3f covered (kill switch %.3f)'
                      % (w, g, lit, cover, before))
            # the same view again, and moved by 37 x 23 whole pixels
            assert frame(on, 2, view) == now, 'area-true frame is not reproducible'
            dx, dy = 37, 23
            moved = (view[0] + dx * PX_UM, view[1] + dy * PX_UM, view[2] + dx * PX_UM, view[3] + dy * PX_UM)
            shifted = frame(on, 3, moved)
            # world y grows upward: the moved view shows old pixel (c, r) at (c - dx, r + dy)
            differ = 0
            for r in range(0, H - dy):
                for c in range(dx, W):
                    a = now[(r * W + c) * 4:(r * W + c) * 4 + 4]
                    b = shifted[((r + dy) * W + c - dx) * 4:((r + dy) * W + c - dx) * 4 + 4]
                    differ += a != b
            assert differ == 0, 'a whole-pixel pan changed %d pixels' % differ
            print('area-true: reproducible, a 37 x 23 px pan changes no pixel')
            # fractional pans: the lit column share of each bar grating
            gen = 10
            for n, (w, g) in enumerate(WIDE):
                shares = []
                for phase in (0.0, 0.25, 0.5, 0.75):
                    gen += 1
                    moved = (view[0] + phase * PX_UM, view[1], view[2] + phase * PX_UM, view[3])
                    pixels = frame(on, gen, moved)
                    flags, _ = columns(pixels, moved, n)
                    bars, gaps = runs(flags)
                    assert gaps and min(gaps) >= 1 and len(gaps) >= len(bars), \
                        'bars %g/%g px at a %g px pan: a gap closed' % (w, g, phase)
                    shares.append(sum(flags) / len(flags))
                cover = w / (w + g)
                mean = sum(shares) / len(shares)
                assert abs(mean - cover) <= 0.08, 'bars %g/%g px: column share %.3f over four phases for %.3f covered (%s)' \
                    % (w, g, mean, cover, ['%.3f' % v for v in shares])
                print('area-true bars %4g / %4g px: column share per phase %s, mean %.3f for %.3f covered'
                      % (w, g, ' '.join('%.3f' % v for v in shares), mean, cover))
            # with the fill cleared, shapes around the whole view draw no rim in it
            on.submit({'kind': 'repattern', 'fills': [(POLY, CLEAR), (RECT, CLEAR)], 'widths': []})
            size = (200, 150)
            for dx, dy in ((0.0, 0.0), (0.37, 0.0), (0.0, 0.61), (1.0, -1.0)):
                bx, by = BIG[0] - 7.0 + dx * PX_UM, BIG[1] - 5.0 + dy * PX_UM
                inside = (bx, by, bx + size[0] * PX_UM, by + size[1] * PX_UM)
                for layer in (POLY, RECT):
                    gen += 1
                    pixels = frame(on, gen, inside, visible=(layer,), size=size)
                    lit = sum(pixels[i:i + 4] != BLACK for i in range(0, len(pixels), 4))
                    assert lit == 0, 'layer %s: %d rim pixels inside the shape at a (%g, %g) px pan' % (layer, lit, dx, dy)
            on.submit({'kind': 'repattern', 'fills': [], 'widths': []})
            print('area-true: shapes running off the view draw no rim in it (4 pans, polygon and rectangle)')
            # sub-pixel polygons keep by their own area
            span = (SMALL[0] - 1.0, SMALL[1] - 1.0, SMALL[0] - 1.0 + 100 * PX_UM, SMALL[1] - 1.0 + 100 * PX_UM)
            counts = {}
            for layer in (TRI, SQUARE):
                gen += 1
                pixels = frame(on, gen, span, visible=(layer,), size=(100, 100))
                counts[layer] = sum(pixels[i:i + 4] != BLACK for i in range(0, len(pixels), 4))
            ratio = counts[TRI] / counts[SQUARE]
            assert 0.35 <= ratio <= 0.65, 'triangles lit %d, squares %d (%.2f)' % (counts[TRI], counts[SQUARE], ratio)
            print('area-true: 900 triangles light %d px, 900 squares %d px (%.2f; areas 0.32 / 0.64 px each)'
                  % (counts[TRI], counts[SQUARE], ratio))
        finally:
            on.stop()
            off.stop()
    print('AREA TRUE: OK')


if __name__ == '__main__':
    main()
