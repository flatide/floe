"""Compact, user-authored grouping of rule-local DRC error numbers.

    [M1.SPACE.1]
    cluster 1 = 1-1000000, 1000010
    cluster 2 = 1000001-1000009

Numbers in the file are inclusive and one-based, like the viewer grid.
The implementation keeps intervals, never one Python object per member.
Repeated labels append intervals; unassigned errors form ``Unclustered``.
"""

import bisect
import heapq
import os
import re
from array import array

import numpy as np


UNCLUSTERED = "Unclustered"
_STATUS_CHUNK = 1 << 20
_MEMBER = re.compile(r"([0-9]+)(?:\s*-\s*([0-9]+))?")


class Cluster:
    """Disjoint intervals with bounded paging and optional status filtering."""

    def __init__(self, name, db, ci, starts, stops):
        self.name = name
        self._db = db
        self._ci = ci
        self._starts = np.asarray(starts, dtype=np.int64)
        self._stops = np.asarray(stops, dtype=np.int64)
        self._prefix = np.cumsum(self._stops - self._starts,
                                 dtype=np.int64)
        self.total = int(self._prefix[-1]) if len(self._prefix) else 0
        self._chunks = None
        self._waived = None
        self._wprefix = None
        self._nprefix = None
        self._uniform = None  # None = unknown; -1 = mixed; 0/1 = uniform

    def contains(self, ei):
        ei = int(ei)
        k = bisect.bisect_right(self._starts, ei) - 1
        return k >= 0 and ei < int(self._stops[k])

    def mask(self, start, count):
        """Membership for one bounded rule-local interval (spatial query)."""
        count = max(0, int(count))
        start = int(start)
        out = np.zeros(count, dtype=bool)
        if not count:
            return out
        lo = bisect.bisect_right(self._stops, start)
        hi = bisect.bisect_left(self._starts, start + count)
        if hi <= lo:
            return out
        if hi - lo <= 16:
            for k in range(lo, hi):
                a = max(start, int(self._starts[k])) - start
                b = min(start + count, int(self._stops[k])) - start
                out[a:b] = True
            return out
        # A difference array handles even millions of scattered ranges
        # without a Python loop per member or a full-rule bitmap.
        delta = np.zeros(count + 1, dtype=np.int8)
        a = np.maximum(self._starts[lo:hi] - start, 0)
        b = np.minimum(self._stops[lo:hi] - start, count)
        delta[a] = 1
        delta[b] = -1
        return np.cumsum(delta[:-1], dtype=np.int32) > 0

    def _status_slice(self, start, stop):
        db = self._db
        if hasattr(db, "_status") and hasattr(db, "_dir_es"):
            base = int(db._dir_es[self._ci])
            return db._status[base + start:base + stop] == 1
        if hasattr(db, "get_status"):
            return np.fromiter((db.get_status(self._ci, ei) == 1
                                for ei in range(start, stop)),
                               dtype=bool, count=stop - start)
        return np.zeros(stop - start, dtype=bool)

    def _has_status(self):
        return hasattr(self._db, "_status") or hasattr(self._db,
                                                      "get_status")

    def _uniform_status(self):
        """Use the pack's O(1) rule count before building member caches."""
        if self._uniform is None:
            if not self._has_status():
                self._uniform = 0
            elif hasattr(self._db, "status_counts"):
                waived, total = self._db.status_counts(self._ci)
                self._uniform = (0 if waived == 0 else
                                 1 if waived == total else -1)
            else:
                self._uniform = -1
        return None if self._uniform < 0 else bool(self._uniform)

    def _ensure_chunks(self):
        # Split long runs into bounded reads. Memory follows interval
        # count + one entry per million contiguous members, not members.
        if self._chunks is None:
            starts, stops = array("q"), array("q")
            for a, b in zip(self._starts, self._stops):
                a, b = int(a), int(b)
                while a < b:
                    end = min(b, ((a // _STATUS_CHUNK) + 1)
                              * _STATUS_CHUNK)
                    starts.append(a)
                    stops.append(end)
                    a = end
            self._chunks = (np.asarray(starts, dtype=np.int64),
                            np.asarray(stops, dtype=np.int64))

    def _ensure_status(self):
        if self._waived is not None:
            return
        self._ensure_chunks()
        starts, stops = self._chunks
        self._waived = np.zeros(len(starts), dtype=np.int64)
        _refresh_cluster_counts([(self, None)])

    def _refresh_chunks(self, indices):
        _refresh_cluster_counts([(self, indices)])

    def _rebuild_prefixes(self):
        starts, stops = self._chunks
        self._wprefix = np.cumsum(self._waived, dtype=np.int64)
        self._nprefix = np.cumsum(stops - starts - self._waived,
                                 dtype=np.int64)

    def _status_prefix(self, waived):
        self._ensure_status()
        return self._wprefix if waived else self._nprefix

    def count(self, waived=None):
        if waived is None:
            return self.total
        uniform = self._uniform_status()
        if uniform is not None:
            return self.total if bool(waived) == uniform else 0
        self._ensure_status()
        n = int(self._wprefix[-1]) if len(self._wprefix) else 0
        return n if waived else self.total - n

    def status_counts(self):
        return self.count(True), self.total

    def page(self, start, limit, waived=None):
        """Return only the requested page of zero-based local indices."""
        start, limit = max(0, int(start)), max(0, int(limit))
        if not limit:
            return []
        if waived is not None:
            uniform = self._uniform_status()
            if uniform is not None:
                return (self.page(start, limit)
                        if bool(waived) == uniform else [])
        if waived is None:
            k = bisect.bisect_right(self._prefix, start)
            before = int(self._prefix[k - 1]) if k else 0
            out = []
            while k < len(self._starts) and len(out) < limit:
                a = int(self._starts[k]) + max(0, start - before)
                b = min(int(self._stops[k]), a + limit - len(out))
                out.extend(range(a, b))
                before = int(self._prefix[k])
                k += 1
            return out
        prefix = self._status_prefix(waived)
        k = bisect.bisect_right(prefix, start)
        before = int(prefix[k - 1]) if k else 0
        starts, stops = self._chunks
        out = []
        while k < len(starts) and len(out) < limit:
            if int(prefix[k]) > before:
                a, b = int(starts[k]), int(stops[k])
                mask = self._status_slice(a, b)
                if not waived:
                    mask = ~mask
                ids = np.flatnonzero(mask)
                skip = max(0, start - before)
                out.extend(int(v) + a for v in
                           ids[skip:skip + limit - len(out)])
            before = int(prefix[k])
            # Zero-count runs can number in the millions for scattered
            # memberships. Jump directly to the next contributing run.
            k = bisect.bisect_right(prefix, before, lo=k + 1)
        return out

    def rank(self, ei, waived=None):
        """Matching-member rank of ei, or None if ei is not included."""
        ei = int(ei)
        k = bisect.bisect_right(self._starts, ei) - 1
        if k < 0 or ei >= int(self._stops[k]):
            return None
        if waived is not None:
            uniform = self._uniform_status()
            if uniform is not None:
                return self.rank(ei) if bool(waived) == uniform else None
        if waived is None:
            before = int(self._prefix[k - 1]) if k else 0
            return before + ei - int(self._starts[k])
        if bool(self._status_slice(ei, ei + 1)[0]) != bool(waived):
            return None
        prefix = self._status_prefix(waived)
        starts, _stops = self._chunks
        k = bisect.bisect_right(starts, ei) - 1
        before = int(prefix[k - 1]) if k else 0
        a = int(starts[k])
        n = int(np.count_nonzero(self._status_slice(a, ei)))
        return before + (n if waived else ei - a - n)

    def _changed_chunks(self, eis):
        starts, stops = self._chunks
        touched = set()
        for ei in eis:
            k = bisect.bisect_right(starts, int(ei)) - 1
            if k >= 0 and int(ei) < int(stops[k]):
                touched.add(k)
        return np.asarray(sorted(touched), dtype=np.int64)

    def _reset_status(self):
        self._waived = self._wprefix = self._nprefix = None
        self._uniform = None


def _refresh_cluster_counts(requests):
    """Share one bounded status prefix across a rule's affected clusters.

    Each request is (cluster, sorted chunk indices), or None for all of
    that cluster's chunks. The heap visits only physical blocks containing
    requested ranges. Even interleaved clusters read each status block once.
    """
    entries, heap = [], []
    for cluster, indices in requests:
        starts, stops = cluster._chunks
        if indices is not None:
            starts, stops = starts[indices], stops[indices]
        i = len(entries)
        entries.append((cluster, indices, starts, stops))
        if len(starts):
            heapq.heappush(heap, (int(starts[0]) // _STATUS_CHUNK, i, 0))
    while heap:
        block = heap[0][0]
        spans = []
        while heap and heap[0][0] == block:
            _block, i, lo = heapq.heappop(heap)
            cluster, indices, starts, stops = entries[i]
            hi = bisect.bisect_left(starts, (block + 1) * _STATUS_CHUNK,
                                   lo=lo + 1)
            spans.append((i, lo, hi))
            if hi < len(starts):
                heapq.heappush(heap, (int(starts[hi]) // _STATUS_CHUNK,
                                     i, hi))
        a = min(int(entries[i][2][lo]) for i, lo, _hi in spans)
        b = max(int(entries[i][3][hi - 1]) for i, _lo, hi in spans)
        cluster = entries[spans[0][0]][0]
        prefix = np.r_[np.int32(0), np.cumsum(
            cluster._status_slice(a, b), dtype=np.int32)]
        for i, lo, hi in spans:
            cluster, indices, starts, stops = entries[i]
            dest = slice(lo, hi) if indices is None else indices[lo:hi]
            cluster._waived[dest] = (prefix[stops[lo:hi] - a]
                                     - prefix[starts[lo:hi] - a])
    for cluster, _indices, _starts, _stops in entries:
        cluster._rebuild_prefixes()


class ClusterFile:
    def __init__(self, path, rules):
        self.path = os.path.abspath(os.fspath(path))
        self.rules = rules

    def prepare_counts(self, ci):
        """Prepare all mixed-status cluster counts with one rule scan.

        Call before filling a rule's tree children. Uniform rule counts are
        O(1); mixed rules share each physical status chunk's prefix across
        clusters instead of scanning the same bytes once per cluster.
        """
        pending = []
        for cluster in self.rules.get(ci, ()):
            if cluster._waived is not None \
                    or cluster._uniform_status() is not None:
                continue
            cluster._ensure_chunks()
            cluster._waived = np.zeros(len(cluster._chunks[0]),
                                       dtype=np.int64)
            pending.append((cluster, None))
        try:
            _refresh_cluster_counts(pending)
        except Exception:
            for cluster, _indices in pending:
                cluster._reset_status()
            raise

    def status_changed(self, ci, eis):
        # GUI supplies the already bounded explicit selection. Preserve a
        # generator once so each cluster receives every changed index.
        if not hasattr(eis, "__len__"):
            eis = tuple(eis)
        pending = []
        for cluster in self.rules.get(ci, ()):
            cluster._uniform = None
            if cluster._waived is not None:
                indices = cluster._changed_chunks(eis)
                if len(indices):
                    pending.append((cluster, indices))
        _refresh_cluster_counts(pending)

    def reset_status(self):
        for clusters in self.rules.values():
            for cluster in clusters:
                cluster._reset_status()


def load_clusters(path, db):
    """Validate a text sidecar completely before returning any grouping.

    Blank lines and whole-line # comments are allowed. A repeated rule
    section or cluster label appends ranges, permitting bounded lines in
    machine-generated files. Errors identify the source filename + line.
    """
    path = os.fspath(path)
    by_name = {}
    for ci, check in enumerate(db.checks):
        by_name.setdefault(check.name, []).append(ci)

    def fail(line, message):
        raise ValueError("%s:%d: %s" % (path, line, message))

    pending = {}
    ci = None
    with open(path, "r", encoding="utf-8-sig") as stream:
        for lineno, raw in enumerate(stream, 1):
            line = raw.strip()
            if not line or line.startswith("#"):
                continue
            if line.startswith("[") and line.endswith("]"):
                name = line[1:-1].strip()
                matches = by_name.get(name, ())
                if not matches:
                    fail(lineno, "unknown rule %r" % name)
                if len(matches) != 1:
                    fail(lineno, "ambiguous duplicate rule name %r" % name)
                ci = matches[0]
                pending.setdefault(ci, {})
                continue
            if ci is None:
                fail(lineno, "cluster needs a [rule name] section")
            label, sep, body = line.partition("=")
            label = label.strip()
            if not sep or not label or not body.strip():
                fail(lineno, "expected cluster name = 1-10, 20")
            if label == UNCLUSTERED:
                fail(lineno, "%r is reserved for unassigned errors" % label)
            ranges = pending[ci].setdefault(label, array("q"))
            end = 0
            count = 0
            n = len(db.checks[ci].errors)
            for match in _MEMBER.finditer(body):
                if body[end:match.start()].strip(" ,\t"):
                    fail(lineno, "invalid member range near %r" %
                         body[end:match.start()])
                try:
                    a = int(match.group(1))
                    b = int(match.group(2) or match.group(1))
                except ValueError:
                    fail(lineno, "invalid error number")
                if a < 1 or b < a or b > n:
                    fail(lineno, "range %d-%d outside rule errors 1-%d"
                         % (a, b, n))
                ranges.extend((a - 1, b, lineno))
                end = match.end()
                count += 1
            if not count or body[end:].strip(" ,\t"):
                fail(lineno, "invalid member range %r" % body[end:])

    rules = {}
    for ci, labels in pending.items():
        if not labels:
            continue
        chunks, owners = [], []
        names = list(labels)
        for owner, label in enumerate(names):
            raw = np.asarray(labels[label], dtype=np.int64).reshape(-1, 3)
            chunks.append(raw)
            owners.append(np.full(len(raw), owner, dtype=np.int64))
        all_ranges = np.concatenate(chunks)
        all_owners = np.concatenate(owners)
        order = np.argsort(all_ranges[:, 0], kind="stable")
        all_ranges, all_owners = all_ranges[order], all_owners[order]
        overlap = np.flatnonzero(all_ranges[1:, 0] < all_ranges[:-1, 1])
        if len(overlap):
            k = int(overlap[0]) + 1
            fail(int(all_ranges[k, 2]),
                 "overlapping memberships in %r and %r"
                 % (names[int(all_owners[k - 1])],
                    names[int(all_owners[k])]))
        # Group once, keeping each owner's file-order sort, instead of
        # scanning every interval again for every cluster.
        group_order = np.argsort(all_owners, kind="stable")
        grouped = all_ranges[group_order]
        group_ends = np.cumsum(np.bincount(all_owners,
                                          minlength=len(names)))
        groups = []
        for owner, label in enumerate(names):
            begin = int(group_ends[owner - 1]) if owner else 0
            rows = grouped[begin:int(group_ends[owner])]
            # Merge adjacent runs so later mask/status work stays compact.
            first = np.r_[True, rows[1:, 0] != rows[:-1, 1]]
            last = np.r_[first[1:], True]
            groups.append(Cluster(label, db, ci,
                                  rows[first, 0], rows[last, 1]))
        n = len(db.checks[ci].errors)
        gap_starts = np.r_[np.int64(0), all_ranges[:, 1]]
        gap_stops = np.r_[all_ranges[:, 0], np.int64(n)]
        gaps = gap_starts < gap_stops
        if np.any(gaps):
            groups.append(Cluster(UNCLUSTERED, db, ci,
                                  gap_starts[gaps], gap_stops[gaps]))
        rules[ci] = groups
    return ClusterFile(path, rules)
