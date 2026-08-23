"""Backend selection and the legacy KLayout render service.

Toolkit-independent - the GUI shell (gui.py) talks to RenderWorker over
multiprocessing queues only.  The KLayout modules are loaded lazily so the
Rust worker and GUI startup path do not require the KLayout Python package.
The legacy renderer still uses a process because its C++ render loop holds
the GIL.
"""

import multiprocessing as mp
import os
import queue
import signal
import tempfile
import time

from . import cache as cache_mod
from .view_policy import frame_layer

db = Renderer = VfsMosaic = None


def _load_klayout_backend():
    """Load modules owned exclusively by the rollback renderer."""
    global db, Renderer, VfsMosaic
    if db is not None:
        return
    import klayout.db as klayout_db
    from .render import Renderer as KLayoutRenderer
    from .viewport import VfsMosaic as KLayoutVfsMosaic
    db = klayout_db
    Renderer = KLayoutRenderer
    VfsMosaic = KLayoutVfsMosaic

_SNAP_CAP = 400   # max shapes examined per snap query
_PICK_CAP = 64    # max candidates per pick query

# the viewer DETAIL level (the `d` dialog / view --detail); the px
# threshold behind each level is an implementation detail. Higher
# detail = smaller cut px = finer wide views. There is no "off":
# drawing everything at a wide view has no realistic performance
# and exposed the frame-cap throttle artifact, so the coarsest
# reachable level is "low".
DETAIL_LEVELS = ("low", "medium", "high")
DETAIL_PX = (5.0, 3.0, 1.0)     # low = coarsest cut, high = finest
DEFAULT_DETAIL = 1              # medium
CUT_PX = DETAIL_PX[DEFAULT_DETAIL]

# a streamed view completes within this many rounds: the last one
# requests an unlimited round and takes the whole remainder
_MAX_STREAM_ROUNDS = 8

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
        # the hierarchy-frontier layers are draw-only navigation aids
        # (depth outlines and block labels) - never pickable
        if (info.layer, info.datatype) in {tuple(k)
                                           for k in mosaic._frame_keys}:
            continue
        if layers_sel is not None and \
                (info.layer, info.datatype) not in layers_sel:
            continue
        it = ly.begin_shapes_touching(top_ci, li, box)
        while not it.at_end():
            cn = it.cell().name
            if cn.startswith("FRAMES") or cn.startswith("LABELS"):
                # VFS hierarchy frames/live labels are draw-only aids:
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
    _load_klayout_backend()
    dbu = cache.meta["dbu"]
    view = (box.left * dbu, box.bottom * dbu,
            box.right * dbu, box.top * dbu)
    m = VfsMosaic(cache)
    r = cache.vfs_client.request(0, view, 1.0, 0.0, layers,
                                 None, probe=True)
    if r["names"]:
        m.load_names(r["names"])
    m.apply_hier(r["delta"], r["top"], [])
    return m


def _svc_snap(cache, mosaic, job, res):
    """Vector snap: nearest vertex within radius wins, else the nearest
    point on an edge."""
    _load_klayout_backend()
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
    _load_klayout_backend()
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


def _tsv_unesc(s):
    """reverse of the writer's tsv_esc: backslash, tab, newline"""
    if "\\" not in s:
        return s
    out = []
    i = 0
    while i < len(s):
        c = s[i]
        if c == "\\" and i + 1 < len(s):
            n = s[i + 1]
            out.append({"t": "\t", "n": "\n", "r": "\r",
                        "\\": "\\"}.get(n, n))
            i += 2
        else:
            out.append(c)
            i += 1
    return "".join(out)


def _labels_from(path, cache):
    """rows of the daemon's per-gen label file (v5, kind-explicit):
    txt rows land on their design layer, blk rows on the runtime
    frame layer. The file is request-scoped - vfsclient deletes it
    with the other response files on the next request."""
    if not path or not os.path.isfile(path):
        return []
    fl, fd = frame_layer(cache.meta)
    out = []
    with open(path) as f:
        for ln in f:
            p = ln.rstrip("\n").split("\t")
            if len(p) < 5:
                continue
            if p[0] == "blk":
                l, d = fl, fd
                if len(p) >= 7:
                    # v6 row: rot + tone; gray names land on the
                    # gray frame dt so the text color follows its box
                    try:
                        rot = int(p[4])
                        if p[5] == "0":
                            d = fd + 1
                    except ValueError:
                        continue
                    text_col = 6
                elif len(p) >= 6:
                    try:
                        rot = int(p[4])
                    except ValueError:
                        continue
                    text_col = 5
                else:
                    # Old daemon row: center it, but no orientation
                    # metadata was available.
                    rot = 0
                    text_col = 4
            elif p[0] == "txt":
                ls, _, ds = p[1].partition("/")
                try:
                    l, d = int(ls), int(ds)
                except ValueError:
                    continue
                rot = 0
                text_col = 4
            else:
                continue
            try:
                out.append((l, d, int(p[2]), int(p[3]),
                            _tsv_unesc(p[text_col]), rot,
                            p[0] == "blk"))
            except ValueError:
                continue
    return out


def _svc_render(cache, mosaic, renderer, lod, tmp, job,
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
        if getattr(cache, "vfs_client", None):
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
        if drawn is not None:
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
    # live-view labels (v5): the DAEMON selects them per request
    # (viewport + depth + visible layers + screen scale) and ships
    # a small per-gen file with round 1; later refinement rounds
    # send nolabels=1 and re-apply the same parsed rows
    labels = None

    draw_total = [0.0]

    def emit(r, t_load):
        """draw the current working set (+coverage fill) and push
        one frame - the hier streaming path emits one per round;
        load/draw times are CUMULATIVE so the settled status line
        adds up to the wall time"""
        td = time.perf_counter()
        renderer.set_abstract(job.get("abstract"))
        renderer.set_text_visible(bool(labels))
        # the hierarchy-frontier layer is structural: always drawn,
        # even when the user has narrowed the visible-layer set
        if job["visible"] is None:
            vis_r = (None if job.get("frames", True)
                     else list(mosaic._layer_keys))
        else:
            vis_r = list(job["visible"])
            if job.get("frames", True):
                vis_r.extend(mosaic._frame_keys)
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
               # pages shipped this VIEW (cache misses, cumulative
               # across its stream rounds); tiles is the plan total
               # including already-resident pages
               "new": r.get("new_total", 0),
               "scope": "live", "bg": False,
               "load_ms": round(t_load * 1000),
               # load phases (cumulative ms; plan+delta+apply ~= load)
               "phase_plan": round(plan_total * 1000),
               "phase_delta": round(
                   max(0.0, daemon_total - plan_total) * 1000),
               "phase_apply": round(apply_total * 1000),
               "draw_ms": round(draw_total[0] * 1000),
               "wait_ms": wait_ms,
               "ms": round((time.perf_counter() - t0) * 1000)}
        if isinstance(r.get("max_depth"), int):
            out["max_depth"] = r["max_depth"]
        if r.get("partial") == "1":
            # refinement in flight: how many pages are still coming
            out["refining"] = int(r.get("deferred", 0) or 0)
        if cut_px:
            out["cut_um"] = round(cut_px / max(1e-9, px_per_um), 3)
        if isinstance(r.get("members"), int):
            out["drawn"] = r["members"]
        if r.get("lod"):
            # pages served as merged-coverage variants this gen
            # (M7): surfaced so the user always knows the current
            # frame is a display approximation
            out["lod"] = r["lod"]
        if isinstance(r.get("text_plan_ms"), (int, float)):
            out["text_plan_ms"] = r["text_plan_ms"]
        if isinstance(r.get("plan_ms"), (int, float)):
            out["plan_ms"] = r["plan_ms"]
        for key in ("wc_cells", "inst_edges", "frame_rects"):
            if isinstance(r.get(key), int):
                out[key] = r[key]
        if r.get("labels_truncated"):
            out["labels_truncated"] = True
        if isinstance(r.get("text_place_records"), int):
            out["text_place_records"] = r["text_place_records"]
        res.put(out)

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
    new_total = 0
    # load-phase breakdown (cumulative seconds; sum ~= load_total):
    #   plan  = daemon-side compute (geometry plan + block-name walk)
    #   delta = daemon round-trip minus plan (author + write + IPC)
    #   apply = klayout parse + working-set (WC/frames) rebuild
    plan_total = 0.0
    daemon_total = 0.0
    apply_total = 0.0
    rounds = 0
    while True:
        rounds += 1
        # a newer job may have arrived while the previous round's
        # drain re-queued it: bail BEFORE paying the round's fixed
        # cost (whole-view re-plan) on a stale generation
        if newer():
            return
        final_round = rounds >= _MAX_STREAM_ROUNDS
        tl = time.perf_counter()
        # dedicated monotonic daemon-gen: GUI job gens coalesce/
        # skip, the transaction wants strict increase, and a failed
        # gen is never reused
        mosaic.req_gen += 1
        want = labels is None
        t_req = time.perf_counter()
        r = cache.vfs_client.request(
            mosaic.req_gen, view_um, px_per_um, cut_px, layers,
            job.get("depth"), ack=mosaic.applied_gen,
            reset=mosaic.need_reset,
            stream_kb=0 if final_round else mosaic.stream_kb,
            want_labels=want, lod=job.get("lod", True),
            frames=job.get("frames", True),
            labels=job.get("labels", True))
        d_req = time.perf_counter() - t_req
        daemon_total += d_req
        # daemon-reported compute (ms). plan_ms is the daemon's
        # whole serve window and already CONTAINS the label walk -
        # adding text_plan_ms again once made the status line claim
        # plan > load (150M field report)
        plan_r = float(r.get("plan_ms", 0) or 0)
        plan_total += plan_r / 1000.0
        mosaic.need_reset = False
        # names= arrives ONCE per daemon run and is view-
        # independent: consume it BEFORE the stale check, or a
        # stale first frame would lose the pick-name table
        if r["names"]:
            mosaic.load_names(r["names"])
        if newer():
            return
        if want:
            labels = _labels_from(r.get("labels"), cache)
        t_ap = time.perf_counter()
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
                job.get("depth"), ack=0, reset=True,
                want_labels=False, lod=job.get("lod", True),
                frames=job.get("frames", True),
                labels=job.get("labels", True))
            mosaic.need_reset = False
            if r["names"]:
                mosaic.load_names(r["names"])
            changed = mosaic.apply_hier(r["delta"], r["top"],
                                        r["evict"], labels,
                                        gen=mosaic.req_gen)
        d_ap = time.perf_counter() - t_ap
        apply_total += d_ap
        if changed:
            renderer.refresh()
        if mosaic.debug:
            import sys as _sys
            print("[svc] gen=%s job=%s pages=%s new=%s partial=%s "
                  "plan_ms=%s wc=%s inst=%s frames=%s "
                  "text_ms=%s text_places=%s "
                  "labels_truncated=%s newer=%s kb=%s" %
                  (mosaic.req_gen, job["gen"], r.get("pages"),
                   r.get("new"), r.get("partial"),
                   r.get("plan_ms"), r.get("wc_cells"),
                   r.get("inst_edges"), r.get("frame_rects"),
                   r.get("text_plan_ms"),
                   r.get("text_place_records"),
                   r.get("labels_truncated"), newer(),
                   mosaic.stream_kb),
                  file=_sys.stderr, flush=True)
        t_round = time.perf_counter() - tl
        load_total += t_round
        try:
            new_total += int(r.get("new", 0) or 0)
        except (TypeError, ValueError):
            pass
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
            ideal = mosaic.stream_kb * mosaic.stream_target_s / t_round
            mosaic.stream_kb = int(
                max(2048, min(32768,
                              (mosaic.stream_kb + ideal) / 2)))
        if newer():
            return
        r["new_total"] = new_total
        dr0 = draw_total[0]
        emit(r, load_total)
        # one stderr line per streaming round, PER-ROUND phase costs
        # (the status line only shows the settled cumulative view -
        # the 9.8G refine zones are measured from the terminal;
        # always on, user call 2026-08-10)
        import sys as _sys
        print("[perf] gen=%d round=%d new=%s bytes=%s tiles=%s "
              "plan=%.0fms delta=%.0fms apply=%.0fms "
              "draw=%.0fms total=%.0fms lod=%s refining=%s "
              "settled=%d"
              % (mosaic.req_gen, rounds, r.get("new", 0),
                 r.get("bytes", 0), r.get("pages", 0),
                 plan_r, max(0.0, d_req * 1000 - plan_r),
                 d_ap * 1000,
                 (draw_total[0] - dr0) * 1000,
                 (t_round + draw_total[0] - dr0) * 1000,
                 r.get("lod", 0), r.get("deferred", 0) or 0,
                 0 if r.get("partial") == "1" else 1),
              file=_sys.stderr, flush=True)
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
                elif k == "recolor":
                    _svc_recolor(cache, renderer, j)
                elif k == "repattern":
                    _svc_repattern(cache, renderer, j)
                elif k == "mono":
                    renderer.set_mono(j["on"])
                else:
                    # render job: put it back and end this
                    # refinement NOW - `latest` was bumped at
                    # submit, and riding on to the next newer()
                    # check used to cost one full stale daemon
                    # round first (withheld ack = daemon rollback,
                    # par.3.7)
                    req.put(j)
                    return
        except queue.Empty:
            pass


def _svc_repattern(cache, renderer, job):
    """Live per-layer fill/width change (pattern palette or a
    layerprops load): RESOLVED bitmaps {[l, d]: rows} + outline
    widths. Wholesale replace - the gui owns the mapping."""
    renderer.set_fill_patterns(
        {tuple(k): v for k, v in job["fills"]})
    renderer.set_line_widths(
        {tuple(k): w for k, w in job.get("widths", [])})


def _apply_personal_fills(cache, renderer):
    """Startup: apply the design-default layerprops fills/widths
    (if a file sits next to the source) so the first frame
    already matches."""
    try:
        from .fillpat import fill_index, default_patterns
        rows, _ = cache_mod.load_layer_props(cache.src)
        if not rows:
            return
        pats = default_patterns()
        fills = {}
        widths = {}
        for key, _color, fill, _name, _f1, f2 in rows:
            i = fill_index(fill)
            if i is not None:
                fills[key] = pats[i]
            try:
                w = int(f2)
                if w > 1:
                    widths[key] = w
            except ValueError:
                pass
        if fills:
            renderer.set_fill_patterns(fills)
        if widths:
            renderer.set_line_widths(widths)
    except Exception:
        pass  # personalization must never kill the service


def _svc_recolor(cache, renderer, job):
    """Live layer recolor (palette pick): renderer color table, the
    meta copy, and the coverage tint palette. The gui bumps its
    render key, so the next frame repaints with the new colors."""
    for k, col in job["colors"]:
        key = (int(k[0]), int(k[1]))
        renderer.colors[key] = col
        for l in cache.meta["layers"]:
            if (l["layer"], l["datatype"]) == key:
                l["color"] = col
        cc = getattr(cache, "_cov_colors", None)
        if cc is not None:
            cc[key] = col
    renderer.refresh()


def _svc_clip(cache, job, res):
    _load_klayout_backend()
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


def _render_service(src, req, res, latest=None, options=None):
    """Entry point of the render process (see RenderWorker)."""
    # terminal Ctrl-C delivers SIGINT to the whole process group; shutdown
    # is coordinated by the parent (None sentinel / terminate), so ignore it
    signal.signal(signal.SIGINT, signal.SIG_IGN)
    try:
        _load_klayout_backend()
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
        options = options or {}
        mosaic = VfsMosaic(
            cache, stream_kb=options.get("stream_kb"),
            stream_target_ms=options.get("stream_target_ms", 500),
            debug=options.get("debug", False))
        fcolors = dict(colors)
        # Calibre size bands: the daemon authors each depth-boundary
        # box on dt+band of the frame layer -
        #   dt+0 white outline (ABOVE design, readable in dense fill)
        #   dt+1 gray outline, dt+2 gray fill, dt+3 gray dotted
        #     (all buried UNDER the design geometry).
        fcolors[mosaic.FRAME_LAYER] = "#ffffff"
        fcolors[mosaic.FRAME_GRAY] = "#808080"
        fcolors[mosaic.FRAME_FILL] = "#808080"
        fcolors[mosaic.FRAME_DOTS] = "#808080"
        renderer = Renderer(mosaic.ly, mosaic.top, fcolors,
                            hier_offset=0,
                            # outline bands are hollow; the fill band
                            # (FRAME_FILL) is solid, so it is omitted
                            hollow=(mosaic.FRAME_LAYER,
                                    mosaic.FRAME_GRAY,
                                    mosaic.FRAME_DOTS),
                            above=(mosaic.FRAME_LAYER,),
                            dotted=(mosaic.FRAME_DOTS,),
                            solid=(mosaic.FRAME_FILL,))
        _apply_personal_fills(cache, renderer)
        lod = None
    except Exception as e:
        res.put({"kind": "error", "msg": f"render service init failed: {e}"})
        return
    # VFS live-view labels are request-scoped daemon responses (v5)
    if cache.meta.get("vfs"):
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
            for j in jobs:
                if j["kind"] == "recolor":
                    _svc_recolor(cache, renderer, j)
                elif j["kind"] == "repattern":
                    _svc_repattern(cache, renderer, j)
                elif j["kind"] == "mono":
                    renderer.set_mono(j["on"])
            renders = [j for j in jobs if j["kind"] == "render"]
            if renders:  # newest by gen: requeued aborted jobs must lose
                _svc_render(cache, mosaic, renderer, lod,
                            tmp, max(renders, key=lambda j: j["gen"]),
                            req, res, latest)
    except (KeyboardInterrupt, EOFError, OSError):
        return  # parent went away or interrupted: exit quietly
    finally:
        vc = getattr(cache, "vfs_client", None)
        if vc is not None:
            vc.stop()


def make_render_worker(cache, stream_kb=None, stream_target_ms=500,
                       debug=False):
    """Create the selected render backend without changing GUI callers.

    Rust is the default.  KLayout remains an explicit rollback backend and its
    modules stay unloaded unless that backend is selected and started.
    """
    backend = os.environ.get(
        "FLOE_RENDERER", "rust").strip().lower() or "rust"
    if backend == "klayout":
        worker_type = RenderWorker
    elif backend == "rust":
        target = os.environ.get(
            "FLOE_RUST_WORKER",
            "floe.rust_render:RustRenderWorker")
        module_name, separator, type_name = target.partition(":")
        if not separator or not module_name or not type_name:
            raise RuntimeError(
                "FLOE_RUST_WORKER must be MODULE:TYPE, got %r" % target)
        try:
            import importlib
            module = importlib.import_module(module_name)
            worker_type = getattr(module, type_name)
        except (ImportError, AttributeError) as exc:
            raise RuntimeError(
                "cannot load Rust render worker %r: %s" %
                (target, exc)) from exc
        if not callable(worker_type):
            raise RuntimeError(
                "Rust render worker %r is not callable" % target)
    else:
        raise RuntimeError(
            "FLOE_RENDERER must be klayout or rust, got %r" % backend)
    return worker_type(cache, stream_kb=stream_kb,
                       stream_target_ms=stream_target_ms, debug=debug)


class RenderWorker:
    """Runs the klayout render service in a separate process."""

    supports_abstract = True

    def __init__(self, cache, stream_kb=None, stream_target_ms=500,
                 debug=False):
        ctx = mp.get_context("spawn")  # fork would clone GUI/klayout state
        self.req = ctx.Queue()
        self.res = ctx.Queue()
        # newest render gen submitted; lets the service abort a fat tile
        # load the moment the user has moved on (queues can't be peeked)
        self.latest = ctx.Value("i", 0)
        self._proc = ctx.Process(target=_render_service,
                                 args=(cache.src, self.req, self.res,
                                       self.latest,
                                       {"stream_kb": stream_kb,
                                        "stream_target_ms":
                                            stream_target_ms,
                                        "debug": debug}),
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
