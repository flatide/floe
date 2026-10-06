"""Latest-request CD-delta grouping with one reusable numeric rule index."""

import copy
import threading

from .drc_delta import DeltaIndex, DeltaQueryCancelled


class DeltaWorker:
    """One worker, one pending request, one result and one measured rule.

    ``step`` is an integer number of 0.00001 units (``parse_step`` output),
    or None for a whole-rule automatic step targeting ten groups per
    criterion. The result reports the actual ``step_ticks`` used.
    ``poll`` consumes ``(key, DeltaGroups or None, error string or None)``.
    Constraint metadata is copied on submission; clusters are immutable
    membership snapshots. Databases remain alive while being measured, so
    callers must not explicitly close a database still used by this worker.

    Step/mode/cluster changes cancel obsolete grouping but let an ongoing
    measurement of the same rule finish. Re-decoding millions of errors on
    each step edit would otherwise prevent the UI from making progress.
    """

    def __init__(self):
        self._condition = threading.Condition()
        self._generation = 0
        self._scope_generation = 0
        self._scope = None
        self._pending = None
        self._result = None
        self._closed = False
        self._thread = threading.Thread(target=self._run,
                                        name="drc-delta", daemon=True)
        self._thread.start()

    def submit(self, key, db, ci, constraints, step, mode="absolute",
               cluster=None):
        """Replace the pending request; preserve same-rule measurements."""
        constraints = copy.deepcopy(tuple(constraints or ()))
        ci = int(ci)
        with self._condition:
            if self._closed:
                return
            scope = self._scope
            if scope is None or scope[0] is not db or scope[1] != ci \
                    or scope[2] != constraints:
                self._scope_generation += 1
                self._scope = (db, ci, constraints)
            self._generation += 1
            self._pending = (self._generation, self._scope_generation,
                             key, db, ci, constraints, step, mode, cluster)
            self._result = None
            self._condition.notify()

    def poll(self):
        with self._condition:
            result, self._result = self._result, None
            return result

    def cancel(self):
        """Cancel all work and release the current measurement scope."""
        with self._condition:
            self._generation += 1
            self._scope_generation += 1
            self._scope = self._pending = self._result = None
            self._condition.notify()

    def close(self):
        """Stop asynchronously; never wait for geometry on the UI thread."""
        with self._condition:
            self._closed = True
            self._generation += 1
            self._scope_generation += 1
            self._scope = self._pending = self._result = None
            self._condition.notify()

    def _run(self):
        index = None
        indexed_scope = None
        while True:
            # Completed groups can retain the full numeric measurement
            # arrays. While idle, only the explicit result slot and current
            # index may own them; cancel() must not leave hidden locals alive.
            result = error = db = constraints = cluster = key = None
            with self._condition:
                while self._pending is None and not self._closed:
                    if indexed_scope != self._scope_generation:
                        index = None
                        indexed_scope = None
                    self._condition.wait()
                if self._closed:
                    return
                (generation, scope_generation, key, db, ci, constraints,
                 step, mode, cluster) = self._pending
                self._pending = None

            def measure_cancelled():
                return (self._closed
                        or scope_generation != self._scope_generation)

            def grouping_cancelled():
                return self._closed or generation != self._generation

            result, error = None, None
            try:
                if indexed_scope != scope_generation:
                    # Release the old rule before allocating the next one.
                    index = None
                    indexed_scope = None
                    index = DeltaIndex(db, ci, constraints)
                    index.measure(cancelled=measure_cancelled)
                    if measure_cancelled():
                        raise DeltaQueryCancelled()
                    indexed_scope = scope_generation
                if grouping_cancelled():
                    continue
                result = index.group(step, cluster=cluster, mode=mode,
                                     cancelled=grouping_cancelled)
            except DeltaQueryCancelled:
                continue
            except Exception as exc:
                error = str(exc)
            with self._condition:
                if not grouping_cancelled():
                    self._result = (key, result, error)
