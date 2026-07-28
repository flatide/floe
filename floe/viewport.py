"""Toolkit-independent viewer core: lazily grown mosaic of cache tiles.

Used by the native GUI (gui.py); all klayout objects live here, the GUI
shell only displays PNG bytes.
"""

import os

import klayout.db as db

from .cache import viewer_mode_preferred

MAX_LIVE_TILES = 32     # a render request may touch at most this many tiles
EVICT_ABOVE = 128       # keep at most this many loaded entries in the mosaic


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
        self.ly = db.Layout(not viewer_mode_preferred(cache.meta))
        self.ly.dbu = cache.meta["dbu"]
        # pre-create layers in meta order: layer indexes (= draw order
        # and default stipple assignment) must not depend on which tile
        # or band file happens to be read first
        for l in cache.meta["layers"]:
            self.ly.layer(db.LayerInfo(l["layer"], l["datatype"]))
        self.top = self.ly.create_cell("FLOE_MOSAIC")
        # (r, c[, k]) -> cell_index or None (empty tile/band)
        self.loaded = {}

    def keys_for(self, tiles, bands=None):
        """Load keys for `tiles`: all bands by default (exact content -
        snap/pick/clip), or only the given band indexes (renders)."""
        if self.bands == 1:
            return list(tiles)
        ks = range(self.bands) if bands is None else bands
        return [(r, c, k) for (r, c) in tiles for k in ks]

    def _band_file(self, key):
        if self.bands == 1:
            return self.path_fn(*key), f"TILE_{key[0]}_{key[1]}"
        r, c, k = key
        return (self.cache.band_tile_path(r, c, k),
                f"TILE_{r}_{c}_b{k}")

    def ensure(self, tiles, stop=None, bands=None):
        """Load missing tiles; returns True if the layout changed.
        stop: optional callable checked between tile loads - loading a
        fat tile can take seconds, and newer work (a pan) must not wait
        for the rest; progress made so far is kept."""
        changed = False
        keys = self.keys_for(tiles, bands)
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
            if cell is None:
                self.loaded[key] = None
                continue
            self.top.insert(db.CellInstArray(cell.cell_index(), db.Trans()))
            self.loaded[key] = cell.cell_index()
            changed = True
        # LRU eviction; prune_cell spares subcells still shared with
        # other loaded tiles (multi-read merges same-named cells)
        active = set(keys)
        while len(self.loaded) > EVICT_ABOVE:
            victim = next((k for k in self.loaded if k not in active), None)
            if victim is None:
                break
            ci = self.loaded.pop(victim)
            if ci is not None:
                self.ly.prune_cell(ci, -1)
                changed = True
        return changed
