#!/usr/bin/env python3
"""Write-once tile gate (rust/render-core/src/raster.rs, F2R-28, 2026-09-18).

Every geometry paint is an opaque overwrite in its plane's one colour, so a
frame is "the last plane to write a pixel wins". The raster paints the planes
in reverse order, writes each pixel once and skips what can only reach written
pixels: a full tile ends its pass sequence, a pass sees only the open pixels'
bounding box, a covered item is never enumerated. This gate renders a synthetic
MAIN01-class chip (tools/gen_main01_like.py; every layer covers the same area)
and the battery's valmini with the kill switch FLOE_RUST_WRITE_ONCE=off and
without it:

  * every frame is byte-identical - wide and near views, keep and cull, with
    hierarchy frames and labels on, at depth 1 and full depth;
  * the dense view fills tiles (once_full_tiles > 0) and paints fewer members;
    the kill switch reports no write-once work at all.

    .venv/bin/python tools/validate_write_once.py
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


def worker(src, on, tile_px):
    os.environ['FLOE_RUST_TILE_PX'] = str(tile_px)
    if on:
        os.environ.pop('FLOE_RUST_WRITE_ONCE', None)
    else:
        os.environ['FLOE_RUST_WRITE_ONCE'] = 'off'
    cache = Cache(str(src))
    cache.load()
    w = RustRenderWorker(cache)
    w.start()
    os.environ.pop('FLOE_RUST_WRITE_ONCE', None)
    os.environ.pop('FLOE_RUST_TILE_PX', None)
    return w


def frame(w, gen, bbox, thin, depth, frames):
    keys = [(int(l['layer']), int(l['datatype'])) for l in w.cache.meta['layers']]
    w.submit({'kind': 'render', 'gen': gen, 'scope': 'headless', 'bbox': bbox, 'view': None,
              'w': W, 'h': H, 'depth': depth, 'cut_px': 1, 'lod': False, 'frames': frames,
              'labels': frames, 'abstract': False, 'visible': keys, 'frame_format': 'raw',
              'thin': thin, 'frame_cache': False})
    deadline = time.monotonic() + 300
    while time.monotonic() < deadline:
        res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
        assert res.get('kind') != 'error', res
        if res.get('kind') == 'frame' and res.get('gen') == gen and not res.get('refining'):
            return bytes(res.pop('rgba')), res
    raise AssertionError('write-once frame timeout')


def views(cache):
    x0, y0, x1, y1 = map(float, cache.meta['bbox'])
    cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
    for zoom in (1, 6, 40):
        s = max((x1 - x0) / W, (y1 - y0) / H) / zoom
        yield zoom, (cx - s * W / 2, cy - s * H / 2, cx + s * W / 2, cy + s * H / 2)


def compare(src, name, tile_px):
    off, on = worker(src, False, tile_px), worker(src, True, tile_px)
    gen, full, fewer = 0, 0, False
    try:
        for zoom, bbox in views(on.cache):
            for thin, depth, frames in (('keep', None, False), ('cull', None, True), ('keep', 1, True)):
                gen += 1
                a, ra = frame(off, gen, bbox, thin, depth, frames)
                b, rb = frame(on, gen, bbox, thin, depth, frames)
                case = '%s x%g %s depth=%s frames=%s' % (name, zoom, thin, depth, frames)
                assert a == b, 'write-once changed the frame: ' + case
                assert (ra['once_full_tiles'], ra['once_passes_skipped'], ra['once_items_skipped']) == (0, 0, 0), (
                    'the kill switch still did write-once work: ' + case)
                assert rb['member_paints'] <= ra['member_paints'], case
                full += rb['once_full_tiles']
                fewer = fewer or rb['member_paints'] < ra['member_paints']
    finally:
        off.stop()
        on.stop()
    return gen, full, fewer


def main():
    os.environ['FLOE_INDEX_BIN'] = str(ROOT / 'rust/target/release/floe-index')
    os.environ['FLOE_RENDERD_BIN'] = str(ROOT / 'rust/target/release/floe-renderd')
    os.environ['FLOE_RUST_RETAINED_MB'] = '0'
    with tempfile.TemporaryDirectory(prefix='floe-once-') as temp:
        chip = Path(temp) / 'chip.oas'
        mini = Path(temp) / 'valmini.oas'
        mini.write_bytes((ROOT / 'data/m1/valmini.oas').read_bytes())
        for argv in ([sys.executable, '-B', str(ROOT / 'tools/gen_main01_like.py'), str(chip),
                      '--scale', '0.003', '--jobs', '2', '--geometry', 'legacy'],
                     [sys.executable, '-B', '-m', 'floe2', 'index', str(chip), '--jobs', '2'],
                     [sys.executable, '-B', '-m', 'floe2', 'index', str(mini), '--jobs', '2']):
            done = subprocess.run(argv, cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
            assert done.returncode == 0, done.stdout + done.stderr
        # small tiles: the 0.003-scale chip covers no 384 px tile completely
        frames, full, fewer = compare(chip, 'chip', 48)
        assert full > 0 and fewer, 'the dense chip must fill tiles and save paints (full tiles %d)' % full
        more, _, _ = compare(mini, 'valmini', 384)
        print('write once: %d frames byte-identical to the ordered overwrite, %d full tiles on the chip'
              % (frames + more, full))
    print('WRITE ONCE: ALL OK')


if __name__ == '__main__':
    main()
