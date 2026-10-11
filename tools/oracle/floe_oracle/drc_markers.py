"""Full-scope DRC point markers backed by exact center caches.

The query worker projects every selected error into a bounded pixel raster.
Coincident screen centers retain one deterministic picking representative;
the original exact coordinates remain available for zoom reprojection.
"""

import numpy as np

from .drc import IcePack, STATUS_WAIVED, _ICE2_BLOCK


class MarkerQueryCancelled(Exception):
    """The caller superseded this query; no final partial result is returned."""


def _check_cancelled(cancelled):
    if cancelled is not None and cancelled():
        raise MarkerQueryCancelled()


class MarkerIndex:
    """Exact point queries for entire rules, clusters and delta groups.

    Membership follows the ``mask(start, count)`` protocol; no rule-sized
    Python error/member list is constructed. Review status is read anew for
    each query, while immutable packed centers use the persistent cache.
    """

    def __init__(self, db):
        self.db = db
        self._packed = isinstance(db, IcePack)

    def _decode_block(self, ci, block, cancelled=None):
        """Read one bounded bbox block from an in-memory/plain-DB fixture.

        Packed errors are decoded directly into integer center sums by
        drc_points, without populating IcePack's GUI geometry-object cache.
        """
        if self._packed:
            raise ValueError('packed centers require direct coordinate decoding')
        errors = self.db.checks[ci].errors
        start = block * _ICE2_BLOCK
        stop = min(start + _ICE2_BLOCK, len(errors))
        boxes = np.empty((stop - start, 4), dtype=np.float64)
        for j, ei in enumerate(range(start, stop)):
            _check_cancelled(cancelled)
            boxes[j] = errors[ei].bbox()
        return boxes

    def _status(self, ci, start, count):
        db = self.db
        if self._packed:
            base = int(db._dir_es[ci]) + start
            return db._status[base:base + count] == STATUS_WAIVED
        if hasattr(db, 'get_status'):
            return np.fromiter((db.get_status(ci, ei) == STATUS_WAIVED
                                for ei in range(start, start + count)),
                               dtype=bool, count=count)
        return np.zeros(count, dtype=bool)

    def query(self, bounds_um, width_px, height_px, checks=None,
              members=None, waived=None, cancelled=None, progress=None,
              selected=None):
        """Return a bounded ``PointMarkers`` raster for complete scopes.

        Every selected error whose center is inside the viewport contributes
        once, including errors beyond the current list page. Duplicate screen
        centers are drawn once. Highlighted selections take picking priority,
        followed by unwaived state and original error order within each group.
        ``progress`` receives independent cumulative frames about every 0.5s;
        the final return is complete or cancellation raises an exception.
        """
        from .drc_points import query_points
        return query_points(self, bounds_um, width_px, height_px, checks,
                            members, waived, cancelled, progress=progress,
                            selected=selected)
