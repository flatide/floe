"""Durable, independent visited/not-required flags without geometry reads.

Only touched 4,096-error chunks are stored. Each flag is one little-endian
bit per error; empty planes take no blob space. Bitmap changes and the
three overlap-aware counters commit together. Waiver files are unrelated.
"""

from collections import OrderedDict
from contextlib import contextmanager
import hashlib
import json
import os
import sqlite3
import sys
import threading
import time

import numpy as np

from . import cachepath


VISITED = 1
NOT_REQUIRED = 2
CHUNK = 4096
_BYTES = CHUNK // 8
_SCAN = 65536
_CACHE_SIZE = 128
_VERSION = 1


class ReviewIdentityError(ValueError):
    """Existing user state belongs to another result set; it is preserved."""


class ReviewCancelled(InterruptedError):
    def __init__(self, processed, total, changed):
        super().__init__("review edit cancelled; committed chunks are retained")
        self.processed, self.total, self.changed = processed, total, changed


def _exists(path):
    # os.path.exists/lexists hide permission errors as absence. Doing that
    # here could silently abandon an inaccessible existing review history.
    try:
        os.stat(path, follow_symlinks=False)
        return True
    except FileNotFoundError:
        return False


def review_path(pack_path, reviewer=None):
    if reviewer is None:
        from .drc import _waive_user
        reviewer = _waive_user()
    reviewer = "".join(c if c.isalnum() or c in "._-" else "_"
                       for c in str(reviewer)) or "user"
    base = cachepath.db_path_of(os.fspath(pack_path))
    name = ".%s.review.%s.sqlite3" % (cachepath.db_name_of(pack_path), reviewer)
    root = os.environ.get("FLOE_DRC_REVIEW_ROOT")
    if root:
        key = hashlib.sha256(os.path.realpath(base).encode("utf-8", "surrogateescape")).hexdigest()[:20]
        return os.path.join(os.path.abspath(os.path.expanduser(root)), key, name)
    return os.path.join(os.path.dirname(base), name)


def _fallback_path(pack_path, reviewer):
    # Unlike the historical waive fallback in /tmp, these flags are durable.
    root = os.environ.get("XDG_DATA_HOME") or os.path.expanduser("~/.local/share")
    if root.startswith("~"):
        raise OSError("no durable user-data directory; set FLOE_DRC_REVIEW_ROOT")
    base = cachepath.db_path_of(os.fspath(pack_path))
    key = hashlib.sha256(os.path.realpath(base).encode("utf-8", "surrogateescape")).hexdigest()[:20]
    return os.path.join(os.path.abspath(root), "floe", "review", key,
                        os.path.basename(review_path(pack_path, reviewer)))


def _counts(total, values):
    visited, not_required, both = map(int, values)
    if not (0 <= both <= min(visited, not_required)
            and max(visited, not_required) <= int(total)
            and visited + not_required - both <= int(total)):
        raise ValueError("corrupt review flag counters")
    return dict(total=int(total), visited=visited, not_required=not_required,
                visited_not_required=both,
                pending=int(total) - visited - not_required + both)


class ReviewStore:
    """Lazy store. Analysis-only instances never open or create sidecars.

    ``counts`` checks cross-connection commits immediately. ``flags`` uses a
    bounded chunk cache, checking external changes at most every 100 ms.
    Readers use a short cache lock; writes have a separate connection/lock.
    Callers must finish background edits before closing this object.
    """

    def __init__(self, pack_path, rule_counts, fingerprint, *, writable=True,
                 path=None, reviewer=None):
        if reviewer is None:
            from .drc import _waive_user
            reviewer = _waive_user()
        self.pack_path = os.fspath(pack_path)
        self.sizes = tuple(int(n) for n in rule_counts)
        if any(n < 0 for n in self.sizes):
            raise ValueError("negative review rule count")
        self.total = sum(self.sizes)
        self.fingerprint = str(fingerprint)
        self.writable = bool(writable)
        self._explicit = path is not None or bool(os.environ.get("FLOE_DRC_REVIEW_ROOT"))
        self.path = os.fspath(path) if path is not None else review_path(pack_path, reviewer)
        self._fallback = None if self._explicit else _fallback_path(pack_path, reviewer)
        # A previous fallback is the durable working copy, unless the primary
        # exists. Never hide an unreadable/corrupt primary behind empty state.
        if (self.writable and not _exists(self.path) and self._fallback
                and _exists(self._fallback)):
            self.path = self._fallback
            sys.stderr.write("[drc] using durable review flags in %s (results-folder fallback)\n" % self.path)
        self._lock = threading.RLock()
        self._write_lock = threading.RLock()
        self._conn = None
        self._writer = None
        self._closed = False
        self._cache = OrderedDict()
        self._rule_counts = {}
        self._totals = (0, 0, 0)
        self._version = None
        self._checked = 0.0
        self._missing_until = 0.0
        self._read_error = None
        self._retry_read_at = 0.0

    def _rule(self, ci):
        ci = int(ci)
        if not 0 <= ci < len(self.sizes):
            raise IndexError(ci)
        return ci, self.sizes[ci]

    def _key(self, ci, ei):
        ci, size = self._rule(ci)
        ei = int(ei)
        if not 0 <= ei < size:
            raise IndexError(ei)
        return ci, ei // CHUNK, 1 << (ei % CHUNK)

    def _metadata(self):
        return json.dumps(dict(version=_VERSION, fingerprint=self.fingerprint,
                               sizes=self.sizes), separators=(",", ":"))

    def _open_path(self, create):
        existed = _exists(self.path)
        if not create and not existed:
            return None
        if create and not existed:
            os.makedirs(os.path.dirname(os.path.abspath(self.path)), exist_ok=True)
        # A URI prevents a read request from accidentally creating an empty DB.
        from pathlib import Path
        uri = Path(os.path.abspath(self.path)).as_uri() + ("?mode=rwc" if create else "?mode=rw")
        conn = sqlite3.connect(uri, uri=True, timeout=10.0 if create else 0.05,
                               check_same_thread=False, isolation_level=None)
        try:
            conn.execute("PRAGMA busy_timeout=%d" % (10000 if create else 50))
            # Rollback journals are intentional: result folders may be shared
            # filesystems on which WAL's shared-memory coordination is unsafe.
            if not existed and create:
                conn.execute("PRAGMA journal_mode=DELETE")
                conn.execute("PRAGMA synchronous=FULL")
                conn.execute("BEGIN IMMEDIATE")
                conn.execute("CREATE TABLE IF NOT EXISTS metadata (id INTEGER PRIMARY KEY CHECK(id=1), value TEXT NOT NULL)")
                conn.execute("CREATE TABLE IF NOT EXISTS totals (id INTEGER PRIMARY KEY CHECK(id=1), visited INTEGER NOT NULL, nr INTEGER NOT NULL, both INTEGER NOT NULL)")
                conn.execute("CREATE TABLE IF NOT EXISTS rules (ci INTEGER PRIMARY KEY, visited INTEGER NOT NULL, nr INTEGER NOT NULL, both INTEGER NOT NULL)")
                conn.execute("CREATE TABLE IF NOT EXISTS chunks (ci INTEGER NOT NULL, chunk INTEGER NOT NULL, visited BLOB NOT NULL, nr BLOB NOT NULL, PRIMARY KEY(ci,chunk)) WITHOUT ROWID")
                conn.execute("INSERT OR IGNORE INTO metadata VALUES(1,?)", (self._metadata(),))
                conn.execute("INSERT OR IGNORE INTO totals VALUES(1,0,0,0)")
                conn.execute("COMMIT")
            # Another opener can observe the newly created SQLite file just
            # before its creator commits the schema. Retry only that narrow
            # missing-table case; never initialize an existing invalid file.
            deadline = time.monotonic() + 1.0
            while True:
                try:
                    row = conn.execute("SELECT value FROM metadata WHERE id=1").fetchone()
                    break
                except sqlite3.OperationalError as exc:
                    if (not create or "no such table: metadata" not in str(exc)
                            or time.monotonic() >= deadline):
                        raise
                    time.sleep(0.01)
            if row is None or row[0] != self._metadata():
                raise ReviewIdentityError("%s: review flags belong to another DRC result; existing file preserved" % self.path)
            conn.execute("PRAGMA busy_timeout=50")
            self._conn = conn
            self._refresh(force=True)
            return conn
        except Exception:
            if conn.in_transaction:
                conn.execute("ROLLBACK")
            conn.close()
            self._conn = None
            raise

    def _connect(self, create=False, probe=False):
        if self._closed:
            raise OSError("review store is closed")
        if not self.writable:
            if create:
                raise OSError("analysis pack is read-only")
            return None
        if not create:
            self._check_read_error()
        if self._conn is not None:
            return self._conn
        if not create and not probe and time.monotonic() < self._missing_until:
            return None
        try:
            conn = self._connect_attempt(create)
            self._read_error = None
            if conn is None:
                self._missing_until = time.monotonic() + 1.0
            return conn
        except (OSError, sqlite3.Error, ValueError) as exc:
            self._remember_read_error(exc)
            raise

    def _check_read_error(self):
        if self._closed:
            raise OSError("review store is closed")
        if self._read_error is not None and time.monotonic() < self._retry_read_at:
            # Preserve "unavailable", never misrepresent failed reads as
            # unvisited. Clear the traceback to avoid a growing frame chain
            # when a page asks about the same failed store 1,000 times.
            raise self._read_error.with_traceback(None)

    def _remember_read_error(self, exc):
        if exc is not self._read_error or time.monotonic() >= self._retry_read_at:
            self._read_error = exc
            self._retry_read_at = time.monotonic() + 1.0

    @contextmanager
    def _read_access(self):
        self._check_read_error()
        # A first background write can still be creating/opening the store.
        # Never let that initialization's disk wait become a GUI mutex wait.
        if not self._lock.acquire(timeout=0.05):
            exc = OSError("review flags temporarily unavailable during initialization")
            self._remember_read_error(exc)
            raise exc
        try:
            yield
        finally:
            self._lock.release()

    def _connect_attempt(self, create):
        existed = _exists(self.path)
        try:
            return self._open_path(create)
        except (OSError, sqlite3.OperationalError) as exc:
            # Only a genuinely missing primary that cannot be created may
            # fall back. Existing files (including corrupt ones) are never
            # bypassed and their history is never silently abandoned.
            permission = isinstance(exc, PermissionError) or any(
                token in str(exc).lower() for token in
                ("unable to open database", "readonly", "read-only", "permission denied"))
            if not (create and not existed and not self._explicit and permission
                    and self._fallback and self.path != self._fallback):
                raise
            self.path = self._fallback
            conn = self._open_path(True)
            sys.stderr.write("[drc] results folder not writable; durable review flags in %s\n" % self.path)
            return conn

    def _refresh(self, force=False):
        conn = self._conn
        if conn is None:
            return
        version = int(conn.execute("PRAGMA data_version").fetchone()[0])
        if force or version != self._version:
            conn.execute("BEGIN")
            try:
                row = conn.execute("SELECT visited,nr,both FROM totals WHERE id=1").fetchone()
                if row is None:
                    raise ValueError("review flags have no totals record")
                conn.execute("COMMIT")
            except Exception:
                if conn.in_transaction:
                    conn.execute("ROLLBACK")
                raise
            self._totals = tuple(map(int, row))
            self._rule_counts.clear()
            self._cache.clear()
            self._version = version
        self._checked = time.monotonic()

    def _read_chunk(self, ci, chunk, *, fresh=False):
        key = (ci, chunk)
        if not fresh and key in self._cache:
            self._cache.move_to_end(key)
            return self._cache[key]
        row = self._conn.execute("SELECT visited,nr FROM chunks WHERE ci=? AND chunk=?", key).fetchone()
        if row is None:
            values = (0, 0)
        else:
            if any(len(b) not in (0, _BYTES) for b in row):
                raise ValueError("corrupt review flag chunk")
            values = tuple(int.from_bytes(b, "little") for b in row)
        self._cache[key] = values
        if len(self._cache) > _CACHE_SIZE:
            self._cache.popitem(last=False)
        return values

    def flags(self, ci, ei):
        ci, chunk, bit = self._key(ci, ei)
        with self._read_access():
            self._check_read_error()
            try:
                if self._connect() is None:
                    return 0
                if time.monotonic() - self._checked >= 0.1:
                    self._refresh()
                visited, nr = self._read_chunk(ci, chunk)
                return (VISITED if visited & bit else 0) | (NOT_REQUIRED if nr & bit else 0)
            except (OSError, sqlite3.Error, ValueError) as exc:
                self._remember_read_error(exc)
                raise

    def counts(self, ci=None):
        if ci is not None:
            ci, size = self._rule(ci)
        else:
            size = self.total
        with self._read_access():
            self._check_read_error()
            try:
                if self._connect(probe=True) is not None:
                    self._refresh()
                    if ci is not None and ci not in self._rule_counts:
                        row = self._conn.execute("SELECT visited,nr,both FROM rules WHERE ci=?", (ci,)).fetchone()
                        self._rule_counts[ci] = (0, 0, 0) if row is None else tuple(map(int, row))
                return _counts(size, self._totals if ci is None else
                               self._rule_counts.get(ci, (0, 0, 0)))
            except (OSError, sqlite3.Error, ValueError) as exc:
                self._remember_read_error(exc)
                raise

    def _change(self, ci, chunk, bits, flag, enabled):
        return self._change_many(ci, [(chunk, bits)], flag, enabled)

    def _change_many(self, ci, edits, flag, enabled):
        edits = [(chunk, bits) for chunk, bits in edits if bits]
        if not edits:
            return 0
        with self._write_lock:
            with self._lock:
                self._connect(create=True)
                if self._writer is None:
                    from pathlib import Path
                    uri = Path(os.path.abspath(self.path)).as_uri() + "?mode=rw"
                    self._writer = sqlite3.connect(uri, uri=True, timeout=10.0,
                                                   check_same_thread=False,
                                                   isolation_level=None)
                    self._writer.execute("PRAGMA synchronous=FULL")
                conn = self._writer
            # SQLite may wait for another process's writer here. The UI's
            # reader connection and chunk/count cache do not share this lock.
            conn.execute("BEGIN IMMEDIATE")
            changed = 0
            delta = [0, 0, 0]
            try:
                for chunk, bits in edits:
                    # Another process may have edited the same chunk. Read
                    # under SQLite's write lock instead of using our cache.
                    row = conn.execute("SELECT visited,nr FROM chunks WHERE ci=? AND chunk=?", (ci, chunk)).fetchone()
                    if row is not None and any(len(b) not in (0, _BYTES) for b in row):
                        raise ValueError("corrupt review flag chunk")
                    old = ((0, 0) if row is None else
                           tuple(int.from_bytes(b, "little") for b in row))
                    new = list(old)
                    slot = 0 if flag == VISITED else 1
                    new[slot] = old[slot] | bits if enabled else old[slot] & ~bits
                    changed_here = (old[slot] ^ new[slot]).bit_count()
                    if not changed_here:
                        continue
                    changed += changed_here
                    before = (old[0].bit_count(), old[1].bit_count(), (old[0] & old[1]).bit_count())
                    after = (new[0].bit_count(), new[1].bit_count(), (new[0] & new[1]).bit_count())
                    for k in range(3):
                        delta[k] += after[k] - before[k]
                    if new[0] or new[1]:
                        conn.execute("INSERT OR REPLACE INTO chunks VALUES(?,?,?,?)",
                                     (ci, chunk, new[0].to_bytes(_BYTES, "little") if new[0] else b"",
                                      new[1].to_bytes(_BYTES, "little") if new[1] else b""))
                    else:
                        conn.execute("DELETE FROM chunks WHERE ci=? AND chunk=?", (ci, chunk))
                if changed:
                    delta = tuple(delta)
                    conn.execute("INSERT OR IGNORE INTO rules VALUES(?,0,0,0)", (ci,))
                    conn.execute("UPDATE rules SET visited=visited+?,nr=nr+?,both=both+? WHERE ci=?", delta + (ci,))
                    conn.execute("UPDATE totals SET visited=visited+?,nr=nr+?,both=both+? WHERE id=1", delta)
                    # Counters may have advanced through another connection.
                    current = conn.execute("SELECT visited,nr,both FROM rules WHERE ci=?", (ci,)).fetchone()
                    totals = conn.execute("SELECT visited,nr,both FROM totals WHERE id=1").fetchone()
                conn.execute("COMMIT")
            except Exception:
                if conn.in_transaction:
                    conn.execute("ROLLBACK")
                raise
            with self._lock:
                self._read_error = None
                if changed:
                    # A different process could have committed after ours.
                    # Force the next read onto a fresh snapshot rather than
                    # overwriting any newer reader cache with our old values.
                    self._cache.clear()
                    self._version = None
                    self._checked = 0.0
                    self._rule_counts[ci] = tuple(current)
                    self._totals = tuple(totals)
            return changed

    def mark_visited(self, ci, ei):
        ci, chunk, bit = self._key(ci, ei)
        return self._change(ci, chunk, bit, VISITED, True)

    def set_members(self, ci, membership, not_required, *, cancelled=None,
                    progress=None):
        """Set/clear exclusion in bounded chunks; previously committed edits
        survive cancellation. Progress reports scanned rule positions.
        """
        ci, size = self._rule(ci)
        if not self.writable:
            raise OSError("analysis pack is read-only")
        if int(getattr(membership, "size", size)) != size:
            raise ValueError("selection size does not match rule")
        changed = done = 0
        for start in range(0, size, _SCAN):
            if cancelled is not None and cancelled():
                raise ReviewCancelled(done, size, changed)
            count = min(_SCAN, size - start)
            if membership is None:
                packed = None
            else:
                mask = np.asarray(membership.mask(start, count), dtype=bool)
                if mask.shape != (count,):
                    raise ValueError("selection mask has incorrect shape")
                packed = np.packbits(mask, bitorder="little").tobytes()
            edits = []
            for offset in range(0, count, CHUNK):
                n = min(CHUNK, count - offset)
                bits = ((1 << n) - 1 if packed is None else int.from_bytes(
                    packed[offset // 8:(offset + n + 7) // 8], "little"))
                if bits:
                    edits.append(((start + offset) // CHUNK, bits))
            if cancelled is not None and cancelled():
                raise ReviewCancelled(done, size, changed)
            changed += self._change_many(ci, edits, NOT_REQUIRED, bool(not_required))
            done = start + count
            if progress is not None:
                progress(done, size, changed)
        if not size and progress is not None:
            progress(0, 0, 0)
        return changed

    def close(self):
        with self._write_lock, self._lock:
            self._closed = True
            if self._writer is not None:
                self._writer.close()
                self._writer = None
            if self._conn is not None:
                self._conn.close()
                self._conn = None
            self._cache.clear()

    def __del__(self):
        try:
            self.close()
        except Exception:
            pass
