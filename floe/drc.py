"""Calibre DRC ASCII results database (.db) parser.

The format Calibre writes with `DRC RESULTS DATABASE "out.db" ASCII`:

    <top-cell> <precision>
    <rulecheck name>
    <n-results> <n-original> <n-desc-lines> <timestamp ...>
    <rule text line 1..n-desc-lines>
    p <ordinal> <n-vertices>
    <x> <y>                     (integers; um = value / precision)
    ...
    e <ordinal> <n-edges>
    <x1> <y1> <x2> <y2>         (edge records: one whole edge per line)
    <next rulecheck name>
    ...

DRC results contain only 'p' (polygon) and 'e' (edge) primitives:
'p' carries one vertex (2 ints) per line, 'e' one edge (4 ints) per
line — klayout's Calibre reader confirms this split. The parser is
deliberately tolerant of real-world variation: blank lines and CRLF
are ignored, declared counts are advisory, unknown record letters
are skipped with their block, and a truncated file yields whatever
parsed cleanly instead of raising.
"""

import os


class DrcError(object):
    """One violation: kind 'p' (polygon) or 'e' (edge), pts in um."""
    __slots__ = ("kind", "num", "pts")

    def __init__(self, kind, num, pts):
        self.kind = kind
        self.num = num          # ordinal within its check (1-based)
        self.pts = pts          # [(x_um, y_um), ...]

    def bbox(self):
        xs = [p[0] for p in self.pts]
        ys = [p[1] for p in self.pts]
        return (min(xs), min(ys), max(xs), max(ys))

    def center(self):
        b = self.bbox()
        return ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0)


class DrcCheck(object):
    """One rule check: name, rule text, and its violations."""
    __slots__ = ("name", "desc", "declared", "errors")

    def __init__(self, name, desc="", declared=0):
        self.name = name
        self.desc = desc        # rule text ('' if none)
        self.declared = declared
        self.errors = []


class DrcDb(object):
    __slots__ = ("path", "cell", "precision", "checks")

    def __init__(self, path, cell, precision, checks):
        self.path = path
        self.cell = cell
        self.precision = precision
        self.checks = checks

    @property
    def total(self):
        return sum(len(c.errors) for c in self.checks)


def _ints_prefix(tokens):
    """Leading integers of a token list (check header: ints, then a
    textual timestamp)."""
    out = []
    for t in tokens:
        try:
            out.append(int(t))
        except ValueError:
            break
    return out


def _is_geom_header(tokens):
    return (len(tokens) >= 3 and len(tokens[0]) == 1
            and tokens[0].isalpha()
            and tokens[1].lstrip("-").isdigit()
            and tokens[2].lstrip("-").isdigit())


def load_db(path):
    """Parse a Calibre ASCII DRC results database -> DrcDb."""
    with open(path, "r", errors="replace") as f:
        lines = [ln.rstrip("\r\n") for ln in f]
    # skip blank leading lines; header = "<cell> <precision>"
    i = 0
    while i < len(lines) and not lines[i].strip():
        i += 1
    if i >= len(lines):
        raise ValueError("%s: empty file" % path)
    head = lines[i].split()
    i += 1
    cell = head[0] if head else os.path.basename(path)
    try:
        precision = float(head[1])
    except (IndexError, ValueError):
        precision = 1000.0
    if precision <= 0:
        precision = 1000.0

    checks = []
    n = len(lines)
    while i < n:
        name = lines[i].strip()
        i += 1
        if not name:
            continue
        check = DrcCheck(name)
        # header: "<results> <original> <desc-lines> <date...>"
        if i < n:
            ints = _ints_prefix(lines[i].split())
            if ints:
                i += 1
                check.declared = ints[0]
                dlines = ints[2] if len(ints) >= 3 else 0
                desc = []
                for _ in range(dlines):
                    if i < n and not _is_geom_header(lines[i].split()):
                        desc.append(lines[i].strip())
                        i += 1
                check.desc = "\n".join(desc)
        # geometry records until the next check name
        while i < n:
            tok = lines[i].split()
            if not tok:
                i += 1
                continue
            if not _is_geom_header(tok):
                break  # next rulecheck name
            kind, num, nv = tok[0].lower(), int(tok[1]), int(tok[2])
            i += 1
            # nv coordinate LINES follow; every integer pair on a line
            # is one point ('p': 2 ints = 1 vertex, 'e': 4 ints = the
            # edge's two endpoints)
            pts = []
            got = 0
            while got < nv and i < n:
                ct = lines[i].split()
                if not ct:
                    i += 1
                    continue  # stray blank inside a record
                try:
                    nums = [float(t) for t in ct]
                except ValueError:
                    break  # next check name: record truncated here
                if len(nums) < 2:
                    break
                i += 1
                got += 1
                for j in range(0, len(nums) - 1, 2):
                    pts.append((nums[j] / precision,
                                nums[j + 1] / precision))
            if pts and kind in ("p", "e"):
                check.errors.append(DrcError(kind, num, pts))
            # unknown kinds: coordinates consumed, record dropped
        checks.append(check)
    return DrcDb(path, cell, precision, checks)
