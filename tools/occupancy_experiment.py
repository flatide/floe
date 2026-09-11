#!/usr/bin/env python3
"""Occupancy-summary experiment for one source and one layer (review
2026-09-11): does a multi-resolution occupancy mask - a bit per grid
cell saying "some geometry touches this cell" - replace the exact
decode + raster at wide views while keeping the POSITION of the empty
space, and what does it cost to build?

Field 2026-09-11: keeping all-thin pages at the deck's fit view (1 px
= 202 um) decoded 25k pages and painted 41.7M hairlines in 32 s - the
mask policy is unusable at wide views without a summary. The summary
must replace the decode, and bbox fills / one density number per bbox
are excluded (they lose where the empty space is).

    floe2-style usage (from the repo root, the venv's python):

    .venv/bin/python tools/occupancy_experiment.py SRC.oas --layer L/D \\
        [--bbox=x0,y0,x1,y1] [--fine-um 4] [--tile-px 2048] [--fit-px 800] \\
        --out DIR

(write --bbox=... with the equals sign: a region whose first coordinate
is negative would otherwise be read as an option, here and in `floe2
render --bbox=`)

Steps and what each number means:
  1. fine exact render: the layer at `fine-um` per pixel, tiled through
     `floe2 render --batch` (cut 0: everything, thin pages included).
     Its wall time is the proxy for the summary's index-time build cost
     (one exact pass over the layer's pages).
  2. level 0 mask = "pixel lit" at fine-um; levels k = OR-pooling by 2
     (a cell is occupied when any finer cell is). Per level: cell size,
     grid, occupied cells, packed bytes - the storage of the summary.
  3. fit renders for the comparison: the exact mask policy (`--detail
     high --thin keep`, the slow path) and the plain policy (`--thin
     cull`), both timed.
  4. fidelity at the fit scale: the exact keep render is the reference
     (a one-pixel-cell summary reproduces it up to raster parity); the
     reference OR-pooled to 2 / 4 / 8 px cells shows what a coarser
     summary loses - extra pixels, Jaccard, and the reference's empty
     regions (4-connected, by size bucket) each cell size keeps empty,
     fills partly, or fills entirely. `cells_to_paint` is the wide
     view's paint cost with that summary.

Outputs in DIR: report.json, summary-fit.png (the reference), diff-fit.png
(the 2 px cell against it: blue = an empty pixel the coarser cell
fills), and the fit renders. Everything else is scratch.
"""
import argparse
import json
import math
import os
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)


def _floe2(*argv, env=None):
    cmd = [sys.executable, "-B", "-m", "floe2"] + [str(a) for a in argv]
    t0 = time.perf_counter()
    res = subprocess.run(cmd, cwd=ROOT, env=dict(os.environ, **(env or {})),
                         capture_output=True, text=True)
    dt = time.perf_counter() - t0
    if res.returncode not in (0, 3):
        raise SystemExit("floe2 %s failed (rc %d):\n%s" % (
            argv[0], res.returncode, res.stderr[-2000:]))
    return dt, res


def _lit(png):
    from PIL import Image
    import numpy as np
    img = Image.open(png).convert("RGB")
    a = np.asarray(img)
    return (a[:, :, 0] | a[:, :, 1] | a[:, :, 2]) > 0


def _or_pool(mask, factor):
    """OR-pool a boolean [H, W] mask by an integer factor (padding the
    tail with empty cells)."""
    import numpy as np
    h, w = mask.shape
    ph, pw = (-h) % factor, (-w) % factor
    if ph or pw:
        mask = np.pad(mask, ((0, ph), (0, pw)))
    h, w = mask.shape
    return mask.reshape(h // factor, factor, w // factor, factor).any(
        axis=(1, 3))


def _components(empty):
    """Sizes of the 4-connected components of `empty` ([H, W] bool) and
    a label map (0 = not empty)."""
    import numpy as np
    h, w = empty.shape
    labels = np.zeros((h, w), dtype=np.int32)
    sizes = []
    next_label = 0
    ys, xs = np.nonzero(empty)
    for sy, sx in zip(ys.tolist(), xs.tolist()):
        if labels[sy, sx]:
            continue
        next_label += 1
        stack = [(sy, sx)]
        labels[sy, sx] = next_label
        n = 0
        while stack:
            y, x = stack.pop()
            n += 1
            for ny, nx in ((y - 1, x), (y + 1, x), (y, x - 1), (y, x + 1)):
                if 0 <= ny < h and 0 <= nx < w and empty[ny, nx] \
                        and not labels[ny, nx]:
                    labels[ny, nx] = next_label
                    stack.append((ny, nx))
        sizes.append(n)
    return labels, sizes


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("src")
    ap.add_argument("--layer", required=True, help="L/D")
    ap.add_argument("--bbox", default=None,
                    help="x0,y0,x1,y1 um (default: the source's bbox); "
                         "write --bbox=... when x0 is negative")
    ap.add_argument("--fine-um", type=float, default=4.0,
                    help="level-0 cell (um per pixel of the fine render)")
    ap.add_argument("--tile-px", type=int, default=2048)
    ap.add_argument("--fit-px", type=int, default=800,
                    help="width of the fit-view comparison renders")
    ap.add_argument("--out", required=True)
    args = ap.parse_args()
    import numpy as np
    from PIL import Image
    from floe.cache import Cache

    os.makedirs(args.out, exist_ok=True)
    c = Cache(args.src)
    if not c.exists():
        raise SystemExit("no .floe cache for %s; run: floe2 index %s"
                         % (args.src, args.src))
    c.load()
    dbu = float(c.meta["dbu"])
    if args.bbox:
        x0, y0, x1, y1 = (float(v) for v in args.bbox.split(","))
    else:
        b = c.meta["bbox"]
        x0, y0, x1, y1 = (b[0] * dbu, b[1] * dbu, b[2] * dbu, b[3] * dbu)
    span_x, span_y = x1 - x0, y1 - y0
    # the fine cell is an exact power-of-two subdivision of the fit
    # pixel, so every level lands on the fit grid and the comparison
    # measures the summary, not a half-pixel registration
    fit_um = span_x / args.fit_px
    k = max(0, int(round(math.log2(max(1e-9, fit_um / args.fine_um)))))
    fine = fit_um / (1 << k)
    report = {"source": os.path.abspath(args.src), "layer": args.layer,
              "bbox_um": [x0, y0, x1, y1], "fine_um": fine,
              "fine_um_requested": args.fine_um, "fit_levels_below": k}

    # ---- 1. fine exact render, tiled
    tile_um = args.tile_px * fine
    nx = max(1, int(-(-span_x // tile_um)))
    ny = max(1, int(-(-span_y // tile_um)))
    lines = []
    for r in range(ny):
        for cc in range(nx):
            tx0 = x0 + cc * tile_um
            ty0 = y0 + r * tile_um
            # a square region at px=W: the height follows the aspect,
            # so the tile is W x W without any region expansion
            lines.append("t_%d_%d bbox=%r,%r,%r,%r px=%d" % (
                r, cc, tx0, ty0, tx0 + tile_um, ty0 + tile_um,
                args.tile_px))
    batch = os.path.join(args.out, "fine.batch")
    with open(batch, "w") as fh:
        fh.write("\n".join(lines) + "\n")
    tiles = os.path.join(args.out, "fine")
    t_fine, res = _floe2("render", args.src, "--batch", batch, "--out",
                         tiles, "--layers", args.layer, "--detail", "exact",
                         "--report", os.path.join(args.out, "fine.json"))
    report["fine_render"] = {"tiles": nx * ny, "tile_px": args.tile_px,
                             "seconds": round(t_fine, 2),
                             "exit": res.returncode}
    W = nx * args.tile_px
    H = ny * args.tile_px
    mask = np.zeros((H, W), dtype=bool)
    for r in range(ny):
        for cc in range(nx):
            t = _lit(os.path.join(tiles, "t_%d_%d.png" % (r, cc)))
            # image rows run top-down: tile row r covers y from the
            # bottom, so it lands at the bottom of the stack
            ry = (ny - 1 - r) * args.tile_px
            mask[ry:ry + t.shape[0], cc * args.tile_px:cc * args.tile_px
                 + t.shape[1]] = t
    # crop to the bbox (the tile grid may overhang)
    W_used = int(round(span_x / fine))
    H_used = int(round(span_y / fine))
    mask = mask[H - H_used:, :W_used] if H_used <= H else mask
    report["level0"] = {"grid": [int(mask.shape[1]), int(mask.shape[0])],
                        "occupied": int(mask.sum()),
                        "fraction": round(float(mask.mean()), 4)}

    # ---- 2. the pyramid
    levels = []
    level_masks = [mask]
    cell = fine
    while True:
        m = level_masks[-1]
        t0 = time.perf_counter()
        levels.append({"level": len(levels), "cell_um": cell,
                       "grid": [int(m.shape[1]), int(m.shape[0])],
                       "occupied": int(m.sum()),
                       "fraction": round(float(m.mean()), 4),
                       "packed_bytes": int(-(-m.shape[0] * m.shape[1] // 8)),
                       "pool_seconds": round(time.perf_counter() - t0, 3)})
        if cell * 2 > fit_um * 4 or min(m.shape) <= 2:
            break
        level_masks.append(_or_pool(m, 2))
        cell *= 2
    report["levels"] = levels

    # ---- 3. fit renders (the two policies), timed
    fit_bbox = "%r,%r,%r,%r" % (x0, y0, x1, y1)
    fit = {}
    for name, thin in (("keep", "keep"), ("cull", "cull")):
        png = os.path.join(args.out, "fit-%s.png" % name)
        # px=W: the height follows the aspect, the region is not
        # expanded, so the image grid is the summary's fit level
        # --bbox=... : a region starting with a negative coordinate
        # would otherwise be taken for an option (argparse)
        dt, res = _floe2("render", args.src, "--bbox=" + fit_bbox, "--px",
                         str(args.fit_px), "--out", png,
                         "--layers", args.layer, "--detail", "high",
                         "--thin", thin)
        fit[name] = {"seconds": round(dt, 2), "png": png,
                     "lit": int(_lit(png).sum())}
    report["fit_renders"] = fit

    # ---- 4. fidelity. The reference is the exact keep render at the fit
    # scale (what the viewer shows today through the slow path): a
    # summary whose cell is one fit pixel reproduces it up to raster
    # parity, so the question is what COARSER cells lose - every
    # coarser level is the reference OR-pooled by 2 / 4 / 8 px and
    # compared back: extra pixels, and the reference's empty regions
    # (4-connected, by size) the coarser cell fills partly or wholly.
    # That is the display-error criterion the review asked for. (The
    # fine pyramid's fit level is also compared, as information only:
    # it differs from the fit render by the hairline parity's y bias,
    # which scales with the pixel, not by summary error.)
    fit_level = min(k, len(level_masks) - 1)
    exact = _lit(fit["keep"]["png"])
    fh, fw = exact.shape

    def compare(ref, mask):
        missed = ref & ~mask
        extra = mask & ~ref
        union = (ref | mask).sum()
        row = {"lit": int(mask.sum()), "missed": int(missed.sum()),
               "extra": int(extra.sum()),
               "jaccard": round(float((ref & mask).sum() / union), 4)
               if union else 1.0}
        labels, sizes = _components(~ref)
        buckets = {"1-3": [0, 0, 0], "4-15": [0, 0, 0],
                   "16-255": [0, 0, 0], "256+": [0, 0, 0]}
        filled_by_label = np.bincount(labels[extra],
                                      minlength=len(sizes) + 1)
        for label, n in enumerate(sizes, 1):
            f = int(filled_by_label[label]) \
                if label < len(filled_by_label) else 0
            key = "1-3" if n <= 3 else "4-15" if n <= 15 else \
                "16-255" if n <= 255 else "256+"
            buckets[key][0 if f == 0 else 2 if f >= n else 1] += 1
        row["empty_regions"] = {
            k_: {"kept": v[0], "partly_filled": v[1], "filled": v[2]}
            for k_, v in buckets.items()}
        return row, missed, extra

    comparison = {"reference": {"what": "exact keep render at the fit scale",
                                "grid": [fw, fh], "lit": int(exact.sum())},
                  "fit_um_per_px": round(fit_um, 3)}
    coarser = []
    for factor in (2, 4, 8):
        pooled = _or_pool(exact, factor)
        up = np.repeat(np.repeat(pooled, factor, axis=0), factor, axis=1)
        up = up[:fh, :fw]
        row, missed, extra = compare(exact, up)
        row.update({"cell_px": factor,
                    "cell_um": round(fit_um * factor, 3),
                    "cells_to_paint": int(pooled.sum())})
        coarser.append(row)
        if factor == 2:
            diff = np.zeros((fh, fw, 3), dtype=np.uint8)
            diff[exact & up] = (255, 255, 255)
            diff[missed] = (255, 0, 0)
            diff[extra] = (0, 0, 255)
            Image.fromarray(diff).save(os.path.join(args.out, "diff-fit.png"))
    comparison["coarser_cells"] = coarser
    # information: the fine pyramid's fit level against the fit render
    ref = level_masks[fit_level]
    rh, rw = ref.shape
    hh, ww = min(fh, rh), min(fw, rw)
    row, _, _ = compare(exact[:hh, :ww], ref[:hh, :ww])
    row.pop("empty_regions", None)
    row.update({"level": fit_level, "cell_um": levels[fit_level]["cell_um"],
                "note": "differs by the hairline parity y bias, not summary error"})
    comparison["fine_pyramid_at_fit"] = row
    Image.fromarray((exact * 255).astype(np.uint8)).save(
        os.path.join(args.out, "summary-fit.png"))
    report["comparison"] = comparison
    with open(os.path.join(args.out, "report.json"), "w") as fh_:
        json.dump(report, fh_, indent=1)
    print(json.dumps(report, indent=1))


if __name__ == "__main__":
    main()
