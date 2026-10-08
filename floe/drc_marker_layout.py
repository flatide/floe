"""Keep dense DRC marker overlays sparse using only bounded summaries.

The spatial query already accounts for every visible error.  This module
works on its at-most-8192 markers, never on error IDs or geometry.  Coarser
screen bins control total painted area; a second neighbor pass also joins
markers whose centroids happen to meet at a bin corner.  Counts and review
statuses are conserved through both operations.
"""

import math

from .drc_marker_style import aggregate_radius, marker_cell_px


_AREA_FRACTION = 0.10
_GAP_PX = 4.0
_SINGLE_RADIUS = math.sqrt(12.5)  # Circumscribed radius of a 5px square.
_MAX_COLLISION_PASSES = 12


def _check_cancelled(cancelled):
    if cancelled is not None and cancelled():
        # Import lazily: MarkerIndex invokes this module after its query.
        from .drc_markers import MarkerQueryCancelled
        raise MarkerQueryCancelled()


def _screen(marker, bounds, width, height):
    """Match the GUI's rounded, clipped painting position, including edges."""
    x0, y0, x1, y1 = bounds
    x = (marker.x - x0) / (x1 - x0) * width
    y = (y1 - marker.y) / (y1 - y0) * height
    return (int(round(min(max(0.0, x), max(0.0, width - 1)))),
            int(round(min(max(0.0, y), max(0.0, height - 1)))))


def _radius(marker, width, height):
    # One extra pixel conservatively includes the antialiased disk edge.
    return (_SINGLE_RADIUS if marker.count == 1 else
            aggregate_radius(marker.count, width, height) + 1.0)


def _area(markers, width, height):
    return math.fsum(25.0 if marker.count == 1 else
                     math.pi * _radius(marker, width, height) ** 2
                     for marker in markers)


def _merge(group, bounds, width, height):
    if len(group) == 1:
        # Exact singleton coordinates/bboxes must survive an untouched bin,
        # even for a long polygon whose true center is outside the viewport.
        return group[0]
    count = sum(int(marker.count) for marker in group)
    waived = sum(int(marker.waived) for marker in group)
    sx = sy = 0
    for marker in group:
        x, y = _screen(marker, bounds, width, height)
        sx += x * int(marker.count)
        sy += y * int(marker.count)
    x0, y0, x1, y1 = bounds
    bbox = (min(marker.bbox[0] for marker in group),
            min(marker.bbox[1] for marker in group),
            max(marker.bbox[2] for marker in group),
            max(marker.bbox[3] for marker in group))
    # Weight the displayed anchors, not off-screen geometry centers.  This
    # prevents crossing polygons from dragging a merged marker out of view.
    return group[0]._replace(
        x=x0 + (sx / count) / width * (x1 - x0),
        y=y1 - (sy / count) / height * (y1 - y0),
        count=count, waived=waived, bbox=bbox, approximate=True)


def _grid_merge(markers, bounds, width, height, pitch, cancelled):
    cells = {}
    for i, marker in enumerate(markers):
        if not i % 64:
            _check_cancelled(cancelled)
        x, y = _screen(marker, bounds, width, height)
        key = (int(x // pitch), int(y // pitch))
        cells.setdefault(key, []).append(marker)
    out = []
    for i, group in enumerate(cells.values()):
        if not i % 64:
            _check_cancelled(cancelled)
        out.append(_merge(group, bounds, width, height))
    return out


def _collision_merge(markers, bounds, width, height, cancelled):
    """Join overlapping disk envelopes using neighboring spatial buckets.

    A grid alone cannot prevent overlap: occupants of four different cells
    can all lie at the same corner.  Connected components merge those
    occupants without losing any counts.  The caller repeats this pass as
    a merged centroid or its larger count band can meet a new neighbor.
    """
    count = len(markers)
    if count < 2:
        return markers
    radii = [_radius(marker, width, height) for marker in markers]
    pitch = 2.0 * max(radii) + _GAP_PX
    positions = [_screen(marker, bounds, width, height) for marker in markers]
    parents = list(range(count))

    def find(i):
        while parents[i] != i:
            parents[i] = parents[parents[i]]
            i = parents[i]
        return i

    buckets = {}
    comparisons = 0
    joined = False
    for i, (x, y) in enumerate(positions):
        if not i % 64:
            _check_cancelled(cancelled)
        bx, by = int(x // pitch), int(y // pitch)
        for ny in range(by - 1, by + 2):
            for nx in range(bx - 1, bx + 2):
                for j in buckets.get((nx, ny), ()):
                    comparisons += 1
                    if not comparisons % 256:
                        _check_cancelled(cancelled)
                    xi, yi = positions[j]
                    distance = radii[i] + radii[j] + _GAP_PX
                    if (x - xi) ** 2 + (y - yi) ** 2 >= distance ** 2:
                        continue
                    a, b = find(i), find(j)
                    if a != b:
                        # Keep the earliest representative deterministically.
                        parents[max(a, b)] = min(a, b)
                        joined = True
        buckets.setdefault((bx, by), []).append(i)
    if not joined:
        return markers
    groups = {}
    for i, marker in enumerate(markers):
        if not i % 64:
            _check_cancelled(cancelled)
        groups.setdefault(find(i), []).append(marker)
    out = []
    for i, group in enumerate(groups.values()):
        if not i % 64:
            _check_cancelled(cancelled)
        out.append(_merge(group, bounds, width, height))
    return out


def compact_markers(markers, bounds, width, height, cancelled=None):
    """Conserve the full population while leaving most layout pixels clear.

    Coarsen screen bins until conservative marker footprints occupy at most
    10% of the viewport, then enforce a 4px gap between their envelopes.
    Refinement only visits the query's bounded summaries, independent of
    whether they represent a hundred or a hundred million errors.  Sparse
    unmerged markers stay exact; changed aggregates retain their union bbox
    for zoom navigation and are explicitly marked approximate.

    A viewport smaller than one marker cannot satisfy an area percentage;
    in that case retain one marker instead of dropping errors or looping.
    """
    _check_cancelled(cancelled)
    current = list(markers)
    if len(current) < 2:
        return current
    width, height = float(width), float(height)
    if width <= 0 or height <= 0:
        return current
    target = width * height * _AREA_FRACTION
    pitch = marker_cell_px(width, height)
    max_pitch = max(width, height, pitch)
    current = _grid_merge(current, bounds, width, height, pitch, cancelled)
    collision_passes = 0
    while len(current) > 1:
        _check_cancelled(cancelled)
        area = _area(current, width, height)
        if area > target or collision_passes >= _MAX_COLLISION_PASSES:
            # Growing pitch monotonically also bounds pathological repeated
            # centroid collisions.  At max_pitch all anchors share one bin.
            growth = max(1.25, math.sqrt(area / target)) if target else 2.0
            if collision_passes >= _MAX_COLLISION_PASSES:
                growth = max(growth, 2.0)
            pitch = min(max_pitch, pitch * growth)
            current = _grid_merge(current, bounds, width, height,
                                  pitch, cancelled)
            collision_passes = 0
            continue
        merged = _collision_merge(current, bounds, width, height, cancelled)
        if len(merged) == len(current):
            break
        current = merged
        collision_passes += 1
        # Recheck both conditions: two square singletons can become a disk
        # with a larger footprint, and weighted centers can move toward an
        # otherwise separate aggregate after a collision merge.
    _check_cancelled(cancelled)
    return current
