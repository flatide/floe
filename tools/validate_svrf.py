"""SVRF subset-parser gate (floe/svrf.py -> .rules.json sidecar).

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

usage: .venv/bin/python tools/validate_svrf.py
"""

import os
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from floe import drc, svrf  # noqa: E402

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
      "  DFM PROPERTY M1 whatever\n"
      "}\n")
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
          d.stats["unknown_in_block"] == 1 and "DFM" in d.unknown)
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


def main():
    with tempfile.TemporaryDirectory() as tmp:
        r1(tmp)
        r2(tmp)
        r3(tmp)
        r3b(tmp)
        r4(tmp)
    print("validate_svrf:", "FAIL" if FAIL else "all green")
    sys.exit(FAIL)


if __name__ == "__main__":
    main()
