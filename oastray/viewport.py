"""Toolkit-independent viewer core: lazily grown mosaic of cache tiles.

Used by the native GUI (gui.py); all klayout objects live here, the GUI
shell only displays PNG bytes.
"""

import os

import klayout.db as db

MAX_LIVE_TILES = 32     # a render request may touch at most this many tiles
EVICT_ABOVE = 128       # keep at most this many tiles in the mosaic


class Mosaic:
    """A Layout that accumulates cache tiles on demand with LRU eviction."""

    def __init__(self, cache):
        self.cache = cache
        self.ly = db.Layout()
        self.ly.dbu = cache.meta["dbu"]
        self.top = self.ly.create_cell("OT_MOSAIC")
        self.loaded = {}  # (r, c) -> cell_index or None (empty tile)

    def ensure(self, tiles):
        """Load missing tiles; returns True if the layout changed."""
        changed = False
        for rc in tiles:
            if rc in self.loaded:
                self.loaded[rc] = self.loaded.pop(rc)  # LRU bump
                continue
            r, c = rc
            path = self.cache.tile_path(r, c)
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
