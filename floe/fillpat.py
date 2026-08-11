"""Built-in Calibre fill pattern set (user-specified names,
2026-08-11).

20 named fills in the user's order; the last two (Solid, Clear)
are fixed all-set / all-clear bitmaps. The other 18 start as
procedural 16x16 approximations of their names and are meant to
be refined in the viewer's bitmap editor popup (right-click a
pattern slot) - each generator is a small (x, y) predicate, so a
one-line edit realigns any fill with Calibre's exact bitmap.

Preview all patterns:  python -m floe.fillpat
"""

SIZE = 16


def _gen(fn):
    return "\n".join(
        "".join("*" if fn(x, y) else "." for x in range(SIZE))
        for y in range(SIZE))


def _diag_ne(p):        # "/" diagonals
    return lambda x, y: (x + y) % p == 0


def _diag_nw(p):        # "\" diagonals
    return lambda x, y: (x - y) % p == 0


def _slope_ne(p, w):    # thick "/"
    return lambda x, y: (x + y) % p < w


def _slope_nw(p, w):    # thick "\"
    return lambda x, y: (x - y) % p < w


def _carets(x, y):      # ^^^^ rows
    return (y % 8) == abs((x % 8) - 4)


def _light_speckle(x, y):   # 12.5% double-phase dots
    return (x % 4, y % 4) in ((0, 0), (2, 2))


def _speckle(x, y):     # the 50% checker the viewer already uses
    return (x + y) & 1 == 0


def _alt_light_speckle(x, y):   # 25% dot lattice
    return x % 2 == 0 and y % 2 == 0


def _alt_speckle(x, y):     # 50% checker, opposite phase
    return (x + y) & 1 == 1


def _triangle_small(x, y):  # small solid pyramids, 8x8 tiles
    return y % 8 < 4 and abs((x % 8) - 4) <= (y % 8)


_WAVE8 = (2, 1, 0, 0, 1, 2, 3, 3)


def _wave_small(x, y):      # short zigzag, 4x4
    return (y % 4) == (0 if (x // 2) % 2 == 0 else 1)


def _wave(x, y):            # long wave, 8x8
    return (y % 8) == _WAVE8[x % 8]


def _plus(x, y):            # + marks, 8x8 tiles
    return ((x % 8 == 4 and 2 <= y % 8 <= 6)
            or (y % 8 == 4 and 2 <= x % 8 <= 6))


def _brick(x, y):           # 16x8 bricks, half-shifted courses
    return y % 8 == 0 or (x + (y // 8) * 8) % 16 == 0


def _circles(x, y):         # small rings, 8x8 tiles
    dx, dy = (x % 8) - 4, (y % 8) - 4
    return 4 <= dx * dx + dy * dy <= 8


def _carpet_1(x, y):        # 2px woven checker
    return ((x % 4) // 2 + (y % 4) // 2) % 2 == 0


# the user's Calibre order, row-major over the 5x4 slot grid
_SPECS = (
    ("diagonal_right_wide", _diag_ne(8)),
    ("diagonal_1", _diag_ne(4)),
    ("diagonal_left_wide", _diag_nw(8)),
    ("diagonal_2", _diag_nw(4)),
    ("carets", _carets),
    ("light_speckle", _light_speckle),
    ("speckle", _speckle),
    ("alt_light_speckle", _alt_light_speckle),
    ("alt_speckle", _alt_speckle),
    ("triangle_small", _triangle_small),
    ("wave_small", _wave_small),
    ("wave", _wave),
    ("right_slope", _slope_ne(8, 2)),
    ("left_slope", _slope_nw(8, 2)),
    ("plus", _plus),
    ("brick", _brick),
    ("circles", _circles),
    ("carpet_1", _carpet_1),
    ("Solid", lambda x, y: True),
    ("Clear", lambda x, y: False),
)

# ordered (name, 16x16 rows); Solid/Clear are FIXED (no editing)
FILL_PATTERNS = tuple((name, _gen(fn)) for name, fn in _SPECS)
FILL_NAMES = tuple(n for n, _ in FILL_PATTERNS)
FIXED_FILLS = ("Solid", "Clear")


def default_patterns():
    """Editable copy of the default bitmaps, grid order."""
    return [p for _, p in FILL_PATTERNS]


# the Calibre 7x7 color table: loaded from the packaged
# colornames.def (single source for the palette grid order, the
# layerprops color names and the hex values)
def _load_color_table():
    import os as _os
    import sys as _sys
    path = _os.path.join(_os.path.dirname(_os.path.abspath(
        __file__)), "colornames.def")
    rows = []
    try:
        with open(path) as f:
            for ln in f:
                ln = ln.strip()
                if not ln or ln.startswith("#"):
                    continue
                parts = ln.split()
                if len(parts) < 2:
                    continue
                rows.append((parts[0],
                             "#" + parts[1].lstrip("#").lower()))
    except OSError as e:
        print("floe: colornames.def unreadable (%s) - empty "
              "color table" % e, file=_sys.stderr)
    return tuple(rows)


COLOR_TABLE = _load_color_table()
_COLOR_HEX = {n: h for n, h in COLOR_TABLE}
# first name wins for a shared value (yellow over yellow1 etc.)
_COLOR_NAME = {}
for _n, _h in COLOR_TABLE:
    _COLOR_NAME.setdefault(_h, _n)
_FILL_INDEX = {}


def color_hex(name):
    """layerprops color field -> '#rrggbb' (names from the 7x7
    table, or a literal #hex); None if unknown."""
    if name.startswith("#"):
        return name.lower()
    return _COLOR_HEX.get(name.lower())


def color_name(hexval):
    """'#rrggbb' -> table name, or the #hex itself if unnamed."""
    return _COLOR_NAME.get(hexval.lower(), hexval.lower())


def fill_index(name):
    """fill name (case-insensitive) -> slot index, or None."""
    if not _FILL_INDEX:
        for i, n in enumerate(FILL_NAMES):
            _FILL_INDEX[n.lower()] = i
    return _FILL_INDEX.get(name.lower())


def parse_layerprops(text):
    """Calibre .layerprops lines ->
    [((layer, dt), color, fill, name, f1, f2)]. '2' means 2/0,
    '7.20' means 7/20; comments and malformed lines skipped."""
    rows = []
    for ln in text.splitlines():
        ln = ln.strip()
        if not ln or ln.startswith("#"):
            continue
        parts = ln.split()
        if len(parts) < 3:
            continue
        ld = parts[0].split(".")
        try:
            key = (int(ld[0]),
                   int(ld[1]) if len(ld) > 1 else 0)
        except ValueError:
            continue
        rows.append((key, parts[1], parts[2],
                     parts[3] if len(parts) > 3 else "",
                     parts[4] if len(parts) > 4 else "1",
                     parts[5] if len(parts) > 5 else "1"))
    return rows


def format_layerprops(rows):
    """[( (l, d), color, fill, name, f1, f2 )] -> file text."""
    out = ["# Generated by floe."]
    for (l, d), color, fill, name, f1, f2 in rows:
        ld = "%d" % l if d == 0 else "%d.%d" % (l, d)
        out.append("%s %s %s %s %s %s"
                   % (ld, color, fill,
                      (name or "%d_%d" % (l, d)).replace(" ", "_"),
                      f1, f2))
    return "\n".join(out) + "\n"


def rows_to_hex(rows):
    """'*'/'.' rows -> 16 four-digit hex words ("FFFF 01B4 ..."),
    MSB = leftmost pixel (the on-disk form, user call 2026-08-11)."""
    words = []
    for line in rows.split("\n"):
        v = 0
        for x, c in enumerate(line[:SIZE]):
            if c == "*":
                v |= 1 << (SIZE - 1 - x)
        words.append("%04X" % v)
    return " ".join(words)


def hex_to_rows(hx):
    """16 hex words -> klayout '*'/'.' rows."""
    rows = []
    for w in hx.split():
        v = int(w, 16)
        rows.append("".join(
            "*" if v & (1 << (SIZE - 1 - x)) else "."
            for x in range(SIZE)))
    return "\n".join(rows)


def pattern(name):
    for n, p in FILL_PATTERNS:
        if n == name:
            return p
    raise KeyError(name)


def _main():
    for name, pat in FILL_PATTERNS:
        print("--", name)
        print(pat)


if __name__ == "__main__":
    _main()
