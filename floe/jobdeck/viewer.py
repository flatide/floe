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


def deck_ready(path, sources_dir=None) -> bool:
    """True when every source the deck names probes ok and has a fresh
    <src>.floe cache - the deck's equivalent of `<src>.floe` existing."""
    try:
        deck = parse_jobdeck(path, strict=True)
    except (OSError, ValueError):
        return False
    catalog = SourceCatalog(deck_sources_dir(path, sources_dir))
    catalog.probe_all(deck.sources())
    infos = catalog.infos.values()
    return bool(infos) and all(i.ok() and i.indexed for i in infos)


class DeckCache:
    """Cache-shaped view of a jobdeck (see the module docstring)."""

    is_jobdeck = True

    def __init__(self, path, mode: str = MODE_LEVEL, sources_dir=None,
                 ids=None):
        self.src = os.path.abspath(path)
        self.mode = normalize_mode(mode)
        self.ids = ids
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

    # ---- Cache protocol ---------------------------------------------
    def exists(self) -> bool:
        return os.path.isfile(self.src)

    def unindexed(self):
        deck = parse_jobdeck(self.src, strict=True)
        catalog = SourceCatalog(self.sources_dir)
        catalog.probe_all(deck.sources())
        return [tc for tc, i in sorted(catalog.infos.items())
                if not (i.ok() and i.indexed)]

    def load(self):
        if self.work is None:
            self.work = tempfile.mkdtemp(prefix="floe-jobdeck-")
        (self.deck, self.catalog, self.placements, self.stats,
         self.scheme, self.colormap) = plan_deck(
            self.src, sources_dir=self.sources_dir, ids=self.ids,
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
        space (level view n/0, chip view pos/level, source layer view
        LY/DT), so each view keeps its own <key>.layerprops: the level
        view's is the deck's (<deck>.jb.layerprops), the others are
        <deck>.chip.jb / <deck>.layer.jb - a name whose <stem>.layerprops
        fallback cannot land on another view's file."""
        if self.mode == MODE_LEVEL:
            return self.src
        stem, ext = os.path.splitext(self.src)
        return "%s.%s%s" % (stem, self.mode, ext)

    def resolve_layers(self, spec):
        """'$1 METAL1,CHIP ID001,2/0' -> [(layer, datatype), ...]; None
        = all. Like Cache.resolve_layers a name selects EVERY row that
        carries it (chip view: "$1 METAL1" in every CHIP that places
        it). Only the chip view's virtual CHIP rows are group heads and
        expand to the level rows under them (review 2026-09-09 P2-2:
        the head alone holds no placement and drew a black screen; 3rd
        pass P2-2: a source-layer DT0 row is a real layer, never a
        head)."""
        if not spec or spec == "all":
            return None
        rows = self.meta["layers"]
        byname = {}
        for l in rows:
            byname.setdefault(l["name"], []).append((l["layer"],
                                                     l["datatype"]))
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
    def set_mode(self, mode: str):
        """Switch between MDPView's level view and chip view (and our
        source layer view); re-plans and rewrites the spec."""
        self.mode = normalize_mode(mode)
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
                "sources": len(self.deck.sources()),
                "placements": len(self.placements),
                "skipped": list(st["skipped"]) + list(self.ledger),
                "colour_order": st["colors"]["order"],
            },
        }
