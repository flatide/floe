"""Latest-view-only marker queries; GTK never waits for a dense DRC scan."""

import threading

from .drc_markers import MarkerIndex, MarkerQueryCancelled


class SelectedMembers:
    """Immutable, bounded selection snapshot implementing the cluster mask."""

    def __init__(self, indices, cluster=None):
        import numpy as np
        self.indices = np.asarray(sorted(set(
            ei for ei in indices if cluster is None or cluster.contains(ei))),
            dtype=np.int64)

    def mask(self, start, count):
        import numpy as np
        out = np.zeros(count, dtype=bool)
        lo, hi = np.searchsorted(self.indices, [start, start + count])
        out[self.indices[lo:hi] - start] = True
        return out


class MarkerWorker:
    """One daemon thread, one pending request, one bounded result slot.

    Query arguments are snapshots. The database stays alive until its query
    finishes; MarkerIndex reads geometry without using IcePack's GUI cache.
    Callers must not explicitly close a database while it is in use here.
    """

    def __init__(self):
        self._condition = threading.Condition()
        self._generation = 0
        self._pending = None
        self._result = None
        self._closed = False
        self._thread = threading.Thread(target=self._run,
                                        name="drc-markers", daemon=True)
        self._thread.start()

    def submit(self, key, db, query):
        with self._condition:
            if self._closed:
                return
            self._generation += 1
            self._pending = (self._generation, key, db, dict(query))
            self._result = None
            self._condition.notify()

    def cancel(self):
        with self._condition:
            self._generation += 1
            self._pending = None
            self._result = None

    def poll(self):
        with self._condition:
            result, self._result = self._result, None
            return result

    def close(self):
        with self._condition:
            self._closed = True
            self._generation += 1
            self._pending = None
            self._result = None
            self._condition.notify()

    def _run(self):
        index = None
        indexed_db = None
        while True:
            # Query keys and membership snapshots can own a delta group's
            # whole numeric index. Keep only the explicit result slot while
            # idle, so poll/cancel releases old scopes without another query.
            key = db = query = result = error = None
            with self._condition:
                while self._pending is None and not self._closed:
                    self._condition.wait()
                if self._closed:
                    return
                generation, key, db, query = self._pending
                self._pending = None

            def cancelled():
                return self._closed or generation != self._generation

            result, error = None, None
            try:
                if db is not indexed_db:
                    index = MarkerIndex(db)
                    indexed_db = db
                result = index.query(cancelled=cancelled, **query)
            except MarkerQueryCancelled:
                continue
            except Exception as exc:
                error = str(exc)
            with self._condition:
                if not cancelled():
                    self._result = (key, result, error)
