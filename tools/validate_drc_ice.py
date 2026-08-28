"""DRC .ice pack gate: reading through the pack == parsing the
ASCII database directly. (The v1 offset sidecar was RETIRED
2026-08-19 - `floe-index drc` always writes the self-contained v2
pack; D2 keeps the retirement honest.)

  D1  pack an adversarial synthetic .db (zero-result checks,
      duplicate check blocks, missing count line, unknown record
      kind, truncated records, CRLF, blank lines, negative coords,
      Waiver Criteria desc lines, *_RDBS admin tail sections
      dropped when empty / kept when they carry errors) -> IcePack
      equals load_ascii check-for-check and error-for-error
      (kind/num/pts exact, global file-order numbering).
  D2  dispatch: load_db(<db>) auto-picks a fresh pack; a stale
      pack (source mtime bumped) and a retired v1 sidecar both
      fall back to the ASCII parse (v1 opened directly raises).
      Corrupt packs (12-byte stub, truncated mid-file, wild
      footer offset) all raise ValueError - never struct.error -
      and a corrupt SIDE pack falls back to ASCII.
  D3  string table dedupes the repeated Rule File Pathname/Title
      lines and lazy slicing/iteration agree with full decode.
  D4  pack round-trip == load_ascii on a gen_drcdb asset too.
  D5  pack output bytes are --jobs invariant (1 vs 5 on the tiny
      fixture forces mid-check segment splits; 1 vs 4 on the
      gen_drcdb asset). D5b: FLOE_DRC_QBOX_RESIDENT=0 forces the
      big-rule streaming qbox path (bbox pre-pass + per-block
      rows) for every check - bytes must equal the default pack.
  D6  IcePack.query_rect == brute-force bbox scan on random rects;
      D6b: the waived= filter applies INSIDE the query, before the
      cap (regression: a capped post-filter lost matches hiding
      past `cap` non-matching errors).
  D7  [status] byte: zero at build, set/get via pwrite into the
      per-user waive autosave (the PACK bytes stay untouched),
      persists across reopen, neighbours untouched; the [wcount]
      per-rule waived counter stays in sync (incl. idempotent sets
      and reserved-status writes) so filter counts are O(1); the
      per-chunk waived-count cache (rank/page jumps) stays in
      sync across toggles made after it is built.
  D8  diagonal closest endpoints of a parallel edge pair retain the
      true minimum first and add deterministic horizontal + vertical
      component rulers; facing and non-parallel pairs stay single.
  D9  waive autosave (user calls 2026-08-28: per-user dotfile
      BESIDE the pack - server-side floe forwards the display, so
      $HOME may be absent; durable records = explicit save-as):
      export -> clear -> import round-trips statuses with wcount
      recomputed and the chunk cache reset; tampered and
      foreign-pack files are refused; a READ-ONLY pack stays
      reviewable (fresh autosave included); a read-only results
      FOLDER falls back to the system temp dir; a corrupt autosave
      is moved aside (review work preserved) and replaced fresh;
      embedded in-pack statuses from the retired scheme seed the
      first autosave; the pack bytes are identical before/after
      everything above.

usage: .venv/bin/python tools/validate_drc_ice.py [floe-index-bin]
"""

import os
import struct
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
__RVE_ERROR_TAG2__
2 2 1 Jul 11 01:56:30 2026
RVE tag bookkeeping mid-file: records are NOT violations
p 1 4
100 100
200 100
200 200
100 200
e 2 1
0 0 10 0
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
    # D1: pack == ASCII on the adversarial fixture
    ice = drc.IcePack(side, src_path=db, verify_src=True)
    # M1.SPACE 3 + dup block 1 + NOCOUNT 1 + SHORTDESC 2
    # + FAKE_RDBS 1 + TRUNC 1 (__RVE_ERROR_TAG2__'s 2 records are
    # excluded AND consume no global numbers - the 1..total
    # contiguity assert above catches a missing rollback)
    if ref.total != 3 + 1 + 1 + 2 + 1 + 1:
        fail("fixture drifted: ascii total=%d" % ref.total)
    names = [c.name for c in ref.checks]
    for gone in ("DENSITY_RDBS", "NET_AREA_RATIO_RDBS", "DFM_RDBS",
                 "LAYOUT_INPUT_EXCEPTION_RDBS",
                 "__RVE_ERROR_TAG2__"):
        if gone in names:
            fail("admin section %s surfaced as a check" % gone)
    if "FAKE_RDBS" not in names:
        fail("non-empty _RDBS check was dropped")
    compare(ref, ice)
    print("D1 OK: %d checks / %d errors identical through the pack"
          % (len(ref.checks), ref.total))

    # D2: dispatch - fresh pack auto-picked; stale pack and RETIRED
    # v1 sidecars fall back to ASCII (v1 dropped 2026-08-19: no
    # status storage, no spatial index)
    auto = drc.load_db(db)
    if not isinstance(auto, drc.IcePack):
        fail("fresh pack not auto-picked")
    st = os.stat(db)
    os.utime(db, (st.st_atime, st.st_mtime + 10))
    try:
        drc.IcePack(side, src_path=db, verify_src=True)
        fail("stale pack accepted")
    except ValueError:
        pass
    stale = drc.load_db(db)
    if not isinstance(stale, drc.DrcDb):
        fail("stale pack did not fall back to ASCII")
    compare(ref, stale)
    good = open(side, "rb").read()   # structurally valid pack bytes
    with open(side, "wb") as f:      # fake RETIRED v1 sidecar
        f.write(b"FLOEICE\0" + (1).to_bytes(4, "little") + b"\0" * 68)
    v1 = drc.load_db(db)
    if not isinstance(v1, drc.DrcDb):
        fail("v1 sidecar did not fall back to ASCII")
    try:
        drc.load_db(side)
        fail("retired v1 file opened directly")
    except ValueError:
        pass
    # corrupt packs: every damage mode must surface as ValueError
    # ("corrupt/truncated - rebuild"), never struct.error etc., and
    # a corrupt SIDE pack must fall back to the ASCII parse
    with open(side, "wb") as f:      # 12-byte stub (header cut off)
        f.write(b"FLOEICE\0" + (4).to_bytes(4, "little"))
    try:
        drc.IcePack(side)
        fail("12-byte corrupt pack opened")
    except ValueError:
        pass
    if not isinstance(drc.load_db(db), drc.DrcDb):
        fail("12-byte side pack did not fall back to ASCII")
    with open(side, "wb") as f:      # truncated mid-file
        f.write(good[:len(good) // 2])
    try:
        drc.IcePack(side)
        fail("truncated pack opened")
    except ValueError:
        pass
    if not isinstance(drc.load_db(db), drc.DrcDb):
        fail("truncated side pack did not fall back to ASCII")
    bad = bytearray(good)            # footer dir_off -> absurd
    off = len(bad) - drc._ICE2_FOOTER.size + 8 * 8
    bad[off:off + 8] = (1 << 60).to_bytes(8, "little")
    with open(side, "wb") as f:
        f.write(bytes(bad))
    try:
        drc.IcePack(side)
        fail("wild dir_off accepted")
    except ValueError:
        pass
    r = subprocess.run([BIN, "drc", db], capture_output=True, text=True)
    if r.returncode != 0:
        fail("re-index rc=%d" % r.returncode)
    again = drc.load_db(db)
    if not isinstance(again, drc.IcePack):
        fail("rebuilt pack not auto-picked")
    print("D2 OK: auto-pick fresh, refuse stale + retired v1, "
          "ASCII fallback")

    # D3: string-table dedup and lazy sequence semantics
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

    # D4/D5: gen_drcdb asset round-trip + jobs-invariant bytes (5
    # jobs on the ~2KB fixture forces mid-check segment boundaries)
    packs = {}
    for jobs in (1, 5):
        p = os.path.join(tmp, "fixture.j%d.ice" % jobs)
        r = subprocess.run(
            [BIN, "drc", db, p, "--jobs", str(jobs)],
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
            [BIN, "drc", gdb, p, "--jobs", str(jobs)],
            capture_output=True, text=True)
        if r.returncode != 0:
            fail("gen pack jobs=%d rc=%d" % (jobs, r.returncode))
        gpacks[jobs] = open(p, "rb").read()
    if gpacks[1] != gpacks[4]:
        fail("gen packed bytes differ between jobs=1 and jobs=4")
    gref = drc.load_ascii(gdb)
    gpk = drc.IcePack(os.path.join(tmp, "gen.j1.ice"))
    compare(gref, gpk)
    print("D4/D5 OK: fixture+gen round-trip, jobs-invariant bytes"
          " (%d checks / %d errors)"
          % (len(gref.checks), gref.total))

    # D5b: forcing the streaming qbox encoder (resident max 0 -> a
    # bbox pre-pass + per-block qbox rows for EVERY check) must not
    # change a single byte vs the resident path
    sp = os.path.join(tmp, "gen.stream.ice")
    r = subprocess.run(
        [BIN, "drc", gdb, sp, "--jobs", "4"],
        capture_output=True, text=True,
        env=dict(os.environ, FLOE_DRC_QBOX_RESIDENT="0"))
    if r.returncode != 0:
        fail("stream pack rc=%d: %s" % (r.returncode, r.stderr))
    if open(sp, "rb").read() != gpacks[1]:
        fail("streaming qbox encoder changed pack bytes")
    print("D5b OK: forced-streaming pack byte-identical")

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

    # D7: per-error review status byte
    import hashlib

    def sha(p):
        with open(p, "rb") as f:
            return hashlib.sha256(f.read()).hexdigest()

    gp = os.path.join(tmp, "gen.j1.ice")
    gh0 = sha(gp)
    if any(int(v) for v in gpk._status):
        fail("status section not zero at build")
    gpk.set_status(3, 2, drc.STATUS_WAIVED)
    gpk.set_status(3, 3, drc.STATUS_RESERVED)
    if gpk.get_status(3, 2) != drc.STATUS_WAIVED \
            or gpk.get_status(3, 3) != drc.STATUS_RESERVED:
        fail("status set/get mismatch")
    if sha(gp) != gh0:
        fail("waive wrote into the PACK (must go to the autosave)")
    gside = drc.waive_autosave_path(gp)
    if not os.path.isfile(gside) \
            or os.path.dirname(gside) != os.path.dirname(gp) \
            or not os.path.basename(gside).startswith("."):
        fail("waive autosave missing / not a dotfile beside the "
             "pack: %s" % gside)
    re2 = drc.IcePack(os.path.join(tmp, "gen.j1.ice"))
    if re2.get_status(3, 2) != drc.STATUS_WAIVED \
            or re2.get_status(3, 3) != drc.STATUS_RESERVED \
            or re2.get_status(3, 1) != drc.STATUS_NONE \
            or re2.get_status(4, 2) != drc.STATUS_NONE:
        fail("status did not persist / leaked to neighbours")
    if int(re2._status.sum()) != drc.STATUS_WAIVED + drc.STATUS_RESERVED:
        fail("stray status bytes written")
    # [wcount]: only the WAIVED write counted; reserved did not
    n3 = len(re2.checks[3].errors)
    if re2.status_counts(3) != (1, n3):
        fail("wcount out of sync: %r" % (re2.status_counts(3),))
    re2.set_status(3, 2, drc.STATUS_WAIVED)   # idempotent
    if re2.status_counts(3) != (1, n3):
        fail("idempotent set bumped wcount")
    re2.set_status(3, 2, drc.STATUS_NONE)
    re2.set_status(3, 3, drc.STATUS_NONE)
    if re2.status_counts(3) != (0, n3):
        fail("unwaive did not restore wcount")
    if int(re2._wcount.sum()) != 0:
        fail("stray wcount entries")
    # lazy paging == the materializing oracle
    n4 = len(re2.checks[4].errors)
    for ei in (1, 5, 9):
        re2.set_status(4, ei, drc.STATUS_WAIVED)
    for waived in (True, False):
        oracle = re2.status_eis(4, waived)
        if re2.status_page(4, waived, 0, 10 ** 9) != oracle:
            fail("status_page != status_eis (waived=%s)" % waived)
        for rank, ei in enumerate(oracle[:5]):
            if re2.status_rank(4, waived, ei) != rank:
                fail("status_rank mismatch")
    for ei in (1, 5, 9):
        re2.set_status(4, ei, drc.STATUS_NONE)
    # chunk-count cache (C-3): toggles AFTER the cache is built
    # (the calls above built it) must keep rank/page == oracle
    for ei in (0, 7):
        re2.set_status(4, ei, drc.STATUS_WAIVED)
    for waived in (True, False):
        oracle = re2.status_eis(4, waived)
        if re2.status_page(4, waived, 0, 10 ** 9) != oracle:
            fail("chunk cache out of sync after toggles")
        for rank, ei in enumerate(oracle[:4]):
            if re2.status_rank(4, waived, ei) != rank:
                fail("chunk-cache rank mismatch")
    for ei in (0, 7):
        re2.set_status(4, ei, drc.STATUS_NONE)
    if re2.status_page(4, True, 0, 10 ** 9):
        fail("chunk cache kept waived entries after clear")
    print("D7 OK: status byte + wcount + lazy paging + chunk "
          "cache in sync")

    # D6b: waived= filters INSIDE query_rect, before the cap - the
    # old caller-side post-filter dropped every match hiding past
    # `cap` non-matching errors
    cb = next(ci for ci, c in enumerate(re2.checks)
              if len(c.errors) >= 20)
    nb = len(re2.checks[cb].errors)
    last = nb - 1
    re2.set_status(cb, last, drc.STATUS_WAIVED)
    bbs = [re2.checks[cb].errors[i].bbox() for i in range(nb)]
    q = (min(b[0] for b in bbs), min(b[1] for b in bbs),
         max(b[2] for b in bbs), max(b[3] for b in bbs))
    got = re2.query_rect(q[0], q[1], q[2], q[3], cap=5,
                         checks=(cb,), waived=True)
    if [(ci, ei) for ci, ei, _e in got] != [(cb, last)]:
        fail("waived query missed the lone waived error under a "
             "small cap: %r" % [(ci, ei) for ci, ei, _e in got])
    nw = re2.query_rect(q[0], q[1], q[2], q[3], cap=10 ** 9,
                        checks=(cb,), waived=False)
    if {(ci, ei) for ci, ei, _e in nw} != \
            {(cb, i) for i in range(nb)} - {(cb, last)}:
        fail("not-waived query wrong")
    allq = re2.query_rect(q[0], q[1], q[2], q[3], cap=10 ** 9,
                          checks=(cb,))
    if {(ci, ei) for ci, ei, _e in allq} != \
            {(cb, i) for i in range(nb)}:
        fail("unfiltered query changed by the waived= addition")
    # waived=True + zero wcount takes the O(1) whole-rule skip
    cz = next(ci for ci, c in enumerate(re2.checks)
              if ci != cb and len(c.errors) >= 1)
    if re2.query_rect(-1e9, -1e9, 1e9, 1e9, cap=10 ** 9,
                      checks=(cz,), waived=True):
        fail("waived query returned errors from a wcount-0 rule")
    re2.set_status(cb, last, drc.STATUS_NONE)
    print("D6b OK: status filter inside query_rect, cap-safe")

    # D8: a parallel pair with disjoint projections needs the true
    # diagonal minimum plus its two axis components.  The first entry
    # is the measurement contract consumed by the DRC details panel.
    diagonal = drc.DrcError("e", 1,
                            [(0.0, 0.0), (1.0, 0.0),
                             (2.0, 1.0), (3.0, 1.0)])
    eq(drc.cd_segments(diagonal),
       [(1.0, 0.0, 2.0, 1.0),
        (1.0, 0.0, 2.0, 0.0),
        (2.0, 0.0, 2.0, 1.0)],
       "diagonal edge-pair component rulers")
    reversed_edges = drc.DrcError(
        "e", 2, [(1.0, 0.0), (0.0, 0.0),
                 (3.0, 1.0), (2.0, 1.0)])
    eq(drc.cd_segments(reversed_edges), drc.cd_segments(diagonal),
       "edge endpoint order changed component rulers")
    facing = drc.DrcError("e", 3,
                          [(0.0, 0.0), (3.0, 0.0),
                           (0.0, 1.0), (3.0, 1.0)])
    if len(drc.cd_segments(facing)) != 1:
        fail("facing parallel pair gained component rulers")
    skew = drc.DrcError("e", 4,
                        [(0.0, 0.0), (1.0, 0.0),
                         (2.0, 1.0), (3.0, 2.0)])
    if len(drc.cd_segments(skew)) != 1:
        fail("non-parallel pair gained component rulers")
    print("D8 OK: diagonal parallel gap + X/Y component rulers")

    # D9: waive sidecar - save-as/load, refusal, read-only pack,
    # corrupt-aside, in-pack seed migration
    # export -> clear -> import round-trip (wcount recomputed,
    # chunk cache reset)
    re2.set_status(4, 2, drc.STATUS_WAIVED)
    re2.set_status(4, 5, drc.STATUS_WAIVED)
    wsave = os.path.join(tmp, "review.waive")
    re2.waive_export(wsave)
    re2.set_status(4, 2, drc.STATUS_NONE)
    re2.set_status(4, 5, drc.STATUS_NONE)
    if re2.waive_import(wsave) != 2:
        fail("import waived-count wrong")
    if re2.get_status(4, 2) != drc.STATUS_WAIVED \
            or re2.get_status(4, 5) != drc.STATUS_WAIVED \
            or re2.status_counts(4) != (2, n4):
        fail("import did not restore statuses/wcount")
    if re2.status_page(4, True, 0, 10 ** 9) != [2, 5]:
        fail("chunk cache stale after import")
    # tampered and foreign-pack files are refused, state untouched
    with open(wsave, "rb") as f:
        blob = bytearray(f.read())
    blob[9] ^= 0xFF   # version field
    bad = os.path.join(tmp, "bad.waive")
    with open(bad, "wb") as f:
        f.write(bytes(blob))
    try:
        re2.waive_import(bad)
        fail("tampered waive file accepted")
    except ValueError:
        pass
    try:
        ice.waive_import(wsave)
        fail("foreign pack accepted another pack's waive file")
    except ValueError:
        pass
    if re2.status_counts(4) != (2, n4):
        fail("refused import disturbed the state")
    re2.set_status(4, 2, drc.STATUS_NONE)
    re2.set_status(4, 5, drc.STATUS_NONE)
    # READ-ONLY pack: reviewable, including fresh sidecar creation
    os.remove(gside)
    mode = os.stat(gp).st_mode
    os.chmod(gp, 0o444)
    try:
        ro = drc.IcePack(gp)
        ro.set_status(3, 0, drc.STATUS_WAIVED)
        if ro.get_status(3, 0) != drc.STATUS_WAIVED:
            fail("read-only pack: waive did not stick")
        ro.close()
    finally:
        os.chmod(gp, mode)
    # corrupt sidecar: moved ASIDE (not deleted) + fresh start
    with open(gside, "r+b") as f:
        f.write(b"JUNKJUNK")
    x = drc.IcePack(gp)
    if x.get_status(3, 0) != drc.STATUS_NONE:
        fail("corrupt sidecar not replaced fresh")
    x.close()
    wdir = os.path.dirname(gside)
    gbase = os.path.basename(gside)
    if not any(p.startswith(gbase) and ".stale-" in p
               for p in os.listdir(wdir)):
        fail("corrupt autosave was not preserved aside")
    # read-only results FOLDER: autosave falls back to the system
    # temp dir and review still works
    import shutil
    rodir = os.path.join(tmp, "ro")
    os.makedirs(rodir)
    rp = os.path.join(rodir, "gen.j1.ice")
    shutil.copyfile(gp, rp)
    os.chmod(rodir, 0o555)
    try:
        rf = drc.IcePack(rp)
        if os.path.dirname(rf._waive_path) != tempfile.gettempdir():
            fail("read-only folder: autosave not in the temp dir "
                 "(%s)" % rf._waive_path)
        rf.set_status(3, 0, drc.STATUS_WAIVED)
        if rf.get_status(3, 0) != drc.STATUS_WAIVED:
            fail("read-only folder: waive did not stick")
        tmpside = rf._waive_path
        rf.close()
    finally:
        os.chmod(rodir, 0o755)
    os.remove(tmpside)
    # migration: embedded in-pack statuses (retired scheme) seed
    # the first autosave; pack restored byte-identical afterwards
    with open(gp, "rb") as f:
        f.seek(os.path.getsize(gp) - drc._ICE2_FOOTER.size)
        foot = drc._ICE2_FOOTER.unpack(f.read(drc._ICE2_FOOTER.size))
    soff, woff = foot[4], foot[5]
    gid = int(x._dir_es[3])   # check 3, error 0
    os.remove(drc.waive_autosave_path(gp))
    fd = os.open(gp, os.O_RDWR)
    try:
        os.pwrite(fd, bytes((drc.STATUS_WAIVED,)), soff + gid)
        os.pwrite(fd, struct.pack("<I", 1), woff + 4 * 3)
        mig = drc.IcePack(gp)
        if mig.get_status(3, 0) != drc.STATUS_WAIVED \
                or mig.status_counts(3)[0] != 1:
            fail("in-pack statuses did not seed the sidecar")
        mig.close()
    finally:
        os.pwrite(fd, bytes((drc.STATUS_NONE,)), soff + gid)
        os.pwrite(fd, struct.pack("<I", 0), woff + 4 * 3)
        os.close(fd)
    if sha(gp) != gh0:
        fail("D9 left the pack modified")
    print("D9 OK: autosave save-as/load + refusal + read-only "
          "pack/folder + corrupt-aside + seed migration")

    print("DRC ICE VALIDATION: ALL OK")


if __name__ == "__main__":
    main()
