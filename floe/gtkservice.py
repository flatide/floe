"""The GTK viewer's link to `floe2 gtk-service` (rust/floe2/src/service.rs,
docs/SHARED_APP_LAYER.ko.md P2): what the viewer decides about a source
that is not drawing - whether it opens as it is, its meta and layer table,
a deck's plan and composite spec, its layer properties - is the shared
Rust app layer's, asked over one JSON line per request. The viewer keeps
its widgets and its own session state (which layers are on).

`ServiceCache` is what floe/gui.py and floe/rust_render.py read of an
open source, the attributes floe.cache.Cache and the jobdeck DeckCache
gave them: src, dir (the cache folder, or the spec renderd opens), meta,
is_jobdeck, ids, mode, props_src, exists(), load(), is_stale(), close().
"""

import atexit
import json
import os
import subprocess
import sys
import threading
import types


class ServiceError(RuntimeError):
    """A request the service refused: `kind` is its error kind (busy,
    input, cache, ...), the message what it said."""

    def __init__(self, kind, message):
        super().__init__(message)
        self.kind = kind


class Service:
    """One `floe2 gtk-service` process, started on the first request and
    again if it ended; requests are answered in order."""

    def __init__(self, binary=None):
        self._binary = binary
        self._proc = None
        self._lock = threading.Lock()
        self._seq = 0

    def _start(self):
        binary = self._binary
        if binary is None:
            from .vfsclient import find_floe2
            binary = find_floe2()
        self._proc = subprocess.Popen(
            [binary, "gtk-service"], stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, text=True, bufsize=1)

    def request(self, op, **fields):
        with self._lock:
            if self._proc is None or self._proc.poll() is not None:
                self._start()
            self._seq += 1
            fields["id"] = self._seq
            fields["op"] = op
            try:
                self._proc.stdin.write(json.dumps(fields) + "\n")
                self._proc.stdin.flush()
                line = self._proc.stdout.readline()
            except OSError as exc:
                self._proc = None
                raise ServiceError("worker", "gtk-service: %s" % exc)
            if not line:
                self._proc = None
                raise ServiceError("worker", "gtk-service ended")
            reply = json.loads(line)
            error = reply.get("error")
            if error is not None:
                raise ServiceError(error.get("kind", "worker"),
                                   error.get("message", "gtk-service error"))
            return reply.get("result")

    def close(self):
        with self._lock:
            proc, self._proc = self._proc, None
        if proc is not None and proc.poll() is None:
            try:
                proc.stdin.close()
                proc.wait(timeout=5)
            except (OSError, subprocess.TimeoutExpired):
                proc.kill()


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


# ---- DRC review (P2b: app-core drc::desktop) ----------------------------

STATUS_NONE = 0
STATUS_WAIVED = 1
_ERROR_PAGE = 256
_STATUS_PAGE = 4096


class DrcError(object):
    """One violation as the service sent it: kind 'p' (polygon) or 'e'
    (edge), its global 1-based number, points in um (floe/drc.py
    DrcError)."""
    __slots__ = ("kind", "num", "pts", "_db", "_ci", "_ei")

    def __init__(self, kind, num, pts, db=None, ci=None, ei=None):
        self.kind = kind
        self.num = num
        self.pts = [tuple(p) for p in pts]
        self._db, self._ci, self._ei = db, ci, ei

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
        return service().request(op, handle=self._handle, **fields)

    def close(self):
        """Let the service close it (and the pack's reader lock with it)."""
        handle, self._handle = self._handle, None
        if handle is not None:
            try:
                service().request("drc_close", handle=handle)
            except ServiceError:
                pass


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
        self.waive_path = result.get("waive_path")
        self.note_path = result.get("note_path")
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
        key = (ci, k)
        got = self._status.get(key)
        if got is None:
            if len(self._status) > 256:
                self._status.pop(next(iter(self._status)))
            hexed = self._request("drc_status", check=ci,
                                  start=k * _STATUS_PAGE, count=_STATUS_PAGE)
            got = bytearray.fromhex(hexed)
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
                   waived=None):
        rows = self._request(
            "drc_query", bbox=[x0_um, y0_um, x1_um, y1_um], cap=int(cap),
            checks=None if checks is None else [int(c) for c in checks],
            waived=waived)
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

    def waive_import(self, src):
        reply = self._request("drc_waive_import", path=os.path.abspath(src))
        self._counts = [tuple(c) for c in reply["counts"]]
        self._status.clear()
        return reply["waived"]


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
