#!/usr/bin/env python3
"""Density stack gate (floe_render_core::GeometryRasterRequest::density_stack,
CUT_DENSITY_DESIGN §10.10; user direction 2026-09-27: the shapes under the
cut are the density - hairlines have their own drawing - drawn in a second
pass into the space the originals left, the top layer first).

With the diagnostic FLOE_RUST_DENSITY_STACK=top the frame is drawn twice:
pass 1 paints the plan as always (max mode: every shape whose larger side
reaches the cut) and records what its originals cover - speckle holes
included; pass 2 plans the same view at a finer cut (1 px, the larger
side) and reads its pages only where pass 1 left a pixel: the top layer's
shapes under the cut draw over the lower layers' originals but not inside
its own, every other layer's only where no original covers the pixel and no
density above stands for it.

One layout written with klayout.db, 0.1 um a pixel, 400 x 200 px: layer 1/0
(the lowest) is a field of 0.15 um (1.5 px) squares 0.5 um apart over the
whole view - pages the cut drops whole, so pass 2 decodes them; layer 2/0 a
rectangle over the top right quadrant; layer 4/0 (the top one) a rectangle
over the left half, 1.5 px squares inside it and 1.5 px squares over layer
2's rectangle; layer 3/0 the left rectangle alone (the reference, never drawn
with the others). Every layer takes the viewer's default speckle; the cut is
3 px (detail medium).

  * without the variable no square draws (the cut); with it the bottom right
    quadrant (nothing above) holds layer 1's squares pixel for pixel as a
    cut-free frame of layer 1 alone draws them there;
  * inside the left rectangle exactly the rectangle alone lights - no square
    of layer 1 in its holes, none of the top layer's own either;
  * in the top right quadrant the top layer's squares light over layer 2's
    rectangle in the top layer's colour, exactly the pixels a cut-free frame
    of the top layer alone lights there, and everything else there is
    layer 2's rectangle as without the variable;
  * pass 2 draws the VISIBLE layers only (field 2026-09-27: with one layer
    on, the other layers' density showed): with layer 1/0 alone on, the frame
    is byte-identical to the cut-free frame of layer 1/0 (its squares are the
    top plane's density, nothing above); with 1/0 and 2/0 on, the top right
    quadrant is 2/0's rectangle alone - none of 4/0's squares;
  * the viewer's margin frame (bg, twice the extent per axis) draws the view
    pixel for pixel as the viewport frame did: pass 2 plans to a reserve of
    its own and its budget fit is remembered per scale and side (2026-09-27:
    planned to half of what the generation had left, a margin's pass 2 on
    the synthetic chip's fit view decoded 23 pages where the viewport's
    decoded 2,208 and 8,110 px changed when it landed);
  * the frame reports the stack's counts (density_stack: lit, top, lower,
    covered, claimed) and pass 2's pages (density_pages: planned, in_hand,
    decoded, over_budget - some decoded); none without the variable, under
    FLOE_RUST_AREA_TRUE=off or FLOE_RUST_WRITE_ONCE=off, which draw as
    without the variable.

The sub-cut dots (FLOE_RUST_DENSITY_DOTS=on with the stack, CUT_DENSITY_DESIGN
§10.12; user 2026-09-30: "a cell of 3 x 3 px or less is one dot, no
descent"): a second layout places a cell DOT (a 0.15 um = 1.5 px square on
1/0) three ways - a 10 x 10 array at a 6 px pitch, a 40 x 40 array that
abuts, and one alone. Pass 2 plans the cells at pass 1's cut and counts a
cell under it as dots in 4 x 4 px blocks, never walking into it:

  * the sparse array lights exactly 100 pixels, each within 2 px of a
    member's centre; the lone DOT one; the abutting array per 4 x 4 block
    min(8, the members whose centre lies in it) - summed here from the
    member positions - and nothing else lights;
  * density_dots reports the items (one per block) and none over the cap;
    without the variable the stack walks into DOT and reports no dots;
  * the margin frame draws the view as the viewport frame did;
  * step 2 (a floor by the work): TOP's own 0.05 um (0.5 px) squares at a
    3 px pitch are decoded under the dots (their pages fit the reserve at a
    zero floor) and draw as a cut-free frame draws them there; the stack's
    1 px floor alone leaves them out;
  * step 3 (progressive): the dots' frame arrives twice - first a refining
    round (final=0) holding pass 1 alone, byte for byte the frame without the
    stack, then the final frame, byte for byte what the dots draw in one
    round (FLOE_RUST_DENSITY_PROGRESSIVE=off); a margin frame has no first
    round.

    .venv/bin/python tools/validate_density_stack.py
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

W, H = 400, 200
PX_UM = 0.1
LOW, MID, ALONE, TOP = (1, 0), (2, 0), (3, 0), (4, 0)
BLACK = bytes((0, 0, 0, 255))
VIEW = (0.0, 0.0, W * PX_UM, H * PX_UM)
SQUARE = 0.15               # um: 1.5 px, under the 3 px cut, over pass 2's 1 px


def layout(path):
    import klayout.db as kdb
    ly = kdb.Layout()
    ly.dbu = 0.001
    top = ly.create_cell('TOP')
    low = ly.layer(*LOW)
    for j in range(H // 5):
        for i in range(W // 5):
            x, y = i * 0.5 + 0.17, j * 0.5 + 0.13
            top.shapes(low).insert(kdb.DBox(x, y, x + SQUARE, y + SQUARE))
    top.shapes(ly.layer(*MID)).insert(kdb.DBox(20.0, 10.0, 40.0, 20.0))
    for layer in (TOP, ALONE):
        top.shapes(ly.layer(*layer)).insert(kdb.DBox(0.0, 0.0, 20.0, 20.0))
    dots = ly.layer(*TOP)
    for j in range(20):
        for i in range(20):
            x, y = 2.0 + i * 0.7, 2.0 + j * 0.7
            top.shapes(dots).insert(kdb.DBox(x, y, x + SQUARE, y + SQUARE))
            x, y = 22.0 + i * 0.8, 11.0 + j * 0.4
            top.shapes(dots).insert(kdb.DBox(x, y, x + SQUARE, y + SQUARE))
    ly.write(str(path))


DOT = 0.15                  # um: the DOT cell's square, 1.5 px
SPARSE = (2030, 2030, 600, 10)      # dbu origin x, y, pitch, n: 6 px apart
DENSE = (20000, 4000, 150, 40)      # abutting: the pitch is the square
ALONE_DOT = (12030, 15030)
TINY = (30.03, 12.03, 20, 10)       # um origin x, y, columns, rows: cols 300-360, rows 50-80


def dots_layout(path):
    import klayout.db as kdb
    ly = kdb.Layout()
    ly.dbu = 0.001
    top = ly.create_cell('TOP')
    dot = ly.create_cell('DOT')
    dot.shapes(ly.layer(*LOW)).insert(kdb.DBox(0.0, 0.0, DOT, DOT))
    for (x, y, pitch, n) in (SPARSE, DENSE):
        top.insert(kdb.CellInstArray(dot.cell_index(), kdb.Trans(kdb.Vector(x, y)), kdb.Vector(pitch, 0), kdb.Vector(0, pitch), n, n))
    top.insert(kdb.CellInstArray(dot.cell_index(), kdb.Trans(kdb.Vector(*ALONE_DOT))))
    # TOP's own specks: 0.05 um squares 0.3 um apart, under a 1 px floor
    low = ly.layer(*LOW)
    for j in range(TINY[3]):
        for i in range(TINY[2]):
            x, y = TINY[0] + i * 0.3, TINY[1] + j * 0.3
            top.shapes(low).insert(kdb.DBox(x, y, x + 0.05, y + 0.05))
    # an original for pass 1 to draw (2/0, away from the rest)
    top.shapes(ly.layer(*MID)).insert(kdb.DBox(36.0, 1.0, 39.0, 5.0))
    ly.write(str(path))


def dots_checks(temp):
    src = Path(temp) / 'dots.oas'
    dots_layout(src)
    done = subprocess.run([sys.executable, '-B', '-m', 'floe2', 'index', str(src)],
                          cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
    assert done.returncode == 0, done.stdout + done.stderr
    workers = {
        'off': worker(src, {}),
        'stack': worker(src, {'FLOE_RUST_DENSITY_STACK': 'top'}),
        'dots': worker(src, {'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on'}),
        'one_round': worker(src, {'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on', 'FLOE_RUST_DENSITY_PROGRESSIVE': 'off'}),
    }
    try:
        on, res = frame(workers['dots'], 1, (LOW,))
        walked, walked_res = frame(workers['stack'], 1, (LOW,))
        half = int(round(DOT * 1000)) // 2
        # the device position of a dbu point (0.1 um a pixel, rows from the top)
        dev = lambda x, y: (x / 100.0, (VIEW[3] * 1000 - y) / 100.0)
        # the sparse array: one dot a member, beside its centre
        x0, y0, pitch, n = SPARSE
        centres = [dev(x0 + half + i * pitch, y0 + half + j * pitch) for i in range(n) for j in range(n)]
        sparse = lit(on, range(10, 90), range(110, 195))
        assert len(sparse) == n * n, 'sparse array: %d px lit, want %d' % (len(sparse), n * n)
        for (c, r) in sparse:
            assert min(abs(c + 0.5 - cx) + abs(r + 0.5 - cy) for (cx, cy) in centres) <= 2.0, 'sparse dot (%d, %d) far from a member' % (c, r)
        alone_px = lit(on, range(110, 135), range(35, 65))
        assert len(alone_px) == 1, 'the lone DOT: %d px' % len(alone_px)
        # the abutting array: min(8, the members centred in it) a 4 x 4 block
        x0, y0, pitch, n = DENSE
        blocks = {}
        for i in range(n):
            for j in range(n):
                key = ((x0 + half + i * pitch) // 400, (y0 + half + j * pitch) // 400)
                blocks[key] = blocks.get(key, 0) + 1
        want = sum(min(8, count) for count in blocks.values())
        dense = lit(on, range(195, 265), range(95, 165))
        assert len(dense) == want, 'abutting array: %d px lit, want %d over %d blocks' % (len(dense), want, len(blocks))
        # step 2: TOP's specks draw as a cut-free frame draws them
        free, _ = frame(workers['stack'], 3, (LOW,), cut_px=0.0)
        specks = (range(295, 365), range(45, 85))
        tiny = lit(on, *specks)
        assert tiny and tiny == lit(free, *specks), "TOP's specks: %d px lit, %d cut-free" % (len(tiny), len(lit(free, *specks)))
        assert not lit(walked, *specks), "the stack's 1 px floor draws no speck"
        everything = lit(on, range(W), range(H))
        assert everything == sparse | alone_px | dense | tiny, '%d px lit outside the arrays' % len(everything - sparse - alone_px - dense - tiny)
        dots = res.get('density_dots')
        assert dots and dots['items'] == SPARSE[3] ** 2 + 1 + len(blocks) and dots['over'] == 0, (dots, len(blocks))
        assert walked_res.get('density_dots') is None and lit(walked, range(W), range(H)), 'the stack alone draws the DOT squares, no dots'
        print('density stack dots: sparse array %d dots beside the members, lone DOT 1, abutting array %d px = sum of min(8, members) '
              'over %d blocks; density_dots %s; TOP specks %d px as cut-free' % (len(sparse), len(dense), len(blocks), dots, len(tiny)))
        margin, _ = frame_bg(workers['dots'], 2, (LOW,))
        centre = b''.join(margin[((H // 2 + r) * 2 * W + W // 2) * 4:((H // 2 + r) * 2 * W + W // 2 + W) * 4] for r in range(H))
        assert centre == on, 'the dots margin draws the view otherwise in %d px' % sum(
            1 for i in range(0, len(on), 4) if centre[i:i + 4] != on[i:i + 4])
        print('density stack dots: the margin frame draws the view as the viewport frame did')
        # step 3: pass 1 first, then the frame with the dots
        both = (LOW, MID)
        rounds = frames_of(workers['dots'], 10, both)
        single = frames_of(workers['one_round'], 10, both)
        plain, _ = frame(workers['off'], 10, both)
        assert len(single) == 1 and len(rounds) == 2 and rounds[0][1].get('refining'), [r.get('refining') for _, r in rounds]
        assert lit(plain, range(W), range(H)), 'pass 1 draws the 2/0 original'
        assert rounds[0][0] == plain, 'the first round is not pass 1 alone (%d px differ)' % sum(
            1 for i in range(0, len(plain), 4) if rounds[0][0][i:i + 4] != plain[i:i + 4])
        assert rounds[1][0] == single[0][0] and rounds[1][0] != plain, 'the final round differs from one round'
        assert len(frames_of(workers['dots'], 11, both, bg=True)) == 1, 'a margin has no first round'
        print('density stack dots: progressive - a refining round of pass 1 (= the frame without the stack), then the one-round frame; '
              'none for a margin')
    finally:
        for w in workers.values():
            w.stop()


def frames_of(w, gen, visible, bg=False):
    """Every frame answer of one render, the refining rounds first: [(pixels,
    result)], the last one final."""
    dbu = float(w.cache.meta['dbu'])
    if bg:
        vw, vh = VIEW[2] - VIEW[0], VIEW[3] - VIEW[1]
        box, size = (VIEW[0] - vw / 2, VIEW[1] - vh / 2, VIEW[2] + vw / 2, VIEW[3] + vh / 2), (2 * W, 2 * H)
    else:
        box, size = VIEW, (W, H)
    job = {'kind': 'render', 'gen': gen, 'scope': 'live', 'bbox': tuple(v / dbu for v in box), 'view': tuple(v / dbu for v in VIEW),
           'w': size[0], 'h': size[1], 'depth': None, 'cut_px': 3.0, 'lod': False, 'frames': False, 'labels': False,
           'abstract': False, 'visible': list(visible), 'frame_format': 'raw', 'thin': 'keep', 'frame_cache': False}
    if bg:
        job['bg'] = True
    w.submit(job)
    out = []
    deadline = time.monotonic() + 300
    while time.monotonic() < deadline:
        res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
        assert res.get('kind') not in ('error', 'dropped'), res
        if res.get('kind') == 'frame' and res.get('gen') == gen:
            out.append((bytes(res.pop('rgba')), res))
            if not res.get('refining'):
                return out
    raise AssertionError('progressive frames timeout')


def worker(src, env):
    for name, value in env.items():
        os.environ[name] = value
    cache = Cache(str(src))
    cache.load()
    w = RustRenderWorker(cache)
    w.start()
    for name in env:
        os.environ.pop(name, None)
    return w


def frame(w, gen, visible, cut_px=3.0):
    dbu = float(w.cache.meta['dbu'])
    w.submit({'kind': 'render', 'gen': gen, 'scope': 'headless', 'bbox': tuple(v / dbu for v in VIEW), 'view': None,
              'w': W, 'h': H, 'depth': None, 'cut_px': cut_px, 'lod': False, 'frames': False,
              'labels': False, 'abstract': False, 'visible': list(visible), 'frame_format': 'raw',
              'thin': 'keep', 'frame_cache': False})
    deadline = time.monotonic() + 300
    while time.monotonic() < deadline:
        res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
        assert res.get('kind') != 'error', res
        if res.get('kind') == 'frame' and res.get('gen') == gen and not res.get('refining'):
            return bytes(res.pop('rgba')), res
    raise AssertionError('density stack frame timeout')


def frame_bg(w, gen, visible, cut_px=3.0):
    """The viewer's margin around VIEW: twice the extent per axis at the same
    scale, flagged bg (gui._submit_margin)."""
    dbu = float(w.cache.meta['dbu'])
    vw, vh = VIEW[2] - VIEW[0], VIEW[3] - VIEW[1]
    box = (VIEW[0] - vw / 2, VIEW[1] - vh / 2, VIEW[2] + vw / 2, VIEW[3] + vh / 2)
    w.submit({'kind': 'render', 'gen': gen, 'scope': 'live', 'bg': True, 'bbox': tuple(v / dbu for v in box),
              'view': tuple(v / dbu for v in VIEW), 'w': 2 * W, 'h': 2 * H, 'depth': None, 'cut_px': cut_px, 'lod': False,
              'frames': False, 'labels': False, 'abstract': False, 'visible': list(visible), 'frame_format': 'raw',
              'thin': 'keep', 'frame_cache': False})
    deadline = time.monotonic() + 300
    while time.monotonic() < deadline:
        res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
        assert res.get('kind') not in ('error', 'dropped'), res
        if res.get('kind') == 'frame' and res.get('gen') == gen and not res.get('refining'):
            return bytes(res.pop('rgba')), res
    raise AssertionError('density stack margin frame timeout')


def px(pixels, c, r):
    return pixels[(r * W + c) * 4:(r * W + c) * 4 + 4]


def lit(pixels, cols, rows):
    return {(c, r) for r in rows for c in cols if px(pixels, c, r) != BLACK}


def main():
    os.environ['FLOE_INDEX_BIN'] = str(ROOT / 'rust/target/release/floe-index')
    os.environ['FLOE_RENDERD_BIN'] = str(ROOT / 'rust/target/release/floe-renderd')
    os.environ['FLOE_RUST_RETAINED_MB'] = '0'
    for name in ('FLOE_RUST_DENSITY_STACK', 'FLOE_RUST_DENSITY_DOTS', 'FLOE_RUST_AREA_TRUE', 'FLOE_RUST_WRITE_ONCE'):
        os.environ.pop(name, None)
    with tempfile.TemporaryDirectory(prefix='floe-density-stack-') as temp:
        src = Path(temp) / 'stack.oas'
        layout(src)
        done = subprocess.run([sys.executable, '-B', '-m', 'floe2', 'index', str(src)],
                              cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
        assert done.returncode == 0, done.stdout + done.stderr
        workers = {
            'off': worker(src, {}),
            'on': worker(src, {'FLOE_RUST_DENSITY_STACK': 'top'}),
            'klayout': worker(src, {'FLOE_RUST_AREA_TRUE': 'off'}),
            'klayout_on': worker(src, {'FLOE_RUST_AREA_TRUE': 'off', 'FLOE_RUST_DENSITY_STACK': 'top'}),
            'ordered': worker(src, {'FLOE_RUST_WRITE_ONCE': 'off'}),
            'ordered_on': worker(src, {'FLOE_RUST_WRITE_ONCE': 'off', 'FLOE_RUST_DENSITY_STACK': 'top'}),
        }
        try:
            both = (LOW, MID, TOP)
            off, off_res = frame(workers['off'], 1, both)
            on, on_res = frame(workers['on'], 1, both)
            alone, _ = frame(workers['off'], 2, (ALONE,))
            # the squares as a cut-free frame draws them, one layer at a time
            low_free, _ = frame(workers['off'], 3, (LOW,), cut_px=0.0)
            top_free, _ = frame(workers['off'], 4, (TOP,), cut_px=0.0)
            # the bottom right quadrant: nothing above layer 1's squares
            br = (range(202, W), range(102, H))
            assert not lit(off, *br), 'without the stack the cut drops the squares'
            want = lit(low_free, *br)
            assert want and lit(on, *br) == want, 'bottom right: %d px lit with the stack, %d in the cut-free frame' % (len(lit(on, *br)), len(want))
            for (c, r) in want:
                assert px(on, c, r) == px(low_free, c, r), 'bottom right (%d, %d): another colour' % (c, r)
            # inside the left rectangle: the rectangle alone
            inside = (range(2, 198), range(2, 198))
            assert lit(on, *inside) == lit(alone, *inside), 'inside the left rectangle %d px lit, alone %d' % (
                len(lit(on, *inside)), len(lit(alone, *inside)))
            assert len(lit(low_free, *inside)) > 0 and len(lit(top_free, range(20, 180), range(20, 180)) - lit(alone, range(20, 180), range(20, 180))) > 0, \
                'squares exist under the rectangle and inside it'
            # the top right quadrant: the top layer's squares over layer 2's rectangle
            tr = (range(202, W), range(2, 98))
            squares = lit(top_free, *tr)
            assert squares, "the top layer has squares over layer 2's rectangle"
            assert lit(on, *tr) == lit(off, *tr) | squares, 'top right: %d px lit, %d as rectangle + squares' % (
                len(lit(on, *tr)), len(lit(off, *tr) | squares))
            for (c, r) in squares:
                assert px(on, c, r) == px(top_free, c, r), "top right (%d, %d): not the top layer's colour" % (c, r)
            for (c, r) in lit(off, *tr) - squares:
                assert px(on, c, r) == px(off, c, r), 'top right (%d, %d): the rectangle changed' % (c, r)
            stack, pages = on_res.get('density_stack'), on_res.get('density_pages')
            assert stack and stack['lit'] > 0 and stack['top'] > 0 and stack['lower'] > 0 and stack['covered'] > 0, stack
            assert pages and pages['planned'] > 0 and pages['decoded'] > 0 and pages['over_budget'] == 0, pages
            assert off_res.get('density_stack') is None and off_res.get('density_pages') is None, off_res.get('density_stack')
            print('density stack: bottom right %d square px as cut-free, left rectangle inside = alone (%d px), top right %d top-layer px over '
                  'layer 2; counts %s; pass 2 pages %s' % (len(want), len(lit(alone, *inside)), len(squares),
                                                          ' '.join('%s=%d' % kv for kv in stack.items()),
                                                          ' '.join('%s=%d' % kv for kv in pages.items())))
            # the visible layers only: layer 1/0 alone is its own top plane
            low_on, low_res = frame(workers['on'], 6, (LOW,))
            assert low_on == low_free, 'layer 1/0 alone with the stack differs from its cut-free frame in %d px' % sum(
                1 for i in range(0, len(low_on), 4) if low_on[i:i + 4] != low_free[i:i + 4])
            assert low_res['density_stack']['top'] > 0 and low_res['density_stack']['lower'] == 0, low_res['density_stack']
            mid_on, _ = frame(workers['on'], 7, (LOW, MID))
            mid_off, _ = frame(workers['off'], 7, (LOW, MID))
            assert lit(mid_on, *tr) == lit(mid_off, *tr), 'with 4/0 off its squares still show in the top right (%d vs %d px)' % (
                len(lit(mid_on, *tr)), len(lit(mid_off, *tr)))
            assert lit(mid_on, *br) == want, 'with 4/0 off the bottom right changed'
            print('density stack: layer 1/0 alone = its cut-free frame; 1/0 + 2/0 shows none of 4/0')
            # the viewer's margin (bg, twice the extent per axis) draws the view's
            # pixels as the viewport frame did: pass 2's fit is remembered per scale
            # and side (2026-09-27: planned to half of what the generation had left,
            # the margin's pass 2 decoded 23 pages where the viewport's decoded 2,208)
            margin, mres = frame_bg(workers['on'], 8, both)
            centre = b''.join(margin[((H // 2 + r) * 2 * W + W // 2) * 4:((H // 2 + r) * 2 * W + W // 2 + W) * 4] for r in range(H))
            assert centre == on, 'the margin draws the view otherwise than the viewport frame in %d px' % sum(
                1 for i in range(0, len(on), 4) if centre[i:i + 4] != on[i:i + 4])
            assert mres.get('density_pages') and mres['density_pages']['over_budget'] == 0, mres.get('density_pages')
            print('density stack: the margin frame draws the view as the viewport frame did (pass 2 pages %s)' % (
                ' '.join('%s=%d' % kv for kv in mres['density_pages'].items())))
            for name, base in (('klayout', 'klayout_on'), ('ordered', 'ordered_on')):
                a, _ = frame(workers[name], 5, both)
                b, b_res = frame(workers[base], 5, both)
                assert a == b and b_res.get('density_stack') is None and b_res.get('density_pages') is None, '%s: the stack must be off' % base
                print('density stack: %s draws as without the variable, no counts' % base)
        finally:
            for w in workers.values():
                w.stop()
        dots_checks(temp)
    print('density stack gate: OK')


if __name__ == '__main__':
    main()
