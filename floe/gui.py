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

import bisect
import math
import os
import queue
import sys
import time

from . import cache as cache_mod
from . import drc as drc_mod
from . import fillpat
from .service import (RenderWorker, DETAIL_PX, DETAIL_LEVELS,
                      DEFAULT_DETAIL)
from .viewport import live_caps

Gtk = Gdk = GdkPixbuf = GLib = Pango = None

APP = "floe"
POLL_MS = 25
DEBOUNCE_MS = 120
# LOD starts ON (rev 31): the skeleton is gone, so the first fit
# view is a live working set - merged variants must engage there
# without a keypress. The planner's fidelity/worth gates and the
# probe exactness rule keep it self-limiting; 'l' still toggles.
DEFAULT_LOD = True
DEFAULT_FRAMES = True
DEFAULT_LABELS = True

BLACK = 0x000000FF
BAND_IN = 0xFFFFFFFF       # forward drag: zoom in (plain white, user
BAND_OUT = 0xFFFFFFFF      # call 2026-08-09; direction no longer
                           # color-coded - backward drag zooms out)
RULER_CORE = 0xFFFFFFFF    # plain white solid ruler (same call)
SNAP_VERTEX = 0xFFFFFFFF   # plain white (user call 2026-08-09;
SNAP_EDGE = 0xFFFFFFFF     # vertex/edge no longer color-coded)
SEL_CORE = 0xFFFFFFFF
DRC_RED = 0xFF5252FF       # NOT-WAIVED errors (geometry+numbers)

DRC_PAGE = 1000            # error-grid cells per page
DRC_GRID_W = 5             # error numbers per browser grid row
DRC_VIEW_FRACTION = 0.3    # error extent ~30% of the view on a jump
DRC_HL_CAP = 1000          # highlight-in-view marker budget
DRC_SEL_CAP = 5000         # box-select budget ('e' mode)
DRC_GREEN = 0x00E676FF     # WAIVED errors (geometry+numbers;
                           # cyan->green, user call 2026-08-17)
DRC_MARK_PX = 5            # collapsed-marker square side; geometry
                           # whose screen span shrinks BELOW this
                           # paints as the marker (no gap where the
                           # shape draws smaller than the marker)
# canonical order of the rule-type combo (SVRF measurement metrics
# from the rules.json sidecar; "other" = no parsed measurement)
DRC_METRICS = ("width", "space", "enclosure", "area", "density",
               "length", "angle", "perimeter", "vertex", "other")

# FLOE_DRC_PROF=1: print a per-stage timing breakdown of the DRC
# browser paths to stderr - for machine-specific slowness reports
_DRC_PROF = bool(os.environ.get("FLOE_DRC_PROF"))


class _dprof(object):
    """with _dprof("tag"): ... -> [drcprof] tag N ms (>=1ms)."""
    __slots__ = ("tag", "t0")

    def __init__(self, tag):
        self.tag = tag

    def __enter__(self):
        self.t0 = time.perf_counter()
        return self

    def __exit__(self, *_exc):
        if _DRC_PROF:
            dt = (time.perf_counter() - self.t0) * 1e3
            if dt >= 1.0:
                sys.stderr.write("[drcprof] %-28s %8.1f ms\n"
                                 % (self.tag, dt))
                sys.stderr.flush()
        return False
DRC_GOLD = 0xFFD700FF      # box-selected errors (canvas + grid)


class _DrcPanel(object):
    """Widget refs of the embedded DRC browser (attribute bag)."""
    __slots__ = ("_info", "_rules", "_rstore", "_grid", "_gstore",
                 "_detail", "_hl", "_wf", "_selv", "_search",
                 "_tf", "_plabel", "_pprev", "_pnext")

MIN_SPP = 0.01     # max zoom-in: 1 px = 0.01 dbu; keeps render bboxes
                   # from collapsing to zero width after int rounding
FIT_ZOOM_OUT = 16.0  # max zoom-out: 16x beyond the fit view. The size
                     # cut is cut_px / px_per_um, so the ladder of the
                     # rev 45 thin-frame lattice (2 -> 1 -> gone) and
                     # Calibre wide-view comparisons need room past
                     # fit; the die stays centered out there.
WHEEL_ZOOM_STEP = 0.96  # at most 4% per wheel event (was 10%)
# Calibre-parity keys (user call 2026-08-10): arrows pan half the
# viewport, Ctrl+arrows the old fine tenth; Ctrl+Z halves the view
# span (zoom in 50%), Shift+Z doubles it back.
KEY_PAN_FRACTION = 0.50
KEY_PAN_FRACTION_FINE = 0.10
CAL_ZOOM_IN = 0.5        # spp factor for Ctrl+Z (Shift+Z = inverse)

# layer recolor palette: (hex, name) in grid order, straight from
# the packaged colornames.def (fillpat.COLOR_TABLE)
PALETTE_COLORS = tuple((h, n) for n, h in fillpat.COLOR_TABLE)

MINIMAP_PX = 180           # square palette area; die keeps its aspect ratio
MINIMAP_DOT_MIN = 6        # view box smaller than this becomes a dot
MINIMAP_BG = 0x141414FF
MINIMAP_EDGE = 0x666666FF
MINIMAP_VIEW = 0x8ECDF5FF
MINIMAP_FRONT = 0x46565FFF  # depth-frontier outlines: dim slate,
                            # well under the view box brightness

# Layer/datatype is operationally capped at 999.999. The number column is
# independent of the current design's actual maximum, so changing files or
# expanding a group never moves the palette columns. A small fractional
# margin is converted from the active font width by LayerRow.
LAYER_NUM_WIDTH = len("999.999")  # MINIMUM number column; the panel
                                  # widens it to the longest actual
                                  # pair + 1 so the swatch column can
                                  # never break alignment
LAYER_NUM_MARGIN_CHARS = 0.6      # number -> swatch breathing room
LAYER_NAME_MARGIN_CHARS = 1.2     # swatch -> name: over a full glyph
LAYER_STRIKE_RGB = (242, 242, 242)  # hidden-layer strike: bright,
                                    # readable over dimmed text/swatch
LAYER_SWATCH_WH = (31, 14)  # layer-row color/fill box, fixed px


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


def stamp_segment(buf, a, b, casing, core, px=2):
    """Line segment: flat rects for H/V, dabs for free angles.
    px=1 draws a hairline core (rulers/zoom band, user call
    2026-08-09); the casing geometry only serves the 2px default."""
    ax, ay = a
    bx, by = b
    off = 0 if px == 1 else 1
    if round(ay) == round(by):    # horizontal
        if casing is not None:
            fill_rect(buf, min(ax, bx), ay - 2, abs(bx - ax) + 1, 5, casing)
        fill_rect(buf, min(ax, bx), ay - off, abs(bx - ax) + 1, px, core)
    elif round(ax) == round(bx):  # vertical
        if casing is not None:
            fill_rect(buf, ax - 2, min(ay, by), 5, abs(by - ay) + 1, casing)
        fill_rect(buf, ax - off, min(ay, by), px, abs(by - ay) + 1, core)
    else:
        steps = min(int(max(abs(bx - ax), abs(by - ay))) + 1, 8000)
        pts = [(ax + (bx - ax) * i / steps, ay + (by - ay) * i / steps)
               for i in range(steps + 1)]
        if casing is not None:
            for x, y in pts:
                fill_rect(buf, x - 1, y - 1, 3, 3, casing)
        for x, y in pts:
            fill_rect(buf, x - off, y - off, px, px, core)


def stamp_dotted(buf, a, b, casing, core):
    """Dotted segment (label leaders): dabs every few px, visually
    distinct from the solid ruler lines."""
    ax, ay = a
    bx, by = b
    steps = min(int(max(abs(bx - ax), abs(by - ay))) + 1, 8000)
    for i in range(0, steps + 1, 5):
        x = ax + (bx - ax) * i / max(1, steps)
        y = ay + (by - ay) * i / max(1, steps)
        if casing is not None:
            fill_rect(buf, x - 1, y - 1, 3, 3, casing)
        fill_rect(buf, x - 1, y - 1, 2, 2, core)


class _DrcOffsetRuler(object):
    """A normal 4-coordinate ruler tagged for screen-space offset.

    Keeping the source segment in world coordinates preserves its exact
    measurement while the viewer can move only its painted dimension line.
    The object remains unpackable everywhere ordinary ruler tuples are used.
    """
    __slots__ = ("segment",)

    def __init__(self, segment):
        self.segment = tuple(segment)

    def __iter__(self):
        return iter(self.segment)


# ---- ruler-label placement (flateyes port) ---------------------------------

def rects_overlap(p, q):
    return (p[0] < q[2] and p[2] > q[0]
            and p[1] < q[3] and p[3] > q[1])


def seg_hits_rect(p, q, rect):
    """Does segment p-q pass through rect? (Liang-Barsky reject test.)"""
    x0, y0, x1, y1 = rect
    dx, dy = q[0] - p[0], q[1] - p[1]
    t0, t1 = 0.0, 1.0
    for num, den in ((p[0] - x0, -dx), (x1 - p[0], dx),
                     (p[1] - y0, -dy), (y1 - p[1], dy)):
        if den == 0:
            if num < 0:
                return False
        else:
            r = num / den
            if den < 0:
                t0 = max(t0, r)   # entering this boundary
            else:
                t1 = min(t1, r)   # leaving it
            if t0 > t1:
                return False
    return True


def leader_seg(a, b, rect):
    """Leader for a readout at rect: from the label edge to the
    closest point of segment a-b. The foot lands on that specific
    line (not a shared crossing), so crossing rulers stay
    identifiable. None when the segment already passes under the
    label - adjacency says it all."""
    lx0, ly0, lx1, ly1 = rect
    lcx, lcy = (lx0 + lx1) / 2.0, (ly0 + ly1) / 2.0
    dx, dy = b[0] - a[0], b[1] - a[1]
    denom = dx * dx + dy * dy
    t = 0.0 if denom == 0 else \
        ((lcx - a[0]) * dx + (lcy - a[1]) * dy) / denom
    t = max(0.0, min(1.0, t))
    px, py = a[0] + t * dx, a[1] + t * dy
    ex = min(max(px, lx0), lx1)   # stop at the label boundary
    ey = min(max(py, ly0), ly1)
    if abs(ex - px) < 1 and abs(ey - py) < 1:
        return None
    return ((ex, ey), (px, py))


def shove_label(x, y, w, h, placed, vw, vh):
    """Nudge a w*h label from its desired (x, y) so it clears every
    label already placed this pass: drop it just below the blocking
    label, wrap to a fresh column when one fills, and keep it inside
    the viewport. Best-effort and bounded: a viewport packed
    edge-to-edge with labels may keep a residual overlap rather than
    loop forever."""
    gap = 3
    max_x = max(2, vw - w - 2)
    max_y = max(2, vh - h - 2)
    x = min(max(2, x), max_x)
    y = min(max(2, y), max_y)
    for _ in range(len(placed) * 2 + 2):
        rect = (x, y, x + w, y + h)
        hit = next((r for r in placed if rects_overlap(rect, r)), None)
        if hit is None:
            break
        y = hit[3] + gap            # just below the blocking label
        if y > max_y:               # column full: start the next one
            x = min(x + w + gap, max_x)
            y = 2
    return x, y


def spread_label_spot(a, b, mid_x, mid_y, w, h):
    """Desired top-left for a ruler's readout when several rulers
    share the view: shift along the line (tangent) so crossing rulers
    don't pile their labels on the shared crossing, then lift off the
    line (normal) so each chip sits beside its own line."""
    dvx, dvy = b[0] - a[0], b[1] - a[1]
    length = math.hypot(dvx, dvy) or 1.0
    tx, ty = dvx / length, dvy / length              # unit tangent
    nx, ny = -ty, tx                                 # unit normal
    if ny > 0 or (abs(ny) < 1e-9 and nx < 0):        # aim it upward
        nx, ny = -nx, -ny
    sdir = 1.0 if tx >= 0 else -1.0                  # toward right end
    shift = min(0.30 * length, 64.0)
    fx = mid_x + sdir * tx * shift
    fy = mid_y + sdir * ty * shift
    lift = (abs(nx) * w + abs(ny) * h) / 2.0 + 12
    return fx + nx * lift - w / 2.0, fy + ny * lift - h / 2.0


def pick_label_spot(a, b, w, h, placed, leaders, others, vw, vh):
    """Place one ruler's readout so the whole arrangement stays
    legible: try anchors along the ruler's own line and both sides of
    it, and take the first spot whose chip covers no other chip or
    leader and whose own leader does not run under an earlier chip
    (chips are widgets above the overlay, so anything under one is
    lost). The strict first sweep also refuses to cover the other
    rulers' lines, which walks chips away from a shared crossing;
    rulers packed too closely for that retry without it, and the
    plain spread + shove remains the last resort."""
    dvx, dvy = b[0] - a[0], b[1] - a[1]
    length = math.hypot(dvx, dvy) or 1.0
    tx, ty = dvx / length, dvy / length
    # first side = the upward normal, matching the single-ruler habit
    first = -1.0 if tx > 0 else 1.0
    for strict in (True, False):
        for frac in (0.5, 0.34, 0.66, 0.2, 0.8):
            ax = a[0] + dvx * frac
            ay = a[1] + dvy * frac
            if not (0 <= ax <= vw and 0 <= ay <= vh):
                continue   # anchor scrolled out: label points nowhere
            for side in (first, -first):
                nx, ny = -ty * side, tx * side
                lift = (abs(nx) * w + abs(ny) * h) / 2.0 + 12
                x = ax + nx * lift - w / 2.0
                y = ay + ny * lift - h / 2.0
                x = min(max(2, x), max(2, vw - w - 2))
                y = min(max(2, y), max(2, vh - h - 2))
                rect = (x, y, x + w, y + h)
                if any(rects_overlap(rect, r) for r in placed):
                    continue
                if any(seg_hits_rect(s[0], s[1], rect)
                       for s in leaders):
                    continue   # chip would sit on an earlier leader
                if strict and any(seg_hits_rect(oa, ob, rect)
                                  for oa, ob in others):
                    continue   # chip would cover someone else's line
                seg = leader_seg(a, b, rect)
                if seg is not None and any(
                        seg_hits_rect(seg[0], seg[1], r)
                        for r in placed):
                    continue   # leader would vanish under a chip
                return x, y
    mx, my = (a[0] + b[0]) / 2, (a[1] + b[1]) / 2
    x, y = spread_label_spot(a, b, mx, my, w, h)
    return shove_label(x, y, w, h, placed, vw, vh)


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


def rect_outline(buf, x0, y0, x1, y1, casing, core, px=2):
    for a, b in (((x0, y0), (x1, y0)), ((x0, y1), (x1, y1)),
                 ((x0, y0), (x0, y1)), ((x1, y0), (x1, y1))):
        stamp_segment(buf, a, b, casing, core, px)


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


def _panel_debug_hook(scroller, box, rows_getter):
    """FLOE_PANEL_DEBUG=1: log palette geometry on every scroll tick
    and size change - allocation vs GdkWindow vs mapped state for the
    first/last rows in view. Field tool for the shrinking-range bug:
    the log shows WHICH layer of the stack (logical allocation, gdk
    window position, map state, adjustment clamp) diverges on the
    machine where it reproduces."""
    if not os.environ.get("FLOE_PANEL_DEBUG"):
        return

    def dump(tag, *_a):
        try:
            adj = scroller.get_vadjustment()
            sa = scroller.get_allocation()
            ba = box.get_allocation()
            vp = scroller.get_child()
            binpos = viewpos = None
            try:
                # BIN window: moves with the scroll, must be as tall
                # as the content and sit at y = -adj.value.
                # VIEW window: the stationary clip, must span the
                # whole scroller interior - a short/misplaced view
                # window clips rows to a moving band.
                bw = vp.get_bin_window() if hasattr(
                    vp, "get_bin_window") else None
                if bw is not None:
                    g = bw.get_geometry()
                    binpos = (g.x, g.y, g.width, g.height)
                vw = vp.get_view_window() if hasattr(
                    vp, "get_view_window") else None
                if vw is not None:
                    g = vw.get_geometry()
                    viewpos = (g.x, g.y, g.width, g.height)
            except Exception:
                pass
            lo = hi = None
            unmapped = 0
            for r in rows_getter():
                w = r.widget
                if not w.get_visible():
                    continue
                a = w.get_allocation()
                inview = (a.y + a.height > adj.get_value()
                          and a.y < adj.get_value()
                          + adj.get_page_size())
                if not inview:
                    continue
                if not w.get_mapped():
                    unmapped += 1
                gw = w.get_window()
                pos = gw.get_position() if gw else None
                ent = (a.y, a.height, w.get_mapped(), pos)
                if lo is None:
                    lo = ent
                hi = ent
            sys.stderr.write(
                "[panel] %s adj=%.0f/%.0f pg=%.0f scr=(y%d h%d) "
                "box_h=%d bin=%s view=%s first=%s last=%s "
                "unmapped_in_view=%d\n"
                % (tag, adj.get_value(), adj.get_upper(),
                   adj.get_page_size(), sa.y, sa.height,
                   ba.height, binpos, viewpos, lo, hi, unmapped))
            sys.stderr.flush()
        except Exception as exc:
            sys.stderr.write("[panel] dump failed: %s\n" % exc)

    scroller.get_vadjustment().connect(
        "value-changed", lambda *a: dump("scroll"))
    scroller.connect(
        "size-allocate", lambda *a: dump("alloc"))
    box.connect(
        "size-allocate", lambda *a: dump("box-alloc"))


def _remote_x_scroll_repaint(scroller):
    """Repaint the whole scrolled area on every scroll step.

    GTK scrolls a viewport by BLITTING the still-visible region
    server-side (copy-area) and repainting only the exposed strip.
    Remote X servers of this deployment (Exceed, XQuartz - the same
    family as the XRender black-image bug) botch that blit: each
    step leaves an unpainted band, so the visible strip appears to
    shrink from the top and bottom as you scroll. Invalidating the
    scroller per adjustment tick forces a full repaint of what is
    on screen - the panels hold at most a few dozen visible rows,
    so the cost is negligible on any display path."""
    for adj in (scroller.get_vadjustment(),
                scroller.get_hadjustment()):
        if adj is not None:
            adj.connect("value-changed",
                        lambda *_a: scroller.queue_draw())
    try:
        scroller.set_kinetic_scrolling(False)
    except AttributeError:
        pass


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
        # datatype 0 displays bare (Calibre style): 2.0 -> 2
        raw_num = ("%d" % self.key[0] if self.key[1] == 0
                   else "%d.%d" % (self.key[0], self.key[1]))
        # Render exactly num_width monospace glyph cells in every row so the
        # swatches stay aligned. Layer/datatype text itself is left-aligned.
        self._num = raw_num.ljust(num_width)
        name = l["name"] or ""
        # placeholder names arrive as "layer/type": show them in
        # the l.d form (same dt-0 omission as the number column)
        if "/" in name:
            a, _, b = name.partition("/")
            if a.isdigit() and b.isdigit():
                name = a if b == "0" else "%s.%s" % (a, b)
        self._name = name
        self._color = l["color"]
        self._marker = marker
        self._on_toggle = on_toggle
        self._on_select = on_select
        self._on_expand = on_expand
        self._active = True
        self._picked = False
        self._selected = False
        self._fill_rows = None      # None = default speckle checker
        self._mlbl = Gtk.Label()
        self._mlbl.set_xalign(0.0)
        mbox = Gtk.EventBox()
        mbox.add(self._mlbl)
        # Every marker accepts the row context menu. Group parents also use
        # its left click as the expand/collapse control.
        mbox.connect("button-press-event", self._on_marker_click)
        self._nlbl = Gtk.Label()
        self._nlbl.set_xalign(0.0)
        probe = self._nlbl.create_pango_layout("")
        probe.set_markup(
            '<span face="monospace" size="small">0</span>', -1)
        probe_width, probe_height = probe.get_pixel_size()
        self._mlbl.set_size_request(max(1, probe_width * 2), -1)
        self._nlbl.set_size_request(max(1, probe_width * num_width), -1)
        small_gap = max(1, round(probe_width * LAYER_NUM_MARGIN_CHARS))
        self._nlbl.set_margin_end(small_gap)
        self._clbl = Gtk.Image()
        self._clbl.set_halign(Gtk.Align.CENTER)
        self._clbl.set_valign(Gtk.Align.CENTER)
        swatch_w, swatch_h = LAYER_SWATCH_WH
        self._swatch_refs = []
        self._swatch_wh = (swatch_w, swatch_h)
        self._swatch_on = self._speckle_swatch(swatch_w, swatch_h)
        self._clbl.set_from_pixbuf(self._swatch_on)
        self._clbl.set_size_request(swatch_w, swatch_h)
        self._lbl = Gtk.Label()
        self._lbl.set_xalign(0.0)
        # the alias sits visibly APART from the swatch: at least a
        # full glyph of air, not the thin number-column gap
        self._lbl.set_margin_start(max(
            1, round(probe_width * LAYER_NAME_MARGIN_CHARS)))
        # Preserve the complete layer name. The palette scroller owns the
        # width constraint and exposes overflow through its bottom scrollbar.
        self._lbl.set_ellipsize(Pango.EllipsizeMode.NONE)
        content = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL)
        content.pack_start(self._nlbl, False, False, 0)
        content.pack_start(self._clbl, False, False, 0)
        content.pack_start(self._lbl, True, True, 0)
        nbox = Gtk.EventBox()
        nbox.add(content)
        nbox.connect("button-press-event", self._on_name_click)
        # input-only sub-boxes: the hidden-layer strike is ONE line
        # cairo-drawn across the whole row AFTER the children, and a
        # child with a visible window would composite over it (the
        # gaps were exactly where the old per-span strike vanished)
        mbox.set_visible_window(False)
        nbox.set_visible_window(False)
        row_box = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL)
        self._row_pad = max(1, round(probe_height * 0.1))
        row_box.set_margin_bottom(self._row_pad)
        row_box.pack_start(mbox, False, False, 0)
        row_box.pack_start(nbox, True, True, 0)
        # Keep the inter-row padding inside an event window. A click in the
        # gap therefore belongs to this (upper) row instead of falling
        # through the layer palette without selecting anything.
        self.widget = Gtk.EventBox()
        self.widget.add(row_box)
        self.widget.connect_after("draw", self._draw_strike)
        self.widget.connect("button-press-event", self._on_row_click)
        self.widget.set_tooltip_text(tooltip)
        self._paint()

    def _speckle_swatch(self, width, height):
        """Layer-palette preview: the layer's ASSIGNED fill pattern
        tiled 1:1 in the layer color (default = the renderer's 1px
        speckle checker), inside a solid border. Hidden rows keep
        the same swatch - the row-wide strike from _draw_strike is
        the only hidden marker."""
        color = int(self._color.lstrip("#"), 16)
        rgb = ((color >> 16) & 255, (color >> 8) & 255, color & 255)
        rgb_bytes = bytes(rgb)
        rows = (self._fill_rows.split("\n")
                if self._fill_rows else None)
        pixels = bytearray(width * height * 3)
        for y in range(height):
            for x in range(width):
                border = x in (0, width - 1) or y in (0, height - 1)
                if not border:
                    if rows is not None:
                        r = rows[y % 16]
                        if x % 16 >= len(r) or r[x % 16] != "*":
                            continue
                    elif (x + y) & 1:
                        continue
                off = (y * width + x) * 3
                pixels[off:off + 3] = rgb_bytes
        # (the strike line itself is drawn row-wide by _draw_strike)
        # Keep the immutable backing bytes with the row. Pixbuf normally
        # retains its own GLib reference, and this also makes that lifetime
        # explicit across older GTK3/PyGObject bundles.
        buf = GLib.Bytes.new(bytes(pixels))
        self._swatch_refs.append(buf)
        return GdkPixbuf.Pixbuf.new_from_bytes(
            buf, GdkPixbuf.Colorspace.RGB, False, 8,
            width, height, width * 3)

    def _paint(self):
        fg = ("#fff2a8" if self._picked else
              "#d9f2ff" if self._selected else
              "#ffffff")
        # hidden = ONE bright line cairo-drawn edge to edge by
        # _draw_strike (no per-span pango strike: it vanished in
        # the margins/swatch gaps and doubled over a drawn line).
        # Text and swatch keep their full colors - the strike alone
        # marks the hidden state (dimmed text was too dark to read,
        # user call 2026-08-10)
        # the name uses the SAME monospace face as the number column:
        # the default proportional font drew thinner strokes that
        # antialiased to gray next to the bright digits (user report
        # 2026-08-13)
        self._mlbl.set_markup(
            '<span face="monospace" size="small" '
            'foreground="%s">%s</span>'
            % (fg, GLib.markup_escape_text(self._marker)))
        self._nlbl.set_markup(
            '<span face="monospace" size="small" '
            'foreground="%s">%s</span>'
            % (fg, GLib.markup_escape_text(self._num)))
        self._lbl.set_markup(
            '<span face="monospace" size="small" '
            'foreground="%s">%s</span>'
            % (fg, GLib.markup_escape_text(self._name)))
        self._clbl.set_from_pixbuf(self._swatch_on)
        self.widget.queue_draw()

    def _draw_strike(self, widget, cr):
        """Hidden layer: one continuous bright line across the FULL
        row - text, swatch, margins and trailing space alike."""
        if self._active:
            return False
        alloc = widget.get_allocation()
        y = max(0, (alloc.height - self._row_pad) // 2)
        cr.set_source_rgb(*(c / 255.0 for c in LAYER_STRIKE_RGB))
        cr.rectangle(0, y, alloc.width, 1)
        cr.fill()
        return False

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

    def set_color(self, color):
        """Palette recolor: rebuild the swatch in place."""
        if color == self._color:
            return
        self._color = color
        self._swatch_on = self._speckle_swatch(*self._swatch_wh)
        self._paint()

    def set_fill(self, rows):
        """Fill assignment changed: retile the swatch (None =
        default speckle checker)."""
        if rows == self._fill_rows:
            return
        self._fill_rows = rows
        self._swatch_on = self._speckle_swatch(*self._swatch_wh)
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
                          Gdk.EventType._2BUTTON_PRESS,
                          Gdk.EventType._3BUTTON_PRESS):
            self._on_select(self, event)
            # GTK turns the 4th rapid click into a TRIPLE press, so
            # back-to-back double-clicks arrive as 2BP then 3BP -
            # only honoring 2BP silently swallowed every second
            # toggle (field report; it looked render-related but is
            # pure event classification)
            if event.type in (Gdk.EventType._2BUTTON_PRESS,
                              Gdk.EventType._3BUTTON_PRESS):
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
                 detail=None, dump=False, depth=None, lod=DEFAULT_LOD,
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
        self._frontier_depths = []  # baked minimap frontier (meta)
        self._minimap_bases = {}    # depth -> rendered base pixbuf
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
        # Frame is the normal UI control for both hierarchy outlines and
        # texts. An explicit --labels off remains available as a startup/
        # automation override, but frames off always suppresses texts.
        self.labels_on = self.frames_on and bool(labels)
        self.stream_kb = stream_kb
        self.stream_target_ms = int(stream_target_ms)
        self.render_debug = bool(render_debug)
        self._depth_used = "?"      # depth of the last frame ("?" = none yet)
        self.max_depth = None        # learned from the VFS daemon
        # DETAIL level: 0=low, 1=medium, 2=high. Users only ever see
        # the name; the screen-px threshold behind each (DETAIL_PX) is
        # an implementation detail that may be retuned without changing
        # what "medium" means. --detail sets the start value, the `d`
        # dialog changes it at runtime. Higher detail = smaller cut.
        self.detail = (DEFAULT_DETAIL if detail is None else
                       max(0, min(len(DETAIL_PX) - 1, int(detail))))
        self.cut_px = DETAIL_PX[self.detail]
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
        self._auto_rulers = []
        self._drc_ruler = []        # auto CD ruler of the current jump
        self.snap_on = True
        self._snap_seq = 0
        self._snap_res = None
        self._snap_sent = 0.0
        self.selection = None
        self._sel_text = ""
        self._pick_seq = 0
        self._pick_px = None
        self._pick_nth = 0
        self._pick_mode = "replace"
        self._cursor = (0, 0)
        self._pending = None
        self._pending_t0 = 0.0
        self._pending_timer = None
        self._color_epoch = 0       # bumped per palette recolor/refill
        # fill pattern palette: 20 slots (Calibre names), editable
        # 16x16 bitmaps; (l, d) -> slot assignments
        self._fill_patterns = fillpat.default_patterns()
        self._layer_patterns = {}
        self._layer_widths = {}     # (l, d) -> outline px (기본 1)
        self._fill_slots = []
        self._ddlg = None
        self._gdlg = None
        self._cdlg = None
        self._digit_last = None     # depth digit-pair state ('99' = full)
        self._digit_t = 0.0
        # DRC results browser ('e' key)
        self._drc = None            # drc.DrcDb or drc.IcePack
        self._drcwin = None
        self.drc_mark = None        # {"kind": 'p'|'e', "pts": [(dbu)]}
        self._drc_hits = []         # frame's painted markers:
                                    # (px, py, ci, ei) for hover/pick
        self._drc_tip = None        # last tooltip text set
        self.overlays_on = True     # Tab: rulers/marks/markers
        # prev/next walk by ARITHMETIC over cumulative counts, never
        # a materialized per-error list: an .ice sidecar can hold
        # hundreds of millions of violations
        self._drc_cum = []          # check idx -> first flat position
        self._drc_total = 0
        self._drc_pos = -1
        self._drc_open = None       # the one open (selected) rule
        self._drc_grid_ci = None    # rule the number grid shows
        self._drc_grid_rows = 0
        self._drc_cell = None       # marked grid cell (row, col)
        self._drc_gridw = DRC_GRID_W   # columns, reflowed to pane
        self._drc_cellw = 60        # px per number cell (probe)
        self._drc_page = 0          # error-grid page of the rule
        self._drc_rules_busy = False   # rebuilding the rules list
        self._drc_page_marks = []   # this grid page's (ci, ei,
                                    # kind, pts dbu) for the canvas
        self._drc_grid_base = None  # filtered ei base (None = all)
        self._drc_wfilter = "all"   # all | notwaived | waived
        self._drc_search = ""       # rule-name substring filter
        self._drc_rmeta = None      # <deck>.rules.json dict (the
                                    # `floe svrf` sidecar metadata)
        self._drc_rmatch = (0, 0)   # sidecar-matched / db rules
        self._drc_tfilter = "all"   # rule-type filter (svrf metric)
        self._drc_rtypes = None     # rule name -> metric frozenset
        self._drc_tf_busy = False   # rebuilding the type combo
        self._drc_shown = 0         # rules listed (info line)
        self._drc_lyr_saved = None  # visibility snapshot before a
                                    # double-click layer isolation
        self._drc_show_sel = False  # 'selected' list filter
        self._drc_sel = None        # OPEN rule's selection: (ci,
                                    # [ei], [(ei, kind, pts dbu)],
                                    # frozenset(ei))
        self._drc_sels = {}         # ci -> selection (kept across
                                    # rule switches, 2026-08-15)
        self._esel_start = None     # pending first box corner (dbu)
        self._drc_grid_map = []     # the grid PAGE's ei list
        self._drc_focus = None      # single-clicked error (ci, ei,
                                    # kind, pts dbu) - emphasized in
                                    # place, no zoom change
        self._drc_hl = False        # highlight-in-view toggle (v2)
        self._mono = False          # grayscale layers (b key)
        self._mono_saved = False    # mono state before highlight
        self._drc_hl_res = None     # (view key, [(kind, pts dbu)])
        self._labels = []           # Gtk.Label pool for ruler distances

        self.window = Gtk.Window(title=APP)
        self.window.set_default_size(1280, 860)
        self.window.connect("delete-event", lambda *_: self._quit())
        self.window.connect("key-press-event", self._on_key)

        # window > outer(V) > [paned(H): view | panel] + status rows.
        # The status/info rows sit OUTSIDE the paned, spanning the
        # full window width - the panel can no longer run over them
        # (field report) - and the paned handle between the view and
        # the panel drags the panel width.
        outer = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
        self.window.add(outer)
        # [left pane (future cell/object lists, minimap at the
        # bottom) | [canvas | layer panel]] - user call 2026-08-10
        lpaned = Gtk.Paned(orientation=Gtk.Orientation.HORIZONTAL)
        paned = Gtk.Paned(orientation=Gtk.Orientation.HORIZONTAL)
        for pn in (lpaned, paned):
            try:
                pn.set_wide_handle(True)
            except AttributeError:
                pass
        outer.pack_start(lpaned, True, True, 0)
        self._outer = outer
        left = Gtk.Box(orientation=Gtk.Orientation.VERTICAL,
                       spacing=2)
        left.set_size_request(MINIMAP_PX + 16, -1)
        # placeholder container: the Calibre-style cell/object
        # browser lands here later; the DRC browser lives here
        # permanently (user call 2026-08-13)
        self._left_stack = Gtk.Box(
            orientation=Gtk.Orientation.VERTICAL)
        self._left_stack.pack_start(self._build_drc_panel(),
                                    True, True, 0)
        left.pack_start(self._left_stack, True, True, 0)
        self._left_pane = left
        self._lpaned = lpaned
        # shrink=True: without it the paned handle floors at the
        # left content's NATURAL minimum (the DRC nav row alone is
        # ~450px) and the pane cannot be dragged small (field
        # report 2026-08-18) - shrinking simply clips the content
        lpaned.pack1(left, resize=False, shrink=True)
        lpaned.pack2(paned, resize=True, shrink=True)
        lpaned.set_position(MINIMAP_PX + 16)

        side = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=2)
        side.set_size_request(210, -1)
        # breathing room against the window edge: without it the
        # layer scrollbar and the palette's last column sit flush
        # on the app border (user call 2026-08-11)
        side.set_margin_end(6)
        paned.pack2(side, resize=False, shrink=False)
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
        # Calibre-style layer panel: black background, white text.
        # FIELD BUG (mac/retina): the universal `.floe-layers *`
        # background MUST NOT include the ScrolledWindow's own
        # subtree - on the quartz backend that background renders
        # with a broken clip band that overpaints the rows (a
        # shrinking strip that tracks the scroll; full invalidation
        # cannot repair it because every redraw repeats the same
        # mis-clip). Pixel-gated bisect: class on the scroller =
        # broken, class only on the rows box = clean. Hence three
        # scoped hooks: `.floe-layers` for the rows box subtree,
        # `.floe-layers-bg` DIRECTLY on the viewport (panel area
        # below the rows stays black), `.floe-layers-frame` on the
        # scroller solely for the scrollbar selectors.
        css = Gtk.CssProvider()
        css.load_from_data(
            # combo popups as a LIST, not a menu: GTK menu grabs
            # misfire under XQuartz/remote X (field report
            # 2026-08-18 - the popup closed on the slightest
            # pointer move during the click). List mode selects on
            # a plain row click and never times out.
            b"combobox { -GtkComboBox-appears-as-list: true; } "
            b".floe-layers, .floe-layers * "
            b"{ background-color: #000000; } "
            b".floe-layers-bg { background-color: #000000; } "
            b".floe-layer-selected, .floe-layer-selected * "
            b"{ background-color: #31566d; } "
            b".floe-layer-picked, .floe-layer-picked * "
            b"{ background-color: #66582f; } "
            b".floe-layers-frame scrollbar trough "
            b"{ background-color: #000000; background-image: none; } "
            b".floe-layers-frame scrollbar slider, "
            b".floe-layers-frame scrollbar slider:hover, "
            b".floe-layers-frame scrollbar slider:active "
            b"{ background-color: #ffffff; background-image: none; "
            b"border-color: #ffffff; opacity: 1; }")
        Gtk.StyleContext.add_provider_for_screen(
            Gdk.Screen.get_default(), css,
            Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION)
        scroller.get_style_context().add_class("floe-layers-frame")
        # GTK overlay/automatic scrollbars can reserve their gutter only while
        # scrolling (notably with the macOS GTK theme). That changes both the
        # width and height of the viewport and clips the edge rows. Keep both
        # gutters allocated for the lifetime of the palette.
        try:
            scroller.set_overlay_scrolling(False)
        except AttributeError:
            pass  # compatibility with older GTK3 builds
        try:
            scroller.set_propagate_natural_width(False)
        except AttributeError:
            pass
        scroller.set_policy(Gtk.PolicyType.ALWAYS,
                            Gtk.PolicyType.ALWAYS)
        _remote_x_scroll_repaint(scroller)
        self._layers_scroller = scroller
        self._layers_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
        self._layers_box.set_margin_top(4)
        self._layers_box.set_margin_bottom(4)
        # NATURAL height, never viewport-fill (field bug): a filled
        # box keeps the tall allocation it got while the window was
        # large; shrink the window into scrollable territory at the
        # wrong moment and the adjustment upper stays at that stale
        # height - a phantom scroll range of empty box. Scrolling
        # then walks the rows out of view with dead black above and
        # below (rows live only at the top of the oversized box).
        # START-aligned, the box is always exactly as tall as its
        # rows and the phantom range cannot exist.
        self._layers_box.set_valign(Gtk.Align.START)
        self._layers_box.get_style_context().add_class("floe-layers")
        scroller.add(self._layers_box)
        # implicit viewport: the black panel background, styled
        # DIRECTLY (never via a `*` selector - see the CSS note)
        scroller.get_child().get_style_context().add_class(
            "floe-layers-bg")
        side.pack_start(scroller, True, True, 4)

        # Range watchdog for the same field bug: the adjustment
        # upper legitimately equals max(content, viewport) - but a
        # missed viewport re-allocate during an interactive window
        # shrink leaves upper at the OLD viewport height while the
        # page has already shrunk. That phantom range scrolls the
        # rows out of a mostly-empty box: the panel shows a
        # shrinking band with dead black above and below. Re-clamp
        # whenever the geometry or the scroll position changes;
        # when GTK got it right this is a no-op.
        def _clamp_palette_range(*_a):
            adj = scroller.get_vadjustment()
            # GTK3 folds widget margins into preferred sizes
            nat = self._layers_box.get_preferred_height()[1]
            want = max(nat, adj.get_page_size())
            if adj.get_upper() > want + 2:
                adj.set_upper(want)
                adj.set_value(max(0.0, min(
                    adj.get_value(),
                    want - adj.get_page_size())))

        scroller.connect("size-allocate", _clamp_palette_range)
        scroller.get_vadjustment().connect(
            "value-changed", _clamp_palette_range)
        _panel_debug_hook(scroller, self._layers_box,
                          lambda: list(self._layer_rows.values()))

        # Keep the overview in the palette instead of painting it over the
        # design pixels in the bottom-right corner of the viewport.
        self._minimap_image = Gtk.Image()
        self._minimap_image.set_size_request(MINIMAP_PX, MINIMAP_PX)
        self._minimap_image.set_halign(Gtk.Align.CENTER)
        self._minimap_image.set_valign(Gtk.Align.CENTER)
        self._minimap_event = Gtk.EventBox()
        self._minimap_event.set_size_request(MINIMAP_PX, MINIMAP_PX)
        self._minimap_event.set_halign(Gtk.Align.CENTER)
        self._minimap_event.add(self._minimap_image)
        self._minimap_event.connect(
            "button-press-event", self._on_minimap_click)
        self._minimap_event.set_tooltip_text(
            "Click inside the die to center the viewport")
        self._left_pane.pack_end(self._minimap_event,
                                 False, False, 4)

        # layer color palette (user call 2026-08-10): click a layer
        # row to select it, then a swatch here to recolor. Personal
        # overrides persist under ~/.cache/floe; meta.json untouched.
        pal = Gtk.Grid()
        pal.set_row_spacing(2)
        pal.set_column_spacing(2)
        # homogeneous + hexpand: the 7 swatch columns share the
        # panel width, so resizing the pane resizes the swatches
        pal.set_column_homogeneous(True)
        pal.set_hexpand(True)
        for i, (col, cname) in enumerate(PALETTE_COLORS):
            rgb = tuple(int(col[j:j + 2], 16) / 255.0
                        for j in (1, 3, 5))

            def _draw_swatch(w, cr, rgb=rgb):
                a = w.get_allocation()
                cr.set_source_rgb(*rgb)
                cr.rectangle(0, 0, a.width, a.height)
                cr.fill()
                cr.set_source_rgb(0.4, 0.4, 0.4)  # outline: reads
                cr.set_line_width(1)              # on black too
                cr.rectangle(0.5, 0.5, a.width - 1, a.height - 1)
                cr.stroke()
                return False

            da = Gtk.DrawingArea()
            da.set_size_request(12, 14)
            da.set_hexpand(True)
            da.connect("draw", _draw_swatch)
            eb = Gtk.EventBox()
            eb.add(da)
            eb.set_tooltip_text(
                "%s (%s) - recolor the selected layer(s)"
                % (cname, col))
            eb.connect("button-press-event",
                       lambda _w, _e, c=col:
                       self._apply_palette_color(c))
            pal.attach(eb, i % 7, i // 7, 1, 1)
        side.pack_start(pal, False, False, 4)

        # fill pattern palette (user call 2026-08-11): 20 Calibre
        # fills, 5x4. Left click assigns to the selected layer(s);
        # right click edits the bitmap (Solid/Clear are fixed).
        patg = Gtk.Grid()
        patg.set_row_spacing(2)
        patg.set_column_spacing(2)
        patg.set_column_homogeneous(True)
        patg.set_hexpand(True)
        for i, fname in enumerate(fillpat.FILL_NAMES):

            def _draw_slot(w, cr, i=i):
                a = w.get_allocation()
                # white paper, black dots (user call 2026-08-11)
                cr.set_source_rgb(1.0, 1.0, 1.0)
                cr.rectangle(0, 0, a.width, a.height)
                cr.fill()
                rows = self._fill_patterns[i].split("\n")
                # tile the 16x16 bitmap 1:1 across the box (no
                # stretching - user call 2026-08-11)
                cr.set_source_rgb(0.0, 0.0, 0.0)
                for y in range(a.height):
                    r = rows[y % 16]
                    for x in range(a.width):
                        if r[x % 16] == "*":
                            cr.rectangle(x, y, 1, 1)
                cr.fill()
                cr.set_source_rgb(0.4, 0.4, 0.4)
                cr.set_line_width(1)
                cr.rectangle(0.5, 0.5, a.width - 1, a.height - 1)
                cr.stroke()
                return False

            da = Gtk.DrawingArea()
            da.set_size_request(12, 20)
            da.set_hexpand(True)
            da.connect("draw", _draw_slot)
            self._fill_slots.append(da)
            eb = Gtk.EventBox()
            eb.add(da)
            eb.set_tooltip_text(
                "%s - click: fill selected layer(s)" % fname)
            eb.connect("button-press-event",
                       lambda _w, ev, i=i:
                       self._on_fill_slot_click(i, ev))
            patg.attach(eb, i % 5, i // 5, 1, 1)
        side.pack_start(patg, False, False, 4)

        brow = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=2)
        side.pack_start(brow, False, False, 4)
        for text, cb in (("fit", lambda: self.fit()),
                         ("clip…", self._clip_dialog)):
            b = Gtk.Button(label=text)
            b.connect("clicked", lambda _w, f=cb: f())
            brow.pack_start(b, True, True, 0)

        main = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
        paned.pack1(main, resize=True, shrink=True)
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
        # full-width status strip UNDER both the view and the panel
        outer.pack_start(sbars, False, False, 2)

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
                              lambda w: self._set_cursor(self._idle_cursor()))

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
        # baked minimap frontier (per-depth structural boxes, v0.11.2
        # caches; absent key = plain minimap) + per-depth base cache
        self._frontier_depths = (self.meta.get("frontier")
                                 or {}).get("depths") or []
        self._minimap_bases = {}
        # fill palette state: user-global edited bitmaps + the
        # effective layerprops (personal, else the design default
        # next to the source - already seeded by Cache.load)
        self._fill_patterns = fillpat.default_patterns()
        self._layer_patterns = {}
        self._layer_widths = {}
        rows, _ = cache_mod.load_layer_props(self.cache.src)
        for key, _color, fill, _name, _f1, f2 in rows:
            i = fillpat.fill_index(fill)
            if i is not None:
                self._layer_patterns[tuple(key)] = i
            try:
                w = int(f2)
                if w > 1:
                    self._layer_widths[tuple(key)] = w
            except ValueError:
                pass
        for w in self._fill_slots:
            w.queue_draw()
        self._apply_props_visibility(rows)
        self._refresh_row_fills()
        self.last_frame = None
        self._frame_anchor = None
        self._depth_used = "?"
        self._job_keys.clear()
        self._clear_pending()
        self.rulers = []
        self._ruler_start = None
        self._auto_rulers = []
        self._drc_ruler = []
        self._snap_res = None
        self.selection = None
        self._sel_text = ""
        self._pick_px = None
        # a loaded DRC db belongs to the previous layout
        self.drc_mark = None
        self._drc = None
        self._drc_rmeta = None
        self._drc_rmatch = (0, 0)
        self._drc_tfilter = "all"
        self._drc_rtypes = None
        self._drc_lyr_saved = None   # new design = new layer table
        self._drc_cum = []
        self._drc_total = 0
        self._drc_pos = -1
        self._drc_open = None
        self._drc_grid_ci = None
        self._drc_grid_rows = 0
        self._drc_cell = None
        self._drc_grid_map = []
        self._drc_grid_base = None
        self._drc_page = 0
        self._drc_page_marks = []
        self._drc_show_sel = False
        self._drc_sel = None
        self._drc_sels = {}
        self._esel_start = None
        self._drc_focus = None
        self._drc_hl = False
        self._drc_hl_res = None
        self._mono = False
        self._mono_saved = False
        w = self._drcwin
        if w is not None:
            w._hl.set_active(False)
            w._rstore.clear()
            w._gstore.clear()
            w._detail.set_text("")
            w._info.set_text("no results database loaded")
            self._drc_types_rebuild()   # empty+disable type combo
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
        # the number column fits the LONGEST actual pair plus one
        # glyph of margin - a fixed width broke the swatch column
        # whenever a real pair outgrew it (field report: 63.63 era)
        num_width = max(
            [LAYER_NUM_WIDTH] +
            [len("%d.%d" % (l["layer"], l["datatype"])) + 1
             for l in self.meta["layers"]])

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
        # groups start EXPANDED (field request); no_show_all is
        # still set so a later window-level show_all cannot reveal
        # children the user collapses
        for pkey, ckeys in self._layer_groups.items():
            self._layer_expanded.add(pkey)
            self._layer_rows[pkey].set_marker("-")
            for k in ckeys:
                self._layer_rows[k].widget.set_no_show_all(True)
        self._refresh_row_fills()

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
        frames = opts.get("frames")
        labels = opts.get("labels")
        if frames in ("on", "off"):
            label_override = None if labels not in ("on", "off") \
                else labels == "on"
            self._set_frames(frames == "on", labels=label_override)
        elif labels in ("on", "off"):
            enabled = self.frames_on and labels == "on"
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
                self.labels_on, self._color_epoch)

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
        # labels place BEFORE the pixbuf is handed over: their dotted
        # leaders are stamped into this frame
        self._update_labels(obox, ospp, disp)
        self.image.set_from_pixbuf(disp)
        self._update_minimap(bbox)

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

        if not self.overlays_on:
            # Tab (flateyes parity): overlays hidden - rulers,
            # marks, markers, selections all skip; only the live
            # zoom band stays interactive. Stale marker hits must
            # not pick invisible markers.
            self._drc_hits = []
            if self._zoomdrag is not None \
                    and self._band_cur is not None:
                x0, y0 = self._zoomdrag
                x1, y1 = self._band_cur
                color = BAND_IN if x1 >= x0 else BAND_OUT
                rect_outline(disp, x0, y0, x1, y1, None, color,
                             px=1)
            return

        for sel in self.selections:
            if not sel.get("points"):
                continue
            pts = [(sx(x), sy(y)) for x, y in sel["points"]]
            for a, b in zip(pts, pts[1:] + pts[:1]):
                stamp_segment(disp, a, b, BLACK, SEL_CORE)
        segs = list(self.rulers)
        if self.mode == "ruler" and self._ruler_start is not None:
            segs.append((*self._ruler_start, *self._ruler_end_preview()))
        for seg in segs:
            x0, y0, x1, y1 = seg
            a, b = (sx(x0), sy(y0)), (sx(x1), sy(y1))
            if isinstance(seg, _DrcOffsetRuler):
                edge_a, edge_b = a, b
                a, b = drc_mod.offset_screen_segment(a, b)
                # Extension lines expose the error edge underneath while
                # tying both endpoints to its parallel dimension line.
                stamp_dotted(disp, edge_a, a, None, RULER_CORE)
                stamp_dotted(disp, edge_b, b, None, RULER_CORE)
            stamp_segment(disp, a, b, None, RULER_CORE, px=1)
            ang = math.atan2(b[1] - a[1], b[0] - a[0])
            stamp_arrow(disp, b, ang, None, RULER_CORE)       # outward
            stamp_arrow(disp, a, ang + math.pi, None, RULER_CORE)
        if self.mode == "ruler" and self.snap_on and self._snap_res \
                and self._snap_res.get("found"):
            mx, my = sx(self._snap_res["x"]), sy(self._snap_res["y"])
            color = SNAP_VERTEX if self._snap_res["snap"] == "vertex" \
                else SNAP_EDGE
            rect_outline(disp, mx - 5, my - 5, mx + 5, my + 5, None, color)
            fill_rect(disp, mx - 9, my, 19, 1, color)
            fill_rect(disp, mx, my - 9, 1, 19, color)
        # screen-space marker hit list rebuilt every frame by
        # _drc_stamp_errs (hover tooltip + canvas pick)
        self._drc_hits = []
        if self.drc_mark is not None:
            # solid 2px lines, speckled polygon interiors (user call
            # 2026-08-13: edges = plain 2px, polygons = 50% fill);
            # spans below the marker size collapse to the marker
            # square (user call 2026-08-16: no in-between zoom range
            # where the shape paints smaller than the marker)
            pts = [(sx(x), sy(y)) for x, y in self.drc_mark["pts"]]
            mcol = self.drc_mark.get("color", DRC_RED)
            mxs = [p[0] for p in pts]
            mys = [p[1] for p in pts]
            if (max(mxs) - min(mxs) < DRC_MARK_PX
                    and max(mys) - min(mys) < DRC_MARK_PX):
                cxp = (min(mxs) + max(mxs)) / 2.0
                cyp = (min(mys) + max(mys)) / 2.0
                fill_rect(disp, cxp - DRC_MARK_PX // 2,
                          cyp - DRC_MARK_PX // 2,
                          DRC_MARK_PX, DRC_MARK_PX, mcol)
            elif self.drc_mark["kind"] == "p":
                self._drc_fill_speckle(disp, pts, mcol)
                for a, b in zip(pts, pts[1:] + pts[:1]):
                    stamp_segment(disp, a, b, None, mcol)
            else:
                # edge records: consecutive point pairs are segments
                for j in range(0, len(pts) - 1, 2):
                    stamp_segment(disp, pts[j], pts[j + 1], None,
                                  mcol)
        if self.mode == "esel" and self._esel_start is not None:
            ax, ay = self._esel_start
            bx, by = self._cursor
            rect_outline(disp, sx(ax), sy(ay), sx(bx), sy(by),
                         None, RULER_CORE, px=1)
        if self._drc is not None and self._drc_open is not None:
            # the canvas shows the CURRENT GRID PAGE's errors (user
            # call 2026-08-15: page flips must move the markers) -
            # geometry prebuilt by _drc_grid_fill
            if self._drc_hl and hasattr(self._drc, "query_rect"):
                # keeps the in-view filter grid following the view
                self._drc_hl_list()
            with _dprof("paint: rule errors"):
                self._drc_stamp_errs(disp, sx, sy,
                                     self._drc_page_marks)
        if self._drc_sel is not None:
            # box selection ('e'): GOLD on top of the status colors
            sci, _seis, marks, _eset = self._drc_sel
            self._drc_stamp_errs(
                disp, sx, sy,
                [(sci, ei_, kind, spts)
                 for ei_, kind, spts in marks], DRC_GOLD)
        if self._zoomdrag is not None and self._band_cur is not None:
            x0, y0 = self._zoomdrag
            x1, y1 = self._band_cur
            color = BAND_IN if x1 >= x0 else BAND_OUT
            rect_outline(disp, x0, y0, x1, y1, None, color, px=1)

    def _minimap_geom(self):
        """(scale, panel x0, panel y0, die px w, die px h) or None."""
        bb = self.meta["bbox"]
        bw, bh = bb[2] - bb[0], bb[3] - bb[1]
        if bw <= 0 or bh <= 0:
            return None
        scale = MINIMAP_PX / max(bw, bh)
        mw, mh = max(2, round(bw * scale)), max(2, round(bh * scale))
        return (scale, (MINIMAP_PX - mw) // 2,
                (MINIMAP_PX - mh) // 2, mw, mh)

    def _minimap_frontier_depth(self):
        """Bucket the minimap mirrors: the CURRENT semantic depth.
        None = no frontier layer (full depth, beyond the baked/
        folded range, or an old cache). The minimap is navigation
        chrome, so its frontier remains visible when main-view
        frames are off. Rev 46b: the frontier is baked at INDEX
        time through the real planner (canonical fit scale, medium
        detail) - the L9 gate holds it byte-equal to vfsd
        mode=frontier, so no runtime requests are needed."""
        if not getattr(self, "_frontier_depths", None):
            return None
        d = self.depth_value
        if d >= 999 or d >= len(self._frontier_depths):
            return None
        return d

    def _minimap_world_point(self, px, py):
        """Map a panel pixel inside the die to world coordinates."""
        geom = self._minimap_geom()
        if geom is None:
            return None
        scale, x0, y0, mw, mh = geom
        if not (x0 <= px <= x0 + mw - 1 and
                y0 <= py <= y0 + mh - 1):
            return None
        bb = self.meta["bbox"]
        return (bb[0] + (px - x0) / scale,
                bb[3] - (py - y0) / scale)

    def _on_minimap_click(self, _widget, event):
        if event.type != Gdk.EventType.BUTTON_PRESS or event.button != 1:
            return False
        point = self._minimap_world_point(event.x, event.y)
        if point is None:
            return True
        self.cx, self.cy = point
        self.redraw(immediate=True)
        return True

    def _minimap_base(self, d):
        """Die background + the depth-d frontier boxes, rendered once
        per depth and cached: the minimap is stamped on every frame,
        and re-drawing up to a thousand boxes in python per display
        would drag pan/refine (the view box rides on a copy())."""
        base = self._minimap_bases.get(d)
        if base is not None:
            return base
        disp = GdkPixbuf.Pixbuf.new(
            GdkPixbuf.Colorspace.RGB, False, 8, MINIMAP_PX, MINIMAP_PX)
        disp.fill(BLACK)
        geom = self._minimap_geom()
        if geom is not None:
            scale, x0, y0, mw, mh = geom
            bb = self.meta["bbox"]
            fill_rect(disp, x0, y0, mw, mh, MINIMAP_BG)
            frame_rect(disp, x0, y0, mw, mh, MINIMAP_EDGE)
            if d is not None:
                for row in self._frontier_depths[d]:
                    fx0, fy0, fx1, fy1 = row[0], row[1], \
                        row[2], row[3]
                    w = (fx1 - fx0) * scale
                    h = (fy1 - fy0) * scale
                    if w < 0.7 and h < 0.7:
                        continue  # sub-pixel dust stays off the map
                    frame_rect(disp,
                               x0 + (fx0 - bb[0]) * scale,
                               y0 + (bb[3] - fy1) * scale,
                               max(1, round(w)), max(1, round(h)),
                               MINIMAP_FRONT)
        self._minimap_bases[d] = disp
        return disp

    def _update_minimap(self, bbox):
        """Draw the die overview below the layer list: the cached
        per-depth base (die + always-visible structural frontier) plus
        the live view box."""
        geom = self._minimap_geom()
        disp = self._minimap_base(
            None if geom is None else self._minimap_frontier_depth())
        disp = disp.copy()
        if geom is None:
            self._minimap_image.set_from_pixbuf(disp)
            return
        scale, x0, y0, mw, mh = geom
        bb = self.meta["bbox"]

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

    def _update_labels(self, obox, ospp, disp=None):
        """Ruler distance labels: a pool of Gtk.Labels on the overlay.
        A single on-screen ruler keeps the plain up-right readout;
        several spread out flateyes-style (anchors along each own
        line, off shared crossings, never covering another chip,
        leader or line) and each chip ties back to ITS line with a
        dotted leader stamped into the frame."""
        # Tab hides overlays: the distance chips are WIDGETS, not
        # pixbuf paint, so they need their own gate (field report
        # 2026-08-19: chips survived the toggle)
        segs = [] if not self.overlays_on else list(self.rulers)
        if self.overlays_on and self.mode == "ruler" \
                and self._ruler_start is not None:
            segs.append((*self._ruler_start, *self._ruler_end_preview()))
        w, h = self._viewport_size()
        vis = []
        for seg in segs:
            x0, y0, x1, y1 = seg
            a = ((x0 - obox[0]) / ospp, (obox[3] - y0) / ospp)
            b = ((x1 - obox[0]) / ospp, (obox[3] - y1) / ospp)
            if isinstance(seg, _DrcOffsetRuler):
                a, b = drc_mod.offset_screen_segment(a, b)
            mx, my = (a[0] + b[0]) / 2, (a[1] + b[1]) / 2
            if -40 <= mx <= w and -20 <= my <= h:
                vis.append((a, b, mx, my,
                            math.hypot(x1 - x0, y1 - y0) * self.dbu))
        while len(self._labels) < len(vis):
            lbl = Gtk.Label()
            lbl.set_halign(Gtk.Align.START)
            lbl.set_valign(Gtk.Align.START)
            self.overlay.add_overlay(lbl)
            self._labels.append(lbl)
        multi = len(vis) > 1
        lines = [(a, b) for a, b, _, _, _ in vis]
        placed = []    # chip rects already positioned this pass
        leaders = []   # leader segments already claimed
        for idx, (a, b, mx, my, d_um) in enumerate(vis):
            lbl = self._labels[idx]
            lbl.set_markup('<span background="#101010" foreground='
                           '"#ffffff"> %s </span>'
                           % GLib.markup_escape_text("%.4f um" % d_um))
            lbl.show()   # a hidden label measures as zero
            # preferred size folds in the margins, which still hold
            # the previous position; subtract for the chip itself
            _, nat = lbl.get_preferred_size()
            tw = nat.width - lbl.get_margin_start()
            th = nat.height - lbl.get_margin_top()
            if multi:   # off the shared crossing, beside its own line
                others = lines[:idx] + lines[idx + 1:]
                x, y = pick_label_spot(a, b, tw, th, placed, leaders,
                                       others, w, h)
            else:
                x, y = shove_label(mx + 8, my - th - 6, tw, th,
                                   placed, w, h)
            lbl.set_margin_start(int(x))
            lbl.set_margin_top(int(y))
            rect = (x, y, x + tw, y + th)
            placed.append(rect)
            if multi:
                seg = leader_seg(a, b, rect)
                if seg is not None:
                    leaders.append(seg)
        if disp is not None:
            # with several rulers a bare readout no longer says which
            # line it measures (crossing rulers especially): dotted
            # leaders tie each chip back to its own line
            for end, foot in leaders:
                stamp_dotted(disp, end, foot, None, RULER_CORE)
        for lbl in self._labels[len(vis):]:
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
        # Render the viewport snapped to the SPECKLE PERIOD. The 2x2
        # checkerboard fill is anchored to the frame's device grid, so
        # two renders whose origins differ by an ODD pixel count show
        # inverted fill patterns - panning visibly "reshuffled" the
        # image (field report 2026-08-09; measured 5.3% of pixels).
        # Landing x0/left and y1/top on even-pixel boundaries of the
        # layout-anchored grid makes the phase a pure function of the
        # layout, so re-renders match across any pan (Calibre
        # behavior). The frame grows by <= 2 px per axis to keep
        # covering the exact viewport. (The old 50%-per-side overdraw
        # margin stays retired - user call, 2026-07-31.)
        spp2 = 2.0 * self.spp
        rx0 = math.floor(bbox[0] / spp2) * spp2
        ry1 = math.ceil(bbox[3] / spp2) * spp2
        w, h = int(w) + 2, int(h) + 2
        eb = (rx0, ry1 - h * self.spp, rx0 + w * self.spp, ry1)
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
        self._set_cursor("move" if self._drag is not None
                         else self._idle_cursor())

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
                        ph = ""
                        if res.get("phase_apply") is not None:
                            # load = plan (rust) + delta (author/IPC)
                            #        + apply (klayout parse + WC build)
                            ph = " [%d plan+%d delta+%d apply]" % (
                                res.get("phase_plan", 0),
                                res.get("phase_delta", 0),
                                res.get("phase_apply", 0))
                        split = " = %d load%s + %d draw" \
                            % (res["load_ms"], ph, res["draw_ms"])
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
                    # tiles = plan total (resident pages included);
                    # +new = pages actually shipped for this view
                    # (cache misses, summed over its stream rounds)
                    mode = "live (%d tiles, +%d new, %d ms" \
                           "%s%s%s%s%s%s%s)" \
                        % (res["tiles"], res.get("new", 0) or 0,
                           res["ms"], split,
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
        """Zooms and pans never lose the die: spp is capped at
        FIT_ZOOM_OUT x the fit-view scale and the viewport stays
        inside the die bbox grown by a 10% outer margin per side
        (user call 2026-08-18: pinned to the exact die edge there
        was no room to band-zoom around edge features). Centered
        on an axis the viewport is wider than."""
        db = self.meta["bbox"]
        mx = (db[2] - db[0]) * 0.10
        my = (db[3] - db[1]) * 0.10
        bb = (db[0] - mx, db[1] - my, db[2] + mx, db[3] + my)
        self.spp = min(max(self.spp, MIN_SPP),
                       self._fit_spp() * FIT_ZOOM_OUT)
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

    def _idle_cursor(self):
        # plain arrow at rest; the crosshair belongs to the ruler
        # and error-box-select tools
        return ("crosshair" if self.mode in ("ruler", "esel")
                else "default")

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
        self._set_cursor(self._idle_cursor())
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
        if ev.type == Gdk.EventType.DOUBLE_BUTTON_PRESS \
                and ev.button == 1 \
                and self.mode not in ("ruler", "esel") \
                and self._drc is not None \
                and not (ev.state & (Gdk.ModifierType.CONTROL_MASK
                                     | Gdk.ModifierType.SHIFT_MASK)):
            # canvas double-click ON a marker = grid-number double-
            # click (full jump). Checked BEFORE the drag guard: the
            # paired single press has already re-armed a pan drag,
            # which used to swallow the double event - disarm it.
            hit = self._drc_hit_at(ev.x, ev.y)
            if hit is not None:
                self._drag = None
                self._drag_origin = None
                self._drag_moved = False
                self._drag_btn = None
                self._set_cursor(self._idle_cursor())
                ci, ei = hit
                if self._drcwin is not None:
                    self._drc_goto_cell(ci, ei)
                self._drc_jump(ci, ei, isolate=True)
                return True
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
            self._set_cursor(self._idle_cursor())
            if panned:
                self.redraw()   # pan ended: render the final position
            elif was_drag and ev.button == 1:
                # A stationary left click keeps its mode-specific action;
                # movement is exclusively a pan gesture in both modes.
                if self.mode == "ruler":
                    self._ruler_click(ev)
                elif self.mode == "esel":
                    self._esel_click(ev)
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
        self._drc_tooltip(ev)
        self._hover(ev)
        return True

    def _drc_tooltip(self, ev):
        """Hover tooltip over a painted DRC marker: rule name,
        #local(global) number and waive status (user call
        2026-08-18)."""
        tip = None
        if self._drc is not None and self._drc_hits:
            hit = self._drc_hit_at(ev.x, ev.y)
            if hit is not None:
                ci, ei = hit
                try:
                    e = self._drc.checks[ci].errors[ei]
                    tip = "%s #%d(%d)%s" % (
                        self._drc.checks[ci].name, ei + 1, e.num,
                        " · waived"
                        if self._drc_waived(self._drc, ci, ei)
                        else "")
                except Exception:
                    tip = None
        if tip != self._drc_tip:
            self._drc_tip = tip
            self.scroller.set_tooltip_text(tip)
            if tip:
                self.scroller.trigger_tooltip_query()

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

    def _pan_view(self, direction, frac=KEY_PAN_FRACTION):
        """Move the viewport by a fraction of its visible extent
        (arrows: half; Ctrl+arrows: the fine tenth - Calibre)."""
        width, height = self._viewport_size()
        dx = width * self.spp * frac
        dy = height * self.spp * frac
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
        ctrl = bool(ev.state & Gdk.ModifierType.CONTROL_MASK)
        # Calibre-parity chords first, so Ctrl+A never falls through
        # to the plain-letter branches below
        if ctrl and name in ("z", "Z"):
            self._zoom_center(CAL_ZOOM_IN)     # zoom in 50%
        elif name == "Z":
            self._zoom_center(1 / CAL_ZOOM_IN)  # Shift+Z: out 50%
        elif ctrl and name in ("a", "A"):
            self.fit()                          # zoom all
        elif ctrl and name == "period":
            self._goto_dialog()                 # Ctrl+.
        elif ctrl and name in ("c", "C"):
            self._copy_view()                   # view -> clipboard
        elif name == "f":
            self._set_frames(not self.frames_on)
        elif name in ("Left", "Right", "Up", "Down"):
            self._pan_view(name, KEY_PAN_FRACTION_FINE if ctrl
                           else KEY_PAN_FRACTION)
        elif name in ("KP_Left", "KP_Right", "KP_Up", "KP_Down"):
            self._pan_view(name[3:], KEY_PAN_FRACTION_FINE if ctrl
                           else KEY_PAN_FRACTION)
        elif name in ("plus", "equal", "KP_Add"):
            self._zoom_center(1 / 1.25)
        elif name in ("minus", "KP_Subtract"):
            self._zoom_center(1.25)
        elif name == "r":
            self._toggle_ruler()
        elif name == "m":
            self._toggle_snap()
        elif name == "C":
            # Shift+C: frame (cell reference outline) toggle,
            # Calibre parity; plain c stays unbound (user call)
            self._set_frames(not self.frames_on)
        elif name == "k":
            self._ruler_pop()
        elif name == "K":
            self._rulers_clear()
        elif name == "Escape":
            self._esc()
        elif name in ("Tab", "ISO_Left_Tab"):
            self._toggle_overlays()
        elif name == "d":
            self._detail_dialog()
        elif name == "g":
            self._goto_dialog()
        elif name == "less":
            self._depth_step(-1)
        elif name == "greater":
            self._depth_step(1)
        elif name == "a":
            self._toggle_abstract()
        elif name == "v":
            self._toggle_coverage()
        elif name == "l":
            self._set_lod(not self.lod_on)
        elif name == "b":
            self._set_mono(not self._mono)
        elif name == "e":
            self._esel_toggle()
        elif name == "w":
            self._drc_waive_key()
        elif name == "n":
            self._drc_step(1)
        elif name == "p":
            self._drc_step(-1)
        elif name == "q":
            self._confirm_quit()
        elif len(name) == 1 and name.isdigit():
            self._depth_digit(int(name))
        elif name.startswith("KP_") and name[3:].isdigit():
            self._depth_digit(int(name[3:]))
        else:
            return False
        return True

    # ---- depth -----------------------------------------------------------------
    def _depth_digit(self, n):
        """Digit keys set the depth directly. A quick '9 9' (typing
        99) means FULL depth (999 internally): 'full' never had a
        key of its own (user call 2026-08-10)."""
        now = time.time()
        if n == 9 and self._digit_last == 9 \
                and now - self._digit_t < 1.0:
            self._digit_last = None
            self._set_depth(999)
            return
        self._digit_last = n
        self._digit_t = now
        self._set_depth(n)

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
        current = "*" if d is None else str(d)
        maximum = ("?" if self.max_depth is None
                   else str(self.max_depth))
        lbl = "depth: %s/%s" % (current, maximum)
        if self.meta.get("bands") or self.meta.get("vfs"):
            lbl += " · detail: %s" % DETAIL_LEVELS[self.detail]
        if self.meta.get("vfs"):
            lbl += " · cov:%s" % (
                "on" if self.coverage_on else "off")
            lbl += " · lod:%s" % ("on" if self.lod_on else "off")
            lbl += " · frame:%s" % (
                "on" if self.frames_on else "off")
        if self.abstract:
            lbl += " · abstract"
        return lbl

    def _depth_step(self, delta):
        """< / > step the depth by one, clamped to [0, max_depth].
        'full' (999) is treated as the deepest explicit level so a
        step down lands on real geometry rather than jumping to 0."""
        cap = self.max_depth if self.max_depth is not None else 999
        cur = self.depth_value
        if cur >= 999:
            cur = cap
        self._set_depth(max(0, min(cap, cur + delta)))

    def _set_depth(self, n):
        self.depth_value = max(0, min(999, int(n)))
        if self._ddlg is not None:
            spin = getattr(self._ddlg, "_spin", None)
            if spin is not None and \
                    int(spin.get_value()) != self.depth_value:
                spin.set_value(self.depth_value)
        self._on_depth()

    def _set_detail(self, n):
        self.detail = max(0, min(len(DETAIL_PX) - 1, int(n)))
        self.cut_px = DETAIL_PX[self.detail]
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

    def _set_frames(self, enabled, labels=None):
        """Set hierarchy frames and their text as one normal UI state.

        `labels` is reserved for an explicit CLI/forwarded-request override.
        Frames off still forces text off; the `f` shortcut passes no override
        and therefore always toggles both together.
        """
        enabled = bool(enabled)
        labels_enabled = enabled if labels is None \
            else enabled and bool(labels)
        changed = (enabled != self.frames_on or
                   labels_enabled != self.labels_on)
        self.frames_on = enabled
        self.labels_on = labels_enabled
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
            "\nwith names - keys: 0-9 = depth, &lt; / &gt; = step</small>")
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

    def _detail_dialog(self):
        """Runtime control of the detail level (low/medium/high). The
        VFS daemon applies the matching screen-px cut in its page plan,
        showing dropped subtrees as outline frames."""
        if self._cdlg is not None:
            self._cdlg.present()
            return
        dlg = Gtk.Window(title="detail")
        self._dialog_setup(dlg)
        self._cdlg = dlg
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
        box.set_margin_top(10)
        box.set_margin_bottom(10)
        box.set_margin_start(14)
        box.set_margin_end(14)
        dlg.add(box)
        box.pack_start(Gtk.Label(
            label="detail level (higher = finer, heavier wide views)"),
            False, False, 0)
        row = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=4)
        box.pack_start(row, False, False, 0)
        btns = []
        for lvl, nm in enumerate(DETAIL_LEVELS):
            b = Gtk.ToggleButton(label=nm)
            b.set_active(lvl == self.detail)

            def _apply(w, n=lvl):
                if not w.get_active():   # ignore the untoggle event
                    return
                for i, other in enumerate(btns):
                    if i != n and other.get_active():
                        other.set_active(False)
                self._set_detail(n)
            b.connect("toggled", _apply)
            btns.append(b)
            row.pack_start(b, False, False, 0)
        note = Gtk.Label()
        note.set_markup(
            "<small>lower detail hides finer features from live "
            "renders;\nareas below the cut draw as merged outlines "
            "instead\n(when the cache carries them - floe index "
            "--merge-only\nupgrades old caches). snap/pick/clip stay "
            "exact.\nthe status line shows the physical cut "
            "(cut&lt;0.35um).\nkeys: d = this dialog</small>")
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
        self._dialog_show(dlg, btns[self.detail])

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
        """Center the view on (x, y) um; window is the resulting
        view width in um (None/0 = keep the current zoom). The old X
        marker is gone (user call 2026-08-09)."""
        self.cx = x_um / self.dbu
        self.cy = y_um / self.dbu
        if window_um and window_um > 0:
            w, _h = self._viewport_size()
            self.spp = (window_um / self.dbu) / w
        self.redraw(immediate=True)

    # ---- DRC results browser -------------------------------------------------
    def _drc_window(self):
        """The browser lives in the LEFT pane - load a db when none
        is open, else focus it (open .db button equivalent)."""
        if self._drc is None:
            self._drc_open_dialog()
        else:
            self._drcwin._rules.grab_focus()

    def _esel_toggle(self):
        """'e': error box-select mode for the OPEN rule (user call
        2026-08-14) - two clicks span a box, every error of that
        rule inside it gets selected; works with highlight mode on
        or off."""
        if self.mode == "esel":
            self.mode = "normal"
            self._esel_start = None
            self._set_cursor(self._idle_cursor())
            self._set_live_status("error select off")
            self._display()
            return
        db, ci = self._drc, self._drc_sel_check()
        if db is None or ci is None:
            self._set_live_status(
                "error select: open a DRC db and select a rule first")
            return
        self.mode = "esel"
        self._esel_start = None
        self._set_cursor(self._idle_cursor())
        self._set_live_status(
            "error select %s: click the 1st box corner (Esc quits)"
            % db.checks[ci].name)

    def _esel_click(self, ev):
        if self._esel_start is None:
            self._esel_start = self._cursor
            self._set_live_status(
                "error select: click the opposite corner "
                "(Shift = add, Ctrl = toggle)")
            self._display()
            return
        a = self._esel_start
        b = self._cursor
        self._esel_start = None
        # like ruler mode the tool STAYS ARMED for the next box
        # until Esc (or 'e') leaves it - user call 2026-08-15
        state = getattr(ev, "state", 0)
        mode = ("toggle" if state & Gdk.ModifierType.CONTROL_MASK
                else "add" if state & Gdk.ModifierType.SHIFT_MASK
                else "replace")
        self._esel_apply(a, b, mode)

    def _esel_apply(self, a, b, mode="replace"):
        """Box done: the VISIBLE errors inside it - only what is
        currently painted (the filtered list's page) can be picked
        up. mode: replace (plain), add (Shift), toggle (Ctrl) -
        same second-click modifiers as the grid (user call
        2026-08-15)."""
        db, ci = self._drc, self._drc_sel_check()
        if db is None or ci is None:
            return
        x0, x1 = sorted((a[0], b[0]))    # dbu, like the marks
        y0, y1 = sorted((a[1], b[1]))
        hits = []
        for mci, ei, kind, pts in self._drc_page_marks:
            if mci != ci:
                continue
            xs = [p[0] for p in pts]
            ys = [p[1] for p in pts]
            if min(xs) <= x1 and max(xs) >= x0 \
                    and min(ys) <= y1 and max(ys) >= y0:
                hits.append((ei, kind, pts))
        sel = self._drc_sel
        if mode == "replace" or sel is None or sel[0] != ci:
            m = {ei: (kind, pts) for ei, kind, pts in hits}
        else:
            m = {ei: (kind, pts) for ei, kind, pts in sel[2]}
            for ei, kind, pts in hits:
                if mode == "toggle" and ei in m:
                    del m[ei]
                else:
                    m[ei] = (kind, pts)
        eis = sorted(m)
        marks = [(ei,) + m[ei] for ei in eis]
        self._drc_set_sel((ci, eis, marks, frozenset(eis))
                          if eis else None)
        self._drc_focus = None
        self._drc_grid_fill(ci)
        self._set_live_status(
            "error select %s [%s]: %d selected - next box ready "
            "(Shift add / Ctrl toggle, Esc exits)"
            % (db.checks[ci].name, mode, len(eis)))
        self._display()

    def _build_drc_panel(self):
        """DRC browser panel, embedded in the left pane (always
        available; formerly a separate window)."""
        win = _DrcPanel()
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
        top = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=4)
        box.pack_start(top, False, False, 2)
        b = Gtk.Button(label="open .db…")
        b.connect("clicked", lambda *_: self._drc_open_dialog())
        top.pack_start(b, False, False, 2)
        b = Gtk.Button(label="rules…")
        b.set_tooltip_text(
            "attach SVRF rule metadata (<deck>.rules.json from "
            "`python -m floe svrf <deck>`): constraint, source "
            "layers and measured value in the error detail")
        b.connect("clicked", lambda *_: self._drc_rules_dialog())
        top.pack_start(b, False, False, 0)
        info = Gtk.Label(label="no results database loaded")
        info.set_xalign(0.0)
        info.set_ellipsize(Pango.EllipsizeMode.START)
        top.pack_start(info, True, True, 2)
        win._info = info
        # top pane = rules list (left) | error-number grid (right);
        # bottom pane = per-error detail. Selecting a rule shows its
        # grid alone (one rule at a time = the accordion ask), and
        # the grid's own equal columns keep the numbers aligned
        # regardless of rule-title widths.
        rstore = Gtk.ListStore(str, str, int)  # name, count, ci
        rules = Gtk.TreeView(model=rstore)
        for j, expand in ((0, True), (1, False)):
            cell = Gtk.CellRendererText()
            if j == 0:
                # long rule names truncate so the error count
                # column always stays visible
                cell.set_property("ellipsize",
                                  Pango.EllipsizeMode.END)
            col = Gtk.TreeViewColumn("", cell, text=j)
            col.set_expand(expand)
            rules.append_column(col)
        rules.set_headers_visible(False)
        rules.set_enable_search(False)
        rules.set_tooltip_column(0)
        rules.get_selection().connect("changed",
                                      self._on_drc_rule_sel)
        rsc = Gtk.ScrolledWindow()
        # NO hscroll: at any pane width the name column ellipsizes
        # instead of the list panning/clipping sideways (user call
        # 2026-08-18 - shrunk panes must degrade to "…", never to
        # content cut off at the edge)
        rsc.set_policy(Gtk.PolicyType.NEVER,
                       Gtk.PolicyType.AUTOMATIC)
        rsc.add(rules)
        _remote_x_scroll_repaint(rsc)
        # rule search on TOP of the rules list (user call
        # 2026-08-18; GTK's built-in typeahead popup stays disabled)
        try:
            se = Gtk.SearchEntry()
            se.connect("search-changed", self._on_drc_search)
        except AttributeError:
            se = Gtk.Entry()
            se.connect("changed", self._on_drc_search)
        se.set_placeholder_text("find rule…")
        se.set_width_chars(8)   # keep the pane's width floor small
        win._search = se
        rbox = Gtk.Box(orientation=Gtk.Orientation.VERTICAL,
                       spacing=2)
        rbox.pack_start(se, False, False, 0)
        rbox.pack_start(rsc, True, True, 0)
        # grid: DRC_GRID_W equal markup columns, NO row selection -
        # the clicked cell alone is marked (user call 2026-08-13)
        gstore = Gtk.ListStore(*([str] * DRC_GRID_W))
        grid = Gtk.TreeView(model=gstore)
        for j in range(DRC_GRID_W):
            col = Gtk.TreeViewColumn("", Gtk.CellRendererText(),
                                     markup=j)
            # every column expands: leftover pane width spreads
            # evenly instead of stacking up as a right margin
            col.set_expand(True)
            grid.append_column(col)
        grid.set_headers_visible(False)
        grid.set_enable_search(False)
        grid.get_selection().set_mode(Gtk.SelectionMode.NONE)
        grid.connect("button-press-event", self._on_drc_grid_click)
        # the number array REFLOWS to the pane width instead of
        # scrolling horizontally (user call 2026-08-13)
        grid.connect("size-allocate", self._on_drc_grid_alloc)
        gsc = Gtk.ScrolledWindow()
        # ALWAYS-on vscrollbar: its appearing/vanishing width made
        # the column reflow oscillate on big rules (field report
        # 2026-08-14 - the 120k rule "kept refreshing")
        gsc.set_policy(Gtk.PolicyType.NEVER,
                       Gtk.PolicyType.ALWAYS)
        gsc.add(grid)
        _remote_x_scroll_repaint(gsc)
        pbar = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL,
                       spacing=2)
        pprev = Gtk.Button(label="◀")
        pnext = Gtk.Button(label="▶")
        plabel = Gtk.Label(label="")
        for b, d in ((pprev, -1), (pnext, 1)):
            b.connect("clicked",
                      lambda _w, dd=d: self._drc_page_step(dd))
            b.set_sensitive(False)
        pbar.pack_start(pprev, False, False, 0)
        pbar.pack_start(plabel, True, True, 0)
        pbar.pack_start(pnext, False, False, 0)
        gbox = Gtk.Box(orientation=Gtk.Orientation.VERTICAL,
                       spacing=2)
        gbox.pack_start(pbar, False, False, 0)
        gbox.pack_start(gsc, True, True, 0)
        hsplit = Gtk.Paned(orientation=Gtk.Orientation.HORIZONTAL)
        hsplit.pack1(rbox, resize=True, shrink=True)
        hsplit.pack2(gbox, resize=True, shrink=True)
        # PROPORTIONAL split (user report 2026-08-18: a fixed 220px
        # position swallowed the whole 196px startup pane, hiding
        # the grid with the handle stuck at the edge): the divider
        # keeps its FRACTION of the pane - drags update it, resizes
        # (like the auto-widen on db load) re-apply it
        frac = [0.45]
        guard = [False]

        def _hs_pos(wdg, _pspec):
            w = wdg.get_allocated_width()
            if not guard[0] and w > 1:
                frac[0] = wdg.get_position() / float(w)

        def _hs_alloc(wdg, alloc):
            if alloc.width <= 1:
                return
            guard[0] = True
            try:
                wdg.set_position(int(alloc.width * frac[0]))
            finally:
                guard[0] = False

        hsplit.connect("notify::position", _hs_pos)
        hsplit.connect("size-allocate", _hs_alloc)
        paned = Gtk.Paned(orientation=Gtk.Orientation.VERTICAL)
        paned.pack1(hsplit, resize=True, shrink=True)
        detail = Gtk.TextView()
        detail.set_editable(False)
        detail.set_cursor_visible(False)
        detail.set_monospace(True)
        # narrow panes WRAP the detail text - no sideways clipping
        # or panning (user call 2026-08-18)
        detail.set_wrap_mode(Gtk.WrapMode.WORD_CHAR)
        dsc = Gtk.ScrolledWindow()
        dsc.set_policy(Gtk.PolicyType.NEVER,
                       Gtk.PolicyType.AUTOMATIC)
        dsc.add(detail)
        paned.pack2(dsc, resize=False, shrink=True)
        paned.set_position(420)
        box.pack_start(paned, True, True, 0)
        win._detail = detail.get_buffer()
        # filter controls in a FlowBox: on a narrow pane they WRAP
        # onto extra rows instead of clipping (user call 2026-08-18)
        nav = Gtk.FlowBox()
        nav.set_selection_mode(Gtk.SelectionMode.NONE)
        nav.set_min_children_per_line(1)
        nav.set_max_children_per_line(4)
        nav.set_column_spacing(4)
        nav.set_row_spacing(2)
        box.pack_start(nav, False, False, 2)
        hl = Gtk.CheckButton(label="in view")
        hl.set_active(self._drc_hl)
        hl.set_tooltip_text("list only the errors inside the "
                            "current view (packed .ice v2 only)")
        hl.connect("toggled", self._on_drc_hl)
        nav.add(hl)
        win._hl = hl
        selv = Gtk.CheckButton(label="selected")
        selv.set_tooltip_text("list only the box/Ctrl-selected "
                              "errors")
        selv.connect("toggled", self._on_drc_selview)
        nav.add(selv)
        win._selv = selv
        tf = Gtk.ComboBoxText()
        tf.append("all", "all types")
        tf.set_active_id("all")
        tf.set_sensitive(False)   # enabled once rules.json attaches
        tf.set_tooltip_text("filter rules by their SVRF measurement "
                            "type (needs the .rules.json sidecar)")
        tf.connect("changed", self._on_drc_tfilter)
        nav.add(tf)
        win._tf = tf
        wf = Gtk.ComboBoxText()
        for wid, lbl in (("all", "All"),
                         ("notwaived", "Not Waived"),
                         ("waived", "Waived")):
            wf.append(wid, lbl)
        wf.set_active_id("all")
        wf.connect("changed", self._on_drc_wfilter)
        nav.add(wf)
        win._wf = wf
        win._rules, win._rstore = rules, rstore
        win._grid, win._gstore = grid, gstore
        win._plabel, win._pprev, win._pnext = plabel, pprev, pnext
        self._drcwin = win
        return box

    def _on_drc_hl(self, btn):
        on = btn.get_active()
        if on and not (self._drc is not None
                       and hasattr(self._drc, "query_rect")):
            btn.set_active(False)
            self._set_live_status(
                "the in-view filter needs a packed index: "
                "floe-index drc <db> --pack")
            return
        self._drc_hl = on
        self._drc_hl_res = None
        if self._drc_open is not None:
            self._drc_grid_fill(self._drc_open)
        self._display()

    def _set_mono(self, on, announce=True):
        """Grayscale all design layers ('b'; DRC visibility)."""
        on = bool(on)
        if on == self._mono:
            return
        self._mono = on
        self.worker.submit({"kind": "mono", "on": on})
        self._color_epoch += 1
        if announce:
            self._set_live_status(
                "layers grayscale %s" % ("on" if on else "off"))
        self.redraw(immediate=True)

    def _drc_sel_check(self):
        """Check index the highlight applies to: the rule selected
        in the browser (grid owner), else the rule of the last
        jumped error, else None."""
        if self._drc_open is not None:
            return self._drc_open
        if self._drc_pos >= 0 and self._drc_cum:
            return bisect.bisect_right(self._drc_cum,
                                       self._drc_pos) - 1
        return None

    def _drc_hl_list(self):
        """The SELECTED rule's violations in the current view as
        [(kind, pts dbu)] - recomputed only when the view, db or
        selected rule changes (user call 2026-08-13: one rule at a
        time, cap DRC_HL_CAP)."""
        ci = self._drc_sel_check()
        # the viewport bbox keys the cache: it folds center, zoom
        # AND canvas size (cx/cy/spp alone served a stale list
        # after a window resize)
        bb = self.view_bbox()
        key = (bb, id(self._drc), ci, self._drc_wfilter)
        cached = self._drc_hl_res
        if cached is not None and cached[0] == key:
            return cached[1]
        if ci is None:
            self._drc_hl_res = (key, [])
            self._set_live_status(
                "DRC filter: select a rule in the browser first")
            return []
        k = self.dbu
        kw = {}
        if self._drc_wfilter != "all" \
                and hasattr(self._drc, "get_status"):
            # the waive filter runs INSIDE the query, before the
            # cap: a post-filter over a capped result dropped every
            # match hiding past `cap` non-matching errors
            kw["waived"] = self._drc_wfilter == "waived"
        with _dprof("hl_list: query_rect"):
            res = self._drc.query_rect(bb[0] * k, bb[1] * k,
                                       bb[2] * k, bb[3] * k,
                                       cap=DRC_HL_CAP, checks=(ci,),
                                       **kw)
        lst = [(rci, rei, e.kind,
                [(x / k, y / k) for x, y in e.pts])
               for rci, rei, e in res]
        self._drc_hl_res = (key, lst)
        if self._drc_hl:
            self._set_live_status(
                "DRC filter %s: %d in view%s"
                % (self._drc.checks[ci].name, len(lst),
                   " (capped)" if len(lst) >= DRC_HL_CAP else ""))
        # the browser grid lists exactly these in-view errors: let
        # it follow pans/zooms (idle: this runs inside the overlay
        # draw path; the refill re-reads the now-cached list)
        if self._drc_hl and self._drc_grid_ci == ci:
            GLib.idle_add(self._drc_grid_fill, ci)
        return lst

    def _drc_open_dialog(self):
        dlg = Gtk.FileChooserDialog(title="open DRC results (.db)",
                                    parent=self.window,
                                    action=Gtk.FileChooserAction.OPEN)
        dlg.add_buttons("Cancel", Gtk.ResponseType.CANCEL,
                        "Open", Gtk.ResponseType.OK)
        dlg.set_current_folder(
            os.path.dirname(self.meta["src"]["path"]))
        for name, pats in (("Calibre DRC results (*.db)",
                            ("*.db",)),
                           ("all files", ("*",))):
            ff = Gtk.FileFilter()
            ff.set_name(name)
            for p in pats:
                ff.add_pattern(p)
            dlg.add_filter(ff)
        if dlg.run() == Gtk.ResponseType.OK:
            path = dlg.get_filename()
            dlg.destroy()
            self._drc_open_db(path)
        else:
            dlg.destroy()

    def _drc_open_db(self, path):
        """Dialog flow (user call 2026-08-14): the user PICKS the
        ASCII .db, floe LOADS only its packed .ice - building the
        pack first (modal log dialog) when it is missing, stale or
        an old layout/v1 sidecar."""
        from . import drc as drc_mod
        side = path + ".ice"
        if os.path.exists(side):
            try:
                db = drc_mod.IcePack(side, src_path=path,
                                     verify_src=True)
            except (ValueError, OSError):
                pass
            else:
                self.load_drc(path, db=db)  # adopt the fresh pack
                return
        self._drc_pack_and_load(path)

    def _drc_pack_and_load(self, path):
        """Run `floe-index drc <db> --pack` with its log in a MODAL
        dialog, then load the pack."""
        import subprocess
        import threading
        from .vfsclient import find_binary
        try:
            bin_ = find_binary()
        except RuntimeError as exc:
            self._set_live_status("DRC indexing failed: %s" % exc)
            return
        dlg = Gtk.Dialog(title="indexing DRC results…",
                         transient_for=self.window, modal=True)
        dlg.set_default_size(600, 340)
        tv = Gtk.TextView()
        tv.set_editable(False)
        tv.set_cursor_visible(False)
        tv.set_monospace(True)
        buf = tv.get_buffer()
        sc = Gtk.ScrolledWindow()
        sc.set_policy(Gtk.PolicyType.AUTOMATIC,
                      Gtk.PolicyType.AUTOMATIC)
        sc.add(tv)
        dlg.get_content_area().pack_start(sc, True, True, 4)
        dlg.add_button("cancel", Gtk.ResponseType.CANCEL)
        dlg.show_all()
        try:
            proc = subprocess.Popen(
                [bin_, "drc", path, "--pack"],
                stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                text=True, bufsize=1)
        except OSError as exc:
            dlg.destroy()
            self._set_live_status("DRC indexing failed: %s" % exc)
            return
        state = {"cancelled": False}

        def on_response(_d, _resp):
            state["cancelled"] = True
            try:
                proc.terminate()
            except OSError:
                pass

        dlg.connect("response", on_response)

        def append(line):
            buf.insert(buf.get_end_iter(), line)
            buf.place_cursor(buf.get_end_iter())
            tv.scroll_mark_onscreen(buf.get_insert())
            return False

        def done(rc):
            dlg.destroy()
            if state["cancelled"]:
                self._set_live_status("DRC indexing cancelled")
            elif rc == 0:
                self.load_drc(path)
                if not isinstance(self._drc, drc_mod.IcePack):
                    # the indexer that just ran wrote a layout the
                    # reader refuses: it is an OUTDATED binary
                    self._set_live_status(
                        "DRC pack from %s is an old layout - "
                        "rebuild it: cd rust && cargo build "
                        "--release" % bin_)
            else:
                self._set_live_status(
                    "DRC indexing failed (rc %d)" % rc)
            return False

        def pump():
            for line in proc.stdout:
                GLib.idle_add(append, line)
            rc = proc.wait()
            GLib.idle_add(done, rc)

        threading.Thread(target=pump, daemon=True).start()

    def load_drc(self, path, db=None):
        """Open a Calibre DRC db (.ice index-aware) and populate the
        browser. `db` = an already-opened backend to adopt (the
        dialog preflight verifies the pack by opening it - opening
        twice pays the block-table bbox scan twice)."""
        from . import drc as drc_mod
        if db is None:
            try:
                db = drc_mod.load_db(path)
            except Exception as exc:
                msg = "DRC load failed: %s" % exc
                if self._drcwin is not None:
                    self._drcwin._info.set_text(msg)
                self._set_live_status(msg)
                return False
        self._drc = db
        # the embedded browser needs elbow room: widen the left
        # pane once a db is loaded (user can still drag it back)
        if self._lpaned.get_position() < 420:
            self._lpaned.set_position(420)
        self._drc_cum = []
        total = 0
        for c in db.checks:
            self._drc_cum.append(total)
            total += len(c.errors)
        self._drc_total = total
        self._drc_pos = -1
        self.drc_mark = None
        self._drc_hl_res = None
        self._drc_sel = None
        self._drc_sels = {}
        self._esel_start = None
        self._drc_tfilter = "all"   # new db = new type census
        self._drc_rules_auto(path)
        self._drc_types_rebuild()   # no-sidecar case: combo empties
        if self._drcwin is not None:
            self._drc_fill()
            # a db opens with NO rule selected (user call
            # 2026-08-18). GTK can auto-select row 0 during the
            # model reattach while the busy guard swallows the
            # event - the row then LOOKS selected with an empty
            # grid; drop that phantom selection instead.
            self._drc_rules_busy = True
            try:
                self._drcwin._rules.get_selection().unselect_all()
            finally:
                self._drc_rules_busy = False
        self._set_live_status(
            "DRC %s: %d checks, %d errors (n/p = step)"
            % (os.path.basename(path), len(db.checks), db.total))
        return True

    def _drc_rules_auto(self, db_path):
        """Auto-pick the `floe svrf` rule-metadata sidecar: the
        deck's Rule File Pathname is an absolute path from the
        CALIBRE RUN machine, so <deck basename>.rules.json NEXT TO
        THE DB is searched first, the recorded path second."""
        self._drc_rmeta = None
        self._drc_rmatch = (0, 0)
        deck = None
        for c in self._drc.checks[:50]:
            for ln in (c.desc or "").split("\n"):
                if ln.startswith("Rule File Pathname:"):
                    deck = ln.split(":", 1)[1].strip()
                    break
            if deck:
                break
        dbdir = os.path.dirname(os.path.abspath(db_path))
        cands = []
        if deck:
            cands.append(os.path.join(
                dbdir, os.path.basename(deck) + ".rules.json"))
            cands.append(deck + ".rules.json")
        cands.append(db_path + ".rules.json")
        for p in cands:
            if os.path.isfile(p):
                self._drc_rules_load(p, silent=True)
                return

    def _drc_rules_load(self, path, silent=False):
        """Attach a <deck>.rules.json metadata sidecar to the open
        db; the detail pane then shows constraint / measured /
        source layers per error."""
        from . import svrf
        db = self._drc
        if db is None:
            if not silent:
                self._set_live_status("open a DRC .db first")
            return False
        try:
            data = svrf.load_rules(path)
        except (OSError, ValueError) as exc:
            if not silent:
                self._set_live_status(
                    "rules metadata load failed: %s" % exc)
            return False
        meta = data.get("checks", {})
        matched = sum(1 for c in db.checks if c.name in meta)
        self._drc_rmeta = data
        self._drc_rmatch = (matched, len(db.checks))
        self._drc_types_rebuild()
        self._drc_info_refresh()
        if not silent or matched:
            self._set_live_status(
                "rules metadata %s: %d/%d rules matched"
                % (os.path.basename(path), matched, len(db.checks)))
        return True

    def _drc_rules_dialog(self):
        dlg = Gtk.FileChooserDialog(
            title="open rule metadata (*.rules.json)",
            parent=self.window,
            action=Gtk.FileChooserAction.OPEN)
        dlg.add_buttons("Cancel", Gtk.ResponseType.CANCEL,
                        "Open", Gtk.ResponseType.OK)
        if self._drc is not None:
            dlg.set_current_folder(
                os.path.dirname(os.path.abspath(self._drc.path)))
        for name, pats in (("rule metadata (*.rules.json)",
                            ("*.rules.json",)),
                           ("all files", ("*",))):
            ff = Gtk.FileFilter()
            ff.set_name(name)
            for p in pats:
                ff.add_pattern(p)
            dlg.add_filter(ff)
        if dlg.run() == Gtk.ResponseType.OK:
            path = dlg.get_filename()
            dlg.destroy()
            self._drc_rules_load(path)
        else:
            dlg.destroy()

    def _drc_fill(self):
        """Rules list: NAME + total/waived counts (user call
        2026-08-18); under a waive filter rules with no matching
        errors are hidden (2026-08-14). No waived state exists in
        real data yet, so every error counts as Not Waived until
        Calibre's notation is known."""
        win, db = self._drcwin, self._drc
        rstore = win._rstore
        _t0 = time.perf_counter()
        self._drc_rules_busy = True
        win._rules.set_model(None)   # detach: clear/append without
        rstore.clear()               # per-row view+selection work
        win._gstore.clear()
        self._drc_open = None
        self._drc_grid_ci = None
        self._drc_grid_rows = 0
        self._drc_grid_map = []
        self._drc_grid_base = None
        self._drc_cell = None
        self._drc_page = 0
        _t1 = time.perf_counter()
        _tc = 0.0
        shown = 0
        for ci, c in enumerate(db.checks):
            _t2 = time.perf_counter()
            if self._drc_search \
                    and self._drc_search not in c.name.lower():
                continue
            if self._drc_tfilter != "all":
                ts = (self._drc_rtypes or {}).get(c.name)
                if not ts or self._drc_tfilter not in ts:
                    continue
            cnt = self._drc_wf_count(db, ci)
            _tc += time.perf_counter() - _t2
            # All lists EVERY rule, zero-error ones included (user
            # call 2026-08-14); the waive filters hide empty rules
            if cnt <= 0 and self._drc_wfilter != "all":
                continue
            # count column = total/waived (user call 2026-08-18);
            # both are O(1) ([wcount] table - the perf contract)
            wv = (db.status_counts(ci)[0]
                  if hasattr(db, "status_counts") else 0)
            rstore.append([c.name,
                           "%d/%d" % (len(c.errors), wv), ci])
            shown += 1
        win._rules.set_model(rstore)
        self._drc_rules_busy = False
        if _DRC_PROF:
            sys.stderr.write(
                "[drcprof] rules_fill rules=%d shown=%d clear=%.1f "
                "counts=%.1f appends=%.1f ms\n"
                % (len(db.checks), shown,
                   (_t1 - _t0) * 1e3, _tc * 1e3,
                   (time.perf_counter() - _t1 - _tc) * 1e3))
            sys.stderr.flush()
        self._drc_shown = shown
        self._drc_info_refresh()

    def _drc_info_refresh(self):
        win, db = self._drcwin, self._drc
        if win is None or db is None:
            return
        backend = ("pack v4" if isinstance(db, drc_mod.IcePack)
                   else "ASCII - NO INDEX")
        rules = ""
        if self._drc_rmeta is not None:
            rules = " · svrf %d/%d" % self._drc_rmatch
        win._info.set_text(
            "%s [%s] — cell %s · %d/%d rules · %d errors%s"
            % (os.path.basename(db.path), backend, db.cell,
               self._drc_shown, len(db.checks), db.total, rules))

    def _drc_types_rebuild(self):
        """Classify every rule by its SVRF measurement metrics
        (rules.json constraints) and rebuild the type combo: 'all
        types' + each metric present with its rule count. A matched
        rule with no parsed measurement (and any unmatched rule)
        classifies as 'other'. No sidecar -> combo empty+disabled,
        filter forced back to 'all'."""
        win, db = self._drcwin, self._drc
        self._drc_rtypes = None
        counts = {}
        meta = (self._drc_rmeta or {}).get("checks", {})
        if db is not None and meta:
            types = {}
            for c in db.checks:
                mc = meta.get(c.name)
                ms = frozenset(
                    x.get("metric")
                    for x in (mc or {}).get("constraints", ())
                    if x.get("metric")) or frozenset(("other",))
                types[c.name] = ms
                for m in ms:
                    counts[m] = counts.get(m, 0) + 1
            self._drc_rtypes = types
        if self._drc_tfilter != "all" and self._drc_tfilter \
                not in counts:
            self._drc_tfilter = "all"
        if win is None:
            return
        tf = win._tf
        self._drc_tf_busy = True
        try:
            tf.remove_all()
            tf.append("all", "all types")
            ordered = [m for m in DRC_METRICS if m in counts]
            ordered += sorted(set(counts) - set(DRC_METRICS))
            for m in ordered:
                tf.append(m, "%s (%d)" % (m, counts[m]))
            tf.set_active_id(self._drc_tfilter)
            tf.set_sensitive(bool(counts))
        finally:
            self._drc_tf_busy = False

    def _on_drc_tfilter(self, combo):
        if self._drc_tf_busy:
            return
        tid = combo.get_active_id() or "all"
        if tid == self._drc_tfilter or self._drc is None:
            return
        self._drc_tfilter = tid
        # per-rule selections stay valid (the filter only hides
        # RULES); the open rule may vanish from the list, though
        keep = self._drc_open
        self._drc_fill()
        if keep is not None:
            for r in self._drcwin._rstore:
                if r[2] == keep:
                    self._drcwin._rules.set_cursor(r.path, None,
                                                   False)
                    break
        self._display()

    def _drc_wf_count(self, db, ci):
        """Error count of a rule under the waive filter."""
        total = len(db.checks[ci].errors)
        if self._drc_wfilter == "all":
            return total
        if not hasattr(db, "status_counts"):
            return 0 if self._drc_wfilter == "waived" else total
        w, t = db.status_counts(ci)
        return w if self._drc_wfilter == "waived" else t - w

    def _drc_wf_base(self, db, ci):
        """Grid base under the waive filter: None = all errors,
        list = explicit eis, ('status', waived) = LAZY - pages come
        from status_page so a 100M-error rule never materializes
        its filtered list (field report 2026-08-14: filter
        switches still took seconds)."""
        if not hasattr(db, "status_page"):
            return [] if self._drc_wfilter == "waived" else None
        return ("status", self._drc_wfilter == "waived")

    def _on_drc_wfilter(self, combo):
        wid = combo.get_active_id() or "all"
        if wid == self._drc_wfilter or self._drc is None:
            return
        if _DRC_PROF:
            import platform
            import numpy
            sys.stderr.write(
                "[drcprof] == filter -> %s | py %s %s numpy %s\n"
                % (wid, sys.version.split()[0], platform.machine(),
                   numpy.__version__))
        self._drc_wfilter = wid
        self._drc_hl_res = None
        # a waive-filter switch RESETS the selections (user call
        # 2026-08-15): the old sets no longer match what is listed
        self._drc_sel = None
        self._drc_sels = {}
        self._drc_focus = None
        keep = self._drc_open
        with _dprof("wfilter: rules refill"):
            self._drc_fill()
        with _dprof("wfilter: reselect rule"):
            if keep is not None:
                for r in self._drcwin._rstore:
                    if r[2] == keep:
                        self._drcwin._rules.set_cursor(
                            r.path, None, False)
                        break
        with _dprof("wfilter: display"):
            self._display()

    def _drc_set_sel(self, selobj):
        """Set the open rule's selection and keep the per-rule
        store in sync (None clears the open rule's entry)."""
        self._drc_sel = selobj
        if selobj is not None:
            self._drc_sels[selobj[0]] = selobj
        elif self._drc_open is not None:
            self._drc_sels.pop(self._drc_open, None)

    def _on_drc_search(self, entry):
        txt = entry.get_text().strip().lower()
        if txt == self._drc_search or self._drc is None:
            return
        self._drc_search = txt
        keep = self._drc_open
        self._drc_fill()
        if keep is not None:
            for r in self._drcwin._rstore:
                if r[2] == keep:
                    self._drcwin._rules.set_cursor(r.path, None,
                                                   False)
                    break

    def _on_drc_selview(self, btn):
        self._drc_show_sel = btn.get_active()
        if self._drc_open is not None:
            self._drc_grid_fill(self._drc_open)
        self._display()

    def _drc_page_step(self, delta):
        ci = self._drc_grid_ci
        if ci is None:
            return
        p = self._drc_page + delta
        if p < 0:
            return
        self._drc_page = p       # the fill clamps to the last page
        self._drc_grid_fill(ci)
        self._display()          # the canvas markers show THIS page

    def _on_drc_rule_sel(self, sel):
        """Selecting a rule shows ITS error grid alone (the
        accordion ask) and its info in the detail pane."""
        if self._drc_rules_busy:
            # ListStore.clear() deletes row by row and SOME GTK
            # builds auto-select the next row each time - without
            # this guard the full handler (grid fill + query +
            # display) ran per deleted row: a 1000-rule filter
            # switch took 9.7s on one machine (field profile
            # 2026-08-15)
            return
        model, it = sel.get_selected()
        if it is None or self._drc is None:
            return
        ci = model.get_value(it, 2)
        if ci == self._drc_open:
            return
        if self._drc_sel is not None:
            self._drc_sels[self._drc_sel[0]] = self._drc_sel
        self._drc_open = ci
        # a live jump belongs to the PREVIOUS rule: drop its mark,
        # position and auto CD rulers or they linger on the canvas
        # over the new rule's errors (user call 2026-08-18)
        if self.drc_mark is not None:
            self.drc_mark = None
            self._drc_pos = -1
            for r in self._drc_ruler:
                if r in self.rulers:
                    self.rulers.remove(r)
            self._drc_ruler = []
        # selections are PER RULE and survive switches (user call
        # 2026-08-15: 'selected' must keep applying)
        self._drc_sel = self._drc_sels.get(ci)
        self._drc_page = 0
        with _dprof("rule_sel: grid fill"):
            self._drc_grid_fill(ci)
        self._drc_show_rule(ci)
        # the rule's errors are painted on the canvas: repaint NOW,
        # not on the next incidental redraw (user call 2026-08-14)
        self._drc_hl_res = None
        with _dprof("rule_sel: display"):
            self._display()

    def _on_drc_grid_alloc(self, _w, alloc):
        n = max(1, min(24, alloc.width // max(24, self._drc_cellw)))
        if n != self._drc_gridw:
            self._drc_grid_set_cols(n)

    def _drc_grid_set_cols(self, n):
        """Rebuild the grid with n columns (model column count is
        fixed per ListStore) and refill, keeping the marked cell."""
        win = self._drcwin
        if win is None:
            return
        grid = win._grid
        mei = None
        if self._drc_cell is not None:
            row, j = self._drc_cell
            idx = row * self._drc_gridw + j
            if self._drc_grid_map is not None:
                if idx < len(self._drc_grid_map):
                    mei = self._drc_grid_map[idx]
            else:
                mei = idx
        self._drc_gridw = n
        for col in list(grid.get_columns()):
            grid.remove_column(col)
        store = Gtk.ListStore(*([str] * n))
        for j in range(n):
            col = Gtk.TreeViewColumn(
                "", Gtk.CellRendererText(), markup=j)
            col.set_expand(True)
            grid.append_column(col)
        grid.set_model(store)
        win._gstore = store
        if self._drc_grid_ci is not None:
            keep = self._drc_focus
            self._drc_grid_fill(self._drc_grid_ci)
            self._drc_focus = keep
            if mei is not None:
                if self._drc_grid_map is not None:
                    idx = (self._drc_grid_map.index(mei)
                           if mei in self._drc_grid_map else None)
                else:
                    idx = mei
                if idx is not None:
                    self._drc_cell_mark(idx // n, idx % n)

    def _drc_grid_fill(self, ci):
        """One PAGE (DRC_PAGE cells) of the rule's error grid.

        Numbers are RULE-LOCAL 1-based (user call 2026-08-14 - the
        Calibre-style global number lives in the detail pane as
        #local(global)). The base honours the waive filter and,
        with 'filter errors in view' on, the viewport;
        _drc_grid_map always holds the page's ei list."""
        win, db = self._drcwin, self._drc
        if win is None or db is None:
            return
        _t0 = time.perf_counter()
        gstore = win._gstore
        gstore.clear()
        self._drc_cell = None
        self._drc_grid_ci = ci
        c = db.checks[ci]
        # the visible list = selected ∧ in-view ∧ waive filter
        # (each stage narrows; None = every error of the rule)
        base = None
        sel = self._drc_sel
        if self._drc_show_sel:
            if sel is not None and sel[0] == ci and sel[1]:
                base = list(sel[1])
            else:
                base = []   # 'selected' with no selection = empty
        if self._drc_hl and hasattr(db, "query_rect"):
            inview = [ei for rci, ei, _k, _p in self._drc_hl_list()
                      if rci == ci]
            if base is None:
                base = inview
            else:
                iv = set(inview)
                base = [ei for ei in base if ei in iv]
        if self._drc_wfilter != "all":
            if base is None:
                base = self._drc_wf_base(db, ci)
            elif hasattr(db, "get_status"):
                want = self._drc_wfilter == "waived"
                base = [ei for ei in base
                        if (db.get_status(ci, ei) == 1) == want]
            elif self._drc_wfilter == "waived":
                base = []
        if base is None:
            count = len(c.errors)
        elif isinstance(base, tuple):
            count = self._drc_wf_count(db, ci)   # O(1) via wcount
        else:
            count = len(base)
        self._drc_grid_base = base
        pages = max(1, -(-count // DRC_PAGE))
        self._drc_page = max(0, min(self._drc_page, pages - 1))
        start = self._drc_page * DRC_PAGE
        stop = min(start + DRC_PAGE, count)
        if base is None:
            eis = list(range(start, stop))
        elif isinstance(base, tuple):
            eis = db.status_page(ci, base[1], start, stop - start)
        else:
            eis = base[start:stop]
        self._drc_grid_map = eis
        win._plabel.set_text("%d / %d" % (self._drc_page + 1,
                                          pages))
        win._pprev.set_sensitive(self._drc_page > 0)
        win._pnext.set_sensitive(self._drc_page < pages - 1)
        sel = self._drc_sel
        eset = (sel[3] if sel is not None and sel[0] == ci
                else frozenset())
        # cell width follows the page's widest LOCAL number
        maxloc = (max(eis) + 1) if eis else 1
        probe = win._grid.create_pango_layout(
            "0" * max(2, len(str(maxloc))))
        self._drc_cellw = probe.get_pixel_size()[0] + 18
        avail = win._grid.get_allocation().width
        n = max(1, min(24, avail // max(24, self._drc_cellw)))
        if n != self._drc_gridw:
            self._drc_grid_set_cols(n)  # refills at the new width
            return
        W = self._drc_gridw

        def cellfmt(ei):
            t = "%d" % (ei + 1)      # rule-local numbering
            fg = ("#00e676" if self._drc_waived(db, ci, ei)
                  else "#ff5252")    # waived green / not-waived red
            if ei in eset:
                return ("<span background='#ffd700' "
                        "foreground='%s'>%s</span>" % (fg, t))
            return "<span foreground='%s'>%s</span>" % (fg, t)

        for b2 in range(0, len(eis), W):
            cells = [cellfmt(ei) for ei in eis[b2:b2 + W]]
            cells += [""] * (W - len(cells))
            gstore.append(cells)
        self._drc_grid_rows = (len(eis) + W - 1) // W
        # keep the focused error marked when it is on this page
        f = self._drc_focus
        if f is not None and f[0] != ci:
            self._drc_focus = None
        elif f is not None and f[1] in eis:
            idx = eis.index(f[1])
            self._drc_cell_mark(idx // W, idx % W)
        # the canvas paints THIS page (user call 2026-08-15):
        # geometry built once per fill; the in-view filter already
        # decoded its pts, other modes decode the page's blocks
        k = self.dbu
        marks = []
        if self._drc_hl and hasattr(db, "query_rect"):
            have = {rei: (k2, p2)
                    for rci, rei, k2, p2 in self._drc_hl_list()
                    if rci == ci}
            for ei in eis:
                kp = have.get(ei)
                if kp is not None:
                    marks.append((ci, ei, kp[0], kp[1]))
        else:
            errs = c.errors
            for ei in eis:
                e = errs[ei]
                marks.append((ci, ei, e.kind,
                              [(x / k, y / k) for x, y in e.pts]))
        self._drc_page_marks = marks
        if _DRC_PROF:
            dt = (time.perf_counter() - _t0) * 1e3
            if dt >= 1.0:
                sys.stderr.write(
                    "[drcprof] grid_fill ci=%d page=%d cells=%d "
                    "base=%s %8.1f ms\n"
                    % (ci, self._drc_page, len(eis),
                       ("all" if base is None else
                        "lazy" if isinstance(base, tuple)
                        else "list"), dt))
                sys.stderr.flush()

    def _drc_sel_click(self, ci, row, j, idx, ei, is_range):
        """Grid selection editing (user call 2026-08-14):
        Ctrl+click toggles ONE error in/out of the selection,
        Shift+click ADDS the visual range from the current cell.
        No pan/zoom change."""
        db = self._drc
        sel = self._drc_sel
        eset = (set(sel[3]) if sel is not None and sel[0] == ci
                else set())
        if is_range:
            aidx = idx
            if self._drc_cell is not None:
                aidx = (self._drc_cell[0] * self._drc_gridw
                        + self._drc_cell[1])
            lo, hi = sorted((aidx, idx))
            gmap = self._drc_grid_map
            for k2 in range(lo, min(hi + 1, len(gmap))):
                eset.add(gmap[k2])
        elif ei in eset:
            eset.discard(ei)
        else:
            eset.add(ei)
        eis = sorted(eset)
        k = self.dbu
        errs = db.checks[ci].errors
        marks = [(e2, errs[e2].kind,
                  [(x / k, y / k) for x, y in errs[e2].pts])
                 for e2 in eis]
        self._drc_set_sel((ci, eis, marks, frozenset(eset))
                          if eis else None)
        self._drc_grid_fill(ci)   # repaint the gold marks
        self._drc_cell_mark(row, j)
        self._drc_show_detail(ci, ei)
        self._set_live_status(
            "error select %s: %d selected"
            % (db.checks[ci].name, len(eis)))
        self._display()

    def _on_drc_grid_menu(self, tree, ev):
        """Right click on an error number: waive/unwaive it - or
        the whole GOLD selection when one exists (user call
        2026-08-14; writes the v2 status byte in place)."""
        hit = tree.get_path_at_pos(int(ev.x), int(ev.y))
        if hit is None:
            return False
        path, col, _cx, _cy = hit
        row = path.get_indices()[0]
        ci = self._drc_grid_ci
        db = self._drc
        if ci is None or db is None \
                or row >= self._drc_grid_rows:
            return False
        try:
            j = tree.get_columns().index(col)
        except ValueError:
            return False
        idx = row * self._drc_gridw + j
        gmap = self._drc_grid_map
        if idx >= len(gmap):
            return False
        ei = gmap[idx]
        if not hasattr(db, "set_status"):
            self._set_live_status(
                "waive needs a packed index: "
                "floe-index drc <db> --pack")
            return True
        self._drc_cell_mark(row, j)
        self._drc_show_detail(ci, ei)
        sel = self._drc_sel
        if sel is not None and sel[0] == ci and sel[1]:
            targets = list(sel[1])
            scope = "%d selected" % len(targets)
        else:
            targets = [ei]
            scope = "#%d" % (ei + 1)
        menu = Gtk.Menu()
        for label, on in (("waive %s" % scope, True),
                          ("unwaive %s" % scope, False)):
            it = Gtk.MenuItem(label=label)
            it.connect("activate",
                       lambda _i, o=on, t=targets:
                       self._drc_set_waived(ci, t, o))
            menu.append(it)
        self._drc_menu = menu

        def released(_menu):
            if self._drc_menu is menu:
                self._drc_menu = None

        menu.connect("deactivate", released)
        menu.show_all()
        if hasattr(menu, "popup_at_pointer"):
            menu.popup_at_pointer(ev)
        else:
            menu.popup(None, None, None, None, ev.button, ev.time)
        return True

    def _drc_set_waived(self, ci, eis, on):
        """Write the status byte for the targets and refresh every
        dependent surface (rule counts, grid, detail, canvas)."""
        db = self._drc
        if db is None or not hasattr(db, "set_status"):
            return
        val = drc_mod.STATUS_WAIVED if on else drc_mod.STATUS_NONE
        try:
            for ei in eis:
                db.set_status(ci, ei, val)
        except OSError as exc:
            self._set_live_status("waive failed (%s) - is the .ice "
                                  "writable?" % exc)
            return
        self._drc_hl_res = None
        # the jump mark stores a RESOLVED color: re-derive it for
        # the marked error or a toggle leaves the old status color
        if self.drc_mark is not None and self._drc_pos >= 0 \
                and self._drc_cum:
            mci = bisect.bisect_right(self._drc_cum,
                                      self._drc_pos) - 1
            mei = self._drc_pos - self._drc_cum[mci]
            self.drc_mark["color"] = (
                DRC_GREEN if self._drc_waived(db, mci, mei)
                else DRC_RED)
        # under a waive filter the toggled errors leave the list -
        # purge them from the gold selection too, or their markers
        # linger on the canvas (user report 2026-08-15)
        sel = self._drc_sel
        if sel is not None and sel[0] == ci \
                and self._drc_wfilter != "all" \
                and hasattr(db, "get_status"):
            want = self._drc_wfilter == "waived"
            keep = [(e2, k2, p2) for e2, k2, p2 in sel[2]
                    if (db.get_status(ci, e2) == 1) == want]
            eis2 = [m[0] for m in keep]
            self._drc_set_sel((ci, eis2, keep, frozenset(eis2))
                              if eis2 else None)
            f = self._drc_focus
            if f is not None and f[0] == ci \
                    and (db.get_status(ci, f[1]) == 1) != want:
                self._drc_focus = None
        if self._drc_wfilter == "all":
            # membership is unchanged but the row's total/waived
            # text is not: refresh that one row in place
            for r in self._drcwin._rstore:
                if r[2] == ci:
                    wv = (db.status_counts(ci)[0]
                          if hasattr(db, "status_counts") else 0)
                    r[1] = "%d/%d" % (len(db.checks[ci].errors), wv)
                    break
            self._drc_grid_fill(ci)
        else:
            # counts and membership changed under a waive filter:
            # rebuild the rule list and reselect the open rule
            keep = self._drc_open
            self._drc_fill()
            if keep is not None:
                for r in self._drcwin._rstore:
                    if r[2] == keep:
                        self._drcwin._rules.set_cursor(
                            r.path, None, False)
                        break
        f = self._drc_focus
        if f is not None and f[0] == ci:
            self._drc_show_detail(ci, f[1])
        self._set_live_status(
            "%s %d error(s) in %s"
            % ("waived" if on else "unwaived", len(eis),
               db.checks[ci].name))
        self._display()

    def _drc_waive_key(self):
        """w: toggle the waive status of the CURRENT error (user
        call 2026-08-20) - the single-clicked/stepped focus first,
        else the jump position (_drc_focus is None while a jump
        mark is live, and _drc_pos survives the Esc restore, so the
        error still shown in the detail pane stays toggleable)."""
        db = self._drc
        if db is None:
            return
        ci = ei = None
        f = self._drc_focus
        if f is not None:
            ci, ei = f[0], f[1]
        elif self._drc_pos >= 0 and self._drc_cum:
            ci = bisect.bisect_right(self._drc_cum,
                                     self._drc_pos) - 1
            ei = self._drc_pos - self._drc_cum[ci]
        if ci is None or ei >= len(db.checks[ci].errors):
            self._set_live_status(
                "w toggles the current error: click or jump one "
                "first")
            return
        if not hasattr(db, "set_status"):
            self._set_live_status(
                "waive needs a packed index: "
                "floe-index drc <db> --pack")
            return
        self._drc_set_waived(ci, [ei],
                             not self._drc_waived(db, ci, ei))

    def _drc_cell_mark(self, row, j):
        """Mark ONE grid cell as current: the previous cell reverts
        through the shared formatter (local number, gold when
        selected)."""
        win = self._drcwin
        ci = self._drc_grid_ci
        if win is None or ci is None:
            return
        gstore = win._gstore
        sel = self._drc_sel
        eset = (sel[3] if sel is not None and sel[0] == ci
                else frozenset())
        gmap = self._drc_grid_map

        db = self._drc

        def cell_at(r, c_, current):
            k2 = r * self._drc_gridw + c_
            if k2 >= len(gmap):
                return ""
            ei = gmap[k2]
            t = "%d" % (ei + 1)
            if current:
                return ("<span background='#3465a4' "
                        "foreground='#ffffff'>%s</span>" % t)
            fg = ("#00ffff" if self._drc_waived(db, ci, ei)
                  else "#ff5252")
            if ei in eset:
                return ("<span background='#ffd700' "
                        "foreground='%s'>%s</span>" % (fg, t))
            return "<span foreground='%s'>%s</span>" % (fg, t)

        old = self._drc_cell
        if old is not None and old != (row, j):
            orow, oj = old
            gstore[orow][oj] = cell_at(orow, oj, False)
        gstore[row][j] = cell_at(row, j, True)
        self._drc_cell = (row, j)

    def _on_drc_grid_click(self, tree, ev):
        """Click an error number: mark the cell + detail pane;
        double-click also jumps; Ctrl/Shift edit the selection;
        right click = waive/unwaive menu."""
        if ev.button == 3 \
                and ev.type == Gdk.EventType.BUTTON_PRESS:
            return self._on_drc_grid_menu(tree, ev)
        if ev.button != 1:
            return False
        hit = tree.get_path_at_pos(int(ev.x), int(ev.y))
        if hit is None:
            return False
        path, col, _cx, _cy = hit
        row = path.get_indices()[0]
        ci = self._drc_grid_ci
        if ci is None or row >= self._drc_grid_rows:
            return False
        try:
            j = tree.get_columns().index(col)
        except ValueError:
            return False
        idx = row * self._drc_gridw + j
        gmap = self._drc_grid_map
        if idx >= len(gmap):
            return False
        ei = gmap[idx]
        db = self._drc
        if db is None or ei >= len(db.checks[ci].errors):
            return False
        if ev.type == Gdk.EventType.DOUBLE_BUTTON_PRESS:
            self._drc_jump(ci, ei, isolate=True)
            return False
        if ev.state & (Gdk.ModifierType.CONTROL_MASK
                       | Gdk.ModifierType.SHIFT_MASK):
            self._drc_sel_click(
                ci, row, j, idx, ei,
                bool(ev.state & Gdk.ModifierType.SHIFT_MASK))
            return False
        self._drc_cell_mark(row, j)
        e = db.checks[ci].errors[ei]
        self._drc_focus = (ci, ei, e.kind,
                           [(x / self.dbu, y / self.dbu)
                            for x, y in e.pts])
        self._drc_show_detail(ci, ei)
        self._display()
        return False

    def _drc_show_rule(self, ci):
        win, db = self._drcwin, self._drc
        if win is None or db is None:
            return
        c = db.checks[ci]
        lines = ["rule: %s" % c.name,
                 "errors: %d (declared %d)"
                 % (len(c.errors), c.declared)]
        if c.desc:
            lines.append("")
            lines += c.desc.split("\n")
        win._detail.set_text("\n".join(lines))

    def _drc_show_detail(self, ci, ei):
        """Bottom pane: rule description, kind, coordinates and the
        review status of one violation."""
        win, db = self._drcwin, self._drc
        if win is None or db is None:
            return
        c = db.checks[ci]
        e = c.errors[ei]
        status = "-"
        if hasattr(db, "get_status"):
            s = db.get_status(ci, ei)
            status = {0: "none", 1: "waived",
                      2: "reserved"}.get(s, "status %d" % s)
        lines = ["#%d(%d)  [%s]" % (ei + 1, e.num, status),
                 "rule: %s" % c.name]
        if c.desc:
            # pathname/title already show in the RULE info (user
            # call 2026-08-15)
            lines += [ln for ln in c.desc.split("\n")
                      if not ln.startswith(("Rule File Pathname:",
                                            "Rule File Title:"))]
        lines += self._drc_meta_lines(c.name, e)
        pts = e.pts
        if e.kind == "e":
            lines.append("edge (um):")
            for j in range(0, min(len(pts) - 1, 128), 2):
                lines.append("  (%.4f, %.4f) - (%.4f, %.4f)"
                             % (pts[j][0], pts[j][1],
                                pts[j + 1][0], pts[j + 1][1]))
            if len(pts) > 129:
                lines.append("  … %d more edges"
                             % ((len(pts) - 128) // 2))
        else:
            lines.append("polygon (um):")
            for x, y in pts[:64]:
                lines.append("  (%.4f, %.4f)" % (x, y))
            if len(pts) > 64:
                lines.append("  … %d more" % (len(pts) - 64))
        win._detail.set_text("\n".join(lines))

    def _drc_jump(self, ci, ei, isolate=False):
        db = self._drc
        check = db.checks[ci]
        e = check.errors[ei]
        self._drc_pos = self._drc_cum[ci] + ei
        self._drc_focus = None    # the jump mark supersedes it
        iso = None
        if isolate:   # double-click only (user call 2026-08-15)
            iso = self._drc_isolate_layers(check.name)
        b = e.bbox()
        w_um, h_um = b[2] - b[0], b[3] - b[1]
        cx, cy = e.center()
        # zoom so the whole violation spans ~DRC_VIEW_FRACTION of
        # the view on BOTH axes (goto sets the view WIDTH, so the
        # vertical requirement converts through the canvas aspect)
        vw, vh = self._viewport_size()
        win = w_um / DRC_VIEW_FRACTION
        if vh > 0:
            win = max(win, h_um / DRC_VIEW_FRACTION * (vw / float(vh)))
        if win <= 0:
            win = 0.1   # degenerate (point-like) violation
        self.goto(cx, cy, win)
        self.drc_mark = {"kind": e.kind,
                         "pts": [(x / self.dbu, y / self.dbu)
                                 for x, y in e.pts],
                         "color": (DRC_GREEN
                                   if self._drc_waived(db, ci, ei)
                                   else DRC_RED)}
        # auto CD rulers: replaced on every jump (hand-drawn rulers
        # stay; k/Esc treat them like any ruler)
        for r in self._drc_ruler:
            if r in self.rulers:
                self.rulers.remove(r)
        self._drc_ruler = self._drc_cd_ruler(e)
        if e.kind == "e" and len(e.pts) == 2 and self._drc_ruler:
            # A ruler painted directly over a one-edge violation hides the
            # red error. Tag it for a constant screen-space parallel offset;
            # rendering adds dotted endpoint extension lines.
            self._drc_ruler = [_DrcOffsetRuler(self._drc_ruler[0])]
        self.rulers.extend(self._drc_ruler)
        self._set_live_status(
            "DRC %s #%d(%d)/%d · %s · %.3f x %.3f um at (%.3f, %.3f)%s"
            % (check.name, ei + 1, e.num, len(check.errors),
               "poly" if e.kind == "p" else "edge",
               w_um, h_um, cx, cy,
               "" if iso is None
               else " · %d layer(s) on, Esc restores" % iso))
        self._drc_show_detail(ci, ei)
        self._display()

    def _drc_isolate_layers(self, name):
        """Double-click jump: show ONLY the layers the rule
        metadata traces this check to (source_gds closure; dt None
        = every datatype of that gds layer). The pre-isolation
        visibility is snapshotted ONCE - Esc restores it (its own
        stage, before the DRC mark). Applied BEFORE goto so the
        jump renders once with the new layer set. Returns the
        matched layer count or None (no sidecar / no match)."""
        meta = self._drc_rmeta
        mc = meta.get("checks", {}).get(name) if meta else None
        gds = (mc or {}).get("source_gds") or []
        if not gds:
            return None
        want = set()
        for l in self.meta["layers"]:
            key = (l["layer"], l["datatype"])
            for g, d in gds:
                if key[0] == g and (d is None or key[1] == d):
                    want.add(key)
                    break
        if not want:
            return None   # metadata names no layer of THIS design
        if self._drc_lyr_saved is None:
            self._drc_lyr_saved = set(self.visible)
        if want != self.visible:
            self._layers_batch = True
            try:
                for key, row in self._layer_rows.items():
                    row.set_active(key in want)
            finally:
                self._layers_batch = False
        return len(want)

    def _drc_restore_layers(self):
        """Esc: back to the visibility snapshot taken by the first
        isolation."""
        saved, self._drc_lyr_saved = self._drc_lyr_saved, None
        if saved is None:
            return
        self._layers_batch = True
        try:
            for key, row in self._layer_rows.items():
                row.set_active(key in saved)
        finally:
            self._layers_batch = False
        self._set_live_status(
            "layer visibility restored (%d on)" % len(saved))
        self.redraw(immediate=True)

    def _drc_stamp_errs(self, disp, sx, sy, items, color=None):
        """Error painter: geometry in `color`, or per-status when
        None (user call 2026-08-14/17: not waived = red, waived =
        green). ONLY the jumped error (double-click / n-p - the one
        drc_mark points at) draws its real shape, collapsing to a
        marker when its screen span is below the marker size; every
        OTHER error is a DRC_MARK_PX square at ANY zoom (user call
        2026-08-18 - shape soup at wide views; the focused one:
        9x9). A 20k segment budget bounds pathological frames."""
        db = self._drc
        focus = self._drc_focus
        jci = jei = -1
        if self.drc_mark is not None and self._drc_pos >= 0 \
                and self._drc_cum:
            jci = bisect.bisect_right(self._drc_cum,
                                      self._drc_pos) - 1
            jei = self._drc_pos - self._drc_cum[jci]
        budget = 20000
        for ci_, ei_, kind, spts in items:
            col = color
            if col is None:
                col = (DRC_GREEN
                       if self._drc_waived(db, ci_, ei_)
                       else DRC_RED)
            sp = [(sx(x), sy(y)) for x, y in spts]
            hxs = [p[0] for p in sp]
            hys = [p[1] for p in sp]
            if (ci_ != jci or ei_ != jei) \
                    or (max(hxs) - min(hxs) < DRC_MARK_PX
                        and max(hys) - min(hys) < DRC_MARK_PX):
                cxp = (min(hxs) + max(hxs)) / 2.0
                cyp = (min(hys) + max(hys)) / 2.0
                s_px = 9 if (focus is not None
                             and focus[0] == ci_
                             and focus[1] == ei_) else DRC_MARK_PX
                fill_rect(disp, cxp - s_px // 2, cyp - s_px // 2,
                          s_px, s_px, col)
                self._drc_hits.append((cxp, cyp, ci_, ei_))
                budget -= 1
            else:
                if kind == "p":
                    segs = list(zip(sp, sp[1:] + sp[:1]))
                else:
                    segs = [(sp[j], sp[j + 1])
                            for j in range(0, len(sp) - 1, 2)]
                for a, b in segs:
                    stamp_segment(disp, a, b, None, col)
                    budget -= 1
            if budget <= 0:
                return

    def _drc_waived(self, db, ci, ei):
        """True when the error's status byte says WAIVED (v1 dbs
        have no status: everything counts as not waived)."""
        return (hasattr(db, "get_status")
                and db.get_status(ci, ei)
                == drc_mod.STATUS_WAIVED)

    def _drc_speckle_strip(self, width, color):
        """Two-row RGBA checker strip in `color` (row r: on where
        (x+r) is even), composited row-by-row into polygon spans -
        GdkPixbuf has no stipple fill, and per-pixel subpixbuf
        fills would be far too slow per frame. Cached per color."""
        strips = getattr(self, "_drc_strips", None)
        if strips is None:
            strips = self._drc_strips = {}
        strip = strips.get(color)
        if strip is not None and strip.get_width() >= width:
            return strip
        strip = GdkPixbuf.Pixbuf.new(GdkPixbuf.Colorspace.RGB, True,
                                     8, max(width, 2), 2)
        strip.fill(0x00000000)
        for r in (0, 1):
            for x in range(r & 1, strip.get_width(), 2):
                strip.new_subpixbuf(x, r, 1, 1).fill(color)
        strips[color] = strip
        return strip

    def _drc_fill_speckle(self, disp, pts, color=DRC_RED):
        """Even-odd scanline fill of the violation polygon with the
        50% speckle checker (screen-anchored phase). Degenerate and
        very complex outlines fall back to outline-only."""
        if len(pts) < 3 or len(pts) > 256:
            return
        w_px, h_px = disp.get_width(), disp.get_height()
        ys = [p[1] for p in pts]
        y0 = max(0, int(math.floor(min(ys))))
        y1 = min(h_px - 1, int(math.ceil(max(ys))))
        if y1 < y0:
            return
        strip = self._drc_speckle_strip(w_px, color)
        edges = [(a, b) for a, b in zip(pts, pts[1:] + pts[:1])
                 if a[1] != b[1]]
        budget = 20000   # spans; beyond this the fill just stops
        for y in range(y0, y1 + 1):
            yc = y + 0.5
            xs = []
            for (ax, ay), (bx, by) in edges:
                if (ay <= yc < by) or (by <= yc < ay):
                    xs.append(ax + (yc - ay) * (bx - ax) / (by - ay))
            if not xs:
                continue
            xs.sort()
            for i in range(0, len(xs) - 1, 2):
                x0 = max(0, int(math.ceil(xs[i])))
                x1 = min(w_px, int(math.floor(xs[i + 1])) + 1)
                if x1 <= x0:
                    continue
                strip.composite(disp, x0, y, x1 - x0, 1,
                                0, y - (y & 1), 1, 1,
                                GdkPixbuf.InterpType.NEAREST, 255)
                budget -= 1
                if budget <= 0:
                    return

    def _drc_meta_lines(self, name, e):
        """Detail-pane block from the `floe svrf` sidecar: the
        constraint as written in the deck, this violation's own
        measured dimension vs the bound (the waive-decision aid),
        the referenced layer names + source gds, and the derivation
        chain that produced them."""
        meta = self._drc_rmeta
        mc = meta.get("checks", {}).get(name) if meta else None
        if not mc:
            return []
        lines = [""]
        cons = mc.get("constraints") or []
        for t in dict.fromkeys(c.get("text", "") for c in cons):
            if t:
                lines.append("constraint: %s" % t)
        cands = []
        for con in cons:
            v = con.get("value")
            if v is None:
                continue
            mv = self._drc_measured(e, con.get("metric"))
            if mv is not None:
                cands.append((con, v, mv))
        if cands:
            # prefer the UPPER bound: "> 0 < v" chains flag by the
            # upper limit - the lower bound reads as a meaningless
            # positive delta (real decks use zero-lower ranges)
            pick = next((c for c in cands
                         if c[0].get("op") in ("<", "<=", "==")),
                        cands[0])
            con, v, mv = pick
            pct = (" (%+.1f%%)" % ((mv - v) / v * 100.0)) if v else ""
            unit = "um2" if con.get("metric") == "area" else "um"
            lines.append("measured: %.4f %s vs %s %.4f · "
                         "Δ %+.4f%s"
                         % (mv, unit, con.get("op", "?"), v,
                            mv - v, pct))
        lays = mc.get("layers") or []
        if lays:
            lines.append("layers: %s" % ", ".join(lays))
        gds = mc.get("source_gds") or []
        if gds:
            lines.append("gds: %s" % ", ".join(
                "%s/%s" % (g, "*" if d is None else d)
                for g, d in gds))
        if mc.get("unresolved"):
            lines.append("unresolved: %s"
                         % ", ".join(mc["unresolved"]))
        from . import svrf
        derived = meta.get("derived", {})
        out, seen, stack = [], set(), list(lays)
        while stack and len(out) < 6:
            n = stack.pop(0)
            if n in seen or n not in derived:
                continue
            seen.add(n)
            out.append("  %s = %s" % (n, derived[n]))
            stack.extend(svrf.rhs_operands(derived[n]))
        if out:
            lines.append("derivation:")
            lines += out
            if any(n in derived and n not in seen for n in stack):
                lines.append("  …")
        return lines

    def _drc_measured(self, e, metric):
        """This violation's own dimension for a sidecar constraint
        metric - only shapes whose measurement is unambiguous (the
        same ones that get auto CD rulers). Returns um (um^2 for
        area) or None."""
        pts = e.pts   # um
        if metric == "area":
            if e.kind != "p" or len(pts) < 3:
                return None
            s = 0.0
            for i in range(len(pts)):
                x0, y0 = pts[i]
                x1, y1 = pts[(i + 1) % len(pts)]
                s += x0 * y1 - x1 * y0
            return abs(s) / 2.0
        if metric in ("width", "space", "enclosure"):
            # rect region -> min span; facing edge pair -> the gap
            if e.kind == "p" or (e.kind == "e" and len(pts) == 4):
                segs = self._drc_cd_ruler(e)   # dbu 4-tuples
                if not segs:
                    return None
                # cd_segments keeps the true closest edge-pair gap first;
                # later entries can be its horizontal/vertical diagnostics.
                if e.kind == "e":
                    x0, y0, x1, y1 = segs[0]
                    return math.hypot(x1 - x0, y1 - y0) * self.dbu
                return min(math.hypot(x1 - x0, y1 - y0)
                           for x0, y0, x1, y1 in segs) * self.dbu
            return None
        if metric == "length" and e.kind == "e" and len(pts) == 2:
            return math.hypot(pts[1][0] - pts[0][0],
                              pts[1][1] - pts[0][1])
        return None

    def _drc_cd_ruler(self, e):
        """CD rulers of a violation as dbu 4-tuples - the geometry
        (single edge / facing gap and optional axis components /
        rect spans) lives in
        drc.cd_segments, SHARED with the CLI snapshot embeds; this
        wrapper only converts um -> dbu."""
        k = self.dbu
        return [(x0 / k, y0 / k, x1 / k, y1 / k)
                for x0, y0, x1, y1 in drc_mod.cd_segments(e)]

    def _drc_step(self, delta):
        """n/p: cycle within the VISIBLE list of the open rule
        (user call 2026-08-15) - whatever the selected/in-view/
        waive filters left, across pages."""
        db = self._drc
        ci = self._drc_sel_check()
        if db is None or ci is None:
            self._set_live_status(
                "n/p cycles the open rule: select a rule first")
            return
        win = self._drcwin
        # sync the browser FIRST so the grid base belongs to ci
        if win is not None and self._drc_grid_ci != ci:
            self._drc_page = 0
            for r in win._rstore:   # the list may be filtered
                if r[2] == ci:
                    win._rules.set_cursor(r.path, None, False)
                    win._rules.scroll_to_cell(
                        r.path, None, False, 0.0, 0.0)
                    break
        n_all = len(db.checks[ci].errors)
        lo = self._drc_cum[ci]
        cur = (self._drc_pos - lo
               if self._drc_pos >= 0
               and lo <= self._drc_pos < lo + n_all else None)
        ei = self._drc_step_ei(db, ci, self._drc_grid_base, cur,
                               delta)
        if ei is None:
            self._set_live_status(
                "no errors in the current list (rule %s)"
                % db.checks[ci].name)
            return
        if win is not None:
            self._drc_goto_cell(ci, ei)
        if self.drc_mark is not None:
            # an error is designated (double-click/pick jump is
            # live): n/p keeps the full framing jump + CD rulers
            self._drc_jump(ci, ei)
            return
        # Esc-restored (or never jumped): n/p acts like a plain
        # click on the grid number - cell mark (goto_cell above),
        # detail, focus - the VIEW does not move (user call
        # 2026-08-18); position still advances for the next step
        e = db.checks[ci].errors[ei]
        self._drc_pos = lo + ei
        self._drc_focus = (ci, ei, e.kind,
                           [(x / self.dbu, y / self.dbu)
                            for x, y in e.pts])
        self._drc_show_detail(ci, ei)
        self._display()

    def _drc_step_ei(self, db, ci, base, cur, delta):
        """Next/previous ei within the visible list, wrapping."""
        if base is None:
            n = len(db.checks[ci].errors)
            if not n:
                return None
            if cur is None:
                return 0 if delta > 0 else n - 1
            return (cur + delta) % n
        if isinstance(base, tuple):     # lazy waive filter
            waived = base[1]
            cnt = self._drc_wf_count(db, ci)
            if not cnt:
                return None
            rank = (db.status_rank(ci, waived, cur)
                    if cur is not None else None)
            nrank = ((rank + delta) % cnt if rank is not None
                     else (0 if delta > 0 else cnt - 1))
            page = db.status_page(ci, waived, nrank, 1)
            return page[0] if page else None
        if not base:
            return None
        if cur is None or cur not in base:
            return base[0] if delta > 0 else base[-1]
        return base[(base.index(cur) + delta) % len(base)]

    def _drc_goto_cell(self, ci, ei):
        """Flip the grid to the page holding ei and mark its
        cell (skips silently when a filter hides it)."""
        win = self._drcwin
        if win is None or self._drc_grid_ci != ci:
            return
        base = self._drc_grid_base
        if base is None:
            bidx = ei
        elif isinstance(base, tuple):
            bidx = self._drc.status_rank(ci, base[1], ei)
            if bidx is None:
                return
        elif ei in base:
            bidx = base.index(ei)
        else:
            return
        page = bidx // DRC_PAGE
        if page != self._drc_page:
            self._drc_page = page
            self._drc_grid_fill(ci)
        rel = bidx - page * DRC_PAGE
        row, j = divmod(rel, self._drc_gridw)
        self._drc_cell_mark(row, j)
        win._grid.scroll_to_cell(
            Gtk.TreePath.new_from_string(str(row)),
            None, False, 0.0, 0.0)

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
        elif self.mode == "esel":
            if self._esel_start is not None:
                parts.append("error select: click the opposite "
                             "corner (Esc cancels)")
                self._display()   # live box preview
            else:
                parts.append("error select: click the 1st corner")
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
            n = self._measure_selection()
            if n:
                self._set_live_status(
                    "ruler: %d auto rulers from selection (closest "
                    "gaps) · click two points, Esc=done" % n)
            else:
                self._set_live_status(
                    "ruler: click two points (Shift=free angle, "
                    "m=snap %s, k=undo, Shift+K=clear, Esc=done)"
                    % ("on" if self.snap_on else "off"))
        self._set_cursor(self._idle_cursor())
        self._display()

    def _measure_selection(self):
        """Entering ruler mode with 2+ picked objects auto-measures
        them at their CLOSEST parts only (field request: no union
        spans): every object pairs with its nearest neighbour, and
        each pair gets a horizontal and/or vertical ruler between
        the facing edges - only on axes where the pair is disjoint,
        so touching or overlapping geometry adds nothing. Bbox
        based. Regenerated on every ruler-mode entry; the previous
        auto set is replaced, hand-drawn rulers are kept."""
        boxes = [tuple(s["bbox"]) for s in self.selections
                 if s.get("bbox")]
        if len(boxes) < 2:
            return 0
        if self._auto_rulers:
            self.rulers = [r for r in self.rulers
                           if r not in self._auto_rulers]

        def sep(a, b):
            dx = max(a[0] - b[2], b[0] - a[2], 0)
            dy = max(a[1] - b[3], b[1] - a[3], 0)
            return math.hypot(dx, dy)

        pairs = set()
        for i in range(len(boxes)):
            best, bd = None, None
            for j in range(len(boxes)):
                if j == i:
                    continue
                d = sep(boxes[i], boxes[j])
                if bd is None or d < bd:
                    bd, best = d, j
            pairs.add((min(i, best), max(i, best)))

        def cross_mid(a, b, olo, ohi):
            # anchor a gap ruler where the pair overlaps on the
            # OTHER axis; midway between centers when it does not
            lo = max(a[olo], b[olo])
            hi = min(a[ohi], b[ohi])
            if lo <= hi:
                return (lo + hi) / 2.0
            return (a[olo] + a[ohi] + b[olo] + b[ohi]) / 4.0

        auto = []
        for i, j in sorted(pairs):
            a, b = boxes[i], boxes[j]
            for horiz in (True, False):
                lo, hi, olo, ohi = ((0, 2, 1, 3) if horiz
                                    else (1, 3, 0, 2))
                if b[lo] >= a[hi]:
                    d, e0, e1 = b[lo] - a[hi], a[hi], b[lo]
                elif a[lo] >= b[hi]:
                    d, e0, e1 = a[lo] - b[hi], b[hi], a[lo]
                else:
                    continue  # overlapping on this axis: no gap
                if d <= 0:
                    continue  # touching edges measure nothing
                m = cross_mid(a, b, olo, ohi)
                auto.append((e0, m, e1, m) if horiz
                            else (m, e0, m, e1))
        self.rulers.extend(auto)
        self._auto_rulers = auto
        return len(auto)

    def _toggle_snap(self):
        self.snap_on = not self.snap_on
        if not self.snap_on:
            self._snap_res = None
        self._set_live_status(
            "vector snap %s" % ("on" if self.snap_on else "off"))
        self._display()

    def _esc(self):
        """Step out flateyes-style: pending point -> ruler mode ->
        finished rulers -> selection -> markers. EVERYTHING
        ruler-related outranks the selection (field reports): with
        an object selected, Esc first cancels the measurement in
        progress, then clears finished rulers, and only then drops
        the selection."""
        if self.mode == "esel":
            if self._esel_start is not None:
                self._esel_start = None
            else:
                self.mode = "normal"
                self._set_cursor(self._idle_cursor())
        elif self._ruler_start is not None:
            self._ruler_start = None
        elif self.mode == "ruler":
            self.mode = "normal"
            self._snap_res = None
            self._set_cursor(self._idle_cursor())
        elif self.rulers:
            self.rulers = []
            self._auto_rulers = []
            self._drc_ruler = []
        elif self.selection is not None:
            self._clear_selection()
        elif self._drc_sel is not None:
            self._drc_set_sel(None)
            if self._drc_grid_ci is not None:
                self._drc_grid_fill(self._drc_grid_ci)
        elif self._drc_lyr_saved is not None:
            self._drc_restore_layers()
            # restoring isolation ALSO ends the jump (user call
            # 2026-08-18): mark and auto CD rulers go in the same
            # Esc; _drc_pos stays so click-mode n/p continues from
            # the same error
            self.drc_mark = None
            for r in self._drc_ruler:
                if r in self.rulers:
                    self.rulers.remove(r)
            self._drc_ruler = []
        elif self.drc_mark is not None or self._drc_focus is not None:
            self.drc_mark = None
            self._drc_focus = None
        self._display()

    def _copy_view(self):
        """Ctrl+C (user call 2026-08-19): copy the canvas AS SEEN -
        grabbed from the WINDOW (flateyes capture_view) so widget
        overlays ride along: ruler distance chips and design labels
        are Gtk.Labels on the overlay, NOT part of the composed
        pixbuf (field report: lengths missing from copies). NO
        clipboard.store() - a clipboard manager (Exceed TurboX sync
        agent) can drop the image targets on store; the viewer
        serves the selection itself and it empties on quit."""
        pb = None
        win = self.window.get_window()
        if win is not None:
            # the overlay's allocation x/y are NOT toplevel-relative
            # in this nested paned/scroller layout (field report:
            # the whole app window got captured) - translate the
            # canvas origin into toplevel coordinates explicitly
            alloc = self.overlay.get_allocation()
            try:
                ox, oy = self.overlay.translate_coordinates(
                    self.window, 0, 0)
            except (TypeError, ValueError):
                ox = None
            if ox is not None:
                pb = Gdk.pixbuf_get_from_window(
                    win, ox, oy, alloc.width, alloc.height)
        if pb is None:
            pb = self.image.get_pixbuf()   # unmapped fallback
        if pb is None:
            self._set_live_status("nothing to copy yet")
            return
        cb = Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD)
        cb.set_image(pb)
        self._set_live_status(
            "copied  %dx%d" % (pb.get_width(), pb.get_height()))

    def _toggle_overlays(self):
        """Tab (flateyes parity): show/hide every overlay - rulers,
        DRC marks and markers, selections, snap cross - for a clean
        look at the design underneath."""
        self.overlays_on = not self.overlays_on
        self._set_live_status(
            "overlays %s" % ("shown" if self.overlays_on
                             else "hidden (Tab restores)"))
        self._display()

    def _ruler_pop(self):
        """k: delete the most recently created ruler."""
        if self.rulers:
            r = self.rulers.pop()
            if r in self._auto_rulers:
                self._auto_rulers.remove(r)
            if r in self._drc_ruler:
                self._drc_ruler.remove(r)
            self._display()

    def _rulers_clear(self):
        """Shift+K: remove every ruler (and a pending first point)."""
        if self.rulers or self._ruler_start is not None:
            self.rulers = []
            self._auto_rulers = []
            self._drc_ruler = []
            self._ruler_start = None
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
            # rulers accumulate (Calibre): k pops the newest,
            # Shift+K clears all (user call 2026-08-10)
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

    @property
    def selection(self):
        """Primary (most recent) picked object, or None. The full
        multi-selection lives in `selections`; this single-object
        view keeps Esc/reset/deselect-contract code unchanged."""
        return self.selections[-1] if self.selections else None

    @selection.setter
    def selection(self, res):
        self.selections = [] if res is None else [res]

    @staticmethod
    def _sel_key(res):
        """Identity for Ctrl-toggle/dedup: geometry + layer, NOT the
        pick index (the same object reports a different nth/index
        when clicked at a different spot)."""
        pts = res.get("points")
        return (res.get("layer"), res.get("datatype"), res.get("cell"),
                tuple(res.get("bbox") or ()),
                tuple(map(tuple, pts)) if pts else None)

    def _drc_hit_at(self, x, y, r=6):
        """(ci, ei) of the nearest DRC marker painted within r px
        of the screen point, else None (list rebuilt per frame)."""
        best = None
        bd = r * r + 1
        for hx, hy, ci, ei in self._drc_hits:
            d = (hx - x) ** 2 + (hy - y) ** 2
            if d <= r * r and d < bd:
                bd = d
                best = (ci, ei)
        return best

    def _drc_pick(self, ci, ei):
        """Canvas marker SINGLE click = plain grid-number click
        (user call 2026-08-18, rev 2): select the error - grid
        cell, focus, detail - the view does not move. The full
        jump lives on the canvas DOUBLE click (_on_press)."""
        db = self._drc
        e = db.checks[ci].errors[ei]
        if self._drcwin is not None:
            self._drc_goto_cell(ci, ei)
        self._drc_focus = (ci, ei, e.kind,
                           [(x / self.dbu, y / self.dbu)
                            for x, y in e.pts])
        self._drc_show_detail(ci, ei)
        self._display()

    def _pick_click(self, ev):
        self._update_cursor(ev)
        x, y = self._cursor
        state = getattr(ev, "state", 0)
        # a click ON a DRC marker picks that error (plain clicks
        # only - Ctrl/Shift stay design-selection gestures)
        if self._drc is not None and not (
                state & (Gdk.ModifierType.CONTROL_MASK
                         | Gdk.ModifierType.SHIFT_MASK)):
            hit = self._drc_hit_at(ev.x, ev.y)
            if hit is not None:
                self._drc_pick(*hit)
                return
        if state & Gdk.ModifierType.CONTROL_MASK:
            mode = "toggle"  # add unselected / remove selected
        elif state & Gdk.ModifierType.SHIFT_MASK:
            mode = "add"     # extend the multi-selection
        else:
            mode = "replace"
        if mode == "replace":
            tol = 8 * self.spp  # same-spot test in world coords: pan-proof
            if self._pick_px is not None and \
                    abs(x - self._pick_px[0]) <= tol and \
                    abs(y - self._pick_px[1]) <= tol:
                self._pick_nth += 1  # same spot: cycle overlapping objects
            else:
                self._pick_nth = 0
            self._pick_px = (x, y)
        else:
            # modifier clicks always pick the topmost object: cycling
            # would toggle a DIFFERENT overlapped object on the second
            # Ctrl-click at the same spot
            self._pick_px = None
            self._pick_nth = 0
        r = max(1, int(3 * self.spp))
        if self.tiles_spanned((x - r, y - r, x + r, y + r)) > 4:
            # too wide to pick - but a plain click must still honor
            # the documented deselect contract instead of leaving the
            # old selection (and its row highlight) stuck on screen
            if mode == "replace" and self.selection is not None:
                self._clear_selection()
                self._set_live_status("selection cleared")
                self._display()
            else:
                self._set_live_status("zoom in to pick objects")
            return
        self._pick_seq += 1
        self._pick_mode = mode
        self.worker.submit({
            "kind": "pick", "seq": self._pick_seq,
            "x": int(x), "y": int(y), "r": r, "nth": self._pick_nth,
            "layers": self._layers_arg()})

    def _on_pick_result(self, res):
        mode = self._pick_mode
        if not res.get("found"):
            if mode == "replace":
                self._clear_selection()
            self._set_live_status("no object here")
        elif mode == "replace":
            self.selections = [res]
            self._refresh_sel_status()
        else:
            key = self._sel_key(res)
            kept = [s for s in self.selections
                    if self._sel_key(s) != key]
            if mode == "add" or len(kept) == len(self.selections):
                # Shift always selects; Ctrl adds a new object and
                # (the filter above) removes an already selected one
                kept.append(res)
            self.selections = kept
            self._refresh_sel_status()
        self._display()

    def _refresh_sel_status(self):
        """Primary selection changed: row highlight + status text."""
        res = self.selection
        if res is None:
            self._sel_text = ""
            self._set_picked_layer(None)
            self._set_live_status("selection cleared")
            return
        self._set_picked_layer(
            (res["layer"], res["datatype"]),
            {(s["layer"], s["datatype"]) for s in self.selections})
        bb = res["bbox"]
        w = (bb[2] - bb[0]) * self.dbu
        h = (bb[3] - bb[1]) * self.dbu
        n = len(self.selections)
        self._sel_text = ("sel%s %s %d/%d · %s · %.3f x %.3f um @ "
                          "(%.3f, %.3f) · %d/%d"
                          % ("(%d)" % n if n > 1 else "",
                             res["lname"], res["layer"],
                             res["datatype"], res["cell"], w, h,
                             bb[0] * self.dbu, bb[1] * self.dbu,
                             res["index"] + 1, res["count"]))
        self._set_live_status(self._sel_text)

    def _clear_selection(self):
        self.selection = None
        self._sel_text = ""
        # a pick may still be in flight: without this, its late
        # result resurrects the selection the user just dismissed
        # (Esc / row hide), re-expanding and re-scrolling the panel
        self._pick_seq += 1
        self._set_picked_layer(None)

    def _set_picked_layer(self, key, keys=None):
        """Highlight the layers of ALL picked polygons; reveal and
        scroll to `key`, the primary pick's layer."""
        if keys is None:
            keys = {key} if key is not None else set()
        for row_key, row in self._layer_rows.items():
            row.set_picked(row_key in keys)
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

    def _apply_palette_color(self, color):
        """Palette swatch click: recolor every SELECTED layer row -
        row swatch, meta copy, render service, coverage tint, and
        the personal override file (~/.cache/floe)."""
        keys = set(self._selected_layers)
        if not keys:
            self._set_live_status(
                "select a layer row first, then pick a color")
            return
        # a COLLAPSED group parent stands for its whole datatype
        # group: recolor the folded members too (user call
        # 2026-08-11; expanded groups keep per-row control)
        for pkey, children in self._layer_groups.items():
            if pkey in keys and pkey not in self._layer_expanded:
                keys.update(children)
        for key in keys:
            row = self._layer_rows.get(key)
            if row is not None:
                row.set_color(color)
            for l in self.meta["layers"]:
                if (l["layer"], l["datatype"]) == tuple(key):
                    l["color"] = color
        self.worker.submit({
            "kind": "recolor",
            "colors": [[list(k), color] for k in sorted(keys)]})
        # colors are part of the frame identity now: force a fresh
        # render (the covered/preview reuse would keep old pixels)
        self._color_epoch += 1
        self.redraw(immediate=True)

    def _on_fill_slot_click(self, slot, ev):
        if ev.type != Gdk.EventType.BUTTON_PRESS:
            return True
        if ev.button == 3:
            # the bitmap editor is a development tool (the Calibre
            # set is finalized) - hidden unless FLOE_FILL_EDIT=1
            if not os.environ.get("FLOE_FILL_EDIT"):
                return True
            menu = Gtk.Menu()
            item = Gtk.MenuItem(label="edit bitmap\u2026")
            fixed = fillpat.FILL_NAMES[slot] in fillpat.FIXED_FILLS
            item.set_sensitive(not fixed)
            item.connect("activate",
                         lambda _i: self._edit_fill_pattern(slot))
            menu.append(item)
            menu.show_all()
            self._fill_menu = menu  # keep alive while popped up
            if hasattr(menu, "popup_at_pointer"):
                menu.popup_at_pointer(ev)
            else:
                menu.popup(None, None, None, None,
                           ev.button, ev.time)
            return True
        self._apply_fill_slot(slot)
        return True

    def _apply_fill_slot(self, slot):
        """Assign a fill slot to every SELECTED layer row (folded
        group parents cover their members, like colors)."""
        keys = set(self._selected_layers)
        if not keys:
            self._set_live_status(
                "select a layer row first, then pick a fill")
            return
        for pkey, children in self._layer_groups.items():
            if pkey in keys and pkey not in self._layer_expanded:
                keys.update(children)
        for key in keys:
            self._layer_patterns[tuple(key)] = slot
        self._refresh_row_fills()
        self._push_fills()
        self._set_live_status(
            "fill '%s' -> %d layer(s)"
            % (fillpat.FILL_NAMES[slot], len(keys)))

    def _props_rows(self):
        """Current layer table as layerprops rows: color (7x7 name
        or #hex), fill name, layer name, visibility flag."""
        rows = []
        for l in self.meta["layers"]:
            key = (l["layer"], l["datatype"])
            slot = self._layer_patterns.get(key)
            fill = (fillpat.FILL_NAMES[slot]
                    if slot is not None else "speckle")
            rows.append((key, fillpat.color_name(l["color"]),
                         fill, l.get("name") or "",
                         "1" if key in self.visible else "0",
                         str(self._layer_widths.get(key, 1))))
        return rows

    def _apply_props_visibility(self, rows):
        """layerprops second-to-last column: 0=hide, 1=show.
        Batch-apply to rows the design knows (set_active fires the
        toggle handler, which maintains self.visible)."""
        self._layers_batch = True
        try:
            for key, _c, _f, _n, f1, _f2 in rows:
                row = self._layer_rows.get(tuple(key))
                if row is None or f1 not in ("0", "1"):
                    continue
                want = f1 == "1"
                if row.get_active() != want:
                    row.set_active(want)
        finally:
            self._layers_batch = False

    def _selected_with_folded(self):
        """Selection for palette-style actions: a COLLAPSED group
        parent stands for its whole datatype group."""
        keys = set(self._selected_layers)
        for pkey, children in self._layer_groups.items():
            if pkey in keys and pkey not in self._layer_expanded:
                keys.update(children)
        return keys

    def _set_layer_width(self, px):
        """Layer menu: outline width for the selected rows
        (layerprops trailing column; width 1 = the default, so it
        drops out of the map). Session-scoped like the palettes."""
        keys = self._selected_with_folded()
        if not keys:
            self._set_live_status(
                "select a layer row first, then pick a width")
            return
        for key in keys:
            k = tuple(key)
            if px <= 1:
                self._layer_widths.pop(k, None)
            else:
                self._layer_widths[k] = min(8, int(px))
        self._push_fills()
        self._set_live_status(
            "line width %d px -> %d layer(s)" % (px, len(keys)))

    def _step_layer_width(self, delta):
        keys = self._selected_with_folded()
        if not keys:
            self._set_live_status(
                "select a layer row first, then pick a width")
            return
        for key in keys:
            k = tuple(key)
            w = max(1, min(8, self._layer_widths.get(k, 1) + delta))
            if w <= 1:
                self._layer_widths.pop(k, None)
            else:
                self._layer_widths[k] = w
        self._push_fills()
        self._set_live_status(
            "line width %+d -> %d layer(s)" % (delta, len(keys)))

    def _refresh_row_fills(self):
        """Sync every layer row's swatch with its assigned fill."""
        for key, row in getattr(self, "_layer_rows", {}).items():
            slot = self._layer_patterns.get(key)
            row.set_fill(
                self._fill_patterns[slot]
                if slot is not None
                and 0 <= slot < len(self._fill_patterns)
                else None)

    def _push_fills(self):
        """Ship the RESOLVED per-layer bitmaps to the render
        service and force a fresh frame."""
        self.worker.submit({
            "kind": "repattern",
            "fills": [[list(k), self._fill_patterns[v]]
                      for k, v in sorted(
                          self._layer_patterns.items())
                      if 0 <= v < len(self._fill_patterns)],
            "widths": [[list(k), w] for k, w in sorted(
                self._layer_widths.items())]})
        self._color_epoch += 1
        self.redraw(immediate=True)

    def _edit_fill_pattern(self, slot):
        """16x16 bitmap editor popup: click toggles a cell, drag
        paints with the first cell's new value."""
        name = fillpat.FILL_NAMES[slot]
        rows = [list(r) for r in
                self._fill_patterns[slot].split("\n")]
        dlg = Gtk.Dialog(title="fill: %s" % name,
                         transient_for=self.window, modal=True)
        for label, code in (("clear", 10), ("solid", 11),
                            ("invert", 12), ("reset", 13)):
            dlg.add_button(label, code)
        dlg.add_button("Cancel", Gtk.ResponseType.CANCEL)
        dlg.add_button("Apply", Gtk.ResponseType.OK)
        cell = 18
        da = Gtk.DrawingArea()
        da.set_size_request(16 * cell + 1, 16 * cell + 1)
        da.set_halign(Gtk.Align.CENTER)
        paint = {"v": None}

        def draw(_w, cr):
            for y in range(16):
                for x in range(16):
                    on = rows[y][x] == "*"
                    cr.set_source_rgb(*((0.0, 0.0, 0.0) if on
                                        else (1.0, 1.0, 1.0)))
                    cr.rectangle(x * cell, y * cell, cell, cell)
                    cr.fill()
            cr.set_source_rgb(0.6, 0.6, 0.6)
            cr.set_line_width(1)
            for i in range(17):
                cr.move_to(i * cell + 0.5, 0)
                cr.line_to(i * cell + 0.5, 16 * cell)
                cr.move_to(0, i * cell + 0.5)
                cr.line_to(16 * cell, i * cell + 0.5)
            cr.stroke()
            return False

        def cell_at(ev):
            return int(ev.x) // cell, int(ev.y) // cell

        def press(_w, ev):
            x, y = cell_at(ev)
            if 0 <= x < 16 and 0 <= y < 16:
                paint["v"] = "." if rows[y][x] == "*" else "*"
                rows[y][x] = paint["v"]
                da.queue_draw()
            return True

        def motion(_w, ev):
            if paint["v"] is None:
                return False
            x, y = cell_at(ev)
            if 0 <= x < 16 and 0 <= y < 16 \
                    and rows[y][x] != paint["v"]:
                rows[y][x] = paint["v"]
                da.queue_draw()
            return True

        da.add_events(Gdk.EventMask.BUTTON_PRESS_MASK
                      | Gdk.EventMask.BUTTON1_MOTION_MASK
                      | Gdk.EventMask.BUTTON_RELEASE_MASK)
        da.connect("draw", draw)
        da.connect("button-press-event", press)
        da.connect("motion-notify-event", motion)
        da.connect("button-release-event",
                   lambda _w, _e: paint.update(v=None) or True)
        dlg.get_content_area().pack_start(da, True, True, 8)
        dlg.show_all()
        while True:
            r = dlg.run()
            if r == 10:
                rows[:] = [["."] * 16 for _ in range(16)]
            elif r == 11:
                rows[:] = [["*"] * 16 for _ in range(16)]
            elif r == 12:
                rows[:] = [["." if c == "*" else "*" for c in rr]
                           for rr in rows]
            elif r == 13:
                rows[:] = [list(rr) for rr in
                           fillpat.pattern(name).split("\n")]
            else:
                break
            da.queue_draw()
        ok = r == Gtk.ResponseType.OK
        dlg.destroy()
        if not ok:
            return
        self._fill_patterns[slot] = "\n".join(
            "".join(rr) for rr in rows)
        self._fill_slots[slot].queue_draw()
        self._refresh_row_fills()
        if slot in self._layer_patterns.values():
            self._push_fills()

    def _props_chooser(self, save):
        dlg = Gtk.FileChooserDialog(
            title=("save layer properties" if save
                   else "load layer properties"),
            transient_for=self.window,
            action=(Gtk.FileChooserAction.SAVE if save
                    else Gtk.FileChooserAction.OPEN))
        dlg.add_buttons("Cancel", Gtk.ResponseType.CANCEL,
                        "Save" if save else "Open",
                        Gtk.ResponseType.OK)
        flt = Gtk.FileFilter()
        flt.set_name("layerprops (*.layerprops)")
        flt.add_pattern("*.layerprops")
        dlg.add_filter(flt)
        allf = Gtk.FileFilter()
        allf.set_name("all files")
        allf.add_pattern("*")
        dlg.add_filter(allf)
        src = os.path.abspath(self.cache.src)
        dlg.set_current_folder(os.path.dirname(src))
        if save:
            dlg.set_do_overwrite_confirmation(True)
            dlg.set_current_name(
                os.path.basename(src) + ".layerprops")
        ok = dlg.run() == Gtk.ResponseType.OK
        path = dlg.get_filename() if ok else None
        dlg.destroy()
        return path

    def _load_props_dialog(self):
        """Layer menu: apply a Calibre .layerprops file to the
        session (colors, fill assignments, visibility and outline
        width; rows for layers the design lacks are skipped,
        unlisted layers keep their state). Session-scoped: persist
        via "save layer properties..." or the design-default
        publish."""
        path = self._props_chooser(save=False)
        if not path:
            return
        try:
            with open(path) as fh:
                rows = fillpat.parse_layerprops(fh.read())
        except OSError as e:
            self._set_live_status(
                "layer properties load failed: %s" % e)
            return
        known = set(self._layer_rows)
        recolors = {}
        nfill = 0
        for key, color, fill, _name, _f1, _f2 in rows:
            key = tuple(key)
            if key not in known:
                continue
            c = fillpat.color_hex(color)
            if c:
                for l in self.meta["layers"]:
                    if (l["layer"], l["datatype"]) == key:
                        l["color"] = c
                row = self._layer_rows.get(key)
                if row is not None:
                    row.set_color(c)
                recolors[key] = c
            i = fillpat.fill_index(fill)
            if i is not None:
                self._layer_patterns[key] = i
                nfill += 1
            try:
                w = int(_f2)
                if w > 1:
                    self._layer_widths[key] = w
                elif key in self._layer_widths:
                    del self._layer_widths[key]
            except ValueError:
                pass
        self._apply_props_visibility(rows)
        if not recolors and not nfill:
            self._set_live_status(
                "no matching layers in %s" % path)
            self.redraw(immediate=True)  # visibility may have changed
            return
        if recolors:
            self.worker.submit({
                "kind": "recolor",
                "colors": [[list(k), v] for k, v
                           in sorted(recolors.items())]})
        self._refresh_row_fills()
        self._push_fills()  # repattern + epoch bump + redraw
        self._set_live_status(
            "layer properties loaded: %s (%d colors, %d fills)"
            % (path, len(recolors), nfill))

    def _save_props_dialog(self):
        """Layer menu: export the current layer table as a Calibre
        .layerprops file wherever the user points."""
        path = self._props_chooser(save=True)
        if not path:
            return
        try:
            with open(path, "w") as fh:
                fh.write(
                    fillpat.format_layerprops(self._props_rows()))
            self._set_live_status(
                "layer properties saved: %s" % path)
        except OSError as e:
            self._set_live_status(
                "layer properties save failed: %s" % e)

    def _publish_default_colors(self):
        """Publish the CURRENT layer table as the design-default
        Calibre layerprops next to the source (<file>.layerprops):
        anyone opening the design with no personal palette adopts
        it - and it seeds their personal cache on first open."""
        try:
            path = cache_mod.save_shared_props(
                self.cache.src,
                fillpat.format_layerprops(self._props_rows()))
            self._set_live_status(
                "design default layerprops saved: %s" % path)
        except OSError as e:
            self._set_live_status(
                "default layerprops save failed: %s" % e)

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
        elif event.type == Gdk.EventType.BUTTON_PRESS \
                and self._selected_layers == {key}:
            # plain click on the sole selected row deselects it
            # (double-click presses re-select via their 2BP/3BP
            # event, so a visibility toggle still ends selected)
            self._set_layer_selection(set())
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
        menu.append(Gtk.SeparatorMenuItem())
        wsub = Gtk.Menu()
        wroot = Gtk.MenuItem(label="line width")
        wroot.set_sensitive(has_selection)
        wroot.set_submenu(wsub)
        for px in (1, 3, 5):
            it = Gtk.MenuItem(label="%d px" % px)
            it.connect("activate",
                       lambda _i, p=px: self._set_layer_width(p))
            wsub.append(it)
        wsub.append(Gtk.SeparatorMenuItem())
        for label, d in (("increase", 1), ("decrease", -1)):
            it = Gtk.MenuItem(label=label)
            it.connect("activate",
                       lambda _i, dd=d: self._step_layer_width(dd))
            wsub.append(it)
        menu.append(wroot)
        menu.append(Gtk.SeparatorMenuItem())
        add_item("load layer properties\u2026",
                 self._load_props_dialog)
        add_item("save layer properties\u2026",
                 self._save_props_dialog)
        # dev-only, like the fill bitmap editor: publishing the design
        # default next to the source stays hidden from end users
        if os.environ.get("FLOE_FILL_EDIT"):
            add_item("save colors+fills as design default",
                     self._publish_default_colors)
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
            if self.selections:
                # hiding a layer drops only ITS objects from the
                # multi-selection
                kept = [s for s in self.selections
                        if (s.get("layer"), s.get("datatype")) != key]
                if not kept:
                    self._clear_selection()
                elif len(kept) != len(self.selections):
                    self.selections = kept
                    self._refresh_sel_status()
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
               detail=None, dump=False, depth=None, lod=DEFAULT_LOD,
               frames=DEFAULT_FRAMES, labels=DEFAULT_LABELS,
               stream_kb=None, stream_target_ms=500,
               render_debug=False):
    import_gtk()
    viewer = Viewer(cache, server_sock, goto=goto, detail=detail,
                    dump=dump, depth=depth, lod=lod, frames=frames,
                    labels=labels, stream_kb=stream_kb,
                    stream_target_ms=stream_target_ms,
                    render_debug=render_debug)
    if drc:
        # NO _drc_window() here: its grab_focus made the BROWSE
        # TreeView auto-select row 0 on focus-in, so --drc startups
        # showed the first rule selected (field reports 2026-08-18;
        # a db must open with no rule selected). The embedded panel
        # is always visible - there is nothing to focus.
        viewer.load_drc(os.path.abspath(drc))
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
