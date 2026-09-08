"""Colour assignment for a jobdeck view - the MDPView rule as measured.

Three modes:

  MODE_IDENTIFIER   flat view: one colour per `$` identifier, ascending.
  MODE_LAYER        chip view: colour by LY (a specific LY/DT can be pinned).
  MODE_CHIP         one colour per CHIP block in deck order; the selected
                    identifier k is spliced in after k-1 CHIPs.

MDPView rotates through TEN colours (JOBDECK_PALETTE - exactly the Tk
colour names blue, yellow, red, pink, orange, white, purple, cyan,
magenta, green) assigned by POSITION in an ordered target list; the
eleventh target gets the first colour again (measured 2026-09-07).
`color_order()` builds that list as strings ("$2", "CHIP:ID001", "LY123")
so it can be compared with the MDPView layer list by eye and stored in a
report. Chip-mode colours therefore depend on the selection; identifier
and layer colours do not. Several selected identifiers at once in chip
mode is an extension (MDPView expands one at a time): inferred, not
measured.

RESERVE_PALETTE (49 colours organised like Calibre's default layer
colours) is kept selectable ("palette": "reserve") because its use is
not known; it is not a guess at MDPView's colours.

Nothing here renders and nothing is KLayout-specific: the result is
`render key -> "#rrggbb"`; the renderer (M2+) turns it into layer styles.
"""

from __future__ import annotations

import json
from dataclasses import dataclass, field

# MDPView's two jobdeck views (manual terms, user 2026-09-09): the LEVEL
# view lists the mask levels - the `$n` entries, MTITLE n naming them -
# and the CHIP view lists the CHIP blocks. The `$n` number is the mask
# level number (Artwork's MEBES job deck syntax: `CHIP name,(1,PATTERN,
# AD=...)`, "1" = level), so what the port called "identifier" is the
# level; "identifier" stays accepted as an alias. The LY/DT view is an
# extra of ours (source layer/datatype), not a manual term.
MODE_LEVEL = "level"
MODE_IDENTIFIER = MODE_LEVEL
MODE_LAYER = "layer"
MODE_CHIP = "chip"
MODES = (MODE_LEVEL, MODE_LAYER, MODE_CHIP)
MODE_ALIASES = {"identifier": MODE_LEVEL, "id": MODE_LEVEL,
                "levels": MODE_LEVEL, "chips": MODE_CHIP,
                "layers": MODE_LAYER}


def normalize_mode(mode) -> str:
    """'identifier' -> 'level'; validates against MODES."""
    m = MODE_ALIASES.get(str(mode).lower(), str(mode).lower())
    if m not in MODES:
        raise ValueError("jobdeck view must be one of %s, got %r"
                         % (MODES, mode))
    return m

JOBDECK_PALETTE = [
    "#0000ff", "#ffff00", "#ff0000", "#ffc0cb", "#ffa500",
    "#ffffff", "#a020f0", "#00ffff", "#ff00ff", "#00ff00",
]
JOBDECK_NAMES = ["blue", "yellow", "red", "pink", "orange",
                 "white", "purple", "cyan", "magenta", "green"]

RESERVE_PALETTE = [
    "#ff0000", "#00ff00", "#2626ff", "#ffff00", "#ff00ff",
    "#00ffff", "#ff8000", "#80ff00", "#00ff80", "#0080ff",
    "#8000ff", "#ff0080", "#ff7373", "#73ff73", "#7373ff",
    "#ffff73", "#ff73ff", "#73ffff", "#ffb973", "#b9ff73",
    "#73ffb9", "#73b9ff", "#b973ff", "#ff73b9", "#b80000",
    "#00b800", "#4949b8", "#b8b800", "#b800b8", "#00b8b8",
    "#b85c00", "#5cb800", "#00b85c", "#005cb8", "#732eb8",
    "#b8005c", "#d94141", "#41d941", "#4141d9", "#d9d941",
    "#d941d9", "#41d9d9", "#d98d41", "#8dd941", "#41d98d",
    "#418dd9", "#8d41d9", "#d9418d", "#ffffff",
]

DEFAULT_PALETTE = JOBDECK_PALETTE
PALETTES = {"jobdeck": JOBDECK_PALETTE, "reserve": RESERVE_PALETTE}


def palette_color(palette, i: int) -> str:
    """Colour for position i; the palette rotates (wraps) like MDPView's."""
    if not palette:
        raise ValueError("empty palette")
    return palette[i % len(palette)]


def color_name(color: str) -> str:
    """'#0000ff' -> 'blue' for the ten jobdeck colours, else the hex."""
    c = str(color).lower()
    for hexval, name in zip(JOBDECK_PALETTE, JOBDECK_NAMES):
        if c == hexval:
            return name
    return color


def resolve_palette(spec) -> list[str]:
    if isinstance(spec, str):
        try:
            return list(PALETTES[spec.lower()])
        except KeyError:
            raise ValueError("unknown palette %r; use one of %s or a list "
                             "of colours" % (spec, sorted(PALETTES)))
    pal = [str(c) for c in spec]
    if not pal:
        raise ValueError("palette must not be empty")
    return pal


def entry_layer_pairs(entry, cross: bool = True) -> list:
    from .geom import entry_pairs
    return entry_pairs(entry, cross)


def key_identifier(idx) -> str:
    return "$%d" % int(idx)


def key_chip(chip_id) -> str:
    return "CHIP:%s" % chip_id


def key_layer(ly) -> str:
    return "LY%d" % int(ly)


def deck_chip_ids(deck) -> list[str]:
    """CHIP ids in deck order, deduplicated (a duplicate id is one target)."""
    out = []
    for c in deck.chips:
        if c.id not in out:
            out.append(c.id)
    return out


def chip_order(chip_ids, ids) -> list[str]:
    """CHIPs in the given order with each selected identifier k spliced in
    after k-1 CHIPs.

    >>> chip_order(["C1", "C2", "C3", "C4"], [2])
    ['CHIP:C1', '$2', 'CHIP:C2', 'CHIP:C3', 'CHIP:C4']
    >>> chip_order(["C1", "C2", "C3"], [1, 3])
    ['$1', 'CHIP:C1', 'CHIP:C2', '$3', 'CHIP:C3']

    An identifier larger than the CHIP count comes after the last CHIP.
    """
    out = []
    taken = 0
    for k in sorted({int(i) for i in ids}):
        want = min(max(k - 1, 0), len(chip_ids))
        while taken < want:
            out.append(key_chip(chip_ids[taken]))
            taken += 1
        out.append(key_identifier(k))
    out.extend(key_chip(c) for c in chip_ids[taken:])
    return out


def color_order(deck, mode: str, ids=None, cross: bool = True) -> list[str]:
    """The ordered colour targets for a mode: position i gets palette
    colour i mod the palette length."""
    if mode == MODE_IDENTIFIER:
        return [key_identifier(i) for i in deck.identifiers()]
    if mode == MODE_CHIP:
        sel = deck.identifiers() if ids is None else ids
        return chip_order(deck_chip_ids(deck), sel)
    if mode == MODE_LAYER:
        seen = set()
        for c in deck.chips:
            for e in c.entries:
                seen.update(ly for ly, _ in entry_layer_pairs(e, cross))
        return [key_layer(ly) for ly in sorted(seen)]
    raise ValueError("mode must be one of %s, got %r" % (MODES, mode))


def layer_pairs(deck, cross: bool = True) -> list:
    seen = set()
    for c in deck.chips:
        for e in c.entries:
            seen.update(entry_layer_pairs(e, cross))
    return sorted(seen)


@dataclass
class ColorScheme:
    mode: str = MODE_IDENTIFIER
    palette: list = field(default_factory=lambda: list(DEFAULT_PALETTE))
    # overrides keys: str(identifier) | str(ly) | "ly/dt" | chip id
    overrides: dict = field(default_factory=dict)
    cross_ly_dt: bool = True
    fallback: str = "#808080"

    def pin_layer(self, ly: int, color: str) -> None:
        self.overrides[str(ly)] = color

    def pin_layer_datatype(self, ly: int, dt: int, color: str) -> None:
        self.overrides["%d/%d" % (ly, dt)] = color

    def pin_identifier(self, idx: int, color: str) -> None:
        self.overrides[str(idx)] = color

    def pin_chip(self, chip_id: str, color: str) -> None:
        self.overrides[str(chip_id)] = color

    def order(self, deck, ids=None, mode=None) -> list[str]:
        return color_order(deck, mode or self.mode, ids, self.cross_ly_dt)

    def order_table(self, deck, ids=None, mode=None) -> list[dict]:
        """One row per position: pos, key, color, name, source (palette or
        pinned) - what a report stores and what is compared with the
        MDPView layer list."""
        mode = mode or self.mode
        rows = []
        for pos, key in enumerate(self.order(deck, ids, mode)):
            pinned = self._pinned(key)
            color = pinned or palette_color(self.palette, pos)
            rows.append({"pos": pos, "key": key, "color": color,
                         "name": color_name(color),
                         "source": "pinned" if pinned else "palette"})
        return rows

    def _pinned(self, key: str):
        if key.startswith("CHIP:"):
            return self.overrides.get(key[len("CHIP:"):])
        if key.startswith("$"):
            return self.overrides.get(key[1:])
        if key.startswith("LY"):
            return self.overrides.get(key[2:])
        return None

    def build(self, deck, ids=None) -> dict:
        """render key -> colour: {idx: c} / {chip_id: c} / {(ly, dt): c}."""
        order = self.order(deck, ids)
        pos = {key: i for i, key in enumerate(order)}

        def auto(key):
            return (palette_color(self.palette, pos[key]) if key in pos
                    else self.fallback)

        if self.mode == MODE_IDENTIFIER:
            return {i: self.overrides.get(str(i)) or auto(key_identifier(i))
                    for i in deck.identifiers()}
        if self.mode == MODE_CHIP:
            return {c: self.overrides.get(str(c)) or auto(key_chip(c))
                    for c in deck_chip_ids(deck)}
        out = {}
        for (ly, dt) in layer_pairs(deck, self.cross_ly_dt):
            out[(ly, dt)] = (self.overrides.get("%d/%d" % (ly, dt))
                             or self.overrides.get(str(ly))
                             or auto(key_layer(ly)))
        return out

    def color_of(self, colormap: dict, placement) -> str:
        """The colour one placement gets under this scheme."""
        if self.mode == MODE_IDENTIFIER:
            return colormap.get(placement.idx, self.fallback)
        if self.mode == MODE_CHIP:
            return colormap.get(placement.chip, self.fallback)
        return colormap.get((placement.ly, placement.dt), self.fallback)

    def save(self, path: str) -> None:
        with open(path, "w") as fh:
            json.dump({"mode": self.mode, "palette": self.palette,
                       "overrides": self.overrides,
                       "cross_ly_dt": self.cross_ly_dt,
                       "fallback": self.fallback}, fh, indent=2)

    @classmethod
    def load(cls, path: str) -> "ColorScheme":
        with open(path) as fh:
            d = json.load(fh)
        if "palette" in d:
            d["palette"] = resolve_palette(d["palette"])
        try:
            d["mode"] = normalize_mode(d.get("mode", MODE_LEVEL))
        except ValueError as exc:
            raise ValueError("colour file %s: %s" % (path, exc))
        return cls(**d)


def order_text(rows) -> str:
    """'CHIP:ID001 blue, $2 yellow, ...' from order_table rows."""
    return ", ".join("%s %s%s" % (r["key"], r["name"],
                                  " (pinned)" if r["source"] == "pinned"
                                  else "")
                     for r in rows)
