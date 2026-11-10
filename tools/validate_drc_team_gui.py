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


def _detail(v):
    """The error-detail area's text."""
    buf = v._drcwin._detail
    return buf.get_text(buf.get_start_iter(), buf.get_end_iter(), False)


def _review_grid_hover(v, check, pump):
    """Hover must not let GTK replace the review markup on a whole row.
    Exercise real widget signals without a Python cairo render callback."""
    from floe import gui
    grid = v._drcwin._grid
    model = v._drcwin._gstore
    before = [tuple(row) for row in model]
    members = list(v._drc_grid_map)
    path = gui.Gtk.TreePath.new_from_indices([0])
    area = grid.get_cell_area(path, grid.get_columns()[0])
    device = gui.Gdk.Display.get_default().get_default_seat().get_pointer()

    def event(kind):
        e = gui.Gdk.Event.new(kind)
        e.window = grid.get_bin_window()
        e.send_event = True
        e.x, e.y = area.x + 3, area.y + area.height / 2
        e.set_device(device)
        return e

    reached = []
    handlers = [grid.connect_after(name, lambda *_: reached.append(True) or False)
                for name in ("enter-notify-event", "motion-notify-event")]
    try:
        for name, kind in (("enter-notify-event", gui.Gdk.EventType.ENTER_NOTIFY),
                           ("motion-notify-event", gui.Gdk.EventType.MOTION_NOTIFY)):
            check(grid.emit(name, event(kind)), "review grid consumes " + name)
        check(not reached, "hover stops before GTK's default row prelight")
    finally:
        for handler in handlers:
            grid.disconnect(handler)
    check([tuple(row) for row in model] == before and v._drc_grid_map == members,
          "hover preserves visited underline and excluded gray markup")
    click = event(gui.Gdk.EventType.BUTTON_PRESS)
    click.button, click.state = 1, gui.Gdk.ModifierType(0)
    grid.event(click)
    release = event(gui.Gdk.EventType.BUTTON_RELEASE)
    release.button, release.state = 1, gui.Gdk.ModifierType(0)
    grid.event(release)
    check(v._drc_cell == (0, 0), "click still marks one error cell after hover")
    check(grid.emit("motion-notify-event", event(gui.Gdk.EventType.MOTION_NOTIFY)),
          "hover remains guarded after a click")
    # Hide the intermediate error's geometry so navigation cannot add an
    # unrelated visit to R7's statistics expectations.
    v.overlay_mode = 2
    v._drc_step(1)
    check(v._drc_pos == v._drc_cum[0] + 1, "next-error navigation still works")
    v._drc_step(-1)
    v.overlay_mode = 0
    position, cols = v._lpaned.get_position(), v._drc_gridw
    v._lpaned.set_position(position + 180)
    pump(v, lambda: v._drc_gridw != cols, "review grid reflow")
    cells = [c for row in v._drcwin._gstore for c in row if c]
    check(v._drc_grid_map == members and any("<u>1</u>" in c for c in cells)
          and any(">2</span>" in c and "foreground='#9e9e9e'" in c
                  and "<s>" not in c for c in cells),
          "resize keeps review marks and the same error membership")
    check(grid.emit("motion-notify-event", event(gui.Gdk.EventType.MOTION_NOTIFY)),
          "reflowed grid keeps the hover guard")
    v._lpaned.set_position(position)
    pump(v, lambda: v._drc_gridw == cols, "review grid width restored")


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
        # the panel is plain (user 2026-11-10): nothing above the lists - no
        # info line, no reviewer line, no review statistics with their
        # export button, no conflict state - and the DRC menu exports no
        # statistics
        kinds = [type(c).__name__ for c in v._left_nb.get_nth_page(1).get_children()]
        check(kinds == ["Paned", "Separator", "FlowBox"], "nothing above the DRC lists", kinds)
        # `Conflict only` is not offered yet (user 2026-11-11: hidden until
        # the feature opens); the filter itself is driven below as before
        filters = v._left_nb.get_nth_page(1).get_children()[2]
        labels = [getattr(c.get_child(), "get_label", lambda: None)()
                  for c in filters.get_children()]
        check(v._drcwin._conflict.get_parent() is None and labels[:2] == ["in view", "selected"]
              and "Conflict only" not in labels,
              "the Conflict only check is in no row of the pane", labels)
        check(not any(hasattr(v._drcwin, n) for n in
                      ("_info", "_reviewer_info", "_review_info", "_conflict_info"))
              and not hasattr(v, "_drc_review_export_stats"),
              "the panel keeps no info line and the viewer no statistics export")
        menu = [m for m in v._menubar.get_children() if m.get_label() == "DRC"]
        labels = [i.get_label() or "" for i in menu[0].get_submenu().get_children()] if menu else []
        check("reviewers…" in labels and "open results .db…" in labels
              and not any("statistics" in label for label in labels),
              "the DRC menu has no statistics export", labels)
        # one session a reviewer (2026-11-10): while another process has
        # this reviewer's review of the database open, the viewer does not
        # open it - it says who has it: the status line, a dialog and -
        # the status line is the cursor's - where the error details go
        # while nothing is loaded
        held = gtkservice.Service(FLOE2)
        try:
            held.request("drc_open", path=str(d / "sample.db"), mode="load", reviewer="gui")
            check(not v.load_drc(str(d / "sample.db")) and v._drc is None,
                  "a review open in another session is not opened again")
            said = v.status.get_text()
            dialog = getattr(v, "_drc_refusal_dialog", None)
            check("gui's review of sample.db is already open in another session" in said
                  and "pid %d" % held._proc.pid in said and dialog is not None
                  and _detail(v) == said,
                  "... and the viewer says who has it", (said, _detail(v)))
            if dialog is not None:
                dialog.response(gui.Gtk.ResponseType.CLOSE)
        finally:
            held.close()
        # a review of other DRC results under this one's name (the DRC ran
        # again) is moved aside at the open (0.12.441): the load says it in
        # the status line and names the files where the error details go -
        # there until a rule is picked - not on stderr alone
        (d / ".sample.db.waive.rerun").write_bytes(b"FLOEWAIV" + bytes(40))
        rerun = gtkservice.drc_load(str(d / "sample.db"), reviewer="rerun")
        check(v.load_drc(str(d / "sample.db"), db=rerun) and v._drc is rerun,
              "load_drc of a review found beside another result's files")
        said, kept = v.status.get_text(), _detail(v)
        check(len(rerun.moved_aside) == 1 and "· reviewer rerun" in said
              and "a review of other DRC results moved aside (1 file(s)" in said
              and kept.startswith("a review of other DRC results was here: 1 file(s) moved aside")
              and "this review starts anew" in kept
              and ".sample.db.waive.rerun.stale-" in kept
              and len(list(d.glob(".sample.db.waive.rerun.stale-*"))) == 1,
              "a review of other results moved aside is said by the load", (said, kept))
        check(v.load_drc(str(d / "sample.db")), "load_drc")
        db = v._drc
        # whose review it is: the load's status line says the id the files
        # are kept by (the panel has no line for it); a review that goes on
        # says nothing more
        said = v.status.get_text()
        check(db is not rerun and v._drc_rmatch == (len(db.checks), len(db.checks))
              and said == "DRC sample.db: %d checks, 38 errors (. / , = step) · svrf %d/%d"
              " · reviewer gui" % ((len(db.checks),) + v._drc_rmatch) and _detail(v) == ""
              and v._drc_moved_aside_text(db) is None,
              "the load says whose review it is, and nothing of one that goes on",
              (said, _detail(v)))

        # one that was not stated (--userid, FLOE_REVIEWER) is said to be
        # where the viewer runs; so are a review that cannot be written and
        # a results file read without its index
        class Derived(object):
            _reviewer, _reviewer_source, _reviewer_stated = "etx01", "DISPLAY host", False
            read_only, packed, checks, total, moved_aside = True, True, (), 0, ()
        text = v._drc_loaded_text(Derived(), "/r/results.db")
        check("· reviewer etx01 (the DISPLAY host - not a personal ID; start with --userid ID)"
              in text and "· read-only (no waive file could be made)" in text
              and "ASCII" not in text and "moved aside" not in text,
              "an id that was not stated is said so", text)

        class Plain(object):
            packed, checks, total = False, (), 0
        text = v._drc_loaded_text(Plain(), "/r/results.db", (3, 12))
        check(text == "DRC results.db: 0 checks, 0 errors (. / , = step) · ASCII read, no index"
              " · svrf 3/12" and v._drc_loaded_text(Plain(), "/r/results.db").endswith("no index"),
              "a results file read without its index is said so, and the rules matched", text)
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
        check(any("<u>1</u>" in c for c in cells), "underline on the visited error's number",
              cells[:3])
        whole, rule = db.review_counts(), db.review_counts(0)
        check((whole["total"], whole["visited"], whole["not_required"]) == (38, 2, 0)
              and (rule["visited"], rule["not_required"]) == (2, 0),
              "the review's counts", (whole, rule))
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
        check(any(">2</span>" in c and "foreground='#9e9e9e'" in c
                  and "<s>" not in c for c in cells),
              "gray without strikethrough on excluded numbers", cells[:4])
        _review_grid_hover(v, check, pump)
        whole = db.review_counts()
        check((whole["total"], whole["visited"], whole["not_required"]) == (38, 2, 2),
              "the review's counts with the exclusions", whole)
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
        # The fixture conflict is waived; include both waive states.
        v._drcwin._wf.set_active_id("all")
        v._drcwin._conflict.set_active(True)
        pump(v, lambda: v._drc_conflict_job is None and v._drc_conflict_members is not None
             and len(v._drc_conflict_members) == 1, "conflict only", 30)
        # the filter's state is the status line's (the panel's lines for it
        # are gone, 2026-11-10)
        said = v.status.get_text()
        check(said.startswith("Conflict only: 1 errors in this scope · snapshot "),
              "the conflict filter says its count in the status line", said)
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
