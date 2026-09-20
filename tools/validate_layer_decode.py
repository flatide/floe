#!/usr/bin/env python3
"""Layer-decode probe gate (docs/LAYER_DECODE_PROBE_PLAN.ko.md step 1).

F2R-28 measured that with every layer visible 92-99 % of the pages a frame
decodes never light a pixel. Before anything is skipped, the painting has to
be able to run LAYER BY LAYER over tiles that stay alive, and produce the very
same frame. `render_probe mode=baseline` is the normal path; `mode=ordered`
paints the same plan and the same selection through `LayerRasterSession`, one
pass at a time, top layer first. `mode=occlusion` (skipping the decode) is not
built yet and must be refused, never served by another mode.

This gate renders one small layout written with klayout.db - overlapping
shapes on five layers (solid over solid, solid over a sparse array, speckle
and clear fills with shapes under their holes, a hairline grid) - and checks:

  * baseline and ordered frames are byte-identical, over fills, zooms, thin
    keep/cull, depth 0/full, hierarchy frames on/off and labels on/off;
  * the same holds at tile sizes 64/128/384 and 1/4 raster workers, at every
    block size (how many layers a worker paints into a tile before the
    workers meet), and the two modes agree on what the write-once mask let
    them skip;
  * the probe answers `probe_frame`, never `frame`, and publishes no scene:
    a snap right after a probe of an empty far view still answers from the
    render before it;
  * `mode=occlusion` is an error, and the worker takes renders after it.

    .venv/bin/python tools/validate_layer_decode.py
"""
import hashlib
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

W, H = 640, 360
SOLID = '\n'.join(['*' * 16] * 16)
CLEAR = '\n'.join(['.' * 16] * 16)
SPECKLE = '\n'.join([('*.' * 8) if row % 2 == 0 else ('.*' * 8) for row in range(16)])
FILLS = (SOLID, SPECKLE, CLEAR, SOLID, SPECKLE)
LAYERS = [(10 + i, 0) for i in range(5)]


def layout(path):
    """Five layers that cover each other in every way the write-once mask
    cares about: an opaque block, a sparse array under it, hairlines that
    cross both, and a cell placed twice so one instance is covered and the
    other is not."""
    import klayout.db as kdb
    ly = kdb.Layout()
    ly.dbu = 0.001
    top = ly.create_cell('TOP')
    leaf = ly.create_cell('LEAF')
    lay = [ly.layer(*key) for key in LAYERS]
    for i in range(40):
        for j in range(30):
            leaf.shapes(lay[1]).insert(kdb.DBox(i * 1.5, j * 1.5, i * 1.5 + 0.6, j * 1.5 + 0.6))
    for k in range(20):
        leaf.shapes(lay[3]).insert(kdb.DBox(0, k * 3.0, 60, k * 3.0 + 0.08))
    top.insert(kdb.DCellInstArray(leaf.cell_index(), kdb.DTrans(kdb.DVector(0, 0))))
    top.insert(kdb.DCellInstArray(leaf.cell_index(), kdb.DTrans(kdb.DVector(80, 0))))
    top.shapes(lay[0]).insert(kdb.DBox(5, 5, 45, 40))
    top.shapes(lay[2]).insert(kdb.DBox(20, 10, 130, 35))
    top.shapes(lay[4]).insert(kdb.DBox(0, 44, 140, 44.3))
    for k in range(12):
        top.shapes(lay[4]).insert(kdb.DBox(k * 12.0, 0, k * 12.0 + 0.1, 50))
    ly.write(str(path))


def worker(src, tile_px, jobs):
    os.environ['FLOE_RUST_TILE_PX'] = str(tile_px)
    os.environ['FLOE_RUST_RASTER_JOBS'] = str(jobs)
    cache = Cache(str(src))
    cache.load()
    w = RustRenderWorker(cache)
    w.start()
    w.submit({'kind': 'recolor', 'colors': [(key, '#%02x60ff' % (0x30 + 40 * i))
                                            for i, key in enumerate(LAYERS)]})
    w.submit({'kind': 'repattern', 'fills': list(zip(LAYERS, FILLS)), 'widths': []})
    time.sleep(0.4)
    os.environ.pop('FLOE_RUST_TILE_PX', None)
    os.environ.pop('FLOE_RUST_RASTER_JOBS', None)
    return w


def frame(w, gen, bbox, mode=None, thin='keep', depth=None, frames=False, labels=False,
          keys=None, block=1):
    job = {'kind': 'render' if mode is None else 'render_probe', 'gen': gen, 'scope': 'headless',
           'bbox': bbox, 'view': None, 'w': W, 'h': H, 'depth': depth, 'cut_px': 3.0, 'lod': False,
           'frames': frames, 'labels': labels, 'abstract': False,
           'visible': LAYERS if keys is None else keys,
           'frame_format': 'raw', 'thin': thin, 'frame_cache': False}
    if mode is not None:
        job['mode'] = mode
        job['block'] = block
    w.submit(job)
    want = 'probe_frame' if mode is not None else 'frame'
    deadline = time.monotonic() + 300
    while time.monotonic() < deadline:
        res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
        assert res.get('kind') != 'error', res
        if res.get('kind') == want and res.get('gen') == gen and not res.get('refining'):
            return bytes(res.pop('rgba')), res
        assert res.get('kind') != ('frame' if mode is not None else 'probe_frame'), (
            'a %s answered %s' % (want, res.get('kind')))
    raise AssertionError('%s timeout' % want)


def views(cache):
    x0, y0, x1, y1 = (v * float(cache.meta['dbu']) for v in cache.meta['bbox'])
    cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
    for zoom in (1, 3, 12, 60):
        s = max((x1 - x0) / W, (y1 - y0) / H) / zoom
        box = (cx - s * W / 2, cy - s * H / 2, cx + s * W / 2, cy + s * H / 2)
        yield zoom, tuple(v / float(cache.meta['dbu']) for v in box)


def main():
    os.environ['FLOE_INDEX_BIN'] = str(ROOT / 'rust/target/release/floe-index')
    os.environ['FLOE_RENDERD_BIN'] = str(ROOT / 'rust/target/release/floe-renderd')
    os.environ['FLOE_RUST_RETAINED_MB'] = '0'
    with tempfile.TemporaryDirectory(prefix='floe-layer-decode-') as temp:
        src = Path(temp) / 'cover.oas'
        layout(src)
        done = subprocess.run([sys.executable, '-B', '-m', 'floe2', 'index', str(src)],
                              cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
        assert done.returncode == 0, done.stdout + done.stderr
        w = worker(src, 128, 4)
        gen = 0
        checked = lit = 0
        try:
            for zoom, bbox in views(w.cache):
                for thin, depth, frames, labels in (('keep', None, False, False),
                                                    ('cull', None, False, False),
                                                    ('keep', 0, False, False),
                                                    ('keep', None, True, True)):
                    gen += 1
                    base, rb = frame(w, gen, bbox, 'baseline', thin, depth, frames, labels)
                    gen += 1
                    ordered, ro = frame(w, gen, bbox, 'ordered', thin, depth, frames, labels)
                    case = 'zoom x%d thin %s depth %s frames %s' % (zoom, thin, depth, frames)
                    assert base == ordered, 'ordered differs from baseline: ' + case
                    # the normal path and the probe's baseline are the same render
                    gen += 1
                    plain, _ = frame(w, gen, bbox, None, thin, depth, frames, labels)
                    assert plain == base, 'the probe baseline differs from a render: ' + case
                    pb, po = rb['probe'], ro['probe']
                    assert pb['mode'] == 'baseline' and po['mode'] == 'ordered', (pb, po)
                    assert (pb['selected_pages'], pb['decoded_pages']) == (
                        po['selected_pages'], po['decoded_pages']), (case, pb, po)
                    assert po['skipped_pages'] == 0, 'step 1 skips no page: %s' % (po,)
                    assert po['passes'] >= po['layer_passes'] >= 1, (case, po)
                    assert (pb['once_tiles'], pb['once_passes'], pb['once_items']) == (
                        po['once_tiles'], po['once_passes'], po['once_items']), (
                        'the two modes skip different work: %s %s %s' % (case, pb, po))
                    checked += 1
                    lit += sum(1 for i in range(0, len(base), 4) if max(base[i:i + 3]) > 8)
            # the block size is scheduling only: a worker painting four or
            # every layer into a tile before the workers meet paints the same
            for block in (2, 4, 1000):
                gen += 1
                blocked, rb = frame(w, gen, bbox, 'ordered', block=block)
                assert blocked == ordered, 'block %d differs' % block
                assert rb['probe']['blocks'] == -(-rb['probe']['passes'] // block), rb['probe']
                checked += 1
            assert lit > 0, 'every frame of the gate was empty'
        finally:
            w.stop()
        # tile size and worker count do not change the pixels either
        shapes = {}
        for tile_px, jobs in ((64, 1), (384, 4), (128, 12)):
            w = worker(src, tile_px, jobs)
            try:
                _, bbox = next(iter(views(w.cache)))
                for mode in ('baseline', 'ordered'):
                    gen += 1
                    pixels, _ = frame(w, gen, bbox, mode)
                    shapes.setdefault(mode, []).append(hashlib.sha256(pixels).hexdigest())
            finally:
                w.stop()
        assert len(set(shapes['baseline'] + shapes['ordered'])) == 1, (
            'tile size or worker count changed the frame: %s' % shapes)
        # a probe publishes no scene: the snap still answers from the render
        w = worker(src, 128, 4)
        try:
            gen += 1
            _, bbox = next(iter(views(w.cache)))
            frame(w, gen, bbox, None)
            gen += 1
            dbu = float(w.cache.meta['dbu'])
            # the corner of the opaque block on layer 10/0
            snap = lambda seq: (w.submit({'kind': 'snap', 'seq': seq, 'x': int(5.4 / dbu),
                                          'y': int(5.4 / dbu), 'r': int(3 / dbu)})
                                or w.res.get(timeout=120))
            before = snap(1)
            assert before.get('kind') == 'snap' and before.get('found'), before
            # a probe of the closest view, whose scene holds neither the
            # corner nor its layer: a published probe scene would lose the snap
            gen += 1
            frame(w, gen, list(views(w.cache))[-1][1], 'ordered')
            after = snap(2)
            assert after.get('kind') == 'snap' and after.get('found'), (
                'the probe replaced the published scene: %s -> %s' % (before, after))
            assert (after['x'], after['y']) == (before['x'], before['y']), (before, after)
        finally:
            w.stop()
        # occlusion is refused and the worker keeps working
        w = worker(src, 128, 4)
        try:
            gen += 1
            _, bbox = next(iter(views(w.cache)))
            w.submit({'kind': 'render_probe', 'mode': 'occlusion', 'gen': gen, 'scope': 'headless',
                      'bbox': bbox, 'view': None, 'w': W, 'h': H, 'depth': None, 'cut_px': 3.0,
                      'lod': False, 'frames': False, 'labels': False, 'abstract': False,
                      'visible': LAYERS, 'frame_format': 'raw', 'thin': 'keep', 'frame_cache': False})
            res = w.res.get(timeout=120)
            assert res.get('kind') == 'error' and 'occlusion' in res.get('msg', ''), res
            # a render after the refusal still works: the worker is not wedged
            gen += 1
            after, _ = frame(w, gen, bbox, None)
            assert after
        finally:
            w.stop()
        print('layer decode: %d view/mode pairs byte-identical (%d lit px over them), '
              '3 tile/worker settings identical, published scene untouched, '
              'occlusion refused' % (checked, lit))
    print('validate_layer_decode: OK')


if __name__ == '__main__':
    main()
