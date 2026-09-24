#!/usr/bin/env python3
"""Per-shape cut gate (floe_vfs::ViewReq::shape_cut, 2026-09-20).

Field (synthetic MAIN01, thin keep, detail medium): an array of 0.4 um squares
stayed a draw target down to a 9,000 um view - the size cut judged its PAGE by
the page's largest shape (12 x 14 um), and thin shapes longer than the cut were
never cut at all. User decision: under `thin keep` the cut judges every shape
by its SMALLER side - a page is cut when none of its shapes reaches the cut on
both sides, and in the pages that stay the raster drops the shapes under it.
One small layout written with klayout.db: a 20 um square, an array of 0.4 um
squares and a 0.2 um wire on one layer (one page), wires alone on another.

  * a view where everything is at or above the cut: byte-identical to the
    kill switch FLOE_RUST_SHAPE_CUT=off, and the frame reports the cut;
  * a wider view (squares 2.6 px, wires 1.3 px, cut 3 px): only the large
    square is drawn - every lit pixel lies in its box - while the kill switch
    still lights the array and the wire; the layer of wires alone is an empty
    frame and its page is cut by the planner;
  * not the feature's business, byte-identical to the kill switch: the same
    views under `thin cull`, and with no cut at all (detail off);
  * the hairline-keeping diagnostic FLOE_RUST_SHAPE_CUT=max (CUT_DENSITY_DESIGN
    §10.6, 2026-09-24): at the wider view the large square is drawn exactly as
    under the shape cut, the array of 0.4 um squares (both sides under the
    cut) is not drawn, the 0.2 um wire (longer than the cut) is, and the
    layer of wires alone keeps its page.

    .venv/bin/python tools/validate_shape_cut.py
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

W, H = 1280, 720
MIXED, WIRES = (7, 0), (8, 0)
SQUARE = (0.0, 0.0, 20.0, 20.0)    # um


def layout(path):
    import klayout.db as kdb
    ly = kdb.Layout()
    ly.dbu = 0.001
    top = ly.create_cell('TOP')
    mixed, wires = ly.layer(*MIXED), ly.layer(*WIRES)
    top.shapes(mixed).insert(kdb.DBox(*SQUARE))
    for i in range(26):
        for j in range(20):
            x, y = 40 + 1.2 * i, 1.2 * j
            top.shapes(mixed).insert(kdb.DBox(x, y, x + 0.4, y + 0.4))
    top.shapes(mixed).insert(kdb.DBox(0, 40, 60, 40.2))
    for k in range(12):
        top.shapes(wires).insert(kdb.DBox(0, 2.0 * k, 70, 2.0 * k + 0.2))
    ly.write(str(path))


def worker(src, on):
    if on is True:
        os.environ.pop('FLOE_RUST_SHAPE_CUT', None)
    else:
        os.environ['FLOE_RUST_SHAPE_CUT'] = 'off' if on is False else on
    cache = Cache(str(src))
    cache.load()
    w = RustRenderWorker(cache)
    w.start()
    os.environ.pop('FLOE_RUST_SHAPE_CUT', None)
    return w


def frame(w, gen, view_um, keys, thin, cut_px=3.0):
    dbu = float(w.cache.meta['dbu'])
    w.submit({'kind': 'render', 'gen': gen, 'scope': 'headless', 'bbox': tuple(v / dbu for v in view_um), 'view': None,
              'w': W, 'h': H, 'depth': None, 'cut_px': cut_px, 'lod': False, 'frames': False,
              'labels': False, 'abstract': False, 'visible': keys, 'frame_format': 'raw',
              'thin': thin, 'frame_cache': False})
    deadline = time.monotonic() + 300
    while time.monotonic() < deadline:
        res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
        assert res.get('kind') != 'error', res
        if res.get('kind') == 'frame' and res.get('gen') == gen and not res.get('refining'):
            return bytes(res.pop('rgba')), res
    raise AssertionError('shape cut frame timeout')


def lit_pixels(pixels):
    background = pixels[:4]
    return [(i // 4 % W, i // 4 // W) for i in range(0, len(pixels), 4) if pixels[i:i + 4] != background]


def view(cx, cy, width_um):
    height = width_um * H / W
    return (cx - width_um / 2, cy - height / 2, cx + width_um / 2, cy + height / 2)


def main():
    os.environ['FLOE_INDEX_BIN'] = str(ROOT / 'rust/target/release/floe-index')
    os.environ['FLOE_RENDERD_BIN'] = str(ROOT / 'rust/target/release/floe-renderd')
    os.environ['FLOE_RUST_RETAINED_MB'] = '0'
    with tempfile.TemporaryDirectory(prefix='floe-shape-cut-') as temp:
        src = Path(temp) / 'shapes.oas'
        layout(src)
        done = subprocess.run([sys.executable, '-B', '-m', 'floe2', 'index', str(src)],
                              cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
        assert done.returncode == 0, done.stdout + done.stderr
        off, on = worker(src, False), worker(src, True)
        try:
            near, wide = view(35, 21, 80), view(35, 21, 200)   # 0.0625 and 0.15625 um a pixel
            gen = 0
            # everything at or above the cut: nothing changes, the cut is reported
            for keys in ([MIXED], [WIRES], [MIXED, WIRES]):
                gen += 1
                a, ra = frame(off, gen, near, keys, 'keep')
                b, rb = frame(on, gen, near, keys, 'keep')
                assert a == b and lit_pixels(a), 'near keep frame changed for %s' % (keys,)
                assert rb['plan_culls']['shape_cut'] > 0 and ra['plan_culls']['shape_cut'] == 0, (ra['plan_culls'], rb['plan_culls'])
            # the wider view: only the large square is left of the mixed page
            gen += 1
            before, _ = frame(off, gen, wide, [MIXED], 'keep')
            after, res = frame(on, gen, wide, [MIXED], 'keep')
            spp = 200.0 / W
            # the square's device box, two pixels of slack for the edge snapping
            c0, c1 = (SQUARE[0] - wide[0]) / spp - 2, (SQUARE[2] - wide[0]) / spp + 2
            r0, r1 = (wide[3] - SQUARE[3]) / spp - 2, (wide[3] - SQUARE[1]) / spp + 2
            inside = lambda p: c0 <= p[0] <= c1 and r0 <= p[1] <= r1
            kept, was = lit_pixels(after), lit_pixels(before)
            assert kept and all(inside(p) for p in kept), 'the shape cut left %d px outside the large square' % sum(not inside(p) for p in kept)
            outside = sum(not inside(p) for p in was)
            assert outside > 500, 'the kill switch frame should still light the array and the wire: %d px' % outside
            assert [p for p in was if inside(p)] == kept, 'the large square itself changed'
            # wires alone: the planner cuts the page, the frame is empty
            gen += 1
            lines, res_off = frame(off, gen, wide, [WIRES], 'keep')
            none, res_on = frame(on, gen, wide, [WIRES], 'keep')
            assert len(lit_pixels(lines)) > 500 and not lit_pixels(none), (len(lit_pixels(lines)), len(lit_pixels(none)))
            assert res_on['plan_culls']['pages_size'] >= 1 and res_off['plan_culls']['pages_size'] == 0, (res_off['plan_culls'], res_on['plan_culls'])
            # thin cull and a frame without a cut are not the feature's business
            same = 0
            for bbox, keys, thin, cut_px in ((wide, [MIXED], 'cull', 3.0), (wide, [WIRES], 'cull', 3.0), (wide, [MIXED, WIRES], 'keep', 0.0)):
                gen += 1
                a, _ = frame(off, gen, bbox, keys, thin, cut_px)
                b, rb = frame(on, gen, bbox, keys, thin, cut_px)
                assert a == b and rb['plan_culls']['shape_cut'] == 0, 'frame %d changed: %s' % (gen, rb['plan_culls'])
                same += 1
            print('shape cut: near views identical (3), wide keep view %d -> %d px (all in the large square), wires alone %d -> 0 px, %d unrelated frames identical'
                  % (len(was), len(kept), len(lit_pixels(lines)), same))
            # the hairline-keeping mode: the square as under the shape cut, the
            # array gone, the wire and the wires-alone page kept
            hair = worker(src, 'max')
            try:
                gen += 1
                mixed, rm = frame(hair, gen, wide, [MIXED], 'keep')
                got = lit_pixels(mixed)
                in_array = lambda p: (40 - wide[0]) / spp - 2 <= p[0] <= (72 - wide[0]) / spp + 2 and (wide[3] - 24.5) / spp - 2 <= p[1] <= (wide[3] - 0) / spp + 2
                in_wire = lambda p: (wide[3] - 40.4) / spp - 2 <= p[1] <= (wide[3] - 39.8) / spp + 2 and p[0] <= (60 - wide[0]) / spp + 2
                assert [p for p in got if inside(p)] == kept, 'max: the large square differs from the shape cut'
                assert not any(in_array(p) for p in got if not inside(p)), 'max: the array of small squares was drawn'
                wire_px = sum(in_wire(p) for p in got if not inside(p))
                assert wire_px > 100, 'max: the wire was not drawn (%d px)' % wire_px
                assert all(inside(p) or in_wire(p) for p in got), 'max: pixels outside the square and the wire'
                assert rm['plan_culls']['shape_cut'] > 0, rm['plan_culls']
                gen += 1
                wires_kept, rw = frame(hair, gen, wide, [WIRES], 'keep')
                assert len(lit_pixels(wires_kept)) > 500 and rw['plan_culls']['pages_size'] == 0, (len(lit_pixels(wires_kept)), rw['plan_culls'])
                print('shape cut max: square identical, array 0 px, wire %d px, wires alone %d px (page kept)' % (wire_px, len(lit_pixels(wires_kept))))
            finally:
                hair.stop()
        finally:
            off.stop()
            on.stop()
    print('validate_shape_cut: OK')


if __name__ == '__main__':
    main()
