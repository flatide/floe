"""The GTK viewer's link to `floe2 gtk-service` (rust/floe2/src/service.rs,
docs/SHARED_APP_LAYER.ko.md P2): what the viewer decides about a source
that is not drawing - whether it opens as it is, its meta and layer table,
a deck's plan and composite spec, its layer properties - is the shared
Rust app layer's, asked over one JSON line per request. The viewer keeps
its widgets and its own session state (which layers are on).

`ServiceCache` is what floe/gui.py and floe_oracle/rust_render.py read of an
open source, the attributes floe.cache.Cache and the jobdeck DeckCache
gave them: src, dir (the cache folder, or the spec renderd opens), meta,
is_jobdeck, ids, mode, props_src, exists(), load(), is_stale(), close().
"""

import atexit
import collections
import json
import os
import queue
import subprocess
import sys
import threading
import time
import types


class ServiceError(RuntimeError):
    """A request the service refused: `kind` is its error kind (busy,
    input, cache, ...), the message what it said."""

    def __init__(self, kind, message):
        super().__init__(message)
        self.kind = kind


class Service:
    """One `floe2 gtk-service` process, started on the first request and
    again if it ended. A reader thread takes its stdout: replies go to the
    request waiting for them, the views' events (P4c) to `events()`."""

    def __init__(self, binary=None):
        self._binary = binary
        self._proc = None
        self._lock = threading.Lock()       # one request on the wire at a time
        self._cond = threading.Condition()  # replies and the reader's end
        self._replies = {}
        self._events = collections.deque()
        self._drc = {}      # DRC database handle -> its jobs' events
        self._seq = 0

    def _start(self):
        binary = self._binary
        if binary is None:
            from .vfsclient import find_floe2
            binary = find_floe2()
        proc = subprocess.Popen(
            [binary, "gtk-service"], stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, text=True, bufsize=1)
        self._proc = proc
        threading.Thread(target=self._read, args=(proc,), daemon=True,
                         name="floe2-gtk-service").start()

    def _read(self, proc):
        try:
            for line in proc.stdout:
                try:
                    message = json.loads(line)
                except ValueError:
                    continue
                with self._cond:
                    if message.get("event") == "drc":
                        q = self._drc.get(message.get("handle"))
                        if q is not None:
                            q.append(message)
                        else:
                            _discard_files([message])
                    elif "event" in message:
                        self._events.append(message)
                    else:
                        self._replies[message.get("id")] = message
                    self._cond.notify_all()
        except (OSError, ValueError):
            pass
        with self._cond:
            self._cond.notify_all()

    def request(self, op, **fields):
        with self._lock:
            if self._proc is None or self._proc.poll() is not None:
                self._start()
            proc = self._proc
            self._seq += 1
            seq = fields["id"] = self._seq
            fields["op"] = op
            try:
                proc.stdin.write(json.dumps(fields) + "\n")
                proc.stdin.flush()
            except OSError as exc:
                self._proc = None
                raise ServiceError("worker", "gtk-service: %s" % exc)
            with self._cond:
                while seq not in self._replies:
                    if proc.poll() is not None:
                        self._proc = None
                        raise ServiceError("worker", "gtk-service ended")
                    self._cond.wait(0.5)
                reply = self._replies.pop(seq)
            error = reply.get("error")
            if error is not None:
                raise ServiceError(error.get("kind", "worker"),
                                   error.get("message", "gtk-service error"))
            return reply.get("result")

    def events(self):
        """The events received since the last call, oldest first."""
        with self._cond:
            out = list(self._events)
            self._events.clear()
        return out

    def drc_watch(self, handle):
        """Keep the DRC jobs' events of database `handle`."""
        with self._cond:
            self._drc.setdefault(handle, collections.deque())

    def drc_events(self, handle, forget=False):
        """Database `handle`'s DRC job events since the last call."""
        with self._cond:
            q = self._drc.pop(handle, None) if forget else self._drc.get(handle)
            if q is None:
                return []
            out = list(q)
            q.clear()
        return out

    def close(self):
        with self._lock:
            proc, self._proc = self._proc, None
        if proc is not None and proc.poll() is None:
            try:
                proc.stdin.close()
                proc.wait(timeout=5)
            except (OSError, subprocess.TimeoutExpired):
                proc.kill()


# selections and groupings the viewer let go of, released at the next
# request: a finalizer must not talk to the service (a garbage collection
# can run inside a request, whose lock is not reentrant)
_DROPS = collections.deque()


def _flush_drops():
    while _DROPS:
        handle, op, key, value = _DROPS.popleft()
        try:
            service().request(op, handle=handle, **{key: value})
        except ServiceError:
            pass


def _discard_files(events):
    """Remove the marker files of events nobody will read."""
    for e in events:
        path = e.get("path")
        if path:
            try:
                os.unlink(path)
            except OSError:
                pass


_SERVICE = None
_SERVICE_LOCK = threading.Lock()


def service():
    """The viewer's service (one per process)."""
    global _SERVICE
    with _SERVICE_LOCK:
        if _SERVICE is None:
            _SERVICE = Service()
            # its spec folders go when it ends: stdin closed at exit
            atexit.register(_SERVICE.close)
        return _SERVICE


def is_deck_path(path):
    return bool(path) and str(path).lower().endswith(".jb")


def normalize_levels(ids):
    """A level selection as a sorted list of distinct ints, None for all
    levels (an empty selection is None too)."""
    if ids is None:
        return None
    out = sorted({int(i) for i in ids})
    return out or None


def readiness(path, ids=None):
    """{ready, current, busy}: whether `path` opens as it is (a layout's
    cache, or every drawable source of a deck's levels `ids`), whether a
    layout's cache is current, and who rebuilds it when it does not
    (floe2 gtk-service `ready`)."""
    return service().request("ready", source=os.path.abspath(path),
                             levels=normalize_levels(ids))


def level_rows(path):
    """The deck's load dialog rows: {level, name, chips, instances,
    sources} per mask level."""
    return service().request("level_rows", source=os.path.abspath(path))


class ServiceCache:
    """An open layout or jobdeck, answered by the service (see the module
    docstring)."""

    def __init__(self, path, ids=None, mode="level"):
        self.src = os.path.abspath(path)
        self.is_jobdeck = is_deck_path(self.src)
        self.ids = normalize_levels(ids) if self.is_jobdeck else None
        self.mode = mode
        self.dir = None
        self.meta = None
        self.props_src = self.src
        self.catalog = None
        self.layout_mode = None     # a KLayout worker option, unused
        self._props = []
        self._handle = None
        self._loaded_mtime = None
        self._visibility = {}       # a deck's session choices per view

    # ---- the Cache protocol ------------------------------------------
    def exists(self):
        return os.path.isfile(self.src)

    def load(self):
        result = service().request("open", source=self.src, levels=self.ids,
                                   mode=self.mode)
        previous, self._handle = self._handle, result.get("handle")
        if previous is not None:
            self._close_handle(previous)
        self.dir = result["dir"]
        self.meta = result["meta"]
        self.props_src = result["props_src"]
        self._props = [((int(p["layer"][0]), int(p["layer"][1])), p["color"],
                        p["fill"], p["name"], p["visibility"], p["width"])
                       for p in result.get("props") or []]
        dirs = result.get("source_dirs") or {}
        self.catalog = types.SimpleNamespace(infos={
            tc: types.SimpleNamespace(cache_dir=d) for tc, d in dirs.items()})
        if result.get("unlisted"):
            sys.stderr.write(result["unlisted"] + "\n")
        try:
            self._loaded_mtime = int(os.stat(self.src).st_mtime)
        except OSError:
            self._loaded_mtime = None
        return self.meta

    def is_stale(self):
        try:
            st = os.stat(self.src)
        except OSError:
            return True
        if self.is_jobdeck:
            return int(st.st_mtime) != self._loaded_mtime
        src = (self.meta or {}).get("src") or {}
        return (st.st_size != src.get("size")
                or int(st.st_mtime) != src.get("mtime"))

    def layer_props(self):
        """The design-default layerprops rows, as floe.fillpat.
        parse_layerprops gave them: ((layer, datatype), color, fill, name,
        visibility, width)."""
        return list(self._props)

    def close(self):
        handle, self._handle = self._handle, None
        if handle is not None:
            self._close_handle(handle)

    @staticmethod
    def _close_handle(handle):
        try:
            service().request("close", handle=handle)
        except ServiceError:
            pass

    # ---- a deck's views ----------------------------------------------
    def save_visibility(self, visible):
        """Remember exact leaf choices, independent of the panel mode."""
        scope = "layer" if self.mode == "layer" else "deck"
        self._visibility[scope] = {
            (r["layer"], r["datatype"]):
            (r["layer"], r["datatype"]) in visible
            for r in self.meta["layers"]}

    def restore_visibility(self, default):
        scope = "layer" if self.mode == "layer" else "deck"
        saved = self._visibility.get(scope, {})
        keys = {(r["layer"], r["datatype"]) for r in self.meta["layers"]}
        return {key for key in keys if saved.get(key, key in default)}

    def set_mode(self, mode):
        """Switch between the level view, the chip view and the source
        layer view: the deck is planned again for it."""
        previous = (self.mode, self.dir, self.meta, self.props_src,
                    self._props, self.catalog)
        self.mode = mode
        try:
            return self.load()
        except Exception:
            (self.mode, self.dir, self.meta, self.props_src, self._props,
             self.catalog) = previous
            raise

    def set_levels(self, ids):
        self.ids = normalize_levels(ids)
        return self.load()


# ---- layer properties (P2c) ----------------------------------------------

def _prop_rows(rows):
    return [{"layer": [int(k[0]), int(k[1])], "color": str(color),
             "fill": str(fill), "name": str(name or ""),
             "visibility": str(f1), "width": str(f2)}
            for k, color, fill, name, f1, f2 in rows]


def layerprops_read(path):
    """A layerprops file's rows: ((layer, datatype), color, fill, name,
    visibility, width) - floe.fillpat.parse_layerprops' shape."""
    return [((int(p["layer"][0]), int(p["layer"][1])), p["color"], p["fill"],
             p["name"], p["visibility"], p["width"])
            for p in service().request("layerprops_read",
                                       path=os.path.abspath(path))]


def layerprops_save(path, rows):
    """Write `rows` as a layerprops file at `path`."""
    service().request("layerprops_save", path=os.path.abspath(path),
                      rows=_prop_rows(rows))


def layerprops_publish(props_src, rows):
    """Publish `rows` as the design default `<props_src>.layerprops`; its
    path."""
    return service().request("layerprops_publish",
                             props_src=os.path.abspath(props_src),
                             rows=_prop_rows(rows))


# ---- DRC review (P2b: app-core drc::desktop) ----------------------------

STATUS_NONE = 0
STATUS_WAIVED = 1
_ERROR_PAGE = 256
_STATUS_PAGE = 4096


class DrcError(object):
    """One violation as the service sent it: kind 'p' (polygon) or 'e'
    (edge), its global 1-based number, points in um (floe/drc.py
    DrcError)."""
    __slots__ = ("kind", "num", "pts", "_db", "_ci", "_ei", "_cd")

    def __init__(self, kind, num, pts, db=None, ci=None, ei=None):
        self.kind = kind
        self.num = num
        self.pts = [tuple(p) for p in pts]
        self._db, self._ci, self._ei = db, ci, ei
        self._cd = None

    def bbox(self):
        xs = [p[0] for p in self.pts]
        ys = [p[1] for p in self.pts]
        return (min(xs), min(ys), max(xs), max(ys))

    def center(self):
        x0, y0, x1, y1 = self.bbox()
        return ((x0 + x1) / 2, (y0 + y1) / 2)

    def cd_segments(self):
        """The CD ruler segments, um 4-tuples (none for complex shapes)."""
        if self._db is None:
            return []
        return [tuple(s) for s in self._db._request(
            "drc_cd", check=self._ci, error=self._ei)]

    def cd_info(self, constraints=()):
        """The rule-aware answer for this violation (app-core
        drc::analysis): {rulers: [[x0, y0, x1, y1], ...] um, area: [x, y,
        text] | None, pick: {constraint, cd, estimated, reason, candidates,
        candidate_ticks, ticks: [measured, bound, delta] | None, ratio} |
        None}; the last one kept per constraint list."""
        constraints = list(constraints or ())
        key = json.dumps(constraints, sort_keys=True)
        if self._cd is not None and self._cd[0] == key:
            return self._cd[1]
        if self._db is not None and self._ci is not None:
            info = self._db._request("drc_cd_info", check=self._ci,
                                     error=self._ei, constraints=constraints)
        else:
            info = drc_cd_markers(constraints, [(self.kind, self.pts)])[0]
        self._cd = (key, info)
        return info


class _Errors(object):
    """A rule's errors, fetched by pages as they are read."""
    __slots__ = ("_db", "_ci", "_count", "_pages")

    def __init__(self, db, ci, count):
        self._db, self._ci, self._count = db, ci, count
        self._pages = {}

    def __len__(self):
        return self._count

    def _page(self, k):
        page = self._pages.get(k)
        if page is None:
            if len(self._pages) > 64:
                self._pages.pop(next(iter(self._pages)))
            rows = self._db._request("drc_errors", check=self._ci,
                                     start=k * _ERROR_PAGE,
                                     count=_ERROR_PAGE)
            page = [DrcError(kind, num, pts, self._db, self._ci,
                             k * _ERROR_PAGE + j)
                    for j, (kind, num, pts) in enumerate(rows)]
            self._pages[k] = page
        return page

    def __getitem__(self, i):
        if isinstance(i, slice):
            return [self[j] for j in range(*i.indices(self._count))]
        if i < 0:
            i += self._count
        if not 0 <= i < self._count:
            raise IndexError(i)
        return self._page(i // _ERROR_PAGE)[i % _ERROR_PAGE]

    def __iter__(self):
        for i in range(self._count):
            yield self[i]


class DrcCheck(object):
    __slots__ = ("name", "desc", "declared", "errors", "start")

    def __init__(self, db, ci, row):
        self.name = row["name"]
        self.desc = row["desc"]
        self.declared = row["declared"]
        self.start = row["start"]
        self.errors = _Errors(db, ci, row["count"])


class _Drc(object):
    """What both kinds of database share: the service handle, the rules
    and their errors."""
    packed = False

    def __init__(self, result):
        self._slots = {}
        self._visits = collections.deque()
        self._handle = result["handle"]
        self.path = result["path"]
        self.cell = result["cell"]
        self.total = result["total"]
        self.precision = result["precision"]
        self.checks = [DrcCheck(self, ci, row)
                       for ci, row in enumerate(result["checks"])]
        for line in result.get("notices") or []:
            sys.stderr.write(line + "\n")

    def _request(self, op, **fields):
        if self._handle is None:
            raise ServiceError("input", "the DRC database is closed")
        _flush_drops()
        return service().request(op, handle=self._handle, **fields)

    def close(self):
        """Let the service close it (and the pack's reader lock with it)."""
        handle, self._handle = self._handle, None
        if handle is not None:
            _discard_files(service().drc_events(handle, forget=True))
            for slot in self._slots.values():
                _discard_files([slot[1]] if slot[1] else [])
            self._slots = {}
            try:
                service().request("drc_close", handle=handle)
            except ServiceError:
                pass

    # ---- analysis jobs (feature/jobdeck-drc; app-core drc::analysis) ----
    def _start(self, kind, request_op, **fields):
        """Start the service's `kind` job (the earlier one of the kind is
        cancelled there); its events reach `poll(kind)`."""
        service().drc_watch(self._handle)
        job = self._request(request_op, **fields)["job"]
        old = self._slots.get(kind)
        if old is not None and old[1] is not None:
            _discard_files([old[1]])
        self._slots[kind] = [job, None]
        return job

    def _pump(self):
        if self._handle is None:
            return
        for e in service().drc_events(self._handle):
            if e.get("kind") == "visit":
                # the session's visit thread (no job slot)
                self._visits.append(e)
                continue
            slot = self._slots.get(e.get("kind"))
            if slot is None or e.get("job") != slot[0]:
                _discard_files([e])
                continue
            if slot[1] is not None:
                _discard_files([slot[1]])
            slot[1] = e

    def poll(self, kind):
        """The latest event of the current `kind` job, once."""
        self._pump()
        slot = self._slots.get(kind)
        if slot is None or slot[1] is None:
            return None
        event, slot[1] = slot[1], None
        if event.get("state") in ("done", "error", "cancelled"):
            self._slots.pop(kind, None)
        return event

    def cancel(self, kind):
        slot = self._slots.pop(kind, None)
        if slot is not None:
            if slot[1] is not None:
                _discard_files([slot[1]])
            try:
                self._request("drc_cancel", kind=kind)
            except ServiceError:
                pass

    def load_clusters(self, path=None):
        """The `<db>.clusters` sidecar (beside the db when `path` is None:
        None when there is none); ValueError naming the file and line."""
        try:
            reply = self._request("drc_clusters_load",
                                  path=None if path is None
                                  else os.path.abspath(path))
        except ServiceError as exc:
            raise ValueError(str(exc))
        if reply is None:
            return None
        return ClusterFile(self, reply)


class AsciiDrc(_Drc):
    """An ASCII results file read whole: rules and errors, no review."""


class PackDrc(_Drc):
    """A pack under review (floe/drc.py IcePack's surface): statuses read
    by pages and written through at once, notes, the waive and note files,
    spatial queries."""
    packed = True

    def __init__(self, result):
        super().__init__(result)
        self._counts = [tuple(c) for c in result["counts"]]
        self._status = {}
        self._cache_lock = threading.Lock()
        self.waive_path = result.get("waive_path")
        self._waive_path = self.waive_path
        self.note_path = result.get("note_path")
        self._reviewer = result.get("reviewer")
        self.read_only = bool(result.get("read_only"))
        self._flags = {}
        self._set_notes(result.get("notes") or [])

    def _set_notes(self, notes):
        self._notes = [(n["text"], sorted(n["members"])) for n in notes]
        self._note_of = {g: text for text, members in self._notes
                         for g in members}

    def _notes_reply(self, reply):
        self._set_notes(reply.get("notes") or [])
        for line in reply.get("notices") or []:
            sys.stderr.write(line + "\n")

    # ---- statuses ------------------------------------------------------
    def _status_chunk(self, ci, k):
        # the reviewers windows read statuses on their worker threads too:
        # the caches change under a lock (never held over a request)
        key = (ci, k)
        with self._cache_lock:
            got = self._status.get(key)
        if got is None:
            hexed = self._request("drc_status", check=ci,
                                  start=k * _STATUS_PAGE, count=_STATUS_PAGE)
            got = bytearray.fromhex(hexed)
            with self._cache_lock:
                if len(self._status) > 256:
                    self._status.pop(next(iter(self._status)))
                self._status[key] = got
        return got

    def get_status(self, ci, ei):
        return self._status_chunk(ci, ei // _STATUS_PAGE)[ei % _STATUS_PAGE]

    def set_statuses(self, ci, eis, value):
        """Set the status of rule `ci`'s errors `eis` (written at once)."""
        eis = [int(e) for e in eis]
        if not eis:
            return
        reply = self._request("drc_set_status", check=ci, errors=eis,
                              status=int(value) & 0xFF)
        self._counts[ci] = tuple(reply["counts"])
        with self._cache_lock:
            for ei in eis:
                chunk = self._status.get((ci, ei // _STATUS_PAGE))
                if chunk is not None:
                    chunk[ei % _STATUS_PAGE] = int(value) & 0xFF

    def set_status(self, ci, ei, value):
        self.set_statuses(ci, [ei], value)

    def status_counts(self, ci):
        return self._counts[ci]

    def status_page(self, ci, waived, start, limit):
        return self._request("drc_status_page", check=ci, waived=bool(waived),
                             start=int(start), limit=int(limit))

    def status_rank(self, ci, waived, ei):
        return self._request("drc_status_rank", check=ci,
                             waived=bool(waived), error=int(ei))

    def query_rect(self, x0_um, y0_um, x1_um, y1_um, cap=2000, checks=None,
                   waived=None, members=None):
        """Errors meeting the um rect; `members` {ci: scope} limits the one
        rule the query is limited to (a cluster, group or selection)."""
        fields = {}
        if members:
            (ci, scope), = members.items()
            fields = dict(check=int(ci), scope=scope.spec)
        rows = self._request(
            "drc_query", bbox=[x0_um, y0_um, x1_um, y1_um], cap=int(cap),
            checks=None if checks is None else [int(c) for c in checks],
            waived=waived, **fields)
        return [(ci, ei, DrcError(kind, num, pts, self, ci, ei))
                for ci, ei, kind, num, pts in rows]

    # ---- notes ---------------------------------------------------------
    def error_gid(self, ci, ei):
        return self.checks[ci].start + ei

    def get_note(self, ci, ei):
        return self._note_of.get(self.error_gid(ci, ei))

    def get_note_gid(self, gid):
        return self._note_of.get(gid)

    def set_note(self, gids, text):
        self._notes_reply(self._request("drc_set_note",
                                        gids=[int(g) for g in gids],
                                        text=text or ""))

    def clear_note(self, gids):
        self._notes_reply(self._request("drc_clear_note",
                                        gids=[int(g) for g in gids]))

    def notes_list(self):
        return list(self._notes)

    def note_export(self, dst):
        self._request("drc_note_export", path=os.path.abspath(dst))

    def note_import(self, src):
        reply = self._request("drc_note_import", path=os.path.abspath(src))
        self._notes_reply(reply)
        return reply["count"]

    def waive_export(self, dst):
        self._request("drc_waive_export", path=os.path.abspath(dst))

    def statuses_changed(self, ci=None, counts=None):
        """A service job rewrote statuses: drop what was read of them."""
        with self._cache_lock:
            self._status.clear()
        if ci is not None and counts is not None:
            self._counts[ci] = tuple(counts)

    def waive_import(self, src):
        reply = self._request("drc_waive_import", path=os.path.abspath(src))
        self._counts = [tuple(c) for c in reply["counts"]]
        with self._cache_lock:
            self._status.clear()
        return reply["waived"]

    def counts_refresh(self):
        """Every rule's (waived, total) again (a team job rewrote them)."""
        self._counts = [tuple(c) for c in self._request("drc_counts")]
        with self._cache_lock:
            self._status.clear()

    # ---- visited / not-required (app-core drc::team::flags) -------------
    def review_flags(self, ci, ei):
        """VISITED | NOT_REQUIRED of one error; a 4096-error chunk asked at
        a time; ServiceError when the flags cannot be read."""
        key = (ci, ei // _STATUS_PAGE)
        with self._cache_lock:
            got = self._flags.get(key)
        if got is None:
            n = len(self.checks[ci].errors)
            start = key[1] * _STATUS_PAGE
            got = self._request("drc_review_flags", check=ci, errors=list(
                range(start, min(n, start + _STATUS_PAGE))))
            with self._cache_lock:
                if len(self._flags) > 256:
                    self._flags.pop(next(iter(self._flags)))
                self._flags[key] = got
        return got[ei % _STATUS_PAGE]

    def review_flags_changed(self, ci=None):
        with self._cache_lock:
            if ci is None:
                self._flags.clear()
            else:
                for key in [k for k in self._flags if k[0] == ci]:
                    del self._flags[key]

    def review_counts(self, ci=None):
        return self._request("drc_review_counts",
                             check=None if ci is None else int(ci))

    @property
    def review_path(self):
        reply = self._request("drc_review_path")
        for line in reply.get("notices") or []:
            sys.stderr.write(line + "\n")
        return reply["path"]

    def mark_visited(self, ci, ei):
        """Queue one visit (written in order on the service's thread; the
        outcome comes back through `visit_events`)."""
        self._request("drc_visit", check=int(ci), error=int(ei))

    def visit_events(self):
        """(ci, ei, error message or None) of the visits written since."""
        self._pump()
        out = []
        while self._visits:
            e = self._visits.popleft()
            ci = e.get("check")
            self.review_flags_changed(ci)
            out.append((ci, e.get("error"),
                        e.get("message") if e.get("state") == "error" else None))
        return out


# ---- DRC analysis: the viewer's objects over the service's (feature/
# jobdeck-drc's drc_clusters / drc_delta / drc_points / drc_selection
# surfaces; the work is app-core drc::analysis)

class DrcScope(object):
    """Errors of one rule the service resolves: `spec` is the service's
    scope - {"cluster": k}, {"group": g, "row": r}, {"selection": s}, their
    union of keys an intersection, {"empty": True} none."""
    __slots__ = ("db", "ci", "spec", "__weakref__")

    def __init__(self, db, ci, spec):
        self.db, self.ci, self.spec = db, ci, dict(spec)

    def _scope(self, op, **fields):
        return self.db._request(op, check=self.ci, scope=self.spec, **fields)

    def count(self, waived=None):
        return self._scope("drc_scope_count", waived=waived)

    def status_counts(self):
        return self.count(True), self.count(None)

    def page(self, start, limit, waived=None):
        return self._scope("drc_scope_page", start=int(start),
                           limit=int(limit), waived=waived)

    def rank(self, ei, waived=None):
        return self._scope("drc_scope_rank", error=int(ei), waived=waived)

    def contains_many(self, eis):
        eis = [int(e) for e in eis]
        if not eis:
            return []
        return self._scope("drc_scope_contains", errors=eis)

    def contains(self, ei):
        return bool(self.contains_many([ei])[0])

    __contains__ = contains


class EmptyScope(DrcScope):
    """No error of a rule (the viewer's `Selection.empty`)."""
    __slots__ = ()

    def __init__(self, db, ci):
        super().__init__(db, ci, {"empty": True})

    def __len__(self):
        return 0

    def __bool__(self):
        return False

    def count(self, waived=None):
        return 0

    def page(self, start, limit, waived=None):
        return []

    def rank(self, ei, waived=None):
        return None

    def contains_many(self, eis):
        return [False for _ in eis]

    def contains(self, ei):
        return False

    __contains__ = contains


def intersect(*parts):
    """The intersection of scopes (None parts are the whole rule)."""
    parts = [p for p in parts if p is not None]
    if not parts:
        return None
    spec = {}
    for p in parts:
        if p.spec.get("empty"):
            return DrcScope(p.db, p.ci, {"empty": True})
        for key in ("cluster", "group", "row", "selection"):
            if key in p.spec:
                spec[key] = p.spec[key]
    return DrcScope(parts[0].db, parts[0].ci, spec)


class DrcCluster(DrcScope):
    """One cluster of a `<db>.clusters` file; its status counts as last
    asked (ClusterFile.prepare_counts)."""
    __slots__ = ("name", "total", "index", "_counts")

    def __init__(self, db, ci, index, name, total):
        super().__init__(db, ci, {"cluster": index})
        self.index, self.name, self.total = index, name, total
        self._counts = None

    def status_counts(self):
        if self._counts is None:
            self._counts = super().status_counts()
        return self._counts

    def _reset_status(self):
        self._counts = None


class ClusterFile(object):
    """A loaded clusters file: `rules` {ci: [DrcCluster, ...]}."""

    def __init__(self, db, reply):
        self.path = reply["path"]
        self.rules = {
            int(ci): [DrcCluster(db, int(ci), k, c["name"], c["total"])
                      for k, c in enumerate(clusters)]
            for ci, clusters in reply["rules"].items()}
        self._db = db

    def prepare_counts(self, ci):
        clusters = self.rules.get(ci)
        if not clusters:
            return
        counts = self._db._request("drc_cluster_counts", check=ci)
        for cluster, (waived, total) in zip(clusters, counts):
            cluster._counts = (waived, total)

    def status_changed(self, ci, eis=None):
        for cluster in self.rules.get(ci, ()):
            cluster._reset_status()

    def reset_status(self):
        for clusters in self.rules.values():
            for cluster in clusters:
                cluster._reset_status()


class DrcSelection(DrcScope):
    """A complete selection of one rule, kept by the service (any size):
    count, contains, pages and ranks are asked of it."""
    __slots__ = ("sid", "size", "total", "_seen")

    def __init__(self, db, ci, sid, size, total):
        super().__init__(db, ci, {"selection": sid})
        self.sid, self.size, self.total = sid, size, total
        self._seen = {}

    def __len__(self):
        return self.total

    def __bool__(self):
        return self.total > 0

    def count(self, waived=None):
        return self.total

    def page(self, start, limit, waived=None):
        return self.db._request("drc_selection_page", selection=self.sid,
                                start=int(start), limit=int(limit))

    def contains(self, ei):
        ei = int(ei)
        got = self._seen.get(ei)
        if got is None:
            if len(self._seen) > 4096:
                self._seen.clear()
            got = self._seen[ei] = super().contains(ei)
        return got

    __contains__ = contains

    def __getitem__(self, i):
        if isinstance(i, slice):
            start, stop, step = i.indices(self.total)
            return self.page(start, stop - start)[::step]
        if i < 0:
            i += self.total
        got = self.page(i, 1) if 0 <= i < self.total else []
        if not got:
            raise IndexError(i)
        return got[0]

    def __iter__(self):
        for start in range(0, self.total, 65536):
            for ei in self.page(start, 65536):
                yield ei

    def __del__(self):
        handle = getattr(self.db, "_handle", None)
        if handle is not None:
            _DROPS.append((handle, "drc_selection_drop", "selection", self.sid))


class ListMembers(object):
    """Explicit rule-local errors (grid edits) as a selection request."""
    __slots__ = ("indices",)

    def __init__(self, indices):
        self.indices = sorted(set(int(i) for i in indices))

    def __len__(self):
        return len(self.indices)


SelectionProgress = collections.namedtuple("SelectionProgress",
                                           "processed total count")
MarkerProgress = collections.namedtuple("MarkerProgress", "markers")


class PointMarkers(object):
    """A marker picture the service drew (app-core drc::analysis::points):
    RGBA bytes, and per pixel the error drawn there."""
    __slots__ = ("width", "height", "rgba", "_ids", "check", "visible_count",
                 "occupied_count", "processed_count", "total_count")

    def __init__(self, event, check):
        path = event["path"]
        try:
            with open(path, "rb") as f:
                data = f.read()
        finally:
            try:
                os.unlink(path)
            except OSError:
                pass
        if data[:8] != b"FLOEMRK1":
            raise ValueError("not a marker picture")
        w = int.from_bytes(data[8:12], "little")
        h = int.from_bytes(data[12:16], "little")
        counts = [int.from_bytes(data[16 + 8 * i:24 + 8 * i], "little")
                  for i in range(4)]
        if len(data) != 48 + w * h * 12:
            raise ValueError("marker picture is truncated")
        self.width, self.height, self.check = w, h, check
        (self.visible_count, self.occupied_count, self.processed_count,
         self.total_count) = counts
        self.rgba = data[48:48 + w * h * 4]
        self._ids = memoryview(data)[48 + w * h * 4:].cast("q")

    def error_id(self, x, y):
        """The rule-local error drawn at pixel (x, y), -1 none."""
        return self._ids[y * self.width + x]


def parse_step(text):
    """A grouping step typed by the user in 0.00001 ticks (ValueError)."""
    try:
        return service().request("drc_parse_step", text=str(text))
    except ServiceError as exc:
        raise ValueError(str(exc))


class _Worker(object):
    """The viewer's latest-request worker of one job kind: submit / poll /
    cancel / close over the service's job (feature/jobdeck-drc's worker
    threads and processes are the service's threads now)."""
    kind = None

    def __init__(self):
        self._db = None
        self._key = None
        self.progress = None

    def _submit(self, key, db, request_op, **fields):
        if self._db is not None and self._db is not db:
            self._db.cancel(self.kind)
        self._db, self._key = db, key
        self.progress = None
        try:
            db._start(self.kind, request_op, **fields)
        except ServiceError as exc:
            self._failed = (key, str(exc))
            return
        self._failed = None

    def _event(self):
        failed = getattr(self, "_failed", None)
        if failed is not None:
            self._failed = None
            return failed, None
        if self._db is None:
            return None, None
        return None, self._db.poll(self.kind)

    def cancel(self):
        if self._db is not None:
            self._db.cancel(self.kind)
        self._key = None
        self.progress = None

    def close(self):
        self.cancel()
        self._db = None


class SelectionWorker(_Worker):
    kind = "select"

    def submit(self, key, db, ci, bounds_um=None, membership=None,
               waived=None, previous=None, mode="replace"):
        fields = dict(check=int(ci), mode=mode, waived=waived,
                      bounds=None if bounds_um is None
                      else [float(v) for v in bounds_um])
        if isinstance(membership, ListMembers):
            fields["errors"] = membership.indices
        elif membership is not None:
            fields["scope"] = membership.spec
        if previous is not None and getattr(previous, "sid", None) is not None:
            fields["previous"] = previous.sid
        self._ci = ci
        self._submit(key, db, "drc_select", **fields)

    def poll(self):
        failed, e = self._event()
        if failed is not None:
            return failed[0], None, failed[1]
        if e is None:
            return None
        key = self._key
        if e["state"] == "progress":
            return key, SelectionProgress(e["processed"], e["total"],
                                          e["count"]), None
        if e["state"] == "done":
            sel = DrcSelection(self._db, self._ci, e["selection"],
                               len(self._db.checks[self._ci].errors),
                               e["count"])
            return key, sel, None
        if e["state"] == "error":
            return key, None, e.get("message")
        return None


class MarkerWorker(_Worker):
    kind = "markers"

    def submit(self, key, db, query):
        (ci,) = tuple(query.get("checks") or ())
        fields = dict(check=int(ci),
                      bounds=[float(v) for v in query["bounds_um"]],
                      width=int(query["width_px"]),
                      height=int(query["height_px"]),
                      waived=query.get("waived"))
        members = (query.get("members") or {}).get(ci)
        if members is not None:
            fields["scope"] = members.spec
        selected = (query.get("selected") or {}).get(ci)
        if selected is not None and getattr(selected, "sid", None) is not None:
            fields["selection"] = selected.sid
        self._ci = ci
        self._submit(key, db, "drc_markers", **fields)

    def poll(self):
        failed, e = self._event()
        if failed is not None:
            return failed[0], None, failed[1]
        if e is None:
            return None
        key = self._key
        if e["state"] in ("partial", "done"):
            try:
                markers = PointMarkers(e, self._ci)
            except (OSError, ValueError) as exc:
                return key, None, str(exc)
            if e["state"] == "partial":
                return key, MarkerProgress(markers), None
            return key, markers, None
        if e["state"] == "error":
            return key, None, e.get("message")
        return None


class DeltaGroup(DrcScope):
    """One CD delta group row."""
    __slots__ = ("row", "key", "name", "total", "estimated_count", "_count")

    def __init__(self, db, ci, gid, row, waived=None):
        super().__init__(db, ci, {"group": gid, "row": row["row"]})
        self.row = row["row"]
        self.key = tuple(row["key"])
        self.name = row["name"]
        self.total = row["total"]
        self.estimated_count = row["estimated"]
        self._count = (waived, row["count"])

    def count(self, waived=None):
        if waived is None:
            return self.total
        if self._count[0] == waived:
            return self._count[1]
        return super().count(waived)


class DeltaGroups(object):
    """A rule's (or cluster's) delta groups the service built."""

    def __init__(self, db, ci, event):
        self._db, self.ci = db, ci
        self.gid = event["group"]
        self.step_ticks = event["step"]
        self.auto_step = event["auto"]
        self.total = event["total"]
        self.estimated_total = event["estimated_total"]
        self._len = event["groups"]

    def __len__(self):
        return self._len

    def page(self, start, limit, waived=None):
        rows = self._db._request("drc_delta_rows", group=self.gid,
                                 start=int(start), limit=int(limit),
                                 waived=waived)
        return [DeltaGroup(self._db, self.ci, self.gid, r, waived)
                for r in rows]

    def __getitem__(self, row):
        got = self.page(row, 1)
        if not got:
            raise IndexError(row)
        return got[0]

    def find(self, key):
        return self._db._request("drc_delta_find", group=self.gid,
                                 key=[int(v) for v in key])

    def status_changed(self, eis=None):
        pass

    def reset_status(self):
        pass

    def __del__(self):
        handle = getattr(self._db, "_handle", None)
        if handle is not None:
            _DROPS.append((handle, "drc_delta_drop", "group", self.gid))


class DeltaWorker(_Worker):
    kind = "delta"

    def submit(self, key, db, ci, constraints, step, mode="absolute",
               cluster=None):
        fields = dict(check=int(ci), constraints=list(constraints or ()),
                      basis=mode, step=step)
        if cluster is not None:
            fields["cluster"] = cluster.index
        self._ci = ci
        self._submit(key, db, "drc_delta", **fields)

    def poll(self):
        failed, e = self._event()
        if failed is not None:
            return failed[0], None, failed[1]
        if e is None:
            return None
        if e["state"] == "progress":
            self.progress = e.get("text")
            return None
        key = self._key
        self.progress = None
        if e["state"] == "done":
            if not e.get("measurable"):
                return key, None, None
            return key, DeltaGroups(self._db, self._ci, e), None
        if e["state"] == "error":
            return key, None, e.get("message")
        return None


class ExactQueryWorker(_Worker):
    kind = "inview"

    def submit(self, key, db, ci, bounds, cap, membership=None, waived=None):
        fields = dict(check=int(ci), bbox=[float(v) for v in bounds],
                      cap=int(cap), waived=waived)
        if membership is not None:
            fields["scope"] = membership.spec
        self._submit(key, db, "drc_inview", **fields)

    def poll(self):
        failed, e = self._event()
        if failed is not None:
            return failed[0], None, failed[1]
        if e is None or e["state"] == "progress":
            return None
        key = self._key
        if e["state"] == "done":
            return key, [(ci, ei, kind, pts)
                         for ci, ei, kind, _num, pts in e["rows"]], None
        if e["state"] == "error":
            return key, None, e.get("message")
        return None


def _drc(result):
    return PackDrc(result) if result["packed"] else AsciiDrc(result)


def drc_busy(db):
    """Who re-packs `db`'s pack now, or None."""
    return service().request("drc_busy", db=os.path.abspath(db))


def drc_find(db):
    """`db`'s pack, or None."""
    return service().request("drc_find", db=os.path.abspath(db))


def drc_open_pack(pack, source=None, reviewer=None):
    """A pack under review; `source` (the .db) must match its fingerprint."""
    return _drc(service().request(
        "drc_open", path=os.path.abspath(pack), mode="pack",
        source=None if source is None else os.path.abspath(source),
        reviewer=reviewer))


def drc_load(path, reviewer=None):
    """floe/drc.py load_db: a pack given, or the .db's current pack, else
    the ASCII file."""
    return _drc(service().request("drc_open", path=os.path.abspath(path),
                                  mode="load", reviewer=reviewer))


def drc_cd_markers(constraints, markers):
    """drc_cd_info for markers given as (kind, [(x, y), ...]) um."""
    return service().request(
        "drc_cd_info", constraints=list(constraints or ()),
        markers=[[kind, [list(p) for p in pts]] for kind, pts in markers])


def svrf_rules(path):
    """A `<deck>.rules.json` sidecar as written (ValueError when it is
    not one)."""
    try:
        reply = service().request("svrf_rules", path=os.path.abspath(path))
    except ServiceError as exc:
        raise ValueError(str(exc))
    if reply.get("warning"):
        sys.stderr.write(reply["warning"] + "\n")
    return reply["rules"]


def svrf_operands(rhs):
    """Operand names of a derivation's right-hand side."""
    return service().request("svrf_operands", rhs=rhs)


def db_name_of(path):
    """The results database's name for a pack or .db path (a hidden
    `.<db>.tray` and a legacy `<db>.ice` both give `<db>`)."""
    name = os.path.basename(path)
    if name.startswith(".") and name.endswith(".tray"):
        return name[1:-len(".tray")] or name
    if name.endswith(".ice"):
        return name[:-len(".ice")] or name
    return name


# --- the view channel (P4c, docs/SHARED_APP_LAYER.ko.md §7): a source
# drawn through the shared Rust ViewController

# the cell tree's questions (view_cells kinds)
CELL_QUERY_KINDS = ("cell_sources", "cells", "cell_find", "cell_bbox",
                    "cell_insts")

RAW_SIGNATURE = b"FLOERAW1"
RAW_HEADER_LEN = 16


class ViewSession:
    """A source viewed through `floe2 gtk-service`'s view channel: the
    controller decides what is drawn and when; this sends the viewer's edits
    and hands back the frames and state changes the service announces.

    `snapshot` is the latest state the service said (its `state` holds the
    viewport, depth, detail, layers ...); `model` the source's dbu, bbox,
    whether it is a deck, its skipped placements."""

    def __init__(self, source, width, height, ids=None, mode="level",
                 patch=None, margin=False, frame_cache=True, svc=None):
        self._svc = svc or service()
        result = self._svc.request(
            "view_open", source=os.path.abspath(source), width=int(width),
            height=int(height), levels=normalize_levels(ids), mode=mode,
            patch=patch or None, margin=bool(margin),
            frame_cache=bool(frame_cache))
        self.view = result["view"]
        self.snapshot = result["snapshot"]
        self.model = result["model"]
        self.closed = False

    def edit(self, **patch):
        """One edit (the controller's Patch: navigation, pixels, depth,
        detail, thin, layers, layer_change, frames, labels, font_px, mono,
        density, style_deltas, root); the state it made."""
        self.snapshot = self._svc.request("view_edit", view=self.view,
                                          patch=patch)
        return self.snapshot

    def cancel(self):
        """Esc: the frame in progress stops."""
        self.snapshot = self._svc.request("view_cancel", view=self.view)
        return self.snapshot

    def query(self, frame, kind, x, y, r_px, nth=0, layers=None):
        """A snap or pick at viewport pixel (x, y) of the frame shown
        (its id); the controller's query id - the answer is a ("query",
        result) event carrying it."""
        return self._svc.request(
            "view_query", view=self.view, frame=int(frame), kind=kind,
            x=float(x), y=float(y), r_px=float(r_px), nth=int(nth),
            layers=[list(l) for l in layers] if layers else None)["id"]

    def minimap(self, depth=None, bbox=None):
        """The overview's base image for `depth` (None: full) and the die's
        place in it: {size, key, base (size x size palette digits: 0 the
        panel, 1 the die, 2 its edge, 3 a frontier box), bbox, die [x, y,
        w, h], scale (px per dbu)}; `bbox` = a view root's die (the plain
        base: the baked frontiers are the top's)."""
        return self._svc.request(
            "view_minimap", view=self.view, depth=depth,
            bbox=None if bbox is None else [float(v) for v in bbox])

    def border(self, bbox=None, width=None, height=None):
        """The scale border (app-core view::border) of the view's viewport,
        or of `bbox` (dbu) at `width` x `height`: {step_um, step_px, label,
        decimals, x, y ([{from, to, even}] in px along the top/bottom and
        the left/right bands), top_left, top_right, bottom_left,
        bottom_right ({x, y} in um, written)}."""
        fields = {}
        if bbox is not None:
            fields = dict(bbox=[float(v) for v in bbox], width=int(width),
                          height=int(height))
        return self._svc.request("view_border", view=self.view, **fields)

    def cells(self, kind, seq, **fields):
        """A cell-tree question (cell_sources, cells, cell_find, cell_bbox,
        cell_insts - floe_oracle/rust_render.py's fields; cell_insts' view box is
        `box`); the answer is a ("cells", result) event with `seq`."""
        self._svc.request("view_cells", view=self.view, kind=kind,
                          seq=int(seq), **fields)

    def clip(self, seq, bbox, out, layers=None, cell_name="FLOE_CLIP"):
        """Save the box (dbu, the root's coordinates) as an OASIS file; a
        ("clip", result) event when it is written or refused."""
        self._svc.request("view_clip", view=self.view, seq=int(seq),
                          bbox=[int(v) for v in bbox], out=os.path.abspath(out),
                          layers=[list(l) for l in layers] if layers else None,
                          cell_name=cell_name)

    def close(self):
        if not self.closed:
            self.closed = True
            try:
                self._svc.request("view_close", view=self.view)
            except ServiceError:
                pass

    def events(self):
        """This view's events since the last call: ("view", snapshot),
        ("frame", frame), ("query" | "cells" | "clip", result), ("closed",
        None), ("frame_error", message). A
        frame of another (closed) view is removed unread."""
        out = []
        for event in self._svc.events():
            kind = event.get("event")
            if event.get("view") != self.view:
                frame = event.get("frame") or {}
                if frame.get("path"):
                    try:
                        os.unlink(frame["path"])
                    except OSError:
                        pass
                continue
            if kind == "view":
                self.snapshot = event["snapshot"]
                out.append(("view", event["snapshot"]))
            elif kind == "frame":
                out.append(("frame", event["frame"]))
            elif kind == "closed":
                self.closed = True
                out.append(("closed", None))
            elif kind in ("query", "cells", "clip"):
                out.append((kind, event["result"]))
            else:
                out.append((kind, event.get("message")))
        return out


class ViewWorker:
    """The viewer's render worker when the shared Rust ViewController draws
    (P4d, docs/SHARED_APP_LAYER.ko.md §7) - floe_oracle/rust_render.py's place in
    floe/gui.py: the view's frames and states come as `events()`, the
    viewer's edits go to `edit()`. The jobs the viewer still submits - snap,
    pick, the cell tree, clip, recolor, repattern, mono - become the view
    channel's requests and edits; their answers are the dicts
    floe_oracle/rust_render.py put on `res`. `cache` is the ServiceCache the panel
    reads (its layers, a deck's levels and mode)."""

    supports_abstract = False
    supports_density = True
    controller = True

    def __init__(self, cache, width, height, patch=None, margin=False,
                 frame_cache=True, svc=None):
        self.cache = cache
        self.deck = bool(getattr(cache, "is_jobdeck", False))
        # no margin and no label font for a deck (the controller's and
        # renderd's rule, as the deck adapter had it)
        self.supports_margin_prefetch = not self.deck
        self.supports_label_font_px = not self.deck
        self.res = queue.Queue()
        self.session = None
        self.open_report = {}
        self.error = None
        self.shown = None        # the frame on screen (the queries' anchor)
        self._args = (int(width), int(height), patch, bool(margin),
                      bool(frame_cache))
        self._svc = svc or service()
        self._queries = {}       # the controller's query id -> (kind, seq)
        self._lock = threading.Lock()
        self._backlog = []       # jobs submitted while the view opens

    # ---- life ---------------------------------------------------------------
    def _open(self):
        w, h, patch, margin, frame_cache = self._args
        started = time.monotonic()
        session = ViewSession(
            self.cache.src, w, h, ids=getattr(self.cache, "ids", None),
            mode=(getattr(self.cache, "mode", None) or "level")
            if self.deck else "level",
            patch=patch, margin=margin, frame_cache=frame_cache,
            svc=self._svc)
        self.open_report = {"service_open_ms":
                            round((time.monotonic() - started) * 1000)}
        with self._lock:
            self.session = session
            backlog, self._backlog = self._backlog, []
        for job in backlog:
            self.submit(job)

    def start(self):
        self._open()

    def start_async(self, done):
        def run():
            error = None
            try:
                self._open()
            except (ServiceError, OSError, RuntimeError) as exc:
                self.error = error = exc
            done(error)
        threading.Thread(target=run, daemon=True,
                         name="floe2-view-open").start()

    @property
    def snapshot(self):
        return None if self.session is None else self.session.snapshot

    def alive(self):
        if self.session is None:
            return self.error is None
        return not self.session.closed and \
            (self.session.snapshot or {}).get("phase") != "failed"

    def exitcode(self):
        failure = (self.session.snapshot or {}).get("failure") \
            if self.session is not None else None
        return (failure or {}).get("message") or self.error

    def stop(self):
        if self.session is not None:
            self.session.close()

    def cancel(self, _before_gen=None):
        """Esc: the frame in progress stops (the controller's rule: the
        same state is not drawn again, its next change is)."""
        if self.session is not None:
            try:
                self.session.cancel()
            except ServiceError as exc:
                self.res.put({"kind": "error", "msg": str(exc)})

    # ---- edits and answers ----------------------------------------------------
    def edit(self, **patch):
        """One edit; the state it made (None when refused - said on res)."""
        if self.session is None:
            return None
        try:
            return self.session.edit(**patch)
        except ServiceError as exc:
            self.res.put({"kind": "error", "msg": str(exc)})
            return None

    def events(self):
        """The view's frames and states, oldest first: ("frame", frame),
        ("view", snapshot), ("closed", None); the answers go to res."""
        if self.session is None:
            return []
        out = []
        for kind, value in self.session.events():
            if kind == "query":
                seq_kind = self._queries.pop(value.get("id"), None)
                if seq_kind is not None:
                    self.res.put(dict(value, kind=seq_kind[0],
                                      seq=seq_kind[1]))
            elif kind in ("cells", "clip"):
                self.res.put(value)
            elif kind == "frame_error":
                self.res.put({"kind": "error", "msg": "frame: %s" % value})
            else:
                out.append((kind, value))
        return out

    def minimap(self, depth=None, bbox=None):
        """ViewSession.minimap; None while the view opens or when refused
        (the viewer bakes its own then)."""
        if self.session is None:
            return None
        try:
            return self.session.minimap(depth, bbox)
        except ServiceError as exc:
            self.res.put({"kind": "error", "msg": "minimap: %s" % exc})
            return None

    def border(self, bbox, width, height):
        """ViewSession.border for the box the viewer shows; None while the
        view opens or when refused (no band then)."""
        if self.session is None:
            return None
        try:
            return self.session.border(bbox, width, height)
        except ServiceError as exc:
            self.res.put({"kind": "error", "msg": "scale border: %s" % exc})
            return None

    def _viewport_px(self, x, y):
        """World (dbu) -> the controller's viewport pixel, and its scale."""
        v = self.session.snapshot["state"]["viewport"]
        b = v["bbox"]
        spp = (b[2] - b[0]) / max(1, v["width"])
        return (x - b[0]) / spp, (b[3] - y) / spp, spp

    def _layer_rows(self):
        return (getattr(self.cache, "meta", None) or {}).get("layers", [])

    def submit(self, job):
        with self._lock:
            if self.session is None:
                self._backlog.append(job)
                return
        kind = job.get("kind")
        try:
            if kind in ("snap", "pick"):
                if self.shown is None:
                    return          # nothing on screen to ask about yet
                px, py, spp = self._viewport_px(float(job["x"]),
                                                float(job["y"]))
                r_px = float(job.get("r_px", float(job["r"]) / spp))
                try:
                    qid = self.session.query(
                        self.shown, kind, px, py, r_px,
                        nth=int(job.get("nth", 0)),
                        layers=job.get("layers"))
                except ServiceError as exc:
                    if exc.kind == "busy":
                        return      # the frame shown is not this state's
                    raise
                self._queries[qid] = (kind, int(job.get("seq", -1)))
            elif kind in CELL_QUERY_KINDS:
                fields = {k: job[k] for k in ("src", "cell", "pattern",
                                              "limit", "cap", "root")
                          if job.get(k) is not None}
                if job.get("view") is not None:
                    fields["box"] = [float(v) for v in job["view"]]
                self.session.cells(kind, int(job.get("seq", -1)), **fields)
            elif kind == "clip":
                self.session.clip(0, job["bbox"], job["out"],
                                  layers=job.get("layers"),
                                  cell_name=job.get("cell_name", "FLOE_CLIP"))
            elif kind == "recolor":
                self.edit(style_deltas=[
                    {"pair": [int(v) for v in key], "color": str(color)}
                    for key, color in job.get("colors", [])])
            elif kind == "repattern":
                self.edit(style_deltas=repattern_deltas(
                    self._layer_rows(), job.get("fills", []),
                    job.get("widths", [])))
            elif kind == "mono":
                self.edit(mono=bool(job.get("on")))
            else:
                raise ValueError("%s jobs belong to the view controller"
                                 % kind)
        except (ServiceError, ValueError, KeyError, TypeError) as exc:
            self.res.put({"kind": "error", "msg": "%s: %s" % (kind, exc)})


def repattern_deltas(rows, fills, widths):
    """floe_oracle/rust_render.py's `repattern` (every layer's fill and width
    replaced whole: a layer not named goes back to the plain speckle and
    width 1; a deck's level head names its datatypes) as the view
    channel's style deltas, one per drawn layer."""
    heads = {int(r["layer"]) for r in rows if r.get("jobdeck_head")}

    def expand(key):
        key = (int(key[0]), int(key[1]))
        if key[1] == 0 and key[0] in heads:
            return [(int(r["layer"]), int(r["datatype"])) for r in rows
                    if int(r["layer"]) == key[0]]
        return [key]

    fill_of, width_of = {}, {}
    for key, bitmap in fills:
        for k in expand(key):
            fill_of[k] = fill_dto(bitmap)
    for key, width in widths:
        width = int(width)
        if width < 1 or width > 8:
            raise ValueError("line width must be in 1..8")
        for k in expand(key):
            width_of[k] = width
    out = []
    for r in rows:
        if r.get("jobdeck_head"):
            continue
        key = (int(r["layer"]), int(r["datatype"]))
        out.append({"pair": list(key),
                    "fill": fill_of.get(key, {"kind": "speckle"}),
                    "width": width_of.get(key, 1)})
    return out


def fill_dto(rows):
    """floe's 16x16 `*`/`.` bitmap as the view channel's fill (the renderd
    style rule floe_oracle/rust_render.py's _pattern_fill had)."""
    if not isinstance(rows, str):
        raise ValueError("fill bitmap must be a string")
    lines = rows.splitlines()
    if len(lines) != 16 or any(len(line) != 16 for line in lines):
        raise ValueError("fill bitmap must contain 16 rows of 16 pixels")
    if any(ch not in ".*" for line in lines for ch in line):
        raise ValueError("fill bitmap pixels must be '.' or '*'")
    words = []
    for line in lines:
        word = 0
        for ch in line:
            word = (word << 1) | (ch == "*")
        words.append(word)
    if all(word == 0xFFFF for word in words):
        return {"kind": "solid"}
    if all(word == 0 for word in words):
        return {"kind": "clear"}
    if words == [0xAAAA if row % 2 == 0 else 0x5555 for row in range(16)]:
        return {"kind": "speckle"}
    return {"kind": "pattern", "rows": words}


def read_frame(frame):
    """A frame event's pixels, the file removed: (bytes, width, height,
    format) - raw: tightly packed top-down RGBA without the header; png:
    the file's bytes."""
    path = frame["path"]
    try:
        with open(path, "rb") as f:
            if frame.get("format") == "raw":
                header = f.read(RAW_HEADER_LEN)
                if not header.startswith(RAW_SIGNATURE):
                    raise ValueError("not a raw frame")
                w = int.from_bytes(header[8:12], "little")
                h = int.from_bytes(header[12:16], "little")
                data = f.read()
                if len(data) != w * h * 4:
                    raise ValueError("raw frame is truncated")
                return data, w, h, "raw"
            return f.read(), int(frame["width"]), int(frame["height"]), "png"
    finally:
        try:
            os.unlink(path)
        except OSError:
            pass


# ---- the team's shared review (feature/jobdeck-drc's drc_team /
# drc_migrate; app-core drc::team over the service). The functions block
# until their service job ends - call them from worker threads.

VISITED = 1
NOT_REQUIRED = 2
ReviewState = collections.namedtuple("ReviewState", "visited waived not_required")
TeamState = collections.namedtuple("TeamState", "visited waived not_required conflict")


def _job(db, kind, op, cancelled=None, progress=None, **fields):
    """Run service job `kind` (`op`) to its end: its done event; progress
    (done, total[, phase]); InterruptedError when `cancelled()` turns
    true; ValueError with the service's message on failure."""
    db._start(kind, op, **fields)
    while True:
        if cancelled is not None and cancelled():
            db.cancel(kind)
            raise InterruptedError("review operation cancelled")
        event = db.poll(kind)
        if event is None:
            time.sleep(0.01)
            continue
        state = event.get("state")
        if state == "progress":
            if progress is not None:
                progress(event.get("done", 0), event.get("total", 0),
                         event.get("phase", ""))
            continue
        if state == "done":
            return event
        if state == "cancelled" or event.get("cancelled"):
            raise InterruptedError(event.get("message") or "review operation cancelled")
        raise ValueError(event.get("message") or "review operation failed")


class SnapshotMembers(object):
    """One plane of one rule of a snapshot or plan (count, pages)."""

    def __init__(self, owner, op, key, ci, kind):
        self.owner, self.op, self.key, self.ci, self.kind = owner, op, key, ci, kind
        self._total = None

    def _page(self, start, limit):
        return self.owner.db._request(
            "drc_%s_page" % self.op, check=int(self.ci), kind=self.kind,
            start=int(start), limit=int(limit), **{self.op: self.key})

    def count(self, waived=None):
        if self._total is None:
            self._total = self._page(0, 0)["count"]
        return self._total

    __len__ = count

    def __bool__(self):
        return self.count() > 0

    def page(self, start, limit, waived=None):
        reply = self._page(start, limit)
        self._total = reply["count"]
        return reply["ids"]

    def contains_many(self, eis):
        eis = [int(e) for e in eis]
        if not eis:
            return []
        return self.owner.db._request(
            "drc_%s_contains" % self.op, check=int(self.ci), kind=self.kind,
            errors=eis, **{self.op: self.key})

    def contains(self, ei):
        return bool(self.contains_many([ei])[0])

    __contains__ = contains


class ReviewSnapshot(object):
    """A reviewer snapshot, converted peer file or team aggregate kept by
    the service (drc_team ReviewSnapshot / ReviewAggregate's surface)."""

    def __init__(self, db, info):
        self.db = db
        self.sid = info["snapshot"]
        self.reviewer = info["reviewer"]
        self.path = info["path"]
        self.original_path = info.get("original_path") or info["path"]
        self.snapshot_id = info["snapshot_id"]
        self.exported_ns = info["exported_ns"]
        self.visited_known = info["visited_known"]
        self.waive_known = info["waive_known"]
        self.not_required_known = info["not_required_known"]
        self.aggregate = bool(info.get("aggregate"))
        self.metadata = {"aggregate": self.aggregate,
                         "source_path": info.get("source_path"),
                         "source_waive_path": info.get("source_waive_path")}
        self._closed = False
        self._leases = 0
        self._lock = threading.Lock()

    def counts(self, ci=None):
        return self.db._request("drc_snapshot_counts", snapshot=self.sid,
                                check=None if ci is None else int(ci))

    def members(self, ci, kind="work"):
        return SnapshotMembers(self, "snapshot", self.sid, ci, kind)

    def page(self, ci, start, limit, kind="work"):
        return self.members(ci, kind).page(start, limit)

    def get_many(self, ci, eis):
        rows = self.db._request("drc_snapshot_get", snapshot=self.sid,
                                check=int(ci), errors=[int(e) for e in eis])
        if self.aggregate:
            return [TeamState(*r) for r in rows]
        return [ReviewState(*r[:3]) for r in rows]

    def get(self, ci, ei):
        return self.get_many(ci, [ei])[0]

    def lease(self):
        with self._lock:
            if self._closed:
                raise OSError("review snapshot is closed")
            self._leases += 1
        return _Lease(self)

    def _release(self):
        with self._lock:
            self._leases -= 1
            drop = self._closed and self._leases == 0
        if drop:
            self._drop()

    def close(self):
        with self._lock:
            if self._closed:
                return
            self._closed = True
            drop = self._leases == 0
        if drop:
            self._drop()

    def _drop(self):
        handle = getattr(self.db, "_handle", None)
        if handle is not None:
            _DROPS.append((handle, "drc_snapshot_drop", "snapshot", self.sid))


class _Lease(object):
    """A snapshot kept open while a job reads it."""

    def __init__(self, source):
        self._source = source
        self._closed = False

    def __getattr__(self, name):
        return getattr(self._source, name)

    def lease(self):
        return self._source.lease()

    def close(self):
        if not self._closed:
            self._closed = True
            self._source._release()

    def __enter__(self):
        return self

    def __exit__(self, *_exc):
        self.close()


def open_review(db, path, cancelled=None, progress=None):
    """A peer's snapshot, working review file or waive file, read."""
    event = _job(db, "team", "drc_team_load", cancelled, progress,
                 paths=[os.path.abspath(path)])
    if event.get("errors"):
        raise ValueError(event["errors"][0].split(": ", 1)[-1])
    return ReviewSnapshot(db, event["loaded"][0])


def aggregate_reviews(db, sources, cancelled=None, progress=None):
    """One's own review and the peers', combined (conflicts marked)."""
    event = _job(db, "team", "drc_team_aggregate", cancelled, progress,
                 sources=[s.sid for s in sources])
    return ReviewSnapshot(db, event["info"])


def import_conflicts(db, source, cancelled=None, progress=None):
    event = _job(db, "team", "drc_team_import", cancelled, progress,
                 source=source.sid)
    return ReviewSnapshot(db, dict(snapshot=event["snapshot"], reviewer="import-conflicts",
                                   path="", snapshot_id="", exported_ns=0,
                                   visited_known=True, waive_known=True,
                                   not_required_known=True, aggregate=True))


def export_review(db, path, cancelled=None, progress=None):
    _job(db, "team-write", "drc_team_export", cancelled, progress,
         path=os.path.abspath(path))
    return types.SimpleNamespace(close=lambda: None, path=os.path.abspath(path))


def team_csv(db, sources, path, cancelled=None, progress=None):
    """The aggregate's statistics, whole database and per rule, as CSV."""
    _job(db, "team-write", "drc_team_csv", cancelled, progress,
         sources=[s.sid for s in sources], path=os.path.abspath(path))


def apply_review(db, source, ci=None, errors=None, force=False, cancelled=None,
                 progress=None):
    """Apply a peer's decisions (safe, or forced on rule `ci`'s `errors`);
    the counts of what changed."""
    try:
        event = _job(db, "team-write", "drc_team_apply", cancelled, progress,
                     source=source.sid, check=ci,
                     errors=None if errors is None else [int(e) for e in errors],
                     force=bool(force))
    finally:
        db.counts_refresh()
        db.review_flags_changed()
    return event["result"]


def set_members(db, ci, errors, field, on, cancelled=None, progress=None):
    """One's own waive or not-required on rule `ci`'s `errors`."""
    try:
        _job(db, "review", "drc_review", cancelled, progress, check=int(ci),
             action="waive" if field == "waive" else "not-required",
             errors=[int(e) for e in errors], on=bool(on))
    finally:
        db.counts_refresh()
        db.review_flags_changed(ci)


def prepare_conflict_selection(db, aggregate, ci, membership=None, waived=None,
                               cancelled=None, progress=None):
    """The aggregate's conflicts in rule `ci` within `membership` (a scope)
    and the waive filter, as a service selection."""
    fields = dict(aggregate=aggregate.sid, check=int(ci), waived=waived)
    if membership is not None:
        fields["scope"] = membership.spec
    event = _job(db, "conflicts", "drc_team_conflicts", cancelled, progress, **fields)
    return DrcSelection(db, ci, event["selection"], len(db.checks[ci].errors),
                        event["total"])


class MigrationPlan(object):
    """A prepared review migration kept by the service."""

    def __init__(self, db, event):
        self.db = db
        self.pid = event["plan"]
        self.summary = dict(event["summary"])
        self.source_path = self.summary.get("source_path")
        self.state_path = self.summary.get("state_path")
        self.reviewer = self.summary.get("reviewer")

    def rule_counts(self, ci=None):
        return self.db._request("drc_plan_counts", plan=self.pid,
                                check=None if ci is None else int(ci))

    def members(self, ci, kind="matched"):
        return SnapshotMembers(self, "plan", self.pid, ci, kind)

    def page(self, ci, start, limit, kind="matched"):
        return self.members(ci, kind).page(start, limit)

    def get_many(self, ci, eis):
        rows = self.db._request("drc_plan_get", plan=self.pid, check=int(ci),
                                errors=[int(e) for e in eis])
        return [None if r is None else ReviewState(*r) for r in rows]

    def get(self, ci, ei):
        return self.get_many(ci, [ei])[0]

    def close(self):
        handle = getattr(self.db, "_handle", None)
        if handle is not None and self.pid is not None:
            _DROPS.append((handle, "drc_plan_drop", "plan", self.pid))
        self.pid = None


def prepare_migration(db, old_path, state_path=None, cancelled=None, progress=None):
    event = _job(db, "migrate", "drc_migrate_prepare", cancelled, progress,
                 previous=os.path.abspath(old_path),
                 state=None if not state_path else os.path.abspath(state_path))
    return MigrationPlan(db, event)


def apply_migration(db, plan, cancelled=None, progress=None):
    try:
        event = _job(db, "migrate-write", "drc_migrate_apply", cancelled, progress,
                     plan=plan.pid)
    finally:
        db.counts_refresh()
        db.review_flags_changed()
    plan.summary = dict(event.get("summary") or plan.summary)
    return event["result"]


def review_stats_export(db, path, cancelled=None, progress=None):
    """One's own review statistics (database and rules) as CSV."""
    _job(db, "review", "drc_review_stats", cancelled, progress,
         path=os.path.abspath(path))
