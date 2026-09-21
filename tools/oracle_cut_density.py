#!/usr/bin/env python3
"""Step 1 of docs/CUT_DENSITY_DESIGN.ko.md §7: the selection rules on more fixtures.

A small, self-contained model of the density representation below the cut, NOT
the renderer or an index format. Every fixture is one layer whose shapes are
all below the cut; its union area is exact (the unique rectangles of a fixture
are disjoint, and duplicates are removed before any area is counted - which is
the union rule the index has to reproduce by construction).

Zones are world-anchored dyadic cells whose projected side is [pitch, 2*pitch)
px; halving the scale merges 2x2 zones into their parent. Every rule places its
dots on the same fixed, world-anchored ANCHORS inside the shapes (never in
empty space), ranked by a hash of their world position, so the rules differ
only in how many dots a zone shows and which anchors they are:

  round       N = round-half-up(M)                    per zone, per level
  dither      N = floor(M) + [u(level, zone) < frac M] per zone, per level
  parent-min  rank < T,  T = min(own M/|A|, T of the zone's children)
  survivors   N = dither(M), chosen among the anchors the children showed

M is the zone's union area in px^2 at the current scale. The presence dot - at
most one per PRESENCE zone (2^k x 2^k area zones; k=0 is one area zone, as §2
of the design writes it) whose density clears tau, only when the area rule gave
none of its area zones a dot - is counted apart from the area dots; for
'survivors' it is taken from the children's dots when they have one. With
px-aware placement a zone takes the next-ranked candidate whose pixel is still
free in this view.

Reported per fixture and rule: the area dots against the true area, the
presence dots added, anchors that POP IN on a zoom-out step (shown at the
coarser level but not at the finer one), steps where the count rises, dots
inside an empty corridor that is still several pixels wide, the dots left at
the coarsest zoom, and pixel collisions.

    .venv/bin/python tools/oracle_cut_density.py [--pitch 8,16] [--tau 1/256]
                                                 [--seed 0] [--png DIR] [--seeds 16]
"""
import argparse
from fractions import Fraction
import hashlib
import math
from pathlib import Path

TILE = 220          # the viewport of every zoom, px
ZOOMS = [1.0, 0.5, 0.25, 0.125, 0.0625, 0.03125, 0.015625]
RULES = ("round", "dither", "parent-min", "survivors")


def h01(*parts):
    key = "/".join(str(part) for part in parts).encode()
    return int.from_bytes(hashlib.blake2b(key, digest_size=8).digest(), "big") / 2**64


# ------------------------------------------------------------------ fixtures

class Fixture:
    def __init__(self, name, rects, note, corridor=None, raw=None):
        unique = sorted(set(rects))
        for i, a in enumerate(unique):
            for b in unique[i + 1:i + 40]:
                assert not _overlap(a, b), (name, a, b)
        self.name, self.note, self.corridor = name, note, corridor
        self.rects = unique
        self.raw = raw if raw is not None else rects
        self.x0 = min(x for x, _, _, _ in unique)
        self.y0 = min(y for _, y, _, _ in unique)
        self.x1 = max(x + w for x, _, w, _ in unique)
        self.y1 = max(y + h for _, y, _, h in unique)
        self.union_area = sum(w * h for _, _, w, h in unique)
        self.raw_area = sum(w * h for _, _, w, h in self.raw)
        self.base_scale = 0.9 * TILE / max(self.x1 - self.x0, self.y1 - self.y0)
        self.anchors = _anchors(unique, max(self.x1 - self.x0, self.y1 - self.y0) / 1024)


def _overlap(a, b):
    ax, ay, aw, ah = a
    bx, by, bw, bh = b
    return ax < bx + bw and bx < ax + aw and ay < by + bh and by < ay + ah


def _anchors(rects, spacing):
    """World-anchored candidate points inside the shapes: a lattice of the
    given world spacing over each shape (a shape thinner than the spacing gets
    one row or column on its centre line, the smallest its centre). Their
    identity is their rounded world position, the same at every level."""
    points = set()
    for x, y, w, h in rects:
        nx, ny = max(1, int(w / spacing)), max(1, int(h / spacing))
        points.update((round(x + (i + 0.5) * w / nx, 6), round(y + (j + 0.5) * h / ny, 6))
                      for j in range(ny) for i in range(nx))
    return sorted(points)


def fixtures():
    array = [(i * 0.8, j * 1.0, 0.45, 0.022) for j in range(34) for i in range(25)]
    out = [Fixture("array", array, "25x34 thin bars (the mock_cut_density fixture)")]
    # two dense blocks of small squares, 40 % of the extent empty between them
    left = [(i * 0.5, j * 0.5, 0.2, 0.2) for j in range(40) for i in range(12)]
    right = [(14.0 + i * 0.5, j * 0.5, 0.2, 0.2) for j in range(40) for i in range(12)]
    out.append(Fixture("corridor", left + right, "two blocks, an 8.2-unit empty corridor",
                       corridor=(6.0, 0.0, 8.0, 20.0)))
    # long hairlines across the extent, 0.01 wide, 2 apart
    lines = [(0.0, k * 2.0, 40.0, 0.01) for k in range(20)]
    out.append(Fixture("lines", lines, "20 hairlines 40 x 0.01, pitch 2"))
    # every bar of the array twice, and a third of them three times
    dup = array + array + array[::3]
    out.append(Fixture("duplicates", dup, "the array with every bar 2-3 times", raw=dup))
    # a dense cluster and sparse specks around it (uneven children)
    dense = [(10.0 + i * 0.25, 10.0 + j * 0.25, 0.15, 0.15) for j in range(16) for i in range(16)]
    specks = [(1.0 + 3.7 * (k % 9), 1.0 + 3.3 * (k // 9), 0.05, 0.05) for k in range(81)]
    specks = [s for s in specks if not (9.5 < s[0] < 14.5 and 9.5 < s[1] < 14.5)]
    out.append(Fixture("uneven", dense + specks, "a 16x16 dense cluster in sparse specks"))
    return out


# ------------------------------------------------------------------ zones

def level_of(scale, pitch):
    return math.ceil(math.log2(pitch / scale))


def zone_areas(fx, level):
    """Exact union area per world-anchored zone (the unique rects are
    disjoint), and each zone's support clipped to the fixture's domain."""
    step = 2.0 ** level
    areas = {}
    for x, y, w, h in fx.rects:
        for gy in range(math.floor(y / step), math.ceil((y + h) / step)):
            for gx in range(math.floor(x / step), math.ceil((x + w) / step)):
                ow = min(x + w, (gx + 1) * step) - max(x, gx * step)
                oh = min(y + h, (gy + 1) * step) - max(y, gy * step)
                if ow > 0 and oh > 0:
                    areas[gx, gy] = areas.get((gx, gy), 0.0) + ow * oh
    support = {}
    for (gx, gy) in areas:
        sw = min(fx.x1, (gx + 1) * step) - max(fx.x0, gx * step)
        sh = min(fx.y1, (gy + 1) * step) - max(fx.y0, gy * step)
        support[gx, gy] = sw * sh
    return step, areas, support


def anchors_by_zone(fx, step):
    zones = {}
    for a in fx.anchors:
        zones.setdefault((math.floor(a[0] / step), math.floor(a[1] / step)), []).append(a)
    return zones


def dithered(mass, level, key, seed):
    whole = math.floor(mass)
    return whole + int(h01("dither", seed, level, key[0], key[1]) < mass - whole)


# ------------------------------------------------------------------ rules

def run_rule(fx, rule, pitch, tau, seed, presence_k=0, pixel_aware=False, index_chain=False):
    """One rule over the zoom series, finest first. Answers per zoom:
    (area px^2, area dots, presence dots, per-zone selection, scale).

    presence_k: the presence dot is decided per presence zone of 2^k x 2^k
    area zones - at most one per such zone, only where none of its zones got
    an area dot (k=0: per area zone, as §2 of the design writes it).
    pixel_aware: a zone's dots take the next-ranked candidate whose pixel is
    still free, so N dots light N pixels where the candidates allow it.
    index_chain: the survivor chain is built WITHOUT the pixel skip (an index
    cannot know the view's pixel grid) and only the shown dots skip pixels -
    so a coarser view can show a candidate a finer one skipped."""
    rank = {a: h01("rank", seed, a[0], a[1]) for a in fx.anchors}
    rows = []
    previous = None          # (level, per-zone selection, per-zone threshold)
    for zoom in ZOOMS:
        scale = fx.base_scale * zoom
        level = level_of(scale, pitch)
        step, areas, support = zone_areas(fx, level)
        zone_anchors = anchors_by_zone(fx, step)
        selected, thresholds, candidates, chain = {}, {}, {}, {}
        area_dots = presence = short = 0
        taken = set()
        for key in sorted(areas):
            area = areas[key]
            mass = area * scale * scale
            pool = sorted(zone_anchors.get(key, []), key=rank.get)
            if rule == "survivors" and previous is not None:
                pool = sorted((a for c in _children(key) for a in previous[1].get(c, ())), key=rank.get)
            candidates[key] = pool
            if rule == "round":
                n = math.floor(mass + 0.5)
            elif rule in ("dither", "survivors"):
                n = dithered(mass, level, key, seed)
            else:  # parent-min
                own_pool = zone_anchors.get(key, [])
                own = min(1.0, mass / len(own_pool)) if own_pool else 0.0
                t = own
                if previous is not None:
                    children = [previous[2][c] for c in _children(key) if c in previous[2]]
                    if children:
                        t = min([t] + children)
                thresholds[key] = t
                pool = [a for a in pool if rank[a] < t]
                n = len(pool)
            chosen = []
            if pixel_aware:
                for a in pool:
                    if len(chosen) >= n:
                        break
                    px = project(fx, a, scale)
                    if px not in taken:
                        taken.add(px)
                        chosen.append(a)
            else:
                chosen = pool[:n]
            area_dots += len(chosen)
            short += n - len(chosen)
            selected[key] = chosen
            chain[key] = pool[:n] if index_chain else chosen
        if tau > 0:
            # one presence dot per presence zone that has content, clears tau
            # and got no area dot in any of its area zones
            groups = {}
            for key in areas:
                groups.setdefault((key[0] >> presence_k, key[1] >> presence_k), []).append(key)
            pstep = step * (1 << presence_k)
            for (px_, py_), keys in groups.items():
                if any(selected[k] for k in keys):
                    continue
                area = sum(areas[k] for k in keys)
                sw = min(fx.x1, (px_ + 1) * pstep) - max(fx.x0, px_ * pstep)
                sh = min(fx.y1, (py_ + 1) * pstep) - max(fx.y0, py_ * pstep)
                if area / (sw * sh) < tau:
                    continue
                pool = sorted((a for k in keys for a in candidates[k]), key=rank.get)
                if not pool:
                    pool = sorted((a for k in keys for a in zone_anchors.get(k, [])), key=rank.get)
                if pool:
                    best = min(keys, key=lambda k: min((rank[a] for a in candidates[k]), default=2.0))
                    selected[best] = [pool[0]]
                    chain[best] = [pool[0]]
                    presence += 1
        rows.append((sum(areas.values()) * scale * scale, area_dots, presence, selected, scale, short))
        previous = (level, chain, thresholds)
    return rows


def _children(key):
    gx, gy = key
    return [(2 * gx + dx, 2 * gy + dy) for dy in (0, 1) for dx in (0, 1)]


# ------------------------------------------------------------------ metrics

def project(fx, point, scale):
    cx, cy = (fx.x0 + fx.x1) / 2, (fx.y0 + fx.y1) / 2
    return (math.floor(TILE / 2 + (point[0] - cx) * scale),
            math.floor(TILE / 2 + (point[1] - cy) * scale))


def measure(fx, rows):
    out = []
    before = None
    for i, (area, area_dots, presence, selected, scale, short) in enumerate(rows):
        dots = {a for chosen in selected.values() for a in chosen}
        pixels = {project(fx, a, scale) for a in dots}
        pop = len(dots - before) if before is not None else 0
        corridor = 0
        if fx.corridor:
            cx0, cy0, cw, ch = fx.corridor
            width_px = cw * scale
            if width_px >= 4:
                p0 = project(fx, (cx0, cy0), scale)
                p1 = project(fx, (cx0 + cw, cy0 + ch), scale)
                corridor = sum(1 for px, py in pixels
                               if p0[0] + 1 < px < p1[0] - 1 and p0[1] < py < p1[1])
        out.append({"zoom": ZOOMS[i], "area": area, "area_dots": area_dots, "presence": presence,
                    "dots": len(dots), "pixels": len(pixels), "collisions": len(dots) - len(pixels),
                    "pop_in": pop, "corridor": corridor, "short": short})
        before = dots
    return out


def summarize(stats):
    mid = [s for s in stats if 0.0625 <= s["zoom"] <= 0.5]
    rel = [abs(s["area_dots"] - s["area"]) / max(s["area"], 1.0) for s in mid]
    worst = min((s["area_dots"] / s["area"] for s in stats if s["area"] >= 2), default=1.0)
    rises = sum(1 for a, b in zip(stats, stats[1:]) if b["dots"] > a["dots"])
    lit = [s["pixels"] / s["area"] for s in mid if s["area"] >= 2]
    return {"mid_err": sum(rel) / len(rel), "worst_ratio": worst,
            "lit": sum(lit) / len(lit) if lit else 0.0,
            "pop_in": sum(s["pop_in"] for s in stats), "rises": rises,
            "corridor": max(s["corridor"] for s in stats), "last": stats[-1]["dots"],
            "presence": sum(s["presence"] for s in stats),
            "collisions": sum(s["collisions"] for s in stats),
            "short": sum(s["short"] for s in stats)}


def draw(fx, per_rule, path):
    from PIL import Image, ImageDraw
    pad, gap, label = 8, 6, 70
    image = Image.new("RGB", (label + len(ZOOMS) * (TILE + gap), len(per_rule) * (TILE + gap) + 20), (10, 16, 24))
    ink = ImageDraw.Draw(image)
    for r, (rule, rows) in enumerate(per_rule):
        y = 20 + r * (TILE + gap)
        ink.text((4, y + TILE // 2), rule, fill=(200, 200, 200))
        for c, (_, _, _, selected, scale, _) in enumerate(rows):
            x = label + c * (TILE + gap)
            ink.rectangle((x, y, x + TILE - 1, y + TILE - 1), outline=(40, 50, 60))
            for chosen in selected.values():
                for a in chosen:
                    px, py = project(fx, a, scale)
                    if 0 <= px < TILE and 0 <= py < TILE:
                        image.putpixel((x + px, y + py), (110, 205, 250))
    for c, zoom in enumerate(ZOOMS):
        ink.text((label + c * (TILE + gap) + 4, 4), "%g%%" % (zoom * 100), fill=(200, 200, 200))
    image.save(path)


# ------------------------------------------------------------------ main

def row(fx, rule, pitch, tau, seed, presence_k, pixel_aware, index_chain=False):
    rows = run_rule(fx, rule, pitch, tau, seed, presence_k, pixel_aware, index_chain)
    return rows, summarize(measure(fx, rows))


HEAD = "%-26s %5s | %7s %6s %6s %6s %5s %5s %8s %4s %6s %5s" % (
    "rule", "pitch", "mid err", "lit", "worst", "pop-in", "rise", "corr", "presence", "last", "coll", "short")


def line(label, pitch, s):
    return "%-26s %5g | %6.1f%% %6.2f %6.2f %6d %5d %5d %8d %4d %6d %5d" % (
        label, pitch, 100 * s["mid_err"], s["lit"], s["worst_ratio"], s["pop_in"], s["rises"],
        s["corridor"], s["presence"], s["last"], s["collisions"], s["short"])


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--pitch", default="8,16")
    ap.add_argument("--tau", default="1/256")
    ap.add_argument("--seed", type=int, default=0)
    ap.add_argument("--png", help="write one image per fixture (first pitch, the four rules) here")
    ap.add_argument("--seeds", type=int, default=0,
                    help="instead: seeds 0..N-1 of three candidates at the first pitch, as ranges")
    args = ap.parse_args()
    pitches = [float(p) for p in args.pitch.split(",")]
    tau = float(Fraction(args.tau))
    if args.seeds:
        return seed_ranges(pitches[0], tau, args.seeds)
    print("columns: mid err = mean |area dots - area| / area over 50 %..6.25 %; lit = unique pixels /")
    print("area there (collisions and presence included); worst = min area dots / area where area >= 2;")
    print("pop-in = anchors shown on a zoom-out step that the finer step did not show; corr = dots in a")
    print("still >= 4 px wide empty corridor; last = dots at the coarsest zoom; short = area dots a")
    print("zone's N asked for that its candidates could not give; tau = %g" % tau)
    for fx in fixtures():
        print("\n== %s: %s (unique %d of %d shapes, union %.4g of summed %.4g, %d anchors)"
              % (fx.name, fx.note, len(fx.rects), len(fx.raw), fx.union_area, fx.raw_area, len(fx.anchors)))
        print(HEAD)
        for pitch in pitches:
            per_rule = []
            for rule in RULES:
                rows, s = row(fx, rule, pitch, tau, args.seed, 0, False)
                per_rule.append((rule, rows))
                print(line(rule, pitch, s))
            if args.png and pitch == pitches[0]:
                Path(args.png).mkdir(parents=True, exist_ok=True)
                draw(fx, per_rule, Path(args.png) / ("oracle-%s.png" % fx.name))
        for pitch in pitches:
            for k, aware, index in ((2, False, False), (0, True, False), (2, True, False),
                                    (2, True, True)):
                _, s = row(fx, "survivors", pitch, tau, args.seed, k, aware, index)
                print(line("survivors k=%d%s%s" % (k, " px-aware" if aware else "",
                                                   " index" if index else ""), pitch, s))


CANDIDATES = (("dither k=0", "dither", 0, False, False),
              ("parent-min k=0", "parent-min", 0, False, False),
              ("survivors k=2 px-aware", "survivors", 2, True, False),
              ("survivors k=2 px index", "survivors", 2, True, True))


def seed_ranges(pitch, tau, seeds):
    """The same fixtures under seeds 0..seeds-1 (the rank and dither hashes):
    how many seeds pop anchors in, the lit range, and the worst cases."""
    print("seeds 0..%d, pitch %g, tau %g: pop-in = seeds with any (max per seed); lit min..max;"
          % (seeds - 1, pitch, tau))
    print("rise = seeds with a count rise; last0 = seeds with no dot left; short = max per seed")
    for fx in fixtures():
        for label, rule, k, aware, index in CANDIDATES:
            ss = [row(fx, rule, pitch, tau, seed, k, aware, index)[1] for seed in range(seeds)]
            pops = [s["pop_in"] for s in ss]
            lits = [s["lit"] for s in ss]
            print("%-11s %-24s pop-in %2d (max %2d)  lit %.2f..%.2f  rise %d  last0 %d  short %d"
                  % (fx.name, label, sum(1 for p in pops if p), max(pops), min(lits), max(lits),
                     sum(1 for s in ss if s["rises"]), sum(1 for s in ss if s["last"] == 0),
                     max(s["short"] for s in ss)))


if __name__ == "__main__":
    main()
