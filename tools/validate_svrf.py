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
  R4  end-to-end vs gen_drcdb --svrf: every db check name resolves
      in the sidecar, constraint values match the generator formula,
      every check reaches a source gds layer; -D SYNTH_EXTRA adds
      exactly the EXTRA.CHECK.1 rule.

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
        r4(tmp)
    print("validate_svrf:", "FAIL" if FAIL else "all green")
    sys.exit(FAIL)


if __name__ == "__main__":
    main()
