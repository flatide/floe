"""Bounded Rust CD measurement over existing, read-only DRC packs.

Python compiles SVRF statement chains once. Rust receives only numeric
predicates and fills the same immutable measurement arrays as Python.
"""

from functools import lru_cache
import json
import operator
import os
import struct
import subprocess
import tempfile
import time

from .drc_delta import _cancel


PROTOCOL = 1
_METRICS = {name: number for number, name in enumerate(
    ("width", "space", "notch", "enclosure", "overlap", "extension", "length", "area"))}
_OPS = {name: number for number, name in enumerate(("<", "<=", ">", ">=", "==", "!="))}
_PREDICATES = {predicate: number for number, predicate in enumerate(
    (operator.lt, operator.le, operator.gt, operator.ge, operator.eq, operator.ne))}


def worker_count(jobs=None):
    if jobs is None:
        jobs = os.environ.get("FLOE_DRC_JOBS")
    if jobs is None:
        return min(4, os.cpu_count() or 1)
    try:
        value = int(jobs)
        if isinstance(jobs, float) or isinstance(jobs, bool) or not 1 <= value <= 256:
            raise ValueError
    except (TypeError, ValueError, OverflowError):
        raise ValueError("DRC worker count must be an integer from 1 to 256") from None
    return value


@lru_cache(maxsize=8)
def _compatible(path, size, mtime_ns, ctime_ns):
    try:
        result = subprocess.run([path, "drc-measure", "--protocol"],
                                stdin=subprocess.DEVNULL, capture_output=True,
                                text=True, timeout=5)
        return result.returncode == 0 and result.stdout.strip() == str(PROTOCOL)
    except (OSError, subprocess.SubprocessError):
        return False


def find_binary(required=False):
    from .vfsclient import find_binary as locate
    try:
        path = locate()
        stat = os.stat(path)
    except (OSError, RuntimeError) as exc:
        if required:
            raise RuntimeError("Rust DRC preprocessing unavailable: " + str(exc)) from exc
        return None
    if _compatible(path, stat.st_size, stat.st_mtime_ns, stat.st_ctime_ns):
        return path
    if required:
        raise RuntimeError("floe-index does not support Rust DRC preprocessing; "
                           "rebuild the bundled Rust binaries or use --backend python")
    return None


def write_plan(index, path):
    """Small versioned LE plan; no rule text, coordinates or error IDs copied."""
    selector = index._selector
    with open(path, "wb") as stream:
        stream.write(b"FDRCMS01")
        stream.write(struct.pack("<BI", selector._uncertain_alternatives,
                                 len(selector._chains)))
        for metric, representative, predicates, options in selector._chains:
            ci, op, _original, bound = representative
            ticks = index.bound_ticks[ci]
            stream.write(struct.pack("<BiBdBqBI", _METRICS[metric], ci, _OPS[op],
                                     bound, ticks is not None, ticks or 0,
                                     bool(options), len(predicates)))
            for predicate, bound in predicates:
                stream.write(struct.pack("<Bd", _PREDICATES[predicate], bound))


def measure(index, folder, binary, jobs=None, cancelled=None, progress=None,
            process_callback=None):
    """Fill raw arrays; caller owns staging cleanup and atomic publication."""
    from .drc_delta_cache import _CHILDREN, _CHILD_LOCK, _stop_child
    _cancel(cancelled)
    jobs = worker_count(jobs)
    with tempfile.TemporaryDirectory(prefix="floe-native-delta-") as temporary:
        plan = os.path.join(temporary, "plan.bin")
        progress_path = os.path.join(temporary, "progress.json")
        write_plan(index, plan)
        command = [binary, "drc-measure", os.path.abspath(index.db.path),
                   "--rule-index", str(index.ci), "--plan", plan, "--out", folder,
                   "--jobs", str(jobs), "--progress", progress_path]
        _cancel(cancelled)
        with open(os.path.join(temporary, "output.log"), "w+b") as log:
            process = subprocess.Popen(command, stdin=subprocess.DEVNULL,
                                       stdout=log, stderr=log)
            with _CHILD_LOCK:
                _CHILDREN.add(process)
            latest = None
            try:
                if process_callback is not None:
                    process_callback(process.pid)
                if progress is not None:
                    progress("CD measurement 0 / %d (Rust, up to %d workers)" %
                             (len(index.db.checks[index.ci].errors), jobs))
                while process.poll() is None:
                    _cancel(cancelled)
                    if progress is not None:
                        try:
                            with open(progress_path, encoding="utf-8") as stream:
                                message = json.load(stream).get("text")
                        except (OSError, ValueError, AttributeError):
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
                    raise RuntimeError("Rust CD preprocessing failed: " + detail)
                if progress is not None:
                    count = len(index.db.checks[index.ci].errors)
                    progress("CD measurement %d / %d (Rust)" % (count, count))
            finally:
                _stop_child(process)
                with _CHILD_LOCK:
                    _CHILDREN.discard(process)
                if process_callback is not None:
                    process_callback(None)
