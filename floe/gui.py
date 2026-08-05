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
from .service import RenderWorker, CUT_LEVEL_PX
from .viewport import live_caps

Gtk = Gdk = GdkPixbuf = GLib = Pango = None

APP = "floe"
POLL_MS = 25
DEBOUNCE_MS = 120
DEFAULT_CUT_LEVEL = 2
DEFAULT_LOD = False
DEFAULT_FRAMES = True
DEFAULT_LABELS = True

BLACK = 0x000000FF
BAND_IN = 0x8ECDF5FF       # forward drag: zoom in
BAND_OUT = 0xF5B62EFF      # backward drag: zoom out
RULER_CORE = 0xFFE97AFF
SNAP_VERTEX = 0x66FFCCFF
SNAP_EDGE = 0x66CCFFFF
SEL_CORE = 0xFFFFFFFF
GOTO_MARK = 0xFF66D9FF
DRC_MARK = 0xFF5252FF      # DRC violation outline

DRC_LIST_MAX = 2000        # tree rows per check (prev/next reaches all)

MIN_SPP = 0.01     # max zoom-in: 1 px = 0.01 dbu; keeps render bboxes
                   # from collapsing to zero width after int rounding
WHEEL_ZOOM_STEP = 0.96  # at most 4% per wheel event (was 10%)
KEY_PAN_FRACTION = 0.10  # arrows move one tenth of the current viewport

MINIMAP_PX = 180           # square palette area; die keeps its aspect ratio
MINIMAP_DOT_MIN = 6        # view box smaller than this becomes a dot
MINIMAP_BG = 0x141414FF
MINIMAP_EDGE = 0x666666FF
MINIMAP_VIEW = 0x8ECDF5FF

# Layer/datatype is operationally capped at 999.999. The number column is
# independent of the current design's actual maximum, so changing files or
# expanding a group never moves the palette columns. A small fractional
# margin is converted from the active font width by LayerRow.
LAYER_NUM_WIDTH = len("999.999")
LAYER_NUM_MARGIN_CHARS = 0.2
LAYER_COLOR_WIDTH = 5       # former 2-char swatch column x 2.5


def import_gtk():
    """flateyes-style lazy GTK import: exit 3 with a clear message when
    PyGObject is missing or the display is unreachable."""
    global Gtk, Gdk, GdkPixbuf, GLib, Pango
    try:
        import gi
        import warnings
        warnings.simplefilter("ignore", getattr(
            gi, "PyGIDeprecationWarning", DeprecationWarning))
        gi.require_version("Gtk", "3.0")
        gi.require_version("GdkPixbuf", "2.0")
        from gi.repository import Gtk as _Gtk, Gdk as _Gdk, \
            GdkPixbuf as _GdkPixbuf, GLib as _GLib, Pango as _Pango
    except (ImportError, ValueError) as exc:
        sys.stderr.write(
            "%s: PyGObject/GTK3 is required to open a window (%s)\n"
            "  verify with: python3 -c 'import gi; "
            "gi.require_version(\"Gtk\", \"3.0\")'\n" % (APP, exc))
        sys.exit(3)
    Gtk, Gdk, GdkPixbuf, GLib, Pango = \
        _Gtk, _Gdk, _GdkPixbuf, _GLib, _Pango
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


def fmt_count(n):
    """Human shape count: 950 / 12k / 3.4M / 1.2G."""
    if n >= 1e9:
        return "%.1fG" % (n / 1e9)
    if n >= 1e6:
        return "%.1fM" % (n / 1e6)
    if n >= 10e3:
        return "%.0fk" % (n / 1e3)
    return str(int(n))


def frame_rect(buf, x0, y0, w, h, color):
    """1-px rectangle border (rect_outline is too heavy for the minimap)."""
    fill_rect(buf, x0, y0, w, 1, color)
    fill_rect(buf, x0, y0 + h - 1, w, 1, color)
    fill_rect(buf, x0, y0, 1, h, color)
    fill_rect(buf, x0 + w - 1, y0, 1, h, color)


class LayerRow(object):
    """One layer row: [marker][layer.datatype][swatch][name].

    Double-clicking the row toggles
    visibility (hidden = strikethrough, place and color kept). The
    fixed-width marker and layer/datatype columns keep every color swatch
    aligned and the marker doubles as the expand control on group parents:
    '+' collapsed / '-' expanded (single click), ' ' for childless layers
    and children."""

    def __init__(self, l, marker, num_width, tooltip, on_toggle,
                 on_select, on_expand=None):
        self.key = (l["layer"], l["datatype"])
        # Calibre-style row: "127.1  <swatch>  NAME" - number first,
        # a color marker, then the name in plain white on black
        raw_num = "%d.%d" % (self.key[0], self.key[1])
        # width_chars is measured using the label's base font, before the
        # Pango "large" span below is applied. The one row that actually
        # consumes num_width (e.g. 63.63) would therefore grow beyond all
        # shorter rows. Render exactly num_width monospace glyph cells in
        # every row instead; Pango preserves the trailing spaces.
        self._num = raw_num.rjust(num_width)
        self._name = l["name"]
        self._color = l["color"]
        self._marker = marker
        self._on_toggle = on_toggle
        self._on_select = on_select
        self._on_expand = on_expand
        self._active = True
        self._picked = False
        self._selected = False
        self._mlbl = Gtk.Label()
        self._mlbl.set_xalign(0.0)
        self._mlbl.set_width_chars(2)
        mbox = Gtk.EventBox()
        mbox.add(self._mlbl)
        # Every marker accepts the row context menu. Group parents also use
        # its left click as the expand/collapse control.
        mbox.connect("button-press-event", self._on_marker_click)
        self._nlbl = Gtk.Label()
        self._nlbl.set_xalign(1.0)
        # rjust gives every markup string the same natural width;
        # width_chars is a second, GTK-allocation-level floor for backends
        # that trim or measure trailing markup spaces differently.
        self._nlbl.set_width_chars(num_width)
        probe_width, probe_height = self._nlbl.create_pango_layout(
            "0").get_pixel_size()
        small_gap = max(
            1, round(probe_width * 1.2 * LAYER_NUM_MARGIN_CHARS))
        self._nlbl.set_margin_end(small_gap)
        self._clbl = Gtk.Image()
        self._clbl.set_halign(Gtk.Align.CENTER)
        self._clbl.set_valign(Gtk.Align.CENTER)
        swatch_w = max(5, round(probe_width * 1.2 * LAYER_COLOR_WIDTH))
        swatch_h = max(3, round(probe_height * 1.2))
        self._clbl.set_from_pixbuf(self._speckle_swatch(swatch_w, swatch_h))
        self._clbl.set_size_request(swatch_w, swatch_h)
        self._lbl = Gtk.Label()
        self._lbl.set_xalign(0.0)
        self._lbl.set_margin_start(small_gap)
        # long layer names must not widen the panel and squeeze the
        # view: ellipsize and show the full name as a tooltip
        self._lbl.set_ellipsize(Pango.EllipsizeMode.END)
        self._lbl.set_max_width_chars(1)
        content = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL)
        content.pack_start(self._nlbl, False, False, 0)
        content.pack_start(self._clbl, False, False, 0)
        content.pack_start(self._lbl, True, True, 0)
        nbox = Gtk.EventBox()
        nbox.add(content)
        nbox.connect("button-press-event", self._on_name_click)
        row_box = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL)
        row_box.set_margin_bottom(max(
            1, round(probe_height * 1.2 * 0.1)))
        row_box.pack_start(mbox, False, False, 0)
        row_box.pack_start(nbox, True, True, 0)
        # Keep the inter-row padding inside an event window. A click in the
        # gap therefore belongs to this (upper) row instead of falling
        # through the layer palette without selecting anything.
        self.widget = Gtk.EventBox()
        self.widget.add(row_box)
        self.widget.connect("button-press-event", self._on_row_click)
        self.widget.set_tooltip_text(tooltip)
        self._paint()

    def _speckle_swatch(self, width, height):
        """Layer-palette preview of the renderer's 1px checker fill."""
        color = int(self._color.lstrip("#"), 16)
        rgb = ((color >> 16) & 255, (color >> 8) & 255, color & 255)
        rgb_bytes = bytes(rgb)
        pixels = bytearray(width * height * 3)
        for y in range(height):
            for x in range(width):
                border = x in (0, width - 1) or y in (0, height - 1)
                if not border and (x + y) & 1:
                    continue
                off = (y * width + x) * 3
                pixels[off:off + 3] = rgb_bytes
        # Keep the immutable backing bytes with the row. Pixbuf normally
        # retains its own GLib reference, and this also makes that lifetime
        # explicit across older GTK3/PyGObject bundles.
        self._swatch_bytes = GLib.Bytes.new(bytes(pixels))
        return GdkPixbuf.Pixbuf.new_from_bytes(
            self._swatch_bytes, GdkPixbuf.Colorspace.RGB, False, 8,
            width, height, width * 3)

    def _paint(self):
        fg = ("#fff2a8" if self._picked else
              "#d9f2ff" if self._selected else
              "#ffffff" if self._active else "#777777")
        self._mlbl.set_markup(
            '<span face="monospace" size="large" '
            'foreground="%s">%s</span>'
            % (fg, GLib.markup_escape_text(self._marker)))
        strike = "" if self._active else ' strikethrough="true"'
        self._nlbl.set_markup(
            '<span face="monospace" size="large" '
            'foreground="%s"%s>%s</span>'
            % (fg, strike, GLib.markup_escape_text(self._num)))
        self._lbl.set_markup(
            '<span size="large" foreground="%s"%s>%s</span>'
            % (fg, strike, GLib.markup_escape_text(self._name)))

    def set_marker(self, marker):
        self._marker = marker
        self._paint()

    def set_picked(self, on):
        on = bool(on)
        if on == self._picked:
            return
        self._picked = on
        context = self.widget.get_style_context()
        if on:
            context.add_class("floe-layer-picked")
        else:
            context.remove_class("floe-layer-picked")
        self._paint()

    def set_selected(self, on):
        on = bool(on)
        if on == self._selected:
            return
        self._selected = on
        context = self.widget.get_style_context()
        if on:
            context.add_class("floe-layer-selected")
        else:
            context.remove_class("floe-layer-selected")
        self._paint()

    def _on_marker_click(self, _w, event):
        if event.type == Gdk.EventType.BUTTON_PRESS and event.button == 3:
            self._on_select(self, event)
            return True
        if event.type != Gdk.EventType.BUTTON_PRESS or event.button != 1:
            return False
        if self._on_expand is None:
            self._on_select(self, event)
            return True
        self._on_expand(self)
        return True

    def _on_row_click(self, _w, event):
        if event.type != Gdk.EventType.BUTTON_PRESS or \
                event.button not in (1, 3):
            return False
        self._on_select(self, event)
        return True

    def _on_name_click(self, _w, event):
        if event.button == 3 and event.type == Gdk.EventType.BUTTON_PRESS:
            self._on_select(self, event)
            return True
        if event.button != 1:
            return False
        if event.type in (Gdk.EventType.BUTTON_PRESS,
                          Gdk.EventType._2BUTTON_PRESS):
            self._on_select(self, event)
            if event.type == Gdk.EventType._2BUTTON_PRESS:
                self.set_active(not self._active)
            return True
        return False

    def get_active(self):
        return self._active

    def set_active(self, on):
        on = bool(on)
        if on == self._active:
            return
        self._active = on
        self._paint()
        self._on_toggle(self, self.key)


class Viewer:
    def __init__(self, cache, server_sock=None, show=True, goto=None,
                 cut_level=None, dump=False, depth=None, lod=DEFAULT_LOD,
                 frames=DEFAULT_FRAMES, labels=DEFAULT_LABELS,
                 stream_kb=None, stream_target_ms=500,
                 render_debug=False):
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
        self._drag_origin = None
        self._drag_moved = False
        self._drag_btn = None       # button that started the pan
        self._zoomdrag = None       # rubber-band anchor (view px)
        self._band_cur = None
        self._band_ext = None       # (min x, max x) of the band drag
        self._debounce = None
        self._did_fit = False
        self.worker = None
        self._layer_rows = {}
        self._selected_layers = set()
        self._layer_select_anchor = None
        self._pick_expanded = None  # group auto-expanded for a pick
        self._layer_order = []
        self._layer_menu = None
        # start depth: a plain open shows hierarchy level 1 (fast first
        # paint on huge chips - the industry default), a --goto jump is
        # an inspection: full depth unless --depth says otherwise.
        # 999 = full; runtime digits/`d` dialog change it as before.
        # Open default is depth 0 (top geometry + outlines +
        # coverage): the fastest truthful first view on any chip.
        if depth is None:
            depth = 999 if goto is not None else 0
        self.depth_value = max(0, min(999, int(depth)))
        self.abstract = False       # `a` key: klayout abstract mode
        self.coverage_on = False    # `v` key: density coverage fill (VFS)
        # Explicit request controls; no shell environment is consulted.
        self.lod_on = bool(lod)
        self.frames_on = bool(frames)
        self.labels_on = bool(labels)
        self.stream_kb = stream_kb
        self.stream_target_ms = int(stream_target_ms)
        self.render_debug = bool(render_debug)
        self._depth_used = "?"      # depth of the last frame ("?" = none yet)
        self.max_depth = None        # learned from the VFS daemon
        # detail cut LEVEL: 0 = off, higher = more aggressive. Users
        # only ever see the level; the screen-px threshold behind each
        # level (CUT_LEVEL_PX) is an implementation detail that may be
        # retuned later without changing what "L1" means. --cut-level
        # sets the start value, the `c` dialog changes it at runtime.
        self.cut_level = (DEFAULT_CUT_LEVEL if cut_level is None else
                          max(0, min(len(CUT_LEVEL_PX) - 1,
                                     int(cut_level))))
        self.cut_px = CUT_LEVEL_PX[self.cut_level]
        # live-render span budget, scaled to the cache's tile size
        # (finer --tile-mb grids allow proportionally more tiles)
        self._live_cap = live_caps(cache.meta)[0]
        self.dump = bool(dump)      # --dump: save debug frame dumps
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
        self._cdlg = None
        self.goto_mark = None       # world point of the last goto (X marker)
        # DRC results browser ('e' key)
        self._drc = None            # drc.DrcDb
        self._drcwin = None
        self.drc_mark = None        # {"kind": 'p'|'e', "pts": [(dbu)]}
        self._drc_flat = []         # [(check idx, err idx)] for prev/next
        self._drc_ord = {}          # (ci, ei) -> flat position
        self._drc_pos = -1
        self._drc_paths = {}        # (ci, ei) -> tree path string
        self._labels = []           # Gtk.Label pool for ruler distances

        self.window = Gtk.Window(title=APP)
        self.window.set_default_size(1280, 860)
        self.window.connect("delete-event", lambda *_: self._quit())
        self.window.connect("key-press-event", self._on_key)

        hbox = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL)
        self.window.add(hbox)

        side = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=2)
        side.set_size_request(210, -1)
        hbox.pack_end(side, False, False, 0)
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

        trow = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=2)
        side.pack_start(trow, False, False, 4)
        for text, cb in (("expand all", self._expand_all),
                         ("collapse all", self._collapse_all)):
            b = Gtk.Button(label=text)
            b.connect("clicked", lambda _w, f=cb: f())
            trow.pack_start(b, True, True, 0)

        scroller = Gtk.ScrolledWindow()
        # Calibre-style layer panel: black background, white text
        css = Gtk.CssProvider()
        css.load_from_data(
            b".floe-layers, .floe-layers * "
            b"{ background-color: #000000; } "
            b".floe-layer-selected, .floe-layer-selected * "
            b"{ background-color: #31566d; } "
            b".floe-layer-picked, .floe-layer-picked * "
            b"{ background-color: #66582f; }")
        Gtk.StyleContext.add_provider_for_screen(
            Gdk.Screen.get_default(), css,
            Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION)
        scroller.get_style_context().add_class("floe-layers")
        scroller.set_policy(Gtk.PolicyType.NEVER, Gtk.PolicyType.AUTOMATIC)
        self._layers_scroller = scroller
        self._layers_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
        self._layers_box.set_margin_top(4)
        self._layers_box.get_style_context().add_class("floe-layers")
        scroller.add(self._layers_box)
        side.pack_start(scroller, True, True, 4)

        # Keep the overview in the palette instead of painting it over the
        # design pixels in the bottom-right corner of the viewport.
        self._minimap_image = Gtk.Image()
        self._minimap_image.set_size_request(MINIMAP_PX, MINIMAP_PX)
        self._minimap_image.set_halign(Gtk.Align.CENTER)
        self._minimap_image.set_valign(Gtk.Align.CENTER)
        side.pack_start(self._minimap_image, False, False, 4)

        brow = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=2)
        side.pack_start(brow, False, False, 4)
        for text, cb in (("fit", lambda: self.fit()),
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
        # ever larger. EXACTLY flateyes' proven containment - AUTOMATIC
        # policy, image directly in the scroller, events on the scroller.
        # The earlier EXTERNAL policy + EventBox variant laid out fine but
        # displayed BLACK on remote X11 (conda GTK bundle over SSH
        # forwarding) while flateyes on the same display worked; the
        # EventBox's own X window / EXTERNAL viewport stacking never
        # brought the drawn pixels to the screen there.
        self.scroller = Gtk.ScrolledWindow()
        self.scroller.set_policy(Gtk.PolicyType.AUTOMATIC,
                                 Gtk.PolicyType.AUTOMATIC)
        self.image = Gtk.Image()
        self.image.set_halign(Gtk.Align.START)
        self.image.set_valign(Gtk.Align.START)
        self.scroller.add(self.image)
        self.overlay.add(self.scroller)
        main.pack_start(self.overlay, True, True, 0)

        # Two-tier status area. Upper: cursor/interaction plus persistent
        # view/depth/cut/cov/lod state. Lower: retained render/performance
        # plus live rendering/refinement progress. Mouse motion only
        # replaces the upper-left text, never the lower render details.
        sbars = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=0)
        livebar = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL)
        infobar = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL)
        self.status = Gtk.Label(label="")
        self.status.set_xalign(0.0)
        self.status.set_margin_start(8)
        # long texts (selection info with long cell names) must not widen
        # the window: ellipsize into the allocated width, full text in the
        # tooltip (same treatment as the layer panel labels)
        self.status.set_ellipsize(Pango.EllipsizeMode.END)
        self.status.set_max_width_chars(1)
        self.rstatus = Gtk.Label(label="")
        self.rstatus.set_margin_end(10)
        self.pstatus = Gtk.Label(label="")
        self.pstatus.set_xalign(0.0)
        self.pstatus.set_margin_start(8)
        self.pstatus.set_ellipsize(Pango.EllipsizeMode.END)
        self.pstatus.set_max_width_chars(1)
        self.dstatus = Gtk.Label(label="depth: full")
        self.dstatus.set_margin_end(14)
        self.vstatus = Gtk.Label(label="")
        self.vstatus.set_margin_end(14)
        livebar.pack_start(self.status, True, True, 0)
        livebar.pack_end(self.dstatus, False, False, 0)
        livebar.pack_end(self.vstatus, False, False, 0)
        infobar.pack_start(self.pstatus, True, True, 0)
        infobar.pack_end(self.rstatus, False, False, 0)
        sbars.pack_start(livebar, False, False, 0)
        sbars.pack_start(infobar, False, False, 0)
        main.pack_start(sbars, False, False, 2)

        self.scroller.add_events(
            Gdk.EventMask.BUTTON_PRESS_MASK |
            Gdk.EventMask.BUTTON_RELEASE_MASK |
            Gdk.EventMask.POINTER_MOTION_MASK |
            Gdk.EventMask.SCROLL_MASK | Gdk.EventMask.SMOOTH_SCROLL_MASK)
        self.scroller.connect("button-press-event", self._on_press)
        self.scroller.connect("button-release-event", self._on_release)
        self.scroller.connect("motion-notify-event", self._on_motion)
        self.scroller.connect("scroll-event", self._on_scroll)
        # a modal dialog (or a broken pointer grab) swallows the button
        # release: without these, _drag survives forever and freezes
        # wheel zoom + render submission until the next canvas click
        self.scroller.connect("grab-broken-event",
                              lambda *_a: self._end_gesture())
        self.window.connect("focus-out-event",
                            lambda *_a: self._end_gesture() or False)
        self._alloc_size = None
        self.scroller.connect("size-allocate", self._on_allocate)
        self.scroller.connect("realize",
                              lambda w: self._set_cursor("crosshair"))

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
        # a loaded DRC db belongs to the previous layout
        self.drc_mark = None
        self._drc = None
        self._drc_flat = []
        self._drc_ord = {}
        self._drc_pos = -1
        self._drc_paths = {}
        if self._drcwin is not None:
            self._drcwin.destroy()
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
        self.worker = RenderWorker(
            cache, stream_kb=self.stream_kb,
            stream_target_ms=self.stream_target_ms,
            debug=self.render_debug)
        self.worker.start()
        if self._did_fit:
            self.fit()

    def _build_layer_panel(self):
        """Calibre-style panel: black background, rows of
        "<num>.<dt> <color-marker> NAME" in white, layer numbers
        ascending. Layers grouped by layer number:
        '+M1' = group parent (lowest datatype of a multi-datatype
        layer); its '+' expands/collapses the remaining datatypes
        below it (collapsed by default). Clicking selects rows,
        double-clicking toggles visibility, and a collapsed parent drags
        every child datatype with it. All names left-aligned via the
        marker column; hidden = strikethrough."""
        for child in self._layers_box.get_children():
            self._layers_box.remove(child)
        self._layer_rows = {}
        self._layer_groups = {}     # parent key -> [child keys]
        self._layer_expanded = set()  # parent keys currently expanded
        self._selected_layers = set()
        self._layer_select_anchor = None
        self._layer_order = []
        self._layers_batch = False
        groups = {}
        for l in self.meta["layers"]:
            groups.setdefault(l["layer"], []).append(l)
        num_width = LAYER_NUM_WIDTH

        def add_row(l, marker, tooltip, on_expand=None):
            row = LayerRow(l, marker, num_width, tooltip,
                           self._on_layer_toggled, self._on_layer_clicked,
                           on_expand)
            self._layers_box.pack_start(row.widget, False, False, 0)
            self._layer_rows[row.key] = row
            self._layer_order.append(row.key)
            return row.key

        # Calibre ordering: layer numbers ascend down the panel
        for lnum in sorted(groups):
            ls = groups[lnum]
            if len(ls) == 1:
                l = ls[0]
                add_row(l, " ", "%d.%d  %s\nclick: select; "
                        "double-click: show/hide; right-click: actions"
                        % (lnum, l["datatype"], l["name"]))
                continue
            ls = sorted(ls, key=lambda e: e["datatype"])
            head, rest = ls[0], ls[1:]
            pkey = add_row(
                head, "+",
                "%d.%d  %s\n+/-: expand/collapse %d more datatypes\n"
                "click: select; double-click: show/hide layer %d group; "
                "right-click: actions"
                % (lnum, head["datatype"], head["name"], len(rest), lnum),
                self._on_group_expand)
            self._layer_groups[pkey] = [
                add_row(l, " ", "%d.%d  %s\nclick: select; "
                        "double-click: show/hide; right-click: actions"
                        % (lnum, l["datatype"], l["name"]))
                for l in rest]
        self._layers_box.show_all()
        # children start collapsed; no_show_all keeps later show_all
        # calls (window level) from revealing them
        for ckeys in self._layer_groups.values():
            for k in ckeys:
                w = self._layer_rows[k].widget
                w.set_no_show_all(True)
                w.hide()

    def _on_group_expand(self, row):
        """'+'/'-' marker click on a group parent."""
        expand = row.key not in self._layer_expanded
        if expand:
            self._layer_expanded.add(row.key)
        else:
            self._layer_expanded.discard(row.key)
        row.set_marker("-" if expand else "+")
        for k in self._layer_groups[row.key]:
            w = self._layer_rows[k].widget
            if expand:
                w.show()
            else:
                w.hide()

    def _expand_all(self, expand=True):
        for pkey in self._layer_groups:
            if (pkey in self._layer_expanded) != expand:
                self._on_group_expand(self._layer_rows[pkey])

    def _collapse_all(self):
        self._expand_all(False)

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
                self._forwarded_view_options(fields[1:])
                self._forwarded_goto(fields[1:])
                self._present()
        except Exception:
            import traceback
            traceback.print_exc()
        finally:
            conn.close()
        return True

    def _forwarded_view_options(self, fields):
        """Apply request-scoped CLI controls to the live instance."""
        opts = {}
        for field in fields:
            if "=" in field:
                key, value = field.split("=", 1)
                opts[key] = value
        if opts.get("lod") in ("on", "off"):
            self._set_lod(opts["lod"] == "on")
        if opts.get("frames") in ("on", "off"):
            self._set_frames(opts["frames"] == "on")
        if opts.get("labels") in ("on", "off"):
            enabled = opts["labels"] == "on"
            if enabled != self.labels_on:
                self.labels_on = enabled
                self.redraw(immediate=True)

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
        if len(self.visible) != len(self._layer_rows):
            return self._visible_list()
        return None

    # ---- display composition (no cairo: pixbuf ops only) -------------------
    def _render_key(self, scope):
        """Identity of a frame: what state it was rendered for."""
        return (scope, tuple(sorted(self.visible)), self._depth_key(),
                self._effective_cut_px(), self.lod_on, self.frames_on,
                self.labels_on)

    def _effective_cut_px(self):
        """Screen-space detail cut is independent of merged LOD."""
        return self.cut_px

    def _structure_visible(self):
        """A finite VFS depth has a hierarchy frontier of its own."""
        return bool(self.meta.get("vfs") and self.frames_on
                    and self._depth() is not None)

    def _frame_compatible(self, frame):
        """A stale frame stays displayable rescaled until the fresh
        frame lands - across pans, zooms of any ratio, layer and depth
        changes alike (briefly stale content beats a black flash).
        A finite-depth hierarchy frame remains useful with every design
        layer hidden (and a degenerate frame is never displayable)."""
        return (frame[3] is not None and frame[2] > 0 and
                (bool(self.visible) or self._structure_visible()))

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
        self._draw_overlays(disp, obox, ospp)
        if self.dump:
            # diagnosis: exactly what is handed to the screen widget, plus
            # whether the widget itself is sized/mapped (a 0-sized or
            # unmapped Gtk.Image displays nothing without any error)
            disp.savev("/tmp/floe_disp.png", "png", [], [])
            ia = self.image.get_allocation()
            sys.stderr.write(
                "[dump] disp %dx%d | image alloc %dx%d at (%d,%d) "
                "mapped=%s visible=%s\n"
                % (disp.get_width(), disp.get_height(),
                   ia.width, ia.height, ia.x, ia.y,
                   self.image.get_mapped(), self.image.get_visible()))
        self.image.set_from_pixbuf(disp)
        self._update_minimap(bbox)
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

    def _draw_overlays(self, disp, obox, ospp):
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
        if self.drc_mark is not None:
            pts = [(sx(x), sy(y)) for x, y in self.drc_mark["pts"]]
            if self.drc_mark["kind"] == "p":
                for a, b in zip(pts, pts[1:] + pts[:1]):
                    stamp_segment(disp, a, b, BLACK, DRC_MARK)
            else:
                # edge records: consecutive point pairs are segments
                for j in range(0, len(pts) - 1, 2):
                    a, b = pts[j], pts[j + 1]
                    stamp_segment(disp, a, b, BLACK, DRC_MARK)
                    for px, py in (a, b):
                        rect_outline(disp, px - 3, py - 3, px + 3,
                                     py + 3, None, DRC_MARK)
        if self._zoomdrag is not None and self._band_cur is not None:
            x0, y0 = self._zoomdrag
            x1, y1 = self._band_cur
            color = BAND_IN if x1 >= x0 else BAND_OUT
            rect_outline(disp, x0, y0, x1, y1, BLACK, color)

    def _update_minimap(self, bbox):
        """Draw the die overview below the layer list."""
        disp = GdkPixbuf.Pixbuf.new(
            GdkPixbuf.Colorspace.RGB, False, 8, MINIMAP_PX, MINIMAP_PX)
        disp.fill(BLACK)
        bb = self.meta["bbox"]
        bw, bh = bb[2] - bb[0], bb[3] - bb[1]
        if bw <= 0 or bh <= 0:
            self._minimap_image.set_from_pixbuf(disp)
            return
        scale = MINIMAP_PX / max(bw, bh)
        mw, mh = max(2, round(bw * scale)), max(2, round(bh * scale))
        x0 = (MINIMAP_PX - mw) // 2
        y0 = (MINIMAP_PX - mh) // 2
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
        self._minimap_image.set_from_pixbuf(disp)

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
        # skeleton retired (rev 24): every view renders live - wide
        # views are carried by coverage + LOD variants
        scope = "live"
        self._display()
        if not self.visible and not self._structure_visible():
            if self._debounce is not None:
                GLib.source_remove(self._debounce)
                self._debounce = None
            self._clear_pending()
            self._set_status(bbox, "no layers visible")
            return
        mode = ("hierarchy depth %d" % self._depth()
                if not self.visible else "live (%d tiles)" % span)
        if self._drag is not None:
            # mid-pan: track visually with the frozen frame only; the
            # render fires once on button release (a brief motion pause
            # used to let the debounce submit mid-drag)
            if self._debounce is not None:
                GLib.source_remove(self._debounce)
                self._debounce = None
            self._set_status(bbox, mode)
            return
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
        # render exactly the viewport. The old 50%-per-side overdraw
        # margin was meant to serve small pans straight from the frame,
        # but in practice any pan re-renders anyway - the margin only
        # multiplied every frame's pixels (4x) and the tiles/content
        # drawn (user call, 2026-07-31)
        eb = bbox
        depth = self._depth()
        self.gen += 1
        self._job_keys[self.gen] = self._render_key(scope)
        self._job_depth[self.gen] = depth
        for g in [g for g in self._job_keys if g < self.gen - 8]:
            del self._job_keys[g]
            self._job_depth.pop(g, None)
        # bboxes stay FLOAT dbu end to end: at deep zoom one dbu spans
        # ~100 screen px (spp bottoms out at 0.01), so int-rounding the
        # request skewed the frame's effective scale by whole percents -
        # the anchor logic then treated every frame as a zoom mismatch
        # and pans stopped tracking (the "weird panning at 0.01um" bug)
        self.worker.submit({
            "kind": "render", "gen": self.gen, "scope": scope,
            "t_sub": time.time(),
            "bbox": tuple(float(v) for v in eb),
            "view": tuple(float(v) for v in bbox),
            "w": int(w), "h": int(h),
            "depth": depth,
            "cut_px": self._effective_cut_px(),
            "lod": self.lod_on,
            "frames": self.frames_on,
            "labels": self.labels_on,
            "abstract": self.abstract,
            "coverage": self.coverage_on,
            "visible": self._layers_arg()})
        self._pending = self.gen
        self._preview_gen = None   # stop a stale preview ticker
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

    def _preview_tick(self, gen):
        """Elapsed-time ticker while the fat parse behind a preview runs
        (without it the preview read as "done" and the eventual real
        frame surprised the user)."""
        if getattr(self, "_preview_gen", None) != gen:
            return False  # real frame landed or a newer render started
        self.rstatus.set_text(
            "preview - loading tiles… %.0fs"
            % (time.perf_counter() - self._preview_t0))
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
        # with "rendering…" forever. Guarded like everything else in this
        # callback - an exception here (seen live: a bundle running a new
        # gui.py against a stale service.py without alive()) would make
        # GLib drop the poll source and silently freeze the viewer.
        try:
            w = self.worker
            if w is not None and not w.alive() \
                    and not getattr(w, "_died_reported", False):
                w._died_reported = True
                self._clear_pending()
                self._set_live_status(
                    "error: render service died (exit %s) - see terminal; "
                    "restart the viewer" % w.exitcode())
        except Exception as exc:
            if not getattr(self, "_watchdog_warned", False):
                self._watchdog_warned = True
                sys.stderr.write(
                    "floe: watchdog check failed (%s) - mixed floe "
                    "versions in the bundle? overwrite the WHOLE floe/ "
                    "package, not single files\n" % exc)
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
                    self._set_live_status(
                        "error: %s result failed: %s (see terminal)"
                        % (res.get("kind"), exc))
        except queue.Empty:
            pass
        # push buffered X output to the server: with cairo's core-protocol
        # fallback (CAIRO_DEBUG=xrender-version=-1, the XQuartz black-image
        # workaround) drawn updates otherwise sit in Xlib's output buffer
        # until some input event flushes them - the viewer looked frozen at
        # "rendering…" until the mouse moved. A flush with an empty buffer
        # is free, so doing it every poll tick is harmless everywhere.
        try:
            Gdk.Display.get_default().flush()
        except Exception:
            pass
        return True

    def _handle_result(self, res):
        kind = res.get("kind")
        if kind == "frame":
            preview = bool(res.get("preview"))
            if res["gen"] == self._pending and not preview:
                # FIRST frame of this gen: content is on screen, so
                # UNBLOCK input immediately (mouse handlers gate on
                # _pending) - a streamed refinement keeps painting
                # behind it, and any interaction simply supersedes
                # the job (the service aborts between rounds and the
                # daemon rolls back the un-acked round, par.3.7)
                self._clear_pending()
                if not res.get("refining"):
                    self.rstatus.set_text("rendering done.")
            if res["gen"] == self.gen and not preview:
                if res.get("refining"):
                    self._refining = True
                    self.rstatus.set_text(
                        "refining %d pages..." % res["refining"])
                elif getattr(self, "_refining", False):
                    self._refining = False
                    self.rstatus.set_text("rendering done.")
            if res["gen"] == self.gen:
                loader = GdkPixbuf.PixbufLoader.new_with_type("png")
                loader.write(res["png"])
                loader.close()
                pix = loader.get_pixbuf()
                if self.dump:
                    # diagnosis: the frame as received from the service
                    pix.savev("/tmp/floe_frame.png", "png", [], [])
                fb = res["bbox"]
                fspp = (fb[2] - fb[0]) / max(1, pix.get_width())
                key = self._job_keys.get(res["gen"])
                used = self._job_depth.get(res["gen"])
                if isinstance(res.get("max_depth"), int):
                    self.max_depth = max(0, res["max_depth"])
                if preview:
                    # LOD preview while fat full tiles parse. The
                    # sentinel key displays (non-None) yet never matches
                    # a render key, so _covered() keeps re-rendering
                    # until the real frame (same gen) replaces this.
                    # Crucially UNBLOCK input: mouse handlers gate on
                    # _pending, and a fat parse can run for minutes - the
                    # user must be able to pan/zoom away (the service
                    # skips the stale fat load when newer work queues).
                    self.last_frame = (pix, fb, fspp, "preview")
                    self._display()
                    if res["gen"] == self._pending:
                        self._clear_pending()
                        self._preview_gen = res["gen"]
                        self._preview_t0 = time.perf_counter()
                        GLib.timeout_add(500, self._preview_tick,
                                         res["gen"])
                    self.rstatus.set_text("preview - loading tiles…")
                    return
                if res["gen"] == getattr(self, "_preview_gen", None):
                    # the fat parse behind an input-unblocking preview
                    # finished: close out its status line
                    self._preview_gen = None
                    self.rstatus.set_text("rendering done.")
                self.last_frame = (pix, fb, fspp, key)
                self._display()
                if res.get("bg"):
                    return  # silent margin upgrade
                self._depth_used = used
                self.dstatus.set_text(self._depth_label())
                if True:
                    split = ""
                    if res.get("load_ms") is not None:
                        split = " = %d load + %d draw" \
                            % (res["load_ms"], res["draw_ms"])
                        if res.get("wait_ms", 0) > 200:
                            split += " + %d wait" % res["wait_ms"]
                    cut = ""
                    if res.get("cut_um"):
                        cut = ", cut<%.3gum" % res["cut_um"]
                    drawn = ""
                    if res.get("drawn") is not None:
                        drawn = ", ~%s drawn" % fmt_count(res["drawn"])
                    refin = ""
                    if res.get("refining"):
                        refin = ", refining %d" % res["refining"]
                    lod = ""
                    if res.get("lod"):
                        lod = ", lod %d" % res["lod"]
                    text = ""
                    if res.get("plan_ms") is not None:
                        text += ", plan %.1fms/%s frames" % (
                            res["plan_ms"],
                            fmt_count(res.get("frame_rects", 0)))
                    if res.get("text_plan_ms") is not None:
                        text += ", text %.1fms/%s places" % (
                            res["text_plan_ms"],
                            fmt_count(res.get("text_place_records", 0)))
                    if res.get("labels_truncated"):
                        text += ", labels partial"
                    mode = "live (%d tiles, %d ms%s%s%s%s%s%s%s)" \
                        % (res["tiles"], res["ms"], split,
                           self._depth_note(used), cut, drawn,
                           refin, lod, text)
                # Also keep a terminal performance log (only the settled
                # frame prints; refining rounds would spam every ~0.4s).
                # The same line now remains in the persistent lower bar.
                if not res.get("refining"):
                    b = self.view_bbox()
                    print("%s  view %.1f x %.1f um"
                          % (mode, (b[2] - b[0]) * self.dbu,
                             (b[3] - b[1]) * self.dbu), flush=True)
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
            self._set_live_status(
                "clip saved: %s (%.2f MB, %d ms)"
                % (res["path"], res["size_mb"], res["ms"]))
        elif kind == "error":
            self._clear_pending()
            self._set_live_status("error: %s" % res.get("msg"))

    def _set_status(self, bbox, mode):
        w_um = (bbox[2] - bbox[0]) * self.dbu
        h_um = (bbox[3] - bbox[1]) * self.dbu
        # CD-zoom views are far below 0.1um: fixed %.1f showed them all
        # as "0.0 x 0.0"
        fmt = lambda v: ("%.4f" if v < 0.1 else
                         "%.3f" if v < 1 else "%.1f") % v
        self.vstatus.set_text("view %s x %s um" % (fmt(w_um), fmt(h_um)))
        self.pstatus.set_text(mode)
        self.pstatus.set_tooltip_text(mode)

    def _set_live_status(self, text):
        """Upper-row message; cursor motion may replace it."""
        self.status.set_text(text)
        self.status.set_tooltip_text(text)

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
        win = self.scroller.get_window()  # flateyes' set_viewport_cursor
        if win is not None:
            try:
                win.set_cursor(Gdk.Cursor.new_from_name(
                    win.get_display(), name))
            except Exception:
                pass

    def _end_gesture(self):
        """Abandon any in-flight pan/band gesture and restore the idle
        cursor. Needed when the button release can never reach us: a
        modal dialog opened mid-drag takes the GTK grab (its window
        receives the release), leaving _drag set forever - wheel zoom
        dead and every redraw debounce cancelled by the mid-pan branch.
        Wired to the toplevel's focus-out and the canvas grab-broken."""
        if self._drag is None and self._zoomdrag is None:
            return
        band = self._band_cur is not None
        self._drag = None
        self._drag_origin = None
        self._drag_moved = False
        self._drag_btn = None
        self._zoomdrag = None
        self._band_cur = None
        self._band_ext = None
        self._set_cursor("crosshair")
        if band:
            self._display()  # erase the rubber band

    def _drag_threshold(self, ox, oy, x, y):
        """Click-vs-drag via GTK's gtk-dnd-drag-threshold (8px default,
        setting-driven): the old hardcoded 3px misread jittery remote-X
        pointers (Exceed/XQuartz/VNC) as pans, silently eating picks."""
        return self.scroller.drag_check_threshold(
            int(ox), int(oy), int(x), int(y))

    def _on_press(self, _w, ev):
        if self._pending is not None:
            return True  # render in flight: mouse input waits
        if self._drag is not None or self._zoomdrag is not None:
            # one gesture at a time: a second button pressed mid-pan
            # must not clobber the drag state (spurious pick on
            # release) or arm an invisible rubber band (chord zoom)
            return True
        if ev.button == 1:
            if self.mode == "ruler":
                self._ruler_free = bool(ev.state &
                                        Gdk.ModifierType.SHIFT_MASK)
                # Defer the measurement click until release. The same
                # button is a pan gesture once it crosses the normal drag
                # threshold, so ruler mode does not lose navigation.
            self._drag = (ev.x, ev.y)
            self._drag_origin = self._drag
            self._drag_moved = False
            self._drag_btn = 1
            self._set_cursor("move")
        elif ev.button == 2:
            self._drag = (ev.x, ev.y)
            self._drag_origin = self._drag
            self._drag_moved = False
            self._drag_btn = 2
            self._set_cursor("move")
        elif ev.button == 3:
            self._zoomdrag = (ev.x, ev.y)
            self._band_cur = None
            self._band_ext = (ev.x, ev.x)
        return True

    def _on_release(self, _w, ev):
        if ev.button in (1, 2):
            if self._drag is not None and ev.button != self._drag_btn:
                return True  # not the button that started the pan
            was_drag = self._drag is not None
            panned = was_drag and self._drag_moved
            self._drag = None
            self._drag_origin = None
            self._drag_moved = False
            self._drag_btn = None
            self._set_cursor("crosshair")
            if panned:
                self.redraw()   # pan ended: render the final position
            elif was_drag and ev.button == 1:
                # A stationary left click keeps its mode-specific action;
                # movement is exclusively a pan gesture in both modes.
                if self.mode == "ruler":
                    self._ruler_click(ev)
                else:
                    self._pick_click(ev)
            return True
        if ev.button != 3 or self._zoomdrag is None:
            return True
        x0, y0 = self._zoomdrag
        bmin, bmax = self._band_ext or (x0, x0)
        self._zoomdrag = None
        self._band_cur = None
        self._band_ext = None
        # Direction from the DOMINANT excursion of the whole gesture,
        # not the release-point sign: a zoom-out drag that wobbles back
        # past the anchor used to fit the residual sliver box to the
        # viewport - a runaway ~100x zoom-in.
        forward = (bmax - x0) >= (x0 - bmin)
        dx = (ev.x - x0) if forward else (x0 - ev.x)
        dy = abs(ev.y - y0)
        if dx < 5 and dy < 5:
            # a plain right click stays inert; a visibly drawn band
            # that collapsed must not vanish without a word
            if max(bmax - x0, x0 - bmin) >= 5:
                self._set_live_status("zoom band cancelled")
            self._display()
            return True
        bbox = self.view_bbox()
        lx0 = bbox[0] + min(x0, ev.x) * self.spp
        lx1 = bbox[0] + max(x0, ev.x) * self.spp
        ly0 = bbox[3] - max(y0, ev.y) * self.spp
        ly1 = bbox[3] - min(y0, ev.y) * self.spp
        w, h = self._viewport_size()
        self.cx = (lx0 + lx1) / 2
        self.cy = (ly0 + ly1) / 2
        # Only the axes the user actually spanned take part in the
        # fit: a long, thin band (routine on wires) zooms by its long
        # axis instead of dying silently or exploding on the sliver.
        if forward:  # the box fills the viewport
            scale = [s for s, d in (((lx1 - lx0) / w, dx),
                                    ((ly1 - ly0) / h, dy)) if d >= 5]
            self.spp = max(scale)
        else:        # zoom out by the viewport/box ratio
            fac = [f for f, d in ((w / max(dx, 1.0), dx),
                                  (h / max(dy, 1.0), dy)) if d >= 5]
            self.spp *= max(fac)
        self.redraw()
        return True

    def _on_motion(self, _w, ev):
        self._update_cursor(ev)
        if self._pending is not None:
            # render in flight: the VIEW must not move, but gesture
            # classification has to continue - otherwise a drag done
            # while pending never sets _drag_moved and the release
            # fires a spurious pick; tracking the anchor also stops
            # the view from jumping once the render clears
            if self._drag is not None and ev.state & (
                    Gdk.ModifierType.BUTTON1_MASK |
                    Gdk.ModifierType.BUTTON2_MASK):
                if not self._drag_moved \
                        and self._drag_origin is not None:
                    ox, oy = self._drag_origin
                    if self._drag_threshold(ox, oy, ev.x, ev.y):
                        self._drag_moved = True
                self._drag = (ev.x, ev.y)
            elif self._zoomdrag is not None and \
                    ev.state & Gdk.ModifierType.BUTTON3_MASK:
                self._track_band(ev)
            return True
        if self._drag is not None and ev.state & (
                Gdk.ModifierType.BUTTON1_MASK |
                Gdk.ModifierType.BUTTON2_MASK):
            if not self._drag_moved and self._drag_origin is not None:
                ox, oy = self._drag_origin
                if not self._drag_threshold(ox, oy, ev.x, ev.y):
                    return True
                self._drag_moved = True
            ddx, ddy = ev.x - self._drag[0], ev.y - self._drag[1]
            self._drag = (ev.x, ev.y)
            self.cx -= ddx * self.spp
            self.cy += ddy * self.spp
            self.redraw()
            return True
        if self._zoomdrag is not None and \
                ev.state & Gdk.ModifierType.BUTTON3_MASK:
            self._track_band(ev)
            self._display()
            return True
        self._hover(ev)
        return True

    def _track_band(self, ev):
        self._band_cur = (ev.x, ev.y)
        bmin, bmax = self._band_ext or (ev.x, ev.x)
        self._band_ext = (min(bmin, ev.x), max(bmax, ev.x))

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
                # Trackpads can report a large accumulated delta in one
                # event. Never turn that into a multi-step zoom jump.
                delta = max(-1.0, min(1.0, -dy))
        if delta:
            self._zoom_at(ev.x, ev.y, WHEEL_ZOOM_STEP ** delta)
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

    def _pan_view(self, direction):
        """Move the viewport by a fixed fraction of its visible extent."""
        width, height = self._viewport_size()
        dx = width * self.spp * KEY_PAN_FRACTION
        dy = height * self.spp * KEY_PAN_FRACTION
        if direction == "Left":
            self.cx -= dx
        elif direction == "Right":
            self.cx += dx
        elif direction == "Up":
            self.cy += dy
        elif direction == "Down":
            self.cy -= dy
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
            self._set_frames(not self.frames_on)
        elif name in ("Left", "Right", "Up", "Down"):
            self._pan_view(name)
        elif name in ("KP_Left", "KP_Right", "KP_Up", "KP_Down"):
            self._pan_view(name[3:])
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
        elif name == "c":
            self._cut_dialog()
        elif name == "a":
            self._toggle_abstract()
        elif name == "v":
            self._toggle_coverage()
        elif name == "l":
            self._set_lod(not self.lod_on)
        elif name == "e":
            self._drc_window()
        elif name == "n":
            self._drc_step(1)
        elif name == "p":
            self._drc_step(-1)
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
        """Render-key component of the frame identity."""
        return self._depth()

    def _depth_note(self, used):
        """Status-line suffix naming the depth a frame rendered at."""
        return "" if used is None else ", depth %d" % used

    def _depth_label(self):
        d = self._depth()
        current = "full" if d is None else str(d)
        maximum = ("?" if self.max_depth is None
                   else str(self.max_depth))
        lbl = "depth: %s/%s" % (current, maximum)
        if self.meta.get("bands") or self.meta.get("vfs"):
            cut_on = self.cut_level > 0
            lbl += " · cut: %s" % (
                "L%d" % self.cut_level if cut_on else "off")
        if self.meta.get("vfs"):
            lbl += " · cov:%s" % (
                "on" if self.coverage_on else "off")
            lbl += " · lod:%s" % ("on" if self.lod_on else "off")
            lbl += " · frame:%s" % (
                "on" if self.frames_on else "off")
            lbl += " · text:%s" % (
                "on" if self.labels_on else "off")
        if self.abstract:
            lbl += " · abstract"
        return lbl

    def _set_depth(self, n):
        self.depth_value = max(0, min(999, int(n)))
        if self._ddlg is not None:
            spin = getattr(self._ddlg, "_spin", None)
            if spin is not None and \
                    int(spin.get_value()) != self.depth_value:
                spin.set_value(self.depth_value)
        self._on_depth()

    def _set_cut_level(self, n):
        self.cut_level = max(0, min(len(CUT_LEVEL_PX) - 1, int(n)))
        self.cut_px = CUT_LEVEL_PX[self.cut_level]
        self._on_depth()  # same refresh: status label + re-render

    def _toggle_abstract(self):
        # klayout abstract mode: sub-10px cells draw as empty frames -
        # a lossy navigation accelerator (wide views 25x, measured);
        # turn it off before reading fine content
        self.abstract = not self.abstract
        self._on_depth()

    def _toggle_coverage(self):
        # `v`: density-coverage fill at cut/wide views (VFS caches).
        # Off = the cut just drops small features (they vanish) with
        # no density stand-in - useful to see exactly what is real.
        self.coverage_on = not self.coverage_on
        self._on_depth()

    def _set_lod(self, enabled):
        enabled = bool(enabled)
        changed = enabled != self.lod_on
        self.lod_on = enabled
        if changed:
            self._on_depth()

    def _set_frames(self, enabled):
        enabled = bool(enabled)
        changed = enabled != self.frames_on
        self.frames_on = enabled
        if changed:
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
        note = Gtk.Label()
        note.set_markup(
            "<small>cells beyond the limit are drawn as outline frames"
            "\nwith names - keys: d = this dialog, 0-9 = depth</small>")
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

    def _cut_dialog(self):
        """Runtime control of the detail cut (screen px). The VFS
        daemon applies the cut in its page plan, showing cut-dropped
        subtrees as outline frames."""
        if self._cdlg is not None:
            self._cdlg.present()
            return
        dlg = Gtk.Window(title="detail cut")
        self._dialog_setup(dlg)
        self._cdlg = dlg
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
        box.set_margin_top(10)
        box.set_margin_bottom(10)
        box.set_margin_start(14)
        box.set_margin_end(14)
        dlg.add(box)
        box.pack_start(Gtk.Label(
            label="detail cut level (higher = lighter wide views)"),
            False, False, 0)
        row = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=4)
        box.pack_start(row, False, False, 0)
        btns = []
        for lvl in range(len(CUT_LEVEL_PX)):
            b = Gtk.ToggleButton(label="off" if lvl == 0 else "L%d" % lvl)
            b.set_active(lvl == self.cut_level)

            def _apply(w, n=lvl):
                if not w.get_active():   # ignore the untoggle event
                    return
                for i, other in enumerate(btns):
                    if i != n and other.get_active():
                        other.set_active(False)
                self._set_cut_level(n)
            b.connect("toggled", _apply)
            btns.append(b)
            row.pack_start(b, False, False, 0)
        note = Gtk.Label()
        note.set_markup(
            "<small>each level hides finer detail from live renders;"
            "\nareas below the cut draw as merged outlines instead"
            "\n(when the cache carries them - floe index --merge-only"
            "\nupgrades old caches). snap/pick/clip stay exact."
            "\nthe status line shows the physical cut (cut&lt;0.35um)."
            "\nkeys: c = this dialog</small>")
        note.set_xalign(0.0)
        box.pack_start(note, False, False, 0)
        ok = Gtk.Button(label="ok")
        ok.connect("clicked", lambda *_: dlg.destroy())
        box.pack_start(ok, False, False, 0)

        def on_dialog_key(_w, ev):
            if Gdk.keyval_name(ev.keyval) == "Escape":
                dlg.destroy()
                return True
            return False
        dlg.connect("key-press-event", on_dialog_key)

        def _gone(*_a):
            self._cdlg = None
            self.window.present()
        dlg.connect("destroy", _gone)
        self._dialog_show(dlg, btns[self.cut_level])

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

    # ---- DRC results browser -------------------------------------------------
    def _drc_window(self):
        """'e': non-modal DRC error browser (Calibre-RVE style)."""
        if self._drcwin is not None:
            self._drcwin.present()
            return
        win = Gtk.Window(title="DRC results")
        win.set_transient_for(self.window)
        win.set_default_size(430, 520)
        win.connect("destroy", self._on_drc_destroy)
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
        win.add(box)
        top = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=4)
        box.pack_start(top, False, False, 2)
        b = Gtk.Button(label="open .db…")
        b.connect("clicked", lambda *_: self._drc_open_dialog())
        top.pack_start(b, False, False, 2)
        info = Gtk.Label(label="no results database loaded")
        info.set_xalign(0.0)
        info.set_ellipsize(Pango.EllipsizeMode.START)
        top.pack_start(info, True, True, 2)
        win._info = info
        # columns: text, position, check index, error index
        # (error index -1 = check row, -2 = "... more" stub)
        store = Gtk.TreeStore(str, str, int, int)
        tree = Gtk.TreeView(model=store)
        for j, (t, expand) in enumerate((("rule / error", True),
                                         ("count / position", False))):
            col = Gtk.TreeViewColumn(t, Gtk.CellRendererText(), text=j)
            col.set_expand(expand)
            tree.append_column(col)
        tree.set_tooltip_column(0)
        tree.connect("row-activated", self._on_drc_row)
        sc = Gtk.ScrolledWindow()
        sc.set_policy(Gtk.PolicyType.AUTOMATIC, Gtk.PolicyType.AUTOMATIC)
        sc.add(tree)
        box.pack_start(sc, True, True, 0)
        nav = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=4)
        box.pack_start(nav, False, False, 2)
        for text, d in (("< prev (p)", -1), ("next (n) >", 1)):
            nb = Gtk.Button(label=text)
            nb.connect("clicked", lambda _w, dd=d: self._drc_step(dd))
            nav.pack_start(nb, True, True, 2)
        hint = Gtk.Label()
        hint.set_markup("<small>double-click an error to jump - "
                        "Esc clears the marker</small>")
        box.pack_start(hint, False, False, 0)
        win._store, win._tree = store, tree
        self._drcwin = win
        win.show_all()
        if self._drc is not None:
            self._drc_fill()

    def _on_drc_destroy(self, _w):
        self._drcwin = None

    def _drc_open_dialog(self):
        dlg = Gtk.FileChooserDialog(title="open DRC results (.db)",
                                    parent=self._drcwin or self.window,
                                    action=Gtk.FileChooserAction.OPEN)
        dlg.add_buttons("Cancel", Gtk.ResponseType.CANCEL,
                        "Open", Gtk.ResponseType.OK)
        dlg.set_current_folder(
            os.path.dirname(self.meta["src"]["path"]))
        for name, pats in (("DRC results (*.db, *.results)",
                            ("*.db", "*.results")),
                           ("all files", ("*",))):
            ff = Gtk.FileFilter()
            ff.set_name(name)
            for p in pats:
                ff.add_pattern(p)
            dlg.add_filter(ff)
        if dlg.run() == Gtk.ResponseType.OK:
            path = dlg.get_filename()
            dlg.destroy()
            self.load_drc(path)
        else:
            dlg.destroy()

    def load_drc(self, path):
        """Parse a Calibre ASCII DRC db and populate the browser."""
        from . import drc as drc_mod
        try:
            db = drc_mod.load_db(path)
        except Exception as exc:
            msg = "DRC load failed: %s" % exc
            if self._drcwin is not None:
                self._drcwin._info.set_text(msg)
            self._set_live_status(msg)
            return False
        self._drc = db
        self._drc_flat = [(ci, ei)
                          for ci, c in enumerate(db.checks)
                          for ei in range(len(c.errors))]
        self._drc_ord = {k: n for n, k in enumerate(self._drc_flat)}
        self._drc_pos = -1
        self.drc_mark = None
        if self._drcwin is not None:
            self._drc_fill()
        self._set_live_status(
            "DRC %s: %d checks, %d errors (n/p = step)"
            % (os.path.basename(path), len(db.checks), db.total))
        return True

    def _drc_fill(self):
        win, db = self._drcwin, self._drc
        store = win._store
        store.clear()
        self._drc_paths = {}
        for ci, c in enumerate(db.checks):
            head = c.name
            if c.desc:
                head += "\n" + c.desc.split("\n")[0]
            pit = store.append(None, [head, "%d" % len(c.errors),
                                      ci, -1])
            for ei, e in enumerate(c.errors[:DRC_LIST_MAX]):
                x, y = e.center()
                it = store.append(
                    pit, ["#%d  %s" % (e.num,
                                       "poly" if e.kind == "p"
                                       else "edge"),
                          "(%.3f, %.3f)" % (x, y), ci, ei])
                self._drc_paths[(ci, ei)] = str(store.get_path(it))
            if len(c.errors) > DRC_LIST_MAX:
                store.append(pit, ["… %d more (use prev/next)"
                                   % (len(c.errors) - DRC_LIST_MAX),
                                   "", ci, -2])
        win._info.set_text("%s — cell %s · %d checks · %d errors"
                           % (os.path.basename(db.path), db.cell,
                              len(db.checks), db.total))

    def _on_drc_row(self, tree, path, _col):
        store = tree.get_model()
        it = store.get_iter(path)
        ci, ei = store.get_value(it, 2), store.get_value(it, 3)
        if ei < 0:  # check row: toggle its children
            if tree.row_expanded(path):
                tree.collapse_row(path)
            else:
                tree.expand_row(path, False)
            return
        self._drc_jump(ci, ei)

    def _drc_jump(self, ci, ei):
        db = self._drc
        check = db.checks[ci]
        e = check.errors[ei]
        self._drc_pos = self._drc_ord.get((ci, ei), -1)
        b = e.bbox()
        w_um, h_um = b[2] - b[0], b[3] - b[1]
        cx, cy = e.center()
        self.goto(cx, cy, max(max(w_um, h_um) * 8.0, 2.0))
        self.goto_mark = None  # the violation outline is the marker
        self.drc_mark = {"kind": e.kind,
                         "pts": [(x / self.dbu, y / self.dbu)
                                 for x, y in e.pts]}
        self._set_live_status(
            "DRC %s #%d/%d · %s · %.3f x %.3f um at (%.3f, %.3f)"
            % (check.name, e.num, len(check.errors),
               "poly" if e.kind == "p" else "edge",
               w_um, h_um, cx, cy))
        self._display()

    def _drc_step(self, delta):
        """n/p keys and the prev/next buttons walk every error."""
        if not self._drc_flat:
            return
        if self._drc_pos < 0:
            pos = 0 if delta > 0 else len(self._drc_flat) - 1
        else:
            pos = (self._drc_pos + delta) % len(self._drc_flat)
        ci, ei = self._drc_flat[pos]
        self._drc_jump(ci, ei)
        win = self._drcwin
        ps = self._drc_paths.get((ci, ei))
        if win is not None and ps is not None:
            path = Gtk.TreePath.new_from_string(ps)
            win._tree.expand_to_path(path)
            win._tree.set_cursor(path, None, False)
            win._tree.scroll_to_cell(path, None, False, 0.0, 0.0)

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
        text = "   |   ".join(parts)
        self._set_live_status(text)

    def _toggle_ruler(self):
        if self.mode == "ruler":
            self.mode = "normal"
            self._ruler_start = None
            self._snap_res = None
            self._set_live_status("ruler off")
        else:
            self.mode = "ruler"
            self._set_live_status(
                "ruler: click two points (Shift=free angle, "
                "m=snap %s, Esc=done)"
                % ("on" if self.snap_on else "off"))
        self._display()

    def _toggle_snap(self):
        self.snap_on = not self.snap_on
        if not self.snap_on:
            self._snap_res = None
        self._set_live_status(
            "vector snap %s" % ("on" if self.snap_on else "off"))
        self._display()

    def _esc(self):
        """Step out flateyes-style: pending point -> selection -> ruler
        mode -> finished rulers -> goto marker."""
        if self._ruler_start is not None:
            self._ruler_start = None
        elif self.selection is not None:
            self._clear_selection()
        elif self.mode == "ruler":
            self.mode = "normal"
            self._snap_res = None
        elif self.rulers:
            self.rulers = []
        elif self.drc_mark is not None:
            self.drc_mark = None
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
            # too wide to pick - but a click must still honor the
            # documented deselect contract instead of leaving the
            # old selection (and its row highlight) stuck on screen
            if self.selection is not None:
                self._clear_selection()
                self._set_live_status("selection cleared")
                self._display()
            else:
                self._set_live_status("zoom in to pick objects")
            return
        self._pick_seq += 1
        self.worker.submit({
            "kind": "pick", "seq": self._pick_seq,
            "x": int(x), "y": int(y), "r": r, "nth": self._pick_nth,
            "layers": self._layers_arg()})

    def _on_pick_result(self, res):
        if not res.get("found"):
            self._clear_selection()
            self._set_live_status("no object here")
        else:
            self.selection = res
            self._set_picked_layer((res["layer"], res["datatype"]))
            bb = res["bbox"]
            w = (bb[2] - bb[0]) * self.dbu
            h = (bb[3] - bb[1]) * self.dbu
            self._sel_text = ("sel %s %d/%d · %s · %.3f x %.3f um @ "
                              "(%.3f, %.3f) · %d/%d"
                              % (res["lname"], res["layer"],
                                 res["datatype"], res["cell"], w, h,
                                 bb[0] * self.dbu, bb[1] * self.dbu,
                                 res["index"] + 1, res["count"]))
            self._set_live_status(self._sel_text)
        self._display()

    def _clear_selection(self):
        self.selection = None
        self._sel_text = ""
        # a pick may still be in flight: without this, its late
        # result resurrects the selection the user just dismissed
        # (Esc / row hide), re-expanding and re-scrolling the panel
        self._pick_seq += 1
        self._set_picked_layer(None)

    def _set_picked_layer(self, key):
        """Highlight and reveal the layer of the picked polygon."""
        for row_key, row in self._layer_rows.items():
            row.set_picked(row_key == key)
        # a group WE expanded for a previous pick collapses back once
        # it is no longer needed - auto-expansion must not silently
        # flip the parent row's double-click semantics forever
        keep = None
        if key is not None and key in self._layer_rows:
            for parent, children in self._layer_groups.items():
                if key in children:
                    keep = parent
                    break
        auto = getattr(self, "_pick_expanded", None)
        if auto is not None and auto != keep \
                and auto in self._layer_expanded \
                and auto in self._layer_rows:
            self._on_group_expand(self._layer_rows[auto])
        if auto != keep:
            self._pick_expanded = None
        if key is None or key not in self._layer_rows:
            return
        if keep is not None and keep not in self._layer_expanded:
            self._on_group_expand(self._layer_rows[keep])
            self._pick_expanded = keep
        # Showing a collapsed child is laid out on the next GTK cycle.
        GLib.idle_add(self._scroll_to_layer_row, key)

    def _scroll_to_layer_row(self, key, retried=False):
        row = self._layer_rows.get(key)
        if row is None or not row.widget.get_visible():
            return False
        alloc = row.widget.get_allocation()
        if alloc.height <= 1 and not retried:
            # just-shown row not laid out yet (frame-clock throttling
            # can defer allocation past a default-priority idle):
            # clamping to (-1, 0) would yank the panel to the top
            GLib.idle_add(self._scroll_to_layer_row, key, True,
                          priority=GLib.PRIORITY_LOW)
            return False
        if alloc.height <= 1:
            return False
        adj = self._layers_scroller.get_vadjustment()
        adj.clamp_page(alloc.y, alloc.y + alloc.height)
        return False

    # ---- layers / clip -------------------------------------------------------
    def _set_layer_selection(self, keys, anchor=None):
        """Replace the palette selection without changing visibility."""
        keys = set(keys).intersection(self._layer_rows)
        self._selected_layers = keys
        if anchor is not None:
            self._layer_select_anchor = anchor
        for key, row in self._layer_rows.items():
            row.set_selected(key in keys)

    def _selectable_layer_order(self):
        """Return palette order without children hidden by collapsed groups."""
        hidden = set()
        for parent, children in self._layer_groups.items():
            if parent not in self._layer_expanded:
                hidden.update(children)
        return [key for key in self._layer_order if key not in hidden]

    def _on_layer_clicked(self, row, event):
        """Apply desktop-list selection rules and open the row menu."""
        key = row.key
        if event.button == 3:
            # Context-menu clicks never alter the palette selection. This
            # keeps a prepared multi-selection intact even when the menu is
            # opened over some other row or the inter-row padding.
            self._popup_layer_menu(event)
            return

        state = event.state
        shift = bool(state & Gdk.ModifierType.SHIFT_MASK)
        control = bool(state & Gdk.ModifierType.CONTROL_MASK)
        selectable = self._selectable_layer_order()
        if shift and self._layer_select_anchor in selectable:
            first = selectable.index(self._layer_select_anchor)
            last = selectable.index(key)
            if first > last:
                first, last = last, first
            selected = set(selectable[first:last + 1])
            if control:
                selected.update(self._selected_layers)
            self._set_layer_selection(selected)
        elif control:
            # Desktop-list toggle: Ctrl-click adds an unselected row and
            # removes an already selected row without disturbing the rest.
            selected = set(self._selected_layers)
            if key in selected:
                selected.remove(key)
            else:
                selected.add(key)
            self._set_layer_selection(selected, anchor=key)
        else:
            self._set_layer_selection({key}, anchor=key)

    def _popup_layer_menu(self, event):
        menu = Gtk.Menu()

        def add_item(label, callback, sensitive=True):
            item = Gtk.MenuItem(label=label)
            item.set_sensitive(sensitive)
            item.connect("activate", lambda _item: callback())
            menu.append(item)

        has_selection = bool(self._selected_layers)
        add_item("show selected", lambda: self._set_selected_layers("show"),
                 has_selection)
        add_item("hide selected", lambda: self._set_selected_layers("hide"),
                 has_selection)
        add_item("toggle selected",
                 lambda: self._set_selected_layers("toggle"), has_selection)
        menu.append(Gtk.SeparatorMenuItem())
        add_item("show all", self._all_layers)
        add_item("hide all", self._no_layers)
        self._layer_menu = menu

        def released(_menu):
            if self._layer_menu is menu:
                self._layer_menu = None

        menu.connect("deactivate", released)
        menu.show_all()
        if hasattr(menu, "popup_at_pointer"):
            menu.popup_at_pointer(event)
        else:
            menu.popup(None, None, None, None, event.button, event.time)

    def _set_selected_layers(self, action):
        """Show, hide, or toggle the selected palette rows as one batch."""
        changes = {}
        covered = set()
        for key in self._layer_order:
            if key not in self._selected_layers or key in covered:
                continue
            row = self._layer_rows[key]
            on = (not row.get_active()) if action == "toggle" \
                else action == "show"
            affected = [key]
            kids = self._layer_groups.get(key)
            if kids and key not in self._layer_expanded:
                affected.extend(kids)
            for affected_key in affected:
                changes[affected_key] = on
                covered.add(affected_key)

        if not changes:
            return
        self._layers_batch = True
        try:
            for key, on in changes.items():
                self._layer_rows[key].set_active(on)
        finally:
            self._layers_batch = False
        self.redraw(immediate=True)

    def _on_layer_toggled(self, row, key):
        if row.get_active():
            self.visible.add(key)
        else:
            self.visible.discard(key)
            if self.selection is not None and \
                    (self.selection.get("layer"),
                     self.selection.get("datatype")) == key:
                self._clear_selection()
        if self._layers_batch:
            return  # group/all/none toggle: one redraw at the end
        kids = self._layer_groups.get(key)
        if kids and key not in self._layer_expanded:
            # A collapsed group acts as one row, so its parent drags every
            # datatype with it. Once expanded, every visible row (including
            # the parent datatype) toggles independently.
            on = row.get_active()
            self._layers_batch = True
            try:
                for k in kids:
                    # set_active fires _on_layer_toggled, which keeps
                    # self.visible in sync even while batched
                    self._layer_rows[k].set_active(on)
            finally:
                self._layers_batch = False
        self.redraw(immediate=True)

    def _set_all_layers(self, on):
        self._layers_batch = True
        try:
            for row in self._layer_rows.values():
                row.set_active(on)
        finally:
            self._layers_batch = False
        self.redraw(immediate=True)

    def _all_layers(self):
        self._set_all_layers(True)

    def _no_layers(self):
        self._set_all_layers(False)

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
        self._set_live_status("clipping…")

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


def run_viewer(cache, server_sock=None, goto=None, drc=None,
               cut_level=None, dump=False, depth=None, lod=DEFAULT_LOD,
               frames=DEFAULT_FRAMES, labels=DEFAULT_LABELS,
               stream_kb=None, stream_target_ms=500,
               render_debug=False):
    import_gtk()
    viewer = Viewer(cache, server_sock, goto=goto, cut_level=cut_level,
                    dump=dump, depth=depth, lod=lod, frames=frames,
                    labels=labels, stream_kb=stream_kb,
                    stream_target_ms=stream_target_ms,
                    render_debug=render_debug)
    if drc:
        if viewer.load_drc(os.path.abspath(drc)):
            viewer._drc_window()
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
