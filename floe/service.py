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
from .viewport import Mosaic, MAX_LIVE_TILES

_SNAP_CAP = 400   # max shapes examined per snap query
_PICK_CAP = 64    # max candidates per pick query


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


def _svc_snap(cache, mosaic, job, res):
    """Vector snap: nearest vertex within radius wins, else the nearest
    point on an edge."""
    out = {"kind": "snap", "seq": job.get("seq", -1), "found": False,
           "x": job.get("x", 0), "y": job.get("y", 0), "snap": ""}
    try:
        px, py, r = job["x"], job["y"], max(1, job["r"])
        box = db.Box(px - r, py - r, px + r, py + r)
        mosaic.ensure(cache.tiles_for_bbox(box.left, box.bottom,
                                           box.right, box.top))
        sel = set(map(tuple, job["layers"])) if job.get("layers") else None
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
        mosaic.ensure(cache.tiles_for_bbox(box.left, box.bottom,
                                           box.right, box.top))
        sel = set(map(tuple, job["layers"])) if job.get("layers") else None
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
        out.update(found=True, count=len(cands), index=i,
                   layer=info.layer, datatype=info.datatype,
                   lname=info.name or f"{info.layer}/{info.datatype}",
                   cell=cell, area=area,
                   bbox=[bb.left, bb.bottom, bb.right, bb.top], points=pts)
    except Exception as e:
        out["err"] = str(e)
    res.put(out)


def _svc_render(cache, mosaic, renderer, tmp, job, res):
    t0 = time.perf_counter()
    x0, y0, x1, y1 = job["bbox"]
    try:
        tiles = cache.tiles_for_bbox(x0, y0, x1, y1)
        if len(tiles) > MAX_LIVE_TILES:
            res.put({"kind": "error",
                     "msg": f"{len(tiles)} tiles > live limit"})
            return
        if mosaic.ensure(tiles):
            renderer.refresh()
        renderer.render_png(tmp, x0, y0, x1, y1, job["w"], job["h"],
                            visible=job["visible"], depth=job.get("depth"))
        with open(tmp, "rb") as f:
            png = f.read()
        res.put({"kind": "frame", "png": png, "bbox": job["bbox"],
                 "gen": job["gen"], "tiles": len(tiles),
                 "ms": round((time.perf_counter() - t0) * 1000)})
    except Exception as e:  # keep the service alive
        res.put({"kind": "error", "msg": str(e)})


def _svc_clip(cache, job, res):
    t0 = time.perf_counter()
    try:
        x0, y0, x1, y1 = job["bbox"]
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


def _render_service(src, req, res):
    """Entry point of the render process (see RenderWorker)."""
    # terminal Ctrl-C delivers SIGINT to the whole process group; shutdown
    # is coordinated by the parent (None sentinel / terminate), so ignore it
    signal.signal(signal.SIGINT, signal.SIG_IGN)
    try:
        cache = cache_mod.Cache(src)
        cache.load()
        mosaic = Mosaic(cache)
        colors = {(l["layer"], l["datatype"]): l["color"]
                  for l in cache.meta["layers"]}
        renderer = Renderer(mosaic.ly, mosaic.top, colors, hier_offset=2)
    except Exception as e:
        res.put({"kind": "error", "msg": f"render service init failed: {e}"})
        return
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
            if renders:
                _svc_render(cache, mosaic, renderer, tmp, renders[-1], res)
    except (KeyboardInterrupt, EOFError, OSError):
        return  # parent went away or interrupted: exit quietly


class RenderWorker:
    """Runs the klayout render service in a separate process."""

    def __init__(self, cache):
        ctx = mp.get_context("spawn")  # fork would clone GUI/klayout state
        self.req = ctx.Queue()
        self.res = ctx.Queue()
        self._proc = ctx.Process(target=_render_service,
                                 args=(cache.src, self.req, self.res),
                                 daemon=True)

    def start(self):
        self._proc.start()

    def submit(self, job):
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
