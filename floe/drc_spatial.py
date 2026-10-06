"""Persistent, bounded-memory quadtree over a pack's conservative qboxes.

The original error order is never changed. Leaves contain original rule-local
IDs. Counts are exact; conservative node bounds only reject / aggregate when
safe, and boundary candidates are refined against packed geometry by callers.
No geometry object is created while preparing this index.
"""
import json
import math
import os
import shutil
import tempfile

import numpy as np

from .cachepath import drc_analysis_dir
from .drc_analysis import pack_identity

VERSION = 1
CHUNK = 1 << 16
# 10M errors: 87,381 small nodes + 40MB postings. Preparation never sorts a
# whole rule or allocates a per-error Python object / in-memory ID array.
NODE = np.dtype([('count', '<u8'), ('sx', '<f8'), ('sy', '<f8'),
                 ('rep', '<u8'), ('box', 'u1', (4,))])
_EMPTY = np.iinfo(np.uint64).max


def _check(cancelled):
    if cancelled is not None and cancelled():
        # Late import avoids a module cycle with MarkerIndex.
        from .drc_markers import MarkerQueryCancelled
        raise MarkerQueryCancelled()


def _depth(n):
    return min(8, max(1, int(math.ceil(math.log(max(1, n / 256), 4)))))


def _codes(q, depth):
    x = ((q[:, 0].astype(np.uint16) + q[:, 2]) // 2) >> (8 - depth)
    y = ((q[:, 1].astype(np.uint16) + q[:, 3]) // 2) >> (8 - depth)
    out = np.zeros(len(q), dtype=np.uint32)
    for bit in range(depth):
        out |= ((x >> bit) & 1).astype(np.uint32) << (bit * 2)
        out |= ((y >> bit) & 1).astype(np.uint32) << (bit * 2 + 1)
    return out


def _empty_nodes(depth):
    nodes = np.zeros((4 ** (depth + 1) - 1) // 3, dtype=NODE)
    nodes['rep'] = _EMPTY
    nodes['box'][:, :2] = 255
    return nodes


def _add(leaves, codes, q, ids):
    if not len(ids):
        return
    size = len(leaves)
    leaves['count'] += np.bincount(codes, minlength=size).astype(np.uint64)
    leaves['sx'] += np.bincount(codes, weights=(q[:, 0].astype(float) + q[:, 2]) / 2,
                                 minlength=size)
    leaves['sy'] += np.bincount(codes, weights=(q[:, 1].astype(float) + q[:, 3]) / 2,
                                 minlength=size)
    np.minimum.at(leaves['rep'], codes, ids.astype(np.uint64))
    for col in (0, 1):
        np.minimum.at(leaves['box'][:, col], codes, q[:, col])
    for col in (2, 3):
        np.maximum.at(leaves['box'][:, col], codes, q[:, col])


def _parents(nodes, depth):
    for level in range(depth - 1, -1, -1):
        first, child = (4 ** level - 1) // 3, (4 ** (level + 1) - 1) // 3
        count = 4 ** level
        parents = nodes[first:first + count]
        children = nodes[child:child + count * 4].reshape(count, 4)
        for field in ('count', 'sx', 'sy'):
            parents[field] = children[field].sum(axis=1)
        parents['rep'] = children['rep'].min(axis=1)
        parents['box'][:, :2] = children['box'][:, :, :2].min(axis=1)
        parents['box'][:, 2:] = children['box'][:, :, 2:].max(axis=1)


def rule_path(db, ci):
    return os.path.join(drc_analysis_dir(db.path), 'spatial-v%d-%s-%d' %
                        (VERSION, pack_identity(db.path), int(ci)))


class SpatialRule:
    def __init__(self, path, db, ci):
        with open(os.path.join(path, 'meta.json')) as stream:
            meta = json.load(stream)
        if not isinstance(meta, dict):
            raise ValueError('invalid spatial index manifest')
        n = len(db.checks[ci].errors)
        if (meta.get('version'), meta.get('identity'), meta.get('ci'), meta.get('n')) != (
                VERSION, pack_identity(db.path), ci, n):
            raise ValueError('stale spatial index')
        depth = meta['depth']
        if not isinstance(depth, int) or not 1 <= depth <= 8:
            raise ValueError('invalid spatial index depth')
        self.nodes = np.load(os.path.join(path, 'nodes.npy'), mmap_mode='r', allow_pickle=False)
        self.ids = np.load(os.path.join(path, 'ids.npy'), mmap_mode='r', allow_pickle=False)
        self.offsets = np.load(os.path.join(path, 'offsets.npy'), mmap_mode='r', allow_pickle=False)
        if (self.nodes.dtype != NODE or self.nodes.shape != ((4 ** (depth + 1) - 1) // 3,)
                or self.ids.dtype != np.dtype('<u4' if n <= 2 ** 32 else '<u8')
                or self.ids.shape != (n,) or self.offsets.dtype != np.dtype('<u8')
                or self.offsets.shape != (4 ** depth + 1,)
                or int(self.offsets[-1]) != n or int(self.nodes['count'][0]) != n):
            raise ValueError('invalid spatial index arrays')
        self.path, self.depth, self.ci, self.db = path, depth, ci, db
        self.leaf_start = (4 ** depth - 1) // 3
        self._summary_key = self._summary_value = None

    def summaries(self, membership=None, waived=None, cancelled=None):
        """Cache one exact filtered tree per scope/review revision.

        First use of an arbitrary membership scans numeric arrays once in the
        worker process. Repeated views then visit only the tree and leaves.
        Review file timestamps, not just waived totals, detect count-preserving
        status swaps. Uniform rules bypass status reads entirely.
        """
        db, ci = self.db, self.ci
        n = len(db.checks[ci].errors)
        wc = int(db._wcount[ci])
        if membership is None and wc in (0, n):
            if waived is not None and bool(waived) != bool(wc):
                return None, None
            return self.nodes, (None if wc == 0 else self.nodes['count'])
        path = getattr(db, '_waive_path', db.path)
        st = os.stat(path)
        key = (getattr(membership, 'cache_key', id(membership)), waived,
               st.st_mtime_ns, st.st_ctime_ns, st.st_size)
        if key == self._summary_key:
            return self._summary_value
        nodes = _empty_nodes(self.depth)
        leaves = nodes[self.leaf_start:]
        wcounts = np.zeros(len(nodes), dtype=np.uint64)
        es = int(db._dir_es[ci])
        for start in range(0, n, CHUNK):
            _check(cancelled)
            count = min(CHUNK, n - start)
            keep = (membership.mask(start, count) if membership is not None
                    else np.ones(count, dtype=bool))
            status = db._status[es + start:es + start + count] == 1
            if waived is not None:
                keep &= status if waived else ~status
            ids = np.flatnonzero(keep) + start
            q = np.asarray(db._qbox[es + start:es + start + count])[keep]
            codes = _codes(q, self.depth)
            _add(leaves, codes, q, ids)
            wcounts[self.leaf_start:] += np.bincount(
                codes[status[keep]], minlength=len(leaves)).astype(np.uint64)
        _parents(nodes, self.depth)
        for level in range(self.depth - 1, -1, -1):
            a, b, size = (4 ** level - 1) // 3, (4 ** (level + 1) - 1) // 3, 4 ** level
            wcounts[a:a + size] = wcounts[b:b + size * 4].reshape(size, 4).sum(axis=1)
        _check(cancelled)
        self._summary_key, self._summary_value = key, (nodes, wcounts)
        return self._summary_value

    def leaf_ids(self, node):
        leaf = node - self.leaf_start
        return self.ids[int(self.offsets[leaf]):int(self.offsets[leaf + 1])]


def prepare_rule(db, ci, cancelled=None, progress=None):
    """Open or atomically build a rule's persistent spatial index."""
    ci = int(ci)
    identity = pack_identity(db.path)
    expected = getattr(db, '_analysis_identity', identity)
    if identity != expected:
        raise ValueError('DRC pack changed; reload the results database')
    path = rule_path(db, ci)
    try:
        return SpatialRule(path, db, ci)
    except (OSError, ValueError, KeyError, TypeError, EOFError):
        pass
    _check(cancelled)
    parent = os.path.dirname(path)
    os.makedirs(parent, exist_ok=True)
    temp = tempfile.mkdtemp(prefix='.spatial-build-', dir=parent)
    try:
        n = len(db.checks[ci].errors)
        depth = _depth(n)
        nodes = _empty_nodes(depth)
        leaf_start = (4 ** depth - 1) // 3
        leaves = nodes[leaf_start:]
        es = int(db._dir_es[ci])
        for start in range(0, n, CHUNK):
            _check(cancelled)
            count = min(CHUNK, n - start)
            q = np.asarray(db._qbox[es + start:es + start + count])
            _add(leaves, _codes(q, depth), q, np.arange(start, start + count, dtype=np.uint64))
            if progress is not None:
                progress(start + count, 2 * n)
        _parents(nodes, depth)
        offsets = np.r_[np.uint64(0), np.cumsum(leaves['count'], dtype=np.uint64)]
        ids = np.lib.format.open_memmap(os.path.join(temp, 'ids.npy'), mode='w+',
                                       dtype='<u4' if n <= 2 ** 32 else '<u8', shape=(n,))
        cursor = offsets[:-1].copy()
        for start in range(0, n, CHUNK):
            _check(cancelled)
            count = min(CHUNK, n - start)
            q = np.asarray(db._qbox[es + start:es + start + count])
            codes = _codes(q, depth)
            order = np.argsort(codes, kind='stable')
            sorted_codes = codes[order]
            unique, first, counts = np.unique(sorted_codes, return_index=True, return_counts=True)
            positions = cursor[sorted_codes] + (np.arange(count) - np.repeat(first, counts))
            ids[positions.astype(np.int64)] = start + order
            cursor[unique] += counts.astype(np.uint64)
            if progress is not None:
                progress(n + start + count, 2 * n)
        ids.flush()
        del ids
        np.save(os.path.join(temp, 'nodes.npy'), nodes, allow_pickle=False)
        np.save(os.path.join(temp, 'offsets.npy'), offsets, allow_pickle=False)
        if pack_identity(db.path) != identity:
            raise ValueError('DRC pack changed during spatial preparation; reload the database')
        with open(os.path.join(temp, 'meta.json'), 'w') as stream:
            json.dump(dict(version=VERSION, identity=identity, ci=ci,
                           n=n, depth=depth), stream)
        _check(cancelled)
        try:
            os.rename(temp, path)
        except OSError:
            # A concurrent successful builder wins. A damaged old cache is
            # replaced only after validating our completed replacement.
            try:
                return SpatialRule(path, db, ci)
            except (OSError, ValueError, KeyError, TypeError, EOFError):
                damaged = path + '.invalid-' + os.path.basename(temp)
                if os.path.exists(path):
                    os.rename(path, damaged)
                os.rename(temp, path)
                shutil.rmtree(damaged, ignore_errors=True)
        return SpatialRule(path, db, ci)
    finally:
        shutil.rmtree(temp, ignore_errors=True)
