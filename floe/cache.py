"""Spatial tile cache for large OASIS files.

`build_index` scans the source file once and produces `<src>.ice/`:

    meta.json           source fingerprint, grid geometry, layer table,
                        per-tile depth-density table, stats
    tiles_b<k>/t_<r>_<c>.oas
                        per grid tile, one OASIS per SIZE BAND k (all
                        layers, absolute coords, geometry cut at tile
                        borders): band 0 holds the largest shapes, the
                        last band the smallest (see _tile_bands). A
                        render loads only the bands whose shapes reach
                        ~cut_px pixels (default 2) at the current zoom, so
                        wide views neither parse nor draw subpixel fill
                        arrays. Empty bands/tiles have no file.
    tiles_lod/...       depth-limited companion tiles (see _tile_lod)
    skeleton.oas        structural far-zoom model (see build_skeleton)

Subsequent viewer/clip operations load only the tiles intersecting the region
of interest, so they run in milliseconds-to-seconds instead of re-parsing the
whole source file.
"""

import colorsys
import functools
import json
import math
import multiprocessing
import os
import time

print = functools.partial(print, flush=True)

import klayout.db as db

CACHE_VERSION = 8
TILE_TARGET_BYTES = 6_000_000
GRID_MIN, GRID_MAX = 4, 96
INDEX_HEARTBEAT_S = 60
# governor's per-worker memory assumption before any tile has finished:
# the highest per-worker peak measured on production content (150 MB
# chip, editable-era partitioning). Deliberately conservative - a big
# host starts budget/12 workers immediately instead of one solo probe,
# a 16 GB laptop still starts just one
MEM_PRIOR_GB = 12.0   # progress line at least this often while tiling
# size-band edges in um (ascending): 4 bands, band 0 = shapes with
# max(bbox w, h) >= 2um ... band 3 = < 0.125um
BAND_THRESHOLDS_UM = (0.125, 0.5, 2.0)


def cache_dir_for(src):
    return os.path.abspath(src) + ".ice"


def _rss_gb(pid):
    """Resident set size of a process in GB (Linux /proc, ps fallback)."""
    try:
        with open("/proc/%d/status" % pid) as f:
            for ln in f:
                if ln.startswith("VmRSS:"):
                    return int(ln.split()[1]) / 1e6   # kB -> GB
    except OSError:
        pass
    try:
        import subprocess
        out = subprocess.check_output(["ps", "-o", "rss=", "-p", str(pid)])
        return int(out.split()[0]) / 1e6
    except Exception:
        return None


def _rss_many_gb(pids):
    """{pid: RSS GB} for several processes (cheap /proc reads on Linux,
    one batched ps on Darwin)."""
    out = {}
    if not pids:
        return out
    if os.path.isdir("/proc"):
        for pid in pids:
            r = _rss_gb(pid)
            if r is not None:
                out[pid] = r
        return out
    try:
        import subprocess
        txt = subprocess.run(
            ["ps", "-o", "pid=,rss=", "-p",
             ",".join(str(p) for p in pids)],
            capture_output=True).stdout
        for ln in txt.decode().splitlines():
            f = ln.split()
            if len(f) == 2:
                out[int(f[0])] = int(f[1]) / 1e6
    except Exception:
        pass
    return out


def _total_ram_gb():
    """Physical RAM in GB (POSIX sysconf; Darwin sysctl fallback)."""
    try:
        return os.sysconf("SC_PAGE_SIZE") * os.sysconf("SC_PHYS_PAGES") / 1e9
    except (AttributeError, ValueError, OSError):
        pass
    try:
        import subprocess
        return int(subprocess.check_output(
            ["sysctl", "-n", "hw.memsize"])) / 1e9
    except Exception:
        return None


def _avail_ram_gb():
    """Memory the system can hand out without swapping, in GB (Linux
    MemAvailable; Darwin vm_stat free+inactive+speculative+purgeable)."""
    try:
        with open("/proc/meminfo") as f:
            for ln in f:
                if ln.startswith("MemAvailable:"):
                    return int(ln.split()[1]) / 1e6   # kB -> GB
    except OSError:
        pass
    try:
        import subprocess
        out = subprocess.check_output(["vm_stat"]).decode()
        page, pages = 4096, 0
        for ln in out.splitlines():
            if "page size of" in ln:
                page = int(ln.split("page size of")[1].split()[0])
            for key in ("Pages free:", "Pages inactive:",
                        "Pages speculative:", "Pages purgeable:"):
                if ln.startswith(key):
                    pages += int(ln.split(":")[1].strip().rstrip("."))
        return pages * page / 1e9
    except Exception:
        return None


def _read_monitor(pid, interval, label):
    """Child process: heartbeat while the parent sits in a long C++
    call (which holds the GIL, so an in-process thread would starve)."""
    t0 = time.time()
    while True:
        time.sleep(interval)
        rss = _rss_gb(pid)
        if rss is None:
            return
        print("[index] %s... %.1f GB RSS (%.0fs)"
              % (label, rss, time.time() - t0), flush=True)


class _phase_monitor:
    """Forked RSS/liveness heartbeat around a GIL-holding phase."""

    def __init__(self, label):
        self.label = label
        self.proc = None

    def __enter__(self):
        try:
            self.proc = multiprocessing.get_context("fork").Process(
                target=_read_monitor,
                args=(os.getpid(), INDEX_HEARTBEAT_S, self.label),
                daemon=True)
            self.proc.start()
        except Exception:
            self.proc = None
        return self

    def __exit__(self, *_exc):
        if self.proc is not None:
            self.proc.terminate()
        return False


def layer_color(i):
    """Distinct, stable per-layer color (golden-angle hue rotation)."""
    h = (i * 137.508) % 360.0 / 360.0
    r, g, b = colorsys.hsv_to_rgb(h, 0.75, 1.0)
    return "#%02x%02x%02x" % (int(r * 255), int(g * 255), int(b * 255))


def save_opts():
    opt = db.SaveLayoutOptions()
    opt.format = "OASIS"
    opt.oasis_write_cblocks = True
    opt.oasis_compression_level = 2
    opt.write_context_info = False
    return opt


def viewer_mode_preferred(meta, mode=None):
    """Pick the Layout mode for READING this cache's data.

    Repetition-heavy sources (bitcell/fill arrays) materialize far more
    shapes than file bytes; klayout's viewer (non-editable) mode keeps
    them as compact shape arrays, collapsing tile loads from ~44s to ms
    (measured). Flat sources read ~3x SLOWER in viewer mode though, so
    choose by stored-shapes-per-byte: testchip-class flat data ~0.15/B,
    array-monster data ~13/B - threshold 1.0.

    mode='viewer'|'editable' (--layout-mode) overrides the heuristic."""
    mode = (mode or "").lower()
    if mode.startswith("view"):
        return True
    if mode.startswith("edit"):
        return False
    try:
        shapes = sum(l["stored_shapes"] for l in meta["layers"])
        return shapes / max(1, meta["src"]["size"]) > 1.0
    except (KeyError, TypeError):
        return False


def pick_top_cell(ly, log=None):
    tops = ly.top_cells()
    if len(tops) > 1:
        tops = sorted(tops, key=lambda c: -c.bbox().area())
        if log:
            log(f"[warn] {len(tops)} top cells, using largest: {tops[0].name}")
    return tops[0]


class Cache:
    """Read-side accessor for a built .ice directory."""

    def __init__(self, src):
        self.src = os.path.abspath(src)
        self.dir = cache_dir_for(src)
        self.meta = None

    @property
    def meta_path(self):
        return os.path.join(self.dir, "meta.json")

    def exists(self):
        return os.path.isfile(self.meta_path)

    def load(self):
        with open(self.meta_path) as f:
            self.meta = json.load(f)
        return self.meta

    def is_stale(self):
        if self.meta.get("version") != CACHE_VERSION:
            return True
        st = os.stat(self.src)
        srcinfo = self.meta["src"]
        return (st.st_size != srcinfo["size"]
                or int(st.st_mtime) != srcinfo["mtime"])

    def tile_path(self, r, c):
        return os.path.join(self.dir, "tiles", f"t_{r}_{c}.oas")

    def band_tile_path(self, r, c, k):
        return os.path.join(self.dir, f"tiles_b{k}", f"t_{r}_{c}.oas")

    def merge_tile_path(self, r, c, k):
        """Merged twin of band k: the band's geometry fused into a few
        coarse polygons (see _merge_band). Shown instead of the raw
        band when the cut drops it; may not exist (sparse band, legacy
        cache)."""
        return os.path.join(self.dir, f"tiles_m{k}", f"t_{r}_{c}.oas")

    def n_bands(self):
        """Size bands per tile; 1 for legacy (unbanded) caches."""
        b = (self.meta or {}).get("bands")
        return len(b["thresholds_um"]) + 1 if b else 1

    def lod_tile_path(self, r, c):
        return os.path.join(self.dir, "tiles_lod", f"t_{r}_{c}.oas")

    def tiles_for_bbox(self, x0, y0, x1, y1):
        """Grid tiles (r, c) intersecting bbox in dbu (ints or floats -
        deep-zoom render requests stay float), clamped to the grid."""
        g = self.meta["grid"]
        c0 = max(0, int((x0 - g["x0"]) // g["tile_w"]))
        c1 = min(g["nx"] - 1, int((x1 - 1 - g["x0"]) // g["tile_w"]))
        r0 = max(0, int((y0 - g["y0"]) // g["tile_h"]))
        r1 = min(g["ny"] - 1, int((y1 - 1 - g["y0"]) // g["tile_h"]))
        if c1 < c0 or r1 < r0:
            return []
        return [(r, c) for r in range(r0, r1 + 1) for c in range(c0, c1 + 1)]

    def resolve_layers(self, spec):
        """Parse 'M1,5/1,MARKER' into [(layer, datatype), ...]. None = all."""
        if not spec or spec == "all":
            return None
        byname = {l["name"]: (l["layer"], l["datatype"])
                  for l in self.meta["layers"] if l.get("name")}
        out = []
        for tok in spec.split(","):
            tok = tok.strip()
            if not tok:
                continue
            if "/" in tok:
                l, d = tok.split("/")
                out.append((int(l), int(d)))
            elif tok in byname:
                out.append(byname[tok])
            else:
                raise ValueError(f"unknown layer: {tok!r} "
                                 f"(known: {sorted(byname)})")
        return out


def _scan_layers(ly, log=None):
    """One pass over every cell: per-layer stored shape counts AND the
    set of text-bearing layers (Shapes.each(STexts) is type-indexed, so
    the text probe skips polygon/box shapes even on dense fill layers).
    Both consumers used to run their own cells x layers sweep - on
    million-cell production files each sweep is minutes of silent
    Python looping, so they are merged and given a heartbeat."""
    lis = ly.layer_indexes()
    counts = {li: 0 for li in lis}
    text_layers = []
    remaining = set(lis)
    n_cells = ly.cells()
    t0 = last = time.perf_counter()
    for i, cell in enumerate(ly.each_cell()):
        for li in lis:
            counts[li] += cell.shapes(li).size()
        for li in list(remaining):
            for _ in cell.shapes(li).each(db.Shapes.STexts):
                remaining.discard(li)
                text_layers.append(li)
                break
        if log is not None and \
                time.perf_counter() - last >= INDEX_HEARTBEAT_S:
            last = time.perf_counter()
            log(f"[index] scanning layers... cell {i + 1:,}/{n_cells:,} "
                f"({last - t0:.0f}s)")
    return counts, sorted(text_layers)


def _text_layers(ly):
    """Layer indexes that hold at least one text (see _scan_layers)."""
    return _scan_layers(ly)[1]


TEXT_LAYER_CAP = 1_000_000   # per-layer full-collection budget
TEXT_TILE_CAP = 10_000       # per-tile budget for over-budget layers


def collect_texts(ly, top_ci, text_layers=None, log=None, cap=None,
                  grid=None, tile_cap=None, budget=None, jobs=None):
    """Gather text objects (any depth) with top-level coordinates.

    Returns ([(layer_index, db.Text in top coords)],
    [layer_index thinned/dropped]). The ONLY consumer is the far-view
    skeleton's label set (texts live nowhere else since 0.5.4), so
    `budget` - the skeleton label cap - sizes the whole collection:
    with a budget of B, each text layer gets a 4xB/n_layers slice
    (oversampled so the final uniform stride still has spatial
    spread) and over-budget layers keep slice/n_tiles texts per tile.
    Without the budget the old fixed caps applied and a 1,600-tile
    host chip measured 16M texts KEPT per marker layer (~10 GB and
    ~5 min each, 20 layers) only to be sampled down to 50k at the
    end. Explicit --text-cap / --text-tile-cap still win.

    Only text-bearing layers are expanded. A plain all-layers recursive
    pass expands every SRAM/fill array to reach a handful of labels
    (measured 10s for 6 texts, 724s for 1.4M on an array-heavy file);
    restricting the iterator to the text layers prunes those subtrees
    entirely (~2600x faster, identical output).

    Layers under the per-layer budget (--text-cap, 0 = unlimited) are
    collected in full - a count-only pass classifies every layer up
    front, aborting at budget+1 so even a billion-text layer costs
    seconds (a host chip hit 1.7e9 texts / 754 GB RSS before this
    existed). Over-budget layers are NOT dropped: bbox-restricted
    region queries (<=16x16 over the die) abort at the region cap, so
    coverage degrades spatially uniformly and total work is bounded
    regardless of the text count. Counting and collection fan out to
    fork workers sharing the loaded layout copy-on-write (jobs; klayout
    holds the GIL, threads cannot do this). No human decision needed;
    the thinning is logged and recorded in meta. tile_cap 0 disables
    thinning (over-budget layers dropped whole, the pre-0.5.3
    behavior); grid None likewise."""
    layers = _text_layers(ly) if text_layers is None else text_layers
    if budget and budget > 0 and layers:
        slice_ = max(1, (budget * 4) // len(layers))
        if cap is None:
            cap = slice_
        if tile_cap is None and grid is not None:
            tile_cap = max(1, slice_ // (grid["nx"] * grid["ny"]))
        if log is not None:
            log(f"[index] text budget: {budget:,} labels -> "
                f"{cap:,}/layer" + (f", {tile_cap:,}/tile for "
                                    f"over-budget layers"
                                    if tile_cap is not None else ""))
    if cap is None:
        cap = TEXT_LAYER_CAP
    if cap <= 0:
        cap = None
    if tile_cap is None:
        tile_cap = TEXT_TILE_CAP
    t0 = time.perf_counter()
    # region grid for heavy-layer thinning: per-TILE queries (1,600 on
    # the host chip) cost ~0.15s each just in hierarchy descent, and a
    # region where fewer than tile_cap texts exist walks its whole
    # subtree before giving up - a full text phase measured 3,583s.
    # Coarser regions (<=16x16) cut the query count ~6x while keeping
    # the spatial spread the skeleton labels need.
    rg = None
    if grid is not None:
        nrx = max(1, min(16, grid["nx"]))
        nry = max(1, min(16, grid["ny"]))
        rg = {"x0": grid["x0"], "y0": grid["y0"], "nx": nrx, "ny": nry,
              "w": -(-(grid["nx"] * grid["tile_w"]) // nrx),
              "h": -(-(grid["ny"] * grid["tile_h"]) // nry)}
        rcap = max(1, (tile_cap * grid["nx"] * grid["ny"])
                   // (nrx * nry))
    global _TEXT_CTX
    _TEXT_CTX = (ly, top_ci, rg)
    pool = None
    if (jobs is None or jobs > 1) and len(layers) > 1:
        try:
            ctx = multiprocessing.get_context("fork")
            pool = ctx.Pool(min(jobs or os.cpu_count() or 1,
                                len(layers)))
        except ValueError:
            pool = None    # no fork: sequential fallback below
    pmap = pool.imap_unordered if pool is not None else map
    thinned = []
    tuples = []
    try:
        # count-only classification pass (nothing stored): every
        # over-budget layer is known and reported before collection
        keep, heavy = [], []
        if cap is None:
            keep = list(layers)
        else:
            for li, cnt in pmap(_text_count_one,
                                [(li, cap) for li in layers]):
                info = ly.get_info(li)
                if cnt <= cap:
                    keep.append(li)
                    continue
                thinned.append(li)
                if rg is not None and tile_cap > 0:
                    heavy.append(li)
                    log and log(
                        f"[index] texts: layer {info.layer}"
                        f"/{info.datatype} over {cap:,} - keeping up "
                        f"to {rcap:,} per region, spatially uniform")
                else:
                    log and log(
                        f"[index] texts: layer {info.layer}"
                        f"/{info.datatype} over {cap:,} - dropped "
                        f"(no grid / thinning disabled)")
        # collection: whole light layers + per-region heavy slices run
        # as one task pool (fork workers share the layout COW; results
        # travel as plain tuples - db.Text does not pickle)
        units = [("full", li) for li in keep]
        if heavy:
            units += [("region", li, ri, rj, rcap) for li in heavy
                      for ri in range(rg["ny"]) for rj in range(rg["nx"])]
        done = 0
        last = time.perf_counter()
        per_layer = {}
        for res in pmap(_text_unit_one, units):
            tuples.extend(res)
            done += 1
            for li, _s, _x, _y in res:
                per_layer[li] = per_layer.get(li, 0) + 1
            if log is not None and \
                    time.perf_counter() - last >= INDEX_HEARTBEAT_S:
                last = time.perf_counter()
                log(f"[index] collecting texts... {len(tuples):,} kept, "
                    f"{done}/{len(units)} units ({last - t0:.0f}s)")
    finally:
        if pool is not None:
            pool.terminate()
            pool.join()
        _TEXT_CTX = None
    if log is not None:
        for i, li in enumerate(heavy):
            info = ly.get_info(li)
            log(f"[index] texts ({i + 1}/{len(heavy)} thinned): layer "
                f"{info.layer}/{info.datatype} kept "
                f"{per_layer.get(li, 0):,}")
    # deterministic output regardless of worker completion order (the
    # skeleton's stride sample must not differ between rebuilds); the
    # per-layer spatial sort also keeps the stride spread
    tuples.sort(key=lambda t: (t[0], t[2], t[3], t[1]))
    out = [(li, db.Text(s, db.Trans(db.Vector(x, y))))
           for li, s, x, y in tuples]
    return out, thinned


_TEXT_CTX = None    # (layout, top cell index, region grid) for forks


def _text_count_one(args):
    """Count texts on one layer, aborting at cap+1 (fork worker)."""
    li, cap = args
    ly, top_ci, _rg = _TEXT_CTX
    it = db.RecursiveShapeIterator(ly, ly.cell(top_ci), [li])
    it.shape_flags = db.Shapes.STexts
    cnt = 0
    while not it.at_end():
        cnt += 1
        if cnt > cap:
            break
        it.next()
    return li, cnt


def _text_unit_one(unit):
    """One collection task (fork worker): a whole under-budget layer,
    or one region slice of an over-budget layer. Returns plain
    (layer_index, string, x, y) tuples."""
    ly, top_ci, rg = _TEXT_CTX
    top = ly.cell(top_ci)
    out = []
    if unit[0] == "full":
        li = unit[1]
        it = db.RecursiveShapeIterator(ly, top, [li])
        it.shape_flags = db.Shapes.STexts
        while not it.at_end():
            t = it.shape().text.transformed(it.trans())
            p = t.trans.disp
            out.append((li, t.string, p.x, p.y))
            it.next()
        return out
    _kind, li, ri, rj, rcap = unit
    x0 = rg["x0"] + rj * rg["w"]
    y0 = rg["y0"] + ri * rg["h"]
    box = db.Box(x0, y0, x0 + rg["w"], y0 + rg["h"])
    it = db.RecursiveShapeIterator(ly, top, [li], box)
    it.shape_flags = db.Shapes.STexts
    while not it.at_end() and len(out) < rcap:
        t = it.shape().text.transformed(it.trans())
        p = t.trans.disp
        # half-open ownership: boundary texts show up in both
        # neighbours' queries - keep exactly one copy
        oj = min(rg["nx"] - 1, max(0, (p.x - rg["x0"]) // rg["w"]))
        oi = min(rg["ny"] - 1, max(0, (p.y - rg["y0"]) // rg["h"]))
        if (oi, oj) == (ri, rj):
            out.append((li, t.string, p.x, p.y))
        it.next()
    return out


def _const_pitch_runs(vals):
    """Split a sorted int array into (start, pitch, count) constant-pitch runs."""
    import numpy as np
    n = len(vals)
    if n == 1:
        return [(int(vals[0]), 0, 1)]
    d = np.diff(vals)
    runs = []
    i = 0
    while i < n:
        if i == n - 1:
            runs.append((int(vals[i]), 0, 1))
            break
        pitch = int(d[i])
        j = i + 1
        while j < n - 1 and int(d[j]) == pitch:
            j += 1
        if pitch == 0:  # duplicate points - keep as singles
            runs.append((int(vals[i]), 0, 1))
            i += 1
        else:
            runs.append((int(vals[i]), pitch, j - i + 1))
            i = j
    return runs


def _find_grids(pts):
    """Detect regular grids in an (N,2) int array of points.

    Returns (arrays, leftovers) where arrays are
    (x0, y0, xpitch, nx, ypitch, ny) and leftovers are (x, y) singles.
    """
    import numpy as np
    order = np.lexsort((pts[:, 1], pts[:, 0]))
    pts = pts[order]
    xs, starts = np.unique(pts[:, 0], return_index=True)
    bounds = list(starts[1:]) + [len(pts)]
    col_sigs = {}
    leftovers = []
    for i, x in enumerate(xs):
        ys = pts[starts[i]:bounds[i], 1]
        for y0, pitch, cnt in _const_pitch_runs(ys):
            if cnt == 1:
                leftovers.append((int(x), y0))
            else:
                col_sigs.setdefault((y0, pitch, cnt), []).append(int(x))
    arrays = []
    for (y0, ypitch, ny), xlist in col_sigs.items():
        xarr = np.asarray(sorted(xlist), dtype=np.int64)
        for x0, xpitch, nx in _const_pitch_runs(xarr):
            arrays.append((x0, y0, xpitch, nx, ypitch, ny))
    return arrays, leftovers


def _strip_texts(ly, layers=None):
    """Remove all text shapes (texts live only in the skeleton).

    Works in viewer mode too, where per-shape erase is forbidden:
    text-only containers (the normal case - marker/label layers) are
    clear()ed wholesale, mixed containers are rebuilt from their
    geometry via a Region. clip_into DOES carry source texts into the
    tile (measured - the old assumption that it drops them was wrong),
    so skipping this would leak boundary-duplicated / unthinned texts
    into band 0. `layers` restricts the sweep to known text-bearing
    layer indexes: the indexer strips the loaded SOURCE once right
    after label collection - a marker chip (1.7e9 texts) used to
    re-copy millions of texts into every tile clip and re-strip them
    1,600 times over."""
    geo_mask = db.Shapes.SAll & ~db.Shapes.STexts
    editable = ly.is_editable()
    lis = ly.layer_indexes() if layers is None else layers
    # start/end_changes batches klayout's internal updates: without it
    # every per-container modification re-triggers them and the sweep
    # goes quadratic-ish - an 80k-container marker repro measured 264s
    # bare vs 0.1s batched (the host source strip sat >20 min)
    ly.start_changes()
    try:
        for cell in ly.each_cell():
            for li in lis:
                shapes = cell.shapes(li)
                has_text = False
                for _ in shapes.each(db.Shapes.STexts):
                    has_text = True
                    break
                if not has_text:
                    continue
                if editable:
                    victims = [s for s in shapes.each(db.Shapes.STexts)]
                    for s in victims:
                        shapes.erase(s)
                    continue
                mixed = False
                for _ in shapes.each(geo_mask):
                    mixed = True
                    break
                if not mixed:
                    shapes.clear()
                    continue
                reg = db.Region()
                reg.merged_semantics = False
                reg.insert(shapes)      # Region excludes texts
                reg = reg.dup()         # detach before the clear
                shapes.clear()
                shapes.insert(reg)
    finally:
        ly.end_changes()


def compact_instances(ly, min_group=500, log=None):
    """Re-fold exploded single instances into regular CellInstArrays.

    Layout.clip explodes arrays that are cut by the clip box into individual
    placements (millions for memory arrays). Files stay small because the
    OASIS writer re-detects repetitions, but the reader materializes single
    instances again, making tile loads slow and memory-hungry. Rebuilding
    true regular arrays fixes both (regular arrays survive the roundtrip).
    """
    import numpy as np
    # batch klayout's internal updates across the per-cell instance
    # rewrites (same modification-storm as _strip_texts: measured
    # 2,640x there)
    ly.start_changes()
    try:
        _compact_instances_inner(ly, min_group, log, np)
    finally:
        ly.end_changes()


def _compact_instances_inner(ly, min_group, log, np):
    for cell in ly.each_cell():
        n_inst = cell.child_instances()
        if n_inst < min_group:
            continue
        groups = {}
        keep = []
        for inst in cell.each_inst():
            ia = inst.cell_inst
            if inst.is_complex() or ia.na > 1 or ia.nb > 1:
                keep.append(ia)
                continue
            t = inst.trans
            groups.setdefault((inst.cell_index, t.rot),
                              []).append((t.disp.x, t.disp.y))
        if not groups or max(len(v) for v in groups.values()) < min_group:
            continue
        rebuilt = []
        n_before = 0
        for (ci, rot), pts in groups.items():
            n_before += len(pts)
            base = db.Trans(rot % 4, rot >= 4, 0, 0)
            if len(pts) < min_group:
                for x, y in pts:
                    rebuilt.append(db.CellInstArray(
                        ci, db.Trans(rot % 4, rot >= 4, x, y)))
                continue
            arrays, singles = _find_grids(
                np.asarray(pts, dtype=np.int64))
            for x0, y0, xp, nx, yp, ny in arrays:
                tr = db.Trans(rot % 4, rot >= 4, x0, y0)
                if nx == 1 and ny == 1:
                    rebuilt.append(db.CellInstArray(ci, tr))
                else:
                    rebuilt.append(db.CellInstArray(
                        ci, tr, db.Vector(xp, 0), db.Vector(0, yp), nx, ny))
            for x, y in singles:
                rebuilt.append(db.CellInstArray(
                    ci, db.Trans(rot % 4, rot >= 4, x, y)))
        cell.clear_insts()
        for ia in keep:
            cell.insert(ia)
        for ia in rebuilt:
            cell.insert(ia)
        if log:
            log(f"[index]   compacted {cell.name}: {n_before} -> "
                f"{len(rebuilt)} instances")


def _skel_harvest(ly, dmaps, stop_cell, cell, trans, min_feat, big, n,
                  cap, level):
    """Copy large stored shapes of big cells into the skeleton,
    transformed to top coordinates, onto the twin layer of the level
    the shape lives at (so the far view can honor depth 1 vs 2)."""
    dmap = dmaps[level]
    for li in ly.layer_indexes():
        shapes = cell.shapes(li)
        if shapes.size() == 0 or shapes.size() > 60_000:
            continue  # fill containers hold millions; skip wholesale
        dst = stop_cell.shapes(dmap[li])
        for sh in shapes.each():
            if n >= cap:
                return n
            if sh.is_text():
                continue
            b = sh.bbox()
            if b.width() >= min_feat or b.height() >= min_feat:
                poly = sh.polygon
                if poly is not None:
                    dst.insert(poly.transformed(trans))
                    n += 1
    if level >= max(dmaps):
        return n
    for inst in cell.each_inst():
        if n >= cap:
            break
        child = ly.cell(inst.cell_index)
        cb = child.bbox()
        if cb.width() < big and cb.height() < big:
            continue
        # viewer-mode reads keep instance arrays compact: one Instance
        # may be a whole array, so walk every member placement
        for t in inst.cell_inst.each_trans():
            if n >= cap:
                break
            n = _skel_harvest(ly, dmaps, stop_cell, child,
                              trans * t, min_feat, big, n, cap,
                              level + 1)
    return n


SKEL_DETAIL_DT = 30000  # datatype offset per level of detail twin layers
SKEL_DETAIL_LEVELS = 2  # harvest big shapes from cells this deep
SKEL_TEXT_CAP = 50_000  # total label texts kept (--skel-texts)


def build_skeleton(ly, top, texts, out_path, log=print, skel_texts=None):
    """Structural far-zoom model, written as a tiny skeleton.oas.

    The depth-0 content of the far view (large top-level shapes,
    outline boxes + names of first-level cells on the synthetic layer
    255/0 OUTLINE) sits on the design layers; large shapes stored in
    big level-k cells (power straps, long routes, seal ring; k <=
    SKEL_DETAIL_LEVELS) sit on per-level twin layers (datatype + k *
    SKEL_DETAIL_DT), text labels on the level-1 twin. The render
    service turns level-k twins visible only for far views at depth >=
    k, so the far view honors depth 0/1/2 consistently with the live
    render. A layer split, not a cell split: klayout labels cells cut
    by a hierarchy limit, which would stamp a bogus name across the
    die. Small enough for the render service to load whole at startup,
    so far-zoom views render live and crisp at any scale.
    """
    bbox = top.bbox()
    min_feat = max(1, max(bbox.width(), bbox.height()) // 500)
    big = min_feat * 4
    skel = db.Layout()
    skel.dbu = ly.dbu
    stop_cell = skel.create_cell("SKEL_TOP")
    outline_li = skel.layer(db.LayerInfo(255, 0, "OUTLINE"))
    lmap = {}
    dmaps = {k: {} for k in range(1, SKEL_DETAIL_LEVELS + 1)}
    for li in ly.layer_indexes():
        info = ly.get_info(li)
        lmap[li] = skel.layer(info)
        for k in dmaps:
            dmaps[k][li] = skel.layer(db.LayerInfo(
                info.layer, info.datatype + k * SKEL_DETAIL_DT))
    cap = 300_000
    n = 0
    for li in ly.layer_indexes():
        dst = stop_cell.shapes(lmap[li])
        for sh in top.shapes(li).each():
            if sh.is_text():
                continue
            b = sh.bbox()
            if b.width() >= min_feat or b.height() >= min_feat:
                poly = sh.polygon
                if poly is not None:
                    dst.insert(poly)
                    n += 1
    for inst in top.each_inst():
        if n >= cap:
            log(f"[index] skeleton capped at {cap} shapes")
            break
        child = ly.cell(inst.cell_index)
        cb = child.bbox()
        if cb.width() < big and cb.height() < big:
            continue
        # per member placement: a viewer-mode read keeps instance
        # arrays compact, and the array-wide bbox would paint one
        # die-sized outline instead of one box per block
        for t in inst.cell_inst.each_trans():
            if n >= cap:
                log(f"[index] skeleton capped at {cap} shapes")
                break
            gb = cb.transformed(t)
            stop_cell.shapes(outline_li).insert(gb)
            c = gb.center()
            stop_cell.shapes(outline_li).insert(
                db.Text(child.name, db.Trans(db.Vector(c.x, c.y))))
            n = _skel_harvest(ly, dmaps, stop_cell, child, t,
                              min_feat, big, n, cap, 1)
    # label texts (level-1 twin): the skeleton renders live at every
    # far scale, so bound the total label count - a uniform stride
    # over the (already per-tile-thinned) collection keeps the spread
    tcap = SKEL_TEXT_CAP if skel_texts is None else skel_texts
    if tcap > 0 and len(texts) > tcap:
        step = len(texts) / tcap
        texts = [texts[int(i * step)] for i in range(tcap)]
        log(f"[index] skeleton labels sampled down to {tcap:,}")
    for li, text in texts:
        stop_cell.shapes(dmaps[1][li]).insert(text)
    skel.write(out_path, save_opts())
    return {"file": os.path.basename(out_path), "shapes": n,
            "texts": len(texts)}


def add_skeleton(cache, log=print, text_cap=None, text_tile_cap=None,
                 skel_texts=None, jobs=None):
    """Upgrade an existing cache in place (one source read, no re-tiling):
    floe index --skeleton-only."""
    t0 = time.perf_counter()
    meta = cache.meta or cache.load()
    log(f"[index] reading {cache.src} for skeleton...")
    with _phase_monitor("reading"):
        ly = db.Layout(not viewer_mode_preferred(meta))
        ly.read(cache.src)
    top = pick_top_cell(ly, log)
    with _phase_monitor("collecting texts"):
        texts, thinned = collect_texts(
            ly, top.cell_index(), log=log, cap=text_cap,
            grid=meta["grid"], tile_cap=text_tile_cap,
            budget=SKEL_TEXT_CAP if skel_texts is None else skel_texts,
            jobs=jobs)
    out = os.path.join(cache.dir, "skeleton.oas")
    meta["skeleton"] = build_skeleton(ly, top, texts, out, log,
                                      skel_texts=skel_texts)
    meta.pop("texts_dropped", None)
    meta.pop("texts_thinned", None)
    if thinned:
        meta["texts_thinned"] = [
            {"layer": ly.get_info(li).layer,
             "datatype": ly.get_info(li).datatype} for li in thinned]
    with open(cache.meta_path, "w") as f:
        json.dump(meta, f, indent=1)
    log(f"[index] skeleton added: {meta['skeleton']['shapes']} shapes, "
        f"{meta['skeleton'].get('texts', 0)} labels "
        f"({time.perf_counter() - t0:.0f}s)")


def rebuild_texts(cache, log=print, text_cap=None, text_tile_cap=None,
                  skel_texts=None, jobs=None):
    """Refresh the text handling of an existing banded cache in place
    (floe index --texts-only), NO re-tiling: strips any texts still
    living in the b0 tiles (pre-0.5.4 caches carried them - texts now
    exist only as far-view skeleton labels) and rebuilds the skeleton
    with a fresh bounded collection. Combine with --text-cap /
    --text-tile-cap / --skel-texts to change label budgets."""
    t_all = time.perf_counter()
    meta = cache.meta or cache.load()
    if not meta.get("bands"):
        raise SystemExit("floe: --texts-only supports banded caches "
                         "(this cache is legacy single-file tiles; "
                         "reindex instead)")
    g = meta["grid"]
    b0dir = os.path.join(cache.dir, "tiles_b0")
    stripped = removed = 0
    for r in range(g["ny"]):
        for c in range(g["nx"]):
            path = os.path.join(b0dir, f"t_{r}_{c}.oas")
            if not os.path.isfile(path):
                continue
            bly = db.Layout(True)   # editable: text strip needs erase
            bly.read(path)
            had = any(True for cc in bly.each_cell()
                      for li in bly.layer_indexes()
                      for _ in cc.shapes(li).each(db.Shapes.STexts))
            if not had:
                bly._destroy()
                continue
            _strip_texts(bly)
            has_any = any(cc.shapes(li).size()
                          for cc in bly.each_cell()
                          for li in bly.layer_indexes())
            if has_any:
                bly.write(path, save_opts())
                stripped += 1
            else:
                os.remove(path)   # only texts lived here
                removed += 1
            bly._destroy()
    if stripped or removed:
        log(f"[index] b0 texts stripped: {stripped} tiles rewritten, "
            f"{removed} text-only tiles removed")
    # skeleton rebuild carries the fresh labels + meta update
    add_skeleton(cache, log, text_cap=text_cap,
                 text_tile_cap=text_tile_cap, skel_texts=skel_texts,
                 jobs=jobs)
    log(f"[index] texts refreshed ({time.perf_counter() - t_all:.0f}s)")


_MERGE_CTX = None


def _merge_one_tile(rc):
    """Build the merged twins of one tile from its existing band files
    (fork worker; no source layout involved)."""
    cdir, th_dbu, opts = _MERGE_CTX
    r, c = rc
    nb = len(th_dbu) + 1
    edges = [0] + list(th_dbu) + [None]
    made = 0
    for k in range(1, nb):
        path = os.path.join(cdir, f"tiles_b{k}", f"t_{r}_{c}.oas")
        if not os.path.isfile(path):
            continue
        bly = db.Layout(False)   # viewer read: arrays stay compact
        bly.read(path)
        btop = bly.cell(f"TILE_{r}_{c}_b{k}")
        if btop is not None:
            made += bool(_merge_band(
                bly, btop, r, c, k, edges[nb - k],
                os.path.join(cdir, f"tiles_m{k}", f"t_{r}_{c}.oas"),
                opts))
        bly._destroy()
    return made


def rebuild_merge(cache, log=print, jobs=None):
    """Add merged twins to an existing banded cache in place (floe
    index --merge-only): band files are read back tile by tile, no
    source read, no re-tiling. Workers are plain forks (the per-band
    Region is the only real memory)."""
    t0 = time.perf_counter()
    meta = cache.meta or cache.load()
    if not meta.get("bands"):
        raise SystemExit("floe: --merge-only needs a banded cache "
                         "(this cache is legacy single-file tiles; "
                         "reindex instead)")
    th_dbu = [t / meta["dbu"] for t in meta["bands"]["thresholds_um"]]
    nb = len(th_dbu) + 1
    for k in range(1, nb):
        d = os.path.join(cache.dir, f"tiles_m{k}")
        os.makedirs(d, exist_ok=True)
        for f in os.listdir(d):
            os.remove(os.path.join(d, f))
    g = meta["grid"]
    coords = [(r, c) for r in range(g["ny"]) for c in range(g["nx"])]
    global _MERGE_CTX
    _MERGE_CTX = (cache.dir, th_dbu, save_opts())
    if jobs is None:
        jobs = os.cpu_count() or 1
    jobs = max(1, min(jobs, len(coords)))
    made = done = 0
    step = max(1, len(coords) // 10)

    def note():
        if done % step == 0 or done == len(coords):
            log(f"[index] merge twins {done}/{len(coords)} tiles "
                f"({time.perf_counter() - t0:.0f}s)")
    try:
        ctx = None
        if jobs > 1:
            try:
                ctx = multiprocessing.get_context("fork")
            except ValueError:
                pass
        if ctx is not None:
            with ctx.Pool(jobs, maxtasksperchild=1) as pool:
                for m in pool.imap_unordered(_merge_one_tile, coords):
                    made += m
                    done += 1
                    note()
        else:
            for rc in coords:
                made += _merge_one_tile(rc)
                done += 1
                note()
    finally:
        _MERGE_CTX = None
    meta["bands"]["merge"] = {"close_frac": MERGE_CLOSE_FRAC}
    with open(cache.meta_path, "w") as f:
        json.dump(meta, f, indent=1)
    log(f"[index] merged twins added: {made} band files "
        f"({time.perf_counter() - t0:.0f}s)")


def load_region(cache, x0, y0, x1, y1, log=None, max_tiles=None,
                layers=None):
    """Load tiles intersecting bbox (dbu) into a fresh mosaic Layout.

    Returns (layout, top_cell, n_tiles_loaded). Tile geometry keeps absolute
    coordinates, so tiles are instantiated at identity transform.
    `layers`: optional [(layer, datatype), ...] to read only those layers
    from the tile files (big speed/memory win for layer extraction).
    """
    tiles = cache.tiles_for_bbox(x0, y0, x1, y1)
    if max_tiles is not None and len(tiles) > max_tiles:
        raise RuntimeError(
            f"region spans {len(tiles)} tiles (> max {max_tiles}); "
            f"narrow the bbox or raise --max-tiles")
    lo = None
    if layers is not None:
        lm = db.LayerMap()
        for i, (l, d) in enumerate(layers):
            lm.map(db.LayerInfo(l, d), i)
        lo = db.LoadLayoutOptions()
        lo.set_layer_map(lm, False)
    ly = db.Layout(not viewer_mode_preferred(
        cache.meta, getattr(cache, "layout_mode", None)))
    ly.dbu = cache.meta["dbu"]
    top = ly.create_cell("FLOE_REGION")
    n = 0
    t0 = time.perf_counter()
    banded = bool(cache.meta.get("bands"))
    for r, c in tiles:
        if banded:  # every band together = the exact tile content
            parts = [(cache.band_tile_path(r, c, k), f"TILE_{r}_{c}_b{k}")
                     for k in range(cache.n_bands())]
        else:
            parts = [(cache.tile_path(r, c), f"TILE_{r}_{c}")]
        got = False
        for p, cellname in parts:
            if not os.path.isfile(p):
                continue  # empty tile/band
            if lo is not None:
                ly.read(p, lo)
            else:
                ly.read(p)
            cell = ly.cell(cellname)
            if cell is None:
                continue
            top.insert(db.CellInstArray(cell.cell_index(), db.Trans()))
            got = True
        n += got
    if log:
        log(f"[view] loaded {n}/{len(tiles)} tiles "
            f"in {time.perf_counter() - t0:.2f}s")
    return ly, top, n


LOD_SHAPE_CAP = 50_000  # per-tile shape budget of the LOD companion


def _tile_lod(tgt, top_ci, out_path, cap=LOD_SHAPE_CAP, always=False,
              lis=None):
    """Depth-limited companion tile, cut adaptively: whole hierarchy
    levels are kept while the running distinct-cell shape total stays
    under cap; the cells of the first level beyond become ghosts (bbox
    on the synthetic layer 254/0, so depth-cut renders still draw the
    correct outline frame + name) and deeper cells are dropped.
    Kilobytes where the full tile is megabytes - shallow-depth renders
    load these instead. Built by dup + prune, so no per-shape Python
    loop. Returns the deepest depth the file serves, or None when the
    whole tree fits under cap (then no file is written and the full
    tile doubles as its own LOD). always: write the file even then (a
    banded cache has no single full-tile file to fall back on)."""
    lod = tgt.dup()
    lvl = {top_ci: 0}
    levels = [[top_ci]]
    while True:
        nxt = []
        for ci in levels[-1]:
            for inst in lod.cell(ci).each_inst():
                ch = inst.cell_index
                if ch not in lvl:
                    lvl[ch] = len(levels)
                    nxt.append(ch)
        if not nxt:
            break
        levels.append(nxt)
    if lis is None:
        lis = list(lod.layer_indexes())

    def count(cells):
        return sum(lod.cell(ci).shapes(li).size()
                   for ci in cells for li in lis)

    cut = 0
    cum = count(levels[0])
    while cut + 1 < len(levels):
        cum += count(levels[cut + 1])
        if cum > cap:
            break
        cut += 1
    if cut + 1 >= len(levels):
        if always:  # whole tree fits under cap: LOD = the full tile
            lod.write(out_path, save_opts())
        return None
    ghost_li = lod.layer(db.LayerInfo(254, 0, "GHOST"))
    # batch the per-cell ghosting mutations (update-storm class, see
    # _strip_texts)
    lod.start_changes()
    try:
        for ci in levels[cut + 1]:
            c = lod.cell(ci)
            b = c.bbox()
            c.clear()
            if not b.empty():
                c.shapes(ghost_li).insert(b)
    finally:
        lod.end_changes()
    doomed = [ci for ci, l in lvl.items() if l > cut + 1]
    if doomed:
        lod.delete_cells(doomed)
    lod.write(out_path, save_opts())
    return cut


# merged-twin knobs: closing distance = frac x band upper edge (fuses
# gaps the twin's display scales cannot resolve anyway); a tile-layer
# still holding more polygons than the cap after closing is sparse
# scatter - a twin would not pay, skip it (the raw band stays available)
MERGE_CLOSE_FRAC = 0.5
MERGE_POLY_CAP = 4096
# bands up to this many members build their twin from exact geometry;
# above it the envelope walk below stands in (expanding hundreds of
# millions of fill members just to fuse them again measured 9x total
# index time - the exact thing the band partitioner avoids)
MERGE_EXPAND_CAP = 500_000
_CONT_FAN = 4096        # own-container members worth expanding
_INST_FAN = 64          # instance-array members worth placing singly


def _twin_boxes(bly, btop, lis=None):
    """{layer_index: [Box, ...]} coarse coverage of a band layout in
    tile coordinates, WITHOUT expanding shape or instance arrays:
    small containers contribute member bboxes, huge ones (fill fields)
    their subtree envelope, and big instance arrays one per-layer
    array bbox (cached in C++). Dense content - the only content that
    needs a twin - is envelope-faithful; sparse scatter overstates,
    which the polygon cap downstream already treats as 'no twin'.
    lis: restrict to these layer indexes (a 449-layer host chip paid
    449 probes per cell for the ~dozens of layers a band holds)."""
    if lis is None:
        lis = bly.layer_indexes()
    memo = {}

    def walk(ci):
        out = memo.get(ci)
        if out is not None:
            return out
        cell = bly.cell(ci)
        out = {}
        for li in lis:
            sh = cell.shapes(li)
            n = sh.size()
            if not n:
                continue
            if n > _CONT_FAN:
                b = cell.bbox_per_layer(li)
                if not b.empty():
                    out.setdefault(li, []).append(b)
            else:
                dst = out.setdefault(li, [])
                for s in sh.each():
                    dst.append(s.bbox())
        for inst in cell.each_inst():
            ia = inst.cell_inst
            child = walk(ia.cell_index)
            if not child:
                continue
            if ia.size() > _INST_FAN:
                for li in child:
                    b = ia.bbox(bly, li)
                    if not b.empty():
                        out.setdefault(li, []).append(b)
            else:
                for t in ia.each_trans():
                    for li, boxes in child.items():
                        dst = out.setdefault(li, [])
                        for b in boxes:
                            dst.append(b.transformed(t))
        for dst in out.values():
            if len(dst) > 4 * MERGE_POLY_CAP:
                env = db.Box()      # runaway scatter: collapse early
                for b in dst:
                    env += b
                dst[:] = [env]
        memo[ci] = out
        return out

    return walk(btop.cell_index())


def _merge_band(bly, btop, r, c, k, upper_dbu, out_path, opts,
                members=None, lis=None):
    """Write the merged twin of one band: geometry (exact when small,
    envelope-walked when huge - see _twin_boxes) with gaps below
    MERGE_CLOSE_FRAC x band-upper-edge closed (sized +d/-d fuses fill
    fields into slabs - klayout merge alone only joins TOUCHING
    polygons, and dummy fill never touches), staircase vertices
    smoothed away. The twin substitutes the raw band on views where
    the cut would hide it: same layers, few big polygons, so layer
    colors/toggles keep working (no prerendering). Returns polygons
    written (0 = no file)."""
    if lis is None:
        lis = bly.layer_indexes()
    if members is None:
        members = sum(cell.shapes(li).size() for cell in bly.each_cell()
                      for li in lis)
    d = max(1, int(upper_dbu * MERGE_CLOSE_FRAC))
    boxes = (None if members <= MERGE_EXPAND_CAP
             else _twin_boxes(bly, btop, lis))
    mly = db.Layout(True)
    mly.dbu = bly.dbu
    mc = mly.create_cell(f"TILE_{r}_{c}_m{k}")
    total = 0
    for li in lis:
        if boxes is None:
            reg = db.Region(bly.begin_shapes(btop, li))
        else:
            reg = db.Region()
            for b in boxes.get(li, ()):
                reg.insert(b)
        if reg.is_empty():
            continue
        reg = reg.sized(d).sized(-d)            # closing
        reg = reg.smoothed(max(1, d // 4))      # sub-quarter-px at use
        n = reg.count()
        if not n or n > MERGE_POLY_CAP:
            continue
        mc.shapes(mly.layer(bly.get_info(li))).insert(reg)
        total += n
    if total:
        mly.write(out_path, opts)
    mly._destroy()
    return total


def _tile_bands(tgt, cdir, r, c, th_dbu, opts, merge=True, lis=None):
    """Partition the tile into len(th_dbu)+1 SIZE-BAND files.

    Band k holds shapes whose max(bbox w, h) falls in
    [edge[n-1-k], edge[n-k]) - band 0 the largest, the last band the
    smallest. The cell tree and instances are mirrored into every band
    (cell names get a __b<k> suffix so the mosaic's multi-read cannot
    merge different bands' same-named cells), so depth semantics are
    identical per band and the union of all bands is the exact tile.

    Uniform containers - fill/array cells, the bulk of production
    content - are detected by testing whether the first member's band
    holds every member; if so the whole container is copied with
    Shapes.insert(Shapes), which preserves records (shape arrays stay
    arrays, boxes stay boxes): no expansion into the band layout, no
    polygon conversion, near-zero time and memory. Mixed containers
    fall back to db.Region bbox filters (those come back as polygons -
    geometry identical, measured). Tiles carry NO texts (the caller
    strips them; the far-view skeleton is the only text consumer).
    Bands with no shapes write no file. Returns per-band counts."""
    nb = len(th_dbu) + 1
    edges = [0] + list(th_dbu) + [None]

    def band_of(w, h):
        s = w if w >= h else h
        for k in range(nb - 1, 0, -1):
            if s < edges[nb - k]:
                return k
        return 0

    top_name = f"TILE_{r}_{c}"
    top_ci = tgt.cell(top_name).cell_index()
    if lis is None:
        lis = tgt.layer_indexes()

    # ---- pass 1: probe every container ------------------------------
    # sampled uniformity: fill/array containers hold one size class,
    # and a mixed container almost surely reveals itself within the
    # first few members (microseconds per container). The probe also
    # decides the band-layout mode below: uniform-dominant tiles get
    # viewer-mode band layouts, where the wholesale record copies
    # stay compact (arrays stay arrays - half the RAM, measured);
    # mixed-dominant tiles get editable ones, because accumulating
    # millions of flat Region polygons in viewer containers measured
    # ~45% more RAM and ~20% more time on an adversarial tile.
    probes = {}                 # (ci, li) -> (k0 or None, complete)
    uni_members = mix_members = 0
    for cell in tgt.each_cell():
        ci = cell.cell_index()
        for li in lis:
            sh = cell.shapes(li)
            n = sh.size()
            if n == 0:
                continue
            has_text = False
            for _ in sh.each(db.Shapes.STexts):
                has_text = True
                break
            k0 = None
            seen = 0
            if not has_text:
                for s in sh.each():
                    b = s.bbox()
                    kk = band_of(b.width(), b.height())
                    if k0 is None:
                        k0 = kk
                    elif kk != k0:
                        k0 = None
                        break
                    seen += 1
                    if seen >= 256:
                        break
            probes[(ci, li)] = (k0, k0 is not None and seen >= n)
            if k0 is not None:
                uni_members += n
            else:
                mix_members += n
    band_viewer = uni_members >= mix_members

    blys, cmap = [], []
    for k in range(nb):
        b = db.Layout(not band_viewer)
        b.dbu = tgt.dbu
        for li in tgt.layer_indexes():
            b.insert_layer_at(li, tgt.get_info(li))
        blys.append(b)
        cmap.append({})
    for cell in tgt.each_cell():
        for k in range(nb):
            nm = (f"{top_name}_b{k}" if cell.name == top_name
                  else f"{cell.name}__b{k}")
            cmap[k][cell.cell_index()] = blys[k].create_cell(nm).cell_index()
    # member counts per band, and which band cells got shapes: tracked
    # while partitioning, because Shapes.size() on freshly filled
    # viewer-mode containers re-runs their update pass per call
    # (a plain per-band size() sweep measured 3.3s/tile)
    bcount = [0] * nb
    filled = [set() for _ in range(nb)]
    flayers = [set() for _ in range(nb)]   # layers with content per band

    def put(k, ci_, li_, obj, members):
        blys[k].cell(cmap[k][ci_]).shapes(li_).insert(obj)
        bcount[k] += members
        filled[k].add(cmap[k][ci_])
        flayers[k].add(li_)

    # ---- pass 2: partition -------------------------------------------
    for cell in tgt.each_cell():
        ci = cell.cell_index()
        for inst in cell.each_inst():
            ia = inst.cell_inst
            for k in range(nb):
                ba = ia.dup()
                ba.cell_index = cmap[k][ia.cell_index]
                blys[k].cell(cmap[k][ci]).insert(ba)
        for li in lis:
            sh = cell.shapes(li)
            n = sh.size()
            if n == 0:
                continue
            k0, complete = probes[(ci, li)]
            if complete:
                # the probe covered every member: uniform proven
                put(k0, ci, li, sh, n)
                continue
            reg = db.Region()
            reg.merged_semantics = False
            reg.insert(sh)          # texts are excluded by Region
            rest = range(nb)
            if k0 is not None:
                # sample says uniform: one confirming filter pass
                lo, hi = edges[nb - 1 - k0], edges[nb - k0]
                p0 = reg.with_bbox_max(lo, hi, False)
                c0 = p0.count()
                if c0 == n:
                    put(k0, ci, li, sh, n)
                    continue
                if c0:
                    put(k0, ci, li, p0, c0)
                rest = [k for k in range(nb) if k != k0]
            for k in rest:
                lo, hi = edges[nb - 1 - k], edges[nb - k]
                part = reg.with_bbox_max(lo, hi, False)
                np_ = part.count()
                if np_:
                    put(k, ci, li, part, np_)
    counts = []
    t_merge = 0.0
    for k in range(nb):
        bly = blys[k]
        # drop cells whose subtree holds no shapes in this band: their
        # duplicated instance records would otherwise bloat every band
        # file (~+35% on std-cell-heavy layouts, measured). Uses the
        # filled-cell sets tracked during partitioning - no size()
        # sweeps over the band layouts.
        has = {}
        for bci in bly.each_cell_bottom_up():
            has[bci] = (bci in filled[k]
                        or any(has.get(inst.cell_index, False)
                               for inst in bly.cell(bci).each_inst()))
        btop = cmap[k][top_ci]
        doomed = [bci for bci, h in has.items() if not h and bci != btop]
        if doomed:
            bly.delete_cells(doomed)
        counts.append(bcount[k])
        if bcount[k]:
            bly.write(
                os.path.join(cdir, f"tiles_b{k}", f"t_{r}_{c}.oas"), opts)
            if merge and k >= 1:    # band 0 is never cut - no twin
                t = time.perf_counter()
                _merge_band(bly, bly.cell(btop), r, c, k, edges[nb - k],
                            os.path.join(cdir, f"tiles_m{k}",
                                         f"t_{r}_{c}.oas"), opts,
                            members=bcount[k],
                            lis=sorted(flayers[k]))
                t_merge += time.perf_counter() - t
        bly._destroy()
    return counts, t_merge


DENSITY_LEVELS = 12     # depth levels recorded in the per-tile density table


def _tile_density(ly, top_ci, max_levels=DENSITY_LEVELS):
    """Density table for one tile: cumulative shape counts per hierarchy
    level below the tile top, per layer ({"5/1": [n_depth0, ...]}), plus
    "cells" = instance count entering each level. Level k equals the
    viewer's depth k; content deeper than max_levels folds into the last
    shape entry, so it always holds the tile's full total. The viewer
    picks its auto depth from this table without loading any tile: the
    cost of depth d is shapes down to d plus one outline frame per cell
    at level d+1 ("cells" catches the bitcell-array trap where a mid
    depth draws millions of frames)."""
    keys = {li: f"{ly.get_info(li).layer}/{ly.get_info(li).datatype}"
            for li in ly.layer_indexes()}
    total = dict.fromkeys(keys.values(), 0)
    counts = {key: [] for key in keys.values()}
    cells = []
    level, depth = {top_ci: 1}, 0
    while level:
        if depth <= max_levels:
            cells.append(sum(level.values()))
        nxt = {}
        for ci, mult in level.items():
            cell = ly.cell(ci)
            for li, key in keys.items():
                n = cell.shapes(li).size()
                if n:
                    total[key] += n * mult
            for inst in cell.each_inst():
                nxt[inst.cell_index] = nxt.get(inst.cell_index, 0) \
                    + inst.size() * mult
        if depth < max_levels:
            for key in counts:
                counts[key].append(total[key])
        level, depth = nxt, depth + 1
    for key, arr in counts.items():
        arr[-1] = total[key]
    out = {key: arr for key, arr in counts.items() if arr[-1]}
    if out:
        out["cells"] = cells
    return out


def _sample_tile(cache, rc, shape_cap=2000):
    """Structure census of one tile file: instance stats (singles vs
    arrays), per-level cell counts, per-layer shape-type mix with polygon
    vertex counts. Numbers only - no geometry leaves this function."""
    r, c = (int(v) for v in rc.split(","))
    ly = db.Layout(False)  # read-only: viewer mode
    if cache.meta.get("bands"):
        got = False
        for k in range(cache.n_bands()):
            p = cache.band_tile_path(r, c, k)
            if os.path.isfile(p):
                ly.read(p)
                got = True
        if not got:
            raise RuntimeError(f"tile {rc}: no band files")
    else:
        ly.read(cache.tile_path(r, c))
    singles = arrays = elems = 0
    top_arrays = []
    for cell in ly.each_cell():
        for inst in cell.each_inst():
            ia = inst.cell_inst
            if ia.na > 1 or ia.nb > 1:
                arrays += 1
                elems += ia.na * ia.nb
                top_arrays.append(ia.na * ia.nb)
            else:
                singles += 1
    top_arrays = sorted(top_arrays, reverse=True)[:5]
    mix = {}
    for li in ly.layer_indexes():
        info = ly.get_info(li)
        key = f"{info.layer}/{info.datatype}"
        n_box = n_poly = n_path = n_text = 0
        pts = []
        seen = 0
        for cell in ly.each_cell():
            shapes = cell.shapes(li)
            if shapes.size() == 0:
                continue
            for sh in shapes.each():
                if sh.is_box():
                    n_box += 1
                elif sh.is_text():
                    n_text += 1
                elif sh.is_path():
                    n_path += 1
                else:
                    n_poly += 1
                    poly = sh.polygon
                    if poly is not None:
                        pts.append(poly.num_points_hull())
                seen += 1
                if seen >= shape_cap:
                    break
            if seen >= shape_cap:
                break
        tot = n_box + n_poly + n_path + n_text
        if tot:
            pts.sort()
            mix[key] = {
                "box": round(n_box / tot, 3),
                "polygon": round(n_poly / tot, 3),
                "path": round(n_path / tot, 3),
                "text": round(n_text / tot, 3),
                "poly_pts_p50": pts[len(pts) // 2] if pts else 4,
                "poly_pts_max": pts[-1] if pts else 4,
            }
    return {"rc": rc, "cells": ly.cells(),
            "insts": {"singles": singles, "arrays": arrays,
                      "array_elems": elems, "largest_arrays": top_arrays},
            "shape_mix": mix}


def profile_cache(cache, sample_tiles=4, anon=False, log=print):
    """Structure-only profile of an indexed layout: everything
    tools/gen_from_profile.py needs to synthesize a render-performance
    lookalike, and nothing else - counts, sizes and grid numbers, no
    geometry or coordinates; --anon also drops the layer names."""
    meta = cache.meta
    g = meta["grid"]
    layers = [{"layer": l["layer"], "datatype": l["datatype"],
               "name": ("L%d_%d" % (l["layer"], l["datatype"])) if anon
                       else l["name"],
               "stored_shapes": l["stored_shapes"]}
              for l in meta["layers"]]
    tile_sizes = {}
    lod_sizes = {}
    banded = bool(meta.get("bands"))
    for r in range(g["ny"]):
        for c in range(g["nx"]):
            if banded:  # per-tile total across the size-band files
                sz = sum(os.path.getsize(p) for p in
                         (cache.band_tile_path(r, c, k)
                          for k in range(cache.n_bands()))
                         if os.path.isfile(p))
                if sz:
                    tile_sizes[f"{r},{c}"] = sz
            else:
                p = cache.tile_path(r, c)
                if os.path.isfile(p):
                    tile_sizes[f"{r},{c}"] = os.path.getsize(p)
            p = cache.lod_tile_path(r, c)
            if os.path.isfile(p):
                lod_sizes[f"{r},{c}"] = os.path.getsize(p)
    dens = (meta.get("density") or {}).get("tiles", {})

    def tile_total(rc):
        t = dens.get(rc) or {}
        return sum(arr[-1] for k, arr in t.items() if k != "cells")

    ranked = sorted(tile_sizes, key=tile_total, reverse=True)
    picks = []
    if ranked and sample_tiles > 0:
        idx = sorted({0, len(ranked) // 4, len(ranked) // 2,
                      len(ranked) - 1})
        picks = [ranked[i] for i in idx][:sample_tiles]
    samples = []
    for rc in picks:
        log(f"[profile] sampling tile {rc}...")
        try:
            samples.append(_sample_tile(cache, rc))
        except Exception as e:
            log(f"[profile][warn] sample {rc} failed: {e}")
    return {
        "profile_version": 1,
        "dbu": meta["dbu"],
        "bbox": meta["bbox"],
        "grid": g,
        "layers": layers,
        "density": meta.get("density"),
        "lod": meta.get("lod"),
        "skeleton": {"shapes": (meta.get("skeleton") or {}).get("shapes")},
        "stats": meta.get("stats"),
        "tile_sizes": tile_sizes,
        "lod_sizes": lod_sizes,
        "samples": samples,
    }


# Tile-build context inherited by fork workers. klayout holds the GIL
# during C++ calls, so threads cannot parallelize tiling; fork workers
# share the loaded source layout copy-on-write instead (no re-read, no
# extra resident memory per worker).
_TILE_CTX = None


def _build_one_tile(rc):
    """Build tile (r, c) from _TILE_CTX. Runs in a fork worker (or inline
    for jobs=1). Returns (r, c, wrote, lod_depth, density, step_times)."""
    (ly, top_ci, bbox, grid, cdir, opts, bands_dbu, tile_editable,
     merge, text_lis) = _TILE_CTX
    r, c = rc
    x0 = bbox.left + c * grid["tile_w"]
    y0 = bbox.bottom + r * grid["tile_h"]
    box = db.Box(x0, y0, min(x0 + grid["tile_w"], bbox.right),
                 min(y0 + grid["tile_h"], bbox.top))
    tm = {}
    t = time.perf_counter()
    # banded tiles build in viewer mode: clip_into of array-heavy
    # sources is ~100x faster (stress30: 20.9s -> 0.2s) and the clip
    # stays compact instead of materializing every member. The legacy
    # path keeps editable (its single write carries texts and needs
    # _strip_texts' erase). --tile-tgt editable restores the old
    # behavior for on-host comparison.
    tgt = db.Layout(not bands_dbu or tile_editable)
    tgt.dbu = ly.dbu
    # pre-create layers with source infos at identical indexes:
    # clip_into copies shapes onto anonymous layers otherwise, and
    # the OASIS writer silently drops layers without layer/datatype
    for li in ly.layer_indexes():
        tgt.insert_layer_at(li, ly.get_info(li))
    ci = ly.clip_into(top_ci, tgt, box)
    tm["clip"] = time.perf_counter() - t
    cell = tgt.cell(ci)
    if cell.bbox().empty():
        return r, c, False, None, None, tm
    cell.name = f"TILE_{r}_{c}"
    t = time.perf_counter()
    # texts live only in the skeleton (far-view labels): the source is
    # stripped once before tiling, this per-tile pass (restricted to
    # the known text layers) is a cheap safety net
    _strip_texts(tgt, text_lis)
    tm["strip"] = time.perf_counter() - t
    t = time.perf_counter()
    compact_instances(tgt)
    tm["compact"] = time.perf_counter() - t
    # layers with content in THIS tile (bbox_per_layer is a cached
    # subtree bbox, O(1) each): every per-cell x per-layer sweep below
    # walks these ~dozens instead of the full layer table (449 on the
    # host chip - the band/merge/lod probes dominated tile time)
    content_lis = [li for li in tgt.layer_indexes()
                   if not cell.bbox_per_layer(li).empty()]
    t = time.perf_counter()
    if bands_dbu:
        _counts, t_merge = _tile_bands(tgt, cdir, r, c, bands_dbu, opts,
                                       merge=merge, lis=content_lis)
        if t_merge:
            tm["merge"] = t_merge
    else:
        t_merge = 0.0
        tgt.write(os.path.join(cdir, "tiles", f"t_{r}_{c}.oas"), opts)
    tm["write"] = time.perf_counter() - t - t_merge
    t = time.perf_counter()
    lod_d = _tile_lod(tgt, ci, os.path.join(cdir, "tiles_lod",
                                            f"t_{r}_{c}.oas"),
                      always=bool(bands_dbu), lis=content_lis)
    tm["lod"] = time.perf_counter() - t
    t = time.perf_counter()
    dens = _tile_density(tgt, ci) or None
    tm["density"] = time.perf_counter() - t
    return r, c, True, lod_d, dens, tm


def build_index(src, tile_bytes=TILE_TARGET_BYTES, log=print, jobs=None,
                bands=BAND_THRESHOLDS_UM, read_mode=None, gov=True,
                mem_gb=None, mem_floor_gb=None, tile_tgt=None,
                text_cap=None, text_tile_cap=None, skel_texts=None,
                merge=True):
    """Scan the source file once and build the tile cache.

    jobs: max fork workers for the tiling phase (None = all cores;
    1 = sequential; platforms without fork fall back to sequential).
    bands: ascending size-band edges in um (see _tile_bands), or None
    for a legacy single-file-per-tile cache.
    read_mode: 'viewer' (default) keeps repetitions as compact shape
    arrays while reading the source - array-heavy production files
    otherwise materialize every member in editable mode (~46 B each:
    a 10 GB file was observed at 400 GB RSS). 'editable' restores the
    old behavior (flat sources read ~3x faster there).
    gov/mem_gb/mem_floor_gb: the tiling memory governor (see the
    tiling section below); mem_gb caps the whole run (--mem).
    tile_tgt: 'editable' rebuilds tile clips the pre-0.5.0 way.
    text_cap/text_tile_cap/skel_texts: skeleton label budgets
    (see collect_texts / build_skeleton)."""
    t_all = time.perf_counter()
    src = os.path.abspath(src)
    cdir = cache_dir_for(src)
    n_bands = len(bands) + 1 if bands else 0
    subs = ([f"tiles_b{k}" for k in range(n_bands)] if bands
            else ["tiles"]) + ["tiles_lod"]
    if bands and merge:
        subs += [f"tiles_m{k}" for k in range(1, n_bands)]
    stale = [d for d in ("tiles", "tiles_lod", "tiles_b0", "tiles_b1",
                         "tiles_b2", "tiles_b3", "tiles_b4", "tiles_b5",
                         "tiles_m1", "tiles_m2", "tiles_m3", "tiles_m4",
                         "tiles_m5")
             if os.path.isdir(os.path.join(cdir, d))]
    for sub in sorted(set(subs) | set(stale)):  # drop prior builds' files
        d = os.path.join(cdir, sub)
        os.makedirs(d, exist_ok=True)
        for f in os.listdir(d):
            os.remove(os.path.join(d, f))

    mode = (read_mode or "viewer").lower()
    editable = mode.startswith("edit")
    st = os.stat(src)
    log(f"[index] reading {src} ({st.st_size / 1e9:.2f} GB, "
        f"{'editable' if editable else 'viewer'} mode)...")
    t0 = time.perf_counter()
    with _phase_monitor("reading"):
        ly = db.Layout(editable)
        ly.read(src)
    t_read = time.perf_counter() - t0
    rss = _rss_gb(os.getpid())
    log(f"[index] read done in {t_read:.0f}s "
        f"({ly.cells()} cells, {len(ly.layer_indexes())} layers"
        + (f", {rss:.1f} GB RSS" if rss else "") + ")")

    top = pick_top_cell(ly, log)
    bbox = top.bbox()

    # grid: aim for ~tile_bytes per tile file
    n = int(round(math.sqrt(st.st_size / tile_bytes)))
    n = max(GRID_MIN, min(GRID_MAX, n))
    tile_w = -(-bbox.width() // n)   # ceil div
    tile_h = -(-bbox.height() // n)
    grid = {"nx": n, "ny": n, "x0": bbox.left, "y0": bbox.bottom,
            "tile_w": tile_w, "tile_h": tile_h}
    log(f"[index] grid {n}x{n}, tile {tile_w / 1000:.0f} x "
        f"{tile_h / 1000:.0f} um")

    t0 = time.perf_counter()
    counts, text_layer_lis = _scan_layers(ly, log)
    layers = []
    for i, li in enumerate(ly.layer_indexes()):
        info = ly.get_info(li)
        layers.append({"layer": info.layer, "datatype": info.datatype,
                       "name": info.name or f"{info.layer}/{info.datatype}",
                       "color": layer_color(i),
                       "stored_shapes": counts[li]})
    log(f"[index] layer scan done ({time.perf_counter() - t0:.1f}s, "
        f"{len(text_layer_lis)} text layers)")

    # --- texts: far-view skeleton labels only (tiles stay text-free) ---
    top_ci = top.cell_index()
    t0 = time.perf_counter()
    with _phase_monitor("collecting texts"):
        all_texts, thinned_lis = collect_texts(
            ly, top_ci, text_layer_lis, log, cap=text_cap, grid=grid,
            tile_cap=text_tile_cap,
            budget=SKEL_TEXT_CAP if skel_texts is None else skel_texts,
            jobs=jobs)
    log(f"[index] {len(all_texts)} texts collected for skeleton "
        f"labels ({time.perf_counter() - t0:.1f}s)")

    # strip texts from the loaded source ONCE, before tiling: every
    # tile clip used to copy its region's share of the source texts
    # (millions per dense tile on marker chips) just for the per-tile
    # strip to remove them again - 1,600 times over. The per-tile
    # strip stays as a cheap no-op safety net. Restricted to the
    # text-bearing layers found by the scan.
    if text_layer_lis:
        t0 = time.perf_counter()
        with _phase_monitor("stripping texts"):
            _strip_texts(ly, text_layer_lis)
        log(f"[index] source texts stripped "
            f"({time.perf_counter() - t0:.1f}s)")

    # --- tiles ---
    t0 = time.perf_counter()
    n_files = 0
    done = 0
    density_tiles = {}
    lod_tiles = {}
    step_tot = {}
    coords = [(r, c) for r in range(n) for c in range(n)]
    step = max(1, len(coords) // 10)

    def _breakdown():
        # cumulative per-step wall time; with fork workers these overlap,
        # so they sum above real elapsed - read as relative weights
        return " ".join("%s %.0fs" % kv for kv in sorted(step_tot.items()))

    def take(r, c, wrote, lod_d, dens, tm=None):
        nonlocal n_files, done
        done += 1
        if wrote:
            n_files += 1
        if lod_d is not None:
            lod_tiles[f"{r},{c}"] = lod_d
        if dens:
            density_tiles[f"{r},{c}"] = dens
        if tm:
            for k, v in tm.items():
                step_tot[k] = step_tot.get(k, 0.0) + v
        if done % step == 0 or done == len(coords):
            log(f"[index] tiles {done}/{len(coords)} "
                f"({time.perf_counter() - t0:.0f}s; {_breakdown()})")

    if jobs is None:
        jobs = os.cpu_count() or 1
    jobs = max(1, min(jobs, len(coords)))
    ctx = None
    if jobs > 1:
        try:
            ctx = multiprocessing.get_context("fork")
        except ValueError:
            log("[index][warn] no fork on this platform - tiling "
                "sequentially")
    bands_dbu = [int(round(um / ly.dbu)) for um in bands] if bands else None
    global _TILE_CTX
    _TILE_CTX = (ly, top_ci, bbox, grid, cdir, save_opts(),
                 bands_dbu, (tile_tgt or "").lower().startswith("edit"),
                 bool(merge), text_layer_lis)
    total_gb = _total_ram_gb()
    try:
        if ctx is not None and gov and total_gb:
            # Memory governor: worker count follows measured demand, not
            # just core count (a 16 GB laptop OOMed at cpu_count workers
            # x 4-8 GB/tile on adversarial content). Until the first
            # tile completes, per-worker demand is assumed to be
            # MEM_PRIOR_GB and the budget runs as many workers as that
            # buys - the probe used to run SOLO, which left a 96-core
            # host idle for the ~20 min its densest (central, queued
            # first) tile took. After the first completion the live
            # estimate takes over and keeps learning from worker RSS
            # in case denser tiles exceed it.
            floor_gb = mem_floor_gb or max(2.0, 0.05 * total_gb)
            parent_gb = _rss_gb(os.getpid()) or 0.0
            est_gb = 0.5      # per-worker private demand, learned live
            pending = list(coords)
            probe_rc = (n // 2, n // 2)   # central tile: usually densest
            pending.remove(probe_rc)
            pending.insert(0, probe_rc)
            log(f"[index] tiling with up to {jobs} fork workers "
                f"(memory-governed; {total_gb:.0f} GB RAM"
                + (f", --mem cap {mem_gb:g} GB" if mem_gb else "")
                + f"; assuming {MEM_PRIOR_GB:g} GB/worker until the "
                f"first tile lands)...")
            if mem_gb and mem_gb <= parent_gb + 1.0:
                log(f"[index][warn] --mem {mem_gb:g} GB leaves <1 GB "
                    f"over the loaded source ({parent_gb:.1f} GB RSS); "
                    f"tiling will run 1 worker at a time")
            inflight = {}     # AsyncResult -> (r, c)
            probing = True
            allowed = last_allowed = 1
            avail = None
            t_beat = time.perf_counter()
            t_note = 0.0
            # maxtasksperchild=1: a tile's memory really returns to the
            # OS when it finishes (long-lived workers sit at their worst
            # tile's RSS forever), and each respawn re-shares the parent
            # image copy-on-write
            with ctx.Pool(jobs, maxtasksperchild=1) as pool:
                while pending or inflight:
                    # private demand ~= child RSS minus the COW-shared
                    # parent image (idle fresh workers read as ~0)
                    kids = _rss_many_gb(
                        [p.pid for p in multiprocessing.active_children()])
                    priv = sum(max(0.0, r - parent_gb)
                               for r in kids.values())
                    for r in kids.values():
                        est_gb = max(est_gb, r - parent_gb)
                    avail = _avail_ram_gb()
                    if avail is None:
                        allowed = 1 if probing else jobs
                    else:
                        # what the pie (still free + already claimed by
                        # workers) buys at est GB per worker, with slack
                        # for tiles denser than anything seen yet. While
                        # no tile has completed, the conservative prior
                        # bounds the fleet instead of a solo probe (the
                        # live samples still tighten it if they exceed
                        # the prior; a burst can still overshoot if real
                        # demand tops the prior after dispatch - the
                        # 1.25 slack and the floor absorb that)
                        budget = avail + priv - floor_gb
                        if mem_gb:
                            # --mem: hard ceiling for the whole run =
                            # loaded source + all worker private memory
                            budget = min(budget, mem_gb - parent_gb)
                        est_eff = (max(est_gb, MEM_PRIOR_GB) if probing
                                   else est_gb)
                        allowed = max(1, min(jobs,
                                             int(budget / (est_eff * 1.25))))
                    if (not probing and allowed != last_allowed
                            and allowed < jobs and pending
                            and time.perf_counter() - t_note >= 10.0):
                        t_note = time.perf_counter()
                        log(f"[index] governor: {allowed} workers "
                            f"(~{est_gb:.1f} GB/worker, "
                            f"{avail:.1f} GB free)")
                    last_allowed = allowed
                    while pending and len(inflight) < allowed:
                        rc = pending.pop(0)
                        inflight[pool.apply_async(
                            _build_one_tile, (rc,))] = rc
                    done_now = [ar for ar in inflight if ar.ready()]
                    for ar in done_now:
                        res = ar.get()
                        del inflight[ar]
                        take(*res)
                        probing = False
                    now = time.perf_counter()
                    if done_now:
                        t_beat = now
                    elif now - t_beat >= INDEX_HEARTBEAT_S:
                        t_beat = now
                        bd = _breakdown()
                        log(f"[index] tiles {done}/{len(coords)} done, "
                            f"{len(inflight)} workers busy "
                            f"(~{est_gb:.1f} GB/worker"
                            + (f", {avail:.1f} GB free"
                               if avail is not None else "")
                            + f"; {now - t0:.0f}s"
                            + (f"; {bd}" if bd else "") + ")")
                    if not done_now:
                        time.sleep(0.5)
        elif ctx is not None:
            log(f"[index] tiling with {jobs} fork workers...")
            with ctx.Pool(jobs) as pool:
                it = pool.imap_unordered(_build_one_tile, coords)
                left = len(coords)
                while left:
                    try:
                        res = it.next(timeout=INDEX_HEARTBEAT_S)
                    except multiprocessing.TimeoutError:
                        # heavy tiles can run many minutes: keep proving
                        # liveness instead of going silent between tiles
                        bd = _breakdown()
                        log(f"[index] tiles {done}/{len(coords)} done, "
                            f"{min(jobs, left)} workers busy "
                            f"({time.perf_counter() - t0:.0f}s"
                            + (f"; {bd}" if bd else "") + ")")
                        continue
                    except StopIteration:
                        break
                    take(*res)
                    left -= 1
        else:
            for rc in coords:
                take(*_build_one_tile(rc))
    finally:
        _TILE_CTX = None
    t_tiles = time.perf_counter() - t0
    log(f"[index] {n_files} tile files in {t_tiles:.0f}s ({_breakdown()})")

    # --- skeleton (far-zoom structural model) ---
    t0 = time.perf_counter()
    skel_meta = build_skeleton(ly, top, all_texts,
                               os.path.join(cdir, "skeleton.oas"), log,
                               skel_texts=skel_texts)
    log(f"[index] skeleton: {skel_meta['shapes']} shapes "
        f"({time.perf_counter() - t0:.1f}s)")

    meta = {
        "version": CACHE_VERSION,
        "src": {"path": src, "size": st.st_size, "mtime": int(st.st_mtime)},
        "dbu": ly.dbu,
        "top_cell": top.name,
        "bbox": [bbox.left, bbox.bottom, bbox.right, bbox.top],
        "grid": grid,
        "layers": layers,
        "density": {"levels": DENSITY_LEVELS, "tiles": density_tiles},
        "lod": {"cap": LOD_SHAPE_CAP, "tiles": lod_tiles},
        **({"bands": {"thresholds_um": list(bands),
                      **({"merge": {"close_frac": MERGE_CLOSE_FRAC}}
                         if merge else {})}} if bands else {}),
        **({"texts_thinned": [
            {"layer": ly.get_info(li).layer,
             "datatype": ly.get_info(li).datatype}
            for li in thinned_lis]} if thinned_lis else {}),
        "skeleton": skel_meta,
        "stats": {"read_s": round(t_read, 1), "tiles_s": round(t_tiles, 1),
                  "read_mode": "editable" if editable else "viewer",
                  "total_s": 0.0,
                  "cells": ly.cells(), "tile_files": n_files},
    }

    meta["stats"]["total_s"] = round(time.perf_counter() - t_all, 1)
    with open(os.path.join(cdir, "meta.json"), "w") as f:
        json.dump(meta, f, indent=1)
    log(f"[index] done in {time.perf_counter() - t_all:.0f}s -> {cdir}")
    return meta
