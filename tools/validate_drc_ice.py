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
  D9  waive autosave (user calls 2026-08-28: per-reviewer dotfile
      BESIDE the pack - server-side floe forwards the display, so
      $HOME may be absent; durable records = explicit save-as; the
      shared server account makes the account name non-unique, so
      the reviewer tag prefers FLOE_REVIEWER > DISPLAY host >
      SSH client IP > account name):
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
from floe import cachepath  # noqa: E402

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


def validate_names(tmp, db, side):
    """2026-09-16 rename (docs/CACHE-NAMING.ko.md): the pack is
    `.<db>.tray`; a pre-rename `<db>.ice` pack is renamed to it on first
    touch; the reviewer sidecars are named from the .db (no `..name`
    from the hidden pack name, and the ones written beside a `<db>.ice`
    pack still match)."""
    user = drc._waive_user()
    want = os.path.join(tmp, ".results.db.waive." + user)
    if drc.waive_autosave_path(side) != want:
        fail("waive sidecar of the hidden pack: %r" % drc.waive_autosave_path(side))
    if drc.waive_autosave_path(os.path.join(tmp, "results.db.ice")) != want:
        fail("waive sidecar differs between the legacy and current pack names")
    if drc.notes_autosave_path(side) != os.path.join(
            tmp, ".results.db.notes.%s.fe" % user):
        fail("notes sidecar of the hidden pack: %r" % drc.notes_autosave_path(side))
    if os.path.basename(drc._waive_tmp_fallback(side)).startswith(".."):
        fail("temp fallback name starts with a double dot")
    # a legacy-named pack is renamed in place by the loader
    legacy = os.path.join(tmp, "results.db.ice")
    os.rename(side, legacy)
    pk = drc.load_db(db)
    if not isinstance(pk, drc.IcePack) or pk.path != side:
        fail("legacy <db>.ice pack was not renamed and reopened: %r"
             % getattr(pk, "path", None))
    if os.path.exists(legacy) or not os.path.exists(side):
        fail("legacy pack still present after the rename")
    pk.close() if hasattr(pk, "close") else None
    # the kill switch keeps the legacy name (and still reads it)
    os.rename(side, legacy)
    os.environ["FLOE_CACHE_MIGRATE"] = "off"
    try:
        pk = drc.load_db(db)
        if not isinstance(pk, drc.IcePack) or pk.path != legacy:
            fail("FLOE_CACHE_MIGRATE=off did not read the legacy pack in place")
        if os.path.exists(side):
            fail("FLOE_CACHE_MIGRATE=off renamed the pack")
    finally:
        del os.environ["FLOE_CACHE_MIGRATE"]
    pk.close() if hasattr(pk, "close") else None
    os.rename(legacy, side)


def validate_service(tmp):
    """D12 (2026-10-09 P2b): the GTK viewer's review through floe2
    gtk-service (app-core drc::desktop) is IcePack's - the same files,
    bytes and answers: checks, errors (kind/num/pts) and CD segments; a
    status the service writes is in the sidecar Python reads (counters
    in step), status pages/ranks and spatial queries (waived filters
    too) equal; notes the service writes are the .fe Python serializes
    and reloads, cleared the file goes; a waive export is byte-equal and
    a foreign one refused; another run's sidecar is moved aside; the
    service's reader lock refuses a re-pack until it closes; svrf
    operands and the rules sidecar read as floe/svrf.py read them."""
    import json
    import random
    from floe import gtkservice, svrf
    floe2 = os.environ.get("FLOE2_BIN") or os.path.join(
        os.path.dirname(__file__), "..", "rust", "target", "release", "floe2")
    gtkservice._SERVICE = gtkservice.Service(floe2)
    d = os.path.join(tmp, "svc")
    os.makedirs(d)
    db = os.path.join(d, "results.db")
    with open(db, "w") as f:
        f.write(DB)
    r = subprocess.run([BIN, "drc", db], capture_output=True, text=True)
    if r.returncode != 0:
        fail("D12 indexer rc=%d: %s" % (r.returncode, r.stderr.strip()))
    side = cachepath.pack_path(db)
    os.environ["FLOE_REVIEWER"] = "gatesvc"
    try:
        eq(gtkservice.drc_busy(db), None, "D12 busy")
        eq(os.path.realpath(gtkservice.drc_find(db)), os.path.realpath(side),
           "D12 find")
        sdb = gtkservice.drc_load(db)
        if not sdb.packed:
            fail("D12 the service did not open the pack")
        eq(os.path.realpath(sdb.waive_path),
           os.path.realpath(drc.waive_autosave_path(side)), "D12 waive path")
        py = drc.IcePack(side, src_path=db, verify_src=True)
        eq(len(sdb.checks), len(py.checks), "D12 check count")
        for ci, (a, b) in enumerate(zip(py.checks, sdb.checks)):
            eq((a.name, a.desc, a.declared, len(a.errors)),
               (b.name, b.desc, b.declared, len(b.errors)),
               "D12 check %d" % ci)
            for ei in range(len(a.errors)):
                ea, eb = a.errors[ei], b.errors[ei]
                eq((ea.kind, ea.num, [tuple(p) for p in ea.pts]),
                   (eb.kind, eb.num, eb.pts), "D12 error %d/%d" % (ci, ei))
                eq([tuple(s) for s in drc.cd_segments(ea)], eb.cd_segments(),
                   "D12 cd %d/%d" % (ci, ei))
        ci = max(range(len(py.checks)), key=lambda c: len(py.checks[c].errors))
        n = len(py.checks[ci].errors)
        sdb.set_statuses(ci, [0, n - 1], drc.STATUS_WAIVED)
        sdb.set_status(ci, 0, drc.STATUS_WAIVED)          # idempotent
        eq([py.get_status(ci, e) for e in range(n)],
           [sdb.get_status(ci, e) for e in range(n)], "D12 statuses")
        eq(py.get_status(ci, 0), drc.STATUS_WAIVED, "D12 written status")
        eq(py.status_counts(ci), sdb.status_counts(ci), "D12 counts")
        for waived in (True, False):
            eq(sdb.status_page(ci, waived, 0, n), py.status_page(ci, waived, 0, n),
               "D12 status page %s" % waived)
            for e in range(n):
                eq(sdb.status_rank(ci, waived, e), py.status_rank(ci, waived, e),
                   "D12 rank %s %d" % (waived, e))
        rng = random.Random(12)
        for _ in range(40):
            x0, x1 = sorted(rng.uniform(-60, 6) for _ in range(2))
            y0, y1 = sorted(rng.uniform(-90, 6) for _ in range(2))
            for waived in (None, True, False):
                got = [(c, e, (er.kind, er.num, er.pts)) for c, e, er in
                       sdb.query_rect(x0, y0, x1, y1, cap=3, waived=waived)]
                want = [(c, e, (er.kind, er.num, [tuple(p) for p in er.pts]))
                        for c, e, er in
                        py.query_rect(x0, y0, x1, y1, cap=3, waived=waived)]
                eq(got, want, "D12 query %r %s" % ((x0, y0, x1, y1), waived))
        # notes: the service writes what Python serializes and reloads
        gid = sdb.error_gid(ci, 0)
        sdb.set_note([gid, gid + 1], "  first line\nsecond 한글 \\ ")
        note = drc.notes_autosave_path(side)
        text = open(note, encoding="utf-8").read()
        py2 = drc.IcePack(side, src_path=db, verify_src=True)
        eq(py2.notes_list(), sdb.notes_list(), "D12 notes reload")
        eq(py2._serialize_notes(), text, "D12 notes bytes")
        eq(sdb.get_note(ci, 1), "first line\nsecond 한글 \\", "D12 note text")
        py2.close()
        sdb.clear_note([gid, gid + 1])
        if os.path.exists(note):
            fail("D12 the notes file stayed with no note left")
        # waive files
        a, b = os.path.join(d, "svc.waive"), os.path.join(d, "py.waive")
        sdb.waive_export(a)
        py.waive_export(b)
        eq(open(a, "rb").read(), open(b, "rb").read(), "D12 waive export")
        eq(sdb.waive_import(a), 2, "D12 waive import")
        bad = bytearray(open(a, "rb").read())
        bad[12] ^= 1
        with open(os.path.join(d, "bad.waive"), "wb") as f:
            f.write(bytes(bad))
        try:
            sdb.waive_import(os.path.join(d, "bad.waive"))
            fail("D12 a foreign waive file was taken")
        except gtkservice.ServiceError as exc:
            if "does not match this pack" not in str(exc):
                fail("D12 foreign waive file: %s" % exc)
        # the reader lock: a re-pack is refused while the service holds it
        py.close()
        r = subprocess.run([BIN, "drc", db], capture_output=True, text=True)
        eq(r.returncode, 75, "D12 re-pack under the service's review")
        sdb.close()
        # another run's sidecar is moved aside, a fresh one seeded
        with open(sdb.waive_path, "r+b") as f:
            f.write(b"XXXXXXXX")
        sdb = gtkservice.drc_load(db)
        if not [x for x in os.listdir(d) if ".waive.gatesvc.stale-" in x]:
            fail("D12 a foreign sidecar was not moved aside")
        eq(sdb.get_status(ci, 0), 0, "D12 fresh sidecar")
        sdb.close()
        # a bigger asset: every error, pages and ranks after random waives,
        # queries at several caps
        gdb = os.path.join(d, "gen.db")
        r = subprocess.run(
            [sys.executable, os.path.join(os.path.dirname(__file__),
                                          "gen_drcdb.py"),
             gdb, "--checks", "60", "--max-errors", "150", "--seed", "12"],
            capture_output=True, text=True)
        if r.returncode != 0:
            fail("D12 gen_drcdb rc=%d: %s" % (r.returncode, r.stderr))
        r = subprocess.run([BIN, "drc", gdb], capture_output=True, text=True)
        if r.returncode != 0:
            fail("D12 gen pack rc=%d: %s" % (r.returncode, r.stderr))
        gside = cachepath.pack_path(gdb)
        gs = gtkservice.drc_load(gdb)
        gp = drc.IcePack(gside, src_path=gdb, verify_src=True)
        rng = random.Random(7)
        for ci, (a, b) in enumerate(zip(gp.checks, gs.checks)):
            eq([(e.kind, e.num, [tuple(p) for p in e.pts]) for e in a.errors],
               [(e.kind, e.num, e.pts) for e in b.errors], "D12 gen check %d" % ci)
            picks = [e for e in range(len(a.errors)) if rng.random() < 0.3]
            gs.set_statuses(ci, picks, drc.STATUS_WAIVED)
        xs, ys = [], []
        for c in gp.checks:
            for e in c.errors:
                for x, y in e.pts:
                    xs.append(x)
                    ys.append(y)
        for ci in range(len(gp.checks)):
            n = len(gp.checks[ci].errors)
            eq(gs.status_counts(ci), gp.status_counts(ci), "D12 gen counts %d" % ci)
            for waived in (True, False):
                eq(gs.status_page(ci, waived, 3, 40), gp.status_page(ci, waived, 3, 40),
                   "D12 gen page %d %s" % (ci, waived))
                for e in range(0, n, 7):
                    eq(gs.status_rank(ci, waived, e), gp.status_rank(ci, waived, e),
                       "D12 gen rank %d %d" % (ci, e))
        for _ in range(30):
            x0, x1 = sorted(rng.uniform(min(xs), max(xs)) for _ in range(2))
            y0, y1 = sorted(rng.uniform(min(ys), max(ys)) for _ in range(2))
            only = sorted(rng.sample(range(len(gp.checks)), 5))
            for cap, checks, waived in ((50, None, None), (2000, None, True),
                                        (2000, only, False), (7, only, None)):
                got = [(c, e, er.num) for c, e, er in gs.query_rect(
                    x0, y0, x1, y1, cap=cap, checks=checks, waived=waived)]
                want = [(c, e, er.num) for c, e, er in gp.query_rect(
                    x0, y0, x1, y1, cap=cap, checks=checks, waived=waived)]
                eq(got, want, "D12 gen query cap=%d %s" % (cap, waived))
        gp.close()
        gs.close()
        # svrf
        for rhs in ("(a AND L3) SIZE BY 0.01 NOT x.y", "M1 INTERACT VIA_1",
                    "INT m1 < 0.05 ABUT<90 SINGULAR REGION"):
            eq(gtkservice.svrf_operands(rhs), svrf.rhs_operands(rhs),
               "D12 operands %r" % rhs)
        rules = os.path.join(d, "deck.rules.json")
        with open(rules, "w") as f:
            json.dump({"format": svrf.FORMAT, "version": svrf.VERSION,
                       "checks": {"M1.S": {"constraints": []}}}, f)
        eq(gtkservice.svrf_rules(rules), svrf.load_rules(rules), "D12 rules")
    finally:
        os.environ.pop("FLOE_REVIEWER", None)
        gtkservice._SERVICE.close()
        gtkservice._SERVICE = None
    print("D12 OK: floe2 gtk-service review == IcePack (files, bytes, "
          "answers)")


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
    side = cachepath.pack_path(db)
    if side != os.path.join(tmp, ".results.db.tray") or \
            not os.path.exists(side):
        fail("pack not written as the hidden sibling .<db>.tray")
    validate_names(tmp, db, side)

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
    # the reviews holding the pack let go before the re-pack: a pack a
    # review has open refuses it (floe/indexlock.py, 2026-10-09)
    ice.close()
    auto.close()
    r = subprocess.run([BIN, "drc", db], capture_output=True, text=True)
    if r.returncode != 0:
        fail("re-index rc=%d" % r.returncode)
    again = drc.load_db(db)
    if not isinstance(again, drc.IcePack):
        fail("rebuilt pack not auto-picked")
    # the results pack again, for the foreign-pack check below
    ice = drc.IcePack(side, src_path=db, verify_src=True)
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

    # D9: waive autosave - save-as/load, refusal, read-only pack,
    # corrupt-aside, in-pack seed migration
    # reviewer tag: the shared server account means the account
    # name is NOT unique (user call 2026-08-28) - the tag prefers
    # FLOE_REVIEWER > DISPLAY host (direct X) > SSH client IP >
    # account name, and local/forwarded DISPLAYs are skipped
    env = os.environ
    saved = {k: env.pop(k, None) for k in
             ("FLOE_REVIEWER", "DISPLAY", "SSH_CONNECTION",
              "SSH_CLIENT")}
    try:
        env["DISPLAY"] = "ws-kim:0.0"
        if drc._waive_user() != "ws-kim":
            fail("DISPLAY host not used for the reviewer tag")
        env["DISPLAY"] = "localhost:10.0"
        env["SSH_CONNECTION"] = "192.168.1.50 55555 10.0.0.1 22"
        if drc._waive_user() != "192.168.1.50":
            fail("SSH client IP not used under X forwarding")
        env["FLOE_REVIEWER"] = "kim review!"
        if drc._waive_user() != "kim_review_":
            fail("FLOE_REVIEWER override not honoured/sanitized")
        for k in ("FLOE_REVIEWER", "DISPLAY", "SSH_CONNECTION"):
            del env[k]
        if not drc._waive_user():
            fail("no-signal fallback produced an empty tag")
    finally:
        for k, v in saved.items():
            if v is None:
                env.pop(k, None)
            else:
                env[k] = v
    # the --floe-reviewer CLI parameter keys the autosave (launcher
    # scripts pass an argument instead of exporting FLOE_REVIEWER)
    root = os.path.join(os.path.dirname(__file__), "..")
    r = subprocess.run(
        [sys.executable, "-m", "floe", "drc", db, "--rules",
         "--floe-reviewer", "gatecli"],
        capture_output=True, text=True, cwd=root)
    if r.returncode != 0:
        fail("floe drc --floe-reviewer rc=%d: %s"
             % (r.returncode, r.stderr.strip()))
    cliside = os.path.join(tmp, ".results.db.waive.gatecli")
    if not os.path.isfile(cliside):
        fail("--floe-reviewer did not key the autosave: %s"
             % cliside)
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

    # D10: per-reviewer error notes (flateyes .fe sidecar) - a shared
    # note across errors, single/group clear, reopen persistence,
    # flateyes readability, export/import round-trip + foreign refusal
    from floe import fe_embed
    os.environ["FLOE_REVIEWER"] = "gate"
    try:
        npk = drc.IcePack(gp)
        g0, g1 = npk.error_gid(3, 0), npk.error_gid(3, 1)
        g2 = npk.error_gid(4, 0)
        npk.set_note([g0, g1], "공유 노트 shared")
        if npk.get_note_gid(g0) != "공유 노트 shared" \
                or npk.get_note_gid(g1) != "공유 노트 shared":
            fail("shared note not attached to both members")
        if npk.get_note_gid(g2) is not None:
            fail("note leaked to an unrelated error")
        if npk.notes_list() != [("공유 노트 shared", sorted([g0, g1]))]:
            fail("notes_list wrong: %r" % (npk.notes_list(),))
        nside = drc.notes_autosave_path(gp)
        if not os.path.isfile(nside) \
                or os.path.dirname(nside) != os.path.dirname(gp):
            fail("note autosave not beside the pack: %s" % nside)
        # the autosave IS a valid flateyes sidecar (fe_embed reads it)
        with open(nside, encoding="utf-8") as f:
            fannos = fe_embed.parse_metadata(f.read())[0]
        if [a["text"] for a in fannos if a["kind"] == "text"] \
                .count("공유 노트 shared") != 2:
            fail("flateyes cannot read the note as text annotations")
        npk.close()
        # persists across reopen
        re3 = drc.IcePack(gp)
        if re3.get_note_gid(g0) != "공유 노트 shared" \
                or re3.notes_list() != [("공유 노트 shared",
                                         sorted([g0, g1]))]:
            fail("notes did not persist across reopen")
        # single-member clear keeps the note for the other member
        re3.clear_note([g0])
        if re3.get_note_gid(g0) is not None \
                or re3.get_note_gid(g1) != "공유 노트 shared":
            fail("single-member clear wrong")
        # group clear drops every note and removes the file
        re3.set_note([g0, g2], "second")
        re3.clear_note([g1, g0, g2])
        if re3.notes_list() or os.path.exists(nside):
            fail("group clear left notes/file behind")
        # export -> clear -> import round-trip
        re3.set_note([g0, g1], "exported")
        exp = os.path.join(tmp, "notes_export.fe")
        re3.note_export(exp)
        re3.clear_note([g0, g1])
        if re3.note_import(exp) != 1 \
                or re3.get_note_gid(g0) != "exported":
            fail("note export/import round-trip failed")
        # a file recorded against another pack is refused, state kept
        with open(exp, encoding="utf-8") as f:
            blob = f.read()
        bad = os.path.join(tmp, "notes_bad.fe")
        with open(bad, "w", encoding="utf-8") as f:
            f.write(blob.replace("floe_pack=", "floe_pack=9,9,"))
        try:
            re3.note_import(bad)
            fail("foreign-pack note file accepted")
        except ValueError:
            pass
        if re3.get_note_gid(g0) != "exported":
            fail("refused import disturbed the notes")
        re3.clear_note([g0, g1])
        re3.close()
    finally:
        os.environ.pop("FLOE_REVIEWER", None)
    if sha(gp) != gh0:
        fail("D10 modified the pack")
    print("D10 OK: per-reviewer notes + flateyes .fe + share/clear/"
          "export/import")

    # D11: bundled dubeolsik hangul composer for the note editor
    # (flateyes port; GTK-free, so unit-testable here). Feed a
    # keystroke string through the same path the key handler uses.
    from floe.hangul import HangulComposer

    def compose(keys):
        c = HangulComposer()
        out = ""
        for ch in keys:
            jamo = (HangulComposer.KEYMAP.get(ch)
                    or HangulComposer.KEYMAP.get(ch.lower()))
            if jamo is None:
                out += c.preedit()
                c.reset()
                out += ch
                continue
            committed, _preedit = c.feed(jamo)
            out += committed
        return out + c.preedit()

    for keys, want in (("dkssud", "안녕"), ("gksrmf", "한글"),
                       ("rks", "간"), ("dhkd", "왕"),
                       ("gksrmf dkssud", "한글 안녕")):
        got = compose(keys)
        if got != want:
            fail("hangul compose %r -> %r != %r" % (keys, got, want))
    # backspace decomposes a syllable one component at a time
    c = HangulComposer()
    for j in ("ㅎ", "ㅏ", "ㄴ"):
        c.feed(j)
    if c.preedit() != "한" or c.backspace() != "하" \
            or c.backspace() != "ㅎ" or c.backspace() != "":
        fail("hangul backspace decomposition wrong")
    print("D11 OK: bundled hangul composer (compose + backspace)")

    validate_service(tempfile.mkdtemp(prefix="floe-drcsvc-"))
    print("DRC ICE VALIDATION: ALL OK")


if __name__ == "__main__":
    main()
