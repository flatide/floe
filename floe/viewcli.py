"""`floe2 view`: the GTK viewer's command line (docs/SHARED_APP_LAYER.ko.md
P2d). The Rust `floe2 view` starts it (`python -m floe.gtkview`), and
`floe2 gtktest`, the GTK display check. Every decision about a source -
whether it opens as it is, its cache, a deck's plan - is floe2
gtk-service's (floe/gtkservice.py); this parses the options, forwards to a
running window or starts one."""

import argparse
import os
import sys
import time

from .product import name as product_name

APP = product_name()


def parse_goto(s):
    """--goto X,Y[,WINDOW] in um -> [x, y] or [x, y, window]."""
    try:
        vals = [float(t) for t in s.replace(",", " ").split()]
    except ValueError:
        vals = []
    if len(vals) not in (2, 3):
        raise SystemExit(f"floe2: invalid --goto {s!r}, expected X,Y[,WINDOW] in um")
    if len(vals) == 3 and vals[2] <= 0:
        raise SystemExit("floe2: --goto WINDOW must be > 0 (um)")
    return vals


def _id_list(value):
    out = []
    for tok in value.split(","):
        tok = tok.strip()
        if not tok:
            continue
        try:
            out.append(int(tok))
        except ValueError:
            raise argparse.ArgumentTypeError(
                "identifier list must be integers, got %r" % tok)
    if not out:
        raise argparse.ArgumentTypeError("identifier list is empty")
    return out


def _add_thin_option(p):
    """The page hairline policy (review 2026-09-11): every source keeps
    all-thin pages as 1 px hairlines by default (a plain layout too
    since 2026-09-23, user decision; it used to cull them at wide views
    for speed) - --thin cull asks for the former layout policy."""
    p.add_argument("--thin", choices=("auto", "keep", "cull"),
                   default=None,
                   help="thin shapes at wide views: auto = keep (every "
                        "source since 0.12.199; a layout used to cull); "
                        "keep = all-thin pages stay as 1 px hairlines; "
                        "cull = drop them (faster at wide views)")


def _add_level_option(p):
    """Commands that open a jobdeck take a mask-level selection (user
    call 2026-09-10, Calibre-style partial load): only those levels'
    placements are planned, indexed and drawn."""
    p.add_argument("--level", dest="level", type=_id_list, default=None,
                   metavar="N[,N...]",
                   help="jobdeck: load only these mask levels ($n; "
                        "default: all - the viewer asks, like Calibre)")


def _add_reviewer_option(p):
    """Commands that open DRC packs (view/drc/render) take the
    reviewer tag as a parameter: the shared server account cannot
    distinguish reviewers, and launcher scripts prefer an explicit
    argument over exporting FLOE_REVIEWER (which stays honored)."""
    p.add_argument("--floe-reviewer", default=None, metavar="NAME",
                   help="reviewer tag for the per-reviewer waive "
                        "autosave next to the DRC pack (overrides "
                        "the DISPLAY/SSH-derived tag; sets "
                        "FLOE_REVIEWER - the env var still works)")


def _is_deck(src):
    return bool(src) and str(src).lower().endswith(".jb")


def _refuse_busy(busy):
    """End the run for a busy target: one line, as floe-index says it,
    and 75."""
    print("[lock] %s" % busy, file=sys.stderr, flush=True)
    raise SystemExit(75)


def _cache_ready(src, ids=None):
    """A source that opens as it is: a current layout cache, or a deck
    whose drawable sources are all indexed (floe2 gtk-service)."""
    from .gtkservice import ServiceError, readiness
    try:
        return bool(readiness(src, ids)["current"])
    except ServiceError:
        return False


def _service_open(src):
    """The start layout, opened by floe2 gtk-service (a refusal ends the
    run)."""
    from .gtkservice import ServiceCache, ServiceError
    c = ServiceCache(src)
    try:
        c.load()
    except ServiceError as exc:
        if exc.kind == "busy":
            _refuse_busy(exc)
        raise SystemExit("floe2: %s" % exc)
    if c.is_stale():
        print("[floe2][warn] cache is outdated (source changed); "
              "rebuild: floe2 index --force", file=sys.stderr)
    return c


def cmd_view(args):
    # src is optional (user call 2026-08-22): no file = empty
    # viewer, attach one later via File > load layout… (or a
    # forwarded `floe view <file>`)
    src = None
    if args.src:
        src = os.path.abspath(args.src)
        if not os.path.isfile(src):
            raise SystemExit(f"floe2: no such file: {src}")
    # documented entry for the frame-tuning knobs: the flags set
    # the env vars vfsclient reads per request (children inherit;
    # a forwarded running instance keeps its own values)
    if args.hairline is not None:
        os.environ["FLOE_HAIRLINE"] = "%g" % args.hairline
    if args.thin_um is not None:
        os.environ["FLOE_THIN_UM"] = "%g" % args.thin_um
    goto = parse_goto(args.goto) if args.goto else None
    if args.stream_kb is not None and args.stream_kb < 0:
        raise SystemExit("floe2: --stream-kb must be >= 0")
    if not 100 <= args.stream_target_ms <= 2000:
        raise SystemExit("floe2: --stream-target-ms must be 100..2000")
    if not 6 <= args.label_font_px <= 96:
        raise SystemExit("floe2: --label-font-px must be 6..96")

    # A baseline comparison must select the same work in floe/KLayout and
    # floe2/Rust.  Keep geometry/detail/depth explicit, but remove optional
    # staging, approximation and presentation work.  Page/working-set caches
    # stay enabled: cold vs warm cache behavior is itself part of the product.
    stream_kb = args.stream_kb
    if args.perf_baseline:
        args.frames = "off"
        args.labels = "off"
        args.refinement = "off"
        args.frame_cache = "off"
    if args.refinement == "off":
        if stream_kb not in (None, 0):
            raise SystemExit(
                "floe2: --refinement off conflicts with nonzero --stream-kb")
        stream_kb = 0
    elif stream_kb == 0:
        # Preserve the established stable-floe spelling.  Rust maps the same
        # construction value to a single all-miss batch.
        args.refinement = "off"

    # Resolve startup view policy before the single-instance branch so a
    # forwarded request and a newly constructed Viewer receive exactly the
    # same detail/depth.  In particular, --goto without an explicit depth is
    # a full-depth inspection in both paths.
    detail_name = args.detail or "medium"
    detail = ("low", "medium", "high").index(detail_name)
    depth = args.depth
    if depth is None:
        # a jobdeck opens at full depth: the deck itself is the thing
        # viewed, and a source whose shapes live in child cells drew
        # nothing at depth 0 (review 2026-09-09 P1-3)
        depth = 999 if (goto is not None or args.drc
                        or _is_deck(src)) else 0
    depth = max(0, min(999, int(depth)))

    # Render-process construction parameters cannot be retrofitted into
    # a running single instance. Open an independent viewer when one is
    # explicitly supplied; live request controls are forwarded below.
    process_options = (stream_kb is not None
                       or args.stream_target_ms != 500
                       or args.render_debug
                       or args.frame_cache == "off"
                       or args.margin == "on")
    server = None
    if not args.multi and not process_options:
        # flateyes-style single instance per (uid, DISPLAY)
        from . import instance
        display = instance.display_key()
        if display is None:
            print("floe2: DISPLAY is not set", file=sys.stderr)
            raise SystemExit(1)
        # a file without an index is forwarded as is: the running
        # window asks the user and builds the index in its log dialog
        # (user call 2026-09-09; it used to fail here in the terminal)
        addr = instance.socket_address(display)
        # no src: an empty path forwards as a present-only request
        # (raise the running window; open nothing)
        request = src or ""
        if goto is not None:
            # repr() round-trips floats exactly, unlike %g
            request += "\tgoto=" + ",".join(repr(v) for v in goto)
        request += ("\tdetail=%s\tdepth=%d\tframes=%s"
                    "\tlabels=%s\tlabelpx=%d" % (
                        detail_name, depth, args.frames,
                        args.labels, args.label_font_px))
        levels = getattr(args, "level", None)
        if levels:
            request += "\tlevels=" + ",".join(str(i) for i in levels)
        # an explicit --thin (auto included) reaches the running window
        # (review 2026-09-11 P2-3: auto was dropped, so a window left on
        # keep could not be told to return to its default)
        if getattr(args, "thin", None) is not None:
            request += "\tthin=" + args.thin
        # an explicit --density reaches the running window (2026-10-05)
        if getattr(args, "density", None) is not None:
            request += "\tdensity=" + args.density
        for _ in range(5):
            code = instance.try_forward(addr, request)
            if code is not None:
                raise SystemExit(code)
            server = instance.try_bind(addr)
            if server is not None:
                break
            time.sleep(0.2)
        if server is None:
            print("floe2: could not create or reach the instance socket",
                  file=sys.stderr)
            raise SystemExit(1)
        if not addr.startswith("\0"):
            import atexit
            atexit.register(lambda: os.path.exists(addr) and os.unlink(addr))

    # no index yet: the viewer starts empty, asks, indexes and opens
    # (the request options - goto included - apply after the open)
    pending_open = None
    pending_fields = ()
    levels = getattr(args, "level", None)
    thin_mode = getattr(args, "thin", None) or "auto"
    # a jobdeck always opens through the window (user call 2026-09-10:
    # like Calibre it asks which mask levels to load first, unless
    # --level or FLOE_JOBDECK_LEVELS says; then indexes what those
    # levels need, then opens)
    if src and (_is_deck(src) or not _cache_ready(src)):
        pending_open = src
        pending_fields = tuple(
            (["goto=" + ",".join(repr(v) for v in goto)] if goto else [])
            + ["detail=%s" % detail_name, "depth=%d" % depth,
               "frames=%s" % args.frames,
               "labels=%s" % args.labels,
               "labelpx=%d" % args.label_font_px]
            + (["levels=" + ",".join(str(i) for i in levels)]
               if levels else [])
            + (["thin=" + thin_mode] if thin_mode != "auto" else [])
            + (["density=" + args.density]
               if getattr(args, "density", None) is not None else []))
        c = None
        goto = None
    elif src and APP == "floe2":
        c = _service_open(src)
    else:
        c = _service_open(src) if src else None
    # PyGObject/GTK3 problems are reported inside import_gtk (exit 3)
    from .gui import run_viewer
    run_viewer(c, server, goto=goto, drc=args.drc,
               detail=detail, dump=args.dump, depth=depth,
               frames=args.frames == "on",
               labels=args.labels == "on",
               label_font_px=args.label_font_px,
               frame_cache=args.frame_cache == "on",
               margin=args.margin == "on",
               stream_kb=stream_kb,
               stream_target_ms=args.stream_target_ms,
               render_debug=args.render_debug,
               pending_open=pending_open, pending_fields=pending_fields,
               thin=thin_mode,
               density=(None if getattr(args, "density", None) is None
                        else args.density == "on"))


def parser():
    p = argparse.ArgumentParser(
        prog="floe2 view",
        description="native desktop viewer (GTK3); one instance per (uid, "
                    "DISPLAY) - later calls forward the path to it")
    p.add_argument("src", nargs="?", default=None,
                   help="OASIS source (omit to start empty and use "
                        "File > load layout…)")
    _add_level_option(p)
    p.add_argument("--multi", action="store_true",
                   help="always open an independent window (skip the "
                        "single-instance socket)")
    p.add_argument("--goto", default=None, metavar="X,Y[,W]",
                   help="start centered on X,Y (um) with an X marker; "
                        "W = view width in um (omitted = fit view). "
                        "Forwarded to a running instance too.")
    p.add_argument("--drc", default=None, metavar="FILE.db",
                   help="preload a Calibre ASCII DRC results db and "
                        "open the error browser (new instance only; "
                        "a fresh pack (.FILE.db.tray) built by "
                        "'floe-index drc' is used automatically)")
    detail_help = (
        "starting detail level (default: medium; higher = finer, heavier "
        "wide views - lower levels omit finer features below the cut). "
        "The `d` dialog changes it at runtime; the px thresholds behind "
        "the levels are internal and may be retuned. Forwarded to a "
        "running instance")
    p.add_argument("--detail", default=None,
                   choices=("low", "medium", "high"), help=detail_help)
    p.add_argument("--depth", type=int, default=None, metavar="N",
                   help="starting hierarchy depth (999 = full). "
                        "Default: 0 for a plain open - top geometry "
                        "plus child outline frames, the fastest "
                        "truthful first paint - and full when "
                        "--goto jumps to an inspection point. "
                        "Digits / the `d` dialog change it at runtime. "
                        "Forwarded to a running instance")
    _add_thin_option(p)
    if True:
        # the density under the cut (user 2026-10-05: "a density on/off
        # option in the viewer"): the Rust renderer's density stack
        p.add_argument("--density", choices=("on", "off"), default=None,
                       help="start with the density under the cut on or off "
                            "- the shapes the detail's cut drops, drawn by "
                            "the area they cover; View > density under the "
                            "cut (`v`) switches it live. Default: on when "
                            "FLOE_RUST_DENSITY_STACK=top, else off. "
                            "Forwarded to a running instance")
    p.add_argument("--refinement", choices=("on", "off"), default="on",
                   help="publish progressive intermediate frames (default "
                        "on); off waits for one settled frame in both floe "
                        "and floe2 and opens an independent instance")
    p.add_argument("--frame-cache", choices=("on", "off"), default="on",
                   help="allow settled-frame reuse: retained-frame pan "
                        "reuse and the background margin prefetch (default "
                        "on; Rust renderer only); off is useful for "
                        "backend-neutral render timing and opens an "
                        "independent instance")
    p.add_argument("--margin", choices=("on", "off"), default="off",
                   help="the background margin prefetch alone (default off, "
                        "user decision 2026-09-27; Rust renderer only): every "
                        "pan and zoom renders a viewport frame, retained-frame "
                        "pan reuse stays; on prefetches a 2x margin behind "
                        "each settled frame and opens an independent "
                        "instance")
    p.add_argument("--perf-baseline", action="store_true",
                   help="backend-neutral timing preset: refinement, frame "
                        "reuse/margin prefetch, LOD, hierarchy frames and "
                        "labels off; detail and depth remain explicit")
    p.add_argument("--frames", choices=("on", "off"), default="on",
                   help="starting hierarchy FRAME_LAYER state (default "
                        "on; the viewer button/`h` changes it live)")
    p.add_argument("--labels", choices=("on", "off"), default="on",
                   help="enable request-scoped design text and block-name "
                        "planning (default on; forwarded to a running "
                        "instance)")
    p.add_argument("--label-font-px", type=int, default=14, metavar="PX",
                   help="bundled Rust renderer label size, 6..96 screen "
                        "pixels (default 14; forwarded to a running "
                        "instance and adjustable from the View menu)")
    p.add_argument("--stream-kb", type=int, default=None, metavar="KB",
                   help="pin progressive payload per round in KiB; 0 "
                        "disables streaming (default: adaptive from 24576; "
                        "opens an independent instance)")
    p.add_argument("--stream-target-ms", type=int, default=500,
                   metavar="MS",
                   help="adaptive refinement round target, 100..2000 ms "
                        "(default 500; a non-default value opens an "
                        "independent instance)")
    p.add_argument("--render-debug", action="store_true",
                   help="print per-round VFS render metrics to stderr "
                        "in an independent instance")
    p.add_argument("--hairline", type=float, default=None, metavar="F",
                   help="hairline factor: frame min-side cut = F x cut "
                        "(default: daemon 0.5; 0 disables the hairline "
                        "cut. Sets FLOE_HAIRLINE - the env var still "
                        "works; new instance only)")
    p.add_argument("--thin-um", type=float, default=None, metavar="UM",
                   help="thin-frame lattice pitch in um (default: "
                        "daemon 7.0; 0 restores the plain cull. Sets "
                        "FLOE_THIN_UM - the env var still works; new "
                        "instance only)")
    p.add_argument("--dump", action="store_true",
                   help="save display-path debug dumps to /tmp/%s_*.png "
                        "(XQuartz black-view diagnosis; new instance only)"
                        % "floe2")
    _add_reviewer_option(p)
    return p


def gtktest(argv):
    """`floe2 gtktest [PNG]`: a minimal pixbuf-display matrix for
    diagnosing a black view. Three panels: (a) pixbuf loaded from a PNG
    file, (b) pixbuf synthesized in memory the way the viewer composes
    frames, (c) the synthesized pixbuf inside the viewer's
    Overlay/ScrolledWindow containment. Report which panels show content."""
    ap = argparse.ArgumentParser(
        prog="floe2 gtktest",
        description="minimal pixbuf display test (diagnoses a black view)")
    ap.add_argument("png", nargs="?", default=None,
                    help="optional PNG to show as the from-file panel")
    args = ap.parse_args(argv)
    from . import gui as g
    g.import_gtk()
    Gtk, GdkPixbuf = g.Gtk, g.GdkPixbuf
    print("[gtktest] GTK %d.%d.%d" % (Gtk.MAJOR_VERSION, Gtk.MINOR_VERSION,
                                      Gtk.MICRO_VERSION))

    def synth():
        pb = GdkPixbuf.Pixbuf.new(GdkPixbuf.Colorspace.RGB, False, 8,
                                  360, 160)
        pb.fill(0x000000FF)
        for i, col in enumerate((0xFF3333FF, 0x33FF33FF, 0x3333FFFF,
                                 0xFFFF33FF)):
            g.fill_rect(pb, 20 + i * 85, 30, 70, 100, col)
        return pb

    win = Gtk.Window(title="floe2 gtktest")
    win.connect("delete-event", Gtk.main_quit)
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
    win.add(box)
    box.pack_start(Gtk.Label(label="(text) if you can read this, "
                             "widget/text rendering works"),
                   False, False, 4)

    def panel(title, widget):
        box.pack_start(Gtk.Label(label=title), False, False, 0)
        box.pack_start(widget, False, False, 0)

    if args.png and os.path.isfile(args.png):
        img_a = Gtk.Image()
        img_a.set_from_pixbuf(
            GdkPixbuf.Pixbuf.new_from_file(args.png)
            .scale_simple(360, 160, GdkPixbuf.InterpType.BILINEAR))
        panel("(a) pixbuf loaded from file:", img_a)
    img_b = Gtk.Image()
    img_b.set_from_pixbuf(synth())
    panel("(b) pixbuf synthesized in memory (4 color bars):", img_b)
    overlay = Gtk.Overlay()
    sc = Gtk.ScrolledWindow()
    sc.set_policy(Gtk.PolicyType.AUTOMATIC, Gtk.PolicyType.AUTOMATIC)
    img_c = Gtk.Image()
    img_c.set_halign(Gtk.Align.START)
    img_c.set_valign(Gtk.Align.START)
    img_c.set_from_pixbuf(synth())
    sc.add(img_c)
    overlay.add(sc)
    overlay.set_size_request(380, 170)
    panel("(c) same bars inside Overlay+ScrolledWindow (viewer's tree):",
          overlay)
    win.show_all()
    print("[gtktest] window up - report which of (a)/(b)/(c) show "
          "content; close the window to exit")
    Gtk.main()


def main(argv=None):
    """`floe2 view ARGS`; `floe2 gtktest [PNG]` (the Rust floe2 passes a
    word it does not know to this entry as it is)."""
    argv = sys.argv[1:] if argv is None else list(argv)
    if argv[:1] == ["gtktest"]:
        return gtktest(argv[1:])
    backend = os.environ.get("FLOE_RENDERER", "rust").strip().lower() or "rust"
    if backend != "rust":
        raise SystemExit("floe2 is Rust-only; FLOE_RENDERER=%r is not "
                         "supported" % backend)
    os.environ["FLOE_RENDERER"] = "rust"
    args = parser().parse_args(argv)
    reviewer = getattr(args, "floe_reviewer", None)
    if reviewer is not None:
        reviewer = reviewer.strip()
        if not reviewer:
            raise SystemExit("floe2: --floe-reviewer must not be empty")
        os.environ["FLOE_REVIEWER"] = reviewer
    # what this viewer's readers say they are in the cache locks
    os.environ.setdefault("FLOE_LOCK_WHAT", "floe2 view")
    return cmd_view(args)
