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
    if not c.exists():
        if not auto_index:
            raise SystemExit(f"no cache for {src}; run: floe index {src}")
        print(f"[floe] no cache yet - building index first (one-time)...")
        cache_mod.build_index(src)
    c.load()
    if c.is_stale():
        print("[floe][warn] cache is outdated (source changed or cache "
              "format bumped); run 'floe index' to rebuild",
              file=sys.stderr)
    return c


def cmd_index(args):
    from . import cache as cache_mod
    if args.skeleton_only:
        c = cache_mod.Cache(args.src)
        if not c.exists():
            raise SystemExit("floe: no cache to upgrade; run a full "
                             "'floe index' first")
        c.load()
        cache_mod.add_skeleton(c)
        return
    c = cache_mod.Cache(args.src)
    if c.exists() and not args.force:
        c.load()
        if not c.is_stale():
            print(f"[floe] cache up to date: {c.dir} (use --force to rebuild)")
            return
    cache_mod.build_index(args.src, tile_bytes=args.tile_mb * 1e6)


def cmd_info(args):
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
        src_ly = db.Layout()
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


def cmd_view(args):
    src = os.path.abspath(args.src)
    if not os.path.isfile(src):
        raise SystemExit(f"floe: no such file: {src}")

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
        for _ in range(5):
            code = instance.try_forward(addr, src)
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
    run_viewer(c, server)


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
    p.set_defaults(fn=cmd_clip)

    p = sub.add_parser("view", help="native desktop viewer (GTK3); "
                                    "one instance per (uid, DISPLAY) - "
                                    "later calls forward the path to it")
    p.add_argument("src")
    p.add_argument("--multi", action="store_true",
                   help="always open an independent window (skip the "
                        "single-instance socket)")
    p.add_argument("--auto-index", action="store_true", default=True)
    p.set_defaults(fn=cmd_view)

    args = ap.parse_args(argv)
    args.fn(args)


if __name__ == "__main__":
    main()
