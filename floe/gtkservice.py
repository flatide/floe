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
