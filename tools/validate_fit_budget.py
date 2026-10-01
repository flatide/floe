#!/usr/bin/env python3
"""Budget-fitted cut gate (rust/vfs/src/hier.rs plan_hier, 2026-09-18).

Field: `thin keep` + detail high is the picture closest to Calibre, but a
wide view, many layers or a deep depth ended in "decoded generation budget
exceeded". The planner fits such a plan to the generation budget: since
0.12.166 by lowering the DENSITY at the requested cut (one page in 2^k below a
complete tier of the largest size classes; field 2026-09-19: raising the cut emptied
the screen where a view's shapes are one size class), before that by raising
the cut (FLOE_RUST_FIT_THIN=off). This gate renders a synthetic MAIN01-class
chip (tools/gen_main01_like.py) under a small budget:

  * keep + cut 1 px over the whole chip, every layer: a frame with something
    on it, fit_thin > 0, where FLOE_RUST_FIT_THIN=off raises the cut
    (fit_pct > 100, fit_thin 0) and FLOE_RUST_FIT_BUDGET=off gives the old
    error;
  * a frame that fits is untouched: fit_pct 0 and pixel-identical to the
    kill switch's frame, at the wide view under a large budget and at a near
    view under the small one;
  * the fit is remembered per scale (2026-09-27: the viewer's margin frame, a
    wider view, thinned differently and the picture changed when it replaced
    the viewport frame): the chip's middle at the small budget decides over
    the extent of its margin (fit_fixed 1), the whole chip at the same scale
    (twice the pixels per axis - the margin) applies that decision (fit_fixed
    1, fit_redecided 0), the middle again too, and both middles equal the
    margin's centre pixel for pixel; a middle shifted by a fraction of a
    pixel's worth (its px_per_dbu differs in the last bits) shares the
    memory; a frame within the budget carries the decision too (everything;
    fit_fixed 1, fit_thin 0);
  * a margin (bg) the decision does not hold is dropped, not decided anew
    (the viewer's margin must look as the viewport already does): on a
    layout whose corner quarter holds four distinct cells and whose far half
    two hundred (layout_uneven), under a 1 MB budget the corner decides
    everything, the whole layout as its margin answers `dropped` (reason
    fit), the corner draws the same after it, and the whole layout as a
    viewport frame refits (fit_redecided 1) - reusing no tile of the corner's
    retained frame (drawn under the old decision) and equal to a fresh render;
    the same view again reuses tiles under the new decision;
  * a frame its budget holds whole keeps every page whatever the scale
    remembers (user 2026-10-01: the decision is kept per scale, not per place,
    and a dense view's emptied a sparse view at the same zoom step): after the
    refit the corner draws exactly as at first (fit_thin 0, not refit) and the
    whole layout again under the refit's decision.

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


def worker(src, budget_mb, fit=True, thin=True, retained_mb=0):
    os.environ['FLOE_RUST_BUDGET_MB'] = str(budget_mb)
    os.environ['FLOE_RUST_RETAINED_MB'] = str(retained_mb)
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
    os.environ['FLOE_RUST_RETAINED_MB'] = '0'
    return w


def frame(w, gen, bbox, size=PX, bg=False):
    keys = [(int(l['layer']), int(l['datatype'])) for l in w.cache.meta['layers']]
    w.submit({'kind': 'render', 'gen': gen, 'scope': 'headless', 'bbox': bbox, 'view': None,
              'w': size, 'h': size, 'depth': None, 'cut_px': 1, 'lod': False, 'frames': False,
              'labels': False, 'abstract': False, 'visible': keys, 'frame_format': 'raw',
              'thin': 'keep', 'bg': bg})
    deadline = time.monotonic() + 300
    while time.monotonic() < deadline:
        res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
        if res.get('kind') == 'error' or (res.get('kind') == 'dropped' and res.get('gen') == gen):
            return None, res
        if res.get('kind') == 'frame' and res.get('gen') == gen and not res.get('refining'):
            pixels = res.pop('rgba')        # keep assertion messages readable
            return pixels, res
    raise AssertionError('fit budget frame timeout')


def layout_uneven(path):
    """400 x 400 um: four distinct 100-rect cells in the corner quarter, two
    hundred more (each its own cell, so its own pages) in the far half."""
    import klayout.db as kdb
    ly = kdb.Layout()
    ly.dbu = 0.001
    top = ly.create_cell('TOP')
    layer = ly.layer(1, 0)
    cells = 0

    def place(x_um, y_um):
        nonlocal cells
        cell = ly.create_cell('C%03d' % cells)
        for j in range(10):
            for i in range(10):
                # no two alike on a lattice: a regular array of one box would be
                # written as a repetition and indexed as ONE record
                x, y = i * 1.8 + 0.1 * ((i * 7 + j * 3) % 4), j * 1.8 + 0.1 * ((i + j * 5) % 3)
                edge = 1.0 + 0.05 * ((i + j * 3 + cells) % 6)
                cell.shapes(layer).insert(kdb.DBox(x, y, x + edge, y + edge))
        top.insert(kdb.DCellInstArray(cell.cell_index(), kdb.DTrans(kdb.DVector(x_um, y_um))))
        cells += 1

    for j in range(2):
        for i in range(2):
            place(10.0 + i * 50.0, 10.0 + j * 50.0)
    for j in range(20):
        for i in range(10):
            place(200.0 + i * 20.0, j * 20.0)
    ly.write(str(path))


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
            # a frame within the budget carries its scale's decision too: everything
            assert rc['plan_culls']['fit_thin'] == 0 and rc['plan_culls']['fit_fixed'] == 1, rc['plan_culls']
            # the fit remembered per scale: the middle, the whole chip at the
            # same scale (the margin), the middle again
            sticky = worker(src, 48)
            try:
                mid = (x0 + (x1 - x0) / 4, y0 + (y1 - y0) / 4, x0 + 3 * (x1 - x0) / 4, y0 + 3 * (y1 - y0) / 4)
                first, rf = frame(sticky, 8, mid)
                margin, rm = frame(sticky, 9, wide, size=2 * PX)
                again, ra2 = frame(sticky, 10, mid)
                fits = (rf['plan_culls'], rm['plan_culls'], ra2['plan_culls'])
                assert fits[0]['fit_thin'] > 0, 'the middle must need the fit: %s' % (fits[0],)
                # the first frame decides over the margin's extent and applies it
                assert fits[0]['fit_fixed'] == 1 and fits[0]['fit_redecided'] == 0, 'the middle must decide over its margin: %s' % (fits[0],)
                assert fits[1]['fit_fixed'] == 1 and fits[1]['fit_redecided'] == 0, 'the margin must apply the decision: %s' % (fits[1],)
                assert fits[2]['fit_fixed'] == 1 and fits[2]['fit_redecided'] == 0, 'the middle again must apply the remembered fit: %s' % (fits[2],)
                assert (fits[1]['fit_thin'], fits[1]['fit_none_pct']) == (fits[0]['fit_thin'], fits[0]['fit_none_pct']), (fits[0], fits[1])
                q = PX // 2
                centre = b''.join(bytes(margin[((q + r) * 2 * PX + q) * 4:((q + r) * 2 * PX + q + PX) * 4]) for r in range(PX))
                assert bytes(again) == centre, 'the middle again differs from the margin\'s centre'
                assert bytes(first) == centre, 'the margin drew the middle differently from the first frame'
                # a frame at the same scale whose box derives px_per_dbu with other last bits shares the memory
                step = (x1 - x0) / 4 / PX
                shifted = (mid[0] + step / 3, mid[1] + step / 7, mid[2] + step / 3, mid[3] + step / 7)
                _, rs = frame(sticky, 11, shifted)
                assert rs['plan_culls']['fit_fixed'] == 1 and rs['plan_culls']['fit_redecided'] == 0, 'a shifted middle must share the memory: %s' % (rs['plan_culls'],)
                print('fit budget: the fit is remembered per scale - middle 1/%d (none below x%.3g) decided over its margin, margin and middle again apply it '
                      '(= margin centre), a shifted middle shares it' % (1 << fits[0]['fit_thin'], fits[0]['fit_none_pct'] / 100.0))
            finally:
                sticky.stop()
            # a margin the decision does not hold is dropped, not decided anew. The
            # synthetic chip repeats its cells, so a wider view needs few new pages;
            # this fixture's far half holds 200 DISTINCT cells (4.6 MB of pages at
            # 192 B a record) and its corner quarter four: under a 1 MB budget the
            # corner decides over its own margin (nothing of the far half in it:
            # everything), the whole layout as its margin cannot hold that
            uneven = Path(temp) / 'uneven.oas'
            layout_uneven(uneven)
            done = subprocess.run([sys.executable, '-B', '-m', 'floe2', 'index', str(uneven), '--jobs', '2'],
                                  cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
            assert done.returncode == 0, done.stdout + done.stderr
            # retention on: the corner's frame would serve the whole layout's
            # frame at the same scale (a pan-like overlap)
            tiny, fresh = worker(uneven, 1, retained_mb=256), worker(uneven, 1)
            try:
                ux0, uy0, ux1, uy1 = map(float, tiny.cache.meta['bbox'])
                side = max(ux1 - ux0, uy1 - uy0)
                whole = (ux0, uy0, ux0 + side, uy0 + side)
                quarter = (ux0, uy0, ux0 + side / 4, uy0 + side / 4)
                corner, rq_ = frame(tiny, 1, quarter, size=PX // 4)
                assert corner is not None and rq_['plan_culls']['fit_fixed'] == 1 and rq_['plan_culls']['fit_thin'] == 0, rq_.get('plan_culls')
                assert any(corner), 'the corner quarter is empty'
                dropped, rd = frame(tiny, 2, whole, size=PX, bg=True)
                assert dropped is None and rd.get('kind') == 'dropped' and rd.get('reason') == 'fit', 'the margin over the decision must be dropped: %s' % (
                    rd.get('plan_culls') if rd.get('kind') == 'frame' else rd,)
                corner2, rq2 = frame(tiny, 3, quarter, size=PX // 4)
                # (with retention on the exact revisit is the retained frame itself - the
                # label-only fast path plans nothing and reports no fit)
                assert (rq2['plan_culls']['fit_fixed'] == 1 or rq2['tiles_reused'] > 0) and rq2['plan_culls']['fit_redecided'] == 0 \
                    and bytes(corner2) == bytes(corner), \
                    'the dropped margin must leave the decision and the viewport as they were: %s' % (rq2['plan_culls'],)
                # the same extent as a VIEWPORT frame decides anew - the legitimate refit -
                # and reuses NO tile of the corner's frame (review 2026-09-28: the reused
                # tiles carried the old decision's pages beside the new one's): it equals
                # a fresh worker's frame byte for byte
                view, rv = frame(tiny, 4, whole, size=PX)
                assert view is not None and rv['plan_culls']['fit_redecided'] == 1 and rv['plan_culls']['fit_thin'] > 0, rv.get('plan_culls')
                assert rv['tiles_reused'] == 0, 'the refit frame reused %d tiles of the frame under the old decision' % rv['tiles_reused']
                plain, rp = frame(fresh, 4, whole, size=PX)
                assert bytes(view) == bytes(plain), 'the refit frame differs from a fresh render (%s vs %s)' % (rv['plan_culls'], rp['plan_culls'])
                # under the new decision the frame is retained and serves the same view again
                again, ra_ = frame(tiny, 5, whole, size=PX)
                assert ra_['tiles_reused'] > 0 and bytes(again) == bytes(view), (ra_['plan_culls'], ra_['tiles_reused'])
                # the corner at the same scale is still one its budget holds whole: it keeps
                # every page whatever the scale now remembers (user 2026-10-01: a dense
                # view's decision, kept per scale, emptied a sparse view at the same zoom
                # step) - drawn as at first, not from the thinned frames that cover it -
                # and the remembered decision stays for the frames that have to thin
                corner3, rq3 = frame(tiny, 6, quarter, size=PX // 4)
                assert bytes(corner3) == bytes(corner) and rq3['plan_culls']['fit_thin'] == 0 \
                    and rq3['plan_culls']['fit_redecided'] == 0, 'the corner after the refit: %s' % (rq3['plan_culls'],)
                again2, ra2_ = frame(tiny, 7, whole, size=PX)
                assert bytes(again2) == bytes(view) and ra2_['plan_culls']['fit_redecided'] == 0 \
                    and ra2_['plan_culls']['fit_thin'] == rv['plan_culls']['fit_thin'], ra2_['plan_culls']
                print('fit budget: a margin the decision does not hold is dropped (reason fit) and the corner draws the same after it; '
                      'the same extent as a viewport refits (1/%d) reusing no tile and equals a fresh render; the same view again reuses %d tiles; '
                      'the corner after it, held whole, draws as at first'
                      % (1 << rv['plan_culls']['fit_thin'], ra_['tiles_reused']))
            finally:
                tiny.stop()
                fresh.stop()
            print('fit budget: wide keep view fits 48 MB with 1/%d of the class it ends in (complete from x%.3g, '
                  'none below x%.3g; 0 = no such class), %d px lit (ladder: cut x%.3g; old: error), '
                  'fitting frames unchanged'
                  % (1 << culls['fit_thin'], culls['fit_full_pct'] / 100.0, culls['fit_none_pct'] / 100.0,
                     lit, rculls['fit_pct'] / 100.0))
        finally:
            for w in (tight, tight_off, ladder, roomy, roomy_off):
                w.stop()
    print('FIT BUDGET: ALL OK')


if __name__ == '__main__':
    main()
