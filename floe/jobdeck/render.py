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
from .color import MODE_CHIP, MODE_IDENTIFIER, MODE_LEVEL
from .geom import SKIP_EMPTY_LAYER, SKIP_NOT_INDEXED, skip_record


def _hex(text: str) -> str:
    return text.encode("utf-8").hex()


def view_layers(deck, stats, scheme, colormap):
    """The deck layers a VIEW shows, in paint order - MDPView's two
    jobdeck views plus our source-layer view:

        level   one row per mask level ($1 METAL1, ...), ascending;
                keyed level/0
        chip    the CHIP blocks in deck order, each expanding into the
                levels it places: the CHIP row is keyed <pos>/0 and
                holds nothing itself, its levels are <pos>/<level> in
                the CHIP's colour - the viewer's layer panel shows
                "+CHIP ID001" with "$1 METAL1", "$2 VIA1" underneath
                and a collapsed CHIP toggles them all
        layer   one row per (LY, DT) placed, keyed LY/DT (LY groups)

    Rows: {"out", "key", "layer", "datatype", "name", "color"}; `out`
    is the spec/style index, layer/datatype the key a viewer and the
    style file use. Placements map to a row with `view_out_of`. (The
    M1 report's `layer_table` keeps the finer (level, ly, dt) rows for
    analysis; a viewer lists what it colours.)"""
    rows = []
    mode = scheme.mode
    if mode == MODE_LEVEL:
        for idx in deck.identifiers():
            title = deck.title(idx)
            rows.append({"key": idx, "layer": idx, "datatype": 0,
                         "name": "$%d%s" % (idx, " " + title if title
                                            else "")})
    elif mode == MODE_CHIP:
        seen = []
        for c in deck.chips:
            if c.id not in seen:
                seen.append(c.id)
        for pos, cid in enumerate(seen, 1):
            rows.append({"key": ("chip", cid), "layer": pos, "datatype": 0,
                         "name": "CHIP %s" % cid, "color_key": cid})
            levels = sorted({e.idx for c in deck.chips if c.id == cid
                             for e in c.entries})
            for idx in levels:
                title = deck.title(idx)
                rows.append({"key": (cid, idx), "layer": pos,
                             "datatype": idx, "color_key": cid,
                             "name": "$%d%s" % (idx, " " + title if title
                                                else "")})
    else:
        pairs = sorted({(r["ly"], r["dt"]) for r in stats["layer_table"]})
        for ly, dt in pairs:
            rows.append({"key": (ly, dt), "layer": ly, "datatype": dt,
                         "name": "LY%d.DT%d" % (ly, dt)})
    for out, row in enumerate(rows):
        row["out"] = out
        row["color"] = colormap.get(row.pop("color_key", row["key"]),
                                    scheme.fallback)
    return rows


def view_out_of(rows, scheme):
    """placement -> `out` of its view row."""
    by_key = {r["key"]: r["out"] for r in rows}
    mode = scheme.mode

    def out_of(p):
        if mode == MODE_LEVEL:
            return by_key[p.idx]
        if mode == MODE_CHIP:
            return by_key[(p.chip, p.idx)]
        return by_key[(p.ly, p.dt)]
    return out_of


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
    rows = view_layers(deck, stats, scheme, colormap)
    out_of = view_out_of(rows, scheme)
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
        out = out_of(p)
        used_outs.add(out)
        scale = p.mag * float(info.dbu) / dbu
        placement_lines.append(
            "placement source=%d layer=%d/%d out=%d scale=%r dx=%d dy=%d "
            "order=%d" % (source_index[p.tc], p.ly, p.dt, out, scale,
                          p.ix, p.iy, out))
    for row in rows:
        # chip view keeps its CHIP rows (they hold no placement but
        # head the expandable group); other views list what is drawn
        if row["out"] not in used_outs and row["datatype"] != 0 \
                or (row["out"] not in used_outs
                    and scheme.mode != MODE_CHIP):
            continue
        lines.append("layer out=%d key=%d/%d name_hex=%s color=%s "
                     "fill=solid width=1"
                     % (row["out"], row["layer"], row["datatype"],
                        _hex(row["name"]), row["color"]))
    lines.extend(placement_lines)
    return lines, ledger


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


def deck_layers_meta(deck, stats, scheme, colormap, placements=None):
    """The `meta["layers"]` rows a viewer and the Rust worker key on:
    one per view layer, as layer=<out> datatype=0, with the row's name
    and colour (and, like a cache, a stored_shapes count = placements)."""
    rows = view_layers(deck, stats, scheme, colormap)
    counts = {}
    if placements is not None:
        out_of = view_out_of(rows, scheme)
        for p in placements:
            counts[out_of(p)] = counts.get(out_of(p), 0) + 1
    return [{"layer": int(r["layer"]), "datatype": int(r["datatype"]),
             "name": r["name"], "color": r["color"],
             "stored_shapes": counts.get(r["out"], 0)}
            for r in rows]


class _DeckCacheShim:
    """What RustRenderWorker reads from its cache object, for a deck."""

    def __init__(self, spec_path, deck_path, dbu, layers):
        self.dir = spec_path
        self.src = deck_path
        self.meta = {"dbu": float(dbu), "layers": layers, "vfs": True}

    def exists(self):
        return os.path.isfile(self.dir)


class DeckRenderWorker:
    """Factory: a RustRenderWorker that opens `open deck=<spec>`.

    `cache` is anything cache-shaped for a deck (`floe.jobdeck.viewer.
    DeckCache`, or the shim below): `.dir` is the spec path, `.src` the
    deck, `.meta` carries dbu and the view layers."""

    def __new__(cls, cache, **kw):
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

        return _Worker(cache, **kw)


def render_deck_png(spec_path, deck_path, dbu, layers, bbox_dbu, width,
                    height, out_png, visible_outs=None, depth=None,
                    timeout_s=600):
    """Headless composite: one PNG of `bbox_dbu` (deck dbu) at
    width x height through renderd. Solid fills, no labels/frames."""
    worker = DeckRenderWorker(
        _DeckCacheShim(spec_path, deck_path, dbu, layers))
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
            visible = [(int(l["layer"]), int(l["datatype"]))
                       for l in layers if l["name"] in visible_outs
                       or (l["layer"], l["datatype"]) in visible_outs]
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
