#!/usr/bin/env python3
"""Standalone area/presence mock, NOT the renderer or a production sampler.

The fixture rectangles are disjoint, so their summed area is the union area.
All rectangles are treated as already below the cut. World-space candidates
come directly from their interiors; no summary file or decoded cache is used.
"""
import argparse
from dataclasses import dataclass
import hashlib
import math
from pathlib import Path
import random

from PIL import Image, ImageDraw, ImageFont


RECTS = [(i * 0.8, j * 1.0, 0.45, 0.022)
         for j in range(34) for i in range(25)]
UNION_AREA = sum(w * h for _, _, w, h in RECTS)
WORLD_W = max(x + w for x, _, w, _ in RECTS)
WORLD_H = max(y + h for _, y, _, h in RECTS)
RHO = UNION_AREA / (WORLD_W * WORLD_H)
BASE_SCALE = math.sqrt(256.0 / UNION_AREA)
ZOOMS = [1.0, 0.5, 0.25, 0.125, 0.0625, 0.03125, 0.015625]
TILE = 220
INK = (110, 205, 250)
BG = (10, 16, 24)


def project(x, y, scale):
    return (math.floor(TILE / 2 + (x - WORLD_W / 2) * scale),
            math.floor(TILE / 2 + (y - WORLD_H / 2) * scale))


def candidates():
    # Equal-area, disjoint rectangles: uniform selection samples union area.
    # The same stream is used at every zoom; its first point is the anchor.
    rng = random.Random(20260921)
    while True:
        x, y, w, h = RECTS[rng.randrange(len(RECTS))]
        yield x + rng.random() * w, y + rng.random() * h


def world_threshold(level, gx, gy, seed=0):
    # Stable across processes (unlike Python's hash). No viewport, tile,
    # frame, instance, page or worker identity. One fixture layer: 109/2.
    key = f"cut-density-v1/{seed}/109/2/{level}/{gx}/{gy}".encode("ascii")
    bits = int.from_bytes(hashlib.blake2b(key, digest_size=8).digest(), "big")
    return (bits >> 11) / (1 << 53)


def neighborhood_areas(scale, pitch_px):
    # World-anchored dyadic cells project to [pitch_px, 2*pitch_px).
    # Halving the scale merges 2x2 cells. Accumulate ALL contributions before
    # rounding; an instance/page/shape never receives its own presence floor.
    level = math.ceil(math.log2(pitch_px / scale))
    step = 2.0 ** level
    areas = {}
    for x, y, w, h in RECTS:
        for gy in range(math.floor(y / step), math.ceil((y + h) / step)):
            for gx in range(math.floor(x / step), math.ceil((x + w) / step)):
                overlap_w = max(0, min(x + w, (gx + 1) * step) - max(x, gx * step))
                overlap_h = max(0, min(y + h, (gy + 1) * step) - max(y, gy * step))
                areas[gx, gy] = areas.get((gx, gy), 0) + overlap_w * overlap_h
    assert math.isclose(sum(areas.values()), UNION_AREA, rel_tol=1e-10)
    return level, step, areas


def neighborhood_targets(scale, threshold, pitch_px, quantization="dither",
                         presence=True, seed=0):
    level, step, areas = neighborhood_areas(scale, pitch_px)
    targets = {}
    area_target = presence_added = 0
    for (gx, gy), area in areas.items():
        # Clip support to the fixed fixture domain: padding outside the whole
        # design must not dilute density indefinitely when it fits in one cell.
        support_w = min(WORLD_W, (gx + 1) * step) - max(0, gx * step)
        support_h = min(WORLD_H, (gy + 1) * step) - max(0, gy * step)
        density = area / (support_w * support_h)
        mass = area * scale * scale
        if quantization == "round":
            count = math.floor(mass + 0.5)
        elif quantization == "dither":
            whole = math.floor(mass)
            count = whole + int(world_threshold(level, gx, gy, seed) < mass - whole)
        else:
            raise ValueError(f"unknown quantization: {quantization}")
        target = max(count, int(presence and density >= threshold))
        targets[gx, gy] = target
        area_target += count
        presence_added += target - count
    return step, targets, area_target, presence_added


@dataclass
class DensityFrame:
    pixels: set
    cell_px: float
    cells: int
    area_target: int
    presence_added: int
    collisions: int


def neighborhoods(scale, threshold, pitch_px, quantization="dither",
                  presence=True, seed=0):
    step, targets, area_target, presence_added = neighborhood_targets(
        scale, threshold, pitch_px, quantization, presence, seed)
    selected = {key: set() for key in targets}
    remaining = sum(targets.values())
    for index, (x, y) in enumerate(candidates()):
        if remaining == 0:
            pixels = set().union(*selected.values())
            return DensityFrame(pixels, step * scale, len(targets), area_target,
                                presence_added, sum(targets.values()) - len(pixels))
        key = math.floor(x / step), math.floor(y / step)
        bucket = selected[key]
        if len(bucket) < targets[key]:
            pixel = project(x, y, scale)
            if pixel not in bucket:
                bucket.add(pixel)
                remaining -= 1
        if index > 1_000_000:
            raise RuntimeError("fixture cannot fill the neighborhood quotas")
    raise AssertionError("unreachable")


def font(size):
    for name in ("/System/Library/Fonts/Supplemental/Arial.ttf", "DejaVuSans.ttf"):
        try:
            return ImageFont.truetype(name, size)
        except OSError:
            pass
    return ImageFont.load_default(size=size)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, default=Path("/tmp/floe-cut-density.png"))
    parser.add_argument("--presence-threshold", type=float, default=1 / 256,
                        help="Experimental density threshold per spatial neighborhood")
    parser.add_argument("--pitch-px", type=float, default=8,
                        help="Minimum projected side of a merging neighborhood")
    parser.add_argument("--quantization", choices=("dither", "round"), default="dither",
                        help="World-keyed fixed dither (default), or the old rounding control")
    parser.add_argument("--no-presence", action="store_true",
                        help="Disable the one-dot presence correction independently")
    parser.add_argument("--seed", type=int, default=0,
                        help="Fixed dither seed, for comparing deterministic realizations")
    parser.add_argument("--compare", action="store_true",
                        help="Also print rounding/dither x presence on/off counts")
    args = parser.parse_args()
    if not 0 <= args.presence_threshold <= 1:
        parser.error("presence threshold must be within 0..1")
    if not math.isfinite(args.pitch_px) or args.pitch_px < 2:
        parser.error("pitch must be finite and at least 2 px")
    pad, gap, top = 20, 10, 80
    stride = TILE + gap
    row_h = TILE + 60
    image = Image.new("RGB", (pad * 2 + len(ZOOMS) * stride - gap,
                              top + row_h * 2 + 75), BG)
    draw = ImageDraw.Draw(image)
    draw.text((pad, 12), "Sub-cut density: merge nearby contributions as the view shrinks",
              font=font(23), fill="white")
    draw.text((pad, 44), "Synthetic 25 x 34 disjoint rectangles; all geometry is below cut",
              font=font(16), fill=(170, 185, 200))
    previous_count = None
    anchor = next(candidates())
    print("zoom_pct  area_px2  reference  area_quota  presence_added  collisions  dots  cells  cell_px")
    for column, zoom in enumerate(ZOOMS):
        scale = BASE_SCALE * zoom
        mass = UNION_AREA * scale * scale
        result = neighborhoods(scale, args.presence_threshold, args.pitch_px,
                               args.quantization, not args.no_presence, args.seed)
        density = result.pixels
        # Count and persistence invariants of THIS fixed, fully visible fixture.
        if (args.presence_threshold == 1 / 256 and args.pitch_px == 8
                and not args.no_presence and args.seed == 0):
            assert project(*anchor, scale) in density
            if previous_count is not None:
                assert len(density) <= previous_count
        previous_count = len(density)
        reference = set()
        for x, y, w, h in RECTS:
            x0, y0 = project(x, y, scale)
            x1, y1 = project(x + w, y + h, scale)
            for py in range(y0, max(y0 + 1, y1)):
                for px in range(x0, max(x0 + 1, x1)):
                    reference.add((px, py))
        for row, (label, pixels) in enumerate((
                ("Each shape: minimum 1 px", reference),
                (f"{args.quantization} + {'area only' if args.no_presence else 'presence'}", density))):
            left, ytop = pad + column * stride, top + row * row_h
            draw.text((left, ytop), f"{zoom * 100:g}% | {len(pixels)} pixels",
                      font=font(16), fill="white")
            draw.text((left, ytop + 22), label, font=font(13), fill=(160, 175, 190))
            canvas_top = ytop + 45
            draw.rectangle((left, canvas_top, left + TILE - 1, canvas_top + TILE - 1),
                           outline=(43, 55, 69))
            for px, py in pixels:
                if 0 <= px < TILE and 0 <= py < TILE:
                    image.putpixel((left + px, canvas_top + py), INK)
        print(f"{zoom * 100:8.4f}  {mass:9.4f}  {len(reference):9d}  "
              f"{result.area_target:10d}  {result.presence_added:14d}  "
              f"{result.collisions:10d}  {len(density):4d}  {result.cells:5d}  {result.cell_px:7.2f}")
    y = top + row_h * 2 + 8
    draw.text((pad, y), f"Fixture density: {RHO:.3%}; experimental presence threshold: "
              f"{args.presence_threshold:.3%}. Merge-cell side: {args.pitch_px:g}-"
              f"{args.pitch_px * 2:g} px; no per-shape floor.",
              font=font(16), fill=(180, 195, 210))
    draw.text((pad, y + 25), f"Quantization: {args.quantization}; seed: {args.seed}; "
              f"presence: {'off' if args.no_presence else 'on'}. "
              "No index-cost, pan-stability or production-performance claim.",
              font=font(14), fill=(155, 170, 185))
    args.out.parent.mkdir(parents=True, exist_ok=True)
    image.save(args.out)
    print(f"wrote {args.out}")
    if args.compare:
        print("\ncomparison: mode | target dots before pixel collisions | unique pixels")
        print("zoom_pct: " + " / ".join(f"{z * 100:g}" for z in ZOOMS))
        for quantization, presence in (("round", False), ("round", True),
                                       ("dither", False), ("dither", True)):
            results = [neighborhoods(BASE_SCALE * z, args.presence_threshold,
                                     args.pitch_px, quantization, presence, args.seed)
                       for z in ZOOMS]
            targets = " / ".join(str(r.area_target + r.presence_added) for r in results)
            pixels = " / ".join(str(len(r.pixels)) for r in results)
            print(f"{quantization} presence={presence}: {targets} | {pixels}")


if __name__ == "__main__":
    main()
