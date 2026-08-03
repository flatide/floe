"""Render service: all klayout work in a dedicated process.

Toolkit-independent - the GUI shell (gui.py) talks to RenderWorker over
multiprocessing queues only. A thread would not do: klayout's C++ render
loop holds the GIL, so a long render (e.g. a depth-limited view over a
large cell array) would freeze the GUI main loop for its whole duration.
"""

import multiprocessing as mp
import os
import queue
import signal
import tempfile
import time

import klayout.db as db

from . import cache as cache_mod
from .render import Renderer
from .viewport import VfsMosaic

_SNAP_CAP = 400   # max shapes examined per snap query
_PICK_CAP = 64    # max candidates per pick query

# the viewer cut LEVEL (the `c` dialog / view --cut-level); the px
# value behind each level is an implementation detail. Level 0 = off
# (draw all), higher = coarser wide views. CUT_PX = the level-1 px.
CUT_LEVEL_PX = (0.0, 2.0, 4.0, 8.0)
CUT_PX = CUT_LEVEL_PX[1]

# Calibre-style label declutter: skeleton text (block names + budget
# labels) is drawn only when few enough of it falls in the view to
# read - so a sparse chip always shows its labels, and a label-dense
# chip shows them only as you zoom in and the viewport holds fewer.
# The count is measured per frame (capped, cheap on the small
# skeleton). Tunable.
LABEL_VIEW_BUDGET = 400

# live-view label declutter: at most one label per this many screen
# pixels (keeps labels non-overlapping as you zoom), capped at
# LABEL_VIEW_BUDGET per frame
LABEL_CELL_PX = 48

# a streamed view completes within this many rounds: the last one
# requests an unlimited round and takes the whole remainder
_MAX_STREAM_ROUNDS = 8

# streaming round target: the adaptive budget aims each refinement
# round at ~this much parse time. Smaller = smoother/more responsive
# refinement, larger = chunkier visible stages (0.6.2 behaved like
# ~1300). Env-tunable for field taste; clamped 100..2000 ms.
try:
    _STREAM_TARGET_S = max(0.1, min(2.0, float(
        os.environ.get("FLOE_STREAM_TARGET_MS", "500")) / 1000.0))
except ValueError:
    _STREAM_TARGET_S = 0.5

# coverage handoff: composite the density bitplanes once a finest
# coverage texel projects to at most this many screen pixels. Set
# generously so coverage turns on slightly BEFORE the cut starts
# dropping small features - otherwise a wide window (more pixels for
# the same um view) leaves a zoom band where the shapes are already
# cut but coverage has not kicked in yet (a blank gap). composite()
# only fills black pixels, so turning it on early is harmless while
# real geometry is still present.
COV_MAX_TEXEL_PX = 160.0


def _iter_global_polys(mosaic, layers_sel, box):
    """Yield (polygon|None, text_pos|None, layer_index, cell_name) for
    shapes touching box, in top-level (global) coordinates."""
    ly = mosaic.ly
    top_ci = mosaic.top.cell_index()
    for li in ly.layer_indexes():
        info = ly.get_info(li)
        # the frame/outline layer (255/0) is a draw-only overview aid
        # (cut-frame outlines, skeleton cell boxes) - never pickable
        if (info.layer, info.datatype) == (255, 0):
            continue
        if layers_sel is not None and \
                (info.layer, info.datatype) not in layers_sel:
            continue
        it = ly.begin_shapes_touching(top_ci, li, box)
        while not it.at_end():
            cn = it.cell().name
            if cn.startswith("FRAMES") or cn.startswith("LABELS"):
                # VFS cut-frames / live labels are draw-only stand-ins:
                # never win a snap/pick over real geometry
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
    view = (box.left * dbu, box.bottom * dbu,
            box.right * dbu, box.top * dbu)
    m = VfsMosaic(cache)
    if getattr(cache, "_hier", True):
        r = cache.vfs_client.request(0, view, 1.0, 0.0, layers,
                                     None, probe=True, hier=True)
        if r["names"]:
            m.load_names(r["names"])
        m.apply_hier(r["delta"], r["top"], [])
    else:
        r = cache.vfs_client.request(0, view, 1.0, 0.0, layers,
                                     None, probe=True)
        m.apply(r["delta"], r["mats"], [])
    return m


def _svc_snap(cache, mosaic, job, res):
    """Vector snap: nearest vertex within radius wins, else the nearest
    point on an edge."""
    out = {"kind": "snap", "seq": job.get("seq", -1), "found": False,
           "x": job.get("x", 0), "y": job.get("y", 0), "snap": ""}
    try:
        px, py, r = job["x"], job["y"], max(1, job["r"])
        box = db.Box(px - r, py - r, px + r, py + r)
        sel = set(map(tuple, job["layers"])) if job.get("layers") else None
        # query the render working set (pick/snap = "what you see")
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
        # query the render working set (pick = "what you see")
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
        cname = getattr(mosaic, "design", {}).get(cell, cell)
        out.update(found=True, count=len(cands), index=i,
                   layer=info.layer, datatype=info.datatype,
                   lname=info.name or f"{info.layer}/{info.datatype}",
                   cell=cname, area=area,
                   bbox=[bb.left, bb.bottom, bb.right, bb.top], points=pts)
    except Exception as e:
        out["err"] = str(e)
    res.put(out)


def _load_skel_labels(skel_renderer):
    """Skeleton labels for live-view reuse: (design_layer, dt, x, y,
    string). Block names ride on (255,0); budget labels sit on the
    level-1 twin (d + SKEL_DETAIL_DT) - map them back to the design
    layer so they draw in that layer's color. The skeleton is a flat
    cell, so this is a one-time scan of its text shapes."""
    out = []
    if skel_renderer is None:
        return out
    ly = skel_renderer.layout
    top = skel_renderer.top
    for li in ly.layer_indexes():
        info = ly.get_info(li)
        l, d = info.layer, info.datatype
        if (l, d) == (255, 0):
            dl, dd = 255, 0
        elif cache_mod.SKEL_DETAIL_DT <= d < 2 * cache_mod.SKEL_DETAIL_DT:
            dl, dd = l, d - cache_mod.SKEL_DETAIL_DT
        else:
            continue  # strap polygons / design geometry: no labels
        for sh in top.shapes(li).each():
            if sh.is_text():
                t = sh.text
                out.append((dl, dd, t.trans.disp.x,
                            t.trans.disp.y, t.string))
    return out


def _view_labels(cache, x0, y0, x1, y1, w, h, visible):
    """Skeleton labels inside the view, decluttered to one per
    LABEL_CELL_PX screen cell (Calibre-style: non-overlapping,
    appear as you zoom), capped at LABEL_VIEW_BUDGET. Respects the
    visible-layer set (block names always allowed)."""
    src = getattr(cache, "_live_labels", None)
    if not src:
        return []
    vis = None if visible is None else {tuple(v) for v in visible}
    sx = w / max(1e-9, x1 - x0)
    sy = h / max(1e-9, y1 - y0)
    seen = set()
    out = []
    for (l, d, x, y, s) in src:
        if x < x0 or x > x1 or y < y0 or y > y1:
            continue
        if vis is not None and (l, d) != (255, 0) \
                and (l, d) not in vis:
            continue
        key = (int((x - x0) * sx) // LABEL_CELL_PX,
               int((y - y0) * sy) // LABEL_CELL_PX)
        if key in seen:
            continue
        seen.add(key)
        out.append((l, d, x, y, s))
        if len(out) >= LABEL_VIEW_BUDGET:
            break
    return out


def _skel_text_fits(skel_renderer, x0, y0, x1, y1):
    """True if the skeleton's texts inside the view are few enough to
    read (<= LABEL_VIEW_BUDGET). Counts text shapes touching the view
    box, capped so a label-dense overview stops early. The skeleton
    is one flat cell, so this is a bounded per-layer scan."""
    ly = skel_renderer.layout
    top = skel_renderer.top
    box = db.Box(int(x0), int(y0), int(x1), int(y1))
    n = 0
    for li in ly.layer_indexes():
        it = top.shapes(li).each_touching(box)
        for sh in it:
            if sh.is_text():
                n += 1
                if n > LABEL_VIEW_BUDGET:
                    return False
    return True


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
            # Calibre-style label declutter: count the skeleton texts
            # inside the view (capped) and draw text only if few
            # enough to read. A sparse chip (a few dozen block names)
            # always shows them; a label-dense chip's overview would
            # overflow the budget - text stays off until you zoom in
            # far enough that the viewport holds fewer than the
            # budget. Outline boxes and straps (polygons) always show.
            skel_renderer.set_text_visible(_skel_text_fits(
                skel_renderer, x0, y0, x1, y1))
            skel_renderer.render_png(tmp, x0, y0, x1, y1,
                                     job["w"], job["h"], visible=vis)
            tiles_n = 0
        elif getattr(cache, "vfs_client", None):
            return _svc_render_vfs(cache, mosaic, renderer, tmp,
                                   job, req, res, newer, wait_ms,
                                   t0)
        else:
            res.put({"kind": "error", "msg": "viewer is "
                     "VFS-only; run floe-index vfs"})
            return
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


def _svc_render_vfs(cache, mosaic, renderer, tmp, job, req, res,
                    newer, wait_ms, t0):
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
    hier = getattr(cache, "_hier", True)
    # live-view labels: reuse the skeleton's (already-loaded) labels,
    # filtered to the view and decluttered - so labels appear as you
    # zoom in, Calibre-style, without any text in the pages
    labels = _view_labels(cache, x0, y0, x1, y1, job["w"],
                          job["h"], job["visible"])

    draw_total = [0.0]

    def emit(r, t_load):
        """draw the current working set (+coverage fill) and push
        one frame - the hier streaming path emits one per round;
        load/draw times are CUMULATIVE so the settled status line
        adds up to the wall time"""
        td = time.perf_counter()
        renderer.set_abstract(job.get("abstract"))
        renderer.set_text_visible(bool(labels))
        # the cut-frame outline layer is structural: always drawn,
        # even when the user has narrowed the visible-layer set
        vis_r = (None if job["visible"] is None
                 else list(job["visible"]) + [VfsMosaic.FRAME_LAYER])
        renderer.render_png(tmp, x0, y0, x1, y1, job["w"], job["h"],
                            visible=vis_r, depth=None)
        draw_total[0] += time.perf_counter() - td
        # coverage fill: where the cut dropped real shapes, tint the
        # density bitplanes with the live palette into blank pixels
        # (Calibre-style density; display-only). Only when the cut
        # is active - full detail has the real geometry already.
        cov = getattr(cache, "_coverage", None)
        # handoff: composite only once zoomed out enough that a
        # finest coverage texel is <= COV_MAX_TEXEL_PX on screen -
        # exactly where the cut starts dropping small features
        tex0_px = (cov.tex0[0] * job["w"] / max(1e-9, x1 - x0)
                   if cov is not None else 1e9)
        if cov is not None and job.get("coverage", True) \
                and cut_px > 0 and tex0_px <= COV_MAX_TEXEL_PX:
            try:
                from .coverage import composite as _cov_composite
                vis_set = (None if job["visible"] is None
                           else {tuple(v) for v in job["visible"]})
                rgb, any_cov = cov.view_rgb(
                    x0, y0, x1, y1, job["w"], job["h"], vis_set,
                    cache._cov_colors)
                if any_cov:
                    png = _cov_composite(tmp, rgb)
                else:
                    with open(tmp, "rb") as f:
                        png = f.read()
            except Exception:
                with open(tmp, "rb") as f:
                    png = f.read()
        else:
            with open(tmp, "rb") as f:
                png = f.read()
        out = {"kind": "frame", "png": png, "bbox": job["bbox"],
               "gen": job["gen"], "tiles": r.get("pages", 0),
               "scope": "live", "bg": False,
               "load_ms": round(t_load * 1000),
               "draw_ms": round(draw_total[0] * 1000),
               "wait_ms": wait_ms,
               "ms": round((time.perf_counter() - t0) * 1000)}
        if r.get("partial") == "1":
            # refinement in flight: how many pages are still coming
            out["refining"] = int(r.get("deferred", 0) or 0)
        if cut_px:
            out["cut_um"] = round(cut_px / max(1e-9, px_per_um), 3)
        if isinstance(r.get("members"), int):
            out["drawn"] = r["members"]
        res.put(out)

    if not hier:
        tl = time.perf_counter()
        r = cache.vfs_client.request(job["gen"], view_um, px_per_um,
                                     cut_px, layers,
                                     job.get("depth"))
        if newer():
            return
        if mosaic.apply(r["delta"], r["mats"], r["evict"],
                        r["frames"], labels):
            renderer.refresh()
        t_load = time.perf_counter() - tl
        if newer():
            return
        emit(r, t_load)
        return

    # hier: budgeted streaming rounds (VFS_HIER.md par.5 M3.5). Each
    # response carries up to --stream-kb of new pages, center first;
    # a frame goes out per round, so the first paint lands in ~1s
    # and the rest fills in behind it. Aborting anywhere on a stale
    # job withholds the ack = daemon rollback (par.3.7).
    #
    # Round cap: every round pays a FIXED cost (re-plan of the whole
    # view, WC re-author + re-parse, apply) independent of payload.
    # On tiny-page assets (9.8G field case: 8KB average pages) the
    # adaptive budget once shrank to its floor and shredded one view
    # into hundreds of such rounds. The LAST allowed round therefore
    # requests stream=0 and swallows the whole remainder.
    load_total = 0.0
    rounds = 0
    while True:
        rounds += 1
        final_round = rounds >= _MAX_STREAM_ROUNDS
        tl = time.perf_counter()
        # dedicated monotonic daemon-gen: GUI job gens coalesce/
        # skip, the transaction wants strict increase, and a failed
        # gen is never reused
        mosaic.req_gen += 1
        r = cache.vfs_client.request(
            mosaic.req_gen, view_um, px_per_um, cut_px, layers,
            job.get("depth"), hier=True, ack=mosaic.applied_gen,
            reset=mosaic.need_reset,
            stream_kb=0 if final_round else mosaic.stream_kb)
        mosaic.need_reset = False
        # names= arrives ONCE per daemon run and is view-
        # independent: consume it BEFORE the stale check, or a
        # stale first frame would lose the pick-name table
        if r["names"]:
            mosaic.load_names(r["names"])
        if newer():
            return
        try:
            changed = mosaic.apply_hier(r["delta"], r["top"],
                                        r["evict"], labels,
                                        gen=mosaic.req_gen)
        except Exception:
            # partial apply must not carry into the next gen
            # (par.3.7): rebuild the mosaic in place, then replay
            # this view once on a fresh gen with reset=1
            mosaic.reset_all()
            renderer.top = mosaic.top
            renderer.refresh()
            mosaic.req_gen += 1
            r = cache.vfs_client.request(
                mosaic.req_gen, view_um, px_per_um, cut_px, layers,
                job.get("depth"), hier=True, ack=0, reset=True)
            mosaic.need_reset = False
            if r["names"]:
                mosaic.load_names(r["names"])
            changed = mosaic.apply_hier(r["delta"], r["top"],
                                        r["evict"], labels,
                                        gen=mosaic.req_gen)
        if changed:
            renderer.refresh()
        if os.environ.get("FLOE_DEBUG"):
            import sys as _sys
            print("[svc] gen=%s job=%s pages=%s new=%s partial=%s "
                  "plan_ms=%s newer=%s kb=%s" %
                  (mosaic.req_gen, job["gen"], r.get("pages"),
                   r.get("new"), r.get("partial"),
                   r.get("plan_ms"), newer(), mosaic.stream_kb),
                  file=_sys.stderr, flush=True)
        t_round = time.perf_counter() - tl
        load_total += t_round
        # adapt the round budget toward ~0.35s of parse per round -
        # decoded bytes only approximate klayout's cost, and fill
        # distributions vary chip to chip (review finding). ONLY a
        # round that actually shipped a meaningful chunk (>= half
        # the budget) is a valid speed sample: extrapolating from a
        # tiny warm round once inflated the budget past the whole
        # heavy view, which collapsed streaming into one long round
        # and killed the visible staging (field report). The 32MB
        # ceiling keeps a 100MB-class view at 3+ visible stages.
        try:
            shipped_kb = float(
                r.get("pending_new_mb", 0) or 0) * 1024
        except (TypeError, ValueError):
            shipped_kb = 0.0
        if not getattr(mosaic, "stream_pinned", False) \
                and t_round > 0.02 \
                and shipped_kb >= 0.5 * mosaic.stream_kb:
            ideal = mosaic.stream_kb * _STREAM_TARGET_S / t_round
            mosaic.stream_kb = int(
                max(2048, min(32768,
                              (mosaic.stream_kb + ideal) / 2)))
        if newer():
            return
        emit(r, load_total)
        if r.get("partial") != "1" or newer():
            return
        # a refinement can run for seconds: serve interactive
        # queries between rounds (renders self-supersede via
        # `latest`; snap/pick answer against the partial mosaic -
        # WYSIWYG, they see exactly what is on screen)
        try:
            while True:
                j = req.get_nowait()
                if j is None:
                    req.put(None)  # shutdown: propagate, stop
                    return
                k = j.get("kind")
                if k == "clip":
                    _svc_clip(cache, j, res)
                elif k == "snap":
                    _svc_snap(cache, mosaic, j, res)
                elif k == "pick":
                    _svc_pick(cache, mosaic, j, res)
                else:
                    # render job: put it back - `latest` was bumped
                    # at submit, so the next newer() check ends this
                    # refinement and the outer loop picks it up
                    req.put(j)
                    break
        except queue.Empty:
            pass


def _svc_clip(cache, job, res):
    t0 = time.perf_counter()
    try:
        x0, y0, x1, y1 = job["bbox"]
        m = _probe_layout(
            cache, db.Box(int(x0), int(y0), int(x1), int(y1)),
            [tuple(l) for l in job["layers"]]
            if job.get("layers") else None)
        ly, top = m.ly, m.top
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
        # viewer is VFS-only
        if not cache.meta.get("vfs"):
            res.put({"kind": "error", "msg": "not a VFS cache; "
                     "run: floe-index vfs <file.oas>"})
            return
        from .vfsclient import VfsClient
        cache.vfs_client = VfsClient(cache.dir)
        # V4 hierarchy-preserving working set is the default;
        # FLOE_VFS_MODE=flat keeps the legacy flat path for A/B
        # (retired in M5, rust/VFS_HIER.md par.5)
        cache._hier = os.environ.get("FLOE_VFS_MODE",
                                     "hier") != "flat"
        mosaic = VfsMosaic(cache)
        fcolors = dict(colors)
        fcolors[VfsMosaic.FRAME_LAYER] = "#93a4ad"
        renderer = Renderer(mosaic.ly, mosaic.top, fcolors,
                            hier_offset=0,
                            hollow=(VfsMosaic.FRAME_LAYER,))
        lod = None
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
    # VFS live view reuses the skeleton's labels (already loaded, <=
    # 50k budget) - no page text, no build/size cost; see _view_labels
    if cache.meta.get("vfs"):
        cache._live_labels = _load_skel_labels(skel_renderer)
        # coverage bitplanes (density overview for cut/wide views)
        cache._coverage = None
        covp = os.path.join(cache.dir, "design.ovc")
        cache._cov_colors = {
            (l["layer"], l["datatype"]): l["color"]
            for l in cache.meta["layers"]}
        if os.path.isfile(covp):
            try:
                from .coverage import Coverage
                cache._coverage = Coverage(covp)
            except Exception:
                cache._coverage = None
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
