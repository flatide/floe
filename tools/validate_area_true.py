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
    no page and draws the geometry; FLOE_RUST_PAGE_WASH=on washes them;
  * the M7 LOD swap is off by default (user decision 2026-09-22): a dense
    layout indexed with `floe2 index --lod` swaps no page for its merged
    variant at a wide view; FLOE_RUST_LOD=on swaps it;
  * an axis-aligned array ranks on the world lattice (candidate 2 of
    ADAPTIVE_CUT_DENSITY_PLAN §4.2): one row of 64 bars 1.5 px wide stored
    as one array, as two placements of a 32-bar cell, and as a column cell
    placed rotated onto the row lights the same pixels at whole and
    fractional pans;
  * the indexer's own page split (review 2026-09-23): a 64 x 6 lattice of
    1.5 x 4.5 px bars among 70,000 single rectangles on its layer, indexed
    with a 16 MiB and a 1 MiB page target - one page, and two pages that cut
    the lattice's Grid record in two (frag_split / frag_rep) - lights the
    same pixels at five pans, whole and fractional on both axes, and about
    its covered area (the per-record ranks differed in 258 px); the build's
    rep-split line counts its 2 grid pieces, none one-row or one-member;
  * a 2 x 64 lattice cut ACROSS its rows (a tall layout, the split plane
    between the rows): the build counts 2 one-row pieces of a 2-D grid,
    each written as a one-dimensional repetition, and they light the same
    pixels as the two rows stored alone on their own layers - a row piece
    ranks as that row's own lattice - while the uncut lattice (16 MiB) picks
    differently (documented: outside the identity guarantee);
  * the extra-sparsening diagnostic FLOE_RUST_WIDTH_C=2 (ADAPTIVE_CUT_DENSITY_PLAN
    §4.2 candidate 1): a 1.5 px array's four-pan column share is (1 + 1/3) / 1.5
    of its covered share (P_2(0.5) = 1/3) within 0.025, the 0.5 px array
    keeps 2/3 of its covered share (0.55..0.8), every column it lights is lit
    under the plain rule too (a narrower box lies within the plain one; the
    pixels differ where a stippled interior column becomes the rim), and an
    out-of-range value (0.5) is the plain rule pixel for pixel;
  * the survivor list (§4.3 step 1 in the renderer, 2026-09-25): the whole
    view with its sub-pixel arrays is byte-identical under the kill switch
    FLOE_RUST_SURVIVOR_LIST=off, which walks more members (the 0.1 and 0.25 px
    arrays are listed; the 0.5 px ones cost as much either way and are walked);
  * the placement lattice (CUT_DENSITY_DESIGN §10.8, diagnostic
    FLOE_RUST_PLACE_LATTICE=on): 0.2 px bars and 0.3 px triangles, 120 x 3,
    stored as shapes of TOP (OASIS repetitions) and as one cell placed by an
    array, light the same pixels at a whole and a fractional pan; with both
    visible the frame is byte-identical with the survivor list off, which
    visits more cells; the frame reports the walk (place_walks walked2).

    .venv/bin/python tools/validate_area_true.py
"""
import math
import os
from pathlib import Path
import shutil
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
ROW = (500.0, 3.0)            # um: one lattice row stored three ways, layers 8, 9, 10
ROW_LAYERS = ((8, 0), (9, 0), (10, 0))
SPLIT_LAYER = (11, 0)         # the page split layout: a lattice among single rectangles
SPLIT_AT = (490.4, 3.0)       # um: the lattice's first bar
ROWS_AT = (100.0, 499.6)      # um: the 2 x 64 lattice the tall layout's split cuts into rows
ROW_A, ROW_B = (12, 0), (13, 0)   # its two rows stored alone
PLACE_AT = (700.0, 3.0)       # um: a lattice of sub-pixel bars and triangles, flat and as a placement array
PLACE_FLAT, PLACE_ARRAY = (14, 0), (15, 0)
SPLIT_PANS = ((0.0, 0.0), (0.2, 0.0), (0.37, 0.0), (0.5, 0.29), (0.81, 0.63))
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
    # one row of 64 bars (0.15 x 6 um, pitch 0.3 um) stored three ways
    bw, bh, pitch = 0.15, 6.0, 0.3
    x0, y0 = ROW
    whole, half, col = ly.layer(*ROW_LAYERS[0]), ly.layer(*ROW_LAYERS[1]), ly.layer(*ROW_LAYERS[2])
    for i in range(64):
        top.shapes(whole).insert(kdb.DBox(x0 + i * pitch, y0, x0 + i * pitch + bw, y0 + bh))
    cell = ly.create_cell('HALF')
    for i in range(32):
        cell.shapes(half).insert(kdb.DBox(i * pitch, 0.0, i * pitch + bw, bh))
    for k in range(2):
        top.insert(kdb.CellInstArray(cell.cell_index(), kdb.Trans(int(round((x0 + 32 * k * pitch) / ly.dbu)),
                                                                   int(round(y0 / ly.dbu)))))
    # a column cell rotated by 90 degrees onto the row: (x, y) -> (-y, x)
    column = ly.create_cell('COL')
    for i in range(64):
        wx = x0 + i * pitch
        column.shapes(col).insert(kdb.DBox(y0, -(wx + bw), y0 + bh, -wx))
    top.insert(kdb.CellInstArray(column.cell_index(), kdb.Trans(1, False, 0, 0)))
    # the placement lattice: a 0.02 um bar and a 0.03 um triangle (0.2 and 0.3 px
    # wide) repeated 120 x 3 times at 0.07 x 3 um, as shapes of TOP (written as
    # OASIS repetitions) and as one cell placed by an array
    px, py = PLACE_AT
    flat, placed = ly.layer(*PLACE_FLAT), ly.layer(*PLACE_ARRAY)
    bar = lambda x, y: kdb.DBox(x, y, x + 0.02, y + 2.0)
    tri = lambda x, y: kdb.DPolygon([kdb.DPoint(x + 0.03, y), kdb.DPoint(x + 0.06, y), kdb.DPoint(x + 0.03, y + 2.0)])
    for j in range(3):
        for i in range(120):
            top.shapes(flat).insert(bar(px + i * 0.07, py + j * 3.0))
            top.shapes(flat).insert(tri(px + i * 0.07, py + j * 3.0))
    unit = ly.create_cell('LATTICE_UNIT')
    unit.shapes(placed).insert(bar(0.0, 0.0))
    unit.shapes(placed).insert(tri(0.0, 0.0))
    top.insert(kdb.CellInstArray(unit.cell_index(), kdb.Trans(int(round(px / ly.dbu)), int(round(py / ly.dbu))),
                                 kdb.Vector(70, 0), kdb.Vector(0, 3000), 120, 3))
    ly.write(str(path))


def split_layout(path, rows=False):
    """A 64 x 6 lattice of 0.15 x 0.45 um bars (pitch 0.3 x 0.8 um: 1.5 x 4.5 px
    at 3 x 8 px) in the middle of 70,000 rectangles of distinct sizes (no
    repetition) 17 um and more above it on the same layer: 1.1 MB of records,
    over a 1 MiB page target, so the indexer splits the layer across x through
    the lattice. With `rows`, a tall layout instead: a 2 x 64 lattice with
    half the rectangles below it and half above (the median record, where the
    indexer puts its plane, is the lattice itself), so the split runs between
    the rows; the same two rows stored alone on ROW_A and ROW_B."""
    import klayout.db as kdb
    ly = kdb.Layout()
    ly.dbu = 0.001
    top = ly.create_cell('TOP')
    li = ly.layer(*SPLIT_LAYER)
    (x0, y0), nrows = (ROWS_AT, 2) if rows else (SPLIT_AT, 6)
    for j in range(nrows):
        for i in range(64):
            top.shapes(li).insert(kdb.DBox(x0 + i * 0.3, y0 + j * 0.8, x0 + i * 0.3 + 0.15, y0 + j * 0.8 + 0.45))
            if rows:
                top.shapes(ly.layer(*(ROW_A, ROW_B)[j])).insert(
                    kdb.DBox(x0 + i * 0.3, y0 + j * 0.8, x0 + i * 0.3 + 0.15, y0 + j * 0.8 + 0.45))
    state = 12345
    for k in range(70000):
        state = (state * 6364136223846793005 + 1442695040888963407) % (1 << 64)
        a = (state >> 33)
        state = (state * 6364136223846793005 + 1442695040888963407) % (1 << 64)
        b = (state >> 33)
        if rows:
            x = a % 200000 / 1000.0
            # the low half ends 2 um under the lattice: the view around it sees no scatter
            y = 20.0 + b % 477000 / 1000.0 if k % 2 == 0 else 521.0 + b % 459000 / 1000.0
        else:
            x, y = a % 1000000 / 1000.0, 20.0 + b % 20000 / 1000.0
        w, h = 0.02 + (k % 500) * 0.001, 0.02 + (k // 500) * 0.001
        top.shapes(li).insert(kdb.DBox(x, y, x + w, y + h))
    ly.write(str(path))


def lit_pixels(pixels):
    return {i // 4 for i in range(0, len(pixels), 4) if pixels[i:i + 4] != BLACK}


def worker(src, on, wash=False, lod=False, width_c=None, survivor_list=None, place_lattice=False):
    if on:
        os.environ.pop('FLOE_RUST_AREA_TRUE', None)
    else:
        os.environ['FLOE_RUST_AREA_TRUE'] = 'off'
    if wash:
        os.environ['FLOE_RUST_PAGE_WASH'] = 'on'
    if lod:
        os.environ['FLOE_RUST_LOD'] = 'on'
    if width_c is not None:
        os.environ['FLOE_RUST_WIDTH_C'] = str(width_c)
    if survivor_list is not None:
        os.environ['FLOE_RUST_SURVIVOR_LIST'] = survivor_list
    if place_lattice:
        os.environ['FLOE_RUST_PLACE_LATTICE'] = 'on'
    cache = Cache(str(src))
    cache.load()
    w = RustRenderWorker(cache)
    w.start()
    for name in ('FLOE_RUST_AREA_TRUE', 'FLOE_RUST_PAGE_WASH', 'FLOE_RUST_LOD', 'FLOE_RUST_WIDTH_C', 'FLOE_RUST_SURVIVOR_LIST',
                 'FLOE_RUST_PLACE_LATTICE'):
        os.environ.pop(name, None)
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
            # the extra-sparsening diagnostic: c = 2 thins the extra pixels by
            # P_2(f) = f / (2 - f), within the plain rule's pixels; a value
            # under 1 is the plain rule
            sparse, plain = worker(src, True, width_c=2), worker(src, True, width_c=0.5)
            try:
                gen += 1
                assert frame(plain, gen, view) == now, 'FLOE_RUST_WIDTH_C=0.5 changed the plain rule'
                n = WIDE.index([(1.5, 1.5)])
                shares = []
                for phase in (0.0, 0.25, 0.5, 0.75):
                    gen += 1
                    moved = (view[0] + phase * PX_UM, view[1], view[2] + phase * PX_UM, view[3])
                    flags, _ = columns(frame(sparse, gen, moved), moved, n)
                    shares.append(sum(flags) / len(flags))
                mean, want = sum(shares) / len(shares), covered(n) * (1 + 0.5 / 1.5) / 1.5
                assert abs(mean - want) <= 0.025, 'c = 2: %s column share %.3f over four pans for %.3f expected' % (name(n), mean, want)
                gen += 1
                thinned = frame(sparse, gen, view)
                k = len(WIDE) + THIN.index([(0.5, 0.5)])
                _, lit = columns(thinned, view, k)
                ratio = lit / covered(k)
                assert 0.55 <= ratio <= 0.8, 'c = 2: %s lit %.3f of covered %.3f' % (name(k), lit, covered(k))
                # the narrower box lies within the plain one: no new column
                # (pixels differ where a stippled interior column turns rim)
                extra = {px % W for px in lit_pixels(thinned)} - {px % W for px in lit_pixels(now)}
                assert not extra, 'c = 2 lit %d columns the plain rule does not' % len(extra)
                print('width c = 2: %s column share %.3f for %.3f expected, %s keeps %.2f of its cover, %d px lit for %d (no new column)'
                      % (name(n), mean, want, name(k), ratio, len(lit_pixels(thinned)), len(lit_pixels(now))))
            finally:
                sparse.stop()
                plain.stop()
            # the survivor list (ADAPTIVE_CUT_DENSITY_PLAN §4.3 step 1 in the
            # renderer, 2026-09-25): the sub-pixel arrays walk only the members
            # that can survive - the member walk's pixels (the kill switch
            # FLOE_RUST_SURVIVOR_LIST=off walks them all), fewer members
            walker = worker(src, True, survivor_list='off')
            try:
                gen += 1
                listed, lreport = frame(on, gen, view, report=True)
                walked, wreport = frame(walker, gen, view, report=True)
                assert listed == walked == now, 'the survivor list changed the pixels (%d vs %d px lit)' \
                    % (len(lit_pixels(listed)), len(lit_pixels(walked)))
                assert lreport['rep_members_tested'] < wreport['rep_members_tested'], \
                    'the survivor list walked %d members, the member walk %d' % (lreport['rep_members_tested'], wreport['rep_members_tested'])
                print('survivor list: the same %d px, %d members walked instead of %d'
                      % (len(lit_pixels(listed)), lreport['rep_members_tested'], wreport['rep_members_tested']))
            finally:
                walker.stop()
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
            # the LOD swap: off by default, FLOE_RUST_LOD=on swaps a merged variant in
            dense = Path(temp) / 'dense.oas'
            import klayout.db as kdb
            ly = kdb.Layout()
            ly.dbu = 0.001
            top = ly.create_cell('TOP')
            li = ly.layer(7, 0)
            for j in range(120):
                for i in range(120):
                    top.shapes(li).insert(kdb.DBox(i * 0.1, j * 0.1, i * 0.1 + 0.05, j * 0.1 + 0.05))
            ly.write(str(dense))
            done = subprocess.run([sys.executable, '-B', '-m', 'floe2', 'index', str(dense), '--lod'],
                                  cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
            assert done.returncode == 0, done.stdout + done.stderr
            swaps = {}
            for lod in (False, True):
                w = worker(dense, True, lod=lod)
                try:
                    _, rep = frame(w, 1, (-50.0, -50.0, 60.0, 60.0), visible=((7, 0),), size=(200, 200), report=True)
                    swaps[lod] = rep['plan_culls']['lod_swapped']
                finally:
                    w.stop()
            assert swaps[False] == 0 and swaps[True] > 0, 'LOD swaps: default %d, FLOE_RUST_LOD=on %d' % (swaps[False], swaps[True])
            print('lod swap: default swaps 0 pages; on, %d' % swaps[True])
            # the same lattice row stored three ways lights the same pixels
            size = (240, 80)
            for pan in (0.0, 0.37):
                box = (ROW[0] - 2.0 + pan * PX_UM, ROW[1] - 1.0, ROW[0] - 2.0 + pan * PX_UM + size[0] * PX_UM,
                       ROW[1] - 1.0 + size[1] * PX_UM)
                lit = []
                for layer in ROW_LAYERS:
                    gen += 1
                    pixels = frame(on, gen, box, visible=(layer,), size=size)
                    lit.append({i // 4 for i in range(0, len(pixels), 4) if pixels[i:i + 4] != BLACK})
                assert lit[0] and lit[0] == lit[1] == lit[2], \
                    'the lattice row at a %g px pan: %d / %d / %d px, %d and %d differ from the array' % (
                        pan, len(lit[0]), len(lit[1]), len(lit[2]), len(lit[0] ^ lit[1]), len(lit[0] ^ lit[2]))
            print('lattice ranks: one array, two cell placements and a rotated column light the same %d px'
                  % len(lit[0]))
            # the placement lattice (CUT_DENSITY_DESIGN §10.8, diagnostic): the
            # sub-pixel bars and triangles placed by an array rank as their flat
            # arrays - the same pixels at whole and fractional pans - and the
            # survivor walk of the placement array draws what visiting every
            # member draws (the kill switch of the list), visiting fewer cells
            lattice_on = worker(src, True, place_lattice=True)
            lattice_all = worker(src, True, place_lattice=True, survivor_list='off')
            try:
                size = (240, 100)
                for pan in (0.0, 0.37):
                    box = (PLACE_AT[0] - 1.0 + pan * PX_UM, PLACE_AT[1] - 1.0, PLACE_AT[0] - 1.0 + pan * PX_UM + size[0] * PX_UM,
                           PLACE_AT[1] - 1.0 + size[1] * PX_UM)
                    lit = {}
                    for kind, w, layer in (('flat', lattice_on, PLACE_FLAT), ('placed', lattice_on, PLACE_ARRAY), ('off', on, PLACE_FLAT)):
                        gen += 1
                        lit[kind] = lit_pixels(frame(w, gen, box, visible=(layer,), size=size))
                    assert lit['flat'] and lit['flat'] == lit['placed'], 'placement lattice at a %g px pan: flat %d px, placed %d px, %d differ' % (
                        pan, len(lit['flat']), len(lit['placed']), len(lit['flat'] ^ lit['placed']))
                    gen += 1
                    listed, lreport = frame(lattice_on, gen, box, visible=(PLACE_FLAT, PLACE_ARRAY), size=size, report=True)
                    every, ereport = frame(lattice_all, gen, box, visible=(PLACE_FLAT, PLACE_ARRAY), size=size, report=True)
                    assert listed == every, 'the placement survivor walk changed %d px' % len(lit_pixels(listed) ^ lit_pixels(every))
                    assert lreport['hier_cells_visited'] < ereport['hier_cells_visited'], (lreport['hier_cells_visited'], ereport['hier_cells_visited'])
                    # the walk's outcome reaches the frame (RenderStats::place_walks):
                    # the 120 x 3 array walked, the list off plans nothing
                    assert lreport['place_walks'].get('walked2', [0])[0] > 0 and not ereport['place_walks'], \
                        (lreport['place_walks'], ereport['place_walks'])
                print('placement lattice: the placed bars and triangles light the flat arrays\' %d px at 2 pans (the rule off: %d px); '
                      'the survivor walk draws the same, visiting %d cells instead of %d (place walks %s)'
                      % (len(lit['flat']), len(lit['off']), lreport['hier_cells_visited'], ereport['hier_cells_visited'], lreport['place_walks']))
            finally:
                lattice_on.stop()
                lattice_all.stop()
            # the indexer's own page split: one layout, a 16 MiB and a 1 MiB page target
            whole_src, split_src = Path(temp) / 'split16.oas', Path(temp) / 'split1.oas'
            split_layout(whole_src)
            shutil.copy(whole_src, split_src)
            for src, mb in ((whole_src, 16), (split_src, 1)):
                done = subprocess.run([sys.executable, '-B', '-m', 'floe2', 'index', str(src), '--page-target-mb', str(mb)],
                                      cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
                assert done.returncode == 0, done.stdout + done.stderr
                pieces = 'rep-split 1 fragments (2 grid pieces: 0 one-row of a 2-D grid, 0 one-member)'
                assert (pieces in done.stderr) == (mb == 1), '%d MiB build log:\n%s' % (mb, done.stderr)
            pair = [worker(whole_src, True), worker(split_src, True)]
            try:
                size, covered_px = (240, 80), 64 * 6 * 1.5 * 4.5
                for px, py in SPLIT_PANS:
                    bx, by = SPLIT_AT[0] - 2.4 + px * PX_UM, SPLIT_AT[1] - 1.0 + py * PX_UM
                    box = (bx, by, bx + size[0] * PX_UM, by + size[1] * PX_UM)
                    lit, pages = [], []
                    for w in pair:
                        gen += 1
                        pixels, report = frame(w, gen, box, visible=(SPLIT_LAYER,), size=size, report=True)
                        lit.append({i // 4 for i in range(0, len(pixels), 4) if pixels[i:i + 4] != BLACK})
                        pages.append(report['tiles'])
                    assert pages == [1, 2], 'page split at a (%g, %g) px pan: %s pages, expected 1 and 2' % (px, py, pages)
                    assert lit[0] == lit[1], 'page split at a (%g, %g) px pan: %d / %d px, %d differ' % (
                        px, py, len(lit[0]), len(lit[1]), len(lit[0] ^ lit[1]))
                    assert abs(len(lit[0]) / covered_px - 1.0) <= 0.02, \
                        'page split lattice: %d px lit for %.0f covered' % (len(lit[0]), covered_px)
            finally:
                for w in pair:
                    w.stop()
            print('page split: the lattice in 1 page (16 MiB) and cut into 2 (1 MiB) lights the same %d px '
                  '(%.0f covered) at %d pans' % (len(lit[0]), covered_px, len(SPLIT_PANS)))
            # a 2-row lattice cut across its rows: the one-row pieces rank as
            # the rows stored alone, apart from the uncut lattice
            rows_src, uncut_src = Path(temp) / 'rows1.oas', Path(temp) / 'rows16.oas'
            split_layout(rows_src, rows=True)
            shutil.copy(rows_src, uncut_src)
            for src, mb in ((rows_src, 1), (uncut_src, 16)):
                done = subprocess.run([sys.executable, '-B', '-m', 'floe2', 'index', str(src), '--page-target-mb', str(mb)],
                                      cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
                assert done.returncode == 0, done.stdout + done.stderr
                pieces = 'rep-split 1 fragments (2 grid pieces: 2 one-row of a 2-D grid, 0 one-member)'
                assert (pieces in done.stderr) == (mb == 1), '%d MiB build log:\n%s' % (mb, done.stderr)
            pair = [worker(rows_src, True), worker(uncut_src, True)]
            try:
                size, covered_px, apart = (240, 80), 2 * 64 * 1.5 * 4.5, []
                for px, py in SPLIT_PANS:
                    bx, by = ROWS_AT[0] - 2.4 + px * PX_UM, ROWS_AT[1] - 1.0 + py * PX_UM
                    box = (bx, by, bx + size[0] * PX_UM, by + size[1] * PX_UM)
                    gen += 1
                    cut, report = frame(pair[0], gen, box, visible=(SPLIT_LAYER,), size=size, report=True)
                    assert report['tiles'] == 2, 'row split at a (%g, %g) px pan: %d pages' % (px, py, report['tiles'])
                    gen += 1
                    alone = frame(pair[0], gen, box, visible=(ROW_A, ROW_B), size=size)
                    gen += 1
                    uncut = frame(pair[1], gen, box, visible=(SPLIT_LAYER,), size=size)
                    cut, alone, uncut = lit_pixels(cut), lit_pixels(alone), lit_pixels(uncut)
                    assert cut and cut == alone, 'row pieces at a (%g, %g) px pan: %d / %d px, %d differ from the rows stored alone' % (
                        px, py, len(cut), len(alone), len(cut ^ alone))
                    assert abs(len(cut) / covered_px - 1.0) <= 0.03, 'row pieces: %d px lit for %.0f covered' % (len(cut), covered_px)
                    apart.append(len(cut ^ uncut))
            finally:
                for w in pair:
                    w.stop()
            print('row pieces: a 2 x 64 lattice cut into one-row pieces lights the same %d px (%.0f covered) as the rows '
                  'stored alone at %d pans; the uncut lattice differs in %s px' % (len(cut), covered_px, len(SPLIT_PANS), apart))
        finally:
            on.stop()
            off.stop()
    print('AREA TRUE: OK')


if __name__ == '__main__':
    main()
