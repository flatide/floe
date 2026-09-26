#!/usr/bin/env python3
"""Density stack gate (floe_render_core::GeometryRasterRequest::density_stack,
CUT_DENSITY_DESIGN §10.10; user direction 2026-09-27: the shapes under the
cut are the density - hairlines have their own drawing - drawn in a second
pass into the space the originals left, the top layer first).

With the diagnostic FLOE_RUST_DENSITY_STACK=top the frame is drawn twice:
pass 1 paints the plan as always (max mode: every shape whose larger side
reaches the cut) and records what its originals cover - speckle holes
included; pass 2 plans the same view at a finer cut (0.5 px, the larger
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
  * the frame reports the stack's counts (density_stack: lit, top, lower,
    covered, claimed) and pass 2's pages (density_pages: candidates, taken,
    decoded, over_budget - some decoded); none without the variable, under
    FLOE_RUST_AREA_TRUE=off or FLOE_RUST_WRITE_ONCE=off, which draw as
    without the variable.

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
SQUARE = 0.15               # um: 1.5 px, under the 3 px cut, over pass 2's 0.5 px


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


def px(pixels, c, r):
    return pixels[(r * W + c) * 4:(r * W + c) * 4 + 4]


def lit(pixels, cols, rows):
    return {(c, r) for r in rows for c in cols if px(pixels, c, r) != BLACK}


def main():
    os.environ['FLOE_INDEX_BIN'] = str(ROOT / 'rust/target/release/floe-index')
    os.environ['FLOE_RENDERD_BIN'] = str(ROOT / 'rust/target/release/floe-renderd')
    os.environ['FLOE_RUST_RETAINED_MB'] = '0'
    for name in ('FLOE_RUST_DENSITY_STACK', 'FLOE_RUST_AREA_TRUE', 'FLOE_RUST_WRITE_ONCE'):
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
            assert pages and pages['decoded'] > 0 and pages['over_budget'] == 0, pages
            assert off_res.get('density_stack') is None and off_res.get('density_pages') is None, off_res.get('density_stack')
            print('density stack: bottom right %d square px as cut-free, left rectangle inside = alone (%d px), top right %d top-layer px over '
                  'layer 2; counts %s; pass 2 pages %s' % (len(want), len(lit(alone, *inside)), len(squares),
                                                          ' '.join('%s=%d' % kv for kv in stack.items()),
                                                          ' '.join('%s=%d' % kv for kv in pages.items())))
            for name, base in (('klayout', 'klayout_on'), ('ordered', 'ordered_on')):
                a, _ = frame(workers[name], 5, both)
                b, b_res = frame(workers[base], 5, both)
                assert a == b and b_res.get('density_stack') is None and b_res.get('density_pages') is None, '%s: the stack must be off' % base
                print('density stack: %s draws as without the variable, no counts' % base)
        finally:
            for w in workers.values():
                w.stop()
    print('density stack gate: OK')


if __name__ == '__main__':
    main()
