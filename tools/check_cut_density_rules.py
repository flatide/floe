#!/usr/bin/env python3
"""Small oracles for density-review proposals; no renderer/index changes.

The rank experiment uses a fixed finite pool ONLY to check selection rules.
It is not a proposed OVR-style production representation or an O(1) query.
Presence correction is deliberately separate from the rank-subset proof.
"""
import hashlib
from bisect import bisect_right
from itertools import islice
import math

from mock_cut_density import (
    BASE_SCALE, ZOOMS, candidates, neighborhood_areas, neighborhood_targets,
)


def independent_quantizer_rises():
    rising = {}
    for presence in (False, True):
        seeds = []
        for seed in range(256):
            rows = [neighborhood_targets(BASE_SCALE * z, 1 / 256, 8,
                                          "dither", presence, seed) for z in ZOOMS]
            counts = [area + added for _, _, area, added in rows]
            if any(b > a for a, b in zip(counts, counts[1:])):
                seeds.append(seed)
        rising[presence] = seeds
        print(f"independent quantizer, presence={presence}: "
              f"{len(seeds)}/256 seeds with a count increase")
    assert len(rising[False]) == 11 and not rising[True], rising


def ranked_array():
    # Both modes use exactly the same candidate identities at every level.
    # T is a selection probability; it is NOT the u used in stochastic rounding.
    points = list(islice(candidates(), 4096))
    levels = []
    previous_min = None
    previous_level = None
    for zoom in ZOOMS:
        scale = BASE_SCALE * zoom
        level, step, areas = neighborhood_areas(scale, 8)
        groups = {}
        for i, (x, y) in enumerate(points):
            groups.setdefault((math.floor(x / step), math.floor(y / step)), []).append(i)
        own = {key: min(1.0, areas[key] * scale**2 / len(ids))
               for key, ids in groups.items()}
        bounded = dict(own)
        if previous_min is not None:
            assert level == previous_level + 1
            for (cx, cy), child_t in previous_min.items():
                parent = cx // 2, cy // 2
                bounded[parent] = min(bounded[parent], child_t)
        levels.append((groups, own, bounded))
        previous_min, previous_level = bounded, level
    for seed in range(16):
        # Pool identities are stable in this oracle; production keys must come
        # from world-space hierarchy, never from a page or worker sequence.
        ranks = [int.from_bytes(hashlib.blake2b(f"rank/{seed}/{i}".encode(),
                                               digest_size=8).digest(), "big") / 2**64
                 for i in range(len(points))]
        counts = {"own": [], "parent-min": []}
        for mode, offset in (("own", 1), ("parent-min", 2)):
            previous = None
            for row in levels:
                groups, threshold = row[0], row[offset]
                selected = {i for key, ids in groups.items() for i in ids
                            if ranks[i] < threshold[key]}
                if previous is not None and mode == "parent-min":
                    assert selected <= previous, (seed, mode)
                previous = selected
                counts[mode].append(len(selected))
        if seed == 0:
            for mode, row in counts.items():
                print(f"fixed-pool rank {mode}, seed 0, no presence: {row}")
    print("parent-min: world-anchor subset holds for all 16 seeds x 6 transitions")


def imbalanced_children():
    # Four nonempty children, 100 valid anchors in each. A legitimate unequal
    # mass distribution: child quotas 80, 80, 80, 1. Half-scale parent mass=60.25.
    child_t = (0.8, 0.8, 0.8, 0.01)
    ranks = {(child, i): (i + 0.5) / 100 for child in range(4) for i in range(100)}
    fine = {a for a, rank in ranks.items() if rank < child_t[a[0]]}
    parent_mass = sum(100 * t for t in child_t) / 4
    own_t = parent_mass / len(ranks)
    min_t = min(own_t, *child_t)
    own = {a for a, rank in ranks.items() if rank < own_t}
    bounded = {a for a, rank in ranks.items() if rank < min_t}
    assert len(fine) == 241 and len(own) == 60 and len(bounded) == 4
    assert bounded <= fine and not own <= fine
    print(f"imbalanced children: parent area={parent_mass:g}, own={len(own)}, "
          f"parent-min={len(bounded)} ({100 * (1-len(bounded)/parent_mass):.2f}% under area)")


def presence_counterexample():
    # Equal support areas. The parent's lowest raw rank belongs to a child
    # below the density threshold that emitted nothing. A raw min anchor pops in.
    tau = 0.01
    densities = (0.001, 0.1, 0.1, 0.1)
    ranks = (0.001, 0.2, 0.3, 0.4)
    # Each child's support after domain clipping projects to 0.01 px2:
    # all area quotas can be zero, even with larger aggregation grid cells.
    fine = {i for i, rho in enumerate(densities) if rho >= tau}
    assert sum(densities) / 4 >= tau
    raw_parent_anchor = min(range(4), key=lambda i: ranks[i])
    inherited_anchor = min(fine, key=lambda i: ranks[i])
    assert raw_parent_anchor not in fine and inherited_anchor in fine
    print(f"presence: raw parent minimum {raw_parent_anchor} was absent; "
          f"minimum of surviving children {inherited_anchor} already existed")


def cumulative_cut_counterexample():
    # One 10x10 square. Consider a pixel wholly inside it: F(8)=0, F(16)=1.
    # Exact shape_cut is strict min_side < cut, not a continuous area weighting.
    for cut in (9, 12):
        alpha = math.log2(cut / 8)
        truth = float(10 < cut)
        interpolated = alpha
        assert 0 <= truth <= 1 and abs(truth-interpolated) <= 1
        print(f"cumulative F8=0,F16=1, min_side=10, cut={cut}: "
              f"exact={truth:g}, log interpolation={interpolated:.6f}, "
              f"error={interpolated-truth:+.6f}")
    # Size-only and depth-only coverage cannot recover joint eligibility:
    # a small deep shape and a large shallow shape can overlap this same pixel.
    shapes = ((4, 2), (12, 0))
    size_only = any(size < 8 for size, _ in shapes)
    depth_only = any(depth <= 0 for _, depth in shapes)
    joint = any(size < 8 and depth <= 0 for size, depth in shapes)
    assert size_only and depth_only and not joint
    print("size/depth marginals: both cover pixel, but joint F(k=3,d=0)=0")


def quantized_cut_partition():
    # Simulated file header: generate canonical integer DBU boundaries ONCE.
    # The consumer chooses a stored boundary; it never regenerates powers.
    sizes = (1, 2, 5, 7, 8, 9, 10, 11, 12, 15, 16, 17, 23, 31, 64)
    pages = (tuple(range(0, 5)), tuple(range(5, 10)), tuple(range(10, len(sizes))))
    depths = {i: depth for page, depth in zip(pages, (0, 1, 3)) for i in page}
    cases = 0
    for divisions in (1, 2, 4):
        boundaries = sorted({math.ceil(2**(i/divisions)) for i in range(8*divisions+1)})
        for requested in range(1, 129):
            cut = boundaries[bisect_right(boundaries, requested)-1]
            assert cut <= requested
            for depth in range(4):
                visible = {i for i, d in depths.items() if d <= depth}
                # Cumulative density sees every eligible source shape, even
                # if its page was never decoded. Original selection uses the
                # same cut in its page AND record predicates.
                density = {i for i in visible if sizes[i] < cut}
                selected_pages = [p for p in pages if max(sizes[i] for i in p) >= cut]
                original = {i for p in selected_pages for i in p
                            if i in visible and sizes[i] >= cut}
                assert not (density & original)
                assert density | original == visible
                cases += 1
        lower = 3 / 2**(1/divisions)
        print(f"quantized cut, 1/{divisions} octave: ideal medium threshold "
              f"({lower:.6f}, 3] px; nominal plane count multiplier {divisions}")
    # An original plan made BEFORE lowering the cut has already lost this page.
    requested, cut, size = 12, 8, 10
    stale_page_selected = size >= requested
    corrected_page_selected = size >= cut
    density_has_shape = size < cut
    assert not stale_page_selected and not density_has_shape
    assert corrected_page_selected and not density_has_shape
    print(f"quantized partition: {cases} mixed-page/depth cases OK; "
          "a stale page cut leaves a gap despite matching raster/density cuts")


def main():
    independent_quantizer_rises()
    ranked_array()
    imbalanced_children()
    presence_counterexample()
    cumulative_cut_counterexample()
    quantized_cut_partition()
    print("CUT DENSITY RULE ORACLES: OK")


if __name__ == "__main__":
    main()
