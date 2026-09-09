#!/usr/bin/env python3
"""OASIS TRAPEZOID / CTRAPEZOID gate (2026-09-09): mask data (jobdeck
sources) is full of records 23-26, which the indexer refused
("TRAPEZOID: out of spike scope"). The parser now materializes them as
polygons using the vertex rules KLayout applies; this gate proves the
whole chain against KLayout as the independent oracle:

  hand-built records -> floe-index -> floe2 clip (OASIS out)
                     -> KLayout reads the clip
  hand-built records -> KLayout reads the original

and the two polygon sets must be identical (normalized vertex sets per
layer). Records: TRAPEZOID 23/24/25 both orientations and every delta
sign combination, CTRAPEZOID all 26 types at two sizes, plus the
implied-dimension modal write-back (a rectangle after a type-16 and a
type-20 shape) and a repetition on a trapezoid.

KLayout is a development dependency here (oracle only). Nothing is
written outside a temporary directory.
"""

import os
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def uint(v):
    out = bytearray()
    while True:
        b = v & 0x7f
        v >>= 7
        if v:
            out.append(b | 0x80)
        else:
            out.append(b)
            return bytes(out)


def sint(v):
    return uint((abs(v) << 1) | (1 if v < 0 else 0))


def bstr(b):
    return uint(len(b)) + b


def build_fixture(path):
    recs = bytearray(b"%SEMI-OASIS\r\n")
    # START: version, unit 1000/um (1 nm), offset-flag 0 + 6 x (flag, off)
    recs += uint(1) + bstr(b"1.0") + uint(0) + uint(1000) + uint(0) + uint(0) * 12
    recs += uint(14) + bstr(b"TOP")
    x = 0
    first = True

    def lead():
        nonlocal first
        if first:
            first = False
            return 0x03, uint(1) + uint(0)
        return 0, b""

    # TRAPEZOID 23: both deltas, both orientations, every sign case
    for o in (0, 1):
        for da, db in ((20, -10), (-20, 10), (20, 10), (-20, -10), (0, 30),
                       (30, 0), (0, 0)):
            ld, lb = lead()
            recs += (uint(23) + bytes([(o << 7) | 0x78 | ld]) + lb + uint(100)
                     + uint(40) + sint(da) + sint(db) + sint(x) + sint(0))
            x += 400
    # 24 (delta-a only) / 25 (delta-b only)
    for o in (0, 1):
        for d in (30, -30):
            recs += (uint(24) + bytes([(o << 7) | 0x78]) + uint(100) + uint(40)
                     + sint(d) + sint(x) + sint(0))
            x += 400
            recs += (uint(25) + bytes([(o << 7) | 0x78]) + uint(100) + uint(40)
                     + sint(d) + sint(x) + sint(0))
            x += 400
    # a trapezoid with a repetition (type 2: horizontal, 3 columns)
    recs += (uint(23) + bytes([0x7C]) + uint(100) + uint(40) + sint(20)
             + sint(-10) + sint(x) + sint(0) + uint(2) + uint(1) + uint(150))
    x += 1000
    # CTRAPEZOID: all 26 types at two sizes
    for w, h in ((100, 30), (60, 100)):
        for t in range(26):
            recs += (uint(26) + bytes([0xF8]) + uint(t) + uint(w) + uint(h)
                     + sint(x) + sint(0))
            x += 1000
    # implied dimensions feed the modals: rectangle after type 16 (h:=w)
    # and after type 20 (w:=2h), both written with X/Y only
    recs += uint(20) + bytes([0x78]) + uint(100) + uint(30) + sint(x) + sint(0)
    x += 400
    recs += uint(26) + bytes([0xD8]) + uint(16) + uint(100) + sint(x) + sint(0)
    x += 400
    recs += uint(20) + bytes([0x18]) + sint(x) + sint(0)
    x += 400
    recs += uint(26) + bytes([0xB8]) + uint(20) + uint(30) + sint(x) + sint(0)
    x += 400
    recs += uint(20) + bytes([0x18]) + sint(x) + sint(0)
    x += 400
    end = uint(2)
    pad = 256 - len(end) - 2 - 1
    end += bstr(b"\0" * pad) + uint(0)
    assert len(end) == 256
    recs += end
    Path(path).write_bytes(bytes(recs))
    return x


def polygon_sets(layout):
    """{(layer, datatype): sorted list of sorted vertex tuples}."""
    import klayout.db as db
    out = {}
    top = layout.top_cell()
    for li in layout.layer_indexes():
        info = layout.get_info(li)
        shapes = []
        for sh in top.shapes(li).each():
            poly = sh.polygon
            if poly is None:
                continue
            pts = tuple(sorted((p.x, p.y) for p in poly.each_point_hull()))
            shapes.append(pts)
        if shapes:
            out[(info.layer, info.datatype)] = sorted(shapes)
    return out


def main():
    import klayout.db as db
    index_bin = os.environ.get("FLOE_INDEX_BIN") or str(
        ROOT / "rust" / "target" / "release" / "floe-index")
    if not os.path.isfile(index_bin):
        raise SystemExit("release floe-index is not built: %s" % index_bin)
    with tempfile.TemporaryDirectory(prefix="floe-oasis-shapes-") as td:
        src = Path(td) / "shapes.oas"
        extent = build_fixture(src)
        env = dict(os.environ, FLOE_INDEX_BIN=index_bin,
                   FLOE_RENDERD_BIN=os.environ.get("FLOE_RENDERD_BIN") or str(
                       ROOT / "rust" / "target" / "release" / "floe-renderd"),
                   PYTHONPATH=str(ROOT), PYTHONDONTWRITEBYTECODE="1")
        res = subprocess.run([sys.executable, "-B", "-m", "floe2", "index",
                              str(src), "--jobs", "2"], cwd=ROOT, env=env,
                             capture_output=True, text=True)
        if res.returncode != 0:
            raise SystemExit("floe2 index failed on the trapezoid fixture:\n%s%s"
                             % (res.stdout, res.stderr))
        clip = Path(td) / "clip.oas"
        # the fixture's shapes span y -100..250 um at most; clip generously
        res = subprocess.run([sys.executable, "-B", "-m", "floe2", "clip",
                              str(src), "--bbox", "-1,-1,%d,1" % (extent / 1000 + 1),
                              "--out", str(clip)], cwd=ROOT, env=env,
                             capture_output=True, text=True)
        if res.returncode != 0:
            raise SystemExit("floe2 clip failed:\n%s%s" % (res.stdout, res.stderr))
        original = db.Layout()
        original.read(str(src))
        clipped = db.Layout()
        clipped.read(str(clip))
        a = polygon_sets(original)
        b = polygon_sets(clipped)
        if a.keys() != b.keys():
            raise SystemExit("layer sets differ: %s vs %s" % (sorted(a), sorted(b)))
        total = 0
        for key in a:
            if a[key] != b[key]:
                missing = [p for p in a[key] if p not in b[key]]
                extra = [p for p in b[key] if p not in a[key]]
                raise SystemExit(
                    "layer %s: %d polygons in the original, %d through floe; "
                    "first missing %s, first extra %s" % (
                        key, len(a[key]), len(b[key]),
                        missing[:1], extra[:1]))
            total += len(a[key])
        print("OASIS SHAPES: ALL OK (%d trapezoid/ctrapezoid/rect polygons "
              "identical through floe-index + clip vs KLayout)" % total)


if __name__ == "__main__":
    main()
