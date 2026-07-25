"""Spatial tile cache for large OASIS files.

`build_index` scans the source file once and produces `<src>.tvcache/`:

    meta.json           source fingerprint, grid geometry, layer table, stats
    tiles/t_<r>_<c>.oas one OASIS per grid tile (all layers, absolute coords,
                        geometry cut at tile borders); empty tiles have no file
    overview/*.png      per-layer full-die renders for far-zoom display

Subsequent viewer/clip operations load only the tiles intersecting the region
of interest, so they run in milliseconds-to-seconds instead of re-parsing the
whole source file.
"""

import colorsys
import functools
import json
import math
import os
import time

print = functools.partial(print, flush=True)

import klayout.db as db

CACHE_VERSION = 1
TILE_TARGET_BYTES = 6_000_000
GRID_MIN, GRID_MAX = 4, 96


def cache_dir_for(src):
    return os.path.abspath(src) + ".tvcache"


def layer_color(i):
    """Distinct, stable per-layer color (golden-angle hue rotation)."""
    h = (i * 137.508) % 360.0 / 360.0
    r, g, b = colorsys.hsv_to_rgb(h, 0.75, 1.0)
    return "#%02x%02x%02x" % (int(r * 255), int(g * 255), int(b * 255))


def save_opts():
    opt = db.SaveLayoutOptions()
    opt.format = "OASIS"
    opt.oasis_write_cblocks = True
    opt.oasis_compression_level = 2
    opt.write_context_info = False
    return opt


def pick_top_cell(ly, log=None):
    tops = ly.top_cells()
    if len(tops) > 1:
        tops = sorted(tops, key=lambda c: -c.bbox().area())
        if log:
            log(f"[warn] {len(tops)} top cells, using largest: {tops[0].name}")
    return tops[0]


class Cache:
    """Read-side accessor for a built .tvcache directory."""

    def __init__(self, src):
        self.src = os.path.abspath(src)
        self.dir = cache_dir_for(src)
        self.meta = None

    @property
    def meta_path(self):
        return os.path.join(self.dir, "meta.json")

    def exists(self):
        return os.path.isfile(self.meta_path)

    def load(self):
        with open(self.meta_path) as f:
            self.meta = json.load(f)
        return self.meta

    def is_stale(self):
        st = os.stat(self.src)
        srcinfo = self.meta["src"]
        return (st.st_size != srcinfo["size"]
                or int(st.st_mtime) != srcinfo["mtime"])

    def tile_path(self, r, c):
        return os.path.join(self.dir, "tiles", f"t_{r}_{c}.oas")

    def tiles_for_bbox(self, x0, y0, x1, y1):
        """Grid tiles (r, c) intersecting bbox in dbu, clamped to the grid."""
        g = self.meta["grid"]
        c0 = max(0, (x0 - g["x0"]) // g["tile_w"])
        c1 = min(g["nx"] - 1, (x1 - 1 - g["x0"]) // g["tile_w"])
        r0 = max(0, (y0 - g["y0"]) // g["tile_h"])
        r1 = min(g["ny"] - 1, (y1 - 1 - g["y0"]) // g["tile_h"])
        if c1 < c0 or r1 < r0:
            return []
        return [(r, c) for r in range(r0, r1 + 1) for c in range(c0, c1 + 1)]

    def resolve_layers(self, spec):
        """Parse 'M1,5/1,MARKER' into [(layer, datatype), ...]. None = all."""
        if not spec or spec == "all":
            return None
        byname = {l["name"]: (l["layer"], l["datatype"])
                  for l in self.meta["layers"] if l.get("name")}
        out = []
        for tok in spec.split(","):
            tok = tok.strip()
            if not tok:
                continue
            if "/" in tok:
                l, d = tok.split("/")
                out.append((int(l), int(d)))
            elif tok in byname:
                out.append(byname[tok])
            else:
                raise ValueError(f"unknown layer: {tok!r} "
                                 f"(known: {sorted(byname)})")
        return out


def collect_texts(ly, top_ci):
    """Gather all text objects (any depth) with top-level coordinates.

    Tiles get texts re-injected with half-open tile assignment so each text
    lands in exactly one tile (clip duplicates edge-coincident texts into
    every adjacent tile). Returns [(layer_index, db.Text in top coords)].
    """
    out = []
    for li in ly.layer_indexes():
        it = ly.begin_shapes(top_ci, li)
        it.shape_flags = db.Shapes.STexts
        while not it.at_end():
            out.append((li, it.shape().text.transformed(it.trans())))
            it.next()
    return out


def _const_pitch_runs(vals):
    """Split a sorted int array into (start, pitch, count) constant-pitch runs."""
    import numpy as np
    n = len(vals)
    if n == 1:
        return [(int(vals[0]), 0, 1)]
    d = np.diff(vals)
    runs = []
    i = 0
    while i < n:
        if i == n - 1:
            runs.append((int(vals[i]), 0, 1))
            break
        pitch = int(d[i])
        j = i + 1
        while j < n - 1 and int(d[j]) == pitch:
            j += 1
        if pitch == 0:  # duplicate points - keep as singles
            runs.append((int(vals[i]), 0, 1))
            i += 1
        else:
            runs.append((int(vals[i]), pitch, j - i + 1))
            i = j
    return runs


def _find_grids(pts):
    """Detect regular grids in an (N,2) int array of points.

    Returns (arrays, leftovers) where arrays are
    (x0, y0, xpitch, nx, ypitch, ny) and leftovers are (x, y) singles.
    """
    import numpy as np
    order = np.lexsort((pts[:, 1], pts[:, 0]))
    pts = pts[order]
    xs, starts = np.unique(pts[:, 0], return_index=True)
    bounds = list(starts[1:]) + [len(pts)]
    col_sigs = {}
    leftovers = []
    for i, x in enumerate(xs):
        ys = pts[starts[i]:bounds[i], 1]
        for y0, pitch, cnt in _const_pitch_runs(ys):
            if cnt == 1:
                leftovers.append((int(x), y0))
            else:
                col_sigs.setdefault((y0, pitch, cnt), []).append(int(x))
    arrays = []
    for (y0, ypitch, ny), xlist in col_sigs.items():
        xarr = np.asarray(sorted(xlist), dtype=np.int64)
        for x0, xpitch, nx in _const_pitch_runs(xarr):
            arrays.append((x0, y0, xpitch, nx, ypitch, ny))
    return arrays, leftovers


def _strip_texts(ly):
    """Remove all text shapes (tiles get exactly-once texts injected)."""
    for cell in ly.each_cell():
        for li in ly.layer_indexes():
            shapes = cell.shapes(li)
            victims = [s for s in shapes.each(db.Shapes.STexts)]
            for s in victims:
                shapes.erase(s)


def compact_instances(ly, min_group=500, log=None):
    """Re-fold exploded single instances into regular CellInstArrays.

    Layout.clip explodes arrays that are cut by the clip box into individual
    placements (millions for memory arrays). Files stay small because the
    OASIS writer re-detects repetitions, but the reader materializes single
    instances again, making tile loads slow and memory-hungry. Rebuilding
    true regular arrays fixes both (regular arrays survive the roundtrip).
    """
    import numpy as np
    for cell in ly.each_cell():
        n_inst = cell.child_instances()
        if n_inst < min_group:
            continue
        groups = {}
        keep = []
        for inst in cell.each_inst():
            ia = inst.cell_inst
            if inst.is_complex() or ia.na > 1 or ia.nb > 1:
                keep.append(ia)
                continue
            t = inst.trans
            groups.setdefault((inst.cell_index, t.rot),
                              []).append((t.disp.x, t.disp.y))
        if not groups or max(len(v) for v in groups.values()) < min_group:
            continue
        rebuilt = []
        n_before = 0
        for (ci, rot), pts in groups.items():
            n_before += len(pts)
            base = db.Trans(rot % 4, rot >= 4, 0, 0)
            if len(pts) < min_group:
                for x, y in pts:
                    rebuilt.append(db.CellInstArray(
                        ci, db.Trans(rot % 4, rot >= 4, x, y)))
                continue
            arrays, singles = _find_grids(
                np.asarray(pts, dtype=np.int64))
            for x0, y0, xp, nx, yp, ny in arrays:
                tr = db.Trans(rot % 4, rot >= 4, x0, y0)
                if nx == 1 and ny == 1:
                    rebuilt.append(db.CellInstArray(ci, tr))
                else:
                    rebuilt.append(db.CellInstArray(
                        ci, tr, db.Vector(xp, 0), db.Vector(0, yp), nx, ny))
            for x, y in singles:
                rebuilt.append(db.CellInstArray(
                    ci, db.Trans(rot % 4, rot >= 4, x, y)))
        cell.clear_insts()
        for ia in keep:
            cell.insert(ia)
        for ia in rebuilt:
            cell.insert(ia)
        if log:
            log(f"[index]   compacted {cell.name}: {n_before} -> "
                f"{len(rebuilt)} instances")


def load_region(cache, x0, y0, x1, y1, log=None, max_tiles=None,
                layers=None):
    """Load tiles intersecting bbox (dbu) into a fresh mosaic Layout.

    Returns (layout, top_cell, n_tiles_loaded). Tile geometry keeps absolute
    coordinates, so tiles are instantiated at identity transform.
    `layers`: optional [(layer, datatype), ...] to read only those layers
    from the tile files (big speed/memory win for layer extraction).
    """
    tiles = cache.tiles_for_bbox(x0, y0, x1, y1)
    if max_tiles is not None and len(tiles) > max_tiles:
        raise RuntimeError(
            f"region spans {len(tiles)} tiles (> max {max_tiles}); "
            f"narrow the bbox or raise --max-tiles")
    lo = None
    if layers is not None:
        lm = db.LayerMap()
        for i, (l, d) in enumerate(layers):
            lm.map(db.LayerInfo(l, d), i)
        lo = db.LoadLayoutOptions()
        lo.set_layer_map(lm, False)
    ly = db.Layout()
    ly.dbu = cache.meta["dbu"]
    top = ly.create_cell("TV_REGION")
    n = 0
    t0 = time.perf_counter()
    for r, c in tiles:
        p = cache.tile_path(r, c)
        if not os.path.isfile(p):
            continue  # empty tile
        if lo is not None:
            ly.read(p, lo)
        else:
            ly.read(p)
        cell = ly.cell(f"TILE_{r}_{c}")
        if cell is None:
            continue
        top.insert(db.CellInstArray(cell.cell_index(), db.Trans()))
        n += 1
    if log:
        log(f"[view] loaded {n}/{len(tiles)} tiles "
            f"in {time.perf_counter() - t0:.2f}s")
    return ly, top, n


def build_index(src, tile_bytes=TILE_TARGET_BYTES, overview_px=1600,
                overview=True, log=print):
    """Scan the source file once and build the tile cache."""
    t_all = time.perf_counter()
    src = os.path.abspath(src)
    cdir = cache_dir_for(src)
    os.makedirs(os.path.join(cdir, "tiles"), exist_ok=True)
    if overview:
        os.makedirs(os.path.join(cdir, "overview"), exist_ok=True)

    st = os.stat(src)
    log(f"[index] reading {src} ({st.st_size / 1e9:.2f} GB)...")
    t0 = time.perf_counter()
    ly = db.Layout()
    ly.read(src)
    t_read = time.perf_counter() - t0
    log(f"[index] read done in {t_read:.0f}s "
        f"({ly.cells()} cells, {len(ly.layer_indexes())} layers)")

    top = pick_top_cell(ly, log)
    bbox = top.bbox()

    # grid: aim for ~tile_bytes per tile file
    n = int(round(math.sqrt(st.st_size / tile_bytes)))
    n = max(GRID_MIN, min(GRID_MAX, n))
    tile_w = -(-bbox.width() // n)   # ceil div
    tile_h = -(-bbox.height() // n)
    grid = {"nx": n, "ny": n, "x0": bbox.left, "y0": bbox.bottom,
            "tile_w": tile_w, "tile_h": tile_h}
    log(f"[index] grid {n}x{n}, tile {tile_w / 1000:.0f} x "
        f"{tile_h / 1000:.0f} um")

    layers = []
    for i, li in enumerate(ly.layer_indexes()):
        info = ly.get_info(li)
        count = sum(cell.shapes(li).size() for cell in ly.each_cell())
        layers.append({"layer": info.layer, "datatype": info.datatype,
                       "name": info.name or f"{info.layer}/{info.datatype}",
                       "color": layer_color(i), "stored_shapes": count})

    # --- texts: clip_into drops them, so bucket them per tile up front ---
    top_ci = top.cell_index()
    t0 = time.perf_counter()
    tile_texts = {}
    n_texts = 0
    for li, text in collect_texts(ly, top_ci):
        p = text.trans.disp
        c = min(n - 1, max(0, (p.x - bbox.left) // tile_w))
        r = min(n - 1, max(0, (p.y - bbox.bottom) // tile_h))
        tile_texts.setdefault((r, c), []).append((li, text))
        n_texts += 1
    log(f"[index] {n_texts} texts collected "
        f"({time.perf_counter() - t0:.1f}s)")

    # --- tiles ---
    t0 = time.perf_counter()
    n_files = 0
    opts = save_opts()
    for r in range(n):
        for c in range(n):
            x0 = bbox.left + c * tile_w
            y0 = bbox.bottom + r * tile_h
            box = db.Box(x0, y0, min(x0 + tile_w, bbox.right),
                         min(y0 + tile_h, bbox.top))
            path = os.path.join(cdir, "tiles", f"t_{r}_{c}.oas")
            tgt = db.Layout()
            tgt.dbu = ly.dbu
            # pre-create layers with source infos at identical indexes:
            # clip_into copies shapes onto anonymous layers otherwise, and
            # the OASIS writer silently drops layers without layer/datatype
            for li in ly.layer_indexes():
                tgt.insert_layer_at(li, ly.get_info(li))
            ci = ly.clip_into(top_ci, tgt, box)
            cell = tgt.cell(ci)
            texts = tile_texts.get((r, c), ())
            if cell.bbox().empty() and not texts:
                continue
            cell.name = f"TILE_{r}_{c}"
            _strip_texts(tgt)
            for li, text in texts:
                cell.shapes(li).insert(text)
            compact_instances(tgt)
            tgt.write(path, opts)
            n_files += 1
        log(f"[index] tiles row {r + 1}/{n} "
            f"({time.perf_counter() - t0:.0f}s)")
    t_tiles = time.perf_counter() - t0
    log(f"[index] {n_files} tile files in {t_tiles:.0f}s")

    # collect meta fields BEFORE overview rendering: show_layout() hands
    # layout ownership to the LayoutView, which destroys it on teardown
    meta = {
        "version": CACHE_VERSION,
        "src": {"path": src, "size": st.st_size, "mtime": int(st.st_mtime)},
        "dbu": ly.dbu,
        "top_cell": top.name,
        "bbox": [bbox.left, bbox.bottom, bbox.right, bbox.top],
        "grid": grid,
        "layers": layers,
        "overview": None,
        "stats": {"read_s": round(t_read, 1), "tiles_s": round(t_tiles, 1),
                  "overview_s": 0.0, "total_s": 0.0,
                  "cells": ly.cells(), "tile_files": n_files},
    }

    if overview:
        from . import render as render_mod
        t0 = time.perf_counter()
        meta["overview"] = render_mod.render_overviews(
            ly, top, layers, os.path.join(cdir, "overview"),
            overview_px, log=log)
        t_ov = time.perf_counter() - t0
        meta["stats"]["overview_s"] = round(t_ov, 1)
        log(f"[index] overviews in {t_ov:.0f}s")

    meta["stats"]["total_s"] = round(time.perf_counter() - t_all, 1)
    with open(os.path.join(cdir, "meta.json"), "w") as f:
        json.dump(meta, f, indent=1)
    log(f"[index] done in {time.perf_counter() - t_all:.0f}s -> {cdir}")
    return meta
