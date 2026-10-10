"""SVRF subset-parser gate: `floe-index svrf` (rust/cli/src/svrf.rs) ->
the <deck>.rules.json sidecar the viewer and `floe drc` read.

The parser moved from floe/svrf.py to floe-index on 2026-09-29 (user call:
the tools that build files from inputs live in floe-index); the port was
checked against the Python parser on every deck below plus 3,000 random
decks - sidecar bytes, scan text and parse state identical. The gate
reaches the parse state through the gate-only `--dump-state` output
(`BuilderView` below keeps the attribute names these checks were written
against).

  R1  preprocessing: INCLUDE merge (relative to the including file,
      cycle-safe), #DEFINE/#IFDEF/#IFNDEF/#ELSE/#ENDIF branch
      selection driven by -D, #DEFINE value substitution into a
      constraint, VARIABLE resolution to a numeric bound.
  R2  derivation graph: operators ignored, operand names only -
      diamond closure reaches every source LAYER, a graph cycle
      terminates, an undefined operand lands in `unresolved`,
      LAYER MAP resolves internal numbers to (gds, datatype).
  R3  check extraction: multi-@ descriptions join, two measurement
      statements in one block, a double bound (>= a <= b) becomes
      two constraints, fused ops (<0.03), option tokens with glued
      comparators (ABUT<90) are NOT bounds, an assignment with a
      measurement RHS inside a block records constraint + operands,
      unknown in-block statements are counted not fatal, quoted
      check names, DMACRO bodies are skipped whole.
  R3b real-world expression forms (user call 2026-08-17): bounds =
      the CONTIGUOUS comparator chain at the first comparator only
      - option comparators (ABUT>0<90, OPPOSITE EXTENDED < x) never
      read as constraints; zero-lower-bound chains (> 0 < v);
      leading-dot values; comparator-leading next lines continue
      the wrapped measurement (multi-line too), without leaking
      across a block close.
  R4  end-to-end vs gen_drcdb --svrf: every db check name resolves
      in the sidecar, constraint values match the generator formula
      through all five emitted syntax styles (spaced/fused/range/
      VARIABLE bound/wrapped line), every check reaches a source
      gds layer; -D SYNTH_EXTRA adds exactly the EXTRA.CHECK.1
      rule.
  R5  the command (2026-09-29): the sidecar is byte-for-byte what
      Python's json.dump(indent=1, sort_keys=True) writes (non-ASCII,
      quotes, 1e-05 / 1e+16 floats, null datatypes); <deck>.rules.json
      by default; --scan prints the inventory and writes nothing;
      -DNAME / -D=NAME / --define=NAME are -D NAME; a missing deck
      exits 1, an unknown option 2, writing nothing; `floe2 svrf`
      points to floe-index and exits 2; the builder's operator words
      equal the viewer's (floe/svrf.py rhs_operands).

usage: .venv/bin/python tools/validate_svrf.py
"""

import collections
import json
import os
import shutil
import subprocess
import sys
import tempfile

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
sys.path.insert(0, ROOT)
sys.path.insert(0, os.path.join(ROOT, "tools", "oracle"))  # floe_oracle (P3)
from floe_oracle import drc  # noqa: E402
from floe_oracle import svrf as reader  # noqa: E402

BIN = os.path.join(ROOT, "rust", "target", "release", "floe-index")
# the product command line: the Rust `floe2` (rust/floe2; the Python
# floe2 CLI is gone - docs/SHARED_APP_LAYER.ko.md P1c)
FLOE2 = os.environ.get("FLOE2_BIN") or os.path.join(ROOT, "rust", "target", "release", "floe2")


class _Check(object):
    def __init__(self, c):
        self.name = c["name"]
        self.desc = c["desc"]
        self.constraints = c["constraints"]
        self.layers = c["layers"]
        self.source_gds = [tuple(p) for p in c["source_gds"]]
        self.unresolved = c["unresolved"]


class _Deck(object):
    """The builder's parse state, under the Python parser's names."""

    def __init__(self, st, sidecar):
        self.checks = collections.OrderedDict(
            (c["name"], _Check(c)) for c in st["checks"])
        self.layers = collections.OrderedDict(
            (k, [tuple(s) if len(s) == 2 else s[0] for s in v])
            for k, v in st["layers"])
        self.defines = collections.OrderedDict(st["defines"])
        self.variables = collections.OrderedDict(st["variables"])
        self.derived = collections.OrderedDict(st["derived"])
        self.derived_ops = collections.OrderedDict(st["derived_ops"])
        self.layer_maps = [tuple(m) for m in st["layer_maps"]]
        self.includes = st["includes"]
        self.warnings = st["warnings"]
        self.stats = collections.Counter(dict(st["stats"]))
        self.unknown = collections.Counter(dict(st["unknown"]))
        self.meas_hist = collections.Counter(dict(st["meas_hist"]))
        self.switches = st["switches"]
        self.switch_values = dict(st["switch_values"])
        self.verbatim_includes = st["verbatim_includes"]
        self.env_used = collections.OrderedDict(st["env_used"])
        self.scan = st["scan"]
        self.keywords = set(st["keywords"])
        self.sidecar = sidecar     # (path, dict) of the written file

    def to_json(self):
        return self.sidecar[1]


def run(args, **kw):
    return subprocess.run([BIN, "svrf"] + list(args), capture_output=True,
                          text=True, **kw)


class BuilderView(object):
    """`svrf.parse_deck(...)` etc. as the checks below call them."""
    FORMAT = reader.FORMAT
    load_rules = staticmethod(reader.load_rules)
    _tmp = None

    @classmethod
    def parse_deck(cls, path, defines=None, include_dirs=(), scan_all=False,
                   follow_verbatim=False, env_switches=True):
        n = len(os.listdir(cls._tmp))
        state = os.path.join(cls._tmp, "state%d.json" % n)
        out = os.path.join(cls._tmp, "side%d.json" % n)
        args = [path, "--dump-state", state]
        for k, v in (defines or {}).items():
            args += ["-D", k if v is None else "%s=%s" % (k, v)]
        for d in include_dirs:
            args += ["-I", d]
        args += ["--scan"] if scan_all else ["-o", out]
        if follow_verbatim:
            args.append("--follow-verbatim")
        if not env_switches:
            args.append("--no-env-switches")
        res = run(args)
        if res.returncode != 0:
            raise AssertionError("floe-index svrf %s -> rc %d\n%s"
                                 % (args, res.returncode, res.stderr))
        with open(state) as f:
            st = json.load(f)
        sidecar = None
        if not scan_all:
            sidecar = (out, reader.load_rules(out))
        return _Deck(st, sidecar)

    @staticmethod
    def format_scan(d):
        return d.scan

    @staticmethod
    def write_json(d, out):
        shutil.copyfile(d.sidecar[0], out)
        return d.sidecar[1]


svrf = BuilderView

FAIL = 0


def check(name, ok, note=""):
    global FAIL
    print("  %-52s %s%s" % (name, "ok" if ok else "FAIL",
                            (" - " + note) if note and not ok else ""))
    if not ok:
        FAIL = 1


def w(path, text):
    with open(path, "w") as f:
        f.write(text)


def r1(tmp):
    print("[R1] preprocessing")
    main = os.path.join(tmp, "main.svrf")
    sub = os.path.join(tmp, "inc", "sub.svrf")
    os.makedirs(os.path.dirname(sub), exist_ok=True)
    w(sub, "LAYER SUB 77\n"
           "INCLUDE \"../main.svrf\"\n")     # cycle: must not hang
    w(main,
      "LAYER M1 31\n"
      "VARIABLE VMIN 0.031\n"
      "#DEFINE WMIN 0.05\n"
      "INCLUDE \"inc/sub.svrf\"\n"
      "#IFDEF OPT\n"
      "OPT.RULE { @ opt on\n  INT M1 < WMIN\n}\n"
      "#ELSE\n"
      "BASE.RULE { @ opt off\n  INT SUB < VMIN\n}\n"
      "#ENDIF\n")
    d0 = svrf.parse_deck(main)
    d1 = svrf.parse_deck(main, {"OPT": None})
    check("no -D: BASE.RULE only",
          list(d0.checks) == ["BASE.RULE"], str(list(d0.checks)))
    check("-D OPT: OPT.RULE only",
          list(d1.checks) == ["OPT.RULE"], str(list(d1.checks)))
    check("INCLUDE merged (SUB layer via relative path)",
          "SUB" in d0.layers)
    check("INCLUDE cycle warned, not hung",
          any("cycle" in x for x in d0.warnings))
    c = d0.checks["BASE.RULE"].constraints
    check("VARIABLE bound resolved",
          len(c) == 1 and c[0]["value"] == 0.031, str(c))
    c = d1.checks["OPT.RULE"].constraints
    check("#DEFINE value substituted into bound",
          len(c) == 1 and c[0]["value"] == 0.05, str(c))
    d2 = svrf.parse_deck(main, scan_all=True)
    check("--scan walks both #IFDEF branches",
          {"OPT.RULE", "BASE.RULE"} <= set(d2.checks))
    # env vars in INCLUDE paths (real decks: $TECHDIR/DRC/...)
    envmain = os.path.join(tmp, "envmain.svrf")
    w(envmain,
      "LAYER M2 32\n"
      "INCLUDE $FLOE_SVRF_T/inc/sub.svrf\n"
      "INCLUDE ${FLOE_SVRF_T}/inc/sub.svrf\n"
      "INCLUDE $FLOE_SVRF_UNSET/x.svrf\n")
    os.environ["FLOE_SVRF_T"] = tmp
    try:
        d3 = svrf.parse_deck(envmain)
    finally:
        del os.environ["FLOE_SVRF_T"]
    check("INCLUDE $VAR expanded from the environment",
          "SUB" in d3.layers, str(sorted(d3.layers)))
    check("INCLUDE ${VAR} form expanded too",
          not any("FLOE_SVRF_T" in x for x in d3.warnings),
          str(d3.warnings))
    check("unset env var: warned with a hint, not crashed",
          any("FLOE_SVRF_UNSET" in x and "env var unset" in x
              for x in d3.warnings), str(d3.warnings))
    # DMACRO with its body brace on the NEXT line (real-deck style)
    # must not swallow the rest of the deck - the drifted depth hid
    # a whole nested-include tree in the field (2026-08-18)
    n2 = os.path.join(tmp, "inc", "nested2.svrf")
    w(n2, "LAYER NEST2 88\n")
    dm = os.path.join(tmp, "dmacro.svrf")
    w(dm,
      "DMACRO CHK L\n"
      "{\n"
      "  INT L < 0.1\n"
      "}\n"
      "LAYER AFTER 9\n"
      "INCLUDE inc/nested2.svrf\n"
      "DMACRO ONE X { EXT X > 0.2 }\n"
      "LAYER TAIL 10\n")
    d4 = svrf.parse_deck(dm)
    check("next-line-brace DMACRO: deck continues after the body",
          "AFTER" in d4.layers and not d4.checks,
          str((sorted(d4.layers), list(d4.checks))))
    check("nested INCLUDE after the DMACRO processed",
          "NEST2" in d4.layers, str(sorted(d4.layers)))
    check("one-line DMACRO still skipped in place",
          "TAIL" in d4.layers and d4.stats["dmacro"] == 2,
          str((sorted(d4.layers), d4.stats["dmacro"])))
    check("clean deck: no unclosed-state warnings",
          not any("unclosed" in x for x in d4.warnings),
          str(d4.warnings))
    # an INCLUDE textually inside a macro body is reported, and an
    # unclosed body is flagged at end of file
    bad = os.path.join(tmp, "badmacro.svrf")
    w(bad,
      "DMACRO B L\n"
      "{\n"
      "  INCLUDE inc/nested2.svrf\n")
    d5 = svrf.parse_deck(bad)
    check("INCLUDE inside a macro body warned, not resolved",
          any("swallowed" in x for x in d5.warnings)
          and "NEST2" not in d5.layers, str(d5.warnings))
    check("unclosed DMACRO body flagged at end of file",
          any("unclosed DMACRO" in x for x in d5.warnings),
          str(d5.warnings))
    # directive precision (field 2026-08-18): two-arg value tests,
    # comments on directive lines, quoted values, #UNDEFINE
    dv = os.path.join(tmp, "dvals.svrf")
    w(dv,
      "#DEFINE STACK 6LM // metal stack\n"
      "#DEFINE REV \"A0\"\n"
      "#DEFINE WMIN2 0.061 // um\n"
      "#IFDEF STACK 6LM\nLAYER SIX 61\n#ENDIF\n"
      "#IFDEF STACK 7LM\nLAYER SEVEN 71\n#ENDIF\n"
      "#IFNDEF STACK 7LM\nLAYER NOT7 72\n#ENDIF\n"
      "#IFDEF REV A0\nLAYER REVA 73\n#ENDIF\n"
      "CMT.RULE { @ c\n  INT NOT7 < WMIN2\n}\n"
      "#UNDEFINE STACK\n"
      "#IFDEF STACK\nLAYER GONE 74\n#ENDIF\n")
    d6 = svrf.parse_deck(dv)
    check("two-arg #IFDEF: matching value taken",
          "SIX" in d6.layers, str(sorted(d6.layers)))
    check("two-arg #IFDEF: other value skipped",
          "SEVEN" not in d6.layers, str(sorted(d6.layers)))
    check("two-arg #IFNDEF: other value taken",
          "NOT7" in d6.layers, str(sorted(d6.layers)))
    check("quoted define value matches a bare test literal",
          "REVA" in d6.layers and d6.defines.get("REV") == "A0",
          str((sorted(d6.layers), d6.defines)))
    check("directive-line comment not glued into the value",
          d6.checks["CMT.RULE"].constraints[0]["value"] == 0.061,
          str(d6.checks["CMT.RULE"].constraints))
    check("#UNDEFINE removes the switch",
          "GONE" not in d6.layers, str(sorted(d6.layers)))
    dv2 = os.path.join(tmp, "dvals2.svrf")
    w(dv2,
      "#IFDEF STACK 7LM\nLAYER SEVEN 71\n"
      "#ELSE\nLAYER OTHER 79\n#ENDIF\n")
    d7 = svrf.parse_deck(dv2, {"STACK": "7LM"})
    check("-D NAME=VAL satisfies a two-arg test",
          "SEVEN" in d7.layers and "OTHER" not in d7.layers,
          str(sorted(d7.layers)))
    d8 = svrf.parse_deck(dv2, {"STACK": "6LM"})
    check("-D other value takes #ELSE",
          "OTHER" in d8.layers and "SEVEN" not in d8.layers,
          str(sorted(d8.layers)))
    d9 = svrf.parse_deck(dv2, scan_all=True)
    check("--scan records tested switch values",
          d9.switch_values.get("STACK") == ["7LM"],
          str(d9.switch_values))
    check("scan report lists the -D candidates",
          "STACK(7LM)" in svrf.format_scan(d9),
          svrf.format_scan(d9))
    # hybrid decks wrap Tcl in VERBATIM blocks (sfa14 field scan):
    # never checks, bodies brace-skipped, INCLUDEs inside are
    # inventoried and followed only by --scan
    vb = os.path.join(tmp, "verbatim.svrf")
    w(vb,
      "LAYER TOP1 11\n"
      "VERBATIM {\n"
      "  if {[info exists env(DRC_SEL)]} {\n"
      "    puts \"sel\"\n"
      "    INCLUDE inc/nested2.svrf\n"
      "  } else {\n"
      "    set x 1\n"
      "  }\n"
      "}\n"
      "if {[info exists env(X)]} {\n"
      "  puts \"x\"\n"
      "}\n"
      "AFTER.RULE { @ a\n  INT TOP1 < 0.1\n}\n")
    dv0 = svrf.parse_deck(vb)
    check("VERBATIM / top-level Tcl if never become checks",
          list(dv0.checks) == ["AFTER.RULE"],
          str(list(dv0.checks)))
    check("normal parse: verbatim include listed, not followed",
          dv0.verbatim_includes == ["inc/nested2.svrf"]
          and "NEST2" not in dv0.layers
          and any("NOT followed" in x for x in dv0.warnings),
          str((dv0.verbatim_includes, sorted(dv0.layers),
               dv0.warnings)))
    dv1 = svrf.parse_deck(vb, scan_all=True)
    check("--scan follows verbatim includes",
          "NEST2" in dv1.layers, str(sorted(dv1.layers)))
    check("deck continues after the Tcl blocks",
          "AFTER.RULE" in dv1.checks
          and dv1.checks["AFTER.RULE"].constraints[0]["value"]
          == 0.1,
          str(list(dv1.checks)))
    check("scan report shows the verbatim inventory",
          "VERBATIM/Tcl blocks 2" in svrf.format_scan(dv1)
          and "verbatim include inc/nested2.svrf"
          in svrf.format_scan(dv1),
          svrf.format_scan(dv1))   # 2 = VERBATIM + top-level if
    dv2f = svrf.parse_deck(vb, follow_verbatim=True)
    check("--follow-verbatim follows them in the normal parse",
          "NEST2" in dv2f.layers
          and not any("NOT followed" in x for x in dv2f.warnings),
          str((sorted(dv2f.layers), dv2f.warnings)))
    # environment-sourced switches (sourceme workflow): names the
    # deck TESTS fall back to os.environ lazily; -D wins; a hit is
    # promoted (value substitution) and recorded for provenance
    ev = os.path.join(tmp, "envsw.svrf")
    w(ev,
      "#IFDEF FLOE_SW_A\nLAYER EA 21\n#ENDIF\n"
      "#IFDEF FLOE_SW_B 6LM\nLAYER EB 22\n#ENDIF\n"
      "#IFDEF FLOE_SW_B 7LM\nLAYER EB7 23\n#ENDIF\n"
      "#IFNDEF FLOE_SW_W\n#DEFINE FLOE_SW_W 0.05\n#ENDIF\n"
      "E.RULE { @ e\n  INT EA < FLOE_SW_W\n}\n")
    os.environ["FLOE_SW_A"] = ""
    os.environ["FLOE_SW_B"] = "6LM"
    os.environ["FLOE_SW_W"] = "0.077"
    try:
        de = svrf.parse_deck(ev)
        de2 = svrf.parse_deck(ev, {"FLOE_SW_B": "7LM"})
        de3 = svrf.parse_deck(ev, env_switches=False)
    finally:
        for k in ("FLOE_SW_A", "FLOE_SW_B", "FLOE_SW_W"):
            del os.environ[k]
    check("env satisfies one-arg and two-arg #IFDEF",
          "EA" in de.layers and "EB" in de.layers
          and "EB7" not in de.layers, str(sorted(de.layers)))
    check("env value substitutes via the #IFNDEF-guard pattern",
          de.checks["E.RULE"].constraints[0]["value"] == 0.077,
          str(de.checks["E.RULE"].constraints))
    check("-D beats the environment",
          "EB7" in de2.layers and "EB" not in de2.layers,
          str(sorted(de2.layers)))
    check("env_switches=False disables the fallback",
          "EA" not in de3.layers
          and de3.checks["E.RULE"].constraints[0]["value"] == 0.05,
          str((sorted(de3.layers),
               de3.checks["E.RULE"].constraints)))
    check("env provenance recorded in scan + json",
          set(de.env_used) == {"FLOE_SW_A", "FLOE_SW_B",
                               "FLOE_SW_W"}
          and "satisfied from the environment"
          in svrf.format_scan(de)
          and de.to_json()["stats"]["env_switches"]
          .get("FLOE_SW_B") == "6LM",
          str(de.env_used))


def r2(tmp):
    print("[R2] derivation graph closure")
    p = os.path.join(tmp, "graph.svrf")
    w(p,
      "LAYER MAP 31 DATATYPE 0 100\n"
      "LAYER L1 100\n"
      "LAYER L2 2\nLAYER L3 3\n"
      "a = L1 NOT L2\n"
      "b = (a AND L3) SIZE BY 0.01\n"
      "c = b OR a\n"
      "x = y INTERACT c\n"
      "y = x OR c\n"
      "DIAMOND.1 { @ d\n  INT c < 0.1\n}\n"
      "CYCLE.1 { @ c\n  INT x < 0.1\n}\n"
      "LOST.1 { @ l\n  INT nosuch < 0.1\n}\n")
    d = svrf.parse_deck(p)
    dia = d.checks["DIAMOND.1"]
    check("diamond closure reaches all sources + LAYER MAP dt",
          dia.source_gds == [(2, None), (3, None), (31, 0)],
          str(dia.source_gds))
    # wrapped derivations (sfa14 field scan: ~1.5k operator-leading
    # and operator-trailing continuation lines)
    p2 = os.path.join(tmp, "wrap.svrf")
    w(p2,
      "LAYER L1 1\nLAYER L2 2\nLAYER L3 3\nLAYER L9 9\n"
      "w1 = L1\n"
      "    NOT L2\n"
      "w2 = L1 OR\n"
      "    L3\n"
      "w3 = L1\n"
      "    NOT L2\n"
      "    AND L3\n"
      "w4 = L1\n"
      "LAYER LX 8\n"
      "NOT L3\n"
      "W1.RULE { @ a\n  INT w1 < 0.1\n}\n"
      "W2.RULE { @ b\n  INT w2 < 0.1\n}\n"
      "W3.RULE { @ c\n  INT w3 < 0.1\n}\n"
      "W4.RULE { @ d\n  INT w4 < 0.1\n}\n")
    d2 = svrf.parse_deck(p2)
    check("operator-LEADING wrap joins the derivation",
          d2.derived_ops["w1"] == ["L1", "L2"],
          str(d2.derived_ops["w1"]))
    check("operator-TRAILING wrap joins the next line",
          d2.derived_ops["w2"] == ["L1", "L3"],
          str(d2.derived_ops["w2"]))
    check("multi-line wrap keeps extending",
          d2.derived_ops["w3"] == ["L1", "L2", "L3"],
          str(d2.derived_ops["w3"]))
    check("closure includes wrapped operands",
          d2.checks["W1.RULE"].source_gds == [(1, None), (2, None)],
          str(d2.checks["W1.RULE"].source_gds))
    check("no false join across an intervening statement",
          d2.derived_ops["w4"] == ["L1"]
          and d2.unknown.get("NOT", 0) == 1,
          str((d2.derived_ops["w4"], dict(d2.unknown))))
    # spec heads inside checks are classified, not unknown noise
    p3 = os.path.join(tmp, "dfm.svrf")
    w(p3,
      "LAYER L1 1\n"
      "DFM.RULE { @ d\n"
      "  INT L1 < 0.1\n"
      "  DFM RDB ONLY x.rdb\n"
      "  [MIN_VOLTAGE(v) > 1.0]\n"
      "  ~(2.751)\n"
      "}\n")
    d3 = svrf.parse_deck(p3)
    check("DFM/property lines skipped quietly inside checks",
          d3.stats["unknown_in_block"] == 0
          and d3.stats["prop_expr"] == 2
          and len(d3.checks["DFM.RULE"].constraints) == 1,
          str((dict(d3.unknown), d3.stats["prop_expr"])))
    cyc = d.checks["CYCLE.1"]
    check("cycle terminates, sources found through it",
          set(cyc.source_gds) == {(2, None), (3, None), (31, 0)},
          str(cyc.source_gds))
    check("undefined operand -> unresolved",
          d.checks["LOST.1"].unresolved == ["nosuch"])
    check("operators never leak into operands",
          d.derived_ops["b"] == ["a", "L3"], str(d.derived_ops["b"]))


def r3(tmp):
    print("[R3] check extraction")
    p = os.path.join(tmp, "checks.svrf")
    w(p,
      "LAYER M1 31\nLAYER M2 32\n"
      "DMACRO SKIPME arg1 {\n"
      "  INT arg1 < 9.9\n"
      "}\n"
      "\"Q.RULE.1\" { @ first line\n"
      "  @ second line\n"
      "  INT M1 >= 0.05 <= 0.10\n"
      "  EXT M1 M2 <0.03 ABUT<90 SINGULAR REGION\n"
      "  bad = ENC M1 M2 < 0.02\n"
      "  FROBNICATE M1 x\n"
      "  DFM PROPERTY M1 whatever\n"
      "}\n")   # FROBNICATE before DFM: after an ignored head the
               # next unrecognized line counts as its continuation
    d = svrf.parse_deck(p)
    check("quoted check name", "Q.RULE.1" in d.checks,
          str(list(d.checks)))
    c = d.checks["Q.RULE.1"]
    check("multi-@ desc joined",
          c.desc == ["first line", "second line"], str(c.desc))
    bounds = [(x["metric"], x["op"], x["value"])
              for x in c.constraints]
    check("double bound -> two constraints",
          ("width", ">=", 0.05) in bounds
          and ("width", "<=", 0.10) in bounds, str(bounds))
    check("fused op parsed, ABUT<90 not a bound",
          ("space", "<", 0.03) in bounds
          and not any(v == 90 for _, _, v in bounds), str(bounds))
    check("assignment-measurement records constraint",
          ("enclosure", "<", 0.02) in bounds, str(bounds))
    check("operands collected in order",
          c.layers == ["M1", "M2"], str(c.layers))
    check("unknown in-block statement counted, not fatal",
          d.stats["unknown_in_block"] == 1
          and "FROBNICATE" in d.unknown
          and "DFM" not in d.unknown)   # DFM = quietly ignored now
    check("DMACRO body skipped (no SKIPME artifacts)",
          "SKIPME" not in d.checks and d.stats["dmacro"] == 1
          and not any(x["value"] == 9.9
                      for ch in d.checks.values()
                      for x in ch.constraints))


def r3b(tmp):
    print("[R3b] real-world expression forms")
    cases = [
        ("abut range not a bound",
         "INT m1 < 0.05 ABUT>0<90 SINGULAR REGION",
         [("width", "<", 0.05)]),
        ("option value not a bound",
         "EXT a b < 0.1 OPPOSITE EXTENDED < 0.05",
         [("space", "<", 0.1)]),
        ("zero-lower chain stops at options",
         "EXT a b > 0 < 0.1 ABUT>0<90",
         [("space", ">", 0.0), ("space", "<", 0.1)]),
        ("leading-dot value",
         "INT m1 < .05", [("width", "<", 0.05)]),
        ("wrapped bound line",
         "INT m1\n  < 0.05 ABUT>0<90", [("width", "<", 0.05)]),
        ("wrapped twice",
         "INT m1\n  >= 0.05\n  <= 0.10",
         [("width", ">=", 0.05), ("width", "<=", 0.10)]),
    ]
    for label, stmt, want in cases:
        p = os.path.join(tmp, "expr.svrf")
        w(p, "LAYER m1 1\nLAYER a 2\nLAYER b 3\n"
             "X.1 { @ d\n  %s\n}\nY.1 { @ y\n  INT m1 < 0.9\n}\n"
             % stmt.replace("\n", "\n  "))
        d = svrf.parse_deck(p)
        got = [(c["metric"], c["op"], c["value"])
               for c in d.checks["X.1"].constraints]
        ynext = [(c["op"], c["value"])
                 for c in d.checks["Y.1"].constraints]
        check(label, got == want and ynext == [("<", 0.9)],
              "got=%s next=%s" % (got, ynext))
    p = os.path.join(tmp, "leak.svrf")
    w(p, "LAYER m1 1\nX.1 { @ d\n  INT m1\n}\n"
         "Z.1 { @ z\n  ENC m1 m1 < 0.2\n}\n")
    d = svrf.parse_deck(p)
    check("boundless wrap never leaks past the block close",
          d.checks["X.1"].constraints == []
          and len(d.checks["Z.1"].constraints) == 1
          and d.stats["meas_no_bound"] == 1)
    # /* */ banners, measurement operator-wraps, ignored-statement
    # continuations (sfa14 re-scan 2026-08-18)
    p = os.path.join(tmp, "wrap2.svrf")
    w(p,
      "/**********************************\n"
      " * D_DOC_LINE : looks like a head *\n"
      " CHK_DOC also a fake head\n"
      " **********************************/\n"
      "LAYER m1 1 /* inline comment */\n"
      "LAYER m2 2\n"
      "W.1 { @ w\n"
      "  EXT m1 m2\n"
      "    NOT m1\n"
      "    < 0.05\n"
      "  DFM RDB ONLY out.rdb\n"
      "    D_ARGWRAP more args\n"
      "}\n"
      "NET AREA RATIO m1 m2 > 400\n"
      "FLATTEN\n")
    d = svrf.parse_deck(p)
    c = d.checks["W.1"]
    check("/* */ banner and inline comments stripped",
          not d.unknown and "m1" in d.layers,
          str((dict(d.unknown), sorted(d.layers))))
    check("operator-wrapped measurement keeps its operands",
          c.layers == ["m1", "m2"], str(c.layers))
    check("comparator after an operator-wrap line still binds",
          [(x["op"], x["value"]) for x in c.constraints]
          == [("<", 0.05)], str(c.constraints))
    check("ignored-statement continuation classified quietly",
          d.stats["unknown_in_block"] == 0, str(dict(d.unknown)))
    check("NET/FLATTEN ignored", d.stats["unknown"] == 0,
          str(dict(d.unknown)))
    p = os.path.join(tmp, "opencmt.svrf")
    w(p, "LAYER m1 1\n/* never closed\nLAYER m2 2\n")
    d = svrf.parse_deck(p)
    check("unclosed /* comment warned, prior layers kept",
          "m1" in d.layers and "m2" not in d.layers
          and any("unclosed /*" in x for x in d.warnings),
          str((sorted(d.layers), d.warnings)))


def r4(tmp):
    print("[R4] end-to-end vs gen_drcdb --svrf")
    db = os.path.join(tmp, "e2e.db")
    deck = os.path.join(tmp, "e2e.svrf")
    gen = os.path.join(os.path.dirname(__file__), "gen_drcdb.py")
    subprocess.run([sys.executable, gen, db, "--checks", "60",
                    "--max-errors", "40", "--zeros", "5",
                    "--svrf", deck],
                   check=True, stdout=subprocess.DEVNULL)
    d = svrf.parse_deck(deck)
    ddb = drc.load_ascii(db)
    names = {c.name for c in ddb.checks}
    meta = set(d.checks)
    check("every db check in the sidecar", names <= meta,
          str(sorted(names - meta)[:5]))
    check("no phantom deck checks", meta <= names,
          str(sorted(meta - names)[:5]))
    bad = []
    for ci, c in enumerate(ddb.checks):
        mc = d.checks[c.name]
        want = float("%.3f" % (0.02 + (ci % 40) * 0.005))
        vals = [x["value"] for x in mc.constraints]
        if want not in vals:
            bad.append((c.name, want, vals))
        if not mc.source_gds:
            bad.append((c.name, "no source_gds"))
    check("constraint values match the generator formula "
          "and every check reaches a gds source",
          not bad, str(bad[:3]))
    d1 = svrf.parse_deck(deck, {"SYNTH_EXTRA": None})
    check("-D SYNTH_EXTRA adds exactly EXTRA.CHECK.1",
          set(d1.checks) - set(d.checks) == {"EXTRA.CHECK.1"})
    out = os.path.join(tmp, "e2e.rules.json")
    svrf.write_json(d, out)
    data = svrf.load_rules(out)
    check("json round-trip (format/version/check count)",
          data["format"] == svrf.FORMAT
          and len(data["checks"]) == len(d.checks))
    gds = data["checks"][ddb.checks[0].name]["source_gds"]
    check("json source_gds serialized as pairs",
          gds and all(len(p) == 2 for p in gds), str(gds))


def r5(tmp):
    print("[R5] the command")
    p = os.path.join(tmp, "fmt.svrf")
    w(p,
      "LAYER MAP 7 DATATYPE 2 100\n"
      "LAYER M1 100\nLAYER M2 5\nLAYER M3 9.1\n"
      "VARIABLE TINY 0.00001\nVARIABLE BIG 10000000000000000\n"
      "VARIABLE WORD some words\n"
      "F.1 { @ caf\u00e9 \"quoted\" back\\slash \u6e2c\u8a66\n"
      "  INT M1 < TINY\n  EXT M1 M2 > BIG < 0.1\n  ENC M2 M3 < WORD\n}\n")
    out = os.path.join(tmp, "fmt.json")
    res = run([p, "-o", out])
    check("builds (rc 0) and reports the counts",
          res.returncode == 0 and "1 checks" in res.stdout,
          res.stdout + res.stderr)
    with open(out, "rb") as f:
        raw = f.read()
    data = json.loads(raw)
    want = (json.dumps(data, indent=1, sort_keys=True) + "\n").encode()
    check("sidecar bytes = Python json.dump(indent=1, sort_keys=True)",
          raw == want, "differs")
    check("ASCII only, floats as Python repr",
          raw.isascii() and b"1e-05" in raw and b"1e+16" in raw
          and b"\\u00e9" in raw, raw[:400])
    check("generated_by names floe-index",
          data["generated_by"].startswith("floe-index "),
          data["generated_by"])
    c = data["checks"]["F.1"]
    check("null datatype and LAYER MAP pair serialized",
          data["layers"]["M2"] == [[5, None]]
          and data["layers"]["M1"] == [[7, 2]]
          and data["layers"]["M3"] == [[9, 1]], str(data["layers"]))
    check("unresolvable bound keeps its raw token",
          any(x.get("raw") == "WORD" and x["value"] is None
              for x in c["constraints"]), str(c["constraints"]))
    d = os.path.join(tmp, "dflt.svrf")
    shutil.copyfile(p, d)
    res = run([d])
    check("default output <deck>.rules.json",
          res.returncode == 0 and os.path.isfile(d + ".rules.json"),
          res.stderr)
    s = os.path.join(tmp, "scan.svrf")
    shutil.copyfile(p, s)
    res = run([s, "--scan"])
    check("--scan prints the inventory and writes nothing",
          res.returncode == 0 and res.stdout.startswith("deck scan: ")
          and "measurements: " in res.stdout
          and not os.path.exists(s + ".rules.json"), res.stdout)
    sw = os.path.join(tmp, "sw.svrf")
    w(sw, "#IFDEF A\nLAYER HAS_A 1\n#ENDIF\n"
          "#IFDEF B 7LM\nLAYER HAS_B 2\n#ENDIF\n")
    forms = [["-DA", "-DB=7LM"], ["-D=A", "-D", "B=7LM"],
             ["--define=A", "--define", "B=7LM"]]
    seen = []
    for k, f in enumerate(forms):
        o = os.path.join(tmp, "sw%d.json" % k)
        r = run([sw, "-o", o, "--no-env-switches"] + f)
        seen.append(sorted(reader.load_rules(o)["layers"])
                    if r.returncode == 0 else r.stderr)
    check("-DNAME / -D=NAME / --define=NAME are -D NAME",
          seen == [["HAS_A", "HAS_B"]] * 3, str(seen))
    missing = os.path.join(tmp, "nosuch.svrf")
    r = run([missing])
    check("a missing deck exits 1 and writes nothing",
          r.returncode == 1 and not os.path.exists(missing + ".rules.json"),
          r.stderr)
    r = run([p, "--frobnicate"])
    check("an unknown option exits 2", r.returncode == 2, r.stderr)
    r = subprocess.run([FLOE2, "svrf", p,
                        "-o", os.path.join(tmp, "old.json"), "-D", "X=1"],
                       capture_output=True, text=True, cwd=ROOT)
    check("floe2 svrf points to floe-index and exits 2",
          r.returncode == 2
          and "floe-index svrf %s -o %s -D X=1"
          % (p, os.path.join(tmp, "old.json")) in r.stderr
          and not os.path.exists(os.path.join(tmp, "old.json")),
          r.stderr)
    st = svrf.parse_deck(p)
    check("builder operator words == the viewer's (rhs_operands)",
          st.keywords == reader.KEYWORDS,
          str(sorted(st.keywords ^ reader.KEYWORDS)))
    check("the viewer splits a derivation as the builder did",
          reader.rhs_operands("(a AND L3) SIZE BY 0.01 NOT x.y")
          == ["a", "L3", "x.y"])


def main():
    if not os.path.isfile(BIN):
        print("missing %s: cd rust && cargo build --release" % BIN)
        sys.exit(2)
    with tempfile.TemporaryDirectory() as tmp:
        work = os.path.join(tmp, "builder")
        os.makedirs(work)
        BuilderView._tmp = work
        r1(tmp)
        r2(tmp)
        r3(tmp)
        r3b(tmp)
        r4(tmp)
        r5(tmp)
    print("validate_svrf:", "FAIL" if FAIL else "all green")
    sys.exit(FAIL)


if __name__ == "__main__":
    main()
