"""Toolkit-independent viewer core: lazily grown mosaic of cache tiles.

Used by the native GUI (gui.py); all klayout objects live here, the GUI
shell only displays PNG bytes.
"""

import os

import klayout.db as db

from .cache import viewer_mode_preferred

MAX_LIVE_TILES = 32     # a render request may touch at most this many tiles
EVICT_ABOVE = 128       # keep at most this many tiles in the mosaic


class Mosaic:
    """A Layout that accumulates cache tiles on demand with LRU eviction.

    path_fn picks the tile file per (r, c); default full tiles, pass
    cache.lod_tile_path for the depth-limited companion tiles."""

    def __init__(self, cache, path_fn=None):
        self.cache = cache
        self.path_fn = path_fn or cache.tile_path
        # array-heavy caches read in viewer (non-editable) mode:
        # klayout keeps repetitions as compact shape arrays instead of
        # materializing every member - tile loads collapse from tens of
        # seconds to ms and klayout.lay renders arrays natively. Flat
        # caches stay editable (viewer-mode reads are ~3x slower there);
        # see cache.viewer_mode_preferred.
        self.ly = db.Layout(not viewer_mode_preferred(cache.meta))
        self.ly.dbu = cache.meta["dbu"]
        self.top = self.ly.create_cell("FLOE_MOSAIC")
        self.loaded = {}  # (r, c) -> cell_index or None (empty tile)

    def ensure(self, tiles, stop=None):
        """Load missing tiles; returns True if the layout changed.
        stop: optional callable checked between tile loads - loading a
        fat tile can take seconds, and newer work (a pan) must not wait
        for the rest; progress made so far is kept."""
        changed = False
        for rc in tiles:
            if stop is not None and stop():
                return changed
            if rc in self.loaded:
                self.loaded[rc] = self.loaded.pop(rc)  # LRU bump
                continue
            r, c = rc
            path = self.path_fn(r, c)
            if not os.path.isfile(path):
                self.loaded[rc] = None
                continue
            self.ly.read(path)
            cell = self.ly.cell(f"TILE_{r}_{c}")
            if cell is None:
                self.loaded[rc] = None
                continue
            self.top.insert(db.CellInstArray(cell.cell_index(), db.Trans()))
            self.loaded[rc] = cell.cell_index()
            changed = True
        # LRU eviction
        active = set(tiles)
        while len(self.loaded) > EVICT_ABOVE:
            victim = next((k for k in self.loaded if k not in active), None)
            if victim is None:
                break
            ci = self.loaded.pop(victim)
            if ci is not None:
                self.ly.delete_cell_rec(ci)
                changed = True
        return changed
