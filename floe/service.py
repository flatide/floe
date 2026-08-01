"""Render service: all klayout work in a dedicated process.

Toolkit-independent - the GUI shell (gui.py) talks to RenderWorker over
multiprocessing queues only. A thread would not do: klayout's C++ render
loop holds the GIL, so a long render (e.g. a depth-limited view over a
large cell array) would freeze the GUI main loop for its whole duration.
"""

import multiprocessing as mp
import os
import queue
import re
import signal
import tempfile
import time

import klayout.db as db

from . import cache as cache_mod
from .render import Renderer
from .viewport import Mosaic, VfsMosaic, live_caps

_SNAP_CAP = 400   # max shapes examined per snap query
_PICK_CAP = 64    # max candidates per pick query

# shapes smaller than this many screen pixels are cut from live renders
# (their whole size band is neither loaded nor drawn - a merged twin
# stands in when the cache has one); snap/pick/clip always use every
# band, so measurements stay exact. Users pick a CUT LEVEL (the `c`
# dialog / view --cut-level); the px value behind each level is an
# implementation detail that may be retuned without changing what
# "level 1" means to a user. CUT_PX = the level-1 default.
CUT_LEVEL_PX = (0.0, 2.0, 4.0, 8.0)
CUT_PX = CUT_LEVEL_PX[1]

# first-visit renders turn progressive when at least this many band-file
# bytes still need parsing (~0.5s at the measured ~18 MB/s parse rate):
# coarse bands + twins paint a near-complete frame first, fine bands
# sharpen it as they parse. Below the threshold a single pass is quicker
# than the extra intermediate draws.
PROGRESSIVE_MIN_BYTES = 8_000_000


def _bands_for_view(cache, x0, x1, w, cut_px=None):
    """(needed band indexes, cut size in dbu) for a view of w pixels
    across x1-x0 dbu; (None, None) on legacy caches. A band is needed
    when its largest shapes would reach the cut size in pixels
    (cut_px, falling back to the CUT_PX default; 0 = no cut)."""
    b = cache.meta.get("bands")
    if not b:
        return None, None
    cp = CUT_PX if cut_px is None else max(0.0, float(cut_px))
    th_dbu = [t / cache.meta["dbu"] for t in b["thresholds_um"]]
    nb = len(th_dbu) + 1
    cutoff = cp * (x1 - x0) / max(1, w)
    need = tuple(k for k in range(nb)
                 if k == 0 or th_dbu[nb - 1 - k] > cutoff)
    return need, cutoff


def _apply_band_sets(mosaic, renderer, raw, twins):
    """Show loaded RAW band cells for bands in `raw`, merged-twin cells
    for bands in `twins`, drop everything else from the drawn tree.
    (Rebuilding the mosaic top's instance list replaced hide_cell,
    which painted every hidden cell as a white bbox frame - visible as
    white tile-boundary lines on cut views.)"""
    if mosaic.bands == 1:
        return
    mosaic.apply_visibility(
        {key for key, ci in mosaic.loaded.items() if ci is not None
         and key[2] in (twins if len(key) == 4 else raw)})


def _apply_bands(mosaic, renderer, need):
    """Steady-state visibility: raw cells for the needed bands, merged
    twins standing in for the bands the cut drops - coarse slabs in
    the layer's live colors, so the cut no longer blanks fill fields."""
    if need is None:
        return
    _apply_band_sets(mosaic, renderer, set(need),
                     set(range(mosaic.bands)) - set(need))


_TWIN_RE = re.compile(r"^TILE_\d+_\d+_m\d+")


def _strip_band(name):
    """User-facing cell name: drop the mosaic's tile-isolation tag
    (@t<key>), size-band markers (__b<k> suffix or TILE_r_c_b<k>) and
    klayout's $n rename counters after them."""
    name = re.sub(r"@t[\d_m]+$", "", name)
    name = re.sub(r"__b\d+(\$\d+)?$", "", name)
    return re.sub(r"^(TILE_\d+_\d+)_b\d+(\$\d+)?$", r"\1", name)


def _iter_global_polys(mosaic, layers_sel, box):
    """Yield (polygon|None, text_pos|None, layer_index, cell_name) for
    shapes touching box, in top-level (global) coordinates."""
    ly = mosaic.ly
    top_ci = mosaic.top.cell_index()
    for li in ly.layer_indexes():
        info = ly.get_info(li)
        if layers_sel is not None and \
                (info.layer, info.datatype) not in layers_sel:
            continue
        it = ly.begin_shapes_touching(top_ci, li, box)
        while not it.at_end():
            if _TWIN_RE.match(it.cell().name):
                # merged twins are draw-only stand-ins: their fused
                # slabs must never win a snap/pick over real geometry
                it.next()
                continue
            sh = it.shape()
            if sh.is_text():
                t = sh.text.transformed(it.trans())
                yield None, (t.trans.disp.x, t.trans.disp.y), li, \
                    it.cell().name
            else:
                poly = sh.polygon
                if poly is not None:
                    yield poly.transformed(it.trans()), None, li, \
                        it.cell().name
            it.next()


def _probe_layout(cache, box, layers):
    """VFS caches: an exact (cut=0, session-less) throwaway working
    set for a small query box - the pick/snap/clip counterpart of
    'load every band'."""
    dbu = cache.meta["dbu"]
    r = cache.vfs_client.request(
        0,
        (box.left * dbu, box.bottom * dbu,
         box.right * dbu, box.top * dbu),
        1.0, 0.0, layers, None, probe=True)
    m = VfsMosaic(cache)
    m.apply(r["delta"], r["mats"], [])
    return m


def _exact_source(cache, mosaic, box, layers):
    """The layout to measure against: probe working set (VFS) or the
    tile mosaic with every band loaded (.ice)."""
    if getattr(cache, "vfs_client", None):
        return _probe_layout(cache, box, layers)
    mosaic.ensure(cache.tiles_for_bbox(box.left, box.bottom,
                                       box.right, box.top))
    mosaic.apply_visibility(None)   # measure against every band
    return mosaic


def _svc_snap(cache, mosaic, job, res):
    """Vector snap: nearest vertex within radius wins, else the nearest
    point on an edge."""
    out = {"kind": "snap", "seq": job.get("seq", -1), "found": False,
           "x": job.get("x", 0), "y": job.get("y", 0), "snap": ""}
    try:
        px, py, r = job["x"], job["y"], max(1, job["r"])
        box = db.Box(px - r, py - r, px + r, py + r)
        sel = set(map(tuple, job["layers"])) if job.get("layers") else None
        mosaic = _exact_source(cache, mosaic, box,
                               sorted(sel) if sel else None)
        r2 = float(r) * r
        best_v = best_e = None
        n = 0
        for poly, tpos, _li, _cell in _iter_global_polys(mosaic, sel, box):
            n += 1
            if n > _SNAP_CAP:
                break
            if poly is None:
                d2 = float(tpos[0] - px) ** 2 + float(tpos[1] - py) ** 2
                if d2 <= r2 and (best_v is None or d2 < best_v[0]):
                    best_v = (d2, tpos[0], tpos[1])
                continue
            for pt in poly.each_point_hull():
                d2 = float(pt.x - px) ** 2 + float(pt.y - py) ** 2
                if d2 <= r2 and (best_v is None or d2 < best_v[0]):
                    best_v = (d2, pt.x, pt.y)
            for edge in poly.each_edge():
                vx, vy = edge.x2 - edge.x1, edge.y2 - edge.y1
                length2 = float(vx) * vx + float(vy) * vy
                if length2 == 0:
                    continue
                t = ((px - edge.x1) * vx + (py - edge.y1) * vy) / length2
                t = 0.0 if t < 0 else (1.0 if t > 1 else t)
                qx, qy = edge.x1 + t * vx, edge.y1 + t * vy
                d2 = (qx - px) ** 2 + (qy - py) ** 2
                if d2 <= r2 and (best_e is None or d2 < best_e[0]):
                    best_e = (d2, qx, qy)
        if best_v is not None:
            out.update(found=True, x=int(best_v[1]), y=int(best_v[2]),
                       snap="vertex")
        elif best_e is not None:
            out.update(found=True, x=int(round(best_e[1])),
                       y=int(round(best_e[2])), snap="edge")
    except Exception as e:
        out["err"] = str(e)
    res.put(out)


def _svc_pick(cache, mosaic, job, res):
    """Calibre-style pick: shapes containing the point, smallest first;
    job['nth'] cycles through overlapping candidates."""
    out = {"kind": "pick", "seq": job.get("seq", -1), "found": False,
           "count": 0}
    try:
        px, py, r = job["x"], job["y"], max(1, job["r"])
        box = db.Box(px - r, py - r, px + r, py + r)
        sel = set(map(tuple, job["layers"])) if job.get("layers") else None
        mosaic = _exact_source(cache, mosaic, box,
                               sorted(sel) if sel else None)
        p = db.Point(px, py)
        ly = mosaic.ly
        cands = []
        for poly, tpos, li, cell in _iter_global_polys(mosaic, sel, box):
            if poly is None:
                if abs(tpos[0] - px) <= r and abs(tpos[1] - py) <= r:
                    cands.append((0.0, li, cell, None, tpos))
            elif poly.inside(p):
                cands.append((float(poly.area()), li, cell, poly, None))
            if len(cands) >= _PICK_CAP:
                break
        if not cands:
            res.put(out)
            return
        cands.sort(key=lambda c: (c[0], ly.get_info(c[1]).layer,
                                  ly.get_info(c[1]).datatype))
        i = job.get("nth", 0) % len(cands)
        area, li, cell, poly, tpos = cands[i]
        info = ly.get_info(li)
        if poly is not None:
            bb = poly.bbox()
            pts = [(pt.x, pt.y) for pt in poly.each_point_hull()][:512]
        else:
            bb = db.Box(tpos[0] - 1, tpos[1] - 1, tpos[0] + 1, tpos[1] + 1)
            pts = []
        cname = getattr(mosaic, "design", {}).get(cell) \
            or _strip_band(cell)
        out.update(found=True, count=len(cands), index=i,
                   layer=info.layer, datatype=info.datatype,
                   lname=info.name or f"{info.layer}/{info.datatype}",
                   cell=cname, area=area,
                   bbox=[bb.left, bb.bottom, bb.right, bb.top], points=pts)
    except Exception as e:
        out["err"] = str(e)
    res.put(out)


def _drawn_estimate(cache, mosaic, tiles, need, x0, y0, x1, y1):
    """~ members inside the rendered bbox: per tile-band counts cached
    at load time, scaled by each tile's overlap fraction with the
    view (klayout culls off-view content at cell/array granularity, so
    the whole-tile count would overstate near zooms)."""
    if not mosaic.counts:
        return None
    g = cache.meta["grid"]
    tw, th = g["tile_w"], g["tile_h"]
    if not tw or not th:
        return None
    twin = ([k for k in range(mosaic.bands) if k not in need]
            if need is not None else None)
    total = 0.0
    for (r, c) in tiles:
        tx0 = g["x0"] + c * tw
        ty0 = g["y0"] + r * th
        ox = max(0, min(x1, tx0 + tw) - max(x0, tx0))
        oy = max(0, min(y1, ty0 + th) - max(y0, ty0))
        if ox <= 0 or oy <= 0:
            continue
        frac = (ox * oy) / float(tw * th)
        for key in mosaic.keys_for([(r, c)], need, twin):
            n = mosaic.counts.get(key)
            if n:
                total += n * frac
    return int(total)


def _svc_render(cache, mosaic, renderer, lod, skel_renderer, tmp, job,
                req, res, latest=None):
    t0 = time.perf_counter()
    x0, y0, x1, y1 = job["bbox"]
    scope = job.get("scope", "live")
    bg = False
    t_load = t_draw = 0.0
    cut_kv = {}
    # how long the job waited behind earlier service work (same host,
    # wall clocks comparable); shown in the status line as "+N wait"
    wait_ms = round((time.time() - job.get("t_sub", time.time())) * 1000)
    # a newer render was submitted: this job is stale - abort fat work
    # and just drop it (the GUI ignores frames of superseded gens anyway)
    newer = (lambda: latest.value > job["gen"]) if latest is not None \
        else (lambda: False)
    try:
        if scope == "skel":
            if skel_renderer is None:
                res.put({"kind": "error", "msg": "no skeleton in cache "
                         "(run: floe index --skeleton-only)"})
                return
            vis = job["visible"]
            if vis is None:
                vis = [(l["layer"], l["datatype"])
                       for l in cache.meta["layers"]]
            else:
                vis = [tuple(v) for v in vis]
            # level-k detail twins (straps, labels) show at depth >= k
            d = job.get("depth")
            kmax = cache_mod.SKEL_DETAIL_LEVELS if d is None \
                else max(0, min(cache_mod.SKEL_DETAIL_LEVELS, d))
            vis += [(l, dt + k * cache_mod.SKEL_DETAIL_DT)
                    for k in range(1, kmax + 1) for l, dt in vis]
            vis += [(255, 0)]          # cell outlines stay visible
            skel_renderer.render_png(tmp, x0, y0, x1, y1,
                                     job["w"], job["h"], visible=vis)
            tiles_n = 0
        elif getattr(cache, "vfs_client", None):
            return _svc_render_vfs(cache, mosaic, renderer, tmp,
                                   job, res, newer, wait_ms, t0)
        else:
            tiles = cache.tiles_for_bbox(x0, y0, x1, y1)
            if len(tiles) > live_caps(cache.meta)[0]:
                res.put({"kind": "error",
                         "msg": f"{len(tiles)} tiles > live limit"})
                return
            depth = job.get("depth")
            need, cut_dbu = _bands_for_view(cache, x0, x1, job["w"],
                                            job.get("cut_px"))
            cut_um = (round(cut_dbu * cache.meta["dbu"], 3)
                      if need is not None and len(need) < mosaic.bands
                      else None)
            cut_kv = {"cut_um": cut_um} if cut_um else {}
            # bands the cut drops render as their merged twins (coarse
            # slab stand-ins); missing twin files load as None once
            twin = (tuple(k for k in range(mosaic.bands)
                          if k not in need)
                    if need is not None else None)
            if lod is not None and depth is not None and \
                    all(lod[2].get("%d,%d" % rc, 99) >= depth
                        for rc in tiles):
                # shallow depth: the tiny LOD tiles carry everything
                # this render can show - skip the full-tile parse
                # (tiles absent from the map fit whole under the cap)
                use_mosaic, use_renderer = lod[0], lod[1]
            else:
                use_mosaic, use_renderer = mosaic, renderer
            if use_mosaic is mosaic and lod is not None:
                # progressive depth: parsing a fat first-visit tile can
                # take tens of seconds (array-heavy designs materialize
                # ~10^8 shapes per tile), so serve an instant preview
                # from the tiny LOD tiles first; content beyond a tile's
                # LOD cut is simply absent until the full frame lands.
                fresh_fat = [rc for rc in tiles
                             if any(key not in mosaic.loaded for key in
                                    mosaic.keys_for([rc], need))
                             and "%d,%d" % rc in lod[2]]
                if fresh_fat:
                    pv = job.get("view") or (x0, y0, x1, y1)
                    ptiles = cache.tiles_for_bbox(*pv)
                    if lod[0].ensure(ptiles):
                        lod[1].refresh()
                    pw = max(1, round(job["w"] * (pv[2] - pv[0])
                                      / max(1, x1 - x0)))
                    ph = max(1, round(job["h"] * (pv[3] - pv[1])
                                      / max(1, y1 - y0)))
                    lod[1].render_png(tmp, pv[0], pv[1], pv[2], pv[3],
                                      pw, ph, visible=job["visible"],
                                      depth=depth)
                    with open(tmp, "rb") as f:
                        png = f.read()
                    res.put({"kind": "frame", "png": png, "bbox": pv,
                             "gen": job["gen"], "tiles": len(ptiles),
                             "scope": scope, "preview": True,
                             "ms": round((time.perf_counter() - t0)
                                         * 1000)})
                    if newer():
                        return  # superseded: the newer job renders next
            if use_mosaic is mosaic and need is not None \
                    and len(need) > 1:
                # progressive band frames (Calibre-style build-up):
                # coarse bands paint a near-complete first frame in the
                # time their small files parse - twins stand in for the
                # bands still pending - and each finer band re-renders
                # the view as its files land. Every step is a full
                # klayout pass over what is shown, so layer draw order
                # stays exact in every frame. Only first visits with
                # enough unparsed bytes bother (the intermediate draws
                # are not free).
                order = sorted(need)
                pending_bytes = 0
                for k in order[1:]:
                    for (r, c) in tiles:
                        if (r, c, k) not in mosaic.loaded:
                            try:
                                pending_bytes += os.path.getsize(
                                    cache.band_tile_path(r, c, k))
                            except OSError:
                                pass
                if pending_bytes > PROGRESSIVE_MIN_BYTES:
                    for i, k in enumerate(order[:-1]):
                        pend = order[i + 1:]
                        tl = time.perf_counter()
                        ch = mosaic.ensure(
                            tiles, stop=newer, bands=(k,),
                            merged=tuple(pend) + tuple(twin or ()))
                        t_load += time.perf_counter() - tl
                        if newer():
                            return
                        if ch:
                            renderer.refresh()
                        _apply_band_sets(
                            mosaic, renderer, set(order[:i + 1]),
                            set(pend) | set(twin or ()))
                        renderer.set_abstract(job.get("abstract"))
                        renderer.render_png(tmp, x0, y0, x1, y1,
                                            job["w"], job["h"],
                                            visible=job["visible"],
                                            depth=depth)
                        with open(tmp, "rb") as f:
                            png = f.read()
                        res.put({"kind": "frame", "png": png,
                                 "bbox": job["bbox"], "gen": job["gen"],
                                 "tiles": len(tiles), "scope": scope,
                                 "preview": True, **cut_kv,
                                 "ms": round((time.perf_counter() - t0)
                                             * 1000)})
                        if newer():
                            return
            view = job.get("view")
            if use_mosaic is mosaic and view is not None:
                # two-phase margin render: when the overdraw margin
                # multiplies the first-visit tile parse, serve the view
                # region first and upgrade to the full margin silently
                vx0, vy0, vx1, vy1 = view
                vtiles = cache.tiles_for_bbox(vx0, vy0, vx1, vy1)
                fresh = sum(1 for key in mosaic.keys_for(tiles, need)
                            if key not in mosaic.loaded)
                vfresh = sum(1 for key in mosaic.keys_for(vtiles, need)
                             if key not in mosaic.loaded)
                if fresh - vfresh >= 4 and x1 > x0 and y1 > y0:
                    tl = time.perf_counter()
                    if mosaic.ensure(vtiles, stop=newer, bands=need,
                                     merged=twin):
                        renderer.refresh()
                    t_load += time.perf_counter() - tl
                    if newer():
                        return  # superseded mid-load: drop this job
                    vw = max(1, round(job["w"] * (vx1 - vx0) / (x1 - x0)))
                    vh = max(1, round(job["h"] * (vy1 - vy0) / (y1 - y0)))
                    td = time.perf_counter()
                    _apply_bands(mosaic, renderer, need)
                    renderer.set_abstract(job.get("abstract"))
                    renderer.render_png(tmp, vx0, vy0, vx1, vy1, vw, vh,
                                        visible=job["visible"],
                                        depth=depth)
                    t_draw = time.perf_counter() - td
                    with open(tmp, "rb") as f:
                        png = f.read()
                    drawn = _drawn_estimate(cache, mosaic, vtiles, need,
                                            vx0, vy0, vx1, vy1)
                    res.put({"kind": "frame", "png": png,
                             "bbox": (vx0, vy0, vx1, vy1),
                             "gen": job["gen"], "tiles": len(vtiles),
                             "scope": scope, "drawn": drawn,
                             "load_ms": round(t_load * 1000),
                             "draw_ms": round(t_draw * 1000),
                             "wait_ms": wait_ms, **cut_kv,
                             "ms": round(
                                 (time.perf_counter() - t0) * 1000)})
                    if not req.empty():
                        return  # newer work queued: skip the margin
                    bg = True   # margin upgrade: no status churn
            tl = time.perf_counter()
            if use_mosaic.ensure(tiles, stop=newer, bands=need,
                                 merged=twin if use_mosaic is mosaic
                                 else None):
                use_renderer.refresh()
            t_load += time.perf_counter() - tl
            if newer():
                return  # superseded mid-load: drop this job
            td = time.perf_counter()
            if use_mosaic is mosaic:
                _apply_bands(mosaic, renderer, need)
            use_renderer.set_abstract(job.get("abstract"))
            use_renderer.render_png(tmp, x0, y0, x1, y1,
                                    job["w"], job["h"],
                                    visible=job["visible"], depth=depth)
            t_draw = time.perf_counter() - td
            tiles_n = len(tiles)
            drawn = _drawn_estimate(
                cache, use_mosaic, tiles,
                need if use_mosaic is mosaic else None,
                x0, y0, x1, y1)
        with open(tmp, "rb") as f:
            png = f.read()
        out = {"kind": "frame", "png": png, "bbox": job["bbox"],
               "gen": job["gen"], "tiles": tiles_n, "scope": scope,
               "bg": bg,
               "load_ms": round(t_load * 1000),
               "draw_ms": round(t_draw * 1000),
               "wait_ms": wait_ms, **cut_kv,
               "ms": round((time.perf_counter() - t0) * 1000)}
        if scope != "skel" and drawn is not None:
            out["drawn"] = drawn
        res.put(out)
    except Exception as e:  # keep the service alive
        res.put({"kind": "error", "msg": str(e)})


def _svc_render_vfs(cache, mosaic, renderer, tmp, job, res, newer,
                    wait_ms, t0):
    """VFS render: one vfsd round-trip plans the viewport, the delta
    file carries only the pages the working set lacks, and klayout
    draws the rebuilt FLOE_WS. Depth semantics live in the plan (the
    daemon stops descending), so klayout renders at full hierarchy."""
    x0, y0, x1, y1 = job["bbox"]
    dbu = cache.meta["dbu"]
    view_um = (x0 * dbu, y0 * dbu, x1 * dbu, y1 * dbu)
    px_per_um = job["w"] / max(1e-9, (x1 - x0) * dbu)
    cut_px = job.get("cut_px")
    cut_px = CUT_PX if cut_px is None else max(0.0, float(cut_px))
    vis = job["visible"]
    layers = [tuple(v) for v in vis] if vis is not None else None
    tl = time.perf_counter()
    r = cache.vfs_client.request(job["gen"], view_um, px_per_um,
                                 cut_px, layers, job.get("depth"))
    if newer():
        return
    if mosaic.apply(r["delta"], r["mats"], r["evict"]):
        renderer.refresh()
    t_load = time.perf_counter() - tl
    if newer():
        return
    td = time.perf_counter()
    renderer.set_abstract(job.get("abstract"))
    renderer.render_png(tmp, x0, y0, x1, y1, job["w"], job["h"],
                        visible=job["visible"], depth=None)
    t_draw = time.perf_counter() - td
    with open(tmp, "rb") as f:
        png = f.read()
    out = {"kind": "frame", "png": png, "bbox": job["bbox"],
           "gen": job["gen"], "tiles": r.get("pages", 0),
           "scope": "live", "bg": False,
           "load_ms": round(t_load * 1000),
           "draw_ms": round(t_draw * 1000), "wait_ms": wait_ms,
           "ms": round((time.perf_counter() - t0) * 1000)}
    if cut_px:
        out["cut_um"] = round(cut_px / max(1e-9, px_per_um), 3)
    if isinstance(r.get("members"), int):
        out["drawn"] = r["members"]
    res.put(out)


def _svc_clip(cache, job, res):
    t0 = time.perf_counter()
    try:
        x0, y0, x1, y1 = job["bbox"]
        if getattr(cache, "vfs_client", None):
            m = _probe_layout(
                cache, db.Box(int(x0), int(y0), int(x1), int(y1)),
                [tuple(l) for l in job["layers"]]
                if job.get("layers") else None)
            ly, top = m.ly, m.top
        else:
            ly, top, _ = cache_mod.load_region(cache, x0, y0, x1, y1)
        ci = ly.clip(top.cell_index(), db.Box(x0, y0, x1, y1))
        ly.cell(ci).name = "FLOE_CLIP"
        opt = cache_mod.save_opts()
        opt.add_cell(ci)
        if job.get("layers"):
            sel = {tuple(l) for l in job["layers"]}
            opt.deselect_all_layers()
            for li in ly.layer_indexes():
                info = ly.get_info(li)
                if (info.layer, info.datatype) in sel:
                    opt.add_layer(li, db.LayerInfo())
        ly.write(job["out"], opt)
        res.put({"kind": "clip", "path": job["out"],
                 "size_mb": os.path.getsize(job["out"]) / 1e6,
                 "ms": round((time.perf_counter() - t0) * 1000)})
    except Exception as e:
        res.put({"kind": "error", "msg": f"clip failed: {e}"})


def _render_service(src, req, res, latest=None):
    """Entry point of the render process (see RenderWorker)."""
    # terminal Ctrl-C delivers SIGINT to the whole process group; shutdown
    # is coordinated by the parent (None sentinel / terminate), so ignore it
    signal.signal(signal.SIGINT, signal.SIG_IGN)
    try:
        cache = cache_mod.Cache(src)
        cache.load()
        colors = {(l["layer"], l["datatype"]): l["color"]
                  for l in cache.meta["layers"]}
        if cache.meta.get("vfs"):
            from .vfsclient import VfsClient
            cache.vfs_client = VfsClient(cache.dir)
            mosaic = VfsMosaic(cache)
            renderer = Renderer(mosaic.ly, mosaic.top, colors,
                                hier_offset=0)
            lod = None
        else:
            mosaic = Mosaic(cache)
            renderer = Renderer(mosaic.ly, mosaic.top, colors,
                                hier_offset=2)
            lod = None
        if not cache.meta.get("vfs") and cache.meta.get("lod"):
            def _lod_path(r, c):
                # tiles small enough to need no LOD double as their own
                p = cache.lod_tile_path(r, c)
                return p if os.path.isfile(p) else cache.tile_path(r, c)
            lod_mosaic = Mosaic(cache, _lod_path)
            lod = (lod_mosaic,
                   Renderer(lod_mosaic.ly, lod_mosaic.top, colors,
                            hier_offset=2),
                   cache.meta["lod"]["tiles"])
    except Exception as e:
        res.put({"kind": "error", "msg": f"render service init failed: {e}"})
        return
    skel_renderer = None
    sk = cache.meta.get("skeleton")
    if sk:
        skel_path = os.path.join(cache.dir, sk["file"])
        if os.path.isfile(skel_path):
            try:
                skel_ly = db.Layout(False)  # read-only: viewer mode
                skel_ly.read(skel_path)
                colors2 = dict(colors)
                for k in range(1, cache_mod.SKEL_DETAIL_LEVELS + 1):
                    colors2.update(  # detail twins keep the layer color
                        {(l, d + k * cache_mod.SKEL_DETAIL_DT): col
                         for (l, d), col in colors.items()})
                colors2[(255, 0)] = "#93a4ad"
                skel_renderer = Renderer(skel_ly, skel_ly.top_cell(),
                                         colors2, hier_offset=1,
                                         show_texts=True,
                                         hollow=((255, 0),))
            except Exception:
                skel_renderer = None
    tmp = os.path.join(tempfile.gettempdir(), f"floe_gui_{os.getpid()}.png")
    try:
        while True:
            job = req.get()
            if job is None:
                return
            # coalesce: run every clip job, but only the newest render job
            jobs = [job]
            while True:
                try:
                    jobs.append(req.get_nowait())
                except queue.Empty:
                    break
            if None in jobs:
                return
            for j in jobs:
                if j["kind"] == "clip":
                    _svc_clip(cache, j, res)
            # snap/pick are cheap and interactive: serve before renders
            snaps = [j for j in jobs if j["kind"] == "snap"]
            if snaps:
                _svc_snap(cache, mosaic, snaps[-1], res)
            picks = [j for j in jobs if j["kind"] == "pick"]
            if picks:
                _svc_pick(cache, mosaic, picks[-1], res)
            renders = [j for j in jobs if j["kind"] == "render"]
            if renders:  # newest by gen: requeued aborted jobs must lose
                _svc_render(cache, mosaic, renderer, lod, skel_renderer,
                            tmp, max(renders, key=lambda j: j["gen"]),
                            req, res, latest)
    except (KeyboardInterrupt, EOFError, OSError):
        return  # parent went away or interrupted: exit quietly
    finally:
        vc = getattr(cache, "vfs_client", None)
        if vc is not None:
            vc.stop()


class RenderWorker:
    """Runs the klayout render service in a separate process."""

    def __init__(self, cache):
        ctx = mp.get_context("spawn")  # fork would clone GUI/klayout state
        self.req = ctx.Queue()
        self.res = ctx.Queue()
        # newest render gen submitted; lets the service abort a fat tile
        # load the moment the user has moved on (queues can't be peeked)
        self.latest = ctx.Value("i", 0)
        self._proc = ctx.Process(target=_render_service,
                                 args=(cache.src, self.req, self.res,
                                       self.latest),
                                 daemon=True)

    def start(self):
        self._proc.start()

    def alive(self):
        return self._proc.is_alive()

    def exitcode(self):
        return self._proc.exitcode

    def submit(self, job):
        if job.get("kind") == "render" and "gen" in job:
            self.latest.value = max(self.latest.value, job["gen"])
        self.req.put(job)

    def stop(self):
        try:
            self.req.put(None)
        except Exception:
            pass
        self._proc.join(timeout=1.5)
        if self._proc.is_alive():
            self._proc.terminate()
            self._proc.join(timeout=1.0)
        for q in (self.req, self.res):  # avoid leaked-semaphore warnings
            try:
                q.close()
                q.cancel_join_thread()
            except Exception:
                pass
