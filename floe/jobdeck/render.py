"""M2: the jobdeck composite through renderd.

`deck_spec_lines` turns an M1 plan into the line-based spec that
`floe_render_core::Deck` opens (`open deck=<spec>`): one `source` per
distinct `.floe` cache, one `layer` per deck output layer (the colour
of the current mode), one `placement` per (CHIP, entry, row, LY/DT)
with `scale` = deck dbu per source dbu and `dx`/`dy` on the deck grid.
Magnification exists only in those lines; the caches are untouched.

`DeckRenderWorker` is the viewer's Rust worker pointed at a spec: the
same frame protocol, coordinates in deck dbu, layers keyed as
`<out>/0`. Queries (pick/snap/clip), labels, hierarchy frames and the
margin prefetch are not available for a deck yet (M3).
"""

from __future__ import annotations

import os
import queue
import tempfile

from ..cache import Cache
from .color import MODE_CHIP, MODE_IDENTIFIER
from .geom import SKIP_EMPTY_LAYER, SKIP_NOT_INDEXED, skip_record


def _hex(text: str) -> str:
    return text.encode("utf-8").hex()


def deck_layer_name(deck, row: dict, mode: str) -> str:
    """The name a deck output layer shows in a layer list."""
    tag = "%s " % row["chip"] if row.get("chip") else ""
    if mode == MODE_CHIP:
        return "%s$%d%s" % (tag, row["idx"],
                            " " + row["title"] if row["title"] else "")
    if mode == MODE_IDENTIFIER:
        return "%s$%d%s" % (tag, row["idx"],
                            " " + row["title"] if row["title"] else "")
    return "%sLY%d.DT%d" % (tag, row["ly"], row["dt"])


def _source_layers(cache_dir_src: str):
    """(ly, dt) pairs a .floe cache holds, via its meta.json."""
    c = Cache(cache_dir_src)
    c.load()
    return {(int(l["layer"]), int(l["datatype"])) for l in c.meta["layers"]}


def deck_spec_lines(deck, placements, stats, scheme, colormap, catalog):
    """(lines, ledger): the spec text and the placements it had to leave
    out - a source without a fresh .floe cache (`not_indexed`) or a
    cache without the entry's LY/DT (`empty_layer`). Nothing is
    silently thinner: the ledger goes to the report and the summary."""
    dbu = float(stats["dbu"])
    out_of = stats["out_of"]
    table = stats["layer_table"]
    sources: list[str] = []
    source_index: dict = {}
    source_layers: dict = {}
    ledger = []
    seen_skip = set()
    lines = ["# floe2 jobdeck composite spec (docs/JOBDECK.ko.md M2)",
             "deck unit=%r" % dbu]
    placement_lines = []
    used_outs = set()
    for p in placements:
        info = catalog.infos.get(p.tc)
        if info is None or not info.ok() or not info.indexed:
            key = (p.chip, p.idx, p.tc, SKIP_NOT_INDEXED)
            if key not in seen_skip:
                seen_skip.add(key)
                ledger.append(skip_record(
                    p.chip, p.idx, p.tc, -1, 0, SKIP_NOT_INDEXED,
                    "no fresh <src>.floe cache (run --index)", "spec",
                    [(p.jx, p.jy)]))
            continue
        if p.tc not in source_index:
            source_index[p.tc] = len(sources)
            sources.append(info.path)
            source_layers[p.tc] = _source_layers(info.path)
            lines.append("source path_hex=%s" % _hex(info.cache_dir))
        if (p.ly, p.dt) not in source_layers[p.tc]:
            key = (p.chip, p.idx, p.tc, p.ly, p.dt)
            if key not in seen_skip:
                seen_skip.add(key)
                ledger.append(skip_record(
                    p.chip, p.idx, p.tc, -1, 0, SKIP_EMPTY_LAYER,
                    "cache has no layer %d/%d" % (p.ly, p.dt), "spec",
                    [(p.jx, p.jy)], ly=p.ly, dt=p.dt))
            continue
        out = out_of[(p.chip, p.idx, p.ly, p.dt)]
        used_outs.add(out)
        scale = p.mag * float(info.dbu) / dbu
        placement_lines.append(
            "placement source=%d layer=%d/%d out=%d scale=%r dx=%d dy=%d "
            "order=%d" % (source_index[p.tc], p.ly, p.dt, out, scale,
                          p.ix, p.iy, out))
    for row in table:
        if row["out"] not in used_outs:
            continue
        color = _row_color(row, scheme, colormap)
        lines.append("layer out=%d name_hex=%s color=%s fill=solid width=1"
                     % (row["out"], _hex(deck_layer_name(deck, row,
                                                         scheme.mode)),
                        color))
    lines.extend(placement_lines)
    return lines, ledger


def _row_color(row, scheme, colormap) -> str:
    if scheme.mode == MODE_IDENTIFIER:
        return colormap.get(row["idx"], scheme.fallback)
    if scheme.mode == MODE_CHIP:
        return colormap.get(row["chip"], scheme.fallback)
    return colormap.get((row["ly"], row["dt"]), scheme.fallback)


def write_deck_spec(path, deck, placements, stats, scheme, colormap,
                    catalog):
    lines, ledger = deck_spec_lines(deck, placements, stats, scheme,
                                    colormap, catalog)
    if not any(l.startswith("placement ") for l in lines):
        raise ValueError("no placement can be drawn: %s" % (
            ", ".join("%s $%d %s" % (r["chip"], r["idx"], r["reason"])
                      for r in ledger) or "empty selection"))
    with open(path, "w", encoding="ascii") as fh:
        fh.write("\n".join(lines) + "\n")
    return ledger


def deck_layers_meta(stats, scheme, colormap):
    """The `meta["layers"]` rows the Rust worker keys its styles on:
    one per deck output layer, as layer=<out> datatype=0."""
    rows = []
    for row in stats["layer_table"]:
        rows.append({"layer": int(row["out"]), "datatype": 0,
                     "name": "out%d" % row["out"],
                     "color": _row_color(row, scheme, colormap)})
    return rows


class _DeckCacheShim:
    """What RustRenderWorker reads from its cache object, for a deck."""

    def __init__(self, spec_path, deck_path, dbu, layers):
        self.dir = spec_path
        self.src = deck_path
        self.meta = {"dbu": float(dbu), "layers": layers, "vfs": True}

    def exists(self):
        return os.path.isfile(self.dir)


class DeckRenderWorker:
    """Factory: a RustRenderWorker that opens `open deck=<spec>`."""

    def __new__(cls, spec_path, deck_path, dbu, layers, **kw):
        from ..rust_render import RustRenderWorker

        class _Worker(RustRenderWorker):
            supports_margin_prefetch = False
            supports_label_font_px = False

            def _open_command(self):
                return "open deck=%s budget_mb=%d jobs=%d" % (
                    self._cache_path, self._budget_mb, self._jobs_count)

            def _submit_snap(self, job):
                raise RuntimeError("snap is not available for a jobdeck yet")

            def _submit_pick(self, job):
                raise RuntimeError("pick is not available for a jobdeck yet")

            def _submit_clip(self, job):
                raise RuntimeError("clip is not available for a jobdeck yet")

        return _Worker(_DeckCacheShim(spec_path, deck_path, dbu, layers),
                       **kw)


def render_deck_png(spec_path, deck_path, dbu, layers, bbox_dbu, width,
                    height, out_png, visible_outs=None, depth=None,
                    timeout_s=600):
    """Headless composite: one PNG of `bbox_dbu` (deck dbu) at
    width x height through renderd. Solid fills, no labels/frames."""
    worker = DeckRenderWorker(spec_path, deck_path, dbu, layers)
    worker.start()
    try:
        # archival output keeps solid fills (the viewer's speckle is a
        # live-view presentation choice), as `floe2 render` does
        solid = "\n".join(["*" * 16] * 16)
        keys = [(int(l["layer"]), 0) for l in layers]
        worker.submit({"kind": "repattern",
                       "fills": [(k, solid) for k in keys],
                       "widths": [(k, 1) for k in keys]})
        if visible_outs is None:
            visible = None
        else:
            visible = [(int(o), 0) for o in visible_outs]
        worker.submit({
            "kind": "render", "gen": 1, "scope": "headless",
            "bbox": tuple(float(v) for v in bbox_dbu),
            "view": None, "w": int(width), "h": int(height),
            "depth": depth, "cut_px": 0.0, "lod": False,
            "frames": False, "labels": False,
            "abstract": False, "visible": visible,
            "frame_format": "png",
        })
        while True:
            try:
                result = worker.res.get(timeout=timeout_s)
            except queue.Empty:
                raise RuntimeError("renderd timeout")
            if result.get("kind") == "error":
                raise RuntimeError(result.get("msg", "render failed"))
            if result.get("kind") != "frame" or result.get("gen") != 1:
                continue
            if result.get("refining"):
                continue
            png = result.get("png", b"")
            if not png.startswith(b"\x89PNG\r\n\x1a\n"):
                raise RuntimeError("renderd returned an invalid PNG")
            break
    finally:
        worker.stop()
    parent = os.path.dirname(os.path.abspath(out_png)) or "."
    fd, staged = tempfile.mkstemp(prefix=".floe-jobdeck-", dir=parent)
    try:
        with os.fdopen(fd, "wb") as fh:
            fh.write(png)
            fh.flush()
            os.fsync(fh.fileno())
        os.replace(staged, out_png)
    except Exception:
        try:
            os.unlink(staged)
        except OSError:
            pass
        raise
    return result


def fit_bbox_to_pixels(bbox, width, height):
    """Expand `bbox` (x0, y0, x1, y1) about its centre to the pixel
    aspect so the image is never distorted (the KLayout tool's default:
    expand, do not stretch)."""
    x0, y0, x1, y1 = (float(v) for v in bbox)
    w, h = x1 - x0, y1 - y0
    if w <= 0 or h <= 0:
        raise ValueError("bbox must have positive width and height")
    aspect = float(width) / float(height)
    if w / h < aspect:
        nw = h * aspect
        cx = (x0 + x1) / 2.0
        x0, x1 = cx - nw / 2.0, cx + nw / 2.0
    else:
        nh = w / aspect
        cy = (y0 + y1) / 2.0
        y0, y1 = cy - nh / 2.0, cy + nh / 2.0
    return (x0, y0, x1, y1)
