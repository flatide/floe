"""R7 of tools/validate_drc_team.py: the viewer's review flags, the
reviewers window and the review-from-previous-run window over floe2
gtk-service, with pycairo blocked (the hosts have none)."""

import csv
import os
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FLOE2 = os.environ.get("FLOE2_BIN") or str(ROOT / "rust/target/release/floe2")
INDEX = str(ROOT / "rust/target/release/floe-index")


def _run(args):
    return subprocess.run(args, capture_output=True, text=True, timeout=600)


def _peer(d, check):
    """A peer's snapshot of the viewer's pack: visited 0 (not waived, the
    viewer will waive it: a conflict) and 4 (waived: carried safely)."""
    from floe import gtkservice
    svc = gtkservice.Service(FLOE2)
    try:
        h = svc.request("drc_open", path=str(d / "sample.db"), mode="load",
                        reviewer="peer")["handle"]
        svc.drc_watch(h)
        svc.request("drc_set_status", handle=h, check=0, errors=[4], status=1)
        for ei in (0, 4):
            svc.request("drc_visit", handle=h, check=0, error=ei)
        import time
        deadline = time.monotonic() + 30
        seen = 0
        while seen < 2 and time.monotonic() < deadline:
            seen += sum(1 for e in svc.drc_events(h) if e.get("kind") == "visit")
            time.sleep(0.01)
        job = svc.request("drc_team_export", handle=h, path=str(d / "peer.floereview"))["job"]
        while True:
            done = [e for e in svc.drc_events(h) if e.get("job") == job
                    and e["state"] in ("done", "error")]
            if done:
                check(done[0]["state"] == "done", "a peer's snapshot", done[0])
                break
            time.sleep(0.01)
    finally:
        svc.close()
    return d / "peer.floereview"


def _previous(d, check):
    """The previous run: sample.db with error 3 of the first rule moved;
    the viewer's reviewer waived 1 and 3 and visited 1, 2, 3 there."""
    prev = d / "prev"
    prev.mkdir()
    text = (d / "sample.db").read_text().splitlines(keepends=True)
    i = text.index("e 4 2\n")
    text[i + 1] = text[i + 1].replace("190000", "190007", 1)
    (prev / "sample.db").write_text("".join(text))
    r = _run([INDEX, "drc", str(prev / "sample.db")])
    check(r.returncode == 0, "floe-index drc of the previous run", r.stderr)
    from floe import gtkservice
    svc = gtkservice.Service(FLOE2)
    try:
        h = svc.request("drc_open", path=str(prev / "sample.db"), mode="load",
                        reviewer="gui")["handle"]
        svc.request("drc_set_status", handle=h, check=0, errors=[1, 3], status=1)
        for ei in (1, 2, 3):
            svc.request("drc_visit", handle=h, check=0, error=ei)
        svc.request("drc_close", handle=h)
    finally:
        svc.close()
    return prev / "sample.db"


def r7(temp, check):
    print("[R7] the viewer")
    sys.path[:0] = [str(ROOT / "tools"), str(ROOT)]
    from validate_gtk_view import NoCairo, gtk_ready, pump
    if not gtk_ready():
        return
    os.environ["FLOE_REVIEWER"] = "gui"
    os.environ["FLOE_INDEX_BIN"] = INDEX
    os.environ["FLOE_RENDERD_BIN"] = str(ROOT / "rust/target/release/floe-renderd")
    sys.meta_path.insert(0, NoCairo())
    from floe import gui, gtkservice
    gui.import_gtk()
    d = temp / "gui"
    d.mkdir()
    for n in ("sample.oas", "sample.db", "sample.db.clusters", "sample.svrf",
              "sample.svrf.rules.json"):
        shutil.copy(ROOT / n, d / n)
    _run([FLOE2, "index", str(d / "sample.oas")])
    _run([INDEX, "drc", str(d / "sample.db")])
    peer = _peer(d, check)
    previous = _previous(d, check)
    cache = gtkservice.ServiceCache(str(d / "sample.oas"))
    cache.load()
    v = gui.Viewer(cache, detail=2)
    try:
        pump(v, lambda: v.last_frame is not None, "first frame")
        check(v.load_drc(str(d / "sample.db")), "load_drc")
        db = v._drc
        store = v._drcwin._rules.get_model()
        v._drcwin._rules.set_cursor(store[0].path, None, False)
        pump(v, lambda: v._drc_open == 0, "the first rule")
        # a visit only once the error's own geometry is drawn
        v.overlay_mode = 2
        v._drc_jump(0, 3)
        v._display()
        pump(v, lambda: v.last_frame is not None, "a frame")
        for _ in range(20):
            pump(v, lambda: True, "idle")
        check(db.review_counts(0)["visited"] == 0, "a hidden overlay is not a visit")
        v.overlay_mode = 0
        v._display()
        pump(v, lambda: db.review_counts(0)["visited"] == 1, "the visit", 30)
        pump(v, lambda: db.review_flags(0, 3) == gtkservice.VISITED, "the flags", 30)
        check(db.review_flags(0, 3) == gtkservice.VISITED, "showing the overlay draws it: a visit")
        v._drc_jump(0, 0)
        v._display()
        pump(v, lambda: db.review_counts(0)["visited"] == 2, "the visit", 30)
        pump(v, lambda: db.review_flags(0, 0) == gtkservice.VISITED, "the flags", 30)
        v._drc_grid_fill(0)
        cells = [c for row in v._drcwin._gstore for c in row if c]
        check(any(">V 1<" in c or "V 1<" in c for c in cells), "V on the visited error's cell",
              cells[:3])
        stats = v._drcwin._review_info.get_text()
        check("Whole DB: total 38 · visited 2 · not for review 0" in stats
              and "Whole rule" in stats, "the review statistics", stats)
        v._drc_show_detail(0, 0)
        detail = v._drcwin._detail
        text = detail.get_text(detail.get_start_iter(), detail.get_end_iter(), False)
        check("visited: yes · not for review: no" in text, "the details' review line")
        # not for review from the list's menu action
        v._drc_set_not_required(0, [1, 2], True)
        pump(v, lambda: v._drc_review_job is None, "not for review", 30)
        check(db.review_counts(0)["not_required"] == 2
              and db.review_flags(0, 1) == gtkservice.NOT_REQUIRED, "not for review set",
              db.review_counts(0))
        cells = [c for row in v._drcwin._gstore for c in row if c]
        check(any("X 2<" in c for c in cells), "X on its cells", cells[:4])
        out = d / "stats.csv"
        v._drc_review_export_start(str(out))
        pump(v, lambda: v._drc_review_job is None, "statistics export", 30)
        rows = list(csv.reader(open(out, newline=""))) if out.exists() else []
        check(rows and rows[1][:7] == ["database", "", "", "38", "2", "2", "0"]
              and len(rows) == 2 + len(db.checks), "the statistics CSV", rows[:2])
        # the viewer's own waive of 0 (visited): the peer's visit says no
        v._drc_set_waived(0, [0], True)
        # reviewers: load the peer, refresh, the page, a safe application
        w = v._drc_team_dialog()
        w._load_paths([str(peer)])
        pump(v, lambda: w.job is None and w.aggregate is not None
             and len(w.store) > 0, "the reviewers' page", 60)
        rows = [tuple(r) for r in w.store]
        check(any(r[1] == 0 and r[4].startswith("yes / no") for r in rows),
              "the peer's work listed with its V / W / NR", rows[:3])
        check("conflicts 1" in w.stats.get_text(), "one conflict among visited reviewers",
              w.stats.get_text())
        w.mode.set_active_id("conflict")
        pump(v, lambda: w.job is None and len(w.store) == 1, "the conflict page", 30)
        check([r[1] for r in w.store] == [0] and "reviewers" in w.store[0][5],
              "the conflict page", [tuple(r) for r in w.store])
        w._apply()
        pump(v, lambda: w.job is None and "Applied" in w.status.get_text()
             and "Refreshing" not in w.status.get_text(), "the safe application", 60)
        check("preserved 1 conflicting visited errors" in w.status.get_text()
              or "preserved 1 conflicting" in w._notice, "one's visited error kept",
              w.status.get_text())
        check(db.get_status(0, 4) == 1 and db.get_status(0, 0) == 1,
              "the peer's waive carried, one's own decision kept",
              (db.get_status(0, 4), db.get_status(0, 0)))
        # conflict only in the main list
        v._drcwin._conflict.set_active(True)
        pump(v, lambda: v._drc_conflict_job is None and v._drc_conflict_members is not None
             and len(v._drc_conflict_members) == 1, "conflict only", 30)
        v._drc_grid_fill(0)
        check(v._drc_grid_map == [0], "conflict only lists the conflict", v._drc_grid_map)
        v._drcwin._conflict.set_active(False)
        pump(v, lambda: True, "idle")
        # an export and the statistics CSV through the window
        target = d / "mine.floereview"
        w._choose = lambda *a, **k: str(target)
        w._export()
        pump(v, lambda: w.job is None and target.exists(), "an export", 30)
        check(target.exists(), "my review exported")
        csv_path = d / "reviewers.csv"
        w._choose = lambda *a, **k: str(csv_path)
        w._csv()
        pump(v, lambda: w.job is None and csv_path.exists(), "the reviewers CSV", 60)
        rows = list(csv.reader(open(csv_path, newline=""))) if csv_path.exists() else []
        check(rows and rows[0][-1] == "reviewers" and rows[1][-1] == "2",
              "the reviewers CSV", rows[:2])
        w.window.destroy()
        # review from the previous run
        m = v._drc_migration_dialog()
        m.previous.set_text(str(previous))
        m._prepare()
        pump(v, lambda: m.job is None and m.plan is not None and len(m.store) > 0,
             "the geometry comparison", 60)
        text = m.stats.get_text()
        check("Whole new DB: matched review 2" in text, "matched by exact geometry", text)
        rows = [tuple(r) for r in m.store]
        check([r[1] for r in rows] == [1, 2] and rows[0][4] == "yes / yes / no",
              "the matched errors and what carries", rows)
        m._apply()
        pump(v, lambda: m.job is None and m._applied is not None, "the application", 60)
        check(m._applied.get("changed_waived") == 1 and m._applied.get("changed_visited") == 2,
              "the previous review applied", m._applied)
        check(db.get_status(0, 1) == 1 and db.get_status(0, 3) == 0
              and db.review_flags(0, 2) & gtkservice.VISITED,
              "a moved error carries nothing", (db.get_status(0, 1), db.get_status(0, 3)))
        m.window.destroy()
    finally:
        v.window.destroy()
