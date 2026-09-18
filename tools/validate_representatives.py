#!/usr/bin/env python3
"""Small OVR integration gate; intentionally independent of the long battery.

Run after building floe-index and floe-renderd:
    .venv/bin/python tools/validate_representatives.py
"""
import hashlib
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

import klayout.db as db

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from floe.cachepath import vfs_cache_dir
from floe.cache import Cache
from floe.rust_render import RustRenderWorker


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def index(src, *args, ok=True):
    result = subprocess.run(
        [sys.executable, '-B', '-m', 'floe2', 'index', str(src), '--jobs', '2', *args],
        cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=60)
    assert (result.returncode == 0) == ok, result.stdout + result.stderr
    return result


def worker(src, off=False, unbinned=False):
    os.environ['FLOE_RUST_REPRESENTATIVES'] = 'off' if off else 'on'
    # Leave unrelated diagnostic paths off, regardless of the caller's shell.
    for key in ('FLOE_RUST_PAGE_REPS', 'FLOE_RUST_SUB_CUT_WASH'):
        os.environ.pop(key, None)
    os.environ['FLOE_RUST_WORK_BIN'] = 'off' if unbinned else 'on'
    cache = Cache(str(src))
    cache.load()
    result = RustRenderWorker(cache)
    result.start()
    return result


def frame(w, gen, depth=None, cut=3, px=500, span=1_500_000., visible=((1, 0),)):
    visible = list(visible)
    w.submit({'kind': 'repattern', 'fills': [(k, '\n'.join(['*' * 16] * 16)) for k in visible],
              'widths': [(k, 1) for k in visible]})
    w.submit({'kind': 'render', 'gen': gen, 'scope': 'headless',
              'bbox': (0., 0., float(span), float(span)), 'view': None,
              'w': px, 'h': px, 'depth': depth, 'cut_px': cut,
              'lod': False, 'frames': False, 'labels': False, 'abstract': False,
              'visible': visible, 'frame_format': 'raw', 'thin': 'cull'})
    deadline = time.monotonic() + 60
    while time.monotonic() < deadline:
        res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
        assert res.get('kind') != 'error', res
        if res.get('kind') == 'frame' and res.get('gen') == gen and not res.get('refining'):
            data = res['rgba']
            lit = {(i % px, i // px) for i in range(px * px)
                   if any(data[i * 4:i * 4 + 3])}
            return lit, res
    raise AssertionError('representative frame timeout')


def ovr2_section(temp):
    """OVR2 step 1 (docs/OVR2_DESIGN.ko.md): the same samples stored as
    shapes. A sub-cut hairline keeps its projected length (4, 3, 2, 1 px as
    the view widens) under a rotation too, every lit pixel lies on real
    geometry, the additive build leaves the cache alone, an OVR1 next to a
    current cache does not hide a requested OVR2, and OVR1 still reads."""
    src = Path(temp) / 'lines.oas'
    ly = db.Layout()
    ly.dbu = .001
    top = ly.create_cell('TOP')
    vl = ly.create_cell('VL')
    l2, l3, l4 = ly.layer(2, 0), ly.layer(3, 0), ly.layer(4, 0)
    for i in range(40):     # 0.05 x 400 um hairlines, 30 um apart
        top.shapes(l2).insert(db.Box(i * 30000 + 1000, 1000, i * 30000 + 1050, 401000))
    top.shapes(l3).insert(db.Box(100000, 800000, 500000, 800050))      # one horizontal line
    vl.shapes(l4).insert(db.Box(0, 0, 400000, 50))                     # horizontal in VL ...
    top.insert(db.CellInstArray(vl.cell_index(), db.Trans(1, False, 900000, 600000)))  # ... vertical in TOP
    ly.write(str(src))
    index(src)
    cache = Path(vfs_cache_dir(src))
    before = {p.name: digest(p) for p in cache.iterdir() if p.is_file()}
    index(src, '--representatives-only', '--representatives-points', '4096')
    sidecar = cache / 'design.ovr'
    assert sidecar.read_bytes()[:8] == b'FLOEOVR1'
    layers = ((2, 0), (3, 0), (4, 0))
    points = worker(src)
    try:
        dots, _ = frame(points, 20, visible=layers)
        dot_line, _ = frame(points, 21, px=200, span=20_000_000., visible=[(3, 0)])
    finally:
        points.stop()
    # a current cache with an OVR1: the format option alone rebuilds it as OVR2
    result = index(src, '--representatives', '--representatives-format', '2',
                   '--representatives-points', '4096')
    assert sidecar.read_bytes()[:8] == b'FLOEOVR2', result.stdout + result.stderr
    assert 'format=2' in result.stderr and 'rects=42' in result.stderr, result.stderr
    assert before == {name: digest(cache / name) for name in before}, 'additive OVR2 build changed the index'
    shapes = worker(src)
    exact = worker(src)
    try:
        lines, report = frame(shapes, 22, visible=layers)
        real, _ = frame(exact, 23, cut=0, visible=layers)
        assert report['plan_culls']['stored_rep_points'] == 42, report['plan_culls']
        assert report['cache_miss'] == 0, 'shapes need no page decode'
        # 40 hairlines of 133 px + two 133 px lines against 42 dots
        assert len(dots) <= 42 and len(lines) > 100 * len(dots) // 2, (len(dots), len(lines))
        stray = [p for p in lines if not any((p[0] + dx, p[1] + dy) in real
                                             for dx in (-1, 0, 1) for dy in (-1, 0, 1))]
        assert not stray, 'shape pixels off the real geometry: %s' % stray[:5]
        # the projected length of ONE 400 um line at 100, 133, 200, 400 um per
        # pixel, horizontal (3/0) and rotated to vertical (4/0): a group of one
        # member is always sampled, and the shape takes the real rect's paint
        # path, so the picture IS the exact one - whatever the raster's
        # hairline rule gives across the line - and the long axis shrinks
        # 4, 3, 2, 1 px with the view instead of being a dot from the start
        lengths = []
        for k, span in enumerate((20_000_000., 26_666_667., 40_000_000., 80_000_000.)):
            for layer, axis in (((3, 0), 0), ((4, 0), 1)):
                lit, _ = frame(shapes, 30 + 4 * k + 2 * axis, px=200, span=span, visible=[layer])
                ref, _ = frame(exact, 31 + 4 * k + 2 * axis, px=200, span=span, cut=0, visible=[layer])
                assert lit == ref and lit, (layer, span, sorted(lit), sorted(ref))
                lengths.append((layer, span, len({p[axis] for p in lit})))
        for layer in ((3, 0), (4, 0)):
            got = [n for l, _, n in lengths if l == layer]
            assert got == sorted(got, reverse=True) and got[0] >= 4 and got[-1] <= 2, lengths
        assert len(dot_line) == 1, 'OVR1 shows the same line as one dot: %d' % len(dot_line)
    finally:
        for w in (shapes, exact):
            w.stop()
    print('representatives OVR2: %d dots -> %d shape px, projected lengths %s' % (
        len(dots), len(lines), [n for _, _, n in lengths]))


def main():
    os.environ['FLOE_INDEX_BIN'] = str(ROOT / 'rust/target/release/floe-index')
    os.environ['FLOE_RENDERD_BIN'] = str(ROOT / 'rust/target/release/floe-renderd')
    os.environ['FLOE_RUST_OCCUPANCY'] = 'off'
    os.environ['FLOE_RUST_RETAINED_MB'] = '0'
    with tempfile.TemporaryDirectory(prefix='floe-representatives-') as temp:
        src = Path(temp) / 'points.oas'
        ly = db.Layout()
        ly.dbu = .001
        top = ly.create_cell('TOP')
        dot = ly.create_cell('DOT')
        layer = ly.layer(1, 0)
        dot.shapes(layer).insert(db.Box(0, 0, 100, 100))
        top.insert(db.CellInstArray(dot.cell_index(), db.Trans(250000, 250000),
                                   db.Vector(1000, 0), db.Vector(0, 1000), 1024, 1024))
        for y in range(128):
            for x in range(128):
                top.shapes(layer).insert(db.Box(x * 1000, y * 1000, x * 1000 + 100, y * 1000 + 100))
        ly.write(str(src))
        index(src)
        cache = Path(vfs_cache_dir(src))
        before = {p.name: digest(p) for p in cache.iterdir() if p.is_file()}
        result = index(src, '--representatives-only', '--representatives-points', '65536')
        print(result.stderr.strip().splitlines()[-2])
        assert before == {name: digest(cache / name) for name in before}, 'additive build changed the index'
        sidecar = cache / 'design.ovr'
        saved = sidecar.read_bytes()
        assert saved[:8] == b'FLOEOVR1'
        assert len(saved) < 4 * 1024 * 1024, len(saved)
        index(src, '--representatives-only', '--representatives-points', '4194305', ok=False)
        assert sidecar.read_bytes() == saved, 'failed build replaced a valid sidecar'

        on = worker(src)
        off = worker(src, off=True)
        slow = worker(src, unbinned=True)
        try:
            full, result = frame(on, 1)
            own, own_result = frame(on, 2, depth=0)
            none, off_result = frame(off, 3)
            walk, _ = frame(slow, 4)
            assert not none, 'kill switch must retain the old cull'
            assert len(full) > 1000 and len(own) > 10, (len(full), len(own))
            assert own < full, 'depth zero must exclude child samples'
            assert walk == full, 'chunked point replay differs from the unbinned renderer'
            assert all(x < 43 and y >= 456 for x, y in own), 'depth-zero point escaped the own-geometry region'
            for report in (result, own_result):
                assert report['cache_miss'] == 0 and report['resident_mb'] == 0, report
                assert report['plan_culls']['stored_rep_points'] > 0, report
                assert report['plan_culls']['rep_kept'] == 0, 'old frontier was revived'
            assert off_result['plan_culls']['stored_rep_points'] == 0
            print('representatives: full=%d px depth0=%d px plan=%.2f ms raster=%.2f ms, no page decode' %
                  (len(full), len(own), result['plan_ms'], result['raster_ms']))
        finally:
            for w in (on, off, slow):
                w.stop()

        # Invalid sidecar falls back to ordinary cull; it cannot make a valid
        # OVM/OVP cache unreadable. Reopen is the sidecar snapshot boundary.
        sidecar.write_bytes(saved[:-1])
        invalid = worker(src)
        try:
            assert not frame(invalid, 5)[0]
        finally:
            invalid.stop()
        sidecar.write_bytes(saved)
        index(src, '--force', '--representatives', '--representatives-points', '65536')
        assert sidecar.read_bytes() == saved, 'normal/additive builds disagree'

        # A combined run whose OVR build fails (simulated by the gate-only
        # --kill-at hook of the binary) still completes the base cache:
        # design.ovm + marker are written, no design.ovr(.tmp) remains,
        # the warning names the additive rerun, and the cache opens.
        fi = os.environ['FLOE_INDEX_BIN']
        failed = subprocess.run(
            [fi, 'vfs', str(src), str(cache), '--representatives', '--kill-at', 'representatives-fail'],
            capture_output=True, text=True, timeout=60)
        assert failed.returncode == 0, failed.stderr
        assert 'representatives-fail' in failed.stderr and '--representatives-only' in failed.stderr, failed.stderr
        assert not sidecar.exists() and not (cache / 'design.ovr.tmp').exists(), 'failed combined build left an OVR'
        assert (cache / 'meta.json').exists() and (cache / 'design.ovm').exists(), 'combined build lost the cache'
        opened = subprocess.run([fi, 'plan', str(cache), '--view', '0,0,1,1'],
                                capture_output=True, text=True, timeout=60)
        assert opened.returncode == 0, opened.stderr
        # the standalone additive run of the same failure is loud and leaves
        # the cache alone: every file's bytes are the same after it
        untouched = {p.name: digest(p) for p in cache.iterdir() if p.is_file()}
        loud = subprocess.run(
            [fi, 'vfs', str(src), str(cache), '--representatives-only', '--kill-at', 'representatives-fail'],
            capture_output=True, text=True, timeout=60)
        assert loud.returncode != 0 and not sidecar.exists(), (loud.returncode, loud.stderr)
        assert untouched == {p.name: digest(p) for p in cache.iterdir() if p.is_file()}, \
            'additive failure touched the cache'
        index(src, '--representatives-only', '--representatives-points', '65536')
        assert sidecar.read_bytes()[:8] == b'FLOEOVR1'
        print('representatives: additive preservation, normal build, depth, pixel replay, kill switch, invalid fallback, '
              'combined-run failure keeps the cache OK')
        ovr2_section(temp)
        print('representatives: OVR2 shapes (length, rotation, on-geometry, additive, format switch) OK')


if __name__ == '__main__':
    main()
