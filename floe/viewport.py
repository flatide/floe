"""Toolkit-independent viewer core: the VFS working-set layout.

Used by the native GUI (gui.py); all klayout objects live here, the GUI
shell only displays PNG bytes. The viewer is VFS-only - it renders a
FLOE_WS layout fed by vfsd deltas (see VfsMosaic); the old .ice tile
mosaic was removed.
"""

import klayout.db as db


MAX_LIVE_TILES = 32     # a render request may touch at most this many tiles
EVICT_ABOVE = 128       # keep at most this many loaded entries in the mosaic


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

    # cut-frame outline layer (matches vfsd FRAME_LAYER/DT and the
    # skeleton cell-outline convention); drawn hollow
    FRAME_LAYER = (255, 0)

    def __init__(self, cache):
        self.ly = db.Layout(False)  # pages keep arrays compact
        self.ly.dbu = cache.meta["dbu"]
        for l in cache.meta["layers"]:
            self.ly.layer(db.LayerInfo(l["layer"], l["datatype"]))
        self.ly.layer(db.LayerInfo(*self.FRAME_LAYER))
        self.top = self.ly.create_cell("FLOE_WS")
        self.cells = {}  # page cell name -> cell index
        self.design = {}  # page cell name -> design cell name
        self.frame_ci = None  # ephemeral cut-frame cell
        self.label_ci = None  # ephemeral live-label cell
        self._lgen = 0

    def apply(self, delta_path, mats, evict, frames_path=None,
              labels=None):
        """Read new pages, drop evicted ones and the previous frame/
        label cells, read the current frames, build the current
        labels, then rebuild the top's instances to exactly the plan.
        labels: list of (layer, dt, x, y, string) to draw this view
        (skeleton labels filtered/decluttered by the caller). True =
        layout changed."""
        changed = False
        # frames and labels are per-view: drop the previous ones first
        # so no stale index survives into the eviction remap below
        for attr in ("frame_ci", "label_ci"):
            ci = getattr(self, attr)
            if ci is not None:
                self.ly.prune_cell(ci, -1)
                setattr(self, attr, None)
                changed = True
        if delta_path:
            self.ly.read(delta_path)
            for c in self.ly.each_cell():
                nm = c.name
                if nm.startswith("P") and nm not in self.cells:
                    self.cells[nm] = c.cell_index()
            changed = True
        for nm in evict:
            ci = self.cells.pop(nm, None)
            if ci is not None:
                self.ly.prune_cell(ci, -1)
                changed = True
        if evict:
            # klayout may reuse freed indexes: remap survivors
            keep = set(self.cells)
            self.cells = {c.name: c.cell_index()
                          for c in self.ly.each_cell()
                          if c.name in keep}
        if frames_path:
            self.ly.read(frames_path)
            for c in self.ly.each_cell():
                if c.name.startswith("FRAMES_"):
                    self.frame_ci = c.cell_index()
            changed = True
        if labels:
            self._lgen += 1
            lc = self.ly.create_cell("LABELS_%d" % self._lgen)
            self.label_ci = lc.cell_index()
            for (l, d, x, y, s) in labels:
                li = self.ly.layer(db.LayerInfo(l, d))
                lc.shapes(li).insert(
                    db.Text(s, db.Trans(db.Vector(int(x), int(y)))))
            changed = True
        self.top.clear_insts()
        for m in mats:
            nm, x, y, rot, flip, na, nb, va, vb, design = m
            self.design[nm] = design
            ci = self.cells.get(nm)
            if ci is None:
                continue
            tr = db.Trans(rot, flip, db.Vector(x, y))
            if na > 1 or nb > 1:
                self.top.insert(db.CellInstArray(
                    ci, tr, db.Vector(va[0], va[1]),
                    db.Vector(vb[0], vb[1]), na, nb))
            else:
                self.top.insert(db.CellInstArray(ci, tr))
        for ci in (self.frame_ci, self.label_ci):
            if ci is not None:
                self.top.insert(db.CellInstArray(ci, db.Trans()))
        return changed

