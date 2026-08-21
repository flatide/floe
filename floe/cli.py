"""floe command line interface."""

import argparse
import functools
import json
import os
import re
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


def open_cache(src, args):
    # the viewer is VFS-only: it opens <src>.floe (built by
    # `floe-index vfs`) and never auto-builds a cache.
    from . import cache as cache_mod
    c = cache_mod.Cache(src)
    c.layout_mode = getattr(args, "layout_mode", None)
    if not c.exists():
        raise SystemExit(
            f"no VFS cache for {src}; run: floe-index vfs {src}")
    c.load()
    if not c.meta.get("vfs"):
        raise SystemExit(
            f"{c.dir} is not a VFS cache; rebuild: floe-index vfs {src}")
    if c.is_stale():
        print("[floe][warn] cache is outdated (source changed); "
              "rebuild: floe-index vfs", file=sys.stderr)
    return c


def _vfs_region(c, x0, y0, x1, y1, layers):
    """An exact (cut=0) working-set layout for a bbox via a vfsd probe -
    the CLI counterpart of the viewer's render working set. Returns
    (layout, top_cell, VfsClient); stop the client when done."""
    from .vfsclient import VfsClient
    from .viewport import VfsMosaic
    dbu = c.meta["dbu"]
    vc = VfsClient(c.dir)
    view = (x0 * dbu, y0 * dbu, x1 * dbu, y1 * dbu)
    m = VfsMosaic(c)
    r = vc.request(0, view, 1.0, 0.0, layers, None, probe=True)
    if r["names"]:
        m.load_names(r["names"])
    m.apply_hier(r["delta"], r["top"], [])
    return m.ly, m.top, vc


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
            cache_mod.rebuild_texts(c, jobs=args.jobs, **caps)
        else:
            cache_mod.add_skeleton(c, jobs=args.jobs, **caps)
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
                          merge=args.merge, force=args.force,
                          **caps)


def cmd_info(args):
    c = open_cache(args.src, args=args)
    m = c.meta
    dbu = m["dbu"]
    bb = m["bbox"]

    def _du(path):
        tot = 0
        for root, _dirs, files in os.walk(path):
            for f in files:
                try:
                    tot += os.path.getsize(os.path.join(root, f))
                except OSError:
                    pass
        return tot

    print(f"source     : {m['src']['path']} "
          f"({m['src']['size'] / 1e9:.2f} GB)")
    print(f"top cell   : {m['top_cell']}")
    print(f"bbox       : ({bb[0] * dbu:.1f}, {bb[1] * dbu:.1f}) - "
          f"({bb[2] * dbu:.1f}, {bb[3] * dbu:.1f}) um")
    print(f"cache      : {c.dir}  ({_du(c.dir) / 1e6:.1f} MB, VFS v"
          f"{m.get('vfs')})")
    for part in ("design.ovm", "design.ovp", "design.ovc"):
        p = os.path.join(c.dir, part)
        if os.path.exists(p):
            print(f"  {part:<11}: {os.path.getsize(p) / 1e6:.1f} MB")
        elif part == "design.ovc":
            print("  design.ovc : (none; add: floe-index vfs "
                  "--coverage-only)")
    sk = m.get("skeleton") or {}
    print(f"skeleton   : {sk.get('shapes', 0):,} shapes, "
          f"{sk.get('texts', 0):,} texts")
    print(f"{'layer':>8}  {'name':<12} {'stored shapes':>14}")
    for l in m["layers"]:
        print(f"{l['layer']:>5}/{l['datatype']:<2} {l['name']:<12} "
              f"{l['stored_shapes']:>14,}")


def _drc_err_spec(spec, n):
    """--drc-err N | A-B | all -> 0-based local indices."""
    if spec in (None, "", "all"):
        return list(range(n))
    if "-" in spec:
        a, b = spec.split("-", 1)
        lo, hi = int(a), int(b)
        if lo < 1 or hi < lo:
            raise SystemExit("floe: bad --drc-err range %r" % spec)
        return list(range(lo - 1, min(hi, n)))
    k = int(spec)
    if not 1 <= k <= n:
        raise SystemExit("floe: --drc-err %d out of 1..%d" % (k, n))
    return [k - 1]


def _drc_layer_legend(c, layers):
    """flateyes legend lines for the layers that are ON in the
    snapshot (user call 2026-08-19): one swatch per visible layer
    in the design color AND its fill pattern BY NAME (user call
    2026-08-20: "box <color> speckle NAME l/d" - flateyes carries
    the same fillpatterns.def table, so the name replaces the
    pat:HEX64 bitmap; layerprops fills like the viewer, speckle
    default). None when everything is on (a full palette table
    would drown the image)."""
    if layers is None:
        return None
    fills = {}
    try:
        from . import fillpat
        from . import cache as cache_mod
        lrows, _ = cache_mod.load_layer_props(c.src)
        if lrows:
            for key, _color, fill, _nm, _f1, _f2 in lrows:
                fl = (fill or "").strip().lower()
                if fl in fillpat.FIXED_FILLS \
                        or fillpat.fill_index(fill) is not None:
                    fills[tuple(key)] = fl
    except Exception:
        pass
    by = {(l["layer"], l["datatype"]): l for l in c.meta["layers"]}
    legend = []
    for key in layers:
        lay = by.get(tuple(key))
        if lay is None:
            continue
        color = lay.get("color") or "#808080"
        name = (lay.get("name") or "").strip()
        label = ("%s %d/%d" % (name, key[0], key[1])).strip()
        legend.append("box %s %s %s"
                      % (color, fills.get(tuple(key), "speckle"),
                         label))
    return legend or None


def _embed_error_png(path, e, bb_um, px, waived, rule, local,
                     legend=None):
    """flateyes-embed annotations (user call 2026-08-19): the error
    geometry, its CD ruler(s) and the length labels ride INSIDE the
    PNG as the flateyes iTXt chunk (fe_embed format, vendored) -
    pixels stay untouched, flateyes shows and edits them on open,
    every other tool sees a plain PNG. Coordinates are image-
    center-origin pixels; the embedded ppu makes flateyes label
    the rulers in um by itself.  A one-edge ruler uses the viewer's
    same 14 px normal offset, without endpoint extension lines."""
    from . import drc as drc_mod
    from . import fe_embed as fe
    x0, y0, x1, y1 = bb_um
    ppu = px / max(1e-9, x1 - x0)          # square frame
    cxu, cyu = (x0 + x1) / 2, (y0 + y1) / 2

    def P(x, y):
        return ((x - cxu) * ppu, (cyu - y) * ppu)   # y down

    col = "#00E676" if waived else "#FF5252"
    annos = []
    pts = [P(x, y) for x, y in e.pts]
    if e.kind == "p" and len(pts) >= 3:
        # viewer parity (user calls 2026-08-21): interior = the
        # speckle PATTERN in opaque status-color pixels (flateyes
        # 1.18 fill_pat replaces the translucent wash - exactly the
        # viewer's 50% checker of full-intensity pixels), outline =
        # 2px without the casing halo; like the viewer,
        # >256-vertex interiors stay outline-only
        # (_drc_fill_speckle cap)
        fill = col if len(pts) <= 256 else None
        annos.append(fe.polygon(
            pts, color=col, fill=fill,
            fill_pat="speckle" if fill else None,
            width=2, casing=False))
    elif len(pts) >= 2:
        for j in range(0, len(pts) - 1, 2):
            a, b = pts[j], pts[j + 1]
            if a != b:
                annos.append(fe.line(a[0], a[1], b[0], b[1],
                                     color=col, width=2))
    offset_single_edge = e.kind == "e" and len(e.pts) == 2
    for sx0, sy0, sx1, sy1 in drc_mod.cd_segments(e):
        a, b = P(sx0, sy0), P(sx1, sy1)
        if a != b:
            if offset_single_edge:
                a, b = drc_mod.offset_screen_segment(a, b)
            annos.append(fe.ruler(a[0], a[1], b[0], b[1]))
    note = "%s #%d(%d)%s" % (rule, local, e.num,
                             " - waived" if waived else "")
    fe.embed(path, annos, ppu=ppu, unit="um", note=note,
             legend=legend)


def _drc_isolate_layers_cli(args, c, d, rule):
    """SVRF sidecar layer isolation for snapshots (viewer double-
    click parity): only the rule's source GDS layers stay on.
    Returns resolved [(l, d), ...] or None = no metadata (render
    all layers, note on stderr). Sidecar search mirrors the
    viewer: --drc-rules, deck-basename NEXT TO the db, recorded
    deck path, <db>.rules.json."""
    from . import svrf
    path = args.drc_rules
    if path is None:
        deck = None
        for ch in d.checks[:50]:
            for ln in (ch.desc or "").split("\n"):
                if ln.startswith("Rule File Pathname:"):
                    deck = ln.split(":", 1)[1].strip()
                    break
            if deck:
                break
        dbdir = os.path.dirname(os.path.abspath(args.drc))
        cands = []
        if deck:
            cands.append(os.path.join(
                dbdir, os.path.basename(deck) + ".rules.json"))
            cands.append(deck + ".rules.json")
        cands.append(args.drc + ".rules.json")
        path = next((p for p in cands if os.path.isfile(p)), None)
        if path is None:
            print("[floe] no rules.json sidecar found - rendering "
                  "all layers", file=sys.stderr)
            return None
    try:
        meta = svrf.load_rules(path)
    except (OSError, ValueError) as exc:
        print("[floe][warn] rules sidecar unusable (%s) - all "
              "layers" % exc, file=sys.stderr)
        return None
    ent = (meta.get("checks") or {}).get(rule) or {}
    sg = ent.get("source_gds") or []
    if not sg:
        print("[floe] rule %r has no svrf layer metadata - all "
              "layers" % rule, file=sys.stderr)
        return None
    sel = []
    for lay in c.meta["layers"]:
        for g, dt in sg:
            if lay["layer"] == g and (dt is None
                                      or lay["datatype"] == dt):
                sel.append((lay["layer"], lay["datatype"]))
                break
    if not sel:
        print("[floe][warn] svrf source layers %r not in this "
              "design - all layers" % sg, file=sys.stderr)
        return None
    print("[floe] svrf isolate %s: %s"
          % (rule, ",".join("%d/%d" % t for t in sel)),
          file=sys.stderr)
    return sel


def _render_drc_errors(args, c):
    """--drc/--drc-rule: one square PNG per requested error through
    the VIEWER'S OWN render service (user call 2026-08-19: the
    exact-cut probe path looked nothing like the app - snapshots
    must carry the default detail/hairline/LOD/coverage knobs with
    depth forced full, and the per-error cold spawn cost 5s+). One
    persistent service = one vfsd session + one klayout working
    set, so consecutive errors render at viewer-jump speed.
    Prints `local<TAB>global<TAB>path` per file."""
    import queue as _queue
    from . import drc as drc_mod
    from .service import RenderWorker
    d = drc_mod.load_db(args.drc)
    ci = _drc_rule_index(d, args.drc_rule)
    ch = d.checks[ci]
    idxs = _drc_err_spec(args.drc_err, len(ch.errors))
    # the cap guards the IMPLICIT all only - an explicit N or A-B
    # is the user's stated intent and renders in full (field
    # report 2026-08-19: 1-1000 silently stopped at 200)
    explicit = args.drc_err not in (None, "", "all")
    if not explicit and len(idxs) > args.drc_cap:
        print("[floe][warn] %d errors in the rule - rendering the "
              "first %d (--drc-cap; pass an explicit range for "
              "more)" % (len(idxs), args.drc_cap),
              file=sys.stderr)
        idxs = idxs[:args.drc_cap]
    if not idxs:
        raise SystemExit("floe: rule %r has no errors"
                         % args.drc_rule)
    dbu = c.meta["dbu"]
    if args.layers is not None:
        layers = c.resolve_layers(args.layers)  # explicit wins
    else:
        layers = _drc_isolate_layers_cli(args, c, d, ch.name)
    depth = (None if args.depth is None or args.depth >= 999
             else args.depth)
    frac = min(max(args.drc_frac, 0.02), 1.0)
    safe = re.sub(r"[^\w.\-]+", "_", args.drc_rule)
    if args.out and args.out != "view.png":
        stem, ext = os.path.splitext(args.out)
        ext = ext or ".png"
    else:
        stem, ext = safe, ".png"
    has_st = hasattr(d, "get_status")
    legend = _drc_layer_legend(c, layers)
    w = RenderWorker(c)
    w.start()
    try:
        gen = 0
        for k in idxs:
            e = ch.errors[k]
            b = e.bbox()
            cx, cy = (b[0] + b[2]) / 2, (b[1] + b[3]) / 2
            span = max(b[2] - b[0], b[3] - b[1], 0.0) / frac
            if span <= 0:
                span = 0.1
            bb_um = (cx - span / 2, cy - span / 2,
                     cx + span / 2, cy + span / 2)
            bx = tuple(int(round(v / dbu)) for v in bb_um)
            gen += 1
            w.submit({"kind": "render", "gen": gen,
                      "scope": "live", "bbox": bx, "view": None,
                      "w": args.px, "h": args.px,
                      "depth": depth, "visible": layers})
            png = None
            while True:
                try:
                    # first frame pays the cold spawn (vfsd +
                    # klayout); later errors ride the warm session
                    res = w.res.get(timeout=300 if gen == 1
                                    else 120)
                except _queue.Empty:
                    raise SystemExit(
                        "floe: render service timeout (alive=%s)"
                        % w.alive())
                kind = res.get("kind")
                if kind == "error":
                    raise SystemExit("floe: render service: %s"
                                     % res.get("msg"))
                if kind != "frame" or res.get("gen") != gen:
                    continue
                if res.get("preview") or res.get("bg") \
                        or res.get("refining"):
                    continue   # streaming round: wait for settled
                png = res.get("png", b"")
                break
            path = ("%s%s" % (stem, ext) if len(idxs) == 1
                    else "%s_%d%s" % (stem, k + 1, ext))
            with open(path, "wb") as f:
                f.write(png)
            waived = (has_st and d.get_status(ci, k)
                      == drc_mod.STATUS_WAIVED)
            _embed_error_png(path, e, bb_um, args.px, waived,
                             ch.name, k + 1, legend=legend)
            print("%d\t%d\t%s" % (k + 1, e.num, path))
    finally:
        w.stop()


def cmd_render(args):
    c = open_cache(args.src, args=args)
    if args.drc or args.drc_rule:
        if not (args.drc and args.drc_rule):
            raise SystemExit(
                "floe: --drc and --drc-rule go together")
        _render_drc_errors(args, c)
        return
    if not args.bbox:
        raise SystemExit("floe: --bbox is required (or use "
                         "--drc/--drc-rule)")
    dbu = c.meta["dbu"]
    x0, y0, x1, y1 = parse_bbox_um(args.bbox, dbu)
    layers = c.resolve_layers(args.layers)
    t0 = time.perf_counter()
    ly, top, vc = _vfs_region(c, x0, y0, x1, y1, layers)
    try:
        from .render import Renderer
        colors = {(l["layer"], l["datatype"]): l["color"]
                  for l in c.meta["layers"]}
        # exports keep solid archival fills; the speckle is a live-
        # viewer presentation choice (README documents it as such)
        r = Renderer(ly, top, colors, hier_offset=0, speckle=False)
        w = args.px
        h = max(1, round(w * (y1 - y0) / (x1 - x0)))
        depth = (None if args.depth is None or args.depth >= 999
                 else args.depth)
        r.render_png(args.out, x0, y0, x1, y1, w, h, visible=layers,
                     depth=depth)
    finally:
        vc.stop()
    print(f"[floe] rendered {args.out} ({w}x{h}) "
          f"in {time.perf_counter() - t0:.2f}s")


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
        vc = None
    else:
        c = open_cache(args.src, args=args)
        dbu = c.meta["dbu"]
        x0, y0, x1, y1 = parse_bbox_um(args.bbox, dbu)
        sel = c.resolve_layers(args.layers) if args.layers else None
        ly, top, vc = _vfs_region(c, x0, y0, x1, y1, sel)
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
    if vc is not None:
        vc.stop()
    sz = os.path.getsize(args.out)
    print(f"[floe] clip saved: {args.out} ({sz / 1e6:.2f} MB) "
          f"in {time.perf_counter() - t0:.2f}s")


def _cache_ready(src):
    """Lightweight cache check without importing klayout: a VFS
    cache at <src>.floe with a matching source fingerprint."""
    try:
        with open(src + ".floe/meta.json") as f:
            meta = json.load(f)
        st = os.stat(src)
        return (bool(meta.get("vfs"))
                and st.st_size == meta["src"]["size"]
                and int(st.st_mtime) == meta["src"]["mtime"])
    except (OSError, ValueError, KeyError):
        return False


def cmd_probe(args):
    """End-to-end test of the GUI's render path WITHOUT a GUI: spawn the
    render service exactly like the viewer does and pull real frames over
    the queues. Isolates 'viewer shows black' to either the service side
    (this fails too) or the display side (this passes)."""
    import queue as _queue
    c = open_cache(args.src, args=args)
    from .service import RenderWorker
    w = RenderWorker(c)
    w.start()
    print(f"[probe] service spawned (pid {w._proc.pid})")
    bb = c.meta["bbox"]
    g = c.meta["grid"]
    cx, cy = (bb[0] + bb[2]) // 2, (bb[1] + bb[3]) // 2
    hw, hh = g["tile_w"] // 2, g["tile_h"] // 2
    jobs = [("fit view (live, depth 0)",
             {"kind": "render", "gen": 1, "scope": "live",
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
    outside the closed network (tools/gen_from_profile.py). This is a
    dev tool paired with the retained .tiles indexer (the rust-validation
    oracle), so it reads a tile cache directly rather than a VFS one."""
    from . import cache as cache_mod
    c = cache_mod.Cache(args.src)
    if not c.exists():
        raise SystemExit(f"no cache for {args.src}; run: floe index "
                         f"{args.src}")
    c.load()
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


def _drc_rule_index(d, name):
    """Check index for a rule name; duplicate names take the first
    occurrence (Calibre dbs may repeat a check block)."""
    hits = [i for i, ch in enumerate(d.checks) if ch.name == name]
    if not hits:
        raise SystemExit("floe: no such rule %r (see --rules)" % name)
    if len(hits) > 1:
        print("[floe][warn] rule %r appears %d times - using the "
              "first" % (name, len(hits)), file=sys.stderr)
    return hits[0]


def cmd_drc(args):
    """Summarize a Calibre ASCII DRC results database."""
    from . import drc as drc_mod
    d = drc_mod.load_db(args.db)
    if args.rules:
        # scripting surface: JSON array, one object per rule.
        # ProperTee workflow (user call 2026-08-19): redirect to a
        # file and json_parse it - its shell capture caps at 1MiB,
        # so piping is not the supported path.
        rules = []
        for ci, ch in enumerate(d.checks):
            wv = (d.status_counts(ci)[0]
                  if hasattr(d, "status_counts") else 0)
            rules.append({"name": ch.name,
                          "errors": len(ch.errors),
                          "waived": wv})
        json.dump(rules, sys.stdout, indent=1)
        sys.stdout.write("\n")
        return
    if args.errs is not None:
        # scripting surface: JSON array of ONE rule's errors -
        # STREAMED object by object (a rule can hold millions)
        ci = _drc_rule_index(d, args.errs)
        has_st = hasattr(d, "get_status")
        sys.stdout.write("[\n")
        for k, e in enumerate(d.checks[ci].errors):
            b = e.bbox()
            st = d.get_status(ci, k) if has_st else 0
            if k:
                sys.stdout.write(",\n")
            sys.stdout.write(json.dumps(
                {"local": k + 1, "global": e.num, "kind": e.kind,
                 "status": st,
                 "bbox": [round(v, 4) for v in b]}))
        sys.stdout.write("\n]\n")
        return
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


def cmd_svrf(args):
    """Parse a Calibre SVRF rule deck (subset) into rule metadata."""
    from . import svrf
    defines = {}
    for d in args.define:
        name, _, val = d.partition("=")
        if name:
            defines[name] = val or None
    deck = svrf.parse_deck(args.deck, defines, args.include_dir,
                           scan_all=args.scan,
                           follow_verbatim=args.follow_verbatim,
                           env_switches=not args.no_env_switches)
    if args.scan:
        print(svrf.format_scan(deck))
        return
    out = args.out or (args.deck + ".rules.json")
    data = svrf.write_json(deck, out)
    st = data["stats"]
    print("%s: %d checks, %d derivations, %d layers -> %s"
          % (args.deck, st["checks"], st["derivations"],
             len(data["layers"]), out))
    if st["cmacro_calls"]:
        print("[floe][warn] %d CMACRO calls NOT expanded - metadata "
              "is incomplete for macro-generated rules"
              % st["cmacro_calls"], file=sys.stderr)
    if st["skipped"]:
        print("[floe] %d unrecognized statements skipped "
              "(--scan lists them)" % st["skipped"], file=sys.stderr)
    for w in st["warnings"][:10]:
        print("[floe][warn] %s" % w, file=sys.stderr)


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
    # documented entry for the frame-tuning knobs: the flags set
    # the env vars vfsclient reads per request (children inherit;
    # a forwarded running instance keeps its own values)
    if args.hairline is not None:
        os.environ["FLOE_HAIRLINE"] = "%g" % args.hairline
    if args.thin_um is not None:
        os.environ["FLOE_THIN_UM"] = "%g" % args.thin_um
    goto = parse_goto(args.goto) if args.goto else None
    if args.stream_kb is not None and args.stream_kb < 0:
        raise SystemExit("floe: --stream-kb must be >= 0")
    if not 100 <= args.stream_target_ms <= 2000:
        raise SystemExit("floe: --stream-target-ms must be 100..2000")

    # Render-process construction parameters cannot be retrofitted into
    # a running single instance. Open an independent viewer when one is
    # explicitly supplied; live request controls are forwarded below.
    process_options = (args.stream_kb is not None
                       or args.stream_target_ms != 500
                       or args.render_debug)
    server = None
    if not args.multi and not process_options:
        # flateyes-style single instance per (uid, DISPLAY)
        from . import instance
        display = instance.display_key()
        if display is None:
            print("floe: DISPLAY is not set", file=sys.stderr)
            raise SystemExit(1)
        # the viewer is VFS-only and never builds a cache: fail here, in
        # this terminal, rather than forwarding an unopenable file to the
        # GUI instance
        if not _cache_ready(src):
            raise SystemExit(f"no VFS cache for {src}; "
                             f"run: floe-index vfs {src}")
        addr = instance.socket_address(display)
        request = src
        if goto is not None:
            # repr() round-trips floats exactly, unlike %g
            request += "\tgoto=" + ",".join(repr(v) for v in goto)
        request += "\tlod=%s\tframes=%s\tlabels=%s" % (
            args.lod, args.frames, args.labels)
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

    c = open_cache(src, args=args)
    # PyGObject/GTK3 problems are reported inside import_gtk (exit 3)
    from .gui import run_viewer
    depth = args.depth
    if depth is None and (goto is not None or args.drc):
        depth = 999   # jumping somewhere = inspecting: full depth
    detail = (None if args.detail is None
              else ("low", "medium", "high").index(args.detail))
    run_viewer(c, server, goto=goto, drc=args.drc,
               detail=detail, dump=args.dump, depth=depth,
               lod=args.lod == "on", frames=args.frames == "on",
               labels=args.labels == "on", stream_kb=args.stream_kb,
               stream_target_ms=args.stream_target_ms,
               render_debug=args.render_debug)


def main(argv=None):
    ap = argparse.ArgumentParser(
        prog="floe",
        description="fast viewer/clipper for large OASIS files "
                    "(spatial tile cache)")
    ap.add_argument("--version", action="version", version=__version__)
    sub = ap.add_subparsers(dest="cmd", required=True)

    p = sub.add_parser("index", help="build the spatial tile cache (one-time)")
    p.add_argument("src")
    p.add_argument("--force", action="store_true",
                   help="rebuild from scratch, ignoring both an "
                        "up-to-date cache and a resumable progress "
                        "journal from an interrupted run")
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
    p.add_argument("--merge", action="store_true",
                   help="also build merged twins, as a post-pass after "
                        "tiling (reads the band files back; no source "
                        "in RAM, so it parallelizes freely). Default: "
                        "off - add them any time later with "
                        "--merge-only. Without twins, bands the "
                        "viewer's cut drops simply disappear)")
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

    p = sub.add_parser("render", help="render a region to PNG "
                                      "(or DRC errors via --drc)")
    p.add_argument("src")
    p.add_argument("--bbox", default=None, help="X0,Y0,X1,Y1 in um "
                   "(omit when using --drc/--drc-rule)")
    p.add_argument("--layers", default=None,
                   help="comma list: names or layer/datatype (default all)")
    p.add_argument("--px", type=int, default=1200, help="output width px")
    p.add_argument("--out", default="view.png")
    p.add_argument("--depth", type=int, default=None,
                   help="hierarchy depth (0=top only, 999/omit=full)")
    p.add_argument("--drc", default=None, metavar="FILE.db",
                   help="DRC results db: render error snapshots of "
                        "--drc-rule instead of a fixed bbox (square "
                        "--px PNGs, geometry stamped red/green by "
                        "waive status; prints local/global/path TSV)")
    p.add_argument("--drc-rule", default=None, metavar="NAME",
                   help="rule name (see `floe drc <db> --rules`)")
    p.add_argument("--drc-err", default="all", metavar="N|A-B|all",
                   help="rule-local error number(s), 1-based "
                        "(default all, capped by --drc-cap)")
    p.add_argument("--drc-cap", type=int, default=200, metavar="N",
                   help="max PNGs when --drc-err is 'all' (default "
                        "200); explicit N / A-B ranges always "
                        "render in full")
    p.add_argument("--drc-frac", type=float, default=0.3, metavar="F",
                   help="error span as a fraction of the frame "
                        "(default 0.3 - viewer framing parity)")
    p.add_argument("--drc-rules", default=None, metavar="RULES.json",
                   help="SVRF rules sidecar for layer isolation "
                        "(default: auto-search like the viewer - "
                        "deck basename next to the db, recorded "
                        "deck path, <db>.rules.json). Snapshots "
                        "then keep only the rule's source GDS "
                        "layers on; an explicit --layers wins")
    p.set_defaults(fn=cmd_render)

    p = sub.add_parser("clip", help="save a region as a new OASIS file")
    p.add_argument("src")
    p.add_argument("--bbox", required=True, help="X0,Y0,X1,Y1 in um")
    p.add_argument("--layers", default=None)
    p.add_argument("--out", default="clip.oas")
    p.add_argument("--cell-name", default="FLOE_CLIP")
    p.add_argument("--exact", action="store_true",
                   help="clip from the original file (slow, boundary-exact)")
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
                                   "results database (.db; a fresh "
                                   "packed .ice built by 'floe-index "
                                   "drc' is used automatically)")
    p.add_argument("db")
    p.add_argument("--list", action="store_true",
                   help="also list every error (center + size, um)")
    p.add_argument("--rules", action="store_true",
                   help="machine-readable rule list as JSON: "
                        "[{name, errors, waived}, ...] - redirect "
                        "to a file for capture-limited callers")
    p.add_argument("--errs", default=None, metavar="RULE",
                   help="machine-readable error list of ONE rule "
                        "as JSON: [{local, global, kind, status, "
                        "bbox um}, ...] (streamed)")
    p.set_defaults(fn=cmd_drc)

    p = sub.add_parser("svrf", help="parse a Calibre SVRF rule deck "
                                    "(subset: layers, derivations, "
                                    "check constraints) into "
                                    "<deck>.rules.json - the viewer "
                                    "loads it next to the DRC .db "
                                    "for waive-decision aid")
    p.add_argument("deck")
    p.add_argument("-o", "--out", default=None,
                   help="output path (default <deck>.rules.json)")
    p.add_argument("--scan", action="store_true",
                   help="print a syntax inventory only (macros, "
                        "#IFDEF switches, measurement kinds, skipped "
                        "statements; both #IFDEF branches walked) - "
                        "run this FIRST on a new deck")
    p.add_argument("-D", "--define", action="append", default=[],
                   metavar="NAME[=VAL]",
                   help="preprocessor switch; pass the SAME set the "
                        "Calibre run used or the check list differs")
    p.add_argument("-I", "--include-dir", action="append", default=[],
                   help="extra search dir for INCLUDE files")
    p.add_argument("--follow-verbatim", action="store_true",
                   help="follow INCLUDEs found inside VERBATIM/Tcl "
                        "blocks in the normal parse too (hybrid "
                        "decks select layer stacks via Tcl; --scan "
                        "always follows them)")
    p.add_argument("--no-env-switches", action="store_true",
                   help="do NOT fall back to environment variables "
                        "for #IFDEF switches. Default: names the "
                        "deck tests are looked up in the "
                        "environment when not -D'd (sourceme "
                        "workflow: `source sourceme.* && floe svrf "
                        "...`); used ones are reported and stored "
                        "in the sidecar for provenance")
    p.set_defaults(fn=cmd_svrf)

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
                        "open the error browser (new instance only; "
                        "a fresh FILE.db.ice index built by "
                        "'floe-index drc' is used automatically)")
    p.add_argument("--detail", default=None,
                   choices=("low", "medium", "high"),
                   help="starting detail level (default: medium; "
                        "higher = finer, heavier wide views - lower "
                        "levels omit finer features below the cut; "
                        "coverage is an independent viewer toggle). "
                        "The `d` dialog changes it at runtime; the px "
                        "thresholds behind the levels are internal "
                        "and may be retuned")
    p.add_argument("--depth", type=int, default=None, metavar="N",
                   help="starting hierarchy depth (999 = full). "
                        "Default: 0 for a plain open - top geometry "
                        "plus child outline frames, the fastest "
                        "truthful first paint - and full when "
                        "--goto jumps to an inspection point. "
                        "Digits / the `d` dialog change it at runtime")
    p.add_argument("--lod", choices=("on", "off"), default="on",
                   help="starting merged geometry LOD state (default on - "
                        "the live first view needs merged variants without "
                        "a keypress; the planner reverts to exact on zoom "
                        "and probes are always exact. The viewer "
                        "button/`l` changes it live)")
    p.add_argument("--frames", choices=("on", "off"), default="on",
                   help="starting hierarchy FRAME_LAYER state (default "
                        "on; the viewer button/`h` changes it live)")
    p.add_argument("--labels", choices=("on", "off"), default="on",
                   help="enable request-scoped design text and block-name "
                        "planning (default on; forwarded to a running "
                        "instance)")
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
    p.add_argument("--layout-mode", default=None,
                   choices=("viewer", "editable"),
                   help="tile read mode (default: per-cache heuristic, "
                        "see 'floe info'; new instance only)")
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
                   help="save display-path debug dumps to /tmp/floe_*.png "
                        "(XQuartz black-view diagnosis; new instance only)")
    p.set_defaults(fn=cmd_view)

    args = ap.parse_args(argv)
    args.fn(args)


if __name__ == "__main__":
    main()
