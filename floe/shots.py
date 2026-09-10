"""Headless shots for `floe2 render` (jobdeck M4, generic for any source).

One open, many captures: a batch file names the shots, each with its own
viewport, pixel size, layers, depth; a mosaic shot composes four captures
(tl, tr / bl, br) into one image with separator lines drawn OVER the
tile edges. Lengths take unit suffixes (nm, um/µm/μm, mm, cm, m; bare =
um). The rules follow docs/JOBDECK.ko.md §7 (mirrored from the
reference tool's cli-spec, decisions kept):

  region       --bbox X0,Y0,X1,Y1 | --at X,Y --size W,H [--anchor center|lb]
               | (neither) the whole source
  pixel        --px W  -> height from the region aspect (the floe rule)
               --px WxH -> the region is EXPANDED to that aspect about
               its anchor (lb keeps the pinned corner), or used exactly
               with --stretch
  mosaic       --mosaic-at "X,Y;X,Y;X,Y;X,Y" --size W,H   four points,
               clockwise from top-left (tl, tr, br, bl), each read like
               --at; composed tl,tr / bl,br
               --corners X1,Y1,X2,Y2 --size W,H            the region's four
               W,H corner rectangles, INSIDE the region (the tl tile's
               top-left corner is the region's top-left corner)
               --line W (px, centred on the seam: floor(W/2) solid each
               side, the remainder one blended pixel each side; 0 = none)
               --line-color COLOR, --keep-tiles (<out>_tl/_tr/_bl/_br.png)

Nothing here needs Pillow: single shots write renderd's PNG as is; a
mosaic composes raw RGBA tiles and encodes the PNG with zlib.
"""

from __future__ import annotations

import json
import os
import queue
import struct
import tempfile
import time
import zlib

ANCHORS = ("center", "lb")
MOSAIC_ORDER = ("tl", "tr", "bl", "br")

_UNITS = {"nm": 1e-3, "um": 1.0, "µm": 1.0, "μm": 1.0,
          "mm": 1e3, "cm": 1e4, "m": 1e6}


def parse_length(text) -> float:
    """'8000' | '8000um' | '8mm' | '500nm' -> micrometres."""
    s = str(text).strip()
    for suffix, factor in sorted(_UNITS.items(), key=lambda kv: -len(kv[0])):
        if s.endswith(suffix):
            return float(s[:-len(suffix)].strip()) * factor
    return float(s)


def parse_lengths(text, count, what) -> tuple:
    parts = [p for p in str(text).replace(";", ",").split(",") if p.strip()]
    if len(parts) != count:
        raise ValueError("%s needs %d values, got %r" % (what, count, text))
    try:
        return tuple(parse_length(p) for p in parts)
    except ValueError as exc:
        raise ValueError("%s: %s" % (what, exc))


def parse_points(text, what="--mosaic-at") -> list:
    """'X,Y;X,Y;X,Y;X,Y' -> four (x, y) um, clockwise from top-left."""
    groups = [g for g in str(text).split(";") if g.strip()]
    if len(groups) != 4:
        raise ValueError("%s needs four X,Y points separated by ';'"
                         % what)
    return [parse_lengths(g, 2, what) for g in groups]


def parse_pixel(text) -> tuple:
    """'1200' -> (1200, None); '1200x900' -> (1200, 900)."""
    s = str(text).lower().strip()
    if "x" in s:
        w, h = s.split("x", 1)
        w, h = int(w), int(h)
        if w <= 0 or h <= 0:
            raise ValueError("--px must be positive")
        return w, h
    w = int(s)
    if w <= 0:
        raise ValueError("--px must be positive")
    return w, None


def parse_color(text) -> tuple:
    s = str(text).strip().lstrip("#")
    if len(s) != 6:
        raise ValueError("color must be #rrggbb: %r" % text)
    return tuple(int(s[i:i + 2], 16) for i in (0, 2, 4))


def region_from(at=None, size=None, anchor="center"):
    """`at` + `size` under `anchor`: center = at is the middle; lb = at
    is the lower-left corner, the region extends right and up."""
    x, y = at
    w, h = size
    if w <= 0 or h <= 0:
        raise ValueError("--size must be positive")
    if anchor == "lb":
        return (x, y, x + w, y + h)
    return (x - w / 2.0, y - h / 2.0, x + w / 2.0, y + h / 2.0)


def normalize_box(box):
    x0, y0, x1, y1 = (float(v) for v in box)
    if x0 == x1 or y0 == y1:
        raise ValueError("region has zero width or height")
    return (min(x0, x1), min(y0, y1), max(x0, x1), max(y0, y1))


def fit_aspect(box, width, height, anchor="center", stretch=False):
    """Expand `box` to the width:height aspect, holding the anchor: the
    centre stays put, or with lb the lower-left corner stays and the
    extra area goes right and up. `stretch` returns the box as is."""
    x0, y0, x1, y1 = normalize_box(box)
    if stretch:
        return (x0, y0, x1, y1)
    want = float(width) / float(height)
    have = (x1 - x0) / (y1 - y0)
    if abs(want - have) < 1e-12:
        return (x0, y0, x1, y1)
    if have < want:
        w, h = (y1 - y0) * want, (y1 - y0)
    else:
        w, h = (x1 - x0), (x1 - x0) / want
    if anchor == "lb":
        return (x0, y0, x0 + w, y0 + h)
    cx, cy = (x0 + x1) / 2.0, (y0 + y1) / 2.0
    return (cx - w / 2.0, cy - h / 2.0, cx + w / 2.0, cy + h / 2.0)


def pixel_size(box, width, height):
    """(w, h): the floe rule derives the height from the region aspect
    when only a width was given."""
    if height is None:
        x0, y0, x1, y1 = box
        return width, max(1, round(width * (y1 - y0) / (x1 - x0)))
    return width, height


class Shot:
    """One capture request (um everywhere)."""

    def __init__(self, name, bbox=None, at=None, size=None, anchor="center",
                 px=(1200, None), stretch=False, layers=None, depth=None,
                 mosaic=None, corners=None, line=2.0, line_color="#ffffff",
                 keep_tiles=False):
        if anchor not in ANCHORS:
            raise ValueError("anchor must be one of %s" % (ANCHORS,))
        self.name = name
        self.bbox = bbox
        self.at = at
        self.size = size
        self.anchor = anchor
        self.px = px
        self.stretch = stretch
        self.layers = layers
        self.depth = depth
        self.mosaic = mosaic
        self.corners = corners
        self.line = float(line)
        self.line_color = line_color
        self.keep_tiles = bool(keep_tiles)
        if self.line < 0:
            raise ValueError("line width must be >= 0")
        forms = sum(1 for f in (bbox, at, mosaic, corners) if f is not None)
        if forms > 1:
            raise ValueError("%s: --bbox, --at, --mosaic-at and --corners "
                             "are mutually exclusive" % name)
        if (at is not None or mosaic is not None or corners is not None) \
                and size is None:
            raise ValueError("%s: --at / --mosaic-at / --corners need "
                             "--size W,H" % name)

    @property
    def is_mosaic(self):
        return self.mosaic is not None or self.corners is not None

    def tile_boxes(self, default_box):
        """The region(s) to capture, each already fitted to the pixel
        aspect: one for a plain shot, four (tl, tr, bl, br) for a
        mosaic. Returns (boxes, (w, h) per tile)."""
        if self.mosaic is not None:
            # input is clockwise from top-left (tl, tr, br, bl); the
            # canvas is row-major (tl, tr, bl, br) - review 2026-09-09
            # P2-4: passing the points through swapped the bottom row
            tl, tr, br, bl = self.mosaic
            boxes = [region_from(p, self.size, self.anchor)
                     for p in (tl, tr, bl, br)]
        elif self.corners is not None:
            # the region's four W,H corner rectangles, inside it (the
            # reference tool's rule; review 2026-09-09 P2-5: centring
            # the tiles on the corners shot half outside the region)
            x0, y0, x1, y1 = normalize_box(self.corners)
            w, h = self.size
            if w <= 0 or h <= 0:
                raise ValueError("--size must be positive")
            boxes = [(x0, y1 - h, x0 + w, y1),          # tl
                     (x1 - w, y1 - h, x1, y1),          # tr
                     (x0, y0, x0 + w, y0 + h),          # bl
                     (x1 - w, y0, x1, y0 + h)]          # br
        elif self.at is not None:
            boxes = [region_from(self.at, self.size, self.anchor)]
        elif self.bbox is not None:
            boxes = [normalize_box(self.bbox)]
        else:
            boxes = [normalize_box(default_box)]
        width, height = self.px
        fitted = []
        for box in boxes:
            w, h = pixel_size(box, width, height)
            if height is not None:
                box = fit_aspect(box, w, h, self.anchor, self.stretch)
            fitted.append(box)
        w, h = pixel_size(fitted[0], width, height)
        return fitted, (w, h)


# ---- batch file ------------------------------------------------------------

BATCH_KEYS = ("bbox", "at", "size", "anchor", "px", "stretch", "layers",
              "depth", "mosaic", "corners", "line", "linecolor",
              "keep_tiles")


def parse_batch(text, defaults=None):
    """One shot per line: `NAME key=value ...` (# comments). Keys are the
    render options: bbox, at, size, anchor, px, stretch, layers, depth,
    mosaic (four points), corners, line, linecolor, keep_tiles. Values
    with blanks go in quotes. Unset keys take the command line's."""
    import shlex
    defaults = defaults or {}
    shots = []
    for line_no, raw in enumerate(str(text).splitlines(), 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        try:
            tokens = shlex.split(line)
        except ValueError as exc:
            raise ValueError("batch line %d: %s" % (line_no, exc))
        name = tokens[0]
        if "=" in name or "/" in name or name in (".", ".."):
            raise ValueError("batch line %d: first token is the shot name"
                             % line_no)
        fields = dict(defaults)
        # a line's own region form replaces the defaults' form
        for key in ("bbox", "at", "mosaic", "corners"):
            if any(tok.split("=", 1)[0] in ("bbox", "at", "mosaic",
                                            "corners")
                   for tok in tokens[1:] if "=" in tok):
                fields.pop(key, None)
        for tok in tokens[1:]:
            if "=" not in tok:
                raise ValueError("batch line %d: expected key=value, got %r"
                                 % (line_no, tok))
            key, value = tok.split("=", 1)
            if key not in BATCH_KEYS:
                raise ValueError("batch line %d: unknown key %r (known: %s)"
                                 % (line_no, key, ", ".join(BATCH_KEYS)))
            fields[key] = value
        try:
            shots.append(shot_from_fields(name, fields))
        except ValueError as exc:
            raise ValueError("batch line %d: %s" % (line_no, exc))
    if not shots:
        raise ValueError("batch file names no shots")
    names = [s.name for s in shots]
    if len(set(names)) != len(names):
        raise ValueError("batch shot names must be unique")
    return shots


def _flag(value):
    return str(value).strip().lower() in ("1", "on", "true", "yes")


def shot_from_fields(name, f):
    """Build a Shot from string-valued fields (batch line or CLI)."""
    px = f.get("px", "1200")
    px = px if isinstance(px, tuple) else parse_pixel(px)
    depth = f.get("depth")
    if depth is not None and not isinstance(depth, int):
        depth = None if str(depth) in ("", "full") else int(depth)
    if depth is not None and depth >= 999:
        depth = None
    layers = f.get("layers")
    if layers in ("", "all"):
        layers = None
    return Shot(
        name,
        bbox=parse_lengths(f["bbox"], 4, "bbox") if f.get("bbox") else None,
        at=parse_lengths(f["at"], 2, "at") if f.get("at") else None,
        size=parse_lengths(f["size"], 2, "size") if f.get("size") else None,
        anchor=str(f.get("anchor", "center")),
        px=px,
        stretch=_flag(f.get("stretch", False)),
        layers=layers,
        depth=depth,
        mosaic=parse_points(f["mosaic"]) if f.get("mosaic") else None,
        corners=parse_lengths(f["corners"], 4, "corners")
        if f.get("corners") else None,
        line=float(f.get("line", 2.0)),
        line_color=str(f.get("linecolor", "#ffffff")),
        keep_tiles=_flag(f.get("keep_tiles", False)),
    )


# ---- PNG / mosaic ----------------------------------------------------------

def png_encode(width, height, rgba) -> bytes:
    """RGBA bytes -> PNG (colour type 6, filter 0), stdlib only."""
    if len(rgba) != width * height * 4:
        raise ValueError("rgba buffer does not match %dx%d" % (width, height))
    stride = width * 4
    raw = bytearray()
    for y in range(height):
        raw.append(0)
        raw += rgba[y * stride:(y + 1) * stride]

    def chunk(kind, body):
        c = struct.pack(">I", len(body)) + kind + body
        return c + struct.pack(">I", zlib.crc32(kind + body) & 0xffffffff)

    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6,
                                         0, 0, 0))
            + chunk(b"IDAT", zlib.compress(bytes(raw), 6))
            + chunk(b"IEND", b""))


def line_spans(seam, width, limit):
    """Pixel indices (index, alpha) a separator of `width` centred on the
    boundary between pixel seam-1 and seam covers: floor(width/2) solid
    each side, the remainder as one blended pixel each side."""
    half = width / 2.0
    full = int(half)
    frac = half - full
    spans = []
    for i in range(full):
        spans.append((seam - 1 - i, 1.0))
        spans.append((seam + i, 1.0))
    if frac > 1e-9:
        spans.append((seam - 1 - full, frac))
        spans.append((seam + full, frac))
    return [(i, a) for i, a in spans if 0 <= i < limit]


def compose_mosaic(tiles, tile_w, tile_h, line=2.0, color="#ffffff"):
    """Four RGBA tiles (tl, tr, bl, br) -> one RGBA canvas twice the
    tile size, separator lines drawn over the seams. Returns (rgba,
    info)."""
    if len(tiles) != 4:
        raise ValueError("a mosaic needs four tiles")
    W, H = 2 * tile_w, 2 * tile_h
    canvas = bytearray(W * H * 4)
    stride_t = tile_w * 4
    stride_c = W * 4
    for n, tile in enumerate(tiles):
        if len(tile) != tile_w * tile_h * 4:
            raise ValueError("tile %d has the wrong size" % n)
        ox = (n % 2) * tile_w
        oy = (n // 2) * tile_h
        for y in range(tile_h):
            dst = (oy + y) * stride_c + ox * 4
            src = y * stride_t
            canvas[dst:dst + stride_t] = tile[src:src + stride_t]
    rgb = parse_color(color)

    def blend(offset, alpha):
        if alpha >= 1.0:
            canvas[offset:offset + 3] = bytes(rgb)
        else:
            for k in range(3):
                canvas[offset + k] = int(round(
                    canvas[offset + k] * (1.0 - alpha) + rgb[k] * alpha))
        canvas[offset + 3] = 255

    for x, a in line_spans(tile_w, line, W):
        for y in range(H):
            blend(y * stride_c + x * 4, a)
    for y, a in line_spans(tile_h, line, H):
        for x in range(W):
            blend(y * stride_c + x * 4, a)
    half = line / 2.0
    full = int(half)
    frac = half - full
    parts = []
    if full:
        parts.append("%d solid" % full)
    if frac > 1e-9:
        parts.append("1 at %.0f%%" % (frac * 100))
    info = {"width": W, "height": H, "tile": [tile_w, tile_h], "line": line,
            "line_pixels": (" + ".join(parts) + " on each side") if parts
            else "none", "line_color": color}
    return bytes(canvas), info


# ---- running ------------------------------------------------------------------

def _write_atomic(path, data):
    parent = os.path.dirname(os.path.abspath(path)) or "."
    fd, staged = tempfile.mkstemp(
        prefix=".%s.floe-shot-" % (os.path.basename(path) or "shot"),
        dir=parent)
    try:
        with os.fdopen(fd, "wb") as fh:
            fh.write(data)
            fh.flush()
            os.fsync(fh.fileno())
        os.replace(staged, path)
    except Exception:
        try:
            os.unlink(staged)
        except OSError:
            pass
        raise


class ShotRunner:
    """One started render worker; `capture` as many times as needed."""

    def __init__(self, cache, frames=False, labels=False, label_font_px=14,
                 timeout_s=600, cut_px=0.0, thin=None):
        from .service import make_render_worker
        self.cache = cache
        self.dbu = float(cache.meta["dbu"])
        self.frames = frames
        self.labels = labels
        self.label_font_px = label_font_px
        self.timeout_s = timeout_s
        # the planner's size cut in screen px: 0 = exact (the archival
        # default), or the viewer's detail (5 / 3 / 1 px) so a capture
        # reproduces what the viewer shows - field 2026-09-10: a
        # region the viewer dropped past a zoom rendered fine here
        # because captures are exact, which pinned the cause to the
        # plan-stage cut
        self.cut_px = max(0.0, float(cut_px or 0.0))
        # the page hairline policy: None = the source's default (a
        # jobdeck keeps thin pages, a layout culls them), or an
        # explicit "keep" / "cull"
        self.thin = thin
        self.worker = make_render_worker(cache)
        self._gen = 0
        self.over_budget_pages = 0
        self.worker.start()
        # archival output keeps solid fills: the viewer's speckle is a
        # live-view presentation choice (as `floe render` always did)
        solid = "\n".join(["*" * 16] * 16)
        keys = [(int(l["layer"]), int(l["datatype"]))
                for l in cache.meta["layers"]]
        self.worker.submit({"kind": "repattern",
                            "fills": [(k, solid) for k in keys],
                            "widths": [(k, 1) for k in keys]})

    def stop(self):
        self.worker.stop()

    def capture(self, box_um, width, height, layers=None, depth=None,
                fmt="png"):
        """One settled frame of `box_um` at width x height: PNG bytes or
        raw RGBA bytes (`fmt`)."""
        self._gen += 1
        gen = self._gen
        bbox = tuple(v / self.dbu for v in box_um)
        self.worker.submit({
            "kind": "render", "gen": gen, "scope": "headless",
            "bbox": bbox, "view": None, "w": int(width), "h": int(height),
            "depth": depth, "cut_px": self.cut_px,
            # the viewer's density gate (LOD) rides with its cut
            "lod": self.cut_px > 0,
            "thin": self.thin,
            "frames": self.frames, "labels": self.labels,
            "label_font_px": self.label_font_px, "abstract": False,
            "visible": layers, "frame_format": fmt,
        })
        while True:
            try:
                result = self.worker.res.get(timeout=self.timeout_s)
            except queue.Empty:
                raise RuntimeError("render service timeout")
            if result.get("kind") == "error":
                raise RuntimeError(result.get("msg", "render failed"))
            if result.get("kind") != "frame" or result.get("gen") != gen:
                continue
            if result.get("preview") or result.get("bg") or \
                    result.get("refining"):
                continue
            if fmt == "raw":
                data = result.get("rgba", b"")
                if len(data) != width * height * 4:
                    raise RuntimeError("render service returned a "
                                       "truncated raw frame")
            else:
                data = result.get("png", b"")
                if not data.startswith(b"\x89PNG\r\n\x1a\n"):
                    raise RuntimeError("render service returned an "
                                       "invalid PNG")
            # analysis 2026-09-09: a capture that stopped at the page
            # budget is not a complete image; the caller records it
            self.over_budget_pages = int(result.get("over_budget_pages", 0)
                                         or 0)
            return data, result


def run_shots(cache, shots, out, report=None, frames=False, labels=False,
              label_font_px=14, log=print, batch=False, cut_px=0.0,
              thin=None):
    """Render every shot through one open. `out` is the PNG path of the
    one shot, or with `batch` a directory (<out>/<name>.png) - stated
    by the caller, never inferred from the shot count or from whether
    the directory already exists (review 2026-09-09 P2-6: a one-line
    batch into a new directory wrote a PNG named like the directory).
    Returns the report rows (also written as JSON to `report`)."""
    single = not batch
    if single and len(shots) != 1:
        raise ValueError("a single --out PNG takes exactly one shot")
    if not single:
        os.makedirs(out, exist_ok=True)
    dbu = float(cache.meta["dbu"])
    bb = cache.meta["bbox"]
    default_box = (bb[0] * dbu, bb[1] * dbu, bb[2] * dbu, bb[3] * dbu)
    # a jobdeck's skipped placements (missing source, absent layer,
    # unsupported container) are absent from EVERY capture: each shot
    # row carries them so a per-shot reader sees the same verdict as
    # the report (review 2026-09-09: complete=true rows under an
    # incomplete report)
    skipped = list((cache.meta.get("jobdeck") or {}).get("skipped") or [])
    rows = []
    runner = ShotRunner(cache, frames=frames, labels=labels,
                        label_font_px=label_font_px, cut_px=cut_px, thin=thin)
    try:
        for shot in shots:
            t0 = time.perf_counter()
            path = out if single else os.path.join(out, shot.name + ".png")
            layers = (cache.resolve_layers(shot.layers)
                      if shot.layers else None)
            boxes, (w, h) = shot.tile_boxes(default_box)
            row = {"name": shot.name, "out": path, "pixel": [w, h],
                   "layers": shot.layers, "depth": shot.depth}
            over_budget = 0
            if shot.is_mosaic:
                tiles = []
                for box in boxes:
                    rgba, _ = runner.capture(box, w, h, layers, shot.depth,
                                             fmt="raw")
                    over_budget += runner.over_budget_pages
                    tiles.append(rgba)
                canvas, info = compose_mosaic(tiles, w, h, shot.line,
                                              shot.line_color)
                _write_atomic(path, png_encode(info["width"],
                                               info["height"], canvas))
                if shot.keep_tiles:
                    stem = path[:-4] if path.lower().endswith(".png") \
                        else path
                    for tag, rgba in zip(MOSAIC_ORDER, tiles):
                        _write_atomic("%s_%s.png" % (stem, tag),
                                      png_encode(w, h, rgba))
                row.update({"mosaic": info,
                            "tiles": {tag: list(box) for tag, box
                                      in zip(MOSAIC_ORDER, boxes)},
                            "pixel": [info["width"], info["height"]]})
            else:
                png, _ = runner.capture(boxes[0], w, h, layers, shot.depth)
                over_budget += runner.over_budget_pages
                _write_atomic(path, png)
                row["bbox_um"] = list(boxes[0])
            row["ms"] = round((time.perf_counter() - t0) * 1000)
            row["over_budget_pages"] = over_budget
            row["skipped_placements"] = len(skipped)
            row["complete"] = over_budget == 0 and not skipped
            if over_budget and log:
                log("[floe] WARNING: %s stopped at the page budget: %d "
                    "page(s) not drawn" % (path, over_budget))
            rows.append(row)
            if log:
                if shot.is_mosaic:
                    log("[floe] rendered %s (%dx%d mosaic of 4 x %dx%d, "
                        "line %s) in %.2fs" % (
                            path, row["pixel"][0], row["pixel"][1], w, h,
                            row["mosaic"]["line_pixels"], row["ms"] / 1000))
                else:
                    x0, y0, x1, y1 = boxes[0]
                    log("[floe] rendered %s (%dx%d, %.4f,%.4f,%.4f,%.4f "
                        "um) in %.2fs" % (path, w, h, x0, y0, x1, y1,
                                          row["ms"] / 1000))
    finally:
        runner.stop()
    # a jobdeck that could not draw every placement says so in the
    # report and the log, and the caller exits non-zero (review
    # 2026-09-09 P1-1: a thinner PNG must never look complete)
    if skipped and log:
        log("[floe] WARNING: %d jobdeck placement(s) not drawn: %s"
            % (len(skipped), "; ".join(
                "CHIP %s $%d %s %s" % (r["chip"], r["idx"], r["tc"],
                                       r["reason"]) for r in skipped[:5])
               + (" ..." if len(skipped) > 5 else "")))
    over_budget_total = sum(r["over_budget_pages"] for r in rows)
    if report:
        doc = {"source": cache.src, "dbu": dbu, "shots": rows,
               "cut_px": cut_px, "thin": thin or "auto",
               "complete": all(r["complete"] for r in rows)}
        if cache.meta.get("jobdeck"):
            doc["jobdeck"] = {"complete": doc["complete"],
                              "skipped": skipped,
                              "over_budget_pages": over_budget_total,
                              "view": cache.meta["jobdeck"].get("mode"),
                              "levels": cache.meta["jobdeck"].get("levels")}
        with open(report, "w") as fh:
            json.dump(doc, fh, indent=1)
        if log:
            log("[floe] report %s (%d shot(s)%s)" % (
                report, len(rows),
                ", INCOMPLETE" if (skipped or over_budget_total) else ""))
    return rows
