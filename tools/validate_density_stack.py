#!/usr/bin/env python3
"""Density stack gate (floe_render_core::GeometryRasterRequest::density_stack,
CUT_DENSITY_DESIGN §10.10; user direction 2026-09-26: with several layers,
the top layer's density and, of the others, only the density in the empty
space).

Under area-true drawing a shape under a pixel on a side is kept with the
chance its area fills its pixels - a density representation. A lower
layer's dots landing in the holes of an upper layer's speckle filled the
pattern in (with one colour the pattern vanished). With the diagnostic
FLOE_RUST_DENSITY_STACK=top the originals paint as always; the top layer's
density shows over the layers below but not inside its own originals, and
every other layer's density only where no original covers the pixel (the
pattern's holes included) and no density above stands for it.

One layout written with klayout.db, 0.1 um a pixel: layer 1/0 is a field of
0.02 um (0.2 px) wires 0.1 um apart over the whole view; layer 2/0, the top
one, a rectangle over the left half with 0.05 um (0.5 px) dots inside it;
layer 3/0 the same rectangle alone (the reference, never drawn with the
others). Every layer takes the viewer's default speckle.

  * without the variable, and with it: the right half (no original there) is
    the same frame, byte for byte - the lower wires in the empty space show;
  * without it the rectangle's inside lights more than the rectangle alone
    (wires and dots in the speckle's holes); with it exactly the rectangle
    alone lights there - no lower wire and no dot of its own in its holes;
  * the frame reports the stack's counts (density_stack: lit, top, lower,
    covered, claimed), and none without the variable, under the kill switch
    of the area-true drawing (FLOE_RUST_AREA_TRUE=off: no density to stack)
    or with the write-once tiles off (FLOE_RUST_WRITE_ONCE=off) - both draw
    their frames as without the variable.

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
WIRES, TOP, ALONE = (1, 0), (2, 0), (3, 0)
BLACK = bytes((0, 0, 0, 255))
VIEW = (0.0, 0.0, W * PX_UM, H * PX_UM)


def layout(path):
    import klayout.db as kdb
    ly = kdb.Layout()
    ly.dbu = 0.001
    top = ly.create_cell('TOP')
    wires = ly.layer(*WIRES)
    for k in range(W):
        x = k * PX_UM + 0.037
        top.shapes(wires).insert(kdb.DBox(x, 0.5, x + 0.02, 19.5))
    for layer in (TOP, ALONE):
        top.shapes(ly.layer(*layer)).insert(kdb.DBox(0.0, 0.0, 20.0, 20.0))
    dots = ly.layer(*TOP)
    for j in range(55):
        for i in range(55):
            x, y = 2.0 + i * 0.29, 2.0 + j * 0.29
            top.shapes(dots).insert(kdb.DBox(x, y, x + 0.05, y + 0.05))
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


def frame(w, gen, visible):
    dbu = float(w.cache.meta['dbu'])
    w.submit({'kind': 'render', 'gen': gen, 'scope': 'headless', 'bbox': tuple(v / dbu for v in VIEW), 'view': None,
              'w': W, 'h': H, 'depth': None, 'cut_px': 0.0, 'lod': False, 'frames': False,
              'labels': False, 'abstract': False, 'visible': list(visible), 'frame_format': 'raw',
              'thin': 'keep', 'frame_cache': False})
    deadline = time.monotonic() + 300
    while time.monotonic() < deadline:
        res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
        assert res.get('kind') != 'error', res
        if res.get('kind') == 'frame' and res.get('gen') == gen and not res.get('refining'):
            return bytes(res.pop('rgba')), res
    raise AssertionError('density stack frame timeout')


def lit(pixels, cols, rows):
    return {(c, r) for r in rows for c in cols if pixels[(r * W + c) * 4:(r * W + c) * 4 + 4] != BLACK}


def half(pixels, right):
    """The right half's rows (from column 202) or the left half's, as bytes."""
    c0, c1 = (202, W) if right else (0, 198)
    return b''.join(pixels[(r * W + c0) * 4:(r * W + c1) * 4] for r in range(H))


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
            both = (WIRES, TOP)
            off, off_res = frame(workers['off'], 1, both)
            on, on_res = frame(workers['on'], 1, both)
            alone, _ = frame(workers['off'], 2, (ALONE,))
            # the empty space: the lower wires show as before
            assert half(on, True) == half(off, True), 'the right half changed under the stack'
            wires_right = len(lit(off, range(202, W), range(H)))
            assert wires_right > 0, 'no wire lit in the right half'
            # inside the rectangle: only the rectangle's own speckle
            inside = (range(2, 198), range(2, 198))
            want = lit(alone, *inside)
            was, now = lit(off, *inside), lit(on, *inside)
            assert was > want, 'without the stack the holes should hold dots (%d vs %d px)' % (len(was), len(want))
            assert now == want, 'with the stack the rectangle lights %d px, alone %d px (%d differ)' % (
                len(now), len(want), len(now ^ want))
            stack = on_res.get('density_stack')
            assert stack and stack['lit'] > 0 and stack['lower'] > 0 and stack['covered'] > 0 and stack['claimed'] > 0, stack
            assert off_res.get('density_stack') is None, off_res.get('density_stack')
            print('density stack: right half byte-identical (%d wire px), rectangle inside %d px with the stack = %d alone '
                  '(%d without); counts %s' % (wires_right, len(now), len(want), len(was),
                                               ' '.join('%s=%d' % kv for kv in stack.items())))
            for name, base in (('klayout', 'klayout_on'), ('ordered', 'ordered_on')):
                a, _ = frame(workers[name], 3, both)
                b, b_res = frame(workers[base], 3, both)
                assert a == b and b_res.get('density_stack') is None, '%s: the stack must be off' % base
                print('density stack: %s draws as without the variable, no counts' % base)
        finally:
            for w in workers.values():
                w.stop()
    print('density stack gate: OK')


if __name__ == '__main__':
    main()
