#!/usr/bin/env python3
"""Sub-cut box gate (rust/vfs/src/hier.rs SUB_CUT_BOX_PX, 2026-09-19).

Field: the `thin keep` picture is Calibre-like except that what the size cut
drops VANISHES - one via layer of the synthetic MAIN01 is an empty screen from
the fit view to x4, where Calibre keeps every shape at a minimum size. Under
`thin keep` with at most four layers visible, what the size cut drops now
stays as a box drawn from index metadata (no page decoded). This gate renders
the chip-geometry synthetic MAIN01 (tools/gen_main01_like.py):

  * one via layer, keep, the whole chip: the kill switch
    FLOE_RUST_SUB_CUT_BOX=off gives an empty frame, the default a frame with
    boxes on it (sub_cut_boxes > 0) and no page decoded for them;
  * what is NOT the feature's business is byte-identical to the kill switch:
    the same view under `thin cull`, the all-layer keep view (more layers than
    the cap), and a near keep view where nothing is under the cut.

    .venv/bin/python tools/validate_sub_cut_box.py
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
VIA = (4, 2)        # layer index 14: role via (index mod 6 == 2)


def worker(src, on):
    if on:
        os.environ.pop('FLOE_RUST_SUB_CUT_BOX', None)
    else:
        os.environ['FLOE_RUST_SUB_CUT_BOX'] = 'off'
    cache = Cache(str(src))
    cache.load()
    w = RustRenderWorker(cache)
    w.start()
    os.environ.pop('FLOE_RUST_SUB_CUT_BOX', None)
    return w


def frame(w, gen, bbox, keys, thin):
    w.submit({'kind': 'render', 'gen': gen, 'scope': 'headless', 'bbox': bbox, 'view': None,
              'w': W, 'h': H, 'depth': None, 'cut_px': 1, 'lod': False, 'frames': False,
              'labels': False, 'abstract': False, 'visible': keys, 'frame_format': 'raw',
              'thin': thin, 'frame_cache': False})
    deadline = time.monotonic() + 300
    while time.monotonic() < deadline:
        res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
        assert res.get('kind') != 'error', res
        if res.get('kind') == 'frame' and res.get('gen') == gen and not res.get('refining'):
            return bytes(res.pop('rgba')), res
    raise AssertionError('sub-cut box frame timeout')


def lit(pixels):
    background = pixels[:4]
    return sum(1 for i in range(0, len(pixels), 4) if pixels[i:i + 4] != background)


def main():
    os.environ['FLOE_INDEX_BIN'] = str(ROOT / 'rust/target/release/floe-index')
    os.environ['FLOE_RENDERD_BIN'] = str(ROOT / 'rust/target/release/floe-renderd')
    os.environ['FLOE_RUST_RETAINED_MB'] = '0'
    with tempfile.TemporaryDirectory(prefix='floe-box-') as temp:
        src = Path(temp) / 'chip.oas'
        for argv in ([sys.executable, '-B', str(ROOT / 'tools/gen_main01_like.py'), str(src),
                      '--scale', '0.003', '--jobs', '2', '--geometry', 'chip'],
                     [sys.executable, '-B', '-m', 'floe2', 'index', str(src), '--jobs', '2']):
            done = subprocess.run(argv, cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
            assert done.returncode == 0, done.stdout + done.stderr
        off, on = worker(src, False), worker(src, True)
        try:
            x0, y0, x1, y1 = map(float, on.cache.meta['bbox'])
            wide = (x0, y0, x1, y1)
            cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
            span = (x1 - x0) / 4000
            near = (cx - span, cy - span * H / W, cx + span, cy + span * H / W)
            every = [(int(l['layer']), int(l['datatype'])) for l in on.cache.meta['layers']]
            empty, res_off = frame(off, 1, wide, [VIA], 'keep')
            boxed, res_on = frame(on, 1, wide, [VIA], 'keep')
            assert lit(empty) == 0, 'the kill switch frame should be empty: %d px lit' % lit(empty)
            culls = res_on['plan_culls']
            assert lit(boxed) > 1000 and culls['sub_cut_boxes'] > 0, (lit(boxed), culls)
            assert res_off['plan_culls']['sub_cut_boxes'] == 0
            assert res_on['new'] == res_off['new'], 'boxes must not decode pages: %s vs %s' % (res_on['new'], res_off['new'])
            same = 0
            for gen, (bbox, keys, thin) in enumerate(((wide, [VIA], 'cull'), (wide, every, 'keep'), (near, [VIA], 'keep')), 2):
                a, ra = frame(off, gen, bbox, keys, thin)
                b, rb = frame(on, gen, bbox, keys, thin)
                assert a == b and rb['plan_culls']['sub_cut_boxes'] == 0, (
                    'frame %d changed: %s' % (gen, rb['plan_culls']))
                same += 1
            print('sub-cut box: one via layer of the chip, keep: %d px lit by %d boxes (kill switch: empty), '
                  '%d other frames unchanged' % (lit(boxed), culls['sub_cut_boxes'], same))
        finally:
            off.stop()
            on.stop()
    print('SUB-CUT BOX: ALL OK')


if __name__ == '__main__':
    main()
