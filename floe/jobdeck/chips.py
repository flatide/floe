"""Chip on/off for captures (user 2026-10-06: "a capture names a level
and a chip"; "several chips - chip on/off").

A chip is a row of the chip view: one source under one mask level
(docs/JOBDECK.ko.md §1a), the unit the viewer turns on and off - its
key is the view's L/D pair (level, source ordinal). A token names chips
by

  - the name the chip view lists (the source's file name),
  - the source's TC path,
  - a CHIP id (the chips that CHIP block places),

with `N:` in front for level N alone and shell wildcards (`*`, `?`,
`[...]`) in the name. A CHIP block that shares a source with another
in the same level turns their common row on or off as a whole - a row
is what the view draws or not.

The region of a capture is the extent of the placements named: the
chips on, or `--fit-chip`'s, which need not be on and whose `#K` picks
the K-th placement alone - counted in deck order (the CHIP blocks as
the deck lists them, their ROWS in turn); a CHIP id frames that block's
placements alone. Extents are the deck's own (the entries' BX..UY
placed), so nothing is read to know them.

Free of the renderer and of KLayout, like the rest of the package.
"""

from __future__ import annotations

import fnmatch
import os
import re
from dataclasses import dataclass, field

_TOKEN = re.compile(r"^(?:(?P<level>\d+):)?(?P<name>.*?)(?:#(?P<k>\d+))?$")
_WILD = "*?["
_SHOW = 12          # names an error message lists before "+N more"


def tokens(text) -> list[str]:
    """'a, b,,c' -> ['a', 'b', 'c']."""
    return [t.strip() for t in str(text or "").split(",") if t.strip()]


def um(v) -> str:
    """A length as the command line takes it: 45020, 44020.5."""
    v = float(v)
    if abs(v) < 5e-5:
        v = 0.0
    return ("%.4f" % v).rstrip("0").rstrip(".")


def box_text(b) -> str:
    """X0,Y0,X1,Y1 - what --bbox and --corners take."""
    return ",".join(um(v) for v in b)


def union(boxes):
    boxes = [b for b in boxes if b is not None]
    if not boxes:
        return None
    return (min(b[0] for b in boxes), min(b[1] for b in boxes),
            max(b[2] for b in boxes), max(b[3] for b in boxes))


@dataclass
class Instance:
    """One placement of a chip: a CHIP block at one of its ROWS."""
    n: int              # 1-based, deck order
    chip: str           # the CHIP id
    row: int            # the ROWS index in that block (0-based)
    at: tuple           # the ROWS position (x, y) um
    bbox: tuple         # um


@dataclass
class ChipRow:
    level: int
    key: tuple          # (level, ordinal): the view's L/D key
    name: str           # what the chip view lists: the source's file name
    tc: str             # the source path, normalised
    chips: list = field(default_factory=list)   # CHIP ids, deck order
    places: list = field(default_factory=list)  # the load's Placements

    @property
    def label(self) -> str:
        return "%d:%s" % (self.level, self.name)


@dataclass
class Selection:
    """What one capture's chip options come to."""
    rows: list | None   # the chips on (None: the options name none)
    keys: list | None   # their L/D keys, for the render's visible set
    region: tuple | None  # um: the placements' extent the options frame
    placements: int     # how many placements that extent covers
    frame: str          # what the region is ("chips on", "--fit-chip X")


class ChipTable:
    """The chip view's rows of a loaded deck (DeckCache.chips())."""

    def __init__(self, deck, placements, layers, levels=None):
        """`layers`: the view's meta rows (the level heads and their
        chips); `placements`: the load's; `levels`: the levels loaded
        (None = all)."""
        self.deck = deck
        self.deck_levels = list(deck.levels())
        self.loaded = None if levels is None else sorted(int(i)
                                                         for i in levels)
        self.titles = {}
        self.rows = []
        by_src = {}
        for l in layers:
            if l.get("jobdeck_head"):
                self.titles[int(l["layer"])] = l["name"]
                continue
            if not l.get("jobdeck_source"):
                continue        # the source layer view has no chips
            row = ChipRow(level=int(l["layer"]),
                          key=(int(l["layer"]), int(l["datatype"])),
                          name=l["name"],
                          tc=os.path.normpath(l["jobdeck_source"]))
            self.rows.append(row)
            by_src[(row.level, row.tc)] = row
        self._by_src = by_src
        self._chip_pos = {}
        for c in deck.chips:
            self._chip_pos.setdefault(c.id, len(self._chip_pos))
            for e in c.entries:
                row = by_src.get((e.idx, os.path.normpath(e.tc)))
                if row is not None and c.id not in row.chips:
                    row.chips.append(c.id)
        for p in placements:
            row = by_src.get((p.idx, os.path.normpath(p.tc)))
            if row is not None:
                row.places.append(p)

    # ---- names -------------------------------------------------------
    @property
    def levels(self) -> list[int]:
        """The levels whose chips this load holds."""
        return sorted({r.level for r in self.rows})

    def row_of(self, idx, tc):
        return self._by_src.get((int(idx), os.path.normpath(tc)))

    def _known(self, level=None) -> str:
        rows = [r for r in self.rows if level is None or r.level == level]
        names = list(dict.fromkeys(r.name for r in rows))
        ids = list(dict.fromkeys(c for r in rows for c in r.chips))

        def cap(items):
            text = ", ".join(items[:_SHOW])
            if len(items) > _SHOW:
                text += ", +%d more" % (len(items) - _SHOW)
            return text or "none"
        where = ("level %d" % level if level is not None else
                 "the loaded levels (%s)" % ",".join(map(str, self.levels)))
        return "chips of %s: %s; CHIP ids: %s (`floe2 info DECK --chips` " \
               "lists them)" % (where, cap(names), cap(ids))

    def match(self, token, allow_k=False):
        """(picks, k): the rows a token names, each with the CHIP ids
        that named it (None: named by its source), and its #K."""
        m = _TOKEN.match(token.strip())
        level = int(m.group("level")) if m.group("level") else None
        name = m.group("name").strip()
        k = int(m.group("k")) if m.group("k") else None
        if not name:
            raise ValueError("chip %r names nothing" % token)
        if k is not None and not allow_k:
            raise ValueError(
                "chip %r: #K picks one placement for the region "
                "(--fit-chip); a chip is on or off as a whole" % token)
        if level is not None and level not in self.levels:
            if level in self.deck_levels:
                raise ValueError("chip %r: level %d is not loaded (--level "
                                 "%s)" % (token, level,
                                          ",".join(map(str, self.levels))))
            raise ValueError("chip %r: level %d is not in the deck (it "
                             "places %s)" % (token, level, ",".join(
                                 map(str, self.deck_levels))))
        wild = any(ch in name for ch in _WILD)
        path = os.path.normpath(name)

        def hit(text):
            return fnmatch.fnmatchcase(text, name) if wild else text == name
        picks = []
        for row in self.rows:
            if level is not None and row.level != level:
                continue
            if hit(row.name) or (fnmatch.fnmatchcase(row.tc, name) if wild
                                 else row.tc == path):
                picks.append((row, None))
                continue
            ids = tuple(c for c in row.chips if hit(c))
            if ids:
                picks.append((row, ids))
        if not picks:
            raise ValueError("chip %r matches no chip - %s"
                             % (token, self._known(level)))
        return picks, k

    def rows_of(self, text) -> list:
        want = set()
        for tok in tokens(text):
            want.update(row.key for row, _ in self.match(tok)[0])
        return [r for r in self.rows if r.key in want]

    # ---- placements and regions -------------------------------------
    def instances(self, picks) -> list:
        """The placements the picks name, one per CHIP block and ROWS
        position, in deck order (a position whose rows several picks
        name is one placement, their extents together)."""
        acc = {}
        for row, ids in picks:
            for p in row.places:
                if ids is not None and p.chip not in ids:
                    continue
                key = (p.chip, p.row)
                at = (p.jx, p.jy)
                prev = acc.get(key)
                acc[key] = (at, p.bbox_um if prev is None
                            else union([prev[1], p.bbox_um]))
        order = sorted(acc, key=lambda k: (self._chip_pos.get(k[0], 0), k[1]))
        return [Instance(n=n, chip=chip, row=r, at=acc[(chip, r)][0],
                         bbox=acc[(chip, r)][1])
                for n, (chip, r) in enumerate(order, 1)]

    def fit(self, text):
        """(region, placements) of `--fit-chip`: every token's
        placements, or its #K alone."""
        boxes = []
        for tok in tokens(text):
            picks, k = self.match(tok, allow_k=True)
            insts = self.instances(picks)
            if not insts:
                raise ValueError(
                    "chip %r has nothing placed (its source is skipped: "
                    "see 'skipped' above)" % tok)
            if k is not None:
                if not 1 <= k <= len(insts):
                    raise ValueError(
                        "chip %r: it has %d placement%s (#1..#%d; `floe2 "
                        "info DECK --chips` lists them)" % (
                            tok, len(insts), "" if len(insts) == 1 else "s",
                            len(insts)))
                insts = [insts[k - 1]]
            boxes.extend(i.bbox for i in insts)
        if not boxes:
            raise ValueError("--fit-chip names nothing")
        return union(boxes), len(boxes)

    def select(self, on=None, off=None, fit=None) -> Selection:
        """A capture's chips: `on`'s (every chip loaded when only `off`
        is given) less `off`'s, and the region - `fit`'s placements,
        else those of the chips on (None: the options name no chips)."""
        rows = None
        if on or off:
            rows = self.rows_of(on) if on else list(self.rows)
            if off:
                drop = {r.key for r in self.rows_of(off)}
                rows = [r for r in rows if r.key not in drop]
            if not rows:
                raise ValueError("no chip is left on (--chip %s, --chip-off "
                                 "%s)" % (on or "-", off or "-"))
        region, count, frame = None, 0, ""
        if fit:
            region, count = self.fit(fit)
            frame = "--fit-chip %s" % fit
        elif rows is not None:
            insts = self.instances([(r, None) for r in rows])
            region = union(i.bbox for i in insts)
            count = len(insts)
            frame = "the chips on"
        return Selection(rows=rows,
                         keys=None if rows is None else [r.key for r in rows],
                         region=region, placements=count, frame=frame)

    def skipped_in(self, records, keys):
        """The ledger records a capture drawing `keys` lacks: those of
        its chips (every record when keys is None - every chip on; a
        record no row names counts, to be safe)."""
        if keys is None:
            return list(records)
        on = set(keys)
        out = []
        for rec in records:
            row = self.row_of(rec["idx"], rec["tc"])
            if row is None or row.key in on:
                out.append(rec)
        return out

    def describe(self, sel: Selection) -> str:
        """One log line: the chips on and the region."""
        parts = []
        if sel.rows is not None:
            names = [r.label for r in sel.rows]
            shown = ", ".join(names[:8])
            if len(names) > 8:
                shown += ", +%d more" % (len(names) - 8)
            parts.append("chips on %d of %d (%s)" % (
                len(sel.rows), len(self.rows), shown))
        if sel.region is not None:
            parts.append("region %s um (%s, %d placement%s)" % (
                box_text(sel.region), sel.frame, sel.placements,
                "" if sel.placements == 1 else "s"))
        return "; ".join(parts)

    # ---- listing -----------------------------------------------------
    def listing(self) -> list[str]:
        """`floe2 info deck.jb --chips`: every chip of the load by
        level, its CHIP blocks, its placements and their extents in the
        form --bbox and --corners take."""
        out = []
        for level in self.levels:
            out.append("$%d %s" % (level, self.titles.get(level, "")))
            for row in (r for r in self.rows if r.level == level):
                insts = self.instances([(row, None)])
                ext = union(i.bbox for i in insts)
                out.append("  %d/%d %s  CHIP %s  %d placement%s%s" % (
                    row.key[0], row.key[1], row.name,
                    ",".join(row.chips) or "-", len(insts),
                    "" if len(insts) == 1 else "s",
                    "  %s um" % box_text(ext) if ext else
                    "  (source skipped)"))
                if row.tc != row.name:
                    out.append("      source %s" % row.tc)
                if len(insts) > 1:
                    for i in insts:
                        out.append("      #%d CHIP %s ROWS %d at %s,%s: "
                                   "%s um" % (i.n, i.chip, i.row + 1,
                                              um(i.at[0]), um(i.at[1]),
                                              box_text(i.bbox)))
        return out
