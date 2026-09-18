#!/usr/bin/env python3
"""Generate a MAIN01-class synthetic OASIS: a streaming byte writer, no KLayout
Layout in memory, so a ~9.8 GB file with 800 M placement records takes minutes
and stays under a gigabyte of RAM.

Targets (the user's MAIN01.oas statistics, 2026-09-17; PROFILE below):
    layer 449   cell 111,255   placement 643 M   array 162 M
    rectangle 17.7 G (members: repetition expanded, hierarchy NOT expanded)
    polygon 53.9 M   path 0   text 224 K   property 111 K (one per cell)
    precision 4000/um   max depth 15   most shapes at depth 0-5   ~9.8 GB

    .venv/bin/python tools/gen_main01_like.py --plan                  # planned totals vs targets
    .venv/bin/python tools/gen_main01_like.py out.oas --scale 0.001   # ~10 MB smoke file
    .venv/bin/python tools/gen_main01_like.py out.oas --jobs 16       # the full file

Hierarchy model (what makes it index like a chip and not explode): two kinds
of cells. HIERARCHY cells form a chain of 16 levels (TOP = level 0); a level-k
hierarchy cell places a few level-(k+1) hierarchy cells (the chain fan-out is
small, so the flattened instance count stays ~1e10, not 1e30) and MANY leaf
cells. LEAF cells (library cells: standard cells, vias, fills, IP) hold shapes
and place nothing; a leaf "homed" at level k is placed by level-k hierarchy
cells, so its shapes sit at depth k+1. Every hierarchy cell and every leaf is
placed at least once. Most placement records target leaves homed at levels
0-5, and the hierarchy cells of levels 0-5 carry the big fill/routing grids,
which puts ~99 % of the rectangle members at depth 0-5 while the chain still
reaches depth 15.

What is reproduced: record mix and counts, depth, fan-in, per-level shape
distribution, repetition kinds (NxM and 1-D grids, arbitrary point lists),
unit, byte size - and, with `--geometry chip` (the default since 2026-09-19),
the SHAPE of a chip's geometry, which is what a viewer's size cut, hairline
rule and budget react to:
  * every layer has a role (layer index mod 6): boxes, horizontal wires,
    vias, vertical wires, fill squares, blocks (markers / boundaries);
  * wires are thin and long - a few widths per layer, lengths log-uniform from
    a few widths up to the cell's extent, one in twenty a rail across the
    cell - laid end to end along routing tracks; widths grow with the level
    (0.02-0.08 um in library cells, 0.05-0.4 um in blocks, 0.4-10 um at the
    top), so one view holds several size classes at once;
  * grids are what they are on a chip: buses (a long wire repeated across its
    width), fields of short thin shapes (the 0.45 x 0.022 um at 0.8 x 1.0 um
    pitch kind), via and fill arrays;
  * blocks are few and large, polygons are wire-sized L and staircase shapes,
    and some placement arrays are dense (pitch = the child's extent) and
    large, like a memory array;
  * a library cell is as large as its parent has room for (the parent's
    placements tile it about once: 0.25-3 um), so at a wide view library
    cells are under the size cut, as on a chip.
What is still not real: no connectivity, shapes of one layer may overlap,
cells are squares on a jittered walk. `--geometry legacy` writes the file of
2026-09-17..18 byte for byte: near-square shapes of ONE size class per level
band, nothing long or thin (every measurement recorded before 2026-09-19 was
made on it).

`--scale s` multiplies the cell counts and the per-cell record counts by
sqrt(s), so totals and bytes scale by ~s while the depth stays 15 (every level
keeps at least one cell). Output is deterministic for a given --seed, --scale
and profile, independent of --jobs.

Encoding follows what floe's parser (rust/oasis/src/doc.rs) and KLayout both
read, mirroring tools/validate_oasis_shapes.py: START with offset-flag 0,
CELLNAME records with implicit numbering, CELL by reference number, XYRELATIVE
per cell, PLACEMENT with reference numbers, RECTANGLE/POLYGON grouped by
layer with modal reuse, TEXT with inline strings, one PROPERTY per cell, END
padded to 256 bytes, no CBLOCK. Repetition dimensions are stored as count-2.
"""
import argparse
import math
import multiprocessing as mp
import os
import random
import sys
import time

MAGIC = b"%SEMI-OASIS\r\n"
UNIT = 4000                      # dbu per um (precision 4000)
LAYERS = 449                     # (layer, datatype) pairs with LAYERNAME records

# Per level: hierarchy cells, leaf cells homed here, per HIERARCHY cell: chain
# placements (to level+1 hierarchy cells; at least the mandatory ones), leaf
# placements (single), leaf arrays, own rect records, own grid records, grid
# members (mean), own polygons, texts; extents (dbu) of a hierarchy cell and
# of a leaf homed here. Sum of cells = 17,510 + 93,745 = 111,255.
#   lvl  hier   leaf   chain  place     array   rect      grid   gmem  poly     text  hext        lext
PROFILE = [
    (0,     1,  3_000,   16, 8_000_000, 2_000_000, 6_000_000, 320_000, 2200, 1_000_000, 20_000, 64_000_000, 400_000),
    (1,    16, 12_000,   13, 6_000_000, 1_500_000, 1_400_000, 160_000, 1700,   500_000,  5_000,  8_000_000, 200_000),
    (2,   200, 20_000,    6,   800_000,   200_000,   140_000,  16_000, 1100,    50_000,    300,  2_000_000, 100_000),
    (3, 1_200, 25_000,  3.5,   120_000,    30_000,    26_000,   5_000,  800,     8_000,     40,    800_000,  40_000),
    (4, 4_000, 20_000,  1.6,    30_000,     8_000,     5_000,   1_500,  600,     2_500,      4,    400_000,  16_000),
    (5, 6_000,  8_000,  0.6,    12_000,     3_000,     1_300,     500,  400,     1_500,      0,    200_000,   8_000),
    (6, 3_000,  3_000,  0.6,     8_000,     2_000,       350,      25,  150,       800,      0,    120_000,   8_000),
    (7, 1_500,  1_500,  0.6,     6_000,     1_500,       180,      10,  100,       400,      0,     80_000,   8_000),
    (8,   800,    700,  0.6,     4_000,     1_000,       120,       5,   80,       200,      0,     60_000,   8_000),
    (9,   400,    300,  0.6,     3_000,       800,        80,       3,   60,       100,      0,     48_000,   8_000),
    (10,  200,    120,  0.6,     2_000,       500,        60,       2,   50,        60,      0,     40_000,   8_000),
    (11,  100,     60,  0.6,     1_500,       400,        50,       1,   40,        40,      0,     32_000,   8_000),
    (12,   50,     30,  0.6,     1_000,       300,        40,       1,   30,        30,      0,     28_000,   8_000),
    (13,   25,     20,  0.6,       800,       200,        30,       1,   20,        20,      0,     24_000,   8_000),
    (14,   12,     10,  0.6,       600,       150,        25,       1,   10,        15,      0,     20_000,   8_000),
    (15,    6,      5,    0,       400,       100,        20,       0,    0,        10,      0,     16_000,   8_000),
]
# leaf cells: rect records, polygons per leaf (means), by home level band
LEAF_SHAPES = {"shallow": (60, 6), "deep": (24, 2)}     # home level <= 2 / deeper
TARGETS = {"cell": 111_255, "placement": 643_173_288, "array": 162_364_653,
           "rectangle": 17_717_387_961, "polygon": 53_906_629, "path": 0,
           "text": 223_918, "property": 111_255, "bytes": 9_375 * 1024 * 1024}
# bytes per record class, from a --scale 0.001 run (the run prints the ratio)
EST_BYTES_BY_GEOMETRY = {
    "legacy": {"placement": 9.2, "array": 14.5, "rect": 5.6, "grid": 11.5,
               "polygon": 11.5, "text": 14.0},
    # lengths differ from wire to wire and polygons are wire-sized (more
    # bytes per shape record), library cells are small (fewer per placement);
    # fitted on a --scale 0.1 run
    "chip": {"placement": 7.8, "array": 13.2, "rect": 8.6, "grid": 14.0,
             "polygon": 15.5, "text": 14.0},
}
EST_BYTES = EST_BYTES_BY_GEOMETRY["chip"]
# chip geometry: the role of a layer (index mod 6)
ROLES = ("box", "wire_h", "via", "wire_v", "fill", "block")

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


# g-delta form 0 (axis-aligned), form 1 (general) - rust/oasis/src/lib.rs g_delta
def gdelta(dx, dy):
    if dy == 0:
        return uint((abs(dx) << 4) | ((2 if dx < 0 else 0) << 1))
    if dx == 0:
        return uint((abs(dy) << 4) | ((3 if dy < 0 else 1) << 1))
    return uint((abs(dx) << 2) | (2 if dx < 0 else 0) | 1) + sint(dy)


class Tables:
    """Precomputed varints (a few MB per worker); values outside fall back."""

    def __init__(self, n=1 << 17):
        self.u = [uint(i) for i in range(n)]
        self.s = [sint(i - n) for i in range(2 * n)]
        self.n = n

    def U(self, v):
        return self.u[v] if 0 <= v < self.n else uint(v)

    def S(self, v):
        return self.s[v + self.n] if -self.n <= v < self.n else sint(v)


# ---------------------------------------------------------------- layout plan
class Plan:
    """Cell numbering: hierarchy cells level by level (TOP = 0), then leaf
    cells by home level. Everything is derived from (scale, seed)."""

    def __init__(self, scale, seed, geometry="chip"):
        f = math.sqrt(scale)
        self.seed = seed
        self.geometry = geometry
        self.est_bytes = EST_BYTES_BY_GEOMETRY[geometry]
        self.f = f
        self.levels = []
        for (lvl, hier, leaf, chain, place, array, rect, grid, gmem, poly, text, hext, lext) in PROFILE:
            self.levels.append(dict(
                level=lvl, hier=max(1, round(hier * f)), leaf=max(1, round(leaf * f)),
                chain=chain, place=place * f, array=array * f, rect=rect * f,
                grid=grid * f, gmem=max(4, gmem * f) if grid else 0, poly=poly * f,
                text=text * f, hext=hext, lext=lext))
        if geometry == "chip":
            # a library cell is as large as its parent has room for: the
            # parent's placements (arrays at about ten members) tile it about
            # once. The legacy extents (100 um leaves placed tens of millions
            # of times in a 16 mm top) cover their parent hundreds of times
            # over - harmless while nothing in a leaf was long enough to draw
            # at a wide view, minutes of raster once leaves hold wires.
            for row in self.levels:
                members = row["place"] + 10.0 * row["array"]
                row["lext"] = max(1_000, min(row["lext"], int(row["hext"] / math.sqrt(max(1.0, members)))))
        self.hstart = []
        ref = 0
        for row in self.levels:
            self.hstart.append(ref)
            ref += row["hier"]
        self.lstart = []
        for row in self.levels:
            self.lstart.append(ref)
            ref += row["leaf"]
        self.cells = ref
        self.hier_cells = self.lstart[0]
        self.layers = [(l, d) for l in range(1, 200) for d in range(4)][:LAYERS]

    def kind_of(self, ci):
        """('hier'|'leaf', level, local)"""
        starts, kind = (self.hstart, "hier") if ci < self.hier_cells else (self.lstart, "leaf")
        for k in range(len(self.levels) - 1, -1, -1):
            if ci >= starts[k]:
                return kind, k, ci - starts[k]
        raise ValueError(ci)

    def name(self, ci):
        kind, k, local = self.kind_of(ci)
        if ci == 0:
            return b"TOP"
        return (b"H%02d_%05d" if kind == "hier" else b"LF%02d_%05d") % (k, local)

    def layer_window(self, k, leaf):
        # leaves and deep cells live on the base layers, blocks and the top on
        # the upper metals; windows overlap so every layer is used somewhere
        if leaf or k >= 6:
            return 0, 120, (3, 10)
        if k >= 3:
            return 40, 300, (8, 24)
        return 120, LAYERS, (24, 60)

    def leaf_shapes(self, k):
        return LEAF_SHAPES["shallow" if k <= 2 else "deep"]

    def expected(self):
        """Record totals, byte estimate and flattened weight (means, no jitter)."""
        t = dict(placement=0, array=0, rect=0, grid=0, rect_members=0, polygon=0, text=0)
        depth_members = {}
        flat_hier = 1.0        # flattened instances of this level's hierarchy cells
        flat_leaf = 0.0
        for k, row in enumerate(self.levels):
            n = row["hier"]
            chain = max(row["chain"], (self.levels[k + 1]["hier"] / n) if k + 1 < len(self.levels) else 0)
            lrect, lpoly = self.leaf_shapes(k)
            t["placement"] += n * (row["place"] + chain)
            t["array"] += n * row["array"]
            t["rect"] += n * row["rect"] + row["leaf"] * lrect * self.f
            t["grid"] += n * row["grid"]
            own = n * (row["rect"] + row["grid"] * row["gmem"])
            leaf_members = row["leaf"] * lrect * self.f
            t["rect_members"] += own + leaf_members
            depth_members[k] = depth_members.get(k, 0) + own
            depth_members[k + 1] = depth_members.get(k + 1, 0) + leaf_members
            t["polygon"] += n * row["poly"] + row["leaf"] * lpoly * self.f
            t["text"] += n * row["text"]
            flat_leaf += flat_hier * (row["place"] + row["array"] * 22)   # flat_hier already sums the level
            flat_hier *= chain
        t["bytes"] = sum(t[k] * self.est_bytes[k] for k in self.est_bytes)
        t["bytes"] += self.cells * 60 + LAYERS * 12 + 300
        t["cells"] = self.cells
        t["depth_members"] = depth_members
        t["flat_leaf"] = flat_leaf
        return t


# ---------------------------------------------------------------- generation
_T = None
_P = None


def _init(scale, seed, geometry="chip"):
    global _T, _P
    _T = Tables()
    _P = Plan(scale, seed, geometry)


def jitter(rng, mean):
    if mean <= 0:
        return 0
    v = mean * rng.uniform(0.7, 1.3)
    return int(v) + (1 if rng.random() < v - int(v) else 0)


# translation that keeps a child of extent e (origin at its lower-left) inside
# [0,e)^2 after (flip, rot): the bbox of the transformed child starts there
def anchor(rot, flip, e):
    return ((0, 0), (e, 0), (e, e), (0, e))[rot] if not flip else ((0, e), (0, 0), (e, 0), (e, e))[rot]


def emit_placements(out, U, S, rng, counts, E, singles, arrays, targets, pitch, e, dense=False):
    """Singles + arrays of `targets` (mandatory refs first, then a pool)
    on a row-major walk over [0, E). Counts updated in place. `dense` (chip
    geometry): one array in fifty is a memory-style block - the children
    abut (pitch = their extent) and fill up to an eighth of the cell."""
    mandatory, pool = targets
    n_mand = len(mandatory)
    n_single = max(n_mand, singles)
    order = [1] * n_single + [2] * arrays
    rng.shuffle(order)
    left = len(order)
    x = y = 0
    px = py = 0
    prev = -1
    mi = 0
    for kind in order:
        if mi < n_mand and rng.random() * left < (n_mand - mi):
            child = mandatory[mi]
            mi += 1
        elif prev >= 0 and rng.random() < 0.35:
            child = prev
        else:
            child = pool[int(rng.random() ** 2 * len(pool))]
        left -= 1
        x += pitch * rng.randint(1, 3)
        if x + e > E:
            x = rng.randrange(pitch)
            y += pitch * rng.randint(1, 2)
            if y + e > E:
                y = rng.randrange(pitch)
        rot = rng.randrange(4) if rng.random() < 0.3 else 0
        flip = 1 if rng.random() < 0.15 else 0
        ax, ay = anchor(rot, flip, e)
        info = 0x30 | (rot << 1) | flip                 # X Y
        if child != prev:
            info |= 0xC0                                # C=1, N=1: reference number
        if kind == 2:
            info |= 0x08
        out.append(bytes((0x11, info)))
        if child != prev:
            out.append(U(child))
            prev = child
        out.append(S(x + ax - px))
        out.append(S(y + ay - py))
        px, py = x + ax, y + ay
        if kind == 2:
            r = rng.random()
            sp = pitch * rng.randint(1, 2)
            room = max(2, (E - x) // sp + 1)             # columns that fit before E
            if dense and rng.random() < 0.02 and e > 0:
                side = max(2, min(256, E // (8 * e)))
                na = max(2, min(rng.randint(2, side), (E - x) // e))
                nb = max(2, min(rng.randint(2, side), (E - y) // e))
                out.append(b"\x01" + U(na - 2) + U(nb - 2) + U(e) + U(e))
            elif r < 0.03:
                # arbitrary repetition (type 10): n points, stored as n-2,
                # n-1 g-deltas follow (dimensions are count-2 like type 1)
                n = min(rng.randint(4, 32), max(4, room))
                out.append(b"\x0a" + U(n - 2))
                for _ in range(n - 1):
                    out.append(gdelta(sp * rng.randint(1, 3), 0) if rng.random() < 0.5
                               else gdelta(0, sp * rng.randint(1, 3)))
            elif r < 0.25:
                out.append(b"\x02" + U(min(rng.randint(0, 14), room - 2)) + U(sp))
            elif r < 0.40:
                out.append(b"\x03" + U(rng.randint(0, 14)) + U(sp))
            else:
                out.append(b"\x01" + U(min(rng.randint(0, 6), room - 2)) + U(rng.randint(0, 6)) + U(sp) + U(sp))
            counts["array"] += 1
        else:
            counts["placement"] += 1
    while mi < n_mand:                                  # never drop a child
        child = mandatory[mi]
        mi += 1
        out.append(bytes((0x11, 0xF0)) + U(child) + S(0) + S(0))
        prev = child
        counts["placement"] += 1


def emit_shapes_legacy(out, U, S, P, rng, counts, k, leaf, E, n_rect, n_grid, gmem, n_poly, n_text):
    """Rectangles (singles + grids), polygons and texts grouped by layer."""
    lo, hi, (nl_lo, nl_hi) = P.layer_window(k, leaf)
    nl = min(hi - lo, rng.randint(nl_lo, nl_hi))
    layer_ids = sorted(rng.sample(range(lo, hi), nl))
    if leaf:
        w_lo, w_hi = (200, 2_000) if k >= 3 else (1_000, 20_000)
    else:
        w_lo, w_hi = (200, 2_000) if k >= 6 else (400, 20_000) if k >= 3 else (2_000, 200_000)
    w_hi = min(w_hi, max(w_lo + 1, E // 8))
    gx = gy = 0                                         # geometry modal (relative)
    per_layer = [0] * nl
    for _ in range(n_rect):
        per_layer[int(rng.random() ** 1.5 * nl)] += 1
    grid_layer = [0] * nl
    for _ in range(n_grid):
        grid_layer[rng.randrange(nl)] += 1
    poly_layer = [0] * nl
    for _ in range(n_poly):
        poly_layer[rng.randrange(nl)] += 1
    for li, lid in enumerate(layer_ids):
        layer, dt = P.layers[lid]
        lead = 0x03                                     # L D on the first record of the layer
        lead_bytes = U(layer) + U(dt)
        w = rng.randint(w_lo, w_hi)
        h = rng.randint(w_lo, w_hi)
        size_bits = 0x60
        size_bytes = U(w) + U(h)
        gap = max(1, w // 2)
        col = 0
        cols = max(1, E // (w + gap))
        for _ in range(per_layer[li]):
            if rng.random() < 0.25:
                w = rng.randint(w_lo, w_hi)
                h = rng.randint(w_lo, w_hi)
                size_bits = 0x60
                size_bytes = U(w) + U(h)
            x = gx + (w + gap) * rng.randint(1, 3)
            y = gy
            col += 1
            if col >= cols or x + w > E:
                col = 0
                x = rng.randrange(w + gap)
                y = gy + (h + gap) * rng.randint(1, 2)
                if y + h > E:
                    y = rng.randrange(h + gap)
            info = 0x18 | size_bits | lead              # X Y (+W H) (+L D)
            out.append(bytes((0x14, info)))
            if lead:
                out.append(lead_bytes)
                lead = 0
            if size_bits:
                out.append(size_bytes)
                size_bits = 0
            out.append(S(x - gx))
            out.append(S(y - gy))
            gx, gy = x, y
        counts["rect"] += per_layer[li]
        counts["rect_members"] += per_layer[li]
        for _ in range(grid_layer[li]):
            w = rng.randint(w_lo, w_hi)
            h = rng.randint(w_lo, w_hi)
            m = max(4, jitter(rng, gmem))
            sp_x = w + max(1, w // rng.randint(1, 4))
            sp_y = h + max(1, h // rng.randint(1, 4))
            r = rng.random()
            if r < 0.15:
                na, nb = min(m, max(2, E // sp_x)), 1
                span_x, span_y = (na - 1) * sp_x + w, h
            elif r < 0.30:
                na, nb = 1, min(m, max(2, E // sp_y))
                span_x, span_y = w, (nb - 1) * sp_y + h
            else:
                na = max(2, min(m // 2, int(math.sqrt(m) * rng.uniform(0.5, 2.0))))
                nb = max(2, m // na)
                na = min(na, max(2, E // sp_x))
                nb = min(nb, max(2, E // sp_y))
                span_x, span_y = (na - 1) * sp_x + w, (nb - 1) * sp_y + h
            x = rng.randrange(max(1, E - span_x))
            y = rng.randrange(max(1, E - span_y))
            out.append(bytes((0x14, 0x7C | lead)))      # W H X Y R (+L D)
            if lead:
                out.append(lead_bytes)
                lead = 0
            out.append(U(w) + U(h) + S(x - gx) + S(y - gy))
            gx, gy = x, y
            if nb == 1:
                out.append(b"\x02" + U(na - 2) + U(sp_x))
            elif na == 1:
                out.append(b"\x03" + U(nb - 2) + U(sp_y))
            else:
                out.append(b"\x01" + U(na - 2) + U(nb - 2) + U(sp_x) + U(sp_y))
            counts["grid"] += 1
            counts["rect_members"] += na * nb
        for _ in range(poly_layer[li]):
            x = gx + rng.randint(1, 3) * (w + gap)
            y = gy
            if x + w > E:
                x = rng.randrange(w + gap)
                y = gy + rng.randint(1, 2) * (h + gap)
                if y + h > E:
                    y = rng.randrange(h + gap)
            # manhattan point list, horizontal first, implicit closure:
            # 4 deltas = a 6-vertex L, 6 deltas = an 8-vertex staircase
            a = rng.randint(4, 60)
            b = rng.randint(2, 60)
            c = rng.randint(1, a - 2)
            d = rng.randint(2, 60)
            if rng.random() < 0.5:
                pts = b"\x00\x04" + S(a) + S(b) + S(-c) + S(d)
            else:
                e = rng.randint(1, a - c - 1)
                f = rng.randint(2, 60)
                pts = b"\x00\x06" + S(a) + S(b) + S(-c) + S(d) + S(-e) + S(f)
            out.append(bytes((0x15, 0x38 | lead)))      # P X Y (+L D)
            if lead:
                out.append(lead_bytes)
                lead = 0
            out.append(pts + S(x - gx) + S(y - gy))
            gx, gy = x, y
        counts["polygon"] += poly_layer[li]
    if n_text:
        tx = ty = 0
        lead = 0x03                                     # T L on the first text
        layer = P.layers[layer_ids[0]][0]
        for i in range(n_text):
            x = rng.randrange(E)
            y = rng.randrange(E)
            s = b"VDD" if i % 97 == 0 else b"VSS" if i % 89 == 0 else b"net%d" % rng.randrange(1_000_000)
            out.append(bytes((0x13, 0x58 | lead)) + bstr(s))    # C (inline) X Y (+T L)
            if lead:
                out.append(U(layer) + U(0))
                lead = 0
            out.append(S(x - tx) + S(y - ty))
            tx, ty = x, y
        counts["text"] += n_text


def logu(rng, lo, hi):
    """Log-uniform integer in [lo, hi]."""
    lo = max(1, lo)
    if hi <= lo:
        return lo
    return int(math.exp(rng.uniform(math.log(lo), math.log(hi))))


def size_band(k, leaf):
    """(wire width, shortest wire, via, box) ranges in dbu by level band."""
    if leaf or k >= 6:
        return (80, 320), 400, (80, 240), (200, 4_000)              # library cells
    if k >= 3:
        return (200, 1_600), 4_000, (200, 800), (1_000, 40_000)     # blocks
    return (1_600, 40_000), 40_000, (800, 4_000), (4_000, 400_000)  # top


def emit_shapes(out, U, S, P, rng, counts, k, leaf, E, n_rect, n_grid, gmem, n_poly, n_text):
    """Chip geometry: rectangles (singles + grids), polygons and texts grouped
    by layer, shaped by the layer's role (see the module docstring)."""
    lo, hi, (nl_lo, nl_hi) = P.layer_window(k, leaf)
    nl = min(hi - lo, rng.randint(nl_lo, nl_hi))
    layer_ids = sorted(rng.sample(range(lo, hi), nl))
    (w_lo, w_hi), len_lo, (v_lo, v_hi), (b_lo, b_hi) = size_band(k, leaf)
    w_hi = min(w_hi, max(w_lo + 1, E // 16))
    b_hi = min(b_hi, max(b_lo + 1, E // 8))
    len_lo = min(len_lo, max(2, E // 4))
    gx = gy = 0                                         # geometry modal (relative)
    per_layer = [0] * nl
    for _ in range(n_rect):
        per_layer[int(rng.random() ** 1.5 * nl)] += 1
    grid_layer = [0] * nl
    for _ in range(n_grid):
        grid_layer[rng.randrange(nl)] += 1
    poly_layer = [0] * nl
    for _ in range(n_poly):
        poly_layer[rng.randrange(nl)] += 1
    for li, lid in enumerate(layer_ids):
        layer, dt = P.layers[lid]
        role = ROLES[lid % 6]
        lead = 0x03                                     # L D on the first record of the layer
        lead_bytes = U(layer) + U(dt)
        widths = [logu(rng, w_lo, w_hi) for _ in range(3)]
        via = logu(rng, v_lo, v_hi)

        def shape():
            """(w, h, track pitch) of the next single of this layer."""
            if role == "wire_h" or role == "wire_v":
                wd = rng.choice(widths)
                ln = int(E * rng.uniform(0.8, 1.0)) if rng.random() < 0.05 else logu(rng, max(len_lo, 2 * wd), E)
                ln = max(wd, min(ln, E - 1))
                return (ln, wd, 2 * wd) if role == "wire_h" else (wd, ln, 2 * wd)
            if role == "via" or role == "fill":
                side = via if role == "via" else 3 * via
                return side, side, 2 * side
            if role == "block":
                # mostly small markers, a few large regions: never a layer that
                # covers its whole cell
                side = b_lo + int((max(b_lo + 1, E // 6) - b_lo) * rng.random() ** 6)
                other = max(b_lo, int(side * rng.uniform(0.4, 1.0)))
                return (side, other, other) if rng.random() < 0.5 else (other, side, side)
            side = logu(rng, b_lo, b_hi)
            other = max(1, int(side * rng.uniform(0.25, 1.0)))
            return (side, other, 2 * other) if rng.random() < 0.5 else (other, side, 2 * side)

        w, h, pitch = shape()
        lw = lh = -1                                    # rectangle modal width / height
        x, y = rng.randrange(max(1, w)), rng.randrange(max(1, pitch))
        for _ in range(per_layer[li]):
            if rng.random() < 0.4:                      # the rest repeat the size: buses, via rows
                w, h, pitch = shape()
            vertical = role == "wire_v"
            # laid end to end along a track (rows for horizontal wires and
            # boxes, columns for vertical wires), then on to a later track
            if vertical:
                y += h + max(1, w) * rng.randint(1, 8)
                if y + h > E:
                    y = rng.randrange(max(1, 2 * w))
                    x += pitch * rng.randint(1, 3)
                    if x + w > E:
                        x = rng.randrange(max(1, pitch))
            else:
                x += w + max(1, h) * rng.randint(1, 8)
                if x + w > E:
                    x = rng.randrange(max(1, 2 * h))
                    y += pitch * rng.randint(1, 3)
                    if y + h > E:
                        y = rng.randrange(max(1, pitch))
            x, y = max(0, min(x, E - w)), max(0, min(y, E - h))
            size_bits = (0x40 if w != lw else 0) | (0x20 if h != lh else 0)
            out.append(bytes((0x14, 0x18 | size_bits | lead)))   # X Y (+W) (+H) (+L D)
            if lead:
                out.append(lead_bytes)
                lead = 0
            if w != lw:
                out.append(U(w))
                lw = w
            if h != lh:
                out.append(U(h))
                lh = h
            out.append(S(x - gx))
            out.append(S(y - gy))
            gx, gy = x, y
        counts["rect"] += per_layer[li]
        counts["rect_members"] += per_layer[li]
        for _ in range(grid_layer[li]):
            m = max(4, jitter(rng, gmem))
            r = rng.random()
            if role == "wire_h" or role == "wire_v":
                wd = rng.choice(widths)
                if r < 0.30:
                    # a bus: one long wire repeated across its width
                    ln = max(wd, min(logu(rng, max(len_lo, 4 * wd), E), E - 1))
                    step = wd * rng.randint(2, 4)
                    n = max(2, min(m, E // step))
                    if role == "wire_h":
                        w, h, na, nb, sp_x, sp_y = ln, wd, 1, n, 0, step
                    else:
                        w, h, na, nb, sp_x, sp_y = wd, ln, n, 1, step, 0
                else:
                    # a field of short thin shapes (0.45 x 0.022 um at 0.8 x 1.0 um),
                    # in the layer's narrowest width so the field holds its members
                    wd = min(widths)
                    ln = wd * rng.randint(4, 20)
                    along, across = ln + wd * rng.randint(2, 8), wd * rng.randint(3, 12)
                    if role == "wire_h":
                        w, h, sp_x, sp_y = ln, wd, along, across
                    else:
                        w, h, sp_x, sp_y = wd, ln, across, along
                    na = max(2, min(m // 2, int(math.sqrt(m) * rng.uniform(0.5, 2.0))))
                    nb = max(2, m // na)
                    na = min(na, max(2, E // sp_x))
                    nb = min(nb, max(2, E // sp_y))
            else:
                if role == "via" or role == "fill":
                    w = h = via if role == "via" else 3 * via
                    sp_x = sp_y = w * rng.randint(2, 4)
                else:
                    w = logu(rng, b_lo, b_hi)
                    h = max(1, int(w * rng.uniform(0.25, 1.0)))
                    sp_x = w + max(1, w // rng.randint(1, 4))
                    sp_y = h + max(1, h // rng.randint(1, 4))
                if r < 0.15:
                    na, nb = min(m, max(2, E // sp_x)), 1
                elif r < 0.30:
                    na, nb = 1, min(m, max(2, E // sp_y))
                else:
                    na = max(2, min(m // 2, int(math.sqrt(m) * rng.uniform(0.5, 2.0))))
                    nb = max(2, m // na)
                    na = min(na, max(2, E // sp_x))
                    nb = min(nb, max(2, E // sp_y))
            w, h = max(1, min(w, E - 1)), max(1, min(h, E - 1))
            span_x, span_y = (na - 1) * sp_x + w, (nb - 1) * sp_y + h
            x = rng.randrange(max(1, E - span_x))
            y = rng.randrange(max(1, E - span_y))
            out.append(bytes((0x14, 0x7C | lead)))      # W H X Y R (+L D)
            if lead:
                out.append(lead_bytes)
                lead = 0
            out.append(U(w) + U(h) + S(x - gx) + S(y - gy))
            lw, lh = w, h
            gx, gy = x, y
            if nb == 1:
                out.append(b"\x02" + U(na - 2) + U(sp_x))
            elif na == 1:
                out.append(b"\x03" + U(nb - 2) + U(sp_y))
            else:
                out.append(b"\x01" + U(na - 2) + U(nb - 2) + U(sp_x) + U(sp_y))
            counts["grid"] += 1
            counts["rect_members"] += na * nb
        unit = max(2, widths[0] if role in ("wire_h", "wire_v") else via if role in ("via", "fill") else b_lo)
        x, y = gx, gy
        for _ in range(poly_layer[li]):
            # manhattan point list, horizontal first, implicit closure:
            # 4 deltas = a 6-vertex L, 6 deltas = an 8-vertex staircase,
            # arms a few wire widths wide and up to tens of widths long
            a = unit * rng.randint(4, 40)
            b = unit * rng.randint(1, 6)
            c = rng.randint(unit, a - 2 * unit)
            d = unit * rng.randint(2, 30)
            x += a + unit * rng.randint(2, 20)
            if x + a > E:
                x = rng.randrange(max(1, a))
                y += (b + d) * rng.randint(2, 6)
                if y + 3 * (b + d) > E:
                    y = rng.randrange(max(1, b + d))
            x, y = max(0, min(x, max(0, E - a))), max(0, min(y, max(0, E - 3 * (b + d))))
            if rng.random() < 0.5:
                pts = b"\x00\x04" + S(a) + S(b) + S(-c) + S(d)
            else:
                e = rng.randint(1, max(1, a - c - 1))
                f = unit * rng.randint(1, 10)
                pts = b"\x00\x06" + S(a) + S(b) + S(-c) + S(d) + S(-e) + S(f)
            out.append(bytes((0x15, 0x38 | lead)))      # P X Y (+L D)
            if lead:
                out.append(lead_bytes)
                lead = 0
            out.append(pts + S(x - gx) + S(y - gy))
            gx, gy = x, y
        counts["polygon"] += poly_layer[li]
    if n_text:
        tx = ty = 0
        lead = 0x03                                     # T L on the first text
        layer = P.layers[layer_ids[0]][0]
        for i in range(n_text):
            x = rng.randrange(E)
            y = rng.randrange(E)
            s = b"VDD" if i % 97 == 0 else b"VSS" if i % 89 == 0 else b"net%d" % rng.randrange(1_000_000)
            out.append(bytes((0x13, 0x58 | lead)) + bstr(s))    # C (inline) X Y (+T L)
            if lead:
                out.append(U(layer) + U(0))
                lead = 0
            out.append(S(x - tx) + S(y - ty))
            tx, ty = x, y
        counts["text"] += n_text


def gen_cell(ci):
    """One CELL record with its property and content; returns (bytes, counts)."""
    T, P = _T, _P
    U, S = T.U, T.S
    kind, k, local = P.kind_of(ci)
    row = P.levels[k]
    rng = random.Random((P.seed * 1_000_003 + ci) & 0xFFFFFFFFFFFF)
    out = [b"\x0d" + U(ci),                                       # CELL by reference
           b"\x1c\x14" + bstr(b"CELL_INFO") + b"\x08" + U(local),  # PROPERTY (1 uint)
           b"\x10"]                                               # XYRELATIVE
    counts = dict(placement=0, array=0, rect=0, grid=0, rect_members=0, polygon=0, text=0)
    chip = P.geometry == "chip"
    shapes = emit_shapes if chip else emit_shapes_legacy
    if kind == "leaf":
        E = row["lext"]
        lrect, lpoly = P.leaf_shapes(k)
        shapes(out, U, S, P, rng, counts, k, True, E,
                    jitter(rng, lrect * P.f) + 1, 0, 0, jitter(rng, lpoly * P.f), 0)
        return b"".join(out), counts
    E = row["hext"]
    nh = row["hier"]
    # chain: level k+1 hierarchy cells, each placed by exactly one level-k cell
    if k + 1 < len(P.levels):
        nxt = P.levels[k + 1]
        cstart, cn = P.hstart[k + 1], nxt["hier"]
        mandatory = list(range(cstart + local, cstart + cn, nh))
        extra = max(0, jitter(rng, row["chain"]) - len(mandatory))
        pool = [cstart + rng.randrange(cn) for _ in range(max(1, min(cn, 8)))]
        pitch = max(1, int(nxt["hext"] * rng.uniform(1.0, 1.2)))
        emit_placements(out, U, S, rng, counts, E, len(mandatory) + extra, 0,
                        (mandatory, pool), pitch, nxt["hext"])
    # leaves homed at this level: many single placements and arrays
    lstart, ln = P.lstart[k], row["leaf"]
    mandatory = list(range(lstart + local, lstart + ln, nh))
    pool = sorted(lstart + rng.randrange(ln) for _ in range(rng.randint(8, 48)))
    pitch = max(1, int(row["lext"] * rng.uniform(1.0, 1.3)))
    emit_placements(out, U, S, rng, counts, E, jitter(rng, row["place"]), jitter(rng, row["array"]),
                    (mandatory, pool), pitch, row["lext"], dense=chip)
    shapes(out, U, S, P, rng, counts, k, False, E, jitter(rng, row["rect"]),
                jitter(rng, row["grid"]), row["gmem"], jitter(rng, row["poly"]), jitter(rng, row["text"]))
    return b"".join(out), counts


def gen_batch(cells):
    blobs = []
    total = dict(placement=0, array=0, rect=0, grid=0, rect_members=0, polygon=0, text=0)
    for ci in cells:
        blob, counts = gen_cell(ci)
        blobs.append(blob)
        for key, v in counts.items():
            total[key] += v
    return b"".join(blobs), total, len(cells)


def batches(plan):
    """Cells in file order; big cells alone, small ones batched."""
    out = []
    for k, row in enumerate(plan.levels):
        per_cell = row["place"] + row["array"] + row["rect"] + row["grid"] + row["poly"]
        size = 1 if per_cell > 50_000 else max(1, min(256, int(200_000 // max(1, per_cell))))
        start, n = plan.hstart[k], row["hier"]
        for i in range(start, start + n, size):
            out.append(list(range(i, min(start + n, i + size))))
    for k, row in enumerate(plan.levels):
        start, n = plan.lstart[k], row["leaf"]
        for i in range(start, start + n, 256):
            out.append(list(range(i, min(start + n, i + 256))))
    return out


def header(plan):
    out = [MAGIC,
           uint(1) + bstr(b"1.0") + uint(0) + uint(UNIT) + uint(0) + uint(0) * 12]  # START
    for ci in range(plan.cells):
        out.append(uint(3) + bstr(plan.name(ci)))                                 # CELLNAME (implicit)
    for layer, dt in plan.layers:
        out.append(uint(11) + bstr(b"LY%dD%d" % (layer, dt)) + uint(3) + uint(layer)
                   + uint(3) + uint(dt))                                          # LAYERNAME exact/exact
    return b"".join(out)


def trailer():
    end = uint(2)
    pad = 256 - len(end) - 2 - 1
    end += bstr(b"\0" * pad) + uint(0)
    assert len(end) == 256
    return end


def fmt(n):
    return f"{int(n):,}"


def table(rows):
    print(f"{'stat':<12}{'this file':>20}{'MAIN01':>20}{'ratio':>8}")
    for name, a, b in rows:
        ratio = f"{a / b:.2f}" if b else "-"
        print(f"{name:<12}{fmt(a):>20}{fmt(b):>20}{ratio:>8}")


def print_plan(plan, scale):
    t = plan.expected()
    print(f"plan: scale {scale:g}, cells {fmt(t['cells'])} ({fmt(plan.hier_cells)} hierarchy + "
          f"{fmt(plan.cells - plan.hier_cells)} leaf), depth {len(plan.levels) - 1}")
    table([("layer", LAYERS, LAYERS), ("cell", t["cells"], TARGETS["cell"]),
           ("placement", t["placement"], TARGETS["placement"]),
           ("array", t["array"], TARGETS["array"]),
           ("rectangle", t["rect_members"], TARGETS["rectangle"]),
           ("polygon", t["polygon"], TARGETS["polygon"]), ("path", 0, 0),
           ("text", t["text"], TARGETS["text"]), ("property", t["cells"], TARGETS["property"]),
           ("bytes", t["bytes"], TARGETS["bytes"])])
    total = sum(t["depth_members"].values()) or 1
    share = sum(v for d, v in t["depth_members"].items() if d <= 5) / total
    print(f"rectangle members at depth 0-5: {share:.1%}; rect records {fmt(t['rect'])}, "
          f"grid records {fmt(t['grid'])}; flattened leaf instances ~{t['flat_leaf']:.2e}; "
          f"estimated size {t['bytes'] / 2**30:.2f} GiB")


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("out", nargs="?", help="output .oas path")
    ap.add_argument("--scale", type=float, default=1.0,
                    help="record totals scale by ~s, cells and per-cell counts by sqrt(s) (default 1)")
    ap.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 2) - 1))
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--geometry", choices=("chip", "legacy"), default="chip",
                    help="chip: layer roles, long thin wires, several size classes (default); "
                         "legacy: the near-square one-class file of 2026-09-17..18, byte for byte")
    ap.add_argument("--plan", action="store_true", help="print planned totals vs MAIN01 and exit")
    ap.add_argument("--verify", action="store_true",
                    help="read the result back with klayout.db (small scales only)")
    args = ap.parse_args()
    plan = Plan(args.scale, args.seed, args.geometry)
    print_plan(plan, args.scale)
    if args.plan or not args.out:
        if not args.out and not args.plan:
            print("no output path given; --plan only", file=sys.stderr)
        return
    t0 = time.monotonic()
    totals = dict(placement=0, array=0, rect=0, grid=0, rect_members=0, polygon=0, text=0)
    done = 0
    written = 0
    work = batches(plan)
    with open(args.out, "wb") as f:
        head = header(plan)
        f.write(head)
        written += len(head)
        last = time.monotonic()
        with mp.Pool(args.jobs, initializer=_init, initargs=(args.scale, args.seed, args.geometry)) as pool:
            for blob, counts, n in pool.imap(gen_batch, work, chunksize=1):
                f.write(blob)
                written += len(blob)
                done += n
                for key, v in counts.items():
                    totals[key] += v
                now = time.monotonic()
                if now - last >= 5:
                    last = now
                    el = now - t0
                    print(f"[gen] cells {done}/{plan.cells} {written / 2**20:,.0f} MiB "
                          f"{written / 2**20 / el:,.1f} MiB/s {el:,.0f}s", flush=True)
        f.write(trailer())
        written += 256
    el = time.monotonic() - t0
    size = os.path.getsize(args.out)
    print(f"wrote {args.out}: {size / 2**30:.2f} GiB in {el:,.0f}s")
    table([("layer", LAYERS, LAYERS), ("cell", plan.cells, TARGETS["cell"]),
           ("placement", totals["placement"], TARGETS["placement"]),
           ("array", totals["array"], TARGETS["array"]),
           ("rectangle", totals["rect_members"], TARGETS["rectangle"]),
           ("polygon", totals["polygon"], TARGETS["polygon"]), ("path", 0, 0),
           ("text", totals["text"], TARGETS["text"]), ("property", plan.cells, TARGETS["property"]),
           ("bytes", size, TARGETS["bytes"])])
    print(f"geometry {plan.geometry}, precision {UNIT}, max depth {len(plan.levels) - 1}; "
          f"rect records {fmt(totals['rect'])}, grid records {fmt(totals['grid'])}")
    nrec = {"placement": totals["placement"], "array": totals["array"], "rect": totals["rect"],
            "grid": totals["grid"], "polygon": totals["polygon"], "text": totals["text"]}
    est = sum(nrec[k] * plan.est_bytes[k] for k in nrec) + plan.cells * 60 + LAYERS * 12 + 300
    print(f"byte model: estimated {est / 2**20:,.0f} MiB vs actual {size / 2**20:,.0f} MiB "
          f"(ratio {size / est:.3f}; adjust EST_BYTES if far from 1)")
    if args.verify:
        import klayout.db as db
        ly = db.Layout()
        ly.read(args.out)
        top = ly.top_cell()
        print(f"klayout: {ly.cells()} cells, top {top.name}, layers {ly.layers()}, "
              f"top bbox {top.bbox()}, dbu {ly.dbu}, top instances {top.child_instances()}")


if __name__ == "__main__":
    main()
