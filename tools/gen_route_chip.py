#!/usr/bin/env python3
"""Generate a routing-heavy synthetic chip for the density stack's open cases:
a streaming byte writer (no KLayout Layout in memory), the encoding of
tools/gen_main01_like.py (precision 4000/um, CELLNAME by reference, modal
records, repetitions stored as count-2, END padded to 256 bytes).

What the field chip MAIN01 showed against Calibre (2026-10-02) and the
synthetic MAIN01 (tools/gen_main01_like.py) never did:
  * depth 0 - the top cell's OWN shapes under a pixel at a fit view: routing
    wires laid along tracks (lines with space between), vias at their ends
    (lines of points) and a dummy fill of random small squares in a few
    regions (even), on their own layers; tens of millions of records, past
    every pass-2 reserve (1 GB) by the planner's estimate;
  * depth 1 - vias placed by the top as huge irregular repetitions (point
    lists of about a million members each, along the routes over the die:
    SUB_CUT_BOX_ARRAY_MAX per plan, the threads' regions dealt round robin)
    and blocks whose own routing is pass 2's pages, past eight reserves;
  * full depth - standard cells in rows (1-D arrays) inside the blocks, about
    27 million instances.

At scale 1: 280.6 MB written in 13 s - 27.6 M rectangle records (58.9 M
members), 47,182 standard-cell arrays, 16 point lists of a million; indexed
in 13 s to 426 MB (588 pages). At a fit view (1350 x 971 px), 0.12.272
(CUT_DENSITY_DESIGN §10.12): depth 0 thins pass 2 under the whole 1 GB and
a zero floor's probe passes it (`floor probe, thinned`); depth 1 counts 11.8 M
list members on four threads, 4.2 M (16 x 2^18) on one, the frames 304k and
124k px lit; under a fixed 128 MB reserve the ladder raises the cut on one
thread.

    .venv/bin/python tools/gen_route_chip.py --plan                     # planned totals
    .venv/bin/python tools/gen_route_chip.py out.oas --scale 0.05       # a quick one
    .venv/bin/python tools/gen_route_chip.py data/synthetic/route_chip.oas

`--scale s` multiplies every count (routes, vias, fill, list members, block
routing, standard-cell rows) by s; the die, the layers and the cells stay.
Output is deterministic for a given --seed and --scale, independent of --jobs.
The top's content is written in pieces by workers: each piece starts in
XYABSOLUTE with its layer, size and place given, then goes on in XYRELATIVE.
"""
import argparse
import math
import multiprocessing as mp
import os
import random
import sys
import time

MAGIC = b"%SEMI-OASIS\r\n"
UNIT = 4000                       # dbu per um
DIE = 10_000 * UNIT               # 10 mm
# metals M1..M8 (horizontal on odd, vertical on even), vias V1..V7 between
# them, fill on the metals (datatype 1), the standard cells' base layers, the
# blocks' boundary
METALS = [(30 + i, 0) for i in range(1, 9)]
VIAS = [(50 + i, 0) for i in range(1, 8)]
FILLS = [(30 + i, 1) for i in range(1, 9)]
BASE = [(1, 0), (2, 0), (3, 0)]   # diffusion, poly, contact
BOUNDARY = (100, 0)
LAYERS = BASE + METALS + VIAS + FILLS + [BOUNDARY]
# track pitch and wire width per metal (dbu): finer below
PITCH = [320, 320, 400, 400, 560, 560, 800, 800]
WIDTH = [p // 2 for p in PITCH]
# at scale 1
ROUTE_SEGMENTS = 2_000_000        # per metal in the top
VIA_SHAPE_SHARE = 0.3             # a via shape at a segment's end, this often
FILL_SQUARES = 2_000_000          # random small squares, in FILL_REGIONS
FILL_REGIONS = 4
LIST_MEMBERS = 1_000_000          # per point list
LISTS_PER_VIA = 4
VIA_CELLS = 4
BLOCKS = 16                       # 4 x 4
BLOCK_SIZE = 2_000 * UNIT         # 2 mm
BLOCK_SEGMENTS = 300_000          # per block, on M1..M4
ROW_HEIGHT = int(1.2 * UNIT)
STD_CELLS = 8
TOP_PIECES = 8                    # pieces per metal / via layer of the top


# ---------------------------------------------------------------- encoding
def uint(v):
    out = bytearray()
    while True:
        b = v & 0x7F
        v >>= 7
        if v:
            out.append(b | 0x80)
        else:
            out.append(b)
            return bytes(out)


def sint(v):
    return uint((abs(v) << 1) | (1 if v < 0 else 0))


def bstr(b):
    return uint(len(b)) + b


def gdelta(dx, dy):
    """g-delta form 0 (axis-aligned) or 1 (general) - rust/oasis g_delta."""
    if dy == 0:
        return uint((abs(dx) << 4) | ((2 if dx < 0 else 0) << 1))
    if dx == 0:
        return uint((abs(dy) << 4) | ((3 if dy < 0 else 1) << 1))
    return uint((abs(dx) << 2) | (2 if dx < 0 else 0) | 1) + sint(dy)


def logu(rng, lo, hi):
    return int(math.exp(rng.uniform(math.log(lo), math.log(hi))))


class Rects:
    """RECTANGLE records of one layer, starting absolute (XYABSOLUTE, layer
    and size given) and relative after: one piece of a cell, written alone."""

    def __init__(self, layer):
        self.out = [b"\x0f"]                           # XYABSOLUTE
        self.layer = layer
        self.lead = True
        self.w = self.h = -1
        self.x = self.y = 0
        self.n = 0

    def add(self, x, y, w, h, rep=b""):
        bits = 0x18 | (0x40 if w != self.w else 0) | (0x20 if h != self.h else 0) | (0x04 if rep else 0)
        if self.lead:
            bits |= 0x03
        self.out.append(bytes((0x14, bits)))
        if self.lead:
            self.out.append(uint(self.layer[0]) + uint(self.layer[1]))
        if w != self.w:
            self.out.append(uint(w))
            self.w = w
        if h != self.h:
            self.out.append(uint(h))
            self.h = h
        if self.lead:
            self.out.append(sint(x) + sint(y) + rep + b"\x10")   # absolute, then XYRELATIVE
            self.lead = False
        else:
            self.out.append(sint(x - self.x) + sint(y - self.y) + rep)
        self.x, self.y = x, y
        self.n += 1

    def bytes(self):
        return b"".join(self.out)


def routes(rng, metal, n_segments, x0, y0, size, out_vias=None, via_layer=None):
    """`n_segments` wire segments of `metal` (index) laid end to end along
    random tracks of the square [x0, x0 + size): routes of 20 um to 2 mm,
    segments of 1 to 20 um with gaps of 0.1 to 1 um; the via shapes at
    segment ends go to `out_vias` (Rects of `via_layer`)."""
    pitch, width = PITCH[metal], WIDTH[metal]
    horizontal = metal % 2 == 0
    rects = Rects(METALS[metal])
    via = max(200, int(width * 0.8))
    left = n_segments
    while left > 0:
        track = rng.randrange(size // pitch) * pitch
        length = logu(rng, 20 * UNIT, 2000 * UNIT)
        at = rng.randrange(max(1, size - length))
        end = min(size, at + length)
        while at < end and left > 0:
            seg = min(end - at, logu(rng, 1 * UNIT, 20 * UNIT))
            if seg <= 0:
                break
            if horizontal:
                rects.add(x0 + at, y0 + track, seg, width)
            else:
                rects.add(x0 + track, y0 + at, width, seg)
            if out_vias is not None and rng.random() < VIA_SHAPE_SHARE:
                if horizontal:
                    out_vias.add(x0 + at + seg - via, y0 + track + (width - via) // 2, via, via)
                else:
                    out_vias.add(x0 + track + (width - via) // 2, y0 + at + seg - via, via, via)
            left -= 1
            at += seg + logu(rng, UNIT // 10, UNIT)
    return rects


def piece(task):
    """One piece of a cell: (key, bytes, counts)."""
    kind, args, seed = task
    rng = random.Random(seed)
    counts = dict(rect=0, members=0, placement=0, array=0, list_points=0)
    if kind == "route":
        metal, n = args
        vias = Rects(VIAS[min(metal, len(VIAS) - 1)])
        rects = routes(rng, metal, n, 0, 0, DIE, vias)
        counts["rect"] = counts["members"] = rects.n + vias.n
        return (kind, args), rects.bytes() + (vias.bytes() if vias.n else b""), counts
    if kind == "fill":
        region, n = args
        side = DIE // 8
        x0 = (1 + 2 * (region % 4)) * DIE // 9
        y0 = (1 + 2 * (region // 4 % 4)) * DIE // 9
        rects = Rects(FILLS[region % len(FILLS)])
        for _ in range(n):
            w, h = rng.randrange(800, 1600), rng.randrange(800, 1600)
            rects.add(x0 + rng.randrange(side), y0 + rng.randrange(side), w, h)
        # and a dummy metal fill of 1 um squares as arrays beside them
        for _ in range(64):
            na, nb = rng.randrange(20, 200), rng.randrange(20, 200)
            rects.add(x0 + rng.randrange(side), y0 + side + rng.randrange(side // 4), 4000, 4000,
                      b"\x01" + uint(na - 2) + uint(nb - 2) + uint(8000) + uint(8000))
            counts["members"] += na * nb - 1
        counts["rect"] = rects.n
        counts["members"] += rects.n
        return (kind, args), rects.bytes(), counts
    if kind == "list":
        via, k, n = args
        # the points along routes: runs on a track, every 0.5 to 5 um, then
        # another track; the first point is the placement's place
        metal = 2 * via
        pitch = PITCH[metal]
        pts = []
        while len(pts) < n:
            track = rng.randrange(DIE // pitch) * pitch
            at = rng.randrange(DIE)
            run = logu(rng, 4, 400)
            horizontal = rng.random() < 0.5
            for _ in range(run):
                pts.append((at, track) if horizontal else (track, at))
                at += logu(rng, UNIT // 2, 5 * UNIT)
                if at >= DIE or len(pts) >= n:
                    break
        x, y = pts[0]
        out = [b"\x0f", bytes((0x11, 0xF8)), uint(1 + via), sint(x), sint(y),   # C N X Y R, absolute
               b"\x0a", uint(len(pts) - 2)]
        for (ax, ay), (bx, by) in zip(pts, pts[1:]):
            out.append(gdelta(bx - ax, by - ay))
        out.append(b"\x10")
        counts["placement"] = 1
        counts["list_points"] = len(pts)
        return (kind, args), b"".join(out), counts
    if kind == "block":
        b, n_seg, rows = args
        ci = 1 + VIA_CELLS + b
        out = [b"\x0d" + uint(ci), b"\x10"]
        # its own routing on M1..M4
        for metal in range(4):
            rects = routes(rng, metal, n_seg // 4, 0, 0, BLOCK_SIZE)
            out.append(rects.bytes())
            counts["rect"] += rects.n
            counts["members"] += rects.n
        # standard cells in rows: 1-D arrays of one cell type at its width
        out.append(b"\x0f")
        first = True
        px = py = 0
        std0 = 1 + VIA_CELLS + BLOCKS
        for r in range(rows):
            y = r * ROW_HEIGHT
            x = rng.randrange(4000)
            while x < BLOCK_SIZE - 40_000:
                t = rng.randrange(STD_CELLS)
                width = std_width(t)
                n = min(rng.randrange(20, 2000), (BLOCK_SIZE - x) // width)
                if n < 2:
                    break
                flip = r % 2
                info = 0xF8 | flip
                out.append(bytes((0x11, info)) + uint(std0 + t))
                yy = y + (ROW_HEIGHT if flip else 0)
                if first:
                    out.append(sint(x) + sint(yy))
                else:
                    out.append(sint(x - px) + sint(yy - py))
                out.append(b"\x02" + uint(n - 2) + uint(width))
                if first:
                    out.append(b"\x10")
                    first = False
                px, py = x, yy
                counts["array"] += 1
                counts["members"] += n
                x += n * width + rng.randrange(0, 4000)
        return (kind, args), b"".join(out), counts
    raise ValueError(kind)


def std_width(t):
    return (2 + 2 * t) * 1000      # 0.5 .. 3.5 um


def leaf_cells():
    """The via cells and the standard cells: (reference number, bytes)."""
    out = []
    for v in range(VIA_CELLS):
        cut, lo, hi = VIAS[2 * v], METALS[2 * v], METALS[2 * v + 1]
        body = [b"\x0d" + uint(1 + v), b"\x10"]
        for layer, side in ((cut, 400), (lo, 640), (hi, 640)):
            r = Rects(layer)
            r.add(-side // 2, -side // 2, side, side)
            body.append(r.bytes())
        out.append(b"".join(body))
    std0 = 1 + VIA_CELLS + BLOCKS
    for t in range(STD_CELLS):
        rng = random.Random(9_000 + t)
        w = std_width(t)
        body = [b"\x0d" + uint(std0 + t), b"\x10"]
        for layer, n in ((BASE[0], 2), (BASE[1], 2 + t), (BASE[2], 4 + t), (METALS[0], 3 + t)):
            r = Rects(layer)
            for i in range(n):
                if layer == BASE[1]:
                    x = rng.randrange(200, w - 300)
                    r.add(x, 400, 80, ROW_HEIGHT - 800)
                elif layer == BASE[2]:
                    r.add(rng.randrange(200, w - 400), rng.randrange(800, ROW_HEIGHT - 800), 200, 200)
                elif layer == BASE[0]:
                    # the n and p diffusions
                    r.add(100, 600 if i == 0 else ROW_HEIGHT // 2 + 200, w - 200, ROW_HEIGHT // 2 - 800)
                else:
                    x = rng.randrange(100, w - 400)
                    r.add(x, rng.randrange(400, ROW_HEIGHT - 1600), 280, 1200)
            body.append(r.bytes())
        out.append(b"".join(body))
    return out


def plan(scale):
    """The tasks of the top and the blocks, and the planned counts."""
    seg = max(1, int(ROUTE_SEGMENTS * scale))
    tasks = []
    for metal in range(len(METALS)):
        for p in range(TOP_PIECES):
            tasks.append(("route", (metal, seg // TOP_PIECES), 1000 * metal + p))
    for region in range(FILL_REGIONS):
        tasks.append(("fill", (region, max(1, int(FILL_SQUARES * scale) // FILL_REGIONS)), 50_000 + region))
    for via in range(VIA_CELLS):
        for k in range(LISTS_PER_VIA):
            tasks.append(("list", (via, k, max(4, int(LIST_MEMBERS * scale))), 60_000 + 16 * via + k))
    rows = max(2, int(BLOCK_SIZE // ROW_HEIGHT * min(1.0, scale)))
    blocks = [("block", (b, max(4, int(BLOCK_SEGMENTS * scale)), rows), 70_000 + b) for b in range(BLOCKS)]
    return tasks, blocks


def header(names):
    out = [MAGIC, uint(1) + bstr(b"1.0") + uint(0) + uint(UNIT) + uint(0) + uint(0) * 12]   # START
    for name in names:
        out.append(uint(3) + bstr(name))                                                    # CELLNAME
    for layer, dt in LAYERS:
        out.append(uint(11) + bstr(b"L%dD%d" % (layer, dt)) + uint(3) + uint(layer) + uint(3) + uint(dt))
    return b"".join(out)


def trailer():
    end = uint(2)
    pad = 256 - len(end) - 2 - 1
    end += bstr(b"\0" * pad) + uint(0)
    assert len(end) == 256
    return end


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("out", nargs="?", help="output .oas path")
    ap.add_argument("--scale", type=float, default=1.0)
    ap.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 2) - 1))
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--plan", action="store_true", help="print the planned totals and exit")
    args = ap.parse_args()
    tasks, blocks = plan(args.scale)
    seg = max(1, int(ROUTE_SEGMENTS * args.scale))
    if args.plan or not args.out:
        rows = blocks[0][1][2]
        print("die %d um; top: %d wire segments on %d metals (+ about %.0f%% via shapes), %d fill squares, "
              "%d point lists of %d members (%d vias); %d blocks of %d um: %d segments each, %d rows of standard cells"
              % (DIE // UNIT, seg * len(METALS), len(METALS), 100 * VIA_SHAPE_SHARE, int(FILL_SQUARES * args.scale),
                 len([t for t in tasks if t[0] == "list"]), max(4, int(LIST_MEMBERS * args.scale)), VIA_CELLS,
                 BLOCKS, BLOCK_SIZE // UNIT, max(4, int(BLOCK_SEGMENTS * args.scale)), rows))
        return
    started = time.monotonic()
    names = [b"RTG_TOP"] + [b"VIA%d%d" % (v + 1, v + 2) for v in range(VIA_CELLS)] \
        + [b"BLK_%02d" % b for b in range(BLOCKS)] + [b"STD_%d" % t for t in range(STD_CELLS)]
    totals = dict(rect=0, members=0, placement=0, array=0, list_points=0)
    jobs = [(kind, a, (args.seed * 1_000_003 + s) & 0xFFFFFFFF) for (kind, a, s) in tasks + blocks]
    with open(args.out, "wb") as f, mp.Pool(args.jobs) as pool:
        f.write(header(names))
        for blob in leaf_cells():
            f.write(blob)
        # the top: its block placements (a 4 x 4 grid), then its pieces in order
        f.write(b"\x0d" + uint(0) + b"\x10")
        gap = (DIE - 4 * BLOCK_SIZE) // 5
        px = py = 0
        for b in range(BLOCKS):
            x, y = gap + (b % 4) * (BLOCK_SIZE + gap), gap + (b // 4) * (BLOCK_SIZE + gap)
            f.write(bytes((0x11, 0xF0)) + uint(1 + VIA_CELLS + b) + sint(x - px) + sint(y - py))
            px, py = x, y
            totals["placement"] += 1
        for (key, blob, counts) in pool.imap(piece, jobs[:len(tasks)], chunksize=1):
            f.write(blob)
            for k, v in counts.items():
                totals[k] += v
        for (key, blob, counts) in pool.imap(piece, jobs[len(tasks):], chunksize=1):
            f.write(blob)
            for k, v in counts.items():
                totals[k] += v
        f.write(trailer())
        size = f.tell()
    print("%s: %.1f MB in %.0f s - top and blocks: %d rectangle records (%d members), %d placements, %d arrays, "
          "%d list points" % (args.out, size / 1e6, time.monotonic() - started, totals["rect"], totals["members"],
                              totals["placement"], totals["array"], totals["list_points"]))


if __name__ == "__main__":
    main()
