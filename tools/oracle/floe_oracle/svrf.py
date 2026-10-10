"""SVRF rule sidecar (<deck>.rules.json) - the READER side.

The sidecar is built by `floe-index svrf deck.cal` (rust/cli/src/svrf.rs;
moved from `floe svrf` on 2026-09-29 - the tools that build files from
inputs live in floe-index: `vfs` the layout cache, `drc` the result pack,
`svrf` the rule sidecar). The viewer and `floe drc` only ever load that
sidecar, never the deck. What is left here is what they need: the format
check (load_rules) and the operand split of a derivation right-hand side
(rhs_operands), which the viewer uses to walk the derivation chain the
sidecar stores as text - it must split exactly as the builder did, so the
operator words below are the builder's list (svrf.rs KEYWORD_WORDS/MEAS).
"""

import json
import re
import sys

FORMAT = "floe-svrf-rules"
VERSION = 1

_ID_RX = re.compile(r"[A-Za-z_][A-Za-z0-9_.\-]*")

# measurement statement heads (their words are operators too)
MEAS = {"INTERNAL": "width", "INT": "width",
        "EXTERNAL": "space", "EXT": "space",
        "ENCLOSURE": "enclosure", "ENC": "enclosure",
        "AREA": "area", "DENSITY": "density",
        "LENGTH": "length", "ANGLE": "angle",
        "PERIMETER": "perimeter", "VERTEX": "vertex"}

# operator / option words excluded from operand-name extraction
KEYWORDS = set(MEAS) | {
    "AND", "OR", "NOT", "XOR", "INTERACT", "INSIDE", "OUTSIDE",
    "TOUCH", "CUT", "ENCLOSE", "BY", "SIZE", "GROW", "SHRINK",
    "EXTENT", "EXTENTS", "HOLES", "WITH", "EDGE", "CONVEX",
    "OPPOSITE", "ABUT", "SINGULAR", "REGION", "PROJECTING",
    "PARALLEL", "PERPENDICULAR", "ONLY", "ALSO", "OVER", "UNDER",
    "UNDEROVER", "COPY", "NET", "RATIO", "WINDOW", "STEP",
    "TRUNCATE", "INNER", "OUTER", "MEASURE", "ALL", "PRINT",
    "RECTANGLE", "SQUARE", "COUNT", "COINCIDENT", "EXPAND",
    "TOP", "LEFT", "RIGHT", "BOTTOM", "GOOD", "BAD", "MAX", "MIN",
    "EVEN", "ODD", "MULTI", "ORTHOGONAL", "POLYGON", "CORNER",
    "CENTERLINE", "SPACE", "WIDTH", "NOTCH"}


def rhs_operands(rhs):
    """Operand NAMES of a derivation right-hand side - operators,
    options and numbers dropped (the builder's rule; the viewer walks the
    sidecar's derivation text with it)."""
    return [t for t in _ID_RX.findall(rhs)
            if t.upper() not in KEYWORDS]


def load_rules(path):
    """Viewer-side loader: returns the dict or raises ValueError."""
    with open(path, "r") as f:
        data = json.load(f)
    if data.get("format") != FORMAT:
        raise ValueError("%s is not a %s file" % (path, FORMAT))
    if data.get("version", 0) > VERSION:
        sys.stderr.write("[floe][warn] %s is a newer rules format "
                         "(v%s > v%d)\n"
                         % (path, data.get("version"), VERSION))
    return data
