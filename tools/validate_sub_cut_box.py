#!/usr/bin/env python3
"""Sub-cut box gate (rust/vfs/src/hier.rs SUB_CUT_BOX_PX, 2026-09-19).

Field: the `thin keep` picture is Calibre-like except that what the size cut
drops VANISHES - one via layer of the synthetic MAIN01 is an empty screen from
the fit view to x4, where Calibre keeps every shape at a minimum size. Under
`thin keep` with at most sixteen layers visible, what the size cut drops now
stays as a box drawn from index metadata (no page decoded). This gate renders
the chip-geometry synthetic MAIN01 (tools/gen_main01_like.py):

  * one via layer, keep, the whole chip: without the boxes the frame is
    empty, with them (FLOE_RUST_SUB_CUT_BOX=on - off by default since
    0.12.182, the density representation below the cut replaces them) a frame
    with boxes on it (sub_cut_boxes > 0) and no page decoded for them;
  * what is NOT the feature's business is byte-identical to the kill switch:
    the same view under `thin cull`, the all-layer keep view (more layers than
    the cap), and a near keep view where nothing is under the cut;
  * sixteen layers at once: a box is a rect on every layer it stands for, so
    there are as many rects as with the layers planned one by one (fewer
    when the plan went a level coarser);
  * a box never claims what is not there, and never loses what is (review
    2026-09-19, four small layouts written with klayout.db): shapes below the
    depth limit get no box; 0.5 px members at a 3 px pitch light the pixels
    of the members, not the array's footprint; a layer held by one placement
    in 64 under a node box, or by one member of a point list, keeps its box;
  * a box never hides a layer the styles would show (review 2026-09-20: a
    150 px box of two layers, the top one with a CLEAR fill, lights what the
    lower layer lights alone), and the viewer's frames switch reaches the
    planner (frames off at depth 0: no depth-boundary outline is planned).

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
        os.environ['FLOE_RUST_SUB_CUT_BOX'] = 'on'
    else:
        os.environ.pop('FLOE_RUST_SUB_CUT_BOX', None)
    cache = Cache(str(src))
    cache.load()
    w = RustRenderWorker(cache)
    w.start()
    os.environ.pop('FLOE_RUST_SUB_CUT_BOX', None)
    return w


def frame(w, gen, bbox, keys, thin, depth=None, size=None, cut_px=1, frames=False):
    width, height = size or (W, H)
    w.submit({'kind': 'render', 'gen': gen, 'scope': 'headless', 'bbox': bbox, 'view': None,
              'w': width, 'h': height, 'depth': depth, 'cut_px': cut_px, 'lod': False, 'frames': frames,
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


def review_layouts(out):
    """1000 x 1000 um layouts viewed at 1 um per pixel (dbu 0.001): a frame on
    2/0 fixes the extent, the small shapes are on 1/0 and 3/0."""
    import klayout.db as db
    um = 1000

    def new():
        layout = db.Layout()
        layout.dbu = 0.001
        top = layout.create_cell('TOP')
        top.shapes(layout.layer(2, 0)).insert(db.Box(0, 0, 1000 * um, 1000 * um))
        return layout, top, layout.layer(1, 0), layout.layer(3, 0)

    # shapes only at depth 2: TOP -> MID (0.9 um, nothing of its own) -> LEAF
    layout, top, l1, _ = new()
    mid, leaf = layout.create_cell('MID'), layout.create_cell('LEAF')
    leaf.shapes(l1).insert(db.Box(0, 0, 200, 200))
    for i in range(4):
        mid.insert(db.CellInstArray(leaf.cell_index(), db.Trans(i * 230, 0)))
    for i in range(10):
        for j in range(10):
            top.insert(db.CellInstArray(mid.cell_index(), db.Trans((50 + i * 90) * um, (50 + j * 90) * um)))
    layout.write(str(out / 'depth.oas'))
    # 30 x 30 members of 0.5 px at a 3 px pitch
    layout, top, l1, _ = new()
    leaf = layout.create_cell('LEAF')
    leaf.shapes(l1).insert(db.Box(0, 0, 500, 500))
    top.insert(db.CellInstArray(leaf.cell_index(), db.Trans(400 * um, 400 * um), db.Vector(3 * um, 0), db.Vector(0, 3 * um), 30, 30))
    layout.write(str(out / 'array.oas'))
    # 3/0 in one placement of 32, three clusters (the indexer merges the repeats
    # of a cell into point lists three members wide apart)
    layout, top, l1, l3 = new()
    common, rare = layout.create_cell('LEAF_A'), layout.create_cell('LEAF_B')
    common.shapes(l1).insert(db.Box(0, 0, 300, 300))
    rare.shapes(l3).insert(db.Box(0, 0, 300, 300))
    for cx, cy, which in ((300, 300, 17), (600, 700, 31), (200, 800, 5)):
        for k in range(32):
            cell = rare if k == which else common
            top.insert(db.CellInstArray(cell.cell_index(), db.Trans(cx * um + (k % 6) * 500, cy * um + (k // 6) * 500)))
    layout.write(str(out / 'points.oas'))
    # eight 3 um clusters of 64 cells found nowhere else, one of each on 3/0: a
    # cluster is one child-BVH node box
    layout, top, l1, l3 = new()
    for n in range(8):
        cx, cy = 100 + 110 * n, 100 + 97 * ((n * 5) % 8)
        for k in range(64):
            cell = layout.create_cell('LEAF_%d_%02d' % (n, k))
            cell.shapes(l3 if k == 37 else l1).insert(db.Box(0, 0, 300, 300))
            top.insert(db.CellInstArray(cell.cell_index(), db.Trans(cx * um + (k % 8) * 400, cy * um + (k // 8) * 400)))
    layout.write(str(out / 'nodes.oas'))
    # an abutting 300 x 300 array of a 0.5 um cell that holds 1/0 AND 3/0: one
    # 150 px box for both layers
    layout, top, l1, l3 = new()
    leaf = layout.create_cell('LEAF')
    leaf.shapes(l1).insert(db.Box(0, 0, 500, 500))
    leaf.shapes(l3).insert(db.Box(0, 0, 500, 500))
    top.insert(db.CellInstArray(leaf.cell_index(), db.Trans(400 * um, 400 * um), db.Vector(500, 0), db.Vector(0, 500), 300, 300))
    layout.write(str(out / 'styles.oas'))
    # 64 children that hold only 1/0, under a top with 2/0
    layout, top, l1, _ = new()
    for k in range(64):
        cell = layout.create_cell('C%02d' % k)
        cell.shapes(l1).insert(db.Box(0, 0, 50 * um, 50 * um))
        top.insert(db.CellInstArray(cell.cell_index(), db.Trans(((k % 8) * 100 + 20) * um, ((k // 8) * 100 + 20) * um)))
    layout.write(str(out / 'frames.oas'))


def review_cases(temp):
    review_layouts(temp)
    view, size = (0.0, 0.0, 1_000_000.0, 1_000_000.0), (1000, 1000)
    seen = {}
    for name in ('depth', 'array', 'points', 'nodes', 'styles', 'frames'):
        done = subprocess.run([sys.executable, '-B', '-m', 'floe2', 'index', str(temp / (name + '.oas'))],
                              cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
        assert done.returncode == 0, done.stdout + done.stderr
        off, on = worker(temp / (name + '.oas'), False), worker(temp / (name + '.oas'), True)
        try:
            if name == 'styles':
                clear = '\n'.join(['.' * 16] * 16)
                alone, _ = frame(on, 1, view, [(1, 0)], 'keep', size=size)
                on.submit({'kind': 'repattern', 'fills': [((3, 0), clear)], 'widths': []})
                under, res = frame(on, 2, view, [(1, 0), (3, 0)], 'keep', size=size)
                assert lit(alone) > 10_000 and lit(under) >= lit(alone), (
                    'a clear layer on top erased the one under it: %d px alone, %d px under it'
                    % (lit(alone), lit(under)))
                assert res['plan_culls']['sub_cut_boxes'] == 2, res['plan_culls']
                seen[name] = '%d px alone, %d px under a clear layer' % (lit(alone), lit(under))
                continue
            if name == 'frames':
                # the same worker, the frames switch toggled: off, on, off
                planned = [frame(on, gen, view, [(2, 0)], 'keep', depth=0, size=size, frames=want)[1]['frame_rects']
                           for gen, want in ((1, False), (2, True), (3, False))]
                assert planned == [0, 64, 0], 'frame rects planned with frames off/on/off: %s' % planned
                seen[name] = 'frames off plans none, on plans 64'
                continue
            if name == 'depth':
                for depth, want in ((1, False), (2, True), (None, True)):
                    pixels, res = frame(on, depth or 9, view, [(1, 0)], 'keep', depth=depth, size=size)
                    assert (lit(pixels) > 0) == want and (res['plan_culls']['sub_cut_boxes'] > 0) == want, (
                        'depth %s: %d px lit, %s' % (depth, lit(pixels), res['plan_culls']))
                seen[name] = 'no box under the depth limit'
                continue
            layer = (1, 0) if name == 'array' else (3, 0)
            # the truth: the members themselves, drawn with a cut under their size
            truth, _ = frame(off, 1, view, [layer], 'keep', size=size, cut_px=0.25)
            gone, _ = frame(off, 2, view, [layer], 'keep', size=size)
            boxed, res = frame(on, 1, view, [layer], 'keep', size=size)
            culls = res['plan_culls']
            assert lit(gone) == 0 and lit(truth) > 0, (name, lit(gone), lit(truth))
            if name == 'nodes':
                # one box of at most 4 x 4 px per cluster, where the rare cell really is
                assert culls['sub_cut_boxes'] == 8 and 8 <= lit(boxed) <= 8 * 16, (lit(boxed), culls)
            else:
                assert lit(boxed) == lit(truth), '%s: %d px lit, the members light %d' % (name, lit(boxed), lit(truth))
            seen[name] = '%d px (members: %d)' % (lit(boxed), lit(truth))
        finally:
            off.stop()
            on.stop()
    return seen


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
            # sixteen layers: more lit than the kill switch, and one rect per box -
            # never more rects than the sixteen layers planned one at a time
            sixteen = every[::max(1, len(every) // 16)][:16]
            sparse, _ = frame(off, 7, wide, sixteen, 'keep')
            many, res16 = frame(on, 7, wide, sixteen, 'keep')
            boxes16 = res16['plan_culls']['sub_cut_boxes']
            singles = sum(frame(on, 10 + k, wide, [key], 'keep')[1]['plan_culls']['sub_cut_boxes']
                          for k, key in enumerate(sixteen))
            assert lit(many) > lit(sparse) and 0 < boxes16 <= singles, (lit(many), lit(sparse), boxes16, singles)
            print('sub-cut box: one via layer of the chip, keep: %d px lit by %d boxes (kill switch: empty), '
                  '%d other frames unchanged; sixteen layers: %d -> %d px lit, %d boxes (one by one: %d)'
                  % (lit(boxed), culls['sub_cut_boxes'], same, lit(sparse), lit(many), boxes16, singles))
        finally:
            off.stop()
            on.stop()
        print('sub-cut box review cases: %s' % review_cases(Path(temp)))
    print('SUB-CUT BOX: ALL OK')


if __name__ == '__main__':
    main()
