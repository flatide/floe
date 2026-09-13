#!/usr/bin/env python3
"""Native SVRF converter vs every original R1-R4 parse invocation + faults.

All decks are synthetic private fixtures. The actual floe.svrf parser is the
oracle, not a second implementation. Native PATH is empty; no Python fallback,
indexer, renderd, GUI, server, proprietary deck or external service is used.
"""
import inspect
import json
import os
from pathlib import Path
import random
import signal
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from floe import svrf
import validate_svrf as original_gate

APP = ROOT / "rust/target/release/floe2-web"
ORIGINAL = svrf.parse_deck
SIGNATURE = inspect.signature(ORIGINAL)
TESTED = 0


def run(args, ok=True, cwd=None):
    env = dict(os.environ, PATH="", FLOE_INDEX_BIN="/no/indexer",
               FLOE_RENDERD_BIN="/no/renderer")
    p = subprocess.run([str(APP), "svrf", *map(str, args)], cwd=cwd,
                       env=env, capture_output=True, text=True, timeout=30)
    assert (p.returncode == 0) == ok, (args, p.returncode, p.stdout, p.stderr)
    return p


def compare(*args, **kwargs):
    global TESTED
    expected = ORIGINAL(*args, **kwargs)
    b = SIGNATURE.bind(*args, **kwargs)
    b.apply_defaults()
    opts = b.arguments
    path = opts["path"]
    flags = []
    for name, value in (opts["defines"] or {}).items():
        flags += ["-D", name + ("=" + value if value is not None else "")]
    for d in opts["include_dirs"]:
        flags += ["-I", d]
    if opts["follow_verbatim"]:
        flags.append("--follow-verbatim")
    if not opts["env_switches"]:
        flags.append("--no-env-switches")
    if opts["scan_all"]:
        native = run([path, *flags, "--scan"])
        assert native.stdout == svrf.format_scan(expected) + "\n", (
            path, native.stdout, svrf.format_scan(expected))
    else:
        out = str(path) + ".native.rules.json"
        native = run([path, *flags, "--out", out])
        actual = json.loads(Path(out).read_text())
        reference = expected.to_json()
        assert actual.pop("generated_by").startswith("floe2-web ")
        reference.pop("generated_by")
        assert actual == reference, (path, actual, reference)
        assert native.stdout == "%s: %d checks, %d derivations, %d layers -> %s\n" % (
            path, len(expected.checks), len(expected.derived), len(expected.layers), out)
        # Every normal case also exercises native --scan (both branches),
        # compared with a fresh original parser under precisely those flags.
        scan_opts = dict(opts, scan_all=True)
        scan_expected = ORIGINAL(**scan_opts)
        scanned = run([path, *flags, "--scan"])
        assert scanned.stdout == svrf.format_scan(scan_expected) + "\n", (
            path, scanned.stdout, svrf.format_scan(scan_expected))
    TESTED += 1
    return expected


def additional(work):
    deck = work / "한 글.svrf"
    deck.write_bytes(b"LAYER M 7\rR {\r\n@\n@\xff\rINT M < .5\n}")
    compare(str(deck), env_switches=False)
    deck.write_text("LAYER M ٧\nLAYER N ３.２\nLAYER MAP ９ DATATYPE ٠ ３ １０٠\n"
                    "VARIABLE V .٠٣١\nR { INT M N < V }\n")
    compare(str(deck), env_switches=False)
    # Alias include cycles, path expansion and include-dir precedence.
    inc = work / "lookup dir"
    inc.mkdir()
    included = inc / "child"
    included.write_text("LAYER M 9\nR { INT M < .05 }\n")
    alias = work / "alias"
    alias.symlink_to(deck.name)
    deck.write_text('INCLUDE "alias"\nINCLUDE "child"\nINCLUDE "${NATIVE_SVRF_DIR}/child"\n')
    os.environ["NATIVE_SVRF_DIR"] = str(inc)
    try:
        compare(str(deck), include_dirs=[str(inc)])
    finally:
        del os.environ["NATIVE_SVRF_DIR"]
    # Precise decimal grammar, punctuation and Unicode word boundaries, not
    # recursive defines. Longest name wins; quoted @ keeps comments literal.
    deck.write_text("#DEFINE W .5\n#DEFINE W-X .25\nLAYER M 7\n"
                    "R { @ \"한글 // /* W\"\nINT M < W-X\n"
                    "INT M < W\nINT 한W W한 W_2 < UNKNOWN\n}\n"
                    "#DEFINE NEXT {\nT NEXT INT M > -1e-3 <= +.25 }\n")
    compare(str(deck))
    # Deterministic mixed statement streams catch parser-state interactions,
    # not just hand-selected valid snippets. Unknowns remain diagnostics.
    rng = random.Random(361)
    statements = [
        "#DEFINE W .05", "#UNDEFINE W", "#IFDEF W", "#ELSE", "#ENDIF",
        "LAYER M 7", "LAYER N 8.2", "LAYER MAP 9 DATATYPE 0 3 100", "LAYER Q 100",
        "VARIABLE V .03", "VARIABLE RAW words remain", "A = M OR B", "B = A NOT N",
        "AND Q", "DRC CHECK MAP x", "wrapped argument", "[expr x]", "UNKNOWN name",
        "R { INT A >0< W ABUT>0<90 }", "R { @ literal /* // W", "INT M < V", "> 0 <= .2",
        "NOT N", "AREA Q >= 4", "COPY B", "}", "} S { INT B != 0 }", "// comment",
        "VERBATIM {", "if {[info exists foo]} {", "}", "CMACRO x", "DMACRO x { }",
        "LAYER invalid", "LAYER empty nope", "LAYER Z 100 7.2 3 3", "LAYER MAP invalid",
    ]
    for _ in range(100):
        deck.write_text("\n".join(rng.choices(statements, k=60)) + "\n")
        compare(str(deck), env_switches=False)


def failures(work):
    deck = work / "source.svrf"
    old = "OLD SIDECAR\n"
    out = work / "protected.json"
    out.write_text(old)
    for content in ["R { INT M < 1e999 }\n", "LAYER M " + "9"*100 + "\n",
                    "x"*65537, "#DEFINE X " + "v"*40000 + "\nX X\n"]:
        deck.write_text(content)
        p = run([deck, "-o", out], ok=False)
        assert p.returncode in (2, 3), p.stderr
        assert out.read_text() == old
    run([work / "missing", "-o", out], ok=False)
    fifo = work / "fifo"
    os.mkfifo(fifo)
    run([fifo, "-o", out], ok=False)
    # Cycles warn; deep acyclic recursion fails rather than overflowing stack.
    for n in range(65):
        (work / ("inc%d" % n)).write_text("INCLUDE inc%d\n" % (n+1) if n < 64 else "LAYER M 7\n")
    run([work / "inc0", "-o", out], ok=False)
    deck.write_text("LAYER M 7\nR { INT M < .05 }\n")
    before = deck.read_bytes()
    run([deck, "-o", deck], ok=False)
    sym = work / "link"
    sym.symlink_to(out.name)
    run([deck, "-o", sym], ok=False)
    hard = work / "hard"
    os.link(out, hard)
    run([deck, "-o", hard], ok=False)
    hard.unlink()
    cache = work / "design.floe"
    cache.mkdir()
    run([deck, "-o", cache / "meta.json"], ok=False)
    include = work / "included"
    include.write_text("LAYER M 8\n")
    deck.write_text('INCLUDE "included"\n')
    run([deck, "-o", include], ok=False)
    assert include.read_text() == "LAYER M 8\n"
    # Even a missing include candidate must not be created by the converter.
    deck.write_text('INCLUDE "future"\n')
    run([deck, "-o", work / "future"], ok=False)
    assert not (work / "future").exists()
    deck.write_bytes(before)
    run([deck, "-o", work / "missing-parent" / "out"], ok=False)
    assert out.read_text() == old
    assert not list(work.glob(".floe-shot-*.tmp"))
    for args in [[], [deck, "--scan=1"], [deck, "--out"], [deck, "--unknown"],
                 [deck, "extra"], [deck, "--out="], [deck, "--follow-verbatim=1"]]:
        run(args, ok=False)
    # Attached shorts, default path, --scan never writes even when -o is root.
    run([deck.name, "-DA=.1", "-I.", "-onative.json"], cwd=work)
    run([deck, "--scan", "-o", deck])
    assert deck.read_bytes() == before
    run([deck])
    assert Path(str(deck) + ".rules.json").is_file()
    # Both cancellation signals while a large ordinary deck is being read.
    # Comments cost input bytes, not metadata budget, and cannot write output.
    with deck.open("wb") as f:
        for _ in range(192):
            f.write((b"// " + b"x"*508 + b"\n") * 1024)
    for sig in (signal.SIGINT, signal.SIGTERM):
        p = subprocess.Popen([str(APP), "svrf", str(deck), "-o", str(out)],
                             env=dict(os.environ, PATH=""), stdout=subprocess.PIPE,
                             stderr=subprocess.PIPE)
        time.sleep(.04)
        assert p.poll() is None, "signal fixture finished before cancellation"
        p.send_signal(sig)
        stdout, stderr = p.communicate(timeout=10)
        assert p.returncode == 128 + sig, (p.returncode, stdout, stderr)
        assert out.read_text() == old
        assert not list(work.glob(".floe-shot-*.tmp"))


def main():
    with tempfile.TemporaryDirectory(prefix="floe-native-svrf-") as td:
        work = Path(td)
        # Retain all original assertions. The wrapper observes each invocation
        # under that test's exact temporary environment and returns its Deck.
        svrf.parse_deck = compare
        try:
            for stage in (original_gate.r1, original_gate.r2, original_gate.r3,
                          original_gate.r3b, original_gate.r4):
                stage(str(work))
        finally:
            svrf.parse_deck = ORIGINAL
        assert original_gate.FAIL == 0
        additional(work)
        failures(work)
    print("NATIVE SVRF ALL OK (%d full parser/scan oracle cases + faults)" % TESTED)


if __name__ == "__main__":
    main()
