#!/usr/bin/env python3
"""Native rules.json/detail parity against real Python parser/GUI methods.

No GTK/display import: extract ONLY the existing pure detail/measurement
methods from gui.py, so the oracle tracks legacy changes without copying its
geometry implementation. All inputs are private, synthetic, and read-only
during native tests. Rust runs with PATH empty (no Python fallback).
"""
import ast
from fractions import Fraction
import json
import math
import os
from pathlib import Path
import random
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from floe import drc, svrf
from validate_app_drc import fingerprint

APP = ROOT / "rust/target/release/floe2-web"
INDEX = ROOT / "rust/target/release/floe-index"
METRICS = ("width", "space", "enclosure", "area", "density", "length",
           "angle", "perimeter", "vertex", "other")


def legacy_class():
    tree = ast.parse((ROOT / "floe/gui.py").read_text())
    names = {"_drc_measured", "_drc_cd_ruler", "_drc_meta_lines"}
    methods = [n for n in ast.walk(tree) if isinstance(n, ast.FunctionDef) and n.name in names]
    assert len(methods) == len(names)
    scope = dict(math=math, drc_mod=drc, __package__="floe")
    exec(compile(ast.Module(body=methods, type_ignores=[]), "gui.py oracle", "exec"), scope)
    return type("LegacyDetail", (), {name: scope[name] for name in names})


def run(args, env=None, ok=True):
    r = subprocess.run([str(a) for a in args], env=env, text=True,
                       capture_output=True, timeout=30)
    assert (r.returncode == 0) == ok, (args, r.stdout, r.stderr)
    return r


def geometry_cases():
    cases = [
        ("p", [(0, 0), (3000, 0), (3000, 5000), (0, 5000)]),
        ("p", [(0, 5000), (3000, 5000), (3000, 0), (0, 0)]),
        ("p", [(0, 0), (3000, 0), (0, 4000)]),
        ("p", [(0, 0), (3000, 5000), (3000, 0), (0, 5000)]),
        ("p", [(0, 0), (0, 0), (0, 0)]),
        ("e", [(0, 0), (3000, 4000)]),
        ("e", [(0, 0), (0, 0)]),
        ("e", [(0, 0), (1000, 0), (2000, 1000), (3000, 1000)]),
        ("e", [(0, 0), (1000, 0), (0, 500), (1000, 500)]),
        ("e", [(0, 0), (1000, 0), (500, -500), (500, 500)]),
        ("e", [(0, 0), (1000, 0), (500, 0), (1500, 0)]),
    ]
    rng = random.Random(42)
    for _ in range(24):
        cases.append(("e", [(rng.randint(-5000, 5000), rng.randint(-5000, 5000)) for _ in range(4)]))
    cases += [(k, [(x+123456, y-987654) for x, y in pts]) for k, pts in cases]
    return cases


def custom_fixture(work, fractional=False):
    stem = "소수 좌표" if fractional else "한 글"
    deck, db = work / (stem + ".svrf"), work / (stem + ".db")
    source = "LAYER M1 7\nLAYER MAP 8 DATATYPE 3 108\nLAYER M2 108\n"
    source += "A = M1 OR B\nB = C NOT M2\nC = D AND E\nD = E\nE = F\nF = G\nG = A OR MISSING\n"
    rules = [(m.upper(), [s]) for s, m in svrf.MEAS.items() if s not in ("INT", "EXT", "ENC")]
    rules += [("RANGE", ["INT A > 0 < 5"]),
              ("MULTI", ["DENSITY A < 0.5", "AREA A > 0", "INT A <= 4"]),
              ("UNRESOLVED", ["INT A < UNKNOWN_LIMIT"]),
              ("OTHER", ["COPY A"])]
    # Expand bare metric statement heads. Every geometry/metric combination is
    # tested, including unsupported and degenerate cases returning null.
    for name, statements in rules:
        stmts = [s + " A < 5" if " " not in s else s for s in statements]
        source += name + " { @ synthetic " + name + "\n  " + "\n  ".join(stmts) + "\n}\n"
    deck.write_text(source)
    cases = geometry_cases()
    if fractional:
        cases = [(k, [(x / 8 + 0.0625, y / 8 - 0.1875) for x, y in pts])
                 for k, pts in cases]
    coord = lambda v: format(v, ".17g")
    text = "TEST 1000\n"
    for name in [r[0] for r in rules] + ["UNMATCHED", "RANGE"]:
        text += "%s\n%d %d 1\nsynthetic\n" % (name, len(cases), len(cases))
        for i, (kind, pts) in enumerate(cases):
            text += "%s %d %d\n" % (kind, i+1, len(pts) if kind == "p" else len(pts)//2)
            if kind == "p":
                text += "".join(" ".join(map(coord, p)) + "\n" for p in pts)
            else:
                text += "".join(" ".join(map(coord, (*pts[j], *pts[j+1]))) + "\n"
                                for j in range(0, len(pts), 2))
    db.write_text(text)
    return db, deck


def expected_comparison(legacy, check, error):
    candidates = []
    for i, c in enumerate((check or {}).get("constraints", [])):
        if c["value"] is None:
            continue
        value = legacy._drc_measured(error, c["metric"])
        if value is not None:
            candidates.append((i, c, value))
    if not candidates:
        return None
    i, c, value = next((v for v in candidates if v[1]["op"] in ("<", "<=", "==")), candidates[0])
    delta, bound = value - c["value"], c["value"]
    return dict(constraint=i, metric=c["metric"], op=c["op"],
                unit="um2" if c["metric"] == "area" else "um", measured=value,
                bound=bound, delta=delta, percent=delta/bound*100 if bound else None)


def main():
    Legacy = legacy_class()
    with tempfile.TemporaryDirectory(prefix="floe-svrf-read-") as td:
        work = Path(td)
        generated, deck = work / "generated.db", work / "generated.svrf"
        run([sys.executable, ROOT / "tools/gen_drcdb.py", generated,
             "--checks", "20", "--max-errors", "8", "--zeros", "2", "--svrf", deck])
        pairs = [(*custom_fixture(work), True), (generated, deck, True),
                 (*custom_fixture(work, fractional=True), False)]
        env = dict(os.environ, PATH="", FLOE_REVIEWER="svrf-oracle", TMPDIR=str(work))
        tested = 0
        for db, deck, packed in pairs:
            if packed:
                run([INDEX, "drc", db, "--jobs", "2"])
            side = Path(str(deck) + ".rules.json")
            svrf.write_json(svrf.parse_deck(str(deck)), str(side))
            meta = svrf.load_rules(str(side))
            legacy = Legacy()
            legacy._drc_rmeta = meta
            pack = drc.IcePack(str(db) + ".ice") if packed else drc.load_ascii(str(db))
            legacy.dbu = 1.0 / pack.precision
            before = fingerprint(work)
            rows = json.loads(run([APP, "drc", db, "--rules", "--svrf-rules", side], env).stdout)
            plain = json.loads(run([APP, "drc", db, "--rules"], env).stdout)
            assert [{k: v for k, v in r.items() if k != "svrf"} for r in rows] == plain
            seen = set()
            for row, ch in zip(rows, pack.checks):
                rule = meta["checks"].get(ch.name)
                if rule is None:
                    assert row["svrf"] is None
                else:
                    detail = row["svrf"]
                    expected = {k: rule.get(k, "" if k == "desc" else []) for k in
                                ("desc", "constraints", "layers", "source_gds", "unresolved")}
                    assert detail["rule"] == expected, (ch.name, detail, expected)
                    types = set(c["metric"] for c in rule["constraints"] if c["metric"]) or {"other"}
                    assert detail["metrics"] == [m for m in METRICS if m in types] + sorted(types - set(METRICS))
                    if ch.errors:
                        lines = legacy._drc_meta_lines(ch.name, ch.errors[0])
                        derived = [] if "derivation:" not in lines else lines[lines.index("derivation:")+1:]
                        actual = ["  %s = %s" % (d["name"], d["rhs"]) for d in detail["derivations"]]
                        if detail["derivations_more"]:
                            actual += ["  …"]
                        assert derived == actual, (ch.name, derived, actual)
                if ch.name in seen:
                    continue   # CLI explicitly picks the first duplicate.
                seen.add(ch.name)
                errors = json.loads(run([APP, "drc", db, "--errs", ch.name, "--svrf-rules", side], env).stdout)
                assert len(errors) == len(ch.errors)
                for error, actual in zip(ch.errors, errors):
                    expected = expected_comparison(legacy, rule, error)
                    got = actual["comparison"]
                    if expected is not None and expected["metric"] == "area":
                        # Legacy shoelace multiplies absolute f64 coordinates.
                        # Check its floating-point error envelope separately;
                        # Rust must match the source's area, not reproduce
                        # cancellation. ASCII floats must NOT be rounded into
                        # pack integers; use exact rational values of the
                        # parsed floating-point coordinates as their oracle.
                        pts = ([(round(x*pack.precision), round(y*pack.precision))
                                for x, y in error.pts] if packed else
                               [(Fraction(x), Fraction(y)) for x, y in error.pts])
                        twice = sum(x*y1-x1*y for (x, y), (x1, y1) in zip(pts, pts[1:]+pts[:1]))
                        exact = (abs(twice)*0.5/pack.precision/pack.precision if packed
                                 else float(abs(twice)/2))
                        products = [abs(x*y1) + abs(x1*y) for (x, y), (x1, y1) in
                                    zip(error.pts, error.pts[1:]+error.pts[:1])]
                        envelope = len(pts)*math.ulp(max(products, default=0.0))*4
                        assert abs(expected["measured"]-exact) <= max(envelope, 1e-15)
                        expected["measured"] = exact
                        expected["delta"] = exact - expected["bound"]
                        expected["percent"] = expected["delta"]/expected["bound"]*100 if expected["bound"] else None
                    if expected is None:
                        assert got is None, (ch.name, actual)
                    else:
                        assert set(got) == set(expected)
                        for key, value in expected.items():
                            if isinstance(value, float):
                                assert math.isclose(got[key], value, rel_tol=1e-9, abs_tol=1e-9), (ch.name, key, got, expected)
                            else:
                                assert got[key] == value, (ch.name, key, got, expected)
                    tested += 1
            assert len(rows) == len(pack.checks)
            assert fingerprint(work) == before, "native metadata/measurement modified input files"
            if packed:
                pack.close()
            else:
                assert not Path(str(db) + ".ice").exists(), "ASCII fallback built a pack"
        # Non-regular, large, malformed, missing and future sidecars must fail
        # promptly, without changing any existing input or emitting valid rows.
        db, _, _ = pairs[0]
        bad = work / "bad.rules.json"
        fifo = work / "fifo.rules.json"
        os.mkfifo(fifo)
        for path in (fifo, work, work / "missing"):
            r = run([APP, "drc", db, "--rules", "--svrf-rules", path], env, False)
            assert not r.stdout
        for contents in ('{"format":"floe-svrf-rules","version":2,"checks":{}}',
                         '{"format":"floe-svrf-rules","version":1,"checks":{"r":{},"r":{}}}',
                         '{"format":"wrong","version":1,"checks":{}}', '\ufffd'):
            bad.write_text(contents)
            before = fingerprint(work)
            r = run([APP, "drc", db, "--rules", "--svrf-rules", bad], env, False)
            assert not r.stdout and fingerprint(work) == before
        with bad.open("wb") as out:
            out.truncate(16*1024*1024+1)
        r = run([APP, "drc", db, "--rules", "--svrf-rules", bad], env, False)
        assert not r.stdout and "16 MiB" in r.stderr
        assert tested > 1800, tested
        print("RUST SVRF METADATA: ALL OK (%d pack/fractional ASCII comparisons, Python GUI oracle, PATH empty)" % tested)


if __name__ == "__main__":
    main()
