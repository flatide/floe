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


def frame(w, gen, depth=None, cut=3, px=500):
    w.submit({'kind': 'repattern', 'fills': [((1, 0), '\n'.join(['*' * 16] * 16))],
              'widths': [((1, 0), 1)]})
    w.submit({'kind': 'render', 'gen': gen, 'scope': 'headless',
              'bbox': (0., 0., 1_500_000., 1_500_000.), 'view': None,
              'w': px, 'h': px, 'depth': depth, 'cut_px': cut,
              'lod': False, 'frames': False, 'labels': False, 'abstract': False,
              'visible': [(1, 0)], 'frame_format': 'raw', 'thin': 'cull'})
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
        print('representatives: additive preservation, normal build, depth, pixel replay, kill switch, invalid fallback OK')


if __name__ == '__main__':
    main()
