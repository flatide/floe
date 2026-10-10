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
    whole layout again under the refit's decision;
  * a frame the planner fitted holds the budget once decoded, and one whose
    pages pass it is planned anew, not failed (field 2026-10-05, two layers
    alone with the density off: "most frames fail with `decoded generation
    budget exceeded`"): the planner fits the pages to the budget by an
    estimate of their decoded size, and pages charged more passed it. On
    plain boxes whose pages' record lists, as the parser reads them, are
    about half full (layout_plain: a page charged 1.12 of its estimate)
    under 26 MB: with the lists as read (FLOE_RUST_DECODE_SHRINK=off) and
    FLOE_RUST_BUDGET_REFIT=off the old error - or the layout no longer
    reproduces it and the gate fails; now the lists are cut to their length
    and the fitted plan is drawn whole (fit_refits 0, fit_scale 0); with the
    lists as read the frame is planned anew under the budget over what its
    pages took (fit_refits 1 or more, fit_scale past 1000, no page over the
    budget, fewer pages than the whole plan) and drawn, the next frame of
    the layers starts there (fit_refits 0, the same pixels), and a frame
    with the density stack, whose pass 1 has pass 2's reserve to spare, is
    not planned under less (fit_scale 0); the density stack counts its pages
    as read, so its frame is the same bytes with the lists cut. The viewer's
    margin (bg) - a viewport inside one page, its margin over four: drawn
    now; with the lists as read dropped (reason budget), where
    FLOE_RUST_BUDGET_REFIT=off gives the error, the next viewport frame
    planned under the raised scale with the same pixels and the margin after
    it a frame or dropped (reason fit), no error. On a layout whose records
    share their repetition lists (layout_shared) under 4 MB: the pages'
    charge counts a shared list once, so the fitted frame holds (fit_refits
    0); with 0.12.300's charge (FLOE_RUST_CHARGE_SHARED=off and the lists as
    read) the frame is planned anew and drawn, the next frame starts there,
    and FLOE_RUST_BUDGET_REFIT=off gives the old error. Under 1 MB, less
    than one of those pages as it was charged, no plan holds: the frame
    draws what the budget holds - nothing here - and reports the pages over
    it, where it was an error too;
  * the fit of a new scale is decided by a plan made for that alone, which
    goes by the hierarchy summary where the walk read every placement of the
    margin's extent (field 2026-10-05: the first frame at a scale was slow,
    the time under `other`): 21 first frames of the chip - whole, and
    windows of a half to a sixteenth of it about five places, each a scale
    of its own - are the same pixels, pages and fit with the summary and
    with FLOE_RUST_FIT_PROBE_SUMMARY=off (the walk), both report the probe's
    time (fit_probe_ms; fit_probe_walk says which), and a scale decided
    before is not probed again.

    .venv/bin/python tools/validate_fit_budget.py
"""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
# the product command line: the Rust `floe2` (rust/floe2; the Python
# floe2 CLI is gone - docs/SHARED_APP_LAYER.ko.md P1c)
FLOE2 = os.environ.get("FLOE2_BIN") or str(ROOT / "rust" / "target" / "release" / "floe2")
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT / "tools" / "oracle"))  # floe_oracle (P3)
from floe_oracle.cache import Cache
from floe_oracle.rust_render import RustRenderWorker

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


def worker_with(src, env, budget_mb):
    """worker() started under `env` (renderd reads it at its start)."""
    saved = {name: os.environ.get(name) for name in env}
    os.environ.update(env)
    try:
        return worker(src, budget_mb)
    finally:
        for name, value in saved.items():
            if value is None:
                os.environ.pop(name, None)
            else:
                os.environ[name] = value


def frame(w, gen, bbox, size=PX, bg=False, density=None):
    keys = [(int(l['layer']), int(l['datatype'])) for l in w.cache.meta['layers']]
    job = {'kind': 'render', 'gen': gen, 'scope': 'headless', 'bbox': bbox, 'view': None,
           'w': size, 'h': size, 'depth': None, 'cut_px': 1, 'lod': False, 'frames': False,
           'labels': False, 'abstract': False, 'visible': keys, 'frame_format': 'raw',
           'thin': 'keep', 'bg': bg}
    if density is not None:
        job['density'] = density
    w.submit(job)
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


def layout_shared(path):
    """3 x 3 mm: forty box sizes (0.10 to 0.49 um), each at the same 30,000
    scattered places - the writer gives each size one record with the places
    as its repetition, and a page's forty records share the list."""
    import random
    import klayout.db as kdb
    ly = kdb.Layout()
    ly.dbu = 0.001
    top = ly.create_cell('TOP')
    layer = ly.layer(1, 0)
    rnd = random.Random(22)
    spots = [(rnd.randrange(3_000_000), rnd.randrange(3_000_000)) for _ in range(30_000)]
    for size in range(40):
        side = 100 + 10 * size
        for (x, y) in spots:
            top.shapes(layer).insert(kdb.Box(x, y, x + side, y + side))
    ly.write(str(path))


def layout_plain(path):
    """3 x 3 mm: 600,000 scattered boxes of 0.6 to 1.0 um and nothing else.
    The indexer's pages of them hold some 33 thousand records, a little past
    half of the room the parser's record lists grow to (65,536), and a page's
    charge counts that room: 1.12 of the planner's estimate of the page."""
    import random
    import klayout.db as kdb
    ly = kdb.Layout()
    ly.dbu = 0.001
    top = ly.create_cell('TOP')
    layer = ly.layer(1, 0)
    rnd = random.Random(21)
    for _ in range(600_000):
        x, y = rnd.randrange(3_000_000), rnd.randrange(3_000_000)
        top.shapes(layer).insert(kdb.Box(x, y, x + 600 + rnd.randrange(400), y + 600 + rnd.randrange(400)))
    ly.write(str(path))


def layout_pair(path):
    """800 x 800 um: 7/59 4,000 squares of 4 um and 14/367 4,000 of 1.5 um
    beside them, each a little off its lattice and written one record each
    (the writer's compression off) - each layer one page of about 0.8 MB,
    every shape over a 0.9 um cut."""
    import random
    import klayout.db as kdb
    ly = kdb.Layout()
    ly.dbu = 0.001
    top = ly.create_cell('TOP')
    low, up = ly.layer(7, 59), ly.layer(14, 367)
    rnd = random.Random(7)
    for k in range(4000):
        i, j = k % 64, k // 64
        x, y = i * 12.5 + rnd.randrange(0, 2000) / 1000, j * 12.5 + rnd.randrange(0, 2000) / 1000
        top.shapes(low).insert(kdb.DBox(x, y, x + 4, y + 4))
        top.shapes(up).insert(kdb.DBox(x + 6, y + 6, x + 7.5, y + 7.5))
    opts = kdb.SaveLayoutOptions()
    opts.format = 'OASIS'
    opts.oasis_compression_level = 0
    ly.write(str(path), opts)


def layout_over(path):
    """7/59's 20 um squares, each under a 30 um square of 14/367, 8 x 8 at
    50 um."""
    import klayout.db as kdb
    ly = kdb.Layout()
    ly.dbu = 0.001
    top = ly.create_cell('TOP')
    low, up = ly.layer(7, 59), ly.layer(14, 367)
    for j in range(8):
        for i in range(8):
            x, y = i * 50.0, j * 50.0
            top.shapes(low).insert(kdb.DBox(x + 5, y + 5, x + 25, y + 25))
            top.shapes(up).insert(kdb.DBox(x, y, x + 30, y + 30))
    ly.write(str(path))


def top_first_worker(src, budget_mb, on=True):
    """worker() with pass 1's budget fit top plane first (the default) or
    by size alone (FLOE_RUST_FIT_TOP_FIRST=off; main() pins it so for the
    checks made by it)."""
    saved = os.environ.get('FLOE_RUST_FIT_TOP_FIRST')
    if on:
        os.environ.pop('FLOE_RUST_FIT_TOP_FIRST', None)
    else:
        os.environ['FLOE_RUST_FIT_TOP_FIRST'] = 'off'
    try:
        return worker(src, budget_mb)
    finally:
        if saved is None:
            os.environ.pop('FLOE_RUST_FIT_TOP_FIRST', None)
        else:
            os.environ['FLOE_RUST_FIT_TOP_FIRST'] = saved


def pair_frame(w, gen, centre_um, width_um, layers, size=(1350, 971)):
    """A frame of the visible `layers` ((layer, datatype)) around `centre_um`,
    `width_um` across, at cut 3 px with the density - the viewer's defaults:
    (pixels as 4-byte colours, result)."""
    dbu = float(w.cache.meta['dbu'])
    wpx, hpx = size
    spp = width_um / wpx / dbu
    cx, cy = centre_um[0] / dbu, centre_um[1] / dbu
    view = (cx - wpx * spp / 2, cy - hpx * spp / 2, cx + wpx * spp / 2, cy + hpx * spp / 2)
    w.submit({'kind': 'render', 'gen': gen, 'scope': 'headless', 'bbox': view, 'view': None, 'w': wpx, 'h': hpx,
              'depth': None, 'cut_px': 3.0, 'lod': False, 'frames': False, 'labels': False, 'abstract': False,
              'visible': list(layers), 'frame_format': 'raw', 'thin': 'keep', 'density': True})
    deadline = time.monotonic() + 300
    while time.monotonic() < deadline:
        res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
        if res.get('kind') == 'error':
            raise AssertionError(res)
        if res.get('kind') == 'frame' and res.get('gen') == gen and not res.get('refining'):
            pixels = bytes(res.pop('rgba'))
            return [pixels[i:i + 4] for i in range(0, len(pixels), 4)], res
    raise AssertionError('top first frame timeout')


def top_first_checks(temp, src):
    """Pass 1's budget fit top plane first (user 2026-10-07, the field chip:
    7.59 and 14.367 on, 7.59 alone drew - `none below x28.2`, its pages of
    larger shapes first in a fit by size alone; "the drawing goes from the
    top, so 14.367 should have been drawn"; the speckle leaves the lower
    shapes' lines showing - pass 1, unlike pass 2, draws the lower shapes in
    the upper ones' speckle holes). On layout_pair, a 400 um view at cut 3
    px under 1 MB (pass 1 about 0.9 MB, a layer's page about 0.8): each layer
    alone draws whole; together the top plane 14/367 draws as alone and
    7/59 is left out (fit_ranked 1, fit_layers_whole 1, no edge layer,
    fit_layers_out 1; the bar `top 1 whole, 1 left out to fit budget`), the
    frame after it under the remembered decision the same pixels; with
    FLOE_RUST_FIT_TOP_FIRST=off the old fit - 7/59 as alone, 14/367 none.
    On layout_over under 1 GB both draw, 7/59's colour inside 14/367's
    squares - its lines in the speckle's holes. On the chip of the checks
    above, every layer under 48 MB: its wide view fits top plane first (the
    layers whole, the one it ended in and those left out make every layer)
    with something lit, and the fit is remembered per scale as by size: the
    middle decides over its margin, the margin and the middle again apply
    it, both middles equal to the margin's centre."""
    pair = Path(temp) / 'pair.oas'
    over = Path(temp) / 'over.oas'
    layout_pair(pair)
    layout_over(over)
    for path in (pair, over):
        done = subprocess.run([FLOE2, 'index', str(path), '--jobs', '2'],
                              cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
        assert done.returncode == 0, done.stdout + done.stderr
    from floe_oracle.perf_line import perf_status
    low, up = (7, 59), (14, 367)
    top, old = top_first_worker(pair, 1), top_first_worker(pair, 1, on=False)
    try:
        alone = {lay: pair_frame(top, g, (400, 400), 400, [lay])[0] for g, lay in ((1, low), (2, up))}
        background = alone[low][0]
        colours = {lay: {c for c in px if c != background} for lay, px in alone.items()}
        assert all(colours.values()) and not (colours[low] & colours[up]), 'each layer draws in a colour of its own'

        def lit(px):
            return {lay: sum(1 for c in px if c in colours[lay]) for lay in (low, up)}
        whole = lit(alone[low])[low], lit(alone[up])[up]
        both, res = pair_frame(top, 3, (400, 400), 400, [low, up])
        culls = res['plan_culls']
        assert lit(both) == {low: 0, up: whole[1]}, ('top first: 14/367 as alone, 7/59 left out', lit(both), whole)
        account = (culls['fit_ranked'], culls['fit_layers_whole'], culls['fit_layer_edge'], culls['fit_layers_out'])
        assert account == (1, 1, None, 1), culls
        bar = perf_status(res)[1]
        assert 'top 1 whole, 1 left out to fit budget' in bar, bar
        after, res2 = pair_frame(top, 4, (400, 400), 400, [low, up])
        assert after == both and res2['plan_culls']['fit_fixed'] == 1 and res2['plan_culls']['fit_redecided'] == 0, res2['plan_culls']
        by_size, ros = pair_frame(old, 1, (400, 400), 400, [low, up])
        assert lit(by_size) == {low: whole[0], up: 0} and ros['plan_culls']['fit_ranked'] == 0, (lit(by_size), ros['plan_culls'])
        print('fit budget: top plane first - 7/59 and 14/367 under 1 MB: 14/367 as alone (%d px), 7/59 left out (`%s`), '
              'the frame after the same; by size (FLOE_RUST_FIT_TOP_FIRST=off) 7/59 as alone (%d px), 14/367 none'
              % (whole[1], bar.split('cut<', 1)[-1].split(' ', 1)[-1], whole[0]))
    finally:
        top.stop()
        old.stop()
    roomy = top_first_worker(over, 1024)
    try:
        lower, _ = pair_frame(roomy, 1, (200, 200), 420, [low])
        upper, _ = pair_frame(roomy, 2, (200, 200), 420, [up])
        both, _ = pair_frame(roomy, 3, (200, 200), 420, [low, up])
        background = lower[0]
        low_colours = {c for c in lower if c != background}
        up_colours = {c for c in upper if c != background}
        under = [i for i, c in enumerate(lower) if c != background]
        shown = sum(1 for i in under if both[i] in low_colours)
        covered = sum(1 for i in under if both[i] in up_colours)
        assert 0 < shown < len(under) and covered > 0, ('7/59 in the holes of 14/367', shown, covered, len(under))
        print('fit budget: pass 1 draws the lower shapes in the upper speckle\'s holes - of 7/59\'s %d px under 14/367, %d show'
              % (len(under), shown))
    finally:
        roomy.stop()
    sticky = top_first_worker(src, 48)
    try:
        x0, y0, x1, y1 = map(float, sticky.cache.meta['bbox'])
        wide = (x0, y0, x1, y1)
        layers = len(sticky.cache.meta['layers'])
        fitted, res = frame(sticky, 1, wide)
        culls = res['plan_culls']
        edge = 1 if culls['fit_layer_edge'] else 0
        assert culls['fit_ranked'] == 1 and culls['fit_over'] == 0 and \
            culls['fit_layers_whole'] + edge + culls['fit_layers_out'] == layers, culls
        assert any(bytes(fitted[i:i + 4]) != bytes(fitted[:4]) for i in range(0, len(fitted), 4)), 'the top-first frame is empty'
        mid = (x0 + (x1 - x0) / 4, y0 + (y1 - y0) / 4, x0 + 3 * (x1 - x0) / 4, y0 + 3 * (y1 - y0) / 4)
        first, rf = frame(sticky, 2, mid)
        margin, rm = frame(sticky, 3, wide, size=2 * PX)
        again, ra = frame(sticky, 4, mid)
        fits = (rf['plan_culls'], rm['plan_culls'], ra['plan_culls'])
        assert fits[0]['fit_ranked'] == 1, 'the middle must need the fit: %s' % (fits[0],)
        assert all(f['fit_fixed'] == 1 and f['fit_redecided'] == 0 for f in fits), fits
        q = PX // 2
        centre = b''.join(bytes(margin[((q + r) * 2 * PX + q) * 4:((q + r) * 2 * PX + q + PX) * 4]) for r in range(PX))
        assert bytes(first) == centre and bytes(again) == centre, 'top first: the middle and the margin\'s centre differ'
        print('fit budget: top plane first over every layer of the chip under 48 MB - %d whole, %s, %d left out; '
              'the middle decided over its margin, the margin and the middle again apply it (= margin centre)'
              % (culls['fit_layers_whole'], culls['fit_layer_edge'] or 'no layer thinned', culls['fit_layers_out']))
    finally:
        sticky.stop()


def refit_checks(temp):
    """A frame the planner fitted to the budget holds it once decoded - its
    pages' record lists are cut to their length and a list they share is
    charged once (render-core decode_shrink, DecodedPage::estimated_bytes) -
    and one whose pages pass it all the same is planned anew under less of
    it, not failed (renderd budget_refit_enabled)."""
    lit = lambda pixels: sum(1 for i in range(0, len(pixels), 4) if pixels[i:i + 3] != b'\x00\x00\x00')

    def index(src):
        done = subprocess.run([FLOE2, 'index', str(src)],
                              cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
        assert done.returncode == 0, done.stdout + done.stderr

    start = worker_with

    # the field's case: plain boxes whose pages, as the parser reads them, are
    # charged past their estimate by the room their record lists keep. 400 um
    # about the centre at 800 px (boxes of 1.2 to 2 px) reads four pages:
    # 25.7 MB estimated, 28.9 MB as read - under 26 MB the plan fits and its
    # pages as read do not
    plain = Path(temp) / 'plain.oas'
    layout_plain(plain)
    index(plain)
    budget_mb = 26
    budget = budget_mb << 20
    view = (1_300_000, 1_300_000, 1_700_000, 1_700_000)
    # the lists as the parser grew them: 0.12.300's charge
    as_read = {'FLOE_RUST_DECODE_SHRINK': 'off'}
    as_300 = dict(as_read, FLOE_RUST_BUDGET_REFIT='off')
    now, read, old = start(plain, {}, budget_mb), start(plain, as_read, budget_mb), start(plain, as_300, budget_mb)
    try:
        # as 0.12.300: the error (were it not, the layout's pages no longer
        # pass their estimate and the checks below would check nothing)
        failed, re_ = frame(old, 1, view, size=800)
        assert failed is None and 'decoded generation budget exceeded' in str(re_.get('msg')), \
            ('the plain boxes no longer reproduce the field error with the lists as read and FLOE_RUST_BUDGET_REFIT=off',
             re_.get('msg'), re_.get('plan_culls'))
        # now: the lists cut to their length, the fitted plan holds whole
        whole, rw = frame(now, 1, view, size=800)
        assert whole is not None, rw
        fit = rw['plan_culls']
        assert fit['fit_refits'] == 0 and fit['fit_scale'] == 0 and fit['fit_over'] == 0 and lit(whole) > 0, fit
        assert not rw.get('over_budget_pages') and rw['resident_mb'] * (1 << 20) <= budget, (rw.get('over_budget_pages'), rw['resident_mb'])
        # the lists as read (FLOE_RUST_DECODE_SHRINK=off): planned anew once,
        # drawn within the budget; the layers' next frames start there
        first, r1 = frame(read, 1, view, size=800)
        assert first is not None, r1
        culls = r1['plan_culls']
        assert culls['fit_refits'] >= 1 and culls['fit_scale'] > 1000 and culls['fit_over'] == 0 and lit(first) > 0, culls
        assert not r1.get('over_budget_pages') and r1['resident_mb'] * (1 << 20) <= budget, (r1.get('over_budget_pages'), r1['resident_mb'])
        again, r2 = frame(read, 2, view, size=800)
        assert again is not None and bytes(again) == bytes(first), r2
        assert r2['plan_culls']['fit_refits'] == 0 and r2['plan_culls']['fit_scale'] == culls['fit_scale'], r2['plan_culls']
        # (the plan anew gives pages up; the cut lists hold the first plan)
        assert rw['tiles'] > r1['tiles'] and lit(whole) > lit(first) and rw['resident_mb'] < r1['resident_mb'], \
            (rw['tiles'], r1['tiles'], lit(whole), lit(first), rw['resident_mb'], r1['resident_mb'])
        # the scale is the plain frames': with the density stack pass 1 plans
        # to the budget less pass 2's reserve, which holds what its pages
        # take, and is not planned under less for it
        stacked, rs = frame(read, 3, view, size=800, density=True)
        assert stacked is not None, rs
        assert rs['plan_culls']['fit_refits'] == 0 and rs['plan_culls']['fit_scale'] == 0, rs['plan_culls']
        back, r3 = frame(read, 4, view, size=800, density=False)
        assert back is not None and bytes(back) == bytes(first) and r3['plan_culls']['fit_scale'] == culls['fit_scale'], r3['plan_culls']
        # the density stack counts its pages as read (renderd
        # density_as_read): its frame is the same with the lists cut
        dense, rn = frame(now, 2, view, size=800, density=True)
        assert dense is not None and bytes(dense) == bytes(stacked), (rn.get('density_pages'), rs.get('density_pages'))
        print('fit budget: a fitted frame holds the budget once decoded - plain boxes under %d MB: the lists as read and '
              'FLOE_RUST_BUDGET_REFIT=off (0.12.300): %s; now the plan drawn whole (%d pages, %.1f MB, %d px lit, no plan anew); '
              'the lists as read (FLOE_RUST_DECODE_SHRINK=off): planned anew %d time(s) at 1/%.3f of the budget and drawn (%d pages, '
              '%.1f MB, %d px lit), the next frame starts there, a frame with the density stack is not planned under less (fit_scale %d) '
              'and is the same picture with the lists cut'
              % (budget_mb, re_.get('msg'), rw['tiles'], rw['resident_mb'], lit(whole), culls['fit_refits'], culls['fit_scale'] / 1000.0,
                 r1['tiles'], r1['resident_mb'], lit(first), rs['plan_culls']['fit_scale']))
    finally:
        for w in (now, read, old):
            w.stop()

    # the viewer's margin (bg: the viewport's scale, twice its pixels a
    # side). A viewport of 200 um inside one page, its margin over the four:
    # the viewport's pages hold, the margin's as read pass the budget
    now, read, old = start(plain, {}, budget_mb), start(plain, as_read, budget_mb), start(plain, as_300, budget_mb)
    try:
        port, margin = (1_550_000, 1_550_000, 1_750_000, 1_750_000), (1_450_000, 1_450_000, 1_850_000, 1_850_000)
        # as 0.12.300: the viewport draws and its margin is the error the
        # status line showed
        shown, ro = frame(old, 1, port, size=800)
        assert shown is not None and lit(shown) > 0, ro
        failed, re_ = frame(old, 2, margin, size=1600, bg=True)
        assert failed is None and re_.get('kind') == 'error' and 'decoded generation budget exceeded' in str(re_.get('msg')), \
            ('the margin of the plain boxes no longer reproduces the field error with the lists as read and FLOE_RUST_BUDGET_REFIT=off', re_)
        # the lists as read: the margin is dropped - its viewport is on the
        # screen as it was planned, and a margin planned under less would
        # change the picture when it lands - the layers' scale is raised and
        # their fits forgotten; the next viewport frame decides anew over its
        # margin's extent, and no margin after it is an error
        first, r1 = frame(read, 1, port, size=800)
        assert first is not None and bytes(first) == bytes(shown) and r1['plan_culls']['fit_scale'] == 0, r1['plan_culls']
        gone, rd = frame(read, 2, margin, size=1600, bg=True)
        assert gone is None and rd.get('kind') == 'dropped' and rd.get('reason') == 'budget', rd
        again, r2 = frame(read, 3, port, size=800)
        assert again is not None, r2
        assert r2['plan_culls']['fit_scale'] > 1000 and r2['plan_culls']['fit_refits'] == 0, r2['plan_culls']
        # (the viewport's own pages hold whole under any decision: the same pixels)
        assert bytes(again) == bytes(first), r2['plan_culls']
        later, rl = frame(read, 4, margin, size=1600, bg=True)
        assert rl.get('kind') in ('frame', 'dropped'), rl
        assert later is not None or rl.get('reason') == 'fit', rl
        # now: the margin's pages hold too
        port_now, rp = frame(now, 1, port, size=800)
        assert port_now is not None and bytes(port_now) == bytes(shown), rp
        margin_now, rm = frame(now, 2, margin, size=1600, bg=True)
        assert margin_now is not None and rm['plan_culls']['fit_scale'] == 0 and rm['plan_culls']['fit_refits'] == 0, rm
        print('fit budget: a margin whose pages pass the budget is dropped, not an error - the lists as read and FLOE_RUST_BUDGET_REFIT=off '
              '(0.12.300): %s; now the margin drawn (%d pages, %.1f MB); the lists as read: dropped (reason %s), the next viewport frame '
              'planned at 1/%.3f of the budget with the same pixels, the margin after it: %s'
              % (re_.get('msg'), rm['tiles'], rm['resident_mb'], rd.get('reason'), r2['plan_culls']['fit_scale'] / 1000.0,
                 'drawn' if later is not None else 'dropped (reason %s: it would thin where the viewport is whole)' % rl.get('reason')))
    finally:
        for w in (now, read, old):
            w.stop()

    # a list the records share: forty box sizes at the same 30,000 places
    src = Path(temp) / 'shared.oas'
    layout_shared(src)
    index(src)
    # 0.12.300's charge: the list once a record, the record lists as read
    as_before = {'FLOE_RUST_CHARGE_SHARED': 'off', 'FLOE_RUST_DECODE_SHRINK': 'off'}
    now, refit, old = start(src, {}, 4), start(src, as_before, 4), start(src, dict(as_before, FLOE_RUST_BUDGET_REFIT='off'), 4)
    small = start(src, as_before, 1)
    try:
        # 126 um about the centre at 800 px: boxes of 0.6 to 3 px, 8 pages of them
        view = (1_437_000, 1_437_000, 1_563_000, 1_563_000)
        budget = 4 << 20
        # the charge as it is: the fitted frame holds
        held, rh = frame(now, 1, view, size=800)
        assert held is not None, rh
        assert rh['plan_culls']['fit_refits'] == 0 and rh['plan_culls']['fit_scale'] == 0 and lit(held) > 0, rh['plan_culls']
        assert rh['resident_mb'] * (1 << 20) <= budget, rh['resident_mb']
        # the charge as it was: planned anew, drawn; the layers' next frame starts there
        first, r1 = frame(refit, 1, view, size=800)
        assert first is not None, r1
        culls = r1['plan_culls']
        assert culls['fit_refits'] >= 1 and culls['fit_scale'] > 1000 and lit(first) > 0, culls
        assert r1['resident_mb'] * (1 << 20) <= budget, r1['resident_mb']
        again, r2 = frame(refit, 2, view, size=800)
        assert again is not None and bytes(again) == bytes(first), r2
        assert r2['plan_culls']['fit_refits'] == 0 and r2['plan_culls']['fit_scale'] == culls['fit_scale'], r2['plan_culls']
        # the kill switch: the error
        failed, re_ = frame(old, 1, view, size=800)
        assert failed is None and 'decoded generation budget exceeded' in str(re_.get('msg')), re_
        # a budget under one page: no plan holds - the frame is what the budget
        # holds, its pages over the budget counted, not an error
        bare, rb = frame(small, 1, view, size=800)
        assert bare is not None, rb
        assert rb.get('over_budget_pages', 0) >= 1 and rb['plan_culls']['fit_over'] == 1 and rb['plan_culls']['fit_refits'] <= 1, (rb.get('over_budget_pages'), rb['plan_culls'])
        print('fit budget: a frame whose pages pass the budget is planned anew, not failed - records on shared repetition lists under 4 MB: '
              'as charged now the fitted frame holds (%.1f MB, %d px lit); as charged before it is planned anew %d time(s) at 1/%.2f of the '
              'budget (%.1f MB, %d px lit) and the next frame starts there; FLOE_RUST_BUDGET_REFIT=off: %s; under 1 MB (less than a page) '
              'the frame draws what the budget holds, %d pages over it'
              % (rh['resident_mb'], lit(held), culls['fit_refits'], culls['fit_scale'] / 1000.0, r1['resident_mb'], lit(first), re_.get('msg'),
                 rb.get('over_budget_pages', 0)))
    finally:
        for w in (now, refit, old, small):
            w.stop()


def probe_checks(src):
    """The fit of a new scale is decided by a plan made for that alone,
    which goes by the hierarchy summary where the walk read every placement
    of the extent (render-core Cache::fit_decision_cancellable, floe_vfs
    HierOpts::decide_by): the same decision - the same pages, fit and pixels
    in every first frame at a scale - and its time a phase of its own."""
    by_summary, walked = worker_with(src, {}, 48), worker_with(src, {'FLOE_RUST_FIT_PROBE_SUMMARY': 'off'}, 48)
    try:
        x0, y0, x1, y1 = map(float, by_summary.cache.meta['bbox'])
        w, h = x1 - x0, y1 - y0
        views = []
        # the whole chip, then windows of a half, a quarter, an eighth and a
        # sixteenth of it about five places - each a scale of its own
        for zoom in (1, 2, 4, 8, 16):
            for at, (fx, fy) in enumerate(((0.5, 0.5), (0.25, 0.3), (0.7, 0.2), (0.3, 0.75), (0.9, 0.9))):
                if zoom == 1 and at:
                    continue
                side = (1.0 + 1e-4 * at) / zoom
                cx, cy = x0 + w * fx, y0 + h * fy
                views.append((cx - w * side / 2, cy - h * side / 2, cx + w * side / 2, cy + h * side / 2))
        fit_of = lambda res: tuple(res['plan_culls'][key] for key in ('fit_pct', 'fit_thin', 'fit_full_pct', 'fit_none_pct', 'fit_fixed', 'fit_redecided', 'fit_over'))
        thinned, times = 0, [0.0, 0.0]
        for gen, view in enumerate(views, 1):
            a, ra = frame(by_summary, gen, view, size=800)
            b, rb = frame(walked, gen, view, size=800)
            assert a is not None and b is not None, (view, ra, rb)
            assert bytes(a) == bytes(b), 'the summary\'s decision draws another picture than the walk\'s at %r' % (view,)
            assert (ra['tiles'], fit_of(ra)) == (rb['tiles'], fit_of(rb)), (view, ra['tiles'], fit_of(ra), rb['tiles'], fit_of(rb))
            # each the first frame at its scale: decided by a probe, the
            # summary's or the walk's, its time reported
            assert ra['fit_probe_ms'] > 0 and rb['fit_probe_ms'] > 0, (ra['fit_probe_ms'], rb['fit_probe_ms'])
            assert ra['fit_probe_walk'] is False and rb['fit_probe_walk'] is True, (ra['fit_probe_walk'], rb['fit_probe_walk'])
            thinned += ra['plan_culls']['fit_thin'] > 0
            times[0] += ra['fit_probe_ms']
            times[1] += rb['fit_probe_ms']
        assert thinned >= 3, 'the views must need the fit: %d of %d thinned' % (thinned, len(views))
        # a scale decided before is not probed again
        _, rr = frame(by_summary, len(views) + 1, views[1], size=800)
        assert rr['fit_probe_ms'] == 0 and rr['plan_culls']['fit_fixed'] == 1, (rr['fit_probe_ms'], rr['plan_culls'])
        print('fit budget: a new scale\'s fit is decided by the hierarchy summary as by the walk - %d first frames (%d thinned) the same pages, fit '
              'and pixels; the probes %.0f ms by the summary, %.0f ms walked; a scale decided before is not probed'
              % (len(views), thinned, times[0], times[1]))
    finally:
        by_summary.stop()
        walked.stop()


def main():
    os.environ['FLOE_INDEX_BIN'] = str(ROOT / 'rust/target/release/floe-index')
    os.environ['FLOE_RENDERD_BIN'] = str(ROOT / 'rust/target/release/floe-renderd')
    os.environ['FLOE_RUST_RETAINED_MB'] = '0'
    # pass 1's fit by size alone, as these checks were made (the top plane
    # first, the default since 0.12.318: top_first_checks)
    os.environ['FLOE_RUST_FIT_TOP_FIRST'] = 'off'
    with tempfile.TemporaryDirectory(prefix='floe-fit-') as temp:
        src = Path(temp) / 'chip.oas'
        for argv in ([sys.executable, '-B', str(ROOT / 'tools/gen_main01_like.py'), str(src),
                      '--scale', '0.003', '--jobs', '2', '--geometry', 'legacy'],
                     [FLOE2, 'index', str(src), '--jobs', '2']):
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
            done = subprocess.run([FLOE2, 'index', str(uneven), '--jobs', '2'],
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
        refit_checks(temp)
        probe_checks(src)
        top_first_checks(temp, src)
    print('FIT BUDGET: ALL OK')


if __name__ == '__main__':
    main()
