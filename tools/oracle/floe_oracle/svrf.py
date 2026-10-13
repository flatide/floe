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
    "CENTERLINE", "SPACE", "WIDTH", "NOTCH", "OVERLAP",
    "INTERSECTING", "EXTENDED"}

_OP_RX = re.compile(r"(?<![A-Za-z_])(?:<=|>=|==|!=|<|>)")
_BOUND_RX = re.compile(r"(?<![A-Za-z_])(?:<=|>=|==|!=|<|>)\s*[A-Za-z0-9_.+\-]+")


def constraint_metric(metric, text):
    """Refine old sidecar labels only when the statement gives evidence.

    Mentor's Calibre Rule Writing: Basic Concepts, slides 1-85, 1-91,
    1-102 and 1-140: two-layer INTERNAL measures overlap, single-layer
    EXTERNAL NOTCH measures a notch, and ENCLOSURE covers both enclosure
    and extension. OVERLAP is also an intersection OPTION for INT/EXT/ENC;
    it must never change EXTERNAL or ENCLOSURE into an overlap metric.
    Compound layer expressions are deliberately not guessed from ID count.
    Keep this classification in sync with rust/cli/src/svrf.rs.
    """
    if metric not in ("width", "space") or not isinstance(text, str):
        return metric
    parts = text.split(None, 1)
    if len(parts) != 2 or MEAS.get(parts[0].upper()) != metric:
        return metric
    rest = parts[1]
    first = _OP_RX.search(rest)
    operands = rest[:first.start()] if first else rest
    names = operands.split()
    for name in names:
        if ((name.startswith("[") and name.endswith("]")) or
                (name.startswith("(") and name.endswith(")"))):
            name = name[1:-1]
        if not _ID_RX.fullmatch(name) or name.upper() in KEYWORDS:
            return metric
    if metric == "width" and len(names) == 2:
        return "overlap"
    if metric == "space" and len(names) == 1 and first:
        end = first.start()
        while True:
            bound = _BOUND_RX.match(rest, end)
            if bound is None:
                break
            end = bound.end()
            while end < len(rest) and rest[end].isspace():
                end += 1
        if "NOTCH" in (word.upper() for word in _ID_RX.findall(rest[end:])):
            return "notch"
    return metric


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
    # Existing v1 sidecars already retain the statement text, so this
    # refinement does not require users to rebuild their metadata.
    for check in data.get("checks", {}).values():
        for constraint in check.get("constraints", ()):
            metric = constraint.get("metric")
            refined = constraint_metric(metric, constraint.get("text"))
            if refined != metric:
                constraint["metric"] = refined
    return data
