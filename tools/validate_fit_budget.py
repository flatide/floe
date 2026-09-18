#!/usr/bin/env python3
"""Budget-fitted cut gate (rust/vfs/src/hier.rs plan_hier, 2026-09-18).

Field: `thin keep` + detail high is the picture closest to Calibre, but a
wide view, many layers or a deep depth ended in "decoded generation budget
exceeded". The planner fits such a plan to the generation budget: since
0.12.166 by lowering the DENSITY at the requested cut (one page in 2^k below a
complete tier of the largest pages; field 2026-09-19: raising the cut emptied
the screen where a view's shapes are one size class), before that by raising
the cut (FLOE_RUST_FIT_THIN=off). This gate renders a synthetic MAIN01-class
chip (tools/gen_main01_like.py) under a small budget:

  * keep + cut 1 px over the whole chip, every layer: a frame with something
    on it, fit_thin > 0, where FLOE_RUST_FIT_THIN=off raises the cut
    (fit_pct > 100, fit_thin 0) and FLOE_RUST_FIT_BUDGET=off gives the old
    error;
  * a frame that fits is untouched: fit_pct 0 and pixel-identical to the
    kill switch's frame, at the wide view under a large budget and at a near
    view under the small one.

    .venv/bin/python tools/validate_fit_budget.py
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

PX = 1920       # at this size keep + cut 1 px of the whole chip decodes ~110 MB


def worker(src, budget_mb, fit=True, thin=True):
    os.environ['FLOE_RUST_BUDGET_MB'] = str(budget_mb)
    if fit:
        os.environ.pop('FLOE_RUST_FIT_BUDGET', None)
    else:
        os.environ['FLOE_RUST_FIT_BUDGET'] = 'off'
    if thin:
        os.environ.pop('FLOE_RUST_FIT_THIN', None)
    else:
        os.environ['FLOE_RUST_FIT_THIN'] = 'off'
    cache = Cache(str(src))
    cache.load()
    w = RustRenderWorker(cache)
    w.start()
    os.environ.pop('FLOE_RUST_BUDGET_MB', None)
    os.environ.pop('FLOE_RUST_FIT_BUDGET', None)
    os.environ.pop('FLOE_RUST_FIT_THIN', None)
    return w


def frame(w, gen, bbox):
    keys = [(int(l['layer']), int(l['datatype'])) for l in w.cache.meta['layers']]
    w.submit({'kind': 'render', 'gen': gen, 'scope': 'headless', 'bbox': bbox, 'view': None,
              'w': PX, 'h': PX, 'depth': None, 'cut_px': 1, 'lod': False, 'frames': False,
              'labels': False, 'abstract': False, 'visible': keys, 'frame_format': 'raw',
              'thin': 'keep'})
    deadline = time.monotonic() + 300
    while time.monotonic() < deadline:
        res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
        if res.get('kind') == 'error':
            return None, res
        if res.get('kind') == 'frame' and res.get('gen') == gen and not res.get('refining'):
            pixels = res.pop('rgba')        # keep assertion messages readable
            return pixels, res
    raise AssertionError('fit budget frame timeout')


def main():
    os.environ['FLOE_INDEX_BIN'] = str(ROOT / 'rust/target/release/floe-index')
    os.environ['FLOE_RENDERD_BIN'] = str(ROOT / 'rust/target/release/floe-renderd')
    os.environ['FLOE_RUST_RETAINED_MB'] = '0'
    with tempfile.TemporaryDirectory(prefix='floe-fit-') as temp:
        src = Path(temp) / 'chip.oas'
        for argv in ([sys.executable, '-B', str(ROOT / 'tools/gen_main01_like.py'), str(src),
                      '--scale', '0.003', '--jobs', '2', '--geometry', 'legacy'],
                     [sys.executable, '-B', '-m', 'floe2', 'index', str(src), '--jobs', '2']):
            done = subprocess.run(argv, cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
            assert done.returncode == 0, done.stdout + done.stderr
        tight, tight_off = worker(src, 48), worker(src, 48, fit=False)
        ladder = worker(src, 48, thin=False)
        roomy, roomy_off = worker(src, 1024), worker(src, 1024, fit=False)
        try:
            x0, y0, x1, y1 = map(float, tight.cache.meta['bbox'])
            wide = (x0, y0, x1, y1)
            near = (x0, y0, x0 + (x1 - x0) / 64, y0 + (y1 - y0) / 64)
            old, err = frame(tight_off, 1, wide)
            assert old is None and 'decoded generation budget exceeded' in str(err), (
                'the kill switch no longer reproduces the field error', err.get('tiles'), err.get('resident_mb'))
            fitted, res = frame(tight, 2, wide)
            culls = res['plan_culls']
            assert fitted is not None and culls['fit_thin'] > 0 and culls['fit_over'] == 0, culls
            background = bytes(fitted[:4])
            lit = sum(1 for i in range(0, len(fitted), 4) if bytes(fitted[i:i + 4]) != background)
            assert lit > 0, 'the fitted frame is empty'
            raised, rres = frame(ladder, 7, wide)
            rculls = rres['plan_culls']
            assert raised is not None and rculls['fit_pct'] > 100 and rculls['fit_thin'] == 0, rculls
            a, ra = frame(roomy, 3, wide)
            b, _ = frame(roomy_off, 4, wide)
            assert ra['plan_culls']['fit_pct'] == 0 and a == b, 'a frame that fits must not change'
            c, rc = frame(tight, 5, near)
            d, _ = frame(tight_off, 6, near)
            assert rc['plan_culls']['fit_pct'] == 0 and c == d and any(c), 'near view under the small budget'
            print('fit budget: wide keep view fits 48 MB at 1/%d below x%.3g, %d px lit '
                  '(ladder: cut x%.3g; old: error), fitting frames unchanged'
                  % (1 << culls['fit_thin'] if culls['fit_thin'] < 255 else 0, culls['fit_full_pct'] / 100.0,
                     lit, rculls['fit_pct'] / 100.0))
        finally:
            for w in (tight, tight_off, ladder, roomy, roomy_off):
                w.stop()
    print('FIT BUDGET: ALL OK')


if __name__ == '__main__':
    main()
