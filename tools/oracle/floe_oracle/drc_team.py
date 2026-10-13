"""Immutable reviewer snapshots, bounded comparisons and explicit application.

Source files are read-only. Sparse 4,096-error bit planes retain visited,
waived and not-required independently; no geometry or per-error Python
collection is needed. All builders and application run off the GUI thread.
"""

from collections import OrderedDict, namedtuple
from contextlib import contextmanager
import json
import heapq
import os
from pathlib import Path
import sqlite3
import tempfile
import threading
import time
import uuid

import numpy as np

from . import cachepath
from .drc import STATUS_WAIVED, STATUS_NONE, _WAIVE_HEADER
from .drc_review import CHUNK, NOT_REQUIRED, ReviewIdentityError


ReviewState = namedtuple("ReviewState", "visited waived not_required")
TeamState = namedtuple("TeamState", "visited waived not_required conflict")
_BYTES = CHUNK // 8
_KINDS = {"visited": "v", "waived": "w", "not_required": "n",
          "conflict": "c", "conflicts": "c", "work": "a"}
_TYPE = "floe-review-snapshot"
_VERSION = 1


def _check(cancelled):
    if cancelled is not None and cancelled():
        raise InterruptedError("review operation cancelled; completed application chunks remain applied")


class _Progress:
    def __init__(self, callback, total):
        self.callback, self.total = callback, int(total)
        self.last = time.monotonic()

    def __call__(self, done, final=False):
        now = time.monotonic()
        if self.callback is not None and (final or now - self.last >= 0.5):
            self.last = now
            self.callback(int(done), self.total)


class _SnapshotFiles:
    """Retain owned snapshot files without retaining their GUI SQL handle."""
    def __init__(self, owner=None):
        self.owner = owner
        self.references = 1
        self.lock = threading.Lock()

    def acquire(self):
        with self.lock:
            if not self.references:
                raise OSError("review snapshot is closed")
            self.references += 1

    def release(self):
        owner = None
        with self.lock:
            self.references -= 1
            if self.references == 0:
                owner, self.owner = self.owner, None
        if owner is not None:
            owner.cleanup()


class ReviewLease:
    """Cheap immutable metadata/file lease; opens no SQLite connection.

    Acquire on the GUI thread before queueing a task, and close in the
    task's finally block. A worker opens its own connection, so closing the
    original window never waits for that worker's SQLite query lock.
    """
    def __init__(self, source):
        self._files = source._files
        self._files.acquire()
        self._closed = False
        self._state_lock = threading.Lock()
        self.path = source.path
        self.metadata = source.metadata
        self.sizes = source.sizes
        self.snapshot_id = source.snapshot_id
        self._counts = source._counts
        self._totals = source._totals

    def _rule(self, ci):
        ci = int(ci)
        if not 0 <= ci < len(self.sizes):
            raise IndexError(ci)
        return ci, int(self.sizes[ci])

    def counts(self, ci=None):
        if self._closed:
            raise OSError("review snapshot lease is closed")
        if ci is None:
            return dict(self._totals)
        return dict(self._counts[self._rule(ci)[0]])

    def lease(self):
        with self._state_lock:
            if self._closed:
                raise OSError("review snapshot lease is closed")
            return ReviewLease(self)

    def close(self):
        with self._state_lock:
            release = not self._closed
            self._closed = True
        if release:
            self._files.release()

    def __enter__(self):
        return self

    def __exit__(self, *_exc):
        self.close()

    def __del__(self):
        try:
            self.close()
        except Exception:
            pass


def _sizes(db):
    return tuple(int(n) for n in db._ecnt)


def _reviewer(db):
    value = getattr(db, "_reviewer", None)
    if value:
        return str(value)
    name = os.path.basename(os.fspath(getattr(db, "_waive_path", "")))
    if ".waive." in name:
        return name.rsplit(".waive.", 1)[1]
    from .drc import _waive_user
    return _waive_user()


def _file_reviewer(path):
    name = os.path.basename(os.fspath(path))
    if ".review." in name:
        return name.rsplit(".review.", 1)[1].removesuffix(".sqlite3")
    if ".waive." in name:
        return name.rsplit(".waive.", 1)[1]
    return name


def _connect(path, *, timeout=0.05):
    return sqlite3.connect(Path(os.path.abspath(path)).as_uri() + "?mode=ro",
                           uri=True, timeout=timeout, check_same_thread=False,
                           isolation_level=None)


def _identity(db, metadata):
    if (metadata.get("fingerprint") != db._review_fingerprint
            or tuple(metadata.get("sizes", ())) != _sizes(db)):
        raise ReviewIdentityError("review source belongs to another DRC result; source preserved")


def _empty_counts(total):
    return dict(total=int(total), visited=0, waived=0, not_required=0,
                conflicts=0, work=0, pending=int(total))


class ReviewMembers:
    """Immutable rule membership; pages seek a persisted prefix index."""
    def __init__(self, source, ci, kind="work"):
        self.source = source
        self.ci, self.size = source._rule(ci)
        if kind not in _KINDS:
            raise ValueError("unsupported review membership: %s" % kind)
        self.kind, self.col = kind, _KINDS[kind]
        key = "conflicts" if self.col == "c" else kind
        self.total = int(source.counts(ci)[key])

    def count(self, waived=None):
        if waived is not None:
            raise ValueError("review membership is already resolved")
        return self.total

    def __len__(self):
        return self.total

    def __bool__(self):
        return bool(self.total)

    def mask(self, start, count):
        start, count = int(start), max(0, int(count))
        out = np.zeros(count, dtype=bool)
        lo, hi = max(0, start), min(self.size, start + count)
        for k in range(lo // CHUNK, (hi + CHUNK - 1) // CHUNK):
            a, b = max(lo, k * CHUNK), min(hi, (k + 1) * CHUNK)
            if a < b:
                bits = self.source._chunk(self.ci, k)["vwnca".index(self.col)]
                values = np.unpackbits(np.frombuffer(bits.to_bytes(_BYTES, "little"), dtype=np.uint8), bitorder="little")
                out[a - start:b - start] = values[a - k * CHUNK:b - k * CHUNK]
        return out

    def contains(self, ei):
        ei = int(ei)
        if not 0 <= ei < self.size:
            return False
        bits = self.source._chunk(self.ci, ei // CHUNK)["vwnca".index(self.col)]
        return bool(bits & (1 << (ei % CHUNK)))

    __contains__ = contains

    def contains_many(self, indices):
        values = np.asarray(indices, dtype=np.int64)
        result = np.zeros(values.shape, dtype=bool)
        for k in np.unique(values[(values >= 0) & (values < self.size)] // CHUNK):
            selected = (values >= int(k) * CHUNK) & (values < (int(k) + 1) * CHUNK)
            bits = self.source._chunk(self.ci, int(k))["vwnca".index(self.col)]
            packed = np.frombuffer(bits.to_bytes(_BYTES, "little"), dtype=np.uint8)
            local = values[selected] - int(k) * CHUNK
            result[selected] = ((packed[local // 8] >> (local % 8)) & 1).astype(bool)
        return result

    def page(self, start, limit, waived=None):
        if waived is not None:
            raise ValueError("review membership is already resolved")
        start, limit = max(0, int(start)), max(0, int(limit))
        if not limit or start >= self.total:
            return []
        col = self.col
        out = []
        with self.source._lock:
            cursor = self.source._conn.execute(
                "SELECT k,%s,%sc,%se FROM chunks WHERE ci=? AND %sc>0 AND %se>? ORDER BY %se,k" %
                (col, col, col, col, col, col), (self.ci, start))
            while len(out) < limit:
                row = cursor.fetchone()
                if row is None:
                    break
                k, blob, count, end = row
                bits = np.unpackbits(np.frombuffer(blob, dtype=np.uint8), bitorder="little")
                ids = np.flatnonzero(bits)
                skip = max(0, start - (int(end) - int(count)))
                out.extend((ids[skip:skip + limit - len(out)] + int(k) * CHUNK).tolist())
            cursor.close()
        return out

    def rank(self, ei, waived=None):
        if waived is not None:
            raise ValueError("review membership is already resolved")
        ei = int(ei)
        if not self.contains(ei):
            return None
        col = self.col
        with self.source._lock:
            blob, count, end = self.source._conn.execute(
                "SELECT %s,%sc,%se FROM chunks WHERE ci=? AND k=?" % (col, col, col),
                (self.ci, ei // CHUNK)).fetchone()
        bits = int.from_bytes(blob, "little")
        return int(end) - int(count) + (bits & ((1 << (ei % CHUNK)) - 1)).bit_count()

    def __getitem__(self, value):
        if isinstance(value, slice):
            start, stop, step = value.indices(self.total)
            if step != 1:
                raise ValueError("review membership supports contiguous slices")
            return self.page(start, max(0, stop - start))
        value = int(value)
        if value < 0:
            value += self.total
        result = self.page(value, 1)
        if not result or value < 0:
            raise IndexError(value)
        return result[0]


class ReviewSnapshot:
    def __init__(self, db, path, *, owner=None):
        self.path = os.fspath(path)
        self.original_path = self.path
        self._owner = owner
        self._files = _SnapshotFiles(owner)
        self._files_closed = False
        self._lock = threading.RLock()
        self._cache = OrderedDict()
        self._conn = None
        self._conn = _connect(self.path)
        try:
            row = self._conn.execute("SELECT value FROM metadata WHERE id=1").fetchone()
            self.metadata = json.loads(row[0])
            if self.metadata.get("type") != _TYPE or self.metadata.get("version") != _VERSION:
                raise ValueError("unsupported reviewer snapshot")
            _identity(db, self.metadata)
            self.fingerprint = str(self.metadata["fingerprint"])
            self.sizes = tuple(self.metadata["sizes"])
            self.reviewer = str(self.metadata["reviewer"])
            self.snapshot_id = str(self.metadata["snapshot_id"])
            self.exported_ns = int(self.metadata["exported_ns"])
            self.visited_known = bool(self.metadata.get("visited_known", True))
            self.waive_known = bool(self.metadata.get("waive_known", True))
            self.not_required_known = bool(self.metadata.get("not_required_known", True))
            self._counts = {}
            totals = _empty_counts(sum(self.sizes))
            for row in self._conn.execute("SELECT ci,total,v,w,n,c,a,done FROM rules"):
                ci, total, v, w, n, c, a, done = map(int, row)
                if not 0 <= ci < len(self.sizes) or total != self.sizes[ci]:
                    raise ValueError("invalid snapshot rule counters")
                counts = dict(total=total, visited=v, waived=w, not_required=n,
                              conflicts=c, work=a, pending=total-done)
                if any(x < 0 or x > total for x in counts.values()):
                    raise ValueError("invalid snapshot counters")
                if (not max(v, n) <= done <= min(total, v+n)
                        or not max(v, w, n, c) <= a <= min(total, v+w+n+c)
                        or c > v):
                    raise ValueError("inconsistent snapshot counters")
                self._counts[ci] = counts
                for key in ("visited", "waived", "not_required", "conflicts", "work"):
                    totals[key] += counts[key]
                totals["pending"] -= done
            if len(self._counts) != len(self.sizes):
                raise ValueError("incomplete reviewer snapshot")
            # These small prefix indexes cover all stored nonzero planes;
            # validate their final ranks without reading the bitmap payloads.
            for col, key in (("v", "visited"), ("w", "waived"),
                             ("n", "not_required"), ("c", "conflicts"), ("a", "work")):
                ends = {int(ci): int(end) for ci, end in self._conn.execute(
                    "SELECT ci,MAX(%se) FROM chunks WHERE %sc>0 GROUP BY ci" % (col, col))}
                if any(ci not in self._counts for ci in ends) or any(
                        ends.get(ci, 0) != row[key] for ci, row in self._counts.items()):
                    raise ValueError("snapshot counters do not match their paging indexes")
            self._totals = totals
        except Exception:
            self._conn.close()
            self._conn = None
            raise

    def _rule(self, ci):
        ci = int(ci)
        if not 0 <= ci < len(self.sizes):
            raise IndexError(ci)
        return ci, int(self.sizes[ci])

    def _chunk(self, ci, k):
        key = (int(ci), int(k))
        with self._lock:
            if self._conn is None:
                raise OSError("review snapshot is closed")
            if key in self._cache:
                self._cache.move_to_end(key)
                return self._cache[key]
            row = self._conn.execute("SELECT v,w,n,c,a FROM chunks WHERE ci=? AND k=?", key).fetchone()
            if row is not None and any(len(x) not in (0, _BYTES) for x in row):
                raise ValueError("corrupt reviewer bitmap")
            result = (0, 0, 0, 0, 0) if row is None else tuple(int.from_bytes(x, "little") for x in row)
            self._cache[key] = result
            if len(self._cache) > 128:
                self._cache.popitem(last=False)
            return result

    def get(self, ci, ei):
        ci, size = self._rule(ci)
        ei = int(ei)
        if not 0 <= ei < size:
            raise IndexError(ei)
        v, w, n, _c, _a = self._chunk(ci, ei // CHUNK)
        bit = 1 << (ei % CHUNK)
        return ReviewState(bool(v & bit), bool(w & bit), bool(n & bit))

    def counts(self, ci=None):
        if ci is None:
            return dict(self._totals)
        ci, _size = self._rule(ci)
        return dict(self._counts[ci])

    def members(self, ci, kind="work"):
        return ReviewMembers(self, ci, kind)

    def page(self, ci, start, limit, kind="work"):
        return self.members(ci, kind).page(start, limit)

    def iter_chunks(self, ci=None):
        # A separate connection permits a background stream without locking
        # out the bounded GUI paging connection for the duration of a build.
        conn = _connect(self.path, timeout=10.0)
        try:
            query = "SELECT ci,k,v,w,n,c,a FROM chunks"
            args = ()
            if ci is not None:
                query += " WHERE ci=?"
                args = (self._rule(ci)[0],)
            for row in conn.execute(query + " ORDER BY ci,k", args):
                if any(len(x) not in (0, _BYTES) for x in row[2:]):
                    raise ValueError("corrupt reviewer bitmap")
                yield (int(row[0]), int(row[1]), *(int.from_bytes(x, "little") for x in row[2:]))
        finally:
            conn.close()

    def lease(self):
        # No SQL, file validation, or GUI reader lock is acquired here.
        if self._files_closed:
            raise OSError("review snapshot is closed")
        return ReviewLease(self)

    def close(self):
        with self._lock:
            if self._conn is not None:
                self._conn.close()
                self._conn = None
            self._cache.clear()
            release = not self._files_closed
            self._files_closed = True
            self._owner = None
        if release:
            self._files.release()

    def __del__(self):
        try:
            self.close()
        except Exception:
            pass


class ReviewAggregate(ReviewSnapshot):
    def get(self, ci, ei):
        state = super().get(ci, ei)
        conflict = bool(self._chunk(ci, int(ei) // CHUNK)[3] & (1 << (int(ei) % CHUNK)))
        return TeamState(*state, conflict)

    def counts(self, ci=None):
        counts = super().counts(ci)
        counts["reviewers"] = len(self.metadata.get("sources", ()))
        return counts

    def close(self):
        own, self.own = getattr(self, "own", None), None
        super().close()
        if own is not None:
            own.close()


class _Writer:
    def __init__(self, db, path, reviewer, *, metadata=None):
        self.path = os.path.abspath(os.fspath(path))
        os.makedirs(os.path.dirname(self.path), exist_ok=True)
        fd, self.temporary = tempfile.mkstemp(prefix=".review-snapshot-", suffix=".sqlite3", dir=os.path.dirname(self.path))
        os.close(fd)
        self.sizes = _sizes(db)
        self.metadata = dict(type=_TYPE, version=_VERSION, fingerprint=db._review_fingerprint,
                             sizes=self.sizes, reviewer=str(reviewer), snapshot_id=uuid.uuid4().hex,
                             exported_ns=time.time_ns(), visited_known=True,
                             waive_known=True, not_required_known=True)
        if metadata:
            self.metadata.update(metadata)
        self.conn = None
        try:
            self.conn = sqlite3.connect(self.temporary, isolation_level=None)
            self.conn.execute("PRAGMA synchronous=FULL")
            self.conn.execute("BEGIN")
            self.conn.execute("CREATE TABLE metadata(id INTEGER PRIMARY KEY,value TEXT NOT NULL)")
            self.conn.execute("CREATE TABLE rules(ci INTEGER PRIMARY KEY,total INTEGER,v INTEGER,w INTEGER,n INTEGER,c INTEGER,a INTEGER,done INTEGER)")
            self.conn.execute("CREATE TABLE chunks(ci INTEGER,k INTEGER,v BLOB,w BLOB,n BLOB,c BLOB,a BLOB,vc INTEGER,wc INTEGER,nc INTEGER,cc INTEGER,ac INTEGER,ve INTEGER,we INTEGER,ne INTEGER,ce INTEGER,ae INTEGER,PRIMARY KEY(ci,k)) WITHOUT ROWID")
            for col in "vwnca":
                self.conn.execute("CREATE INDEX page_%s ON chunks(ci,%se,k) WHERE %sc>0" % (col, col, col))
        except Exception:
            self.close()
            raise
        self.counters = {}

    def add(self, ci, k, v, w, n, conflict=0, work=None):
        if not 0 <= ci < len(self.sizes) or k < 0:
            raise ValueError("snapshot chunk is outside its rule")
        limit = min(CHUNK, self.sizes[ci] - k * CHUNK)
        if limit <= 0:
            raise ValueError("snapshot chunk is outside its rule")
        valid = (1 << limit) - 1
        values = (v, w, n, conflict, (v | w | n) if work is None else work)
        if any(x < 0 or x & ~valid for x in values):
            raise ValueError("snapshot chunk has out-of-range bits")
        if not any(values):
            return
        counts = tuple(x.bit_count() for x in values)
        counters = self.counters.setdefault(ci, [0] * 6)
        for i, count in enumerate(counts):
            counters[i] += count
        counters[5] += (v | n).bit_count()
        blobs = tuple(x.to_bytes(_BYTES, "little") if x else b"" for x in values)
        self.conn.execute("INSERT INTO chunks VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                          (ci, k) + blobs + counts + tuple(counters[:5]))

    def finish(self):
        for ci, size in enumerate(self.sizes):
            self.conn.execute("INSERT INTO rules VALUES(?,?,?,?,?,?,?,?)",
                              (ci, size, *self.counters.get(ci, [0] * 6)))
        self.conn.execute("INSERT INTO metadata VALUES(1,?)", (json.dumps(self.metadata, separators=(",", ":")),))
        self.conn.execute("COMMIT")
        self.conn.close()
        self.conn = None
        with open(self.temporary, "rb") as stream:
            os.fsync(stream.fileno())
        os.replace(self.temporary, self.path)
        self.temporary = None

    def close(self):
        if self.conn is not None:
            self.conn.close()
            self.conn = None
        if self.temporary is not None:
            try:
                os.unlink(self.temporary)
            except FileNotFoundError:
                pass
            self.temporary = None


@contextmanager
def _destination(db, path, reviewer, metadata=None):
    prohibited = [os.fspath(db.path), getattr(db, "_waive_path", None)]
    store = getattr(db, "_review_store", None)
    if store is not None:
        prohibited.append(store.path)
    target = os.path.realpath(path)
    if any(value and os.path.realpath(value) == target for value in prohibited):
        raise ValueError("a snapshot cannot replace the DRC pack or working review files")
    writer = _Writer(db, path, reviewer, metadata=metadata)
    try:
        yield writer
        writer.finish()
    finally:
        writer.close()


def export_review(db, path, *, reviewer=None, cancelled=None, progress=None):
    """Export own W/NR/V together, in bounded chunks, to an atomic snapshot."""
    reviewer = _reviewer(db) if reviewer is None else str(reviewer)
    sizes = _sizes(db)
    report = _Progress(progress, sum(sizes))
    store = db._review_flags_store()
    processed = 0
    _check(cancelled)
    with db._status_lock, store._write_lock:
        waive_path = getattr(db, "_waive_path", None)
        waive_stamp = _stamp(waive_path) if waive_path is not None else None
        with store._lock:
            existing = store._connect(probe=True) is not None
        flags = _connect(store.path, timeout=10.0) if existing else None
        try:
            if flags is not None:
                flags.execute("BEGIN")
            with _destination(db, path, reviewer) as writer:
                for ci, size in enumerate(sizes):
                    rows = iter(flags.execute("SELECT chunk,visited,nr FROM chunks WHERE ci=? ORDER BY chunk", (ci,))) if flags is not None else iter(())
                    row = next(rows, None)
                    es = int(db._dir_es[ci])
                    # Existing O(1) own counters let billion-error, zero-
                    # waived rules stream just the sparse review chunks.
                    zero_waived = (hasattr(db, "status_counts")
                                   and db.status_counts(ci)[0] == 0)
                    if zero_waived:
                        while row is not None:
                            _check(cancelled)
                            k, vb, nb = row
                            if any(len(x) not in (0, _BYTES) for x in (vb, nb)):
                                raise ValueError("corrupt review flag chunk")
                            writer.add(ci, int(k), int.from_bytes(vb, "little"), 0,
                                       int.from_bytes(nb, "little"))
                            report(processed + min(size, (int(k) + 1) * CHUNK))
                            row = next(rows, None)
                        processed += size
                        report(processed)
                        continue
                    for start in range(0, size, CHUNK):
                        _check(cancelled)
                        k, count = start // CHUNK, min(CHUNK, size - start)
                        v = n = 0
                        if row is not None and int(row[0]) == k:
                            if any(len(x) not in (0, _BYTES) for x in row[1:]):
                                raise ValueError("corrupt review flag chunk")
                            v, n = (int.from_bytes(x, "little") for x in row[1:])
                            row = next(rows, None)
                        w = int.from_bytes(np.packbits(np.asarray(db._status[es + start:es + start + count]) == STATUS_WAIVED, bitorder="little").tobytes(), "little")
                        writer.add(ci, k, v, w, n)
                        processed += count
                        report(processed)
                _check(cancelled)
                if waive_stamp is not None and _stamp(waive_path) != waive_stamp:
                    raise OSError("own waive file changed in another session during export; retry")
        finally:
            if flags is not None:
                flags.close()
    report(processed, final=True)
    return ReviewSnapshot(db, path)


def _legacy_status(db, path):
    path = os.fspath(path)
    with open(path, "rb") as stream:
        header = stream.read(_WAIVE_HEADER.size)
        size = os.fstat(stream.fileno()).st_size
    total = sum(_sizes(db))
    if header != db._waive_hdr or size != _WAIVE_HEADER.size + total + 4 * len(_sizes(db)):
        raise ReviewIdentityError("waive source does not match this DRC result")
    return (np.memmap(path, mode="r", dtype=np.uint8, offset=_WAIVE_HEADER.size,
                      shape=(total,)) if total else np.empty(0, dtype=np.uint8))


def _stamp(path):
    stat = os.stat(path)
    return stat.st_size, stat.st_mtime_ns, stat.st_ctime_ns, stat.st_ino


def _paired_waive(path):
    name = os.path.basename(path)
    if ".review." not in name or not name.endswith(".sqlite3"):
        return None
    base, reviewer = name[:-8].rsplit(".review.", 1)
    candidate = os.path.join(os.path.dirname(path), base + ".waive." + reviewer)
    try:
        os.stat(candidate)
    except FileNotFoundError:
        return None
    return candidate


def open_review(db, path, *, waive_path=None, reviewer=None, cancelled=None,
                progress=None):
    """Read a snapshot, legacy waive, or V/NR SQLite sidecar without edits.

    Missing legacy dimensions remain unknown, rather than implicit false.
    A sidecar's matching waive filename is the only automatic companion.
    """
    path = os.path.abspath(os.fspath(path))
    _check(cancelled)
    with open(path, "rb") as stream:
        magic = stream.read(16)
    raw = None
    raw_metadata = None
    if magic == b"SQLite format 3\x00":
        raw = _connect(path, timeout=10.0)
        try:
            row = raw.execute("SELECT value FROM metadata WHERE id=1").fetchone()
            raw_metadata = json.loads(row[0])
            if raw_metadata.get("type") == _TYPE:
                raw.close()
                if raw_metadata.get("aggregate"):
                    raise ValueError("load individual reviewer snapshots, not a derived team result")
                return ReviewSnapshot(db, path)
            if raw_metadata.get("version") != 1:
                raise ValueError("unsupported working review database")
            _identity(db, raw_metadata)
            raw.execute("BEGIN")
        except Exception:
            raw.close()
            raise
    elif not magic.startswith(b"FLOEWAIV"):
        raise ValueError("not a reviewer snapshot, working review database, or waive file")
    if raw is None:
        waive_path = path
    elif waive_path is None:
        waive_path = _paired_waive(path)
    if waive_path is not None:
        waive_path = os.path.abspath(os.fspath(waive_path))
    status = None
    status_stamp = None
    owner = tempfile.TemporaryDirectory(prefix="floe-peer-")
    output = os.path.join(owner.name, "snapshot.sqlite3")
    sizes = _sizes(db)
    report = _Progress(progress, sum(sizes))
    try:
        if waive_path is not None:
            status_stamp = _stamp(waive_path)
            status = _legacy_status(db, waive_path)
        metadata = dict(visited_known=raw is not None,
                        not_required_known=raw is not None,
                        waive_known=status is not None,
                        source_path=path, source_waive_path=waive_path,
                        source_format="working-review" if raw else "legacy-waive")
        processed = 0
        with _destination(db, output, reviewer or _file_reviewer(path), metadata) as writer:
            for ci, size in enumerate(sizes):
                rows = iter(raw.execute("SELECT chunk,visited,nr FROM chunks WHERE ci=? ORDER BY chunk", (ci,))) if raw else iter(())
                row = next(rows, None)
                if status is None:
                    while row is not None:
                        _check(cancelled)
                        k, vb, nb = row
                        if any(len(x) not in (0, _BYTES) for x in (vb, nb)):
                            raise ValueError("corrupt working review bitmap")
                        writer.add(ci, int(k), int.from_bytes(vb, "little"), 0,
                                   int.from_bytes(nb, "little"))
                        report(processed + min(size, (int(k) + 1) * CHUNK))
                        row = next(rows, None)
                    processed += size
                    report(processed)
                    continue
                es = int(db._dir_es[ci])
                for start in range(0, size, CHUNK):
                    _check(cancelled)
                    k, count = start // CHUNK, min(CHUNK, size-start)
                    v = n = 0
                    if row is not None and int(row[0]) == k:
                        if any(len(x) not in (0, _BYTES) for x in row[1:]):
                            raise ValueError("corrupt working review bitmap")
                        v, n = (int.from_bytes(x, "little") for x in row[1:])
                        row = next(rows, None)
                    w = int.from_bytes(np.packbits(status[es+start:es+start+count] == STATUS_WAIVED,
                                                  bitorder="little").tobytes(), "little")
                    writer.add(ci, k, v, w, n)
                    processed += count
                    report(processed)
            _check(cancelled)
            if status is not None and _stamp(waive_path) != status_stamp:
                raise OSError("waive source changed while reading; load a saved snapshot or retry")
        report(processed, final=True)
        result = ReviewSnapshot(db, output, owner=owner)
        result.original_path = path
        return result
    except Exception:
        owner.cleanup()
        raise
    finally:
        if raw is not None:
            raw.close()


def _temporary_path(path, prefix):
    if path is not None:
        return os.fspath(path), None
    owner = tempfile.TemporaryDirectory(prefix=prefix)
    return os.path.join(owner.name, "snapshot.sqlite3"), owner


def aggregate_reviews(db, sources, *, include_own=True, path=None,
                      cancelled=None, progress=None):
    """Combine sparse chunks; conflicts require observed opposite decisions.

    Reviewers count once. Their newest exported snapshot wins; a current own
    snapshot overrides any older saved copy of the same reviewer.
    """
    _check(cancelled)
    chosen = {}
    for source in sources:
        if source.metadata.get("aggregate"):
            raise ValueError("a derived team result cannot vote as a reviewer")
        _identity(db, source.metadata)
        previous = chosen.get(source.reviewer)
        if previous is None or (source.exported_ns, source.snapshot_id) > (previous.exported_ns, previous.snapshot_id):
            chosen[source.reviewer] = source
    own = None
    sizes = _sizes(db)
    offsets = np.r_[np.int64(0), np.cumsum(sizes, dtype=np.int64)]
    total = int(offsets[-1])
    phase_base = total if include_own else 0
    report = _Progress(progress, total + phase_base)
    output, owner = _temporary_path(path, "floe-team-")
    try:
        if include_own:
            own_owner = tempfile.TemporaryDirectory(prefix="floe-own-review-")
            try:
                own = export_review(db, os.path.join(own_owner.name, "snapshot.sqlite3"), cancelled=cancelled,
                                    progress=lambda done, _total: report(done))
                own._owner = own_owner
                own._files.owner = own_owner
            except Exception:
                own_owner.cleanup()
                raise
            chosen[own.reviewer] = own
        selected = [chosen[key] for key in sorted(chosen)]
        if any(os.path.realpath(output) == os.path.realpath(source.path) for source in selected):
            raise ValueError("aggregation cannot replace a source snapshot")
        metadata = dict(sources=[dict(reviewer=s.reviewer, snapshot_id=s.snapshot_id) for s in selected],
                        aggregate=True)
        iterators = [iter(source.iter_chunks()) for source in selected]
        heap = []
        for index, rows in enumerate(iterators):
            row = next(rows, None)
            if row is not None:
                heapq.heappush(heap, (row[0], row[1], index, row))
        with _destination(db, output, "team", metadata) as writer:
            while heap:
                _check(cancelled)
                ci, k = heap[0][:2]
                v = w = n = work = wy = wn = ny = nn = 0
                while heap and heap[0][:2] == (ci, k):
                    _ci, _k, index, row = heapq.heappop(heap)
                    sv, sw, sn, _sc, sa = row[2:]
                    source = selected[index]
                    v |= sv
                    w |= sw
                    n |= sn
                    work |= sa
                    if source.visited_known:
                        if source.waive_known:
                            wy |= sv & sw
                            wn |= sv & ~sw
                        if source.not_required_known:
                            ny |= sv & sn
                            nn |= sv & ~sn
                    next_row = next(iterators[index], None)
                    if next_row is not None:
                        heapq.heappush(heap, (next_row[0], next_row[1], index, next_row))
                conflict = (wy & wn) | (ny & nn)
                writer.add(ci, k, v, w & ~conflict, n & ~conflict, conflict, work)
                report(phase_base + int(offsets[ci]) + min(sizes[ci], (k+1)*CHUNK))
            _check(cancelled)
        result = ReviewAggregate(db, output, owner=owner)
        result.sources, result.own = selected, own
        report(phase_base + total, final=True)
        return result
    except Exception:
        if own is not None:
            own.close()
        if owner is not None:
            owner.cleanup()
        raise
    finally:
        for rows in locals().get("iterators", ()):
            rows.close()


def _own_bits(db, store, ci, k):
    size = int(db._ecnt[ci])
    start, count = k * CHUNK, min(CHUNK, size - k * CHUNK)
    if count <= 0:
        raise ValueError("source chunk is outside its rule")
    with store._lock:
        if store._connect() is None:
            v = n = 0
        else:
            v, n = store._read_chunk(ci, k, fresh=True)
    es = int(db._dir_es[ci])
    before = np.array(db._status[es + start:es + start + count], copy=True)
    w = int.from_bytes(np.packbits(before == STATUS_WAIVED, bitorder="little").tobytes(), "little")
    return v, w, n, before


def _different(source, own_w, own_n, peer_w, peer_n):
    return ((own_w ^ peer_w) if source.waive_known else 0) | ((own_n ^ peer_n) if source.not_required_known else 0)


def import_conflicts(db, source, *, path=None, cancelled=None, progress=None):
    """Prepare the local visited decisions that safe application protects.

    This includes legacy positive work even though it cannot vote in the
    team conflict calculation without a visited record.
    """
    _identity(db, source.metadata)
    output, owner = _temporary_path(path, "floe-import-conflicts-")
    if os.path.realpath(output) == os.path.realpath(source.path):
        raise ValueError("comparison cannot replace a source snapshot")
    sizes = _sizes(db)
    offsets = np.r_[np.int64(0), np.cumsum(sizes, dtype=np.int64)]
    report = _Progress(progress, int(offsets[-1]))
    store = db._review_flags_store()
    try:
        with db._status_lock, store._write_lock:
            with _destination(db, output, "import-conflicts", dict(sources=[dict(reviewer=source.reviewer, snapshot_id=source.snapshot_id)], aggregate=True)) as writer:
                for ci, k, _v, w, n, _c, work in source.iter_chunks():
                    _check(cancelled)
                    ov, ow, on, _before = _own_bits(db, store, ci, k)
                    conflict = ov & work & _different(source, ow, on, w, n)
                    if conflict:
                        writer.add(ci, k, ov & conflict, ow & conflict, on & conflict,
                                   conflict, conflict)
                    report(int(offsets[ci]) + min(sizes[ci], (k+1)*CHUNK))
                _check(cancelled)
        report(int(offsets[-1]), final=True)
        result = ReviewAggregate(db, output, owner=owner)
        result.sources, result.own = [source], None
        return result
    except Exception:
        if owner is not None:
            owner.cleanup()
        raise


def prepare_conflict_selection(db, aggregate, ci, membership=None, waived=None,
                               *, cancelled=None, progress=None):
    """Prepare one main-list conflict scope as an independent Selection.

    Only nonzero conflict chunks are read. Optional cluster/CD membership
    and the current personal waive filter are resolved in bounded 4,096-ID
    masks. The returned read-only bit mmap uses the existing packed-bits
    marker/query protocol and survives closing the reviewer window.

    Pass an ``aggregate.lease()`` acquired before queueing an async job when
    the original aggregate may be closed before that job starts. This helper
    retains its own lease for its duration and never holds the GUI SQL lock.
    """
    from .drc_selection import Selection, CHUNK as SELECTION_CHUNK

    _check(cancelled)
    with aggregate.lease() as source:
        _identity(db, source.metadata)
        if not source.metadata.get("aggregate"):
            raise ValueError("a team comparison is required for conflict filtering")
        ci, size = source._rule(ci)
        if int(getattr(membership, "size", size)) != size:
            raise ValueError("selection size does not match rule")
        if waived is not None and not isinstance(waived, (bool, np.bool_)):
            raise ValueError("waived must be None or boolean")
        report = _Progress(progress, size)
        if (not size or not source.counts(ci)["conflicts"]
                or getattr(membership, "total", None) == 0):
            _check(cancelled)
            report(size, final=True)
            return Selection.empty(size)

        generation = getattr(db, "_status_generation", None) if waived is not None else None
        if generation is not None and generation % 2:
            raise InterruptedError("waive status is changing; retry conflict filtering")
        owner = tempfile.TemporaryDirectory(prefix="floe-conflict-selection-")
        conn = bits = None
        adopted = False
        try:
            # Newly extended file bytes are zero-filled by the filesystem.
            # Do not write a rule-sized zero array: Python only fills
            # conflict chunks (physical allocation is filesystem-specific).
            bits = np.lib.format.open_memmap(os.path.join(owner.name, "bits.npy"),
                                            mode="w+", dtype=np.uint8,
                                            shape=((size + 7) // 8,))
            counts = np.zeros((size + SELECTION_CHUNK - 1) // SELECTION_CHUNK,
                              dtype=np.int64)
            conn = _connect(source.path, timeout=10.0)
            total = 0
            base = int(db._dir_es[ci]) if waived is not None else 0
            # Force the partial conflict index: a primary-key plan ordered
            # by k could scan every visited chunk in a mostly agreed rule.
            cursor = conn.execute(
                "SELECT k,c,cc FROM chunks INDEXED BY page_c "
                "WHERE ci=? AND cc>0 ORDER BY ce,k", (ci,))
            for k, blob, declared_count in cursor:
                _check(cancelled)
                start = int(k) * CHUNK
                if start < 0 or start >= size or len(blob) != _BYTES:
                    raise ValueError("corrupt conflict chunk")
                count = min(CHUNK, size - start)
                value = int.from_bytes(blob, "little")
                if value >> count or value.bit_count() != int(declared_count):
                    raise ValueError("conflict bitmap does not match its count")
                if membership is not None:
                    keep = np.asarray(membership.mask(start, count), dtype=bool)
                    if keep.shape != (count,):
                        raise ValueError("selection mask has incorrect shape")
                    value &= int.from_bytes(np.packbits(keep, bitorder="little").tobytes(), "little")
                if waived is not None and value:
                    keep = np.asarray(db._status[base + start:base + start + count]) == STATUS_WAIVED
                    if not waived:
                        keep = ~keep
                    value &= int.from_bytes(np.packbits(keep, bitorder="little").tobytes(), "little")
                if value:
                    packed = value.to_bytes((count + 7) // 8, "little")
                    bits[start // 8:start // 8 + len(packed)] = np.frombuffer(packed, dtype=np.uint8)
                    found = value.bit_count()
                    counts[start // SELECTION_CHUNK] += found
                    total += found
                report(start + count)
            cursor.close()
            _check(cancelled)
            if (generation is not None
                    and getattr(db, "_status_generation", None) != generation):
                raise InterruptedError("waive status changed; retry conflict filtering")
            if not total:
                report(size, final=True)
                return Selection.empty(size)
            bits.flush()
            prefix = np.r_[np.int64(0), np.cumsum(counts, dtype=np.int64)]
            np.save(os.path.join(owner.name, "prefix.npy"), prefix, allow_pickle=False)
            _check(cancelled)
            if (generation is not None
                    and getattr(db, "_status_generation", None) != generation):
                raise InterruptedError("waive status changed; retry conflict filtering")
            report(size, final=True)
            _check(cancelled)
            selection = Selection.open(owner.name, size, owner=owner)
            adopted = True
            return selection
        finally:
            if conn is not None:
                conn.close()
            if bits is not None:
                bits._mmap.close()
            if not adopted:
                owner.cleanup()


def apply_review(db, source, *, ci=None, membership=None, force=False,
                 cancelled=None, progress=None):
    """Apply known W/NR decisions from source work, preserving own visited.

    Ordinary application protects local visited errors with a differing
    incoming decision. Explicit force resolves only selected source work.
    Chunks commit independently; failures/cancellation never alter sources.
    """
    _identity(db, source.metadata)
    if getattr(source, "metadata", {}).get("aggregate"):
        raise ValueError("choose a reviewer source when applying decisions")
    if membership is not None and ci is None:
        raise ValueError("a rule is required for selected review application")
    sizes = _sizes(db)
    if ci is not None:
        ci = int(ci)
        if not 0 <= ci < len(sizes):
            raise IndexError(ci)
        if int(getattr(membership, "size", sizes[ci])) != sizes[ci]:
            raise ValueError("selection size does not match rule")
    if hasattr(db, "_require_review"):
        db._require_review()
    offsets = np.r_[np.int64(0), np.cumsum(sizes, dtype=np.int64)]
    total = sum(sizes) if ci is None else sizes[ci]
    report = _Progress(progress, total)
    result = dict(changed_waived=0, changed_not_required=0, conflicts_skipped=0,
                  processed=0, applied=0)
    store = db._review_flags_store()
    _check(cancelled)
    for rule, k, _v, w, n, _c, work in source.iter_chunks(ci):
        _check(cancelled)
        count = min(CHUNK, sizes[rule]-k*CHUNK)
        eligible = work
        if membership is not None:
            selected = np.asarray(membership.mask(k*CHUNK, count), dtype=bool)
            if selected.shape != (count,):
                raise ValueError("selection mask has incorrect shape")
            eligible &= int.from_bytes(np.packbits(selected, bitorder="little").tobytes(), "little")
        if eligible:
            # Same lock order as export. The reentrant review writer lock
            # prevents a just-viewed error racing the protection decision.
            with db._status_lock, store._write_lock:
                _check(cancelled)
                ov, ow, on, before = _own_bits(db, store, rule, k)
                different = _different(source, ow, on, w, n)
                conflicts = eligible & ov & different
                if not force:
                    result["conflicts_skipped"] += conflicts.bit_count()
                    eligible &= ~conflicts
                changes = eligible & different
                if source.waive_known:
                    change_w = eligible & (ow ^ w)
                    if change_w:
                        mask = np.unpackbits(np.frombuffer(change_w.to_bytes(_BYTES, "little"), dtype=np.uint8), bitorder="little")[:count].astype(bool)
                        desired = np.unpackbits(np.frombuffer(w.to_bytes(_BYTES, "little"), dtype=np.uint8), bitorder="little")[:count].astype(bool)
                        after = before.copy()
                        after[mask] = np.where(desired[mask], STATUS_WAIVED, STATUS_NONE)
                        db._write_status_span(rule, k*CHUNK, before, after)
                        result["changed_waived"] += change_w.bit_count()
                if source.not_required_known:
                    set_bits, clear_bits = eligible & n & ~on, eligible & on & ~n
                    if set_bits:
                        result["changed_not_required"] += store._change_many(rule, [(k, set_bits)], NOT_REQUIRED, True)
                    if clear_bits:
                        result["changed_not_required"] += store._change_many(rule, [(k, clear_bits)], NOT_REQUIRED, False)
                result["applied"] += changes.bit_count()
        result["processed"] = (int(offsets[rule]) if ci is None else 0) + min(sizes[rule], (k+1)*CHUNK)
        report(result["processed"])
    _check(cancelled)
    result["processed"] = total
    report(total, final=True)
    return result
