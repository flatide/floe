"""floe command line interface."""

import argparse
import functools
import json
import os
import sys
import time

print = functools.partial(print, flush=True)

from . import __version__

# NOTE: klayout / floe.cache are imported inside the commands that need
# them - `view` must be able to forward to a running instance without
# paying the klayout import cost (see instance.py)


def parse_goto(s):
    """--goto X,Y[,WINDOW] in um -> [x, y] or [x, y, window]."""
    try:
        vals = [float(t) for t in s.replace(",", " ").split()]
    except ValueError:
        vals = []
    if len(vals) not in (2, 3):
        raise SystemExit(f"invalid --goto {s!r}, expected X,Y[,WINDOW] in um")
    if len(vals) == 3 and vals[2] <= 0:
        raise SystemExit("--goto WINDOW must be > 0 (um)")
    return vals


def parse_bbox_um(s, dbu):
    try:
        x0, y0, x1, y1 = (float(v) for v in s.split(","))
    except ValueError:
        raise SystemExit(f"invalid --bbox {s!r}, expected X0,Y0,X1,Y1 in um")
    to_dbu = lambda v: int(round(v / dbu))
    a, b = sorted((to_dbu(x0), to_dbu(x1)))
    c, d = sorted((to_dbu(y0), to_dbu(y1)))
    if a == b or c == d:
        raise SystemExit("--bbox has zero width or height")
    return a, c, b, d


def open_cache(src, auto_index, args):
    from . import cache as cache_mod
    c = cache_mod.Cache(src)
    # --layout-mode viewer|editable overrides the per-cache read-mode
    # heuristic everywhere this cache handle is used
    c.layout_mode = getattr(args, "layout_mode", None)
    if not c.exists():
        if not auto_index:
            raise SystemExit(f"no cache for {src}; run: floe index {src}")
        print(f"[floe] no cache yet - building index first (one-time)...")
        cache_mod.build_index(src, jobs=getattr(args, "jobs", None))
    c.load()
    if c.is_stale():
        print("[floe][warn] cache is outdated (source changed or cache "
              "format bumped); run 'floe index' to rebuild",
              file=sys.stderr)
    return c


def cmd_index(args):
    from . import cache as cache_mod
    caps = dict(text_cap=args.text_cap, text_tile_cap=args.text_tile_cap,
                skel_texts=args.skel_texts)
    if args.skeleton_only or args.texts_only or args.merge_only:
        c = cache_mod.Cache(args.src)
        if not c.exists():
            raise SystemExit("floe: no cache to upgrade; run a full "
                             "'floe index' first")
        c.load()
        if args.merge_only:
            cache_mod.rebuild_merge(c, jobs=args.jobs)
        elif args.texts_only:
            cache_mod.rebuild_texts(c, **caps)
        else:
            cache_mod.add_skeleton(c, **caps)
        return
    c = cache_mod.Cache(args.src)
    if c.exists() and not args.force:
        c.load()
        if not c.is_stale():
            print(f"[floe] cache up to date: {c.dir} (use --force to rebuild)")
            return
    bands = (None if args.bands.strip().lower() in ("none", "0", "")
             else tuple(sorted(float(t) for t in args.bands.split(","))))
    cache_mod.build_index(args.src, tile_bytes=args.tile_mb * 1e6,
                          jobs=args.jobs, bands=bands,
                          read_mode=args.read_mode,
                          gov=not args.no_gov, mem_gb=args.mem,
                          mem_floor_gb=args.mem_floor,
                          tile_tgt=args.tile_tgt,
                          merge=not args.no_merge, **caps)


def cmd_info(args):
    from . import cache as cache_mod
    c = open_cache(args.src, auto_index=False, args=args)
    m = c.meta
    dbu = m["dbu"]
    bb = m["bbox"]
    print(f"source     : {m['src']['path']} "
          f"({m['src']['size'] / 1e9:.2f} GB)")
    print(f"top cell   : {m['top_cell']}   cells: {m['stats']['cells']}")
    print(f"bbox       : ({bb[0] * dbu:.1f}, {bb[1] * dbu:.1f}) - "
          f"({bb[2] * dbu:.1f}, {bb[3] * dbu:.1f}) um")
    g = m["grid"]
    print(f"grid       : {g['nx']}x{g['ny']} tiles "
          f"({m['stats']['tile_files']} non-empty), "
          f"tile {g['tile_w'] * dbu:.0f}x{g['tile_h'] * dbu:.0f} um")
    print(f"index time : {m['stats']['total_s']}s "
          f"(read {m['stats']['read_s']}s, tiles {m['stats']['tiles_s']}s)")
    shapes = sum(l["stored_shapes"] for l in m["layers"])
    print(f"read mode  : %s (%.2f shapes/byte; --layout-mode overrides)"
          % ("viewer" if cache_mod.viewer_mode_preferred(m)
             else "editable", shapes / max(1, m["src"]["size"])))
    tinfo = m.get("texts_thinned") or m.get("texts_dropped")
    if tinfo:
        kind = "thinned" if m.get("texts_thinned") else "dropped"
        print(f"texts      : {kind} layers "
              + ", ".join(f"{t['layer']}/{t['datatype']}" for t in tinfo)
              + "  (over per-layer cap; kept per-tile - "
                "--text-cap / --text-tile-cap adjust)")
    if m.get("bands"):
        th = m["bands"]["thresholds_um"]
        n = len(th)
        edges = [f">={th[-1]}um" if k == 0 else
                 f"<{th[0]}um" if k == n else
                 f"{th[n - 1 - k]}-{th[n - k]}um" for k in range(n + 1)]
        parts = []
        for k in range(n + 1):
            d = os.path.join(c.dir, f"tiles_b{k}")
            files = os.listdir(d) if os.path.isdir(d) else []
            sz = sum(os.path.getsize(os.path.join(d, f)) for f in files)
            parts.append(f"b{k}({edges[k]}): {len(files)} files "
                         f"{sz / 1e6:.1f}MB")
        print("bands      : " + "  ".join(parts))
        mparts = []
        for k in range(1, n + 1):
            d = os.path.join(c.dir, f"tiles_m{k}")
            files = os.listdir(d) if os.path.isdir(d) else []
            if files:
                sz = sum(os.path.getsize(os.path.join(d, f))
                         for f in files)
                mparts.append(f"m{k}: {len(files)} files "
                              f"{sz / 1e6:.1f}MB")
        print("merged     : " + ("  ".join(mparts) if mparts else
                                 "none (floe index --merge-only adds "
                                 "cut stand-ins)"))
    print(f"{'layer':>8}  {'name':<12} {'stored shapes':>14}")
    for l in m["layers"]:
        print(f"{l['layer']:>5}/{l['datatype']:<2} {l['name']:<12} "
              f"{l['stored_shapes']:>14,}")


def cmd_render(args):
    from . import cache as cache_mod
    c = open_cache(args.src, auto_index=args.auto_index, args=args)
    dbu = c.meta["dbu"]
    x0, y0, x1, y1 = parse_bbox_um(args.bbox, dbu)
    layers = c.resolve_layers(args.layers)
    t0 = time.perf_counter()
    ly, top, ntiles = cache_mod.load_region(
        c, x0, y0, x1, y1, log=print, max_tiles=args.max_tiles,
        layers=layers)
    from .render import Renderer
    colors = {(l["layer"], l["datatype"]): l["color"]
              for l in c.meta["layers"]}
    r = Renderer(ly, top, colors, hier_offset=2)
    w = args.px
    h = max(1, round(w * (y1 - y0) / (x1 - x0)))
    depth = None if args.depth is None or args.depth >= 999 else args.depth
    r.render_png(args.out, x0, y0, x1, y1, w, h, visible=layers,
                 depth=depth)
    print(f"[floe] rendered {args.out} ({w}x{h}) "
          f"in {time.perf_counter() - t0:.2f}s ({ntiles} tiles)")


def cmd_clip(args):
    import klayout.db as db
    from . import cache as cache_mod
    t0 = time.perf_counter()
    if args.exact:
        # slow path: parse the original file for boundary-exact geometry
        src_ly = db.Layout(False)  # read-only: viewer mode
        print("[floe] --exact: full read of source (slow)...")
        src_ly.read(args.src)
        top = cache_mod.pick_top_cell(src_ly, print)
        dbu = src_ly.dbu
        x0, y0, x1, y1 = parse_bbox_um(args.bbox, dbu)
        ci = src_ly.clip(top.cell_index(), db.Box(x0, y0, x1, y1))
        ly, clip_ci = src_ly, ci
        meta_layers = None
    else:
        c = open_cache(args.src, auto_index=args.auto_index, args=args)
        dbu = c.meta["dbu"]
        x0, y0, x1, y1 = parse_bbox_um(args.bbox, dbu)
        sel = c.resolve_layers(args.layers) if args.layers else None
        ly, top, ntiles = cache_mod.load_region(
            c, x0, y0, x1, y1, log=print, max_tiles=args.max_tiles,
            layers=sel)
        clip_ci = ly.clip(top.cell_index(), db.Box(x0, y0, x1, y1))
        meta_layers = c.meta["layers"]

    cell = ly.cell(clip_ci)
    cell.name = args.cell_name
    opt = cache_mod.save_opts()
    opt.add_cell(clip_ci)
    layers = None
    if args.layers:
        if meta_layers is not None:
            cc = cache_mod.Cache(args.src)
            cc.meta = {"layers": meta_layers}
            layers = cc.resolve_layers(args.layers)
        else:
            layers = [tuple(int(v) for v in tok.split("/"))
                      for tok in args.layers.split(",")]
        opt.deselect_all_layers()
        for li in ly.layer_indexes():
            info = ly.get_info(li)
            if (info.layer, info.datatype) in layers:
                opt.add_layer(li, db.LayerInfo())
    ly.write(args.out, opt)
    sz = os.path.getsize(args.out)
    print(f"[floe] clip saved: {args.out} ({sz / 1e6:.2f} MB) "
          f"in {time.perf_counter() - t0:.2f}s")


def _cache_ready(src):
    """Lightweight cache check without importing klayout (kept in sync
    with cache.Cache: <src>.ice/meta.json + size/mtime fingerprint)."""
    try:
        with open(src + ".ice/meta.json") as f:
            meta = json.load(f)
        st = os.stat(src)
        return (st.st_size == meta["src"]["size"]
                and int(st.st_mtime) == meta["src"]["mtime"])
    except (OSError, ValueError, KeyError):
        return False


def cmd_probe(args):
    """End-to-end test of the GUI's render path WITHOUT a GUI: spawn the
    render service exactly like the viewer does and pull real frames over
    the queues. Isolates 'viewer shows black' to either the service side
    (this fails too) or the display side (this passes)."""
    import queue as _queue
    c = open_cache(args.src, auto_index=False, args=args)
    from .service import RenderWorker
    w = RenderWorker(c)
    w.start()
    print(f"[probe] service spawned (pid {w._proc.pid})")
    bb = c.meta["bbox"]
    g = c.meta["grid"]
    cx, cy = (bb[0] + bb[2]) // 2, (bb[1] + bb[3]) // 2
    hw, hh = g["tile_w"] // 2, g["tile_h"] // 2
    jobs = [("skeleton (fit view)",
             {"kind": "render", "gen": 1, "scope": "skel",
              "bbox": tuple(bb), "view": None,
              "w": 600, "h": 600, "depth": 0, "visible": None}),
            ("live (1-tile region at center)",
             {"kind": "render", "gen": 2, "scope": "live",
              "bbox": (cx - hw, cy - hh, cx + hw, cy + hh), "view": None,
              "w": 600, "h": 600, "depth": None, "visible": None})]
    failed = False
    for label, job in jobs:  # sequentially: the service coalesces renders
        w.submit(job)
        while True:
            try:
                res = w.res.get(timeout=180)
            except _queue.Empty:
                res = None
                break
            if res.get("kind") == "frame" and (res.get("preview")
                                               or res.get("bg")):
                sub = "preview" if res.get("preview") else "margin"
                print(f"[probe] {label}: {sub} frame "
                      f"{len(res.get('png', b'')):,} bytes, "
                      f"{res.get('ms')} ms")
                continue
            break
        if res is None:
            print(f"[probe] {label}: TIMEOUT after 180s "
                  f"(service alive: {w._proc.is_alive()})")
            failed = True
            break
        if res.get("kind") == "frame":
            png = res.get("png", b"")
            ok = png[:4] == b"\x89PNG"
            cut = (f", cut<{res['cut_um']}um" if res.get("cut_um")
                   else "")
            drawn = ("" if res.get("drawn") is None
                     else f", ~{res['drawn']:,} drawn")
            print(f"[probe] {label}: frame OK  {len(png):,} bytes png "
                  f"(magic {'OK' if ok else 'BAD'}), "
                  f"{res.get('ms')} ms, tiles {res.get('tiles')}"
                  f"{cut}{drawn}")
            failed = failed or not ok
        else:
            print(f"[probe] {label}: {res}")
            failed = True
    alive = w._proc.is_alive()
    w.stop()
    print(f"[probe] service alive at end: {alive}")
    if failed or not alive:
        print("[probe] FAILED - the GUI's render path is broken on this "
              "host (the viewer would show a black view)")
        raise SystemExit(1)
    print("[probe] OK - service+queues work; a black viewer would be a "
          "display-side problem")


def cmd_profile(args):
    """Emit a structure-only profile (counts/sizes/grid, no geometry) of
    an indexed layout, for synthesizing a render-performance lookalike
    outside the closed network (tools/gen_from_profile.py)."""
    from . import cache as cache_mod
    c = open_cache(args.src, auto_index=False, args=args)
    prof = cache_mod.profile_cache(c, sample_tiles=args.sample_tiles,
                                   anon=args.anon,
                                   log=lambda *a: print(*a,
                                                        file=sys.stderr))
    text = json.dumps(prof, separators=(",", ":"))
    if args.out:
        with open(args.out, "w") as f:
            f.write(text)
        print(f"[profile] wrote {args.out} ({len(text) / 1e3:.0f} KB, "
              f"{len(prof['samples'])} sampled tiles)")
    else:
        print(text)


def cmd_drc(args):
    """Summarize a Calibre ASCII DRC results database."""
    from . import drc as drc_mod
    d = drc_mod.load_db(args.db)
    print(f"{d.path}: cell {d.cell}, precision {d.precision:g}")
    print(f"{len(d.checks)} checks, {d.total} errors")
    for c in d.checks:
        desc = c.desc.split("\n")[0] if c.desc else ""
        print(" %-28s %6d  %s" % (c.name, len(c.errors), desc))
        if args.list:
            for e in c.errors:
                x, y = e.center()
                b = e.bbox()
                print("   #%-5d %-4s (%.3f, %.3f) um  %.3f x %.3f"
                      % (e.num, "poly" if e.kind == "p" else "edge",
                         x, y, b[2] - b[0], b[3] - b[1]))


def cmd_gtktest(args):
    """Minimal pixbuf-display matrix for diagnosing a black view.
    Three panels: (a) pixbuf loaded from a PNG file, (b) pixbuf
    synthesized in memory the way the viewer composes frames,
    (c) the synthesized pixbuf inside the viewer's Overlay/ScrolledWindow
    containment. Report which panels show content."""
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

    win = Gtk.Window(title="floe gtktest")
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


def cmd_view(args):
    src = os.path.abspath(args.src)
    if not os.path.isfile(src):
        raise SystemExit(f"floe: no such file: {src}")
    goto = parse_goto(args.goto) if args.goto else None

    server = None
    if not args.multi:  # flateyes-style single instance per (uid, DISPLAY)
        from . import instance
        display = instance.display_key()
        if display is None:
            print("floe: DISPLAY is not set", file=sys.stderr)
            raise SystemExit(1)
        # the receiving instance must be able to load the cache, and index
        # progress belongs in this terminal, not inside the GUI process
        if not _cache_ready(src):
            open_cache(src, auto_index=True, args=args)
        addr = instance.socket_address(display)
        request = src
        if goto is not None:
            # repr() round-trips floats exactly, unlike %g
            request += "\tgoto=" + ",".join(repr(v) for v in goto)
        for _ in range(5):
            code = instance.try_forward(addr, request)
            if code is not None:
                raise SystemExit(code)
            server = instance.try_bind(addr)
            if server is not None:
                break
            time.sleep(0.2)
        if server is None:
            print("floe: could not create or reach the instance socket",
                  file=sys.stderr)
            raise SystemExit(1)
        if not addr.startswith("\0"):
            import atexit
            atexit.register(lambda: os.path.exists(addr) and os.unlink(addr))

    c = open_cache(src, auto_index=args.auto_index, args=args)
    # PyGObject/GTK3 problems are reported inside import_gtk (exit 3)
    from .gui import run_viewer
    depth = args.depth
    if depth is None and (goto is not None or args.drc):
        depth = 999   # jumping somewhere = inspecting: full depth
    run_viewer(c, server, goto=goto, drc=args.drc,
               cut_level=args.cut_level, dump=args.dump, depth=depth)


def main(argv=None):
    ap = argparse.ArgumentParser(
        prog="floe",
        description="fast viewer/clipper for large OASIS files "
                    "(spatial tile cache)")
    ap.add_argument("--version", action="version", version=__version__)
    sub = ap.add_subparsers(dest="cmd", required=True)

    p = sub.add_parser("index", help="build the spatial tile cache (one-time)")
    p.add_argument("src")
    p.add_argument("--force", action="store_true")
    p.add_argument("--tile-mb", type=float, default=6.0,
                   help="target tile file size in MB (default 6)")
    p.add_argument("--skeleton-only", action="store_true",
                   help="add the far-zoom skeleton to an existing cache "
                        "(one source read, no re-tiling)")
    p.add_argument("--texts-only", action="store_true",
                   help="refresh text handling of an existing banded "
                        "cache without re-tiling: strip pre-0.5.4 b0 "
                        "texts and rebuild the skeleton labels "
                        "(--text-cap / --text-tile-cap / --skel-texts "
                        "adjust the label budgets)")
    p.add_argument("--merge-only", action="store_true",
                   help="add merged twins to an existing banded cache "
                        "without re-tiling (reads the band files back; "
                        "no source read). Twins stand in for bands the "
                        "viewer's cut drops")
    p.add_argument("--no-merge", action="store_true",
                   help="skip building merged twins during a full "
                        "index (cut-dropped bands disappear instead "
                        "of drawing as coarse outlines)")
    p.add_argument("--jobs", type=int, default=None, metavar="N",
                   help="max fork workers for the tiling phase (default: "
                        "all cores; 1 = sequential). A memory governor "
                        "probes one tile solo, then keeps only as many "
                        "workers busy as free RAM affords (see --mem / "
                        "--mem-floor / --no-gov).")
    p.add_argument("--mem", type=float, default=None, metavar="GB",
                   help="memory ceiling for this index run: loaded "
                        "source + tile workers stay under it. Use when "
                        "other programs (or users) need their share of "
                        "RAM. Default: bounded only by free RAM")
    p.add_argument("--mem-floor", type=float, default=None, metavar="GB",
                   help="free-RAM reserve the governor never dips into "
                        "(default: max(2, 5%% of RAM))")
    p.add_argument("--no-gov", action="store_true",
                   help="disable the memory governor and always run "
                        "--jobs workers (pre-0.5.5 behavior; can OOM "
                        "on small machines)")
    p.add_argument("--text-cap", type=int, default=None, metavar="N",
                   help="per-layer text budget when collecting skeleton "
                        "labels (default 1,000,000; layers over it are "
                        "thinned per tile; 0 = unlimited)")
    p.add_argument("--text-tile-cap", type=int, default=None, metavar="N",
                   help="per-tile text sample kept for over-budget "
                        "layers (default 10,000; 0 = drop those layers "
                        "whole)")
    p.add_argument("--skel-texts", type=int, default=None, metavar="N",
                   help="total far-view skeleton labels kept (default "
                        "50,000; 0 = unlimited)")
    p.add_argument("--tile-tgt", default=None,
                   choices=("viewer", "editable"),
                   help="layout mode for tile clip targets (default "
                        "viewer; editable = pre-0.5.0 path, for "
                        "comparison runs)")
    p.add_argument("--bands", default="0.125,0.5,2",
                   help="size-band edges in um (ascending); shapes are "
                        "split per band so wide views skip subpixel "
                        "content entirely. 'none' = legacy single-file "
                        "tiles (default: 0.125,0.5,2)")
    p.add_argument("--read-mode", default=None,
                   choices=("viewer", "editable"),
                   help="source read mode (default viewer): viewer keeps "
                        "repetition arrays compact - editable "
                        "materializes every member (~46 B each; a 10 GB "
                        "array-heavy file was observed at 400 GB RSS)")
    p.set_defaults(fn=cmd_index)

    p = sub.add_parser("info", help="show cache/layout summary")
    p.add_argument("src")
    p.set_defaults(fn=cmd_info)

    common = dict(auto_index=True)

    p = sub.add_parser("render", help="render a region to PNG")
    p.add_argument("src")
    p.add_argument("--bbox", required=True, help="X0,Y0,X1,Y1 in um")
    p.add_argument("--layers", default=None,
                   help="comma list: names or layer/datatype (default all)")
    p.add_argument("--px", type=int, default=1200, help="output width px")
    p.add_argument("--out", default="view.png")
    p.add_argument("--depth", type=int, default=None,
                   help="hierarchy depth (0=top only, 999/omit=full)")
    p.add_argument("--max-tiles", type=int, default=64)
    p.add_argument("--auto-index", action="store_true", default=True)
    p.add_argument("--layout-mode", default=None,
                   choices=("viewer", "editable"),
                   help="tile read mode (default: per-cache heuristic, "
                        "see 'floe info')")
    p.set_defaults(fn=cmd_render)

    p = sub.add_parser("clip", help="save a region as a new OASIS file")
    p.add_argument("src")
    p.add_argument("--bbox", required=True, help="X0,Y0,X1,Y1 in um")
    p.add_argument("--layers", default=None)
    p.add_argument("--out", default="clip.oas")
    p.add_argument("--cell-name", default="FLOE_CLIP")
    p.add_argument("--exact", action="store_true",
                   help="clip from the original file (slow, boundary-exact)")
    p.add_argument("--max-tiles", type=int, default=256)
    p.add_argument("--auto-index", action="store_true", default=True)
    p.add_argument("--layout-mode", default=None,
                   choices=("viewer", "editable"),
                   help="tile read mode (default: per-cache heuristic)")
    p.set_defaults(fn=cmd_clip)

    p = sub.add_parser("probe", help="test the viewer's render service "
                                     "headlessly (spawn + queues + frames); "
                                     "diagnoses a black viewer")
    p.add_argument("src")
    p.add_argument("--layout-mode", default=None,
                   choices=("viewer", "editable"),
                   help="tile read mode (default: per-cache heuristic)")
    p.set_defaults(fn=cmd_probe)

    p = sub.add_parser("profile", help="emit a structure-only profile "
                                       "(counts/sizes only, no geometry) "
                                       "for building a lookalike sample")
    p.add_argument("src")
    p.add_argument("--out", default=None, help="write JSON here "
                                               "(default: stdout)")
    p.add_argument("--sample-tiles", type=int, default=4,
                   help="tile files to census for instance/shape-type "
                        "stats (default 4; 0 = meta only)")
    p.add_argument("--anon", action="store_true",
                   help="replace layer names with L<num>_<dt>")
    p.set_defaults(fn=cmd_profile)

    p = sub.add_parser("drc", help="summarize a Calibre ASCII DRC "
                                   "results database (.db)")
    p.add_argument("db")
    p.add_argument("--list", action="store_true",
                   help="also list every error (center + size, um)")
    p.set_defaults(fn=cmd_drc)

    p = sub.add_parser("gtktest", help="minimal pixbuf display test "
                                       "(diagnoses a black view)")
    p.add_argument("png", nargs="?", default=None,
                   help="optional PNG to show as the from-file panel")
    p.set_defaults(fn=cmd_gtktest)

    p = sub.add_parser("view", help="native desktop viewer (GTK3); "
                                    "one instance per (uid, DISPLAY) - "
                                    "later calls forward the path to it")
    p.add_argument("src")
    p.add_argument("--multi", action="store_true",
                   help="always open an independent window (skip the "
                        "single-instance socket)")
    p.add_argument("--goto", default=None, metavar="X,Y[,W]",
                   help="start centered on X,Y (um) with an X marker; "
                        "W = view width in um (omitted = fit view). "
                        "Forwarded to a running instance too.")
    p.add_argument("--drc", default=None, metavar="FILE.db",
                   help="preload a Calibre ASCII DRC results db and "
                        "open the error browser (new instance only)")
    p.add_argument("--auto-index", action="store_true", default=True)
    p.add_argument("--cut-level", type=int, default=None, metavar="N",
                   choices=(0, 1, 2, 3),
                   help="starting detail cut level: 0 = off, 1 = "
                        "default, higher = lighter wide views (finer "
                        "detail drawn as merged outlines when the "
                        "cache carries them). The `c` dialog changes "
                        "it at runtime; the px thresholds behind the "
                        "levels are internal and may be retuned")
    p.add_argument("--depth", type=int, default=None, metavar="N",
                   help="starting hierarchy depth (999 = full). "
                        "Default: 1 for a plain open - the fast, "
                        "industry-standard first paint - and full "
                        "when --goto jumps to an inspection point. "
                        "Digits / the `d` dialog change it at runtime")
    p.add_argument("--layout-mode", default=None,
                   choices=("viewer", "editable"),
                   help="tile read mode (default: per-cache heuristic, "
                        "see 'floe info'; new instance only)")
    p.add_argument("--dump", action="store_true",
                   help="save display-path debug dumps to /tmp/floe_*.png "
                        "(XQuartz black-view diagnosis; new instance only)")
    p.set_defaults(fn=cmd_view)

    args = ap.parse_args(argv)
    args.fn(args)


if __name__ == "__main__":
    main()
