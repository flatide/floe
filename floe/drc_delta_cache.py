"""Persistent, disk-backed CD preprocessing outside the viewer process.

Only immutable measurements and membership are shared. Review snapshots and
their paging prefixes belong to one result, so another viewer cannot change
the current viewer's group counts. A cache hit never decodes geometry or
sorts memberships. Temporary directories are published only on completion.
"""

import atexit
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import weakref
from collections import OrderedDict
from contextlib import contextmanager

import numpy as np

from . import cachepath
from .drc import IcePack
from .drc_analysis import pack_identity
from .drc_delta import (DeltaGroups, DeltaIndex, DeltaQueryCancelled, _CHUNK,
                        _PAGE_CHUNK, _UNKNOWN, _cancel, _read_status)


# Bump when measurement selection, rounding, or membership semantics change.
VERSION = 1
_MEASURE = {"values": "<i8", "choices": "<i4", "estimated": "|b1"}
_DIRECTORY = {"constraints": "<i4", "bins": "<i8", "offsets": "<i8",
              "counts": "<i8", "estimated_counts": "<i8"}
_SCATTER_LIMIT = 4096
_CHILDREN = set()
_CHILD_LOCK = threading.Lock()


def _stop_child(process):
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()


def _stop_children():
    with _CHILD_LOCK:
        children = tuple(_CHILDREN)
    for process in children:
        try:
            _stop_child(process)
        except OSError:
            pass


atexit.register(_stop_children)


def _digest(value):
    raw = json.dumps(value, sort_keys=True, separators=(",", ":"),
                     ensure_ascii=True).encode("utf-8")
    return hashlib.sha256(raw).hexdigest()


def _write_json(path, value):
    temporary = path + ".tmp"
    with open(temporary, "w", encoding="utf-8") as stream:
        json.dump(value, stream, sort_keys=True, separators=(",", ":"))
    os.replace(temporary, path)


def _read_json(path):
    with open(path, encoding="utf-8") as stream:
        return json.load(stream)


def _rule_key(index):
    identity = getattr(index.db, "_analysis_identity", None)
    return {"version": VERSION, "pack": identity or pack_identity(index.db.path),
            "rule": index.ci, "count": len(index.db.checks[index.ci].errors),
            "constraints": index.constraints}


def _rule_dir(index):
    key = _rule_key(index)
    return os.path.join(cachepath.drc_analysis_dir(index.db.path), "delta",
                        _digest(key)), key


def _array(folder, name, dtype, count, mode="r"):
    """Raw arrays keep exact file sizes; empty arrays need no mmap."""
    path = os.path.join(folder, name + ".bin")
    dtype, count = np.dtype(dtype), int(count)
    if mode == "w+":
        with open(path, "wb") as stream:
            stream.truncate(count * dtype.itemsize)
        mode = "r+"
    elif os.path.getsize(path) != count * dtype.itemsize:
        raise ValueError("invalid delta cache array size: " + name)
    if not count:
        return np.empty(0, dtype=dtype)
    return np.memmap(path, dtype=dtype, mode=mode, shape=(count,))


def _flush(*arrays):
    for array in arrays:
        if isinstance(array, np.memmap):
            array.flush()


def _valid_manifest(folder, key):
    try:
        meta = _read_json(os.path.join(folder, "complete.json"))
        if not isinstance(meta, dict):
            return None
        if meta.get("key") != json.loads(json.dumps(key)):
            return None
        return meta
    except (OSError, ValueError, TypeError):
        return None


def _publish(staging, destination):
    """Same-key concurrent builders may reuse the first completed directory."""
    try:
        os.rename(staging, destination)
    except OSError:
        if not os.path.isfile(os.path.join(destination, "complete.json")):
            raise
        shutil.rmtree(staging)


def _staging(parent, prefix, job):
    # A viewer killed without atexit can leave unpublished sparse files.
    # Only reap work whose recorded builder PID no longer exists.
    for name in os.listdir(parent):
        if not name.startswith(prefix):
            continue
        pid = name[len(prefix):].split("-", 1)[0]
        if not pid.isdigit():
            continue
        try:
            os.kill(int(pid), 0)
        except ProcessLookupError:
            shutil.rmtree(os.path.join(parent, name), ignore_errors=True)
        except PermissionError:
            pass
    folder = tempfile.mkdtemp(prefix=prefix + str(os.getpid()) + "-", dir=parent)
    # SIGTERM need not execute Python finally blocks. The coordinator owns
    # this registry and removes only this child's unpublished directories.
    if job.get("staging_registry"):
        with open(job["staging_registry"], "a", encoding="utf-8") as stream:
            stream.write(json.dumps(folder) + "\n")
    return folder


def _check_identity(index):
    if pack_identity(index.db.path) != _rule_key(index)["pack"]:
        raise ValueError("DRC pack changed during delta preprocessing; reopen it")


def _remove_invalid(folder):
    # This is an exact content-addressed derived directory, never user data.
    if os.path.isdir(folder):
        stale = folder + ".invalid-" + os.urandom(8).hex()
        try:
            os.rename(folder, stale)
        except FileNotFoundError:
            return
        shutil.rmtree(stale)


@contextmanager
def _build_lock(destination, cancelled=None):
    """Serialize same-key publishers; process death releases the OS lock."""
    import fcntl
    with open(destination + ".lock", "a+b") as stream:
        while True:
            _cancel(cancelled)
            try:
                fcntl.flock(stream.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
                break
            except BlockingIOError:
                time.sleep(0.025)
        try:
            yield
        finally:
            fcntl.flock(stream.fileno(), fcntl.LOCK_UN)


def load_measurements(index):
    """Attach completed read-only maps; return False for absent/stale data."""
    folder, key = _rule_dir(index)
    folder = os.path.join(folder, "measure")
    meta = _valid_manifest(folder, key)
    if meta is None:
        return False
    try:
        auto_steps = meta["auto_steps"]
        if (set(auto_steps) != {"absolute", "percent"} or any(
                type(step) is not int or not 0 < step <= np.iinfo(np.int64).max
                for step in auto_steps.values())):
            return False
        arrays = [_array(folder, name, dtype, key["count"])
                  for name, dtype in _MEASURE.items()]
    except (OSError, ValueError, TypeError, KeyError):
        return False
    (index.measured_ticks, index.constraint_indices,
     index.estimated_flags) = arrays
    index._auto_steps = dict(auto_steps)
    index._measurement_cache = folder
    return True


def _progress(job, text):
    path = job.get("progress")
    if path:
        _write_json(path, {"text": str(text)})


def _build_measure(index, job, cancelled=None, progress=None, process_callback=None):
    if load_measurements(index):
        return
    base, _key = _rule_dir(index)
    os.makedirs(base, exist_ok=True)
    with _build_lock(os.path.join(base, "measure"), cancelled):
        _build_measure_locked(index, job, cancelled, progress, process_callback)


def _build_measure_locked(index, job, cancelled=None, progress=None, process_callback=None):
    if load_measurements(index):
        return
    base, key = _rule_dir(index)
    destination = os.path.join(base, "measure")
    os.makedirs(base, exist_ok=True)
    _remove_invalid(destination)
    staging = _staging(base, ".measure-", job)
    try:
        _check_identity(index)
        fallback_count = 0
        if job.get("native_binary"):
            from .drc_native import measure
            measure(index, staging, job["native_binary"], jobs=job.get("jobs"),
                    cancelled=cancelled, progress=progress,
                    process_callback=process_callback)
            arrays = [_array(staging, name, dtype, key["count"], "r+")
                      for name, dtype in _MEASURE.items()]
            values, choices, estimated = arrays
            for start in range(0, len(choices), _CHUNK):
                _cancel(cancelled)
                fallback_count += int(np.count_nonzero(choices[start:start + _CHUNK] == -2))
            if fallback_count:
                # A whole rule can sit on a numeric boundary. Keep even
                # that worst-case Python recheck outside the viewer GIL.
                _run_child({"phase": "precision", "pack": os.path.abspath(index.db.path),
                            "pack_identity": key["pack"], "ci": index.ci,
                            "constraints": index.constraints, "arrays": staging},
                           cancelled, progress, process_callback)
            values.flags.writeable = choices.flags.writeable = estimated.flags.writeable = False
            index.measured_ticks, index.constraint_indices, index.estimated_flags = arrays
        else:
            arrays = [_array(staging, name, dtype, key["count"], "w+")
                      for name, dtype in _MEASURE.items()]
            values, choices, estimated = arrays
            choices.fill(_UNKNOWN)
            index._measure_arrays(values, choices, estimated, cancelled=cancelled,
                                  progress=lambda done, total: _progress(
                                      job, "CD measurement %d / %d" % (done, total)))
        _progress(job, "Computing absolute and ratio delta ranges")
        if progress is not None:
            progress("Computing absolute and ratio delta ranges")
        for mode in ("absolute", "percent"):
            index._automatic_step(mode, cancelled)
        _flush(*arrays)
        _cancel(cancelled)
        _check_identity(index)
        _write_json(os.path.join(staging, "complete.json"),
                    {"key": key, "auto_steps": index._auto_steps,
                     "backend": "rust" if job.get("native_binary") else "python",
                     "precision_fallback_count": fallback_count})
        _publish(staging, destination)
    except BaseException:
        index.measured_ticks = index.constraint_indices = index.estimated_flags = None
        index._auto_steps.clear()
        raise
    finally:
        if os.path.isdir(staging):
            shutil.rmtree(staging)
    if not load_measurements(index):
        raise ValueError("completed CD measurement cache could not be read")


class _Mask:
    """Rule-local boolean membership passed by a bounded disk snapshot."""

    def __init__(self, path, count):
        self.values = _array(os.path.dirname(path), os.path.basename(path)[:-4],
                             "|b1", count)

    def mask(self, start, count):
        return self.values[start:start + count]


def _snapshot_members(cluster, n, folder, cancelled):
    if cluster is None:
        return None, "all"
    # The writer runs on DeltaWorker's coordinator, not the GUI thread, and
    # only one chunk is materialized. No arbitrary member list is pickled.
    values = _array(folder, "members", "|b1", n, "w+")
    digest = hashlib.sha256()
    for start in range(0, n, _CHUNK):
        _cancel(cancelled)
        mask = np.asarray(cluster.mask(start, min(_CHUNK, n - start)), dtype=bool)
        values[start:start + len(mask)] = mask
        digest.update(mask.tobytes())
    _flush(values)
    return os.path.join(folder, "members.bin"), digest.hexdigest()


def _group_key(index, step, mode, members_key):
    if mode not in ("absolute", "percent"):
        raise ValueError("unknown grouping basis %r" % mode)
    automatic = step is None
    if automatic:
        step = index._auto_steps[mode]
    else:
        original = step
        step = int(step)
        if step != original or not 0 < step <= np.iinfo(np.int64).max:
            raise ValueError("grouping step must be positive fixed-precision ticks")
    return {"version": VERSION, "step": step, "mode": mode,
            "members": members_key}, automatic


def _group_folder(index, key):
    base, _rule = _rule_dir(index)
    return os.path.join(base, "groups", _digest(key))


def _load_group_arrays(folder, key, n):
    meta = _valid_manifest(folder, key)
    if meta is None:
        return None
    try:
        g, total = int(meta["groups"]), int(meta["total"])
        id_dtype = "<u4" if n <= np.iinfo(np.uint32).max else "<u8"
        if (g < 0 or total < 0 or total > n or g > total or
                g >= np.iinfo(np.int32).max or meta["id_dtype"] != id_dtype):
            return None
        arrays = {name: _array(folder, name, dtype, g + (name == "offsets"))
                  for name, dtype in _DIRECTORY.items()}
        arrays["ids"] = _array(folder, "ids", meta["id_dtype"], total)
        arrays["row_for_error"] = _array(folder, "row_for_error", "<i4", n)
        if arrays["offsets"][0] != 0 or arrays["offsets"][-1] != total:
            return None
    except (OSError, ValueError, KeyError, TypeError):
        return None
    return meta, arrays


def _build_group(index, key, cluster, job):
    destination = _group_folder(index, key)
    existing = _load_group_arrays(destination, key, len(index.measured_ticks))
    if existing is not None:
        return destination, existing
    os.makedirs(os.path.dirname(destination), exist_ok=True)
    with _build_lock(destination):
        return _build_group_locked(index, key, cluster, job)


def _build_group_locked(index, key, cluster, job):
    """External in-place sort: bounded heap even with millions of bins.

    The numeric sort record is a scratch mmap, avoiding lexsort's multiple
    full-rule arrays. IDs stay in increasing original order within each
    group. Only final IDs, row map and the compact directory are retained.
    """
    n = len(index.measured_ticks)
    destination = _group_folder(index, key)
    existing = _load_group_arrays(destination, key, n)
    if existing is not None:
        return destination, existing
    parent = os.path.dirname(destination)
    os.makedirs(parent, exist_ok=True)
    _remove_invalid(destination)
    staging = _staging(parent, ".groups-", job)
    id_dtype = "<u4" if n <= np.iinfo(np.uint32).max else "<u8"
    record_dtype = np.dtype([("choice", "<i4"), ("bin", "<i8"),
                             ("id", id_dtype)])
    try:
        records = _array(staging, "sort", record_dtype, n, "w+")
        total = 0
        histogram = {}
        _progress(job, "Preparing delta memberships")
        for start in range(0, n, _CHUNK):
            ids = np.arange(start, min(start + _CHUNK, n), dtype=np.int64)
            if cluster is not None:
                ids = ids[cluster.mask(start, len(ids))]
            choices, values = index._difference_chunk(ids, key["mode"], None)
            stop = total + len(ids)
            part = records[total:stop]
            part["id"] = ids
            part["choice"] = np.where(choices < 0, len(index.constraints), choices)
            part["bin"] = values // key["step"]
            part["bin"][choices < 0] = 0
            if histogram is not None:
                for choice in np.unique(part["choice"]):
                    bins, counts = np.unique(part["bin"][part["choice"] == choice],
                                              return_counts=True)
                    for bin_id, count in zip(bins, counts):
                        pair = int(choice), int(bin_id)
                        histogram[pair] = histogram.get(pair, 0) + int(count)
                        if len(histogram) > _SCATTER_LIMIT:
                            histogram = None
                            break
                    if histogram is None:
                        break
            total = stop
        if histogram is not None:
            _progress(job, "Writing delta groups with bounded chunk scatter")
            ids, rows, directory, count = _scatter_records(
                index, records, total, histogram, staging, id_dtype)
        else:
            _progress(job, "Sorting delta memberships on disk")
            records[:total].sort(order=("choice", "bin", "id"), kind="quicksort")
            _progress(job, "Writing delta group directory")
            ids, rows, directory, count = _sorted_records(
                index, records, total, staging, id_dtype)
        _flush(ids, rows, *directory.values())
        del records
        os.unlink(os.path.join(staging, "sort.bin"))
        for name, dtype in _DIRECTORY.items():
            os.truncate(os.path.join(staging, name + ".bin"),
                        (count + (name == "offsets")) * np.dtype(dtype).itemsize)
        _check_identity(index)
        _write_json(os.path.join(staging, "complete.json"),
                    {"key": key, "groups": count, "total": total,
                     "id_dtype": id_dtype})
        _publish(staging, destination)
    finally:
        if os.path.isdir(staging):
            shutil.rmtree(staging)
    result = _load_group_arrays(destination, key, n)
    if result is None:
        raise ValueError("completed delta group cache could not be read")
    return destination, result


def _scatter_records(index, records, total, histogram, folder, id_dtype):
    """Common automatic groups need only tiny chunk sorts, never an N sort."""
    keys = sorted(histogram)
    count = len(keys)
    n = len(index.measured_ticks)
    ids = _array(folder, "ids", id_dtype, total, "w+")
    rows = _array(folder, "row_for_error", "<i4", n, "w+")
    rows.fill(-1)
    directory = {name: _array(folder, name, dtype, count + (name == "offsets"), "w+")
                 for name, dtype in _DIRECTORY.items()}
    directory["constraints"][:] = [(_UNKNOWN if ci == len(index.constraints) else ci)
                                    for ci, _bin in keys]
    directory["bins"][:] = [bin_id for _ci, bin_id in keys]
    directory["counts"][:] = [histogram[pair] for pair in keys]
    directory["offsets"][0] = 0
    directory["offsets"][1:] = np.cumsum(directory["counts"], dtype=np.int64)
    positions = directory["offsets"][:-1].copy()
    lookups = {}
    for row, (choice, bin_id) in enumerate(keys):
        if choice not in lookups:
            lookups[choice] = (row, [])
        lookups[choice][1].append(bin_id)
    lookups = [(choice, row, np.asarray(bins, dtype=np.int64))
               for choice, (row, bins) in lookups.items()]
    for start in range(0, total, _CHUNK):
        part = records[start:min(start + _CHUNK, total)]
        labels = np.empty(len(part), dtype=np.int32)
        for choice, first, bins in lookups:
            match = part["choice"] == choice
            labels[match] = first + np.searchsorted(bins, part["bin"][match])
        rows[part["id"]] = labels
        directory["estimated_counts"][:] += np.bincount(
            labels, weights=index.estimated_flags[part["id"]],
            minlength=count).astype(np.int64)
        order = np.argsort(labels, kind="stable")
        ordered = labels[order]
        starts = np.r_[0, np.flatnonzero(ordered[1:] != ordered[:-1]) + 1]
        stops = np.r_[starts[1:], len(ordered)]
        for lo, hi in zip(starts, stops):
            row = int(ordered[lo])
            position = int(positions[row])
            ids[position:position + hi - lo] = part["id"][order[lo:hi]]
            positions[row] += hi - lo
    return ids, rows, directory, count


def _sorted_records(index, records, total, folder, id_dtype):
    n = len(index.measured_ticks)
    ids = _array(folder, "ids", id_dtype, total, "w+")
    rows = _array(folder, "row_for_error", "<i4", n, "w+")
    rows.fill(-1)
    # Sparse files reserve at most one directory row per member, but only
    # actual groups are touched; they are truncated before publication.
    directory = {name: _array(folder, name, dtype, total + (name == "offsets"), "w+")
                 for name, dtype in _DIRECTORY.items()}
    count = 0
    previous = None
    for start in range(0, total, _CHUNK):
        part = records[start:min(start + _CHUNK, total)]
        different = np.empty(len(part), dtype=bool)
        different[0] = previous != (int(part[0]["choice"]), int(part[0]["bin"]))
        different[1:] = ((part["choice"][1:] != part["choice"][:-1]) |
                         (part["bin"][1:] != part["bin"][:-1]))
        starts = np.flatnonzero(different)
        end = count + len(starts)
        new = part[starts]
        directory["constraints"][count:end] = np.where(
            new["choice"] == len(index.constraints), _UNKNOWN, new["choice"])
        directory["bins"][count:end] = new["bin"]
        directory["offsets"][count:end] = start + starts
        labels = np.cumsum(different, dtype=np.int64) + count - 1
        if int(labels[-1]) >= np.iinfo(np.int32).max:
            raise ValueError("too many distinct delta groups for this rule")
        member_ids = part["id"]
        ids[start:start + len(part)] = member_ids
        rows[member_ids] = labels
        flags = index.estimated_flags[member_ids]
        first, last = int(labels[0]), int(labels[-1])
        directory["estimated_counts"][first:last + 1] += np.bincount(
            labels - first, weights=flags, minlength=last - first + 1).astype(np.int64)
        count = end
        previous = int(part[-1]["choice"]), int(part[-1]["bin"])
    directory["offsets"][count] = total
    for start in range(0, count, _CHUNK):
        stop = min(start + _CHUNK, count)
        directory["counts"][start:stop] = np.diff(directory["offsets"][start:stop + 1])
    return ids, rows, directory, count


def _review_snapshot(index, group_data, folder, job):
    """Create mutable review maps and all page prefixes in the child."""
    meta, arrays = group_data
    n, g = len(index.measured_ticks), meta["groups"]
    statuses = _array(folder, "statuses", "|b1", n, "w+")
    waived = _array(folder, "waived", "<i8", g, "w+")
    ids, rows = arrays["ids"], arrays["row_for_error"]
    _progress(job, "Reading current review statuses")
    for start in range(0, len(ids), _CHUNK):
        members = ids[start:start + _CHUNK]
        current = _read_status(index.db, index.ci, members)
        statuses[members] = current
        if np.any(current):
            np.add.at(waived, rows[members[current]], 1)
    offsets = _array(folder, "page_offsets", "<i8", g + 1, "w+")
    offsets[0] = 0
    running = 0
    for start in range(0, g, _CHUNK):
        stop = min(start + _CHUNK, g)
        counts = arrays["counts"][start:stop]
        sizes = (counts + _PAGE_CHUNK - 1) // _PAGE_CHUNK
        offsets[start + 1:stop + 1] = running + np.cumsum(sizes, dtype=np.int64)
        if len(sizes):
            running = int(offsets[stop])
    prefix = _array(folder, "page_prefix", "<i8", running, "w+")
    _progress(job, "Preparing filtered delta pages")
    for row in range(g):
        lo, hi = int(arrays["offsets"][row]), int(arrays["offsets"][row + 1])
        out = int(offsets[row])
        before = 0
        # Membership chunks align with page chunks to preserve reduceat.
        for start in range(lo, hi, _CHUNK):
            members = ids[start:min(start + _CHUNK, hi)]
            page_starts = np.arange(0, len(members), _PAGE_CHUNK)
            counts = np.add.reduceat(statuses[members], page_starts, dtype=np.int64)
            sums = before + np.cumsum(counts, dtype=np.int64)
            prefix[out:out + len(sums)] = sums
            out += len(sums)
            before = int(sums[-1])
    _flush(statuses, waived, offsets, prefix)
    return {"page_count": running}


def _restore_groups(index, folder, key, scratch, automatic, review_meta):
    loaded = _load_group_arrays(folder, key, len(index.measured_ticks))
    if loaded is None:
        raise ValueError("delta group cache is absent or stale")
    meta, arrays = loaded
    group = DeltaGroups.__new__(DeltaGroups)
    group.index, group.step_ticks, group.mode = index, key["step"], key["mode"]
    group.auto_step = automatic
    group._ids = arrays["ids"]
    for name in _DIRECTORY:
        setattr(group, "_" + name, arrays[name])
    group._row_for_error = arrays["row_for_error"]
    group.total = meta["total"]
    # Keep a scalar in the manifest/child result; do not scan a potentially
    # million-row group directory while restoring on the viewer thread.
    group.estimated_total = int(review_meta["estimated_total"])
    g, n = meta["groups"], len(index.measured_ticks)
    group._statuses = _array(scratch, "statuses", "|b1", n, "r+")
    group._waived = _array(scratch, "waived", "<i8", g, "r+")
    group._page_offsets = _array(scratch, "page_offsets", "<i8", g + 1, "r+")
    group._page_prefix = _array(scratch, "page_prefix", "<i8",
                               review_meta["page_count"], "r+")
    group._revision = 0
    group._descriptors, group._visible_rows = OrderedDict(), {}
    group._persistent = True
    group._scratch_cleanup = weakref.finalize(group, shutil.rmtree, scratch, True)
    return group


def _child(job):
    with open(job["request"], encoding="utf-8") as stream:
        request = json.load(stream)
    request.update(job)
    if pack_identity(request["pack"]) != request["pack_identity"]:
        raise ValueError("DRC pack changed before delta preprocessing; reopen it")
    db = IcePack(request["pack"], review=False,
                 review_path=request.get("review_path"))
    try:
        index = DeltaIndex(db, request["ci"], request["constraints"])
        if _rule_key(index)["pack"] != request["pack_identity"]:
            raise ValueError("DRC pack changed while opening delta preprocessing")
        if request["phase"] == "precision":
            arrays = [_array(request["arrays"], name, dtype,
                             len(db.checks[index.ci].errors), "r+")
                      for name, dtype in _MEASURE.items()]
            count = index._native_fallback(*arrays, progress=lambda text: _progress(request, text))
            _flush(*arrays)
            _check_identity(index)
            result = {"precision_fallback_count": count}
        elif request["phase"] == "measure":
            _build_measure(index, request)
            result = {"ok": True}
        else:
            if not load_measurements(index):
                raise ValueError("CD cache changed while preparing groups")
            members = (_Mask(request["members"], len(index.measured_ticks))
                       if request.get("members") else None)
            key = request["group_key"]
            folder, data = _build_group(index, key, members, request)
            review = _review_snapshot(index, data, request["scratch"], request)
            review["estimated_total"] = int(data[1]["estimated_counts"].sum())
            result = {"folder": folder, "review": review}
        _write_json(request["result"], result)
    finally:
        db.close()


def _run_child(request, cancelled=None, progress=None, process_callback=None):
    """Coordinator waits release the GIL; terminate stale work promptly."""
    with tempfile.TemporaryDirectory(prefix="floe-delta-job-") as jobdir:
        request_path = os.path.join(jobdir, "request.json")
        result_path = os.path.join(jobdir, "result.json")
        progress_path = os.path.join(jobdir, "progress.json")
        registry_path = os.path.join(jobdir, "staging.jsonl")
        _write_json(request_path, request)
        job = {"request": request_path, "result": result_path,
               "progress": progress_path, "staging_registry": registry_path}
        env = os.environ.copy()
        root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
        env["PYTHONPATH"] = root + os.pathsep + env.get("PYTHONPATH", "")
        # Each viewer needs one preprocessing core, not a BLAS thread pool.
        env["OPENBLAS_NUM_THREADS"] = env["OMP_NUM_THREADS"] = "1"
        with open(os.path.join(jobdir, "output.log"), "w+b") as log:
            process = subprocess.Popen(
                [sys.executable, "-m", "floe.drc_delta_cache", json.dumps(job)],
                stdin=subprocess.DEVNULL, stdout=log, stderr=log, env=env)
            with _CHILD_LOCK:
                _CHILDREN.add(process)
            if process_callback is not None:
                process_callback(process.pid)
            latest = None
            try:
                while process.poll() is None:
                    _cancel(cancelled)
                    if progress is not None:
                        try:
                            message = _read_json(progress_path).get("text")
                        except (OSError, ValueError):
                            message = None
                        if message and message != latest:
                            progress(message)
                            latest = message
                    time.sleep(0.025)
                _cancel(cancelled)
                if process.returncode:
                    log.seek(0, os.SEEK_END)
                    log.seek(max(0, log.tell() - 5000))
                    detail = log.read().decode("utf-8", "replace").strip()
                    raise RuntimeError("delta preprocessing failed: " + detail)
                return _read_json(result_path)
            finally:
                _stop_child(process)
                with _CHILD_LOCK:
                    _CHILDREN.discard(process)
                if process_callback is not None:
                    process_callback(None)
                try:
                    with open(registry_path, encoding="utf-8") as stream:
                        folders = [json.loads(line) for line in stream]
                except (OSError, ValueError):
                    folders = []
                for folder in folders:
                    shutil.rmtree(folder, ignore_errors=True)


def process_measure(index, cancelled=None, progress=None, process_callback=None,
                    *, backend="auto", jobs=None):
    """Ensure a persistent measurement cache without computing in this process."""
    _cancel(cancelled)
    if backend not in ("auto", "rust", "python"):
        raise ValueError("unknown DRC preprocessing backend %r" % backend)
    from .drc_native import find_binary, worker_count
    jobs = worker_count(jobs)
    if not len(index.db.checks[index.ci].errors):
        return index.measure(cancelled=cancelled)
    if load_measurements(index):
        return index
    binary = find_binary(required=backend == "rust") if backend != "python" else None
    if binary is not None:
        _build_measure(index, {"native_binary": binary, "jobs": jobs},
                       cancelled, progress, process_callback)
        _cancel(cancelled)
        return index
    if progress is not None:
        progress("Preparing CD measurements in a Python process" +
                 (" (compatible Rust helper unavailable)" if backend == "auto" else ""))
    _run_child({"phase": "measure", "pack": os.path.abspath(index.db.path),
                "pack_identity": _rule_key(index)["pack"],
                "ci": index.ci, "constraints": index.constraints},
               cancelled, progress, process_callback)
    _cancel(cancelled)
    if not load_measurements(index):
        raise ValueError("CD cache is absent or stale after preprocessing")
    return index


def prepare_group_cache(index, step=None, mode="absolute"):
    """CLI-only immutable groups: no disposable review snapshot or child."""
    if index.measured_ticks is None:
        raise ValueError("measure the rule before preparing delta groups")
    if not len(index.measured_ticks):
        return len(index.group(step, mode=mode))
    key, _automatic = _group_key(index, step, mode, "all")
    _folder, data = _build_group(index, key, None, {})
    return data[0]["groups"]


def process_group(index, step=None, mode="absolute", cluster=None,
                  cancelled=None, progress=None, process_callback=None):
    """Group from disk and return ready-to-page maps, without an O(N) restore."""
    _cancel(cancelled)
    if not len(index.db.checks[index.ci].errors):
        return index.group(step, cluster=cluster, mode=mode, cancelled=cancelled)
    if index.measured_ticks is None or not hasattr(index, "_measurement_cache"):
        process_measure(index, cancelled, progress, process_callback)
    scratch = tempfile.mkdtemp(prefix="floe-delta-review-")
    transferred = False
    try:
        members, membership_key = _snapshot_members(
            cluster, len(index.measured_ticks), scratch, cancelled)
        key, automatic = _group_key(index, step, mode, membership_key)
        if progress is not None:
            progress("Preparing delta groups in a separate process")
        result = _run_child(
            {"phase": "group", "pack": os.path.abspath(index.db.path),
             "pack_identity": _rule_key(index)["pack"],
             "ci": index.ci, "constraints": index.constraints,
             "review_path": getattr(index.db, "_waive_path", None),
             "group_key": key, "members": members, "scratch": scratch},
            cancelled, progress, process_callback)
        _cancel(cancelled)
        groups = _restore_groups(index, result["folder"], key, scratch,
                                  automatic, result["review"])
        transferred = True
        return groups
    finally:
        if not transferred:
            shutil.rmtree(scratch, ignore_errors=True)


if __name__ == "__main__":
    _child(json.loads(sys.argv[1]))
