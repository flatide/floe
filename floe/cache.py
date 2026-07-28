"""Spatial tile cache for large OASIS files.

`build_index` scans the source file once and produces `<src>.ice/`:

    meta.json           source fingerprint, grid geometry, layer table,
                        per-tile depth-density table, stats
    tiles/t_<r>_<c>.oas one OASIS per grid tile (all layers, absolute coords,
                        geometry cut at tile borders); empty tiles have no file
    tiles_lod/...       depth-limited companion tiles (see _tile_lod)
    skeleton.oas        structural far-zoom model (see build_skeleton)

Subsequent viewer/clip operations load only the tiles intersecting the region
of interest, so they run in milliseconds-to-seconds instead of re-parsing the
whole source file.
"""

import colorsys
import functools
import json
import math
import multiprocessing
import os
import time

print = functools.partial(print, flush=True)

import klayout.db as db

CACHE_VERSION = 7
TILE_TARGET_BYTES = 6_000_000
GRID_MIN, GRID_MAX = 4, 96


def cache_dir_for(src):
    return os.path.abspath(src) + ".ice"


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


def viewer_mode_preferred(meta):
    """Pick the Layout mode for READING this cache's data.

    Repetition-heavy sources (bitcell/fill arrays) materialize far more
    shapes than file bytes; klayout's viewer (non-editable) mode keeps
    them as compact shape arrays, collapsing tile loads from ~44s to ms
    (measured). Flat sources read ~3x SLOWER in viewer mode though, so
    choose by stored-shapes-per-byte: testchip-class flat data ~0.15/B,
    array-monster data ~13/B - threshold 1.0."""
    try:
        shapes = sum(l["stored_shapes"] for l in meta["layers"])
        return shapes / max(1, meta["src"]["size"]) > 1.0
    except (KeyError, TypeError):
        return False


def pick_top_cell(ly, log=None):
    tops = ly.top_cells()
    if len(tops) > 1:
        tops = sorted(tops, key=lambda c: -c.bbox().area())
        if log:
            log(f"[warn] {len(tops)} top cells, using largest: {tops[0].name}")
    return tops[0]


class Cache:
    """Read-side accessor for a built .ice directory."""

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
        if self.meta.get("version") != CACHE_VERSION:
            return True
        st = os.stat(self.src)
        srcinfo = self.meta["src"]
        return (st.st_size != srcinfo["size"]
                or int(st.st_mtime) != srcinfo["mtime"])

    def tile_path(self, r, c):
        return os.path.join(self.dir, "tiles", f"t_{r}_{c}.oas")

    def lod_tile_path(self, r, c):
        return os.path.join(self.dir, "tiles_lod", f"t_{r}_{c}.oas")

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


def _text_layers(ly):
    """Layer indexes that hold at least one text, found by scanning stored
    shapes only (Shapes.each(STexts) is type-indexed, so this skips
    polygon/box shapes and stays cheap even on dense fill layers)."""
    out = []
    for li in ly.layer_indexes():
        for cell in ly.each_cell():
            hit = False
            for _ in cell.shapes(li).each(db.Shapes.STexts):
                hit = True
                break
            if hit:
                out.append(li)
                break
    return out


def collect_texts(ly, top_ci):
    """Gather all text objects (any depth) with top-level coordinates.

    Tiles get texts re-injected with half-open tile assignment so each text
    lands in exactly one tile (clip duplicates edge-coincident texts into
    every adjacent tile). Returns [(layer_index, db.Text in top coords)].

    Only text-bearing layers are expanded. A plain all-layers recursive
    pass expands every SRAM/fill array to reach a handful of labels
    (measured 10s for 6 texts, 724s for 1.4M on an array-heavy file);
    restricting the iterator to the text layers prunes those subtrees
    entirely (~2600x faster, identical output).
    """
    layers = _text_layers(ly)
    if not layers:
        return []
    out = []
    it = db.RecursiveShapeIterator(ly, ly.cell(top_ci), layers)
    it.shape_flags = db.Shapes.STexts
    while not it.at_end():
        out.append((it.layer(), it.shape().text.transformed(it.trans())))
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


def _skel_harvest(ly, dmaps, stop_cell, cell, trans, min_feat, big, n,
                  cap, level):
    """Copy large stored shapes of big cells into the skeleton,
    transformed to top coordinates, onto the twin layer of the level
    the shape lives at (so the far view can honor depth 1 vs 2)."""
    dmap = dmaps[level]
    for li in ly.layer_indexes():
        shapes = cell.shapes(li)
        if shapes.size() == 0 or shapes.size() > 60_000:
            continue  # fill containers hold millions; skip wholesale
        dst = stop_cell.shapes(dmap[li])
        for sh in shapes.each():
            if n >= cap:
                return n
            if sh.is_text():
                continue
            b = sh.bbox()
            if b.width() >= min_feat or b.height() >= min_feat:
                poly = sh.polygon
                if poly is not None:
                    dst.insert(poly.transformed(trans))
                    n += 1
    if level >= max(dmaps):
        return n
    for inst in cell.each_inst():
        if n >= cap:
            break
        gb = inst.bbox()
        if gb.width() < big and gb.height() < big:
            continue
        n = _skel_harvest(ly, dmaps, stop_cell, ly.cell(inst.cell_index),
                          trans * inst.trans, min_feat, big, n, cap,
                          level + 1)
    return n


SKEL_DETAIL_DT = 30000  # datatype offset per level of detail twin layers
SKEL_DETAIL_LEVELS = 2  # harvest big shapes from cells this deep


def build_skeleton(ly, top, texts, out_path, log=print):
    """Structural far-zoom model, written as a tiny skeleton.oas.

    The depth-0 content of the far view (large top-level shapes,
    outline boxes + names of first-level cells on the synthetic layer
    255/0 OUTLINE) sits on the design layers; large shapes stored in
    big level-k cells (power straps, long routes, seal ring; k <=
    SKEL_DETAIL_LEVELS) sit on per-level twin layers (datatype + k *
    SKEL_DETAIL_DT), text labels on the level-1 twin. The render
    service turns level-k twins visible only for far views at depth >=
    k, so the far view honors depth 0/1/2 consistently with the live
    render. A layer split, not a cell split: klayout labels cells cut
    by a hierarchy limit, which would stamp a bogus name across the
    die. Small enough for the render service to load whole at startup,
    so far-zoom views render live and crisp at any scale.
    """
    bbox = top.bbox()
    min_feat = max(1, max(bbox.width(), bbox.height()) // 500)
    big = min_feat * 4
    skel = db.Layout()
    skel.dbu = ly.dbu
    stop_cell = skel.create_cell("SKEL_TOP")
    outline_li = skel.layer(db.LayerInfo(255, 0, "OUTLINE"))
    lmap = {}
    dmaps = {k: {} for k in range(1, SKEL_DETAIL_LEVELS + 1)}
    for li in ly.layer_indexes():
        info = ly.get_info(li)
        lmap[li] = skel.layer(info)
        for k in dmaps:
            dmaps[k][li] = skel.layer(db.LayerInfo(
                info.layer, info.datatype + k * SKEL_DETAIL_DT))
    cap = 300_000
    n = 0
    for li in ly.layer_indexes():
        dst = stop_cell.shapes(lmap[li])
        for sh in top.shapes(li).each():
            if sh.is_text():
                continue
            b = sh.bbox()
            if b.width() >= min_feat or b.height() >= min_feat:
                poly = sh.polygon
                if poly is not None:
                    dst.insert(poly)
                    n += 1
    for inst in top.each_inst():
        if n >= cap:
            log(f"[index] skeleton capped at {cap} shapes")
            break
        gb = inst.bbox()
        if gb.width() < big and gb.height() < big:
            continue
        child = ly.cell(inst.cell_index)
        stop_cell.shapes(outline_li).insert(gb)
        c = gb.center()
        stop_cell.shapes(outline_li).insert(
            db.Text(child.name, db.Trans(db.Vector(c.x, c.y))))
        n = _skel_harvest(ly, dmaps, stop_cell, child, inst.trans,
                          min_feat, big, n, cap, 1)
    for li, text in texts:
        stop_cell.shapes(dmaps[1][li]).insert(text)
    skel.write(out_path, save_opts())
    return {"file": os.path.basename(out_path), "shapes": n}


def add_skeleton(cache, log=print):
    """Upgrade an existing cache in place (one source read, no re-tiling):
    floe index --skeleton-only."""
    t0 = time.perf_counter()
    meta = cache.meta or cache.load()
    log(f"[index] reading {cache.src} for skeleton...")
    ly = db.Layout(not viewer_mode_preferred(meta))
    ly.read(cache.src)
    top = pick_top_cell(ly, log)
    texts = collect_texts(ly, top.cell_index())
    out = os.path.join(cache.dir, "skeleton.oas")
    meta["skeleton"] = build_skeleton(ly, top, texts, out, log)
    with open(cache.meta_path, "w") as f:
        json.dump(meta, f, indent=1)
    log(f"[index] skeleton added: {meta['skeleton']['shapes']} shapes "
        f"({time.perf_counter() - t0:.0f}s)")


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
    ly = db.Layout(not viewer_mode_preferred(cache.meta))
    ly.dbu = cache.meta["dbu"]
    top = ly.create_cell("FLOE_REGION")
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


LOD_SHAPE_CAP = 50_000  # per-tile shape budget of the LOD companion


def _tile_lod(tgt, top_ci, out_path, cap=LOD_SHAPE_CAP):
    """Depth-limited companion tile, cut adaptively: whole hierarchy
    levels are kept while the running distinct-cell shape total stays
    under cap; the cells of the first level beyond become ghosts (bbox
    on the synthetic layer 254/0, so depth-cut renders still draw the
    correct outline frame + name) and deeper cells are dropped.
    Kilobytes where the full tile is megabytes - shallow-depth renders
    load these instead. Built by dup + prune, so no per-shape Python
    loop. Returns the deepest depth the file serves, or None when the
    whole tree fits under cap (then no file is written and the full
    tile doubles as its own LOD)."""
    lod = tgt.dup()
    lvl = {top_ci: 0}
    levels = [[top_ci]]
    while True:
        nxt = []
        for ci in levels[-1]:
            for inst in lod.cell(ci).each_inst():
                ch = inst.cell_index
                if ch not in lvl:
                    lvl[ch] = len(levels)
                    nxt.append(ch)
        if not nxt:
            break
        levels.append(nxt)
    lis = list(lod.layer_indexes())

    def count(cells):
        return sum(lod.cell(ci).shapes(li).size()
                   for ci in cells for li in lis)

    cut = 0
    cum = count(levels[0])
    while cut + 1 < len(levels):
        cum += count(levels[cut + 1])
        if cum > cap:
            break
        cut += 1
    if cut + 1 >= len(levels):
        return None  # whole tree fits: the full tile is small already
    ghost_li = lod.layer(db.LayerInfo(254, 0, "GHOST"))
    for ci in levels[cut + 1]:
        c = lod.cell(ci)
        b = c.bbox()
        c.clear()
        if not b.empty():
            c.shapes(ghost_li).insert(b)
    doomed = [ci for ci, l in lvl.items() if l > cut + 1]
    if doomed:
        lod.delete_cells(doomed)
    lod.write(out_path, save_opts())
    return cut


DENSITY_LEVELS = 12     # depth levels recorded in the per-tile density table


def _tile_density(ly, top_ci, max_levels=DENSITY_LEVELS):
    """Density table for one tile: cumulative shape counts per hierarchy
    level below the tile top, per layer ({"5/1": [n_depth0, ...]}), plus
    "cells" = instance count entering each level. Level k equals the
    viewer's depth k; content deeper than max_levels folds into the last
    shape entry, so it always holds the tile's full total. The viewer
    picks its auto depth from this table without loading any tile: the
    cost of depth d is shapes down to d plus one outline frame per cell
    at level d+1 ("cells" catches the bitcell-array trap where a mid
    depth draws millions of frames)."""
    keys = {li: f"{ly.get_info(li).layer}/{ly.get_info(li).datatype}"
            for li in ly.layer_indexes()}
    total = dict.fromkeys(keys.values(), 0)
    counts = {key: [] for key in keys.values()}
    cells = []
    level, depth = {top_ci: 1}, 0
    while level:
        if depth <= max_levels:
            cells.append(sum(level.values()))
        nxt = {}
        for ci, mult in level.items():
            cell = ly.cell(ci)
            for li, key in keys.items():
                n = cell.shapes(li).size()
                if n:
                    total[key] += n * mult
            for inst in cell.each_inst():
                nxt[inst.cell_index] = nxt.get(inst.cell_index, 0) \
                    + inst.size() * mult
        if depth < max_levels:
            for key in counts:
                counts[key].append(total[key])
        level, depth = nxt, depth + 1
    for key, arr in counts.items():
        arr[-1] = total[key]
    out = {key: arr for key, arr in counts.items() if arr[-1]}
    if out:
        out["cells"] = cells
    return out


def _sample_tile(cache, rc, shape_cap=2000):
    """Structure census of one tile file: instance stats (singles vs
    arrays), per-level cell counts, per-layer shape-type mix with polygon
    vertex counts. Numbers only - no geometry leaves this function."""
    r, c = (int(v) for v in rc.split(","))
    ly = db.Layout(False)  # read-only: viewer mode
    ly.read(cache.tile_path(r, c))
    top = ly.cell(f"TILE_{r}_{c}") or pick_top_cell(ly)
    singles = arrays = elems = 0
    top_arrays = []
    for cell in ly.each_cell():
        for inst in cell.each_inst():
            ia = inst.cell_inst
            if ia.na > 1 or ia.nb > 1:
                arrays += 1
                elems += ia.na * ia.nb
                top_arrays.append(ia.na * ia.nb)
            else:
                singles += 1
    top_arrays = sorted(top_arrays, reverse=True)[:5]
    mix = {}
    for li in ly.layer_indexes():
        info = ly.get_info(li)
        key = f"{info.layer}/{info.datatype}"
        n_box = n_poly = n_path = n_text = 0
        pts = []
        seen = 0
        for cell in ly.each_cell():
            shapes = cell.shapes(li)
            if shapes.size() == 0:
                continue
            for sh in shapes.each():
                if sh.is_box():
                    n_box += 1
                elif sh.is_text():
                    n_text += 1
                elif sh.is_path():
                    n_path += 1
                else:
                    n_poly += 1
                    poly = sh.polygon
                    if poly is not None:
                        pts.append(poly.num_points_hull())
                seen += 1
                if seen >= shape_cap:
                    break
            if seen >= shape_cap:
                break
        tot = n_box + n_poly + n_path + n_text
        if tot:
            pts.sort()
            mix[key] = {
                "box": round(n_box / tot, 3),
                "polygon": round(n_poly / tot, 3),
                "path": round(n_path / tot, 3),
                "text": round(n_text / tot, 3),
                "poly_pts_p50": pts[len(pts) // 2] if pts else 4,
                "poly_pts_max": pts[-1] if pts else 4,
            }
    return {"rc": rc, "cells": ly.cells(),
            "insts": {"singles": singles, "arrays": arrays,
                      "array_elems": elems, "largest_arrays": top_arrays},
            "shape_mix": mix}


def profile_cache(cache, sample_tiles=4, anon=False, log=print):
    """Structure-only profile of an indexed layout: everything
    tools/gen_from_profile.py needs to synthesize a render-performance
    lookalike, and nothing else - counts, sizes and grid numbers, no
    geometry or coordinates; --anon also drops the layer names."""
    meta = cache.meta
    g = meta["grid"]
    layers = [{"layer": l["layer"], "datatype": l["datatype"],
               "name": ("L%d_%d" % (l["layer"], l["datatype"])) if anon
                       else l["name"],
               "stored_shapes": l["stored_shapes"]}
              for l in meta["layers"]]
    tile_sizes = {}
    lod_sizes = {}
    for r in range(g["ny"]):
        for c in range(g["nx"]):
            p = cache.tile_path(r, c)
            if os.path.isfile(p):
                tile_sizes[f"{r},{c}"] = os.path.getsize(p)
            p = cache.lod_tile_path(r, c)
            if os.path.isfile(p):
                lod_sizes[f"{r},{c}"] = os.path.getsize(p)
    dens = (meta.get("density") or {}).get("tiles", {})

    def tile_total(rc):
        t = dens.get(rc) or {}
        return sum(arr[-1] for k, arr in t.items() if k != "cells")

    ranked = sorted(tile_sizes, key=tile_total, reverse=True)
    picks = []
    if ranked and sample_tiles > 0:
        idx = sorted({0, len(ranked) // 4, len(ranked) // 2,
                      len(ranked) - 1})
        picks = [ranked[i] for i in idx][:sample_tiles]
    samples = []
    for rc in picks:
        log(f"[profile] sampling tile {rc}...")
        try:
            samples.append(_sample_tile(cache, rc))
        except Exception as e:
            log(f"[profile][warn] sample {rc} failed: {e}")
    return {
        "profile_version": 1,
        "dbu": meta["dbu"],
        "bbox": meta["bbox"],
        "grid": g,
        "layers": layers,
        "density": meta.get("density"),
        "lod": meta.get("lod"),
        "skeleton": {"shapes": (meta.get("skeleton") or {}).get("shapes")},
        "stats": meta.get("stats"),
        "tile_sizes": tile_sizes,
        "lod_sizes": lod_sizes,
        "samples": samples,
    }


# Tile-build context inherited by fork workers. klayout holds the GIL
# during C++ calls, so threads cannot parallelize tiling; fork workers
# share the loaded source layout copy-on-write instead (no re-read, no
# extra resident memory per worker).
_TILE_CTX = None


def _build_one_tile(rc):
    """Build tile (r, c) from _TILE_CTX. Runs in a fork worker (or inline
    for jobs=1). Returns (r, c, wrote, lod_depth, density, step_times)."""
    ly, top_ci, bbox, grid, cdir, tile_texts, opts = _TILE_CTX
    r, c = rc
    x0 = bbox.left + c * grid["tile_w"]
    y0 = bbox.bottom + r * grid["tile_h"]
    box = db.Box(x0, y0, min(x0 + grid["tile_w"], bbox.right),
                 min(y0 + grid["tile_h"], bbox.top))
    tm = {}
    t = time.perf_counter()
    tgt = db.Layout()
    tgt.dbu = ly.dbu
    # pre-create layers with source infos at identical indexes:
    # clip_into copies shapes onto anonymous layers otherwise, and
    # the OASIS writer silently drops layers without layer/datatype
    for li in ly.layer_indexes():
        tgt.insert_layer_at(li, ly.get_info(li))
    ci = ly.clip_into(top_ci, tgt, box)
    tm["clip"] = time.perf_counter() - t
    cell = tgt.cell(ci)
    texts = tile_texts.get((r, c), ())
    if cell.bbox().empty() and not texts:
        return r, c, False, None, None, tm
    cell.name = f"TILE_{r}_{c}"
    t = time.perf_counter()
    _strip_texts(tgt)
    for li, text in texts:
        cell.shapes(li).insert(text)
    tm["strip"] = time.perf_counter() - t
    t = time.perf_counter()
    compact_instances(tgt)
    tm["compact"] = time.perf_counter() - t
    t = time.perf_counter()
    tgt.write(os.path.join(cdir, "tiles", f"t_{r}_{c}.oas"), opts)
    tm["write"] = time.perf_counter() - t
    t = time.perf_counter()
    lod_d = _tile_lod(tgt, ci, os.path.join(cdir, "tiles_lod",
                                            f"t_{r}_{c}.oas"))
    tm["lod"] = time.perf_counter() - t
    t = time.perf_counter()
    dens = _tile_density(tgt, ci) or None
    tm["density"] = time.perf_counter() - t
    return r, c, True, lod_d, dens, tm


def build_index(src, tile_bytes=TILE_TARGET_BYTES, log=print, jobs=None):
    """Scan the source file once and build the tile cache.

    jobs: fork workers for the tiling phase (None = all cores;
    1 = sequential; platforms without fork fall back to sequential)."""
    t_all = time.perf_counter()
    src = os.path.abspath(src)
    cdir = cache_dir_for(src)
    for sub in ("tiles", "tiles_lod"):  # drop stale files of prior builds
        d = os.path.join(cdir, sub)
        os.makedirs(d, exist_ok=True)
        for f in os.listdir(d):
            os.remove(os.path.join(d, f))

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
    all_texts = collect_texts(ly, top_ci)
    tile_texts = {}
    for li, text in all_texts:
        p = text.trans.disp
        c = min(n - 1, max(0, (p.x - bbox.left) // tile_w))
        r = min(n - 1, max(0, (p.y - bbox.bottom) // tile_h))
        tile_texts.setdefault((r, c), []).append((li, text))
    log(f"[index] {len(all_texts)} texts collected "
        f"({time.perf_counter() - t0:.1f}s)")

    # --- tiles ---
    t0 = time.perf_counter()
    n_files = 0
    done = 0
    density_tiles = {}
    lod_tiles = {}
    step_tot = {}
    coords = [(r, c) for r in range(n) for c in range(n)]
    step = max(1, len(coords) // 10)

    def _breakdown():
        # cumulative per-step wall time; with fork workers these overlap,
        # so they sum above real elapsed - read as relative weights
        return " ".join("%s %.0fs" % kv for kv in sorted(step_tot.items()))

    def take(r, c, wrote, lod_d, dens, tm=None):
        nonlocal n_files, done
        done += 1
        if wrote:
            n_files += 1
        if lod_d is not None:
            lod_tiles[f"{r},{c}"] = lod_d
        if dens:
            density_tiles[f"{r},{c}"] = dens
        if tm:
            for k, v in tm.items():
                step_tot[k] = step_tot.get(k, 0.0) + v
        if done % step == 0 or done == len(coords):
            log(f"[index] tiles {done}/{len(coords)} "
                f"({time.perf_counter() - t0:.0f}s; {_breakdown()})")

    if jobs is None:
        jobs = os.cpu_count() or 1
    jobs = max(1, min(jobs, len(coords)))
    ctx = None
    if jobs > 1:
        try:
            ctx = multiprocessing.get_context("fork")
        except ValueError:
            log("[index][warn] no fork on this platform - tiling "
                "sequentially")
    global _TILE_CTX
    _TILE_CTX = (ly, top_ci, bbox, grid, cdir, tile_texts, save_opts())
    try:
        if ctx is not None:
            log(f"[index] tiling with {jobs} fork workers...")
            with ctx.Pool(jobs) as pool:
                for res in pool.imap_unordered(_build_one_tile, coords):
                    take(*res)
        else:
            for rc in coords:
                take(*_build_one_tile(rc))
    finally:
        _TILE_CTX = None
    t_tiles = time.perf_counter() - t0
    log(f"[index] {n_files} tile files in {t_tiles:.0f}s ({_breakdown()})")

    # --- skeleton (far-zoom structural model) ---
    t0 = time.perf_counter()
    skel_meta = build_skeleton(ly, top, all_texts,
                               os.path.join(cdir, "skeleton.oas"), log)
    log(f"[index] skeleton: {skel_meta['shapes']} shapes "
        f"({time.perf_counter() - t0:.1f}s)")

    meta = {
        "version": CACHE_VERSION,
        "src": {"path": src, "size": st.st_size, "mtime": int(st.st_mtime)},
        "dbu": ly.dbu,
        "top_cell": top.name,
        "bbox": [bbox.left, bbox.bottom, bbox.right, bbox.top],
        "grid": grid,
        "layers": layers,
        "density": {"levels": DENSITY_LEVELS, "tiles": density_tiles},
        "lod": {"cap": LOD_SHAPE_CAP, "tiles": lod_tiles},
        "skeleton": skel_meta,
        "stats": {"read_s": round(t_read, 1), "tiles_s": round(t_tiles, 1),
                  "total_s": 0.0,
                  "cells": ly.cells(), "tile_files": n_files},
    }

    meta["stats"]["total_s"] = round(time.perf_counter() - t_all, 1)
    with open(os.path.join(cdir, "meta.json"), "w") as f:
        json.dump(meta, f, indent=1)
    log(f"[index] done in {time.perf_counter() - t_all:.0f}s -> {cdir}")
    return meta
