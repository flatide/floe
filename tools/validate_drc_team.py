"""DRC team review gate (feature/jobdeck-drc 2026-10-09..11, ported to Rust:
app-core drc::team and drc::matcher behind floe2 gtk-service): the
service's review flags, snapshots, team aggregate, application and
migration from a previous run equal the Python reference's (tools/oracle
floe_oracle drc_review, drc_team, drc_migrate - the branch's own modules)
file for file, and the GTK viewer shows them.

Two reviewers work the same pack side by side - "py" through the
reference, "rs" through the service - with the same edits; their files
must match row for row (snapshots: all but the reviewer, id and time).

  R1  visited / not-required flags: `.<db>.review.<reviewer>.sqlite3`
      tables, counts per rule and per database, every error's flags; a
      file of another result refused; FLOE_DRC_REVIEW_ROOT; reads never
      create the file; the statistics CSV.
  R2  snapshots: an export of one's own review, both ways loaded; a
      working review file with its waive file and an old waive file
      converted (unknown dimensions kept unknown).
  R3  the team aggregate of two peers and one's own (conflicts only
      between visited reviewers), import conflicts, the conflict
      selection under each waive filter; member pages.
  R4  application: safe (one's visited errors with another decision
      kept) and forced on selected errors - the waive file bytes and the
      flags tables after it.
  R5  review from a previous run: exact geometry matches (moved,
      duplicate, reordered, renamed and duplicate-named rules), the plan's
      counts, pages and carried states, its application; a review
      snapshot as the source; waives embedded in an old pack; refusals
      (same pack, same result, no review).
  R6  `floe2 drc-match` (protocol 2) = the branch's own test cases.
  R7  the viewer (pycairo blocked): visited underlines / not-required gray numbers
      and the review statistics,
      a visit recorded only when an error's geometry is drawn, not for
      review from the list's menu, the statistics CSV; the reviewers
      window (load a peer, refresh, pages, safe apply, conflict only);
      the review from previous run window (compare, pages, apply).
  R8  tools/gen_drc_samples.py (the manual-test samples): its files, the
      layout indexed, the reviewers' conflicts and the previous run's
      matches it promises.
  R9  one session a reviewer and result (drc::session): a second process
      of the same reviewer id is refused and told who has it, the same
      process and a later session are not, a killed holder holds nothing;
      FLOE_REVIEWER_REQUIRED refuses a review and a waive read without a
      stated id (2026-11-10).
  R10 `floe2 drc --review-stats`: every reviewer found by the working
      files, each one's counts and the team's - by hand, and the reviewers
      window's; stored counters compared and never counted, leftovers and
      another result's files, the reviewers named, a stale or missing pack
      refused, --workdir (2026-11-10).

usage: .venv/bin/python tools/validate_drc_team.py
"""

import json
import os
import shutil
import sqlite3
import struct
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path[:0] = [str(ROOT / "tools"), str(ROOT / "tools" / "oracle"), str(ROOT)]
FLOE2 = os.environ.get("FLOE2_BIN") or str(ROOT / "rust/target/release/floe2")
os.environ["FLOE2_BIN"] = FLOE2
INDEX = str(ROOT / "rust/target/release/floe-index")
os.environ.pop("FLOE_DRC_REVIEW_ROOT", None)

from floe import gtkservice  # noqa: E402
from floe_oracle import drc as odrc  # noqa: E402
from floe_oracle import drc_team as oteam  # noqa: E402
from floe_oracle import drc_migrate as omigrate  # noqa: E402
from floe_oracle.drc_selection import Selection as OSelection  # noqa: E402

FAILED = []


def check(ok, what, detail=""):
    if ok:
        print("  %-66s ok" % what)
    else:
        FAILED.append(what)
        print("  %-66s FAIL %s" % (what, str(detail)[:800]))


def run(args, **kw):
    return subprocess.run(args, capture_output=True, text=True, timeout=1800, **kw)


class Svc(object):
    def __init__(self):
        self.svc = gtkservice.Service(FLOE2)
        self.pending = []

    def __call__(self, op, **fields):
        return self.svc.request(op, **fields)

    def job(self, handle, op, **fields):
        self.svc.drc_watch(handle)
        job = self.svc.request(op, handle=handle, **fields)["job"]
        deadline = time.monotonic() + 600
        while time.monotonic() < deadline:
            for e in self.svc.drc_events(handle):
                if e.get("kind") == "visit":
                    self.pending.append(e)
                    continue
                if e.get("job") != job:
                    continue
                if e["state"] in ("done", "error", "cancelled"):
                    return e
            time.sleep(0.005)
        raise AssertionError("no answer to %s" % op)

    def visits(self, handle, n):
        """Wait for `n` visit events."""
        got, self.pending = list(self.pending), []
        self.svc.drc_watch(handle)
        deadline = time.monotonic() + 60
        while len(got) < n and time.monotonic() < deadline:
            for e in self.svc.drc_events(handle):
                if e.get("kind") == "visit":
                    got.append(e)
            time.sleep(0.005)
        if len(got) < n:
            check(False, "%d of %d visit events" % (len(got), n))
        return got

    def close(self):
        self.svc.close()


# ---- data --------------------------------------------------------------

def _poly(x, y, w=40, h=20):
    return "p 1 4\n%d %d\n%d %d\n%d %d\n%d %d\n" % (x, y, x + w, y, x + w, y + h, x, y + h)


def _edge(x, y, d=30):
    return "e 1 2\n%d %d %d %d\n%d %d %d %d\n" % (x, y, x + 50, y, x, y + d, x + 50, y + d)


def write_db(path, rules, precision=1000):
    """rules: [(name, [record text...])]; numbers rewritten in order."""
    out = ["TOP %d\n" % precision]
    for name, records in rules:
        out.append("%s\n%d %d 0\n" % (name, len(records), len(records)))
        for i, rec in enumerate(records):
            kind, _num, rest = rec.split(" ", 2)
            out.append("%s %d %s" % (kind, i + 1, rest))
    Path(path).write_text("".join(out))


def other_bytes(raw):
    """A .db's bytes with its last coordinate digit changed: the same size,
    another result."""
    body = raw.rstrip(b"\n")
    assert body[-1:].isdigit(), body[-20:]
    return body[:-1] + (b"1" if body[-1:] != b"1" else b"2") + raw[len(body):]


def runs(d):
    """run1 (the previous result) and run2 (this one)."""
    a = [_poly(100 * (i % 120), 60 * (i // 120)) for i in range(9000)]
    a += [_poly(0, 0)] * 2 + [_poly(100, 0)]           # duplicates of errors 0 and 1
    b = [_edge(200 * (i % 50), 100 * (i // 50)) for i in range(5000)]
    c = [_poly(7 * i, 3 * i, 5, 5) for i in range(700)]
    d1 = [_poly(i, i) for i in range(10)]
    e = [_poly(-1000 - 10 * i, -1000, 8, 8) for i in range(4500)]
    write_db(d / "run1.db", [("A.WIDTH", a), ("B.SPACE", b), ("C.GONE", c),
                             ("D.DUP", d1), ("D.DUP", d1), ("E.AREA", e)])
    a2 = list(a[:3000])
    a2 += [_poly(100 * (i % 120) + 1, 60 * (i // 120)) for i in range(3000, 5000)]   # moved
    a2 += list(reversed(a[5000:9000]))                                              # reordered
    a2 += [_poly(5, 5, 9, 9)] * 3 + [_poly(0, 0)]       # new, and one more duplicate
    b2 = b[2500:] + b[:2500]
    e2 = e[:4000] + [_poly(-5, -5)] * 600
    write_db(d / "run2.db", [("B.SPACE", b2), ("A.WIDTH", a2), ("D.DUP", d1[:5]),
                             ("E.AREA", e2), ("F.NEW", [_poly(1, 1)] * 20)])
    for name in ("run1.db", "run2.db"):
        r = run([INDEX, "drc", str(d / name)])
        check(r.returncode == 0, "floe-index drc %s" % name, r.stderr)


# ---- file comparisons ---------------------------------------------------

def dump(path, skip=("snapshot_id", "exported_ns", "reviewer")):
    con = sqlite3.connect("file:%s?mode=ro" % path, uri=True)
    try:
        schema = sorted(r[0] for r in con.execute(
            "SELECT sql FROM sqlite_master WHERE sql IS NOT NULL"))
        tables = {}
        for (name,) in con.execute("SELECT name FROM sqlite_master WHERE type='table'"):
            rows = sorted(con.execute("SELECT * FROM %s" % name))
            if name == "metadata":
                meta = json.loads(rows[0][1])
                for k in skip:
                    meta.pop(k, None)
                for k in ("source_path", "source_waive_path", "sources"):
                    if k in meta and k in skip:
                        meta.pop(k)
                rows = meta
            tables[name] = rows
        return schema, tables
    finally:
        con.close()


def same_files(a, b, what, skip=("snapshot_id", "exported_ns", "reviewer")):
    da, db_ = dump(a, skip), dump(b, skip)
    rows = len(da[1].get("chunks") or ())
    what = "%s [%d chunks]" % (what, rows)
    if da == db_:
        check(rows > 0, what, "nothing compared")
        return
    detail = []
    if da[0] != db_[0]:
        detail.append(("schema", da[0], db_[0]))
    for t in sorted(set(da[1]) | set(db_[1])):
        if da[1].get(t) != db_[1].get(t):
            x, y = da[1].get(t), db_[1].get(t)
            if isinstance(x, list) and isinstance(y, list):
                diff = [r for r in x if r not in y][:2] + [r for r in y if r not in x][:2]
                detail.append((t, len(x), len(y), [str(r)[:160] for r in diff]))
            else:
                detail.append((t, x, y))
    check(False, what, detail)


def py_db(path, reviewer):
    os.environ["FLOE_REVIEWER"] = reviewer
    try:
        return odrc.IcePack(odrc.cachepath.pack_path(str(path)), src_path=str(path),
                            verify_src=True)
    finally:
        os.environ.pop("FLOE_REVIEWER", None)


def rs_open(svc, path, reviewer):
    opened = svc("drc_open", path=str(path), mode="load", reviewer=reviewer)
    # the visit events come from the session's thread: keep them from the start
    svc.svc.drc_watch(opened["handle"])
    return opened


# ---- R1 ------------------------------------------------------------------

EDITS = {  # rule index in run2: (waived, visited, not-required on, off)
    0: ([1, 2, 3, 2600, 4999], [0, 1, 7, 8, 4096, 4097, 4999], [9, 10, 11, 4999], [10]),
    1: ([0, 3000, 9002], [0, 1, 3000, 3001, 8999, 9002], list(range(100, 4200)), list(range(4000, 4100))),
    3: (list(range(0, 4600, 7)), [5, 6, 4100], [], []),
}


def apply_edits(od, svc, h, edits):
    for ci, (waive, visits, nr_on, nr_off) in edits.items():
        if waive:
            for ei in waive:
                od.set_status(ci, ei, 1)
            svc("drc_set_status", handle=h, check=ci, errors=waive, status=1)
        for ei in visits:
            od.mark_visited(ci, ei)
            svc("drc_visit", handle=h, check=ci, error=ei)
        svc.visits(h, len(visits))
        size = len(od.checks[ci].errors)
        for members, on in ((nr_on, True), (nr_off, False)):
            if members:
                od.set_review_members(ci, OSelection.from_indices(size, members), on)
                e = svc.job(h, "drc_review", check=ci, action="not-required",
                            errors=members, on=on)
                if e["state"] != "done":
                    check(False, "not-required job", e)


def r1(svc, d):
    print("[R1] visited / not-required flags")
    od = py_db(d / "run2.db", "py")
    opened = rs_open(svc, d / "run2.db", "rs")
    h = opened["handle"]
    path_rs = svc("drc_review_path", handle=h)["path"]
    check(path_rs == str(d / ".run2.db.review.rs.sqlite3"), "the flags file beside the db", path_rs)
    check(not os.path.exists(path_rs), "no file before the first edit")
    counts = svc("drc_review_counts", handle=h)
    check(counts == dict(od.review_counts()), "counts of an absent file = the reference", counts)
    check(not os.path.exists(path_rs), "reads never create the file")
    apply_edits(od, svc, h, EDITS)
    path_py = od.review_path
    same_files(path_py, path_rs, "flags tables (metadata, totals, rules, chunks)", skip=())
    bad = []
    for ci, c in enumerate(od.checks):
        if svc("drc_review_counts", handle=h, check=ci) != dict(od.review_counts(ci)):
            bad.append(("counts", ci))
        n = len(c.errors)
        got = []
        for start in range(0, n, 4000):
            got += svc("drc_review_flags", handle=h, check=ci,
                       errors=list(range(start, min(n, start + 4000))))
        want = [od.review_flags(ci, ei) for ei in range(n)]
        if got != want:
            bad.append(("flags", ci))
    check(not bad and svc("drc_review_counts", handle=h) == dict(od.review_counts()),
          "every error's flags, counts per rule and database", bad)
    # the statistics CSV (the service's job against the reference's rows; the
    # viewer's menu and panel no longer ask for it, 2026-11-10 - the numbers
    # of every reviewer are `floe2 drc DB --review-stats`)
    out = d / "stats.csv"
    e = svc.job(h, "drc_review_stats", path=str(out))
    import csv
    rows = list(csv.reader(open(out, newline="")))
    want = [["scope", "rule_index", "rule", "total", "visited", "not_required",
             "visited_not_required", "unvisited_targets"]]
    keys = ("total", "visited", "not_required", "visited_not_required", "pending")
    tot = dict.fromkeys(keys, 0)
    per = []
    for ci, c in enumerate(od.checks):
        k = od.review_counts(ci)
        per.append(["rule", str(ci + 1), c.name] + [str(k[x]) for x in keys])
        for x in keys:
            tot[x] += k[x]
    want.append(["database", "", ""] + [str(tot[x]) for x in keys])
    check(e["state"] == "done" and rows == want + per, "the statistics CSV", (e, rows[:3]))
    od.close()
    return h


def r1_identity(d):
    print("[R1b] a file of another result, read-only results, FLOE_DRC_REVIEW_ROOT")
    other = Svc()
    try:
        # a run1 flags file under run2's name (the DRC ran again in place):
        # of another result - moved aside as the waive sidecar is, its bytes
        # kept, and this result's review starts (0.12.441; until then the
        # file was refused where it lay and the statistics stayed out)
        h = rs_open(other, d / "run1.db", "copy")["handle"]
        other("drc_visit", handle=h, check=0, error=0)
        other.visits(h, 1)
        other("drc_close", handle=h)
        src = d / ".run1.db.review.copy.sqlite3"
        dst = d / ".run2.db.review.copy.sqlite3"
        shutil.copy(src, dst)
        before = dst.read_bytes()
        opened = rs_open(other, d / "run2.db", "copy")
        h = opened["handle"]
        aside = sorted(d.glob(".run2.db.review.copy.sqlite3.stale-*"))
        check(len(aside) == 1 and aside[0].read_bytes() == before and not dst.exists(),
              "another result's flags are moved aside, kept as they were", aside)
        check(any("review flags do not match this pack" in n for n in opened["notices"]),
              "... and the open says so", opened["notices"])
        check(other("drc_review_counts", handle=h)["visited"] == 0,
              "this result's review starts empty")
        other("drc_visit", handle=h, check=0, error=1)
        other.visits(h, 1)
        check(other("drc_review_counts", handle=h)["visited"] == 1 and dst.exists(),
              "... and records anew")
        other("drc_close", handle=h)
        for path in aside + [dst]:
            path.unlink()
    finally:
        other.close()
    # a results folder and temp folder that cannot be written: the pack's
    # embedded statuses are shown read-only (never written for one
    # reviewer), the flags go to $XDG_DATA_HOME/floe/review/<key>/
    ro = d / "ro"
    ro.mkdir()
    shutil.copy2(d / "run2.db", ro / "run2.db")
    shutil.copy2(d / ".run2.db.tray", ro / ".run2.db.tray")
    notmp, xdg = d / "no-tmp", d / "xdg"
    notmp.mkdir()
    xdg.mkdir()
    for p in (ro, notmp):
        os.chmod(p, 0o555)
    saved = {k: os.environ.get(k) for k in ("TMPDIR", "XDG_DATA_HOME")}
    os.environ.update(TMPDIR=str(notmp), XDG_DATA_HOME=str(xdg))
    fallback = Svc()
    try:
        opened = rs_open(fallback, ro / "run2.db", "ro")
        h = opened["handle"]
        check(opened["read_only"] and any("read-only" in n for n in opened["notices"]),
              "no waive file possible: the embedded statuses read-only", opened["notices"])
        before = (ro / ".run2.db.tray").read_bytes()
        try:
            fallback("drc_set_status", handle=h, check=0, errors=[0], status=1)
            check(False, "a waive refused")
        except gtkservice.ServiceError as exc:
            check("read-only" in str(exc), "a waive refused", exc)
        check((ro / ".run2.db.tray").read_bytes() == before, "the shared pack unchanged")
        fallback("drc_visit", handle=h, check=0, error=1)
        fallback.visits(h, 1)
        got = fallback("drc_review_path", handle=h)["path"]
        from floe_oracle.drc_review import _fallback_path
        os.environ["XDG_DATA_HOME"] = str(xdg)
        want = _fallback_path(str(ro / ".run2.db.tray"), "ro")
        check(got == want and fallback("drc_review_counts", handle=h)["visited"] == 1,
              "the flags in the durable fallback = the reference's", (got, want))
    finally:
        fallback.close()
        for p in (ro, notmp):
            os.chmod(p, 0o755)
        for k, v in saved.items():
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v
    root = d / "review-root"
    os.environ["FLOE_DRC_REVIEW_ROOT"] = str(root)
    rooted = Svc()
    try:
        h = rs_open(rooted, d / "run2.db", "rooted")["handle"]
        rooted("drc_visit", handle=h, check=0, error=3)
        rooted.visits(h, 1)
        got = rooted("drc_review_path", handle=h)["path"]
        from floe_oracle.drc_review import review_path
        want = review_path(odrc.cachepath.pack_path(str(d / "run2.db")), "rooted")
        check(got == want and os.path.exists(got), "FLOE_DRC_REVIEW_ROOT/<key>/ = the reference", (got, want))
    finally:
        rooted.close()
        os.environ.pop("FLOE_DRC_REVIEW_ROOT", None)


def r1_reindex(d):
    print("[R1c] unchanged reindex preserves the pack and its review")
    folder = d / "reindex"
    folder.mkdir()
    src, pack = folder / "same.db", folder / ".same.db.tray"
    write_db(src, [("M1.S", [_poly(100, 100), _poly(200, 100), _edge(300, 100)])])
    r = run([INDEX, "drc", str(src)])
    check(r.returncode == 0, "initial reindex fixture", r.stderr)
    # Pin an old ns timestamp before creating any review. Replacing a
    # byte-identical pack will deterministically invalidate that review.
    stamp = 1_700_000_000_123_456_789
    os.utime(pack, ns=(stamp, stamp))
    before = pack.read_bytes()
    stat = pack.stat()
    identity = (stat.st_dev, stat.st_ino, stat.st_mtime_ns)
    svc = Svc()
    try:
        h = rs_open(svc, src, "reindex")["handle"]
        svc("drc_visit", handle=h, check=0, error=0)
        svc.visits(h, 1)
        e = svc.job(h, "drc_review", check=0, action="not-required", errors=[0, 1], on=True)
        check(e["state"] == "done", "exclude errors before reindex", e)
        svc("drc_set_status", handle=h, check=0, errors=[1], status=1)
        svc("drc_set_note", handle=h, gids=[2], text="keep this note")
        counts = svc("drc_review_counts", handle=h)
        flags = svc("drc_review_flags", handle=h, check=0, errors=[0, 1, 2])
        review_path = Path(svc("drc_review_path", handle=h)["path"])
        svc("drc_close", handle=h)
    finally:
        svc.close()
    review_bytes = review_path.read_bytes()
    # (0.12.459: a plain run of the packer keeps a pack that is current and
    # packs nothing - said, the file as it was. The runs below say --force:
    # the re-pack whose identical bytes keep the file)
    r = run([INDEX, "drc", str(src)])
    stat = pack.stat()
    check(r.returncode == 0 and "kept; --force packs it again" in r.stderr
          and "packed v2" not in r.stderr
          and (stat.st_dev, stat.st_ino, stat.st_mtime_ns) == identity and pack.read_bytes() == before,
          "a plain reindex keeps the current pack without packing", r.stderr)
    for jobs, stream in ((1, False), (4, True)):
        env = dict(os.environ)
        if stream:
            env["FLOE_DRC_QBOX_RESIDENT"] = "0"
        r = run([INDEX, "drc", str(src), "--force", "--jobs", str(jobs)], env=env)
        check(r.returncode == 0 and "packed v2" in r.stderr, "reindex with jobs=%d" % jobs, r.stderr)
        stat = pack.stat()
        check((stat.st_dev, stat.st_ino, stat.st_mtime_ns) == identity
              and pack.read_bytes() == before, "identical reindex keeps pack inode and ns mtime")
        svc = Svc()
        try:
            reopened = rs_open(svc, src, "reindex")
            h = reopened["handle"]
            try:
                check(svc("drc_review_counts", handle=h) == counts
                      and svc("drc_review_flags", handle=h, check=0, errors=[0, 1, 2]) == flags,
                      "visited and not-required flags survive unchanged reindex")
            except gtkservice.ServiceError as exc:
                check(False, "visited and not-required flags survive unchanged reindex", exc)
            check(svc("drc_status", handle=h, check=0, start=0, count=3) == "000100",
                  "waive state survives unchanged reindex")
            check(any(n["text"] == "keep this note" for n in reopened["notes"]),
                  "note survives unchanged reindex")
            svc("drc_close", handle=h)
        finally:
            svc.close()
    # Same source stamp, rule/count metadata and file length, but one
    # coordinate differs: only a full byte comparison can retain the pack.
    # Layout 5's footer: 15 u64 | u32 cell | u32 source CRC-32 | 32 B content
    # id | 8 B magic - the sections lie where they lay, and the CRC and the
    # content id are another file's and other errors'.
    src_stat = src.stat()
    text = src.read_text()
    src.write_text(text.replace("100 100", "101 100", 1))
    os.utime(src, ns=(src_stat.st_atime_ns, src_stat.st_mtime_ns))
    # (0.12.459: under the size and the time that were packed a plain run
    # asks no more - they are every reader's first question, `floe2 drc
    # --build`'s too - and keeps the pack; --force packs what is there)
    r = run([INDEX, "drc", str(src)])
    check(r.returncode == 0 and "kept; --force packs it again" in r.stderr
          and pack.read_bytes() == before,
          "a coordinate change under the packed stamp: a plain run keeps the pack", r.stderr)
    r = run([INDEX, "drc", str(src), "--force"])
    after = pack.read_bytes()
    check(r.returncode == 0 and after != before and len(after) == len(before)
          and after[:40] == before[:40] and after[-168:-44] == before[-168:-44]
          and after[-8:] == before[-8:],
          "--force replaces the pack: a coordinate change despite the matching source stamp", r.stderr)
    check(after[-44:-40] != before[-44:-40] and after[-40:-8] != before[-40:-8],
          "... with another source CRC and another content id")
    check(review_path.read_bytes() == review_bytes, "reindex never rewrites a saved review")
    waive_path = folder / ".same.db.waive.reindex"
    waive_bytes = waive_path.read_bytes()
    svc = Svc()
    try:
        # other errors: the review of the result before is moved aside whole
        # (0.12.441; until then the flags file was refused where it lay) and
        # this result's starts - `review from a previous run` carries it over
        opened = rs_open(svc, src, "reindex")
        h = opened["handle"]
        flags_aside = sorted(folder.glob(review_path.name + ".stale-*"))
        waive_aside = sorted(folder.glob(waive_path.name + ".stale-*"))
        check(len(flags_aside) == 1 and flags_aside[0].read_bytes() == review_bytes
              and not review_path.exists(),
              "changed geometry moves the old review's flags aside, kept as they were", flags_aside)
        check(len(waive_aside) == 1 and waive_aside[0].read_bytes() == waive_bytes,
              "... and its waives", waive_aside)
        check(any("review flags do not match this pack" in n for n in opened["notices"])
              and any("waive autosave does not match this pack" in n for n in opened["notices"]),
              "... and the open says both", opened["notices"])
        blank = svc("drc_review_counts", handle=h)
        check(blank["visited"] == 0 and blank["not_required"] == 0
              and svc("drc_status", handle=h, check=0, start=0, count=3) == "000000"
              and not any(n["text"] == "keep this note" for n in opened["notes"]),
              "... and this result's review starts empty", (blank, opened["notes"]))
        svc("drc_close", handle=h)
    finally:
        svc.close()


# ---- R2 ------------------------------------------------------------------


def team_load(svc, h, paths):
    e = svc.job(h, "drc_team_load", paths=[str(p) for p in paths])
    if e["state"] != "done":
        raise AssertionError(e)
    return e


def snap_view(svc, h, sid, n_rules, kinds=("work", "visited", "waived", "not_required", "conflict")):
    out = {"counts": svc("drc_snapshot_counts", handle=h, snapshot=sid)}
    for ci in range(n_rules):
        out[ci] = {"counts": svc("drc_snapshot_counts", handle=h, snapshot=sid, check=ci)}
        for k in kinds:
            page = svc("drc_snapshot_page", handle=h, snapshot=sid, check=ci, kind=k,
                       start=0, limit=1 << 20)
            out[ci][k] = (page["count"], page["ids"])
    return out


def py_view(snap, n_rules, kinds=("work", "visited", "waived", "not_required", "conflict")):
    def counts(c):
        c = dict(c)
        if c.get("reviewers") is None:
            c.pop("reviewers", None)
        return c
    out = {"counts": counts(snap.counts())}
    for ci in range(n_rules):
        out[ci] = {"counts": counts(snap.counts(ci))}
        for k in kinds:
            m = snap.members(ci, k)
            out[ci][k] = (m.count(), m.page(0, 1 << 20))
    return out


def r2(svc, d, h):
    print("[R2] snapshots: export, load both ways, conversions")
    od = py_db(d / "run2.db", "py")
    n = len(od.checks)
    py_path, rs_path = d / "py.floereview", d / "rs.floereview"
    oteam.export_review(od, str(py_path)).close()
    e = svc.job(h, "drc_team_export", path=str(rs_path))
    check(e["state"] == "done", "export job", e)
    same_files(py_path, rs_path, "an export = the reference's (rules, chunks, metadata)")
    loaded = team_load(svc, h, [py_path])["loaded"][0]
    ref = oteam.ReviewSnapshot(od, str(py_path))
    check(snap_view(svc, h, loaded["snapshot"], n) == py_view(ref, n),
          "the reference's export read by the service: counts and pages")
    back = oteam.open_review(od, str(rs_path))
    check(py_view(back, n) == py_view(ref, n), "the service's export read by the reference")
    ref.close()
    back.close()
    # a working review file (and the waive file paired by name)
    work = d / ".run2.db.review.py.sqlite3"
    conv_py = oteam.open_review(od, str(work))
    got = team_load(svc, h, [work])["loaded"][0]
    check(got["visited_known"] and got["waive_known"] and got["not_required_known"]
          and got["reviewer"] == "py"
          and got["source_waive_path"] == str(d / ".run2.db.waive.py"),
          "a working review file: its waive file paired, all known", got)
    same_files(conv_py.path, got["path"], "a working review file converted = the reference",
               skip=("snapshot_id", "exported_ns"))
    check(snap_view(svc, h, got["snapshot"], n) == py_view(conv_py, n), "its counts and pages")
    conv_py.close()
    waive = d / ".run2.db.waive.py"
    conv_py = oteam.open_review(od, str(waive))
    got = team_load(svc, h, [waive])["loaded"][0]
    check(not got["visited_known"] and got["waive_known"] and not got["not_required_known"],
          "a waive file: visits and exclusions unknown", got)
    same_files(conv_py.path, got["path"], "a waive file converted = the reference",
               skip=("snapshot_id", "exported_ns"))
    conv_py.close()
    e = svc.job(h, "drc_team_load", paths=[str(d / "run1.db"), str(d / ".run1.db.tray")])
    check(len(e["errors"]) == 2 and not e["loaded"], "a non-review file refused per file", e)
    od.close()
    return py_path, rs_path


# ---- R3 ------------------------------------------------------------------

PEERS = {
    "p1": {0: ([1, 5], [1, 2, 5, 9, 10], [2, 9], []), 1: ([3001, 0], [0, 3001, 9002], [9002], [])},
    "p2": {0: ([2, 9], [1, 2, 3, 9], [1], []), 1: ([], [0, 1, 100], [1], [])},
}


def peers(d):
    out = {}
    for name, edits in PEERS.items():
        od = py_db(d / "run2.db", name)
        for ci, (waive, visits, nr, _off) in edits.items():
            for ei in waive:
                od.set_status(ci, ei, 1)
            for ei in visits:
                od.mark_visited(ci, ei)
            if nr:
                od.set_review_members(ci, OSelection.from_indices(len(od.checks[ci].errors), nr), True)
        p = d / ("%s.floereview" % name)
        oteam.export_review(od, str(p)).close()
        od.close()
        out[name] = p
    return out


def r3(svc, d, h):
    print("[R3] team aggregate, import conflicts, conflict selection")
    od = py_db(d / "run2.db", "py")
    n = len(od.checks)
    pp = peers(d)
    py_sources = [oteam.open_review(od, str(p)) for p in pp.values()]
    loaded = team_load(svc, h, list(pp.values()))["loaded"]
    sids = [x["snapshot"] for x in loaded]
    agg_py = oteam.aggregate_reviews(od, py_sources)
    e = svc.job(h, "drc_team_aggregate", sources=sids)
    check(e["state"] == "done", "aggregate job", e)
    agg = e["snapshot"]
    same_files(agg_py.path, e["info"]["path"], "the aggregate = the reference's",
               skip=("snapshot_id", "exported_ns", "sources"))
    check(snap_view(svc, h, agg, n) == py_view(agg_py, n), "its counts (reviewers) and pages")
    bad = []
    for ci in range(n):
        size = len(od.checks[ci].errors)
        got = svc("drc_snapshot_get", handle=h, snapshot=agg, check=ci,
                  errors=list(range(size)))
        want = [list(agg_py.get(ci, ei)) for ei in range(size)]
        if got != want:
            bad.append(ci)
    check(not bad, "every error's team state", bad)
    conflicts = agg_py.counts()["conflicts"]
    check(conflicts > 0, "the peers disagree somewhere (%d conflicts)" % conflicts)
    # import conflicts with p1
    imp_py = oteam.import_conflicts(od, py_sources[0])
    e = svc.job(h, "drc_team_import", source=sids[0])
    check(e["state"] == "done", "import conflicts job", e)
    imp = e["snapshot"]
    check(snap_view(svc, h, imp, n) == py_view(imp_py, n), "import conflicts = the reference's")
    # the conflict selection under each waive filter
    bad = []
    for ci in range(n):
        for waived in (None, True, False):
            sel = oteam.prepare_conflict_selection(od, agg_py, ci, waived=waived)
            want = sel.page(0, 1 << 20)
            e = svc.job(h, "drc_team_conflicts", aggregate=agg, check=ci, waived=waived)
            if e["state"] != "done":
                bad.append((ci, waived, e))
                continue
            got = svc("drc_selection_page", handle=h, selection=e["selection"], start=0,
                      limit=1 << 20)
            if got != want or e["total"] != len(want):
                bad.append((ci, waived, got[:5], want[:5]))
    check(not bad, "the conflict selection of every rule and waive filter", bad[:3])
    for s in py_sources:
        s.close()
    agg_py.close()
    imp_py.close()
    od.close()
    return pp, sids


# ---- R4 ------------------------------------------------------------------

def r4(svc, d, h, pp, sids):
    print("[R4] application: safe, forced on selected errors")
    od = py_db(d / "run2.db", "py")
    src = oteam.open_review(od, str(pp["p1"]))
    want = oteam.apply_review(od, src)
    e = svc.job(h, "drc_team_apply", source=sids[0], force=False)
    check(e["state"] == "done" and e["result"] == want, "safe application = the reference's",
          (e.get("result"), want))
    check(want["conflicts_skipped"] > 0, "one's visited errors kept (%d)" % want["conflicts_skipped"])
    od.close()
    od = py_db(d / "run2.db", "py")
    src2 = oteam.open_review(od, str(pp["p2"]))
    members = OSelection.from_indices(len(od.checks[0].errors), [1, 2, 3, 9])
    want = oteam.apply_review(od, src2, ci=0, membership=members, force=True)
    e = svc.job(h, "drc_team_apply", source=sids[1], check=0, errors=[1, 2, 3, 9], force=True)
    check(e["state"] == "done" and e["result"] == want, "forced application on selected errors",
          (e.get("result"), want))
    od.close()
    a = (d / ".run2.db.waive.py").read_bytes()
    b = (d / ".run2.db.waive.rs").read_bytes()
    check(a == b, "the waive files after it, byte for byte",
          [i for i in range(min(len(a), len(b))) if a[i] != b[i]][:10])
    same_files(d / ".run2.db.review.py.sqlite3", d / ".run2.db.review.rs.sqlite3",
               "the flags tables after it", skip=())
    src.close()
    src2.close()


# ---- R5 ------------------------------------------------------------------

RUN1 = {  # rule index in run1
    0: ([0, 1, 2, 3, 3001, 5000, 8999, 9000], [0, 1, 3, 3001, 4100, 5000, 9001], [2, 4100, 8999], []),
    1: ([10, 2600], [10, 11, 4999], [12], []),
    2: ([1], [1], [], []),
    5: ([3, 4], [3, 4100], [5], []),
}


def plan_view(svc, h, pid, n, sizes):
    out = {"summary": svc("drc_plan_summary", handle=h, plan=pid)}
    for ci in range(n):
        out[ci] = {"counts": svc("drc_plan_counts", handle=h, plan=pid, check=ci)}
        for k in ("matched", "partial", "ambiguous", "conflict", "protected"):
            page = svc("drc_plan_page", handle=h, plan=pid, check=ci, kind=k, start=0, limit=1 << 20)
            out[ci][k] = (page["count"], page["ids"])
        out[ci]["get"] = svc("drc_plan_get", handle=h, plan=pid, check=ci,
                             errors=list(range(sizes[ci])))
    return out


def plan_ref(plan, n, sizes):
    out = {"summary": dict(plan.summary)}
    for ci in range(n):
        out[ci] = {"counts": plan.rule_counts(ci)}
        for k in ("matched", "partial", "ambiguous", "conflict", "protected"):
            m = plan.members(ci, k)
            out[ci][k] = (m.count(), m.page(0, 1 << 20))
        out[ci]["get"] = [None if v is None else list(v) for v in
                          (plan.get(ci, ei) for ei in range(sizes[ci]))]
    return out


def comparable(view):
    s = dict(view["summary"])
    for k in ("source_path", "old_pack", "state_path", "reviewer", "prior_snapshot_id"):
        s.pop(k, None)
    return dict(view, summary=s)


def r5(d):
    print("[R5] review from a previous run")
    svc = Svc()
    try:
        # the same previous review by both reviewers
        od1 = py_db(d / "run1.db", "py")
        h1 = rs_open(svc, d / "run1.db", "rs")["handle"]
        apply_edits(od1, svc, h1, RUN1)
        od1.close()
        svc("drc_close", handle=h1)
        od = py_db(d / "run2.db", "py")
        h = rs_open(svc, d / "run2.db", "rs")["handle"]
        n = len(od.checks)
        sizes = [len(c.errors) for c in od.checks]
        plan = omigrate.prepare_migration(od, str(d / "run1.db"))
        e = svc.job(h, "drc_migrate_prepare", previous=str(d / "run1.db"))
        check(e["state"] == "done", "prepare job", e)
        pid = e["plan"]
        got, want = comparable(plan_view(svc, h, pid, n, sizes)), comparable(plan_ref(plan, n, sizes))
        check(got["summary"] == want["summary"], "the plan's summary = the reference's",
              (got["summary"], want["summary"]))
        bad = [ci for ci in range(n) if got[ci] != want[ci]]
        check(not bad, "counts, member pages, carried states of every rule",
              [(ci, {k: (got[ci][k], want[ci][k]) for k in got[ci] if got[ci][k] != want[ci][k]})
               for ci in bad[:1]])
        s = want["summary"]
        check(s["matched"] > 0 and s["ambiguous"] > 0 and s["partial"] > 0
              and s["no_rule_match_source_work"] > 0,
              "matched %(matched)d, ambiguous %(ambiguous)d, partial %(partial)d, "
              "no rule %(no_rule_match_source_work)d" % s)
        res_py = omigrate.apply_migration(od, plan)
        e = svc.job(h, "drc_migrate_apply", plan=pid)
        check(e["state"] == "done" and e["result"] == res_py, "application = the reference's",
              (e.get("result"), res_py))
        after = comparable(plan_view(svc, h, pid, n, sizes))
        ref_after = comparable(plan_ref(plan, n, sizes))
        check(after == ref_after, "the plan after it (protected conflicts refreshed)")
        plan.close()
        od.close()
        a = (d / ".run2.db.waive.py").read_bytes()
        b = (d / ".run2.db.waive.rs").read_bytes()
        check(a == b, "the waive files after it, byte for byte")
        same_files(d / ".run2.db.review.py.sqlite3", d / ".run2.db.review.rs.sqlite3",
                   "the flags tables after it", skip=())
        # (0.12.465) what the application wrote is said in the review's change
        # log: the team's kept counts are the exact report's after it (R14)
        r, kept = counts_of(d / "run2.db", "--all-rules")
        r2, exact = stats_of(d / "run2.db")
        check(kept is not None and exact is not None and kept["team"]["current"] is True
              and kept["team"]["waived"] > 0
              and all(kept["team"][k] == exact["team"][k] for k in TEAM_KEYS)
              and [[x[k] for k in TEAM_KEYS] for x in kept["team"]["rules"]]
              == [[x[k] for k in TEAM_KEYS] for x in exact["team"]["rules"]],
              "the team's kept counts after it = the exact report's", kept and kept["team"])
        # a snapshot as the source
        od1 = py_db(d / "run1.db", "py")
        snap = d / "run1-py.floereview"
        oteam.export_review(od1, str(snap)).close()
        od1.close()
        od = py_db(d / "run2.db", "py")
        plan = omigrate.prepare_migration(od, str(d / "run1.db"), state_path=str(snap))
        e = svc.job(h, "drc_migrate_prepare", previous=str(d / ".run1.db.tray"), state=str(snap))
        check(e["state"] == "done", "prepare from a snapshot", e)
        got = comparable(plan_view(svc, h, e["plan"], n, sizes))
        want = comparable(plan_ref(plan, n, sizes))
        check(got == want, "a snapshot source = the reference's plan")
        plan.close()
        od.close()
        # refusals
        for fields, words in ((dict(previous=str(d / "run2.db")), "currently open"),
                              (dict(previous=str(d / "nothing.db")), ""),
                              (dict(previous=str(d / "run1.db"), state=str(d / "run1.db")),
                               "not a reviewer snapshot")):
            e = svc.job(h, "drc_migrate_prepare", **fields)
            check(e["state"] == "error" and words in e.get("message", ""),
                  "refused: %s" % (words or "missing previous result"), e)
        svc("drc_close", handle=h)
        # nobody's files: the waives embedded in the old pack
        emb = d / "emb"
        emb.mkdir()
        for name in ("run1.db", "run2.db"):
            shutil.copy2(d / name, emb / name)
            r = run([INDEX, "drc", str(emb / name)])
            check(r.returncode == 0, "floe-index drc emb/%s" % name, r.stderr)
        epack = Path(odrc.cachepath.pack_path(str(emb / "run1.db")))
        data = bytearray(epack.read_bytes())
        footer = struct.unpack_from("<15QII32s8s", data, len(data) - 168)
        status_off, wcount_off = footer[4], footer[5]
        for gid in (0, 1, 2, 9003, 9004):
            data[status_off + gid] = 1
        struct.pack_into("<II", data, wcount_off, 3, 2)
        epack.write_bytes(bytes(data))
        od = py_db(emb / "run2.db", "nobody")
        plan = omigrate.prepare_migration(od, str(emb / "run1.db"))
        h = rs_open(svc, emb / "run2.db", "nobody")["handle"]
        e = svc.job(h, "drc_migrate_prepare", previous=str(emb / "run1.db"))
        sz = [len(c.errors) for c in od.checks]
        check(e["state"] == "done" and comparable(plan_view(svc, h, e["plan"], len(sz), sz))
              == comparable(plan_ref(plan, len(sz), sz)),
              "waives embedded in an old pack = the reference's plan", e)
        plan.close()
        od.close()
        e = svc.job(h, "drc_migrate_prepare", previous=str(d / "run1.db"))
        check(e["state"] == "error" and "no saved review state" in e.get("message", ""),
              "refused: no review of this reviewer", e)
    finally:
        svc.close()


# ---- R6 ------------------------------------------------------------------

def r6(d):
    print("[R6] floe2 drc-match = the branch's cases")
    t = d / "match"
    t.mkdir()
    old = t / "old.db"
    new = t / "new.db"
    old.write_text("TOP 1000\nA\n5 5 0\np 1 1\n10 -5\np 2 1\n20 3\np 3 1\n10 -5\np 4 1\n30 4\n"
                   "p 5 1\n30 4\nB\n1 1 0\ne 1 1\n-2 0 2 0\n")
    new.write_text("TOP 2000\nB\n1 1 0\ne 1 1\n4 0 -4 0\nA\n5 5 0\np 1 1\n60 8\np 2 1\n40 6\n"
                   "p 3 1\n20 -10\np 4 1\n22 -10\np 5 1\n20 -10\n")
    for p in (old, new):
        run([INDEX, "drc", str(p)])
    (t / "plan.json").write_text("[[0,1],[1,0]]")
    (t / "st.u8").write_bytes(bytes([3, 4, 0, 5, 5, 2]))
    for jobs in ("2", "1"):
        out, rep = t / ("out%s" % jobs), t / ("rep%s" % jobs)
        r = run([FLOE2, "drc-match", str(t / ".old.db.tray"), str(t / ".new.db.tray"),
                 "--plan", str(t / "plan.json"), "--states", str(t / "st.u8"), "--out", str(out),
                 "--work-dir", str(t / "work"), "--jobs", jobs, "--memory-mib", "4",
                 "--report", str(rep)])
        b = out.read_bytes() if out.exists() else b""
        rows = [(struct.unpack_from("<I", b, i)[0], struct.unpack_from("<Q", b, i + 8)[0],
                 b[i + 4], b[i + 5], b[i + 6]) for i in range(0, len(b), 16)]
        report = rep.read_text() if rep.exists() else ""
        check(r.returncode == 0 and rows == [(0, 0, 2, 0, 0), (1, 0, 5, 0, 0), (1, 1, 4, 0, 0),
                                             (1, 2, 1, 0, 2), (1, 4, 1, 0, 2)]
              and '"ambiguous_target":2' in report and '"unmatched_source_work":0' in report,
              "reordered rules, duplicate zeros (--jobs %s)" % jobs, (r.stderr, rows, report))
    check(not any((t / "work").iterdir()), "no scratch left")
    r = run([FLOE2, "drc-match", str(t / ".old.db.tray"), str(t / ".new.db.tray"),
             "--plan", str(t / "plan.json"), "--states", str(t / "st.u8"),
             "--out", str(t / ".old.db.tray"), "--work-dir", str(t / "work")])
    check(r.returncode != 0 and "fresh" in r.stderr, "an output over an input refused", r.stderr)
    (t / "bad.u8").write_bytes(bytes([8, 0, 0, 0, 0, 0]))
    r = run([FLOE2, "drc-match", str(t / ".old.db.tray"), str(t / ".new.db.tray"),
             "--plan", str(t / "plan.json"), "--states", str(t / "bad.u8"),
             "--out", str(t / "o3"), "--work-dir", str(t / "work")])
    check(r.returncode != 0 and "unknown bits" in r.stderr, "unknown state bits refused", r.stderr)
    r = run([FLOE2, "drc-match", "--protocol"])
    check(r.stdout.strip() == "2", "--protocol 2")
    # duplicate votes (the branch's table: states of two old duplicates)
    o2, n2 = t / "o2.db", t / "n2.db"
    o2.write_text("TOP 1000\nR\n2 2 0\np 1 1\n1 1\np 2 1\n1 1\n")
    n2.write_text("TOP 1000\nR\n1 1 0\np 1 1\n1 1\n")
    for p in (o2, n2):
        run([INDEX, "drc", str(p)])
    (t / "p2.json").write_text("[[0,0]]")
    bad = []
    for states, flags, unknown in (([1, 0], 1, 0), ([3, 1], 1, 2), ([3, 2], 3, 0), ([7, 3], 3, 4),
                                   ([5, 1], 1, 4), ([2, 0], 0, 2), ([4, 0], 0, 4), ([6, 2], 2, 4),
                                   ([2, 4], 0, 6), ([7, 0], 1, 6), ([7, 7], 7, 0), ([0, 0], 0, 0)):
        stem = "s%d%d" % tuple(states)
        (t / (stem + ".u8")).write_bytes(bytes(states))
        r = run([FLOE2, "drc-match", str(t / ".o2.db.tray"), str(t / ".n2.db.tray"),
                 "--plan", str(t / "p2.json"), "--states", str(t / (stem + ".u8")),
                 "--out", str(t / (stem + ".out")), "--work-dir", str(t / "work")])
        b = (t / (stem + ".out")).read_bytes()
        want = b"" if states == [0, 0] else struct.pack("<IBBBBQ", 0, flags, 0, unknown, 0, 0)
        if r.returncode or b != want:
            bad.append((states, b, r.stderr))
    check(not bad, "duplicate votes: visits united, each judgement its consensus", bad[:2])


def timed(f):
    def run(*a, **k):
        t = time.monotonic()
        try:
            return f(*a, **k)
        finally:
            print("  (%s %.1f s)" % (f.__name__, time.monotonic() - t))
    return run


def r8(temp):
    print("[R8] tools/gen_drc_samples.py: the manual-test samples")
    out = temp / "samples"
    r = run([sys.executable, str(ROOT / "tools/gen_drc_samples.py"), "--out", str(out),
             "--heavy", "1000", "--me", "gate"])
    check(r.returncode == 0, "the generator", r.stderr[-800:])
    if r.returncode:
        return
    names = ["chip.oas", "deck.svrf", "GUIDE.ko.md", "run1/results.db", "run1/.results.db.tray",
             "run1/deck.svrf.rules.json", "run1/my-review.floereview", "run1/my-waives.waive",
             "run2/results.db", "run2/.results.db.tray", "run2/results.db.clusters",
             "run2/peers/alice.floereview", "run2/peers/results.db.review.bob.sqlite3",
             "run2/peers/results.db.waive.bob", "run2/peers/results.db.waive.carol"]
    missing = [n for n in names if not (out / n).exists()]
    check(not missing, "its files", missing)
    check(not (out / "run2/.results.db.waive.gate").exists(), "my current review left empty")
    got = json.loads((out / "expected.json").read_text())
    rules = {x["name"]: x for x in got["rules"]}
    s1, w1, s2 = rules["M1.S.1"], rules["M1.W.1"], rules["M2.S.1"]
    check((s1["team"]["conflicts"], s1["team"]["visited"], s2["team"]["conflicts"])
          == (175, 1300, 100), "the reviewers' conflicts (M1.S.1 175, M2.S.1 100)",
          (s1["team"], s2["team"]))
    check((s1["migration"]["matched"], s1["migration"]["partial"], s1["migration"]["ambiguous"],
           w1["migration"]["matched"], got["migration"]["no_rule_match_source_work"])
          == (1200, 5, 5, 150, 12), "the previous run's matches (exact, partial, renamed)",
          (s1["migration"], w1["migration"], got["migration"].get("no_rule_match_source_work")))
    check(got["team"]["reviewers"] == 4, "four reviewers (me + three peers)", got["team"])
    oas = (out / "chip.oas").read_bytes()
    check(oas.startswith(b"%SEMI-OASIS\r\n") and len(oas) > 100000, "the layout")
    r = run([FLOE2, "info", str(out / "chip.oas"), "--json"])
    try:
        info = json.loads(r.stdout)
    except ValueError:
        info = {}
    layers = sorted((x["layer"], x["datatype"]) for x in info.get("metadata", {}).get("layers", []))
    check(r.returncode == 0 and layers == [(31, 0), (32, 0), (51, 0)],
          "the layout indexed: M1, M2, VIA1", (r.stderr[-300:], layers))
    # a DRC capture's layer legend (crate::legend, 2026-10-13): the rule's
    # isolated layers (its sidecar), none for an all-layer shot, every
    # layer's with --legend - as flateyes metadata, in the panel's order
    colours = {(x["layer"], x["datatype"]): (x["name"], x["color"].lower())
               for x in info.get("metadata", {}).get("layers", [])}

    def legend(*extra):
        png = temp / "drc_legend.png"
        if png.exists():
            png.unlink()
        r = run([FLOE2, "render", str(out / "chip.oas"), "--drc", str(out / "run1/results.db"),
                 "--drc-rule", "M1.S.1", "--drc-err", "1", "--px", "200",
                 "--out", str(temp / "drc_legend"), *extra])
        dump = run([FLOE2, "fe-embed", "--dump", str(png)])
        return r.returncode, [x.split("=", 1)[1] for x in dump.stdout.splitlines()
                              if x.startswith("legend=")]

    def rows(keys):
        return ["box %s speckle %s %d/%d" % (colours[k][1], colours[k][0], *k) for k in keys]

    if len(colours) == 3:
        check(legend() == (0, rows([(31, 0)])), "a DRC capture's legend: its isolated layer",
              legend())
        check(legend("--layers", "all") == (0, []), "an all-layer DRC capture: no legend")
        check(legend("--layers", "all", "--legend") == (0, rows([(31, 0), (32, 0), (51, 0)])),
              "--legend: every layer's", legend("--layers", "all", "--legend"))
        # the run's layer properties file on a DRC capture (0.12.452, user
        # 2026-11-12: "floe2 render에도 --layerprops 추가해줘"): its colours and
        # fills on the layers drawn; the rule's isolated layers are the
        # rule's, --layers all every layer; a rule whose layers are not known
        # is drawn with every layer less those the file hides
        props = temp / "drc.layerprops"
        props.write_text("31.0 #ff0000 solid M1 1 1\n32.0 #00ff00 solid M2 0 1\n")
        m1, m2, via = "box #ff0000 solid M1 31/0", "box #00ff00 solid M2 32/0", rows([(51, 0)])[0]
        got = legend("--layerprops", str(props))
        check(got == (0, [m1]), "a DRC capture with --layerprops: the isolated layer, the file's colour and fill", got)
        got = legend("--layerprops", str(props), "--layers", "all", "--legend")
        check(got == (0, [m1, m2, via]), "--layerprops with --layers all: every layer, the hidden one too", got)
        none = temp / "none.rules.json"
        none.write_text('{"version": 1, "rules": {}}')
        got = legend("--drc-rules", str(none))
        check(got == (0, []), "a rule without its layers: every layer, no legend", got)
        got = legend("--drc-rules", str(none), "--layerprops", str(props))
        check(got == (0, [m1, via]),
              "a rule without its layers and --layerprops: every layer less the file's hidden one", got)
        got = legend("--layerprops", str(temp / "nope.layerprops"))
        check(got[0] == 1, "a DRC capture with a layer properties file that is not there: refused", got)
        # a snapshot's embedded rulers (0.12.453, user 2026-11-12: no
        # horizontal and vertical rulers where the CD is the Euclidean
        # distance): an edge pair's closest gap alone - a diagonal gap is
        # one ruler=, not three - and a facing pair's as before
        diag = temp / "diag.db"
        diag.write_text("SAMPLE 100000\nDIAG.SPACE\n2 2 0\n"
                        "e 1 2\n0 20000 100000 20000\n150000 60000 250000 60000\n"
                        "e 2 2\n0 300000 100000 300000\n0 330000 100000 330000\n")
        sidecar = temp / "diag.rules.json"
        sidecar.write_text(json.dumps({"format": "floe-svrf-rules", "version": 1, "checks": {
            "DIAG.SPACE": {"constraints": [{"metric": "space", "op": "<", "value": 0.7,
                                            "text": "EXTERNAL M1 < 0.7"}]}}}))
        r = run([INDEX, "drc", str(diag)])
        check(r.returncode == 0, "the diagonal-gap results packed", r.stderr[-300:])

        def embedded(err):
            png = temp / "diag_snap.png"
            if png.exists():
                png.unlink()
            r = run([FLOE2, "render", str(out / "chip.oas"), "--drc", str(diag), "--drc-rule",
                     "DIAG.SPACE", "--drc-err", str(err), "--drc-rules", str(sidecar),
                     "--px", "200", "--out", str(temp / "diag_snap")])
            dump = run([FLOE2, "fe-embed", "--dump", str(png)]).stdout
            return r.returncode, [[float(n) for n in x.split("=", 1)[1].split(",")[:4]]
                                  for x in dump.splitlines() if x.startswith("ruler=")]
        code, rulers = embedded(1)
        check(code == 0 and len(rulers) == 1 and rulers[0][0] != rulers[0][2]
              and rulers[0][1] != rulers[0][3],
              "a DRC snapshot of a diagonal gap embeds the diagonal ruler alone", (code, rulers))
        code, rulers = embedded(2)
        check(code == 0 and len(rulers) == 1 and abs(rulers[0][0] - rulers[0][2]) < 1e-9,
              "a DRC snapshot of a facing pair embeds its one ruler", (code, rulers))


# ---- R9, R10 (2026-11-10: one session a reviewer, the reviewers' statistics) ----

REVIEW_ENV = ("FLOE_REVIEWER", "FLOE_REVIEWER_REQUIRED", "FLOE_REVIEW_SESSION_LOCK",
              "FLOE_WORKDIR", "FLOE_DRC_VERIFY_MB", "SSH_CONNECTION", "SSH_CLIENT")


def svc_with(env):
    """A service started under `env` (its process takes the environment
    of its first request), without the review switches of this shell."""
    saved = dict(os.environ)
    try:
        for k in REVIEW_ENV:
            os.environ.pop(k, None)
        os.environ.update(env)
        s = Svc()
        try:
            s("drc_find", db="/nonexistent/x.db")
        except Exception:
            pass
        return s
    finally:
        os.environ.clear()
        os.environ.update(saved)


def run_env(args, env):
    e = dict(os.environ)
    for k in REVIEW_ENV:
        e.pop(k, None)
    e.update(env)
    return run(args, env=e)


def refusal(svc, **fields):
    try:
        svc("drc_open", **fields)
    except gtkservice.ServiceError as exc:
        return exc.kind, str(exc)
    return None


def r9(temp):
    print("[R9] one session a reviewer and result; a reviewer id where it is required")
    d = temp / "session"
    d.mkdir()
    write_db(d / "s.db", [("A", [_poly(100 * i, 0) for i in range(300)]),
                          ("B", [_edge(0, 100 * i) for i in range(50)])])
    r = run([INDEX, "drc", str(d / "s.db")])
    check(r.returncode == 0, "floe-index drc s.db", r.stderr)
    db, tray = str(d / "s.db"), str(d / ".s.db.tray")
    a, b = svc_with(dict(DISPLAY="etx01:15.0")), svc_with(dict(DISPLAY="etx01:27.0"))
    others = []
    try:
        oa = a("drc_open", path=db, mode="load", reviewer="kim")
        check((oa["reviewer"], oa["reviewer_source"], oa["reviewer_stated"])
              == ("kim", "given", True), "an id given is said as given", oa.get("reviewer_source"))
        a("drc_set_status", handle=oa["handle"], check=0, errors=[1, 2, 3], status=1)
        # the same reviewer from another process: refused, told who has it -
        # by the .db and by its pack - and nothing written
        for fields in (dict(path=db, mode="load"), dict(path=tray, mode="pack", source=db)):
            got = refusal(b, reviewer="kim", **fields)
            check(got is not None and got[0] == "busy"
                  and "kim's review of s.db is already open in another session" in got[1]
                  and "pid %d" % a.svc._proc.pid in got[1] and "--userid" in got[1],
                  "a second session of the reviewer is refused (%s)" % fields["mode"], got)
        ob = b("drc_open", path=db, mode="load", reviewer="lee")
        check(ob["reviewer"] == "lee", "another reviewer opens beside it")
        # the same process again (a viewer reloading its database): shared
        oa2 = a("drc_open", path=db, mode="load", reviewer="kim")
        check(oa2["counts"][0][0] == 3, "the reviewer's own process opens it again", oa2["counts"])
        a("drc_close", handle=oa["handle"])
        check(refusal(b, path=db, mode="load", reviewer="kim") is not None,
              "held while one handle of the process is open")
        a("drc_close", handle=oa2["handle"])
        ob2 = b("drc_open", path=db, mode="load", reviewer="kim")
        check(ob2["counts"][0][0] == 3, "let go with the last handle: the next session opens",
              ob2["counts"])
        b("drc_set_status", handle=ob2["handle"], check=0, errors=[10], status=1)
        b("drc_close", handle=ob2["handle"])
        oa3 = a("drc_open", path=db, mode="load", reviewer="kim")
        check(oa3["counts"][0][0] == 4, "sessions one after another keep one count", oa3["counts"])
        # a holder that dies lets go (a kernel lock: nothing stale)
        c = svc_with(dict(DISPLAY="etx01:31.0"))
        c("drc_open", path=db, mode="load", reviewer="park")
        c.svc._proc.kill()
        c.svc._proc.wait()
        op = b("drc_open", path=db, mode="load", reviewer="park")
        check(op["reviewer"] == "park", "a killed session holds nothing")
        # the switch: as before 0.12.440
        off = svc_with(dict(DISPLAY="etx01:33.0", FLOE_REVIEW_SESSION_LOCK="off"))
        others.append(off)
        check(refusal(off, path=db, mode="load", reviewer="kim") is None,
              "FLOE_REVIEW_SESSION_LOCK=off opens a second session")
        check((d / ".floe-lock" / "s.db.waive.kim.session.build").exists(),
              "the session's lock beside the sidecar")
        # no id given: the DISPLAY host, said as not stated
        plain = svc_with(dict(DISPLAY="etx01:35.0"))
        others.append(plain)
        o = plain("drc_open", path=db, mode="load")
        check((o["reviewer"], o["reviewer_source"], o["reviewer_stated"])
              == ("etx01", "DISPLAY host", False), "an id taken from the DISPLAY host is said so",
              (o["reviewer"], o.get("reviewer_source")))
        # FLOE_REVIEWER_REQUIRED: no review without a stated id - refused,
        # not answered with the ASCII file
        need = svc_with(dict(DISPLAY="etx01:37.0", FLOE_REVIEWER_REQUIRED="1"))
        others.append(need)
        for fields in (dict(path=db, mode="load"), dict(path=tray, mode="pack", source=db)):
            got = refusal(need, **fields)
            check(got is not None and got[0] == "admission" and "FLOE_REVIEWER_REQUIRED" in got[1]
                  and '"etx01" (the DISPLAY host)' in got[1] and "--userid" in got[1],
                  "a reviewer id is required (%s)" % fields["mode"], got)
        o = need("drc_open", path=db, mode="load", reviewer="choi")
        check(o["reviewer"] == "choi" and o["packed"], "with an id given it opens")
        env_id = svc_with(dict(DISPLAY="etx01:39.0", FLOE_REVIEWER_REQUIRED="on",
                               FLOE_REVIEWER="han"))
        others.append(env_id)
        o = env_id("drc_open", path=db, mode="load")
        check((o["reviewer"], o["reviewer_source"], o["reviewer_stated"])
              == ("han", "FLOE_REVIEWER", True), "FLOE_REVIEWER states it (the viewer's --userid)",
              (o["reviewer"], o.get("reviewer_source")))
    finally:
        for s in [a, b] + others:
            s.close()
    need = dict(DISPLAY="etx01:41.0", FLOE_REVIEWER_REQUIRED="1")
    r = run_env([FLOE2, "drc", db, "--rules"], need)
    check(r.returncode == 1 and not r.stdout and "FLOE_REVIEWER_REQUIRED" in r.stderr,
          "floe2 drc --rules without an id: refused where one is required",
          (r.returncode, r.stderr[-300:]))
    r = run_env([FLOE2, "drc", db, "--rules", "--floe-reviewer", "kim"], need)
    try:
        waived = [x["waived"] for x in json.loads(r.stdout)]
    except ValueError:
        waived = None
    check(r.returncode == 0 and waived == [4, 0], "... and read with one", (r.returncode, waived))
    r = run_env([FLOE2, "drc", db, "--review-stats"], need)
    check(r.returncode == 0, "the statistics name no reviewer: not refused", r.stderr[-300:])


def brute(sizes, people):
    """The statistics by hand: people = {id: (waived, visited, excluded,
    flags known)} with {rule: set}; (own counts, team counts by rule)."""
    own, team = {}, []
    for name, (w, v, n, known) in people.items():
        rows = []
        for ci, size in enumerate(sizes):
            ws, vs, ns = w.get(ci, set()), v.get(ci, set()), n.get(ci, set())
            row = {"waived": len(ws)}
            if known:
                row.update(visited=len(vs), not_required=len(ns),
                           visited_not_required=len(vs & ns), pending=size - len(vs | ns))
            rows.append(row)
        own[name] = rows
    for ci, size in enumerate(sizes):
        visited, waived, excluded, conflict = set(), set(), set(), set()
        for w, v, n, known in people.values():
            visited |= v.get(ci, set())
            waived |= w.get(ci, set())
            excluded |= n.get(ci, set())
        voters = [(w.get(ci, set()), v.get(ci, set()), n.get(ci, set()))
                  for w, v, n, known in people.values() if known]
        for e in visited:
            seen = [(e in w, e in n) for w, v, n in voters if e in v]
            if len({x[0] for x in seen}) > 1 or len({x[1] for x in seen}) > 1:
                conflict.add(e)
        excluded -= conflict
        team.append({"errors": size, "visited": len(visited), "waived": len(waived - conflict),
                     "not_required": len(excluded), "conflicts": len(conflict),
                     "pending": size - len(visited | excluded)})
    return own, team


def stats_of(db, *more, env=None):
    r = run_env([FLOE2, "drc", str(db), "--review-stats"] + list(more), env or {})
    try:
        return r, json.loads(r.stdout)
    except ValueError:
        return r, None


def r10(temp):
    print("[R10] floe2 drc --review-stats: every reviewer's counts and the team's")
    d = temp / "stats"
    d.mkdir()
    sizes = [9000, 5000, 10]
    write_db(d / "t.db", [("A.W", [_poly(100 * (i % 120), 60 * (i // 120)) for i in range(9000)]),
                          ("B.S", [_edge(200 * (i % 50), 100 * (i // 50)) for i in range(5000)]),
                          ("C,odd \"name\"", [_poly(7 * i, 3 * i, 5, 5) for i in range(10)])])
    r = run([INDEX, "drc", str(d / "t.db")])
    check(r.returncode == 0, "floe-index drc t.db", r.stderr)
    db = d / "t.db"
    # kim and lee review with flags, across the 4096-error chunks; park only
    # opens it (a waive file alone: visits unknown); error 0 of A is a
    # conflict (both visited, one waived), B's 1 too (one excluded it)
    people = {
        "kim": ({0: set(range(0, 10)) | set(range(4090, 4100)), 2: {9}},
                {0: {0, 1, 2, 3, 4, 4095, 4096, 8999}, 1: {1}},
                {0: set(range(100, 110)), 1: {1, 2}}, True),
        "lee": ({0: set(range(5, 15)) | {8999}, 1: {7}},
                {0: {0, 5, 20, 4096}, 1: {1, 7}},
                {0: {4096}}, True),
        "park": ({}, {}, {}, False),
    }
    svc = Svc()
    try:
        h = {}
        for name, (w, v, n, known) in people.items():
            h[name] = rs_open(svc, db, name)["handle"]
            for ci, errs in w.items():
                svc("drc_set_status", handle=h[name], check=ci, errors=sorted(errs), status=1)
            count = 0
            for ci, errs in v.items():
                for e in sorted(errs):
                    svc("drc_visit", handle=h[name], check=ci, error=e)
                    count += 1
            svc.visits(h[name], count)
            for ci, errs in n.items():
                e = svc.job(h[name], "drc_review", check=ci, action="not-required",
                            errors=sorted(errs), on=True)
                check(e["state"] == "done", "%s: not for review" % name, e)
        want_own, want_team = brute(sizes, people)
        r, got = stats_of(db)
        check(r.returncode == 0 and got is not None and got["type"] == "floe-drc-review-statistics"
              and got["version"] == 1, "the report", r.stderr[-300:])
        if got is None:
            return
        check([x["id"] for x in got["reviewers"]] == ["kim", "lee", "park"] and not got["skipped"],
              "the reviewers found by their working files", [x["id"] for x in got["reviewers"]])
        check((got["errors"], [(x["index"], x["name"], x["errors"]) for x in got["rules"]])
              == (14010, [(1, "A.W", 9000), (2, "B.S", 5000), (3, "C,odd \"name\"", 10)]),
              "the result's rules", got["rules"])
        keys = ("waived", "visited", "not_required", "visited_not_required", "pending")
        for person in got["reviewers"]:
            name = person["id"]
            rows = want_own[name]
            whole = {k: sum(x[k] for x in rows) for k in rows[0]}
            check({k: person.get(k) for k in keys if k in whole} == whole
                  and all(k in whole or k not in person for k in keys),
                  "%s: the whole database" % name, ({k: person.get(k) for k in keys}, whole))
            sparse = {x["index"] - 1: {k: x[k] for k in keys if k in x} for x in person["rules"]}
            active = {ci: x for ci, x in enumerate(rows)
                      if x["waived"] or x.get("visited") or x.get("not_required")}
            check(sparse == active, "%s: its rules with work, each" % name, (sparse, active))
            check(person["counters_match"] and person["waive_location"] == "results"
                  and person["waive_modified"].endswith("Z"), "%s: its waive file" % name)
        park = got["reviewers"][2]
        check(park["waive_known"] and not park["visited_known"] and "visited" not in park
              and "pending" not in park, "a waive file alone: visits not known, not zero", park)
        team = got["team"]
        check([{k: x[k] for k in want_team[0]} for x in team["rules"]] == want_team,
              "the team by rule = by hand", (team["rules"], want_team))
        whole = {k: sum(x[k] for x in want_team) for k in want_team[0]}
        check({k: team[k] for k in whole} == whole and team["reviewers"] == 3
              and team["conflicts"] == 3, "the team's whole database", (team, whole))
        # ... and the reviewers window's numbers: kim loads the others
        e = svc.job(h["kim"], "drc_team_load",
                    paths=[str(d / ".t.db.review.lee.sqlite3"), str(d / ".t.db.waive.park")])
        sids = [x["snapshot"] for x in e.get("loaded") or []]
        e = svc.job(h["kim"], "drc_team_aggregate", sources=sids)
        gui_whole = e.get("counts") or {}
        check(len(sids) == 2 and all(gui_whole.get(k) == team[k] for k in
              ("visited", "waived", "not_required", "conflicts", "pending", "reviewers")),
              "the team's numbers are the reviewers window's", (gui_whole, team))
        by_rule = [svc("drc_snapshot_counts", handle=h["kim"], snapshot=e["snapshot"], check=ci)
                   for ci in range(len(sizes))]
        check(all(by_rule[ci].get(k) == team["rules"][ci][k] for ci in range(len(sizes))
                  for k in ("visited", "waived", "not_required", "conflicts", "pending")),
              "... rule by rule", (by_rule, team["rules"]))
        for name in ("kim", "lee"):
            mine = svc("drc_review_counts", handle=h[name])
            person = [x for x in got["reviewers"] if x["id"] == name][0]
            check(all(mine[k] == person[k] for k in
                      ("visited", "not_required", "visited_not_required", "pending")),
                  "%s: the viewer's own review statistics" % name, (mine, person))
    finally:
        svc.close()
    # the statuses are counted again: stored counters two sessions left
    # wrong (before 0.12.440) are said, and do not change the counts
    side = d / ".t.db.waive.lee"
    data = bytearray(side.read_bytes())
    at = 40 + sum(sizes)
    data[at:at + 4] = struct.pack("<I", 3)
    side.write_bytes(bytes(data))
    r, bad = stats_of(db)
    lee = [x for x in bad["reviewers"] if x["id"] == "lee"][0] if bad else {}
    check(lee.get("counters_match") is False and lee.get("waived") == 12
          and lee.get("counter_mismatch") == [{"index": 1, "name": "A.W", "stored": 3,
                                               "counted": 11}]
          and bad["team"] == got["team"], "stored counters are compared, never counted",
          lee.get("counter_mismatch"))
    rr = run_env([FLOE2, "drc", str(db), "--rules", "--floe-reviewer", "lee"], {})
    check(json.loads(rr.stdout)[0]["waived"] == 3, "(--rules still reads the stored counter)")
    # leftovers are no reviewers; a file of another result is said, not counted
    shutil.copy(d / ".t.db.waive.kim", d / ".t.db.waive.kim.stale-1794300165")
    shutil.copy(d / ".t.db.waive.kim", d / ".t.db.waive.kim.tmp-4242")
    ghost = bytearray((d / ".t.db.waive.kim").read_bytes())
    ghost[12] ^= 0xFF
    (d / ".t.db.waive.ghost").write_bytes(bytes(ghost))
    r, g2 = stats_of(db)
    check([x["id"] for x in g2["reviewers"]] == ["kim", "lee", "park"]
          and [(x["reviewer"], Path(x["file"]).name) for x in g2["skipped"]]
          == [("ghost", ".t.db.waive.ghost")] and "another DRC result" in g2["skipped"][0]["reason"],
          "leftovers ignored, another result's file skipped with its reason", g2["skipped"])
    # the reviewers named: these alone, one that never opened it as absent
    r, some = stats_of(db, "--floe-reviewer", "kim,nobody")
    _, one_team = brute(sizes, {"kim": people["kim"]})
    check([(x["id"], x["present"]) for x in some["reviewers"]] == [("kim", True), ("nobody", False)]
          and some["team"]["reviewers"] == 1
          and [{k: x[k] for k in one_team[0]} for x in some["team"]["rules"]] == one_team,
          "--floe-reviewer ID,ID: these alone", some["team"])
    # the pack must be the current one: a stale or missing pack is an error,
    # never zeros. Current is the .db's content (0.12.441): a .db whose time
    # alone changed is the file that was indexed, other bytes of its size
    # are not
    st = os.stat(db)
    raw = db.read_bytes()
    os.utime(db, (st.st_atime, st.st_mtime + 5))
    r, touched = stats_of(db)
    check(r.returncode == 0 and touched is not None and touched["team"] == got["team"],
          "a .db whose time alone changed: its pack is current", (r.returncode, r.stderr[-300:]))
    db.write_bytes(other_bytes(raw))
    os.utime(db, (st.st_atime, st.st_mtime + 9))
    r, none = stats_of(db)
    check(r.returncode == 1 and not r.stdout and "stale DRC pack" in r.stderr,
          "a stale pack: refused, nothing printed", (r.returncode, r.stderr[-300:]))
    db.write_bytes(raw)
    os.utime(db, (st.st_atime, st.st_mtime))
    r, again = stats_of(db)
    check(r.returncode == 0 and again["team"] == got["team"], "(the .db as indexed: read again)")
    bare = d / "bare"
    bare.mkdir()
    shutil.copy(db, bare / "t.db")
    r, none = stats_of(bare / "t.db")
    check(r.returncode == 1 and not r.stdout and "no DRC pack" in r.stderr,
          "no pack: refused", (r.returncode, r.stderr[-300:]))
    r = run_env([FLOE2, "drc", str(db), "--review-stats", "--rules"], {})
    check(r.returncode == 2 and "--review-stats is a report of its own" in r.stderr,
          "not with the other reports (an argument error)", (r.returncode, r.stderr[-200:]))
    # --workdir: the reviews kept under it are the ones read with it
    work = d / "work"
    r = run_env([FLOE2, "--workdir", str(work), "drc", str(db), "--build"], {})
    check(r.returncode == 0, "a pack under --workdir", r.stderr[-300:])
    ws = svc_with(dict(FLOE_WORKDIR=str(work)))
    try:
        o = ws("drc_open", path=str(db), mode="load", reviewer="yoon")
        ws("drc_set_status", handle=o["handle"], check=1, errors=[3, 4], status=1)
        check(str(work) in o["waive_path"], "a review under the work directory", o["waive_path"])
    finally:
        ws.close()
    r, w = stats_of(db, env=dict(FLOE_WORKDIR=str(work)))
    check(w is not None and [(x["id"], x["waived"]) for x in w["reviewers"]] == [("yoon", 2)]
          and str(work) in w["pack"], "its statistics with the same work directory",
          w and w["reviewers"])
    r, beside = stats_of(db)
    check([x["id"] for x in beside["reviewers"]] == ["kim", "lee", "park"],
          "... and the reviews beside the .db without it")



# ---- R11 -----------------------------------------------------------------


def write_run(path, rules, date="Nov 10 12:00:00 2026", deck="rules/a.drc"):
    """A result as a DRC run writes it: each rule's header carries the run's
    date, its text the rule file's path."""
    out = ["TOP 1000\n"]
    for name, records in rules:
        out.append("%s\n%d %d 2 %s\nRule File Pathname: %s\n%s check\n"
                   % (name, len(records), len(records), date, deck, name))
        for i, rec in enumerate(records):
            kind, _num, rest = rec.split(" ", 2)
            out.append("%s %d %s" % (kind, i + 1, rest))
    Path(path).write_text("".join(out))


def review_of(svc, db, who):
    """What a reviewer's review of `db` holds, as its open shows it."""
    opened = rs_open(svc, db, who)
    h = opened["handle"]
    try:
        if not opened["packed"]:
            return {"packed": False}
        return {
            "packed": True,
            "counts": svc("drc_review_counts", handle=h),
            "flags": svc("drc_review_flags", handle=h, check=0, errors=[0, 1, 2, 3]),
            "status": [svc("drc_status", handle=h, check=ci, start=0, count=4) for ci in (0, 1)],
            "notes": sorted(n["text"] for n in opened["notes"]),
            "moved": [n for n in opened["notices"] if "moved aside" in n],
        }
    finally:
        svc("drc_close", handle=h)


def numbers(stats):
    """A statistics report without its paths and times."""
    if stats is None:
        return None
    keys = ("id", "present", "waived", "visited", "not_required", "visited_not_required",
            "pending", "counters_match")
    return ([{k: x.get(k) for k in keys} for x in stats["reviewers"]], stats["team"],
            stats["fingerprint"], [x["reviewer"] for x in stats["skipped"]])


def r11(temp):
    print("[R11] a result is known by its content: the .db's CRC-32, the pack's content id")
    import zlib
    d = temp / "identity"
    d.mkdir()
    db, pack, hint = d / "a.db", d / ".a.db.tray", d / ".a.db.tray.source"
    rules = [("M1.S", [_poly(100 * i, 0) for i in range(40)]),
             ("M2.W", [_edge(0, 100 * i) for i in range(30)])]
    write_run(db, rules)
    r = run([INDEX, "drc", str(db)])
    check(r.returncode == 0, "floe-index drc a.db", r.stderr)
    content = pack.read_bytes()[-40:-8]
    svc = svc_with({})
    try:
        h = rs_open(svc, db, "kim")["handle"]
        for e in (0, 1):
            svc("drc_visit", handle=h, check=0, error=e)
        svc.visits(h, 2)
        e = svc.job(h, "drc_review", check=0, action="not-required", errors=[2], on=True)
        check(e["state"] == "done", "kim's review", e)
        svc("drc_set_status", handle=h, check=0, errors=[1], status=1)
        svc("drc_set_status", handle=h, check=1, errors=[0, 3], status=1)
        svc("drc_set_note", handle=h, gids=[3], text="kept")
        svc("drc_close", handle=h)
        want = review_of(svc, db, "kim")
        r, s0 = stats_of(db)
        base = numbers(s0)
        check(want["packed"] and want["counts"]["visited"] == 2
              and want["status"] == ["00010000", "01000001"]
              and want["notes"] == ["kept"] and not want["moved"]
              and r.returncode == 0 and base[2] == content.hex()
              and [(x["id"], x["waived"], x["visited"], x["not_required"]) for x in base[0]]
              == [("kim", 3, 2, 1)], "the review and its statistics, bound to the content id",
              (want, base))

        # 1. the .db's time alone changes (a copy back, a restore, a touch)
        raw = db.read_bytes()
        crc = "%08x" % (zlib.crc32(raw) & 0xFFFFFFFF)
        t1 = int(os.stat(db).st_mtime) + 7
        os.utime(db, (t1, t1))
        r, s = stats_of(db)
        check(r.returncode == 0 and numbers(s) == base,
              "a .db whose time alone changed: its pack is current, the statistics the same",
              (r.returncode, r.stderr[-300:]))
        check(hint.exists() and hint.read_text() == "floe-drc-source 1\n%d %d %s\n" % (len(raw), t1, crc),
              "... its content was compared (CRC-32) and the stamp kept beside the pack",
              hint.read_text() if hint.exists() else None)
        check(review_of(svc, db, "kim") == want, "... and the review is as it was")
        # the inline limit (FLOE_DRC_VERIFY_MB) is for the open that must
        # not wait - the viewer opening a pack chosen with its .db, where a
        # stale pack is asked about: the kept stamp answers it with nothing
        # read; without the stamp a source over the limit is not read
        stamp = (pack.read_bytes(), pack.stat().st_mtime_ns, pack.stat().st_ino)
        limited = dict(FLOE_DRC_VERIFY_MB="0")
        off = svc_with(limited)
        try:
            o = off("drc_open", path=str(pack), mode="pack", source=str(db), reviewer="kim")
            check(o["packed"], "the kept stamp answers an open that may read nothing")
            off("drc_close", handle=o["handle"])
            hint.unlink()
            kind = refusal(off, path=str(pack), mode="pack", source=str(db), reviewer="kim")
            check(kind is not None and kind[0] == "cache" and "stale pack" in kind[1]
                  and not hint.exists(),
                  "over the inline limit: the viewer's open of the pack does not read it - stale",
                  kind)
            # ... and whatever would read the .db anyway compares one of any
            # size: a load (else it parses the ASCII file), the statistics,
            # the reads, an explicit build
            check(review_of(off, db, "kim") == want and hint.exists(),
                  "... a load compares it whatever its size: the pack and the review")
        finally:
            off.close()
        hint.unlink()
        r, s = stats_of(db, env=limited)
        check(r.returncode == 0 and numbers(s) == base and hint.exists(),
              "... and the statistics", (r.returncode, r.stderr[-300:]))
        hint.unlink()
        r = run_env([FLOE2, "drc", str(db), "--rules", "--floe-reviewer", "kim"], limited)
        try:
            waived = [x["waived"] for x in json.loads(r.stdout)]
        except ValueError:
            waived = None
        check(r.returncode == 0 and waived == [1, 2] and "parsing ASCII" not in r.stderr
              and hint.exists(), "... and --rules, with no ASCII fallback",
              (r.returncode, waived, r.stderr[-300:]))
        hint.unlink()
        r = run_env([FLOE2, "drc", str(db), "--build"], limited)
        try:
            out = json.loads(r.stdout)
        except ValueError:
            out = {}
        check(r.returncode == 0 and out.get("reused") is True
              and (pack.read_bytes(), pack.stat().st_mtime_ns, pack.stat().st_ino) == stamp
              and hint.exists(), "... and floe2 drc --build, which keeps the pack",
              (r.returncode, r.stderr[-300:]))

        # 2. other bytes of the same size: another file
        os.utime(db, (t1 + 1, t1 + 1))
        db.write_bytes(other_bytes(raw))
        os.utime(db, (t1 + 2, t1 + 2))
        r, s = stats_of(db)
        check(r.returncode == 1 and "stale DRC pack" in r.stderr and "--build --force" in r.stderr,
              "other bytes of the same size: stale, and what to do", (r.returncode, r.stderr[-300:]))
        r = run_env([FLOE2, "drc", str(db), "--build"], {})
        check(r.returncode == 1 and "is stale" in r.stderr and "--force" in r.stderr
              and (pack.read_bytes(), pack.stat().st_mtime_ns, pack.stat().st_ino) == stamp,
              "... --build refuses to replace the pack without --force, and says so",
              (r.returncode, r.stderr[-300:]))
        db.write_bytes(raw)
        t3 = t1 + 3
        os.utime(db, (t3, t3))
        check(review_of(svc, db, "kim") == want and "%d %d %s" % (len(raw), t3, crc) in hint.read_text(),
              "the file as it was, another time again: current, the stamp renewed")

        # 3. the pack removed and made again (a cache clean-up): another
        # file, another time - the same content id, the same review
        pack.unlink()
        hint.unlink()
        r = run([INDEX, "drc", str(db)])
        check(r.returncode == 0 and pack.read_bytes()[-40:-8] == content
              and pack.stat().st_mtime_ns != stamp[1],
              "the pack made again: another file, the same content id", r.stderr)
        r, s = stats_of(db)
        check(review_of(svc, db, "kim") == want and numbers(s) == base,
              "... flags, waives and notes are as they were", numbers(s))

        # 4. the folder copied elsewhere, its files' times not kept
        copy = temp / "identity-copy"
        shutil.copytree(str(d), str(copy), copy_function=shutil.copy)
        t4 = t3 - 5000
        os.utime(copy / "a.db", (t4, t4))
        os.utime(copy / ".a.db.tray", (t4 + 60, t4 + 60))
        r, s = stats_of(copy / "a.db")
        check(review_of(svc, copy / "a.db", "kim") == want and numbers(s) == base,
              "a copy of the results folder: the review came with it", numbers(s))

        # 5. the DRC run again with the same errors: other header dates,
        # another rule file path, another size - the same content id
        write_run(db, rules, date="Nov 11 09:30:15 2026", deck="/proj/other/place/rules/a.drc")
        check(len(db.read_bytes()) != len(raw), "(the run again: a .db of another size)")
        r, s = stats_of(db)
        check(r.returncode == 1 and "stale DRC pack" in r.stderr, "the run again: its pack is stale")
        r = run([INDEX, "drc", str(db)])
        check(r.returncode == 0 and pack.read_bytes()[-40:-8] == content
              and pack.read_bytes()[-44:-40] != bytes.fromhex(crc)[::-1],
              "indexed again: another CRC-32, the same content id", r.stderr)
        r, s = stats_of(db)
        check(review_of(svc, db, "kim") == want and numbers(s) == base,
              "the same errors found again: the review goes on", numbers(s))

        # 6. the run again with other errors: the review files are another
        # result's - listed so, and moved aside when their reviewer opens it
        # (`review from a previous run` carries a review over, R5)
        moved = list(rules)
        moved[0] = ("M1.S", [_poly(100 * i + 1, 0) for i in range(40)])
        write_run(db, moved, date="Nov 12 10:00:00 2026")
        r = run([INDEX, "drc", str(db)])
        check(r.returncode == 0 and pack.read_bytes()[-40:-8] != content,
              "other errors: another content id", r.stderr)
        r, s = stats_of(db)
        check(r.returncode == 0 and s is not None and not [x for x in s["reviewers"] if x.get("present")]
              and sorted(Path(x["file"]).name for x in s["skipped"])
              == [".a.db.review.kim.sqlite3", ".a.db.waive.kim"]
              and all("another DRC result" in x["reason"] for x in s["skipped"]),
              "its statistics list the old review files as another result's", s and s["skipped"])
        after = review_of(svc, db, "kim")
        check(after["packed"] and after["counts"]["visited"] == 0
              and after["status"] == ["00000000", "00000000"] and not after["notes"]
              and len(after["moved"]) == 3
              and all(len(list(d.glob(name + ".stale-*"))) == 1 for name in
                      (".a.db.review.kim.sqlite3", ".a.db.waive.kim", ".a.db.notes.kim.fe")),
              "kim's open moves them aside - flags, waives, notes - and starts anew", after)
        r, s = stats_of(db)
        check(r.returncode == 0 and s is not None and not s["skipped"]
              and [(x["id"], x["waived"]) for x in s["reviewers"]] == [("kim", 0)],
              "... and the statistics count the new review", s and (s["reviewers"], s["skipped"]))
    finally:
        svc.close()


def reasons_read(path, sizes):
    """A reasons file read by the gate's own reader (app-core drc::reasons'
    format): its three head lines, its action lines without their times,
    and - the lines taken in order, a later one winning - per kind (`w`,
    `n`) and rule a list an error long of the reason held."""
    text = Path(path).read_text()
    lines = text.split("\n")
    table = {k: [[None] * n for n in sizes] for k in "wn"}
    said = []
    for line in lines[3:]:
        if not line:
            continue
        tag, ci, _time, rest = line.split(" ", 3)
        runs, _, why = rest.partition(" ")
        said.append(" ".join([tag, ci, rest]))
        numbers = [int(x) for x in runs.split(",")]
        at = 0
        for gap, length in zip(numbers[0::2], numbers[1::2]):
            start = at + gap
            table[tag.lower()][int(ci)][start:start + length] = (
                [why if tag in "wn" else None] * length)
            at = start + length
    return lines[:3], said, table, len(text.encode())


def r12(temp):
    print("[R12] review reasons: why errors are waived or set aside, a line an action")
    import random
    d = temp / "reasons"
    d.mkdir()
    sizes = [300000, 5000, 10]
    write_db(d / "t.db", [("A.W", [_poly(100 * (i % 600), 60 * (i // 600)) for i in range(sizes[0])]),
                          ("B.S", [_edge(200 * (i % 50), 100 * (i // 50)) for i in range(sizes[1])]),
                          ("C.X", [_poly(7 * i, 3 * i, 5, 5) for i in range(sizes[2])])])
    r = run([INDEX, "drc", str(d / "t.db")])
    check(r.returncode == 0, "floe-index drc t.db", r.stderr)
    db, file = d / "t.db", d / ".t.db.reasons.kim"
    svc = Svc()
    try:
        opened = rs_open(svc, db, "kim")
        h = opened["handle"]
        check(opened["reason_path"] == str(file) and not file.exists(),
              "the reasons file is the waive sidecar's neighbour, not made by the open",
              opened.get("reason_path"))
        get = lambda ci, e: tuple((svc("drc_reason_get", handle=h, check=ci, error=e)[k] or {}).get("text")
                                  for k in ("waive", "not_required"))
        # ---- a click's waive: without a reason nothing is written
        reply = svc("drc_set_status", handle=h, check=1, errors=[0, 1], status=1)
        check(reply["reason_error"] is None and not file.exists() and get(1, 0) == (None, None),
              "waived without a reason: nothing is written", reply)
        # with one: every error of the action holds it, one waived before too
        reply = svc("drc_set_status", handle=h, check=1, errors=[1, 2, 3, 9], status=1,
                    reason="  dummy fill\t(approved)\n")
        head, said, _table, _size = reasons_read(file, sizes)
        check(reply["reason_error"] is None and reply["counts"] == [5, 5000]
              and head[:2] == ["# floe drc review reasons", "floe_reasons=1"]
              and head[2] == "floe_pack=%s,%d" % (opened_tag(svc, h, d), sum(sizes))
              and said == ["w 1 1,3,5,1 dummy fill (approved)"]
              and [get(1, e)[0] for e in (0, 1, 2, 3, 4, 9)]
              == [None] + ["dummy fill (approved)"] * 3 + [None, "dummy fill (approved)"],
              "waived with a reason: one line, kept as one line of text, every error's of the action",
              (head, said))
        time_of = svc("drc_reason_get", handle=h, check=1, error=1)["waive"]["time"]
        check(abs(time_of - time.time()) < 120, "the line says when", time_of)
        # what the dialog starts from
        read = lambda errors, **more: svc("drc_reason_read", handle=h, check=1, reason_of="waive",
                                          errors=errors, **more)
        check(read([1, 2, 9]) == {"on": True, "prefill": "dummy fill (approved)",
                                  "known": ["dummy fill (approved)"]}
              and read([0, 1])["prefill"] == "" and read([4])["prefill"] == ""
              and read([1, 2], toggle=True)["on"] is False
              and read([1, 4], toggle=True) == {"on": True, "prefill": "",
                                                "known": ["dummy fill (approved)"]},
              "the dialog starts from the one reason its errors hold; a toggle says its way",
              (read([1, 2, 9]), read([1, 4], toggle=True)))
        try:
            svc("drc_reason_read", handle=h, check=1, reason_of="note", errors=[1])
            check(False, "an unknown kind refused")
        except gtkservice.ServiceError as exc:
            check("reason_of is waive or not-required" in str(exc), "an unknown kind refused", exc)
        # refused before anything is written: a reason past 1000 characters
        before = file.read_bytes()
        for request in (lambda: svc("drc_set_status", handle=h, check=1, errors=[20], status=1,
                                    reason="x" * 1001),
                        lambda: svc.job(h, "drc_review", check=1, action="waive", errors=[20],
                                        on=True, reason="y" * 1001)):
            try:
                request()
                check(False, "a reason past 1000 characters refused")
            except gtkservice.ServiceError as exc:
                check("at most 1000 characters" in str(exc), "a reason past 1000 characters refused", exc)
        check(file.read_bytes() == before and svc("drc_status", handle=h, check=1, start=20, count=1) == "00",
              "... and nothing was waived or written")
        svc("drc_set_status", handle=h, check=1, errors=[20], status=1, reason="가" * 1000)
        check(get(1, 20)[0] == "가" * 1000, "1000 characters are kept, whatever their bytes")
        # turned back: the reason goes with the status; waived again: none
        svc("drc_set_status", handle=h, check=1, errors=[2, 20], status=0)
        said = reasons_read(file, sizes)[1]
        check(get(1, 2) == (None, None) and said[-1] == "W 1 2,1,17,1",
              "turned back: the reason is gone and the file says so", said[-1])
        svc("drc_set_status", handle=h, check=1, errors=[2], status=1)
        check(get(1, 2) == (None, None) and len(reasons_read(file, sizes)[1]) == len(said),
              "waived again without a reason it has none - not the one it had")
        # ---- a job on a selection: one line whatever it takes
        t0 = time.monotonic()
        sel = svc.job(h, "drc_select", check=0, mode="replace")
        e = svc.job(h, "drc_review", check=0, action="waive", selection=sel["selection"], on=True,
                    reason="whole rule: dummy metal, no electrical function")
        took = time.monotonic() - t0
        head, said, _table, size = reasons_read(file, sizes)
        whole = "whole rule: dummy metal, no electrical function"
        check(e["state"] == "done" and e["changed"] == sizes[0] and e["reason_error"] is None
              and said[-1] == "w 0 0,300000 " + whole
              and get(0, 0)[0] == get(0, 299999)[0] == whole,
              "300,000 errors waived with a reason: one short line", (e, said[-1]))
        print("  (300,000 errors waived with their reason in %.0f ms; the reasons file is %d bytes)"
              % (took * 1e3, size))
        check(size < 5000 and not list(d.glob(".t.db.notes.*")),
              "... the file does not grow with the errors, and no note is made", size)
        # every third error: a run each, a few bytes a run
        third = svc.job(h, "drc_select", check=0, mode="replace", errors=list(range(0, sizes[0], 3)))
        e = svc.job(h, "drc_review", check=0, action="waive", selection=third["selection"], on=True,
                    reason="every third")
        _head, said, table, grown = reasons_read(file, sizes)
        check(e["state"] == "done" and e["changed"] == 0 and e["count"] == 100000
              and said[-1].startswith("w 0 0,1,2,1,2,1,") and said[-1].endswith(" every third")
              and grown - size < 100000 * 5
              and [get(0, x)[0] for x in (0, 1, 2, 3, 299997, 299999)]
              == ["every third", whole, whole, "every third", "every third", whole],
              "100,000 scattered errors given another reason: one line, under 5 bytes an error",
              (e, grown - size))
        check(table["w"][0][::3] == ["every third"] * 100000
              and set(table["w"][0][1::3]) == {whole},
              "the file read by the gate's own reader says the same")
        # the job reads what the dialog starts from, and which way a toggle goes
        e = svc.job(h, "drc_review", check=0, action="reason-read", reason_of="waive",
                    selection=third["selection"], toggle=True)
        check((e["on"], e["prefill"], e["known"][:2]) == (False, "", ["every third", whole]),
              "reason-read of a selection that is waived whole: the toggle turns it back", e)
        e = svc.job(h, "drc_review", check=0, action="reason-read", reason_of="waive",
                    selection=third["selection"])
        check((e["on"], e["prefill"]) == (True, "every third"),
              "... and waived again its dialog starts from the reason it holds", e)
        # turned back by the job: one line for the rule, whatever its chunks
        lines_before = len(said)
        e = svc.job(h, "drc_review", check=0, action="waive", selection=sel["selection"], on=False)
        said = reasons_read(file, sizes)[1]
        check(e["state"] == "done" and e["changed"] == sizes[0] and said[lines_before:] == ["W 0 0,300000"]
              and get(0, 0) == get(0, 150000) == (None, None),
              "300,000 errors turned back: their reasons gone in one line", said[lines_before:])
        # ---- set aside from the review: asked the same, kept apart
        e = svc.job(h, "drc_review", check=1, action="not-required", errors=[1, 2, 3, 4000], on=True,
                    reason="out of scope")
        said = reasons_read(file, sizes)[1]
        check(e["state"] == "done" and e["changed"] == 4 and e["reason_error"] is None
              and said[-1] == "n 1 1,3,3996,1 out of scope"
              and get(1, 1) == ("dummy fill (approved)", "out of scope")
              and get(1, 2) == (None, "out of scope") and get(1, 5) == (None, None),
              "set aside with a reason: its own line, beside the waive's", (e, said[-1]))
        e = svc.job(h, "drc_review", check=1, action="reason-read", reason_of="not-required",
                    errors=[1, 2])
        check((e["on"], e["prefill"], e["known"]) == (True, "out of scope", ["out of scope"]),
              "the exclusion's dialog starts from its own reasons", e)
        e = svc.job(h, "drc_review", check=1, action="not-required", errors=[2, 3], on=False)
        said = reasons_read(file, sizes)[1]
        check(e["state"] == "done" and said[-1] == "N 1 2,2" and get(1, 2) == (None, None)
              and get(1, 1) == ("dummy fill (approved)", "out of scope"),
              "back in the review: the exclusion's reason goes, the waive's stays", said[-1])
        e = svc.job(h, "drc_review", check=1, action="not-required", errors=[3], on=True)
        check(get(1, 3) == ("dummy fill (approved)", None) and len(reasons_read(file, sizes)[1]) == len(said),
              "set aside again without a reason it has none")
        # ---- a saved waive file loaded: a reason stays with an error the
        # file has waived too and goes with one it turns back
        saved = d / "saved.waive"
        svc("drc_waive_export", handle=h, path=str(saved))
        svc("drc_set_status", handle=h, check=1, errors=[9], status=0)
        svc("drc_set_status", handle=h, check=1, errors=[30], status=1, reason="after the save")
        svc("drc_waive_import", handle=h, path=str(saved))
        check([get(1, e)[0] for e in (1, 9, 30)] == ["dummy fill (approved)", None, None]
              and svc("drc_status", handle=h, check=1, start=9, count=1) == "01"
              and svc("drc_status", handle=h, check=1, start=30, count=1) == "00",
              "a saved waive file loaded keeps the reasons of what it leaves waived",
              [get(1, e) for e in (1, 9, 30)])
        # ---- any sequence of actions: the answers, the file and the
        # statistics against a table an error long
        for ci in (0, 1):
            n = sizes[ci]
            clear = svc.job(h, "drc_select", check=ci, mode="replace")
            svc.job(h, "drc_review", check=ci, action="waive", selection=clear["selection"], on=False)
            svc.job(h, "drc_review", check=ci, action="not-required", selection=clear["selection"],
                    on=False)
        n = sizes[1]
        rng = random.Random(20261113)
        state = {k: [False] * n for k in "wn"}
        model = {k: [None] * n for k in "wn"}
        steps = 0
        for _ in range(140):
            kind = rng.choice("wwn")
            on = rng.random() < 0.7
            errs = set()
            for _run in range(rng.randint(1, 4)):
                a = rng.randrange(n)
                errs.update(range(a, min(n, a + rng.choice([1, 1, 3, 40, 700]))))
            errs = sorted(errs)
            why = rng.choice(["", "", "reason %d" % rng.randrange(6)]) if on else ""
            fields = {"reason": why} if why else {}
            if kind == "w" and rng.random() < 0.5:
                reply = svc("drc_set_status", handle=h, check=1, errors=errs, status=int(on), **fields)
                ok = reply["reason_error"] is None
            else:
                target = {"errors": errs}
                if rng.random() < 0.5:
                    target = {"selection": svc.job(h, "drc_select", check=1, mode="replace",
                                                   errors=errs)["selection"]}
                e = svc.job(h, "drc_review", check=1, on=on,
                            action="waive" if kind == "w" else "not-required", **target, **fields)
                ok = e["state"] == "done" and e["reason_error"] is None
            steps += ok
            for x in errs:
                if not on:
                    state[kind][x], model[kind][x] = False, None
                elif why:
                    state[kind][x], model[kind][x] = True, why
                elif not state[kind][x]:
                    state[kind][x], model[kind][x] = True, None
        probe = sorted(set(range(0, n, 7)) | {n - 1})
        got = [get(1, x) for x in probe]
        want = [(model["w"][x], model["n"][x]) for x in probe]
        bad = [(x, g, w) for x, g, w in zip(probe, got, want) if g != w]
        check(steps == 140 and not bad and len(set(want)) > 6,
              "140 actions by clicks, jobs and selections: each error's reasons are the table's",
              (steps, bad[:3]))
        _head, said, table, _size = reasons_read(file, sizes)
        check(table["w"][1] == model["w"] and table["n"][1] == model["n"],
              "the file, read by the gate's own reader, says the table",
              [x for x in range(n) if table["w"][1][x] != model["w"][x]][:5])
        check(all(state[k][x] or table[k][1][x] is None for k in "wn" for x in range(n)),
              "no error that is not waived (set aside) holds a reason")
        svc("drc_close", handle=h)
        # the statistics: each reason with the errors that hold it
        r, got = stats_of(db, "--floe-reviewer", "kim")
        person = got["reviewers"][0] if got and got["reviewers"] else {}
        counted = {}
        for k, name in (("w", "waive"), ("n", "not_required")):
            for why in model[k]:
                if why is not None:
                    counted[(name, why)] = counted.get((name, why), 0) + 1
        listed = {(x["kind"], x["text"]): x["errors"] for x in person.get("reasons", [])}
        check(r.returncode == 0 and listed == counted
              and person.get("waived_with_reason") == sum(1 for w in model["w"] if w)
              and person.get("not_required_with_reason") == sum(1 for w in model["n"] if w)
              and person.get("waived") == sum(state["w"]) and person.get("not_required") == sum(state["n"])
              and person.get("reasons_file") == str(file) and person.get("reasons_location") == "results"
              and person.get("reasons_modified", "").endswith("Z"),
              "floe2 drc --review-stats lists each reason with the errors that hold it",
              (r.stderr[-200:], sorted(listed.items())[:4], sorted(counted.items())[:4]))
        order = [(x["kind"], -x["errors"], x["text"]) for x in person.get("reasons", [])]
        rows = person.get("reasons") or [{}]
        check(order == sorted(order, key=lambda x: (x[0] != "waive", x[1], x[2]))
              and all(x["actions"] >= 1 and x["first"] <= x["last"] and x["first"].endswith("Z")
                      and [y["index"] for y in x["rules"]] == [2] and x["rules"][0]["name"] == "B.S"
                      and x["rules"][0]["errors"] == x["errors"] for x in rows),
              "... the waive's first, the most errors first, with its actions, times and rules",
              rows[:2])
        # ---- the next open reads them - and writes the file again: it had
        # grown far past what it still says (the line of 100,000 runs that
        # were turned back since, every line taken back)
        grown = file.stat().st_size
        opened = rs_open(svc, db, "kim")
        h = opened["handle"]
        got = [get(1, x) for x in probe]
        check(got == want and not [n_ for n_ in opened["notices"] if "reason" in n_],
              "the next open reads the reasons", opened["notices"])
        _head, said, table, size = reasons_read(file, sizes)
        check(grown > 400000 and size < 64 * 1024 and table["w"][1] == model["w"]
              and table["n"][1] == model["n"] and not [x for x in said if x[0] in "WN"],
              "a file grown far past what it says is written again at the open: what is held, "
              "no line taken back", (grown, size))
        lee = rs_open(svc, db, "lee")
        check(lee["reason_path"] == str(d / ".t.db.reasons.lee")
              and svc("drc_reason_get", handle=lee["handle"], check=1, error=probe[0])
              == {"waive": None, "not_required": None}
              and svc("drc_reason_read", handle=lee["handle"], check=1, reason_of="waive",
                      errors=[1])["known"] == [],
              "another reviewer has reasons of their own - none yet")
        svc("drc_close", handle=lee["handle"])
        # ---- on the disk when the request answers: the service killed
        waived = next(x for x in range(n) if not state["w"][x])
        svc("drc_set_status", handle=h, check=1, errors=[waived], status=1, reason="before the kill")
        svc.svc._proc.kill()
        svc.svc._proc.wait()
    finally:
        try:
            svc.close()
        except Exception:
            pass
    svc = Svc()
    try:
        opened = rs_open(svc, db, "kim")
        h = opened["handle"]
        get = lambda ci, e: tuple((svc("drc_reason_get", handle=h, check=ci, error=e)[k] or {}).get("text")
                                  for k in ("waive", "not_required"))
        check(get(1, waived)[0] == "before the kill",
              "a reason is on the disk when its request answers (the service killed after it)",
              get(1, waived))
        svc("drc_close", handle=h)
        # ---- an error an older build turned back (it knows no reasons):
        # the status says whether, so its reason is neither shown nor counted
        side = d / ".t.db.waive.kim"
        raw = bytearray(side.read_bytes())
        at = 40 + sizes[0] + waived
        check(raw[at] == 1, "(the waive sidecar's byte of that error)")
        raw[at] = 0
        counter = 40 + sum(sizes) + 4
        struct.pack_into("<I", raw, counter, struct.unpack_from("<I", raw, counter)[0] - 1)
        side.write_bytes(bytes(raw))
        opened = rs_open(svc, db, "kim")
        h = opened["handle"]
        r, got = stats_of(db, "--floe-reviewer", "kim")
        person = got["reviewers"][0] if got and got["reviewers"] else {}
        check(get(1, waived) == (None, None)
              and "before the kill" not in [x["text"] for x in person.get("reasons", [])]
              and person.get("counters_match") is True,
              "a reason left on an error that is no longer waived is neither shown nor counted",
              (get(1, waived), [x["text"] for x in person.get("reasons", [])][:3]))
        svc("drc_close", handle=h)
        # ---- another result under the name: the reasons are moved aside
        # with the reviewer's other files
        kept = file.read_bytes()
        write_db(db, [("A.W", [_poly(100 * i, 0) for i in range(7)]),
                      ("B.S", [_edge(200 * i, 0) for i in range(5)])])
        r = run([INDEX, "drc", str(db), "--force"])
        check(r.returncode == 0, "another result packed under the name", r.stderr)
        r, got = stats_of(db, "--floe-reviewer", "kim")
        skipped = [(x["file"], x["reason"]) for x in (got or {}).get("skipped", [])]
        check(any(f == str(file) and "review reasons of another DRC result (not this pack's)" in why
                  for f, why in skipped) and file.read_bytes() == kept,
              "the statistics leave another result's reasons out, and where they are", skipped)
        opened = rs_open(svc, db, "kim")
        aside = sorted(d.glob(".t.db.reasons.kim.stale-*"))
        check(len(aside) == 1 and aside[0].read_bytes() == kept and not file.exists()
              and any("review reasons do not match this pack" in x and "moved aside" in x
                      for x in opened["notices"]),
              "the review moves them aside, as the waive sidecar and the flags are", opened["notices"])
        svc("drc_close", handle=opened["handle"])
    finally:
        svc.close()
    # ---- a results folder that cannot be written: the reasons go where the
    # waive sidecar goes, and the statistics find them there
    ro = d / "ro"
    ro.mkdir()
    write_db(ro / "r.db", [("A.W", [_poly(100 * i, 0) for i in range(7)])])
    r = run([INDEX, "drc", str(ro / "r.db")])
    check(r.returncode == 0, "floe-index drc r.db", r.stderr)
    tmp = d / "tmp"
    tmp.mkdir()
    os.chmod(ro, 0o555)
    saved = os.environ.get("TMPDIR")
    os.environ["TMPDIR"] = str(tmp)
    fallback = Svc()
    try:
        opened = rs_open(fallback, ro / "r.db", "ro")
        h = opened["handle"]
        reply = fallback("drc_set_status", handle=h, check=0, errors=[2, 3], status=1,
                         reason="kept elsewhere")
        found = sorted(p.name for p in tmp.iterdir() if ".reasons." in p.name)
        check(reply["reason_error"] is None and len(found) == 1
              and found[0].startswith(".r.db.reasons.ro-") and not list(ro.glob(".r.db.reasons.*"))
              and fallback("drc_reason_get", handle=h, check=0, error=3)["waive"]["text"] == "kept elsewhere",
              "a results folder that cannot be written: the reasons beside the waive sidecar's fallback",
              (reply, found))
        fallback("drc_close", handle=h)
        r, got = stats_of(ro / "r.db", env={"TMPDIR": str(tmp)})
        person = got["reviewers"][0] if got and got["reviewers"] else {}
        check(person.get("reasons_location") == "temp" and person.get("waived_with_reason") == 2
              and [(x["text"], x["errors"]) for x in person.get("reasons", [])] == [("kept elsewhere", 2)],
              "... and the statistics find them there", person.get("reasons"))
    finally:
        fallback.close()
        os.chmod(ro, 0o755)
        if saved is None:
            os.environ.pop("TMPDIR", None)
        else:
            os.environ["TMPDIR"] = saved


def opened_tag(svc, h, d):
    """The result's `floe_pack=` tag without its error count, from the waive
    sidecar's header (FLOEWAIV version 2: magic, version, 16 bytes of the
    content id)."""
    raw = (d / ".t.db.waive.kim").read_bytes()[:28]
    return raw[12:28].hex()


def counts_of(db, *more, env=None):
    r = run_env([FLOE2, "drc", str(db), "--review-counts"] + list(more), env or {})
    try:
        return r, json.loads(r.stdout)
    except ValueError:
        return r, None


def r13(temp):
    print("[R13] floe2 drc --review-counts: the counts the review files keep, at once")
    d = temp / "counts"
    d.mkdir()
    sizes = [9000, 5000, 10, 300]
    names = ["A.W", "B.S", "C,odd \"name\"", "A.W"]      # a name twice
    write_db(d / "t.db", [(names[0], [_poly(100 * (i % 120), 60 * (i // 120)) for i in range(sizes[0])]),
                          (names[1], [_edge(200 * (i % 50), 100 * (i // 50)) for i in range(sizes[1])]),
                          (names[2], [_poly(7 * i, 3 * i, 5, 5) for i in range(sizes[2])]),
                          (names[3], [_poly(90 * i, 9000) for i in range(sizes[3])])])
    r = run([INDEX, "drc", str(d / "t.db")])
    check(r.returncode == 0, "floe-index drc t.db", r.stderr)
    db = d / "t.db"
    total = sum(sizes)
    keys = ("waived", "visited", "not_required", "visited_not_required", "pending")
    listed = [{"index": ci + 1, "name": names[ci], "errors": sizes[ci]} for ci in range(4)]
    # ---- nobody has reviewed: the result's errors are said all the same
    r, got = counts_of(db)
    check(r.returncode == 0 and got is not None and got["type"] == "floe-drc-review-counts"
          and got["version"] == 1 and got["errors"] == total and got["rule_count"] == 4
          and got["reviewers"] == [] and got["skipped"] == [] and "rules" not in got
          and got["database"] == str(db) and got["pack"] == str(d / ".t.db.tray")
          and len(got["fingerprint"]) == 64 and got["generated"].endswith("Z"),
          "a result nobody reviewed: its errors, no reviewer", (r.stderr[-200:], got))
    r, got = counts_of(db, "--rule", "B.S")
    check(r.returncode == 0 and got["rules"] == [listed[1]] and got["reviewers"] == []
          and got["errors"] == total, "... and a rule's errors", got and got.get("rules"))
    # ---- the reviewers: kim and lee with flags across the 4096-error
    # chunks and in both rules named A.W, park with a waive file alone
    people = {
        "kim": ({0: set(range(0, 10)) | set(range(4090, 4100)), 2: {9}, 3: {0, 299}},
                {0: {0, 1, 2, 3, 4, 4095, 4096, 8999}, 1: {1}, 3: {5}},
                {0: set(range(100, 110)), 1: {1, 2}, 3: {5, 6}}, True),
        "lee": ({0: set(range(5, 15)) | {8999}, 1: {7}},
                {0: {0, 5, 20, 4096}, 1: {1, 7}},
                {0: {4096}}, True),
        "park": ({1: {0}}, {}, {}, False),
    }
    svc = Svc()
    try:
        h = {}
        for name, (w, v, n, known) in people.items():
            h[name] = rs_open(svc, db, name)["handle"]
            for ci, errs in w.items():
                svc("drc_set_status", handle=h[name], check=ci, errors=sorted(errs), status=1)
            count = 0
            for ci, errs in v.items():
                for e in sorted(errs):
                    svc("drc_visit", handle=h[name], check=ci, error=e)
                    count += 1
            svc.visits(h[name], count)
            for ci, errs in n.items():
                e = svc.job(h[name], "drc_review", check=ci, action="not-required",
                            errors=sorted(errs), on=True)
                check(e["state"] == "done", "%s: not for review" % name, e)
        want_own, _team = brute(sizes, people)

        def row_of(x):
            return {k: x[k] for k in keys if k in x}

        def whole_of(name):
            rows = want_own[name]
            return {k: sum(x[k] for x in rows) for k in rows[0]}
        # ---- 1. the whole result: each reviewer's four numbers
        r, got = counts_of(db)
        check(r.returncode == 0 and [x["id"] for x in got["reviewers"]] == ["kim", "lee", "park"]
              and not got["skipped"] and "rules" not in got
              and all("rules" not in x for x in got["reviewers"]),
              "the reviewers found by their working files; no rule listed unasked",
              (r.stderr[-200:], [x["id"] for x in got["reviewers"]] if got else None))
        for person in got["reviewers"]:
            name = person["id"]
            check(row_of(person) == whole_of(name) and person["errors"] == total
                  and person["present"] is True and person["waive_known"] is True
                  and person["visited_known"] is (name != "park")
                  and person["not_required_known"] is (name != "park")
                  and person["waive_location"] == "results" and person["waive_modified"].endswith("Z")
                  and Path(person["waive_file"]).name == ".t.db.waive.%s" % name
                  and ("review_file" in person) is (name != "park"),
                  "%s: the whole result's errors, waived, visited, not for review = by hand" % name,
                  (row_of(person), whole_of(name)))
        r2, exact = stats_of(db)
        check(exact is not None and all(
            row_of(a) == row_of(b) and a["id"] == b["id"]
            for a, b in zip(got["reviewers"], exact["reviewers"]))
            and got["fingerprint"] == exact["fingerprint"] and got["errors"] == exact["errors"],
            "... and what --review-stats counts from the statuses")
        # ---- 2. a rule: its four numbers; a name twice is both of them
        r, one = counts_of(db, "--rule", "B.S")
        check(r.returncode == 0 and one["rules"] == [listed[1]]
              and all(len(x["rules"]) == 1 and x["rules"][0]["index"] == 2
                      and x["rules"][0]["name"] == "B.S" and x["rules"][0]["errors"] == 5000
                      and row_of(x["rules"][0]) == want_own[x["id"]][1]
                      and row_of(x) == whole_of(x["id"]) for x in one["reviewers"]),
              "--rule NAME: the rule's errors, waived, visited, not for review beside the whole",
              [x["rules"] for x in one["reviewers"]])
        r, odd = counts_of(db, "--rule", names[2], "--floe-reviewer", "kim")
        check(r.returncode == 0 and [x["id"] for x in odd["reviewers"]] == ["kim"]
              and odd["rules"] == [listed[2]]
              and row_of(odd["reviewers"][0]["rules"][0]) == want_own["kim"][2],
              "a rule named with a comma and quotes; one reviewer asked",
              odd and odd["reviewers"])
        r, twice = counts_of(db, "--rule", "A.W")
        check(r.returncode == 0 and twice["rules"] == [listed[0], listed[3]]
              and all([y["index"] for y in x["rules"]] == [1, 4]
                      and [row_of(y) for y in x["rules"]]
                      == [want_own[x["id"]][0], want_own[x["id"]][3]] for x in twice["reviewers"]),
              "a name two rules have: both, each with its own numbers",
              [x["rules"] for x in twice["reviewers"]])
        r, _none = counts_of(db, "--rule", "NO.SUCH")
        check(r.returncode == 1 and not r.stdout and 'no such rule "NO.SUCH"' in r.stderr,
              "a rule the result does not have: refused, nothing printed", (r.returncode, r.stderr))
        # every rule: a row each, the ones nobody touched too
        r, every = counts_of(db, "--all-rules")
        check(r.returncode == 0 and every["rules"] == listed
              and all([row_of(y) for y in x["rules"]] == want_own[x["id"]]
                      and [(y["index"], y["name"], y["errors"]) for y in x["rules"]]
                      == [(z["index"], z["name"], z["errors"]) for z in listed]
                      for x in every["reviewers"]),
              "--all-rules: every rule's four numbers, a rule nobody touched too",
              [x["rules"] for x in every["reviewers"]][:1])
        # ---- 3. at once: each action is in the next answer, the review open
        def kim_now(*more):
            r, got = counts_of(db, "--floe-reviewer", "kim", *more)
            x = got["reviewers"][0]
            return ((x["waived"], x["visited"], x["not_required"]),
                    [(y["waived"], y["visited"], y["not_required"]) for y in x.get("rules", [])])
        base, _ = kim_now()
        steps = []
        svc("drc_set_status", handle=h["kim"], check=1, errors=[10, 11, 12], status=1, reason="live")
        steps.append(kim_now("--rule", "B.S"))
        sel = svc.job(h["kim"], "drc_select", check=1, mode="replace")
        svc.job(h["kim"], "drc_review", check=1, action="waive", selection=sel["selection"], on=True)
        steps.append(kim_now("--rule", "B.S"))
        svc("drc_visit", handle=h["kim"], check=1, error=4096)
        svc.visits(h["kim"], 1)
        steps.append(kim_now("--rule", "B.S"))
        svc.job(h["kim"], "drc_review", check=1, action="not-required",
                errors=list(range(4000, 4200)), on=True)
        steps.append(kim_now("--rule", "B.S"))
        svc.job(h["kim"], "drc_review", check=1, action="waive", selection=sel["selection"], on=False)
        svc.job(h["kim"], "drc_review", check=1, action="not-required",
                errors=list(range(4000, 4200)), on=False)
        svc("drc_set_status", handle=h["kim"], check=1, errors=[10, 11, 12], status=0)
        steps.append(kim_now("--rule", "B.S"))
        w0, v0, n0 = base
        check(steps == [((w0 + 3, v0, n0), [(3, 1, 2)]),
                        ((w0 + 5000, v0, n0), [(5000, 1, 2)]),
                        ((w0 + 5000, v0 + 1, n0), [(5000, 2, 2)]),
                        ((w0 + 5000, v0 + 1, n0 + 200), [(5000, 2, 202)]),
                        ((w0, v0 + 1, n0), [(0, 2, 2)])],
              "each waive, visit and exclusion is in the next answer while the review is open",
              (base, steps))
        people["kim"][1].setdefault(1, set()).add(4096)        # (the visit stays)
    finally:
        svc.close()
    # ---- 4. what it reads is the counters, never the statuses. A stored
    # counter that is not what its statuses count (two sessions of one
    # reviewer could leave one before 0.12.440) is what it says - the exact
    # report counts and says so
    side = d / ".t.db.waive.lee"
    data = bytearray(side.read_bytes())
    at = 40 + total
    data[at:at + 4] = struct.pack("<I", 3)
    side.write_bytes(bytes(data))
    r, kept = counts_of(db, "--rule", "A.W", "--floe-reviewer", "lee")
    r2, exact = stats_of(db, "--floe-reviewer", "lee")
    lee, counted = kept["reviewers"][0], exact["reviewers"][0]
    check(lee["rules"][0]["waived"] == 3 and lee["waived"] == 3 + 1
          and counted["waived"] == 12 and counted["counters_match"] is False,
          "the kept counter is what is said; --review-stats counts the statuses and says they differ",
          (lee["waived"], counted["waived"], counted.get("counter_mismatch")))
    data[at:at + 4] = struct.pack("<I", 11)
    side.write_bytes(bytes(data))
    # a counter past its rule's errors is no count: left out, and said
    data[at:at + 4] = struct.pack("<I", sizes[0] + 1)
    side.write_bytes(bytes(data))
    r, bad = counts_of(db, "--floe-reviewer", "lee")
    check(r.returncode == 0 and bad["reviewers"][0]["waive_known"] is False
          and "waived" not in bad["reviewers"][0] and bad["reviewers"][0]["visited_known"] is True
          and [(x["reviewer"], Path(x["file"]).name) for x in bad["skipped"]]
          == [("lee", ".t.db.waive.lee")] and "past their rules' errors" in bad["skipped"][0]["reason"],
          "a counter that cannot be a rule's is left out and said", bad["skipped"])
    data[at:at + 4] = struct.pack("<I", 11)
    side.write_bytes(bytes(data))
    # ... and no block of the pack: with its block table damaged the pack
    # does not open for the exact report, and the counts are the same
    r, before = counts_of(db, "--all-rules")
    pack = d / ".t.db.tray"
    raw = bytearray(pack.read_bytes())
    foot = len(raw) - 168
    blocks = struct.unpack_from("<Q", raw, foot + 48)[0]
    raw[blocks + 8] ^= 1
    whole = pack.read_bytes()
    pack.write_bytes(bytes(raw))
    r2, exact = stats_of(db)
    r, after = counts_of(db, "--all-rules")
    pack.write_bytes(whole)
    strip = lambda x: {k: v for k, v in x.items() if k != "generated"}
    check(r2.returncode == 1 and "block metadata" in r2.stderr
          and r.returncode == 0 and strip(after) == strip(before),
          "the pack is opened for its rules alone: its block table is not read",
          (r2.returncode, r2.stderr[-160:], r.returncode))
    # ---- 5. whose files: leftovers ignored, another result's said, a
    # reviewer named who never opened it absent
    shutil.copy(d / ".t.db.waive.kim", d / ".t.db.waive.kim.stale-1794300165")
    shutil.copy(d / ".t.db.waive.kim", d / ".t.db.waive.kim.tmp-4242")
    ghost = bytearray((d / ".t.db.waive.kim").read_bytes())
    ghost[12] ^= 0xFF
    (d / ".t.db.waive.ghost").write_bytes(bytes(ghost))
    r, g2 = counts_of(db)
    check([x["id"] for x in g2["reviewers"]] == ["kim", "lee", "park"]
          and [(x["reviewer"], Path(x["file"]).name) for x in g2["skipped"]]
          == [("ghost", ".t.db.waive.ghost")] and "another DRC result" in g2["skipped"][0]["reason"],
          "leftovers ignored, another result's file skipped with its reason", g2["skipped"])
    r, some = counts_of(db, "--floe-reviewer", "kim,nobody", "--rule", "B.S")
    check([(x["id"], x["present"]) for x in some["reviewers"]] == [("kim", True), ("nobody", False)]
          and "waived" not in some["reviewers"][1] and some["rules"] == [listed[1]],
          "--floe-reviewer ID,ID: these alone, one without files absent", some["reviewers"][1])
    # ---- 6. the pack must be the current one, as for the exact report
    st = os.stat(db)
    raw = db.read_bytes()
    os.utime(db, (st.st_atime, st.st_mtime + 5))
    r, touched = counts_of(db)
    check(r.returncode == 0 and touched is not None
          and [x["waived"] for x in touched["reviewers"]] == [x["waived"] for x in g2["reviewers"]],
          "a .db whose time alone changed: its pack is current", (r.returncode, r.stderr[-300:]))
    db.write_bytes(other_bytes(raw))
    os.utime(db, (st.st_atime, st.st_mtime + 9))
    r, none = counts_of(db)
    check(r.returncode == 1 and not r.stdout and "stale DRC pack" in r.stderr,
          "a stale pack: refused, nothing printed", (r.returncode, r.stderr[-300:]))
    db.write_bytes(raw)
    os.utime(db, (st.st_atime, st.st_mtime))
    bare = d / "bare"
    bare.mkdir()
    shutil.copy(db, bare / "t.db")
    r, none = counts_of(bare / "t.db")
    check(r.returncode == 1 and not r.stdout and "no DRC pack" in r.stderr,
          "no pack: refused", (r.returncode, r.stderr[-300:]))
    # the pack itself named: read without its .db
    r, direct = counts_of(d / ".t.db.tray", "--floe-reviewer", "kim")
    check(r.returncode == 0 and direct["reviewers"][0]["waived"] == g2["reviewers"][0]["waived"],
          "the pack named in place of the .db", (r.returncode, r.stderr[-200:]))
    for args, want in ((["--review-counts", "--review-stats"], "--review-counts is a report of its own"),
                       (["--review-counts", "--rules"], "--review-counts is a report of its own"),
                       (["--rule", "A.W"], "--rule / --all-rules require --review-counts"),
                       (["--all-rules"], "--rule / --all-rules require --review-counts"),
                       (["--review-counts", "--rule", "A.W", "--all-rules"], "one of the two"),
                       (["--review-counts", "--rule", "A.W", "--rule", "B.S"], "--rule names one rule")):
        r = run_env([FLOE2, "drc", str(db)] + args, {})
        check(r.returncode == 2 and not r.stdout and want in r.stderr,
              "refused (an argument error): %s" % " ".join(args), (r.returncode, r.stderr[-200:]))
    r = run_env([FLOE2, "drc", str(db), "--review-counts", "--floe-reviewer", ","], {})
    check(r.returncode == 1 and not r.stdout and "names no reviewer" in r.stderr,
          "a reviewer list that names nobody: refused, as --review-stats refuses it",
          (r.returncode, r.stderr[-200:]))
    # ---- 7. --workdir: the reviews kept under it are the ones read with it
    work = d / "work"
    r = run_env([FLOE2, "--workdir", str(work), "drc", str(db), "--build"], {})
    check(r.returncode == 0, "a pack under --workdir", r.stderr[-300:])
    ws = svc_with(dict(FLOE_WORKDIR=str(work)))
    try:
        o = ws("drc_open", path=str(db), mode="load", reviewer="yoon")
        ws("drc_set_status", handle=o["handle"], check=1, errors=[3, 4], status=1)
    finally:
        ws.close()
    r, w = counts_of(db, "--rule", "B.S", env=dict(FLOE_WORKDIR=str(work)))
    check(w is not None and [(x["id"], x["waived"], x["rules"][0]["waived"]) for x in w["reviewers"]]
          == [("yoon", 2, 2)] and str(work) in w["pack"],
          "its counts with the same work directory", w and w["reviewers"])


# ---- R14 (2026-11-13: the team's union, kept) ----

TEAM_KEYS = ("errors", "visited", "waived", "not_required", "conflicts", "pending")


def team_now(db, *more, env=None):
    """The team of `--review-counts`: its numbers, the whole object, the report."""
    r, got = counts_of(db, *more, env=env)
    if got is None:
        raise AssertionError("--review-counts: %s" % r.stderr[-400:])
    t = got["team"]
    return {k: t[k] for k in TEAM_KEYS}, t, got


def file_state(path):
    try:
        st = os.stat(path)
    except OSError:
        return None
    return st.st_ino, st.st_mtime_ns, st.st_size


def r14(temp):
    print("[R14] floe2 drc --review-counts: the team's union, kept and brought up to date by what changed")
    d = temp / "live"
    d.mkdir()
    sizes = [9000, 5000, 10, 300, 70000]
    names = ["A.W", "B.S", "C,odd \"name\"", "A.W", "E.BIG"]     # a name twice
    write_db(d / "t.db", [(names[0], [_poly(100 * (i % 120), 60 * (i // 120)) for i in range(sizes[0])]),
                          (names[1], [_edge(200 * (i % 50), 100 * (i // 50)) for i in range(sizes[1])]),
                          (names[2], [_poly(7 * i, 3 * i, 5, 5) for i in range(sizes[2])]),
                          (names[3], [_poly(90 * i, 9000) for i in range(sizes[3])]),
                          (names[4], [_poly(60 * (i % 400), 30000 + 40 * (i // 400), 20, 10)
                                      for i in range(sizes[4])])])
    r = run([INDEX, "drc", str(d / "t.db")])
    check(r.returncode == 0, "floe-index drc t.db", r.stderr)
    db = d / "t.db"
    total = sum(sizes)
    starts = [sum(sizes[:ci]) for ci in range(len(sizes))]
    cache = d / ".t.db.team"
    lock = d / ".floe-lock" / "t.db.team.build"
    zero = {"errors": total, "visited": 0, "waived": 0, "not_required": 0, "conflicts": 0,
            "pending": total}
    # ---- 0. nobody has reviewed: a team of nobody, and nothing made for it
    nums, t, got = team_now(db)
    check(nums == zero and t["reviewers"] == 0 and t["current"] is True and t["kept"] is True
          and t["counted"] == 0 and "rules" not in t and "waiting" not in t and "skipped" not in t
          and not cache.exists() and not lock.exists(),
          "a result nobody reviewed: a team of nobody, nothing kept for it", t)
    nums, t, got = team_now(db, "--rule", "A.W")
    check([(x["index"], x["name"], x["errors"], x["pending"], x["waived"]) for x in t["rules"]]
          == [(1, "A.W", 9000, 9000, 0), (4, "A.W", 300, 300, 0)],
          "... and the team's row of a rule asked for (a name twice: both)", t.get("rules"))
    # ---- the reviewers and what they did, by hand: {id: (waived, visited,
    # set aside, flags known)} with {rule: set}
    people = {}

    def of(name, slot, ci):
        return people[name][slot].setdefault(ci, set())

    def want():
        _own, rows = brute(sizes, people)
        return {k: sum(x[k] for x in rows) for k in TEAM_KEYS}, rows

    def agree(what, counted=None, env=None, exact_only=False):
        """The team of --review-counts = by hand = what --review-stats counts
        from every status byte and bit plane, whole and rule by rule; and
        how many chunks the answer had to count again."""
        nums, t, got = team_now(db, "--all-rules", env=env)
        r2, exact = stats_of(db)
        et = exact["team"]
        rows = [{k: x[k] for k in TEAM_KEYS} for x in t["rules"]]
        ok = (t["current"] is True and "waiting" not in t and t["kept"] is True
              and {k: et[k] for k in TEAM_KEYS} == nums
              and [{k: x[k] for k in TEAM_KEYS} for x in et["rules"]] == rows
              and t["reviewers"] == et["reviewers"]
              and [(x["index"], x["name"]) for x in t["rules"]]
              == [(ci + 1, names[ci]) for ci in range(len(sizes))])
        if not exact_only:
            whole, by_rule = want()
            ok = ok and nums == whole and rows == by_rule
        if counted is not None:
            ok = ok and t["counted"] == counted
        check(ok, what, (nums, t.get("waiting"), "counted %s" % t["counted"],
                         {k: et[k] for k in TEAM_KEYS}, None if exact_only else want()[0]))
        return t

    a, b = Svc(), Svc()
    try:
        # kim and lee in one viewer process, park in another
        svc_of = {"kim": a, "lee": a, "park": b}
        h = {}
        for name in ("kim", "lee", "park"):
            h[name] = rs_open(svc_of[name], db, name)["handle"]
            people[name] = ({}, {}, {}, True)
        t = agree("three sessions that have done nothing: three reviewers with nothing")
        check(t["reviewers"] == 3 and cache.exists(), "... and their counts are kept", t)

        def waive(name, ci, errs, on=True):
            svc_of[name]("drc_set_status", handle=h[name], check=ci, errors=sorted(errs),
                         status=1 if on else 0)
            (of(name, 0, ci).update if on else of(name, 0, ci).difference_update)(errs)

        def visit(name, ci, errs):
            for e in sorted(errs):
                svc_of[name]("drc_visit", handle=h[name], check=ci, error=e)
            svc_of[name].visits(h[name], len(errs))
            of(name, 1, ci).update(errs)

        def exclude(name, ci, errs, on=True):
            e = svc_of[name].job(h[name], "drc_review", check=ci, action="not-required",
                                 errors=sorted(errs), on=on)
            check(e["state"] == "done", "%s: not for review" % name, e)
            (of(name, 2, ci).update if on else of(name, 2, ci).difference_update)(errs)

        # (each reviewer's first waive and first visit make their change log
        # and their flags file: a file that comes has its reviewer's rules
        # counted whole, once)
        for i, name in enumerate(("kim", "lee", "park")):
            waive(name, 2, {i})
            visit(name, 2, {i})
        agree("each reviewer's first waive and visit")
        for name in ("kim", "lee", "park"):
            check((d / (".t.db.changes.%s" % name)).exists(), "%s's change log beside their sidecar" % name)
        # ---- 1. each action is in the next answer, and costs what it changed
        waive("kim", 0, {0, 1, 2})
        agree("a click's waive: one chunk counted again", counted=1)
        waive("kim", 0, {4095, 4096})
        agree("two errors either side of a chunk's end: two chunks", counted=2)
        before = (file_state(cache), file_state(lock))
        agree("nothing changed: nothing counted", counted=0)
        check((file_state(cache), file_state(lock)) == before and before[0] and before[1],
              "... the kept file is not written again and no lock is taken", before)
        # two who visited one error and judged it differently: a conflict,
        # and no waive of the team's; one of them alone visiting is none
        visit("lee", 0, {1, 4096})
        visit("kim", 0, {1})
        t = agree("visits by two reviewers", counted=2)
        check(t["rules"][0]["conflicts"] == 1 and t["rules"][0]["waived"] == 4
              and t["rules"][0]["visited"] == 2, "a conflict: the error is not the team's waive", t["rules"][0])
        waive("park", 0, {1})
        t = agree("a third reviewer, in another viewer, who did not visit it: still a conflict", counted=1)
        check(t["rules"][0]["conflicts"] == 1, "... (their waive is no vote)", t["rules"][0])
        waive("lee", 0, {1})
        t = agree("the other visitor waives it too: no conflict", counted=1)
        check(t["rules"][0]["conflicts"] == 0 and t["rules"][0]["waived"] == 5, "... and the team's waive", t["rules"][0])
        # an error two reviewers waived is one
        waive("lee", 0, {0, 2})
        t = agree("an error two reviewers waived is one of the team's", counted=1)
        check(t["rules"][0]["waived"] == 5 and t["waived"] == 5 + 3, "... (the union, not the sum)", t)
        # exclusions, across a chunk's end; visited and judged differently
        exclude("lee", 1, set(range(4090, 4100)))
        agree("an exclusion across a chunk's end", counted=2)
        visit("lee", 1, {4091})
        visit("kim", 1, {4091})
        t = agree("an exclusion one visitor made and another did not: a conflict", counted=1)
        check(t["rules"][1]["conflicts"] == 1 and t["rules"][1]["not_required"] == 9, "... not the team's exclusion", t["rules"][1])
        # a job over a selection (the whole rule), and its taking back
        sel = svc_of["kim"].job(h["kim"], "drc_select", check=1, mode="replace")
        e = svc_of["kim"].job(h["kim"], "drc_review", check=1, action="waive",
                              selection=sel["selection"], on=True)
        check(e["state"] == "done", "a job's waive of a rule", e)
        of("kim", 0, 1).update(range(sizes[1]))
        agree("a job's waive of 5,000 errors: its two chunks", counted=2)
        e = svc_of["kim"].job(h["kim"], "drc_review", check=1, action="waive",
                              selection=sel["selection"], on=False)
        of("kim", 0, 1).clear()
        agree("... taken back", counted=2)
        exclude("lee", 1, {4090, 4091}, on=False)
        waive("kim", 0, {0}, on=False)
        agree("an exclusion and a waive taken back", counted=2)
        # ---- 2. a job that changes many errors: the viewer brings the kept
        # counts up to date itself when the job ends, so whoever asks next
        # has nothing to count
        big = svc_of["park"].job(h["park"], "drc_select", check=4, mode="replace")

        def folded(what, action, on):
            was = file_state(cache)
            e = svc_of["park"].job(h["park"], "drc_review", check=4, action=action,
                                   selection=big["selection"], on=on)
            check(e["state"] == "done" and e["count"] == sizes[4], "park: %s" % what, e)
            deadline = time.monotonic() + 30
            while file_state(cache) == was and time.monotonic() < deadline:
                time.sleep(0.01)
            check(file_state(cache) != was, "... the viewer wrote the kept counts when its job ended")

        folded("70,000 errors waived by a job", "waive", True)
        of("park", 0, 4).update(range(sizes[4]))
        agree("... and the next answer counts nothing", counted=0)
        folded("70,000 errors set aside by a job", "not-required", True)
        of("park", 2, 4).update(range(sizes[4]))
        t = agree("the same for an exclusion", counted=0)
        check(t["rules"][4]["waived"] == sizes[4] and t["rules"][4]["not_required"] == sizes[4]
              and t["rules"][4]["pending"] == 0, "... the rule is done", t["rules"][4])
        folded("the waive taken back", "waive", False)
        of("park", 0, 4).clear()
        agree("... counted by the viewer too", counted=0)
        # (a smaller job is left to whoever asks: the exclusion of 3,000)
        exclude("park", 4, set(range(100, 3100)), on=False)
        agree("a smaller job: counted by the answer", counted=1)
        # ---- 3. the team is everyone's, whoever is asked about
        whole, by_rule = want()
        nums, t, got = team_now(db, "--floe-reviewer", "kim", "--rule", "B.S")
        check(nums == whole and t["reviewers"] == 3 and [x["id"] for x in got["reviewers"]] == ["kim"]
              and [{k: x[k] for k in TEAM_KEYS} for x in t["rules"]] == [by_rule[1]]
              and t["rules"][0]["index"] == 2 and t["rules"][0]["name"] == "B.S",
              "--floe-reviewer narrows the reviewers' rows, not the team", (nums, whole, t))
        nums, t, got = team_now(db)
        check(nums == whole and "rules" not in t, "no rule asked: the team's whole numbers alone", t)
        # ---- 4. a peer's decisions applied: said by the session that took them
        e = svc_of["lee"].job(h["lee"], "drc_team_load",
                              paths=[str(d / ".t.db.review.kim.sqlite3")])
        sids = [x["snapshot"] for x in e.get("loaded") or []]
        was = file_state(cache)
        e = svc_of["lee"].job(h["lee"], "drc_team_apply", source=sids[0], force=False) if sids else {}
        check(e.get("state") == "done" and e["result"]["applied"] > 0, "lee applies kim's decisions", e)
        deadline = time.monotonic() + 30
        while file_state(cache) == was and time.monotonic() < deadline:
            time.sleep(0.01)
        agree("the team after an application = the exact report's", counted=0, exact_only=True)
        # (what lee holds now, read back for the hand count)
        r2, exact = stats_of(db)
        check(exact is not None, "the exact report")
    finally:
        a.close()
        b.close()
    # the hand count goes on from what the files say now (the application's
    # rules are R4's): each reviewer's statuses and planes, read here
    def read_people():
        out = {}
        for side in sorted(d.glob(".t.db.waive.*")):
            name = side.name[len(".t.db.waive."):]
            if "." in name or "-" in name:
                continue
            raw = side.read_bytes()
            if len(raw) != 40 + total + 4 * len(sizes):
                continue
            w = {ci: {e for e in range(sizes[ci]) if raw[40 + starts[ci] + e] == 1}
                 for ci in range(len(sizes))}
            v, n, known = {}, {}, False
            flags = d / (".t.db.review.%s.sqlite3" % name)
            if flags.exists():
                known = True
                con = sqlite3.connect("file:%s?mode=ro" % flags, uri=True)
                for ci, k, vb, nb in con.execute("SELECT ci,chunk,visited,nr FROM chunks"):
                    for blob, into in ((vb, v), (nb, n)):
                        for i, byte in enumerate(blob):
                            for bit in range(8):
                                if byte >> bit & 1:
                                    into.setdefault(ci, set()).add(k * 4096 + i * 8 + bit)
                con.close()
            out[name] = ({ci: s for ci, s in w.items() if s}, v, n, known)
        return out

    people.clear()
    people.update(read_people())
    agree("the sessions closed: the team by hand from the files, nothing counted", counted=0)

    def poke(name, ci, error, value):
        """A status written as a writer that says nothing would: the byte
        and the rule's counter."""
        side = d / (".t.db.waive.%s" % name)
        data = bytearray(side.read_bytes())
        at = 40 + starts[ci] + error
        old = data[at]
        data[at] = value
        c = 40 + total + 4 * ci
        struct.pack_into("<I", data, c, struct.unpack_from("<I", data, c)[0] + (value == 1) - (old == 1))
        was = file_state(side)
        side.write_bytes(bytes(data))
        while file_state(side) is not None and was is not None and os.stat(side).st_mtime_ns <= was[1]:
            time.sleep(0.002)
            os.utime(side)
        if name in people:
            (people[name][0].setdefault(ci, set()).add if value == 1
             else people[name][0].setdefault(ci, set()).discard)(error)

    # ---- 5. a reviewer of a build before the change log - files, no log:
    # counted again at once, each time their files change
    head = (d / ".t.db.waive.kim").read_bytes()[:40]
    (d / ".t.db.waive.old").write_bytes(head + bytes(total + 4 * len(sizes)))
    people["old"] = ({}, {}, {}, False)
    poke("old", 0, 5, 1)
    poke("old", 0, 6, 1)
    t = agree("a reviewer without a change log: in the team at once")
    check(t["reviewers"] == 4 and not (d / ".t.db.changes.old").exists(), "... (no log made for them)", t)
    poke("old", 0, 7, 1)
    poke("old", 4, 69999, 1)
    agree("... and each change of their files")
    agree("... counted once", counted=0)
    # ---- 6. a reviewer with a log whose files change without a word (a
    # session of an older build, a run that ended before its record): the
    # record is waited for - the counts as they were, said not current -
    # and their rules counted again when the grace ends; at once without one
    before, _t, _got = team_now(db)
    poke("kim", 0, 100, 1)
    nums, t, got = team_now(db)
    check(nums == before and t["current"] is False and len(t.get("waiting") or []) == 1
          and t["waiting"][0].startswith("kim's review files") and t["counted"] == 0,
          "files changed without a record: the counts as they were, said not current", t)
    was = file_state(cache)
    nums, t, got = team_now(db)
    check(nums == before and t["current"] is False and file_state(cache) == was,
          "... still waiting: nothing written for it", t)
    grace0 = dict(FLOE_TEAM_GRACE_S="0")
    agree("FLOE_TEAM_GRACE_S=0: counted again at once", env=grace0)
    before, _t, _got = team_now(db)
    poke("kim", 0, 101, 1)
    grace1 = dict(FLOE_TEAM_GRACE_S="1")
    nums, t, got = team_now(db, env=grace1)
    check(nums == before and t["current"] is False, "a grace of a second: waited for", t)
    time.sleep(1.2)
    agree("... and counted again when it ended", env=grace1)
    # the reviewer's next session finds its files changed without a word and
    # says so before anything else: at once, whatever the grace
    poke("kim", 0, 102, 1)
    c = Svc()
    try:
        hk = rs_open(c, db, "kim")["handle"]
        agree("a session that finds its files changed says so: counted at once")
        c("drc_set_status", handle=hk, check=0, errors=[103], status=1)
        people["kim"][0].setdefault(0, set()).add(103)
        agree("... and its own changes cost a chunk again", counted=1)
        # a record lost between the bits and the word
        before, _t, _got = team_now(db)
        c("drc_set_status", handle=hk, check=0, errors=[104], status=1)
        people["kim"][0][0].add(104)
        log = d / ".t.db.changes.kim"
        with open(log, "r+b") as f:
            f.truncate(os.path.getsize(log) - 48)
        nums, t, got = team_now(db)
        check(nums == before and t["current"] is False, "a record lost: the counts as they were, not current", t)
        agree("... counted again without a grace", env=grace0)
        c("drc_set_status", handle=hk, check=0, errors=[105], status=1)
        people["kim"][0][0].add(105)
        agree("the session goes on saying what it changes", counted=1)
    finally:
        c.close()
    # ---- 7. a reviewer's files removed: their bits go with them
    (d / ".t.db.waive.old").unlink()
    del people["old"]
    t = agree("a reviewer's files removed: no longer in the team")
    check(t["reviewers"] == 3, "... three reviewers", t["reviewers"])
    # ---- 8. the kept file damaged or gone: made again
    raw = bytearray(cache.read_bytes())
    raw[-5] ^= 1
    cache.write_bytes(bytes(raw))
    t = agree("the kept file damaged: counted again")
    check(t["counted"] > 20, "... (every chunk anyone has a bit in)", t["counted"])
    agree("... and kept", counted=0)
    # several asking at once, nothing kept yet: every answer is right
    cache.unlink()
    whole, _rows = want()
    procs = [subprocess.Popen([FLOE2, "drc", str(db), "--review-counts"], stdout=subprocess.PIPE,
                              stderr=subprocess.PIPE, text=True) for _ in range(6)]
    outs = [p.communicate(timeout=120) for p in procs]
    answers = [json.loads(o[0])["team"] for o in outs if o[0]]
    check(len(answers) == 6 and all(p.returncode == 0 for p in procs)
          and all({k: x[k] for k in TEAM_KEYS} == whole and x["current"] for x in answers),
          "six asking at once with nothing kept: every answer right", [x.get("waiting") for x in answers])
    agree("... and kept once they are done", counted=0)
    # ---- 9. a results folder that takes no file: counted for the answer
    cache.unlink()
    os.chmod(d, 0o555)
    try:
        nums, t, got = team_now(db)
    finally:
        os.chmod(d, 0o755)
    check(nums == whole and t["current"] is True and t["kept"] is False and not cache.exists(),
          "a folder that cannot be written: the team counted for the answer, kept nowhere", t)
    agree("... kept again when it can be")
    check(not list(d.glob(".t.db.team.tmp-*")), "no temporary of the kept file is left")
    # ---- 9b. FLOE_TEAM_KEEP=off, the switch: a session says nothing in a
    # change log and removes the one it has, the viewer keeps no counts
    # after its jobs, and the command counts the team for each answer
    off = dict(FLOE_TEAM_KEEP="off")
    check((d / ".t.db.changes.kim").exists(), "(kim has a change log)")
    s = svc_with(off)
    try:
        hk = rs_open(s, db, "kim")["handle"]
        check(not (d / ".t.db.changes.kim").exists(),
              "FLOE_TEAM_KEEP=off: the session removes its change log")
        s("drc_set_status", handle=hk, check=0, errors=[200, 4200], status=1)
        people["kim"][0][0].update({200, 4200})
        was = file_state(cache)
        sel = s.job(hk, "drc_select", check=4, mode="replace")
        e = s.job(hk, "drc_review", check=4, action="waive", selection=sel["selection"], on=True)
        check(e["state"] == "done" and e["count"] == sizes[4], "... a job of 70,000 under it", e)
        people["kim"][0].setdefault(4, set()).update(range(sizes[4]))
    finally:
        s.close()
    time.sleep(0.3)
    check(not (d / ".t.db.changes.kim").exists() and file_state(cache) == was,
          "... it says nothing, and its viewer keeps no counts", (file_state(cache), was))
    whole, by_rule = want()
    for n in (1, 2):
        nums, t, got = team_now(db, "--all-rules", env=off)
        check(nums == whole and [{k: x[k] for k in TEAM_KEYS} for x in t["rules"]] == by_rule
              and t["current"] is True and t["kept"] is False and t["counted"] > 18
              and file_state(cache) == was,
              "... the command counts the team for the answer (%d), the kept file untouched" % n,
              (nums, whole, t["kept"], t["counted"]))
    t = agree("the switch on again: the reviewer without a log is counted by their files' times")
    check(t["counted"] > 18 and file_state(cache) != was, "... and kept", t["counted"])
    agree("... once", counted=0)
    # ---- 10. --workdir: kept with the reviews under it, nothing beside the source
    work = d / "work"
    r = run_env([FLOE2, "--workdir", str(work), "drc", str(db), "--build"], {})
    check(r.returncode == 0, "a pack under --workdir", r.stderr[-300:])
    ws = svc_with(dict(FLOE_WORKDIR=str(work)))
    try:
        o = ws("drc_open", path=str(db), mode="load", reviewer="yoon")
        ws("drc_set_status", handle=o["handle"], check=1, errors=[3, 4], status=1)
    finally:
        ws.close()
    nums, t, got = team_now(db, env=dict(FLOE_WORKDIR=str(work)))
    kept = [p for p in work.rglob(".t.db.team")]
    logs = [p for p in work.rglob(".t.db.changes.yoon")]
    check(nums["waived"] == 2 and t["reviewers"] == 1 and t["current"] is True and len(kept) == 1
          and len(logs) == 1 and not (d / ".t.db.changes.yoon").exists(),
          "under --workdir: the team of the reviews kept there, its files with them", (nums, kept, logs))


# ---- R15 (2026-11-13: the confirmer and the confirm state) ----

def r15(temp):
    print("[R15] confirming the team's waives: the confirmer's own file, the lists, the statistics")
    d = temp / "confirm"
    d.mkdir()
    sizes = [9000, 5000, 70000]
    names = ["A.W", "B.S", "E.BIG"]
    write_db(d / "t.db", [(names[0], [_poly(100 * (i % 120), 60 * (i // 120)) for i in range(sizes[0])]),
                          (names[1], [_edge(200 * (i % 50), 100 * (i // 50)) for i in range(sizes[1])]),
                          (names[2], [_poly(60 * (i % 400), 30000 + 40 * (i // 400), 20, 10)
                                      for i in range(sizes[2])])])
    r = run([INDEX, "drc", str(d / "t.db")])
    check(r.returncode == 0, "floe-index drc t.db", r.stderr)
    db = d / "t.db"
    total = sum(sizes)
    starts = [sum(sizes[:ci]) for ci in range(len(sizes))]
    cache = d / ".t.db.team"
    # by hand: who waived, visited and set aside what (brute's people), and
    # who confirmed what
    people = {n: ({}, {}, {}, True) for n in ("kim", "lee", "hong")}
    confirms = {}

    def want():
        """The team by hand: brute's rows with the confirmed and the
        unconfirmed - of the errors anybody has waived, a conflict's too."""
        _own, rows = brute(sizes, people)
        for ci in range(len(sizes)):
            waived = set().union(*[p[0].get(ci, set()) for p in people.values()])
            marked = set().union(*[c.get(ci, set()) for c in confirms.values()] or [set()])
            rows[ci]["confirmed"] = len(waived & marked)
            rows[ci]["unconfirmed"] = len(waived - marked)
        keys = TEAM_KEYS + ("confirmed", "unconfirmed")
        return {k: sum(x[k] for x in rows) for k in keys}, [{k: x[k] for k in keys} for x in rows]

    def agree(what, counted=None, env=None):
        nums, t, got = team_now(db, "--all-rules", env=env)
        whole, by_rule = want()
        keys = tuple(whole)
        r2, exact = stats_of(db)
        et = exact["team"]
        ok = (t["current"] is True and {k: t[k] for k in keys} == whole
              and [{k: x[k] for k in keys} for x in t["rules"]] == by_rule
              and {k: et[k] for k in keys} == whole
              and [{k: x[k] for k in keys} for x in et["rules"]] == by_rule)
        if counted is not None:
            ok = ok and t["counted"] == counted
        check(ok, what, ({k: t[k] for k in keys}, whole, t.get("waiting"), "counted %s" % t["counted"]))
        return t, got, exact

    def person(report, name):
        return [x for x in report["reviewers"] if x["id"] == name][0]

    a, b = Svc(), Svc()         # the reviewers' viewer, the confirmer's
    try:
        opened = rs_open(a, db, "kim")
        hk = opened["handle"]
        hl = rs_open(a, db, "lee")["handle"]
        o = b("drc_open", path=str(db), mode="load", reviewer="hong", confirmer=True)
        hh = o["handle"]
        side = Path(o["confirm_path"])
        check(o["confirmer"] is True and opened["confirmer"] is False
              and side.name == ".t.db.confirm.hong" and not side.exists(),
              "a session opened as a confirmer's; no confirm file until something is confirmed", o.get("confirmer"))
        a("drc_set_status", handle=hk, check=0, errors=[0, 1, 2, 4095, 4096], status=1, reason="dummy fill")
        people["kim"][0][0] = {0, 1, 2, 4095, 4096}
        a("drc_set_status", handle=hl, check=0, errors=[2, 3], status=1)
        a("drc_set_status", handle=hl, check=1, errors=[7], status=1, reason="seal ring")
        people["lee"][0].update({0: {2, 3}, 1: {7}})
        # a conflict: both visited error 1, one of them waived it
        for h, name in ((hk, "kim"), (hl, "lee")):
            a("drc_visit", handle=h, check=0, error=1)
            a.visits(h, 1)
            people[name][1][0] = {1}
        t, got, exact = agree("nothing confirmed: every waive - a conflict's too - waits for it")
        check(t["conflicts"] == 1 and t["waived"] == 6 and t["unconfirmed"] == 7 and t["confirmed"] == 0
              and "confirmed" not in person(got, "kim") and person(exact, "kim")["waived_confirmed"] == 0,
              "... (unconfirmed counts the conflict's waive, waived does not)", t)
        # ---- 1. the list: every error somebody waived, who and why
        e = b.job(hh, "drc_team_waives", check=0, show="unconfirmed", start=0, limit=100, refresh=True)
        rows = {x["error"]: x for x in e.get("rows", [])}
        check(e["state"] == "done" and e["count"] == 6 and sorted(rows) == [0, 1, 2, 3, 4095, 4096]
              and rows[2]["waived_by"] == ["kim", "lee"] and rows[3]["waived_by"] == ["lee"]
              and [(w["reviewer"], w["text"]) for w in rows[2]["reasons"]] == [("kim", "dummy fill")]
              and rows[3]["reasons"] == [] and all(x["confirmed_by"] == [] for x in rows.values())
              and e["rule"] == dict(waived=6, confirmed=0, unconfirmed=6)
              and e["all"] == dict(waived=7, confirmed=0, unconfirmed=7)
              and e["rules"] == [dict(check=0, waived=6, confirmed=0, unconfirmed=6),
                                 dict(check=1, waived=1, confirmed=0, unconfirmed=1)]
              and e["reviewers"] == ["hong", "kim", "lee"] and e["confirmers"] == [] and e["skipped"] == [],
              "the team's waives listed: an error two reviewers waived is one, with who and why", e)
        e = b.job(hh, "drc_team_waives", check=0, show="waived", start=4, limit=1)
        check(e["count"] == 6 and [x["error"] for x in e["rows"]] == [4095], "... a page of it", e.get("rows"))
        e = b.job(hh, "drc_team_waives", check=0, show="confirmed", start=0, limit=100)
        check(e["count"] == 0 and e["rows"] == [], "... nothing under confirmed yet")
        for fields, word in ((dict(check=0, show="everything", start=0, limit=1), "unsupported team waive list"),
                             (dict(check=9, show="waived", start=0, limit=1), "check index out of range")):
            try:
                b("drc_team_waives", handle=hh, **fields)
                said = None
            except gtkservice.ServiceError as exc:
                said = str(exc)
            check(said is not None and word in said, "refused: %s" % word, said)
        # ---- 2. only a confirmer's session confirms
        try:
            a("drc_team_confirm", handle=hk, check=0, on=True)
            said = None
        except gtkservice.ServiceError as exc:
            said = (exc.kind, str(exc))
        check(said is not None and "--confirmer" in said[1] and not side.exists()
              and not (d / ".t.db.confirm.kim").exists(),
              "a reviewer's session does not confirm: refused, nothing written", said)
        # ---- 3. errors named: the waived ones are confirmed, the rest said
        e = b.job(hh, "drc_team_confirm", check=0, errors=[0, 2, 7], on=True)
        check(e["state"] == "done" and (e["changed"], e["skipped"]) == (2, 1), "confirm three named errors: two waived, one nobody's", e)
        confirms["hong"] = {0: {0, 2}}
        t, got, exact = agree("the confirmed in the team's counts, kept and exact")
        raw = side.read_bytes()
        check(len(raw) == 40 + total + 4 * len(sizes) and raw[:8] == b"FLOECONF"
              and struct.unpack_from("<I", raw, 8)[0] == 1
              and raw[12:40] == (d / ".t.db.waive.hong").read_bytes()[12:40]
              and [i for i in range(sizes[0]) if raw[40 + i] == 1] == [0, 2]
              and list(struct.unpack_from("<3I", raw, 40 + total)) == [2, 0, 0],
              "the confirmer's file: its header, a byte an error, a counter a rule", raw[:12])
        check(person(got, "hong")["confirmed"] == 2 and Path(person(got, "hong")["confirm_file"]) == side
              and person(got, "hong")["confirm_location"] == "results"
              and person(exact, "hong")["confirmed"] == 2
              and person(exact, "kim")["waived_confirmed"] == 2 and person(exact, "lee")["waived_confirmed"] == 1
              and "confirmed" not in person(exact, "kim"),
              "each reviewer's row: what they confirmed, and of their waives the confirmed ones",
              (person(got, "hong"), person(exact, "kim")))
        e = b.job(hh, "drc_team_waives", check=0, show="confirmed", start=0, limit=100, refresh=True)
        check([(x["error"], x["confirmed_by"]) for x in e["rows"]] == [(0, ["hong"]), (2, ["hong"])]
              and e["confirmers"] == ["hong"] and e["rule"] == dict(waived=6, confirmed=2, unconfirmed=4),
              "the confirmed list says by whom", e.get("rows"))
        one = a.job(hk, "drc_team_waive_get", check=0, error=2)
        check(one["state"] == "done" and (one["check"], one["error"]) == (0, 2)
              and one["waived_by"] == ["kim", "lee"] and one["confirmed_by"] == ["hong"]
              and one["confirmer"] is False and one["reviewer"] == "kim"
              and [(w["reviewer"], w["text"]) for w in one["reasons"]] == [("kim", "dummy fill")],
              "one error asked from a reviewer's session: who waived, why, who confirmed", one)
        # ---- 4. what an answer costs is what changed
        e = b.job(hh, "drc_team_confirm", check=0, errors=[4095, 4096], on=True)
        confirms["hong"][0] |= {4095, 4096}
        agree("two errors either side of a chunk's end confirmed: two chunks counted", counted=2)
        e = b.job(hh, "drc_team_confirm", check=0, errors=[1], on=True)
        confirms["hong"][0].add(1)
        t, got, exact = agree("a conflict's waive confirmed: one chunk", counted=1)
        check(t["rules"][0]["conflicts"] == 1 and t["rules"][0]["confirmed"] == 5,
              "... confirmed, and still the reviewers' conflict", t["rules"][0])
        # every waive of a rule
        e = b.job(hh, "drc_team_confirm", check=1, on=True)
        check((e["changed"], e["skipped"]) == (1, 0), "every waive of a rule confirmed", e)
        confirms["hong"][1] = {7}
        agree("... in the counts")
        # ---- 5. a confirmed waive turned back by its reviewer no longer
        # counts - the confirmer's file still has it - and waived again by
        # anybody it stands again
        a("drc_set_status", handle=hk, check=0, errors=[0], status=0)
        people["kim"][0][0].discard(0)
        t, got, exact = agree("a confirmed waive turned back: out of the confirmed", counted=1)
        check(person(got, "hong")["confirmed"] == 6 and t["rules"][0]["confirmed"] == 4,
              "... the confirmer's own count is the file's", (person(got, "hong")["confirmed"], t["rules"][0]))
        a("drc_set_status", handle=hl, check=0, errors=[0], status=1)
        people["lee"][0][0].add(0)
        agree("waived again by another reviewer: confirmed again", counted=1)
        # ---- 6. taken back; a second confirmer
        e = b.job(hh, "drc_team_confirm", check=0, errors=[2, 3], on=False)
        check((e["changed"], e["skipped"]) == (1, 0), "a confirmation taken back (one of the two was none)", e)
        confirms["hong"][0].discard(2)
        agree("... out of the counts", counted=1)
        c = Svc()
        try:
            hy = c("drc_open", path=str(db), mode="load", reviewer="yoon", confirmer=True)["handle"]
            c.svc.drc_watch(hy)
            e = c.job(hy, "drc_team_confirm", check=0, errors=[2, 3, 4096], on=True)
            check((e["changed"], e["skipped"]) == (3, 0), "a second confirmer", e)
            confirms["yoon"] = {0: {2, 3, 4096}}
            people["yoon"] = ({}, {}, {}, False)
        finally:
            c.close()
        t, got, exact = agree("two confirmers: an error both confirmed is one")
        e = b.job(hh, "drc_team_waives", check=0, show="confirmed", start=0, limit=100, refresh=True)
        by = {x["error"]: x["confirmed_by"] for x in e["rows"]}
        check(by.get(4096) == ["hong", "yoon"] and by.get(3) == ["yoon"] and e["confirmers"] == ["hong", "yoon"],
              "... and the list names both", by)
        # ---- 7. a job that confirms many: the viewer keeps the counts itself
        sel = a.job(hk, "drc_select", check=2, mode="replace")
        was = file_state(cache)
        e = a.job(hk, "drc_review", check=2, action="waive", selection=sel["selection"], on=True)
        check(e["state"] == "done" and e["count"] == sizes[2], "a reviewer waives a rule of 70,000", e)
        people["kim"][0][2] = set(range(sizes[2]))
        deadline = time.monotonic() + 30
        while file_state(cache) == was and time.monotonic() < deadline:
            time.sleep(0.01)
        t, got, exact = agree("... 70,000 waives wait for a confirmation", counted=0)
        was = file_state(cache)
        e = b.job(hh, "drc_team_confirm", check=2, on=True)
        check(e["state"] == "done" and (e["changed"], e["skipped"]) == (sizes[2], 0), "the confirmer confirms the rule", e)
        confirms["hong"][2] = set(range(sizes[2]))
        deadline = time.monotonic() + 30
        while file_state(cache) == was and time.monotonic() < deadline:
            time.sleep(0.01)
        t, got, exact = agree("... and the next answer counts nothing: the confirmer's viewer kept the counts", counted=0)
        check(t["rules"][2]["confirmed"] == sizes[2] and t["rules"][2]["unconfirmed"] == 0, "the rule is confirmed", t["rules"][2])
        e = b.job(hh, "drc_team_confirm", check=2, errors=list(range(100, 300)), on=False)
        confirms["hong"][2] -= set(range(100, 300))
        agree("200 of them taken back", counted=1)
    finally:
        a.close()
        b.close()
    agree("the sessions closed: the same team", counted=0)
    # the change log says the confirm file's time (record 4) when it changed
    log = (d / ".t.db.changes.hong").read_bytes()
    kinds = [struct.unpack_from("<I", log, 48 + 24 * i)[0] for i in range((len(log) - 48) // 24)]
    last = [i for i, k in enumerate(kinds) if k == 4][-1]
    check(4 in kinds and struct.unpack_from("<Q", log, 48 + 24 * last + 8)[0] == os.stat(side).st_mtime_ns
          and 4 not in [struct.unpack_from("<I", (d / ".t.db.changes.kim").read_bytes(), 48 + 24 * i)[0]
                        for i in range(((d / ".t.db.changes.kim").stat().st_size - 48) // 24)],
          "the confirmer's change log says the confirm file's time; a reviewer's has no such record", kinds[-6:])
    # ---- 8. the role is the open's: the same reviewer opened without it
    # confirms nothing, and what they confirmed stands
    s = Svc()
    try:
        h = rs_open(s, db, "hong")["handle"]
        try:
            s("drc_team_confirm", handle=h, check=0, errors=[3], on=False)
            said = None
        except gtkservice.ServiceError as exc:
            said = str(exc)
        check(said is not None and "--confirmer" in said, "the same reviewer without the role: refused", said)
    finally:
        s.close()
    agree("... and their confirmations stand", counted=0)
    # ---- 9. a confirmation nobody said (a file changed by hand): waited
    # for, then counted - at once without a grace
    before, _t, _got = team_now(db)
    raw = bytearray(side.read_bytes())
    raw[40 + starts[0] + 3] = 1
    struct.pack_into("<I", raw, 40 + total, struct.unpack_from("<I", raw, 40 + total)[0] + 1)
    was = os.stat(side).st_mtime_ns
    side.write_bytes(bytes(raw))
    while os.stat(side).st_mtime_ns <= was:
        time.sleep(0.002)
        os.utime(side)
    confirms["hong"][0].add(3)
    nums, t, got = team_now(db)
    check({k: nums[k] for k in TEAM_KEYS} == {k: before[k] for k in TEAM_KEYS} and t["current"] is False
          and t["confirmed"] == _t["confirmed"] and t["waiting"][0].startswith("hong's review files"),
          "a confirm file changed without a record: the counts as they were, said not current", t)
    agree("... counted at once without a grace", env=dict(FLOE_TEAM_GRACE_S="0"))
    # ---- 10. a confirm file of another result is nobody's confirmation
    whole = side.read_bytes()
    other = bytearray(whole)
    other[20] ^= 1
    side.write_bytes(bytes(other))
    nums, t, got = team_now(db, env=dict(FLOE_TEAM_GRACE_S="0"))
    theirs = len(confirms["yoon"][0] & (people["kim"][0][0] | people["lee"][0][0]))
    check(t["confirmed"] == theirs and [(x["reviewer"], Path(x["file"]).name) for x in t.get("skipped", [])]
          == [("hong", ".t.db.confirm.hong")]
          and [(x["reviewer"], Path(x["file"]).name) for x in got["skipped"]] == [("hong", ".t.db.confirm.hong")]
          and "another DRC result" in got["skipped"][0]["reason"] and "confirmed" not in person(got, "hong"),
          "a confirm file of another result: left out and said, the other confirmer's stand", (t.get("confirmed"), t.get("skipped")))
    side.write_bytes(whole)
    agree("... back: counted again", env=dict(FLOE_TEAM_GRACE_S="0"))
    # ---- 11. a reviewer's row alone
    r, one = stats_of(db, "--floe-reviewer", "hong")
    r2, kept = counts_of(db, "--floe-reviewer", "hong", "--rule", "A.W")
    mine = len(confirms["hong"][0]) + len(confirms["hong"][1]) + len(confirms["hong"][2])
    check(one["reviewers"][0]["confirmed"] == mine and kept["reviewers"][0]["confirmed"] == mine
          and kept["reviewers"][0]["rules"][0]["confirmed"] == len(confirms["hong"][0])
          and kept["team"]["rules"][0]["confirmed"] == want()[1][0]["confirmed"],
          "--floe-reviewer: the confirmer's own counts, whole and of a rule", kept["reviewers"][0])


def main():
    global r1, r1_identity, r1_reindex, r2, r3, r4, r5, r6, r8, r9, r10, r11, r12, r13, r14, r15
    r1, r1_identity, r1_reindex, r2, r3, r4, r5, r6, r8, r9, r10, r11, r12, r13, r14, r15 = map(
        timed, (r1, r1_identity, r1_reindex, r2, r3, r4, r5, r6, r8, r9, r10, r11, r12, r13, r14, r15))
    temp = Path(tempfile.mkdtemp(prefix="floe-drc-team-"))
    try:
        d = temp / "runs"
        d.mkdir()
        runs(d)
        svc = Svc()
        try:
            h = r1(svc, d)
            r1_identity(d)
            r1_reindex(d)
            r2(svc, d, h)
            pp, sids = r3(svc, d, h)
            r4(svc, d, h, pp, sids)
        finally:
            svc.close()
        r5(d)
        r6(temp)
        r8(temp)
        r9(temp)
        r10(temp)
        r11(temp)
        r12(temp)
        r13(temp)
        r14(temp)
        r15(temp)
        if "--no-gui" not in sys.argv:
            from validate_drc_team_gui import r7
            timed(r7)(temp, check)
    finally:
        shutil.rmtree(temp, ignore_errors=True)
    if FAILED:
        print("validate_drc_team: %d FAILED: %s" % (len(FAILED), ", ".join(FAILED)))
        sys.exit(1)
    print("validate_drc_team: all green")


if __name__ == "__main__":
    main()
