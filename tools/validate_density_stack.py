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
    (0.5 px) squares at a 3 px pitch (one page of 200) are under it - spread
    by their page's occupancy grid (design.ovb; the page spread on by default
    since 0.12.277) about as many pixels as a cut-free frame lights, none with
    FLOE_RUST_DENSITY_PAGE_SPREAD=off (as before; FLOE_RUST_DENSITY_PAGE_DOTS
    off: page dots were far denser than their shapes) - and pass 2 fits at
    the density cut with no probe (density_plan2: no probe, one pass);
    FLOE_RUST_DENSITY_FLOOR_PX
    lowers it - 0 and 0.25 px decode the squares and draw them as a cut-free
    frame does (a zero floor's probe fits), 0.6 px leaves them out - and the
    frame reports the floor it planned at (density_floor 1 / 0 / 0.25 / 0.59);
  * the one walk (FLOE_RUST_DENSITY_ONE_WALK=on; the default for a day,
    opt-in since 0.12.261): pass 2 plans once (no probe, one pass), its pages
    at the cells' cut; the squares' page, all under the cut, is not drawn -
    with the page dots at a zero floor it stands as exactly ceil(200 x 0.25)
    = 50 dots, what the squares' area lights cut-free;
  * pass 2's regions planned apart and merged - on two threads
    (FLOE_RUST_DENSITY_PLAN_THREADS=2, density_plan2 threads 2) and on the
    default's four or the cores there are (0.12.268) - draw the frame one
    plan draws (FLOE_RUST_DENSITY_PLAN_THREADS=1);
  * the dot blocks kept in a grid over each cell's view (0.12.268) draw the
    frame the hash map draws (FLOE_RUST_DENSITY_DOT_GRID=off), and the dot
    items are what came from where (density_plan2's by_*);
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
the corner a fresh worker draws, and the whole extent planned on two threads
(FLOE_RUST_DENSITY_PLAN_THREADS=2) and on the default's thins to one plan's
frame (history_checks). Pass 1's pages cost pass 2's
reserve nothing: under a 1 MB reserve, which pass 1's own pages pass, the frame
equals the default reserve's and reports nothing over budget (held_checks).
Pass 2's threads walk a point list's members in their own regions: over nine
tiles one, two and four threads and the bounds (FLOE_RUST_DENSITY_DOT_BOXES=off)
draw one frame, four threads count the members about once, the bounds about
twice (lists_checks). A point list's chunk whose blocks are full is passed
over unread and a dense one read at a step: two via cells' lists of 60,000
random vias over 20 um, at 100 and 200 px, draw the frame every member read
draws (FLOE_RUST_DENSITY_LIST_FULL=off, FLOE_RUST_DENSITY_LIST_SAMPLE=off and
FLOE_RUST_DENSITY_LIST_FAST=off) and one thread's, reading under a fifth of
the members at 100 px (dense_lists_checks). Pass 2's reserve is what pass 1 left of the budget when
that is more: at depth 0 a TOP's own boxes under a pixel, past a 32 MB
budget's fixed 4 MB reserve, are drawn as a cut-free frame draws them, and the
fixed reserve (FLOE_RUST_DENSITY_RESERVE_LEFT=off) draws none (left_checks).
Pass 2's budget fit keeps the cells' cut: its pages past eight reserves are
fitted in one pass, on the threads; the ladder (FLOE_RUST_DENSITY_FIT_LADDER=on)
takes passes up the cut (ladder_checks). A page under the floor, with the page
spread on, puts its dots where its occupancy grid (design.ovb) holds shapes:
none between two squares one page holds, as a cut-free frame; the kill switch
(FLOE_RUST_DENSITY_PAGE_OCC=off), a cache without design.ovb and one with
another index's spread it over its box (occ_checks).

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
# the product command line: the Rust `floe2` (rust/floe2; the Python
# floe2 CLI is gone - docs/SHARED_APP_LAYER.ko.md P1c)
FLOE2 = os.environ.get("FLOE2_BIN") or str(ROOT / "rust" / "target" / "release" / "floe2")
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT / "tools" / "oracle"))  # floe_oracle (P3)
from floe_oracle.cache import Cache
from floe_oracle.rust_render import RustRenderWorker

W, H = 400, 200
PX_UM = 0.1
LOW, MID, ALONE, TOP = (1, 0), (2, 0), (3, 0), (4, 0)
DEEP = (5, 0)
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
    done = subprocess.run([FLOE2, 'index', str(src)],
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
        # (the page spread off: a floor taken leaves the specks out)
        'floor06': worker(src, {'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on', 'FLOE_RUST_DENSITY_FLOOR_PX': '0.6',
                                'FLOE_RUST_DENSITY_PAGE_SPREAD': 'off'}),
        # the page spread off (its kill switch; the default since 0.12.277)
        'nospread': worker(src, {'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on', 'FLOE_RUST_DENSITY_PAGE_SPREAD': 'off'}),
        # pass 2's regions planned apart on two threads and merged, as one plan
        # (the default since 0.12.268: four threads, or the cores there are)
        'split': worker(src, {'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on', 'FLOE_RUST_DENSITY_PLAN_THREADS': '2'}),
        'one_plan': worker(src, {'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on', 'FLOE_RUST_DENSITY_PLAN_THREADS': '1'}),
        # the dot blocks in the hash map, as before 2026-10-02
        'map': worker(src, {'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on', 'FLOE_RUST_DENSITY_DOT_GRID': 'off'}),
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
        # floor at 1 px"): TOP's 0.5 px specks are under it and no probe is made
        # (one fitted pass at the density cut). Their page is spread by its
        # occupancy grid (design.ovb; the page spread on by default since
        # 0.12.277, user 2026-10-03): about the pixels a cut-free frame lights,
        # in their own place; FLOE_RUST_DENSITY_PAGE_SPREAD=off (the kill
        # switch) neither draws nor dots them, as before.
        # FLOE_RUST_DENSITY_FLOOR_PX lowers the floor: 0 and 0.25 px draw the
        # specks as a cut-free frame does, 0.6 px (the spread off) does not;
        # the frame says which floor it took
        free, _ = frame(workers['stack'], 3, (LOW,), cut_px=0.0)
        tiny = lit(on, *specks)
        want_free = len(lit(free, *specks))
        assert want_free and abs(len(tiny) - want_free) <= want_free // 4 and res['density_plan2']['occ_pages'] >= 1, \
            "TOP's specks under the 1 px floor, spread: %d px, cut-free %d (%s)" % (len(tiny), want_free, res['density_plan2'])
        unspread, unspread_res = frame(workers['nospread'], 1, (LOW,))
        assert not lit(unspread, *specks), "TOP's specks under the 1 px floor, the spread off: %d px" % len(lit(unspread, *specks))
        assert lit(unspread, range(W), range(H)) == lit(on, range(W), range(H)) - tiny, 'the spread changes more than the specks'
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
        # the regions planned apart (FLOE_RUST_DENSITY_PLAN_THREADS=2, and the
        # default's four threads or the cores there are): the same frame from
        # the merged plans as from one
        split, split_res = frame(workers['split'], 1, (LOW,))
        split_plan2 = split_res.get('density_plan2')
        assert split_plan2 and split_plan2['threads'] == 2, split_plan2
        single, single_res = frame(workers['one_plan'], 1, (LOW,))
        assert single_res['density_plan2']['threads'] == 1, single_res['density_plan2']
        assert plan2['threads'] == min(4, os.cpu_count() or 1, plan2['regions']), plan2
        for name, other in (('two threads', split), ('the default threads', on)):
            assert other == single, '%s draw otherwise than one plan in %d px' % (name, sum(1 for i in range(0, len(on), 4) if other[i:i + 4] != single[i:i + 4]))
        # the dot blocks in a grid over each cell's view (FLOE_RUST_DENSITY_DOT_GRID,
        # 2026-10-02): the frame of the hash map, every block in it; the dot
        # items are what came from where
        mapped, mapped_res = frame(workers['map'], 1, (LOW,))
        assert mapped == on, 'the dot grid draws otherwise than the hash map in %d px' % sum(1 for i in range(0, len(on), 4) if mapped[i:i + 4] != on[i:i + 4])
        for p2 in (plan2, mapped_res['density_plan2']):
            assert p2['items'] == sum(p2[k] for k in ('by_nodes', 'by_placements', 'by_arrays', 'by_list_members', 'by_list_chunks',
                                                        'by_array_members', 'by_pages')), p2
        assert plan2['map_updates'] == 0 < mapped_res['density_plan2']['map_updates'], (plan2, mapped_res['density_plan2'])
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
        # the cells' items, and the specks' blocks with the spread
        unspread_dots = unspread_res.get('density_dots')
        assert unspread_dots and unspread_dots['items'] == cells_items and unspread_dots['over'] == 0, (unspread_dots, cells_items)
        assert dots and dots['items'] > cells_items and dots['over'] == 0, (dots, cells_items)
        assert res.get('density_block') == 4.0, res.get('density_block')
        assert walked_res.get('density_dots') is None and walked_res.get('density_block') is None and lit(walked, range(W), range(H)), \
            'the stack alone draws the DOT squares, no dots'
        print('density stack dots (4 px, spread): sparse array %d dots on the members, lone DOT 1, abutting array %d px = '
              'sum of min(8, members) over %d blocks; density_dots %s; TOP specks spread under the 1 px floor %d px (cut-free %d; '
              'the spread off none), %d px as cut-free at 0 / 0.25 px '
              '(floors %s; plans %s / %s); one walk %s, %d dots with the page dots (floors %s)' % (
                  len(sparse), len(dense), len(blocks), dots, len(tiny), want_free, len(drawn), floors, plan2, plan2_0, one_plan2, len(one_tiny),
                  one_floors))
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
    done = subprocess.run([FLOE2, 'index', str(src)],
                          cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
    assert done.returncode == 0, done.stdout + done.stderr
    # the fixed reserve (FLOE_RUST_DENSITY_RESERVE_LEFT=off): what pass 1
    # leaves would hold the whole layout (0.12.270)
    env = {'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on', 'FLOE_RUST_DENSITY_BUDGET_MB': '1',
           'FLOE_RUST_DENSITY_PLAN_THREADS': '1', 'FLOE_RUST_DENSITY_RESERVE_LEFT': 'off'}
    fresh, after, roomy = worker(src, env), worker(src, env), worker(src, {k: v for k, v in env.items() if k != 'FLOE_RUST_DENSITY_BUDGET_MB'})
    split = worker(src, dict(env, FLOE_RUST_DENSITY_PLAN_THREADS='2'))
    # the default: four threads, or the cores there are (0.12.268)
    default = worker(src, {k: v for k, v in env.items() if k != 'FLOE_RUST_DENSITY_PLAN_THREADS'})
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
        # the regions planned on two threads and merged are fitted as the one
        # plan is (Cache::fit_plan; 0.12.267 - before, a view that thins
        # took one thread): the same frame, two threads, thinned
        apart, rs = view(split, 1, whole)
        assert apart == thinned and (rs['density_plan2']['threads'], rs['density_plan2']['thinned']) == (2, 1), (
            'the threaded pass 2 that thins differs in %d px: %s' % (
                sum(1 for i in range(0, len(apart), 4) if apart[i:i + 4] != thinned[i:i + 4]), rs['density_plan2']))
        dealt, rd = view(default, 1, whole)
        threads = min(4, os.cpu_count() or 1, rd['density_plan2']['regions'])
        assert dealt == thinned and (rd['density_plan2']['threads'], rd['density_plan2']['thinned']) == (threads, 1), (
            'the default threads that thin differ in %d px: %s' % (
                sum(1 for i in range(0, len(dealt), 4) if dealt[i:i + 4] != thinned[i:i + 4]), rd['density_plan2']))
        again, ra = view(after, 2, corner)
        assert rf['density_stack']['lit'] > 0 and again == first, (
            'the corner after the whole layout at the same scale differs from a fresh one in %d px (lit %d vs %d)' % (
                sum(1 for i in range(0, len(first), 4) if first[i:i + 4] != again[i:i + 4]), ra['density_stack']['lit'], rf['density_stack']['lit']))
        print('density stack: pass 2 is decided per frame - the corner lights %d px fresh and the same after the whole layout thinned '
              'its pass 2 (%d px lit, %d under 128 MB); on two threads it thins alike' % (
                  rf['density_stack']['lit'], rt['density_stack']['lit'], rr['density_stack']['lit']))
    finally:
        for w in (fresh, after, roomy, split, default):
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
    done = subprocess.run([FLOE2, 'index', str(src)],
                          cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
    assert done.returncode == 0, done.stdout + done.stderr
    env = {'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on'}
    # the fixed reserve (FLOE_RUST_DENSITY_RESERVE_LEFT=off): what pass 1
    # leaves would hold it all (0.12.270)
    tight, roomy = worker(src, dict(env, FLOE_RUST_DENSITY_BUDGET_MB='1', FLOE_RUST_DENSITY_RESERVE_LEFT='off')), worker(src, env)
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


def probe_threads_checks(temp):
    """Pass 2's floor probe plans on the threads its fit does (user
    2026-10-04, the field chip with a floor under the density cut: the probe
    planned as one, and one that fits is the plan - `pass 2 plan 7258 ms`;
    renderd density_probe_threads, FLOE_RUST_DENSITY_PROBE_THREADS=off the
    kill switch). The point-list vias of lists_layout at a zero floor, 9
    tiles: the probe holds, the frame is the one plan's to the pixel, planned
    on four threads (one with the switch off)."""
    src = Path(temp) / 'probe_lists.oas'
    lists_layout(src)
    done = subprocess.run([FLOE2, 'index', str(src)],
                          cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
    assert done.returncode == 0, done.stdout + done.stderr
    env = {'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on', 'FLOE_RUST_DENSITY_FLOOR_PX': '0', 'FLOE_RUST_DENSITY_PLAN_THREADS': '4'}
    workers = {'on': worker(src, dict(env, FLOE_RUST_DENSITY_PROBE_THREADS='on')), 'off': worker(src, dict(env, FLOE_RUST_DENSITY_PROBE_THREADS='off'))}
    try:
        dbu = float(workers['on'].cache.meta['dbu'])

        def view(w):
            w.submit({'kind': 'render', 'gen': 1, 'scope': 'headless', 'bbox': (0.0, 0.0, 300.1 / dbu, 300.1 / dbu), 'view': None,
                      'w': 1000, 'h': 1000, 'depth': None, 'cut_px': 3.0, 'lod': False, 'frames': False, 'labels': False,
                      'abstract': False, 'visible': [LOW], 'frame_format': 'raw', 'thin': 'keep', 'frame_cache': False})
            deadline = time.monotonic() + 300
            while time.monotonic() < deadline:
                res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
                assert res.get('kind') != 'error', res
                if res.get('kind') == 'frame' and res.get('gen') == 1 and not res.get('refining'):
                    return bytes(res.pop('rgba')), res
            raise AssertionError('probe frame timeout')

        (on, ron), (off, roff) = view(workers['on']), view(workers['off'])
        p_on, p_off = ron['density_plan2'], roff['density_plan2']
        assert p_on['probes'] == p_off['probes'] == 1 and p_on['probes_over'] == p_off['probes_over'] == 0, (p_on, p_off)
        assert ron['density_floor'] == 0.0 and ron['density_stack']['lit'] > 0, (ron['density_floor'], ron['density_stack'])
        assert on == off and (p_on['threads'], p_off['threads']) == (4, 1), (
            sum(1 for i in range(0, len(on), 4) if on[i:i + 4] != off[i:i + 4]), p_on['threads'], p_off['threads'])
        print('density stack: the floor probe on four threads - its frame the one plan\'s (%d px lit), probe %d / %d us'
              % (ron['density_stack']['lit'], p_on['probe_us'], p_off['probe_us']))
    finally:
        for w in workers.values():
            w.stop()


def lists_layout(path):
    """A routing cell's vias: a 0.1 um VIA placed at 4,000 random places over
    300 x 300 um, written with KLayout's strongest compression - an irregular
    repetition, indexed as point lists."""
    import random
    import klayout.db as kdb
    ly = kdb.Layout()
    ly.dbu = 0.001
    top = ly.create_cell('TOP')
    via = ly.create_cell('VIA')
    via.shapes(ly.layer(*LOW)).insert(kdb.DBox(0, 0, 0.1, 0.1))
    rnd = random.Random(5)
    for _ in range(4000):
        top.insert(kdb.DCellInstArray(via.cell_index(), kdb.DTrans(kdb.DVector(rnd.randrange(300_000) / 1000.0, rnd.randrange(300_000) / 1000.0))))
    options = kdb.SaveLayoutOptions()
    options.format = 'OASIS'
    options.oasis_compression_level = 10
    ly.write(str(path), options)


def lists_checks(temp):
    """Pass 2's threads walk a point list's members in their own regions
    (user 2026-10-02, field: `list members 86.6M`): renderd deals the regions
    round robin, so a thread's regions span the view, and each thread counted
    every member of a list across it - a million random vias, 1 / 2 / 4
    threads: 1 / 2 / 2.96 M members, pass 2 planned 28 / 46 / 71 ms. Over 9
    tiles (1000 x 1000 px) the frame of one, two and four threads and of the
    bounds (FLOE_RUST_DENSITY_DOT_BOXES=off) is one; four threads count the
    members once and a block's edge more, the bounds about twice - every
    member read (FLOE_RUST_DENSITY_LIST_BY_DOT=off). A member reads for the
    members that make a dot (floe_vfs HierOpts::dot_list_by_dot, 0.12.295:
    a 0.11 px^2 VIA one in 8): the frames of the threads and the bounds
    are one too, from an eighth of the members or fewer."""
    src = Path(temp) / 'lists.oas'
    lists_layout(src)
    done = subprocess.run([FLOE2, 'index', str(src)],
                          cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
    assert done.returncode == 0, done.stdout + done.stderr
    every = {'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on', 'FLOE_RUST_DENSITY_LIST_BY_DOT': 'off'}
    by_dot = dict(every, FLOE_RUST_DENSITY_LIST_BY_DOT='on')
    by_members = None
    for env in (every, by_dot):
        by_members = lists_threads(src, env, by_members)


def lists_threads(src, env, every_members):
    """lists_checks under `env`: every member read (`every_members` None) or
    a member for those that make a dot (the counts every member gave)."""
    workers = {threads: worker(src, dict(env, FLOE_RUST_DENSITY_PLAN_THREADS=threads)) for threads in ('1', '2', '4')}
    workers['bounds'] = worker(src, dict(env, FLOE_RUST_DENSITY_PLAN_THREADS='4', FLOE_RUST_DENSITY_DOT_BOXES='off'))
    try:
        dbu = float(workers['1'].cache.meta['dbu'])
        side = 1000

        def view(w):
            w.submit({'kind': 'render', 'gen': 1, 'scope': 'headless', 'bbox': (0.0, 0.0, 300.1 / dbu, 300.1 / dbu), 'view': None,
                      'w': side, 'h': side, 'depth': None, 'cut_px': 3.0, 'lod': False, 'frames': False, 'labels': False,
                      'abstract': False, 'visible': [LOW], 'frame_format': 'raw', 'thin': 'keep', 'frame_cache': False})
            deadline = time.monotonic() + 300
            while time.monotonic() < deadline:
                res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
                assert res.get('kind') != 'error', res
                if res.get('kind') == 'frame' and res.get('gen') == 1 and not res.get('refining'):
                    return bytes(res.pop('rgba')), res
            raise AssertionError('point list frame timeout')

        frames = {name: view(w) for name, w in workers.items()}
        one, one_res = frames['1']
        members = {name: res['density_plan2']['by_list_members'] for name, (_, res) in frames.items()}
        if every_members is None:
            assert one_res['density_stack']['lit'] > 0 and 3_000 <= members['1'] <= 4_000, (one_res['density_stack'], members)
        else:
            # a member for the members that make a dot: an eighth or fewer
            assert one_res['density_stack']['lit'] > 0 and 0 < members['1'] * 8 <= every_members['1'], (one_res['density_stack'], members, every_members)
        for name, (pixels, res) in frames.items():
            assert pixels == one, '%s draws otherwise than one thread in %d px' % (
                name, sum(1 for i in range(0, len(one), 4) if pixels[i:i + 4] != one[i:i + 4]))
        assert frames['4'][1]['density_plan2']['threads'] == 4, frames['4'][1]['density_plan2']
        assert members['4'] <= 1.25 * members['1'] and members['bounds'] >= 1.5 * members['1'], members
        print('density stack: point lists%s - one, two and four threads and the bounds draw alike (%d px lit); members '
              'counted %s' % ('' if every_members is None else ', a member for those that make a dot', one_res['density_stack']['lit'],
                              ' / '.join('%s %d' % kv for kv in members.items())))
        return members
    finally:
        for w in workers.values():
            w.stop()


def dense_lists_layout(path):
    """Two via cells' 0.1 um VIAs, 60,000 each at random over 20 x 20 um,
    written with KLayout's strongest compression - a point list a cell, the
    second over the first's blocks."""
    import random
    import klayout.db as kdb
    ly = kdb.Layout()
    ly.dbu = 0.001
    top = ly.create_cell('TOP')
    rnd = random.Random(7)
    for name in ('VIA_A', 'VIA_B'):
        via = ly.create_cell(name)
        via.shapes(ly.layer(*LOW)).insert(kdb.DBox(0, 0, 0.1, 0.1))
        for _ in range(60_000):
            top.insert(kdb.DCellInstArray(via.cell_index(), kdb.DTrans(kdb.DVector(rnd.randrange(20_000) / 1000.0, rnd.randrange(20_000) / 1000.0))))
    options = kdb.SaveLayoutOptions()
    options.format = 'OASIS'
    options.oasis_compression_level = 10
    ly.write(str(path), options)


def dense_lists_checks(temp):
    """A point list's chunks (256 members in Morton order) whose blocks are
    full are passed over unread, and dense ones read every step-th member,
    each standing for the step (user 2026-10-03, the field chip with all 449
    layers: `list members 765.7M`, pass 2 planned 32 s). Two lists of 60,000
    random vias over 20 um - about 380 a list in a block at 100 px, 96 at
    200 px - draw the frame of every member read (the three switches off:
    0.12.279's walk) on four threads and on one; the chunks of the second
    list in the first's full blocks are passed over, and at 100 px the dense
    ones read at a step - under a fifth of the members read (at 200 px none
    is dense enough). A member read for those that make a dot (floe_vfs
    HierOpts::dot_list_by_dot) draws otherwise: off here, lists_checks has
    it."""
    src = Path(temp) / 'dense_lists.oas'
    dense_lists_layout(src)
    done = subprocess.run([FLOE2, 'index', str(src)],
                          cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
    assert done.returncode == 0, done.stdout + done.stderr
    env = {'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on', 'FLOE_RUST_DENSITY_LIST_BY_DOT': 'off'}
    every = {'FLOE_RUST_DENSITY_LIST_FULL': 'off', 'FLOE_RUST_DENSITY_LIST_SAMPLE': 'off', 'FLOE_RUST_DENSITY_LIST_FAST': 'off'}
    workers = {
        'default': worker(src, env),
        'one thread': worker(src, dict(env, FLOE_RUST_DENSITY_PLAN_THREADS='1')),
        'full off': worker(src, dict(env, FLOE_RUST_DENSITY_LIST_FULL='off')),
        'sample off': worker(src, dict(env, FLOE_RUST_DENSITY_LIST_SAMPLE='off')),
        'every member': worker(src, dict(env, **every)),
    }
    try:
        dbu = float(workers['default'].cache.meta['dbu'])

        def view(w, gen, side):
            w.submit({'kind': 'render', 'gen': gen, 'scope': 'headless', 'bbox': (0.0, 0.0, 20.01 / dbu, 20.01 / dbu), 'view': None,
                      'w': side, 'h': side, 'depth': None, 'cut_px': 1.0, 'lod': False, 'frames': False, 'labels': False,
                      'abstract': False, 'visible': [LOW], 'frame_format': 'raw', 'thin': 'keep', 'frame_cache': False})
            deadline = time.monotonic() + 300
            while time.monotonic() < deadline:
                res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
                assert res.get('kind') != 'error', res
                if res.get('kind') == 'frame' and res.get('gen') == gen and not res.get('refining'):
                    return bytes(res.pop('rgba')), res
            raise AssertionError('dense list frame timeout')

        for gen, side in ((1, 100), (2, 200)):
            frames = {name: view(w, gen, side) for name, w in workers.items()}
            want, want_res = frames['every member']
            assert want_res['density_stack']['lit'] > 0, want_res['density_stack']
            for name, (pixels, res) in frames.items():
                assert pixels == want, '%d px: %s draws otherwise than every member read in %d px' % (
                    side, name, sum(1 for i in range(0, len(want), 4) if pixels[i:i + 4] != want[i:i + 4]))
            p2 = {name: res['density_plan2'] for name, (_, res) in frames.items()}
            read = {name: p['by_list_members'] for name, p in p2.items()}
            assert read['every member'] == 120_000 and p2['every member']['full_chunks'] == p2['every member']['sampled_chunks'] == 0, p2['every member']
            assert p2['full off']['full_chunks'] == 0 < p2['sample off']['full_chunks'] and p2['sample off']['sampled_chunks'] == 0, (p2['full off'], p2['sample off'])
            assert p2['default']['full_chunks'] > 0, p2['default']
            assert read['default'] == read['one thread'] and read['sample off'] < read['every member'], read
            if side == 100:
                # 380 a block: dense enough to read at a step (twice
                # CHUNK_SAMPLE_PER_BLOCK on a block's area of a chunk's run)
                assert p2['default']['sampled_chunks'] > 0 and read['default'] * 5 < read['every member'], (read, p2['default'])
            else:
                assert p2['default']['sampled_chunks'] == 0 and read['default'] == read['sample off'], (read, p2['default'])
            print('density stack: dense point lists at %d px - one frame (%d px lit), members read %s; default: %d chunks in full '
                  'blocks, %d sampled' % (side, want_res['density_stack']['lit'], ' / '.join('%s %d' % kv for kv in read.items()),
                                          p2['default']['full_chunks'], p2['default']['sampled_chunks']))
    finally:
        for w in workers.values():
            w.stop()


def cells_layout(path):
    """Pass 2's space by cells: LOW's 7.7 um boxes on an 8 um pitch over x
    0-32 um (3 px gaps: a 32 px cell a gap crosses has 96 px free, under an
    eighth), the right 8 um open; a 0.1 um VIA on MID every 0.5 um over the
    view (an array, its dots); TOP's 0.1 um specks every 2 um (the top
    plane's dots, over LOW too); DEEP's squares two levels down in the open
    part; ALONE named and empty (a layer of the file no cell holds)."""
    import klayout.db as kdb
    ly = kdb.Layout()
    ly.dbu = 0.001
    top = ly.create_cell('TOP')
    low = ly.layer(*LOW)
    for i in range(4):
        for j in range(3):
            top.shapes(low).insert(kdb.DBox(i * 8.0, j * 8.0, i * 8.0 + 7.7, min(20.0, j * 8.0 + 7.7)))
    via = ly.create_cell('VIA')
    via.shapes(ly.layer(*MID)).insert(kdb.DBox(0, 0, 0.1, 0.1))
    top.insert(kdb.DCellInstArray(via.cell_index(), kdb.DTrans(kdb.DVector(0.2, 0.2)), kdb.DVector(0.5, 0), kdb.DVector(0, 0.5), 80, 40))
    specks = ly.layer(*TOP)
    for i in range(20):
        for j in range(10):
            top.shapes(specks).insert(kdb.DBox(1.0 + i * 2.0, 1.0 + j * 2.0, 1.1 + i * 2.0, 1.1 + j * 2.0))
    # DEEP two levels down (TOP > NEST > DEEP_CELL, a 0.1 um square placed
    # 240 times in the open right part): within depth 2, not 1 (a cell at the
    # depth draws its children as outlines)
    deep = ly.create_cell('DEEP_CELL')
    deep.shapes(ly.layer(*DEEP)).insert(kdb.DBox(0, 0, 0.1, 0.1))
    nest = ly.create_cell('NEST')
    for i in range(12):
        for j in range(20):
            nest.insert(kdb.DCellInstArray(deep.cell_index(), kdb.DTrans(kdb.DVector(i * 0.5, j * 0.5))))
    top.insert(kdb.DCellInstArray(nest.cell_index(), kdb.DTrans(kdb.DVector(33.0, 6.0))))
    # ALONE named and empty: a layer of the file (its LAYERNAME) no cell holds
    ly.layer(kdb.LayerInfo(ALONE[0], ALONE[1], 'ALONE'))
    options = kdb.SaveLayoutOptions()
    options.format = 'OASIS'
    ly.write(str(path), options)


def cells_checks(temp):
    """Pass 2 plans the space by cells (a reviewer, 2026-10-03: per tile the
    bounding box of its free pixels was nearly the tile when 1 % of it was
    free, and the joint plan was decided by those boxes' area - an all-layer
    fit view at full depth walked every layer over the whole frame). LOW's
    boxes cover x 0-32 um but for 3 px gaps, MID's VIAs are dots over the
    view, TOP's specks the top plane's: the tile boxes
    (FLOE_RUST_DENSITY_FREE_CELLS=off) span the frame and plan both sides
    jointly, one pass; by cells (the default) the sides plan apart, the
    others' over cells with an eighth of their pixels free - a cell a gap
    crosses alone is left to the originals, its MID dots out - with fewer
    items, and the frame is the joint plan's but in the gaps; every cell
    with a free pixel (FLOE_RUST_DENSITY_OTHERS_MIN=0) draws it whole. A
    topmost layer on that no cell holds is not the top plane: the topmost
    with shapes is (the frame of the layers without it); the kill switch
    (FLOE_RUST_DENSITY_TOP_HELD=off) plans an empty top plane's side,
    drawn."""
    src = Path(temp) / 'cells.oas'
    cells_layout(src)
    done = subprocess.run([FLOE2, 'index', str(src)],
                          cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
    assert done.returncode == 0, done.stdout + done.stderr
    env = {'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on'}
    workers = {'cells': worker(src, env), 'tiles': worker(src, dict(env, FLOE_RUST_DENSITY_FREE_CELLS='off')),
               'any': worker(src, dict(env, FLOE_RUST_DENSITY_OTHERS_MIN='0')),
               'top_any': worker(src, dict(env, FLOE_RUST_DENSITY_TOP_HELD='off'))}
    try:
        layers = (LOW, MID, TOP)
        frames = {name: frame(w, 1, layers) for name, w in workers.items()}
        (cells, cells_res), (tiles, tiles_res), (any_free, _) = frames['cells'], frames['tiles'], frames['any']
        differ = [(c, r) for r in range(H) for c in range(W) if px(cells, c, r) != px(tiles, c, r)]
        # LOW's gaps (a pixel either side) left of 32 um: 77-80 px of every 80
        gaps = {(c, r) for r in range(H) for c in range(320) if c % 80 >= 76 or (H - 1 - r) % 80 >= 76}
        assert differ and set(differ) <= gaps, 'by cells the frame differs off the gaps: %d px, %d in them' % (len(differ), len(set(differ) & gaps))
        lower = (cells_res['density_stack']['lower'], tiles_res['density_stack']['lower'])
        assert lower[0] < lower[1] and cells_res['density_stack']['top'] == tiles_res['density_stack']['top'], (cells_res['density_stack'], tiles_res['density_stack'])
        assert any_free == tiles, 'every free cell draws otherwise than the tile boxes in %d px' % sum(
            1 for i in range(0, len(tiles), 4) if any_free[i:i + 4] != tiles[i:i + 4])
        p_cells, p_tiles = cells_res['density_plan2'], tiles_res['density_plan2']
        assert (p_cells['passes'], p_tiles['passes']) == (2, 1) and p_cells['items'] < p_tiles['items'], (p_cells, p_tiles)
        # the topmost layer on one no cell holds (ALONE named and empty - the
        # routing chip's BOUNDARY, user 2026-10-03: "the density never
        # finishes"; "the topmost of the layers on that has shapes"): the top
        # plane is MID's, the frame the one of LOW and MID alone; the topmost
        # layer on whatever it holds (FLOE_RUST_DENSITY_TOP_HELD=off) plans an
        # empty top plane's side, the frame drawn
        held, held_res = frame(workers['cells'], 2, (LOW, MID, ALONE))
        two, two_res = frame(workers['cells'], 3, (LOW, MID))
        assert held == two and held_res['density_stack'] == two_res['density_stack'] and held_res['density_stack']['top'] > 0, (
            held_res['density_stack'], two_res['density_stack'])
        _, empty_res = frame(workers['top_any'], 2, (LOW, MID, ALONE))
        assert empty_res['density_stack']['top'] == 0 < empty_res['density_stack']['lower'], empty_res['density_stack']

        # the layers a cell holds within the depth (Cache::layers_held): DEEP
        # two levels down is not the top plane at depth 1 (the frame of LOW
        # and MID), it is at depth 2 (its dots, the top plane's)
        def at_depth(w, gen, layers, depth):
            dbu = float(w.cache.meta['dbu'])
            w.submit({'kind': 'render', 'gen': gen, 'scope': 'headless', 'bbox': tuple(v / dbu for v in VIEW), 'view': None,
                      'w': W, 'h': H, 'depth': depth, 'cut_px': 3.0, 'lod': False, 'frames': False, 'labels': False,
                      'abstract': False, 'visible': list(layers), 'frame_format': 'raw', 'thin': 'keep', 'frame_cache': False})
            deadline = time.monotonic() + 300
            while time.monotonic() < deadline:
                res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
                assert res.get('kind') != 'error', res
                if res.get('kind') == 'frame' and res.get('gen') == gen and not res.get('refining'):
                    return bytes(res.pop('rgba')), res
            raise AssertionError('depth frame timeout')

        deep1, deep1_res = at_depth(workers['cells'], 4, (LOW, MID, DEEP), 1)
        plain1, plain1_res = at_depth(workers['cells'], 5, (LOW, MID), 1)
        deep2, deep2_res = at_depth(workers['cells'], 6, (LOW, MID, DEEP), 2)
        plain2, _ = at_depth(workers['cells'], 7, (LOW, MID), 2)
        assert deep1 == plain1 and deep1_res['density_stack'] == plain1_res['density_stack'] and deep1_res['density_stack']['top'] > 0, (
            deep1_res['density_stack'], plain1_res['density_stack'])
        assert deep2 != plain2 and deep2_res['density_stack']['top'] > 0, deep2_res['density_stack']
        _, any1_res = at_depth(workers['top_any'], 4, (LOW, MID, DEEP), 1)
        assert any1_res['density_stack']['top'] == 0, any1_res['density_stack']
        print('density stack: pass 2 by cells - the sides apart (the tile boxes joint), LOW\'s gaps\' lone cells left to the originals: '
              '%d px differ from the tile boxes, all in the gaps, the lower dots %d px against %d; every free cell as the tile boxes; '
              'items %d against %d' % (len(differ), lower[0], lower[1], p_cells['items'], p_tiles['items']))
    finally:
        for w in workers.values():
            w.stop()


def shift_layouts(pan_path, edge_path, origin_path):
    """Pass 2's cells under a pan and at a frame's edge: (pan) LOW over the
    view but 14 channels 0.6-1.4 um wide every 3.7 um, MID's 0.1 um VIA
    every 0.2 um over them, a speck of TOP far left (the top plane); (edge)
    LOW to x 38.4 um, MID's VIAs every 0.2 um from 30 um, the speck;
    (origin) as pan with one channel, x 2.88-3.58 um (7 px across the 3.2 um
    bound of the cells from the world's origin)."""
    import klayout.db as kdb
    for path, edges in ((pan_path, [(1.0 + k * 3.7, 1.6 + k * 3.7 + 0.1 * (k % 9)) for k in range(14)]), (edge_path, None),
                        (origin_path, [(2.88, 3.58)])):
        ly = kdb.Layout()
        ly.dbu = 0.001
        top = ly.create_cell('TOP')
        low = ly.layer(*LOW)
        via = ly.create_cell('VIA')
        via.shapes(ly.layer(*MID)).insert(kdb.DBox(0, 0, 0.1, 0.1))
        if edges:
            lo = -10.0
            for (a, b) in edges:
                top.shapes(low).insert(kdb.DBox(lo, -1.0, a, 21.0))
                lo = b
            top.shapes(low).insert(kdb.DBox(lo, -1.0, 80.0, 21.0))
            top.insert(kdb.DCellInstArray(via.cell_index(), kdb.DTrans(kdb.DVector(-5.0, 0.05)), kdb.DVector(0.2, 0), kdb.DVector(0, 0.2), 300, 100))
        else:
            top.shapes(low).insert(kdb.DBox(-10.0, -1.0, 38.4, 21.0))
            top.insert(kdb.DCellInstArray(via.cell_index(), kdb.DTrans(kdb.DVector(30.05, 0.05)), kdb.DVector(0.2, 0), kdb.DVector(0, 0.2), 100, 100))
        top.shapes(ly.layer(*TOP)).insert(kdb.DBox(-8.0, 10.0, -7.9, 10.1))
        ly.write(str(path))


def shift_checks(temp):
    """Pass 2's cells are the world's, a cell's share by its part in the frame
    (a reviewer, 2026-10-03: cells from a tile's corner kept or dropped one
    channel by where a pan put it - a lower density of 47 px to none - and a
    cell cut by the frame's edge to 1 x 32 px, all free, fell under the 128
    px of a whole one). Channels across the cells, panned by 0, 3, 7 and 11
    px: the lower density and the cells' free pixels the same each time, as
    the tile boxes' (FLOE_RUST_DENSITY_FREE_CELLS=off); the open strip at a
    frame's right edge, 386 and 400 px wide (its last cell 2 and 16 px): the
    lower density the tile boxes'; a pan by a pixel across the world's origin
    (x0 -0.05 and 0.05 um at 0.1 um a pixel - the grid's offset rounded half
    away from zero moved by two, a reviewer on fd4fdcb: 0 against 20 px):
    the one channel's lower density the same, the tile boxes'."""
    pan_src, edge_src, origin_src = Path(temp) / 'pan.oas', Path(temp) / 'edge.oas', Path(temp) / 'origin.oas'
    shift_layouts(pan_src, edge_src, origin_src)
    for src in (pan_src, edge_src, origin_src):
        done = subprocess.run([FLOE2, 'index', str(src)],
                              cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
        assert done.returncode == 0, done.stdout + done.stderr
    env = {'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on'}
    workers = {(src, mode): worker(src, dict(env, **extra)) for src in (pan_src, edge_src, origin_src)
               for mode, extra in (('cells', {}), ('tiles', {'FLOE_RUST_DENSITY_FREE_CELLS': 'off'}))}
    try:
        def view(w, gen, x0, width):
            dbu = float(w.cache.meta['dbu'])
            box = (x0, VIEW[1], x0 + width * PX_UM, VIEW[3])
            w.submit({'kind': 'render', 'gen': gen, 'scope': 'headless', 'bbox': tuple(c / dbu for c in box), 'view': None,
                      'w': width, 'h': H, 'depth': None, 'cut_px': 3.0, 'lod': False, 'frames': False, 'labels': False,
                      'abstract': False, 'visible': [LOW, MID, TOP], 'frame_format': 'raw', 'thin': 'keep', 'frame_cache': False})
            deadline = time.monotonic() + 300
            while time.monotonic() < deadline:
                res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
                assert res.get('kind') != 'error', res
                if res.get('kind') == 'frame' and res.get('gen') == gen and not res.get('refining'):
                    return res
            raise AssertionError('shift frame timeout')

        pans = {}
        for mode in ('cells', 'tiles'):
            pans[mode] = [view(workers[(pan_src, mode)], gen, -shift * PX_UM, W) for gen, shift in enumerate((0, 3, 7, 11), 1)]
        lower = {mode: [res['density_stack']['lower'] for res in found] for mode, found in pans.items()}
        free = [res['density_plan2']['free_others'] for res in pans['cells']]
        assert len(set(lower['cells'])) == 1 and len(set(free)) == 1 and lower['cells'] == lower['tiles'] and lower['cells'][0] > 0, (lower, free)
        edge = {mode: [view(workers[(edge_src, mode)], gen, 0.0, width)['density_stack']['lower'] for gen, width in ((1, 386), (2, 400))]
                for mode in ('cells', 'tiles')}
        assert edge['cells'] == edge['tiles'] and edge['cells'][0] > 0, edge
        origin = {mode: [view(workers[(origin_src, mode)], gen, x0, W)['density_stack']['lower'] for gen, x0 in ((1, -0.05), (2, 0.05))]
                  for mode in ('cells', 'tiles')}
        assert origin['cells'] == origin['tiles'] and origin['cells'][0] > 0, origin
        print('density stack: pass 2\'s cells the world\'s - panned 0/3/7/11 px the lower density %d px and the cells\' free %d px each '
              'time, as the tile boxes; a frame\'s edge cell by its part: %s px lower density at 386 / 400 px wide, as the tile boxes; '
              'a pixel\'s pan across the origin %s px, as the tile boxes' % (
                  lower['cells'][0], free[0], ' / '.join(str(n) for n in edge['cells']), ' / '.join(str(n) for n in origin['cells'])))
    finally:
        for w in workers.values():
            w.stop()


def zoom_layout(path):
    """A 40 x 20 um die of 3,000 0.1 um VIAs on MID at random (a point list:
    the dots under the cut), a speck of TOP (the top plane)."""
    import random
    import klayout.db as kdb
    ly = kdb.Layout()
    ly.dbu = 0.001
    top = ly.create_cell('TOP')
    via = ly.create_cell('VIA')
    via.shapes(ly.layer(*MID)).insert(kdb.DBox(0, 0, 0.1, 0.1))
    rnd = random.Random(21)
    for _ in range(3000):
        top.insert(kdb.DCellInstArray(via.cell_index(), kdb.DTrans(kdb.DVector(rnd.randrange(39_800) / 1000.0, rnd.randrange(19_800) / 1000.0))))
    top.shapes(ly.layer(*TOP)).insert(kdb.DBox(0.0, 0.0, 0.1, 0.1))
    options = kdb.SaveLayoutOptions()
    options.format = 'OASIS'
    options.oasis_compression_level = 10
    ly.write(str(path), options)


def zoom_out_checks(temp):
    """Zoomed out past the viewer's fit view of the die, the dots thin by the
    fit's scale over the frame's (user 2026-10-04: "how about drawing it
    sparser the more it is zoomed out"; renderd density_zoom_gain): at the
    fit view a gain of 1; two times out (0.2 um a pixel) the fit's scale -
    the die over the viewport, and 5 % - over the frame's, about 0.52, the
    dots that many of the kill switch's (FLOE_RUST_DENSITY_ZOOM_OUT=off);
    the margin around that view (the viewport's `vw`/`vh` sent) draws the
    view as the viewport frame did. The other checks run with the thinning
    off (main): their VIEW is wider than their dies."""
    src = Path(temp) / 'zoom.oas'
    zoom_layout(src)
    done = subprocess.run([FLOE2, 'index', str(src)],
                          cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
    assert done.returncode == 0, done.stdout + done.stderr
    env = {'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on'}
    workers = {'on': worker(src, dict(env, FLOE_RUST_DENSITY_ZOOM_OUT='on')), 'off': worker(src, dict(env, FLOE_RUST_DENSITY_ZOOM_OUT='off'))}
    try:
        dbu = float(workers['on'].cache.meta['dbu'])
        die = [v * dbu for v in workers['on'].cache.meta['bbox']]
        cx, cy = (die[0] + die[2]) / 2, (die[1] + die[3]) / 2
        layers = [MID, TOP]

        def view(w, gen, spp, margin=False):
            box = (cx - W * spp / 2, cy - H * spp / 2, cx + W * spp / 2, cy + H * spp / 2)
            job = {'kind': 'render', 'gen': gen, 'scope': 'live', 'bbox': tuple(v / dbu for v in box), 'view': None, 'w': W, 'h': H,
                   'depth': None, 'cut_px': 3.0, 'lod': False, 'frames': False, 'labels': False, 'abstract': False, 'visible': layers,
                   'frame_format': 'raw', 'thin': 'keep', 'frame_cache': False}
            if margin:
                bw, bh = box[2] - box[0], box[3] - box[1]
                big = (box[0] - bw / 2, box[1] - bh / 2, box[2] + bw / 2, box[3] + bh / 2)
                job.update(bg=True, bbox=tuple(v / dbu for v in big), view=tuple(v / dbu for v in box), w=2 * W, h=2 * H)
            w.submit(job)
            deadline = time.monotonic() + 300
            while time.monotonic() < deadline:
                res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
                assert res.get('kind') not in ('error', 'dropped'), res
                if res.get('kind') == 'frame' and res.get('gen') == gen and not res.get('refining'):
                    return bytes(res.pop('rgba')), res
            raise AssertionError('zoom frame timeout')

        fit = max((die[2] - die[0]) / W, (die[3] - die[1]) / H) * 1.05
        _, at_fit = view(workers['on'], 1, fit)
        assert at_fit['density_plan2']['dot_gain_milli'] == 1000, at_fit['density_plan2']
        spp = 0.2
        out, out_res = view(workers['on'], 2, spp)
        _, off_res = view(workers['off'], 2, spp)
        gain = out_res['density_plan2']['dot_gain_milli']
        assert gain == round(1000 * fit / spp) and off_res['density_plan2']['dot_gain_milli'] == 1000, (out_res['density_plan2'], fit)
        dots, all_dots = out_res['density_stack']['lit'], off_res['density_stack']['lit']
        assert all_dots > 0 and abs(dots - all_dots * gain / 1000) <= max(3, 0.12 * all_dots), (dots, all_dots, gain)
        margin, margin_res = view(workers['on'], 3, spp, margin=True)
        assert margin_res['density_plan2']['dot_gain_milli'] == gain, margin_res['density_plan2']
        centre = b''.join(margin[((H // 2 + r) * 2 * W + W // 2) * 4:((H // 2 + r) * 2 * W + W // 2 + W) * 4] for r in range(H))
        assert centre == out, 'the zoomed-out margin draws the view otherwise in %d px' % sum(
            1 for i in range(0, len(out), 4) if centre[i:i + 4] != out[i:i + 4])
        print('density stack: zoomed out past the fit view the dots thin - gain 1 at the fit, %.3f two times out: %d px of dots against '
              '%d (the switch off); the margin (vw/vh) draws the view alike' % (gain / 1000, dots, all_dots))
    finally:
        for w in workers.values():
            w.stop()


def gate_layout(path):
    """MID's own shapes, 0.05 um squares under every floor: 4,000 at random
    over the left 40 x 40 um (0.6 % of it) and a 20 x 20 um field of them at
    0.1 um (25 %) on the right."""
    import random
    import klayout.db as kdb
    ly = kdb.Layout()
    ly.dbu = 0.001
    top = ly.create_cell('TOP')
    mid = ly.layer(*MID)
    rnd = random.Random(11)
    for _ in range(4000):
        x, y = rnd.randrange(40_000) / 1000.0, rnd.randrange(40_000) / 1000.0
        top.shapes(mid).insert(kdb.DBox(x, y, x + 0.05, y + 0.05))
    for i in range(200):
        for j in range(200):
            x, y = 50 + i * 0.1, 10 + j * 0.1
            top.shapes(mid).insert(kdb.DBox(x, y, x + 0.05, y + 0.05))
    options = kdb.SaveLayoutOptions()
    options.format = 'OASIS'
    options.oasis_compression_level = 10
    ly.write(str(path), options)


def gate_checks(temp):
    """A dot block too sparse for the detail is left out (user 2026-10-04:
    "a pixel should light only when the shapes' size in it passes a level";
    floe_vfs HierOpts::dot_gate): MID's squares at 0.4 um a pixel, all under
    the floor and spread over their page's occupancy (the user's case: the
    routing chip zoomed out past where its wires were decoded). At medium's
    cut (3 px) a 4 px block needs 2 dots: the 0.6 % half lights nothing, the
    25 % field its dots; the kill switch (FLOE_RUST_DENSITY_GATE=off) lights
    the sparse half too, the field alike; high's cut (1 px) gates nothing."""
    src = Path(temp) / 'gate.oas'
    gate_layout(src)
    done = subprocess.run([FLOE2, 'index', str(src)],
                          cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
    assert done.returncode == 0, done.stdout + done.stderr
    env = {'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on'}
    workers = {'on': worker(src, dict(env, FLOE_RUST_DENSITY_GATE='on')), 'off': worker(src, dict(env, FLOE_RUST_DENSITY_GATE='off'))}
    try:
        dbu = float(workers['on'].cache.meta['dbu'])
        spp = 0.4
        x0, y0 = -10.0, -20.0

        def view(w, gen, cut_px):
            box = (x0, y0, x0 + W * spp, y0 + H * spp)
            w.submit({'kind': 'render', 'gen': gen, 'scope': 'live', 'bbox': tuple(v / dbu for v in box), 'view': None, 'w': W, 'h': H,
                      'depth': None, 'cut_px': cut_px, 'lod': False, 'frames': False, 'labels': False, 'abstract': False, 'visible': [MID],
                      'frame_format': 'raw', 'thin': 'keep', 'frame_cache': False})
            deadline = time.monotonic() + 300
            while time.monotonic() < deadline:
                res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
                assert res.get('kind') not in ('error', 'dropped'), res
                if res.get('kind') == 'frame' and res.get('gen') == gen and not res.get('refining'):
                    return bytes(res.pop('rgba')), res
            raise AssertionError('gate frame timeout')

        def cols(um0, um1):
            return range(max(0, int((um0 - x0) / spp)), min(W, int((um1 - x0) / spp)))

        def rows(um0, um1):
            # frame rows run down from the view's top
            return range(max(0, int((y0 + H * spp - um1) / spp)), min(H, int((y0 + H * spp - um0) / spp)))

        sparse, field = (cols(0, 40), rows(0, 40)), (cols(50, 70), rows(10, 30))
        on, on_res = view(workers['on'], 1, 3.0)
        off, off_res = view(workers['off'], 1, 3.0)
        high, high_res = view(workers['on'], 2, 1.0)
        plan2, pages = on_res['density_plan2'], on_res['density_pages']
        # the squares spread over their page's occupancy, none decoded
        assert plan2['occ_pages'] >= 1 and pages['decoded'] == 0, (plan2, pages)
        assert plan2['dot_gate_min'] == 2 and plan2['dot_gated'] > 0, plan2
        assert off_res['density_plan2']['dot_gate_min'] == 1 and high_res['density_plan2']['dot_gate_min'] == 1, (off_res['density_plan2'], high_res['density_plan2'])
        lit_on, lit_off, lit_high = (len(lit(f, *sparse)) for f in (on, off, high))
        assert lit_on == 0 and lit_off > 0 and lit_high > 0, ('the sparse half', lit_on, lit_off, lit_high)
        field_on, field_off = len(lit(on, *field)), len(lit(off, *field))
        n = len(field[0]) * len(field[1])
        assert field_on >= 0.15 * n and field_on >= 0.9 * field_off, ('the field', field_on, field_off, n)
        print('density stack: a dot block too sparse for the detail is left out - medium (2 dots of 16 px): the 0.6 %% half %d px '
              '(the switch off %d, high %d), the 25 %% field %d px of %d (off %d); %d blocks out'
              % (lit_on, lit_off, lit_high, field_on, n, field_off, plan2['dot_gated']))
    finally:
        for w in workers.values():
            w.stop()


def bright_checks(temp):
    """The density's brightness (user 2026-10-05: "the brightness of a pixel
    by the shapes' size that gathers on it", "never brighter than the original
    colour", then "g = 1, 2, 4"; renderd density_bright_gain,
    FLOE_RUST_DENSITY_BRIGHT=off the kill switch): gate_layout's MID squares
    at 0.4 um a pixel, under every floor (0.016 px^2 each). A pixel shows
    min(1, g x its covered area) of MID's colour - every lit pixel a multiple
    of the colour, none past it - g by the detail: low (5 px) 1, medium (3 px)
    2, high (1 px) 4. The 0.6 % half's alphas add up to g x its squares' area
    (62.5 px^2) - nothing left out as too sparse, as the dots' gate does -
    and the 25 % field shows a quarter, a half and the colour itself; two
    steps out (0.8 um a pixel) the half's alphas are a quarter of the fit's,
    not thinned past it. Pass 1's shapes stay: shapes_first_layout's LOW
    original is the same pixels with the brightness on and off, TOP's density
    where LOW alone leaves the frame dark. The switch off: the dots as lit
    pixels, the gate's frame."""
    src = Path(temp) / 'bright.oas'
    gate_layout(src)
    done = subprocess.run([FLOE2, 'index', str(src)],
                          cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
    assert done.returncode == 0, done.stdout + done.stderr
    env = {'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on', 'FLOE_RUST_DENSITY_GATE': 'on', 'FLOE_RUST_DENSITY_ZOOM_OUT': 'on',
           'FLOE_RUST_DENSITY_PATTERN': 'off'}
    workers = {'on': worker(src, dict(env, FLOE_RUST_DENSITY_BRIGHT='on')), 'off': worker(src, dict(env, FLOE_RUST_DENSITY_BRIGHT='off'))}
    try:
        dbu = float(workers['on'].cache.meta['dbu'])
        colour = layer_colour(workers['on'], MID)
        x0, y0 = -10.0, -20.0

        def view(w, gen, cut_px, spp):
            box = (x0, y0, x0 + W * spp, y0 + H * spp)
            w.submit({'kind': 'render', 'gen': gen, 'scope': 'live', 'bbox': tuple(v / dbu for v in box), 'view': None, 'w': W, 'h': H,
                      'depth': None, 'cut_px': cut_px, 'lod': False, 'frames': False, 'labels': False, 'abstract': False, 'visible': [MID],
                      'frame_format': 'raw', 'thin': 'keep', 'frame_cache': False})
            deadline = time.monotonic() + 300
            while time.monotonic() < deadline:
                res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
                assert res.get('kind') not in ('error', 'dropped'), res
                if res.get('kind') == 'frame' and res.get('gen') == gen and not res.get('refining'):
                    return bytes(res.pop('rgba')), res
            raise AssertionError('bright frame timeout')

        def part(spp, um):
            (ux0, uy0, ux1, uy1) = um
            cs = range(max(0, int((ux0 - x0) / spp)), min(W, int((ux1 - x0) / spp)))
            rs = range(max(0, int((y0 + H * spp - uy1) / spp)), min(H, int((y0 + H * spp - uy0) / spp)))
            return cs, rs

        # the channel MID's colour is brightest in: a pixel's alpha
        k = max(range(3), key=lambda i: colour[i])

        def alphas(pixels, cs, rs):
            out = []
            for r in rs:
                for c in cs:
                    p = px(pixels, c, r)
                    for i in range(3):
                        # never past the colour, and of its hue
                        assert p[i] <= colour[i], ('past the colour', (c, r), tuple(p), tuple(colour))
                        assert abs(p[i] - colour[i] * p[k] / colour[k]) <= 1.0, ('another hue', (c, r), tuple(p), tuple(colour))
                    out.append(p[k] / colour[k])
            return out

        gen, sums = 0, {}
        sparse_area = 4000 * (0.05 / 0.4) ** 2
        for cut, g in ((5.0, 1), (3.0, 2), (1.0, 4)):
            gen += 1
            on, res = view(workers['on'], gen, cut, 0.4)
            assert res['density_plan2']['dot_gated'] == 0, res['density_plan2']
            sparse, field = alphas(on, *part(0.4, (0, 0, 40, 40))), alphas(on, *part(0.4, (50.5, 10.5, 69.5, 29.5)))
            sums[g] = sum(sparse)
            assert abs(sums[g] - g * sparse_area) < 0.15 * g * sparse_area, ('the sparse half', g, sums[g], g * sparse_area)
            mean = sum(field) / len(field)
            assert abs(mean - min(1.0, g * 0.25)) < 0.06, ('the field', g, mean)
            print('density stack: the brightness, g %d (cut %g px) - the 0.6 %% half alphas %.1f for %.1f px^2 x g, the 25 %% field %.2f of the colour, '
                  'none past it' % (g, cut, sums[g], sparse_area, mean))
        # two steps out: a quarter of the area in px^2, not thinned past it
        gen += 1
        out, _ = view(workers['on'], gen, 3.0, 0.8)
        far = sum(alphas(out, *part(0.8, (0, 0, 40, 40))))
        assert abs(far - sums[2] / 4) < 0.2 * sums[2] / 4, ('two steps out', far, sums[2] / 4)
        # the switch off: the dots, the gate leaves the sparse half out
        gen += 1
        off, off_res = view(workers['off'], gen, 3.0, 0.4)
        assert not lit(off, *part(0.4, (0, 0, 40, 40))) and off_res['density_plan2']['dot_gated'] > 0, off_res['density_plan2']
        assert all(px(off, c, r) in (BLACK, colour) for r in range(H) for c in range(W)), 'the switch off: lit pixels in the colour'
        print('density stack: the brightness two steps out - the half %.1f (a quarter of %.1f); the switch off: the dots, the half gated' % (far, sums[2]))
    finally:
        for w in workers.values():
            w.stop()
    # pass 1's shapes stay
    src = Path(temp) / 'bright_first.oas'
    shapes_first_layout(src)
    done = subprocess.run([FLOE2, 'index', str(src)],
                          cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
    assert done.returncode == 0, done.stdout + done.stderr
    env = {'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on', 'FLOE_RUST_DENSITY_TOP_GROUP': 'on', 'FLOE_RUST_DENSITY_SHAPES_FIRST': 'on',
           'FLOE_RUST_DENSITY_PATTERN': 'off'}
    workers = {'on': worker(src, dict(env, FLOE_RUST_DENSITY_BRIGHT='on')), 'off': worker(src, dict(env, FLOE_RUST_DENSITY_BRIGHT='off'))}
    try:
        low_alone = frame(workers['on'], 1, (LOW,))[0]
        on = frame(workers['on'], 2, (LOW, TOP))[0]
        off = frame(workers['off'], 2, (LOW, TOP))[0]
        low_px = {(c, r) for r in range(H) for c in range(W) if px(low_alone, c, r) != BLACK}
        assert low_px and all(px(on, c, r) == px(off, c, r) == px(low_alone, c, r) for (c, r) in low_px), 'pass 1 changed'
        shown = {(c, r) for r in range(H) for c in range(W) if px(on, c, r) != BLACK} - low_px
        assert shown, 'TOP\'s density where LOW leaves the frame dark'
        print('density stack: the brightness keeps pass 1 - LOW\'s %d px as with it off and alone, TOP\'s density on %d px around it' % (len(low_px), len(shown)))
    finally:
        for w in workers.values():
            w.stop()


def pattern_checks(temp):
    """The default density display uses opaque layer-colour dots. Its switch
    off restores accumulated brightness, with the same originals and plans.
    A speckled original's holes remain protected, and tile size or raster
    worker count cannot change the pattern.
    """
    src = Path(temp) / 'pattern.oas'
    shapes_first_layout(src)
    done = subprocess.run([FLOE2, 'index', str(src)],
                          cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
    assert done.returncode == 0, done.stdout + done.stderr
    env = {
        'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on',
        'FLOE_RUST_DENSITY_BRIGHT': 'on', 'FLOE_RUST_DENSITY_PATTERN': None,
        'FLOE_RUST_DENSITY_TOP_GROUP': 'on', 'FLOE_RUST_DENSITY_SHAPES_FIRST': 'on',
        # This check isolates the display mode; the evolving-mask planner
        # is tested separately by staged_density_checks.
        'FLOE_RUST_DENSITY_STAGES': 'off',
        'FLOE_RUST_TILE_PX': '64', 'FLOE_RUST_RASTER_JOBS': '1',
    }
    workers = {
        'default': worker(src, env),
        'on': worker(src, dict(env, FLOE_RUST_DENSITY_PATTERN='on')),
        'off': worker(src, dict(env, FLOE_RUST_DENSITY_PATTERN='off')),
        'parallel': worker(src, dict(env, FLOE_RUST_TILE_PX='127', FLOE_RUST_RASTER_JOBS='4')),
    }
    try:
        low_c, top_c = layer_colour(workers['default'], LOW), layer_colour(workers['default'], TOP)
        frames = {name: frame(w, 1, (LOW, TOP)) for name, w in workers.items()}
        on, result = frames['default']
        legacy, legacy_result = frames['off']
        for name, (_, reported) in frames.items():
            assert reported['density_plan2']['pattern'] == int(name != 'off'), (name, reported['density_plan2'])
        assert on == frames['on'][0] == frames['parallel'][0], 'default, explicit on, or tile/worker pattern differs'
        assert on != legacy, 'pattern switch must restore the legacy brightness'
        palette = (BLACK, low_c, top_c)
        assert all(on[i:i + 4] in palette for i in range(0, len(on), 4)), 'pattern contains blended colours'
        assert any(legacy[i:i + 4] not in palette for i in range(0, len(legacy), 4)), 'legacy brightness needs fractional coverage'
        alone, _ = frame(workers['default'], 2, (LOW,))
        covered = ((c, r) for r in range(20, 180) for c in range(20, 200))
        assert all(px(on, c, r) == px(legacy, c, r) == px(alone, c, r) for c, r in covered), 'original or its speckle holes changed'
        shown = sum(on[i:i + 4] == top_c for i in range(0, len(on), 4))
        assert shown > 0 and result['density_stack']['lit'] > 0, 'pattern lost the open-space density'
        assert result['density_pages'] == legacy_result['density_pages'], ('pattern changed page planning', result['density_pages'], legacy_result['density_pages'])
        print('density stack: opaque pattern default/on, legacy brightness off, originals protected; %d density px, identical across tiles/workers' % shown)
    finally:
        for w in workers.values():
            w.stop()


def toggle_checks(temp):
    """The viewer's density toggle (user 2026-10-05: "a density on/off option
    in the viewer"; a render command's density=on|off, renderd
    density_stack_on / density_dots_on): gate_layout's MID squares at 0.4 um
    a pixel. A worker without FLOE_RUST_DENSITY_STACK draws with density=on
    what a FLOE_RUST_DENSITY_STACK=top FLOE_RUST_DENSITY_DOTS=on worker draws
    without the field, byte for byte, and that worker with density=off what
    the first draws without it. The setting is part of a retained frame's
    identity: at one view, off, then on (the `v` key), then off again - the
    second frame draws the density as a fresh worker does (a frame retained
    without it covering the view would have served it whole), the third the
    first's pixels."""
    src = Path(temp) / 'toggle.oas'
    gate_layout(src)
    done = subprocess.run([FLOE2, 'index', str(src)],
                          cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
    assert done.returncode == 0, done.stdout + done.stderr
    retained = {'FLOE_RUST_RETAINED_MB': '256'}
    workers = {
        'plain': worker(src, dict(retained)),
        'env': worker(src, dict(retained, FLOE_RUST_DENSITY_STACK='top', FLOE_RUST_DENSITY_DOTS='on')),
        'fresh': worker(src, dict(retained)),
    }
    try:
        dbu = float(workers['plain'].cache.meta['dbu'])
        spp, x0, y0 = 0.4, -10.0, -20.0

        def view(w, gen, density):
            box = (x0, y0, x0 + W * spp, y0 + H * spp)
            job = {'kind': 'render', 'gen': gen, 'scope': 'live', 'bbox': tuple(v / dbu for v in box), 'view': None, 'w': W, 'h': H,
                   'depth': None, 'cut_px': 3.0, 'lod': False, 'frames': False, 'labels': False, 'abstract': False, 'visible': [MID],
                   'frame_format': 'raw', 'thin': 'keep', 'frame_cache': True}
            if density is not None:
                job['density'] = density
            w.submit(job)
            deadline = time.monotonic() + 300
            while time.monotonic() < deadline:
                res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
                assert res.get('kind') not in ('error', 'dropped'), res
                if res.get('kind') == 'frame' and res.get('gen') == gen and not res.get('refining'):
                    return bytes(res.pop('rgba')), res
            raise AssertionError('toggle frame timeout')

        on, on_res = view(workers['plain'], 1, True)
        env_on, _ = view(workers['env'], 1, None)
        assert on_res.get('density_stack') and on == env_on, ('density=on as the environment\'s', on_res.get('density_stack'))
        off, off_res = view(workers['env'], 2, False)
        plain, _ = view(workers['plain'], 2, None)
        assert off_res.get('density_stack') is None and off == plain, ('density=off as without the environment', off_res.get('density_stack'))
        assert len(lit(on, range(W), range(H))) > len(lit(off, range(W), range(H))), 'the density shows'
        # one view: off, on, off - the setting is part of the retained frame's identity
        first, _ = view(workers['fresh'], 1, False)
        toggled, toggled_res = view(workers['fresh'], 2, True)
        again, again_res = view(workers['fresh'], 3, False)
        assert toggled == on and toggled_res.get('density_stack'), 'the toggle at one view: not the retained frame without the density'
        assert again == first and again_res.get('density_stack') is None, 'back off: the frame without it'
        print('density stack: the viewer\'s toggle - density=on draws as FLOE_RUST_DENSITY_STACK=top + DOTS=on (%d px lit), density=off as '
              'without them (%d px); at one view off -> on -> off the retained frames keep to their setting (reused %d / %d tiles)'
              % (len(lit(on, range(W), range(H))), len(lit(off, range(W), range(H))), toggled_res.get('tiles_reused', 0), again_res.get('tiles_reused', 0)))
    finally:
        for w in workers.values():
            w.stop()


def first_layout(path):
    """MID's own shapes, 0.6 um squares - 1.5 px at 0.4 um a pixel: under the
    3 px cut, over the 1 px density cut: 1,000 at random over the left 60 x 60
    um (about a tenth of it), a 20 x 20 um field of them every 1.2 um (a
    quarter) on the right, and 16,000 more over the strip above both (y 62-78
    um) - the page (one point list) past a 128 KB reserve."""
    import random
    import klayout.db as kdb
    ly = kdb.Layout()
    ly.dbu = 0.001
    top = ly.create_cell('TOP')
    mid = ly.layer(*MID)
    rnd = random.Random(17)
    for _ in range(1000):
        x, y = rnd.randrange(59_000) / 1000.0, rnd.randrange(59_000) / 1000.0
        top.shapes(mid).insert(kdb.DBox(x, y, x + 0.6, y + 0.6))
    for i in range(16):
        for j in range(16):
            x, y = 70 + i * 1.2, 10 + j * 1.2
            top.shapes(mid).insert(kdb.DBox(x, y, x + 0.6, y + 0.6))
    for _ in range(16000):
        x, y = rnd.randrange(88_000) / 1000.0, 62 + rnd.randrange(16_000) / 1000.0
        top.shapes(mid).insert(kdb.DBox(x, y, x + 0.6, y + 0.6))
    options = kdb.SaveLayoutOptions()
    options.format = 'OASIS'
    options.oasis_compression_level = 10
    ly.write(str(path), options)


def first_checks(temp):
    """The occupancy first and the stand-in for a page a budget leaves out (a
    reviewer 2026-10-05, then the exact cover: on the routing chip's fit view
    pass 2 kept 110 of the 277 pages it decodes from 1 px up under 1 GB and
    drew x0.49 of them; their occupancy grids draw x1.01 in a third of the
    time). first_layout's 1.5 px squares at 0.4 um a pixel under the
    brightness: by default (renderd density_ovb_first) pass 2 decodes no page
    - the page is spread by its grid, whose cells show 3 px - and the kill
    switch (FLOE_RUST_DENSITY_OVB_FIRST=off) decodes it. Spread, the alphas
    add up to g x the squares' area and the field shows a half of the colour;
    decoded, each square is drawn where it is and a pixel it covers stops at
    the colour (min(1, g x cover) a pixel: about 3.3 of the 4.5 a 1.5 px
    square's area makes), so the sum is less - the two agree by 16 px cells
    within that. Under a reserve that holds no page (1 MB, the
    fixed 128 KB) with the page decoded as before, its grid stands in (floe_vfs
    HierOpts::dot_stand_in: density_plan2 stood_in) and draws that area too;
    FLOE_RUST_DENSITY_STAND_IN=off draws nothing, as 0.12.298."""
    src = Path(temp) / 'first.oas'
    first_layout(src)
    done = subprocess.run([FLOE2, 'index', str(src)],
                          cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
    assert done.returncode == 0, done.stdout + done.stderr
    env = {'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on', 'FLOE_RUST_DENSITY_BRIGHT': 'on',
           'FLOE_RUST_DENSITY_PATTERN': 'off'}
    tight = dict(env, FLOE_RUST_BUDGET_MB='1', FLOE_RUST_DENSITY_RESERVE_LEFT='off', FLOE_RUST_DENSITY_OVB_FIRST='off')
    workers = {'first': worker(src, env), 'decode': worker(src, dict(env, FLOE_RUST_DENSITY_OVB_FIRST='off')),
               'stand_in': worker(src, tight), 'none': worker(src, dict(tight, FLOE_RUST_DENSITY_STAND_IN='off'))}
    try:
        dbu = float(workers['first'].cache.meta['dbu'])
        colour = layer_colour(workers['first'], MID)
        k = max(range(3), key=lambda i: colour[i])
        spp, x0, y0 = 0.4, -10.0, -10.0

        def view(w):
            box = (x0, y0, x0 + W * spp, y0 + H * spp)
            w.submit({'kind': 'render', 'gen': 1, 'scope': 'live', 'bbox': tuple(v / dbu for v in box), 'view': None, 'w': W, 'h': H,
                      'depth': None, 'cut_px': 3.0, 'lod': False, 'frames': False, 'labels': False, 'abstract': False, 'visible': [MID],
                      'frame_format': 'raw', 'thin': 'keep', 'frame_cache': False})
            deadline = time.monotonic() + 300
            while time.monotonic() < deadline:
                res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
                assert res.get('kind') not in ('error', 'dropped'), res
                if res.get('kind') == 'frame' and res.get('gen') == 1 and not res.get('refining'):
                    return bytes(res.pop('rgba')), res
            raise AssertionError('first frame timeout')

        def alpha(pixels, c, r):
            p = px(pixels, c, r)
            assert all(p[i] <= colour[i] for i in range(3)), ('past the colour', (c, r), tuple(p))
            return p[k] / colour[k]

        def total(pixels, um):
            cs = range(max(0, int((um[0] - x0) / spp)), min(W, int((um[2] - x0) / spp)))
            rs = range(max(0, int((y0 + H * spp - um[3]) / spp)), min(H, int((y0 + H * spp - um[1]) / spp)))
            return sum(alpha(pixels, c, r) for r in rs for c in cs), len(cs) * len(rs)

        frames = {name: view(w) for name, w in workers.items()}
        pages = {name: res.get('density_pages') or {} for name, (_, res) in frames.items()}
        plan2 = {name: res.get('density_plan2') or {} for name, (_, res) in frames.items()}
        sparse, field = (-1, -1, 61, 61), (70.2, 10.2, 88.8, 28.8)
        # the squares' area on screen, px^2 (overlaps counted twice: about 2 %), and g = 2
        want = 2 * 1000 * (0.6 / spp) ** 2
        got = {name: total(pixels, sparse)[0] for name, (pixels, _) in frames.items()}
        assert pages['first']['decoded'] == 0 and plan2['first']['occ_pages'] >= 1, (pages['first'], plan2['first'])
        assert pages['decode']['decoded'] >= 1, pages['decode']
        # spread: the area itself; decoded: each pixel a square covers stops at
        # the colour - 3.3 to 3.5 of the 4.5 a square's area makes at g = 2
        assert abs(got['first'] - want) < 0.15 * want, ('spread', got['first'], want)
        assert 0.62 * want < got['decode'] < 0.85 * want, ('decoded', got['decode'], want)
        fields = {name: total(frames[name][0], field) for name in ('first', 'decode')}
        assert abs(fields['first'][0] / fields['first'][1] - 0.5) < 0.08, ('the field, spread', fields['first'][0] / fields['first'][1])
        assert 0.3 < fields['decode'][0] / fields['decode'][1] < 0.45, ('the field, decoded', fields['decode'][0] / fields['decode'][1])
        # by 16 px cells the two agree within that: the mean difference under
        # two fifths of the mean
        cells = [(c, r) for r in range(0, H - 15, 16) for c in range(0, W - 15, 16)]
        sums = {name: [sum(alpha(frames[name][0], c + i, r + j) for j in range(16) for i in range(16)) for (c, r) in cells] for name in ('first', 'decode')}
        mean = sum(sums['decode']) / len(cells)
        differ = sum(abs(a - b) for a, b in zip(sums['first'], sums['decode'])) / len(cells)
        assert mean > 0 and differ < 0.4 * mean, ('16 px cells', differ, mean)
        # the page left out: its grid stands in, or nothing does
        assert plan2['stand_in']['stood_in'] >= 1 and pages['stand_in']['decoded'] == 0, (plan2['stand_in'], pages['stand_in'])
        assert abs(got['stand_in'] - want) < 0.15 * want, ('the stand-in', got['stand_in'], want)
        assert plan2['none']['stood_in'] == 0 and got['none'] == 0 and not lit(frames['none'][0], range(W), range(H)), (plan2['none'], got['none'])
        print('density stack: the occupancy first - the 1.5 px squares\' page spread by its grid, no page decoded: alphas %.0f for g x %.0f px^2 '
              '(decoded, the switch off: %.0f; by 16 px cells they differ by %.2f of %.2f); a reserve that holds no page: its grid stands in, '
              '%.0f (%d pages) - the stand-in off: nothing'
              % (got['first'], want / 2, got['decode'], differ, mean, got['stand_in'], plan2['stand_in']['stood_in']))
    finally:
        for w in workers.values():
            w.stop()


def sums_layout(single_path, list_path):
    """2,000 cells of 0.32 um (0.32 px at 1 um a pixel: 0.1024 px^2, 1.64
    sixteenths) at random over 990 x 190 um: each its own cell placed once
    (lone placements), and one cell placed 2,000 times (KLayout writes a point
    list)."""
    import random
    import klayout.db as kdb
    for path, lone in ((single_path, True), (list_path, False)):
        ly = kdb.Layout()
        ly.dbu = 0.001
        top = ly.create_cell('TOP')
        mid = ly.layer(*MID)
        rnd = random.Random(5)
        leaf = None
        for n in range(2000):
            if lone or leaf is None:
                leaf = ly.create_cell('L%d' % n)
                leaf.shapes(mid).insert(kdb.Box(0, 0, 320, 320))
            top.insert(kdb.CellInstArray(leaf.cell_index(), kdb.Trans(rnd.randrange(390_000), rnd.randrange(190_000))))
        ly.write(str(path))


def sums_checks(temp):
    """Under the brightness an item's cover keeps its fraction (a reviewer
    2026-10-05: "0.1 px^2 is 1.6 sixteenths, cut to 1"; floe_vfs
    HierOpts::dot_bright_sums, FLOE_RUST_DENSITY_BRIGHT_SUMS=off the kill
    switch). sums_layout's 2,000 cells of 0.1024 px^2 at 1 um a pixel, g = 2:
    as lone placements their alphas add up to g x their area (410) - the
    switch off 0.61 of it, each cut to a sixteenth; as one point list read one
    member for a window, each window stands over its block's part of the
    chunk - the area again, no pixel past half the colour - where the switch
    off gathers it on the member's own pixel (pixels at the colour itself)."""
    single, listed = Path(temp) / 'sums_single.oas', Path(temp) / 'sums_list.oas'
    sums_layout(single, listed)
    for src in (single, listed):
        done = subprocess.run([FLOE2, 'index', str(src)],
                              cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
        assert done.returncode == 0, done.stdout + done.stderr
    env = {'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on', 'FLOE_RUST_DENSITY_BRIGHT': 'on',
           'FLOE_RUST_DENSITY_PATTERN': 'off'}
    off = dict(env, FLOE_RUST_DENSITY_BRIGHT_SUMS='off')
    workers = {('single', 'on'): worker(single, env), ('single', 'off'): worker(single, off), ('list', 'on'): worker(listed, env), ('list', 'off'): worker(listed, off)}
    try:
        w0 = workers[('single', 'on')]
        dbu = float(w0.cache.meta['dbu'])
        colour = layer_colour(w0, MID)
        k = max(range(3), key=lambda i: colour[i])

        def view(w):
            box = (0.0, 0.0, float(W), float(H))
            w.submit({'kind': 'render', 'gen': 1, 'scope': 'live', 'bbox': tuple(v / dbu for v in box), 'view': None, 'w': W, 'h': H,
                      'depth': None, 'cut_px': 3.0, 'lod': False, 'frames': False, 'labels': False, 'abstract': False, 'visible': [MID],
                      'frame_format': 'raw', 'thin': 'keep', 'frame_cache': False})
            deadline = time.monotonic() + 300
            while time.monotonic() < deadline:
                res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
                assert res.get('kind') not in ('error', 'dropped'), res
                if res.get('kind') == 'frame' and res.get('gen') == 1 and not res.get('refining'):
                    return bytes(res.pop('rgba')), res
            raise AssertionError('sums frame timeout')

        alphas = {}
        for key, w in workers.items():
            pixels, res = view(w)
            alphas[key] = [px(pixels, c, r)[k] / colour[k] for r in range(H) for c in range(W)]
        want = 2 * 2000 * 0.32 ** 2
        got = {key: sum(a) for key, a in alphas.items()}
        top = {key: max(a) for key, a in alphas.items()}
        assert abs(got[('single', 'on')] - want) < 0.06 * want, ('lone cells', got[('single', 'on')], want)
        assert abs(got[('single', 'off')] - want / 1.6384) < 0.06 * want, ('lone cells, the switch off', got[('single', 'off')], want / 1.6384)
        assert abs(got[('list', 'on')] - want) < 0.06 * want and top[('list', 'on')] <= 0.5, ('the list', got[('list', 'on')], want, top[('list', 'on')])
        assert got[('list', 'off')] < 0.85 * want and top[('list', 'off')] >= 0.99, ('the list, the switch off', got[('list', 'off')], top[('list', 'off')])
        print('density stack: the brightness keeps an item\'s fraction - 2,000 lone cells of 1.64 sixteenths: alphas %.0f for g x area %.0f '
              '(the switch off %.0f); as one point list %.0f, no pixel past %.2f of the colour (off %.0f, pixels at %.2f)'
              % (got[('single', 'on')], want, got[('single', 'off')], got[('list', 'on')], top[('list', 'on')], got[('list', 'off')], top[('list', 'off')]))
    finally:
        for w in workers.values():
            w.stop()


def hier_layouts(flat_path, cells_path, cover_path, even_path):
    """At 1 um a pixel. flat / cells: 160 x 160 squares of 0.05 um (0.0025
    px^2) every 0.25 um from (10, 10) um - 64 px^2 over 40 x 40 px, a 25th of
    it - as TOP's own shapes, and each a cell of its own placed once (256
    placements under a 4 px node: as many as its box has sixteenths). cover:
    a cell of 1 um (a 1/0 box) holding a 2/0 square of 0.2 um, a 25th of it -
    as an array of 60 x 60 from (10, 100), 2,000 at random over 150 x 60 um
    from (100, 10) (a point list) and 500 cells of their own placed once over
    150 x 60 um from (100, 100). even: 48 x 48 cells of their own, a 2/0
    square of 0.5 um each, every 1 um from (10, 10) - a quarter covered - and
    an array of 40 x 40 every 1.5 um from (100, 10) of a cell of 1.2 um (a
    1/0 box) holding a 2/0 square of 0.6 um (two or three of its members a
    side to a 4 px block; 0.36 px^2 of every 2.25 covered)."""
    import random
    import klayout.db as kdb
    for path, lone in ((flat_path, False), (cells_path, True)):
        ly = kdb.Layout()
        ly.dbu = 0.001
        top = ly.create_cell('TOP')
        mid = ly.layer(*MID)
        for j in range(160):
            for i in range(160):
                x, y = 10_000 + i * 250, 10_000 + j * 250
                if lone:
                    leaf = ly.create_cell('S%d_%d' % (i, j))
                    leaf.shapes(mid).insert(kdb.Box(0, 0, 50, 50))
                    top.insert(kdb.CellInstArray(leaf.cell_index(), kdb.Trans(x, y)))
                else:
                    top.shapes(mid).insert(kdb.Box(x, y, x + 50, y + 50))
        ly.write(str(path))
    ly = kdb.Layout()
    ly.dbu = 0.001
    top = ly.create_cell('TOP')
    low, mid = ly.layer(*LOW), ly.layer(*MID)

    def cell(name):
        c = ly.create_cell(name)
        c.shapes(low).insert(kdb.Box(0, 0, 1000, 1000))
        c.shapes(mid).insert(kdb.Box(400, 400, 600, 600))
        return c.cell_index()

    one = cell('C')
    top.insert(kdb.CellInstArray(one, kdb.Trans(10_000, 100_000), kdb.Vector(1000, 0), kdb.Vector(0, 1000), 60, 60))
    rnd = random.Random(7)
    for _ in range(2000):
        top.insert(kdb.CellInstArray(one, kdb.Trans(100_000 + rnd.randrange(149_000), 10_000 + rnd.randrange(59_000))))
    for n in range(500):
        top.insert(kdb.CellInstArray(cell('D%d' % n), kdb.Trans(100_000 + rnd.randrange(149_000), 100_000 + rnd.randrange(59_000))))
    ly.write(str(cover_path))
    ly = kdb.Layout()
    ly.dbu = 0.001
    top = ly.create_cell('TOP')
    mid = ly.layer(*MID)
    for j in range(48):
        for i in range(48):
            leaf = ly.create_cell('E%d_%d' % (i, j))
            leaf.shapes(mid).insert(kdb.Box(0, 0, 500, 500))
            top.insert(kdb.CellInstArray(leaf.cell_index(), kdb.Trans(10_000 + i * 1000, 10_000 + j * 1000)))
    member = ly.create_cell('F')
    member.shapes(ly.layer(*LOW)).insert(kdb.Box(0, 0, 1200, 1200))
    member.shapes(mid).insert(kdb.Box(300, 300, 900, 900))
    top.insert(kdb.CellInstArray(member.cell_index(), kdb.Trans(100_000, 10_000), kdb.Vector(1500, 0), kdb.Vector(0, 1500), 40, 40))
    ly.write(str(even_path))


def hier_checks(temp):
    """The brightness does not depend on the hierarchy the shapes are stored
    in (a reviewer 2026-10-05: 4,096 squares of 1 dbu drew 0 px flat and 121
    px at full colour as a cell each; the standard-cell layout's 2/0 alone
    drew x6.9 of its exact cover). hier_layouts at 1 um a pixel, 2/0 alone,
    g = 2. A node counts what its placements hold (floe_vfs HierOpts::
    dot_node_sample, FLOE_RUST_DENSITY_NODE_SAMPLE=off the kill switch): the
    squares as cells of their own draw what they draw as TOP's shapes, g x
    their area - the switch off, their nodes' boxes at the colour. A cell
    stands for the area its shapes cover (HierOpts::cell_cover, render-core
    Cache::cell_cover, FLOE_RUST_DENSITY_CELL_COVER=off): the array's, the
    list's and the lone cells' members g x their 2/0 square each - the switch
    off, their 1 um boxes. An item across a block boundary is shared between
    the blocks (HierOpts::dot_item_share, FLOE_RUST_DENSITY_ITEM_SHARE=off):
    the even field is even - the switch off, blocks at the colour beside
    blocks left dark - and so is an array whose pitch does not divide the
    block (the switch off: blocks of four, six and nine members)."""
    names = ('hier_flat', 'hier_cells', 'hier_cover', 'hier_even')
    paths = {name: Path(temp) / (name + '.oas') for name in names}
    hier_layouts(*(paths[name] for name in names))
    for src in paths.values():
        done = subprocess.run([FLOE2, 'index', str(src)],
                              cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
        assert done.returncode == 0, done.stdout + done.stderr
    env = {'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on', 'FLOE_RUST_DENSITY_BRIGHT': 'on',
           'FLOE_RUST_DENSITY_PATTERN': 'off'}
    workers = {
        'flat': worker(paths['hier_flat'], env),
        'cells': worker(paths['hier_cells'], env),
        'cells_off': worker(paths['hier_cells'], dict(env, FLOE_RUST_DENSITY_NODE_SAMPLE='off')),
        'cover': worker(paths['hier_cover'], env),
        'cover_off': worker(paths['hier_cover'], dict(env, FLOE_RUST_DENSITY_CELL_COVER='off')),
        'even': worker(paths['hier_even'], env),
        'even_off': worker(paths['hier_even'], dict(env, FLOE_RUST_DENSITY_ITEM_SHARE='off')),
    }
    try:
        w0 = workers['flat']
        dbu = float(w0.cache.meta['dbu'])
        colour = layer_colour(w0, MID)
        k = max(range(3), key=lambda i: colour[i])

        def view(w):
            box = (0.0, 0.0, float(W), float(H))
            w.submit({'kind': 'render', 'gen': 1, 'scope': 'live', 'bbox': tuple(v / dbu for v in box), 'view': None, 'w': W, 'h': H,
                      'depth': None, 'cut_px': 3.0, 'lod': False, 'frames': False, 'labels': False, 'abstract': False, 'visible': [MID],
                      'frame_format': 'raw', 'thin': 'keep', 'frame_cache': False})
            deadline = time.monotonic() + 300
            while time.monotonic() < deadline:
                res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
                assert res.get('kind') not in ('error', 'dropped'), res
                if res.get('kind') == 'frame' and res.get('gen') == 1 and not res.get('refining'):
                    return bytes(res.pop('rgba')), res
            raise AssertionError('hier frame timeout')

        # (rows count from the top: y um from the bottom is row H - y)
        def alphas(pixels, x, y):
            return [px(pixels, c, H - 1 - r)[k] / colour[k] for r in range(*y) for c in range(*x)]

        got, plan2 = {}, {}
        for key, w in workers.items():
            got[key], res = view(w)
            plan2[key] = res.get('density_plan2') or {}
        # a node counts what its placements hold
        want = 2 * 160 * 160 * 0.05 ** 2
        field = ((5, 55), (5, 55))
        flat, cells, boxed = (sum(alphas(got[key], *field)) for key in ('flat', 'cells', 'cells_off'))
        assert abs(flat - want) < 0.12 * want, ('the squares as shapes', flat, want)
        assert abs(cells - want) < 0.2 * want, ('the squares as cells', cells, want)
        assert plan2['cells'].get('by_nodes', 0) > 0 and plan2['cells'].get('node_sampled', 0) > 0, plan2['cells']
        assert boxed > 5 * want and plan2['cells_off'].get('node_sampled', 0) == 0, ('the switch off', boxed, want, plan2['cells_off'])
        print('density stack: a node counts what its placements hold - 25,600 squares of 0.0025 px^2: alphas %.0f as shapes, %.0f as a cell '
              'each (g x area %.0f; %d nodes by their placements) - the switch off %.0f, the nodes\' boxes'
              % (flat, cells, want, plan2['cells'].get('node_sampled', 0), boxed))
        # a cell stands for the area its shapes cover
        regions = {'array': ((8, 72), (98, 162)), 'list': ((98, 252), (8, 72)), 'lone': ((98, 252), (98, 162))}
        members = {'array': 3600, 'list': 2000, 'lone': 500}
        said = []
        for name, region in regions.items():
            want = 2 * members[name] * 0.2 ** 2
            on, off = sum(alphas(got['cover'], *region)), sum(alphas(got['cover_off'], *region))
            assert abs(on - want) < 0.15 * want, (name, on, want)
            assert off > 4 * want, (name, 'the switch off', off, want)
            said.append('%s %.0f for %.0f (off %.0f)' % (name, on, want, off))
        assert plan2['cover'].get('cell_cover') == 1 and plan2['cover'].get('cover_cells', 0) >= 1, plan2['cover']
        assert plan2['cover_off'].get('cell_cover') == 0 and plan2['cover_off'].get('cover_cells', 0) == 0, plan2['cover_off']
        print('density stack: a cell stands for its shapes\' cover - a 2/0 square of 0.04 px^2 in a 1 px cell: %s' % ', '.join(said))
        # an item across a block boundary is shared between the blocks
        inner = ((14, 54), (14, 54))
        on, off = alphas(got['even'], *inner), alphas(got['even_off'], *inner)
        mean_on, mean_off = sum(on) / len(on), sum(off) / len(off)
        dev_on = (sum((v - mean_on) ** 2 for v in on) / len(on)) ** 0.5 / mean_on
        dev_off = (sum((v - mean_off) ** 2 for v in off) / len(off)) ** 0.5 / mean_off
        assert abs(mean_on - 0.5) < 0.06 and dev_on < 0.35 and 0.05 < min(on) and max(on) < 0.8, ('the even field', mean_on, dev_on, min(on), max(on))
        assert abs(mean_off - 0.5) < 0.06 and dev_off > 0.5 and min(off) == 0.0 and max(off) >= 0.99, ('the even field, the switch off', mean_off, dev_off, min(off), max(off))
        print('density stack: an item across blocks is shared between them - cells a quarter covered, every 1 px: alpha %.2f, deviation %.2f '
              'of it, %.2f to %.2f - the switch off %.2f, deviation %.2f, %.2f to %.2f (blocks at the colour beside dark ones)'
              % (mean_on, dev_on, min(on), max(on), mean_off, dev_off, min(off), max(off)))
        # an array every 1.5 px: by 4 px blocks (the frame's own: the view starts at a block's corner)
        def by_blocks(pixels):
            cells = []
            for by in range(4, 13):
                for bx in range(26, 39):
                    cells.append(sum(alphas(pixels, (bx * 4, bx * 4 + 4), (by * 4, by * 4 + 4))) / 16.0)
            mean = sum(cells) / len(cells)
            return mean, (sum((v - mean) ** 2 for v in cells) / len(cells)) ** 0.5 / mean
        (mean_on, dev_on), (mean_off, dev_off) = by_blocks(got['even']), by_blocks(got['even_off'])
        assert abs(mean_on - 0.32) < 0.03 and dev_on < 0.12, ('the array', mean_on, dev_on)
        assert abs(mean_off - 0.32) < 0.03 and dev_off > 0.18, ('the array, the switch off', mean_off, dev_off)
        print('density stack: an array every 1.5 px takes its members\' parts in a block by their area - alpha %.2f for 0.32, its 4 px blocks '
              'within %.2f of it - the switch off %.2f (blocks of four, six and nine members)' % (mean_on, dev_on, dev_off))
    finally:
        for w in workers.values():
            w.stop()


def layer_colour(w, layer):
    """A layer's colour as the renderer paints it (the cache's style)."""
    for l in w.cache.meta['layers']:
        if (l['layer'], l['datatype']) == tuple(layer):
            c = l['color'].lstrip('#')
            return bytes((int(c[0:2], 16), int(c[2:4], 16), int(c[4:6], 16), 255))
    raise AssertionError('layer %s not found' % (layer,))


def top_first_checks(temp):
    """Pass 2 decodes the top plane's pages first (user 2026-10-04: "789's
    dots, lit alone, went with 787 on - the density draws 789 first, so they
    should stay"; renderd density_top_first, FLOE_RUST_DENSITY_TOP_FIRST=off
    the kill switch): the routing chip at a quarter scale (tools/
    gen_route_chip.py) at its fit view, depth 0, under a 64 MB budget - its
    M8 (38/0) lights the same pixels with M1 (31/0) on as alone; with the
    switch off the two sides' pages shared one list by distance and M8 lost
    some to M1."""
    src = Path(temp) / 'route.oas'
    done = subprocess.run([sys.executable, '-B', str(ROOT / 'tools/gen_route_chip.py'), str(src), '--scale', '0.25'],
                          cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
    assert done.returncode == 0, done.stdout + done.stderr
    done = subprocess.run([FLOE2, 'index', str(src)],
                          cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
    assert done.returncode == 0, done.stdout + done.stderr
    env = {'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on', 'FLOE_RUST_BUDGET_MB': '64'}
    workers = {'on': worker(src, dict(env, FLOE_RUST_DENSITY_TOP_FIRST='on')), 'off': worker(src, dict(env, FLOE_RUST_DENSITY_TOP_FIRST='off'))}
    try:
        cache = workers['on'].cache
        x0, y0, x1, y1 = cache.meta['bbox']
        fw, fh = 1350, 971
        spp = max((x1 - x0) / fw, (y1 - y0) / fh) * 1.05
        cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
        box = (cx - fw * spp / 2, cy - fh * spp / 2, cx + fw * spp / 2, cy + fh * spp / 2)
        up, low = (38, 0), (31, 0)
        up_c = layer_colour(workers['on'], up)

        def lit_up(w, gen, visible):
            w.submit({'kind': 'render', 'gen': gen, 'scope': 'headless', 'bbox': box, 'view': None, 'w': fw, 'h': fh, 'depth': 0, 'cut_px': 3.0,
                      'lod': False, 'frames': False, 'labels': False, 'abstract': False, 'visible': visible, 'frame_format': 'raw',
                      'thin': 'keep', 'frame_cache': False})
            deadline = time.monotonic() + 300
            while time.monotonic() < deadline:
                res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
                assert res.get('kind') not in ('error', 'dropped'), res
                if res.get('kind') == 'frame' and res.get('gen') == gen and not res.get('refining'):
                    rgba = bytes(res.pop('rgba'))
                    return {i for i in range(0, len(rgba), 4) if rgba[i:i + 4] == up_c}, res
            raise AssertionError('top first frame timeout')

        alone, _ = lit_up(workers['on'], 1, [up])
        both, both_res = lit_up(workers['on'], 2, [low, up])
        alone_off, _ = lit_up(workers['off'], 1, [up])
        both_off, off_res = lit_up(workers['off'], 2, [low, up])
        assert alone and alone == alone_off, ('M8 alone', len(alone), len(alone_off))
        assert off_res['density_pages']['over_budget'] > 0, ('the reserve must not hold both', off_res['density_pages'])
        assert both == alone, ('M8 with M1 on', len(both), len(alone), both_res['density_pages'])
        kept_off = len(both_off & alone) / len(alone)
        assert kept_off < 0.95, ('the switch off', kept_off)
        print('density stack: pass 2 decodes the top plane first - M8 %d px alone, all kept with M1 on (the switch off kept %.0f %%; '
              '%d pages over the reserve)' % (len(alone), 100 * kept_off, off_res['density_pages']['over_budget']))
    finally:
        for w in workers.values():
            w.stop()


def density_only_layout(path):
    """LOW's 36 x 16 um square (an original past every cut) under MID's
    40,000 0.05 um squares at random over a 20 x 10 um band in its middle
    (each under a pixel at 0.1 um)."""
    import random
    import klayout.db as kdb
    ly = kdb.Layout()
    ly.dbu = 0.001
    top = ly.create_cell('TOP')
    top.shapes(ly.layer(*LOW)).insert(kdb.DBox(2, 2, 38, 18))
    mid = ly.layer(*MID)
    rnd = random.Random(13)
    for _ in range(40_000):
        x, y = 10 + rnd.randrange(20_000) / 1000.0, 5 + rnd.randrange(10_000) / 1000.0
        top.shapes(mid).insert(kdb.DBox(x, y, x + 0.05, y + 0.05))
    ly.write(str(path))


def density_only_checks(temp):
    """The density alone (user 2026-10-04: "an option to pass the shapes by
    and draw the density alone, to compare" - is it pass 1's budget that
    takes 789's dots; renderd density_only, FLOE_RUST_DENSITY_ONLY=on,
    diagnostic): under a 64 MB budget pass 1 reads no page (none in hand
    for pass 2) and draws no shape - LOW's original is gone - while MID's
    density lights as it does with the shapes, and pass 2's reserve is the
    whole budget."""
    src = Path(temp) / 'density_only.oas'
    density_only_layout(src)
    done = subprocess.run([FLOE2, 'index', str(src)],
                          cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
    assert done.returncode == 0, done.stdout + done.stderr
    env = {'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on', 'FLOE_RUST_BUDGET_MB': '64'}
    workers = {'shapes': worker(src, env), 'only': worker(src, dict(env, FLOE_RUST_DENSITY_ONLY='on'))}
    try:
        low_c, mid_c = layer_colour(workers['shapes'], LOW), layer_colour(workers['shapes'], MID)
        (shapes, shapes_res), (only, only_res) = (frame(w, 1, (LOW, MID)) for w in (workers['shapes'], workers['only']))

        def of(pixels, colour):
            return {(c, r) for r in range(H) for c in range(W) if px(pixels, c, r) == colour}

        assert of(shapes, low_c) and not of(only, low_c), ('LOW original', len(of(shapes, low_c)), len(of(only, low_c)))
        assert of(only, mid_c) and of(only, mid_c) == of(shapes, mid_c), ('MID density', len(of(only, mid_c)), len(of(shapes, mid_c)))
        assert shapes_res['density_pages']['in_hand'] > 0 and only_res['density_pages']['in_hand'] == 0, (shapes_res['density_pages'], only_res['density_pages'])
        assert only_res['density_plan2']['reserve_mb'] == 64, only_res['density_plan2']
        print('density stack: the density alone - LOW\'s original %d px -> none, MID %d px either way, pass 1 pages in hand %d -> 0, '
              'reserve %d MB' % (len(of(shapes, low_c)), len(of(only, mid_c)), shapes_res['density_pages']['in_hand'], only_res['density_plan2']['reserve_mb']))
    finally:
        for w in workers.values():
            w.stop()


TOP1 = (TOP[0], 1)


def top_group_layout(path):
    """LOW's 36 x 16 um square (an original past every cut); TOP's 40,000
    0.05 um squares at random over a 20 x 10 um band across it and TOP1's (the
    same layer number, its next datatype) 0.1 um specks every 4 um above y 18
    um, clear of the band."""
    import random
    import klayout.db as kdb
    ly = kdb.Layout()
    ly.dbu = 0.001
    top = ly.create_cell('TOP')
    top.shapes(ly.layer(*LOW)).insert(kdb.DBox(2, 2, 38, 18))
    dense = ly.layer(*TOP)
    rnd = random.Random(19)
    for _ in range(40_000):
        x, y = 10 + rnd.randrange(20_000) / 1000.0, 5 + rnd.randrange(10_000) / 1000.0
        top.shapes(dense).insert(kdb.DBox(x, y, x + 0.05, y + 0.05))
    upper = ly.layer(*TOP1)
    for i in range(10):
        top.shapes(upper).insert(kdb.DBox(1.0 + i * 4.0, 18.5, 1.1 + i * 4.0, 18.6))
    ly.write(str(path))


def top_group_checks(temp):
    """The topmost planes by the drawing order are top planes, each planned
    on its own (user 2026-10-04, the real chip: with 787.* and 789.* on,
    "789's density still shrinks when 787 is on"; then "787.0 ... 789.55: the
    top layer is 789.55 - we draw 789.55 first, then 789.20, 789.0, 787.55,
    787.20, 787.0, filling what is empty"; renderd density_top_group,
    FLOE_RUST_DENSITY_TOP_GROUP=off the kill switch): TOP1 is the topmost
    plane, TOP under it a top plane too, so its dense specks over LOW's
    original light the same pixels with LOW on as without it; with the
    switch off TOP is a lower plane and LOW's original keeps them out; LOW
    shows where TOP does not light. Standard cells under the cut holding LOW
    and MID, both top planes, are dots of each - planned as one, a plan
    counts a cell for its topmost layer alone and LOW had none. The top planes
    over the originals below them: with the shapes first off (main;
    shapes_first_checks the default)."""
    src = Path(temp) / 'top_group.oas'
    top_group_layout(src)
    done = subprocess.run([FLOE2, 'index', str(src)],
                          cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
    assert done.returncode == 0, done.stdout + done.stderr
    env = {'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on'}
    workers = {'on': worker(src, dict(env, FLOE_RUST_DENSITY_TOP_GROUP='on')), 'off': worker(src, dict(env, FLOE_RUST_DENSITY_TOP_GROUP='off'))}
    try:
        low_c, top_c = layer_colour(workers['on'], LOW), layer_colour(workers['on'], TOP)
        band = (range(100, 300), range(50, 150))

        def of(pixels, colour):
            return {(c, r) for r in band[1] for c in band[0] if px(pixels, c, r) == colour}

        alone = {name: of(frame(w, 1, (TOP, TOP1))[0], top_c) for name, w in workers.items()}
        both = {name: frame(w, 2, (LOW, TOP, TOP1))[0] for name, w in workers.items()}
        assert alone['on'] and alone['on'] == alone['off'], ('TOP without LOW', len(alone['on']), len(alone['off']))
        assert of(both['on'], top_c) == alone['on'], ('TOP over LOW', len(of(both['on'], top_c)), len(alone['on']))
        kept_off = len(of(both['off'], top_c) & alone['on'])
        assert kept_off < 0.5 * len(alone['on']), ('the switch off', kept_off, len(alone['on']))
        low_on = of(both['on'], low_c)
        assert low_on and not (low_on & alone['on']), ('LOW where TOP does not light', len(low_on))
        print('density stack: the topmost planes are top planes - TOP %d px in its band with LOW on as without it '
              '(the switch off kept %d); LOW %d px there where TOP does not light' % (len(alone['on']), kept_off, len(low_on)))
    finally:
        for w in workers.values():
            w.stop()
    # standard cells under the cut, each holding LOW (its box) and MID (inside
    # it) - both top planes: planned as one a cell counts for its topmost layer
    # alone and LOW had no dot; each on its own, LOW as when it is a lower plane
    src = Path(temp) / 'top_cells.oas'
    import random
    import klayout.db as kdb
    ly = kdb.Layout()
    ly.dbu = 0.001
    top = ly.create_cell('TOP')
    rnd = random.Random(23)
    for t in range(4):
        cell = ly.create_cell('S%d' % t)
        w = 0.02 * (1 + 2 * t)
        cell.shapes(ly.layer(*LOW)).insert(kdb.DBox(0, 0, w, 0.12))
        cell.shapes(ly.layer(*MID)).insert(kdb.DBox(0.005, 0.01, w - 0.005, 0.03))
        for _ in range(1500):
            top.insert(kdb.DCellInstArray(cell.cell_index(), kdb.DTrans(kdb.DVector(rnd.randrange(39_800) / 1000.0, rnd.randrange(19_800) / 1000.0))))
    ly.write(str(src))
    done = subprocess.run([FLOE2, 'index', str(src)],
                          cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
    assert done.returncode == 0, done.stdout + done.stderr
    workers = {'on': worker(src, dict(env, FLOE_RUST_DENSITY_TOP_GROUP='on')), 'off': worker(src, dict(env, FLOE_RUST_DENSITY_TOP_GROUP='off'))}
    try:
        low_c, mid_c = layer_colour(workers['on'], LOW), layer_colour(workers['on'], MID)
        frames = {name: frame(w, 3, (LOW, MID))[0] for name, w in workers.items()}
        count = lambda pixels, colour: sum(1 for r in range(H) for c in range(W) if px(pixels, c, r) == colour)
        low_on, low_off, mid_on = count(frames['on'], low_c), count(frames['off'], low_c), count(frames['on'], mid_c)
        assert low_on > 0 and mid_on > 0 and abs(low_on - low_off) <= 0.1 * low_off, ('standard cells of two top planes', low_on, low_off, mid_on)
        print('density stack: standard cells under the cut, LOW and MID top planes each planned on its own - LOW %d px (a lower plane %d), '
              'MID %d px' % (low_on, low_off, mid_on))
    finally:
        for w in workers.values():
            w.stop()


def shapes_first_layout(path):
    """LOW's 18 x 16 um square (an original past every cut, x 2-20 um); TOP's
    40,000 0.05 um squares at random over a 20 x 10 um band across its right
    edge - over it left of x 20 um, where no original is right of it - and
    TOP1's 0.1 um specks every 4 um above y 18 um, clear of the band."""
    import random
    import klayout.db as kdb
    ly = kdb.Layout()
    ly.dbu = 0.001
    top = ly.create_cell('TOP')
    top.shapes(ly.layer(*LOW)).insert(kdb.DBox(2, 2, 20, 18))
    dense = ly.layer(*TOP)
    rnd = random.Random(29)
    for _ in range(40_000):
        x, y = 10 + rnd.randrange(20_000) / 1000.0, 5 + rnd.randrange(10_000) / 1000.0
        top.shapes(dense).insert(kdb.DBox(x, y, x + 0.05, y + 0.05))
    upper = ly.layer(*TOP1)
    for i in range(10):
        top.shapes(upper).insert(kdb.DBox(1.0 + i * 4.0, 18.5, 1.1 + i * 4.0, 18.6))
    ly.write(str(path))


def shapes_first_checks(temp):
    """Pass 1's shapes come first (user 2026-10-04, the real chip: "if pass 1
    drew the shapes past the cut, density drawn only in the space left will
    hardly jar"; renderd density_shapes_first, FLOE_RUST_DENSITY_SHAPES_FIRST=off
    the kill switch): no plane's density, the top planes' neither, shows where
    an original is. TOP's dense specks across LOW's right edge - TOP the
    topmost plane, or a top plane under TOP1 - light nothing over LOW's
    original, which shows as LOW alone, and right of it the pixels they light
    without LOW where LOW alone leaves the frame dark (its outline takes the
    column past its edge); pass 2's top side plans less free space than with
    the switch off, where they light over LOW's original as without it."""
    src = Path(temp) / 'shapes_first.oas'
    shapes_first_layout(src)
    done = subprocess.run([FLOE2, 'index', str(src)],
                          cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
    assert done.returncode == 0, done.stdout + done.stderr
    env = {'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on', 'FLOE_RUST_DENSITY_TOP_GROUP': 'on'}
    workers = {'on': worker(src, dict(env, FLOE_RUST_DENSITY_SHAPES_FIRST='on')), 'off': worker(src, dict(env, FLOE_RUST_DENSITY_SHAPES_FIRST='off'))}
    try:
        low_c, top_c = layer_colour(workers['on'], LOW), layer_colour(workers['on'], TOP)
        # the band over LOW's original (x 10-20 um) and right of it (20-30 um)
        over, free = (range(100, 200), range(50, 150)), (range(200, 300), range(50, 150))

        def of(pixels, colour, part):
            return {(c, r) for r in part[1] for c in part[0] if px(pixels, c, r) == colour}

        def same(a, b, part):
            return all(px(a, c, r) == px(b, c, r) for r in part[1] for c in part[0])

        alone = frame(workers['on'], 1, (TOP,))[0]
        low_alone = frame(workers['on'], 2, (LOW,))[0]
        want_free, want_over = of(alone, top_c, free), of(alone, top_c, over)
        # where no original is: LOW alone leaves it dark
        empty = {(c, r) for (c, r) in want_free if px(low_alone, c, r) == BLACK}
        assert want_free and want_over and len(empty) < len(want_free), ('TOP alone', len(want_free), len(want_over), len(empty))
        gen = 2
        for visible in ((LOW, TOP), (LOW, TOP, TOP1)):
            gen += 1
            on, res_on = frame(workers['on'], gen, visible)
            off, res_off = frame(workers['off'], gen, visible)
            assert not of(on, top_c, over) and same(on, low_alone, over), (visible, 'TOP over LOW', len(of(on, top_c, over)))
            assert of(on, top_c, free) == empty, (visible, 'TOP where no original is', len(of(on, top_c, free)), len(empty))
            assert of(off, top_c, over) == want_over and of(off, top_c, free) == want_free, (visible, 'the switch off', len(of(off, top_c, over)))
            free_on, free_off = res_on['density_plan2']['free_top'], res_off['density_plan2']['free_top']
            assert 0 < free_on < free_off, (visible, 'the top side planned', free_on, free_off)
            print('density stack: pass 1\'s shapes first, %s - TOP none over LOW\'s original (the switch off %d px, as alone), '
                  '%d px right of it as alone; the top side planned %d free px (off %d)'
                  % ('TOP topmost' if len(visible) == 2 else 'TOP under TOP1', len(of(off, top_c, over)), len(empty), free_on, free_off))
    finally:
        for w in workers.values():
            w.stop()


def full_shapes_first_checks(temp):
    """A full tile stops before the last original plane. Freeze its mask
    at the pass-2 boundary anyway: no density plans, decode or coverage
    initialization, and the same frame as with density off. Abutting 1 px
    hairlines exercise the early exit with the viewer's speckle fill; one
    large speckled shape blocks density through coverage instead of ink.
    With only half covered, keep drawing density into the other half.
    """
    import klayout.db as kdb
    env = {
        'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on',
        'FLOE_RUST_DENSITY_TOP_GROUP': 'on', 'FLOE_RUST_DENSITY_TOP_PLANES': '2',
        'FLOE_RUST_DENSITY_SHAPES_FIRST': 'on', 'FLOE_RUST_DENSITY_BRIGHT': 'on',
        'FLOE_RUST_DENSITY_PATTERN': None,
        'FLOE_RUST_DENSITY_FREE_CELLS': 'on', 'FLOE_RUST_OCCUPANCY': 'off',
        'FLOE_RUST_SHAPE_CUT': 'max',
    }
    for kind in ('solid', 'hairlines', 'speckle', 'half'):
        src = Path(temp) / ('full_shapes_first_%s.oas' % kind)
        ly = kdb.Layout()
        ly.dbu = 0.001
        top = ly.create_cell('TOP')
        top.shapes(ly.layer(*LOW)).insert(kdb.Box(50_000, 30_000, 51_000, 31_000))
        for y in range(50, 20_000, 500):
            for x in range(50, 40_000, 500):
                top.shapes(ly.layer(*MID)).insert(kdb.Box(x, y, x + 150, y + 150))
        layer = ly.layer(*TOP)
        if kind == 'hairlines':
            for x in range(0, 40_000, 100):
                top.shapes(layer).insert(kdb.Box(x, -1_000, x + 100, 21_000))
        else:
            top.shapes(layer).insert(kdb.Box(-1_000, -1_000, 20_000 if kind == 'half' else 41_000, 21_000))
        ly.write(str(src))
        done = subprocess.run([FLOE2, 'index', str(src)],
                              cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
        assert done.returncode == 0, done.stdout + done.stderr
        # An edge tile as well as full tiles, and a one-tile frame.
        for tile in (64, 512):
            settings = dict(env, FLOE_RUST_TILE_PX=str(tile))
            workers = {
                'on': worker(src, settings),
                'off': worker(src, dict(settings, FLOE_RUST_DENSITY_STACK='off')),
            }
            try:
                for w in workers.values():
                    if kind in ('solid', 'half'):
                        w._fills[TOP] = 'solid'
                        w._publish_style(wait=True)
                off, _ = frame(workers['off'], 1, (LOW, MID, TOP))
                on, res = frame(workers['on'], 1, (LOW, MID, TOP))
                p2, pages, stack = res['density_plan2'], res['density_pages'], res['density_stack']
                assert p2['pattern'] == 1, p2
                assert p2['stages'] == 0, 'layer staging must remain opt-in'
                if kind == 'half':
                    assert all(px(on, c, r) == px(off, c, r) for r in range(H) for c in range(W // 2)), 'covered half changed'
                    assert any(px(on, c, r) != px(off, c, r) for r in range(H) for c in range(W // 2 + 2, W)), 'open half lost its density'
                    assert 0 < p2['free_top'] < W * H and p2['passes'] > 0, p2
                else:
                    assert on == off, (kind, tile, 'density changed a fully covered frame')
                    assert p2['free_top'] == p2['free_others'] == p2['passes'] == p2['regions'] == p2['cell_cover'] == 0, p2
                    assert p2['nodes'] == p2['page_nodes'] == p2['page_candidates'] == p2['reads'] == p2['items'] == 0, p2
                    assert pages['planned'] == pages['decoded'] == 0, pages
                    assert res['density_us']['plan2_us'] == res['density_us']['scene2_us'] == res['density_us']['decode2_us'] == 0, res['density_us']
                    assert stack['covered'] == W * H and stack['lit'] == stack['top'] == stack['lower'] == 0, stack
                    if kind != 'speckle':
                        assert res['once_full_tiles'] > 0, 'must exercise early full-tile exit'
                print('density stack: shapes first, %s tile %d - free %d/%d px, %d plans, %d decoded pages'
                      % (kind, tile, p2['free_top'], p2['free_others'], p2['passes'], pages['decoded']))
            finally:
                for w in workers.values():
                    w.stop()


def nearly_full_pattern_checks(temp):
    """One open pixel per 32 x 32 block must not request density when
    every hole is on the shared pattern's forbidden parity. The old demand
    planned all eight top layers over the whole view for zero added pixels.
    Move the holes one column: real drawable gaps must remain eligible.
    """
    import klayout.db as kdb
    size = 512
    visible = [(i, 0) for i in range(9)]
    env = {
        'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on',
        'FLOE_RUST_DENSITY_BRIGHT': 'on', 'FLOE_RUST_DENSITY_PATTERN': 'on',
        'FLOE_RUST_DENSITY_TOP_GROUP': 'on', 'FLOE_RUST_DENSITY_TOP_PLANES': '8',
        'FLOE_RUST_DENSITY_SHAPES_FIRST': 'on', 'FLOE_RUST_DENSITY_FREE_CELLS': 'on',
        'FLOE_RUST_DENSITY_FREE_CELL_PX': '32', 'FLOE_RUST_DENSITY_OTHERS_MIN': '0.125',
        'FLOE_RUST_OCCUPANCY': 'off', 'FLOE_RUST_SHAPE_CUT': 'max',
        'FLOE_RUST_EDGE_EXACT': 'on',
    }
    for allowed in (False, True):
        src = Path(temp) / ('nearly_full_%s.oas' % allowed)
        ly = kdb.Layout()
        ly.dbu = 0.001
        top = ly.create_cell('TOP')
        own = top.shapes(ly.layer(0, 0))
        hx = 30 if allowed else 29
        for y in range(0, size, 32):
            for x in range(0, size, 32):
                # The edge-exact rim leaves only (x+hx, y+31) open.
                # The right strip is tall enough to survive the 3 px cut.
                own.insert(kdb.Box(x * 100, y * 100, (x + 32) * 100, (y + 30) * 100))
                own.insert(kdb.Box(x * 100, (y + 30) * 100, (x + hx - 1) * 100, (y + 32) * 100))
                own.insert(kdb.Box((x + hx + 1) * 100, y * 100, (x + 32) * 100, (y + 32) * 100))
        for layer in range(1, 9):
            leaf = ly.create_cell('DOT%d' % layer)
            leaf.shapes(ly.layer(layer, 0)).insert(kdb.Box(0, 0, 150, 150))
            top.insert(kdb.CellInstArray(leaf.cell_index(), kdb.Trans(hx * 100 - 25, 3075),
                                        kdb.Vector(3200, 0), kdb.Vector(0, 3200), 16, 16))
        ly.write(str(src))
        done = subprocess.run([FLOE2, 'index', str(src)],
                              cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
        assert done.returncode == 0, done.stdout + done.stderr
        images = []
        for tile in (64, 127):
            w = worker(src, dict(env, FLOE_RUST_TILE_PX=str(tile)))
            try:
                frames = []
                for gen, density in ((1, False), (2, True)):
                    w.submit({'kind': 'render', 'gen': gen, 'scope': 'headless',
                              'bbox': (0, 0, size * 100, size * 100), 'w': size, 'h': size,
                              'depth': None, 'cut_px': 3.0, 'lod': False, 'frames': False,
                              'labels': False, 'abstract': False, 'visible': visible,
                              'frame_format': 'raw', 'thin': 'keep', 'frame_cache': False,
                              'density': density})
                    deadline = time.monotonic() + 60
                    while True:
                        res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
                        assert res.get('kind') != 'error', res
                        if res.get('kind') == 'frame' and res.get('gen') == gen and not res.get('refining'):
                            frames.append(bytes(res.pop('rgba')))
                            break
                p2, stack = res['density_plan2'], res['density_stack']
                assert stack['covered'] == size * size - 256, stack
                if allowed:
                    assert frames[0] != frames[1] and stack['top'] > 0, 'drawable holes lost their density'
                    assert p2['free_top'] == 256 and p2['passes'] > 0, p2
                else:
                    assert frames[0] == frames[1], 'forbidden holes changed the frame'
                    assert p2['passes'] == p2['regions'] == p2['cell_cover'] == 0, p2
                    assert res['density_pages']['planned'] == res['density_pages']['decoded'] == 0, res
                    assert res['density_us']['plan2_us'] == 0, res
                images.append(frames[1])
                print('density stack: 99.9%% covered, %s holes, tile %d - %d plans, %d added px'
                      % ('drawable' if allowed else 'forbidden', tile, p2['passes'], stack['top']))
            finally:
                w.stop()
        assert images[0] == images[1], 'near-full pattern depends on tile size'


def masked_density_frame(w, gen, visible, size=128, density=True):
    """A square 0.1 um/px view for the direct-mask and staged-plan cases."""
    w.submit({'kind': 'render', 'gen': gen, 'scope': 'headless',
              'bbox': (0, 0, size * 100, size * 100), 'w': size, 'h': size,
              'depth': None, 'cut_px': 3.0, 'lod': False, 'frames': False,
              'labels': False, 'abstract': False, 'visible': list(visible),
              'frame_format': 'raw', 'thin': 'keep', 'frame_cache': False,
              'density': density})
    deadline = time.monotonic() + 60
    while time.monotonic() < deadline:
        res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
        assert res.get('kind') not in ('error', 'dropped'), res
        if res.get('kind') == 'frame' and res.get('gen') == gen and not res.get('refining'):
            return bytes(res.pop('rgba')), res
    raise AssertionError('masked density frame timeout')


def masked_density_env():
    return {
        'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on',
        'FLOE_RUST_DENSITY_BRIGHT': 'on', 'FLOE_RUST_DENSITY_PATTERN': 'on',
        'FLOE_RUST_DENSITY_TOP_GROUP': 'on', 'FLOE_RUST_DENSITY_TOP_PLANES': '8',
        'FLOE_RUST_DENSITY_SHAPES_FIRST': 'on', 'FLOE_RUST_DENSITY_FREE_CELLS': 'on',
        'FLOE_RUST_DENSITY_FREE_CELL_PX': '32', 'FLOE_RUST_DENSITY_OTHERS_MIN': '0',
        'FLOE_RUST_DENSITY_STAGES': None, 'FLOE_RUST_DENSITY_MASK': None,
        'FLOE_RUST_OCCUPANCY': 'off', 'FLOE_RUST_SHAPE_CUT': 'max',
        'FLOE_RUST_EDGE_EXACT': 'on', 'FLOE_RUST_DENSITY_OVB_FIRST': 'off',
        'FLOE_RUST_RETAINED_MB': '0',
    }


def planner_mask_checks(temp):
    """Every 32px demand cell has one drawable hole. Its covered interior
    contains shared, rotated and mirrored repeated subtrees: the planner's
    pixel mask must skip them before walking their dots, while preserving
    repeated, rotated instances of the same dot cell at each hole. Both
    modes use staged planning;
    only FLOE_RUST_DENSITY_MASK=off removes the direct mask query.
    """
    import klayout.db as kdb
    src = Path(temp) / 'planner_mask.oas'
    ly = kdb.Layout()
    ly.dbu = 0.001
    top = ly.create_cell('TOP')
    own = top.shapes(ly.layer(0, 0))
    for y in range(0, 128, 32):
        for x in range(0, 128, 32):
            own.insert(kdb.Box(x * 100, y * 100, (x + 32) * 100, (y + 30) * 100))
            own.insert(kdb.Box(x * 100, (y + 30) * 100, (x + 29) * 100, (y + 32) * 100))
            own.insert(kdb.Box((x + 31) * 100, y * 100, (x + 32) * 100, (y + 32) * 100))
    leaf = ly.create_cell('BLOCKED_DOT')
    leaf.shapes(ly.layer(1, 0)).insert(kdb.Box(0, 0, 150, 150))
    group = ly.create_cell('BLOCKED_GROUP')
    # Distinct cells require a real BVH walk; a single regular array can
    # aggregate all its members in one item without visiting child nodes.
    for row in range(12):
        for col in range(12):
            dot = leaf if row == col == 0 else ly.create_cell('BLOCKED_%d_%d' % (row, col))
            if dot != leaf:
                dot.shapes(ly.layer(1, 0)).insert(kdb.Box(0, 0, 150 + row, 150 + col))
            group.insert(kdb.CellInstArray(dot.cell_index(), kdb.Trans(col * 200, row * 200)))
    for row, y in enumerate(range(0, 128, 32)):
        for col, x in enumerate(range(0, 128, 32)):
            transform = kdb.Trans((row + col) % 4, bool(row % 2), 0, 0)
            box = group.bbox().transformed(transform)
            placed = kdb.Trans(transform.rot, transform.is_mirror(),
                               (x + 2) * 100 - box.left, (y + 2) * 100 - box.bottom)
            top.insert(kdb.CellInstArray(group.cell_index(), placed))
    top.insert(kdb.CellInstArray(leaf.cell_index(), kdb.Trans(2975, 3075),
                                kdb.Vector(3200, 0), kdb.Vector(0, 3200), 4, 2))
    top.insert(kdb.CellInstArray(leaf.cell_index(), kdb.Trans(1, False, 3125, 9475),
                                kdb.Vector(3200, 0), kdb.Vector(0, 3200), 4, 2))
    ly.write(str(src))
    done = subprocess.run([FLOE2, 'index', str(src)],
                          cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
    assert done.returncode == 0, done.stdout + done.stderr
    images = []
    for tile in (64, 127):
        frames = {}
        for mode in ('on', 'off'):
            env = dict(masked_density_env(), FLOE_RUST_TILE_PX=str(tile),
                       FLOE_RUST_DENSITY_MASK=mode, FLOE_RUST_DENSITY_STAGES='on')
            w = worker(src, env)
            try:
                baseline, _ = masked_density_frame(w, 1, [(0, 0), (1, 0)], density=False)
                frames[mode] = masked_density_frame(w, 2, [(0, 0), (1, 0)])
                pixels, result = frames[mode]
                assert result['density_stack']['covered'] == 128 * 128 - 16, result['density_stack']
                assert sum(pixels[i:i + 4] != baseline[i:i + 4] for i in range(0, len(pixels), 4)) == 16, 'visible repeated dots lost'
            finally:
                w.stop()
        assert frames['on'][0] == frames['off'][0], 'direct mask changed visible density'
        on, off = (frames[mode][1]['density_plan2'] for mode in ('on', 'off'))
        assert on['mask_tests'] > 0 and on['mask_pruned'] > 0 and off['mask_tests'] == 0, (on, off)
        assert on['nodes'] < off['nodes'] and on['items'] < off['items'], (on, off)
        images.append(frames['on'][0])
        print('density stack: direct mask tile %d - nodes %d/%d, items %d/%d, same 16 visible dots'
              % (tile, on['nodes'], off['nodes'], on['items'], off['items']))
    assert images[0] == images[1], 'direct mask depends on tile size'


def staged_density_checks(temp):
    """A density shape fills the drawable slots of each 3x3 original hole.
    Plans below it must observe the completed draw, including its unlit
    pattern support. Twelve visible layers put the filling layer either
    first or ninth, beyond the former eight top planes. The lower layers
    have identical support, making the legacy frame an exact oracle. The
    default retains legacy planning; staged planning is explicitly enabled.
    """
    import klayout.db as kdb
    visible = [(layer, 0) for layer in range(12)]
    for active in (11, 3):
        src = Path(temp) / ('staged_density_%d.oas' % active)
        ly = kdb.Layout()
        ly.dbu = 0.001
        top = ly.create_cell('TOP')
        own = top.shapes(ly.layer(0, 0))
        for y in range(0, 128, 32):
            for x in range(0, 128, 32):
                own.insert(kdb.Box(x * 100, y * 100, (x + 32) * 100, (y + 12) * 100))
                own.insert(kdb.Box(x * 100, (y + 16) * 100, (x + 32) * 100, (y + 32) * 100))
                own.insert(kdb.Box(x * 100, (y + 12) * 100, (x + 12) * 100, (y + 16) * 100))
                own.insert(kdb.Box((x + 16) * 100, (y + 12) * 100, (x + 32) * 100, (y + 16) * 100))
        for layer in range(1, 12):
            shapes = top.shapes(ly.layer(layer, 0))
            at = 1325 if layer <= active else 400
            for y in range(0, 128, 32):
                for x in range(0, 128, 32):
                    shapes.insert(kdb.Box(x * 100 + at, y * 100 + at,
                                          x * 100 + at + 250, y * 100 + at + 250))
        ly.write(str(src))
        done = subprocess.run([FLOE2, 'index', str(src)],
                              cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
        assert done.returncode == 0, done.stdout + done.stderr
        frames = {}
        for mode, tile, workers in (('default', 64, 1), ('on', 64, 1), ('parallel', 127, 4), ('off', 64, 1)):
            stages = {'default': None, 'off': 'off'}.get(mode, 'on')
            env = dict(masked_density_env(), FLOE_RUST_TILE_PX=str(tile),
                       FLOE_RUST_RASTER_JOBS=str(workers),
                       FLOE_RUST_DENSITY_STAGES=stages)
            w = worker(src, env)
            try:
                baseline, _ = masked_density_frame(w, 1, visible, density=False)
                frames[mode] = masked_density_frame(w, 2, visible)
                pixels, result = frames[mode]
                assert result['density_stack']['covered'] == 128 * 128 - 16 * 9, result['density_stack']
                assert sum(pixels[i:i + 4] != baseline[i:i + 4] for i in range(0, len(pixels), 4)) == 64, 'density shape lost its visible slots'
                colour = layer_colour(w, (active, 0))
                assert sum(pixels[i:i + 4] == colour for i in range(0, len(pixels), 4)) == 64, 'wrong staged layer owns the density'
            finally:
                w.stop()
        assert frames['default'][0] == frames['on'][0] == frames['parallel'][0] == frames['off'][0], 'staged pattern, tile or worker count changed this controlled frame'
        current, legacy = frames['on'][1], frames['off'][1]
        default = frames['default'][1]
        assert default['density_plan2']['stages'] == 0 and default['density_plan2']['passes'] == 9, default['density_plan2']
        assert default['density_pages'] == legacy['density_pages'], (default['density_pages'], legacy['density_pages'])
        assert current['density_plan2']['stages'] == 12 - active and legacy['density_plan2']['stages'] == 0, (current['density_plan2'], legacy['density_plan2'])
        assert current['density_pages']['decoded'] < legacy['density_pages']['decoded'], (current['density_pages'], legacy['density_pages'])
        if active == 11:
            assert current['density_plan2']['passes'] == 1 < legacy['density_plan2']['passes'], (current['density_plan2'], legacy['density_plan2'])
        else:
            assert current['density_pages']['decoded'] <= 9, current['density_pages']
        print('density stack: staged layer %d - %d/%d plans, %d/%d decoded pages, same 64 visible pixels'
              % (active, current['density_plan2']['passes'], legacy['density_plan2']['passes'],
                 current['density_pages']['decoded'], legacy['density_pages']['decoded']))


def own_layout(path):
    """A TOP whose own shapes are 60,000 boxes of 0.05-0.3 um at random over
    300 x 300 um - under a pixel at 1000 px; of 62,500 sizes, so the writer
    keeps most one record each (boxes of a size become a repetition): 38,554
    records, 7.7 MB by the planner's estimate, in a page pass 1 leaves (a box
    over the cut beside them would put it in pass 1's hands, free in pass 2's
    budget)."""
    import random
    import klayout.db as kdb
    ly = kdb.Layout()
    ly.dbu = 0.001
    top = ly.create_cell('TOP')
    layer = ly.layer(*LOW)
    rnd = random.Random(9)
    for _ in range(60_000):
        x, y = rnd.randrange(300_000) / 1000.0, rnd.randrange(300_000) / 1000.0
        w, h = 0.05 + rnd.randrange(250) / 1000.0, 0.05 + rnd.randrange(250) / 1000.0
        top.shapes(layer).insert(kdb.DBox(x, y, x + w, y + h))
    ly.write(str(path))


def left_checks(temp):
    """Pass 2's reserve is what pass 1 left of the budget when that is more
    (user 2026-10-02, the field chip at depth 0: a root's own shapes under a
    pixel failed the 0 px floor's probe of the fixed reserve and the view drew
    nothing under the cut - `lit 0 px, cell dots 0, 0 pages, pass 2 over
    budget`). A 32 MB budget (a fixed reserve of 4 MB) and the TOP's own
    boxes, 7.7 MB by estimate, at depth 0 with a zero floor: pass 2 draws them
    as a cut-free frame does; FLOE_RUST_DENSITY_RESERVE_LEFT=off draws none
    and reports the floor probe over budget."""
    src = Path(temp) / 'own.oas'
    own_layout(src)
    done = subprocess.run([FLOE2, 'index', str(src)],
                          cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
    assert done.returncode == 0, done.stdout + done.stderr
    env = {'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on', 'FLOE_RUST_DENSITY_FLOOR_PX': '0', 'FLOE_RUST_BUDGET_MB': '32'}
    left, fixed, free = worker(src, env), worker(src, dict(env, FLOE_RUST_DENSITY_RESERVE_LEFT='off')), worker(src, {'FLOE_RUST_BUDGET_MB': '32'})
    # the fixed reserve without the page spread (on by default since 0.12.277)
    unspread = worker(src, dict(env, FLOE_RUST_DENSITY_RESERVE_LEFT='off', FLOE_RUST_DENSITY_PAGE_SPREAD='off'))
    try:
        dbu = float(left.cache.meta['dbu'])
        side = 1000

        def view(w, cut_px=3.0):
            w.submit({'kind': 'render', 'gen': 1, 'scope': 'headless', 'bbox': (0.0, 0.0, 300.2 / dbu, 300.2 / dbu), 'view': None,
                      'w': side, 'h': side, 'depth': 0, 'cut_px': cut_px, 'lod': False, 'frames': False, 'labels': False,
                      'abstract': False, 'visible': [LOW], 'frame_format': 'raw', 'thin': 'keep', 'frame_cache': False})
            deadline = time.monotonic() + 300
            while time.monotonic() < deadline:
                res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
                assert res.get('kind') != 'error', res
                if res.get('kind') == 'frame' and res.get('gen') == 1 and not res.get('refining'):
                    return bytes(res.pop('rgba')), res
            raise AssertionError('own shapes frame timeout')

        drawn, rd = view(left)
        old, ro = view(fixed)
        bare, rb = view(unspread)
        truth, _ = view(free, cut_px=0.0)
        # the lit pixels of a whole frame, by index (the frame is side x side)
        lit_of = lambda pixels: {i // 4 for i in range(0, len(pixels), 4) if pixels[i:i + 4] != BLACK}
        want = lit_of(truth)
        pd, po = rd['density_plan2'], ro['density_plan2']
        assert want and lit_of(drawn) == want and rd['density_floor'] == 0.0 and pd['probes_over'] == 0 and pd['reserve_mb'] > 4, (
            'what pass 1 left: %d px lit, %d cut-free; floor %s, plan %s' % (len(lit_of(drawn)), len(want), rd['density_floor'], pd))
        # the fixed reserve: the zero floor's probe over, the 1 px floor's page
        # spread by its occupancy grid - about the cut-free count; without the
        # spread none (0.12.270's case)
        assert abs(len(lit_of(old)) - len(want)) <= len(want) // 4 and po['probes_over'] == 1 and po['reserve_mb'] == 4 and po['occ_pages'] >= 1, (
            'the fixed reserve: %d px lit (cut-free %d), plan %s' % (len(lit_of(old)), len(want), po))
        assert not lit_of(bare) and rb['density_plan2']['probes_over'] == 1, ('the fixed reserve, the spread off: %d px lit' % len(lit_of(bare)), rb['density_plan2'])
        print('density stack: pass 2 takes what pass 1 left - %d MB, the TOP\'s own boxes under a pixel lit as cut-free (%d px) at '
              'depth 0; the fixed 4 MB (floor probe over) spreads them by the index, %d px - the spread off none'
              % (pd['reserve_mb'], len(want), len(lit_of(old))))
    finally:
        for w in (left, fixed, free, unspread):
            w.stop()


def ladder_layout(path):
    """Pass 2's work on both sides of the budget: a 0.1 um VIA cell placed at
    20,000 random places over 300 um (cells under the cut: dots, no page) and
    the TOP's own 60,000 boxes of 0.3-0.9 um (1-3 px at 1000 px: records under
    the cut over the 1 px floor - one page of 55,164 records, 10.6 MB by
    estimate)."""
    import random
    import klayout.db as kdb
    ly = kdb.Layout()
    ly.dbu = 0.001
    top = ly.create_cell('TOP')
    layer = ly.layer(*LOW)
    rnd = random.Random(13)
    for _ in range(60_000):
        x, y = rnd.randrange(300_000) / 1000.0, rnd.randrange(300_000) / 1000.0
        w, h = 0.3 + rnd.randrange(600) / 1000.0, 0.3 + rnd.randrange(600) / 1000.0
        top.shapes(layer).insert(kdb.DBox(x, y, x + w, y + h))
    via = ly.create_cell('VIA')
    via.shapes(ly.layer(*MID)).insert(kdb.DBox(0, 0, 0.1, 0.1))
    for _ in range(20_000):
        top.insert(kdb.DCellInstArray(via.cell_index(), kdb.DTrans(kdb.DVector(rnd.randrange(300_000) / 1000.0, rnd.randrange(300_000) / 1000.0))))
    options = kdb.SaveLayoutOptions()
    options.format = 'OASIS'
    options.oasis_compression_level = 10
    ly.write(str(path), options)


def ladder_checks(temp):
    """Pass 2's budget fit keeps the cells' cut and fits its pages in one
    pass (user 2026-10-02, field: the top cell at depth 1, `fit 75019 ms x6
    passes on 1 threads … cell dots 219.3M` - its pages past eight reserves
    sent the fit up the cut ladder, the cells' cut with the pages', the view
    walked again each step, and the threads' merged plan to the one plan).
    Under a 4 MB budget (a fixed reserve of 0.5 MB) the TOP's page, 10.6 MB,
    is past eight reserves and fits none: one pass, the page left out
    (thinned), the VIAs' dots; two threads draw what one draws; the ladder
    (FLOE_RUST_DENSITY_FIT_LADDER=on) takes passes up the cut to the same
    frame. The default budget decodes the page."""
    src = Path(temp) / 'ladder.oas'
    ladder_layout(src)
    done = subprocess.run([FLOE2, 'index', str(src)],
                          cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
    assert done.returncode == 0, done.stdout + done.stderr
    env = {'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on'}
    tight = dict(env, FLOE_RUST_BUDGET_MB='4', FLOE_RUST_DENSITY_RESERVE_LEFT='off')
    workers = {'roomy': worker(src, env), 'one': worker(src, dict(tight, FLOE_RUST_DENSITY_PLAN_THREADS='1')),
               'two': worker(src, dict(tight, FLOE_RUST_DENSITY_PLAN_THREADS='2')),
               'ladder': worker(src, dict(tight, FLOE_RUST_DENSITY_PLAN_THREADS='1', FLOE_RUST_DENSITY_FIT_LADDER='on'))}
    try:
        dbu = float(workers['roomy'].cache.meta['dbu'])
        side = 1000

        def view(w):
            w.submit({'kind': 'render', 'gen': 1, 'scope': 'headless', 'bbox': (0.0, 0.0, 300.2 / dbu, 300.2 / dbu), 'view': None,
                      'w': side, 'h': side, 'depth': None, 'cut_px': 3.0, 'lod': False, 'frames': False, 'labels': False,
                      'abstract': False, 'visible': [LOW, MID], 'frame_format': 'raw', 'thin': 'keep', 'frame_cache': False})
            deadline = time.monotonic() + 300
            while time.monotonic() < deadline:
                res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
                assert res.get('kind') != 'error', res
                if res.get('kind') == 'frame' and res.get('gen') == 1 and not res.get('refining'):
                    return bytes(res.pop('rgba')), res
            raise AssertionError('ladder frame timeout')

        frames = {name: view(w) for name, w in workers.items()}
        plans = {name: res['density_plan2'] for name, (_, res) in frames.items()}
        lit = {name: res['density_stack']['lit'] for name, (_, res) in frames.items()}
        one, two, ladder = plans['one'], plans['two'], plans['ladder']
        # one pass a side: by cells of the free space the top plane's layer
        # and the other plane's plan apart (FLOE_RUST_DENSITY_FREE_CELLS,
        # 2026-10-03), each fitted once
        assert (one['passes'], one['thinned'], two['passes'], two['threads'], two['thinned']) == (2, 1, 2, 2, 1), plans
        assert ladder['passes'] > 1 and ladder['items'] == one['items'] > 0, plans
        for name in ('two', 'ladder'):
            assert frames[name][0] == frames['one'][0], '%s draws otherwise than one thread in %d px' % (
                name, sum(1 for i in range(0, len(frames['one'][0]), 4) if frames[name][0][i:i + 4] != frames['one'][0][i:i + 4]))
        assert frames['roomy'][1]['density_pages']['decoded'] > 0 and lit['roomy'] > lit['one'], (frames['roomy'][1]['density_pages'], lit)
        print('density stack: pass 2 past eight reserves fits in one pass (two threads alike, %d px of VIA dots, the page left out); '
              'the ladder %d passes to the same frame; the default budget %d px with the page' % (lit['one'], ladder['passes'], lit['roomy']))
    finally:
        for w in workers.values():
            w.stop()


def occ_layout(path):
    """A TOP whose own shapes are 40,000 boxes of 0.3 um (0.5 px at 1000 px
    over 600 um) in two 100 um squares at the corners of a 600 um span - the
    writer makes point lists of them, one page whose box is the span."""
    import random
    import klayout.db as kdb
    ly = kdb.Layout()
    ly.dbu = 0.001
    top = ly.create_cell('TOP')
    layer = ly.layer(*LOW)
    rnd = random.Random(17)
    for x0, y0 in ((0, 0), (500, 500)):
        for _ in range(20_000):
            x, y = x0 + rnd.randrange(100_000) / 1000.0, y0 + rnd.randrange(100_000) / 1000.0
            top.shapes(layer).insert(kdb.DBox(x, y, x + 0.3, y + 0.3))
    options = kdb.SaveLayoutOptions()
    options.format = 'OASIS'
    options.oasis_compression_level = 10
    ly.write(str(path), options)


def occ_checks(temp):
    """A page under the floor spreads its dots over the cells of its
    occupancy grid (design.ovb) that hold a shape (user 2026-10-02: the page
    spread "filled places where nothing is"; then "go with the occupancy
    bits"). The two squares' page at depth 0 with the page spread on
    (FLOE_RUST_DENSITY_PAGE_SPREAD=on): the index writes design.ovb (64 bytes
    and 512 a page), the frame lights nothing between the squares - as a
    cut-free frame - and places the page by its grid (density_plan2
    occ_pages); FLOE_RUST_DENSITY_PAGE_OCC=off (the kill switch) spreads it
    over its box, the space between lit; a cache indexed with
    --no-page-occupancy, and that cache holding another index's design.ovb,
    draw that frame."""
    import shutil
    src = Path(temp) / 'occ.oas'
    occ_layout(src)
    bare = Path(temp) / 'occ_bare.oas'
    shutil.copyfile(src, bare)
    # an hour older: the other index's source by its mtime
    when = src.stat().st_mtime - 3600
    os.utime(bare, (when, when))
    for path, extra in ((src, []), (bare, ['--no-page-occupancy'])):
        done = subprocess.run([FLOE2, 'index', str(path)] + extra,
                              cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
        assert done.returncode == 0, done.stdout + done.stderr
    ice, bare_ice = Path(temp) / '.occ.oas.ice', Path(temp) / '.occ_bare.oas.ice'
    pages = int.from_bytes((ice / 'design.ovm').read_bytes()[52:56], 'little')
    # v2: a header, each page's grid of levels deflated, the page table
    ovb = (ice / 'design.ovb').read_bytes()
    assert ovb[:8] == b'FLOEOVB1' and int.from_bytes(ovb[8:12], 'little') == 2 and int.from_bytes(ovb[16:20], 'little') == pages, ovb[:24]
    assert len(ovb) < 64 + 8 * (pages + 1) + 2048 * pages and not (bare_ice / 'design.ovb').exists(), (len(ovb), pages)
    # the page spread is on by default (0.12.277) where the index has
    # design.ovb; FLOE_RUST_DENSITY_PAGE_SPREAD=on spreads a page with no
    # record over its box too. The page shows 1000 px here, cells of 15.6 px:
    # spread, not decoded (FLOE_RUST_DENSITY_OCC_DECODE=off; coarse_checks)
    env = {'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on', 'FLOE_RUST_DENSITY_OCC_DECODE': 'off'}
    boxes = dict(env, FLOE_RUST_DENSITY_PAGE_SPREAD='on')
    workers = {'occ': worker(src, env), 'box': worker(src, dict(boxes, FLOE_RUST_DENSITY_PAGE_OCC='off')),
               'bare': worker(bare, boxes), 'bare_default': worker(bare, env), 'truth': worker(src, {})}
    try:
        side = 1000

        def view(w, cut_px=3.0):
            dbu = float(w.cache.meta['dbu'])
            w.submit({'kind': 'render', 'gen': 1, 'scope': 'headless', 'bbox': (0.0, 0.0, 600.3 / dbu, 600.3 / dbu), 'view': None,
                      'w': side, 'h': side, 'depth': 0, 'cut_px': cut_px, 'lod': False, 'frames': False, 'labels': False,
                      'abstract': False, 'visible': [LOW], 'frame_format': 'raw', 'thin': 'keep', 'frame_cache': False})
            deadline = time.monotonic() + 300
            while time.monotonic() < deadline:
                res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
                assert res.get('kind') != 'error', res
                if res.get('kind') == 'frame' and res.get('gen') == 1 and not res.get('refining'):
                    return bytes(res.pop('rgba')), res
            raise AssertionError('occupancy frame timeout')

        def lit_in(pixels, lo, hi):
            return sum(1 for r in range(lo, hi) for c in range(lo, hi) if pixels[(r * side + c) * 4:(r * side + c) * 4 + 4] != BLACK)

        frames = {name: view(w, 0.0 if name == 'truth' else 3.0) for name, w in workers.items()}
        lit = {name: res['density_stack']['lit'] if res.get('density_stack') else None for name, (_, res) in frames.items()}
        occ_pages = {name: (res.get('density_plan2') or {}).get('occ_pages') for name, (_, res) in frames.items()}
        # the space between the squares: 200-800 px (120-480 um) on both axes
        between = {name: lit_in(pixels, 200, 800) for name, (pixels, _) in frames.items()}
        assert between['truth'] == 0 and lit_in(frames['truth'][0], 0, side) > 0, between
        assert between['occ'] == 0 and lit['occ'] > 0 and occ_pages['occ'] >= 1, (between, lit, occ_pages)
        assert between['box'] > 0 and occ_pages['box'] == 0, (between, occ_pages)
        assert frames['bare'][0] == frames['box'][0] and occ_pages['bare'] == 0, (between, occ_pages)
        # no design.ovb, by default: the page under the floor is not drawn
        assert lit_in(frames['bare_default'][0], 0, side) == 0 and not occ_pages['bare_default'], (lit, occ_pages)
        # another index's design.ovb (the other source's mtime): left out
        shutil.copyfile(ice / 'design.ovb', bare_ice / 'design.ovb')
        stale = worker(bare, boxes)
        try:
            pixels, res = view(stale)
            assert pixels == frames['box'][0] and res['density_plan2']['occ_pages'] == 0, res['density_plan2']
        finally:
            stale.stop()
        print('density stack: a page under the floor spread over its occupancy grid (%d pages, design.ovb %d B) lights %d px, none '
              'between its squares (cut-free none); over its box %d px between; no design.ovb, or another index\'s, as over the box'
              % (pages, len(ovb), lit['occ'], between['box']))
    finally:
        for w in workers.values():
            w.stop()
    coarse_checks(temp, src)
    mixed_checks(temp)


def coarse_checks(temp, src):
    """A page under the floor too large on screen for its occupancy cells is
    decoded (HierOpts::dot_occ_decode; user 2026-10-03, the field chip: dots
    "where there is no shape"). occ_checks' page at 1 um a px - 600 px, cells
    of 9.4 px, its 0.3 um boxes under the 1 px floor: drawn as a cut-free frame
    draws them (density_plan2 occ_decoded); FLOE_RUST_DENSITY_OCC_DECODE=off
    spreads it; under a budget whose fit leaves the page out (a fixed 128 KB
    reserve) the dots its spread made stand in - as many as the spread lights."""
    env = {'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on'}
    tight = dict(env, FLOE_RUST_BUDGET_MB='1', FLOE_RUST_DENSITY_RESERVE_LEFT='off')
    workers = {'decode': worker(src, env), 'spread': worker(src, dict(env, FLOE_RUST_DENSITY_OCC_DECODE='off')),
               'tight': worker(src, tight), 'tight_spread': worker(src, dict(tight, FLOE_RUST_DENSITY_OCC_DECODE='off')),
               'truth': worker(src, {})}
    try:
        side = 1000

        def view(w, cut_px=3.0):
            dbu = float(w.cache.meta['dbu'])
            w.submit({'kind': 'render', 'gen': 1, 'scope': 'headless', 'bbox': (0.0, 0.0, 1000.0 / dbu, 1000.0 / dbu), 'view': None,
                      'w': side, 'h': side, 'depth': 0, 'cut_px': cut_px, 'lod': False, 'frames': False, 'labels': False,
                      'abstract': False, 'visible': [LOW], 'frame_format': 'raw', 'thin': 'keep', 'frame_cache': False})
            deadline = time.monotonic() + 300
            while time.monotonic() < deadline:
                res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
                assert res.get('kind') != 'error', res
                if res.get('kind') == 'frame' and res.get('gen') == 1 and not res.get('refining'):
                    return bytes(res.pop('rgba')), res
            raise AssertionError('coarse frame timeout')

        lit_of = lambda pixels: {i // 4 for i in range(0, len(pixels), 4) if pixels[i:i + 4] != BLACK}
        frames = {name: view(w, 0.0 if name == 'truth' else 3.0) for name, w in workers.items()}
        lit = {name: lit_of(px) for name, (px, _) in frames.items()}
        plan2 = {name: res.get('density_plan2') or {} for name, (_, res) in frames.items()}
        assert lit['truth'] and lit['decode'] == lit['truth'] and plan2['decode']['occ_decoded'] >= 1, (
            len(lit['decode']), len(lit['truth']), plan2['decode'])
        assert lit['spread'] != lit['truth'] and plan2['spread']['occ_decoded'] == 0 and plan2['spread']['occ_pages'] >= 1, plan2['spread']
        assert plan2['tight']['occ_decoded'] == 0 and plan2['tight']['occ_pages'] >= 1 and lit['tight'] != lit['truth'], plan2['tight']
        assert abs(len(lit['tight']) - len(lit['tight_spread'])) <= len(lit['tight_spread']) // 50, (len(lit['tight']), len(lit['tight_spread']))
        print('density stack: a page under the floor too coarse for its cells (9.4 px) decoded - %d px as cut-free; spread %d px; '
              'left out by a 128 KB reserve, its spread\'s dots stand in, %d px (the spread %d)'
              % (len(lit['decode']), len(lit['spread']), len(lit['tight']), len(lit['tight_spread'])))
    finally:
        for w in workers.values():
            w.stop()


def mixed_layout(path):
    """One page of two sizes: TOP's own 20,000 boxes of 0.2 um at random over
    the lower half of 300 x 300 um (2 % of it covered) and 30,000 of 1.2 um over
    the upper half (about all of it)."""
    import random
    import klayout.db as kdb
    ly = kdb.Layout()
    ly.dbu = 0.001
    top = ly.create_cell('TOP')
    layer = ly.layer(*LOW)
    rnd = random.Random(23)
    for side, y0, n in ((0.2, 0, 20_000), (1.2, 150, 30_000)):
        for _ in range(n):
            x, y = rnd.randrange(300_000) / 1000.0, y0 + rnd.randrange(150_000) / 1000.0
            top.shapes(layer).insert(kdb.DBox(x, y, x + side, y + side))
    options = kdb.SaveLayoutOptions()
    options.format = 'OASIS'
    options.oasis_compression_level = 10
    ly.write(str(path), options)


def mixed_checks(temp):
    """A page's dots are what its shapes cover, cell by cell (design.ovb v2;
    user 2026-10-03, the routing chip's fill: 0.3 um squares paged with 1 um
    array squares were blank where the page was decoded and, one zoom step out,
    as dense as the arrays - "without their density"). The two sizes' page at
    depth 0 with the page spread on, 1000 px:
      * over 300 um (0.3 um a px): the 1.2 um boxes are pass 1's and the page
        decoded - its 0.2 um boxes, under the floor, light the lower half as a
        cut-free frame does (FLOE_RUST_DENSITY_UNDER_FLOOR=drop: none);
      * over 1,600 um (1.6 um a px): every box is under the floor and the page
        spread - the lower half's dots are what its boxes cover (within a
        third of the cut-free frame's) where an even share of the members at
        the largest box's area (FLOE_RUST_DENSITY_OCC_COVER=off) lights it
        three times over."""
    src = Path(temp) / 'mixed.oas'
    mixed_layout(src)
    done = subprocess.run([FLOE2, 'index', str(src)],
                          cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
    assert done.returncode == 0, done.stdout + done.stderr
    # the page spread on by default (0.12.277): the index has design.ovb
    env = {'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on'}
    workers = {'new': worker(src, env), 'drop': worker(src, dict(env, FLOE_RUST_DENSITY_UNDER_FLOOR='drop')),
               'even': worker(src, dict(env, FLOE_RUST_DENSITY_OCC_COVER='off')), 'truth': worker(src, {})}
    try:
        side = 1000

        def view(w, gen, span, cut_px=3.0):
            dbu = float(w.cache.meta['dbu'])
            w.submit({'kind': 'render', 'gen': gen, 'scope': 'headless', 'bbox': (0.0, 0.0, span / dbu, span / dbu), 'view': None,
                      'w': side, 'h': side, 'depth': 0, 'cut_px': cut_px, 'lod': False, 'frames': False, 'labels': False,
                      'abstract': False, 'visible': [LOW], 'frame_format': 'raw', 'thin': 'keep', 'frame_cache': False})
            deadline = time.monotonic() + 300
            while time.monotonic() < deadline:
                res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
                assert res.get('kind') != 'error', res
                if res.get('kind') == 'frame' and res.get('gen') == gen and not res.get('refining'):
                    return bytes(res.pop('rgba')), res
            raise AssertionError('mixed frame timeout')

        def lit(pixels, c0, r0, c1, r1):
            return sum(1 for r in range(r0, r1) for c in range(c0, c1) if pixels[(r * side + c) * 4:(r * side + c) * 4 + 4] != BLACK)

        # the lower half on screen (rows from the top): over 300 um rows 520-980,
        # over 1,600 um the layout is the bottom left 188 px, its lower half rows
        # 909-996, columns 4-184
        near = {name: view(w, 1, 300.0, 0.0 if name == 'truth' else 3.0) for name, w in workers.items() if name != 'even'}
        low = {name: lit(px, 20, 520, 980, 980) for name, (px, _) in near.items()}
        assert near['new'][1]['density_pages']['decoded'] + near['new'][1]['density_pages']['in_hand'] > 0, near['new'][1]['density_pages']
        assert low['truth'] > 1000 and abs(low['new'] - low['truth']) <= low['truth'] // 4 and low['drop'] == 0, low
        far = {name: view(w, 2, 1600.0, 0.0 if name == 'truth' else 3.0) for name, w in workers.items() if name != 'drop'}
        low_far = {name: lit(px, 4, 909, 184, 996) for name, (px, _) in far.items()}
        assert far['new'][1]['density_plan2']['occ_pages'] >= 1, far['new'][1]['density_plan2']
        assert low_far['truth'] > 0 and abs(low_far['new'] - low_far['truth']) <= low_far['truth'] // 3 and low_far['even'] >= 3 * low_far['truth'], low_far
        print('density stack: a page of 0.2 and 1.2 um boxes - decoded, its 0.2 um ones light %d px (cut-free %d, dropped 0); spread, '
              'the 0.2 um half %d px by its cells\' cover (cut-free %d, an even share %d)'
              % (low['new'], low['truth'], low_far['new'], low_far['truth'], low_far['even']))
    finally:
        for w in workers.values():
            w.stop()


OCC_DOT = 0.5               # um: the occupancy density's DOT square
OCC_PITCH = 3.0             # um: its array's pitch (not a multiple of the checker's 2 px)
OCC_ARRAY = (66, 33)        # its columns and rows: x 0-198, y 0-99 um
OCC_BIG = (220.0, 20.0, 300.0, 100.0)  # TOP's own 3/0 box, over the cut
OCC_SPECKS = (310.0, 20.0, 3.0, 26)    # TOP's own 3/0 0.4 um squares: x, y, pitch, n per side
# (pitches of 3 px: at 2 every member falls on the dot checker's other parity)


def occ_density_layout(path):
    """A 1/0 DOT cell (a 0.5 um square) placed as a 66 x 33 array at 3 um,
    and TOP's own 3/0: an 80 um box and 26 x 26 squares of 0.4 um at 3 um
    beside it - a page holding a shape over the cut and many under it."""
    import klayout.db as kdb
    ly = kdb.Layout()
    ly.dbu = 0.001
    top = ly.create_cell('TOP')
    dot = ly.create_cell('DOT')
    dot.shapes(ly.layer(*LOW)).insert(kdb.DBox(0.25, 0.25, 0.25 + OCC_DOT, 0.25 + OCC_DOT))
    pitch = int(OCC_PITCH * 1000)
    top.insert(kdb.CellInstArray(dot.cell_index(), kdb.Trans(), kdb.Vector(pitch, 0), kdb.Vector(0, pitch), *OCC_ARRAY))
    alone = ly.layer(*ALONE)
    top.shapes(alone).insert(kdb.DBox(*OCC_BIG))
    x0, y0, step, n = OCC_SPECKS
    for j in range(n):
        for i in range(n):
            x, y = x0 + i * step + 0.3, y0 + j * step + 0.3
            top.shapes(alone).insert(kdb.DBox(x, y, x + 0.4, y + 0.4))
    ly.write(str(path))


def occ_density_checks(temp):
    """Pass 2 from the occupancy density (FLOE_RUST_DENSITY_OCC=on, user
    2026-10-06: "push it, I will try it on a real chip"; then, design.ovo
    taking 3,362 s on the synthetic chip: "this won't do"): `floe-index ovs`
    adds design.ovs to a plain index, its bits and means in one walk (a cache
    without design.ovb refused), the same bytes built twice; at 1 um a pixel
    over 1 um cells the frame draws pass 2 with no plan (density_plan2
    occ_layers 2, occ_cell_nm 1000, no region, no node) - the DOT array's
    dots within its extent and in 1/0's colour, the 3/0 squares' within
    theirs, though their page holds a box over the cut (decoded at the build
    for its smaller shapes), nothing elsewhere but the box pass 1 draws, the
    same frame over other tiles and raster workers and with the layers made
    one at a time (FLOE_RUST_DENSITY_OCC_THREADS=1); at depth 0 TOP's own
    squares alone; the plans draw a view whose cells pass
    FLOE_RUST_DENSITY_OCC_PX pixels, one whose layers pass
    FLOE_RUST_DENSITY_OCC_MB, and a cache without design.ovs - that one byte
    for byte the frame without the switch."""
    import shutil
    src = Path(temp) / 'occd.oas'
    occ_density_layout(src)
    plain = Path(temp) / 'occd_plain.oas'
    shutil.copyfile(src, plain)
    said = {}
    for path, extra in ((src, []), (plain, ['--no-page-occupancy'])):
        done = subprocess.run([FLOE2, 'index', str(path)] + extra,
                              cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
        assert done.returncode == 0, done.stdout + done.stderr
        said[path] = done.stderr
    ice, plain_ice = Path(temp) / '.occd.oas.ice', Path(temp) / '.occd_plain.oas.ice'
    assert not (ice / 'design.ovo').exists(), 'the occupancy density needs no design.ovo'
    # the index makes design.ovs (user 2026-10-07: "include ovs in indexing
    # by default"); without design.ovb it says why it made none
    assert (ice / 'design.ovs').read_bytes()[:8] == b'FLOEOVS1' and '[vfs] ovs design.ovs: ' in said[src], said[src][-2000:]
    assert not (plain_ice / 'design.ovs').exists() and '[vfs] ovs: none' in said[plain], said[plain][-2000:]
    # indexed again with --no-ovs: the last one gone, none made
    again = subprocess.run([FLOE2, 'index', str(src), '--force', '--no-ovs'],
                           cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
    assert again.returncode == 0 and not (ice / 'design.ovs').exists() and '[vfs] ovs' not in again.stderr, again.stderr[-2000:]
    index_bin = os.environ['FLOE_INDEX_BIN']
    refused = subprocess.run([index_bin, 'ovs', str(plain_ice)], capture_output=True, text=True, timeout=600)
    assert refused.returncode == 1 and 'design.ovb' in refused.stderr and not (plain_ice / 'design.ovs').exists(), refused.stderr
    built = subprocess.run([index_bin, 'ovs', str(ice), '--um', '1'], capture_output=True, text=True, timeout=600)
    assert built.returncode == 0 and (ice / 'design.ovs').read_bytes()[:8] == b'FLOEOVS1', built.stdout + built.stderr
    stats = dict(kv.split('=', 1) for kv in built.stdout.split()[1:])
    assert int(stats['big_pages']) >= 1 and int(stats['decoded']) >= 1 and stats['base_um'] == '1', stats
    # where the build is (user 2026-10-07: no log for 11 minutes on the real
    # chip): its phases, and at FLOE_OVS_PROGRESS_S=0 the long ones' lines
    for phase in ('index open', 'grid 1 um', 'decoded', 'walked in', 'settled in', 'written in'):
        assert '[ovs] ' in built.stderr and phase in built.stderr, (phase, built.stderr)
    first = (ice / 'design.ovs').read_bytes()
    again = subprocess.run([index_bin, 'ovs', str(ice), '--um', '1', '--jobs', '1'], capture_output=True, text=True, timeout=600,
                           env=dict(os.environ, FLOE_OVS_PROGRESS_S='0'))
    assert again.returncode == 0 and (ice / 'design.ovs').read_bytes() == first, 'design.ovs differs built again (one decode thread)'
    for beat in ('listing the pages to decode: ', 'settle: 0/', 'write: layer 0/'):
        assert '[ovs] ' + beat in again.stderr, (beat, again.stderr)
    env = {'FLOE_RUST_DENSITY_STACK': 'top', 'FLOE_RUST_DENSITY_DOTS': 'on', 'FLOE_RUST_DENSITY_BRIGHT': 'on',
           'FLOE_RUST_DENSITY_PATTERN': None, 'FLOE_RUST_DENSITY_TOP_GROUP': 'on', 'FLOE_RUST_DENSITY_SHAPES_FIRST': 'on',
           'FLOE_RUST_DENSITY_STAGES': 'off', 'FLOE_RUST_TILE_PX': '64', 'FLOE_RUST_RASTER_JOBS': '1'}
    occ = dict(env, FLOE_RUST_DENSITY_OCC='on')
    workers = {'walk': worker(src, env), 'occ': worker(src, occ),
               'tiles': worker(src, dict(occ, FLOE_RUST_TILE_PX='127', FLOE_RUST_RASTER_JOBS='4')),
               'serial': worker(src, dict(occ, FLOE_RUST_DENSITY_OCC_THREADS='1')),
               'capped': worker(src, dict(occ, FLOE_RUST_DENSITY_OCC_MB='0.0001')),
               'default': worker(src, dict(env, FLOE_RUST_DENSITY_OCC=None))}
    side_w, side_h = 400, 200

    def view(w, gen, box_um=(0.0, 0.0, 400.0, 200.0), depth=None):
        dbu = float(w.cache.meta['dbu'])
        w.submit({'kind': 'render', 'gen': gen, 'scope': 'headless', 'bbox': tuple(v / dbu for v in box_um), 'view': None,
                  'w': side_w, 'h': side_h, 'depth': depth, 'cut_px': 3.0, 'lod': False, 'frames': False, 'labels': False,
                  'abstract': False, 'visible': [LOW, ALONE], 'frame_format': 'raw', 'thin': 'keep', 'frame_cache': False})
        deadline = time.monotonic() + 300
        while time.monotonic() < deadline:
            res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
            assert res.get('kind') != 'error', res
            if res.get('kind') == 'frame' and res.get('gen') == gen and not res.get('refining'):
                return bytes(res.pop('rgba')), res
        raise AssertionError('occupancy density frame timeout')

    try:
        low_c, alone_c = layer_colour(workers['occ'], LOW), layer_colour(workers['occ'], ALONE)
        # 1 um a pixel, rows from the top: the array's extent, the squares',
        # the box pass 1 draws (a pixel of slack about each)
        cols_um, rows_um = OCC_PITCH * OCC_ARRAY[0], OCC_PITCH * OCC_ARRAY[1]
        array = (range(0, int(cols_um) + 1), range(side_h - int(rows_um) - 1, side_h))
        x0, y0, step, n = OCC_SPECKS
        specks = (range(int(x0) - 1, int(x0 + step * n) + 2), range(side_h - int(y0 + step * n) - 2, side_h - int(y0) + 1))
        box = (range(int(OCC_BIG[0]) - 1, int(OCC_BIG[2]) + 2), range(side_h - int(OCC_BIG[3]) - 1, side_h - int(OCC_BIG[1]) + 2))

        def where(pixels):
            """lit pixels by colour and place: (1/0 in the array, 3/0 in the
            squares, the rest outside the box)"""
            a = s = 0
            rest = []
            for r in range(side_h):
                for c in range(side_w):
                    p = pixels[(r * side_w + c) * 4:(r * side_w + c) * 4 + 4]
                    if p == BLACK or (c in box[0] and r in box[1]):
                        continue
                    if p == low_c and c in array[0] and r in array[1]:
                        a += 1
                    elif p == alone_c and c in specks[0] and r in specks[1]:
                        s += 1
                    else:
                        rest.append((c, r, p))
            return a, s, rest

        walk, walk_res = view(workers['walk'], 1)
        on, on_res = view(workers['occ'], 1)
        tiles, _ = view(workers['tiles'], 1)
        # the switch unset: the occupancy density (user 2026-10-07: "turn
        # OCC on by default")
        default, default_res = view(workers['default'], 1)
        assert default == on and default_res['density_plan2']['occ_layers'] == 2, default_res['density_plan2']
        p2 = on_res['density_plan2']
        assert (p2['occ_layers'], p2['occ_cell_nm'], p2['regions'], p2['nodes']) == (2, 1000, 0, 0), p2
        assert walk_res['density_plan2']['occ_layers'] == 0 and walk_res['density_plan2']['regions'] > 0, walk_res['density_plan2']
        a, s, rest = where(on)
        wa, ws, wrest = where(walk)
        assert a > 0 and s > 0 and not rest, (a, s, rest[:5])
        assert not wrest and wa > 0 and ws > 0, (wa, ws, wrest[:5])
        # as many dots as the walk's within a few times: the DOT array at
        # its cover, about a quarter of its cells' pixels (2178 cells); the
        # squares at theirs (a sixth of 676), where the walk lights half
        assert 300 <= a <= 900 and 0.25 * wa <= a <= 4 * wa and 0.15 * ws <= s <= 2 * ws, (a, wa, s, ws)
        assert tiles == on, 'the occupancy density differs over other tiles and workers'
        serial, serial_res = view(workers['serial'], 1)
        assert serial == on and serial_res['density_plan2']['occ_layers'] == 2, 'the layers made one at a time differ'
        # depth 0: TOP's own squares, not the DOT cells a level down
        top_only, top_res = view(workers['occ'], 2, depth=0)
        ta, ts, trest = where(top_only)
        assert top_res['density_plan2']['occ_layers'] == 1 and ta == 0 and ts == s and not trest, (top_res['density_plan2'], ta, ts, s)
        # 4 px a cell, past the 2 allowed: the plans
        close, close_res = view(workers['occ'], 3, box_um=(0.0, 0.0, 100.0, 50.0))
        close_walk, _ = view(workers['walk'], 3, box_um=(0.0, 0.0, 100.0, 50.0))
        assert close_res['density_plan2']['occ_layers'] == 0 and close == close_walk, close_res['density_plan2']
        # the layers past the cap at every level within two pixels: the plans
        capped, capped_res = view(workers['capped'], 4)
        assert capped_res['density_plan2']['occ_layers'] == 0 and capped == walk, capped_res['density_plan2']
        print('density stack: occupancy density (design.ovs %d B, %s pages decoded) draws pass 2 with no plan - %d DOT px, %d square px '
              '(the walk %d, %d), none elsewhere, the same over tiles; depth 0 its own squares alone; 4 px cells and the cap plan'
              % ((ice / 'design.ovs').stat().st_size, stats['decoded'], a, s, wa, ws))
    finally:
        for w in workers.values():
            w.stop()
    occ_cost_checks(src, ice, env, occ)
    # no design.ovs: the plans, byte for byte the frame without the switch
    (ice / 'design.ovs').unlink()
    bare = worker(src, occ)
    try:
        pixels, res = view(bare, 1)
        assert res['density_plan2']['occ_layers'] == 0 and pixels == walk, res['density_plan2']
    finally:
        bare.stop()
    print('density stack: no design.ovs - the walk\'s frame; floe-index ovs refuses a cache without design.ovb, builds the same bytes again')
    occ_review_checks(temp, env, occ)
    occ_root_checks(temp, env, occ)


def occ_cost_checks(src, ice, env, occ):
    """What the occupancy density costs (review 2026-10-07): a frame pass 1
    covers - the 3/0 box filling the view - makes no layer (density_plan2
    occ_made 0, the walk's frame byte for byte); the layers turned on and off
    at one zoom keep the cache within FLOE_RUST_DENSITY_OCC_MB - one layer's
    worth and a half: 1/0, 3/0, 1/0 each made again (the first let go for the
    second), the cache at most a layer, the frames those the default cap
    draws."""
    import struct
    head = (ice / 'design.ovs').read_bytes()[:80]
    w, h = struct.unpack_from('<II', head, 64)
    layer = -(-w // 8) * (h + -(-h // 8))
    cap = layer * 3 // 2
    workers = {'walk': worker(src, env), 'occ': worker(src, occ),
               'capped': worker(src, dict(occ, FLOE_RUST_DENSITY_OCC_MB=repr(cap / 1048576.0)))}

    def view(w, gen, box_um, size, visible):
        dbu = float(w.cache.meta['dbu'])
        w.submit({'kind': 'render', 'gen': gen, 'scope': 'headless', 'bbox': tuple(v / dbu for v in box_um), 'view': None,
                  'w': size[0], 'h': size[1], 'depth': None, 'cut_px': 3.0, 'lod': False, 'frames': False, 'labels': False,
                  'abstract': False, 'visible': list(visible), 'frame_format': 'raw', 'thin': 'keep', 'frame_cache': False})
        deadline = time.monotonic() + 300
        while time.monotonic() < deadline:
            res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
            assert res.get('kind') != 'error', res
            if res.get('kind') == 'frame' and res.get('gen') == gen and not res.get('refining'):
                return bytes(res.pop('rgba')), res
        raise AssertionError('occupancy cost frame timeout')

    try:
        covered = ((225.0, 25.0, 295.0, 95.0), (70, 70), (ALONE,))
        walked, _ = view(workers['walk'], 1, *covered)
        drawn, res = view(workers['occ'], 1, *covered)
        p2 = res['density_plan2']
        assert drawn == walked and p2['occ_made'] == 0 and p2['occ_layers'] == 0, p2
        whole = ((0.0, 0.0, 400.0, 200.0), (400, 200))
        made, held = [], []
        for gen, visible in enumerate(((LOW,), (ALONE,), (LOW,)), 2):
            free, _ = view(workers['occ'], gen, *whole, visible)
            capped, res = view(workers['capped'], gen, *whole, visible)
            p2 = res['density_plan2']
            assert capped == free and p2['occ_layers'] == 1, (visible, p2)
            made.append(p2['occ_made'])
            held.append(p2['occ_cache_kb'])
        assert made == [1, 1, 1] and all(kb * 1024 <= cap for kb in held), (made, held, cap)
    finally:
        for w in workers.values():
            w.stop()
    print('density stack: occupancy density - a covered frame makes no layer (the walk\'s frame); layers turned on and off '
          'keep its cache within the cap (%d B: made %s, held %s KiB)' % (cap, made, held))


def ovs_table(path):
    """design.ovs's table (version 3): (version, n_levels, per layer [(depth,
    [(bits off, len, means off, len) per level])])."""
    import struct
    data = Path(path).read_bytes()
    assert data[:8] == b'FLOEOVS1', data[:8]
    version, n_levels, n_layers = struct.unpack_from('<I', data, 8)[0], *struct.unpack_from('<II', data, 72)
    at, layers = 80, []
    for _ in range(n_layers):
        n_planes, at = data[at], at + 1
        planes = []
        for _ in range(n_planes):
            depth, at = data[at], at + 1
            planes.append((depth, [struct.unpack_from('<4Q', data, at + 32 * lv) for lv in range(n_levels)]))
            at += 32 * n_levels
        layers.append(planes)
    return version, n_levels, layers


CUT_SQUARES = (2.0, 2.0, 8.0, 40, 20)       # um: x, y, pitch, columns, rows of 3.5 um squares (1/0)
CUT_WIRES = (330.0, 100.0, 2.0, 32)         # um: x, y, pitch, count of 0.4 x 32 um wires (2/0)


def occ_cut_layout(path, tiny_path):
    """1/0: 3.5 um squares at 8 um, a 40 x 20 array - under a 3 px cut from
    1.17 um a pixel on, over the 3 um cut of 1 um cells; 2/0: 32 wires of
    0.4 x 32 um at 2 um - under the cut by their smaller side alone. And a
    TOP that is a 0.4 um square of 1/0 alone."""
    import klayout.db as kdb
    ly = kdb.Layout()
    ly.dbu = 0.001
    top = ly.create_cell('TOP')
    x0, y0, pitch, nx, ny = CUT_SQUARES
    low = ly.layer(*LOW)
    for j in range(ny):
        for i in range(nx):
            x, y = x0 + i * pitch, y0 + j * pitch
            top.shapes(low).insert(kdb.DBox(x, y, x + 3.5, y + 3.5))
    x0, y0, pitch, n = CUT_WIRES
    mid = ly.layer(*MID)
    for i in range(n):
        x = x0 + i * pitch
        top.shapes(mid).insert(kdb.DBox(x, y0, x + 0.4, y0 + 32.0))
    ly.write(str(path))
    tiny = kdb.Layout()
    tiny.dbu = 0.001
    tiny.create_cell('TOP').shapes(tiny.layer(*LOW)).insert(kdb.DBox(1.0, 1.0, 1.4, 1.4))
    tiny.write(str(tiny_path))


def occ_review_checks(temp, env, occ):
    """The occupancy density against the frame's own cut (review 2026-10-07,
    each reproduced first): design.ovs classes shapes by their larger side at
    the level whose cut reaches the frame's - 3.5 um squares over 1 um cells
    at 1.5 um a pixel (cut 4.5 um) light dots where they lit none (level 0's 3
    um cut left them out, pass 1's 4.5 um too), and 0.4 x 32 um wires, pass
    1's hairlines, come back as no dot (by their smaller side they did: 639 px
    for 396) - the frame byte for byte the walk's and the one without the
    stack; a TOP under the cut (a 0.4 um square alone) has its plane; a
    design.ovs plane that will not read sends the frame to the plans, byte for
    byte the walk's; a page that will not read fails `floe-index ovs`, the
    last design.ovs left as it was."""
    import shutil
    src, tiny = Path(temp) / 'occcut.oas', Path(temp) / 'occtiny.oas'
    occ_cut_layout(src, tiny)
    for path in (src, tiny):
        done = subprocess.run([FLOE2, 'index', str(path)],
                              cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
        assert done.returncode == 0, done.stdout + done.stderr
    ice, tiny_ice = Path(temp) / '.occcut.oas.ice', Path(temp) / '.occtiny.oas.ice'
    index_bin = os.environ['FLOE_INDEX_BIN']
    for cache, extra in ((ice, ['--um', '1']), (tiny_ice, [])):
        built = subprocess.run([index_bin, 'ovs', str(cache)] + extra, capture_output=True, text=True, timeout=600)
        assert built.returncode == 0, built.stdout + built.stderr
    # the tiny TOP: a plane of 1/0 at depth 0 with cells set
    version, _, layers = ovs_table(tiny_ice / 'design.ovs')
    assert version == 3 and any(depth == 0 and levels[0][1] > 0 for planes in layers for depth, levels in planes), layers

    def view(w, gen, box_um, size, visible):
        dbu = float(w.cache.meta['dbu'])
        w.submit({'kind': 'render', 'gen': gen, 'scope': 'headless', 'bbox': tuple(v / dbu for v in box_um), 'view': None,
                  'w': size[0], 'h': size[1], 'depth': None, 'cut_px': 3.0, 'lod': False, 'frames': False, 'labels': False,
                  'abstract': False, 'visible': list(visible), 'frame_format': 'raw', 'thin': 'keep', 'frame_cache': False})
        deadline = time.monotonic() + 300
        while time.monotonic() < deadline:
            res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
            assert res.get('kind') != 'error', res
            if res.get('kind') == 'frame' and res.get('gen') == gen and not res.get('refining'):
                return bytes(res.pop('rgba')), res
        raise AssertionError('occupancy review frame timeout')

    def lit_px(pixels):
        return sum(1 for i in range(0, len(pixels), 4) if pixels[i:i + 4] != BLACK)

    squares = ((0.0, 0.0, 300.0, 150.0), (200, 100), (LOW,))
    wires = ((320.0, 90.0, 400.0, 140.0), (80, 50), (MID,))
    plain = {name: value for name, value in env.items() if not name.startswith('FLOE_RUST_DENSITY')}
    workers = {'walk': worker(src, env), 'occ': worker(src, occ), 'off': worker(src, plain)}
    try:
        sq = {name: view(w, 1, *squares) for name, w in workers.items()}
        p2 = sq['occ'][1]['density_plan2']
        # level 1 (2 um cells, 6 um cut) for the 4.5 um cut
        assert p2['occ_layers'] == 1 and p2['occ_cell_nm'] == 2000, p2
        dots, walk_dots, bare = lit_px(sq['occ'][0]), lit_px(sq['walk'][0]), lit_px(sq['off'][0])
        assert bare == 0 and walk_dots > 0 and 0.25 * walk_dots <= dots <= 4 * walk_dots, (dots, walk_dots, bare)
        wi = {name: view(w, 2, *wires) for name, w in workers.items()}
        assert wi['occ'][1]['density_plan2']['occ_layers'] == 0, wi['occ'][1]['density_plan2']
        assert wi['occ'][0] == wi['walk'][0] == wi['off'][0], 'the wires: %d px with the occupancy density, %d walked, %d without the stack' % (
            lit_px(wi['occ'][0]), lit_px(wi['walk'][0]), lit_px(wi['off'][0]))
    finally:
        for w in workers.values():
            w.stop()
    # a plane that will not read: the squares' level 1 bits (the frame's;
    # level 0 holds none of them) overwritten - the plans draw
    table = ovs_table(ice / 'design.ovs')[2]
    assert table[0] and table[0][0][1][0][1] == 0 < table[0][0][1][1][1], table[0]
    off, length = table[0][0][1][1][:2]
    data = bytearray((ice / 'design.ovs').read_bytes())
    data[off:off + length] = b'\xff' * length
    good = (ice / 'design.ovs').read_bytes()
    (ice / 'design.ovs').write_bytes(bytes(data))
    workers = {'walk': worker(src, env), 'bad': worker(src, occ)}
    try:
        walked, _ = view(workers['walk'], 3, *squares)
        bad, bad_res = view(workers['bad'], 3, *squares)
        assert bad == walked and bad_res['density_plan2']['occ_layers'] == 0, bad_res['density_plan2']
    finally:
        for w in workers.values():
            w.stop()
    (ice / 'design.ovs').write_bytes(good)
    # a page that will not read: the build fails, the last file stays
    broken = Path(temp) / 'occbroken.oas'
    shutil.copy2(src, broken)
    shutil.copytree(ice, Path(temp) / '.occbroken.oas.ice')
    broken_ice = Path(temp) / '.occbroken.oas.ice'
    with open(broken_ice / 'design.ovp', 'r+b') as fh:
        fh.truncate(16)
    failed = subprocess.run([index_bin, 'ovs', str(broken_ice), '--um', '1'], capture_output=True, text=True, timeout=600)
    assert failed.returncode == 1 and 'page' in failed.stderr and (broken_ice / 'design.ovs').read_bytes() == good, (failed.returncode, failed.stderr)
    print('density stack: occupancy density at the frame\'s cut - 3.5 um squares at 1.5 um/px %d px over 2 um cells (walk %d, none '
          'without the stack), 0.4 x 32 um wires the walk\'s frame byte for byte; a TOP under the cut has its plane; a plane that '
          'will not read draws the walk\'s frame; a page that will not read fails the build, the last file kept' % (dots, walk_dots))


ROOT_ARRAY = (40, 20)        # a big cell's DOT array at OCC_PITCH: x 0-118, y 0-58 um
ROOT_SPECKS = (0.0, 70.0, 3.0, 26, 10)  # its own 3/0 0.4 um squares: x, y, pitch, columns, rows - y 70-98 um
ROOT_VIEW = (0.0, 0.0, 120.0, 100.0)    # um: a big cell's box in its own coordinates


def occ_root_layout(path):
    """TOP over 215 x 200 um: BLK - a DOT array and squares of its own -
    turned a quarter (R90) at (100, 0), BLK2 - the same content - mirrored
    at (100, 100) and (100, 200), each a quarter of TOP's box and more; and
    SML, a 5 x 5 DOT array, at (10, 130)."""
    import klayout.db as kdb
    ly = kdb.Layout()
    ly.dbu = 0.001
    top = ly.create_cell('TOP')
    dot = ly.create_cell('DOT')
    dot.shapes(ly.layer(*LOW)).insert(kdb.DBox(0.25, 0.25, 0.25 + OCC_DOT, 0.25 + OCC_DOT))
    pitch = int(OCC_PITCH * 1000)
    alone = ly.layer(*ALONE)
    for name, trans in (('BLK', ((1, False, 100000, 0),)), ('BLK2', ((0, True, 100000, 100000), (0, True, 100000, 200000)))):
        cell = ly.create_cell(name)
        cell.insert(kdb.CellInstArray(dot.cell_index(), kdb.Trans(), kdb.Vector(pitch, 0), kdb.Vector(0, pitch), *ROOT_ARRAY))
        x0, y0, step, nx, ny = ROOT_SPECKS
        for j in range(ny):
            for i in range(nx):
                x, y = x0 + i * step + 0.3, y0 + j * step + 0.3
                cell.shapes(alone).insert(kdb.DBox(x, y, x + 0.4, y + 0.4))
        for rot, mirror, x, y in trans:
            top.insert(kdb.CellInstArray(cell.cell_index(), kdb.Trans(rot, mirror, x, y)))
    sml = ly.create_cell('SML')
    sml.insert(kdb.CellInstArray(dot.cell_index(), kdb.Trans(), kdb.Vector(pitch, 0), kdb.Vector(0, pitch), 5, 5))
    top.insert(kdb.CellInstArray(sml.cell_index(), kdb.Trans(10000, 130000)))
    ly.write(str(path))


def occ_root_checks(temp, env, occ):
    """A view root by its cell's own occupancy density (user 2026-10-07: two
    cells under the field chip's top drew their root views by the plans,
    1.6 s; "record the cells over 25 % as the top unfolds and make them at
    once"): `floe-index ovs` writes design.ovs.<cell> for BLK and BLK2 - a
    quarter of TOP's box and more - in the same walk (roots=2, version 4,
    their placements in it), none for SML; design.ovs the same bytes as with
    --roots 0 (TOP's own made of the cells' at their places). BLK's root view
    - turned a quarter in TOP - and BLK2's - mirrored, placed twice - draw
    pass 2 with no plan over 1 um cells, the DOT array's dots within its
    extent and the squares' within theirs in their own coordinates, as many
    as the walk's within a few times, nothing elsewhere; at depth 0 their own
    squares alone; SML's root view the plans, byte for byte the walk's; built
    again with --roots 0, the cells' files go and BLK's root view is the
    walk's frame byte for byte."""
    import struct
    src = Path(temp) / 'occroot.oas'
    occ_root_layout(src)
    done = subprocess.run([FLOE2, 'index', str(src)],
                          cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
    assert done.returncode == 0, done.stdout + done.stderr
    ice = Path(temp) / '.occroot.oas.ice'
    index_bin = os.environ['FLOE_INDEX_BIN']
    # the index makes the cells' files with design.ovs; indexed again (here
    # with --no-ovs) they all go first
    assert len(list(ice.glob('design.ovs.*'))) == 2 and "cell BLK's root views" in done.stderr, done.stderr[-2000:]
    again = subprocess.run([FLOE2, 'index', str(src), '--force', '--no-ovs'],
                           cwd=ROOT, env=os.environ, capture_output=True, text=True, timeout=600)
    assert again.returncode == 0 and not list(ice.glob('design.ovs*')), (again.returncode, list(ice.glob('design.ovs*')))

    def ovs(*extra):
        built = subprocess.run([index_bin, 'ovs', str(ice), '--um', '1'] + list(extra), capture_output=True, text=True, timeout=600)
        assert built.returncode == 0, built.stdout + built.stderr
        return dict(kv.split('=', 1) for kv in built.stdout.split()[1:]), built.stderr

    stats, _ = ovs('--roots', '0')
    assert stats['roots'] == '0' and not list(ice.glob('design.ovs.*')), (stats, list(ice.glob('design.ovs.*')))
    alone_top = (ice / 'design.ovs').read_bytes()
    stats, said = ovs()
    roots = {}
    for line in said.splitlines():
        if line.startswith('[ovs] ') and "'s root views" in line:
            path, _, rest = line[len('[ovs] '):].partition(': cell ')
            roots[rest.split("'s root views")[0]] = int(path.rsplit('.', 1)[1])
    assert stats['roots'] == '2' and sorted(roots) == ['BLK', 'BLK2'], (stats, said)
    assert (ice / 'design.ovs').read_bytes() == alone_top, 'design.ovs differs made of the cells\' own'
    placed = {}
    for name, ci in roots.items():
        data = (ice / ('design.ovs.%d' % ci)).read_bytes()
        assert data[:8] == b'FLOEOVS1' and struct.unpack_from('<I', data, 8)[0] == 4, data[:12]
        placed[name] = (struct.unpack_from('<I', data, 80)[0], data[84], data[85], *struct.unpack_from('<qq', data, 88))
    assert placed['BLK'] == (roots['BLK'], 1, 0, 100000, 0), placed
    assert placed['BLK2'][:3] == (roots['BLK2'], 0, 1) and placed['BLK2'][3] == 100000, placed
    side_w, side_h = int(ROOT_VIEW[2] - ROOT_VIEW[0]), int(ROOT_VIEW[3] - ROOT_VIEW[1])

    def view(w, gen, root, depth=None):
        dbu = float(w.cache.meta['dbu'])
        w.submit({'kind': 'render', 'gen': gen, 'scope': 'headless', 'bbox': tuple(v / dbu for v in ROOT_VIEW), 'view': None,
                  'w': side_w, 'h': side_h, 'depth': depth, 'cut_px': 3.0, 'lod': False, 'frames': False, 'labels': False,
                  'abstract': False, 'visible': [LOW, ALONE], 'frame_format': 'raw', 'thin': 'keep', 'frame_cache': False,
                  'root': root})
        deadline = time.monotonic() + 300
        while time.monotonic() < deadline:
            res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
            assert res.get('kind') not in ('error', 'dropped'), res
            if res.get('kind') == 'frame' and res.get('gen') == gen and not res.get('refining'):
                return bytes(res.pop('rgba')), res
        raise AssertionError('occupancy root frame timeout')

    def cell_of(w, name):
        w.submit({'kind': 'cell_find', 'seq': 7, 'pattern': name})
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            res = w.res.get(timeout=max(0.1, deadline - time.monotonic()))
            assert res.get('kind') != 'error', res
            if res.get('kind') == 'cell_find' and res.get('seq') == 7:
                return [m['cell'] for m in res['matches'] if m['name'] == name][0]
        raise AssertionError('cell_find timeout')

    # the array's extent and the squares', in the cell's own coordinates
    # (rows from the top; a pixel of slack about each)
    array = (range(0, int(OCC_PITCH * ROOT_ARRAY[0]) + 1), range(side_h - int(OCC_PITCH * ROOT_ARRAY[1]) - 1, side_h))
    x0, y0, step, nx, ny = ROOT_SPECKS
    specks = (range(int(x0), int(x0 + step * nx) + 2), range(side_h - int(y0 + step * ny) - 2, side_h - int(y0) + 1))
    workers = {'walk': worker(src, env), 'occ': worker(src, occ)}
    try:
        low_c, alone_c = layer_colour(workers['occ'], LOW), layer_colour(workers['occ'], ALONE)

        def where(pixels):
            a = s = 0
            rest = []
            for r in range(side_h):
                for c in range(side_w):
                    p = pixels[(r * side_w + c) * 4:(r * side_w + c) * 4 + 4]
                    if p == BLACK:
                        continue
                    if p == low_c and c in array[0] and r in array[1]:
                        a += 1
                    elif p == alone_c and c in specks[0] and r in specks[1]:
                        s += 1
                    else:
                        rest.append((c, r, p))
            return a, s, rest

        counts = {}
        # a worker's generations rise (an older one is dropped)
        for gen, name in ((1, 'BLK'), (3, 'BLK2')):
            walked, walk_res = view(workers['walk'], gen, roots[name])
            drawn, res = view(workers['occ'], gen, roots[name])
            p2 = res['density_plan2']
            assert (p2['occ_layers'], p2['occ_cell_nm'], p2['regions'], p2['nodes']) == (2, 1000, 0, 0), (name, p2)
            assert walk_res['density_plan2']['occ_layers'] == 0, walk_res['density_plan2']
            (a, s, rest), (wa, ws, wrest) = where(drawn), where(walked)
            assert not wrest and wa > 0 and ws > 0, (name, wa, ws, wrest[:5])
            assert a > 0 and s > 0 and not rest, (name, a, s, rest[:5])
            assert 0.25 * wa <= a <= 4 * wa and 0.15 * ws <= s <= 2 * ws, (name, a, wa, s, ws)
            # depth 0: the cell's own squares alone
            own, own_res = view(workers['occ'], gen + 1, roots[name], depth=0)
            oa, os_, orest = where(own)
            assert own_res['density_plan2']['occ_layers'] == 1 and oa == 0 and os_ == s and not orest, (name, own_res['density_plan2'], oa, os_, s)
            counts[name] = (a, s, wa, ws)
        sml = cell_of(workers['occ'], 'SML')
        walked, _ = view(workers['walk'], 20, sml)
        drawn, res = view(workers['occ'], 20, sml)
        assert res['density_plan2']['occ_layers'] == 0 and drawn == walked, res['density_plan2']
    finally:
        for w in workers.values():
            w.stop()
    # built again with --roots 0: the cells' files go, the plans draw BLK
    stats, said = ovs('--roots', '0')
    assert stats['roots'] == '0' and not list(ice.glob('design.ovs.*')) and 'removed' in said, (stats, said)
    workers = {'walk': worker(src, env), 'occ': worker(src, occ)}
    try:
        walked, _ = view(workers['walk'], 30, roots['BLK'])
        drawn, res = view(workers['occ'], 30, roots['BLK'])
        assert res['density_plan2']['occ_layers'] == 0 and drawn == walked, res['density_plan2']
    finally:
        for w in workers.values():
            w.stop()
    print('density stack: occupancy density by view root - design.ovs.<cell> for BLK (R90) and BLK2 (mirrored, placed twice) '
          'in the same walk, design.ovs the same bytes; their root views by their files (DOT px, square px; the walk\'s): %s; '
          'depth 0 their own squares alone; SML and BLK without its file the walk\'s frame' % counts)


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
    saved = {name: os.environ.get(name) for name in env}
    for name, value in env.items():
        if value is None:
            os.environ.pop(name, None)
        else:
            os.environ[name] = value
    cache = Cache(str(src))
    cache.load()
    w = RustRenderWorker(cache)
    w.start()
    for name, value in saved.items():
        if value is None:
            os.environ.pop(name, None)
        else:
            os.environ[name] = value
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
    for name in ('FLOE_RUST_DENSITY_STACK', 'FLOE_RUST_DENSITY_DOTS', 'FLOE_RUST_AREA_TRUE', 'FLOE_RUST_WRITE_ONCE',
                 'FLOE_RUST_DENSITY_PATTERN', 'FLOE_RUST_DENSITY_MASK', 'FLOE_RUST_DENSITY_STAGES'):
        os.environ.pop(name, None)
    # the fixed VIEW (W x H at PX_UM) is wider than most of these dies: past
    # their fit views the dots would thin (density_zoom_gain); the checks
    # count dots as drawn at a fit, zoom_out_checks alone thins them
    os.environ['FLOE_RUST_DENSITY_ZOOM_OUT'] = 'off'
    # and their dots are counted as drawn without the detail's gate (a
    # block too sparse is left out, floe_vfs HierOpts::dot_gate): gate_checks
    # alone gates them
    os.environ['FLOE_RUST_DENSITY_GATE'] = 'off'
    # and one top plane over the lower planes' walk, as these checks were made
    # (renderd density_top_group: the topmost eight are top planes, each over
    # the originals below it - every plane of these small views): top_group_checks
    # alone draws the eight
    os.environ['FLOE_RUST_DENSITY_TOP_GROUP'] = 'off'
    # and the top planes' density over the originals below them, as these
    # checks were made (renderd density_shapes_first: every plane's density
    # where no original is): shapes_first_checks alone keeps it to that space
    os.environ['FLOE_RUST_DENSITY_SHAPES_FIRST'] = 'off'
    # and the dots as lit pixels, as these checks were made (renderd
    # density_bright_gain: a pixel shows its covered area's brightness):
    # bright_checks alone draws the brightness
    os.environ['FLOE_RUST_DENSITY_BRIGHT'] = 'off'
    # and pass 2 by the plans, as these checks were made (the occupancy
    # density, on by default since 0.12.317 where an index has design.ovs -
    # every index here makes it): occ_density_checks turn it on, one with
    # the switch unset
    os.environ['FLOE_RUST_DENSITY_OCC'] = 'off'
    with tempfile.TemporaryDirectory(prefix='floe-density-stack-') as temp:
        src = Path(temp) / 'stack.oas'
        layout(src)
        done = subprocess.run([FLOE2, 'index', str(src)],
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
        lists_checks(temp)
        dense_lists_checks(temp)
        probe_threads_checks(temp)
        cells_checks(temp)
        shift_checks(temp)
        zoom_out_checks(temp)
        gate_checks(temp)
        top_first_checks(temp)
        density_only_checks(temp)
        top_group_checks(temp)
        shapes_first_checks(temp)
        full_shapes_first_checks(temp)
        nearly_full_pattern_checks(temp)
        planner_mask_checks(temp)
        staged_density_checks(temp)
        bright_checks(temp)
        pattern_checks(temp)
        toggle_checks(temp)
        first_checks(temp)
        sums_checks(temp)
        hier_checks(temp)
        left_checks(temp)
        ladder_checks(temp)
        occ_checks(temp)
        occ_density_checks(temp)
    print('density stack gate: OK')


if __name__ == '__main__':
    main()
