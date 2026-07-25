"""Native viewer - GTK3/PyGObject shell.

Same hard environment constraints as flateyes (the sibling image viewer):
targets are closed-network RHEL-family hosts where only PyGObject/GTK3 is
stock and NOTHING can be installed. In particular there is NO pycairo, so
the GTK "draw" signal is unusable: every frame is composed into a
GdkPixbuf (klayout render frames, overview crops, rubber band, rulers,
snap marker, selection outline) and shown by one Gtk.Image; text labels
are Gtk.Label widgets positioned with margins inside a Gtk.Overlay.

GTK imports are lazy (import_gtk), so importing this module works
headless and the spawned render process never touches GTK.
"""

import math
import os
import queue
import sys
import time

from . import cache as cache_mod
from .service import RenderWorker
from .viewport import MAX_LIVE_TILES

Gtk = Gdk = GdkPixbuf = GLib = None

APP = "oastray"
POLL_MS = 25
DEBOUNCE_MS = 120

BLACK = 0x000000FF
BAND_IN = 0x8ECDF5FF       # forward drag: zoom in
BAND_OUT = 0xF5B62EFF      # backward drag: zoom out
RULER_CORE = 0xFFE97AFF
SNAP_VERTEX = 0x66FFCCFF
SNAP_EDGE = 0x66CCFFFF
SEL_CORE = 0xFFFFFFFF


def import_gtk():
    """flateyes-style lazy GTK import: exit 3 with a clear message when
    PyGObject is missing or the display is unreachable."""
    global Gtk, Gdk, GdkPixbuf, GLib
    try:
        import gi
        import warnings
        warnings.simplefilter("ignore", getattr(
            gi, "PyGIDeprecationWarning", DeprecationWarning))
        gi.require_version("Gtk", "3.0")
        gi.require_version("GdkPixbuf", "2.0")
        from gi.repository import Gtk as _Gtk, Gdk as _Gdk, \
            GdkPixbuf as _GdkPixbuf, GLib as _GLib
    except (ImportError, ValueError) as exc:
        sys.stderr.write(
            "%s: PyGObject/GTK3 is required to open a window (%s)\n"
            "  verify with: python3 -c 'import gi; "
            "gi.require_version(\"Gtk\", \"3.0\")'\n" % (APP, exc))
        sys.exit(3)
    Gtk, Gdk, GdkPixbuf, GLib = _Gtk, _Gdk, _GdkPixbuf, _GLib
    ok = Gtk.init_check(sys.argv)
    if isinstance(ok, tuple):
        ok = ok[0]
    if not ok:
        sys.stderr.write(
            "%s: cannot open display %s (X session not reachable)\n"
            % (APP, os.environ.get("DISPLAY", "")))
        sys.exit(3)


def fill_rect(buf, x, y, w, h, rgba):
    """Clipped rectangle fill via a shared-pixels subpixbuf (flateyes
    pattern - the only way to draw without cairo)."""
    x, y, w, h = int(round(x)), int(round(y)), int(round(w)), int(round(h))
    if x < 0:
        w += x
        x = 0
    if y < 0:
        h += y
        y = 0
    w = min(w, buf.get_width() - x)
    h = min(h, buf.get_height() - y)
    if w > 0 and h > 0:
        buf.new_subpixbuf(x, y, w, h).fill(rgba)


def stamp_segment(buf, a, b, casing, core):
    """Line segment: flat rects for H/V, 3x3/2x2 dabs for free angles."""
    ax, ay = a
    bx, by = b
    if round(ay) == round(by):    # horizontal
        if casing is not None:
            fill_rect(buf, min(ax, bx), ay - 2, abs(bx - ax) + 1, 5, casing)
        fill_rect(buf, min(ax, bx), ay - 1, abs(bx - ax) + 1, 2, core)
    elif round(ax) == round(bx):  # vertical
        if casing is not None:
            fill_rect(buf, ax - 2, min(ay, by), 5, abs(by - ay) + 1, casing)
        fill_rect(buf, ax - 1, min(ay, by), 2, abs(by - ay) + 1, core)
    else:
        steps = min(int(max(abs(bx - ax), abs(by - ay))) + 1, 8000)
        pts = [(ax + (bx - ax) * i / steps, ay + (by - ay) * i / steps)
               for i in range(steps + 1)]
        if casing is not None:
            for x, y in pts:
                fill_rect(buf, x - 1, y - 1, 3, 3, casing)
        for x, y in pts:
            fill_rect(buf, x - 1, y - 1, 2, 2, core)


def rect_outline(buf, x0, y0, x1, y1, casing, core):
    for a, b in (((x0, y0), (x1, y0)), ((x0, y1), (x1, y1)),
                 ((x0, y0), (x0, y1)), ((x1, y0), (x1, y1))):
        stamp_segment(buf, a, b, casing, core)


class Viewer:
    def __init__(self, cache, server_sock=None, show=True):
        self.server_sock = server_sock
        self.cx = self.cy = 0
        self.spp = 1.0              # dbu per screen pixel
        self.visible = set()
        self.gen = 0
        self.last_frame = None      # (pixbuf, bbox, dbu_per_px)
        self._ov_pixbufs = {}
        self._drag = None
        self._zoomdrag = None       # rubber-band anchor (view px)
        self._band_cur = None
        self._debounce = None
        self._did_fit = False
        self.worker = None
        self._layer_checks = {}
        self.depth_value = 999
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
        self._labels = []           # Gtk.Label pool for ruler distances

        self.window = Gtk.Window(title=APP)
        self.window.set_default_size(1280, 860)
        self.window.connect("delete-event", lambda *_: self._quit())
        self.window.connect("key-press-event", self._on_key)

        hbox = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL)
        self.window.add(hbox)

        side = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=2)
        side.set_size_request(210, -1)
        hbox.pack_start(side, False, False, 0)
        title = Gtk.Label()
        title.set_markup("<b>%s</b>" % APP)
        title.set_xalign(0.0)
        title.set_margin_start(10)
        title.set_margin_top(8)
        side.pack_start(title, False, False, 0)
        self._src_label = Gtk.Label(label="")
        self._src_label.set_xalign(0.0)
        self._src_label.set_margin_start(10)
        side.pack_start(self._src_label, False, False, 0)

        scroller = Gtk.ScrolledWindow()
        scroller.set_policy(Gtk.PolicyType.NEVER, Gtk.PolicyType.AUTOMATIC)
        self._layers_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
        scroller.add(self._layers_box)
        side.pack_start(scroller, True, True, 4)

        self._depth_btn = Gtk.Button(label="depth: full")
        self._depth_btn.connect("clicked", lambda *_: self._depth_dialog())
        side.pack_start(self._depth_btn, False, False, 2)
        brow = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=2)
        side.pack_start(brow, False, False, 4)
        for text, cb in (("all", self._all_layers),
                         ("none", self._no_layers),
                         ("fit", lambda: self.fit()),
                         ("clip…", self._clip_dialog)):
            b = Gtk.Button(label=text)
            b.connect("clicked", lambda _w, f=cb: f())
            brow.pack_start(b, True, True, 0)

        main = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
        hbox.pack_start(main, True, True, 0)
        self.overlay = Gtk.Overlay()
        # the image lives in a ScrolledWindow so the window can shrink:
        # a bare Gtk.Image's minimum size is its pixbuf, and since we
        # render pixbufs at allocation size that would ratchet the window
        # ever larger (flateyes uses the same containment)
        self.scroller = Gtk.ScrolledWindow()
        try:
            self.scroller.set_policy(Gtk.PolicyType.EXTERNAL,
                                     Gtk.PolicyType.EXTERNAL)
        except AttributeError:  # GTK < 3.16
            self.scroller.set_policy(Gtk.PolicyType.NEVER,
                                     Gtk.PolicyType.NEVER)
        self.ebox = Gtk.EventBox()
        self.image = Gtk.Image()
        self.image.set_halign(Gtk.Align.START)
        self.image.set_valign(Gtk.Align.START)
        self.ebox.add(self.image)
        self.scroller.add(self.ebox)
        self.overlay.add(self.scroller)
        main.pack_start(self.overlay, True, True, 0)

        sbar = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL)
        self.status = Gtk.Label(label="")
        self.status.set_xalign(0.0)
        self.status.set_margin_start(8)
        self.rstatus = Gtk.Label(label="")
        self.rstatus.set_margin_end(10)
        sbar.pack_start(self.status, True, True, 0)
        sbar.pack_end(self.rstatus, False, False, 0)
        main.pack_start(sbar, False, False, 2)

        self.ebox.add_events(
            Gdk.EventMask.BUTTON_PRESS_MASK |
            Gdk.EventMask.BUTTON_RELEASE_MASK |
            Gdk.EventMask.POINTER_MOTION_MASK |
            Gdk.EventMask.SCROLL_MASK | Gdk.EventMask.SMOOTH_SCROLL_MASK)
        self.ebox.connect("button-press-event", self._on_press)
        self.ebox.connect("button-release-event", self._on_release)
        self.ebox.connect("motion-notify-event", self._on_motion)
        self.ebox.connect("scroll-event", self._on_scroll)
        self._alloc_size = None
        self.scroller.connect("size-allocate", self._on_allocate)
        self.ebox.connect("realize", lambda w: self._set_cursor("crosshair"))

        if server_sock is not None:
            GLib.io_add_watch(server_sock.fileno(), GLib.IO_IN,
                              self._on_incoming)
        GLib.timeout_add(POLL_MS, self._poll)

        self._apply_cache(cache)
        if show:
            self.window.show_all()

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
        self._ov_pixbufs = {}
        self._clear_pending()
        self.rulers = []
        self._ruler_start = None
        self._snap_res = None
        self.selection = None
        self._sel_text = ""
        self._pick_px = None
        src = self.meta["src"]
        self.window.set_title(
            "%s - %s" % (APP, os.path.basename(src["path"])))
        self._src_label.set_text(
            "%.2f GB · grid %dx%d" % (src["size"] / 1e9,
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
        for child in self._layers_box.get_children():
            self._layers_box.remove(child)
        self._layer_checks = {}
        for l in self.meta["layers"]:
            key = (l["layer"], l["datatype"])
            cb = Gtk.CheckButton()
            lbl = Gtk.Label()
            lbl.set_markup('<span foreground="%s">%s  %d/%d</span>'
                           % (l["color"], GLib.markup_escape_text(l["name"]),
                              key[0], key[1]))
            lbl.set_xalign(0.0)
            cb.add(lbl)
            cb.set_active(True)
            cb.connect("toggled", self._on_layer_toggled, key)
            self._layers_box.pack_start(cb, False, False, 0)
            self._layer_checks[key] = cb
        self._layers_box.show_all()

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

    def _on_incoming(self, _source, _cond):
        try:
            conn, _ = self.server_sock.accept()
        except OSError:
            return True
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
                    error = "ERR %s" % exc
            try:
                conn.sendall((error or "OK").encode("utf-8") + b"\n")
            except OSError:
                pass
            if not error:
                self._present()
        finally:
            conn.close()
        return True

    def _present(self):
        self.window.deiconify()
        self.window.present()
        self.window.set_urgency_hint(True)

    # ---- geometry ----------------------------------------------------------
    def _viewport_size(self):
        alloc = self.scroller.get_allocation()
        w = alloc.width if alloc.width >= 50 else 1200
        h = alloc.height if alloc.height >= 50 else 800
        return w, h

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
        if len(self.visible) != len(self._layer_checks):
            return self._visible_list()
        return None

    # ---- display composition (no cairo: pixbuf ops only) -------------------
    def _display(self):
        w, h = self._viewport_size()
        disp = GdkPixbuf.Pixbuf.new(GdkPixbuf.Colorspace.RGB, False, 8, w, h)
        disp.fill(BLACK)
        bbox = self.view_bbox()
        span = self.tiles_spanned(bbox)
        live = 0 < span <= MAX_LIVE_TILES and bool(self.visible)
        drew = False
        if live and self.last_frame is not None:
            pix, fb, fspp = self.last_frame
            if 0.2 < fspp / self.spp < 5.0:
                self._composite_world(disp, pix, fb, bbox)
                drew = True
        if not drew and self.visible:
            src = self._ov_pixbuf()
            if src is not None:
                ob = self.meta["overview"]["bbox"]
                self._composite_world(disp, src, ob, bbox)
        self._draw_overlays(disp, bbox)
        self.image.set_from_pixbuf(disp)
        self._update_labels(bbox)

    def _composite_world(self, disp, src, src_bbox, bbox):
        sw = src.get_width()
        if sw < 1 or src_bbox[2] <= src_bbox[0]:
            return
        sppd = sw / (src_bbox[2] - src_bbox[0])   # src px per dbu
        scale = (1.0 / self.spp) / sppd           # view px per src px
        if scale <= 0:
            return
        off_x = (src_bbox[0] - bbox[0]) / self.spp
        off_y = (bbox[3] - src_bbox[3]) / self.spp
        dx0 = max(0, int(math.floor(off_x)))
        dy0 = max(0, int(math.floor(off_y)))
        dx1 = min(disp.get_width(), int(math.ceil(off_x + sw * scale)))
        dy1 = min(disp.get_height(),
                  int(math.ceil(off_y + src.get_height() * scale)))
        if dx1 <= dx0 or dy1 <= dy0:
            return
        interp = GdkPixbuf.InterpType.BILINEAR if scale < 1 \
            else GdkPixbuf.InterpType.NEAREST
        src.composite(disp, dx0, dy0, dx1 - dx0, dy1 - dy0,
                      off_x, off_y, scale, scale, interp, 255)

    def _ov_pixbuf(self):
        ov = self.meta.get("overview")
        if not ov:
            return None
        if len(self.visible) == 1:
            l, d = next(iter(self.visible))
            key = "%d/%d" % (l, d)
        else:
            key = "__all__"
        if key not in self._ov_pixbufs:
            fname = ov["files"].get(key) or ov["files"].get("__all__")
            try:
                self._ov_pixbufs[key] = GdkPixbuf.Pixbuf.new_from_file(
                    os.path.join(self.cache.dir, "overview", fname))
            except Exception:
                self._ov_pixbufs[key] = None
        return self._ov_pixbufs[key] or self._ov_pixbufs.get("__all__")

    def _draw_overlays(self, disp, bbox):
        def sx(v):
            return (v - bbox[0]) / self.spp

        def sy(v):
            return (bbox[3] - v) / self.spp

        if self.selection and self.selection.get("points"):
            pts = [(sx(x), sy(y)) for x, y in self.selection["points"]]
            for a, b in zip(pts, pts[1:] + pts[:1]):
                stamp_segment(disp, a, b, BLACK, SEL_CORE)
        segs = list(self.rulers)
        if self.mode == "ruler" and self._ruler_start is not None:
            segs.append((*self._ruler_start, *self._ruler_end_preview()))
        for x0, y0, x1, y1 in segs:
            a, b = (sx(x0), sy(y0)), (sx(x1), sy(y1))
            stamp_segment(disp, a, b, BLACK, RULER_CORE)
            for x, y in (a, b):  # end markers
                fill_rect(disp, x - 3, y - 3, 7, 7, BLACK)
                fill_rect(disp, x - 2, y - 2, 5, 5, RULER_CORE)
        if self.mode == "ruler" and self.snap_on and self._snap_res \
                and self._snap_res.get("found"):
            mx, my = sx(self._snap_res["x"]), sy(self._snap_res["y"])
            color = SNAP_VERTEX if self._snap_res["snap"] == "vertex" \
                else SNAP_EDGE
            rect_outline(disp, mx - 5, my - 5, mx + 5, my + 5, None, color)
            fill_rect(disp, mx - 9, my, 19, 1, color)
            fill_rect(disp, mx, my - 9, 1, 19, color)
        if self._zoomdrag is not None and self._band_cur is not None:
            x0, y0 = self._zoomdrag
            x1, y1 = self._band_cur
            color = BAND_IN if x1 >= x0 else BAND_OUT
            rect_outline(disp, x0, y0, x1, y1, BLACK, color)

    def _update_labels(self, bbox):
        """Ruler distance labels: a pool of Gtk.Labels on the overlay."""
        needed = []
        segs = list(self.rulers)
        if self.mode == "ruler" and self._ruler_start is not None:
            segs.append((*self._ruler_start, *self._ruler_end_preview()))
        for x0, y0, x1, y1 in segs:
            d_um = math.hypot(x1 - x0, y1 - y0) * self.dbu
            mx = ((x0 + x1) / 2 - bbox[0]) / self.spp
            my = (bbox[3] - (y0 + y1) / 2) / self.spp
            needed.append((mx + 8, my - 22, "%.4f um" % d_um))
        while len(self._labels) < len(needed):
            lbl = Gtk.Label()
            lbl.set_halign(Gtk.Align.START)
            lbl.set_valign(Gtk.Align.START)
            self.overlay.add_overlay(lbl)
            self._labels.append(lbl)
        w, h = self._viewport_size()
        for lbl, (x, y, text) in zip(self._labels, needed):
            lbl.set_markup('<span background="#101010" foreground='
                           '"#ffe97a"> %s </span>'
                           % GLib.markup_escape_text(text))
            lbl.set_margin_start(int(max(0, min(x, w - 90))))
            lbl.set_margin_top(int(max(0, min(y, h - 20))))
            lbl.show()
        for lbl in self._labels[len(needed):]:
            lbl.hide()

    # ---- drawing / rendering ------------------------------------------------
    def redraw(self, immediate=False):
        bbox = self.view_bbox()
        span = self.tiles_spanned(bbox)
        live = 0 < span <= MAX_LIVE_TILES and bool(self.visible)
        self._display()
        if not live:
            if self._debounce is not None:
                GLib.source_remove(self._debounce)
                self._debounce = None
            self._clear_pending()
            self._set_status(bbox, "overview (%d tiles spanned)" % span)
            return
        if self._debounce is not None:
            GLib.source_remove(self._debounce)
        self._debounce = GLib.timeout_add(
            1 if immediate else DEBOUNCE_MS, self._submit_render)
        self._set_status(bbox, "live (%d tiles)" % span)

    def _submit_render(self):
        self._debounce = None
        bbox = self.view_bbox()
        w, h = self._viewport_size()
        self.gen += 1
        self.worker.submit({
            "kind": "render", "gen": self.gen,
            "bbox": tuple(int(round(v)) for v in bbox),
            "w": w, "h": h, "depth": self._depth(),
            "visible": self._layers_arg()})
        self._pending = self.gen
        self._pending_t0 = time.perf_counter()
        self.rstatus.set_text("rendering…")
        if self._pending_timer is None:
            self._pending_timer = GLib.timeout_add(400, self._pending_tick)
        return False  # one-shot timeout

    def _pending_tick(self):
        if self._pending is None:
            self._pending_timer = None
            self.rstatus.set_text("")
            return False
        el = time.perf_counter() - self._pending_t0
        self.rstatus.set_text("rendering…" if el < 1.5
                              else "rendering… %.0fs" % el)
        return True

    def _clear_pending(self):
        self._pending = None
        if self._pending_timer is not None:
            GLib.source_remove(self._pending_timer)
            self._pending_timer = None
        self.rstatus.set_text("")

    def _poll(self):
        if self._quitting:
            return False
        try:
            while True:
                res = self.worker.res.get_nowait()
                kind = res.get("kind")
                if kind == "frame":
                    if res["gen"] == self._pending:
                        self._clear_pending()
                    if res["gen"] == self.gen:
                        loader = GdkPixbuf.PixbufLoader.new_with_type("png")
                        loader.write(res["png"])
                        loader.close()
                        pix = loader.get_pixbuf()
                        fb = res["bbox"]
                        fspp = (fb[2] - fb[0]) / max(1, pix.get_width())
                        self.last_frame = (pix, fb, fspp)
                        self._display()
                        self._set_status(self.view_bbox(),
                                         "live (%d tiles, %d ms)"
                                         % (res["tiles"], res["ms"]))
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
        return True

    def _set_status(self, bbox, mode):
        w_um = (bbox[2] - bbox[0]) * self.dbu
        h_um = (bbox[3] - bbox[1]) * self.dbu
        self.status.set_text("view %.1f x %.1f um   |   %s"
                             % (w_um, h_um, mode))

    # ---- interaction --------------------------------------------------------
    def fit(self):
        bb = self.meta["bbox"]
        w, h = self._viewport_size()
        self.cx = (bb[0] + bb[2]) / 2
        self.cy = (bb[1] + bb[3]) / 2
        self.spp = max((bb[2] - bb[0]) / w, (bb[3] - bb[1]) / h) * 1.05
        self.redraw()

    def _on_allocate(self, _w, alloc):
        # GTK emits size-allocate on every set_from_pixbuf; reacting to
        # all of them would loop redraw -> allocate -> redraw forever
        # (visible as an endlessly re-submitted render). Only a real
        # size change matters.
        size = (alloc.width, alloc.height)
        if size == self._alloc_size:
            return
        self._alloc_size = size
        if not self._did_fit and alloc.width > 50:
            self._did_fit = True
            self.fit()
        else:
            self.redraw()

    def _set_cursor(self, name):
        win = self.ebox.get_window()
        if win is not None:
            try:
                win.set_cursor(Gdk.Cursor.new_from_name(
                    win.get_display(), name))
            except Exception:
                pass

    def _on_press(self, _w, ev):
        if ev.button == 1:
            if self.mode == "ruler":
                self._ruler_free = bool(ev.state &
                                        Gdk.ModifierType.SHIFT_MASK)
                self._ruler_click(ev)
                return True
            self._zoomdrag = (ev.x, ev.y)
            self._band_cur = None
        elif ev.button in (2, 3):
            self._drag = (ev.x, ev.y)
            self._set_cursor("move")
        return True

    def _on_release(self, _w, ev):
        if ev.button in (2, 3):
            self._drag = None
            self._set_cursor("crosshair")
            return True
        if ev.button != 1 or self._zoomdrag is None:
            return True
        x0, y0 = self._zoomdrag
        self._zoomdrag = None
        self._band_cur = None
        dx, dy = abs(ev.x - x0), abs(ev.y - y0)
        if dx < 5 or dy < 5:
            self._display()          # erase the band
            self._pick_click(ev)     # a click, not a box
            return True
        bbox = self.view_bbox()
        lx0 = bbox[0] + min(x0, ev.x) * self.spp
        lx1 = bbox[0] + max(x0, ev.x) * self.spp
        ly0 = bbox[3] - max(y0, ev.y) * self.spp
        ly1 = bbox[3] - min(y0, ev.y) * self.spp
        w, h = self._viewport_size()
        self.cx = (lx0 + lx1) / 2
        self.cy = (ly0 + ly1) / 2
        if ev.x >= x0:  # forward: the box fills the viewport
            self.spp = max((lx1 - lx0) / w, (ly1 - ly0) / h)
        else:           # backward: zoom out by the viewport/box ratio
            self.spp *= max(w / dx, h / dy)
        self.redraw()
        return True

    def _on_motion(self, _w, ev):
        self._update_cursor(ev)
        if self._drag is not None and ev.state & (
                Gdk.ModifierType.BUTTON2_MASK |
                Gdk.ModifierType.BUTTON3_MASK):
            ddx, ddy = ev.x - self._drag[0], ev.y - self._drag[1]
            self._drag = (ev.x, ev.y)
            self.cx -= ddx * self.spp
            self.cy += ddy * self.spp
            self.redraw()
            return True
        if self._zoomdrag is not None and \
                ev.state & Gdk.ModifierType.BUTTON1_MASK:
            self._band_cur = (ev.x, ev.y)
            self._display()
            return True
        self._hover(ev)
        return True

    def _on_scroll(self, _w, ev):
        delta = 0.0
        if ev.direction == Gdk.ScrollDirection.UP:
            delta = 1.0
        elif ev.direction == Gdk.ScrollDirection.DOWN:
            delta = -1.0
        elif ev.direction == Gdk.ScrollDirection.SMOOTH:
            ok, _dx, dy = ev.get_scroll_deltas()
            if ok:
                delta = max(-3.0, min(3.0, -dy))
        if delta:
            self._zoom_at(ev.x, ev.y, 0.9 ** delta)
        return True

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
    def _on_key(self, _w, ev):
        focus = self.window.get_focus()
        if isinstance(focus, Gtk.Entry):
            return False  # typing in the depth spinbox etc.
        name = Gdk.keyval_name(ev.keyval) or ""
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
        elif name == "a":
            self._set_depth(999)
        elif len(name) == 1 and name.isdigit():
            self._set_depth(int(name))
        elif name.startswith("KP_") and name[3:].isdigit():
            self._set_depth(int(name[3:]))
        else:
            return False
        return True

    # ---- depth -----------------------------------------------------------------
    def _depth(self):
        d = self.depth_value
        return None if d >= 999 else max(0, d)

    def _set_depth(self, n):
        self.depth_value = max(0, min(999, int(n)))
        if self._ddlg is not None:
            spin = getattr(self._ddlg, "_spin", None)
            if spin is not None and \
                    int(spin.get_value()) != self.depth_value:
                spin.set_value(self.depth_value)
        self._on_depth()

    def _on_depth(self):
        d = self._depth()
        self._depth_btn.set_label(
            "depth: full" if d is None else "depth: %d" % d)
        self.redraw(immediate=True)

    def _depth_dialog(self):
        if self._ddlg is not None:
            self._ddlg.present()
            return
        dlg = Gtk.Window(title="hierarchy depth")
        dlg.set_transient_for(self.window)
        dlg.set_resizable(False)
        self._ddlg = dlg
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
        box.set_margin_top(10)
        box.set_margin_bottom(10)
        box.set_margin_start(14)
        box.set_margin_end(14)
        dlg.add(box)
        box.pack_start(Gtk.Label(
            label="hierarchy depth (0 = top only, 999 = full)"),
            False, False, 0)
        row = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=4)
        box.pack_start(row, False, False, 0)
        spin = Gtk.SpinButton.new_with_range(0, 999, 1)
        spin.set_value(self.depth_value)
        spin.connect("value-changed",
                     lambda s: self._set_depth(int(s.get_value())))
        dlg._spin = spin
        row.pack_start(spin, False, False, 0)
        for preset in (0, 1, 2, 3, 999):
            b = Gtk.Button(label="full" if preset == 999 else str(preset))
            b.connect("clicked", lambda _w, p=preset: self._set_depth(p))
            row.pack_start(b, False, False, 0)
        note = Gtk.Label()
        note.set_markup(
            "<small>cells beyond the limit are drawn as outline frames"
            "\nwith names (live mode only; the far-zoom overview stays"
            "\nfull depth) - keys: 0-9 = depth, a = full</small>")
        note.set_xalign(0.0)
        box.pack_start(note, False, False, 0)
        close = Gtk.Button(label="close")
        close.connect("clicked", lambda *_: dlg.destroy())
        box.pack_start(close, False, False, 0)

        def _gone(*_a):
            self._ddlg = None
        dlg.connect("destroy", _gone)
        dlg.show_all()

    # ---- ruler / snap / pick -----------------------------------------------
    def _update_cursor(self, ev):
        bbox = self.view_bbox()
        self._cursor = (bbox[0] + ev.x * self.spp,
                        bbox[3] - ev.y * self.spp)

    def _hover(self, ev):
        x, y = self._cursor
        parts = ["x %.3f  y %.3f um" % (x * self.dbu, y * self.dbu)]
        if self.mode == "ruler":
            self._ruler_free = bool(ev.state &
                                    Gdk.ModifierType.SHIFT_MASK)
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
        self.status.set_text("   |   ".join(parts))

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
    def _on_layer_toggled(self, cb, key):
        if cb.get_active():
            self.visible.add(key)
        else:
            self.visible.discard(key)
        self.redraw(immediate=True)

    def _all_layers(self):
        for cb in self._layer_checks.values():
            cb.set_active(True)

    def _no_layers(self):
        for cb in self._layer_checks.values():
            cb.set_active(False)

    def _clip_dialog(self):
        bbox = self.view_bbox()
        um = [round(v * self.dbu, 1) for v in bbox]
        dlg = Gtk.FileChooserDialog(title="save clip as OASIS",
                                    parent=self.window,
                                    action=Gtk.FileChooserAction.SAVE)
        dlg.add_buttons("Cancel", Gtk.ResponseType.CANCEL,
                        "Save", Gtk.ResponseType.OK)
        dlg.set_do_overwrite_confirmation(True)
        dlg.set_current_name("ot_clip_%s_%s_%s_%sum.oas"
                             % (um[0], um[1], um[2], um[3]))
        out = dlg.get_filename() if dlg.run() == Gtk.ResponseType.OK \
            else None
        dlg.destroy()
        if not out:
            return
        self.worker.submit({"kind": "clip",
                            "bbox": tuple(int(round(v)) for v in bbox),
                            "layers": self._layers_arg(), "out": out})
        self._set_status(bbox, "clipping…")

    # ---- shutdown -------------------------------------------------------------
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
        if Gtk.main_level() > 0:
            Gtk.main_quit()


def run_viewer(cache, server_sock=None):
    import_gtk()
    viewer = Viewer(cache, server_sock)
    try:
        import signal as _signal
        GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, _signal.SIGINT,
                             lambda *_: (viewer._quit(), False)[1])
    except Exception:
        pass
    try:
        Gtk.main()
    except KeyboardInterrupt:
        pass
    finally:
        viewer._quit()
