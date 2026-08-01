"""Toolkit-independent viewer core: lazily grown mosaic of cache tiles.

Used by the native GUI (gui.py); all klayout objects live here, the GUI
shell only displays PNG bytes.
"""

import os

import klayout.db as db

from .cache import viewer_mode_preferred

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

    def __init__(self, cache):
        self.ly = db.Layout(False)  # pages keep arrays compact
        self.ly.dbu = cache.meta["dbu"]
        for l in cache.meta["layers"]:
            self.ly.layer(db.LayerInfo(l["layer"], l["datatype"]))
        self.top = self.ly.create_cell("FLOE_WS")
        self.cells = {}  # page cell name -> cell index
        self.design = {}  # page cell name -> design cell name

    def apply(self, delta_path, mats, evict):
        """Read new pages, drop evicted ones, rebuild the top's
        instances to exactly the plan. True = layout changed."""
        changed = False
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
        self.top.clear_insts()
        for m in mats:
            nm, x, y, rot, flip = m[:5]
            if len(m) > 5:
                self.design[nm] = m[5]
            ci = self.cells.get(nm)
            if ci is not None:
                self.top.insert(db.CellInstArray(
                    ci, db.Trans(rot, flip, db.Vector(x, y))))
        return changed


class Mosaic:
    """A Layout that accumulates cache tiles on demand with LRU eviction.

    Banded caches load per (tile, size-band): keys are (r, c, k) and a
    render passes `bands` so wide views never parse the fine-band files
    (see cache._tile_bands). Legacy caches - and any Mosaic built with
    an explicit path_fn, like the LOD companion mosaic - keep one file
    per tile with (r, c) keys."""

    def __init__(self, cache, path_fn=None):
        self.cache = cache
        self.path_fn = path_fn or cache.tile_path
        # size bands apply to the main tile mosaic only; an explicit
        # path_fn (LOD tiles) is always single-file-per-tile
        self.bands = cache.n_bands() if path_fn is None else 1
        # array-heavy caches read in viewer (non-editable) mode:
        # klayout keeps repetitions as compact shape arrays instead of
        # materializing every member - tile loads collapse from tens of
        # seconds to ms and klayout.lay renders arrays natively. Flat
        # caches stay editable (viewer-mode reads are ~3x slower there);
        # see cache.viewer_mode_preferred.
        self.ly = db.Layout(not viewer_mode_preferred(
            cache.meta, getattr(cache, "layout_mode", None)))
        self.ly.dbu = cache.meta["dbu"]
        # pre-create layers in meta order: layer indexes (= draw order
        # and default stipple assignment) must not depend on which tile
        # or band file happens to be read first
        for l in cache.meta["layers"]:
            self.ly.layer(db.LayerInfo(l["layer"], l["datatype"]))
        self.top = self.ly.create_cell("FLOE_MOSAIC")
        self.evict_above = live_caps(cache.meta)[1]
        # (r, c[, k]) -> cell_index or None (empty tile/band)
        self.loaded = {}
        # (r, c[, k]) -> estimated draw members of that subtree,
        # cached at load time (status line "~N drawn")
        self.counts = {}
        # live cell indexes, used to spot what each read() created
        self._known = {self.top.cell_index()}

    def apply_visibility(self, visible):
        """Rebuild the top cell's instance list to exactly the loaded
        keys in `visible` (None = all). Cells left out are simply not
        part of the drawn tree - unlike LayoutView.hide_cell, which
        paints every hidden cell as a white bbox frame (measured: the
        band cut used to leave white tile-boundary lines on wide
        views). clear_insts + insert are viewer-mode-safe."""
        self.top.clear_insts()
        for key, ci in self.loaded.items():
            if ci is not None and (visible is None or key in visible):
                self.top.insert(db.CellInstArray(ci, db.Trans()))

    def _members(self, cell, memo):
        """Estimated draw members of a cell subtree: stored shape
        counts (klayout's Shapes.size() counts array members) times
        instance multiplicity. O(cells + instances) - cheap next to
        the file parse that just happened."""
        ci = cell.cell_index()
        n = memo.get(ci)
        if n is None:
            n = sum(cell.shapes(li).size()
                    for li in self.ly.layer_indexes())
            for inst in cell.each_inst():
                n += inst.size() * self._members(
                    self.ly.cell(inst.cell_index), memo)
            memo[ci] = n
        return n

    def keys_for(self, tiles, bands=None, merged=None):
        """Load keys for `tiles`: all bands by default (exact content -
        snap/pick/clip), or only the given band indexes (renders).
        merged: band indexes whose MERGED TWIN to load instead of the
        raw band - the coarse stand-in a render shows for bands the
        cut drops (never part of snap/pick/clip)."""
        if self.bands == 1:
            return list(tiles)
        ks = range(self.bands) if bands is None else bands
        keys = [(r, c, k) for (r, c) in tiles for k in ks]
        if merged:
            keys += [(r, c, k, "m") for (r, c) in tiles for k in merged]
        return keys

    def _band_file(self, key):
        if self.bands == 1:
            return self.path_fn(*key), f"TILE_{key[0]}_{key[1]}"
        if len(key) == 4:   # merged twin of band k
            r, c, k, _ = key
            return (self.cache.merge_tile_path(r, c, k),
                    f"TILE_{r}_{c}_m{k}")
        r, c, k = key
        return (self.cache.band_tile_path(r, c, k),
                f"TILE_{r}_{c}_b{k}")

    def ensure(self, tiles, stop=None, bands=None, merged=None):
        """Load missing tiles; returns True if the layout changed.
        stop: optional callable checked between tile loads - loading a
        fat tile can take seconds, and newer work (a pan) must not wait
        for the rest; progress made so far is kept."""
        changed = False
        keys = self.keys_for(tiles, bands, merged)
        for key in keys:
            if stop is not None and stop():
                return changed
            if key in self.loaded:
                self.loaded[key] = self.loaded.pop(key)  # LRU bump
                continue
            path, cellname = self._band_file(key)
            if not os.path.isfile(path):
                self.loaded[key] = None
                continue
            self.ly.read(path)
            cell = self.ly.cell(cellname)
            # Isolate this tile's cells: tag every cell the read
            # created with a tile-unique suffix so a later read can
            # NEVER merge into them. klayout merges same-named cells
            # across reads, which (a) instanced a cell spanning k
            # tiles k times at the same spot - k x draw cost - and
            # (b) let evict/re-read cycles re-merge shapes that
            # prune_cell had left behind in shared cells, growing the
            # mosaic without bound over a session.
            tag = "@t" + "_".join(str(v) for v in key)
            for c in self.ly.each_cell():
                ci = c.cell_index()
                if ci in self._known:
                    continue
                self._known.add(ci)
                c.name = c.name + tag
            if cell is None:
                self.loaded[key] = None
                continue
            self.top.insert(db.CellInstArray(cell.cell_index(), db.Trans()))
            self.loaded[key] = cell.cell_index()
            self.counts[key] = self._members(cell, {})
            changed = True
        # LRU eviction; with tile-isolated cells prune_cell removes
        # exactly the victim's subtree
        active = set(keys)
        evicted = False
        while len(self.loaded) > self.evict_above:
            victim = next((k for k in self.loaded if k not in active), None)
            if victim is None:
                break
            ci = self.loaded.pop(victim)
            self.counts.pop(victim, None)
            if ci is not None:
                self.ly.prune_cell(ci, -1)
                changed = evicted = True
        if evicted:
            # klayout may reuse freed cell indexes; rebuild the
            # known-index set so reused indexes are seen as new
            self._known = {c.cell_index() for c in self.ly.each_cell()}
        return changed
