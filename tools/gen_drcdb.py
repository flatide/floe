"""Synthetic Calibre ASCII DRC results database for load testing.

Defaults produce ~100 MB: 1000 rule checks, 0..1000 errors each
(deterministic per-check counts, some zero-result checks), realistic
block structure - shared Rule File Pathname/Title lines, occasional
Waiver Criteria: lines, and the four *_RDBS admin tail sections the
viewer must hide. Error mix exercises every viewer path:

  rect      35%  axis-aligned width/space region  -> dual CD rulers
  edgepair  30%  two parallel facing edges        -> gap CD ruler
  edge      10%  single edge                      -> length CD ruler
  stair     25%  rectilinear staircase (12..48 v) -> no ruler (complex)

usage:
  .venv/bin/python tools/gen_drcdb.py data/drctest.db \
      [--checks 1000] [--max-errors 1000] [--precision 40000] \
      [--die 0,0,4300,3100] [--seed 42] \
      [--zeros N] [--heavy 120000,250000,...]

--zeros N spreads N zero-error rules evenly through the list;
--heavy c1,c2,... assigns those exact counts to evenly spaced
rules (browser stress: 0-rule display, >100k-rule grids, 6-digit
global numbers).

--svrf DECK also writes a synthetic SVRF rule deck (plus a
DECK.layers INCLUDE file) whose check names/constraint values
match the db (floe/svrf.py end-to-end fixture): LAYER MAP + LAYER
tables, fill_excl/*_drawn derivations, one measurement per check
kind, a VARIABLE, and an #IFDEF SYNTH_EXTRA rule that exists only
under -D SYNTH_EXTRA. Rule names repeat past ~2328 checks - keep
--checks below that when the deck matters (names key the JSON).

To align the deck with a REAL design (viewer layer-isolation
tests): --layers renames the rule/derivation pool, --svrf-gds
NAME=layer[/dt],... pins each name (and FILLA/FILLB) to the
design's gds numbers as direct LAYER specs, --pathname sets the
Rule File Pathname the db records (the viewer looks for
<pathname-basename>.rules.json next to the db). See README §DRC
for the testchip_1g5 recipe.

Then index and open:
  rust/target/release/floe-index drc data/drctest.db
  .venv/bin/python -m floe view <design.oas> --drc data/drctest.db
"""

import argparse
import os
import random


def staircase(rng, x, y, prec):
    """Rectilinear staircase polygon (simple, 2k+2 vertices)."""
    k = rng.randrange(5, 24)
    s = rng.randrange(int(0.03 * prec), int(0.12 * prec))
    pts = [(x, y), (x + k * s, y)]
    for i in range(k, 0, -1):
        pts.append((x + i * s, y + (k - i + 1) * s))
        pts.append((x + (i - 1) * s, y + (k - i + 1) * s))
    return pts


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("out")
    ap.add_argument("--checks", type=int, default=1000)
    ap.add_argument("--max-errors", type=int, default=1000)
    ap.add_argument("--precision", type=int, default=40000)
    ap.add_argument("--die", default="0,0,4300,3100",
                    help="die extent in um: x0,y0,x1,y1")
    ap.add_argument("--seed", type=int, default=42)
    ap.add_argument("--zeros", type=int, default=0,
                    help="this many rules get 0 errors")
    ap.add_argument("--heavy", default="",
                    help="comma list of exact error counts to "
                         "assign to evenly spaced rules")
    ap.add_argument("--svrf", default="",
                    help="also write a matching synthetic SVRF "
                         "rule deck to this path")
    ap.add_argument("--layers", default="",
                    help="comma list overriding the layer-name pool "
                         "(rule-name prefixes + *_drawn derivations)")
    ap.add_argument("--svrf-gds", default="",
                    help="NAME=layer[/dt],... gds table for --svrf "
                         "(direct LAYER specs, no LAYER MAP) - align "
                         "the deck with a real design's layers; must "
                         "cover every --layers name + FILLA,FILLB")
    ap.add_argument("--pathname", default="sfa14.drc.cal",
                    help="Rule File Pathname recorded in the db - "
                         "the viewer looks for <basename>.rules.json "
                         "NEXT TO the db")
    a = ap.parse_args()
    prec = a.precision
    x0, y0, x1, y1 = (int(float(v) * prec)
                      for v in a.die.split(","))
    rng = random.Random(a.seed)
    layers = ("M1", "M2", "M3", "M4", "V1", "V2", "CT", "GT",
              "AA", "NW", "PP", "NP")
    if a.layers:
        layers = tuple(t.strip() for t in a.layers.split(",")
                       if t.strip())
    gds_map = None
    if a.svrf_gds:
        gds_map = {}
        for ent in a.svrf_gds.split(","):
            name, _, spec = ent.strip().partition("=")
            l, _, dt = spec.partition("/")
            gds_map[name] = (int(l), int(dt) if dt else None)
        missing = [n for n in tuple(layers) + ("FILLA", "FILLB")
                   if n not in gds_map]
        if missing:
            raise SystemExit("--svrf-gds missing: %s"
                             % ",".join(missing))
    kinds = ("SPACE", "WIDTH", "ENC", "EXT", "AREA", "DENSITY.W",
             "NOTCH", "OVERLAP")
    waivers = ("Waiver Criteria: none - -",
               "Waiver Criteria: waivable inside approved IP with "
               "foundry sign-off - -",
               "Waiver Criteria: see DRM chapter 7 - -")

    counts = [(ci * 37 + 13) % (a.max_errors + 1)
              for ci in range(a.checks)]
    if a.zeros:
        for k in range(min(a.zeros, a.checks)):
            counts[(k * a.checks) // min(a.zeros, a.checks)
                   % a.checks] = 0
    heavy = [int(v) for v in a.heavy.split(",") if v.strip()]
    for k, h in enumerate(heavy):
        # offset by 1 so heavies never land on the zero slots
        counts[(1 + (k * a.checks) // max(1, len(heavy)))
               % a.checks] = h

    total = 0
    svrf_rules = []   # (name, layer, kind, value-um) for --svrf
    with open(a.out, "w") as f:
        f.write("MAIN09_ESD %d\n" % prec)
        for ci in range(a.checks):
            name = "%s.%s.%d" % (layers[ci % len(layers)],
                                 kinds[(ci * 7) % len(kinds)],
                                 ci % 97 + 1)
            if a.svrf:
                svrf_rules.append(
                    (name, layers[ci % len(layers)],
                     kinds[(ci * 7) % len(kinds)], ci,
                     0.02 + (ci % 40) * 0.005))
            n = counts[ci]
            desc = ["Rule File Pathname: %s" % a.pathname,
                    "Rule File Title: SFA14 CalibreDRC "
                    "S00-V0.5.0.0-ENG_0520"]
            if ci % 3 == 0:
                desc.append(waivers[ci % len(waivers)])
            desc.append("%s : rule text for synthetic check %d, "
                        "min dimension %.3fum - -"
                        % (name, ci, 0.02 + (ci % 40) * 0.005))
            f.write("%s\n%d %d %d Jul 11 01:55:00 2026\n%s\n"
                    % (name, n, n, len(desc), "\n".join(desc)))
            out = []
            for ei in range(1, n + 1):
                ex = rng.randrange(x0, max(x0 + 1, x1 - prec * 6))
                ey = rng.randrange(y0, max(y0 + 1, y1 - prec * 6))
                cd = rng.randrange(int(0.02 * prec),
                                   int(0.2 * prec))
                ln = rng.randrange(int(0.2 * prec),
                                   int(2.0 * prec))
                m = rng.random()
                if m < 0.35:      # rect region
                    if rng.random() < 0.5:
                        w, h = cd, ln
                    else:
                        w, h = ln, cd
                    out.append("p %d 4\n%d %d\n%d %d\n%d %d\n%d %d"
                               % (ei, ex, ey, ex + w, ey,
                                  ex + w, ey + h, ex, ey + h))
                elif m < 0.65:    # facing edge pair, gap = cd
                    if rng.random() < 0.5:   # vertical edges
                        out.append(
                            "e %d 2\n%d %d %d %d\n%d %d %d %d"
                            % (ei, ex, ey, ex, ey + ln,
                               ex + cd, ey, ex + cd, ey + ln))
                    else:                    # horizontal edges
                        out.append(
                            "e %d 2\n%d %d %d %d\n%d %d %d %d"
                            % (ei, ex, ey, ex + ln, ey,
                               ex, ey + cd, ex + ln, ey + cd))
                elif m < 0.75:    # single edge, length = cd
                    out.append("e %d 1\n%d %d %d %d"
                               % (ei, ex, ey, ex + cd, ey))
                else:             # complex staircase: no CD ruler
                    pts = staircase(rng, ex, ey, prec)
                    out.append("p %d %d\n%s"
                               % (ei, len(pts),
                                  "\n".join("%d %d" % p
                                            for p in pts)))
            if out:
                f.write("\n".join(out) + "\n")
            total += n
        # the four admin tail sections every real db carries -
        # the viewer must NOT list them as checks
        f.write("DENSITY_RDBS\n0 0 2 Jul 11 01:59:00 2026\n"
                "density.rdb\ndensity_window.rdb\n")
        f.write("NET_AREA_RATIO_RDBS\n0 0 2 Jul 11 01:59:00 2026\n"
                "nar.rdb\nnar_accumulate.rdb\n")
        f.write("DFM_RDBS\n0 0 2 Jul 11 01:59:00 2026\n"
                "dfm.rdb\ndfm_property.rdb\n")
        f.write("LAYOUT_INPUT_EXCEPTION_RDBS\n"
                "0 0 1 Jul 11 01:59:00 2026\n"
                "layout_input_exceptions.rdb\n")
    print("%s: %d checks + 4 admin sections, %d errors"
          % (a.out, a.checks, total))
    if a.svrf:
        write_svrf(a.svrf, layers, svrf_rules, gds_map)
        print("%s: matching SVRF deck (%d checks + 1 under "
              "-D SYNTH_EXTRA)" % (a.svrf, len(svrf_rules)))


# gds numbers for the synthetic layer names (M1/M2 go through
# LAYER MAP so the parser's map resolution is exercised too)
SVRF_GDS = {"M1": 31, "M2": 32, "M3": 33, "M4": 34, "V1": 51,
            "V2": 52, "CT": 17, "GT": 20, "AA": 10, "NW": 11,
            "PP": 12, "NP": 13, "FILLA": 60, "FILLB": 61}


def write_svrf(path, layers, rules, gds_map=None):
    """Synthetic deck mirroring the db: every check derives from
    <layer>_drawn = LAYER NOT fill_excl, so source_gds closures
    always reach the LAYER MAP/LAYER tables. With a --svrf-gds map
    every LAYER is a direct layer[.datatype] spec (aligning the
    deck with a real design for viewer layer-isolation tests)."""
    lay = path + ".layers"
    with open(lay, "w") as f:
        f.write("// layer tables for %s (INCLUDE fixture)\n" % path)
        if gds_map is not None:
            for n in tuple(layers) + ("FILLA", "FILLB"):
                l, dt = gds_map[n]
                f.write("LAYER %s %s\n"
                        % (n, "%d.%d" % (l, dt) if dt is not None
                           else "%d" % l))
        else:
            f.write("LAYER MAP 31 DATATYPE 0 100\n")
            f.write("LAYER MAP 32 DATATYPE 0 101\n")
            f.write("LAYER M1 100\nLAYER M2 101\n")
            for n in layers[2:] + ("FILLA", "FILLB"):
                f.write("LAYER %s %d\n" % (n, SVRF_GDS[n]))

    def drawn(n):
        return n.lower() + "_drawn"

    def meas(kind, x, y, v):
        vs = "%.3f" % v
        return {"SPACE": "EXT %s < %s" % (x, vs),
                "WIDTH": "INT %s < %s" % (x, vs),
                "ENC": "ENC %s %s < %s" % (x, y, vs),
                "EXT": "EXT %s %s < %s" % (x, y, vs),
                "AREA": "AREA %s < %s" % (x, vs),
                "DENSITY.W": "DENSITY %s < %s WINDOW 50 50"
                             % (x, vs),
                "NOTCH": "INT %s < %s ABUT<90 SINGULAR REGION"
                         % (x, vs),
                "OVERLAP": "ENC %s %s < %s" % (y, x, vs)}[kind]

    with open(path, "w") as f:
        f.write("// synthetic SVRF deck (subset-parser fixture)\n"
                "PRECISION 40000\n"
                "INCLUDE \"%s\"\n"
                "VARIABLE SYN_GRID 0.005\n"
                "fill_excl = FILLA OR FILLB\n"
                % os.path.basename(lay))
        for n in layers:
            f.write("%s = %s NOT fill_excl\n" % (drawn(n), n))
        f.write("#IFDEF SYNTH_EXTRA\n"
                "EXTRA.CHECK.1 { @ extra rule, -D SYNTH_EXTRA only\n"
                "    INT m1_drawn < 0.001\n"
                "}\n"
                "#ENDIF\n")
        for name, layer, kind, ci, v in rules:
            x = drawn(layer)
            y = drawn(layers[(layers.index(layer) + 1)
                             % len(layers)])
            f.write("%s { @ %s : rule text for synthetic check %d, "
                    "min dimension %.3fum\n    %s\n}\n"
                    % (name, name, ci, v, meas(kind, x, y, v)))


if __name__ == "__main__":
    main()
