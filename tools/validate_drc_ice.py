"""DRC .ice sidecar gate: reading through the index == parsing the
ASCII database directly.

  D1  build sidecar on an adversarial synthetic .db (zero-result
      checks, duplicate check blocks, missing count line, unknown
      record kind, truncated records, CRLF, blank lines, negative
      coords) -> IceDb equals load_ascii check-for-check and
      error-for-error (kind/num/pts exact).
  D2  load_db(<db>) auto-picks a fresh sidecar; a stale sidecar
      (source mtime bumped) falls back to the ASCII parse and
      IceDb() raises.
  D3  string table dedupes the repeated Rule File Pathname/Title
      lines (sidecar stays small) and lazy slicing/iteration agree
      with full decode.

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
0 0 3 Jul 11 01:55:00 2026
Rule File Pathname: sfa14.drc.cal
Rule File Title: SFA14 CalibreDRC S00-V0.5.0.0-ENG_0520
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
    # D1: explicit sidecar open
    ice = drc.IceDb(side)
    # M1.SPACE 3 + dup block 1 + NOCOUNT 1 + SHORTDESC 2 + TRUNC 1
    if ref.total != 3 + 1 + 1 + 2 + 1:
        fail("fixture drifted: ascii total=%d" % ref.total)
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

    print("DRC ICE VALIDATION: ALL OK")


if __name__ == "__main__":
    main()
