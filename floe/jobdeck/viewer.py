"""M3: a jobdeck opened like a layout.

`DeckCache` is what `floe2 view/info/render <deck.jb>` hand to the GUI
and the headless paths in place of `floe.cache.Cache`: the same
attributes they read (`src`, `dir`, `meta` with dbu/bbox/layers/src/
grid, `exists`, `load`, `is_stale`, `resolve_layers`), backed by the
M1 plan and the M2 spec. Its "cache" is the set of `<src>.floe` caches
of the deck's sources - `deck_ready` says whether they all exist - and
its `dir` is the spec file renderd opens (`open deck=`), written into a
private work directory.

Colour modes (identifier / layer / chip) change the view layer table
and the spec, so `set_mode` re-plans and rewrites; the viewer then
restarts its worker as it does for a newly loaded layout.
"""

from __future__ import annotations

import os
import shutil
import tempfile

from .color import MODES, MODE_LEVEL, normalize_mode
from .geom import MISSING_SKIP
from .parser import parse_jobdeck
from .plan import plan_deck
from .render import deck_layers_meta, view_layers, write_deck_spec
from ..cache import apply_personal_colors
from .sources import SourceCatalog

DECK_SUFFIXES = (".jb",)


def is_deck_path(path) -> bool:
    return bool(path) and str(path).lower().endswith(DECK_SUFFIXES)


def deck_sources_dir(path, sources_dir=None) -> str:
    return sources_dir or os.path.dirname(os.path.abspath(path)) or "."


def normalize_levels(ids):
    """A level selection as a sorted list of distinct ints, None for
    all levels (an empty selection is None too)."""
    if ids is None:
        return None
    out = sorted({int(i) for i in ids})
    return out or None


def level_rows(deck):
    """What a Calibre-style load dialog lists: one row per mask level
    with its name, the CHIPs placing it, its placed instances and its
    sources."""
    rows = []
    for idx in deck.levels():
        chips, instances, sources = [], 0, []
        for c in deck.chips:
            entries = [e for e in c.entries if e.idx == idx]
            if not entries:
                continue
            chips.append(c.id)
            instances += len(entries) * max(1, len(c.rows))
            for e in entries:
                if e.tc not in sources:
                    sources.append(e.tc)
        rows.append({"level": idx, "name": deck.title(idx), "chips": chips,
                     "instances": instances, "sources": sources})
    return rows


def level_row_text(row, keep: int = 3):
    """(summary, full) for one level row of the load dialog: the
    summary names at most `keep` sources and counts the rest (review
    2026-09-10 (9th) P2-2: 250 file names in one label made the dialog
    26,000 px wide); `full` lists them all for the tooltip."""
    names = [os.path.basename(s) for s in row["sources"]]
    shown = ", ".join(names[:keep])
    if len(names) > keep:
        shown += ", +%d more" % (len(names) - keep)
    summary = "%d CHIP%s · %d instance%s · %d source%s: %s" % (
        len(row["chips"]), "" if len(row["chips"]) == 1 else "s",
        row["instances"], "" if row["instances"] == 1 else "s",
        len(names), "" if len(names) == 1 else "s", shown)
    full = "CHIPs: %s\nsources:\n  %s" % (
        ", ".join(row["chips"]), "\n  ".join(names))
    return summary, full


def deck_ready(path, sources_dir=None, ids=None) -> bool:
    """True when every source the deck names that CAN be drawn (probes
    ok) has a fresh <src>.floe cache - the deck's equivalent of
    `<src>.floe` existing. A missing, unreadable or unknown-format
    source is a skipped placement in the ledger, not a reason to keep
    the deck closed (field 2026-09-09: three 'file not found' sources
    blocked the viewer after everything else was indexed); at least one
    drawable source is required."""
    try:
        deck = parse_jobdeck(path, strict=True)
    except (OSError, ValueError):
        return False
    catalog = SourceCatalog(deck_sources_dir(path, sources_dir))
    catalog.probe_all(deck.sources(normalize_levels(ids)))
    drawable = [i for i in catalog.infos.values() if i.ok()]
    return bool(drawable) and all(i.indexed for i in drawable)


class DeckCache:
    """Cache-shaped view of a jobdeck (see the module docstring)."""

    is_jobdeck = True

    def __init__(self, path, mode: str = MODE_LEVEL, sources_dir=None,
                 ids=None):
        self.src = os.path.abspath(path)
        self.mode = normalize_mode(mode)
        # the mask levels loaded (Calibre-style level selection at
        # load, user call 2026-09-10); None = every level
        self.ids = normalize_levels(ids)
        self.sources_dir = deck_sources_dir(self.src, sources_dir)
        self.work = None
        self.dir = None            # the spec renderd opens
        self.meta = None
        self.deck = None
        self.catalog = None
        self.placements = None
        self.stats = None
        self.scheme = None
        self.colormap = None
        self.ledger = []
        self._loaded_mtime = None
        self.layout_mode = None    # KLayout worker option, unused
        self._visibility = {}      # session only; level/chip share leaf keys

    # ---- Cache protocol ---------------------------------------------
    def exists(self) -> bool:
        return os.path.isfile(self.src)

    def unindexed(self):
        """Sources that could be drawn but have no fresh cache yet; a
        missing/unreadable source is not listed (it is a skipped
        placement, reported when the deck opens)."""
        deck = parse_jobdeck(self.src, strict=True)
        catalog = SourceCatalog(self.sources_dir)
        catalog.probe_all(deck.sources(self.ids))
        return [tc for tc, i in sorted(catalog.infos.items())
                if i.ok() and not i.indexed]

    def load(self):
        if self.work is None:
            self.work = tempfile.mkdtemp(prefix="floe-jobdeck-")
        if self.ids is not None:
            have = parse_jobdeck(self.src, strict=True).levels()
            unknown = [i for i in self.ids if i not in have]
            if unknown:
                raise ValueError(
                    "level%s %s not in the deck (it places %s)" % (
                        "s" if len(unknown) > 1 else "",
                        ",".join(str(i) for i in unknown),
                        ",".join(str(i) for i in have)))
        (self.deck, self.catalog, self.placements, self.stats,
         self.scheme, self.colormap) = plan_deck(
            self.src, sources_dir=self.sources_dir, load_ids=self.ids,
            mode=self.mode, missing=MISSING_SKIP)
        spec = os.path.join(self.work, "deck-%s.spec" % self.mode)
        self.ledger = write_deck_spec(
            spec, self.deck, self.placements, self.stats, self.scheme,
            self.colormap, self.catalog)
        self.dir = spec
        self.meta = self._build_meta()
        # the saved <props_src>.layerprops colours overlay the view's
        # palette exactly as Cache.load does for a layout (review
        # 2026-09-09 P2-3: widths came back, colours did not)
        apply_personal_colors(self.meta, self.props_src)
        if self.mode == MODE_LEVEL:
            colors = {r["layer"]: r["color"] for r in self.meta["layers"]
                      if r.get("jobdeck_head")}
            for row in self.meta["layers"]:
                row["color"] = colors[row["layer"]]
        self._loaded_mtime = int(os.stat(self.src).st_mtime)
        return self.meta

    def is_stale(self) -> bool:
        try:
            return int(os.stat(self.src).st_mtime) != self._loaded_mtime
        except OSError:
            return True

    @property
    def props_src(self) -> str:
        """The path layerprops are keyed by. Each view has its own key
        space (level/chip view level/source, source layer view
        LY/DT), so each view keeps its own <key>.layerprops: the level
        view's is the deck's (<deck>.jb.layerprops), the others are
        <deck>.chip-by-level.jb / <deck>.layer.jb - a name whose <stem>.layerprops
        fallback cannot land on another view's file."""
        if self.mode == MODE_LEVEL:
            return self.src
        stem, ext = os.path.splitext(self.src)
        # Old chip props use CHIP-position/level keys. Never silently
        # apply them to the new level/source key space.
        mode_key = "chip-by-level" if self.mode == "chip" else self.mode
        return "%s.%s%s" % (stem, mode_key, ext)

    def resolve_layers(self, spec):
        """Names or L/D keys, with level heads expanded to their chips.
        A source name selects all matching rows, even across levels.
        Legacy '$n TITLE'/'$n' level selectors remain accepted; source
        layer DT0 is a real layer, never a virtual group head.
        """
        if not spec or spec == "all":
            return None
        rows = self.meta["layers"]
        byname = {}
        for l in rows:
            byname.setdefault(l["name"], []).append((l["layer"],
                                                     l["datatype"]))
            if l.get("jobdeck_head"):
                idx = l["layer"]
                title = self.deck.title(idx)
                for alias in {"$%d" % idx,
                              "$%d%s" % (idx, " " + title if title else "")}:
                    byname.setdefault(alias, []).append((idx, 0))
        out = []
        for tok in spec.split(","):
            tok = tok.strip()
            if not tok:
                continue
            if tok in byname:
                keys = list(byname[tok])
            elif "/" in tok:
                l, d = tok.split("/")
                keys = [(int(l), int(d))]
                if keys[0] not in {(r["layer"], r["datatype"])
                                   for r in rows}:
                    raise ValueError("unknown deck layer: %r (known: %s)"
                                     % (tok, sorted(byname)))
            else:
                raise ValueError("unknown deck layer: %r (known: %s)"
                                 % (tok, sorted(byname)))
            heads = {(r["layer"], r["datatype"]) for r in rows
                     if r.get("jobdeck_head")}
            for key in keys:
                out.append(key)
                if key in heads:
                    out.extend((r["layer"], r["datatype"]) for r in rows
                               if r["layer"] == key[0] and r["datatype"] != 0)
        return list(dict.fromkeys(out))

    def close(self):
        if self.work is not None:
            shutil.rmtree(self.work, ignore_errors=True)
            self.work = None

    # ---- deck-specific --------------------------------------------------
    def save_visibility(self, visible):
        """Remember exact leaf choices, independent of panel mode."""
        scope = "layer" if self.mode == "layer" else "deck"
        self._visibility[scope] = {
            (r["layer"], r["datatype"]):
            (r["layer"], r["datatype"]) in visible
            for r in self.meta["layers"]}

    def restore_visibility(self, default):
        scope = "layer" if self.mode == "layer" else "deck"
        saved = self._visibility.get(scope, {})
        keys = {(r["layer"], r["datatype"]) for r in self.meta["layers"]}
        return {key for key in keys if saved.get(key, key in default)}

    def set_mode(self, mode: str):
        """Switch between MDPView's level view and chip view (and our
        source layer view); re-plans and rewrites the spec."""
        previous = self.__dict__.copy()
        self.mode = normalize_mode(mode)
        try:
            return self.load()
        except Exception:
            self.__dict__.update(previous)
            raise

    def set_levels(self, ids):
        """Load another level selection (None = all); re-plans and
        rewrites the spec like set_mode."""
        self.ids = normalize_levels(ids)
        return self.load()

    def view_rows(self):
        return view_layers(self.deck, self.stats, self.scheme, self.colormap)

    @property
    def skipped(self):
        """Every placement the deck asked for that is not drawn: plan-
        time skips (missing/unreadable source) and spec-time ones
        (not_indexed, empty_layer)."""
        return list(self.stats["skipped"]) + list(self.ledger)

    @property
    def incomplete(self) -> bool:
        return bool(self.skipped)

    def _build_meta(self):
        st = self.stats
        dbu = float(st["dbu"])
        bb = st["bbox_um"]
        if bb is None:
            raise ValueError("jobdeck %s places nothing" % self.src)
        bbox = [int(round(bb[0] / dbu)), int(round(bb[1] / dbu)),
                int(round(bb[2] / dbu)), int(round(bb[3] / dbu))]
        fst = os.stat(self.src)
        return {
            "dbu": dbu,
            "bbox": bbox,
            "layers": deck_layers_meta(self.deck, st, self.scheme,
                                       self.colormap, self.placements),
            "src": {"path": self.src, "size": fst.st_size,
                    "mtime": int(fst.st_mtime)},
            # one "tile" spanning the deck: the viewer's minimap and
            # live caps read a grid, a deck has none
            "grid": {"nx": 1, "ny": 1, "x0": bbox[0], "y0": bbox[1],
                     "tile_w": max(1, bbox[2] - bbox[0]),
                     "tile_h": max(1, bbox[3] - bbox[1])},
            "vfs": True,
            "top_cell": self.deck.jb_name or os.path.basename(self.src),
            "jobdeck": {
                "mode": self.mode,
                "chips": len(self.deck.chips),
                "identifiers": self.deck.identifiers(),
                # the level selection this load carries (None = all)
                "levels": self.ids,
                "sources": len(self.deck.sources(self.ids)),
                "placements": len(self.placements),
                "skipped": list(st["skipped"]) + list(self.ledger),
                "colour_order": st["colors"]["order"],
            },
        }
