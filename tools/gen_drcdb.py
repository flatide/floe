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
      [--die 0,0,4300,3100] [--seed 42]

Then index and open:
  rust/target/release/floe-index drc data/drctest.db
  .venv/bin/python -m floe view <design.oas> --drc data/drctest.db
"""

import argparse
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
    a = ap.parse_args()
    prec = a.precision
    x0, y0, x1, y1 = (int(float(v) * prec)
                      for v in a.die.split(","))
    rng = random.Random(a.seed)
    layers = ("M1", "M2", "M3", "M4", "V1", "V2", "CT", "GT",
              "AA", "NW", "PP", "NP")
    kinds = ("SPACE", "WIDTH", "ENC", "EXT", "AREA", "DENSITY.W",
             "NOTCH", "OVERLAP")
    waivers = ("Waiver Criteria: none - -",
               "Waiver Criteria: waivable inside approved IP with "
               "foundry sign-off - -",
               "Waiver Criteria: see DRM chapter 7 - -")

    total = 0
    with open(a.out, "w") as f:
        f.write("MAIN09_ESD %d\n" % prec)
        for ci in range(a.checks):
            name = "%s.%s.%d" % (layers[ci % len(layers)],
                                 kinds[(ci * 7) % len(kinds)],
                                 ci % 97 + 1)
            n = (ci * 37 + 13) % (a.max_errors + 1)
            desc = ["Rule File Pathname: sfa14.drc.cal",
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


if __name__ == "__main__":
    main()
