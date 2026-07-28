"""Native viewer - GTK3/PyGObject shell.

Same hard environment constraints as flateyes (the sibling image viewer):
targets are closed-network RHEL-family hosts where only PyGObject/GTK3 is
stock and NOTHING can be installed. In particular there is NO pycairo, so
the GTK "draw" signal is unusable: every frame is composed into a
GdkPixbuf (klayout render frames, rubber band, rulers,
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

APP = "floe"
POLL_MS = 25
DEBOUNCE_MS = 120

BLACK = 0x000000FF
BAND_IN = 0x8ECDF5FF       # forward drag: zoom in
BAND_OUT = 0xF5B62EFF      # backward drag: zoom out
RULER_CORE = 0xFFE97AFF
SNAP_VERTEX = 0x66FFCCFF
SNAP_EDGE = 0x66CCFFFF
SEL_CORE = 0xFFFFFFFF
GOTO_MARK = 0xFF66D9FF

AUTO_DEPTH_BUDGET = 120_000   # est. shapes auto depth allows on screen
MIN_SPP = 0.01     # max zoom-in: 1 px = 0.01 dbu; keeps render bboxes
                   # from collapsing to zero width after int rounding

MINIMAP_PX = 110           # longest edge of the minimap (view px)
MINIMAP_MARGIN = 12
MINIMAP_DOT_MIN = 6        # view box smaller than this becomes a dot
MINIMAP_BG = 0x141414FF
MINIMAP_EDGE = 0x666666FF
MINIMAP_VIEW = 0x8ECDF5FF


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


def fill_triangle(buf, p0, p1, p2, color):
    """Solid triangle via 1-px horizontal scanline fills."""
    pts = (p0, p1, p2)
    y0 = int(math.floor(min(p[1] for p in pts)))
    y1 = int(math.ceil(max(p[1] for p in pts)))
    for y in range(y0, y1 + 1):
        yc = y + 0.5
        xs = []
        for a, b in ((p0, p1), (p1, p2), (p2, p0)):
            if (a[1] <= yc) != (b[1] <= yc):
                t = (yc - a[1]) / (b[1] - a[1])
                xs.append(a[0] + t * (b[0] - a[0]))
        if len(xs) >= 2:
            fill_rect(buf, min(xs), y, max(xs) - min(xs) + 1, 1, color)


def stamp_arrow(buf, tip, ang, casing, core, size=11, half=4):
    """Solid triangular arrowhead (the original tkinter-viewer look):
    tip at the given point, pointing along ang (screen radians)."""
    ux, uy = math.cos(ang), math.sin(ang)
    px, py = -uy, ux

    def tri(tx, ty, sz, hf):
        bx, by = tx - sz * ux, ty - sz * uy
        return ((tx, ty), (bx + hf * px, by + hf * py),
                (bx - hf * px, by - hf * py))

    if casing is not None:
        fill_triangle(buf, *tri(tip[0] + 2 * ux, tip[1] + 2 * uy,
                                size + 4, half + 1.5), casing)
    fill_triangle(buf, *tri(tip[0], tip[1], size, half), core)


def rect_outline(buf, x0, y0, x1, y1, casing, core):
    for a, b in (((x0, y0), (x1, y0)), ((x0, y1), (x1, y1)),
                 ((x0, y0), (x0, y1)), ((x1, y0), (x1, y1))):
        stamp_segment(buf, a, b, casing, core)


def frame_rect(buf, x0, y0, w, h, color):
    """1-px rectangle border (rect_outline is too heavy for the minimap)."""
    fill_rect(buf, x0, y0, w, 1, color)
    fill_rect(buf, x0, y0 + h - 1, w, 1, color)
    fill_rect(buf, x0, y0, 1, h, color)
    fill_rect(buf, x0 + w - 1, y0, 1, h, color)


class Viewer:
    def __init__(self, cache, server_sock=None, show=True, goto=None):
        self.server_sock = server_sock
        self.cx = self.cy = 0
        self.spp = 1.0              # dbu per screen pixel
        self._start_goto = goto     # [x_um, y_um(, window_um)] from the CLI
        self.visible = set()
        self.gen = 0
        self.last_frame = None      # (pixbuf, bbox, dbu_per_px, key)
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
        self._layer_checks = {}
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
        self.dstatus = Gtk.Label(label="depth: auto")
        self.dstatus.set_margin_end(14)
        self.vstatus = Gtk.Label(label="")
        self.vstatus.set_margin_end(14)
        sbar.pack_start(self.status, True, True, 0)
        sbar.pack_end(self.rstatus, False, False, 0)
        sbar.pack_end(self.dstatus, False, False, 0)
        sbar.pack_end(self.vstatus, False, False, 0)
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
        # like _poll: any exception here would make GLib drop the watch,
        # silently killing single-instance forwarding for the session
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
        except Exception:
            import traceback
            traceback.print_exc()
        finally:
            conn.close()
        return True

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
        w, h = self._viewport_size()
        disp = GdkPixbuf.Pixbuf.new(GdkPixbuf.Colorspace.RGB, False, 8, w, h)
        disp.fill(BLACK)
        bbox = self.view_bbox()
        if self.last_frame is not None and \
                self._frame_compatible(self.last_frame):
            # the stale frame is never rescaled (a blurry zoomed base
            # reads as a glitch): it stays frozen at its own resolution
            # until the fresh frame lands. While the scale matches, the
            # anchor tracks the center so pans move 1:1; once a zoom
            # changes the scale the anchor stays put - zoom-at-cursor
            # center compensation must not slide the frozen image.
            frame, fb, fspp, _key = self.last_frame
            if self._frame_anchor is None or \
                    abs(self.spp / fspp - 1.0) < 0.001:
                self._frame_anchor = (self.cx, self.cy)
            ax, ay = self._frame_anchor
            vb = (ax - w / 2 * fspp, ay - h / 2 * fspp,
                  ax + w / 2 * fspp, ay + h / 2 * fspp)
            self._composite_world(disp, frame, fb, vb, fspp)
            # world-anchored overlays (rulers, selection, snap) stay
            # glued to the frozen base and jump together with it when
            # the fresh frame lands; the minimap keeps the real view
            obox, ospp = vb, fspp
        else:
            obox, ospp = bbox, self.spp
        self._draw_overlays(disp, obox, ospp, bbox)
        if os.environ.get("FLOE_DUMP"):
            # diagnosis: exactly what is handed to the screen widget
            disp.savev("/tmp/floe_disp.png", "png", [], [])
        self.image.set_from_pixbuf(disp)
        self._update_labels(obox, ospp)

    def _composite_world(self, disp, src, src_bbox, bbox, spp=None):
        spp = spp or self.spp
        sw = src.get_width()
        if sw < 1 or src_bbox[2] <= src_bbox[0]:
            return
        sppd = sw / (src_bbox[2] - src_bbox[0])   # src px per dbu
        scale = (1.0 / spp) / sppd                # view px per src px
        if scale <= 0:
            return
        off_x = (src_bbox[0] - bbox[0]) / spp
        off_y = (bbox[3] - src_bbox[3]) / spp
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

    def _draw_overlays(self, disp, obox, ospp, bbox):
        def sx(v):
            return (v - obox[0]) / ospp

        def sy(v):
            return (obox[3] - v) / ospp

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
            ang = math.atan2(b[1] - a[1], b[0] - a[0])
            stamp_arrow(disp, b, ang, BLACK, RULER_CORE)      # outward
            stamp_arrow(disp, a, ang + math.pi, BLACK, RULER_CORE)
        if self.mode == "ruler" and self.snap_on and self._snap_res \
                and self._snap_res.get("found"):
            mx, my = sx(self._snap_res["x"]), sy(self._snap_res["y"])
            color = SNAP_VERTEX if self._snap_res["snap"] == "vertex" \
                else SNAP_EDGE
            rect_outline(disp, mx - 5, my - 5, mx + 5, my + 5, None, color)
            fill_rect(disp, mx - 9, my, 19, 1, color)
            fill_rect(disp, mx, my - 9, 1, 19, color)
        if self.goto_mark is not None:
            gx, gy = sx(self.goto_mark[0]), sy(self.goto_mark[1])
            if -12 <= gx <= disp.get_width() + 12 and \
                    -12 <= gy <= disp.get_height() + 12:
                stamp_segment(disp, (gx - 10, gy - 10), (gx + 10, gy + 10),
                              BLACK, GOTO_MARK)
                stamp_segment(disp, (gx - 10, gy + 10), (gx + 10, gy - 10),
                              BLACK, GOTO_MARK)
        if self._zoomdrag is not None and self._band_cur is not None:
            x0, y0 = self._zoomdrag
            x1, y1 = self._band_cur
            color = BAND_IN if x1 >= x0 else BAND_OUT
            rect_outline(disp, x0, y0, x1, y1, BLACK, color)
        self._draw_minimap(disp, bbox)

    def _draw_minimap(self, disp, bbox):
        """Die outline in the bottom-right corner with the current view
        marked: a box while it is still readable, a dot once the zoom
        makes the box degenerate."""
        bb = self.meta["bbox"]
        bw, bh = bb[2] - bb[0], bb[3] - bb[1]
        if bw <= 0 or bh <= 0:
            return
        scale = MINIMAP_PX / max(bw, bh)
        mw, mh = max(2, round(bw * scale)), max(2, round(bh * scale))
        x0 = disp.get_width() - MINIMAP_MARGIN - mw
        y0 = disp.get_height() - MINIMAP_MARGIN - mh
        if x0 < 0 or y0 < 0:
            return  # viewport too small for a minimap
        fill_rect(disp, x0, y0, mw, mh, MINIMAP_BG)
        frame_rect(disp, x0, y0, mw, mh, MINIMAP_EDGE)

        def mx(v):
            return x0 + (v - bb[0]) * scale

        def my(v):
            return y0 + (bb[3] - v) * scale

        vw = (bbox[2] - bbox[0]) * scale
        vh = (bbox[3] - bbox[1]) * scale
        if vw >= MINIMAP_DOT_MIN and vh >= MINIMAP_DOT_MIN:
            # the fit view overshoots the die by its margin: clip the
            # view box to the minimap
            rx0 = max(x0, mx(bbox[0]))
            ry0 = max(y0, my(bbox[3]))
            rx1 = min(x0 + mw - 1, mx(bbox[2]))
            ry1 = min(y0 + mh - 1, my(bbox[1]))
            frame_rect(disp, rx0, ry0, max(2, round(rx1 - rx0 + 1)),
                       max(2, round(ry1 - ry0 + 1)), MINIMAP_VIEW)
        else:
            px, py = mx(self.cx), my(self.cy)
            fill_rect(disp, px - 3, py - 3, 7, 7, BLACK)
            fill_rect(disp, px - 2, py - 2, 5, 5, MINIMAP_VIEW)

    def _update_labels(self, obox, ospp):
        """Ruler distance labels: a pool of Gtk.Labels on the overlay."""
        needed = []
        segs = list(self.rulers)
        if self.mode == "ruler" and self._ruler_start is not None:
            segs.append((*self._ruler_start, *self._ruler_end_preview()))
        for x0, y0, x1, y1 in segs:
            d_um = math.hypot(x1 - x0, y1 - y0) * self.dbu
            mx = ((x0 + x1) / 2 - obox[0]) / ospp
            my = (obox[3] - (y0 + y1) / 2) / ospp
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
            if self._debounce is not None:
                GLib.source_remove(self._debounce)
                self._debounce = None
            self._clear_pending()
            self._set_status(bbox, "no layers visible")
            return
        mode = "live (%d tiles)" % span if scope == "live" \
            else "far view (skeleton)"
        if self._covered(bbox, scope):
            if self._debounce is not None:
                GLib.source_remove(self._debounce)
                self._debounce = None
            self._set_status(bbox, mode)
            return
        if self._debounce is not None:
            GLib.source_remove(self._debounce)
        self._pending_scope = scope
        self._debounce = GLib.timeout_add(
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
        self.rstatus.set_text("rendering…")
        self._set_cursor("wait")  # mouse input is ignored until the frame
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
        self._set_cursor("move" if self._drag is not None else "crosshair")

    def _poll(self):
        if self._quitting:
            return False
        # watchdog: a render-service child that died (spawn failure,
        # crash, OOM kill) would otherwise leave a silent black view
        # with "rendering…" forever
        w = self.worker
        if w is not None and not w.alive() \
                and not getattr(w, "_died_reported", False):
            w._died_reported = True
            self._clear_pending()
            self._set_status(
                self.view_bbox(),
                "error: render service died (exit %s) - see terminal; "
                "restart the viewer" % w.exitcode())
        try:
            while True:
                res = self.worker.res.get_nowait()
                try:
                    self._handle_result(res)
                except Exception as exc:
                    # One bad result must never kill this callback: GLib
                    # removes a raising timeout source, which would freeze
                    # the viewer on a stale (usually black) frame with no
                    # error anywhere but the terminal. Keep polling and put
                    # the failure where the user looks - the status bar.
                    import traceback
                    traceback.print_exc()
                    self._clear_pending()
                    self._set_status(
                        self.view_bbox(),
                        "error: %s result failed: %s (see terminal)"
                        % (res.get("kind"), exc))
        except queue.Empty:
            pass
        return True

    def _handle_result(self, res):
        kind = res.get("kind")
        if kind == "frame":
            if res["gen"] == self._pending:
                self._clear_pending()
                self.rstatus.set_text("rendering done.")
            if res["gen"] == self.gen:
                loader = GdkPixbuf.PixbufLoader.new_with_type("png")
                loader.write(res["png"])
                loader.close()
                pix = loader.get_pixbuf()
                if os.environ.get("FLOE_DUMP"):
                    # diagnosis: the frame as received from the service
                    pix.savev("/tmp/floe_frame.png", "png", [], [])
                fb = res["bbox"]
                fspp = (fb[2] - fb[0]) / max(1, pix.get_width())
                key = self._job_keys.get(res["gen"])
                used = self._job_depth.get(res["gen"])
                self.last_frame = (pix, fb, fspp, key)
                self._display()
                if res.get("bg"):
                    return  # silent margin upgrade
                self._depth_used = used
                self.dstatus.set_text(self._depth_label())
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

    def _set_status(self, bbox, mode):
        w_um = (bbox[2] - bbox[0]) * self.dbu
        h_um = (bbox[3] - bbox[1]) * self.dbu
        self.vstatus.set_text("view %.1f x %.1f um" % (w_um, h_um))
        self.status.set_text(mode)

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
            if self._start_goto is not None:
                self.spp = self._fit_spp()  # zoom baseline if no window given
                self.goto(*self._start_goto)
                self._start_goto = None
            else:
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
        if self._pending is not None:
            return True  # render in flight: mouse input waits
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
        if self._pending is not None:
            return True  # render in flight: mouse input waits
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
        if self._pending is not None:
            return True  # render in flight: mouse input waits
        # some X setups (libinput button-scroll, Exceed pointer emulation)
        # synthesize wheel events while a button is held down - that must
        # never zoom in the middle of a pan or a rubber-band drag
        if self._drag is not None or self._zoomdrag is not None:
            return True
        if ev.state & (Gdk.ModifierType.BUTTON1_MASK |
                       Gdk.ModifierType.BUTTON2_MASK |
                       Gdk.ModifierType.BUTTON3_MASK):
            return True
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
    def _command_key(self, ev):
        """Key name for shortcut matching. When a non-Latin layout owns
        the keyboard (e.g. the OS IME in hangul mode) the keyval is a
        jamo and "d"/"f"/... would go dead - re-translate the hardware
        keycode against the keymap's groups and take the first Latin
        result (flateyes' command_key). Special keys (Escape, arrows:
        no unicode) keep their name as is."""
        name = Gdk.keyval_name(ev.keyval) or ""
        uni = Gdk.keyval_to_unicode(ev.keyval)
        if not uni or uni < 0x80:
            return name  # ASCII or a special key: usable as is
        try:
            keymap = Gdk.Keymap.get_for_display(self.window.get_display())
            shift = ev.state & Gdk.ModifierType.SHIFT_MASK
            for group in range(4):
                res = keymap.translate_keyboard_state(
                    ev.hardware_keycode, shift, group)
                if res[0] and 0 < Gdk.keyval_to_unicode(res[1]) < 0x80:
                    return Gdk.keyval_name(res[1])
        except (AttributeError, TypeError):
            pass  # keymap API surprises: fall back to the raw name
        return name

    def _on_key(self, _w, ev):
        if self._gdlg is not None:
            # the goto dialog owns keyboard input, but some backends
            # (macOS quartz) still deliver its keys to the main window
            # too. The Entry guard below only checks the *main* window's
            # focus, so a coordinate typed into the dialog would walk the
            # depth shortcut (e.g. "5240" -> depth 5,2,4,0). Yield to it.
            return False
        focus = self.window.get_focus()
        if isinstance(focus, Gtk.Entry):
            return False  # typing in the depth spinbox etc.
        name = self._command_key(ev)
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
        else:
            return False
        return True

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
            spin = getattr(self._ddlg, "_spin", None)
            if spin is not None and \
                    int(spin.get_value()) != self.depth_value:
                spin.set_value(self.depth_value)
        self._on_depth()

    def _set_depth_auto(self):
        self.depth_auto = True
        self._on_depth()

    def _on_depth(self):
        self.dstatus.set_text(self._depth_label())
        self.redraw(immediate=True)

    def _only_close_button(self, dlg):
        """Leave only the window-close button in the title bar - no
        minimize/maximize. The GdkWindow functions drive the macOS
        traffic-light buttons; restrict them once the window realizes."""
        dlg.set_type_hint(Gdk.WindowTypeHint.DIALOG)

        def _restrict(_w):
            win = dlg.get_window()
            if win is not None:
                try:
                    win.set_functions(
                        Gdk.WMFunction.CLOSE | Gdk.WMFunction.MOVE)
                except Exception:
                    pass  # backend without WM-function support: harmless
        dlg.connect("realize", _restrict)

    def _center_on_parent(self, dlg):
        """Center the dialog on the main window. GTK's CENTER_ON_PARENT
        leans on the WM and miscomputes on some of them - X11 via XQuartz
        pins the dialog to the top (only horizontally centered) because it
        positions before the height is known. Instead move() explicitly
        from the parent geometry and the dialog's own size, at realize
        (before map, so the WM honors it as it did the GTK centering) and
        again on map as a correction."""
        dlg.set_position(Gtk.WindowPosition.NONE)

        def _place(*_a):
            par = self.window
            if not (par.get_realized() and dlg.get_realized()):
                return False
            pw, ph = par.get_size()
            px, py = par.get_position()
            req = dlg.get_preferred_size()[1]     # natural GtkRequisition
            dw = dlg.get_allocated_width()
            dh = dlg.get_allocated_height()
            if dw <= 1:                           # not allocated yet
                dw = req.width
            if dh <= 1:
                dh = req.height
            dlg.move(px + max(0, (pw - dw) // 2),
                     py + max(0, (ph - dh) // 2))
            return False
        dlg.connect("realize", _place)
        dlg.connect("map", _place)

    def _dialog_setup(self, dlg):
        """Shared chrome for the tool dialogs (depth, goto): transient and
        modal (macOS quartz denies a non-modal secondary window keyboard
        focus, so its keys leaked to the main window), centered on the
        parent, non-resizable, close button only."""
        dlg.set_transient_for(self.window)
        dlg.set_modal(True)
        self._center_on_parent(dlg)
        dlg.set_resizable(False)
        self._only_close_button(dlg)

    def _grab_focus_once(self, dlg, target):
        """Present `dlg` and grab keyboard focus on `target` exactly once
        it is mapped. quartz does not focus a freshly shown dialog - even
        a modal one - until then; one-shot so a later map does not re-grab
        (which would reselect an entry and trap focus there). `target` is a
        widget or a zero-arg callable returning one (deferred so a run()
        dialog's response buttons, built lazily, resolve after show)."""
        def _grab(*_a):
            if getattr(dlg, "_focused", False):
                return False
            dlg._focused = True
            dlg.present()
            w = target() if callable(target) else target
            if w is not None:
                w.grab_focus()
            return False
        dlg.connect("map-event", _grab)
        GLib.idle_add(_grab)

    def _dialog_show(self, dlg, focus):
        """Grab focus once mapped, then show. See _grab_focus_once."""
        self._grab_focus_once(dlg, focus)
        dlg.show_all()

    def _depth_dialog(self):
        if self._ddlg is not None:
            self._ddlg.present()
            return
        dlg = Gtk.Window(title="hierarchy depth")
        self._dialog_setup(dlg)
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
        spin.connect("activate", lambda *_: dlg.destroy())  # Enter = ok
        dlg._spin = spin
        row.pack_start(spin, False, False, 0)
        for preset in (0, 1, 2, 3, 999):
            b = Gtk.Button(label="full" if preset == 999 else str(preset))
            b.connect("clicked", lambda _w, p=preset: self._set_depth(p))
            row.pack_start(b, False, False, 0)
        b = Gtk.Button(label="auto")
        b.connect("clicked", lambda *_: self._set_depth_auto())
        row.pack_start(b, False, False, 0)
        note = Gtk.Label()
        note.set_markup(
            "<small>auto picks the deepest level whose estimated shape"
            "\ncount stays interactive (index density table); explicit"
            "\nvalues override it. cells beyond the limit are drawn as"
            "\noutline frames with names - keys: d = this dialog,"
            "\n0-9 = depth, a = auto</small>")
        note.set_xalign(0.0)
        box.pack_start(note, False, False, 0)
        # depth applies live (spin/presets); ok just closes
        ok = Gtk.Button(label="ok")
        ok.connect("clicked", lambda *_: dlg.destroy())
        box.pack_start(ok, False, False, 0)

        def on_dialog_key(_w, ev):
            # Esc closes even when the focus sits in the spinbox, whose
            # input method would swallow the key first (flateyes trick)
            if Gdk.keyval_name(ev.keyval) == "Escape":
                dlg.destroy()
                return True
            return False
        dlg.connect("key-press-event", on_dialog_key)

        def _gone(*_a):
            self._ddlg = None
            # hand the keyboard back: some backends (macOS quartz
            # notably) fail to refocus the parent when a transient
            # closes, leaving every key command dead until a click
            self.window.present()
        dlg.connect("destroy", _gone)
        self._dialog_show(dlg, spin)

    # ---- goto (Calibre-style jump to coordinates) ---------------------------
    GOTO_HINT = ("um coordinates. window = view width after the jump"
                 "\n(blank = keep zoom). a pasted \"x, y\" pair in one"
                 "\nfield works too. Esc clears the X marker.")

    def _goto_dialog(self):
        if self._gdlg is not None:
            self._gdlg.present()
            return
        dlg = Gtk.Window(title="goto position")
        self._dialog_setup(dlg)
        self._gdlg = dlg
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
        box.set_margin_top(10)
        box.set_margin_bottom(10)
        box.set_margin_start(14)
        box.set_margin_end(14)
        dlg.add(box)
        row = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=4)
        box.pack_start(row, False, False, 0)
        entries = []
        for label, chars, text in (
                ("x", 11, "%.3f" % (self.cx * self.dbu)),
                ("y", 11, "%.3f" % (self.cy * self.dbu)),
                ("window", 8, "")):
            row.pack_start(Gtk.Label(label=label), False, False, 0)
            e = Gtk.Entry()
            e.set_width_chars(chars)
            e.set_text(text)
            e.connect("activate", lambda *_: self._goto_apply())
            row.pack_start(e, False, False, 0)
            entries.append(e)
        dlg._entries = entries
        note = Gtk.Label()
        note.set_markup("<small>%s</small>" % self.GOTO_HINT)
        note.set_xalign(0.0)
        dlg._note = note
        box.pack_start(note, False, False, 0)
        brow = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=4)
        box.pack_start(brow, False, False, 0)
        ok = Gtk.Button(label="ok")
        ok.connect("clicked", lambda *_: self._goto_apply())
        brow.pack_start(ok, True, True, 0)
        close = Gtk.Button(label="close")
        close.connect("clicked", lambda *_: dlg.destroy())
        brow.pack_start(close, True, True, 0)

        def on_dialog_key(_w, ev):
            if Gdk.keyval_name(ev.keyval) == "Escape":
                dlg.destroy()
                return True
            return False
        dlg.connect("key-press-event", on_dialog_key)

        def _gone(*_a):
            self._gdlg = None
            self.window.present()
        dlg.connect("destroy", _gone)
        self._dialog_show(dlg, entries[0])

    def _goto_apply(self):
        """Jump to the entered position: values fill x, y, window in
        order, so a DRC-report "x, y" pair pasted into any one field
        spreads across both coordinates."""
        dlg = self._gdlg
        if dlg is None:
            return
        try:
            part = [[float(t) for t in
                     e.get_text().replace(",", " ").split()]
                    for e in dlg._entries]
        except ValueError:
            dlg._note.set_markup("<small>not a number - %s</small>"
                                 % self.GOTO_HINT)
            return
        if len(part[0]) >= 2:
            part[1] = []  # pair pasted into x: the y field is stale
        vals = part[0] + part[1] + part[2]
        if len(vals) < 2:
            dlg._note.set_markup("<small>need both x and y - %s</small>"
                                 % self.GOTO_HINT)
            return
        self.goto(vals[0], vals[1], vals[2] if len(vals) > 2 else None)
        dlg.destroy()  # ok / Enter applied successfully: close

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

    def _ruler_click(self, ev):
        self._update_cursor(ev)
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
        self._only_close_button(dlg)
        self._center_on_parent(dlg)
        dlg.set_do_overwrite_confirmation(True)
        dlg.set_current_name("floe_clip_%s_%s_%s_%sum.oas"
                             % (um[0], um[1], um[2], um[3]))
        out = dlg.get_filename() if dlg.run() == Gtk.ResponseType.OK \
            else None
        dlg.destroy()
        self.window.present()  # quartz: refocus parent after the dialog
        if not out:
            return
        self.worker.submit({"kind": "clip",
                            "bbox": tuple(int(round(v)) for v in bbox),
                            "layers": self._layers_arg(), "out": out})
        self._set_status(bbox, "clipping…")

    # ---- shutdown -------------------------------------------------------------
    def _confirm_quit(self):
        """Ask before quitting (q key). Modal, centered on the parent;
        default is No so a stray Enter does not exit."""
        if self._quitting:
            return
        dlg = Gtk.MessageDialog(
            transient_for=self.window, modal=True,
            message_type=Gtk.MessageType.QUESTION,
            buttons=Gtk.ButtonsType.YES_NO,
            text="Quit floe?")
        self._center_on_parent(dlg)
        self._only_close_button(dlg)
        dlg.set_default_response(Gtk.ResponseType.NO)
        self._grab_focus_once(
            dlg, lambda: dlg.get_widget_for_response(Gtk.ResponseType.NO))
        resp = dlg.run()
        dlg.destroy()
        # quartz fails to refocus the parent when a transient closes,
        # leaving key commands dead until a click (same as _gone)
        self.window.present()
        if resp == Gtk.ResponseType.YES:
            self._quit()

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


def run_viewer(cache, server_sock=None, goto=None):
    import_gtk()
    viewer = Viewer(cache, server_sock, goto=goto)
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
