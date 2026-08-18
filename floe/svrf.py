"""Calibre SVRF rule deck SUBSET parser -> per-check rule metadata.

Offline step (user call 2026-08-15): `python -m floe svrf deck.cal`
writes <deck>.rules.json and the viewer only ever loads that sidecar
(never the deck). floe does NOT implement SVRF geometry semantics -
per check block it extracts the @ description, the measurement
constraints (operator + numeric bound), the referenced layer names
and, through the derivation graph, the source GDS layers. Derivation
handling is the key scope cut: every operator (AND/NOT/SIZE/...) is
IGNORED and only the operand NAMES on the right-hand side become
edges of a directed graph, so "which drawn layers feed this rule"
never requires implementing the operations themselves.

SVRF is not a public grammar: unrecognized statements are counted
into a histogram and skipped, never fatal. Known gaps, deliberate:
  - DMACRO/CMACRO are NOT expanded (usage is counted; --scan makes
    the gap visible before trusting a converted deck).
  - TVF (Tcl) decks are out of scope - parse the SVRF that Calibre
    generates from them, never the Tcl itself.
  - Statements are line-oriented; a derivation wrapped across lines
    is dropped into the unknown histogram, not mis-parsed.
Preprocessing (INCLUDE / #DEFINE / #UNDEFINE / #IFDEF / #IFNDEF /
#ELSE / #ENDIF / VARIABLE) IS implemented because in-house decks
gate optional rules on switches: pass the same -D set as the
Calibre run or the check list will differ. #IFDEF/#IFNDEF support
the two-arg value form (`#IFDEF STACK 6LM` = defined AND equal),
directive lines strip // comments, values may be quoted, and
INCLUDE paths expand $VAR/${VAR}/~ from the environment. Switch
names the deck tests also FALL BACK to the environment (sourceme
workflow: `source sourceme.* && floe svrf ...` - lazy per-name
lookup, never a bulk env import; -D wins; used names are reported
and stored in the sidecar; --no-env-switches disables).
"""

import json
import os
import re
import sys
from collections import Counter, OrderedDict

FORMAT = "floe-svrf-rules"
VERSION = 1

_ID_RX = re.compile(r"[A-Za-z_][A-Za-z0-9_.\-]*")
# a constraint operator: not glued to an identifier tail, so option
# tokens like ABUT<90 never read as bounds
_OP_RX = re.compile(r"(?<![A-Za-z_])(<=|>=|==|!=|<|>)")
_BOUND_RX = re.compile(
    r"(?<![A-Za-z_])(<=|>=|==|!=|<|>)\s*([A-Za-z0-9_.\-+]+)")
_NUM_RX = re.compile(r"^[-+]?(\d+\.?\d*|\.\d+)([eE][-+]?\d+)?$")
_CHECK_RX = re.compile(r'^("?)([A-Za-z0-9_.\-$]+)\1\s*\{(.*)$')
_ASSIGN_RX = re.compile(r"^([A-Za-z_][A-Za-z0-9_.\-]*)\s*=\s*(.+)$")

# measurement statement -> the metric its bound constrains
MEAS = {"INTERNAL": "width", "INT": "width",
        "EXTERNAL": "space", "EXT": "space",
        "ENCLOSURE": "enclosure", "ENC": "enclosure",
        "AREA": "area", "DENSITY": "density",
        "LENGTH": "length", "ANGLE": "angle",
        "PERIMETER": "perimeter", "VERTEX": "vertex"}

# operator / option words excluded from operand-name extraction (a
# layer whose NAME collides with one of these is mis-filtered - the
# unresolved list makes that visible instead of silently wrong)
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
    "CENTERLINE", "SPACE", "WIDTH", "OPPOSITE", "NOTCH"}

# statement heads that are structural, never checks or layers -
# skipped without polluting the unknown histogram
IGNORED_HEADS = {"PRECISION", "RESOLUTION", "TITLE", "DRC", "LAYOUT",
                 "TEXT", "CONNECT", "SCONNECT", "VIRTUAL", "ATTACH",
                 "MASK", "LVS", "ERC", "PEX", "SOURCE", "GROUND",
                 "UNIT", "FLAG", "GROUP", "PORT", "EXCLUDE",
                 "CAPACITANCE", "RESISTANCE", "DEVICE", "TRACE",
                 "SVRF", "PUSHDOWN", "POLYGON", "FILTER",
                 "DFM", "RDB", "DVPARAMS", "OFFGRID",
                 "NET", "FLATTEN"}

# `NAME { ... }` blocks that are NOT rule checks: hybrid decks wrap
# Tcl in VERBATIM blocks (sfa14 field scan 2026-08-18 - the body
# held `if {[info exists env(...)]}` selection logic and 97
# conditional INCLUDEs), and Tcl control flow surfaces if/else/...
# at statement level. Bodies are brace-skipped; INCLUDEs inside are
# inventoried (and followed under --scan).
_NONCHECK_BLOCKS = {"VERBATIM", "IF", "ELSE", "ELSEIF",
                    "FOREACH", "WHILE", "PROC", "SWITCH"}


class Check(object):
    __slots__ = ("name", "desc", "constraints", "layers",
                 "source_gds", "unresolved")

    def __init__(self, name):
        self.name = name
        self.desc = []          # @ lines
        self.constraints = []   # {metric, op, value, text}
        self.layers = []        # direct operand names, order kept
        self.source_gds = []    # [[layer, dt-or-None], ...]
        self.unresolved = []


class Deck(object):
    """Parse result: layer tables, derivation graph, checks, stats."""

    def __init__(self, path):
        self.path = path
        self.defines = {}        # switch -> value or None
        self.variables = {}      # VARIABLE name -> float (or raw str)
        self.layers = OrderedDict()   # name -> [spec ints/(l,dt)]
        self.layer_maps = []     # (gds, dt-or-None, target)
        self.derived = OrderedDict()  # name -> rhs text
        self.derived_ops = {}    # name -> [operand names]
        self.checks = OrderedDict()   # name -> Check
        self.includes = []
        self.warnings = []
        self.stats = Counter()
        self.unknown = Counter()      # first token of skipped lines
        self.switches = []            # #IFDEF names, in order seen
        self.switch_values = {}       # name -> values tested by the
                                      # two-arg #IFDEF form (the -D
                                      # candidates --scan reports)
        self.verbatim_includes = []   # INCLUDE targets seen inside
                                      # VERBATIM/Tcl blocks
        self.env_used = OrderedDict()  # switches satisfied from the
                                       # ENVIRONMENT (sourceme
                                       # workflow) - provenance
        self.meas_hist = Counter()

    # -- resolution ---------------------------------------------------

    def _gds_of_layer(self, name):
        out = []
        for spec in self.layers.get(name, ()):
            if isinstance(spec, tuple):
                out.append(spec)
                continue
            mapped = [(g, d) for g, d, t in self.layer_maps
                      if t == spec]
            out.extend(mapped if mapped else [(spec, None)])
        return out

    def resolve(self):
        """Fill source_gds/unresolved of every check by walking the
        operand graph down to LAYER names (cycle-safe)."""
        for c in self.checks.values():
            seen, gds, unres = set(), set(), []
            stack = list(c.layers)
            while stack:
                n = stack.pop()
                if n in seen:
                    continue
                seen.add(n)
                if n in self.layers:
                    gds.update(self._gds_of_layer(n))
                elif n in self.derived_ops:
                    stack.extend(self.derived_ops[n])
                elif n not in self.variables:
                    unres.append(n)
            c.source_gds = sorted(gds, key=lambda p:
                                  (p[0], -1 if p[1] is None else p[1]))
            c.unresolved = sorted(set(unres))

    # -- output -------------------------------------------------------

    def to_json(self):
        from . import __version__
        checks = OrderedDict()
        for c in self.checks.values():
            e = {"desc": "\n".join(c.desc),
                 "constraints": c.constraints,
                 "layers": c.layers,
                 "source_gds": [[g, d] for g, d in c.source_gds]}
            if c.unresolved:
                e["unresolved"] = c.unresolved
            checks[c.name] = e
        return {"format": FORMAT, "version": VERSION,
                "deck": os.path.abspath(self.path),
                "generated_by": "floe %s" % __version__,
                "defines": self.defines,
                "variables": self.variables,
                "layers": {n: [[g, d] for g, d in self._gds_of_layer(n)]
                           for n in self.layers},
                "derived": dict(self.derived),
                "checks": checks,
                "stats": {"files": self.stats["files"],
                          "lines": self.stats["lines"],
                          "checks": len(self.checks),
                          "derivations": len(self.derived),
                          "skipped": self.stats["unknown"],
                          "cmacro_calls": self.stats["cmacro"],
                          "includes": self.includes,
                          "env_switches": dict(self.env_used),
                          "warnings": self.warnings}}


def rhs_operands(rhs):
    """Operand NAMES of a derivation right-hand side - operators,
    options and numbers dropped. Shared by the parser's graph build
    and the viewer's derivation-chain walk over the JSON sidecar
    (which stores rhs text only)."""
    return [t for t in _ID_RX.findall(rhs)
            if t.upper() not in KEYWORDS]


def _to_num(tok, variables):
    if _NUM_RX.match(tok):
        return float(tok)
    v = variables.get(tok)
    if isinstance(v, float):
        return v
    return None


class _Parser(object):
    def __init__(self, deck, scan_all):
        self.d = deck
        self.scan_all = scan_all   # --scan: walk BOTH #IFDEF branches
        self.cond = []             # stack of booleans (active branch?)
        self.cur = None            # open Check
        self.depth = 0
        self.macro_depth = 0       # >0: inside a DMACRO body (skipped)
        self.macro_pending = False  # DMACRO header seen, body { on
                                    # a LATER line (real-deck style)
        self.verbatim_depth = 0    # >0: inside a VERBATIM/Tcl block
        self.in_comment = False    # inside a /* ... */ banner
        self._icont = False        # last statement was IGNORED: its
                                   # wrapped continuation lines are
                                   # classified quietly too
        self.follow_verbatim = False  # normal parse follows Tcl-
                                      # conditional includes too
                                      # (--follow-verbatim; layers
                                      # picked via Tcl need it)
        self.env_switches = True   # #IFDEF falls back to the
                                   # ENVIRONMENT for names the deck
                                   # tests (sourceme workflow)
        self._cont = None          # [check, metric, text, had_bound]
                                   # of the last measurement - a
                                   # comparator-leading next line
                                   # continues it (wrapped bounds)
        self._acont = None         # (lhs, check) of the last assign
                                   # - wrapped derivations continue
                                   # on operator-leading lines
        self._acont_open = False   # rhs ended with an operator: the
                                   # NEXT line continues regardless
                                   # of its head (layer-name wraps)
        self._sub_rx = None        # compiled #DEFINE substitution
        self._sub_map = {}

    # -- preprocessing ------------------------------------------------

    def active(self):
        return all(self.cond)

    def _switch_val(self, name):
        """(defined, value) of a preprocessor switch: -D / #DEFINE
        first, then the ENVIRONMENT - the in-house flow exports
        every deck switch via `source sourceme.*` (user call
        2026-08-18), so names the deck TESTS are looked up lazily
        in os.environ (never a bulk import - unrelated env vars
        cannot leak in). A hit is promoted into defines (value
        substitution + provenance in env_used); -D still wins."""
        if name in self.d.defines:
            return True, self.d.defines[name]
        if self.env_switches:
            key = name[1:] if name.startswith("$") else name
            if key in os.environ:
                v = os.environ[key]
                self.d.env_used[name] = v
                if not name.startswith("$"):
                    self.d.defines[name] = v or None
                    self._rebuild_sub()
                return True, (v or None)
        return False, None

    def _rebuild_sub(self):
        vals = {n: v for n, v in self.d.defines.items() if v}
        self._sub_map = vals
        self._sub_rx = (re.compile(
            r"\b(%s)\b" % "|".join(re.escape(n)
                                   for n in sorted(vals, key=len,
                                                   reverse=True)))
            if vals else None)

    def directive(self, s):
        # comments ride on directive lines too: `#DEFINE W 5 // um`
        # must not glue the comment into the stored value (it broke
        # both value tests and numeric substitution)
        s = s.split("//", 1)[0].strip()
        if not s:
            return
        tok = s.split()
        head = tok[0].upper()

        def unq(t):
            if len(t) >= 2 and t[0] in "\"'" and t[-1] == t[0]:
                return t[1:-1]
            return t

        if head == "#DEFINE" and len(tok) >= 2:
            if self.active() or self.scan_all:
                val = unq(" ".join(tok[2:]))
                self.d.defines[tok[1]] = val or None
                self._rebuild_sub()
        elif head == "#UNDEFINE" and len(tok) >= 2:
            if self.active() or self.scan_all:
                self.d.defines.pop(tok[1], None)
                self._rebuild_sub()
        elif head in ("#IFDEF", "#IFNDEF") and len(tok) >= 2:
            name = tok[1]
            if name not in self.d.switches:
                self.d.switches.append(name)
            if len(tok) >= 3:
                # Calibre two-arg form: true iff NAME is defined
                # AND its value equals the literal (real configs
                # branch stacks/options this way)
                want = unq(" ".join(tok[2:]))
                vals = self.d.switch_values.setdefault(name, [])
                if want not in vals:
                    vals.append(want)
                ok, cur = self._switch_val(name)
                on = ok and cur is not None and str(cur) == want
            else:
                on = self._switch_val(name)[0]
            if head == "#IFNDEF":
                on = not on
            self.cond.append(on or self.scan_all)
        elif head == "#ELSE":
            if self.cond:
                self.cond[-1] = ((not self.cond[-1]) or self.scan_all)
            else:
                self.d.warnings.append("#ELSE without #IFDEF")
        elif head == "#ENDIF":
            if self.cond:
                self.cond.pop()
            else:
                self.d.warnings.append("#ENDIF without #IFDEF")
        else:
            self.d.stats["unknown_directive"] += 1

    def feed_file(self, path, incdirs, chain):
        real = os.path.realpath(path)
        if real in chain:
            self.d.warnings.append("INCLUDE cycle: %s" % path)
            return
        try:
            fh = open(path, "r", errors="replace")
        except OSError as exc:
            self.d.warnings.append("INCLUDE unreadable: %s (%s)"
                                   % (path, exc))
            return
        self.d.stats["files"] += 1
        with fh:
            for line in fh:
                self.d.stats["lines"] += 1
                s = line.strip()
                if not s:
                    continue
                # /* ... */ block comments (real decks carry banner
                # blocks whose doc lines leaked into the unknown
                # histogram as arbitrary heads)
                if self.in_comment:
                    j = s.find("*/")
                    if j < 0:
                        continue
                    s = s[j + 2:].strip()
                    self.in_comment = False
                    if not s:
                        continue
                while not s.startswith("@") and "/*" in s:
                    i = s.find("/*")
                    j = s.find("*/", i + 2)
                    if j < 0:
                        s = s[:i].rstrip()
                        self.in_comment = True
                        break
                    s = (s[:i] + " " + s[j + 2:]).strip()
                if not s:
                    continue
                if s.startswith("#"):
                    self.directive(s)
                    continue
                if not self.active():
                    continue
                if not s.startswith("@"):
                    s = s.split("//", 1)[0].strip()
                    if not s:
                        continue
                if self._sub_rx and not s.startswith("@"):
                    s = self._sub_rx.sub(
                        lambda m: self._sub_map[m.group(1)], s)
                tok0 = s.split(None, 1)
                if tok0 and tok0[0].upper() == "INCLUDE" \
                        and self.verbatim_depth > 0:
                    # Tcl-conditional include: inventory it always,
                    # dive into it only under --scan (same
                    # philosophy as walking both #IFDEF branches -
                    # the normal parse cannot evaluate the Tcl)
                    tgt = (tok0[1].strip().strip('"\'')
                           if len(tok0) > 1 else "")
                    if tgt and tgt not in self.d.verbatim_includes:
                        self.d.verbatim_includes.append(tgt)
                    if (self.scan_all or self.follow_verbatim) \
                            and tgt:
                        inc = self._find_include(tgt, path, incdirs)
                        if inc:
                            self.d.includes.append(inc)
                            sav = self.verbatim_depth
                            self.verbatim_depth = 0
                            self.feed_file(inc, incdirs,
                                           chain | {real})
                            self.verbatim_depth = sav
                        else:
                            exp = os.path.expanduser(
                                os.path.expandvars(tgt))
                            msg = ("INCLUDE (in VERBATIM) not "
                                   "found: %s" % tgt)
                            if exp != tgt:
                                msg += " -> %s" % exp
                            if "$" in exp:
                                msg += (" (env var unset in this "
                                        "shell?)")
                            self.d.warnings.append(msg)
                    continue
                if tok0 and tok0[0].upper() == "INCLUDE" \
                        and (self.cur is not None
                             or self.macro_depth > 0
                             or self.macro_pending):
                    # an INCLUDE textually inside a macro body or
                    # an open block never executes: say so instead
                    # of losing the file silently (the DMACRO
                    # brace-drift bug hid a whole include tree)
                    self.d.warnings.append(
                        "INCLUDE swallowed by %s: %s"
                        % ("a DMACRO body"
                           if (self.macro_depth or self.macro_pending)
                           else "open block %s" % self.cur.name, s))
                    self.statement(s)
                    continue
                if tok0 and tok0[0].upper() == "INCLUDE":
                    tgt = (tok0[1].strip().strip('"\'')
                           if len(tok0) > 1 else "")
                    inc = self._find_include(tgt, path, incdirs)
                    if inc:
                        self.d.includes.append(inc)
                        self.feed_file(inc, incdirs, chain | {real})
                    else:
                        exp = os.path.expanduser(
                            os.path.expandvars(tgt))
                        msg = "INCLUDE not found: %s" % tgt
                        if exp != tgt:
                            msg += " -> %s" % exp
                        if "$" in exp:
                            msg += " (env var unset in this shell?)"
                        self.d.warnings.append(msg)
                    continue
                self.statement(s)
        # parse state open at a file boundary is always an anomaly
        # (INCLUDE is blocked while a block/macro is open, so state
        # cannot legitimately span files)
        if self.cur is not None:
            self.d.warnings.append(
                "unclosed block %s at end of %s"
                % (self.cur.name, os.path.basename(path)))
        if self.macro_depth > 0 or self.macro_pending:
            self.d.warnings.append(
                "unclosed DMACRO body at end of %s (brace drift?)"
                % os.path.basename(path))
        if self.verbatim_depth > 0:
            # contain Tcl brace drift (quoted braces etc.) to the
            # file it happened in
            self.d.warnings.append(
                "unclosed VERBATIM/Tcl block at end of %s (brace "
                "drift?)" % os.path.basename(path))
            self.verbatim_depth = 0
        if self.in_comment:
            self.d.warnings.append(
                "unclosed /* comment at end of %s"
                % os.path.basename(path))
            self.in_comment = False
        if self.cond and not chain:
            self.d.warnings.append("unbalanced #IFDEF at end of deck")

    @staticmethod
    def _find_include(tgt, src, incdirs):
        if not tgt:
            return None
        # Calibre expands $VAR / ${VAR} (and ~) in INCLUDE paths -
        # real decks do `INCLUDE $TECHDIR/DRC/...`. Mirror it from
        # this process's environment; an unset var keeps the
        # literal text and the caller's warning flags it.
        tgt = os.path.expanduser(os.path.expandvars(tgt))
        cands = [tgt] if os.path.isabs(tgt) else \
            [os.path.join(os.path.dirname(src), tgt)] + \
            [os.path.join(d, tgt) for d in incdirs]
        for c in cands:
            if os.path.isfile(c):
                return c
        return None

    # -- statements ---------------------------------------------------

    def statement(self, s):
        if self.verbatim_depth > 0:
            # VERBATIM/Tcl body: pure brace tracking - `} else {`
            # nets 0 and correctly stays inside. INCLUDE lines are
            # intercepted in feed_file for the scan inventory.
            self.verbatim_depth = max(
                self.verbatim_depth + s.count("{") - s.count("}"),
                0)
            return
        if self.macro_depth > 0 or self.macro_pending:
            # inside (or awaiting) a DMACRO body: skip everything,
            # track braces. The body's first { often sits ALONE on
            # the line below the header - counting it on top of an
            # assumed depth left macro_depth stuck at 1 and
            # swallowed the rest of the deck, nested INCLUDEs
            # included (field report 2026-08-18)
            d = s.count("{") - s.count("}")
            if self.macro_pending:
                if "{" not in s:
                    return       # header continuation line
                self.macro_pending = False
                self.macro_depth = max(d, 0)
            else:
                self.macro_depth = max(self.macro_depth + d, 0)
            return
        if self.cur is not None:
            self.block_line(s)
            return
        if self._try_cont(s):
            return
        self._cont = None
        m = _CHECK_RX.match(s)
        if m and m.group(2).upper() in _NONCHECK_BLOCKS:
            # VERBATIM / Tcl control block: never a check - the
            # sfa14 scan grew phantom checks named "if"/"VERBATIM"
            # whose "bodies" ate the rest of the deck
            self.d.stats["verbatim"] += 1
            rest = m.group(3)
            self.verbatim_depth = max(
                1 + rest.count("{") - rest.count("}"), 0)
            self._cont = None
            self._acont = None
            return
        if m and m.group(2).upper() not in IGNORED_HEADS \
                and not _ASSIGN_RX.match(s):
            name = m.group(2)
            if name in self.d.checks:
                self.d.warnings.append("duplicate check %s "
                                       "(kept last)" % name)
            self.cur = Check(name)
            self.d.checks[name] = self.cur
            self.depth = 1
            self._acont = None
            self._icont = False
            rest = m.group(3).strip()
            if rest:
                self.block_line(rest)
            return
        head = s.split(None, 1)[0].upper()
        if head == "LAYER":
            self._acont = None
            self._icont = False
            self.layer_stmt(s)
        elif head == "VARIABLE":
            self._acont = None
            self._icont = False
            self.variable_stmt(s)
        elif head == "DMACRO":
            # macros are NOT expanded (scope call: --scan reports
            # usage); the body is skipped by brace depth
            self._acont = None
            self._icont = False
            self.d.stats["dmacro"] += 1
            d = s.count("{") - s.count("}")
            if "{" in s:
                if d > 0:
                    self.macro_depth = d
                # else: one-line macro, opened and closed here
            else:
                self.macro_pending = True   # { on a later line
        elif head == "CMACRO":
            self._acont = None
            self.d.stats["cmacro"] += 1
        else:
            am = _ASSIGN_RX.match(s)
            if am:
                self.assign(am.group(1), am.group(2), None)
            elif self._try_acont(s):
                self._icont = False
            elif head in IGNORED_HEADS:
                self._acont = None
                self._icont = True
                self.d.stats["ignored"] += 1
            elif head[:1] in "[~(":
                # DFM property math / expression continuations
                self._acont = None
                self.d.stats["prop_expr"] += 1
            elif self._icont:
                # wrapped continuation of an ignored statement
                self.d.stats["ignored"] += 1
            else:
                self._acont = None
                self.d.stats["unknown"] += 1
                self.d.unknown[head] += 1

    def block_line(self, s):
        if s.startswith("@"):
            t = s.lstrip("@").strip()
            if len(t) >= 2 and t[0] == '"' and t[-1] == '"':
                t = t[1:-1]
            self.cur.desc.append(t)
            return
        while s.startswith("}"):
            self.depth -= 1
            s = s[1:].lstrip()
            if self.depth <= 0:
                self.cur = None
                self._cont = None
                if s:
                    self.statement(s)
                return
        if not s:
            return
        closes = 0
        while s.endswith("}"):
            closes += 1
            s = s[:-1].rstrip()
        nested = s.count("{") - s.count("}") if s else 0
        if s:
            if self._try_cont(s):
                pass
            else:
                am = _ASSIGN_RX.match(s)
                if am:
                    self._cont = None
                    self.assign(am.group(1), am.group(2), self.cur)
                else:
                    head = s.split(None, 1)[0].upper()
                    if head in MEAS:
                        self._acont = None
                        self._icont = False
                        self.measurement(s, self.cur)
                    elif self._try_acont(s):
                        self._icont = False
                    elif self._cont is not None \
                            and head in KEYWORDS:
                        # operator-leading wrap of the previous
                        # measurement (NOT/WITH/ENCLOSE...): its
                        # operands join the check (source closure);
                        # a comparator line may still follow and
                        # lands via _try_cont
                        ck = self._cont[0]
                        if ck is not None:
                            for n in rhs_operands(s):
                                if n not in ck.layers:
                                    ck.layers.append(n)
                        self._icont = False
                    elif head in IGNORED_HEADS:
                        # DFM RDB / spec statements inside checks
                        self._cont = None
                        self._acont = None
                        self._icont = True
                        self.d.stats["ignored"] += 1
                    elif head[:1] in "[~(":
                        # DFM property math wraps ([expr], ~(..))
                        self._cont = None
                        self._acont = None
                        self.d.stats["prop_expr"] += 1
                    elif self._icont:
                        # wrapped continuation of an ignored
                        # statement (DFM RDB argument lists etc.)
                        self._cont = None
                        self.d.stats["ignored"] += 1
                    else:
                        self._cont = None
                        self._acont = None
                        self.d.stats["unknown_in_block"] += 1
                        self.d.unknown[head] += 1
        self.depth += nested - closes
        if self.depth <= 0:
            self.cur = None
            self._cont = None

    def layer_stmt(self, s):
        tok = s.split()
        if len(tok) >= 2 and tok[1].upper() == "MAP":
            ints = [int(t) for t in tok[2:]
                    if re.match(r"^-?\d+$", t)]
            has_dt = any(t.upper() == "DATATYPE" for t in tok)
            if len(ints) >= 2:
                gds, tgt = ints[0], ints[-1]
                dts = ints[1:-1] if has_dt else [None]
                for dt in (dts or [None]):
                    self.d.layer_maps.append((gds, dt, tgt))
            self.d.stats["layer_map"] += 1
            return
        if len(tok) < 3:
            self.d.stats["unknown"] += 1
            return
        name, specs = tok[1], []
        for t in tok[2:]:
            if re.match(r"^\d+$", t):
                specs.append(int(t))
            elif re.match(r"^\d+\.\d+$", t):
                l, dt = t.split(".")
                specs.append((int(l), int(dt)))
        if specs:
            self.d.layers[name] = specs
            self.d.stats["layer"] += 1
        else:
            self.d.stats["unknown"] += 1
            self.d.unknown["LAYER"] += 1

    def variable_stmt(self, s):
        tok = s.split()
        if len(tok) >= 3:
            v = _to_num(tok[2], {})
            self.d.variables[tok[1]] = (v if v is not None
                                        else " ".join(tok[2:]))

    def assign(self, lhs, rhs, check):
        """name = expr. Operators ignored: RHS identifier names,
        minus keywords/numbers, are the graph edges. A measurement
        RHS inside a check block ALSO records its constraint."""
        self.d.derived[lhs] = rhs.strip()
        head = rhs.split(None, 1)[0].upper() if rhs.split() else ""
        if head in MEAS:
            ops = self.measurement(rhs, check)
            self.d.derived_ops[lhs] = ops
            self._acont = None
        else:
            names = rhs_operands(rhs)
            self.d.derived_ops[lhs] = names
            if check is not None:
                for n in names:
                    if n not in check.layers:
                        check.layers.append(n)
            self._icont = False
            # the rhs may wrap onto following lines (sfa14 field
            # scan: ~1.5k NOT/WITH/layer-name-leading wraps)
            toks = rhs.split()
            self._acont = (lhs, check)
            self._acont_open = bool(toks) \
                and toks[-1].upper() in KEYWORDS
        self.d.stats["assign"] += 1

    def _try_acont(self, s):
        """Continuation line of a wrapped derivation: taken when the
        previous assign's rhs ended with an operator, or this line
        LEADS with one - both real-deck wrap styles. Operands join
        the derivation's operand list (closure completeness)."""
        if self._acont is None:
            return False
        head = s.split(None, 1)[0].upper()
        if not (self._acont_open or head in KEYWORDS):
            return False
        lhs, check = self._acont
        self.d.derived[lhs] = (self.d.derived.get(lhs, "")
                               + " " + s.strip())
        ops = self.d.derived_ops.setdefault(lhs, [])
        names = [n for n in rhs_operands(s) if n not in ops]
        ops.extend(names)
        if check is not None:
            for n in names:
                if n not in check.layers:
                    check.layers.append(n)
        toks = s.split()
        self._acont_open = bool(toks) \
            and toks[-1].upper() in KEYWORDS
        return True

    @staticmethod
    def _chain(text, pos):
        """The CONTIGUOUS comparator+value chain starting at pos -
        the statement's own bounds end at the first non-comparator
        token, so option comparators further right (ABUT>0<90,
        OPPOSITE EXTENDED < x) never read as constraints."""
        out = []
        while True:
            bm = _BOUND_RX.match(text, pos)
            if bm is None:
                break
            out.append((bm.group(1), bm.group(2)))
            pos = bm.end()
            while pos < len(text) and text[pos].isspace():
                pos += 1
        return out

    def _add_bounds(self, check, metric, bounds, text):
        for op, tok in bounds:
            val = _to_num(tok, self.d.variables)
            c = {"metric": metric, "op": op, "value": val,
                 "text": text}
            if val is None:
                c["raw"] = tok
            check.constraints.append(c)

    def _try_cont(self, s):
        """A comparator-leading line continues the previous
        measurement statement - real decks wrap the constraint
        onto its own line (SVRF is free-format)."""
        if self._cont is None or _OP_RX.match(s) is None:
            return False
        bounds = self._chain(s, 0)
        if not bounds:
            return False
        check, metric, base, had = self._cont
        text = base + " " + s.strip()
        self._add_bounds(check, metric, bounds, text)
        if not had:
            self.d.stats["meas_no_bound"] -= 1
        self._cont = [check, metric, text, True]
        return True

    def measurement(self, s, check):
        """INTERNAL/EXTERNAL/... [layers] op value [op value] opts.
        Returns the operand names (for assignment RHS reuse)."""
        head, rest = (s.split(None, 1) + [""])[:2]
        metric = MEAS[head.upper()]
        self.d.meas_hist[head.upper()] += 1
        m = _OP_RX.search(rest)
        headpart = rest[:m.start()] if m else rest
        ops = [t for t in _ID_RX.findall(headpart)
               if t.upper() not in KEYWORDS]
        if check is not None:
            for n in ops:
                if n not in check.layers:
                    check.layers.append(n)
            bounds = self._chain(rest, m.start()) if m else []
            self._add_bounds(check, metric, bounds, s.strip())
            if not bounds:
                self.d.stats["meas_no_bound"] += 1
            self._cont = [check, metric, s.strip(), bool(bounds)]
        return ops


def parse_deck(path, defines=None, include_dirs=(), scan_all=False,
               follow_verbatim=False, env_switches=True):
    deck = Deck(path)
    deck.defines.update(defines or {})
    p = _Parser(deck, scan_all)
    p.follow_verbatim = follow_verbatim
    p.env_switches = env_switches
    if deck.defines:
        p._rebuild_sub()
    p.feed_file(path, list(include_dirs), frozenset())
    if p.cur is not None:
        deck.warnings.append("unterminated check block %s"
                             % p.cur.name)
    if deck.verbatim_includes and not scan_all \
            and not follow_verbatim:
        deck.warnings.append(
            "%d INCLUDE(s) inside VERBATIM/Tcl blocks were NOT "
            "followed (Tcl-conditional; --scan or "
            "--follow-verbatim follows them)"
            % len(deck.verbatim_includes))
    deck.resolve()
    return deck


def format_scan(deck):
    """--scan: the inventory that decides the parser scope BEFORE
    trusting a converted deck (macros? switches? statement kinds?)."""
    d = deck
    L = ["deck scan: %s" % d.path,
         "  files %d (%d includes), %d lines"
         % (d.stats["files"], len(d.includes), d.stats["lines"])]
    for inc in d.includes[:20]:
        L.append("    include %s" % inc)
    if len(d.includes) > 20:
        L.append("    ... %d more" % (len(d.includes) - 20))
    sw = []
    for name in d.switches:
        vals = d.switch_values.get(name)
        sw.append("%s(%s)" % (name, "|".join(vals))
                  if vals else name)
    L.append("  switches (#IFDEF): %s" % (", ".join(sw) or "-"))
    if d.env_used:
        L.append("  switches satisfied from the environment: %s"
                 % ", ".join(
                     "%s=%s" % (n, v) if v else n
                     for n, v in d.env_used.items()))
    L.append("  defines in effect: %s"
             % (", ".join(sorted(d.defines)) or "-"))
    L.append("  layers %d, layer maps %d, variables %d"
             % (d.stats["layer"], d.stats["layer_map"],
                len(d.variables)))
    L.append("  derivations %d, checks %d"
             % (d.stats["assign"], len(d.checks)))
    L.append("  measurements: %s"
             % (", ".join("%s %d" % kv
                          for kv in d.meas_hist.most_common()) or "-"))
    dm, cm = d.stats["dmacro"], d.stats["cmacro"]
    L.append("  DMACRO %d / CMACRO %d%s"
             % (dm, cm, "  << macros in use: expansion is NOT "
                "implemented, metadata will be incomplete"
                if cm else ""))
    if d.stats["verbatim"] or d.verbatim_includes:
        L.append("  VERBATIM/Tcl blocks %d; INCLUDEs inside %d "
                 "(--scan follows them, the normal parse skips)"
                 % (d.stats["verbatim"],
                    len(d.verbatim_includes)))
        for t in d.verbatim_includes[:10]:
            L.append("    verbatim include %s" % t)
        if len(d.verbatim_includes) > 10:
            L.append("    ... %d more"
                     % (len(d.verbatim_includes) - 10))
    if d.stats["prop_expr"]:
        L.append("  property-expression continuations skipped %d"
                 % d.stats["prop_expr"])
    unk = d.unknown.most_common(20)
    L.append("  skipped statements %d%s"
             % (d.stats["unknown"] + d.stats["unknown_in_block"],
                (":" if unk else "")))
    for name, n in unk:
        L.append("    %-24s %d" % (name, n))
    for w in d.warnings[:20]:
        L.append("  warn: %s" % w)
    return "\n".join(L)


def write_json(deck, out):
    data = deck.to_json()
    tmp = out + ".tmp"
    with open(tmp, "w") as f:
        json.dump(data, f, indent=1, sort_keys=True)
        f.write("\n")
    os.replace(tmp, out)
    return data


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
