"""Exact DRC centers and bounded pixel rasters for individual error markers.

The immutable cache holds two integer center sums per original error,
including half-grid centers. Queries scan it in bounded chunks and retain
one deterministic representative per occupied screen pixel. Reprojection
always starts from exact centers, so zooming separates nearby locations.
"""

from dataclasses import dataclass
from functools import lru_cache
import json
import math
import os
import shutil
import subprocess
import tempfile
import time

import numpy as np

from .cachepath import drc_analysis_dir
from .drc_analysis import pack_identity
from .drc_marker_style import circle_rgba

VERSION = 1
PROTOCOL = 2
CHUNK = 1 << 18
NATIVE_TASK = 1 << 16
PROGRESS_INTERVAL = 0.5
RED = 0xFF5252FF
GREEN = 0x00E676FF
GOLD = 0xFFD700FF
# Lexicographic priority: selected first, then not-waived, then original ID.
_UNSELECTED = np.uint64(1 << 63)
_WAIVED = np.uint64(1 << 62)
_ID_MASK = _WAIVED - np.uint64(1)
_EMPTY = np.uint64((1 << 64) - 1)


@dataclass
class PointMarkers:
    width: int
    height: int
    rgba: bytes
    error_ids: np.ndarray
    check_ids: np.ndarray
    visible_count: int
    occupied_count: int
    processed_count: int = 0
    total_count: int = 0


def _check(cancelled):
    if cancelled is not None and cancelled():
        from .drc_markers import MarkerQueryCancelled
        raise MarkerQueryCancelled()


@lru_cache(maxsize=8)
def _compatible(path, size, mtime_ns, ctime_ns):
    try:
        result = subprocess.run([path, 'drc-centers', '--protocol'],
                                stdin=subprocess.DEVNULL, capture_output=True,
                                text=True, timeout=5)
        return result.returncode == 0 and result.stdout.strip() == str(PROTOCOL)
    except (OSError, subprocess.SubprocessError):
        return False


def find_binary():
    from .vfsclient import find_binary as locate
    try:
        path = locate()
        stat = os.stat(path)
    except (OSError, RuntimeError):
        return None
    return path if _compatible(path, stat.st_size, stat.st_mtime_ns,
                               stat.st_ctime_ns) else None


def _native(index, ci, path, binary, cancelled, on_chunk=None):
    from .drc_native import worker_count
    command = [binary, 'drc-centers', os.path.abspath(index.db.path),
               '--rule-index', str(ci), '--out', path,
               '--jobs', str(worker_count())]
    n = len(index.db.checks[ci].errors)
    ready_path = path + '.ready' if on_chunk is not None else None
    if ready_path is not None:
        command += ['--ready', ready_path]
    delivered = np.zeros((n + NATIVE_TASK - 1) // NATIVE_TASK, dtype=bool)
    centers = None

    def ready_chunks():
        nonlocal centers
        if ready_path is None or not len(delivered):
            return
        try:
            # File creation and sizing are separate native operations. Never
            # map a not-yet-sized output or treat unwritten zeros as centers.
            if (os.path.getsize(ready_path) != len(delivered) or
                    os.path.getsize(path) != n * 16):
                return
            with open(ready_path, 'rb') as stream:
                ready = np.frombuffer(stream.read(), dtype=np.uint8)
        except FileNotFoundError:
            return
        if len(ready) != len(delivered):
            return
        if centers is None:
            centers = np.memmap(path, mode='r', dtype='<i8', shape=(n, 2))
        for task in np.flatnonzero((ready == 1) & ~delivered):
            _check(cancelled)
            start = int(task) * NATIVE_TASK
            on_chunk(start, centers[start:start + NATIVE_TASK], 2 * index.db.precision)
            delivered[task] = True
    _check(cancelled)
    # File-backed logs keep even unexpected verbose child output bounded.
    with tempfile.TemporaryFile() as log:
        process = subprocess.Popen(command, stdin=subprocess.DEVNULL,
                                   stdout=log, stderr=log)
        native_pid = getattr(index, '_point_native_pid', None)
        if native_pid is not None:
            native_pid.value = process.pid
        try:
            while process.poll() is None:
                _check(cancelled)
                ready_chunks()
                time.sleep(0.025)
            _check(cancelled)
            if process.returncode:
                log.seek(0, os.SEEK_END)
                log.seek(max(0, log.tell() - 5000))
                raise RuntimeError('Rust DRC center preparation failed: ' +
                                   log.read().decode('utf-8', 'replace').strip())
            ready_chunks()
            if on_chunk is not None and not np.all(delivered):
                raise ValueError('Rust DRC center preparation omitted ready chunks')
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=1)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
            if native_pid is not None:
                native_pid.value = 0
            if ready_path is not None:
                try:
                    os.unlink(ready_path)
                except FileNotFoundError:
                    pass


def _decode_centers(index, ci, block, cancelled):
    """Decode integer extrema directly, never the conservative qbox lattice."""
    from .drc import _ICE2_BLOCK, _uv, _unzz
    db = index.db
    if not index._packed:
        boxes = index._decode_block(ci, block, cancelled)
        return boxes[:, :2] / 2 + boxes[:, 2:] / 2
    rec = db._blk[int(db._dir_bs[ci]) + block]
    pos, count = int(rec['off']), int(rec['cnt'])
    out = np.empty((count, 2), dtype='<i8')
    buf = db._map
    pfx = pfy = 0
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
        out[j] = xmin + xmax, ymin + ymax
    return out


def rule_path(db, ci):
    return os.path.join(drc_analysis_dir(db.path), 'centers-v%d-%s-%d' %
                        (VERSION, pack_identity(db.path), int(ci)))


class CenterRule:
    def __init__(self, path, db, ci):
        with open(os.path.join(path, 'meta.json'), encoding='utf-8') as stream:
            meta = json.load(stream)
        n = len(db.checks[ci].errors)
        if (not isinstance(meta, dict) or
                (meta.get('version'), meta.get('identity'), meta.get('ci'),
                 meta.get('n'), meta.get('precision')) !=
                (VERSION, pack_identity(db.path), ci, n, db.precision)):
            raise ValueError('invalid or stale DRC center cache')
        raw = os.path.join(path, 'centers.bin')
        if os.path.getsize(raw) != n * 16:
            raise ValueError('invalid DRC center cache size')
        self.centers = (np.memmap(raw, mode='r', dtype='<i8', shape=(n, 2))
                        if n else np.empty((0, 2), dtype='<i8'))
        self.path, self.scale = path, 2 * db.precision


def prepare_rule(index, ci, cancelled=None, on_chunk=None):
    """Open/build an atomic exact-center cache for one packed rule.

    Each original error costs 16 bytes on disk; arrays remain memory mapped.
    Native parallel decoding is preferred, with a cancellable Python fallback
    for installations whose floe-index predates the centers protocol.
    During a new build, ``on_chunk(start, centers, scale)`` receives every
    completed row once; native chunks can arrive out of original error order.
    Cache hits do not invoke this callback. Only completed native tasks are
    visible; the persistent cache remains unpublished until fully validated.
    """
    db, ci = index.db, int(ci)
    if not index._packed:
        raise ValueError('persistent centers require a packed DRC database')
    identity = pack_identity(db.path)
    if identity != getattr(db, '_analysis_identity', identity):
        raise ValueError('DRC pack changed; reload the results database')
    path = rule_path(db, ci)
    try:
        return CenterRule(path, db, ci)
    except (OSError, ValueError, KeyError, TypeError, EOFError):
        pass
    _check(cancelled)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    temp = tempfile.mkdtemp(prefix='.centers-build-', dir=os.path.dirname(path))
    try:
        raw = os.path.join(temp, 'centers.bin')
        n = len(db.checks[ci].errors)
        binary = find_binary()
        if binary is not None:
            _native(index, ci, raw, binary, cancelled, on_chunk)
        else:
            with open(raw, 'wb') as stream:
                for start, centers, scale in _decoded_chunks(index, ci, cancelled):
                    _check(cancelled)
                    stream.write(centers.tobytes())
                    if on_chunk is not None:
                        on_chunk(start, centers, scale)
        if pack_identity(db.path) != identity:
            raise ValueError('DRC pack changed during center preparation; reload the database')
        with open(os.path.join(temp, 'meta.json'), 'w', encoding='utf-8') as stream:
            json.dump(dict(version=VERSION, identity=identity, ci=ci, n=n,
                           precision=db.precision), stream)
        # Validate before publishing; a partial/truncated native file is never used.
        validated = CenterRule(temp, db, ci)
        del validated
        _check(cancelled)
        try:
            os.rename(temp, path)
        except OSError:
            try:
                return CenterRule(path, db, ci)
            except (OSError, ValueError, KeyError, TypeError, EOFError):
                damaged = path + '.invalid-' + os.path.basename(temp)
                if os.path.exists(path):
                    os.rename(path, damaged)
                os.rename(temp, path)
                shutil.rmtree(damaged, ignore_errors=True)
        return CenterRule(path, db, ci)
    finally:
        shutil.rmtree(temp, ignore_errors=True)


def _decoded_chunks(index, ci, cancelled):
    from .drc import _ICE2_BLOCK
    n = len(index.db.checks[ci].errors)
    scale = 2 * index.db.precision if index._packed else 1
    start = 0
    while start < n:
        _check(cancelled)
        count = min(NATIVE_TASK, n - start)
        centers = np.empty((count, 2), dtype='<i8' if index._packed else float)
        begun = time.monotonic()
        stop = 0
        for offset in range(0, count, _ICE2_BLOCK):
            values = _decode_centers(index, ci, (start + offset) // _ICE2_BLOCK, cancelled)
            centers[offset:offset + len(values)] = values
            stop = offset + len(values)
            # Python/plain-DB decoding can be much slower than native. Expose
            # already decoded blocks promptly even before a full task is ready.
            if time.monotonic() - begun >= 0.1:
                break
        yield start, centers[:stop], scale
        start += stop


def _visit_chunks(index, ci, cancelled, consume, progressive):
    """Consume new-build chunks directly; do not rescan them after publication."""
    n = len(index.db.checks[ci].errors)
    delivered = 0

    def prepared(start, values, scale):
        nonlocal delivered
        delivered += len(values)
        consume(start, values, scale)

    if index._packed:
        try:
            rule = prepare_rule(index, ci, cancelled,
                                on_chunk=prepared if progressive else None)
        except OSError:
            # Read-only caches still work, without retaining rule-sized RAM.
            # A failure after emitting build data must fail that query instead
            # of rescanning and counting the partial scope twice.
            if delivered:
                raise
            rule = None
        if delivered:
            if delivered != n:
                raise ValueError('DRC center preparation returned an incomplete scope')
            return
        if rule is not None:
            for start in range(0, n, CHUNK):
                _check(cancelled)
                consume(start, rule.centers[start:start + CHUNK], rule.scale)
            return
    for start, values, scale in _decoded_chunks(index, ci, cancelled):
        consume(start, values, scale)


def _rgba(ranks, cancelled):
    """Stamp the same solid AA 5px circle through 25 vectorized shifts."""
    height, width = ranks.shape
    occupied = ranks != _EMPTY
    unselected = occupied & (ranks >= _UNSELECTED)
    masks = ((unselected & ((ranks & _WAIVED) != 0), GREEN),
             (unselected & ((ranks & _WAIVED) == 0), RED),
             (occupied & (ranks < _UNSELECTED), GOLD))
    out = np.zeros((height, width, 4), dtype=np.uint8)
    for mask, color in masks:
        if not np.any(mask):
            continue
        alpha = np.zeros((height, width), dtype=np.uint8)
        sprite = np.frombuffer(circle_rgba(2, RED, solid=True), np.uint8).reshape(5, 5, 4)
        for sy in range(5):
            _check(cancelled)
            for sx in range(5):
                coverage = sprite[sy, sx, 3]
                if not coverage:
                    continue
                dx, dy = sx - 2, sy - 2
                xa, xb = max(0, -dx), min(width, width - dx)
                ya, yb = max(0, -dy), min(height, height - dy)
                if xa >= xb or ya >= yb:
                    continue
                dest = alpha[ya + dy:yb + dy, xa + dx:xb + dx]
                np.maximum(dest, mask[ya:yb, xa:xb] * coverage, out=dest)
        # Gold is painted last, including circle fringes, just as the former
        # selected-error overlay. Red wins remaining mixed-status overlaps.
        keep = alpha > 0
        out[keep, :3] = ((color >> 24) & 255, (color >> 16) & 255,
                         (color >> 8) & 255)
        out[keep, 3] = alpha[keep]
    return out.tobytes()


def query_points(index, bounds_um, width_px, height_px, checks=None,
                 members=None, waived=None, cancelled=None, progress=None,
                 selected=None):
    """Project complete scopes into a bounded raster, preserving picking IDs.

    ``progress`` receives independent cumulative snapshots roughly every
    half second, including while a cold center cache is still being built.
    The final return is always complete; cancellation never returns a final
    partial result. The caller owns discarding superseded progress messages.
    """
    _check(cancelled)
    last_progress = time.monotonic()
    bounds = tuple(float(v) for v in bounds_um)
    if len(bounds) != 4 or not all(math.isfinite(v) for v in bounds):
        raise ValueError('marker bounds must contain four finite values')
    if not all(math.isfinite(float(v)) for v in (width_px, height_px)):
        raise ValueError('marker screen dimensions must be finite')
    width, height = max(0, int(width_px)), max(0, int(height_px))
    ranks = np.full((height, width), _EMPTY, dtype=np.uint64)
    flat = ranks.ravel()
    db = index.db
    checks = (range(len(db.checks)) if checks is None else sorted(set(map(int, checks))))
    checks = [ci for ci in checks if 0 <= ci < len(db.checks) and len(db.checks[ci].errors)]
    offsets, total = [], 0
    for ci in checks:
        offsets.append(total)
        total += len(db.checks[ci].errors)
    if total >= int(_WAIVED):
        raise ValueError('too many DRC errors for marker representatives')
    visible = processed = 0
    bases = np.asarray(offsets, dtype=np.int64)
    check_array = np.asarray(checks, dtype=np.int32)

    def snapshot():
        _check(cancelled)
        occupied = ranks != _EMPTY
        # These arrays are independent on every publication. Neither later
        # raster updates nor duplicate/status replacement can mutate a frame
        # that the GTK thread or a multiprocessing pipe is already reading.
        error_ids = np.full((height, width), -1, dtype=np.int64)
        check_ids = np.full((height, width), -1, dtype=np.int32)
        if np.any(occupied):
            ids = (ranks[occupied] & _ID_MASK).astype(np.int64)
            rule = np.searchsorted(bases, ids, side='right') - 1
            error_ids[occupied] = ids - bases[rule]
            check_ids[occupied] = check_array[rule]
        error_ids.flags.writeable = check_ids.flags.writeable = False
        return PointMarkers(width, height, _rgba(ranks, cancelled), error_ids,
                            check_ids, int(visible), int(np.count_nonzero(occupied)),
                            int(processed), int(total))

    def publish_due():
        nonlocal last_progress
        if progress is not None:
            now = time.monotonic()
            if now - last_progress >= PROGRESS_INTERVAL:
                # Measure start-to-start; rendering and IPC belong inside
                # the half-second interval rather than extending every tick.
                last_progress = now
                progress(snapshot())

    x0, y0, x1, y1 = bounds
    if width and height and x0 < x1 and y0 < y1:
        for ci, base in zip(checks, offsets):
            _check(cancelled)
            member = members.get(ci) if members is not None else None
            selection = selected.get(ci) if selected is not None else None
            def consume(start, values, scale):
                nonlocal visible, processed
                _check(cancelled)
                count = len(values)
                processed += count
                # Convert only this bounded chunk. Centers retain their exact
                # integer half-grid values in persistent storage at every zoom.
                xy = np.asarray(values, dtype=np.float64) / scale
                keep = ((xy[:, 0] >= x0) & (xy[:, 0] <= x1) &
                        (xy[:, 1] >= y0) & (xy[:, 1] <= y1))
                if member is not None:
                    keep &= member.mask(start, count)
                status = index._status(ci, start, count)
                if waived is not None:
                    keep &= status if waived else ~status
                local = np.flatnonzero(keep)
                visible += len(local)
                if not len(local):
                    publish_due()
                    return
                xy = xy[local]
                xp = np.rint((xy[:, 0] - x0) * (width / (x1 - x0))).astype(np.int64)
                yp = np.rint((y1 - xy[:, 1]) * (height / (y1 - y0))).astype(np.int64)
                # Closed viewport boundaries map to the last visible pixel;
                # centers outside the viewport are never dragged onto an edge.
                np.clip(xp, 0, width - 1, out=xp)
                np.clip(yp, 0, height - 1, out=yp)
                ids = (local + start + base).astype(np.uint64)
                ids |= status[local].astype(np.uint64) * _WAIVED
                if selection is None:
                    ids |= _UNSELECTED
                else:
                    selected_mask = np.asarray(selection.mask(start, count), dtype=bool)
                    ids |= (~selected_mask[local]).astype(np.uint64) * _UNSELECTED
                np.minimum.at(flat, yp * width + xp, ids)
                publish_due()

            _visit_chunks(index, ci, cancelled, consume, progress is not None)
    return snapshot()
