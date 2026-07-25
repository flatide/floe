"""Native desktop viewer built on tkinter.

Toolkit choice: tkinter ships with CPython itself (RHEL: python3-tkinter RPM,
Debian: python3-tk, macOS: python-tk brew formula) so an air-gapped host
needs no extra GUI stack (no PyGObject/GTK or Qt version matching). All
geometry rendering runs in klayout's C++ engine inside a worker thread that
produces PNG frames; the Tk main thread only blits bitmaps and handles
input, so UI responsiveness is independent of layout size.

Far zoom (viewport covering more than MAX_LIVE_TILES tiles) displays the
pre-rendered overview PNGs from the cache: the exact visible layer's image
when exactly one layer is on, otherwise the combined all-layer image. Zoomed
in, tiles are loaded lazily into the mosaic (LRU-bounded) and rendered live
with full layer control.
"""

import math
import multiprocessing as mp
import os
import queue
import signal
import sys
import tempfile
import time

import tkinter as tk
from tkinter import filedialog

import klayout.db as db

from . import cache as cache_mod
from .render import Renderer
from .viewport import Mosaic, MAX_LIVE_TILES

BG = "#000000"
PANEL_BG = "#161616"
PANEL_FG = "#cccccc"
ACCENT = "#8ecdf5"
DEBOUNCE_MS = 120
POLL_MS = 25


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
        ly.cell(ci).name = "ZN_CLIP"
        opt = cache_mod.save_opts()
        opt.add_cell(ci)
        if job.get("layers"):
            sel = set(job["layers"])
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
    tmp = os.path.join(tempfile.gettempdir(), f"zn_gui_{os.getpid()}.png")
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
    """Runs the klayout render service in a separate PROCESS.

    A thread is not enough: klayout's C++ render loop holds the GIL, so a
    long render (e.g. a depth-limited view over a large cell array) would
    freeze the Tk main loop for its whole duration. A process keeps the UI
    responsive no matter how long a frame takes.
    """

    def __init__(self, cache):
        ctx = mp.get_context("spawn")  # fork would clone Tk/klayout state
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


class Viewer:
    def __init__(self, root, cache, server_sock=None):
        self.root = root
        self.server_sock = server_sock  # instance socket (None with --multi)
        if server_sock is not None:
            server_sock.setblocking(False)
        self.cx = self.cy = 0
        self.spp = 1.0  # dbu per screen pixel; set properly on first fit
        self.visible = set()
        self.gen = 0
        self.last_frame = None      # (photo, bbox, dbu_per_px)
        self._ov_photos = {}        # overview source images by key
        self._scaled_photo = None   # keep ref of current scaled blit
        self._last_blit = 0.0
        self._drag = None
        self._zoomdrag = None       # rubber-band anchor (canvas px)
        self._band = None           # rubber-band canvas item
        self._debounce = None
        self._did_fit = False
        self.worker = None
        self._layer_vars = {}
        # ruler / snap / pick state
        self.mode = "normal"        # "normal" | "ruler"
        self.rulers = []            # finished rulers [(x0,y0,x1,y1) dbu]
        self._ruler_start = None    # first point while measuring
        self._ruler_free = False    # Shift held: free angle (default H/V)
        self.snap_on = True         # 'm' toggles vector snap
        self._snap_seq = 0
        self._snap_res = None
        self._snap_sent = 0.0
        self.selection = None       # latest pick result
        self._sel_text = ""
        self._pick_seq = 0
        self._pick_px = None        # canvas px of last pick click
        self._pick_nth = 0
        self._cursor = (0, 0)       # pointer position in dbu

        root.configure(bg=PANEL_BG)
        root.geometry("1280x860")

        side = tk.Frame(root, bg=PANEL_BG, width=210)
        side.pack(side="left", fill="y")
        side.pack_propagate(False)
        tk.Label(side, text="zenoas", fg=ACCENT, bg=PANEL_BG,
                 font=("TkDefaultFont", 13, "bold")).pack(anchor="w",
                                                          padx=10, pady=(8, 0))
        self._src_label = tk.Label(side, text="", fg="#777777", bg=PANEL_BG)
        self._src_label.pack(anchor="w", padx=10)

        self._layers_frame = tk.Frame(side, bg=PANEL_BG)
        self._layers_frame.pack(fill="both", expand=True, padx=6, pady=6)

        self.depth_var = tk.IntVar(value=999)
        df = tk.Frame(side, bg=PANEL_BG)
        df.pack(fill="x", padx=6, pady=(4, 0))
        self._depth_btn = tk.Button(
            df, text="depth: full", command=self._depth_dialog,
            bg="#252d33", fg=PANEL_FG, activebackground="#33414a", bd=0,
            padx=10, highlightthickness=0)
        self._depth_btn.pack(fill="x", padx=2)

        bf = tk.Frame(side, bg=PANEL_BG)
        bf.pack(fill="x", padx=6, pady=6)
        for txt, cmd in (("all", self._all_layers),
                         ("none", self._no_layers),
                         ("fit", self.fit),
                         ("clip…", self._clip_dialog)):
            tk.Button(bf, text=txt, command=cmd, bg="#252d33", fg=PANEL_FG,
                      activebackground="#33414a", bd=0, padx=10,
                      highlightthickness=0).pack(side="left", padx=2)

        main = tk.Frame(root, bg=BG)
        main.pack(side="left", fill="both", expand=True)
        self.canvas = tk.Canvas(main, bg=BG, highlightthickness=0,
                                cursor="crosshair")
        self.canvas.pack(fill="both", expand=True)
        sb = tk.Frame(main, bg="#101010")
        sb.pack(fill="x")
        # right side: render-in-progress indicator (packed first so it
        # stays visible even when the left status text grows long)
        self.rstatus = tk.Label(sb, text="", anchor="e", fg=ACCENT,
                                bg="#101010", padx=10)
        self.rstatus.pack(side="right")
        self.status = tk.Label(sb, text="", anchor="w", fg="#99aabb",
                               bg="#101010", padx=8)
        self.status.pack(side="left", fill="x", expand=True)

        self.img_item = self.canvas.create_image(0, 0, anchor="nw")

        self._pending = None        # gen of the render in flight
        self._pending_t0 = 0.0
        self._pending_timer = None
        self._ddlg = None

        c = self.canvas
        # Calibre-style: left-drag selects a zoom box (forward = zoom in,
        # backward = zoom out); pan is on middle/right drag
        c.bind("<ButtonPress-1>", self._zoom_press)
        c.bind("<B1-Motion>", self._zoom_drag)
        c.bind("<ButtonRelease-1>", self._zoom_release)
        for btn in ("2", "3"):  # aqua: 2 = right; X11: 2 = middle, 3 = right
            c.bind(f"<ButtonPress-{btn}>", self._pan_press)
            c.bind(f"<B{btn}-Motion>", self._motion)
            c.bind(f"<ButtonRelease-{btn}>", self._pan_release)
        c.bind("<MouseWheel>", self._wheel)
        c.bind("<Button-4>", lambda e: self._zoom_at(e.x, e.y, 1 / 1.15))
        c.bind("<Button-5>", lambda e: self._zoom_at(e.x, e.y, 1.15))
        try:
            # Tk 9 delivers macOS trackpad / Magic Mouse scrolling as
            # TouchpadScroll, not MouseWheel
            c.bind("<TouchpadScroll>", self._touchpad)
        except tk.TclError:
            pass
        c.bind("<Motion>", self._hover)
        c.bind("<Configure>", self._configure)
        root.bind("<Key-f>", self._key(self.fit))
        for key in ("<Key-plus>", "<Key-equal>", "<Key-KP_Add>"):
            root.bind(key, self._key(lambda: self._zoom_center(1 / 1.25)))
        for key in ("<Key-minus>", "<Key-KP_Subtract>"):
            root.bind(key, self._key(lambda: self._zoom_center(1.25)))
        root.bind("<Key-r>", self._key(self._toggle_ruler))
        root.bind("<Key-m>", self._key(self._toggle_snap))
        root.bind("<Escape>", self._key(self._esc))
        # Calibre-style depth keys: digit = that depth, 'a' = full
        for d in range(10):
            root.bind(f"<Key-{d}>",
                      self._key(lambda n=d: self._set_depth(n)))
        root.bind("<Key-a>", self._key(lambda: self._set_depth(999)))
        root.protocol("WM_DELETE_WINDOW", self._quit)

        self._apply_cache(cache)
        root.after(POLL_MS, self._poll)

    # ---- cache binding / instance requests --------------------------------
    def _apply_cache(self, cache):
        """Bind the viewer to a cache (initial open and forwarded opens)."""
        self.cache = cache
        self.meta = cache.meta
        self.dbu = self.meta["dbu"]
        bb = self.meta["bbox"]
        self.cx = (bb[0] + bb[2]) / 2
        self.cy = (bb[1] + bb[3]) / 2
        self.visible = {(l["layer"], l["datatype"])
                        for l in self.meta["layers"]}
        self.last_frame = None
        self._ov_photos = {}
        self._scaled_photo = None
        self._clear_pending()
        # measurements/selection belong to the previous file
        self.rulers = []
        self._ruler_start = None
        self._snap_res = None
        self.selection = None
        self._sel_text = ""
        self._pick_px = None
        src = self.meta["src"]
        self.root.title(f"zenoas - {os.path.basename(src['path'])}")
        self._src_label.config(
            text=f"{src['size'] / 1e9:.2f} GB · grid "
                 f"{self.meta['grid']['nx']}x{self.meta['grid']['ny']}")
        self._build_layer_panel()
        if self.worker is not None:
            self.worker.stop()
        self.worker = RenderWorker(cache)
        self.worker.start()
        if self._did_fit:
            self.fit()

    def _build_layer_panel(self):
        for w in self._layers_frame.winfo_children():
            w.destroy()
        self._layer_vars = {}
        for l in self.meta["layers"]:
            key = (l["layer"], l["datatype"])
            var = tk.BooleanVar(value=True)
            cb = tk.Checkbutton(
                self._layers_frame, text=f"{l['name']}  {key[0]}/{key[1]}",
                variable=var, command=self._on_layers_changed, anchor="w",
                fg=l["color"], bg=PANEL_BG, activebackground=PANEL_BG,
                activeforeground=l["color"], selectcolor="#303030",
                highlightthickness=0, bd=0)
            cb.pack(fill="x")
            self._layer_vars[key] = var

    def open_file(self, path):
        """Open another OASIS file (instance-forwarded request).
        Returns None on success or an 'ERR ...' reply line."""
        path = os.path.abspath(path)
        if path == self.cache.src and not self.cache.is_stale():
            return None  # same file: just present the window
        c = cache_mod.Cache(path)
        if not c.exists():
            return f"ERR no index for {path}; run: zenoas index {path}"
        c.load()
        self._apply_cache(c)
        return None

    def _service_socket(self):
        """Accept one forwarded request from a later invocation."""
        try:
            conn, _ = self.server_sock.accept()
        except (BlockingIOError, InterruptedError):
            return
        except OSError:
            return
        conn.settimeout(2.0)
        try:
            data = b""
            while b"\n" not in data and len(data) < 65536:
                chunk = conn.recv(4096)
                if not chunk:
                    break
                data += chunk
            line = data.decode("utf-8", "replace").strip()
            path = line.split("\t")[0].strip()
            if not path:
                error = "ERR empty request"
            else:
                try:
                    error = self.open_file(path)
                except Exception as exc:
                    error = f"ERR {exc}"
            try:
                conn.sendall((error or "OK").encode("utf-8") + b"\n")
            except OSError:
                pass
            if not error:
                self._present()
        finally:
            conn.close()

    def _present(self):
        self.root.deiconify()
        self.root.lift()
        try:
            self.root.focus_force()
        except tk.TclError:
            pass

    # ---- geometry helpers -------------------------------------------------
    def canvas_size(self):
        w = self.canvas.winfo_width()
        h = self.canvas.winfo_height()
        return (w if w >= 50 else 1200), (h if h >= 50 else 800)

    def view_bbox(self):
        w, h = self.canvas_size()
        return (self.cx - w / 2 * self.spp, self.cy - h / 2 * self.spp,
                self.cx + w / 2 * self.spp, self.cy + h / 2 * self.spp)

    def tiles_spanned(self, bbox):
        g = self.meta["grid"]
        c0 = max(0, int((bbox[0] - g["x0"]) // g["tile_w"]))
        c1 = min(g["nx"] - 1, int((bbox[2] - g["x0"]) // g["tile_w"]))
        r0 = max(0, int((bbox[1] - g["y0"]) // g["tile_h"]))
        r1 = min(g["ny"] - 1, int((bbox[3] - g["y0"]) // g["tile_h"]))
        if c1 < c0 or r1 < r0:
            return 0
        return (c1 - c0 + 1) * (r1 - r0 + 1)

    def _visible_list(self):
        return sorted(self.visible)

    # ---- drawing ----------------------------------------------------------
    def redraw(self, immediate=False):
        bbox = self.view_bbox()
        span = self.tiles_spanned(bbox)
        live = 0 < span <= MAX_LIVE_TILES and bool(self.visible)
        if not live:
            if self._debounce is not None:  # a live render is no longer due
                self.root.after_cancel(self._debounce)
                self._debounce = None
            self._clear_pending()
            self._draw_overview(bbox)
            self._set_status(bbox, f"overview ({span} tiles spanned)")
            self._update_overlays()
            return
        self._preview(bbox)
        if self._debounce is not None:
            self.root.after_cancel(self._debounce)
        delay = 1 if immediate else DEBOUNCE_MS
        self._debounce = self.root.after(delay, self._submit_render)
        self._set_status(bbox, f"live ({span} tiles)")
        self._update_overlays()

    def _preview(self, bbox):
        """Immediate scale-correct feedback while the worker renders."""
        if self.last_frame is not None:
            photo, fb, fspp = self.last_frame
            if fspp == self.spp:  # pure pan: reposition only, no copy
                self.canvas.coords(self.img_item,
                                   (fb[0] - bbox[0]) / self.spp,
                                   (bbox[3] - fb[3]) / self.spp)
                self.canvas.itemconfig(self.img_item, image=photo)
                return
        now = time.perf_counter()
        if now - self._last_blit < 0.025:
            return  # throttle scaled blits during wheel/drag bursts
        if self.last_frame is not None:
            photo, fb, fspp = self.last_frame
            if 0.3 < fspp / self.spp < 3.5 \
                    and self._blit_scaled(photo, fb, bbox):
                self._last_blit = time.perf_counter()
                return
        if self._blit_ov(bbox):
            self._last_blit = time.perf_counter()
        # else: keep whatever is on screen; the fresh frame will replace it

    def _submit_render(self):
        self._debounce = None
        bbox = self.view_bbox()
        w, h = self.canvas_size()
        self.gen += 1
        self.worker.submit({
            "kind": "render", "gen": self.gen,
            "bbox": tuple(int(round(v)) for v in bbox),
            "w": w, "h": h,
            "depth": self._depth(),
            "visible": self._visible_list() if len(self.visible) !=
            len(self._layer_vars) else None})
        self._pending = self.gen
        self._pending_t0 = time.perf_counter()
        self.rstatus.config(text="rendering…")  # visible from the start
        if self._pending_timer is None:  # tick adds elapsed seconds
            self._pending_timer = self.root.after(400, self._pending_tick)

    def _pending_tick(self):
        if self._pending is None:
            self._pending_timer = None
            self.rstatus.config(text="")
            return
        el = time.perf_counter() - self._pending_t0
        self.rstatus.config(text="rendering…" if el < 1.5
                            else f"rendering… {el:.0f}s")
        self._pending_timer = self.root.after(400, self._pending_tick)

    def _clear_pending(self):
        self._pending = None
        if self._pending_timer is not None:
            self.root.after_cancel(self._pending_timer)
            self._pending_timer = None
        self.rstatus.config(text="")

    def _key(self, fn):
        """Route a root key binding, swallowing it while a text-entry
        widget (e.g. the depth spinbox) has focus."""
        def handler(_ev):
            w = self.root.focus_get()
            if isinstance(w, (tk.Entry, tk.Spinbox, tk.Text)):
                return
            fn()
        return handler

    def _set_depth(self, n):
        self.depth_var.set(n)
        self._on_depth()

    def _depth(self):
        try:
            d = int(self.depth_var.get())
        except (tk.TclError, ValueError):
            return None
        return None if d >= 999 else max(0, d)

    def _on_depth(self):
        d = self._depth()
        self._depth_btn.config(
            text="depth: full" if d is None else f"depth: {d}")
        self.redraw(immediate=True)

    def _depth_dialog(self):
        if self._ddlg is not None and self._ddlg.winfo_exists():
            self._ddlg.lift()
            return
        dlg = tk.Toplevel(self.root)
        self._ddlg = dlg
        dlg.title("hierarchy depth")
        dlg.configure(bg=PANEL_BG)
        dlg.resizable(False, False)
        dlg.transient(self.root)  # stays above, non-modal: adjust live
        dlg.geometry(f"+{self.root.winfo_rootx() + 240}"
                     f"+{self.root.winfo_rooty() + 80}")
        tk.Label(dlg, text="hierarchy depth (0 = top only, 999 = full)",
                 fg=PANEL_FG, bg=PANEL_BG).pack(padx=14, pady=(12, 6))
        row = tk.Frame(dlg, bg=PANEL_BG)
        row.pack(padx=14, pady=2)
        sp = tk.Spinbox(row, from_=0, to=999, width=5,
                        textvariable=self.depth_var, command=self._on_depth,
                        bg="#252d33", fg=PANEL_FG,
                        buttonbackground="#252d33",
                        insertbackground=PANEL_FG, highlightthickness=0,
                        bd=0)
        sp.pack(side="left", padx=(0, 10))
        sp.bind("<Return>", lambda e: self._on_depth())
        for preset in (0, 1, 2, 3, 999):
            txt = "full" if preset == 999 else str(preset)
            tk.Button(row, text=txt, width=3,
                      command=lambda p=preset: (self.depth_var.set(p),
                                                self._on_depth()),
                      bg="#252d33", fg=PANEL_FG,
                      activebackground="#33414a", bd=0,
                      highlightthickness=0).pack(side="left", padx=1)
        tk.Label(dlg, text="cells beyond the limit are drawn as outline "
                           "frames with names\n(applies to live mode; the "
                           "far-zoom overview stays full depth)\n"
                           "keys: 0-9 = depth, a = full",
                 fg="#777777", bg=PANEL_BG, justify="left").pack(
            padx=14, pady=(6, 4))
        tk.Button(dlg, text="close", command=dlg.destroy, bg="#252d33",
                  fg=PANEL_FG, activebackground="#33414a", bd=0, padx=12,
                  highlightthickness=0).pack(pady=(2, 10))

    def _ov_photo(self, key):
        ov = self.meta.get("overview")
        if not ov:
            return None
        fname = ov["files"].get(key)
        if fname is None:
            return None
        if key not in self._ov_photos:
            path = os.path.join(self.cache.dir, "overview", fname)
            try:
                self._ov_photos[key] = tk.PhotoImage(file=path)
            except tk.TclError:
                self._ov_photos[key] = None
        return self._ov_photos[key]

    def _draw_overview(self, bbox):
        if not self.visible:
            self.canvas.itemconfig(self.img_item, image="")
            return
        if not self._blit_ov(bbox):
            self.canvas.itemconfig(self.img_item, image="")
            self._set_status(bbox, "no overview in cache - zoom in")

    def _blit_ov(self, bbox):
        """Show the pre-rendered overview (single layer's own image when
        exactly one layer is visible, combined image otherwise)."""
        if self.meta.get("overview") is None or not self.visible:
            return False
        if len(self.visible) == 1:
            l, d = next(iter(self.visible))
            key = f"{l}/{d}"
        else:
            key = "__all__"
        src = self._ov_photo(key) or self._ov_photo("__all__")
        if src is None:
            return False
        return self._blit_scaled(src, tuple(self.meta["overview"]["bbox"]),
                                 bbox)

    @staticmethod
    def _best_ratio(m, max_z=16, max_s=64, tol=0.12):
        """Best integer zoom/subsample pair approximating scale m within
        tol relative error, or None (Tk photo copy only scales by z/s)."""
        if m <= 0:
            return None
        best = None
        for s in range(1, max_s + 1):
            z = round(m * s)
            if z < 1 or z > max_z:
                continue
            err = abs(z / s - m) / m
            if best is None or err < best[2]:
                best = (z, s, err)
            if err < 0.001:
                break
        if best is not None and best[2] <= tol:
            return best[0], best[1]
        return None

    def _blit_scaled(self, src, src_bbox, bbox):
        """Crop/scale `src` (a PhotoImage covering src_bbox in dbu) to the
        current view using Tk's integer-rational zoom/subsample. Returns
        False when no reasonable integer approximation exists."""
        W, H = src.width(), src.height()
        if W <= 1 or H <= 1 or src_bbox[2] <= src_bbox[0]:
            return False
        ppd = W / (src_bbox[2] - src_bbox[0])     # source px per dbu
        m = (1.0 / self.spp) / ppd                # screen px per source px
        zs = self._best_ratio(m)
        if zs is None:
            return False
        z, s = zs
        sx0 = max(0, int((bbox[0] - src_bbox[0]) * ppd))
        sy0 = max(0, int((src_bbox[3] - bbox[3]) * ppd))
        sx1 = min(W, int((bbox[2] - src_bbox[0]) * ppd) + 1)
        sy1 = min(H, int((src_bbox[3] - bbox[1]) * ppd) + 1)
        if sx1 <= sx0 or sy1 <= sy0:
            return False
        dst = tk.PhotoImage()
        dst.tk.call(str(dst), "copy", str(src),
                    "-from", sx0, sy0, sx1, sy1,
                    "-zoom", z, z, "-subsample", s, s)
        lx = src_bbox[0] + sx0 / ppd
        ly_ = src_bbox[3] - sy0 / ppd
        self._scaled_photo = dst  # keep a reference or Tk drops the image
        self.canvas.coords(self.img_item, (lx - bbox[0]) / self.spp,
                           (bbox[3] - ly_) / self.spp)
        self.canvas.itemconfig(self.img_item, image=dst)
        return True

    def _poll(self):
        if self.server_sock is not None:
            self._service_socket()
        try:
            while True:
                res = self.worker.res.get_nowait()
                if res["kind"] == "frame":
                    if res["gen"] == self._pending:
                        self._clear_pending()
                    if res["gen"] == self.gen:
                        photo = tk.PhotoImage(data=res["png"])
                        fb = res["bbox"]
                        fspp = (fb[2] - fb[0]) / max(1, photo.width())
                        self.last_frame = (photo, fb, fspp)
                        bbox = self.view_bbox()
                        if self.tiles_spanned(bbox) > MAX_LIVE_TILES:
                            continue  # user zoomed out to overview meanwhile
                        # place exactly; view may have panned meanwhile
                        if fspp == self.spp:
                            self.canvas.coords(
                                self.img_item,
                                (fb[0] - bbox[0]) / self.spp,
                                (bbox[3] - fb[3]) / self.spp)
                            self.canvas.itemconfig(self.img_item,
                                                   image=photo)
                        else:  # zoomed while rendering: show scaled
                            self._blit_scaled(photo, fb, bbox)
                        self._set_status(self.view_bbox(),
                                         f"live ({res['tiles']} tiles, "
                                         f"{res['ms']} ms)")
                elif res["kind"] == "snap":
                    if res["seq"] == self._snap_seq \
                            and self.mode == "ruler":
                        self._snap_res = res if res.get("found") else None
                        self._update_overlays()
                elif res["kind"] == "pick":
                    if res["seq"] == self._pick_seq:
                        self._on_pick_result(res)
                elif res["kind"] == "clip":
                    self._set_status(self.view_bbox(),
                                     f"clip saved: {res['path']} "
                                     f"({res['size_mb']:.2f} MB, "
                                     f"{res['ms']} ms)")
                elif res["kind"] == "error":
                    self._clear_pending()
                    self._set_status(self.view_bbox(),
                                     f"error: {res['msg']}")
        except queue.Empty:
            pass
        self.root.after(POLL_MS, self._poll)

    def _set_status(self, bbox, mode):
        w_um = (bbox[2] - bbox[0]) * self.dbu
        h_um = (bbox[3] - bbox[1]) * self.dbu
        self.status.config(text=f"view {w_um:.1f} x {h_um:.1f} um   |   "
                                f"{mode}")

    # ---- interaction ------------------------------------------------------
    def fit(self):
        bb = self.meta["bbox"]
        w, h = self.canvas_size()
        self.cx = (bb[0] + bb[2]) / 2
        self.cy = (bb[1] + bb[3]) / 2
        self.spp = max((bb[2] - bb[0]) / w, (bb[3] - bb[1]) / h) * 1.05
        self.redraw()

    def _configure(self, ev):
        if not self._did_fit and ev.width > 50:
            self._did_fit = True
            self.fit()
        else:
            self.redraw()

    def _pan_press(self, ev):
        self._drag = (ev.x, ev.y)
        self.canvas.config(cursor="fleur")

    def _pan_release(self, ev):
        self._drag = None
        self.canvas.config(cursor="crosshair")

    def _motion(self, ev):
        if self._drag is None:
            return
        dx, dy = ev.x - self._drag[0], ev.y - self._drag[1]
        self._drag = (ev.x, ev.y)
        self.cx -= dx * self.spp
        self.cy += dy * self.spp
        self.redraw()

    def _zoom_press(self, ev):
        if self.mode == "ruler":
            self._ruler_free = bool(ev.state & 0x1)  # Shift
            self._ruler_click(ev)
            return
        self._zoomdrag = (ev.x, ev.y)

    def _zoom_drag(self, ev):
        if self._zoomdrag is None:
            return
        x0, y0 = self._zoomdrag
        color = ACCENT if ev.x >= x0 else "#f5b62e"  # backward = zoom out
        if self._band is None:
            # black halo + bright inner line stays visible on any content
            self._band = (
                self.canvas.create_rectangle(x0, y0, ev.x, ev.y,
                                             outline="#000000", width=4),
                self.canvas.create_rectangle(x0, y0, ev.x, ev.y,
                                             outline=color, width=2))
        for item in self._band:
            self.canvas.coords(item, x0, y0, ev.x, ev.y)
        self.canvas.itemconfig(self._band[1], outline=color)

    def _zoom_release(self, ev):
        if self._zoomdrag is None:
            return
        x0, y0 = self._zoomdrag
        self._zoomdrag = None
        if self._band is not None:
            for item in self._band:
                self.canvas.delete(item)
            self._band = None
        dx, dy = abs(ev.x - x0), abs(ev.y - y0)
        if dx < 5 or dy < 5:
            self._pick_click(ev)  # a click, not a box: object picking
            return
        bbox = self.view_bbox()
        lx0 = bbox[0] + min(x0, ev.x) * self.spp
        lx1 = bbox[0] + max(x0, ev.x) * self.spp
        ly0 = bbox[3] - max(y0, ev.y) * self.spp
        ly1 = bbox[3] - min(y0, ev.y) * self.spp
        w, h = self.canvas_size()
        self.cx = (lx0 + lx1) / 2
        self.cy = (ly0 + ly1) / 2
        if ev.x >= x0:
            # forward drag: the selected box fills the canvas
            self.spp = max((lx1 - lx0) / w, (ly1 - ly0) / h)
        else:
            # backward drag: zoom out by the canvas/box ratio
            self.spp *= max(w / dx, h / dy)
        self.redraw()

    def _wheel(self, ev):
        delta = ev.delta
        if abs(delta) >= 120:  # windows-style units
            delta /= 120
        delta = max(-3.0, min(3.0, delta))
        if delta:
            self._zoom_at(ev.x, ev.y, 0.9 ** delta)

    def _touchpad(self, ev):
        # Tk 9 TouchpadScroll: %D packs signed 16-bit dx/dy into one int
        raw = ev.delta & 0xFFFFFFFF
        dy = raw & 0xFFFF
        if dy >= 0x8000:
            dy -= 0x10000
        if dy:
            factor = max(0.6, min(1.6, math.exp(-dy * 0.01)))
            self._zoom_at(ev.x, ev.y, factor)

    def _zoom_at(self, x, y, factor):
        bbox = self.view_bbox()
        px = bbox[0] + x * self.spp
        py = bbox[3] - y * self.spp
        self.spp *= factor
        nb = self.view_bbox()
        self.cx += px - (nb[0] + x * self.spp)
        self.cy += py - (nb[3] - y * self.spp)
        self.redraw()

    def _zoom_center(self, factor):
        self.spp *= factor
        self.redraw()

    def _update_cursor(self, ev):
        bbox = self.view_bbox()
        self._cursor = (bbox[0] + ev.x * self.spp,
                        bbox[3] - ev.y * self.spp)

    def _hover(self, ev):
        self._update_cursor(ev)
        x, y = self._cursor
        parts = [f"x {x * self.dbu:.3f}  y {y * self.dbu:.3f} um"]
        if self.mode == "ruler":
            self._ruler_free = bool(ev.state & 0x1)
            self._request_snap()
            if self._ruler_start is not None:
                x1, y1 = self._ruler_end_preview()
                x0, y0 = self._ruler_start
                d = math.hypot(x1 - x0, y1 - y0) * self.dbu
                parts.append(f"measure {d:.4f} um "
                             f"(dx {(x1 - x0) * self.dbu:.4f}, "
                             f"dy {(y1 - y0) * self.dbu:.4f})")
            else:
                parts.append("ruler: click 1st point"
                             + (" [snap]" if self.snap_on else ""))
            self._update_overlays()
        elif self._sel_text:
            parts.append(self._sel_text)
        else:
            w_um = (self.view_bbox()[2] - self.view_bbox()[0]) * self.dbu
            parts.append(f"view {w_um:.1f} um wide")
        self.status.config(text="   |   ".join(parts))

    # ---- ruler / snap / pick ----------------------------------------------
    def _toggle_ruler(self):
        if self.mode == "ruler":
            self.mode = "normal"
            self._ruler_start = None
            self._snap_res = None
            self._set_status(self.view_bbox(), "ruler off")
        else:
            self.mode = "ruler"
            self._set_status(self.view_bbox(),
                             "ruler: click two points (Shift=free angle, "
                             "m=snap %s, Esc=done)"
                             % ("on" if self.snap_on else "off"))
        self._update_overlays()

    def _toggle_snap(self):
        self.snap_on = not self.snap_on
        if not self.snap_on:
            self._snap_res = None
        self._set_status(self.view_bbox(),
                         f"vector snap {'on' if self.snap_on else 'off'}")
        self._update_overlays()

    def _esc(self):
        """Step out flateyes-style: pending point -> selection -> ruler
        mode -> finished rulers."""
        if self._ruler_start is not None:
            self._ruler_start = None
        elif self.selection is not None:
            self.selection = None
            self._sel_text = ""
        elif self.mode == "ruler":
            self.mode = "normal"
            self._snap_res = None
        elif self.rulers:
            self.rulers = []
        self._update_overlays()

    def _cursor_snapped(self):
        if self.snap_on and self._snap_res \
                and self._snap_res.get("found"):
            return self._snap_res["x"], self._snap_res["y"]
        return self._cursor

    def _ruler_end_preview(self):
        x0, y0 = self._ruler_start
        x1, y1 = self._cursor_snapped()
        if not self._ruler_free:  # snap to the dominant axis
            if abs(x1 - x0) >= abs(y1 - y0):
                y1 = y0
            else:
                x1 = x0
        return x1, y1

    def _ruler_click(self, ev):
        self._update_cursor(ev)
        if self._ruler_start is None:
            self._ruler_start = self._cursor_snapped()
        else:
            x0, y0 = self._ruler_start
            x1, y1 = self._ruler_end_preview()
            if (x0, y0) != (x1, y1):
                self.rulers.append((x0, y0, x1, y1))
            self._ruler_start = None
        self._update_overlays()

    def _request_snap(self):
        if not self.snap_on:
            return
        now = time.perf_counter()
        if now - self._snap_sent < 0.04:
            return
        x, y = self._cursor
        r = max(1, int(10 * self.spp))
        if self.tiles_spanned((x - r, y - r, x + r, y + r)) > 4:
            return  # too far out for meaningful geometry snapping
        self._snap_sent = now
        self._snap_seq += 1
        self.worker.submit({
            "kind": "snap", "seq": self._snap_seq,
            "x": int(x), "y": int(y), "r": r,
            "layers": self._visible_list() if len(self.visible) !=
            len(self._layer_vars) else None})

    def _pick_click(self, ev):
        self._update_cursor(ev)
        x, y = self._cursor
        tol = 8 * self.spp  # same-spot test in world coords: pan-proof
        if self._pick_px is not None and \
                abs(x - self._pick_px[0]) <= tol and \
                abs(y - self._pick_px[1]) <= tol:
            self._pick_nth += 1  # same spot: cycle overlapping objects
        else:
            self._pick_nth = 0
        self._pick_px = (x, y)
        r = max(1, int(3 * self.spp))
        if self.tiles_spanned((x - r, y - r, x + r, y + r)) > 4:
            self._set_status(self.view_bbox(), "zoom in to pick objects")
            return
        self._pick_seq += 1
        self.worker.submit({
            "kind": "pick", "seq": self._pick_seq,
            "x": int(x), "y": int(y), "r": r, "nth": self._pick_nth,
            "layers": self._visible_list() if len(self.visible) !=
            len(self._layer_vars) else None})

    def _on_pick_result(self, res):
        if not res.get("found"):
            self.selection = None
            self._sel_text = ""
            self._set_status(self.view_bbox(), "no object here")
        else:
            self.selection = res
            bb = res["bbox"]
            w = (bb[2] - bb[0]) * self.dbu
            h = (bb[3] - bb[1]) * self.dbu
            self._sel_text = (f"sel {res['lname']} "
                              f"{res['layer']}/{res['datatype']} · "
                              f"{res['cell']} · {w:.3f} x {h:.3f} um @ "
                              f"({bb[0] * self.dbu:.3f}, "
                              f"{bb[1] * self.dbu:.3f}) · "
                              f"{res['index'] + 1}/{res['count']}")
            self._set_status(self.view_bbox(), self._sel_text)
        self._update_overlays()

    def _update_overlays(self):
        c = self.canvas
        c.delete("ov")
        bbox = self.view_bbox()

        def sx(v):
            return (v - bbox[0]) / self.spp

        def sy(v):
            return (bbox[3] - v) / self.spp

        if self.selection and self.selection.get("points"):
            flat = []
            for x, y in self.selection["points"]:
                flat += [sx(x), sy(y)]
            if len(flat) >= 6:
                c.create_polygon(*flat, outline="#000000", fill="",
                                 width=4, tags="ov")
                c.create_polygon(*flat, outline="#ffffff", fill="",
                                 width=2, tags="ov")
        segs = list(self.rulers)
        if self.mode == "ruler" and self._ruler_start is not None:
            segs.append((*self._ruler_start, *self._ruler_end_preview()))
        for x0, y0, x1, y1 in segs:
            a, b, e, f = sx(x0), sy(y0), sx(x1), sy(y1)
            c.create_line(a, b, e, f, fill="#000000", width=4, tags="ov")
            c.create_line(a, b, e, f, fill="#ffe97a", width=2, tags="ov",
                          arrow="both", arrowshape=(10, 12, 4))
            d_um = math.hypot(x1 - x0, y1 - y0) * self.dbu
            label = c.create_text((a + e) / 2, (b + f) / 2 - 12,
                                  text=f"{d_um:.4f} um", fill="#ffe97a",
                                  tags="ov")
            tb = c.bbox(label)
            back = c.create_rectangle(tb[0] - 3, tb[1] - 1, tb[2] + 3,
                                      tb[3] + 1, fill="#101010",
                                      outline="", tags="ov")
            c.tag_raise(label, back)
        if self.mode == "ruler" and self.snap_on and self._snap_res \
                and self._snap_res.get("found"):
            mx, my = sx(self._snap_res["x"]), sy(self._snap_res["y"])
            color = "#66ffcc" if self._snap_res["snap"] == "vertex" \
                else "#66ccff"
            c.create_oval(mx - 5, my - 5, mx + 5, my + 5, outline=color,
                          width=2, tags="ov")
            c.create_line(mx - 9, my, mx + 9, my, fill=color, tags="ov")
            c.create_line(mx, my - 9, mx, my + 9, fill=color, tags="ov")

    def _on_layers_changed(self):
        self.visible = {k for k, v in self._layer_vars.items() if v.get()}
        self.redraw(immediate=True)

    def _all_layers(self):
        for v in self._layer_vars.values():
            v.set(True)
        self._on_layers_changed()

    def _no_layers(self):
        for v in self._layer_vars.values():
            v.set(False)
        self._on_layers_changed()

    def _clip_dialog(self):
        bbox = self.view_bbox()
        um = [round(v * self.dbu, 1) for v in bbox]
        out = filedialog.asksaveasfilename(
            defaultextension=".oas",
            initialfile=f"zn_clip_{um[0]}_{um[1]}_{um[2]}_{um[3]}um.oas",
            filetypes=[("OASIS", "*.oas"), ("all", "*")])
        if not out:
            return
        layers = (self._visible_list()
                  if len(self.visible) != len(self._layer_vars) else None)
        self.worker.submit({"kind": "clip",
                            "bbox": tuple(int(round(v)) for v in bbox),
                            "layers": layers, "out": out})
        self._set_status(bbox, "clipping…")

    def _quit(self):
        if getattr(self, "_quitting", False):
            return
        self._quitting = True
        if self.server_sock is not None:
            try:
                self.server_sock.close()
            except OSError:
                pass
        self.worker.stop()
        try:
            self.root.destroy()
        except tk.TclError:
            pass


def run_viewer(cache, server_sock=None):
    try:
        root = tk.Tk()
    except tk.TclError as exc:
        sys.stderr.write("zenoas: cannot open display %s "
                         "(X session not reachable: %s)\n"
                         % (os.environ.get("DISPLAY", ""), exc))
        raise SystemExit(3)
    viewer = Viewer(root, cache, server_sock)
    try:
        root.mainloop()
    except KeyboardInterrupt:
        pass
    finally:
        viewer._quit()
