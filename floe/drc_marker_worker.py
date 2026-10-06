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


# Larger packed rules run in a spawned process: Python geometry refinement
# must never contend for the GTK thread's GIL. Small/in-memory fixtures retain
# the lightweight thread backend.
LARGE_RULE = 100_000


def export_members(member, directory, count=None):
    """Serialize membership to mmap files; never pickle a rule-sized array.

    Caller owns directory lifetime and invokes this off the UI thread.
    Returned dictionaries contain only paths, dimensions and scalar values.
    """
    import os
    import uuid
    import numpy as np
    if member is None:
        return None
    os.makedirs(directory, exist_ok=True)

    def array_spec(array):
        if isinstance(array, np.memmap) and array.flags.c_contiguous:
            base = array
            while isinstance(base.base, np.memmap):
                base = base.base
            offset = int(base.offset) + int(array.ctypes.data - base.ctypes.data)
            return dict(path=os.path.abspath(array.filename), offset=offset,
                        dtype=array.dtype.str, shape=list(array.shape))
        path = os.path.join(directory, uuid.uuid4().hex + '.npy')
        np.save(path, array, allow_pickle=False)
        mapped = np.load(path, mmap_mode='r', allow_pickle=False)
        return dict(path=path, offset=int(mapped.offset), dtype=mapped.dtype.str,
                    shape=list(mapped.shape))

    owner = getattr(member, '_owner', None)
    if owner is not None and hasattr(owner, '_row_for_error'):
        return dict(kind='rows', array=array_spec(owner._row_for_error), row=int(member._row))
    if hasattr(member, '_starts') and hasattr(member, '_stops'):
        return dict(kind='intervals', starts=array_spec(member._starts), stops=array_spec(member._stops))
    if hasattr(member, 'indices'):
        return dict(kind='indices', array=array_spec(member.indices))
    if count is None:
        raise TypeError('membership snapshot requires the rule error count')
    path = os.path.join(directory, uuid.uuid4().hex + '.npy')
    out = np.lib.format.open_memmap(path, mode='w+', dtype=np.uint8, shape=(count,))
    for start in range(0, count, 1 << 16):
        end = min(count, start + (1 << 16))
        out[start:end] = member.mask(start, end - start)
    out.flush()
    return dict(kind='bitmap', array=array_spec(out))


class _MappedMembers:
    def __init__(self, spec):
        import json
        import numpy as np
        self.kind, self.row = spec['kind'], spec.get('row')
        self.cache_key = json.dumps(spec, sort_keys=True)

        def mapped(meta):
            shape = tuple(meta['shape'])
            if not all(isinstance(n, int) and n >= 0 for n in shape):
                raise ValueError('invalid membership shape')
            if not all(shape):
                return np.empty(shape, dtype=meta['dtype'])
            return np.memmap(meta['path'], mode='r', offset=meta['offset'],
                             dtype=meta['dtype'], shape=shape)
        if self.kind == 'intervals':
            self.starts, self.stops = mapped(spec['starts']), mapped(spec['stops'])
        else:
            self.array = mapped(spec['array'])

    def contains_many(self, indices):
        import numpy as np
        if self.kind == 'intervals':
            if not len(self.starts):
                return np.zeros(len(indices), dtype=bool)
            k = np.searchsorted(self.starts, indices, side='right') - 1
            return (k >= 0) & (indices < self.stops[np.maximum(k, 0)])
        if self.kind == 'indices':
            if not len(self.array):
                return np.zeros(len(indices), dtype=bool)
            k = np.searchsorted(self.array, indices)
            return (k < len(self.array)) & (self.array[np.minimum(k, len(self.array)-1)] == indices)
        return self.array[indices] == (self.row if self.kind == 'rows' else 1)

    def contains(self, ei):
        import numpy as np
        return bool(self.contains_many(np.asarray([ei], dtype=np.int64))[0])

    def mask(self, start, count):
        import numpy as np
        if self.kind in ('rows', 'bitmap'):
            out = np.zeros(count, dtype=bool)
            stop = min(start + count, len(self.array))
            if stop > start:
                out[:stop-start] = self.array[start:stop] == (self.row if self.kind == 'rows' else 1)
            return out
        return self.contains_many(np.arange(start, start + count, dtype=np.int64))


def import_members(spec):
    return None if spec is None else _MappedMembers(spec)


def _process_main(connection, generation):
    from .drc import IcePack
    from .drc_analysis import pack_identity
    db = index = identity = None
    try:
        while True:
            request = connection.recv()
            if request is None:
                return
            token, path, expected_identity, review_path, query = request
            cancelled = lambda: generation.value != token
            try:
                current = (path, review_path, pack_identity(path))
                if expected_identity is not None and current[2] != expected_identity:
                    raise ValueError('DRC pack changed; reload the results database')
                if current != identity:
                    if db is not None:
                        db.close()
                    db = IcePack(path, review=False, review_path=review_path)
                    index, identity = MarkerIndex(db, spatial=True), current
                _check = cancelled()
                if _check:
                    raise MarkerQueryCancelled()
                members = query.get('members')
                if members is not None:
                    query['members'] = {ci: import_members(spec) for ci, spec in members.items()}
                markers = index.query(cancelled=cancelled, **query)
                connection.send((token, markers, None))
            except MarkerQueryCancelled:
                connection.send((token, None, None))
            except Exception as exc:
                connection.send((token, None, str(exc)))
    except (EOFError, BrokenPipeError, OSError):
        pass
    finally:
        if db is not None:
            db.close()
        connection.close()


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
        self._process = self._pipe = self._process_generation = None
        self._snapshot_dir = None
        self._member_specs = {}
        self._thread = threading.Thread(target=self._run,
                                        name="drc-markers", daemon=True)
        self._thread.start()

    def submit(self, key, db, query):
        with self._condition:
            if self._closed:
                return
            self._generation += 1
            if self._process_generation is not None:
                self._process_generation.value = self._generation
            self._pending = (self._generation, key, db, dict(query))
            self._result = None
            self._condition.notify()

    def cancel(self):
        with self._condition:
            self._generation += 1
            if self._process_generation is not None:
                self._process_generation.value = self._generation
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
            if self._process_generation is not None:
                self._process_generation.value = self._generation
            self._pending = None
            self._result = None
            self._condition.notify()

    def _large(self, db, query):
        from .drc import IcePack
        if not isinstance(db, IcePack):
            return False
        checks = query.get('checks')
        return any(len(db.checks[ci].errors) >= LARGE_RULE
                   for ci in (range(len(db.checks)) if checks is None else checks)
                   if 0 <= ci < len(db.checks))

    def _process_query(self, generation, db, query, cancelled):
        import multiprocessing
        import tempfile
        import weakref
        if self._process is None or not self._process.is_alive():
            self._stop_process()
            context = multiprocessing.get_context('spawn')
            parent, child = context.Pipe()
            shared = context.Value('q', self._generation)
            process = context.Process(target=_process_main, args=(child, shared),
                                      name='drc-markers-process', daemon=True)
            self._pipe, self._process_generation, self._process = parent, shared, process
            process.start()
            child.close()
        if self._snapshot_dir is None:
            self._snapshot_dir = tempfile.TemporaryDirectory(prefix='floe-marker-members-')
        query = dict(query)
        if query.get('members') is not None:
            snapshots = {}
            for ci, member in query['members'].items():
                entry = self._member_specs.get(id(member))
                if entry is not None and entry[0]() is member:
                    spec = entry[1]
                else:
                    spec = export_members(member, self._snapshot_dir.name,
                                          len(db.checks[ci].errors))
                    try:
                        self._member_specs[id(member)] = (weakref.ref(member), spec)
                    except TypeError:
                        pass
                snapshots[ci] = spec
            self._member_specs = {k: v for k, v in self._member_specs.items() if v[0]() is not None}
            query['members'] = snapshots
        if cancelled():
            raise MarkerQueryCancelled()
        self._pipe.send((generation, db.path, getattr(db, '_analysis_identity', None),
                         getattr(db, '_waive_path', None), query))
        close_started = None
        while True:
            if self._pipe.poll(0.05):
                token, result, error = self._pipe.recv()
                if cancelled() or token != generation:
                    raise MarkerQueryCancelled()
                if error is not None:
                    raise RuntimeError(error)
                return result
            if not self._process.is_alive():
                raise RuntimeError('DRC marker process exited unexpectedly')
            if self._closed:
                import time
                if close_started is None:
                    close_started = time.monotonic()
                elif time.monotonic() - close_started > 2:
                    self._stop_process()
                    raise MarkerQueryCancelled()
            # Cancellation is cooperative: the child checks the shared
            # generation during scans/builds. Drain its bounded reply before
            # sending the newest request, preserving one in-flight job.

    def _stop_process(self):
        process, self._process = self._process, None
        if process is not None:
            if process.is_alive():
                process.terminate()
            process.join(timeout=1)
        pipe, self._pipe = self._pipe, None
        if pipe is not None:
            pipe.close()
        self._process_generation = None

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
                    self._stop_process()
                    if self._snapshot_dir is not None:
                        self._snapshot_dir.cleanup()
                    return
                generation, key, db, query = self._pending
                self._pending = None

            def cancelled():
                return self._closed or generation != self._generation

            result, error = None, None
            try:
                if self._large(db, query):
                    result = self._process_query(generation, db, query, cancelled)
                else:
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
