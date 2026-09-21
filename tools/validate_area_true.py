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
    number of pixels gives the same pixels where the two overlap.

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


def frame(w, gen, view_um, cut_px=0.0):
    dbu = float(w.cache.meta['dbu'])
    w.submit({'kind': 'render', 'gen': gen, 'scope': 'headless', 'bbox': tuple(v / dbu for v in view_um), 'view': None,
              'w': W, 'h': H, 'depth': None, 'cut_px': cut_px, 'lod': False, 'frames': False,
              'labels': False, 'abstract': False, 'visible': [LAYER], 'frame_format': 'raw',
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
        finally:
            on.stop()
            off.stop()
    print('AREA TRUE: OK')


if __name__ == '__main__':
    main()
