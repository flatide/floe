"""MEBES jobdeck (.jb) parser - the reverse-engineered subset.

Grammar assumed (confirmed by observation on real decks, not by a vendor
specification):

    SLICE 1,17
    RETICLE
    * <deckname>.jb
    OPTION PA, AA=0.0200, BA=0.002000, SA=80
    MTITLE 1,<name>
    *PLACE-INFO
    CHIP ID001, * MAIN 1.0000
    $ (1, NAME, AD=0.00020, SF=1, TC=file.oas, LY={1}, DT={0}, BX=0.0, BY=0.0, UX=2000.0, UY=2550.0)
    ROWS 85120.0/45020.0
    *END-PLACE
    END

Facts baked into this module (docs/JOBDECK.ko.md):
  * A CHIP line is `CHIP <id>` optionally followed by a comma and free
    text. The id is the text up to the first comma or blank with commas
    removed; the rest is kept verbatim in Chip.tail, never interpreted.
    CHIP ids are unique per deck (a duplicate is a warning).
  * A $ entry is bracketed ( ... ) and must close on its own line;
    continuation lines are rejected, not joined (the joining rule is
    unknown). Braces appear only inside LY={..} / DT={..}.
  * ROWS values are y/x - Y FIRST - and belong to the CHIP block, shared
    by every $ entry in it.
  * Entry fields (AD, SF, BX..UY) may differ between CHIP blocks for the
    same identifier: every entry is stored independently.
  * Placement: X = ((xn * sf) - cx) * ratio + jx, ratio = ad / source_dbu,
    cx = (bx + ux) / 2.
  * Unknown $ keys, header items and unparsed lines are preserved and
    reported, never guessed at.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field


def split_top_level(s: str, sep: str = ",") -> list[str]:
    """Split on `sep` but ignore separators inside {} () []."""
    out, depth, buf = [], 0, []
    for ch in s:
        if ch in "{([":
            depth += 1
        elif ch in "})]":
            depth -= 1
        if ch == sep and depth == 0:
            out.append("".join(buf))
            buf = []
        else:
            buf.append(ch)
    out.append("".join(buf))
    return [t.strip() for t in out if t.strip()]


def parse_braced_list(s: str) -> list[int]:
    """'{1,2}' -> [1, 2] ; '{0}' -> [0] ; '3' -> [3]"""
    s = s.strip().strip("{}").strip()
    if not s:
        return []
    return [int(float(t)) for t in split_top_level(s)]


def as_num(s: str) -> float:
    return float(s.strip())


@dataclass
class Entry:
    """One `$ (...)` line: a source layout reference inside a CHIP block."""
    idx: int                       # identifier, repeats across CHIP blocks
    name: str = ""
    ad: float | None = None        # jobdeck address unit (um)
    sf: float = 1.0                # scale factor
    tc: str = ""                   # source OASIS path (as written)
    ly: list[int] = field(default_factory=list)
    dt: list[int] = field(default_factory=list)
    bx: float = 0.0
    by: float = 0.0
    ux: float = 0.0
    uy: float = 0.0
    extra: dict = field(default_factory=dict)   # unknown keys, preserved
    lineno: int = -1

    @property
    def level(self) -> int:
        """The `$n` number is the MASK LEVEL (MDPView: level view)."""
        return self.idx

    @property
    def cx(self) -> float:
        return (self.bx + self.ux) / 2.0

    @property
    def cy(self) -> float:
        return (self.by + self.uy) / 2.0

    @property
    def width(self) -> float:
        return self.ux - self.bx

    @property
    def height(self) -> float:
        return self.uy - self.by

    def ratio(self, source_dbu: float) -> float:
        """source_precision * ad, where source_precision = 1 / source_dbu."""
        if self.ad is None:
            raise ValueError("entry %d: AD missing" % self.idx)
        return self.ad / source_dbu

    def placement(self, jy: float, jx: float, source_dbu: float):
        """(mag, dx, dy) for X = mag*xn + dx / Y = mag*yn + dy (um)."""
        r = self.ratio(source_dbu)
        return (self.sf * r, jx - self.cx * r, jy - self.cy * r)

    def signature(self) -> tuple:
        """Field set used to detect per-CHIP variation of one identifier."""
        return (self.name, self.ad, self.sf, self.tc,
                tuple(self.ly), tuple(self.dt),
                self.bx, self.by, self.ux, self.uy)


@dataclass
class Chip:
    """One `CHIP <id>` block."""
    id: str
    entries: list[Entry] = field(default_factory=list)
    rows: list[tuple[float, float]] = field(default_factory=list)  # (y, x)
    lineno: int = -1
    tail: str = ""          # whatever followed the id on the CHIP line

    def entry(self, idx: int) -> Entry | None:
        for e in self.entries:
            if e.idx == idx:
                return e
        return None


@dataclass
class JobDeck:
    header: dict = field(default_factory=dict)      # SLICE, RETICLE, ...
    option_flags: list[str] = field(default_factory=list)   # e.g. ['PA']
    options: dict = field(default_factory=dict)     # AA, BA, SA (unverified)
    mtitles: dict = field(default_factory=dict)     # {1: 'name', ...}
    comments: list[tuple[int, str]] = field(default_factory=list)
    chips: list[Chip] = field(default_factory=list)
    unknown: list[tuple[int, str]] = field(default_factory=list)
    errors: list[tuple[int, str]] = field(default_factory=list)    # structural
    warnings: list[tuple[int, str]] = field(default_factory=list)  # suspicious
    path: str = ""

    def extras(self) -> dict:
        """Unknown `$` keys grouped by key name - reported, never guessed."""
        out: dict = {}
        for c in self.chips:
            for e in c.entries:
                for k, v in e.extra.items():
                    out.setdefault(k, []).append(
                        {"chip": c.id, "idx": e.idx, "line": e.lineno,
                         "value": v})
        return out

    def report(self) -> dict:
        """Everything parsed but NOT interpreted (SLICE / RETICLE / OPTION
        drive nothing: placement uses only chip.rows and entry fields)."""
        return {
            "path": self.path,
            "jb_name": self.jb_name,
            "not_interpreted": {
                "note": "parsed and preserved; nothing reads these and "
                        "they do not affect placement",
                "header": dict(self.header),
                "option_flags": list(self.option_flags),
                "options": dict(self.options),
                "chip_line_tails": {c.id: c.tail for c in self.chips
                                    if c.tail},
            },
            "chip_lines": [
                {"chip": c.id, "line": c.lineno, "tail": c.tail,
                 "entries": len(c.entries), "rows": len(c.rows)}
                for c in self.chips
            ],
            "undocumented_entry_fields": self.extras(),
            "mtitles": {str(k): v for k, v in self.mtitles.items()},
            "title_check": self.title_check(),
            "coverage": self.coverage(),
            "identifiers_varying_across_chips":
                sorted(self.variation_report()),
            "errors": [{"line": n, "msg": m} for n, m in self.errors],
            "warnings": [{"line": n, "msg": m} for n, m in self.warnings],
            "unparsed_lines": [{"line": n, "text": t}
                               for n, t in self.unknown],
        }

    @property
    def jb_name(self) -> str:
        """First comment ending in .jb - observed to hold the deck name."""
        for _, txt in self.comments:
            if txt.lower().endswith(".jb"):
                return txt
        return ""

    def coverage(self) -> dict:
        """Which identifiers each CHIP actually carries (a block may omit
        identifiers that others have)."""
        ids = self.identifiers()
        rows = []
        for c in self.chips:
            present = sorted({e.idx for e in c.entries})
            rows.append({"chip": c.id, "present": present,
                         "missing": [i for i in ids if i not in present],
                         "rows": len(c.rows)})
        full = [r for r in rows if not r["missing"]]
        return {"identifiers": ids, "chips": rows,
                "complete": len(full), "partial": len(rows) - len(full)}

    def title(self, idx: int) -> str:
        """MTITLE entry for an identifier, '' if absent."""
        return self.mtitles.get(idx, "")

    def title_check(self) -> dict:
        ids = set(self.identifiers())
        titled = set(self.mtitles)
        return {"identifiers": sorted(ids),
                "mtitle_keys": sorted(titled),
                "missing_title": sorted(ids - titled),
                "unused_title": sorted(titled - ids)}

    def levels(self) -> list[int]:
        """Mask levels placed (the `$n` numbers) - MDPView's level view."""
        return self.identifiers()

    def identifiers(self) -> list[int]:
        seen = []
        for c in self.chips:
            for e in c.entries:
                if e.idx not in seen:
                    seen.append(e.idx)
        return sorted(seen)

    def sources(self, ids=None) -> list[str]:
        """Source paths (as written), in first-use order; `ids` keeps
        only the sources the given mask levels place (loading a deck
        with a level selection indexes and opens those alone)."""
        want = None if ids is None else {int(i) for i in ids}
        seen = []
        for c in self.chips:
            for e in c.entries:
                if want is not None and e.idx not in want:
                    continue
                if e.tc not in seen:
                    seen.append(e.tc)
        return seen

    def instances(self):
        """Yield (chip, entry, jy, jx) for every placed instance."""
        for c in self.chips:
            for (jy, jx) in c.rows:
                for e in c.entries:
                    yield c, e, jy, jx

    def variation_report(self) -> dict:
        """Identifiers whose $ definition is NOT identical across CHIPs."""
        acc: dict = {}
        for c in self.chips:
            for e in c.entries:
                acc.setdefault(e.idx, set()).add(e.signature())
        return {k: sorted(v) for k, v in acc.items() if len(v) > 1}


RE_ENTRY = re.compile(r"^\$\s*[({]\s*(?P<body>.*?)\s*[)}]?\s*$")
RE_ROWS = re.compile(r"^ROWS\b\s*(?P<body>.*)$", re.I)
RE_CHIP = re.compile(r"^CHIP\b\s*(?P<body>.*)$", re.I)
RE_OPTION = re.compile(r"^OPTION\b\s*(?P<body>.*)$", re.I)
RE_MTITLE = re.compile(r"^MTITLE\b\s*(?P<body>.*)$", re.I)
RE_ORPHAN_KV = re.compile(r"^[A-Za-z_]\w*\s*=")   # tail of a wrapped $ entry

DIRECTIVES = {"*PLACE-INFO", "*END-PLACE"}
_FLOAT_KEYS = {"AD", "SF", "BX", "BY", "UX", "UY"}
_LIST_KEYS = {"LY", "DT"}


def _parse_entry(body: str, lineno: int) -> Entry:
    toks = split_top_level(body)
    if not toks:
        raise ValueError("line %d: empty $ entry" % lineno)
    e = Entry(idx=int(float(toks[0])), lineno=lineno)
    rest = toks[1:]
    if rest and "=" not in rest[0]:
        e.name = rest[0].strip()
        rest = rest[1:]
    for t in rest:
        if "=" not in t:
            e.extra.setdefault("_positional", []).append(t)
            continue
        k, v = t.split("=", 1)
        k, v = k.strip().upper(), v.strip()
        if k in _FLOAT_KEYS:
            setattr(e, k.lower(), as_num(v))
        elif k in _LIST_KEYS:
            setattr(e, k.lower(), parse_braced_list(v))
        elif k == "TC":
            e.tc = v.strip("'\"")
        else:
            e.extra[k] = v
    return e


def _parse_chip(body: str) -> tuple[str, str]:
    """'ID001, * MAIN 1.0000' -> ('ID001', '* MAIN 1.0000')"""
    body = body.strip()
    m = re.match(r"^([^,\s]*)\s*,?\s*(.*)$", body, re.S)
    return m.group(1).replace(",", "").strip(), m.group(2).strip()


def _parse_rows(body: str, lineno: int) -> list[tuple[float, float]]:
    """'85120.0/45020.0' (possibly several per line) -> [(y, x), ...]"""
    out = []
    for tok in re.split(r"[,\s]+", body.strip()):
        if not tok:
            continue
        if "/" not in tok:
            raise ValueError("line %d: bad ROWS token %r" % (lineno, tok))
        y, x = tok.split("/", 1)
        out.append((as_num(y), as_num(x)))       # Y FIRST
    return out


def parse_jobdeck(path: str, strict: bool = True) -> JobDeck:
    """Parse a .jb file. strict=True raises on structural errors rather
    than returning a deck that would place geometry wrongly."""
    deck = JobDeck(path=path)
    chip: Chip | None = None
    with open(path, "r", errors="replace") as fh:
        for lineno, raw in enumerate(fh, 1):
            line = raw.strip()
            if not line:
                continue
            up = line.upper()
            if line.startswith("*"):
                if up.split()[0] in DIRECTIVES:
                    continue
                txt = line.lstrip("*").strip()
                if txt:
                    deck.comments.append((lineno, txt))
                continue
            if up == "END":
                break
            m = RE_OPTION.match(line)
            if m:
                for t in split_top_level(m.group("body")):
                    if "=" in t:
                        k, v = t.split("=", 1)
                        k, v = k.strip().upper(), v.strip()
                        try:
                            deck.options[k] = as_num(v)
                        except ValueError:
                            deck.options[k] = v
                    else:
                        deck.option_flags.append(t.strip().upper())
                continue
            m = RE_MTITLE.match(line)
            if m:
                toks = split_top_level(m.group("body"))
                if toks:
                    try:
                        n = int(float(toks[0]))
                    except ValueError:
                        n = len(deck.mtitles) + 1
                        toks = [""] + toks
                    deck.mtitles[n] = toks[1].strip() if len(toks) > 1 else ""
                continue
            m = RE_CHIP.match(line)
            if m:
                cid, tail = _parse_chip(m.group("body"))
                if not cid:
                    deck.errors.append(
                        (lineno, "CHIP line without an id: %s" % line))
                chip = Chip(id=cid, lineno=lineno, tail=tail)
                deck.chips.append(chip)
                continue
            if line.startswith("$"):
                m = RE_ENTRY.match(line)
                if not m or chip is None:
                    deck.unknown.append((lineno, line))
                    continue
                if not line.rstrip().endswith(("}", ")")):
                    # a silently truncated entry keeps BX..UY at 0 and
                    # places the chip wrongly: reject loudly
                    deck.errors.append(
                        (lineno, "unterminated $ entry (no closing ')'): %s"
                         % line))
                    continue
                chip.entries.append(_parse_entry(m.group("body"), lineno))
                continue
            m = RE_ROWS.match(line)
            if m:
                if chip is None:
                    deck.unknown.append((lineno, line))
                    continue
                chip.rows.extend(_parse_rows(m.group("body"), lineno))
                continue
            if chip is None:
                parts = line.split(None, 1)
                deck.header[parts[0].upper()] = (
                    parts[1].strip() if len(parts) == 2 else "")
                continue
            if RE_ORPHAN_KV.match(line):
                deck.errors.append(
                    (lineno, "orphan 'KEY=VALUE' line inside CHIP %s "
                             "(wrapped $ entry?): %s" % (chip.id, line)))
                continue
            deck.unknown.append((lineno, line))
    _post_checks(deck)
    if strict and deck.errors:
        detail = "\n".join("  line %d: %s" % (ln, msg)
                           for ln, msg in deck.errors)
        raise ValueError(
            "%s: %d structural error(s); refusing to return a deck that "
            "would place geometry wrongly.\n%s" % (path, len(deck.errors),
                                                    detail))
    return deck


def _post_checks(deck: JobDeck) -> None:
    seen_ids: dict = {}
    for c in deck.chips:
        if c.id in seen_ids:
            deck.warnings.append(
                (c.lineno, "duplicate CHIP id %r (first at line %d); "
                           "chip-mode colours and per-chip listings merge "
                           "them" % (c.id, seen_ids[c.id])))
        else:
            seen_ids[c.id] = c.lineno
    for c in deck.chips:
        for e in c.entries:
            if e.ad is None:
                deck.warnings.append(
                    (e.lineno, "chip %s $%d: AD missing; placement will "
                               "raise" % (c.id, e.idx)))
            if e.width <= 0 or e.height <= 0:
                deck.warnings.append(
                    (e.lineno, "chip %s $%d: degenerate bbox %gx%g um "
                               "(BX..UY missing?)"
                               % (c.id, e.idx, e.width, e.height)))
            if not e.ly:
                deck.warnings.append(
                    (e.lineno, "chip %s $%d: no LY given" % (c.id, e.idx)))
        if not c.rows:
            deck.warnings.append(
                (c.lineno, "chip %s: no ROWS; nothing will be placed"
                 % c.id))
