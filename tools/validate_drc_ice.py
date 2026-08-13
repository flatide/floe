"""DRC .ice sidecar gate: reading through the index == parsing the
ASCII database directly.

  D1  build sidecar on an adversarial synthetic .db (zero-result
      checks, duplicate check blocks, missing count line, unknown
      record kind, truncated records, CRLF, blank lines, negative
      coords, Waiver Criteria desc lines, *_RDBS admin tail
      sections dropped when empty / kept when they carry errors)
      -> IceDb equals load_ascii check-for-check and
      error-for-error (kind/num/pts exact).
  D2  load_db(<db>) auto-picks a fresh sidecar; a stale sidecar
      (source mtime bumped) falls back to the ASCII parse and
      IceDb() raises.
  D3  string table dedupes the repeated Rule File Pathname/Title
      lines (sidecar stays small) and lazy slicing/iteration agree
      with full decode.
  D4  --pack (self-contained .ice v2) round-trip == load_ascii on
      the adversarial fixture AND on a gen_drcdb asset (ordered:
      v2 stores file order; numbers are the global sequence).
  D5  pack output bytes are --jobs invariant (1 vs 5 on the tiny
      fixture forces mid-check segment splits; 1 vs 4 on the
      gen_drcdb asset).
  D6  IcePack.query_rect == brute-force bbox scan on random rects.

usage: .venv/bin/python tools/validate_drc_ice.py [floe-index-bin]
"""

import os
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from floe import drc  # noqa: E402

BIN = sys.argv[1] if len(sys.argv) > 1 else os.path.join(
    os.path.dirname(__file__), "..", "rust", "target", "release",
    "floe-index")

DB = """MAIN09_ESD 40000
GRGEOM.1_BFMOAT
0 0 4 Jul 11 01:55:00 2026
Rule File Pathname: sfa14.drc.cal
Rule File Title: SFA14 CalibreDRC S00-V0.5.0.0-ENG_0520
Waiver Criteria: none - -
All Design Layers grid must be an integer multiple of 0.00025um. - -
GRGEOM.1_BIPOLAR
0 0 3 Jul 11 01:55:00 2026
Rule File Pathname: sfa14.drc.cal
Rule File Title: SFA14 CalibreDRC S00-V0.5.0.0-ENG_0520
Second zero-result check sharing pathname/title lines.
M1.SPACE
3 5 2 Jul 11 01:55:00 2026
Rule File Pathname: sfa14.drc.cal
M1 space < 0.05um
p 1 4
100 200
300 200
300 400

100 400
p 2 3
-40000 -80000
-40000 -79000
-39000 -79000
x 9 2
1 2
3 4
p 3 5
7 8
9 10
M1.SPACE
1 1 1 Jul 11 01:56:00 2026
duplicate check name: merged runs keep separate blocks
e 1 2
0 0 4000 0
4000 0 4000 4000
NOCOUNT.CHECK
p 1 1
123456 654321
SHORTDESC.CHECK
2 2 3 Jul 11 01:57:00 2026
only one desc line before geometry
e 1 1
-1 -2 -3 -4
e 2 1
5 6 7 8
FAKE_RDBS
1 1 0 Jul 11 01:58:00 2026
p 1 1
42 42
DENSITY_RDBS
0 0 2 Jul 11 01:59:00 2026
density.rdb
density2.rdb
NET_AREA_RATIO_RDBS
0 0 2 Jul 11 01:59:00 2026
nar.rdb
nar2.rdb
DFM_RDBS
0 0 2 Jul 11 01:59:00 2026
dfm.rdb
dfm2.rdb
LAYOUT_INPUT_EXCEPTION_RDBS
0 0 1 Jul 11 01:59:00 2026
layout_input_exceptions.rdb
TRUNC.TAIL
1 1 0 Jul 11 01:58:00 2026
p 1 5
11 12
13 14
"""


def fail(msg):
    print("FAIL:", msg)
    sys.exit(1)


def eq(a, b, what):
    if a != b:
        fail("%s: %r != %r" % (what, a, b))


def compare(ref, ice):
    eq(ref.cell, ice.cell, "cell")
    eq(ref.precision, ice.precision, "precision")
    eq(len(ref.checks), len(ice.checks), "check count")
    for ci, (rc, xc) in enumerate(zip(ref.checks, ice.checks)):
        tag = "check[%d] %s" % (ci, rc.name)
        eq(rc.name, xc.name, tag + " name")
        eq(rc.desc, xc.desc, tag + " desc")
        eq(rc.declared, xc.declared, tag + " declared")
        eq(len(rc.errors), len(xc.errors), tag + " error count")
        for ei, re_ in enumerate(rc.errors):
            xe = xc.errors[ei]
            eq(re_.kind, xe.kind, tag + " err[%d] kind" % ei)
            eq(re_.num, xe.num, tag + " err[%d] num" % ei)
            eq(re_.pts, xe.pts, tag + " err[%d] pts" % ei)


def main():
    tmp = tempfile.mkdtemp(prefix="floe-drcice-")
    db = os.path.join(tmp, "results.db")
    # CRLF stretch: rewrite one whole check block with \r\n line ends
    text = DB.replace("e 1 1\n-1 -2 -3 -4\n",
                      "e 1 1\r\n-1 -2 -3 -4\r\n")
    with open(db, "w", newline="") as f:
        f.write(text)

    r = subprocess.run([BIN, "drc", db], capture_output=True, text=True)
    if r.returncode != 0:
        fail("indexer rc=%d: %s" % (r.returncode, r.stderr.strip()))
    side = db + ".ice"
    if not os.path.exists(side):
        fail("sidecar not written")

    ref = drc.load_ascii(db)
    nums = [e.num for c in ref.checks for e in c.errors]
    if nums != list(range(1, ref.total + 1)):
        fail("global file-order numbering broken: %r" % nums[:10])
    # D1: explicit sidecar open
    ice = drc.IceDb(side)
    # M1.SPACE 3 + dup block 1 + NOCOUNT 1 + SHORTDESC 2
    # + FAKE_RDBS 1 + TRUNC 1
    if ref.total != 3 + 1 + 1 + 2 + 1 + 1:
        fail("fixture drifted: ascii total=%d" % ref.total)
    names = [c.name for c in ref.checks]
    for gone in ("DENSITY_RDBS", "NET_AREA_RATIO_RDBS", "DFM_RDBS",
                 "LAYOUT_INPUT_EXCEPTION_RDBS"):
        if gone in names:
            fail("admin section %s surfaced as a check" % gone)
    if "FAKE_RDBS" not in names:
        fail("non-empty _RDBS check was dropped")
    compare(ref, ice)
    print("D1 OK: %d checks / %d errors identical through the sidecar"
          % (len(ref.checks), ref.total))

    # D2: auto-pick + stale fallback
    auto = drc.load_db(db)
    if not isinstance(auto, drc.IceDb):
        fail("fresh sidecar not auto-picked")
    st = os.stat(db)
    os.utime(db, (st.st_atime, st.st_mtime + 10))
    try:
        drc.IceDb(side)
        fail("stale sidecar accepted")
    except ValueError:
        pass
    stale = drc.load_db(db)
    if not isinstance(stale, drc.DrcDb):
        fail("stale sidecar did not fall back to ASCII")
    compare(ref, stale)
    r = subprocess.run([BIN, "drc", db], capture_output=True, text=True)
    if r.returncode != 0:
        fail("re-index rc=%d" % r.returncode)
    again = drc.load_db(db)
    if not isinstance(again, drc.IceDb):
        fail("rebuilt sidecar not auto-picked")
    print("D2 OK: auto-pick fresh, refuse stale, ASCII fallback")

    # D3: dedup (sidecar much smaller than the strings it names) and
    # lazy sequence semantics
    c = again.checks[2]           # M1.SPACE first block
    full = [c.errors[i] for i in range(len(c.errors))]
    sliced = c.errors[:2]
    eq([e.pts for e in sliced], [e.pts for e in full[:2]], "slice")
    eq([e.num for e in c.errors], [e.num for e in full], "iteration")
    blob = open(side, "rb").read()
    if blob.count(b"Rule File Pathname: sfa14.drc.cal") != 1:
        fail("pathname line stored more than once")
    if blob.count(b"Rule File Title:") != 1:
        fail("title line stored more than once")
    print("D3 OK: line-level dedup + lazy slicing/iteration")

    # D4/D5 on the tiny adversarial fixture: 5 jobs on a ~2KB file
    # forces segment boundaries inside checks and records
    packs = {}
    for jobs in (1, 5):
        p = os.path.join(tmp, "fixture.j%d.ice" % jobs)
        r = subprocess.run(
            [BIN, "drc", db, p, "--pack", "--jobs", str(jobs)],
            capture_output=True, text=True)
        if r.returncode != 0:
            fail("pack jobs=%d rc=%d: %s"
                 % (jobs, r.returncode, r.stderr.strip()))
        packs[jobs] = open(p, "rb").read()
    if packs[1] != packs[5]:
        fail("packed bytes differ between jobs=1 and jobs=5")
    pk = drc.load_db(os.path.join(tmp, "fixture.j1.ice"))
    if not isinstance(pk, drc.IcePack):
        fail("packed file not opened as IcePack")
    compare(ref, pk)
    print("D4/D5 OK (fixture): pack == ASCII, jobs-invariant bytes")

    # bigger deterministic asset via gen_drcdb (few MB)
    gdb = os.path.join(tmp, "gen.db")
    r = subprocess.run(
        [sys.executable,
         os.path.join(os.path.dirname(__file__), "gen_drcdb.py"),
         gdb, "--checks", "60", "--max-errors", "150", "--seed", "7"],
        capture_output=True, text=True)
    if r.returncode != 0:
        fail("gen_drcdb rc=%d: %s" % (r.returncode, r.stderr))
    gpacks = {}
    for jobs in (1, 4):
        p = os.path.join(tmp, "gen.j%d.ice" % jobs)
        r = subprocess.run(
            [BIN, "drc", gdb, p, "--pack", "--jobs", str(jobs)],
            capture_output=True, text=True)
        if r.returncode != 0:
            fail("gen pack jobs=%d rc=%d" % (jobs, r.returncode))
        gpacks[jobs] = open(p, "rb").read()
    if gpacks[1] != gpacks[4]:
        fail("gen packed bytes differ between jobs=1 and jobs=4")
    gref = drc.load_ascii(gdb)
    gpk = drc.IcePack(os.path.join(tmp, "gen.j1.ice"))
    compare(gref, gpk)
    print("D4/D5 OK (gen_drcdb): %d checks / %d errors round-trip"
          % (len(gref.checks), gref.total))

    # D6: query_rect == brute force bbox scan
    import random
    rng = random.Random(11)
    brute = []
    for ci, c in enumerate(gpk.checks):
        for ei in range(len(c.errors)):
            brute.append((ci, ei, c.errors[ei].bbox()))
    for _ in range(12):
        x = rng.uniform(0, 4300)
        y = rng.uniform(0, 3100)
        w = rng.uniform(0.5, 400)
        h = rng.uniform(0.5, 400)
        q = (x, y, x + w, y + h)
        want = {(ci, ei) for ci, ei, bb in brute
                if bb[0] <= q[2] and bb[2] >= q[0]
                and bb[1] <= q[3] and bb[3] >= q[1]}
        got = {(ci, ei) for ci, ei, _e in gpk.query_rect(
            q[0], q[1], q[2], q[3], cap=10 ** 9)}
        if got != want:
            fail("query_rect mismatch at %r: %d vs %d (sym diff %d)"
                 % (q, len(got), len(want),
                    len(got.symmetric_difference(want))))
    print("D6 OK: query_rect == brute force on 12 random rects")

    print("DRC ICE VALIDATION: ALL OK")


if __name__ == "__main__":
    main()
