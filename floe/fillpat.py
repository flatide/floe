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


# the Calibre 7x7 color table (user-specified names/values,
# grid order) - layerprops rows reference colors by these names
COLOR_TABLE = (
    ("darkorange", "#ff8c00"), ("tomato", "#ff6347"),
    ("red", "#ff0000"), ("violetred", "#d02090"),
    ("firebrick", "#b22222"), ("brown", "#a52a2a"),
    ("red4", "#8b0000"),
    ("yellow", "#ffff00"), ("yellow1", "#ffff00"),
    ("gold", "#ffd700"), ("orange", "#ffa500"),
    ("peru", "#cd853f"), ("chocolate", "#d2691e"),
    ("orange4", "#8b5a00"),
    ("azure", "#f0ffff"), ("chartreuse", "#7fff00"),
    ("green", "#00ff00"), ("yellowgreen", "#9acd32"),
    ("limegreen", "#32cd32"), ("forestgreen", "#228b22"),
    ("green4", "#008b00"),
    ("cyan", "#00ffff"), ("aquamarine", "#7fffd4"),
    ("skyblue", "#87ceeb"), ("cyan4", "#008b8b"),
    ("slateblue", "#6a5acd"), ("blue", "#0000ff"),
    ("navyblue", "#000080"),
    ("pink", "#ffc0cb"), ("orchid", "#da70d6"),
    ("violet", "#ee82ee"), ("hotpink", "#ff69b4"),
    ("magenta", "#ff00ff"), ("purple", "#a020f0"),
    ("darkviolet", "#9400d3"),
    ("white", "#ffffff"), ("gray100", "#ffffff"),
    ("gray75", "#bfbfbf"), ("thistle", "#d8bfd8"),
    ("gray50", "#7f7f7f"), ("gray25", "#404040"),
    ("black", "#000000"),
    ("linen", "#faf0e6"), ("bisque", "#ffe4c4"),
    ("burlywood", "#deb887"), ("tan", "#d2b48c"),
    ("salmon", "#fa8072"), ("sienna", "#a0522d"),
    ("maroon", "#b03060"),
)
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
