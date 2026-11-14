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
    """The viewer takes a review user ID: a shared server account cannot
    distinguish reviewers, and launcher scripts prefer an explicit
    argument over exporting FLOE_REVIEWER (which stays honored)."""
    p.add_argument("--userid", "--floe-reviewer", dest="floe_reviewer",
                   default=None, metavar="ID",
                   help="user ID for DRC review, waive and note files "
                        "(sets FLOE_REVIEWER and overrides automatic "
                        "DISPLAY/SSH identification; default: existing "
                        "identification - the DISPLAY host, shared by "
                        "everyone on it; FLOE_REVIEWER_REQUIRED=1 refuses a "
                        "review without an ID; one ID reviews a result in "
                        "one session). Forwarded to a running instance, "
                        "which opens the results that follow as this ID's "
                        "review")


def file_load_off(args, environ=None):
    """Whether this viewer is to load no file but its command line's (user
    2026-11-11: a launcher that starts everyone's viewer under one account
    - TeeBox - must not let a user open whatever that account can read):
    `--no-file-load`, or FLOE_NO_FILE_LOAD set to `1`, `on`, `yes` or
    `true` in the launcher's environment (the words FLOE_REVIEWER_REQUIRED
    takes)."""
    environ = os.environ if environ is None else environ
    word = (environ.get("FLOE_NO_FILE_LOAD") or "").strip().lower()
    return bool(getattr(args, "no_file_load", False)) or word in (
        "1", "on", "yes", "true")


def confirmer_on(args, environ=None):
    """Whether this viewer is started to confirm the team's waives (user
    2026-11-13: "사용자중 팀의 모든 waive를 뷰어에서 보고 confirm하는 역할이
    필요함"): `--confirmer`, or FLOE_CONFIRMER set to `1`, `on`, `yes` or
    `true` in the launcher's environment. A role the launcher gives, as it
    gives the user ID - the viewer does not ask who the user is."""
    environ = os.environ if environ is None else environ
    word = (environ.get("FLOE_CONFIRMER") or "").strip().lower()
    return bool(getattr(args, "confirmer", False)) or word in (
        "1", "on", "yes", "true")


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
    rules = getattr(args, "rules", None)
    if rules is not None:
        if not args.drc:
            raise SystemExit("floe2: --rules requires --drc")
        rules = os.path.abspath(rules)
        if not os.path.isfile(rules):
            raise SystemExit(f"floe2: no such rules metadata file: {rules}")
    # the results are said to be missing here, where the launcher reads it:
    # a request handed to a running window answers before that window has
    # tried to load them
    drc = args.drc
    if drc:
        drc = os.path.abspath(drc)
        if not os.path.isfile(drc):
            raise SystemExit(f"floe2: no such DRC results file: {drc}")
    # the rule, and the error of it, to select in the results as they open
    # (user 2026-11-12: "오픈과 동시에 특정 룰이 선택되게 하거나 특정 룰의 특정 에러가
    # 선택되게") - the names a DRC snapshot takes (floe2 render --drc DB
    # --drc-rule NAME --drc-err N). Whether the results have the rule and the
    # error is known once they are open: the window says so then.
    drc_rule = getattr(args, "drc_rule", None)
    drc_err = getattr(args, "drc_err", None)
    if drc_rule is not None:
        if not drc:
            raise SystemExit("floe2: --drc-rule requires --drc")
        if not drc_rule.strip():
            raise SystemExit("floe2: --drc-rule must name a rule")
    if drc_err is not None:
        if drc_rule is None:
            raise SystemExit("floe2: --drc-err requires --drc-rule")
        if drc_err < 1:
            raise SystemExit("floe2: --drc-err is an error's number in its "
                             "rule's list, from 1")
        if args.goto:
            raise SystemExit("floe2: --goto and --drc-err both place the "
                             "view - give one of them")
    # the layer properties file for the layout this launch opens (user
    # 2026-11-11: "layer property 파일도 실행 옵션으로 지정할 수 있게 해줘" -
    # a launcher's viewer has its file loads off, and a design default
    # cannot be put beside a source in a folder that is not to be written):
    # a Calibre .layerprops file, applied as Layer > load layer properties…
    # applies one. A link is read through - the file is this command line's
    # own (the service reads no link by itself)
    props = getattr(args, "layerprops", None)
    if props is not None:
        if not src:
            raise SystemExit("floe2: --layerprops requires a layout or a jobdeck")
        props = os.path.abspath(props)
        if not os.path.isfile(props):
            raise SystemExit(f"floe2: no such layer properties file: {props}")
        props = os.path.realpath(props)
    # documented entry for the frame-tuning knobs: the flags set
    # the env vars vfsclient reads per request (children inherit;
    # a forwarded running instance keeps its own values)
    if args.hairline is not None:
        os.environ["FLOE_HAIRLINE"] = "%g" % args.hairline
    if args.thin_um is not None:
        os.environ["FLOE_THIN_UM"] = "%g" % args.thin_um
    # a layout's default layer colours (user 2026-10-13: row = every row
    # of the layer panel its own colour, group = one per layer number):
    # the service this viewer starts opens layouts by FLOE_LAYER_COLORS
    # (app-core styles::LayerColors), so the flag sets it - a choice made
    # when a layout opens, so it opens a window of its own
    # (process_options; a running one keeps its colours)
    layer_colors = getattr(args, "layer_colors", None)
    if layer_colors is not None:
        os.environ["FLOE_LAYER_COLORS"] = layer_colors
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
    # The DRC results, the rules metadata, the reviewer and the file loads
    # turned off are part of the request since 0.12.446 (user 2026-11-11:
    # "이미 떠있는 floe 창을 이용하는게 좋을 것 같음 ... DISPLAY가 동일할 경우
    # 전환하는 flateyes 방식"): the running window opens the results as the
    # stated reviewer's and turns its own file loads off (floe/gui.py
    # _on_incoming), so a launcher's --drc --userid --no-file-load launches
    # share one window - one viewer, one service and one renderer a display
    # - where each was a set of its own. A review loses nothing by the
    # switch: every waive, visit, exclusion and note is on the disk when it
    # is made. --multi still opens a window of its own.
    no_load = file_load_off(args)
    confirmer = confirmer_on(args)
    # the reviewer this launch states (main() put --userid in the
    # environment): sent along - a window serves the request under it
    stated = (os.environ.get("FLOE_REVIEWER") or "").strip()
    # (the request is one line of tab-separated fields)
    unsendable = any(c in text for text in (src or "", drc or "", rules or "", stated,
                                            props or "", drc_rule or "")
                     for c in "\t\r\n")
    process_options = (stream_kb is not None
                       or args.stream_target_ms != 500
                       or args.render_debug
                       or args.frame_cache == "off"
                       or args.margin == "on"
                       or layer_colors is not None
                       or unsendable)
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
        # an explicit --occupancy too (2026-10-20)
        if getattr(args, "occupancy", None) is not None:
            request += "\toccupancy=" + args.occupancy
        # the review this launch asks for (0.12.446): the results, the rules
        # metadata named for them, whose review it is, and the file loads
        # turned off - the window that takes the request turns its own off
        if drc:
            request += "\tdrc=" + drc
        if rules:
            request += "\trules=" + rules
        # the rule, and the error of it, to select in them (0.12.461)
        if drc_rule:
            request += "\tdrcrule=" + drc_rule
            if drc_err is not None:
                request += "\tdrcerr=%d" % drc_err
        if stated:
            request += "\tuserid=" + stated
        if no_load:
            request += "\tnoload=1"
        # the results are opened as a confirmer's (--confirmer)
        if confirmer:
            request += "\tconfirmer=1"
        # the layout's layer properties file: the window applies it to the
        # layout the request opens, or to the one it shows when that is it
        if props:
            request += "\tlayerprops=" + props
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

    # this process is the window: a layer properties file that cannot be
    # read ends the launch here, said where the launcher reads it (a running
    # window answers a forwarded request the same way, gui.py _on_incoming)
    if props:
        from .gtkservice import ServiceError, layerprops_read
        try:
            layerprops_read(props)
        except (OSError, ServiceError) as exc:
            raise SystemExit("floe2: layer properties file %s: %s" % (props, exc))
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
               if getattr(args, "density", None) is not None else [])
            + (["occupancy=" + args.occupancy]
               if getattr(args, "occupancy", None) is not None else [])
            + (["layerprops=" + props] if props else []))
        c = None
        goto = None
    elif src and APP == "floe2":
        c = _service_open(src)
    else:
        c = _service_open(src) if src else None
    # PyGObject/GTK3 problems are reported inside import_gtk (exit 3)
    from .gui import run_viewer
    run_viewer(c, server, goto=goto, drc=drc, rules=rules,
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
                        else args.density == "on"),
               occupancy=(None if getattr(args, "occupancy", None) is None
                          else args.occupancy == "on"),
               file_load=not no_load,
               confirmer=confirmer,
               layerprops=props if c is not None else None,
               drc_pick=(drc_rule, drc_err) if drc_rule else None)


def parser():
    p = argparse.ArgumentParser(
        prog="floe2 view",
        description="native desktop viewer (GTK3); one instance per (uid, "
                    "DISPLAY) - later calls hand their request (the layout, "
                    "--layerprops, --drc, --rules, --drc-rule, --drc-err, "
                    "--userid, --no-file-load, --confirmer "
                    "and the view options) to it and the window switches; "
                    "--multi opens a window of its own",
        epilog="--workdir DIR (every floe2 command, anywhere on the line; "
               "FLOE_WORKDIR): the caches are read and written under DIR at "
               "the source's absolute path, not beside the source; floe2 "
               "takes it before this entry")
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
                   help="load a Calibre ASCII DRC results db and open "
                        "the error browser (a fresh pack (.FILE.db.tray) "
                        "built by 'floe-index drc' is used automatically). "
                        "Forwarded to a running instance, which opens it in "
                        "place of the results it shows - their review is on "
                        "the disk as it stands, nothing is asked; results "
                        "already open there only bring the window forward")
    p.add_argument("--rules", default=None, metavar="FILE.rules.json",
                   help="load existing SVRF rule metadata with --drc "
                        "(overrides automatic rules lookup; does not "
                        "index an SVRF deck). Forwarded with --drc")
    p.add_argument("--drc-rule", dest="drc_rule", default=None, metavar="NAME",
                   help="with --drc: open the results with this rule "
                        "selected - its errors listed, its markers drawn "
                        "(NAME as the rules list shows it; 'floe2 drc DB "
                        "--rules' lists the names). Forwarded with --drc: "
                        "results already open there are not opened again")
    p.add_argument("--drc-err", dest="drc_err", type=int, default=None,
                   metavar="N",
                   help="with --drc-rule: that rule's error N - its number "
                        "in the rule's list, from 1 - is jumped to as a "
                        "double-click on it jumps: framed, its CD rulers "
                        "drawn, the rule's layers alone on (Esc restores). "
                        "Forwarded with --drc")
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
                            "cut (`v`) switches it live. Default: on "
                            "(FLOE_RUST_DENSITY_STACK=off: off). "
                            "Forwarded to a running instance")
        # the occupancy summary at wide views (user 2026-10-20: "an option
        # to see the occupancy when a jobdeck-indexed layout is opened in
        # the plain layout mode"): the Rust renderer's design.ovo summary
        p.add_argument("--occupancy", choices=("on", "off"), default=None,
                       help="start with the occupancy summary at wide views "
                            "on or off - the wide views drawn from the "
                            "cache's design.ovo (a source a jobdeck indexed "
                            "has one; a layout gets it with `floe2 index "
                            "SRC --occupancy-only`); View > occupancy "
                            "summary at wide views (`o`) switches it live. "
                            "Default: a jobdeck on, a layout off. Forwarded "
                            "to a running instance")
    # a layout's default layer colours (user 2026-10-13)
    p.add_argument("--layer-colors", choices=("row", "group"), default=None,
                   help="a layout's default layer colours (where no layer "
                        "property names one): row = every row of the layer "
                        "panel its own colour, neighbours differ (default); "
                        "group = one colour per layer number, its datatypes "
                        "alike (as before 0.12.352). FLOE_LAYER_COLORS sets "
                        "the default for the viewer and floe2 render. Given, "
                        "opens an independent instance")
    # the layout's layer properties file (user 2026-11-11)
    p.add_argument("--layerprops", default=None, metavar="FILE",
                   help="layer properties for SRC: a Calibre .layerprops "
                        "file (L.D color fill name visible width), applied "
                        "when the layout or jobdeck opens as Layer > load "
                        "layer properties… applies one - the colours, fills, "
                        "line widths and visibility of the layers it lists, "
                        "over the design default beside the source "
                        "(SRC.layerprops); the layers it does not list keep "
                        "theirs. It stays the layout's while the window "
                        "shows it (a reload, a jobdeck's level view). A file "
                        "that cannot be read ends the launch. Forwarded to a "
                        "running instance, which applies it to the layout "
                        "the request opens; --no-file-load does not turn it "
                        "off")
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
    # a launcher's viewer (user 2026-11-11, TeeBox: one account starts
    # everyone's viewer - a user must not open what that account can read)
    p.add_argument("--no-file-load", action="store_true",
                   help="load no file but this command line's (SRC, "
                        "--layerprops, --drc, --rules, and what follows them "
                        "by name): File > "
                        "load layout… / load jobdeck…, DRC > open results "
                        ".db… / load SVRF rules… / load clusters… / load "
                        "waives… / review from previous run… / load notes…, "
                        "the layer menu's load layer properties… and the "
                        "reviewers window's Load peer work… are insensitive "
                        "and refuse. For a launcher that starts viewers "
                        "under a shared account; FLOE_NO_FILE_LOAD=1 (on / "
                        "yes / true) in its environment does the same. "
                        "Forwarded to a running instance, whose file loads "
                        "go off with it and stay off. Saving and exporting "
                        "are not affected")
    # the one who confirms the team's waives (user 2026-11-13)
    p.add_argument("--confirmer", action="store_true",
                   help="open the DRC results as a confirmer's: DRC > team "
                        "waives… lists every reviewer's waives - who waived "
                        "each error and why - and this viewer may confirm "
                        "them (kept in the confirmer's own file, "
                        ".DB.confirm.ID; floe2 drc --review-counts / "
                        "--review-stats say the confirmed and the "
                        "unconfirmed). A confirmed waive is locked: a "
                        "viewer started without this option does not turn "
                        "it back or change its reason - its confirmer "
                        "takes the confirmation back, or a viewer started "
                        "with this option turns it back. This viewer may "
                        "also reject a waive (Reject selected, asking "
                        "why): a rejected waive counts for nothing and "
                        "stays with its reviewer, shown as rejected, until "
                        "they waive the error again with a reason - what "
                        "was said before is kept (.DB.rejects.ID). Without "
                        "it the list is "
                        "read-only. "
                        "FLOE_CONFIRMER=1 (on / yes / true) in the "
                        "launcher's environment does the same. Forwarded "
                        "to a running instance with the results it opens")
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
            raise SystemExit("floe2: --userid/--floe-reviewer must not be empty")
        os.environ["FLOE_REVIEWER"] = reviewer
    # what this viewer's readers say they are in the cache locks
    os.environ.setdefault("FLOE_LOCK_WHAT", "floe2 view")
    return cmd_view(args)
