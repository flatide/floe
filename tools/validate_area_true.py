#!/usr/bin/env python3
"""Area-true drawing gate (floe_render_core::GeometryRasterRequest::area_true,
user decision 2026-09-22).

The KLayout rule grew every drawn shape by about a pixel per axis - fill by
two sampling phases plus an edge stroke on the pixel holding each edge - so
gaps up to ~1.5 px closed (3.8 px bars 1.2 px apart drew as one block) and a
shape under a pixel lit a whole one (0.1 px wires 1 px apart lit every
column, 10x their area). Under area-true a RECTANGLE is drawn width first -
each axis ceil(w - t) px (its whole pixels always, one more when the fraction
beats t, t its world rank for that axis), centred - and any other shape
lights the pixels whose centres it covers with its outline on their rim, a
sub-pixel one kept with the chance its own area fills its pixels. Contract:
a rectangle keeps its whole pixels and, over rectangles, its mean width; a
gap under 2 px may close and neighbours may share pixels (quantization).

One layout written with klayout.db: fields of vertical bars (widths / gaps in
pixels at the 0.1 um/px view, cycling through a list per field), at pans of
0, 1/4, 1/2 and 3/4 px:

  * no lit column lies outside the columns the bars touch;
  * in fields whose gaps are all 2 px or more, every bar draws floor(w) or
    floor(w) + 1 px, the same width at every pan, and no gap closes;
  * every field of bars a pixel or wider - integer and non-integer pitches,
    neighbours of different widths - lights, averaged over the four pans, a
    column share within 0.08 of its covered share; a field of one width and
    gap is written as an ARRAY (an OASIS repetition), whose members spread
    their extra pixels by index (GridRanks), and stays within 0.025;
  * bars under a pixel (arrays too): the lit share is within 0.85..1.15 of the
    covered share;
  * under the kill switch FLOE_RUST_AREA_TRUE=off the 1.2 and 1.5 px gaps
    close and every sub-pixel field lights every pixel, as before;
  * the same view twice gives the same pixels, and a view moved by a whole
    number of pixels gives the same pixels where the two overlap;
  * with the fill cleared, a polygon and a rectangle that run off every side
    of the view light no pixel in it (review 2026-09-22: the rim took the row
    over the top edge for a border), at whole and fractional pans;
  * 900 triangles and 900 squares of the same 0.8 x 0.8 px box, 3 px apart:
    the triangles light about half as many pixels (review 2026-09-22: a
    sub-pixel polygon was kept by its box's area);
  * the M7-C page wash is off by default (user decision 2026-09-22): a wide
    view of a small cell placed 10 x 10 times, each page under a pixel, washes
    no page and draws the geometry; FLOE_RUST_PAGE_WASH=on washes them.

    .venv/bin/python tools/validate_area_true.py
"""
import math
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
POLY, RECT, TRI, SQUARE, DOTS = (2, 0), (3, 0), (4, 0), (5, 0), (6, 0)
TINY = (300.0, 0.0)           # um: the 10 x 10 placements of a 2 um cell, 40 um apart
BIG = (60.0, 40.0)            # um: the shapes that run off the edges, around this point
SMALL = (120.0, 5.0)          # um: the triangle and square fields' corner
CLEAR = '\n'.join(['.' * 16] * 16)
BLACK = bytes((0, 0, 0, 255))              # the frame background
# (bar width px, gap px); the first six are at least a pixel wide
# fields of bars: (width px, gap px) cycled across the field
WIDE = [[(3.8, 3.8)], [(3.8, 1.2)], [(5.2, 2.8)], [(7.6, 2.4)], [(1.5, 1.5)], [(2.0, 2.0)],
        [(1.5, 1.0)], [(3.3, 1.4)], [(2.7, 2.1)],                       # pitch 2.5, 4.7, 4.8 px
        [(1.7, 2.3), (3.2, 1.1), (2.45, 2.6), (1.15, 1.9)]]            # neighbours of different widths
THIN = [[(0.1, 0.9)], [(0.25, 0.75)], [(0.5, 1.5)], [(0.5, 0.5)]]
FIELDS = WIDE + THIN
PER_ROW = 5


def origin(n):
    return (PAD + (n % PER_ROW) * (FIELD_W + PAD), PAD + (n // PER_ROW) * (FIELD_H + PAD))


def bars_of(n):
    """Field n's bars: (x0 um, x1 um, width px), the (width, gap) list cycled."""
    fx, _ = origin(n)
    out, x, k = [], fx, 0
    while True:
        w, g = FIELDS[n][k % len(FIELDS[n])]
        x0, x1 = round(x, 4), round(x + w * PX_UM, 4)
        if x1 > fx + FIELD_W + 1e-9:
            return out
        out.append((x0, x1, w))
        x += (w + g) * PX_UM
        k += 1


def name(n):
    return ' '.join('%g/%g' % pair for pair in FIELDS[n]) + ' px'


def touched(view, n):
    """The columns field n's bars touch in this view."""
    cols = set()
    for x0, x1, _ in bars_of(n):
        a, b = round((x0 - view[0]) / PX_UM, 6), round((x1 - view[0]) / PX_UM, 6)
        cols.update(range(math.floor(a), math.ceil(b)))
    return cols


def bar_widths(pixels, view, n):
    """Per bar of field n: (true width px, lengths of the lit runs within the
    columns it touches)."""
    runs_ = []
    for c in sorted(lit_columns(pixels, view, n)):
        if runs_ and runs_[-1][1] == c:
            runs_[-1][1] = c + 1
        else:
            runs_.append([c, c + 1])
    out = {}
    for k, (x0, x1, w) in enumerate(bars_of(n)):
        lo = math.floor(round((x0 - view[0]) / PX_UM, 6))
        hi = math.ceil(round((x1 - view[0]) / PX_UM, 6))
        out[k] = (w, [b - a for a, b in runs_ if a < hi and b > lo])
    return out


def covered(n):
    lo, hi = origin(n)[0], origin(n)[0] + FIELD_W
    inner = (lo + 2 * PX_UM, hi - 2 * PX_UM)
    area = sum(max(0.0, min(x1, inner[1]) - max(x0, inner[0])) for x0, x1, _ in bars_of(n))
    return area / (inner[1] - inner[0])


def layout(path):
    import klayout.db as kdb
    ly = kdb.Layout()
    ly.dbu = 0.001
    top = ly.create_cell('TOP')
    li = ly.layer(*LAYER)
    for n in range(len(FIELDS)):
        fx, fy = origin(n)
        for x0, x1, _ in bars_of(n):
            top.shapes(li).insert(kdb.DBox(x0, fy, x1, fy + FIELD_H))
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
    dot = ly.create_cell('DOT')
    for x0, y0, x1, y1 in ((0.0, 0.0, 0.6, 2.0), (1.0, 0.0, 2.0, 0.5), (1.2, 1.1, 1.9, 1.8)):
        dot.shapes(ly.layer(*DOTS)).insert(kdb.DBox(x0, y0, x1, y1))
    step = int(40.0 / ly.dbu)
    top.insert(kdb.CellInstArray(dot.cell_index(), kdb.Trans(int(TINY[0] / ly.dbu), int(TINY[1] / ly.dbu)),
                                 kdb.Vector(step, 0), kdb.Vector(0, step), 10, 10))
    ly.write(str(path))


def worker(src, on, wash=False):
    if on:
        os.environ.pop('FLOE_RUST_AREA_TRUE', None)
    else:
        os.environ['FLOE_RUST_AREA_TRUE'] = 'off'
    if wash:
        os.environ['FLOE_RUST_PAGE_WASH'] = 'on'
    cache = Cache(str(src))
    cache.load()
    w = RustRenderWorker(cache)
    w.start()
    os.environ.pop('FLOE_RUST_AREA_TRUE', None)
    os.environ.pop('FLOE_RUST_PAGE_WASH', None)
    return w


def frame(w, gen, view_um, cut_px=0.0, visible=(LAYER,), size=(None, None), report=False):
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
            pixels = bytes(res.pop('rgba'))
            return (pixels, res) if report else pixels
    raise AssertionError('area-true frame timeout')


def columns(pixels, view, n):
    """Per column of field n's interior (2 px in): whether any pixel is lit,
    and the lit share of the interior."""
    fx, fy = origin(n)
    c0 = int(round((fx - view[0]) / PX_UM)) + 2
    c1 = int(round((fx + FIELD_W - view[0]) / PX_UM)) - 2
    r0 = int(round((view[3] - (fy + FIELD_H)) / PX_UM)) + 2
    r1 = int(round((view[3] - fy) / PX_UM)) - 2
    flags, lit = [], 0
    for c in range(c0, c1):
        k = sum(pixels[(r * W + c) * 4:(r * W + c) * 4 + 4] != BLACK for r in range(r0, r1))
        flags.append(k > 0)
        lit += k
    return flags, lit / ((c1 - c0) * (r1 - r0))


def lit_columns(pixels, view, n):
    """The absolute columns of field n's rows with any lit pixel."""
    fx, fy = origin(n)
    r0 = int(round((view[3] - (fy + FIELD_H)) / PX_UM)) + 2
    r1 = int(round((view[3] - fy) / PX_UM)) - 2
    c0 = max(0, int(math.floor((fx - view[0]) / PX_UM)) - 3)
    c1 = min(W, int(math.ceil((fx + FIELD_W - view[0]) / PX_UM)) + 3)
    return {c for c in range(c0, c1)
            if any(pixels[(r * W + c) * 4:(r * W + c) * 4 + 4] != BLACK for r in range(r0, r1))}


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
            gen = 10
            for n in range(len(WIDE)):
                gaps_wide = all(g >= 2 for _, g in FIELDS[n])
                shares, widths_seen = [], None
                for phase in (0.0, 0.25, 0.5, 0.75):
                    gen += 1
                    moved = (view[0] + phase * PX_UM, view[1], view[2] + phase * PX_UM, view[3])
                    pixels = now if phase == 0.0 else frame(on, gen, moved)
                    stray = lit_columns(pixels, moved, n) - touched(moved, n)
                    assert not stray, '%s at a %g px pan: columns %s lit outside the bars' % (name(n), phase, sorted(stray)[:6])
                    flags, _ = columns(pixels, moved, n)
                    shares.append(sum(flags) / len(flags))
                    if gaps_wide:
                        # each bar by position: its one run, floor(w) or floor(w) + 1 wide
                        widths = bar_widths(pixels, moved, n)
                        for k, (w, hit) in widths.items():
                            assert len(hit) == 1, '%s at a %g px pan: bar %d drew runs %s (a gap closed?)' % (name(n), phase, k, hit)
                            assert math.floor(w) <= hit[0] <= math.floor(w) + 1, \
                                '%s at a %g px pan: a %g px bar drew %d px' % (name(n), phase, w, hit[0])
                        assert widths_seen in (None, widths), '%s: a pan changed a width' % name(n)
                        widths_seen = widths
                cover = covered(n)
                mean = sum(shares) / len(shares)
                # one (width, gap): an array, spread by index; else world-box hashes
                tolerance = 0.025 if len(FIELDS[n]) == 1 else 0.08
                assert abs(mean - cover) <= tolerance, '%s: column share %.3f over four pans for %.3f covered (%s)' \
                    % (name(n), mean, cover, ['%.3f' % v for v in shares])
                print('area-true bars %-28s column share per pan %s, mean %.3f for %.3f covered%s'
                      % (name(n), ' '.join('%.3f' % v for v in shares), mean, cover,
                         ', widths kept at every pan' if gaps_wide else ''))
            for n in (WIDE.index([(3.8, 1.2)]), WIDE.index([(1.5, 1.5)])):
                flags, _ = columns(was, view, n)
                assert all(flags), 'kill switch: the %s gaps should close as before' % name(n)
            for k in range(len(THIN)):
                n = len(WIDE) + k
                cover = covered(n)
                _, lit = columns(now, view, n)
                _, before = columns(was, view, n)
                assert 0.85 <= lit / cover <= 1.15, '%s: lit %.3f of covered %.3f' % (name(n), lit, cover)
                assert before > 0.99, 'kill switch: %s lit %.3f, expected every pixel' % (name(n), before)
                stray = lit_columns(now, view, n) - touched(view, n)
                assert not stray, '%s: columns lit outside the bars' % name(n)
                print('area-true bars %-28s lit %.3f for %.3f covered (kill switch %.3f)' % (name(n), lit, cover, before))
            # the same view again, and moved by 37 x 23 whole pixels
            gen += 1
            assert frame(on, gen, view) == now, 'area-true frame is not reproducible'
            dx, dy = 37, 23
            moved = (view[0] + dx * PX_UM, view[1] + dy * PX_UM, view[2] + dx * PX_UM, view[3] + dy * PX_UM)
            gen += 1
            shifted = frame(on, gen, moved)
            # world y grows upward: the moved view shows old pixel (c, r) at (c - dx, r + dy)
            differ = 0
            for r in range(0, H - dy):
                for c in range(dx, W):
                    a = now[(r * W + c) * 4:(r * W + c) * 4 + 4]
                    b = shifted[((r + dy) * W + c - dx) * 4:((r + dy) * W + c - dx) * 4 + 4]
                    differ += a != b
            assert differ == 0, 'a whole-pixel pan changed %d pixels' % differ
            print('area-true: reproducible, a 37 x 23 px pan changes no pixel')
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
            # the page wash: off by default, FLOE_RUST_PAGE_WASH=on turns it back on
            washer = worker(src, True, wash=True)
            try:
                wide = (TINY[0] - 100.0, TINY[1] - 100.0, TINY[0] - 100.0 + 200 * 4.0, TINY[1] - 100.0 + 150 * 4.0)
                gen += 1
                plain, report = frame(on, gen, wide, visible=(DOTS,), size=(200, 150), report=True)
                washed, wreport = frame(washer, 1, wide, visible=(DOTS,), size=(200, 150), report=True)
                lit = sum(plain[i:i + 4] != BLACK for i in range(0, len(plain), 4))
                assert report['plan_culls']['washed'] == 0 and lit > 0, \
                    'default frame: %d pages washed, %d px lit' % (report['plan_culls']['washed'], lit)
                assert wreport['plan_culls']['washed'] > 0, 'FLOE_RUST_PAGE_WASH=on washed no page'
                print('page wash: default washes 0 pages and draws %d px of geometry; on, %d pages washed'
                      % (lit, wreport['plan_culls']['washed']))
            finally:
                washer.stop()
        finally:
            on.stop()
            off.stop()
    print('AREA TRUE: OK')


if __name__ == '__main__':
    main()
