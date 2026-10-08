"""Bounded, zoom-dependent markers for an entire DRC rule or cluster.

The pack's four-byte qboxes support overview aggregation without loading
the error geometry. Their lattice is deliberately NOT treated as exact:
boundary candidates, singletons, and views beyond its useful resolution
are refined from coordinate blocks. Refinement caches compact numeric
bboxes with a byte limit, independently of IcePack's DrcError cache.
"""

import math
from collections import OrderedDict, namedtuple

import numpy as np

from .drc import IcePack, STATUS_WAIVED, _ICE2_BLOCK, _uv, _unzz


Marker = namedtuple("Marker", "ci ei x y count waived bbox approximate")
_CHUNK = 1 << 16
_MAX_CELLS = 8192


class MarkerQueryCancelled(Exception):
    """The caller superseded this query; no partial result is returned."""


def _check_cancelled(cancelled):
    if cancelled is not None and cancelled():
        raise MarkerQueryCancelled()


def _intersects(boxes, bounds):
    x0, y0, x1, y1 = bounds
    return ((boxes[:, 0] <= x1) & (boxes[:, 2] >= x0) &
            (boxes[:, 1] <= y1) & (boxes[:, 3] >= y0))


class _Bins:
    """One fixed-size numeric accumulator, shared by all selected rules."""

    def __init__(self, bounds, width, height, cell_px):
        self.bounds = bounds
        # Keep the rendering and accumulator cost bounded even on 4K/8K
        # monitors. This changes spatial resolution, never the population.
        cell_px = max(1.0, float(cell_px))
        while True:
            nx = max(1, int(math.ceil(width / cell_px)))
            ny = max(1, int(math.ceil(height / cell_px)))
            if nx * ny <= _MAX_CELLS:
                break
            cell_px *= max(1.01, math.sqrt(nx * ny / _MAX_CELLS))
        self.cell_px = cell_px
        self.nx, self.ny = nx, ny
        size = nx * ny
        self.count = np.zeros(size, dtype=np.int64)
        self.waived = np.zeros(size, dtype=np.int64)
        self.sx = np.zeros(size, dtype=np.float64)
        self.sy = np.zeros(size, dtype=np.float64)
        self.bbox = np.empty((size, 4), dtype=np.float64)
        self.bbox[:, :2] = np.inf
        self.bbox[:, 2:] = -np.inf
        self.ci = np.full(size, -1, dtype=np.int64)
        self.ei = np.full(size, -1, dtype=np.int64)
        self.approximate = np.zeros(size, dtype=bool)

    def add(self, ci, indices, boxes, waived, approximate=False):
        if not len(indices):
            return
        x0, y0, x1, y1 = self.bounds
        # Intersecting long polygons can have their centers outside the
        # view. Their aggregate anchors stay visible at the nearest edge.
        x = np.clip(boxes[:, 0] / 2 + boxes[:, 2] / 2, x0, x1)
        y = np.clip(boxes[:, 1] / 2 + boxes[:, 3] / 2, y0, y1)
        ix = np.minimum(((x - x0) / (x1 - x0) * self.nx)
                        .astype(np.int64), self.nx - 1)
        iy = np.minimum(((y - y0) / (y1 - y0) * self.ny)
                        .astype(np.int64), self.ny - 1)
        keys = iy * self.nx + ix
        occupied, first = np.unique(keys, return_index=True)
        new = self.count[occupied] == 0
        self.ci[occupied[new]] = ci
        self.ei[occupied[new]] = indices[first[new]]
        size = len(self.count)
        self.count += np.bincount(keys, minlength=size)
        self.waived += np.bincount(keys[waived], minlength=size)
        self.sx += np.bincount(keys, weights=x, minlength=size)
        self.sy += np.bincount(keys, weights=y, minlength=size)
        for col in (0, 1):
            np.minimum.at(self.bbox[:, col], keys, boxes[:, col])
        for col in (2, 3):
            np.maximum.at(self.bbox[:, col], keys, boxes[:, col])
        if approximate:
            self.approximate[occupied] = True


    def add_aggregates(self, ci, indices, boxes, counts, waived, centers):
        """Merge tree summaries into the same bounded screen bins."""
        if not len(indices):
            return
        x0, y0, x1, y1 = self.bounds
        x, y = np.clip(centers[:, 0], x0, x1), np.clip(centers[:, 1], y0, y1)
        ix = np.minimum(((x - x0) / (x1 - x0) * self.nx).astype(np.int64), self.nx - 1)
        iy = np.minimum(((y - y0) / (y1 - y0) * self.ny).astype(np.int64), self.ny - 1)
        keys = iy * self.nx + ix
        occupied, first = np.unique(keys, return_index=True)
        new = self.count[occupied] == 0
        self.ci[occupied[new]], self.ei[occupied[new]] = ci, indices[first[new]]
        size = len(self.count)
        self.count += np.bincount(keys, weights=counts, minlength=size).astype(np.int64)
        self.waived += np.bincount(keys, weights=waived, minlength=size).astype(np.int64)
        self.sx += np.bincount(keys, weights=x * counts, minlength=size)
        self.sy += np.bincount(keys, weights=y * counts, minlength=size)
        for col in (0, 1):
            np.minimum.at(self.bbox[:, col], keys, boxes[:, col])
        for col in (2, 3):
            np.maximum.at(self.bbox[:, col], keys, boxes[:, col])
        self.approximate[occupied] = True


class MarkerIndex:
    """Screen-space aggregation with bounded working memory.

    Instances are intended for one query worker, not concurrent queries.
    Membership uses the same ``mask(start, count)`` protocol as
    ``IcePack.query_rect``; neither clusters nor whole rules are enumerated
    as Python member lists. Review bytes are read anew on every query, so
    only geometry is cached and status changes cannot stale the cache.
    """

    def __init__(self, db, cache_bytes=32 * 1024 * 1024, spatial=False):
        self.db = db
        self.cache_bytes = max(0, int(cache_bytes))
        self._cache = OrderedDict()
        self._cache_size = 0
        self._packed = isinstance(db, IcePack)
        self.spatial = bool(spatial)
        self._spatial_rules = OrderedDict()
        self._spatial_unavailable = set()

    def _decode_block(self, ci, block, cancelled=None):
        """Read exact um bboxes without constructing geometry objects."""
        db = self.db
        start = block * _ICE2_BLOCK
        if not self._packed:
            errors = db.checks[ci].errors
            stop = min(start + _ICE2_BLOCK, len(errors))
            boxes = np.empty((stop - start, 4), dtype=np.float64)
            for j, ei in enumerate(range(start, stop)):
                _check_cancelled(cancelled)
                boxes[j] = errors[ei].bbox()
            return boxes
        rec = db._blk[int(db._dir_bs[ci]) + block]
        pos, count = int(rec["off"]), int(rec["cnt"])
        boxes = np.empty((count, 4), dtype=np.float64)
        buf, prec = db._map, db.precision
        pfx = pfy = 0
        for j in range(count):
            _check_cancelled(cancelled)
            knpts, pos = _uv(buf, pos)
            npts = knpts >> 1
            if not npts:
                raise ValueError("packed DRC error has no vertices")
            d, pos = _uv(buf, pos)
            x = pfx + _unzz(d)
            d, pos = _uv(buf, pos)
            y = pfy + _unzz(d)
            pfx, pfy = x, y
            xmin = xmax = x
            ymin = ymax = y
            for k in range(npts - 1):
                if not k % 1024:
                    _check_cancelled(cancelled)
                d, pos = _uv(buf, pos)
                x += _unzz(d)
                d, pos = _uv(buf, pos)
                y += _unzz(d)
                xmin, xmax = min(xmin, x), max(xmax, x)
                ymin, ymax = min(ymin, y), max(ymax, y)
            boxes[j] = (xmin / prec, ymin / prec,
                        xmax / prec, ymax / prec)
        return boxes

    def _exact_boxes(self, ci, indices, cancelled):
        out = np.empty((len(indices), 4), dtype=np.float64)
        if not len(indices):
            return out
        # Callers provide sorted indices, so no full-member sort or large
        # temporary mapping is needed to visit each 64-error block once.
        blocks = indices // _ICE2_BLOCK
        starts = np.r_[0, np.flatnonzero(blocks[1:] != blocks[:-1]) + 1]
        stops = np.r_[starts[1:], len(indices)]
        for a, b in zip(starts, stops):
            _check_cancelled(cancelled)
            block = int(blocks[a])
            key = (ci, block)
            boxes = self._cache.get(key)
            if boxes is None:
                boxes = self._decode_block(ci, block, cancelled)
                if boxes.nbytes <= self.cache_bytes:
                    while self._cache_size + boxes.nbytes > self.cache_bytes:
                        _key, old = self._cache.popitem(last=False)
                        self._cache_size -= old.nbytes
                    self._cache[key] = boxes
                    self._cache_size += boxes.nbytes
            else:
                self._cache.move_to_end(key)
            out[a:b] = boxes[indices[a:b] - block * _ICE2_BLOCK]
        return out

    def _status(self, ci, start, count):
        db = self.db
        if self._packed:
            base = int(db._dir_es[ci]) + start
            return db._status[base:base + count] == STATUS_WAIVED
        if hasattr(db, "get_status"):
            return np.fromiter((db.get_status(ci, ei) == STATUS_WAIVED
                                for ei in range(start, start + count)),
                               dtype=bool, count=count)
        return np.zeros(count, dtype=bool)

    @staticmethod
    def _member_values(member, indices):
        if member is None:
            return np.ones(len(indices), dtype=bool)
        if hasattr(member, 'contains_many'):
            return member.contains_many(indices)
        owner = getattr(member, '_owner', None)
        if owner is not None and hasattr(owner, '_row_for_error'):
            return owner._row_for_error[indices] == member._row
        if hasattr(member, '_starts'):
            starts, stops = member._starts, member._stops
            k = np.searchsorted(starts, indices, side='right') - 1
            if not len(starts):
                return np.zeros(len(indices), dtype=bool)
            return (k >= 0) & (indices < stops[np.maximum(k, 0)])
        if hasattr(member, 'indices'):
            k = np.searchsorted(member.indices, indices)
            if not len(member.indices):
                return np.zeros(len(indices), dtype=bool)
            return (k < len(member.indices)) & (member.indices[np.minimum(k, len(member.indices)-1)] == indices)
        # Protocol fallback stays bounded even if original IDs are scattered.
        out = np.zeros(len(indices), dtype=bool)
        chunks = indices // _CHUNK
        for chunk in np.unique(chunks):
            keep = chunks == chunk
            start = int(chunk) * _CHUNK
            out[keep] = member.mask(start, _CHUNK)[indices[keep] - start]
        return out

    def _spatial_query(self, ci, membership, waived, bins, bounds,
                       width, height, cancelled):
        from .drc_spatial import prepare_rule
        tree = self._spatial_rules.get(ci)
        if tree is None:
            try:
                tree = prepare_rule(self.db, ci, cancelled=cancelled)
            except OSError:
                # Unwritable/shared cache: preserve full counts through the
                # original scanner, still isolated in the worker process.
                self._spatial_unavailable.add(ci)
                return False
            self._spatial_rules[ci] = tree
            # Fixed node/filtered-summary memory independent of visited rules.
            while len(self._spatial_rules) > 2:
                self._spatial_rules.popitem(last=False)
        else:
            self._spatial_rules.move_to_end(ci)
        nodes, wcounts = tree.summaries(membership, waived, cancelled)
        if nodes is None:
            return True
        db = self.db
        cb = np.asarray(db._cbb[ci], dtype=float) / db.precision
        origin, step = cb[:2], (cb[2:] - cb[:2]) / 255
        roundoff = (np.abs(origin) + 255 * np.abs(step)) * (4 * np.finfo(float).eps)
        x0, y0, x1, y1 = bounds
        pixels = np.array([width / (x1-x0), height / (y1-y0)])
        refine = bool(np.any(step * pixels > bins.cell_px / 2))
        es = int(db._dir_es[ci])

        def boxes_um(q):
            boxes = np.empty((len(q), 4), dtype=float)
            boxes[:, :2] = np.maximum(np.nextafter(origin + q[:, :2] * step - roundoff, -np.inf), cb[:2])
            boxes[:, 2:] = np.minimum(np.nextafter(origin + q[:, 2:] * step + roundoff, np.inf), cb[2:])
            return boxes

        aggregates, aggregate_boxes = [], []
        current = np.array([0], dtype=np.int64)
        level = 0
        while len(current):
            _check_cancelled(cancelled)
            current = current[nodes['count'][current] > 0]
            boxes = boxes_um(nodes['box'][current])
            hit = _intersects(boxes, bounds)
            current, boxes = current[hit], boxes[hit]
            # Vectorized breadth-first traversal keeps overview cost tied to
            # visible tree nodes, without one Python/Numpy call per node.
            small = np.all((256 / (2 ** level) + 1) * step * pixels <= bins.cell_px)
            inside = ((boxes[:, 0] >= x0) & (boxes[:, 1] >= y0) &
                      (boxes[:, 2] <= x1) & (boxes[:, 3] <= y1))
            aggregate = inside if small else np.zeros(len(current), dtype=bool)
            aggregates.extend(current[aggregate].tolist())
            aggregate_boxes.extend(boxes[aggregate])
            current = current[~aggregate]
            if level < tree.depth:
                current = (current[:, None] * 4 + np.arange(1, 5)).reshape(-1)
                level += 1
                continue
            break
        for node in current:
            _check_cancelled(cancelled)
            ids = tree.leaf_ids(node)
            for start in range(0, len(ids), _CHUNK):
                _check_cancelled(cancelled)
                indices = np.sort(np.asarray(ids[start:start+_CHUNK], dtype=np.int64))
                keep = self._member_values(membership, indices)
                status = db._status[es + indices] == STATUS_WAIVED
                if waived is not None:
                    keep &= status if waived else ~status
                indices, status = indices[keep], status[keep]
                if not len(indices):
                    continue
                boxes = boxes_um(db._qbox[es + indices])
                hit = _intersects(boxes, bounds)
                indices, status, boxes = indices[hit], status[hit], boxes[hit]
                inside_rows = ((boxes[:, 0] >= x0) & (boxes[:, 1] >= y0) &
                               (boxes[:, 2] <= x1) & (boxes[:, 3] <= y1))
                exact = np.ones(len(indices), dtype=bool) if refine else ~inside_rows
                bins.add(ci, indices[~exact], boxes[~exact], status[~exact], approximate=True)
                if np.any(exact):
                    exact_boxes = self._exact_boxes(ci, indices[exact], cancelled)
                    hit = _intersects(exact_boxes, bounds)
                    bins.add(ci, indices[exact][hit], exact_boxes[hit], status[exact][hit])
        if aggregates:
            ids = np.asarray(aggregates, dtype=np.int64)
            counts = nodes['count'][ids].astype(np.int64)
            centers = origin + np.column_stack((nodes['sx'][ids], nodes['sy'][ids])) / counts[:, None] * step
            bins.add_aggregates(ci, nodes['rep'][ids].astype(np.int64),
                                np.asarray(aggregate_boxes), counts,
                                np.zeros(len(ids), dtype=np.int64) if wcounts is None else wcounts[ids], centers)
        return True

    def query(self, bounds_um, width_px, height_px, checks=None,
              members=None, waived=None, cell_px=16, cancelled=None,
              declutter=False):
        """Return at most 8192 immutable ``Marker`` values.

        Every selected error whose bbox intersects the view contributes
        once, including errors after the first list page. ``waived`` is
        None/True/False; each marker's ``waived`` field is the number of
        waived contributors. ``ci, ei`` identify a representative member,
        not an expandable member list. Counts are exact; only aggregate
        locations/bboxes may use the pack lattice (``approximate=True``).
        Singletons always have exact centers/bboxes. A cancelled query
        raises ``MarkerQueryCancelled``, never returns a partial scope.
        ``declutter`` additionally merges only these bounded summaries to
        limit screen coverage and separate neighboring markers.  Its merged
        centers are approximate, while counts and bbox unions are retained.
        """
        _check_cancelled(cancelled)
        bounds = tuple(float(v) for v in bounds_um)
        if len(bounds) != 4 or not all(math.isfinite(v) for v in bounds):
            raise ValueError("marker bounds must contain four finite values")
        x0, y0, x1, y1 = bounds
        width, height = float(width_px), float(height_px)
        if x1 <= x0 or y1 <= y0 or width <= 0 or height <= 0:
            return []
        if not all(math.isfinite(v) for v in (width, height, cell_px)):
            raise ValueError("marker screen dimensions must be finite")
        bins = _Bins(bounds, width, height, cell_px)
        db = self.db
        selected = (range(len(db.checks)) if checks is None
                    else sorted(set(int(ci) for ci in checks)))
        for ci in selected:
            _check_cancelled(cancelled)
            if not 0 <= ci < len(db.checks):
                continue
            n = len(db.checks[ci].errors)
            membership = members.get(ci) if members is not None else None
            if not n:
                continue
            if self.spatial and self._packed and ci not in self._spatial_unavailable:
                if self._spatial_query(ci, membership, waived, bins, bounds,
                                       width, height, cancelled):
                    continue
            if self._packed:
                cb = db._cbb[ci]
                cb_um = np.asarray(cb, dtype=np.float64) / db.precision
                if not _intersects(cb_um.reshape(1, 4), bounds)[0]:
                    continue
                origin = cb_um[:2]
                step = np.array((int(cb[2]) - int(cb[0]),
                                 int(cb[3]) - int(cb[1])),
                                dtype=np.float64) / (255 * db.precision)
                roundoff = ((np.abs(origin) + 255 * np.abs(step)) *
                            (4 * np.finfo(np.float64).eps))
                refine = (step[0] * width / (x1 - x0) > bins.cell_px / 2
                          or step[1] * height / (y1 - y0) > bins.cell_px / 2)
                es = int(db._dir_es[ci])
                bs = int(db._dir_bs[ci])
                bx0, by0 = (math.nextafter(v * db.precision, -math.inf)
                            for v in bounds[:2])
                bx1, by1 = (math.nextafter(v * db.precision, math.inf)
                            for v in bounds[2:])
            for start in range(0, n, _CHUNK):
                _check_cancelled(cancelled)
                count = min(_CHUNK, n - start)
                keep = (membership.mask(start, count) if membership is not None
                        else np.ones(count, dtype=bool))
                status = self._status(ci, start, count)
                if waived is not None:
                    keep &= status if waived else ~status
                if not np.any(keep):
                    continue
                if self._packed:
                    # Block bboxes are exact. They reject even qbox-cell
                    # false positives cheaply when file order is spatial.
                    first = start // _ICE2_BLOCK
                    last = (start + count + _ICE2_BLOCK - 1) // _ICE2_BLOCK
                    blocks = db._blk[bs + first:bs + last]
                    block_keep = ((blocks["x0"] <= bx1) &
                                  (blocks["x1"] >= bx0) &
                                  (blocks["y0"] <= by1) &
                                  (blocks["y1"] >= by0))
                    keep &= np.repeat(block_keep, _ICE2_BLOCK)[:count]
                    if not np.any(keep):
                        continue
                    qs = db._qbox[es + start:es + start + count]
                    boxes = np.empty((count, 4), dtype=np.float64)
                    boxes[:, :2] = origin + qs[:, :2] * step
                    boxes[:, 2:] = origin + qs[:, 2:] * step
                    # Preserve conservative bounds through floating-point
                    # conversion as well as through lattice quantization.
                    boxes[:, :2] = np.nextafter(boxes[:, :2] - roundoff, -np.inf)
                    boxes[:, 2:] = np.nextafter(boxes[:, 2:] + roundoff, np.inf)
                    boxes[:, :2] = np.maximum(boxes[:, :2], cb_um[:2])
                    boxes[:, 2:] = np.minimum(boxes[:, 2:], cb_um[2:])
                    keep &= _intersects(boxes, bounds)
                    indices = np.flatnonzero(keep)
                    if not len(indices):
                        continue
                    boxes = boxes[indices]
                    status = status[indices]
                    indices = indices + start
                    inside = ((boxes[:, 0] >= x0) & (boxes[:, 1] >= y0) &
                              (boxes[:, 2] <= x1) & (boxes[:, 3] <= y1))
                    exact = np.ones(len(indices), dtype=bool) if refine else ~inside
                    coarse = ~exact
                    bins.add(ci, indices[coarse], boxes[coarse], status[coarse],
                             approximate=True)
                    if np.any(exact):
                        boxes = self._exact_boxes(ci, indices[exact], cancelled)
                        hit = _intersects(boxes, bounds)
                        bins.add(ci, indices[exact][hit], boxes[hit],
                                 status[exact][hit])
                else:
                    indices = np.flatnonzero(keep) + start
                    boxes = self._exact_boxes(ci, indices, cancelled)
                    hit = _intersects(boxes, bounds)
                    bins.add(ci, indices[hit], boxes[hit], status[keep][hit])
        out = []
        for k in np.flatnonzero(bins.count):
            _check_cancelled(cancelled)
            count = int(bins.count[k])
            ci, ei = int(bins.ci[k]), int(bins.ei[k])
            approximate = bool(bins.approximate[k])
            if count == 1:
                bb = self._exact_boxes(ci, np.array([ei], dtype=np.int64),
                                       cancelled)[0]
                x, y = bb[0] / 2 + bb[2] / 2, bb[1] / 2 + bb[3] / 2
                approximate = False
            else:
                bb = bins.bbox[k]
                x, y = bins.sx[k] / count, bins.sy[k] / count
            out.append(Marker(ci, ei, float(x), float(y), count,
                              int(bins.waived[k]), tuple(float(v) for v in bb),
                              approximate))
        if declutter:
            from .drc_marker_layout import compact_markers
            return compact_markers(out, bounds, width, height, cancelled)
        return out
