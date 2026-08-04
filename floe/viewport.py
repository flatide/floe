"""Toolkit-independent viewer core: the VFS working-set layout.

Used by the native GUI (gui.py); all klayout objects live here, the GUI
shell only displays PNG bytes. The viewer is VFS-only - it renders a
FLOE_WS layout fed by vfsd deltas (see VfsMosaic); the old .ice tile
mosaic was removed.
"""

import os

import klayout.db as db


MAX_LIVE_TILES = 32     # a render request may touch at most this many tiles
EVICT_ABOVE = 128       # keep at most this many loaded entries in the mosaic


class _WsNames:
    """design-name resolver for the hier working set: page cells are
    P<ci>_<li>_<seq>, working-set cells W<gen>_<r>_<ci> - both carry
    the source cell index; the names= table (sent once per daemon
    run) carries the name. Drop-in for the flat path's dict via
    .get()."""

    def __init__(self, names):
        self.names = names  # ci -> design cell name (shared dict)

    def get(self, cell, default=None):
        try:
            if cell.startswith("P"):
                return self.names.get(
                    int(cell[1:].split("_", 1)[0]), default)
            if cell.startswith("W"):
                return self.names.get(
                    int(cell.rsplit("_", 1)[1]), default)
        except (ValueError, IndexError):
            pass
        return default


def frame_layer(meta):
    """runtime frame/outline layer, dt 0: max design layer + 1,
    saturated at u32::MAX and stepping DOWN to the nearest unused
    number on collision - MUST match the daemon's floe_vfs
    frame_layer() rule exactly (both derive it from the same layer
    list; u32::MAX is a legal design layer)"""
    used = {int(l["layer"]) for l in meta.get("layers", [])}
    fl = max(min(max(used) + 1, 0xFFFFFFFF) if used else 1, 1)
    while fl in used:
        fl -= 1
    return (fl, 0)


def live_caps(meta):
    """(max live tiles, LRU evict-above) scaled to the cache's tile
    size. The base constants are tuned for the 6 MB --tile-mb default;
    a finer grid means proportionally lighter tiles, so a view may
    span proportionally more of them for the same load/draw/memory
    budget (capped at 8x so a sparse file cannot unbound the caps)."""
    try:
        g = meta["grid"]
        avg = meta["src"]["size"] / max(1, g["nx"] * g["ny"])
        f = max(1, min(8, round(6e6 / max(1.0, avg))))
    except (KeyError, TypeError):
        f = 1
    return MAX_LIVE_TILES * f, EVICT_ABOVE * f


class VfsMosaic:
    """Working-set layout fed by vfsd deltas (VFS caches): page
    cells arrive pre-spliced in one delta file per view, the top's
    instance list mirrors the daemon's placement plan. Page cell
    names are globally unique, so reads never merge (no @t tags) and
    eviction prunes exactly one page subtree by name."""

    def __init__(self, cache):
        self._dbu = cache.meta["dbu"]
        self._layer_keys = [(l["layer"], l["datatype"])
                            for l in cache.meta["layers"]]
        # cut-frame outline layer, drawn hollow: one past the
        # highest DESIGN layer (same rule as the daemon's
        # frame_layer()) so it can never collide with real content
        # - (255,0) exists in real designs (review finding)
        self.FRAME_LAYER = frame_layer(cache.meta)
        self.ly = db.Layout(False)  # pages keep arrays compact
        self.ly.dbu = self._dbu
        for (l, d) in self._layer_keys:
            self.ly.layer(db.LayerInfo(l, d))
        self.ly.layer(db.LayerInfo(*self.FRAME_LAYER))
        self.top = self.ly.create_cell("FLOE_WS")
        self.cells = {}  # page cell name -> cell index
        self.design = {}  # ci -> design cell name (_WsNames)
        self.label_ci = None  # ephemeral live-label cell
        self._lgen = 0
        # ---- hier session state (VFS_HIER.md par.3.1/3.7). The
        # ci->name table is daemon-run-wide and sent exactly once,
        # so it lives on the CACHE and is shared by every mosaic
        # (render working set + probe layouts).
        if not hasattr(cache, "_vfs_names"):
            cache._vfs_names = {}
        self.names = cache._vfs_names
        self._wc_cells = []   # current gen's WC cell names
        self.req_gen = 0      # daemon-gen counter (monotonic)
        self.applied_gen = 0  # last gen fully applied (the ack)
        self.need_reset = False
        # adaptive streaming budget (KB of decoded payload per
        # round): the service re-tunes it toward the round target
        # from measured apply times. FLOE_STREAM_KB pins it (no
        # adaptation); 0 disables streaming (single-shot loads).
        env_kb = os.environ.get("FLOE_STREAM_KB")
        self.stream_kb = int(env_kb) if env_kb else 24576
        self.stream_pinned = env_kb is not None
        # gate-only hook (par.7 fault injection): apply_hier raises
        # at step N once, exercising the reset_all recovery path
        self._fault_step = None

    # ------------------------------------------------ hier (V4)

    def load_names(self, path):
        """ci -> design-name table (names=, once per daemon run):
        load into the cache-shared dict and delete the file - the
        par.3.4 contract has no re-request key."""
        try:
            with open(path) as f:
                for ln in f:
                    ci, _, nm = ln.rstrip("\n").partition("\t")
                    self.names[int(ci)] = nm
        finally:
            try:
                os.unlink(path)
            except OSError:
                pass
        self.design = _WsNames(self.names)

    def apply_hier(self, delta_path, top_name, evict, labels=None,
                   gen=None):
        """Hier apply, par.3.7 steps (1)-(4): (1) read the delta (new
        pages + this gen's WC cells; resident page refs bind by name)
        (2) link the gen top (+labels) under FLOE_WS (3) batch-
        SHALLOW-delete the previous gen's WC cells - prune_cell would
        take resident pages down with them (4) prune evicted pages
        and remap surviving indexes. The caller sends ack=gen only
        after this returns (step (5)); on a stale frame it never
        calls this, which IS the rollback signal. True = changed."""
        changed = False

        def _fault(step):
            if self._fault_step == step:
                self._fault_step = None
                raise RuntimeError("injected fault at step %d" % step)

        if self.label_ci is not None:
            self.ly.prune_cell(self.label_ci, -1)
            self.label_ci = None
            changed = True
        prev_wc = self._wc_cells
        gen_prefix = (top_name.split("_", 1)[0] + "_"
                      if top_name else None)
        _fault(1)
        if delta_path:
            self.ly.read(delta_path)
            changed = True
        wc_now = []
        if gen_prefix:
            for c in self.ly.each_cell():
                nm = c.name
                if nm.startswith("P") and nm not in self.cells:
                    self.cells[nm] = c.cell_index()
                elif nm.startswith(gen_prefix):
                    wc_now.append(nm)
        _fault(2)
        self.top.clear_insts()
        if top_name:
            tc = self.ly.cell(top_name)
            if tc is None:
                raise RuntimeError(
                    "hier delta top %s missing" % top_name)
            self.top.insert(
                db.CellInstArray(tc.cell_index(), db.Trans()))
            changed = True
        if labels:
            self._lgen += 1
            lc = self.ly.create_cell("LABELS_%d" % self._lgen)
            self.label_ci = lc.cell_index()
            for (l, d, x, y, s) in labels:
                li = self.ly.layer(db.LayerInfo(l, d))
                lc.shapes(li).insert(
                    db.Text(s, db.Trans(db.Vector(int(x), int(y)))))
            self.top.insert(
                db.CellInstArray(self.label_ci, db.Trans()))
            changed = True
        _fault(3)
        if prev_wc:
            idxs = [self.ly.cell(nm).cell_index() for nm in prev_wc
                    if self.ly.cell(nm) is not None]
            if idxs:
                self.ly.delete_cells(idxs)
                changed = True
        self._wc_cells = wc_now
        _fault(4)
        for nm in evict:
            ci = self.cells.pop(nm, None)
            if ci is not None:
                self.ly.prune_cell(ci, -1)
                changed = True
        if evict or prev_wc:
            # klayout may reuse freed indexes: remap survivors (and
            # the label cell) by name
            keep = set(self.cells)
            self.cells = {c.name: c.cell_index()
                          for c in self.ly.each_cell()
                          if c.name in keep}
            if self.label_ci is not None:
                lc = self.ly.cell("LABELS_%d" % self._lgen)
                self.label_ci = (None if lc is None
                                 else lc.cell_index())
        if not isinstance(self.design, _WsNames):
            self.design = _WsNames(self.names)
        if gen is not None:
            self.applied_gen = gen
        return changed

    def reset_all(self):
        """Partial-apply recovery (par.3.7): a layout that failed
        mid-apply must not carry into the next gen. Wipe and rebuild
        IN PLACE - same Layout object, so the renderer's
        show_layout binding survives; the caller re-points
        renderer.top and refreshes, and sends reset=1 next."""
        self.ly.clear()
        self.ly.dbu = self._dbu
        for (l, d) in self._layer_keys:
            self.ly.layer(db.LayerInfo(l, d))
        self.ly.layer(db.LayerInfo(*self.FRAME_LAYER))
        self.top = self.ly.create_cell("FLOE_WS")
        self.cells = {}
        self._wc_cells = []
        self.label_ci = None
        self.applied_gen = 0
        self.need_reset = True

