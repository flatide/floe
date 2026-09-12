#!/usr/bin/env python3
"""Jobdeck Rust migration oracle. Synthetic decks, never customer files.

Phase 3a validates the public parser/coordinate API. CLI, real source header
probing and render-spec parity are separate subsequent stages, not implied.
"""
from dataclasses import asdict
import json
import os
from pathlib import Path
import random
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from floe.jobdeck import parser, geom
import validate_jobdeck as fixtures


def main():
    with tempfile.TemporaryDirectory(prefix="floe-app-deck-model-") as td:
        work = Path(td)
        cases = []

        def add(name, text, dbus=None, bad=None, selections=(None,), hand=False):
            path = work / (name + " 한 글.jb")
            if isinstance(text, bytes):
                path.write_bytes(text)
            else:
                path.write_text(text)
            deck = parser.parse_jobdeck(str(path), strict=False)
            row = {"path": str(path), "report": deck.report(),
                   "chips": [asdict(c) for c in deck.chips],
                   "sources": deck.sources(),
                   "instance_count": sum(len(c.rows) * len(c.entries) for c in deck.chips),
                   "plans": []}
            if hand:
                row["hand"] = fixtures.EXPECTED
            if dbus is not None:
                for selected in selections:
                    for cross, by_chip, skip in ((True, False, False), (True, True, True), (False, False, True)):
                        options = {"dbus": dbus, "bad": bad or {}, "selection": selected,
                                   "cross": cross, "by_chip": by_chip, "skip": skip}
                        try:
                            pl, st = geom.plan(deck, dbus, ids=selected, cross=cross,
                                               by_chip=by_chip, bad=bad or {},
                                               missing=geom.MISSING_SKIP if skip else geom.MISSING_RAISE)
                        except KeyError:
                            options["error"] = True
                        else:
                            lookup = st.pop("out_of")
                            options.update(error=False, placements=[asdict(p) for p in pl],
                                           stats=st, outputs=[lookup[p.chip, p.idx, p.ly, p.dt] for p in pl])
                        row["plans"].append(options)
            cases.append(row)

        for name in dir(fixtures):
            if name.endswith("DECK") and isinstance(getattr(fixtures, name), str):
                add(name, getattr(fixtures, name))
        add("hand", fixtures.DECK, fixtures.EXPECTED["source_dbu"],
            selections=(None, [], [1], [3], [1, 5]), hand=True)
        add("unsupported", fixtures.FORMAT_DECK, {"chipA.oas": 5e-5},
            bad={"chipA.gds": ["unsupported", "GDS", "probe"],
                 "chipA.oas.gz": ["unsupported", "gzip", "probe"],
                 "chipA.gds.gz": ["unsupported", "gzip GDS", "probe"],
                 "junk.bin": ["unknown_format", "unknown", "probe"],
                 "absent.oas": ["missing", "file not found", "probe"]},
            selections=(None, [1], [8]))
        add("diagnostics", """* sample.jb
CUSTOM unknown header
OPTION PA, CUSTOM=one, AA=2.5
MTITLE label_without_number
CHIP A, retained tail
$ (1, MISSING, TC='한 글.oas', XYZ=[1,2], extra)
ROWS 1/2 3/4
CHIP A
future_command value
$ {2, COMPLETE, AD=1, TC=z, LY=1, UX=10, UY=5}
END
SHOULD NOT PARSE
""")
        add("no_rows", "CHIP A\n$ (1,A,AD=0.001,TC=a,LY=7,UX=1,UY=2)\n", {"a": 0.001})
        add("legacy_encoding", b"* legacy.jb\nCHIP A\n$ (1,A,AD=1,TC=bad\xff.oas,LY=7,UX=2,UY=3)\nROWS 0/0\n")
        for i, value in enumerate([0, -0.0000123456789, -0.000001, -0.00009999999,
                                   -999999.9, -1000000, -123.456789]):
            add(f"warning-number-{i}", f"CHIP A\n$ (1,A,AD=1,TC=a,LY=7,UX={value},UY=0)\n")
        add("coarsened", "CHIP A\n$ (1,A,AD=0.000001,TC=a,LY=7,UX=1,UY=2)\nROWS 1e15/-1e15\n", {"a": 0.000001})
        rng = random.Random(792)
        dbus = {"a.oas": 5e-5, "b.oas": 0.001, "c 한 글.oas": 0.002}
        for n in range(100):
            lines = ["* randomized.jb", "SLICE 1,17", "OPTION PA, AA=0.0200, UNKNOWN={1,2}", "MTITLE 2,MASK_2"]
            for ci in range(1 + n % 4):
                lines.append(f"CHIP C{ci}, * retained {ci}")
                for idx in range(1, 2 + rng.randrange(4)):
                    ad = rng.choice([0.00005, 0.0002, 0.001])
                    sf = rng.choice([0.5, 1, 2])
                    tc = rng.choice(list(dbus))
                    bx, by = rng.randrange(-100, 100), rng.randrange(-100, 100)
                    ux, uy = bx + rng.randrange(1, 100), by + rng.randrange(1, 100)
                    ly = rng.choice(["{1,2}", "{7}", "{}"])  # no LY is a warning, no pair
                    dt = rng.choice(["{0,3}", "{0}", "{}"])
                    lines.append(f"$ ({idx}.0, P{idx}, AD={ad}, SF={sf}, TC=\"{tc}\", LY={ly}, DT={dt}, BX={bx}, BY={by}, UX={ux}, UY={uy}, Z={ci})")
                lines.append("ROWS " + " ".join(f"{rng.randrange(-10000,10000)/8}/{rng.randrange(-10000,10000)/8}" for _ in range(1 + n % 3)))
            lines.append("END")
            add(f"random-{n}", "\n".join(lines), dbus, selections=(None, [2]))
        oracle = work / "oracle.json"
        oracle.write_text(json.dumps({"cases": cases}, ensure_ascii=False, allow_nan=False))
        env = dict(os.environ, FLOE_APP_JOBDECK_ORACLE=str(oracle))
        subprocess.run(["cargo", "test", "--offline", "--locked", "-p", "floe-app-core",
                        "--test", "jobdeck_oracle", "--", "--ignored", "--nocapture"],
                       cwd=ROOT / "rust", env=env, check=True, timeout=120)


if __name__ == "__main__":
    main()
