#!/usr/bin/env python3
"""Density stack gate (floe_render_core::GeometryRasterRequest::density_stack,
CUT_DENSITY_DESIGN §10.10; user direction 2026-09-27: the shapes under the
cut are the density - hairlines have their own drawing - drawn in a second
pass into the space the originals left, the top layer first).

With the diagnostic FLOE_RUST_DENSITY_STACK=top the frame is drawn twice:
pass 1 paints the plan as always (max mode: every shape whose larger side
reaches the cut) and records what its originals cover - speckle holes
included; pass 2 plans the same view at a finer cut (1 px, the larger
side) and reads its pages only where pass 1 left a pixel: the top layer's
shapes under the cut draw over the lower layers' originals but not inside
its own, every other layer's only where no original covers the pixel and no
density above stands for it.

One layout written with klayout.db, 0.1 um a pixel, 400 x 200 px: layer 1/0
(the lowest) is a field of 0.15 um (1.5 px) squares 0.5 um apart over the
whole view - pages the cut drops whole, so pass 2 decodes them; layer 2/0 a
rectangle over the top right quadrant; layer 4/0 (the top one) a rectangle
over the left half, 1.5 px squares inside it and 1.5 px squares over layer
2's rectangle; layer 3/0 the left rectangle alone (the reference, never drawn
with the others). Every layer takes the viewer's default speckle; the cut is
3 px (detail medium).

  * without the variable no square draws (the cut); with it the bottom right
    quadrant (nothing above) holds layer 1's squares pixel for pixel as a
    cut-free frame of layer 1 alone draws them there;
  * inside the left rectangle exactly the rectangle alone lights - no square
    of layer 1 in its holes, none of the top layer's own either;
  * in the top right quadrant the top layer's squares light over layer 2's
    rectangle in the top layer's colour, exactly the pixels a cut-free frame
    of the top layer alone lights there, and everything else there is
    layer 2's rectangle as without the variable;
  * pass 2 draws the VISIBLE layers only (field 2026-09-27: with one layer
    on, the other layers' density showed): with layer 1/0 alone on, the frame
    is byte-identical to the cut-free frame of layer 1/0 (its squares are the
    top plane's density, nothing above); with 1/0 and 2/0 on, the top right
    quadrant is 2/0's rectangle alone - none of 4/0's squares;
  * the viewer's margin frame (bg, twice the extent per axis) draws the view
    pixel for pixel as the viewport frame did: pass 2 plans to a reserve of
    its own and its budget fit is remembered per scale and side (2026-09-27:
    planned to half of what the generation had left, a margin's pass 2 on
    the synthetic chip's fit view decoded 23 pages where the viewport's
    decoded 2,208 and 8,110 px changed when it landed);
  * the frame reports the stack's counts (density_stack: lit, top, lower,
    covered, claimed) and pass 2's pages (density_pages: planned, in_hand,
    decoded, over_budget - some decoded); none without the variable, under
    FLOE_RUST_AREA_TRUE=off or FLOE_RUST_WRITE_ONCE=off, which draw as
    without the variable.

The sub-cut dots (FLOE_RUST_DENSITY_DOTS=on with the stack, CUT_DENSITY_DESIGN
§10.12; user 2026-09-30: "a cell of 3 x 3 px or less is one dot, no
descent"): a second layout places a cell DOT (a 0.15 um = 1.5 px square on
1/0) three ways - a 10 x 10 array at a 6 px pitch, a 40 x 40 array that
abuts, and one alone. Pass 2 plans the cells at pass 1's cut and counts a
cell under it as dots in blocks, never walking into it:

  * by default (4 x 4 px blocks, spread; 8 px for a day - field 2026-10-01:
    past 4 px dots landed where nothing is) a block's dots are what it holds
    - an array its members, not its box - spread over what they stand for
    within the block: the sparse array lights exactly 100 pixels, each within
    1.5 px of its member's centre; the lone DOT one; the abutting array per
    4 x 4 block min(8, the members whose centre lies in it) - summed here
    from the member positions - and nothing else lights; the frame reports
    the block (density_block 4);
  * under the rules before (FLOE_RUST_DENSITY_SPREAD=off
    FLOE_RUST_DENSITY_FLOOR_PX=0 FLOE_RUST_DENSITY_PAGE_DOTS=on: a compact box
    of the count's area, the zero floor's probe and the fit, the page dots)
    the sparse array lights 100 pixels, each within 2 px of a member's
    centre, the lone DOT one, the abutting array per 4 x 4 block min(8, the
    members centred in it);
  * density_dots reports the items (one per block and layer) and none over
    the cap; without the variable the stack walks into DOT and reports no
    dots;
  * the margin frame draws the view as the viewport frame did;
  * the floor (1 px since 2026-10-01; 0 px before): TOP's own 0.05 um
    (0.5 px) squares at a 3 px pitch (one page of 200) are under it - not
    drawn and not dotted (FLOE_RUST_DENSITY_PAGE_DOTS off: page dots were far
    denser than their shapes) - and pass 2 fits at the density cut with no
    probe (density_plan2: no probe, one pass); FLOE_RUST_DENSITY_FLOOR_PX
    lowers it - 0 and 0.25 px decode the squares and draw them as a cut-free
    frame does (a zero floor's probe fits), 0.6 px leaves them out - and the
    frame reports the floor it planned at (density_floor 1 / 0 / 0.25 / 0.59);
  * the one walk (FLOE_RUST_DENSITY_ONE_WALK=on; the default for a day,
    opt-in since 0.12.261): pass 2 plans once (no probe, one pass), its pages
    at the cells' cut; the squares' page, all under the cut, is not drawn -
    with the page dots at a zero floor it stands as exactly ceil(200 x 0.25)
    = 50 dots, what the squares' area lights cut-free;
  * pass 2's regions planned apart on two threads and merged
    (FLOE_RUST_DENSITY_PLAN_THREADS=2, density_plan2 threads 2) draw the
    frame one plan draws;
  * step 3 (progressive): the dots' frame arrives twice - first a refining
    round (final=0) holding pass 1 alone, byte for byte the frame without the
    stack, then the final frame, byte for byte what the dots draw in one
    round (FLOE_RUST_DENSITY_PROGRESSIVE=off); a margin frame has no first
    round;
  * a zoom while pass 2 is drawing (field 2026-09-30: the viewer looked hung
    - the next view waited for the old plan): the next generation, submitted
    on the first round, is answered - its final frame equals its own one-round
    frame - and the old generation publishes no final frame after that.

Pass 2's budget decision is the frame's own (user 2026-10-01: kept per scale,
not per place, a dense view's decision emptied a sparse view at the same zoom
step): on an uneven layout under a 1 MB pass-2 reserve the whole extent thins
its pass 2 and the corner quarter, drawn after it at the same scale, equals
the corner a fresh worker draws (history_checks). Pass 1's pages cost pass 2's
reserve nothing: under a 1 MB reserve, which pass 1's own pages pass, the frame
equals the default reserve's and reports nothing over budget (held_checks).

    .venv/bin/python tools/validate_density_stack.py
"""
import os
from pathlib import Path
import math
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from floe.cache import Cache
from floe.rust_render import RustRenderWorker

W, H = 400, 200
PX_UM = 0.1
LOW, MID, ALONE, TOP = (1, 0), (2, 0), (3, 0), (4, 0)
BLACK = bytes((0, 0, 0, 255))
VIEW = (0.0, 0.0, W * PX_UM, H * PX_UM)
SQUARE = 0.15               # um: 1.5 px, under the 3 px cut, over pass 2's 1 px


def layout(path):
    import klayout.db as kdb
    ly = kdb.Layout()
    ly.dbu = 0.001
    top = ly.create_cell('TOP')
    low = ly.layer(*LOW)
    for j in range(H // 5):
        for i in range(W // 5):
            x, y = i * 0.5 + 0.17, j * 0.5 + 0.13
            top.shapes(low).insert(kdb.DBox(x, y, x + SQUARE, y + SQUARE))
    top.shapes(ly.layer(*MID)).insert(kdb.DBox(20.0, 10.0, 40.0, 20.0))
    for layer in (TOP, ALONE):
        top.shapes(ly.layer(*layer)).insert(kdb.DBox(0.0, 0.0, 20.0, 20.0))
    dots = ly.layer(*TOP)
    for j in range(20):
        for i in range(20):
            x, y = 2.0 + i * 0.7, 2.0 + j * 0.7
            top.shapes(dots).insert(kdb.DBox(x, y, x + SQUARE, y + SQUARE))
            x, y = 22.0 + i * 0.8, 11.0 + j * 0.4
            top.shapes(dots).insert(kdb.DBox(x, y, x + SQUARE, y + SQUARE))
    ly.write(str(path))


DOT = 0.15                  # um: the DOT cell's square, 1.5 px
SPARSE = (2030, 2030, 600, 10)      # dbu origin x, y, pitch, n: 6 px apart
DENSE = (20000, 4000, 150, 40)      # abutting: the pitch is the square
ALONE_DOT = (12030, 15030)
TINY = (30.03, 12.03, 20, 10)       # um origin x, y, columns, rows: cols 300-360, rows 50-80


def dots_layout(path):
    import klayout.db as kdb
    ly = kdb.Layout()
    ly.dbu = 0.001
    top = ly.create_cell('TOP')
    dot = ly.create_cell('DOT')
    dot.shapes(ly.layer(*LOW)).insert(kdb.DBox(0.0, 0.0, DOT, DOT))
    for (x, y, pitch, n) in (SPARSE, DENSE):
        top.insert(kdb.CellInstArray(dot.cell_index(), kdb.Trans(kdb.Vector(x, y)), kdb.Vector(pitch, 0), kdb.Vector(0, pitch), n, n))
    top.insert(kdb.CellInstArray(dot.cell_index(), kdb.Trans(kdb.Vector(*ALONE_DOT))))
    # TOP's own specks: 0.05 um squares 0.3 um apart, under a 1 px floor
    low = ly.layer(*LOW)
    for j in range(TINY[3]):
        for i in range(TINY[2]):
            x, y = TINY[0] + i * 0.3, TINY[1] + j * 0.3
            top.shapes(low).insert(kdb.DBox(x, y, x + 0.05, y + 0.05))
    # an original for pass 1 to draw (2/0, away from the rest)
    top.shapes(ly.layer(*MID)).insert(kdb.DBox(36.0, 1.0, 39.0, 5.0))
    ly.write(str(path))


def dots_checks(temp):
    src = Path(temp) / 'dots.oas'
    dots_layout(src)
    done = subprocess.run([sys.executable, '-B', '-m', 'floe2', 'index', str(src)],
                          cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
    assert done.returncode == 0, done.stdout + done.stderr
    workers = {
        'off': worker(src, {}),
        'stack': worker(src, {'FLOE_RUST_DENSITY_STACK': 'top'}),
        'dots': worker(src, {'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on'}),
        # the rules before 2026-10-01: 4 px blocks, a count's compact box, the
        # floor probe at 0 px and the budget fit, the page dots
        'dots4': worker(src, {'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on',
                              'FLOE_RUST_DENSITY_BLOCK_PX': '4', 'FLOE_RUST_DENSITY_SPREAD': 'off',
                              'FLOE_RUST_DENSITY_FLOOR_PX': '0', 'FLOE_RUST_DENSITY_PAGE_DOTS': 'on'}),
        'one_round': worker(src, {'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on', 'FLOE_RUST_DENSITY_PROGRESSIVE': 'off'}),
        'floor0': worker(src, {'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on', 'FLOE_RUST_DENSITY_FLOOR_PX': '0'}),
        'floor025': worker(src, {'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on', 'FLOE_RUST_DENSITY_FLOOR_PX': '0.25'}),
        'floor06': worker(src, {'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on', 'FLOE_RUST_DENSITY_FLOOR_PX': '0.6'}),
        # pass 2's regions planned apart on two threads and merged
        'split': worker(src, {'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on', 'FLOE_RUST_DENSITY_PLAN_THREADS': '2'}),
        # the one walk (FLOE_RUST_DENSITY_ONE_WALK=on, opt-in), and with the
        # page dots (FLOE_RUST_DENSITY_PAGE_DOTS=on) at a zero floor
        'one': worker(src, {'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on', 'FLOE_RUST_DENSITY_ONE_WALK': 'on'}),
        'one_dots': worker(src, {'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on', 'FLOE_RUST_DENSITY_ONE_WALK': 'on',
                                 'FLOE_RUST_DENSITY_PAGE_DOTS': 'on', 'FLOE_RUST_DENSITY_FLOOR_PX': '0'}),
    }
    try:
        on4, res4 = frame(workers['dots4'], 1, (LOW,))
        on, res = frame(workers['dots'], 1, (LOW,))
        walked, walked_res = frame(workers['stack'], 1, (LOW,))
        half = int(round(DOT * 1000)) // 2
        # the device position of a dbu point (0.1 um a pixel, rows from the top)
        dev = lambda x, y: (x / 100.0, (VIEW[3] * 1000 - y) / 100.0)
        centres_of = lambda x0, y0, pitch, n: [(x0 + half + i * pitch, y0 + half + j * pitch) for i in range(n) for j in range(n)]
        def blocks_of(centres, side):
            # the members whose centre lies in each block of `side` dbu
            blocks = {}
            for (x, y) in centres:
                blocks[(x // side, y // side)] = blocks.get((x // side, y // side), 0) + 1
            return blocks
        sparse_centres, dense_centres = centres_of(*SPARSE), centres_of(*DENSE)
        sparse_area, alone_area, dense_area = (range(10, 90), range(110, 195)), (range(110, 135), range(35, 65)), (range(195, 265), range(95, 165))
        specks = (range(295, 365), range(45, 85))
        # the rules before 2026-10-01 (FLOE_RUST_DENSITY_BLOCK_PX=4
        # FLOE_RUST_DENSITY_SPREAD=off): the sparse array one dot a member,
        # beside its centre; the abutting array min(8, the members centred in
        # it) a 4 x 4 block
        sparse4 = lit(on4, *sparse_area)
        assert len(sparse4) == SPARSE[3] ** 2, 'sparse array (4 px): %d px lit, want %d' % (len(sparse4), SPARSE[3] ** 2)
        for (c, r) in sparse4:
            assert min(abs(c + 0.5 - cx) + abs(r + 0.5 - cy) for (cx, cy) in (dev(*p) for p in sparse_centres)) <= 2.0, 'sparse dot (%d, %d) far from a member' % (c, r)
        assert len(lit(on4, *alone_area)) == 1, 'the lone DOT (4 px): %d px' % len(lit(on4, *alone_area))
        blocks4 = blocks_of(dense_centres, 400)
        want4 = sum(min(8, count) for count in blocks4.values())
        dense4 = lit(on4, *dense_area)
        assert len(dense4) == want4, 'abutting array (4 px): %d px lit, want %d over %d blocks' % (len(dense4), want4, len(blocks4))
        assert lit(on4, range(W), range(H)) == sparse4 | lit(on4, *alone_area) | dense4 | lit(on4, *specks), 'the 4 px rules light outside the arrays'
        dots4 = res4.get('density_dots')
        assert dots4 and dots4['items'] == SPARSE[3] ** 2 + 1 + len(blocks4) and dots4['over'] == 0, (dots4, len(blocks4))
        assert res4.get('density_block') == 4.0, res4.get('density_block')
        print('density stack dots (4 px, compact - the rules before): sparse array %d dots beside the members, lone DOT 1, abutting array '
              '%d px = sum of min(8, members) over %d blocks; density_dots %s' % (len(sparse4), len(dense4), len(blocks4), dots4))
        # the default (4 px blocks, spread - 8 px for a day, field 2026-10-01:
        # past 4 px dots landed where nothing is): a block's dots are its
        # members' count - the array's members, not its box - spread over what
        # they stand for within the block; the abutting array min(8, the
        # members centred in it) a 4 x 4 block
        sparse = lit(on, *sparse_area)
        assert len(sparse) == SPARSE[3] ** 2, 'sparse array: %d px lit, want %d' % (len(sparse), SPARSE[3] ** 2)
        for (c, r) in sparse:
            assert min(max(abs(c + 0.5 - cx), abs(r + 0.5 - cy)) for (cx, cy) in (dev(*p) for p in sparse_centres)) <= 1.5, 'sparse dot (%d, %d) off its member' % (c, r)
        alone_px = lit(on, *alone_area)
        assert len(alone_px) == 1, 'the lone DOT: %d px' % len(alone_px)
        blocks = blocks_of(dense_centres, 400)
        want = sum(min(8, count) for count in blocks.values())
        dense = lit(on, *dense_area)
        assert len(dense) == want, 'abutting array: %d px lit, want %d over %d blocks' % (len(dense), want, len(blocks))
        # the floor (step 2; 1 px since 2026-10-01, user "what about fixing the
        # floor at 1 px"): TOP's 0.5 px specks are under it - not drawn, not
        # dotted (FLOE_RUST_DENSITY_PAGE_DOTS off) - and no probe is made (one
        # fitted pass at the density cut); FLOE_RUST_DENSITY_FLOOR_PX lowers
        # it: 0 and 0.25 px draw the specks as a cut-free frame does, 0.6 px
        # does not; the frame says which floor it took
        free, _ = frame(workers['stack'], 3, (LOW,), cut_px=0.0)
        tiny = lit(on, *specks)
        assert not tiny, "TOP's specks under the 1 px floor: %d px" % len(tiny)
        assert not lit(walked, *specks), "the stack's 1 px floor draws no speck"
        at0, res0 = frame(workers['floor0'], 3, (LOW,))
        at025, res025 = frame(workers['floor025'], 3, (LOW,))
        at06, res06 = frame(workers['floor06'], 3, (LOW,))
        drawn = lit(at0, *specks)
        assert drawn and drawn == lit(free, *specks) == lit(at025, *specks) and not lit(at06, *specks), \
            'floors 0 / 0.25 / 0.6: %d / %d / %d speck px, %d cut-free' % (len(drawn), len(lit(at025, *specks)), len(lit(at06, *specks)), len(lit(free, *specks)))
        floors = (res.get('density_floor'), res0.get('density_floor'), res025.get('density_floor'), res06.get('density_floor'))
        assert abs(floors[0] - 1.0) < 0.02 and floors[1] == 0.0 and abs(floors[2] - 0.25) < 0.02 and abs(floors[3] - 0.6) < 0.02, floors
        plan2, plan2_0 = res.get('density_plan2'), res0.get('density_plan2')
        assert plan2 and plan2['probes'] == 0 and plan2['passes'] == 1, plan2
        assert plan2_0 and plan2_0['probes'] == 1 and plan2_0['passes'] == 0, plan2_0
        # the regions planned apart (FLOE_RUST_DENSITY_PLAN_THREADS=2): the
        # same frame from two merged plans
        split, split_res = frame(workers['split'], 1, (LOW,))
        split_plan2 = split_res.get('density_plan2')
        assert split_plan2 and split_plan2['threads'] == 2, split_plan2
        assert split == on, 'two threads draw otherwise in %d px' % sum(1 for i in range(0, len(on), 4) if split[i:i + 4] != on[i:i + 4])
        # the one walk (FLOE_RUST_DENSITY_ONE_WALK=on): one fitted pass, no
        # probe; the specks' page (all under the cut) is not drawn - with the
        # page dots at a zero floor it stands as dots over its box, as many as
        # its shapes light (200 squares of 0.5 px: 50)
        on1, one_res = frame(workers['one'], 3, (LOW,))
        assert not lit(on1, *specks), "TOP's specks (one walk): %d px" % len(lit(on1, *specks))
        one_plan2 = one_res.get('density_plan2')
        assert one_plan2 and one_plan2['probes'] == 0 and one_plan2['passes'] == 1, one_plan2
        dotted, dotted_res = frame(workers['one_dots'], 3, (LOW,))
        one_tiny = lit(dotted, *specks)
        want_tiny = math.ceil(TINY[2] * TINY[3] * (0.05 / PX_UM) ** 2)
        assert len(one_tiny) == want_tiny, "TOP's specks (one walk, page dots): %d dots, want %d" % (len(one_tiny), want_tiny)
        one_floors = (one_res.get('density_floor'), dotted_res.get('density_floor'))
        everything = lit(on, range(W), range(H))
        assert everything == sparse | alone_px | dense | tiny, '%d px lit outside the arrays' % len(everything - sparse - alone_px - dense - tiny)
        # one item per 4 px block the sparse array's members are centred in,
        # the lone DOT's, one per abutting block
        dots = res.get('density_dots')
        cells_items = len(blocks_of(sparse_centres, 400)) + 1 + len(blocks)
        assert dots and dots['items'] == cells_items and dots['over'] == 0, (dots, cells_items)
        assert res.get('density_block') == 4.0, res.get('density_block')
        assert walked_res.get('density_dots') is None and walked_res.get('density_block') is None and lit(walked, range(W), range(H)), \
            'the stack alone draws the DOT squares, no dots'
        print('density stack dots (4 px, spread): sparse array %d dots on the members, lone DOT 1, abutting array %d px = '
              'sum of min(8, members) over %d blocks; density_dots %s; TOP specks none under the 1 px floor, %d px as cut-free at 0 / 0.25 px '
              '(floors %s; plans %s / %s); one walk %s, %d dots with the page dots (floors %s)' % (
                  len(sparse), len(dense), len(blocks), dots, len(drawn), floors, plan2, plan2_0, one_plan2, len(one_tiny), one_floors))
        margin, _ = frame_bg(workers['dots'], 2, (LOW,))
        centre = b''.join(margin[((H // 2 + r) * 2 * W + W // 2) * 4:((H // 2 + r) * 2 * W + W // 2 + W) * 4] for r in range(H))
        assert centre == on, 'the dots margin draws the view otherwise in %d px' % sum(
            1 for i in range(0, len(on), 4) if centre[i:i + 4] != on[i:i + 4])
        print('density stack dots: the margin frame draws the view as the viewport frame did')
        # step 3: pass 1 first, then the frame with the dots
        both = (LOW, MID)
        rounds = frames_of(workers['dots'], 10, both)
        single = frames_of(workers['one_round'], 10, both)
        plain, _ = frame(workers['off'], 10, both)
        assert len(single) == 1 and len(rounds) == 2 and rounds[0][1].get('refining'), [r.get('refining') for _, r in rounds]
        assert lit(plain, range(W), range(H)), 'pass 1 draws the 2/0 original'
        assert rounds[0][0] == plain, 'the first round is not pass 1 alone (%d px differ)' % sum(
            1 for i in range(0, len(plain), 4) if rounds[0][0][i:i + 4] != plain[i:i + 4])
        assert rounds[1][0] == single[0][0] and rounds[1][0] != plain, 'the final round differs from one round'
        assert len(frames_of(workers['dots'], 11, both, bg=True)) == 1, 'a margin has no first round'
        print('density stack dots: progressive - a refining round of pass 1 (= the frame without the stack), then the one-round frame; '
              'none for a margin')
        # a zoom while pass 2 draws: the next generation is answered, the old one dropped
        results, zoomed = zoom_during_pass2(workers['dots'], 20, 21, both)
        assert not any(gen == 20 and not refining and after for gen, refining, _, after in results), 'the old generation published a final frame after the zoom'
        final_b = [px for gen, refining, px, _ in results if gen == 21 and not refining][-1]
        dbu = float(workers['one_round'].cache.meta['dbu'])
        workers['one_round'].submit({'kind': 'render', 'gen': 21, 'scope': 'live', 'bbox': tuple(v / dbu for v in zoomed), 'view': tuple(v / dbu for v in zoomed),
                                     'w': W, 'h': H, 'depth': None, 'cut_px': 3.0, 'lod': False, 'frames': False, 'labels': False,
                                     'abstract': False, 'visible': list(both), 'frame_format': 'raw', 'thin': 'keep', 'frame_cache': False})
        while True:
            res = workers['one_round'].res.get(timeout=300)
            if res.get('kind') == 'frame' and res.get('gen') == 21 and not res.get('refining'):
                break
        assert final_b == bytes(res['rgba']), 'the zoomed view differs from its own one-round frame'
        print('density stack dots: a zoom on the first round is answered (%d results), the old generation drops' % len(results))
    finally:
        for w in workers.values():
            w.stop()


def uneven_layout(path):
    """400 x 400 um: four distinct cells in the corner quarter, two hundred
    more in the far half (tools/validate_fit_budget.py's layout_uneven), each
    a hundred 1-1.3 um boxes - shapes of 1.5-2 px at the scale below, under
    medium's 3 px cut and over pass 2's 1 px floor, in pages pass 1 leaves."""
    import klayout.db as kdb
    ly = kdb.Layout()
    ly.dbu = 0.001
    top = ly.create_cell('TOP')
    layer = ly.layer(*LOW)
    cells = 0

    def place(x_um, y_um):
        nonlocal cells
        cell = ly.create_cell('C%03d' % cells)
        for j in range(10):
            for i in range(10):
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


def history_checks(temp):
    """Pass 2's budget decision is remembered per scale, not per place, and
    the viewer's zoom steps recur everywhere (user 2026-10-01, the synthetic
    chip: a view planned 28 pass-2 pages and lit 85k px fresh, 0 pages and
    69k px after a dense view at the same zoom step had decided). Under a
    1 MB pass-2 reserve the uneven layout's whole extent has to thin and its
    corner quarter fits whole: the corner drawn after the whole layout at the
    same scale equals the corner drawn first by a fresh worker."""
    src = Path(temp) / 'uneven.oas'
    uneven_layout(src)
    done = subprocess.run([sys.executable, '-B', '-m', 'floe2', 'index', str(src)],
                          cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
    assert done.returncode == 0, done.stdout + done.stderr
    env = {'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on', 'FLOE_RUST_DENSITY_BUDGET_MB': '1'}
    fresh, after, roomy = worker(src, env), worker(src, env), worker(src, {k: v for k, v in env.items() if k != 'FLOE_RUST_DENSITY_BUDGET_MB'})
    try:
        dbu = float(fresh.cache.meta['dbu'])
        side, um_per_px = 400.0, 400.0 / 600

        def view(w, gen, box_um):
            px_w = round((box_um[2] - box_um[0]) / um_per_px)
            w.submit({'kind': 'render', 'gen': gen, 'scope': 'headless', 'bbox': tuple(v / dbu for v in box_um), 'view': None,
                      'w': px_w, 'h': px_w, 'depth': None, 'cut_px': 3.0, 'lod': False, 'frames': False, 'labels': False,
                      'abstract': False, 'visible': [LOW], 'frame_format': 'raw', 'thin': 'keep', 'frame_cache': False})
            deadline = time.monotonic() + 300
            while time.monotonic() < deadline:
                res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
                assert res.get('kind') != 'error', res
                if res.get('kind') == 'frame' and res.get('gen') == gen and not res.get('refining'):
                    return bytes(res.pop('rgba')), res
            raise AssertionError('uneven frame timeout')

        whole, corner = (0.0, 0.0, side, side), (0.0, 0.0, side / 4, side / 4)
        first, rf = view(fresh, 1, corner)
        thinned, rt = view(after, 1, whole)
        full, rr = view(roomy, 1, whole)
        assert rt['density_stack']['lit'] < rr['density_stack']['lit'], (
            'the whole layout must thin its pass 2 under the 1 MB reserve: lit %d, %d under 128 MB' % (rt['density_stack']['lit'], rr['density_stack']['lit']))
        again, ra = view(after, 2, corner)
        assert rf['density_stack']['lit'] > 0 and again == first, (
            'the corner after the whole layout at the same scale differs from a fresh one in %d px (lit %d vs %d)' % (
                sum(1 for i in range(0, len(first), 4) if first[i:i + 4] != again[i:i + 4]), ra['density_stack']['lit'], rf['density_stack']['lit']))
        print('density stack: pass 2 is decided per frame - the corner lights %d px fresh and the same after the whole layout thinned '
              'its pass 2 (%d px lit, %d under 128 MB)' % (rf['density_stack']['lit'], rt['density_stack']['lit'], rr['density_stack']['lit']))
    finally:
        for w in (fresh, after, roomy):
            w.stop()


def held_layout(path):
    """300 x 200 um: a BIG cell of 3,000 boxes of 2.1-2.4 um at a 2.5 um pitch
    over (0..150, 0..125) um - over 3 px at the scale below, over medium's
    3 px cut: pass 1's - and thirty distinct cells of a hundred 1-1.3 um boxes
    (1.5-2 px: pass 2's) beside it over (220..295, 0..200) um, every box at a
    random place and size (boxes alike at regular offsets would be written as
    repetitions and indexed as a few records, too light for the budget). Under
    a 1 MB reserve the two pass it together, not alone (a page over the budget
    by itself makes the fit give up and plan everything)."""
    import random
    import klayout.db as kdb
    ly = kdb.Layout()
    ly.dbu = 0.001
    top = ly.create_cell('TOP')
    layer = ly.layer(*LOW)
    big = ly.create_cell('BIG')
    # random jitter of a few nm: boxes at regular offsets would be written as
    # repetitions and indexed as a few records, too light for the budget
    rnd = random.Random(7)
    for j in range(50):
        for i in range(60):
            x, y = i * 2.5 + rnd.randrange(50) / 1000.0, j * 2.5 + rnd.randrange(50) / 1000.0
            w, h = 2.1 + rnd.randrange(300) / 1000.0, 2.1 + rnd.randrange(300) / 1000.0
            big.shapes(layer).insert(kdb.DBox(x, y, x + w, y + h))
    top.insert(kdb.DCellInstArray(big.cell_index(), kdb.DTrans(kdb.DVector(0.0, 0.0))))
    cells = 0
    for j in range(10):
        for i in range(3):
            cell = ly.create_cell('S%03d' % cells)
            for b in range(10):
                for a in range(10):
                    x, y = a * 1.8 + rnd.randrange(100) / 1000.0, b * 1.8 + rnd.randrange(100) / 1000.0
                    w, h = 1.0 + rnd.randrange(300) / 1000.0, 1.0 + rnd.randrange(300) / 1000.0
                    cell.shapes(layer).insert(kdb.DBox(x, y, x + w, y + h))
            top.insert(kdb.DCellInstArray(cell.cell_index(), kdb.DTrans(kdb.DVector(220.0 + i * 25.0, j * 20.0))))
            cells += 1
    ly.write(str(path))


def held_checks(temp):
    """Pass 1's pages cost pass 2's reserve nothing (user 2026-10-01: 37 pages
    of pass 1's, 201 MB by estimate, failed the 0 px floor's probe of the 128 MB
    reserve with nothing new to decode, and medium's pass 2 lit 2.7k px where
    a larger reserve lit 136k): under a 1 MB reserve, which pass 1's BIG page
    and pass 2's new pages pass together, the frame equals the frame under the
    default reserve and reports nothing over budget."""
    src = Path(temp) / 'held.oas'
    held_layout(src)
    done = subprocess.run([sys.executable, '-B', '-m', 'floe2', 'index', str(src)],
                          cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
    assert done.returncode == 0, done.stdout + done.stderr
    env = {'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on'}
    tight, roomy = worker(src, dict(env, FLOE_RUST_DENSITY_BUDGET_MB='1')), worker(src, env)
    try:
        dbu = float(tight.cache.meta['dbu'])
        box_um, px_w, px_h = (0.0, 0.0, 300.0, 200.0), 450, 300

        def view(w, gen):
            w.submit({'kind': 'render', 'gen': gen, 'scope': 'headless', 'bbox': tuple(v / dbu for v in box_um), 'view': None,
                      'w': px_w, 'h': px_h, 'depth': None, 'cut_px': 3.0, 'lod': False, 'frames': False, 'labels': False,
                      'abstract': False, 'visible': [LOW], 'frame_format': 'raw', 'thin': 'keep', 'frame_cache': False})
            deadline = time.monotonic() + 300
            while time.monotonic() < deadline:
                res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
                assert res.get('kind') != 'error', res
                if res.get('kind') == 'frame' and res.get('gen') == gen and not res.get('refining'):
                    return bytes(res.pop('rgba')), res
            raise AssertionError('held frame timeout')

        a, ra = view(tight, 1)
        b, rb = view(roomy, 1)
        assert ra['density_pages']['in_hand'] > 0 and ra['density_pages']['decoded'] > 0 and ra['density_stack']['lit'] > 0, (
            ra['density_pages'], ra['density_stack'])
        p2 = ra['density_plan2']
        assert a == b and (p2['probes_over'], p2['thinned'], ra['density_pages']['over_budget']) == (0, 0, 0), (
            'under the 1 MB reserve pass 2 differs in %d px (lit %d vs %d; pages %s vs %s; plan %s)' % (
                sum(1 for i in range(0, len(a), 4) if a[i:i + 4] != b[i:i + 4]), ra['density_stack']['lit'], rb['density_stack']['lit'],
                ra['density_pages'], rb['density_pages'], p2))
        print('density stack: pass 1\'s pages cost pass 2 nothing - under a 1 MB reserve it lights %d px from %d new pages beside %d '
              'held, as under 128 MB' % (ra['density_stack']['lit'], ra['density_pages']['decoded'], ra['density_pages']['in_hand']))
    finally:
        tight.stop()
        roomy.stop()


def frames_of(w, gen, visible, bg=False):
    """Every frame answer of one render, the refining rounds first: [(pixels,
    result)], the last one final."""
    dbu = float(w.cache.meta['dbu'])
    if bg:
        vw, vh = VIEW[2] - VIEW[0], VIEW[3] - VIEW[1]
        box, size = (VIEW[0] - vw / 2, VIEW[1] - vh / 2, VIEW[2] + vw / 2, VIEW[3] + vh / 2), (2 * W, 2 * H)
    else:
        box, size = VIEW, (W, H)
    job = {'kind': 'render', 'gen': gen, 'scope': 'live', 'bbox': tuple(v / dbu for v in box), 'view': tuple(v / dbu for v in VIEW),
           'w': size[0], 'h': size[1], 'depth': None, 'cut_px': 3.0, 'lod': False, 'frames': False, 'labels': False,
           'abstract': False, 'visible': list(visible), 'frame_format': 'raw', 'thin': 'keep', 'frame_cache': False}
    if bg:
        job['bg'] = True
    w.submit(job)
    out = []
    deadline = time.monotonic() + 300
    while time.monotonic() < deadline:
        res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
        assert res.get('kind') not in ('error', 'dropped'), res
        if res.get('kind') == 'frame' and res.get('gen') == gen:
            out.append((bytes(res.pop('rgba')), res))
            if not res.get('refining'):
                return out
    raise AssertionError('progressive frames timeout')


def zoom_during_pass2(w, gen_a, gen_b, visible):
    """Submit gen_a (VIEW); on its first round submit gen_b (the right half
    of VIEW, zoomed x2) - a mouse zoom while pass 2 draws. Every result until
    gen_b's final: [(gen, refining, pixels)]."""
    dbu = float(w.cache.meta['dbu'])
    vw, vh = VIEW[2] - VIEW[0], VIEW[3] - VIEW[1]
    zoomed = (VIEW[0] + vw / 2, VIEW[1] + vh / 4, VIEW[2], VIEW[1] + 3 * vh / 4)
    def job(gen, box):
        return {'kind': 'render', 'gen': gen, 'scope': 'live', 'bbox': tuple(v / dbu for v in box), 'view': tuple(v / dbu for v in box),
                'w': W, 'h': H, 'depth': None, 'cut_px': 3.0, 'lod': False, 'frames': False, 'labels': False,
                'abstract': False, 'visible': list(visible), 'frame_format': 'raw', 'thin': 'keep', 'frame_cache': False}
    w.submit(job(gen_a, VIEW))
    out, zoomed_at = [], None
    deadline = time.monotonic() + 300
    while time.monotonic() < deadline:
        res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
        assert res.get('kind') not in ('error', 'dropped'), res
        if res.get('kind') != 'frame':
            continue
        out.append((res.get('gen'), bool(res.get('refining')), bytes(res.pop('rgba')), zoomed_at is not None))
        if res.get('gen') == gen_a and res.get('refining') and zoomed_at is None:
            zoomed_at = len(out)
            w.submit(job(gen_b, zoomed))
        if res.get('gen') == gen_b and not res.get('refining'):
            return out, zoomed
    raise AssertionError('zoom during pass 2: no final frame for gen %d' % gen_b)


def worker(src, env):
    for name, value in env.items():
        os.environ[name] = value
    cache = Cache(str(src))
    cache.load()
    w = RustRenderWorker(cache)
    w.start()
    for name in env:
        os.environ.pop(name, None)
    return w


def frame(w, gen, visible, cut_px=3.0):
    dbu = float(w.cache.meta['dbu'])
    w.submit({'kind': 'render', 'gen': gen, 'scope': 'headless', 'bbox': tuple(v / dbu for v in VIEW), 'view': None,
              'w': W, 'h': H, 'depth': None, 'cut_px': cut_px, 'lod': False, 'frames': False,
              'labels': False, 'abstract': False, 'visible': list(visible), 'frame_format': 'raw',
              'thin': 'keep', 'frame_cache': False})
    deadline = time.monotonic() + 300
    while time.monotonic() < deadline:
        res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
        assert res.get('kind') != 'error', res
        if res.get('kind') == 'frame' and res.get('gen') == gen and not res.get('refining'):
            return bytes(res.pop('rgba')), res
    raise AssertionError('density stack frame timeout')


def frame_bg(w, gen, visible, cut_px=3.0):
    """The viewer's margin around VIEW: twice the extent per axis at the same
    scale, flagged bg (gui._submit_margin)."""
    dbu = float(w.cache.meta['dbu'])
    vw, vh = VIEW[2] - VIEW[0], VIEW[3] - VIEW[1]
    box = (VIEW[0] - vw / 2, VIEW[1] - vh / 2, VIEW[2] + vw / 2, VIEW[3] + vh / 2)
    w.submit({'kind': 'render', 'gen': gen, 'scope': 'live', 'bg': True, 'bbox': tuple(v / dbu for v in box),
              'view': tuple(v / dbu for v in VIEW), 'w': 2 * W, 'h': 2 * H, 'depth': None, 'cut_px': cut_px, 'lod': False,
              'frames': False, 'labels': False, 'abstract': False, 'visible': list(visible), 'frame_format': 'raw',
              'thin': 'keep', 'frame_cache': False})
    deadline = time.monotonic() + 300
    while time.monotonic() < deadline:
        res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
        assert res.get('kind') not in ('error', 'dropped'), res
        if res.get('kind') == 'frame' and res.get('gen') == gen and not res.get('refining'):
            return bytes(res.pop('rgba')), res
    raise AssertionError('density stack margin frame timeout')


def px(pixels, c, r):
    return pixels[(r * W + c) * 4:(r * W + c) * 4 + 4]


def lit(pixels, cols, rows):
    return {(c, r) for r in rows for c in cols if px(pixels, c, r) != BLACK}


def main():
    os.environ['FLOE_INDEX_BIN'] = str(ROOT / 'rust/target/release/floe-index')
    os.environ['FLOE_RENDERD_BIN'] = str(ROOT / 'rust/target/release/floe-renderd')
    os.environ['FLOE_RUST_RETAINED_MB'] = '0'
    for name in ('FLOE_RUST_DENSITY_STACK', 'FLOE_RUST_DENSITY_DOTS', 'FLOE_RUST_AREA_TRUE', 'FLOE_RUST_WRITE_ONCE'):
        os.environ.pop(name, None)
    with tempfile.TemporaryDirectory(prefix='floe-density-stack-') as temp:
        src = Path(temp) / 'stack.oas'
        layout(src)
        done = subprocess.run([sys.executable, '-B', '-m', 'floe2', 'index', str(src)],
                              cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
        assert done.returncode == 0, done.stdout + done.stderr
        workers = {
            'off': worker(src, {}),
            'on': worker(src, {'FLOE_RUST_DENSITY_STACK': 'top'}),
            'klayout': worker(src, {'FLOE_RUST_AREA_TRUE': 'off'}),
            'klayout_on': worker(src, {'FLOE_RUST_AREA_TRUE': 'off', 'FLOE_RUST_DENSITY_STACK': 'top'}),
            'ordered': worker(src, {'FLOE_RUST_WRITE_ONCE': 'off'}),
            'ordered_on': worker(src, {'FLOE_RUST_WRITE_ONCE': 'off', 'FLOE_RUST_DENSITY_STACK': 'top'}),
        }
        try:
            both = (LOW, MID, TOP)
            off, off_res = frame(workers['off'], 1, both)
            on, on_res = frame(workers['on'], 1, both)
            alone, _ = frame(workers['off'], 2, (ALONE,))
            # the squares as a cut-free frame draws them, one layer at a time
            low_free, _ = frame(workers['off'], 3, (LOW,), cut_px=0.0)
            top_free, _ = frame(workers['off'], 4, (TOP,), cut_px=0.0)
            # the bottom right quadrant: nothing above layer 1's squares
            br = (range(202, W), range(102, H))
            assert not lit(off, *br), 'without the stack the cut drops the squares'
            want = lit(low_free, *br)
            assert want and lit(on, *br) == want, 'bottom right: %d px lit with the stack, %d in the cut-free frame' % (len(lit(on, *br)), len(want))
            for (c, r) in want:
                assert px(on, c, r) == px(low_free, c, r), 'bottom right (%d, %d): another colour' % (c, r)
            # inside the left rectangle: the rectangle alone
            inside = (range(2, 198), range(2, 198))
            assert lit(on, *inside) == lit(alone, *inside), 'inside the left rectangle %d px lit, alone %d' % (
                len(lit(on, *inside)), len(lit(alone, *inside)))
            assert len(lit(low_free, *inside)) > 0 and len(lit(top_free, range(20, 180), range(20, 180)) - lit(alone, range(20, 180), range(20, 180))) > 0, \
                'squares exist under the rectangle and inside it'
            # the top right quadrant: the top layer's squares over layer 2's rectangle
            tr = (range(202, W), range(2, 98))
            squares = lit(top_free, *tr)
            assert squares, "the top layer has squares over layer 2's rectangle"
            assert lit(on, *tr) == lit(off, *tr) | squares, 'top right: %d px lit, %d as rectangle + squares' % (
                len(lit(on, *tr)), len(lit(off, *tr) | squares))
            for (c, r) in squares:
                assert px(on, c, r) == px(top_free, c, r), "top right (%d, %d): not the top layer's colour" % (c, r)
            for (c, r) in lit(off, *tr) - squares:
                assert px(on, c, r) == px(off, c, r), 'top right (%d, %d): the rectangle changed' % (c, r)
            stack, pages = on_res.get('density_stack'), on_res.get('density_pages')
            assert stack and stack['lit'] > 0 and stack['top'] > 0 and stack['lower'] > 0 and stack['covered'] > 0, stack
            assert pages and pages['planned'] > 0 and pages['decoded'] > 0 and pages['over_budget'] == 0, pages
            assert off_res.get('density_stack') is None and off_res.get('density_pages') is None, off_res.get('density_stack')
            print('density stack: bottom right %d square px as cut-free, left rectangle inside = alone (%d px), top right %d top-layer px over '
                  'layer 2; counts %s; pass 2 pages %s' % (len(want), len(lit(alone, *inside)), len(squares),
                                                          ' '.join('%s=%d' % kv for kv in stack.items()),
                                                          ' '.join('%s=%d' % kv for kv in pages.items())))
            # the visible layers only: layer 1/0 alone is its own top plane
            low_on, low_res = frame(workers['on'], 6, (LOW,))
            assert low_on == low_free, 'layer 1/0 alone with the stack differs from its cut-free frame in %d px' % sum(
                1 for i in range(0, len(low_on), 4) if low_on[i:i + 4] != low_free[i:i + 4])
            assert low_res['density_stack']['top'] > 0 and low_res['density_stack']['lower'] == 0, low_res['density_stack']
            mid_on, _ = frame(workers['on'], 7, (LOW, MID))
            mid_off, _ = frame(workers['off'], 7, (LOW, MID))
            assert lit(mid_on, *tr) == lit(mid_off, *tr), 'with 4/0 off its squares still show in the top right (%d vs %d px)' % (
                len(lit(mid_on, *tr)), len(lit(mid_off, *tr)))
            assert lit(mid_on, *br) == want, 'with 4/0 off the bottom right changed'
            print('density stack: layer 1/0 alone = its cut-free frame; 1/0 + 2/0 shows none of 4/0')
            # the viewer's margin (bg, twice the extent per axis) draws the view's
            # pixels as the viewport frame did: pass 2's fit is remembered per scale
            # and side (2026-09-27: planned to half of what the generation had left,
            # the margin's pass 2 decoded 23 pages where the viewport's decoded 2,208)
            margin, mres = frame_bg(workers['on'], 8, both)
            centre = b''.join(margin[((H // 2 + r) * 2 * W + W // 2) * 4:((H // 2 + r) * 2 * W + W // 2 + W) * 4] for r in range(H))
            assert centre == on, 'the margin draws the view otherwise than the viewport frame in %d px' % sum(
                1 for i in range(0, len(on), 4) if centre[i:i + 4] != on[i:i + 4])
            assert mres.get('density_pages') and mres['density_pages']['over_budget'] == 0, mres.get('density_pages')
            print('density stack: the margin frame draws the view as the viewport frame did (pass 2 pages %s)' % (
                ' '.join('%s=%d' % kv for kv in mres['density_pages'].items())))
            for name, base in (('klayout', 'klayout_on'), ('ordered', 'ordered_on')):
                a, _ = frame(workers[name], 5, both)
                b, b_res = frame(workers[base], 5, both)
                assert a == b and b_res.get('density_stack') is None and b_res.get('density_pages') is None, '%s: the stack must be off' % base
                print('density stack: %s draws as without the variable, no counts' % base)
        finally:
            for w in workers.values():
                w.stop()
        dots_checks(temp)
        history_checks(temp)
        held_checks(temp)
    print('density stack gate: OK')


if __name__ == '__main__':
    main()
