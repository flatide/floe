"""Immutable, disk-backed DRC selections with bounded paging and queries.

Sparse UI edits retain sorted numeric IDs. Dense selections use one little-
endian bit per rule-local error: 200 million errors need at most 25 MB, with
small rank/page prefixes. No selected geometry or Python set is retained.
"""

from collections import namedtuple
import json
import math
import os
import subprocess
import sys
import tempfile
import threading
import time

import numpy as np

from .drc import IcePack, STATUS_WAIVED, _ICE2_BLOCK, _uv, _unzz


CHUNK = 1 << 16
_BYTE_CHUNK = CHUNK // 8
_POPCOUNT = np.asarray([i.bit_count() for i in range(256)], dtype=np.uint8)
SelectionProgress = namedtuple('SelectionProgress', 'processed total count')


class SelectionCancelled(Exception):
    pass


def _check(cancelled):
    if cancelled is not None and cancelled():
        raise SelectionCancelled()


class Selection:
    """Immutable membership for one rule; ``size`` is its universe size.

    ``total``/``count``/``len`` are selected counts. Geometry and review
    predicates are resolved when a selection is built, not while paging it.
    """

    @classmethod
    def empty(cls, size):
        return cls.from_indices(size, np.empty(0, dtype=np.int64))

    @classmethod
    def from_indices(cls, size, indices):
        size = int(size)
        if size < 0:
            raise ValueError('selection size must be nonnegative')
        values = (np.asarray(indices, dtype=np.int64) if isinstance(indices, (np.ndarray, list, tuple))
                  else np.fromiter(indices, dtype=np.int64))
        if values.ndim != 1 or np.any(values < 0) or np.any(values >= size):
            raise ValueError('selection indices are outside the rule')
        obj = cls()
        obj.size = size
        obj.indices = np.unique(values)
        obj.indices.flags.writeable = False
        obj.total = len(obj.indices)
        obj._owner = None
        return obj

    @classmethod
    def from_packed(cls, size, bits, owner=None, prefix=None):
        size = int(size)
        if size < 0:
            raise ValueError('selection size must be nonnegative')
        values = bits if isinstance(bits, np.memmap) else np.asarray(bits)
        if values.dtype != np.dtype('u1') or values.shape != ((size + 7) // 8,):
            raise ValueError('invalid packed selection shape or dtype')
        if size % 8 and len(values) and int(values[-1]) >> (size % 8):
            raise ValueError('packed selection has out-of-range bits')
        # Read-only mappings can be shared. Other callers cannot mutate a
        # published selection by retaining their original writable array.
        if values.flags.writeable:
            values = values.copy()
        values.flags.writeable = False
        obj = cls()
        obj.size, obj.bits, obj._owner = size, values, owner
        if prefix is None:
            counts = np.fromiter((int(_POPCOUNT[values[a:a + _BYTE_CHUNK]].sum())
                                  for a in range(0, len(values), _BYTE_CHUNK)),
                                 dtype=np.int64)
            prefix = np.r_[np.int64(0), np.cumsum(counts, dtype=np.int64)]
        obj._prefix = np.asarray(prefix, dtype=np.int64)
        if obj._prefix.shape != ((size + CHUNK - 1) // CHUNK + 1,):
            raise ValueError('invalid selection prefix shape')
        obj._prefix.flags.writeable = False
        obj.total = int(obj._prefix[-1])
        return obj

    @classmethod
    def open(cls, folder, size, owner=None):
        bits = np.load(os.path.join(folder, 'bits.npy'), mmap_mode='r', allow_pickle=False)
        prefix = np.load(os.path.join(folder, 'prefix.npy'), mmap_mode='r', allow_pickle=False)
        return cls.from_packed(size, bits, owner=owner, prefix=prefix)

    def __len__(self):
        return self.total

    def __del__(self):
        owner = getattr(self, '_owner', None)
        if owner is not None:
            try:
                owner.cleanup()
            except OSError:
                pass

    def __bool__(self):
        return self.total > 0

    def count(self, waived=None):
        if waived is not None:
            raise ValueError('selection status must be filtered before paging')
        return self.total

    def contains(self, ei):
        ei = int(ei)
        if not 0 <= ei < self.size:
            return False
        if hasattr(self, 'indices'):
            k = int(np.searchsorted(self.indices, ei))
            return k < self.total and int(self.indices[k]) == ei
        return bool(int(self.bits[ei // 8]) & (1 << (ei % 8)))

    __contains__ = contains

    def contains_many(self, indices):
        ids = np.asarray(indices, dtype=np.int64)
        valid = (ids >= 0) & (ids < self.size)
        out = np.zeros(ids.shape, dtype=bool)
        if hasattr(self, 'indices'):
            k = np.searchsorted(self.indices, ids[valid])
            if self.total:
                out[valid] = (k < self.total) & (self.indices[np.minimum(k, self.total - 1)] == ids[valid])
        elif np.any(valid):
            out[valid] = ((self.bits[ids[valid] // 8] >> (ids[valid] % 8)) & 1).astype(bool)
        return out

    def mask(self, start, count):
        start, count = int(start), max(0, int(count))
        out = np.zeros(count, dtype=bool)
        a, b = max(0, start), min(self.size, start + count)
        if a >= b:
            return out
        if hasattr(self, 'indices'):
            lo, hi = np.searchsorted(self.indices, [a, b])
            out[self.indices[lo:hi] - start] = True
        else:
            unpacked = np.unpackbits(self.bits[a // 8:(b + 7) // 8], bitorder='little')
            out[a - start:b - start] = unpacked[a % 8:a % 8 + b - a]
        return out

    def page(self, start, limit, waived=None):
        if waived is not None:
            raise ValueError('selection status must be filtered before paging')
        start, limit = max(0, int(start)), max(0, int(limit))
        if not limit or start >= self.total:
            return []
        if hasattr(self, 'indices'):
            return self.indices[start:start + limit].tolist()
        chunk = int(np.searchsorted(self._prefix, start, side='right') - 1)
        out = []
        while chunk < len(self._prefix) - 1 and len(out) < limit:
            before = int(self._prefix[chunk])
            ids = np.flatnonzero(np.unpackbits(
                self.bits[chunk * _BYTE_CHUNK:(chunk + 1) * _BYTE_CHUNK], bitorder='little'))
            skip = max(0, start - before)
            out.extend((ids[skip:skip + limit - len(out)] + chunk * CHUNK).tolist())
            end = int(self._prefix[chunk + 1])
            chunk = int(np.searchsorted(self._prefix, end, side='right') - 1)
        return out

    def rank(self, ei, waived=None):
        if waived is not None:
            raise ValueError('selection status must be filtered before paging')
        ei = int(ei)
        if not self.contains(ei):
            return None
        if hasattr(self, 'indices'):
            return int(np.searchsorted(self.indices, ei))
        chunk, byte = ei // CHUNK, ei // 8
        return (int(self._prefix[chunk]) +
                int(_POPCOUNT[self.bits[chunk * _BYTE_CHUNK:byte]].sum()) +
                (int(self.bits[byte]) & ((1 << (ei % 8)) - 1)).bit_count())

    def __getitem__(self, key):
        if isinstance(key, slice):
            start, stop, step = key.indices(self.total)
            if step != 1:
                raise ValueError('selection slices require step 1')
            return self.page(start, stop - start)
        index = int(key)
        if index < 0:
            index += self.total
        page = self.page(index, 1) if 0 <= index < self.total else []
        if not page:
            raise IndexError(key)
        return page[0]

    def __iter__(self):
        # For background bulk operations only. UI consumers use bounded pages.
        for start in range(0, self.total, CHUNK):
            yield from self.page(start, CHUNK)

    def combine(self, other, mode='replace', cancelled=None):
        if self.size != other.size:
            raise ValueError('cannot combine selections from different rule sizes')
        if mode == 'replace':
            return other
        if mode not in ('add', 'toggle', 'remove', 'intersect'):
            raise ValueError('invalid selection combination mode')
        if hasattr(self, 'indices') and hasattr(other, 'indices'):
            operation = {'add': np.union1d, 'toggle': np.setxor1d,
                         'remove': np.setdiff1d, 'intersect': np.intersect1d}[mode]
            return Selection.from_indices(self.size, operation(self.indices, other.indices))
        bits = np.empty((self.size + 7) // 8, dtype=np.uint8)
        for start in range(0, self.size, CHUNK):
            _check(cancelled)
            count = min(CHUNK, self.size - start)
            mask = _combine(self.mask(start, count), other.mask(start, count), mode)
            packed = np.packbits(mask, bitorder='little')
            bits[start // 8:start // 8 + len(packed)] = packed
        bits.flags.writeable = False
        return Selection.from_packed(self.size, bits)


def _combine(previous, current, mode):
    if mode == 'replace':
        return current
    if mode == 'add':
        return previous | current
    if mode == 'toggle':
        return previous ^ current
    if mode == 'remove':
        return previous & ~current
    if mode == 'intersect':
        return previous & current
    raise ValueError('invalid selection combination mode')


def _exact_boxes(db, ci, block, cancelled):
    """One packed block's integer bboxes, without DrcError allocations."""
    rec = db._blk[int(db._dir_bs[ci]) + block]
    pos, count = int(rec['off']), int(rec['cnt'])
    boxes = np.empty((count, 4), dtype=np.int64)
    buf, pfx, pfy = db._map, 0, 0
    for j in range(count):
        _check(cancelled)
        knpts, pos = _uv(buf, pos)
        npts = knpts >> 1
        if not npts:
            raise ValueError('packed DRC error has no vertices')
        d, pos = _uv(buf, pos)
        x = pfx + _unzz(d)
        d, pos = _uv(buf, pos)
        y = pfy + _unzz(d)
        pfx, pfy = x, y
        xmin = xmax = x
        ymin = ymax = y
        for k in range(npts - 1):
            if not k % 1024:
                _check(cancelled)
            d, pos = _uv(buf, pos)
            x += _unzz(d)
            d, pos = _uv(buf, pos)
            y += _unzz(d)
            xmin, xmax = min(xmin, x), max(xmax, x)
            ymin, ymax = min(ymin, y), max(ymax, y)
        boxes[j] = xmin, ymin, xmax, ymax
    return boxes


def _hits(boxes, bounds):
    x0, y0, x1, y1 = bounds
    return ((boxes[:, 0] <= x1) & (boxes[:, 2] >= x0) &
            (boxes[:, 1] <= y1) & (boxes[:, 3] >= y0))


def _packed_spatial_mask(db, ci, start, keep, bounds, cancelled):
    count = len(keep)
    raw_bounds = tuple(v * db.precision for v in bounds)
    x0, y0, x1, y1 = raw_bounds
    first = start // _ICE2_BLOCK
    block_count = (count + _ICE2_BLOCK - 1) // _ICE2_BLOCK
    bs = int(db._dir_bs[ci]) + first
    records = db._blk[bs:bs + block_count]
    # Entire contained blocks contribute at once, even for 200M errors.
    inside = ((records['x0'] >= math.nextafter(x0, math.inf)) &
              (records['y0'] >= math.nextafter(y0, math.inf)) &
              (records['x1'] <= math.nextafter(x1, -math.inf)) &
              (records['y1'] <= math.nextafter(y1, -math.inf)))
    hit = ((records['x0'] <= math.nextafter(x1, math.inf)) &
           (records['y0'] <= math.nextafter(y1, math.inf)) &
           (records['x1'] >= math.nextafter(x0, -math.inf)) &
           (records['y1'] >= math.nextafter(y0, -math.inf)))
    keep &= np.repeat(hit, _ICE2_BLOCK)[:count]
    if not np.any(keep):
        return keep
    accepted = np.repeat(inside, _ICE2_BLOCK)[:count]
    ambiguous = keep & ~accepted
    if not np.any(ambiguous):
        return keep
    es = int(db._dir_es[ci]) + start
    raw_cb = db._cbb[ci]
    cb = np.asarray(raw_cb, dtype=np.float64) / db.precision
    step = np.asarray((int(raw_cb[2]) - int(raw_cb[0]),
                       int(raw_cb[3]) - int(raw_cb[1])), dtype=float) / (255 * db.precision)
    roundoff = (np.abs(cb[:2]) + 255 * np.abs(step)) * (4 * np.finfo(float).eps)
    q = db._qbox[es:es + count]
    boxes = np.empty((count, 4), dtype=np.float64)
    boxes[:, :2] = np.nextafter(cb[:2] + q[:, :2] * step - roundoff, -np.inf)
    boxes[:, 2:] = np.nextafter(cb[:2] + q[:, 2:] * step + roundoff, np.inf)
    boxes[:, :2] = np.maximum(boxes[:, :2], cb[:2])
    boxes[:, 2:] = np.minimum(boxes[:, 2:], cb[2:])
    keep &= accepted | _hits(boxes, bounds)
    x0, y0, x1, y1 = bounds
    accepted |= ((boxes[:, 0] >= x0) & (boxes[:, 1] >= y0) &
                 (boxes[:, 2] <= x1) & (boxes[:, 3] <= y1))
    ambiguous = keep & ~accepted
    for block in np.unique(np.flatnonzero(ambiguous) // _ICE2_BLOCK):
        _check(cancelled)
        offset = int(block) * _ICE2_BLOCK
        exact = _exact_boxes(db, ci, first + int(block), cancelled).astype(np.float64) / db.precision
        end = offset + len(exact)
        keep[offset:end] &= ~ambiguous[offset:end] | _hits(exact, bounds)
    return keep


def select_rect(db, ci, bounds_um=None, membership=None, waived=None,
                previous=None, mode='replace', cancelled=None, progress=None,
                output=None):
    """Build a complete selection atomically, with exact bbox intersection.

    A ``None`` rectangle applies only membership/review filters. ``output``
    is an owned staging directory for process results, never a public cache.
    """
    _check(cancelled)
    ci = int(ci)
    size = len(db.checks[ci].errors)
    if previous is not None and previous.size != size:
        raise ValueError('previous selection belongs to a different rule size')
    if mode not in ('replace', 'add', 'toggle', 'remove', 'intersect'):
        raise ValueError('invalid selection mode')
    bounds = None if bounds_um is None else tuple(float(v) for v in bounds_um)
    if bounds is not None and (len(bounds) != 4 or not all(math.isfinite(v) for v in bounds)):
        raise ValueError('selection bounds must contain four finite values')
    packed = isinstance(db, IcePack)
    if output is not None:
        os.makedirs(output, exist_ok=True)
        bits = np.lib.format.open_memmap(os.path.join(output, 'bits.npy'),
                                        mode='w+', dtype='u1', shape=((size + 7) // 8,))
    else:
        bits = np.empty((size + 7) // 8, dtype=np.uint8)
    counts = []
    found = 0
    last_progress = time.monotonic()
    for start in range(0, size, CHUNK):
        _check(cancelled)
        count = min(CHUNK, size - start)
        keep = (membership.mask(start, count).copy() if membership is not None
                else np.ones(count, dtype=bool))
        if waived is not None:
            if packed:
                base = int(db._dir_es[ci]) + start
                status = db._status[base:base + count] == STATUS_WAIVED
            elif hasattr(db, 'get_status'):
                status = np.fromiter((db.get_status(ci, ei) == STATUS_WAIVED
                                      for ei in range(start, start + count)), bool, count)
            else:
                status = np.zeros(count, dtype=bool)
            keep &= status if waived else ~status
        if bounds is not None and np.any(keep):
            if bounds[0] > bounds[2] or bounds[1] > bounds[3]:
                keep[:] = False
            elif packed:
                keep = _packed_spatial_mask(db, ci, start, keep, bounds, cancelled)
            else:
                for offset in np.flatnonzero(keep):
                    _check(cancelled)
                    box = db.checks[ci].errors[start + int(offset)].bbox()
                    keep[offset] = (box[0] <= bounds[2] and box[2] >= bounds[0] and
                                    box[1] <= bounds[3] and box[3] >= bounds[1])
        if mode != 'replace':
            old = (previous.mask(start, count) if previous is not None
                   else np.zeros(count, dtype=bool))
            keep = _combine(old, keep, mode)
        part = np.packbits(keep, bitorder='little')
        bits[start // 8:start // 8 + len(part)] = part
        selected_count = int(np.count_nonzero(keep))
        counts.append(selected_count)
        found += selected_count
        now = time.monotonic()
        if progress is not None and now - last_progress >= .5:
            last_progress = now
            progress(SelectionProgress(start + count, size, found))
    _check(cancelled)
    prefix = np.r_[np.int64(0), np.cumsum(counts, dtype=np.int64)]
    if output is not None:
        bits.flush()
        del bits
        np.save(os.path.join(output, 'prefix.npy'), prefix, allow_pickle=False)
        return Selection.open(output, size)
    bits.flags.writeable = False
    return Selection.from_packed(size, bits, prefix=prefix)


class SelectionWorker:
    """Latest-request-only box/edit queries; publish only complete selections."""

    def __init__(self):
        self._condition = threading.Condition()
        self._generation = 0
        self._pending = self._result = None
        self._closed = False
        self._process_pid = None
        self._thread = threading.Thread(target=self._run, name='drc-selection', daemon=True)
        self._thread.start()

    def submit(self, key, db, ci, bounds_um=None, membership=None, waived=None,
               previous=None, mode='replace'):
        with self._condition:
            if self._closed:
                return
            self._generation += 1
            self._pending = (self._generation, key, db, int(ci),
                             None if bounds_um is None else tuple(float(v) for v in bounds_um),
                             membership, waived, previous, mode)
            self._result = None
            self._condition.notify()

    def poll(self):
        with self._condition:
            result, self._result = self._result, None
            return result

    def cancel(self):
        with self._condition:
            self._generation += 1
            self._pending = self._result = None
            self._condition.notify()

    def close(self):
        with self._condition:
            self._closed = True
            self._generation += 1
            self._pending = self._result = None
            self._condition.notify()

    def _process_query(self, db, ci, bounds, membership, waived, previous,
                       mode, cancelled, progress):
        from .drc_marker_worker import export_members
        from .drc_query_worker import _children
        folder = tempfile.TemporaryDirectory(prefix='floe-drc-selection-')
        process = None
        adopted = False
        try:
            source = os.path.join(folder.name, 'request.json')
            target = os.path.join(folder.name, 'result.json')
            progress_path = os.path.join(folder.name, 'progress.json')
            size = len(db.checks[ci].errors)
            request = dict(pack=os.fspath(db.path), identity=db._analysis_identity,
                           review_path=os.fspath(db._waive_path), ci=ci, bounds=bounds,
                           member=export_members(membership, folder.name, size),
                           previous=export_members(previous, folder.name, size),
                           waived=waived, mode=mode, folder=folder.name)
            _check(cancelled)
            with open(source, 'w', encoding='utf-8') as stream:
                json.dump(request, stream)
            env = os.environ.copy()
            root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
            env['PYTHONPATH'] = root + os.pathsep + env.get('PYTHONPATH', '')
            with tempfile.TemporaryFile(mode='w+b') as log:
                process = subprocess.Popen([sys.executable, '-m', 'floe.drc_selection', source, target],
                                           stdout=subprocess.DEVNULL, stderr=log, env=env)
                _children.add(process)
                self._process_pid = process.pid
                latest = None
                while process.poll() is None:
                    _check(cancelled)
                    try:
                        with open(progress_path, encoding='utf-8') as stream:
                            value = json.load(stream)
                    except (OSError, ValueError):
                        value = None
                    if value is not None and value != latest:
                        latest = value
                        progress(SelectionProgress(*value))
                    with self._condition:
                        self._condition.wait(timeout=.025)
                _check(cancelled)
                if process.returncode:
                    log.seek(0, os.SEEK_END)
                    log.seek(max(0, log.tell() - 4096))
                    raise RuntimeError(log.read().decode('utf-8', 'replace').strip()
                                       or 'selection query process failed')
            with open(target, encoding='utf-8') as stream:
                meta = json.load(stream)
            result = Selection.open(folder.name, meta['size'], owner=folder)
            if result.total != meta['count']:
                raise ValueError('selection result count mismatch')
            adopted = True
            return result
        finally:
            self._process_pid = None
            if process is not None:
                if process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=1)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait()
                _children.discard(process)
            if not adopted:
                folder.cleanup()

    def _run(self):
        from .drc_marker_worker import LARGE_RULE
        while True:
            db = key = membership = previous = result = None
            with self._condition:
                while self._pending is None and not self._closed:
                    self._condition.wait()
                if self._closed:
                    return
                generation, key, db, ci, bounds, membership, waived, previous, mode = self._pending
                self._pending = None
            cancelled = lambda: self._closed or generation != self._generation
            def progress(value):
                with self._condition:
                    if not cancelled():
                        self._result = (key, value, None)
            result = error = None
            try:
                if isinstance(db, IcePack) and len(db.checks[ci].errors) >= LARGE_RULE:
                    result = self._process_query(db, ci, bounds, membership, waived,
                                                 previous, mode, cancelled, progress)
                else:
                    result = select_rect(db, ci, bounds, membership, waived, previous,
                                         mode, cancelled, progress)
            except SelectionCancelled:
                continue
            except Exception as exc:
                error = str(exc)
            with self._condition:
                if not cancelled():
                    self._result = (key, result, error)


def _main(source, target):
    from .drc_marker_worker import import_members
    try:
        os.nice(5)
    except (AttributeError, OSError):
        pass
    with open(source, encoding='utf-8') as stream:
        request = json.load(stream)
    db = IcePack(request['pack'], review=False, review_path=request['review_path'])
    try:
        if db._analysis_identity != request['identity']:
            raise ValueError('DRC pack changed; reload the results database')
        ci = request['ci']
        previous = import_members(request['previous'])
        if previous is not None:
            size = len(db.checks[ci].errors)
            if previous.kind == 'packed-bits':
                previous = Selection.from_packed(size, previous.array)
            elif previous.kind == 'indices':
                previous = Selection.from_indices(size, previous.array)
            else:
                raise ValueError('invalid previous selection encoding')
        def progress(value):
            path = os.path.join(request['folder'], 'progress.json')
            with open(path + '.tmp', 'w', encoding='utf-8') as stream:
                json.dump(tuple(value), stream)
            os.replace(path + '.tmp', path)
        result = select_rect(db, ci, request['bounds'], import_members(request['member']),
                             request['waived'], previous, request['mode'],
                             progress=progress, output=request['folder'])
        with open(target, 'w', encoding='utf-8') as stream:
            json.dump(dict(size=result.size, count=result.total), stream)
    finally:
        db.close()


if __name__ == '__main__':
    _main(*sys.argv[1:])
