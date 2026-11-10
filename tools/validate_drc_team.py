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
    for jobs, stream in ((1, False), (4, True)):
        env = dict(os.environ)
        if stream:
            env["FLOE_DRC_QBOX_RESIDENT"] = "0"
        r = run([INDEX, "drc", str(src), "--jobs", str(jobs)], env=env)
        check(r.returncode == 0, "reindex with jobs=%d" % jobs, r.stderr)
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
    r = run([INDEX, "drc", str(src)])
    after = pack.read_bytes()
    check(r.returncode == 0 and after != before and len(after) == len(before)
          and after[:40] == before[:40] and after[-168:-44] == before[-168:-44]
          and after[-8:] == before[-8:],
          "a coordinate change replaces the pack despite matching source stamp", r.stderr)
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


def main():
    global r1, r1_identity, r1_reindex, r2, r3, r4, r5, r6, r8, r9, r10, r11
    r1, r1_identity, r1_reindex, r2, r3, r4, r5, r6, r8, r9, r10, r11 = map(
        timed, (r1, r1_identity, r1_reindex, r2, r3, r4, r5, r6, r8, r9, r10, r11))
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
