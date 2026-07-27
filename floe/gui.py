"""Native viewer - Tkinter shell.

Ported from GTK3/PyGObject to Tkinter. The viewer now depends only on the
stdlib's tkinter plus Pillow, both pip-installable, which removes the
system-PyGObject / Python-version-match pain on closed-network hosts
(gi is a C extension tied to the OS Python; tkinter ships with the
interpreter). The render service (klayout) is unchanged and still runs in
a separate process returning PNG frames; here they are decoded with
Pillow and drawn - together with every overlay - on a Tk Canvas as native
items (no cairo, no pixbuf stamping).

Tk/PIL imports are lazy (import_tk), so importing this module works
headless and the spawned render process never touches the GUI toolkit.
"""

import io
import math
import os
import queue
import sys
import time

from . import cache as cache_mod
from .service import RenderWorker
from .viewport import MAX_LIVE_TILES

tk = ttk = filedialog = messagebox = Image = ImageTk = None
RS_BILINEAR = RS_NEAREST = None

APP = "floe"
POLL_MS = 25
DEBOUNCE_MS = 120

BLACK = "#000000"
BAND_IN = "#8ecdf5"        # forward drag: zoom in
BAND_OUT = "#f5b62e"       # backward drag: zoom out
RULER_CORE = "#ffe97a"
SNAP_VERTEX = "#66ffcc"
SNAP_EDGE = "#66ccff"
SEL_CORE = "#ffffff"
GOTO_MARK = "#ff66d9"
LABEL_BG = "#101010"

AUTO_DEPTH_BUDGET = 120_000   # est. shapes auto depth allows on screen
MIN_SPP = 0.01     # max zoom-in: 1 px = 0.01 dbu; keeps render bboxes
                   # from collapsing to zero width after int rounding

MINIMAP_PX = 110           # longest edge of the minimap (view px)
MINIMAP_MARGIN = 12
MINIMAP_DOT_MIN = 6        # view box smaller than this becomes a dot
MINIMAP_BG = "#141414"
MINIMAP_EDGE = "#666666"
MINIMAP_VIEW = "#8ecdf5"


def import_tk():
    """Lazy Tkinter/Pillow import: exit 3 with a clear message when either
    is missing or the display is unreachable (mirrors the old import_gtk
    contract cli.py relies on)."""
    global tk, ttk, filedialog, messagebox, Image, ImageTk
    global RS_BILINEAR, RS_NEAREST
    try:
        import tkinter as _tk
        from tkinter import ttk as _ttk, filedialog as _fd, messagebox as _mb
    except ImportError as exc:
        sys.stderr.write(
            "%s: tkinter is required to open a window (%s)\n"
            "  it ships with CPython built against Tcl/Tk; verify with:\n"
            "  python3 -c 'import tkinter; tkinter.Tk()'\n" % (APP, exc))
        sys.exit(3)
    try:
        from PIL import Image as _Image, ImageTk as _ImageTk
    except ImportError as exc:
        sys.stderr.write(
            "%s: Pillow is required to display render frames (%s)\n"
            "  install with: pip install pillow\n" % (APP, exc))
        sys.exit(3)
    tk, ttk, filedialog, messagebox = _tk, _ttk, _fd, _mb
    Image, ImageTk = _Image, _ImageTk
    rs = getattr(Image, "Resampling", Image)
    RS_BILINEAR, RS_NEAREST = rs.BILINEAR, rs.NEAREST


class Viewer:
    def __init__(self, cache, server_sock=None, show=True, goto=None):
        self.server_sock = server_sock
        if server_sock is not None:
            server_sock.setblocking(False)
        self.cx = self.cy = 0
        self.spp = 1.0              # dbu per screen pixel
        self._start_goto = goto     # [x_um, y_um(, window_um)] from the CLI
        self.visible = set()
        self.gen = 0
        self.last_frame = None      # (PIL.Image, bbox, dbu_per_px, key)
        self._frame_photo = None    # ImageTk of the full current frame
        self._preview_photo = None  # ImageTk of a cropped/scaled preview
        self._frame_anchor = None   # view center the frame was shown at
        self._job_keys = {}         # gen -> render key of submitted job
        self._job_depth = {}        # gen -> depth the job rendered at
        self._pending_scope = "live"
        self._drag = None
        self._zoomdrag = None       # rubber-band anchor (view px)
        self._band_cur = None
        self._debounce = None
        self._did_fit = False
        self.worker = None
        self._layer_vars = {}
        self.depth_value = 999
        self.depth_auto = True      # density-based depth until set explicitly
        self._depth_used = "?"      # depth of the last frame ("?" = none yet)
        self._quitting = False
        # ruler / snap / pick state
        self.mode = "normal"
        self.rulers = []
        self._ruler_start = None
        self._ruler_free = False
        self.snap_on = True
        self._snap_seq = 0
        self._snap_res = None
        self._snap_sent = 0.0
        self.selection = None
        self._sel_text = ""
        self._pick_seq = 0
        self._pick_px = None
        self._pick_nth = 0
        self._cursor = (0, 0)
        self._pending = None
        self._pending_t0 = 0.0
        self._pending_timer = None
        self._ddlg = None
        self._gdlg = None
        self.goto_mark = None       # world point of the last goto (X marker)
        self._alloc_size = None

        try:
            self.window = tk.Tk()
        except tk.TclError as exc:
            sys.stderr.write("%s: cannot open display (%s)\n" % (APP, exc))
            sys.exit(3)
        self.window.title(APP)
        self.window.geometry("1280x860")
        self.window.protocol("WM_DELETE_WINDOW", self._quit)
        self.window.bind("<Key>", self._on_key)
        self._smallfont = ("TkDefaultFont", 9)

        # left panel: title, source, scrollable layer list, buttons
        side = tk.Frame(self.window, width=210)
        side.pack(side="left", fill="y")
        side.pack_propagate(False)
        tk.Label(side, text=APP, font=("TkDefaultFont", 12, "bold"),
                 anchor="w").pack(fill="x", padx=10, pady=(8, 0))
        self._src_label = tk.Label(side, text="", anchor="w", justify="left")
        self._src_label.pack(fill="x", padx=10)

        lwrap = tk.Frame(side)
        lwrap.pack(side="top", fill="both", expand=True, pady=4)
        self._lcanvas = tk.Canvas(lwrap, highlightthickness=0, width=196)
        lscroll = ttk.Scrollbar(lwrap, orient="vertical",
                                command=self._lcanvas.yview)
        self._lcanvas.configure(yscrollcommand=lscroll.set)
        lscroll.pack(side="right", fill="y")
        self._lcanvas.pack(side="left", fill="both", expand=True)
        self._layers_frame = tk.Frame(self._lcanvas)
        self._lcanvas.create_window((0, 0), window=self._layers_frame,
                                    anchor="nw")
        self._layers_frame.bind(
            "<Configure>",
            lambda e: self._lcanvas.configure(
                scrollregion=self._lcanvas.bbox("all")))
        for w in (self._lcanvas, self._layers_frame):
            w.bind("<MouseWheel>", self._layer_scroll)
            w.bind("<Button-4>", lambda e: self._lcanvas.yview_scroll(-1, "units"))
            w.bind("<Button-5>", lambda e: self._lcanvas.yview_scroll(1, "units"))

        brow = tk.Frame(side)
        brow.pack(side="bottom", fill="x", pady=4)
        for text, cb in (("all", self._all_layers),
                         ("none", self._no_layers),
                         ("fit", self.fit),
                         ("clip…", self._clip_dialog)):
            tk.Button(brow, text=text, command=cb).pack(
                side="left", expand=True, fill="x")

        # main area: canvas (frame + overlays) and a status bar
        main = tk.Frame(self.window)
        main.pack(side="right", fill="both", expand=True)
        self.canvas = tk.Canvas(main, bg=BLACK, highlightthickness=0)
        self.canvas.pack(side="top", fill="both", expand=True)
        sbar = tk.Frame(main)
        sbar.pack(side="bottom", fill="x")
        self.status = tk.Label(sbar, text="", anchor="w")
        self.status.pack(side="left", fill="x", expand=True, padx=8)
        self.vstatus = tk.Label(sbar, text="", anchor="e")
        self.vstatus.pack(side="right", padx=(0, 14))
        self.dstatus = tk.Label(sbar, text="depth: auto", anchor="e")
        self.dstatus.pack(side="right", padx=(0, 14))
        self.rstatus = tk.Label(sbar, text="", anchor="e")
        self.rstatus.pack(side="right", padx=(0, 10))

        self.canvas.bind("<Configure>", self._on_configure)
        self.canvas.bind("<ButtonPress-1>", self._b1_press)
        self.canvas.bind("<B1-Motion>", self._b1_motion)
        self.canvas.bind("<ButtonRelease-1>", self._b1_release)
        for b in (2, 3):
            self.canvas.bind("<ButtonPress-%d>" % b, self._pan_press)
            self.canvas.bind("<B%d-Motion>" % b, self._pan_motion)
            self.canvas.bind("<ButtonRelease-%d>" % b, self._pan_release)
        self.canvas.bind("<Motion>", self._hover_motion)
        self.canvas.bind("<MouseWheel>", self._on_wheel)
        self.canvas.bind("<Button-4>", lambda e: self._on_wheel(e, "up"))
        self.canvas.bind("<Button-5>", lambda e: self._on_wheel(e, "down"))
        self.canvas.configure(cursor="crosshair")

        self.window.after(POLL_MS, self._poll)
        self._apply_cache(cache)
        if not show:
            self.window.withdraw()

    # ---- cache binding / instance requests --------------------------------
    def _apply_cache(self, cache):
        self.cache = cache
        self.meta = cache.meta
        self.dbu = self.meta["dbu"]
        bb = self.meta["bbox"]
        self.cx = (bb[0] + bb[2]) / 2
        self.cy = (bb[1] + bb[3]) / 2
        self.visible = {(l["layer"], l["datatype"])
                        for l in self.meta["layers"]}
        self.last_frame = None
        self._frame_photo = None
        self._frame_anchor = None
        self._depth_used = "?"
        self._job_keys.clear()
        self._clear_pending()
        self.rulers = []
        self._ruler_start = None
        self._snap_res = None
        self.selection = None
        self._sel_text = ""
        self._pick_px = None
        self.goto_mark = None
        src = self.meta["src"]
        self.window.title("%s - %s" % (APP, os.path.basename(src["path"])))
        self._src_label.config(
            text="%.2f GB · grid %dx%d" % (src["size"] / 1e9,
                                           self.meta["grid"]["nx"],
                                           self.meta["grid"]["ny"]))
        self._build_layer_panel()
        if self.worker is not None:
            self.worker.stop()
        self.worker = RenderWorker(cache)
        self.worker.start()
        if self._did_fit:
            self.fit()

    def _build_layer_panel(self):
        for child in self._layers_frame.winfo_children():
            child.destroy()
        self._layer_vars = {}
        for l in self.meta["layers"]:
            key = (l["layer"], l["datatype"])
            row = tk.Frame(self._layers_frame)
            row.pack(fill="x", anchor="w")
            var = tk.BooleanVar(value=True)
            cb = tk.Checkbutton(row, variable=var,
                                command=lambda k=key: self._on_layer_toggled(k))
            cb.pack(side="left")
            tk.Label(row, text="%s  %d/%d" % (l["name"], key[0], key[1]),
                     fg=l["color"], anchor="w").pack(side="left", fill="x")
            for w in (row, cb):
                w.bind("<MouseWheel>", self._layer_scroll)
                w.bind("<Button-4>",
                       lambda e: self._lcanvas.yview_scroll(-1, "units"))
                w.bind("<Button-5>",
                       lambda e: self._lcanvas.yview_scroll(1, "units"))
            self._layer_vars[key] = var

    def _layer_scroll(self, event):
        self._lcanvas.yview_scroll(-1 if event.delta > 0 else 1, "units")

    def open_file(self, path):
        """Open another OASIS file (instance-forwarded request)."""
        path = os.path.abspath(path)
        if path == self.cache.src and not self.cache.is_stale():
            return None
        c = cache_mod.Cache(path)
        if not c.exists():
            return "ERR no index for %s; run: %s index %s" % (path, APP,
                                                              path)
        c.load()
        self._apply_cache(c)
        return None

    def _poll_socket(self):
        """Non-blocking accept for the single-instance forward socket
        (replaces GLib.io_add_watch; polled from the render loop)."""
        if self.server_sock is None:
            return
        while True:
            try:
                conn, _ = self.server_sock.accept()
            except (BlockingIOError, OSError):
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
                fields = line.split("\t")
                path = fields[0].strip()
                if not path:
                    error = "ERR empty request"
                else:
                    try:
                        error = self.open_file(path)
                    except Exception as exc:
                        error = "ERR %s" % exc
                try:
                    conn.sendall((error or "OK").encode("utf-8") + b"\n")
                except OSError:
                    pass
                if not error:
                    self._forwarded_goto(fields[1:])
                    self._present()
            finally:
                conn.close()

    def _forwarded_goto(self, fields):
        """Apply a 'goto=X,Y[,W]' option (um) from a forwarded request.
        The sender already validated it, so parse leniently."""
        for f in fields:
            if not f.startswith("goto="):
                continue
            try:
                vals = [float(t) for t in
                        f[len("goto="):].replace(",", " ").split()]
            except ValueError:
                return
            if len(vals) >= 2:
                self.goto(vals[0], vals[1],
                          vals[2] if len(vals) > 2 else None)
            return

    def _present(self):
        self.window.deiconify()
        self.window.lift()
        try:
            self.window.focus_force()
        except tk.TclError:
            pass

    # ---- geometry ----------------------------------------------------------
    def _viewport_size(self):
        w = self.canvas.winfo_width()
        h = self.canvas.winfo_height()
        return (w if w >= 50 else 1200, h if h >= 50 else 800)

    def view_bbox(self):
        w, h = self._viewport_size()
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

    def _layers_arg(self):
        if len(self.visible) != len(self._layer_vars):
            return self._visible_list()
        return None

    # ---- display composition (Canvas: image item + overlay items) ---------
    def _render_key(self, scope):
        """Identity of a frame: what state it was rendered for.
        Auto-depth frames share one identity so they stay reusable
        while the chosen depth varies per view."""
        return (scope, tuple(sorted(self.visible)), self._depth_key())

    def _frame_compatible(self, frame):
        """A stale frame stays displayable rescaled until the fresh
        frame lands - across pans, zooms of any ratio, layer and depth
        changes alike (briefly stale content beats a black flash).
        Only an empty layer set blanks the screen (and a degenerate
        frame - zero-width bbox - is never displayable)."""
        return frame[3] is not None and frame[2] > 0 and bool(self.visible)

    def _display(self):
        self.canvas.delete("frame")
        self.canvas.delete("ov")
        bbox = self.view_bbox()
        if self.last_frame is not None and \
                self._frame_compatible(self.last_frame):
            # the stale frame is never rescaled blurily as a base: it stays
            # frozen at its own resolution until the fresh frame lands.
            # While the scale matches, the anchor tracks the center so pans
            # move 1:1; once a zoom changes the scale the anchor stays put.
            img, fb, fspp, _key = self.last_frame
            w, h = self._viewport_size()
            if self._frame_anchor is None or \
                    abs(self.spp / fspp - 1.0) < 0.001:
                self._frame_anchor = (self.cx, self.cy)
            ax, ay = self._frame_anchor
            vb = (ax - w / 2 * fspp, ay - h / 2 * fspp,
                  ax + w / 2 * fspp, ay + h / 2 * fspp)
            self._place_frame(img, fb, vb, fspp)
            obox, ospp = vb, fspp
        else:
            obox, ospp = bbox, self.spp
        self._draw_overlays(obox, ospp, bbox)

    def _place_frame(self, src, src_bbox, bbox, spp):
        """Put the (frozen or fresh) frame on the canvas: reposition the
        cached full photo for a pure pan, else crop the visible region and
        resize it for a zoom preview (never materializes a huge image)."""
        W, H = self._viewport_size()
        sw, sh = src.size
        if sw < 1 or src_bbox[2] <= src_bbox[0]:
            return
        sppd = sw / (src_bbox[2] - src_bbox[0])   # src px per dbu
        scale = (1.0 / spp) / sppd                # view px per src px
        if scale <= 0:
            return
        off_x = (src_bbox[0] - bbox[0]) / spp
        off_y = (bbox[3] - src_bbox[3]) / spp
        if abs(scale - 1.0) < 1e-3 and self._frame_photo is not None:
            self.canvas.create_image(int(round(off_x)), int(round(off_y)),
                                     anchor="nw", image=self._frame_photo,
                                     tags="frame")
            return
        vx0, vy0 = max(0.0, off_x), max(0.0, off_y)
        vx1 = min(W, off_x + sw * scale)
        vy1 = min(H, off_y + sh * scale)
        if vx1 <= vx0 or vy1 <= vy0:
            return
        sx0, sy0 = (vx0 - off_x) / scale, (vy0 - off_y) / scale
        sx1, sy1 = (vx1 - off_x) / scale, (vy1 - off_y) / scale
        crop = src.crop((int(math.floor(sx0)), int(math.floor(sy0)),
                         int(math.ceil(sx1)), int(math.ceil(sy1))))
        tw, th = max(1, int(round(vx1 - vx0))), max(1, int(round(vy1 - vy0)))
        if crop.size != (tw, th):
            crop = crop.resize((tw, th),
                               RS_BILINEAR if scale < 1 else RS_NEAREST)
        self._preview_photo = ImageTk.PhotoImage(crop)
        self.canvas.create_image(int(round(vx0)), int(round(vy0)),
                                 anchor="nw", image=self._preview_photo,
                                 tags="frame")

    def _draw_overlays(self, obox, ospp, bbox):
        c = self.canvas

        def sx(v):
            return (v - obox[0]) / ospp

        def sy(v):
            return (obox[3] - v) / ospp

        if self.selection and self.selection.get("points"):
            flat = [co for x, y in self.selection["points"]
                    for co in (sx(x), sy(y))]
            if len(flat) >= 6:
                c.create_polygon(*flat, outline=SEL_CORE, fill="",
                                 width=2, tags="ov")
        segs = list(self.rulers)
        if self.mode == "ruler" and self._ruler_start is not None:
            segs.append((*self._ruler_start, *self._ruler_end_preview()))
        for x0, y0, x1, y1 in segs:
            ax, ay, bx, by = sx(x0), sy(y0), sx(x1), sy(y1)
            c.create_line(ax, ay, bx, by, fill=RULER_CORE, width=2,
                          arrow="both", arrowshape=(12, 14, 4), tags="ov")
            d_um = math.hypot(x1 - x0, y1 - y0) * self.dbu
            self._label((ax + bx) / 2 + 8, (ay + by) / 2 - 16,
                        "%.4f um" % d_um)
        if self.mode == "ruler" and self.snap_on and self._snap_res \
                and self._snap_res.get("found"):
            mx, my = sx(self._snap_res["x"]), sy(self._snap_res["y"])
            color = SNAP_VERTEX if self._snap_res["snap"] == "vertex" \
                else SNAP_EDGE
            c.create_rectangle(mx - 5, my - 5, mx + 5, my + 5,
                               outline=color, tags="ov")
            c.create_line(mx - 9, my, mx + 9, my, fill=color, tags="ov")
            c.create_line(mx, my - 9, mx, my + 9, fill=color, tags="ov")
        if self.goto_mark is not None:
            gx, gy = sx(self.goto_mark[0]), sy(self.goto_mark[1])
            W, H = self._viewport_size()
            if -12 <= gx <= W + 12 and -12 <= gy <= H + 12:
                c.create_line(gx - 10, gy - 10, gx + 10, gy + 10,
                              fill=GOTO_MARK, width=2, tags="ov")
                c.create_line(gx - 10, gy + 10, gx + 10, gy - 10,
                              fill=GOTO_MARK, width=2, tags="ov")
        if self._zoomdrag is not None and self._band_cur is not None:
            x0, y0 = self._zoomdrag
            x1, y1 = self._band_cur
            color = BAND_IN if x1 >= x0 else BAND_OUT
            c.create_rectangle(x0, y0, x1, y1, outline=color, tags="ov")
        self._draw_minimap(bbox)

    def _label(self, x, y, text):
        c = self.canvas
        t = c.create_text(x + 4, y + 2, text=text, anchor="nw",
                          fill=RULER_CORE, font=self._smallfont, tags="ov")
        bb = c.bbox(t)
        if bb:
            r = c.create_rectangle(bb[0] - 2, bb[1] - 1, bb[2] + 2, bb[3] + 1,
                                   fill=LABEL_BG, outline="", tags="ov")
            c.tag_lower(r, t)

    def _draw_minimap(self, bbox):
        """Die outline in the bottom-right corner with the current view
        marked: a box while it is still readable, a dot once the zoom
        makes the box degenerate."""
        c = self.canvas
        W, H = self._viewport_size()
        bb = self.meta["bbox"]
        bw, bh = bb[2] - bb[0], bb[3] - bb[1]
        if bw <= 0 or bh <= 0:
            return
        scale = MINIMAP_PX / max(bw, bh)
        mw, mh = max(2, round(bw * scale)), max(2, round(bh * scale))
        x0 = W - MINIMAP_MARGIN - mw
        y0 = H - MINIMAP_MARGIN - mh
        if x0 < 0 or y0 < 0:
            return  # viewport too small for a minimap
        c.create_rectangle(x0, y0, x0 + mw, y0 + mh, fill=MINIMAP_BG,
                           outline=MINIMAP_EDGE, tags="ov")

        def mx(v):
            return x0 + (v - bb[0]) * scale

        def my(v):
            return y0 + (bb[3] - v) * scale

        vw = (bbox[2] - bbox[0]) * scale
        vh = (bbox[3] - bbox[1]) * scale
        if vw >= MINIMAP_DOT_MIN and vh >= MINIMAP_DOT_MIN:
            rx0 = max(x0, mx(bbox[0]))
            ry0 = max(y0, my(bbox[3]))
            rx1 = min(x0 + mw, mx(bbox[2]))
            ry1 = min(y0 + mh, my(bbox[1]))
            c.create_rectangle(rx0, ry0, rx1, ry1, outline=MINIMAP_VIEW,
                               tags="ov")
        else:
            px, py = mx(self.cx), my(self.cy)
            c.create_rectangle(px - 3, py - 3, px + 3, py + 3,
                               fill=MINIMAP_VIEW, outline=BLACK, tags="ov")

    # ---- drawing / rendering ------------------------------------------------
    @staticmethod
    def _expand(bbox, m):
        w = bbox[2] - bbox[0]
        h = bbox[3] - bbox[1]
        return (bbox[0] - m * w, bbox[1] - m * h,
                bbox[2] + m * w, bbox[3] + m * h)

    def _covered(self, bbox, scope):
        """True when the current frame still serves this view: same
        render state AND the viewport sits inside the frame with some
        comfort left, so no re-render is needed (Calibre-style margin
        panning)."""
        lf = self.last_frame
        if lf is None or lf[3] != self._render_key(scope):
            return False
        if abs(lf[2] - self.spp) > 1e-9 * self.spp:
            return False  # zoom changed: frame is scaled preview only
        fb = lf[1]
        vw, vh = bbox[2] - bbox[0], bbox[3] - bbox[1]
        pad_x = min(0.1 * vw, 0.25 * max(0.0, (fb[2] - fb[0]) - vw))
        pad_y = min(0.1 * vh, 0.25 * max(0.0, (fb[3] - fb[1]) - vh))
        return (fb[0] <= bbox[0] - pad_x and fb[1] <= bbox[1] - pad_y and
                fb[2] >= bbox[2] + pad_x and fb[3] >= bbox[3] + pad_y)

    def redraw(self, immediate=False):
        self._clamp_view()
        bbox = self.view_bbox()
        span = self.tiles_spanned(bbox)
        live = 0 < span <= MAX_LIVE_TILES and bool(self.visible)
        scope = "live" if live else "skel"
        self._display()
        if not self.visible:
            self._after_cancel("_debounce")
            self._clear_pending()
            self._set_status(bbox, "no layers visible")
            return
        mode = "live (%d tiles)" % span if scope == "live" \
            else "far view (skeleton)"
        if self._covered(bbox, scope):
            self._after_cancel("_debounce")
            self._set_status(bbox, mode)
            return
        self._after_cancel("_debounce")
        self._pending_scope = scope
        self._debounce = self.window.after(
            1 if immediate else DEBOUNCE_MS, self._submit_render)
        self._set_status(bbox, mode)

    def _submit_render(self):
        self._debounce = None
        scope = self._pending_scope
        bbox = self.view_bbox()
        w, h = self._viewport_size()
        # overdraw margin: pans inside it are served from the frame with
        # no re-render at all; renders re-center in the background
        m = 0.5
        while m > 0 and (w * (1 + 2 * m) > 4096 or
                         h * (1 + 2 * m) > 4096):
            m /= 2
        if scope == "live":
            while m > 0 and self.tiles_spanned(
                    self._expand(bbox, m)) > MAX_LIVE_TILES:
                m = 0 if m <= 0.13 else m / 2
        eb = self._expand(bbox, m)
        if scope == "live":
            depth = self._auto_depth(eb) if self.depth_auto \
                else self._depth()
        elif self.depth_auto:
            depth = 0     # far view default: block-outline (depth 0)
        else:
            depth = self._depth()  # far view honors explicit depth 0/1/2
        self.gen += 1
        self._job_keys[self.gen] = self._render_key(scope)
        self._job_depth[self.gen] = depth
        for g in [g for g in self._job_keys if g < self.gen - 8]:
            del self._job_keys[g]
            self._job_depth.pop(g, None)
        self.worker.submit({
            "kind": "render", "gen": self.gen, "scope": scope,
            "bbox": tuple(int(round(v)) for v in eb),
            "view": tuple(int(round(v)) for v in bbox),
            "w": int(round(w * (1 + 2 * m))),
            "h": int(round(h * (1 + 2 * m))),
            "depth": depth,
            "visible": self._layers_arg()})
        self._pending = self.gen
        self._pending_t0 = time.perf_counter()
        self.rstatus.config(text="rendering…")
        self._set_cursor("wait")  # mouse input is ignored until the frame
        if self._pending_timer is None:
            self._pending_timer = self.window.after(400, self._pending_tick)

    def _pending_tick(self):
        if self._pending is None:
            self._pending_timer = None
            self.rstatus.config(text="")
            return
        el = time.perf_counter() - self._pending_t0
        self.rstatus.config(text="rendering…" if el < 1.5
                            else "rendering… %.0fs" % el)
        self._pending_timer = self.window.after(400, self._pending_tick)

    def _clear_pending(self):
        self._pending = None
        self._after_cancel("_pending_timer")
        self.rstatus.config(text="")
        self._set_cursor("move" if self._drag is not None else "crosshair")

    def _after_cancel(self, attr):
        tid = getattr(self, attr, None)
        if tid is not None:
            try:
                self.window.after_cancel(tid)
            except Exception:
                pass
            setattr(self, attr, None)

    def _poll(self):
        if self._quitting:
            return
        self._poll_socket()
        try:
            while True:
                res = self.worker.res.get_nowait()
                kind = res.get("kind")
                if kind == "frame":
                    if res["gen"] == self._pending:
                        self._clear_pending()
                        self.rstatus.config(text="rendering done.")
                    if res["gen"] == self.gen:
                        img = Image.open(io.BytesIO(res["png"]))
                        img.load()
                        if img.mode not in ("RGB", "RGBA"):
                            img = img.convert("RGB")
                        fb = res["bbox"]
                        fspp = (fb[2] - fb[0]) / max(1, img.width)
                        key = self._job_keys.get(res["gen"])
                        used = self._job_depth.get(res["gen"])
                        self.last_frame = (img, fb, fspp, key)
                        self._frame_photo = ImageTk.PhotoImage(img)
                        self._display()
                        if res.get("bg"):
                            continue  # silent margin upgrade
                        self._depth_used = used
                        self.dstatus.config(text=self._depth_label())
                        if res.get("scope") == "skel":
                            mode = "far view (%s, %d ms)" % (
                                "outline" if used == 0 else "skeleton",
                                res["ms"])
                        else:
                            mode = "live (%d tiles, %d ms%s)" \
                                % (res["tiles"], res["ms"],
                                   self._depth_note(used))
                        self._set_status(self.view_bbox(), mode)
                elif kind == "snap":
                    if res["seq"] == self._snap_seq \
                            and self.mode == "ruler":
                        self._snap_res = res if res.get("found") else None
                        self._display()
                elif kind == "pick":
                    if res["seq"] == self._pick_seq:
                        self._on_pick_result(res)
                elif kind == "clip":
                    self._set_status(self.view_bbox(),
                                     "clip saved: %s (%.2f MB, %d ms)"
                                     % (res["path"], res["size_mb"],
                                        res["ms"]))
                elif kind == "error":
                    self._clear_pending()
                    self._set_status(self.view_bbox(),
                                     "error: %s" % res.get("msg"))
        except queue.Empty:
            pass
        self.window.after(POLL_MS, self._poll)

    def _set_status(self, bbox, mode):
        w_um = (bbox[2] - bbox[0]) * self.dbu
        h_um = (bbox[3] - bbox[1]) * self.dbu
        self.vstatus.config(text="view %.1f x %.1f um" % (w_um, h_um))
        self.status.config(text=mode)

    # ---- interaction --------------------------------------------------------
    def fit(self):
        bb = self.meta["bbox"]
        self.cx = (bb[0] + bb[2]) / 2
        self.cy = (bb[1] + bb[3]) / 2
        self.spp = self._fit_spp()
        self.redraw()

    def _fit_spp(self):
        bb = self.meta["bbox"]
        w, h = self._viewport_size()
        return max((bb[2] - bb[0]) / w, (bb[3] - bb[1]) / h) * 1.05

    def _clamp_view(self):
        """Zooms and pans never leave the die: spp is capped at the
        fit-view scale and the viewport stays inside the die bbox
        (centered on an axis the viewport is wider than)."""
        bb = self.meta["bbox"]
        self.spp = min(max(self.spp, MIN_SPP), self._fit_spp())
        w, h = self._viewport_size()
        hx, hy = w / 2 * self.spp, h / 2 * self.spp
        if 2 * hx >= bb[2] - bb[0]:
            self.cx = (bb[0] + bb[2]) / 2
        else:
            self.cx = min(max(self.cx, bb[0] + hx), bb[2] - hx)
        if 2 * hy >= bb[3] - bb[1]:
            self.cy = (bb[1] + bb[3]) / 2
        else:
            self.cy = min(max(self.cy, bb[1] + hy), bb[3] - hy)

    def _on_configure(self, event):
        # Canvas resized: first real size fits (or applies a startup goto);
        # later size changes just redraw.
        size = (event.width, event.height)
        if size == self._alloc_size:
            return
        self._alloc_size = size
        if not self._did_fit and event.width > 50:
            self._did_fit = True
            if self._start_goto is not None:
                self.spp = self._fit_spp()  # zoom baseline if no window given
                self.goto(*self._start_goto)
                self._start_goto = None
            else:
                self.fit()
        else:
            self.redraw()

    def _set_cursor(self, name):
        try:
            self.canvas.configure(
                cursor={"wait": "watch", "move": "fleur"}.get(
                    name, "crosshair"))
        except tk.TclError:
            pass

    def _b1_press(self, event):
        self.canvas.focus_set()
        if self._pending is not None:
            return
        if self.mode == "ruler":
            self._ruler_free = bool(event.state & 0x0001)  # Shift
            self._ruler_click(event)
            return
        self._zoomdrag = (event.x, event.y)
        self._band_cur = None

    def _b1_motion(self, event):
        self._update_cursor(event)
        if self._pending is not None:
            return
        if self._zoomdrag is not None:
            self._band_cur = (event.x, event.y)
            self._display()

    def _b1_release(self, event):
        if self._zoomdrag is None:
            return
        x0, y0 = self._zoomdrag
        self._zoomdrag = None
        self._band_cur = None
        dx, dy = abs(event.x - x0), abs(event.y - y0)
        if dx < 5 or dy < 5:
            self._display()          # erase the band
            self._pick_click(event)  # a click, not a box
            return
        bbox = self.view_bbox()
        lx0 = bbox[0] + min(x0, event.x) * self.spp
        lx1 = bbox[0] + max(x0, event.x) * self.spp
        ly0 = bbox[3] - max(y0, event.y) * self.spp
        ly1 = bbox[3] - min(y0, event.y) * self.spp
        w, h = self._viewport_size()
        self.cx = (lx0 + lx1) / 2
        self.cy = (ly0 + ly1) / 2
        if event.x >= x0:  # forward: the box fills the viewport
            self.spp = max((lx1 - lx0) / w, (ly1 - ly0) / h)
        else:              # backward: zoom out by the viewport/box ratio
            self.spp *= max(w / dx, h / dy)
        self.redraw()

    def _pan_press(self, event):
        if self._pending is not None:
            return
        self._drag = (event.x, event.y)
        self._set_cursor("move")

    def _pan_motion(self, event):
        self._update_cursor(event)
        if self._pending is not None or self._drag is None:
            return
        ddx, ddy = event.x - self._drag[0], event.y - self._drag[1]
        self._drag = (event.x, event.y)
        self.cx -= ddx * self.spp
        self.cy += ddy * self.spp
        self.redraw()

    def _pan_release(self, event):
        self._drag = None
        self._set_cursor("crosshair")

    def _hover_motion(self, event):
        self._update_cursor(event)
        if self._pending is not None:
            return
        self._hover(event)

    def _on_wheel(self, event, direction=None):
        if self._pending is not None:
            return
        # a wheel event while a button is held (some X setups synthesize
        # them during a drag) must never zoom mid-pan/mid-band
        if self._drag is not None or self._zoomdrag is not None:
            return
        if direction is not None:
            delta = 1.0 if direction == "up" else -1.0
        else:
            delta = 1.0 if event.delta > 0 else -1.0
        self._zoom_at(event.x, event.y, 0.9 ** delta)

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

    # ---- keys ----------------------------------------------------------------
    def _on_key(self, event):
        if self._gdlg is not None or self._ddlg is not None:
            return  # a modal tool dialog owns the keyboard
        focus = self.window.focus_get()
        if isinstance(focus, (tk.Entry, tk.Spinbox)):
            return
        name = event.keysym
        if name == "f":
            self.fit()
        elif name in ("plus", "equal", "KP_Add"):
            self._zoom_center(1 / 1.25)
        elif name in ("minus", "KP_Subtract"):
            self._zoom_center(1.25)
        elif name == "r":
            self._toggle_ruler()
        elif name == "m":
            self._toggle_snap()
        elif name == "Escape":
            self._esc()
        elif name == "d":
            self._depth_dialog()
        elif name == "g":
            self._goto_dialog()
        elif name == "a":
            self._set_depth_auto()
        elif name == "q":
            self._confirm_quit()
        elif len(name) == 1 and name.isdigit():
            self._set_depth(int(name))
        elif name.startswith("KP_") and name[3:].isdigit():
            self._set_depth(int(name[3:]))

    # ---- depth -----------------------------------------------------------------
    def _depth(self):
        d = self.depth_value
        return None if d >= 999 else max(0, d)

    def _depth_key(self):
        """Render-key component: one identity for the whole auto mode."""
        return "auto" if self.depth_auto else self._depth()

    def _depth_note(self, used):
        """Status-line suffix naming the depth a frame rendered at."""
        if self.depth_auto:
            return ", depth auto(%s)" % ("full" if used is None else used)
        return "" if used is None else ", depth %d" % used

    def _depth_label(self):
        if not self.depth_auto:
            d = self._depth()
            return "depth: full" if d is None else "depth: %d" % d
        if self._depth_used == "?":
            return "depth: auto"
        return "depth: auto(%s)" % ("full" if self._depth_used is None
                                    else self._depth_used)

    def _auto_depth(self, bbox):
        """Deepest depth whose estimated draw cost stays under
        AUTO_DEPTH_BUDGET, from the index-time density table (per-tile
        counts scaled by view overlap). Cost of depth d = shapes down
        to d + one outline frame per cell at level d+1, so a mid depth
        that would stroke millions of array-cell frames is skipped.
        None = no table or the full hierarchy fits."""
        dens = self.meta.get("density")
        if not dens or not self.visible:
            return None
        tiles = dens.get("tiles", {})
        g = self.meta["grid"]
        vis = ["%d/%d" % key for key in self.visible]
        levels = max(1, dens.get("levels", 1))
        est = [0.0] * levels
        frames = [0.0] * (levels + 1)
        area = float(g["tile_w"]) * g["tile_h"]
        for r, c in self.cache.tiles_for_bbox(*[int(v) for v in bbox]):
            t = tiles.get("%d,%d" % (r, c))
            if not t:
                continue
            tx0 = g["x0"] + c * g["tile_w"]
            ty0 = g["y0"] + r * g["tile_h"]
            frac = (max(0.0, min(bbox[2], tx0 + g["tile_w"])
                        - max(bbox[0], tx0))
                    * max(0.0, min(bbox[3], ty0 + g["tile_h"])
                          - max(bbox[1], ty0)) / area)
            if frac <= 0:
                continue
            for lv, n in enumerate(t.get("cells", ())):
                if lv <= levels:
                    frames[lv] += n * frac
            for lkey in vis:
                arr = t.get(lkey)
                if not arr:
                    continue
                for lv in range(levels):
                    est[lv] += arr[min(lv, len(arr) - 1)] * frac
        total = est[-1]
        if total <= AUTO_DEPTH_BUDGET:
            return None
        best = None
        for lv in range(levels):
            if est[lv] + frames[lv + 1] <= AUTO_DEPTH_BUDGET:
                best = lv
        if best is not None:
            return best
        # nothing fits the budget: least-bad choice (a full render at
        # least draws no frames)
        lv = min(range(levels), key=lambda i: est[i] + frames[i + 1])
        return None if total <= est[lv] + frames[lv + 1] else lv

    def _set_depth(self, n):
        self.depth_auto = False
        self.depth_value = max(0, min(999, int(n)))
        if self._ddlg is not None:
            var = getattr(self._ddlg, "_spinvar", None)
            if var is not None and var.get() != str(self.depth_value):
                var.set(str(self.depth_value))
        self._on_depth()

    def _set_depth_auto(self):
        self.depth_auto = True
        self._on_depth()

    def _on_depth(self):
        self.dstatus.config(text=self._depth_label())
        self.redraw(immediate=True)

    # ---- dialogs ---------------------------------------------------------------
    def _center_dialog(self, dlg):
        """Center the dialog on the main window (parent geometry + the
        dialog's own requested size)."""
        dlg.update_idletasks()
        pw, ph = self.window.winfo_width(), self.window.winfo_height()
        px, py = self.window.winfo_rootx(), self.window.winfo_rooty()
        dw, dh = dlg.winfo_reqwidth(), dlg.winfo_reqheight()
        dlg.geometry("+%d+%d" % (px + max(0, (pw - dw) // 2),
                                 py + max(0, (ph - dh) // 2)))

    def _depth_dialog(self):
        if self._ddlg is not None:
            self._ddlg.lift()
            return
        dlg = tk.Toplevel(self.window)
        dlg.title("hierarchy depth")
        dlg.transient(self.window)
        dlg.resizable(False, False)
        self._ddlg = dlg
        tk.Label(dlg, text="hierarchy depth (0 = top only, 999 = full)"
                 ).pack(anchor="w", padx=14, pady=(10, 2))
        row = tk.Frame(dlg)
        row.pack(anchor="w", padx=14)
        var = tk.StringVar(value=str(self.depth_value))
        dlg._spinvar = var
        spin = tk.Spinbox(row, from_=0, to=999, width=5, textvariable=var,
                          command=lambda: self._spin_depth(var))
        spin.pack(side="left")
        spin.bind("<Return>", lambda e: self._close_depth())
        for preset in (0, 1, 2, 3, 999):
            tk.Button(row, text="full" if preset == 999 else str(preset),
                      width=3,
                      command=lambda p=preset: self._set_depth(p)).pack(
                          side="left", padx=1)
        tk.Button(row, text="auto",
                  command=self._set_depth_auto).pack(side="left", padx=1)
        tk.Label(dlg, justify="left", font=self._smallfont,
                 text="auto picks the deepest level whose estimated shape\n"
                      "count stays interactive (index density table); "
                      "explicit\nvalues override it. cells beyond the limit "
                      "are drawn as\noutline frames with names - keys: "
                      "d = this dialog,\n0-9 = depth, a = auto").pack(
                          anchor="w", padx=14, pady=4)
        tk.Button(dlg, text="ok", command=self._close_depth).pack(pady=(0, 8))
        dlg.protocol("WM_DELETE_WINDOW", self._close_depth)
        dlg.bind("<Escape>", lambda e: self._close_depth())
        self._center_dialog(dlg)
        dlg.grab_set()
        spin.focus_set()

    def _spin_depth(self, var):
        try:
            self._set_depth(int(var.get()))
        except ValueError:
            pass

    def _close_depth(self):
        dlg = self._ddlg
        if dlg is None:
            return
        try:
            self._spin_depth(dlg._spinvar)  # commit a typed value
        except Exception:
            pass
        self._ddlg = None
        dlg.grab_release()
        dlg.destroy()
        self._present()

    # ---- goto (Calibre-style jump to coordinates) ---------------------------
    GOTO_HINT = ("um coordinates. window = view width after the jump\n"
                 "(blank = keep zoom). a pasted \"x, y\" pair in one\n"
                 "field works too. Esc clears the X marker.")

    def _goto_dialog(self):
        if self._gdlg is not None:
            self._gdlg.lift()
            return
        dlg = tk.Toplevel(self.window)
        dlg.title("goto position")
        dlg.transient(self.window)
        dlg.resizable(False, False)
        self._gdlg = dlg
        row = tk.Frame(dlg)
        row.pack(anchor="w", padx=14, pady=(10, 2))
        entries = []
        for label, width, text in (
                ("x", 12, "%.3f" % (self.cx * self.dbu)),
                ("y", 12, "%.3f" % (self.cy * self.dbu)),
                ("window", 9, "")):
            tk.Label(row, text=label).pack(side="left")
            e = tk.Entry(row, width=width)
            e.insert(0, text)
            e.bind("<Return>", lambda ev: self._goto_apply())
            e.pack(side="left", padx=(0, 4))
            entries.append(e)
        dlg._entries = entries
        note = tk.Label(dlg, justify="left", font=self._smallfont,
                        text=self.GOTO_HINT)
        note.pack(anchor="w", padx=14)
        dlg._note = note
        brow = tk.Frame(dlg)
        brow.pack(fill="x", padx=14, pady=8)
        tk.Button(brow, text="ok", command=self._goto_apply).pack(
            side="left", expand=True, fill="x")
        tk.Button(brow, text="close", command=self._close_goto).pack(
            side="left", expand=True, fill="x")
        dlg.protocol("WM_DELETE_WINDOW", self._close_goto)
        dlg.bind("<Escape>", lambda e: self._close_goto())
        self._center_dialog(dlg)
        dlg.grab_set()
        entries[0].focus_set()
        entries[0].select_range(0, "end")

    def _close_goto(self):
        dlg = self._gdlg
        if dlg is None:
            return
        self._gdlg = None
        dlg.grab_release()
        dlg.destroy()
        self._present()

    def _goto_apply(self):
        """Jump to the entered position: values fill x, y, window in
        order, so a DRC-report "x, y" pair pasted into any one field
        spreads across both coordinates."""
        dlg = self._gdlg
        if dlg is None:
            return
        try:
            part = [[float(t) for t in
                     e.get().replace(",", " ").split()]
                    for e in dlg._entries]
        except ValueError:
            dlg._note.config(text="not a number\n" + self.GOTO_HINT)
            return
        if len(part[0]) >= 2:
            part[1] = []  # pair pasted into x: the y field is stale
        vals = part[0] + part[1] + part[2]
        if len(vals) < 2:
            dlg._note.config(text="need both x and y\n" + self.GOTO_HINT)
            return
        self.goto(vals[0], vals[1], vals[2] if len(vals) > 2 else None)
        self._close_goto()  # ok / Enter applied successfully: close

    def goto(self, x_um, y_um, window_um=None):
        """Center the view on (x, y) um with an X marker; window is the
        resulting view width in um (None/0 = keep the current zoom)."""
        self.goto_mark = (x_um / self.dbu, y_um / self.dbu)
        self.cx, self.cy = self.goto_mark
        if window_um and window_um > 0:
            w, _h = self._viewport_size()
            self.spp = (window_um / self.dbu) / w
        self.redraw(immediate=True)

    # ---- ruler / snap / pick -----------------------------------------------
    def _update_cursor(self, event):
        bbox = self.view_bbox()
        self._cursor = (bbox[0] + event.x * self.spp,
                        bbox[3] - event.y * self.spp)

    def _hover(self, event):
        x, y = self._cursor
        parts = ["x %.3f  y %.3f um" % (x * self.dbu, y * self.dbu)]
        if self.mode == "ruler":
            self._ruler_free = bool(event.state & 0x0001)  # Shift
            self._request_snap()
            if self._ruler_start is not None:
                x1, y1 = self._ruler_end_preview()
                x0, y0 = self._ruler_start
                d = math.hypot(x1 - x0, y1 - y0) * self.dbu
                parts.append("measure %.4f um (dx %.4f, dy %.4f)"
                             % (d, (x1 - x0) * self.dbu,
                                (y1 - y0) * self.dbu))
            else:
                parts.append("ruler: click 1st point"
                             + (" [snap]" if self.snap_on else ""))
            self._display()
        elif self._sel_text:
            parts.append(self._sel_text)
        self.status.config(text="   |   ".join(parts))

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
        self._display()

    def _toggle_snap(self):
        self.snap_on = not self.snap_on
        if not self.snap_on:
            self._snap_res = None
        self._set_status(self.view_bbox(),
                         "vector snap %s"
                         % ("on" if self.snap_on else "off"))
        self._display()

    def _esc(self):
        """Step out flateyes-style: pending point -> selection -> ruler
        mode -> finished rulers -> goto marker."""
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
        elif self.goto_mark is not None:
            self.goto_mark = None
        self._display()

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

    def _ruler_click(self, event):
        self._update_cursor(event)
        if self._ruler_start is None:
            self.rulers = []  # single ruler: starting a new one clears it
            self._ruler_start = self._cursor_snapped()
        else:
            x0, y0 = self._ruler_start
            x1, y1 = self._ruler_end_preview()
            if (x0, y0) != (x1, y1):
                self.rulers.append((x0, y0, x1, y1))
            self._ruler_start = None
        self._display()

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
            "layers": self._layers_arg()})

    def _pick_click(self, event):
        self._update_cursor(event)
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
            "layers": self._layers_arg()})

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
            self._sel_text = ("sel %s %d/%d · %s · %.3f x %.3f um @ "
                              "(%.3f, %.3f) · %d/%d"
                              % (res["lname"], res["layer"],
                                 res["datatype"], res["cell"], w, h,
                                 bb[0] * self.dbu, bb[1] * self.dbu,
                                 res["index"] + 1, res["count"]))
            self._set_status(self.view_bbox(), self._sel_text)
        self._display()

    # ---- layers / clip -------------------------------------------------------
    def _on_layer_toggled(self, key):
        if self._layer_vars[key].get():
            self.visible.add(key)
        else:
            self.visible.discard(key)
        self.redraw(immediate=True)

    def _all_layers(self):
        for key, var in self._layer_vars.items():
            var.set(True)
            self.visible.add(key)
        self.redraw(immediate=True)

    def _no_layers(self):
        for var in self._layer_vars.values():
            var.set(False)
        self.visible.clear()
        self.redraw(immediate=True)

    def _clip_dialog(self):
        bbox = self.view_bbox()
        um = [round(v * self.dbu, 1) for v in bbox]
        out = filedialog.asksaveasfilename(
            parent=self.window, title="save clip as OASIS",
            defaultextension=".oas",
            filetypes=[("OASIS", "*.oas"), ("all files", "*")],
            initialfile="floe_clip_%s_%s_%s_%sum.oas"
                        % (um[0], um[1], um[2], um[3]))
        self._present()
        if not out:
            return
        self.worker.submit({"kind": "clip",
                            "bbox": tuple(int(round(v)) for v in bbox),
                            "layers": self._layers_arg(), "out": out})
        self._set_status(bbox, "clipping…")

    # ---- shutdown -------------------------------------------------------------
    def _confirm_quit(self):
        """Ask before quitting (q key)."""
        if self._quitting:
            return
        if messagebox.askyesno(APP, "Quit floe?", default="no",
                               parent=self.window):
            self._quit()
        else:
            self._present()

    def _quit(self):
        if self._quitting:
            return
        self._quitting = True
        if self.server_sock is not None:
            try:
                self.server_sock.close()
            except OSError:
                pass
        if self.worker is not None:
            self.worker.stop()
        try:
            self.window.quit()
            self.window.destroy()
        except Exception:
            pass


def run_viewer(cache, server_sock=None, goto=None):
    import_tk()
    viewer = Viewer(cache, server_sock, goto=goto)
    try:
        import signal as _signal
        _signal.signal(_signal.SIGINT, lambda *_: viewer._quit())
    except Exception:
        pass
    try:
        viewer.window.mainloop()
    except KeyboardInterrupt:
        viewer._quit()
