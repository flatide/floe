"""DRC analysis gate (feature/jobdeck-drc, ported to Rust 2026-10-11:
app-core drc::analysis behind floe2 gtk-service): the service's answers
equal the Python reference's (tools/oracle/floe_oracle drc_delta,
drc_clusters, drc_points, drc_selection - the branch's own modules), and
the GTK viewer shows them.

  A1  sample.db (8 rules, 38 errors, sample.svrf.rules.json,
      sample.db.clusters): every error's rulers, AREA label and CD pick
      (ticks, ratio, estimated, reason); every rule's delta groups for
      both bases, the automatic and two manual steps, the whole rule and
      each cluster (names, totals, keys, members, steps); clusters'
      counts, pages and ranks under each waive filter after some waives;
      group counts under each filter; marker pictures (RGBA and the
      pixel's error) for views, filters, scopes and a selection; box
      selections for boxes, modes, scopes, filters and a previous one.
  A2  a gen_drcdb pack with a 150k-error rule: its measurement table and
      groups (persistent: published in .<db>.analysis, reused by a new
      open) and its markers = the reference; a re-built pack gets a new
      cache entry.
  A3  clusters files that are wrong are refused naming file and line.
  A4  `floe2 drc-prepare` writes the centres and the delta caches (both
      bases), FLOE_DRC_ANALYSIS_ROOT moves them, an unknown rule and a
      missing sidecar are refused; `floe-index drc-prepare` hands over
      to floe2.
  A5  the viewer (pycairo blocked): the rule tree's cluster children, the
      delta list and its label, the marker picture and its info line,
      the details' CD lines, a cluster child's grid, a whole-view box
      selection, a waive of a selection past the page as a service job
      with the counts after it, Tab hiding the markers' hit map;
      DB descriptions alone supply no CD pick or delta groups, while
      loading SVRF metadata restores those results; a checkmark before
      matched rule names follows real #IFDEF conversion without changing
      the selected scope.

usage: .venv/bin/python tools/validate_drc_analysis.py
"""

import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path[:0] = [str(ROOT / "tools"), str(ROOT / "tools" / "oracle"), str(ROOT)]
os.environ["FLOE_REVIEWER"] = "drc-analysis-gate"
FLOE2 = os.environ.get("FLOE2_BIN") or str(ROOT / "rust/target/release/floe2")
INDEX = str(ROOT / "rust/target/release/floe-index")

from floe import gtkservice  # noqa: E402
from floe_oracle import drc as odrc  # noqa: E402
from floe_oracle.drc_clusters import load_clusters  # noqa: E402
from floe_oracle.drc_delta import (DeltaIndex, MeasurementSelector,  # noqa: E402
                                   area_label, measurement_ticks, percent_ticks)
from floe_oracle.drc_markers import MarkerIndex  # noqa: E402
from floe_oracle.drc_selection import Selection as OSelection, select_rect  # noqa: E402

FAILED = []


def check(ok, what, detail=""):
    if ok:
        print("  %-62s ok" % what)
    else:
        FAILED.append(what)
        print("  %-62s FAIL %s" % (what, str(detail)[:600]))


def run(args, **kw):
    return subprocess.run(args, capture_output=True, text=True, timeout=1800, **kw)


class Svc(object):
    def __init__(self):
        self.svc = gtkservice.Service(FLOE2)

    def __call__(self, op, **fields):
        return self.svc.request(op, **fields)

    def job(self, handle, op, **fields):
        self.svc.drc_watch(handle)
        job = self.svc.request(op, handle=handle, **fields)["job"]
        deadline = time.monotonic() + 600
        while time.monotonic() < deadline:
            for e in self.svc.drc_events(handle):
                if e["job"] != job:
                    gtkservice._discard_files([e])
                    continue
                if e["state"] in ("done", "error", "cancelled"):
                    return e
                gtkservice._discard_files([e])
            time.sleep(0.005)
        raise AssertionError("no answer to %s" % op)

    def close(self):
        self.svc.close()


def markers_of(event):
    """(rgba bytes, ids list) of a markers event (its file removed)."""
    with open(event["path"], "rb") as f:
        data = f.read()
    os.unlink(event["path"])
    w, h = event["width"], event["height"]
    rgba = data[48:48 + w * h * 4]
    ids = list(memoryview(data)[48 + w * h * 4:].cast("q"))
    return rgba, ids, event["visible"], event["occupied"]


def oracle_markers(od, ci, bounds, w, h, members=None, waived=None, selected=None):
    m = MarkerIndex(od).query(bounds, w, h, checks=(ci,),
                              members=None if members is None else {ci: members},
                              waived=waived,
                              selected=None if selected is None else {ci: selected})
    return (m.rgba, [int(v) for v in m.error_ids.ravel()], m.visible_count,
            m.occupied_count)


def sample_dir(temp):
    d = temp / "sample"
    d.mkdir()
    for name in ("sample.oas", "sample.db", "sample.db.clusters", "sample.svrf",
                 "sample.svrf.rules.json"):
        shutil.copy(ROOT / name, d / name)
    r = run([INDEX, "drc", str(d / "sample.db")])
    check(r.returncode == 0, "floe-index drc sample.db", r.stderr)
    return d


def a1(svc, d):
    print("[A1] sample.db: the service = the reference")
    rules = gtkservice.svrf_rules(str(d / "sample.svrf.rules.json"))
    opened = svc("drc_open", path=str(d / "sample.db"), mode="load")
    h = opened["handle"]
    od = odrc.load_db(str(d / "sample.db"))
    check(opened["packed"] and isinstance(od, odrc.IcePack), "both read the pack")
    bad = []
    for ci, c in enumerate(od.checks):
        cons = (rules["checks"].get(c.name) or {}).get("constraints") or []
        sel = MeasurementSelector(cons)
        for ei, e in enumerate(c.errors):
            info = svc("drc_cd_info", handle=h, check=ci, error=ei, constraints=cons)
            p = sel.pick(e)
            want = {"rulers": [list(s) for s in sel.ruler_segments(e)],
                    "area": list(area_label(e, cons)) if area_label(e, cons) else None}
            got = {"rulers": info["rulers"], "area": info["area"]}
            if p is None:
                ok = info["pick"] is None
            else:
                mt = list(measurement_ticks(p))
                ok = (info["pick"]["ticks"] == mt
                      and info["pick"]["ratio"] == percent_ticks(mt[2], mt[1])
                      and info["pick"]["estimated"] == p.estimated
                      and info["pick"]["reason"] == p.reason
                      and info["pick"]["constraint"] == p[0])
            if want != got or not ok:
                bad.append((c.name, ei, info, p))
    check(not bad, "rulers, AREA labels, CD picks of all 38 errors", bad[:2])
    svc("drc_clusters_load", handle=h)
    ocl = load_clusters(str(d / "sample.db.clusters"), od)
    # some waives (the reviewer's sidecar both sides read)
    svc("drc_set_status", handle=h, check=0, errors=[0, 2, 5, 11, 15, 19], status=1)
    svc("drc_set_status", handle=h, check=1, errors=[1], status=1)
    od.close()
    od = odrc.load_db(str(d / "sample.db"))
    ocl = load_clusters(str(d / "sample.db.clusters"), od)
    bad = []
    for k, oc in enumerate(ocl.rules[0]):
        for waived in (None, True, False):
            scope = {"cluster": k}
            if svc("drc_scope_count", handle=h, check=0, scope=scope, waived=waived) \
                    != oc.count(waived):
                bad.append(("count", k, waived))
            if svc("drc_scope_page", handle=h, check=0, scope=scope, start=1, limit=100,
                   waived=waived) != oc.page(1, 100, waived):
                bad.append(("page", k, waived))
            for ei in range(20):
                if svc("drc_scope_rank", handle=h, check=0, scope=scope, error=ei,
                       waived=waived) != oc.rank(ei, waived):
                    bad.append(("rank", k, waived, ei))
    check(not bad, "cluster counts, pages, ranks under each waive filter", bad[:4])
    bad = []
    steps = (None, 1000, 7)
    for ci, c in enumerate(od.checks):
        cons = (rules["checks"].get(c.name) or {}).get("constraints") or []
        if not cons:
            continue
        clusters = [None] + list(range(len(ocl.rules.get(ci, []))))
        for basis in ("absolute", "percent"):
            for step in steps:
                for cl in clusters:
                    og = DeltaIndex(od, ci, cons).group(
                        step, cluster=None if cl is None else ocl.rules[ci][cl], mode=basis)
                    fields = dict(check=ci, constraints=cons, basis=basis, step=step)
                    if cl is not None:
                        fields["cluster"] = cl
                    e = svc.job(h, "drc_delta", **fields)
                    if e["state"] != "done":
                        bad.append(("job", c.name, e))
                        continue
                    rows = svc("drc_delta_rows", handle=h, group=e["group"], start=0, limit=1000)
                    want = [(g.name, g.total, list(g.key), g.page(0, 10 ** 6),
                             g.count(True), g.count(False)) for g in og.page(0, 10 ** 6)]
                    got = []
                    for r in rows:
                        scope = {"group": e["group"], "row": r["row"]}
                        got.append((r["name"], r["total"], r["key"],
                                    svc("drc_scope_page", handle=h, check=ci, scope=scope,
                                        start=0, limit=10 ** 6),
                                    svc("drc_scope_count", handle=h, check=ci, scope=scope, waived=True),
                                    svc("drc_scope_count", handle=h, check=ci, scope=scope, waived=False)))
                    if want != got or e["step"] != og.step_ticks \
                            or e["estimated_total"] != og.estimated_total:
                        bad.append((c.name, basis, step, cl, want[:2], got[:2]))
    check(not bad, "delta groups: rules x bases x steps x clusters (+ filters)", bad[:2])
    # markers
    bad = []
    precision = od.precision
    views = [(0.0, 0.0, 20.0, 20.0, 200, 160), (0.5, 0.5, 2.5, 2.5, 120, 120),
             (-5.0, -3.0, 40.0, 30.0, 333, 211), (1.0, 1.0, 1.1, 1.05, 64, 32)]
    sel_event = svc.job(h, "drc_select", check=0, bounds=[0, 0, 5, 20], mode="replace")
    osel = select_rect(od, 0, (0, 0, 5, 20))
    for (x0, y0, x1, y1, w, hh) in views:
        for waived in (None, True, False):
            for scope, omembers in ((None, None), ({"cluster": 1}, ocl.rules[0][1])):
                for chosen in (None, sel_event["selection"]):
                    fields = dict(check=0, bounds=[x0, y0, x1, y1], width=w, height=hh,
                                  waived=waived)
                    if scope:
                        fields["scope"] = scope
                    if chosen:
                        fields["selection"] = chosen
                    got = markers_of(svc.job(h, "drc_markers", **fields))
                    want = oracle_markers(od, 0, (x0, y0, x1, y1), w, hh, omembers, waived,
                                          osel if chosen else None)
                    if got != want:
                        bad.append(((x0, y0, x1, y1, w, hh), waived, scope, chosen,
                                    got[2:], want[2:]))
    check(not bad, "marker pictures: views x filters x scopes x selection", bad[:2])
    # selections
    bad = []
    previous = None
    oprev = None
    for box, mode, waived, scope, omembers in (
            ((0, 0, 20, 20), "replace", None, None, None),
            ((0, 0, 3, 3), "remove", None, None, None),
            ((1, 1, 6, 12), "toggle", True, None, None),
            ((0, 0, 20, 20), "intersect", None, {"cluster": 3}, ocl.rules[0][3]),
            ((0.99, 0.99, 1.06, 1.02), "add", False, None, None),
            ((7, 7, 5, 5), "replace", None, None, None)):
        fields = dict(check=0, bounds=list(box), mode=mode, waived=waived)
        if scope:
            fields["scope"] = scope
        if previous is not None:
            fields["previous"] = previous
        e = svc.job(h, "drc_select", **fields)
        got = svc("drc_selection_page", handle=h, selection=e["selection"], start=0, limit=10 ** 6)
        oprev = select_rect(od, 0, box, membership=omembers, waived=waived,
                            previous=oprev, mode=mode)
        want = oprev.page(0, 10 ** 6)
        if got != want or e["count"] != len(want):
            bad.append((box, mode, got, want))
        previous = e["selection"]
    check(not bad, "box selections: modes, filters, scope, previous", bad[:2])
    od.close()
    svc("drc_close", handle=h)


def a2(svc, temp):
    print("[A2] a pack with a 150k-error rule: persistent tables and groups")
    d = temp / "big"
    d.mkdir()
    db = d / "big.db"
    r = run([sys.executable, str(ROOT / "tools/gen_drcdb.py"), str(db), "--checks", "12",
             "--max-errors", "400", "--heavy", "150000", "--seed", "7",
             "--svrf", str(d / "big.svrf"), "--pathname", "big.svrf"])
    check(r.returncode == 0, "gen_drcdb", r.stderr)
    r = run([INDEX, "svrf", str(d / "big.svrf")])
    check(r.returncode == 0, "floe-index svrf", r.stderr)
    r = run([INDEX, "drc", str(db)])
    check(r.returncode == 0, "floe-index drc", r.stderr)
    rules = gtkservice.svrf_rules(str(d / "big.svrf.rules.json"))
    od = odrc.load_db(str(db))
    heavy = max(range(len(od.checks)), key=lambda ci: len(od.checks[ci].errors))
    name = od.checks[heavy].name
    cons = rules["checks"][name]["constraints"]
    check(len(od.checks[heavy].errors) == 150000, "the heavy rule has 150000 errors")
    opened = svc("drc_open", path=str(db), mode="load")
    h = opened["handle"]
    t0 = time.monotonic()
    e = svc.job(h, "drc_delta", check=heavy, constraints=cons, basis="absolute")
    cold = time.monotonic() - t0
    og = DeltaIndex(od, heavy, cons).group(None, mode="absolute")
    rows = svc("drc_delta_rows", handle=h, group=e["group"], start=0, limit=1000)
    want = [(g.name, g.total, list(g.key)) for g in og.page(0, 10 ** 6)]
    got = [(r["name"], r["total"], r["key"]) for r in rows]
    check(want == got and e["step"] == og.step_ticks, "the heavy rule's groups = the reference",
          (want[:2], got[:2]))
    first = og.page(0, 10 ** 6)[0]
    check(svc("drc_scope_page", handle=h, check=heavy, scope={"group": e["group"], "row": 0},
              start=100, limit=50) == first.page(100, 50), "a group's member page")
    analysis = Path(opened["analysis"])
    measured = list(analysis.glob("delta/*/measure/complete.json"))
    grouped = list(analysis.glob("delta/*/groups/*/complete.json"))
    check(len(measured) == 1 and len(grouped) == 1, "table and groups published", (measured, grouped))
    svc("drc_close", handle=h)
    opened = svc("drc_open", path=str(db), mode="load")
    h = opened["handle"]
    t0 = time.monotonic()
    e = svc.job(h, "drc_delta", check=heavy, constraints=cons, basis="absolute")
    warm = time.monotonic() - t0
    check(e["state"] == "done" and len(list(analysis.glob("delta/*/measure/complete.json"))) == 1,
          "a new open reuses them (cold %.2f s, warm %.2f s)" % (cold, warm))
    e = svc.job(h, "drc_delta", check=heavy, constraints=cons, basis="percent", step=12345)
    og = DeltaIndex(od, heavy, cons).group(12345, mode="percent")
    rows = svc("drc_delta_rows", handle=h, group=e["group"], start=0, limit=1000)
    check([(r["name"], r["total"]) for r in rows] == [(g.name, g.total) for g in og.page(0, 10 ** 6)],
          "a manual ratio step = the reference")
    b = od._cbb[heavy] / od.precision
    bounds = [float(b[0]), float(b[1]), float(b[2]), float(b[3])]
    got = markers_of(svc.job(h, "drc_markers", check=heavy, bounds=bounds, width=300, height=200))
    want = oracle_markers(od, heavy, bounds, 300, 200)
    check(got == want, "the heavy rule's markers = the reference (%d errors, %d pixels)"
          % (want[2], want[3]), (got[2:], want[2:]))
    check(len(list(analysis.glob("centers-r1-*/complete.json"))) == 1, "its centres published")
    svc("drc_close", handle=h)
    od.close()
    # a re-built pack: a new identity, a new entry
    later = os.stat(str(db)).st_mtime + 10
    os.utime(str(db), (later, later))
    r = run([INDEX, "drc", str(db)])
    check(r.returncode == 0, "the pack re-built", r.stderr)
    opened = svc("drc_open", path=str(db), mode="load")
    h = opened["handle"]
    svc.job(h, "drc_delta", check=heavy, constraints=cons, basis="absolute")
    check(len(list(analysis.glob("delta/*/measure/complete.json"))) == 2,
          "a re-built pack gets its own table")
    svc("drc_close", handle=h)
    return d, db, heavy, name


def a3(svc, temp):
    print("[A3] clusters files that are wrong are refused")
    d = temp / "sample"
    opened = svc("drc_open", path=str(d / "sample.db"), mode="load")
    h = opened["handle"]
    for text, want in (("[NOPE]\n", ":1: unknown rule 'NOPE'"),
                       ("[M1.SPACE.1]\nx = 0-2\n", ":2: range 0-2 outside rule errors 1-20"),
                       ("[M1.SPACE.1]\nx = 1-5\ny = 5\n", ":3: overlapping memberships in 'x' and 'y'"),
                       ("x = 1\n", ":1: cluster needs a [rule name] section"),
                       ("[M1.SPACE.1]\nUnclustered = 1\n", "reserved for unassigned errors")):
        p = temp / "bad.clusters"
        p.write_text(text, encoding="utf-8")
        try:
            svc("drc_clusters_load", handle=h, path=str(p))
            check(False, "refused: %r" % text.strip())
        except gtkservice.ServiceError as exc:
            check(str(p) in str(exc) and want in str(exc), "refused: %s" % want, exc)
    svc("drc_close", handle=h)


def a4(temp, db, heavy, name):
    print("[A4] drc-prepare")
    d = db.parent
    root = temp / "ssd"
    env = dict(os.environ, FLOE_DRC_ANALYSIS_ROOT=str(root))
    r = run([FLOE2, "drc-prepare", str(db), "--svrf", str(d / "big.svrf.rules.json"),
             "--jobs", "2"], env=env)
    check(r.returncode == 0 and "DRC analysis ready" in r.stderr, "floe2 drc-prepare", r.stderr[-800:])
    check(len(list(root.glob("*/centers-r1-*/complete.json"))) == 12
          and len(list(root.glob("*/delta/*/groups/*/complete.json"))) >= 12,
          "centres and both bases' groups under FLOE_DRC_ANALYSIS_ROOT",
          (list(root.glob("*/centers-r1-*")), r.stderr[-400:]))
    r = run([FLOE2, "drc-prepare", str(db), "--svrf", str(d / "big.svrf.rules.json"),
             "--rule", "NOPE"], env=env)
    check(r.returncode != 0 and "unknown DRC rules: NOPE" in r.stderr, "an unknown rule is refused", r.stderr)
    r = run([FLOE2, "drc-prepare", str(db)], env=env)
    check(r.returncode != 0 and "--svrf" in r.stderr, "the CD caches need --svrf", r.stderr)
    r = run([INDEX, "drc-prepare", str(db), "--spatial-only", "--rule", name], env=env)
    check(r.returncode == 0 and "marker centres ready" in r.stderr,
          "floe-index drc-prepare hands over to floe2", r.stdout + r.stderr)


def a5(temp):
    print("[A5] the viewer")
    from validate_gtk_view import NoCairo, gtk_ready, pump
    if not gtk_ready():
        return
    sys.meta_path.insert(0, NoCairo())
    from floe import gui
    gui.import_gtk()
    d = temp / "gui"
    d.mkdir()
    for n in ("sample.oas", "sample.db", "sample.db.clusters", "sample.svrf",
              "sample.svrf.rules.json"):
        shutil.copy(ROOT / n, d / n)
    run([FLOE2, "index", str(d / "sample.oas")])
    run([INDEX, "drc", str(d / "sample.db")])
    cache = gtkservice.ServiceCache(str(d / "sample.oas"))
    cache.load()
    v = gui.Viewer(cache, detail=2)
    try:
        pump(v, lambda: v.last_frame is not None, "first frame")
        check(v.load_drc(str(d / "sample.db")), "load_drc")
        store = v._drcwin._rstore
        name_column = v._drcwin._rules.get_column(0)
        name_column.cell_set_cell_data(store, store.get_iter_first(), False, False)
        check(len(v._drcwin._rules.get_columns()) == 2
              and [r[0] for r in store] == [c.name for c in v._drc.checks]
              and all(r[4] == "✓ " + r[0] and r[0] in r[5] for r in store)
              and name_column.get_cells()[0].get_property("text") == store[0][4]
              and v._drcwin._rules.get_tooltip_column() == 5,
              "auto-loaded SVRF prefixes names with a checkmark and keeps raw names")
        children = [(c[0], c[1]) for c in store[0].iterchildren()]
        check(children == [("클러스터 1", "5/0"), ("클러스터 2", "4/0"),
                           ("클러스터 3", "3/0"), ("Unclustered", "8/0")]
              and all(c[4] == c[0] for r in store for c in r.iterchildren()),
              "the rule tree's cluster children", children)
        v._drcwin._rules.set_cursor(store[0].path, None, False)
        pump(v, lambda: v._drc_delta_groups is not None, "delta groups", 60)
        rows = [(r[0], r[1]) for r in v._drcwin._delta_store]
        check(len(rows) == 6 and rows[0] == ("All errors", "20")
              and rows[1][0].startswith("Group #1 · condition 1 · space < 0.05000: [0.02000, 0.02200) um"),
              "the delta list", rows[:2])
        label = v._drcwin._delta_label.get_text()
        check("5 groups" in label and "auto step 0.00200" in label, "its label", label)
        v._display()
        pump(v, lambda: v._drc_marker_result is not None and not v._drc_marker_busy, "markers", 60)
        m = v._drc_marker_result[1]
        check((m.visible_count, m.total_count) == (20, 20), "the marker picture",
              (m.visible_count, m.total_count))
        check("20 errors / %d markers in view" % m.occupied_count in v._drcwin._info.get_text(),
              "its info line", v._drcwin._info.get_text())
        lines = v._drc_meta_lines(v._drc.checks[0].name, v._drc.checks[0].errors[0])
        check("estimated CD: 0.01001 um vs < 0.05000 · Δ 0.03999 (79.98000%)" in lines
              and "CD selection: SVRF options are not geometrically evaluated" in lines,
              "the details' CD lines", lines)
        # hit map: the pixel of error 0's centre picks it
        v._display()
        pump(v, lambda: v._drc_point_hits is not None, "hit map", 30)
        hits = v._drc_point_hits
        ids = [(x, y) for y in range(hits.height) for x in range(hits.width)
               if hits.error_id(x, y) >= 0]
        x, y = ids[0]
        check(v._drc_hit_at(x, y) is not None, "a marker pixel picks its error")
        v.overlay_mode = 2
        v._display()
        check(v._drc_point_hits is None, "Tab hides the markers' hit map")
        v.overlay_mode = 0
        bb = v.view_bbox()
        v._esel_apply((bb[0], bb[1]), (bb[2], bb[3]))
        pump(v, lambda: v._drc_selection_key is None, "selection", 60)
        check(v._drc_sel is not None and len(v._drc_sel[1]) == 20, "a whole-view box selection",
              v._drc_sel and len(v._drc_sel[1]))
        page = gui.DRC_PAGE
        gui.DRC_PAGE = 5
        try:
            v._drc_set_waived(0, v._drc_sel[1], True)
            check(v._drc_review_job is not None, "a waive past the page is a service job")
            pump(v, lambda: v._drc_review_job is None, "review", 60)
        finally:
            gui.DRC_PAGE = page
        check(v._drc.status_counts(0) == (20, 20) and store[0][1] == "20/20"
              and [c[1] for c in store[0].iterchildren()] == ["5/5", "4/4", "3/3", "8/8"],
              "its counts after it", (v._drc.status_counts(0), store[0][1]))
        child = next(store[0].iterchildren())
        v._drcwin._rules.expand_row(store[0].path, False)
        v._drcwin._rules.set_cursor(child.path, None, False)
        pump(v, lambda: v._drc_cluster is not None and v._drc_delta_groups is not None,
             "cluster groups", 60)
        check(v._drc_grid_map == [2, 4, 10, 15, 16], "a cluster child's grid", v._drc_grid_map)
        conditional = d / "conditional.cal"
        conditional.write_text((d / "sample.svrf").read_text().replace(
            "M1.NOTCH {", "#IFDEF FLOE_GATE_NOTCH\nM1.NOTCH {", 1).replace(
            "VIA1.ENCLOSURE {", "#ENDIF\n\nVIA1.ENCLOSURE {", 1))
        sidecars = {}
        for enabled in (False, True):
            path = d / ("notch-on.rules.json" if enabled else "notch-off.rules.json")
            args = [INDEX, "svrf", str(conditional), "--no-env-switches", "-o", str(path)]
            if enabled:
                args += ["-D", "FLOE_GATE_NOTCH"]
            r = run(args)
            check(r.returncode == 0, "conditional SVRF converted (%s)" % enabled, r.stderr)
            sidecars[enabled] = path
        scope = (v._drc_open, v._drc_cluster)
        selected_path = v._drcwin._rules.get_cursor()[0].to_string()
        counts = [(r[0], r[1], [(c[0], c[1]) for c in r.iterchildren()]) for r in store]
        for enabled in (False, True):
            check(v._drc_rules_load(str(sidecars[enabled])), "reload conditional SVRF (%s)" % enabled)
            pump(v, lambda: v._drc_delta_groups is not None, "retained cluster groups", 60)
            notch = next(r for r in store if r[0] == "M1.NOTCH")
            label, explanation = v._drc_svrf_status("M1.NOTCH")
            check(label == ("✓" if enabled else "")
                  and notch[4] == ("✓ M1.NOTCH" if enabled else "M1.NOTCH")
                  and "M1.NOTCH" in notch[5] and explanation in notch[5]
                  and (enabled or "#IFDEF" in explanation)
                  and all(r[4] == "✓ " + r[0] for r in store if r[0] != "M1.NOTCH")
                  and all(c[4] == c[0] for r in store for c in r.iterchildren()),
                  "rule checkmarks follow #IFDEF and cluster names stay plain", (label, explanation))
            check(v._drcwin._rstore is store and v._drc_open == scope[0]
                  and v._drc_cluster is scope[1]
                  and v._drcwin._rules.get_cursor()[0].to_string() == selected_path
                  and v._drc_grid_map == [2, 4, 10, 15, 16]
                  and counts == [(r[0], r[1], [(c[0], c[1]) for c in r.iterchildren()]) for r in store],
                  "SVRF reload preserves cluster selection, error numbers and counts")
        notch_ci = next(i for i, c in enumerate(v._drc.checks) if c.name == "M1.NOTCH")
        v._drc_select_row(notch_ci)
        pump(v, lambda: v._drc_delta_groups is not None, "conditional notch groups", 60)
        v._drc_goto_cell(notch_ci, 0)
        v._drc_show_detail(notch_ci, 0)
        check(v._drc_rules_load(str(sidecars[False])), "reload with the displayed rule excluded")
        notch = next(r for r in store if r[0] == "M1.NOTCH")
        check(notch[4] == "M1.NOTCH" and v._drc_open == notch_ci
              and v._drc_delta_groups is None and not v._drc_delta_busy
              and v._drc_cd_info(v._drc.checks[notch_ci].errors[0], "M1.NOTCH")["pick"] is None
              and "#IFDEF" in v._drcwin._detail.get_text(*v._drcwin._detail.get_bounds(), True),
              "excluded rule loses CD groups and its current detail explains the missing SVRF")
        empty = json.loads(sidecars[False].read_text())
        empty["checks"]["M1.NOTCH"] = {}
        empty_path = d / "empty-notch.rules.json"
        empty_path.write_text(json.dumps(empty))
        check(v._drc_rules_load(str(empty_path)), "load a matched rule with empty metadata")
        check(next(r for r in store if r[0] == "M1.NOTCH")[4] == "✓ M1.NOTCH"
              and v._drc_svrf_status("M1.NOTCH")[0] == "✓"
              and v._drc_delta_groups is None,
              "SVRF applied status uses rule-name membership, even without constraints")
        check(v._drc_rules_load(str(sidecars[True])), "reload with the displayed rule enabled")
        pump(v, lambda: v._drc_delta_groups is not None, "enabled notch groups", 60)
        check(v._drc_cd_info(v._drc.checks[notch_ci].errors[0], "M1.NOTCH")["pick"]["ticks"]
              == [2000, 5000, 3000]
              and "#IFDEF" not in v._drcwin._detail.get_text(*v._drcwin._detail.get_bounds(), True),
              "enabling the rule restores CD and refreshes its displayed detail")
        no_svrf = temp / "without-svrf"
        no_svrf.mkdir()
        db = no_svrf / "sample.db"
        # Do not let auto-discovery find sample.svrf.rules.json through cwd.
        db.write_text((ROOT / "sample.db").read_text().replace(
            "Rule File Pathname: sample.svrf",
            "Rule File Pathname: " + str(no_svrf / "absent.svrf")))
        r = run([INDEX, "drc", str(db)])
        check(r.returncode == 0, "description-only fixture packed", r.stderr)
        check(v.load_drc(str(db)), "load DB with descriptions and no SVRF")
        ci = next(i for i, c in enumerate(v._drc.checks) if c.name == "M1.NOTCH")
        v._drc_select_row(ci)
        pump(v, lambda: v._drc_grid_ci == ci, "rule without SVRF")
        e = v._drc.checks[ci].errors[0]
        check(v._drc_rmeta is None and v._drc_delta_groups is None
              and not v._drc_delta_busy
              and [(r[0], r[1]) for r in v._drcwin._delta_store] == [("All errors", "3")]
              and v._drc_cd_info(e, "M1.NOTCH")["pick"] is None
              and v._drc_meta_lines("M1.NOTCH", e) == [],
              "GUI does not infer CD or delta groups from a rule description")
        check(len(v._drcwin._rules.get_columns()) == 2
              and all(r[4] == r[0] and r[0] in r[5] for r in v._drcwin._rstore)
              and v._drc_svrf_status("M1.NOTCH") == ("", "SVRF: not loaded"),
              "without metadata rule names have no checkmarks or extra column")
        svc = Svc()
        try:
            h = svc("drc_open", path=str(db), mode="load")["handle"]
            info = svc("drc_cd_info", handle=h, check=ci, error=0)
            event = svc.job(h, "drc_delta", check=ci, basis="absolute")
            check(info["pick"] is None and event["state"] == "done"
                  and event.get("measurable") is False and "group" not in event,
                  "service leaves description-only CD/delta unmeasurable", (info, event))
            svc("drc_close", handle=h)
        finally:
            svc.close()
        check(v._drc_rules_load(str(d / "sample.svrf.rules.json")), "load SVRF for the same DB")
        pump(v, lambda: v._drc_delta_groups is not None, "SVRF groups restored", 60)
        check(v._drc_cd_info(e, "M1.NOTCH")["pick"]["ticks"] == [2000, 5000, 3000]
              and len(v._drcwin._delta_store) > 1
              and all(r[4] == "✓ " + r[0] for r in v._drcwin._rstore),
              "SVRF metadata restores CD measurement and delta groups")
        small = d / "small.db"
        small.write_text("MAIN 100000\nSMALL.ONE\n0 0 0\nSMALL.TWO\n0 0 0\n")
        (d / "small.db.rules.json").write_text(json.dumps({
            "format": "floe-svrf-rules", "version": 1,
            "checks": {"SMALL.ONE": {}, "SMALL.TWO": {}}}))
        r = run([INDEX, "drc", str(small)])
        check(r.returncode == 0, "smaller DB fixture packed", r.stderr)
        check(v.load_drc(str(small)) and len(v._drcwin._rstore) == 2
              and all(r[4] == "✓ " + r[0] and r[0] in r[5] for r in v._drcwin._rstore),
              "switching to a smaller DB safely refreshes auto-loaded SVRF statuses")
        v._drc_release()
        check(v._drc is None and len(v._drcwin._rstore) == 0
              and len(v._drcwin._rules.get_columns()) == 2,
              "releasing the DB clears rule rows and keeps the two-column tree")
    finally:
        v._quit()
        v.window.destroy()
        cache.close()


def main():
    os.environ["FLOE_INDEX_BIN"] = INDEX
    os.environ["FLOE_RENDERD_BIN"] = str(ROOT / "rust/target/release/floe-renderd")
    with tempfile.TemporaryDirectory(prefix="floe-drc-analysis-") as t:
        temp = Path(t)
        svc = Svc()
        try:
            d = sample_dir(temp)
            a1(svc, d)
            _d, db, heavy, name = a2(svc, temp)
            a3(svc, temp)
            a4(temp, db, heavy, name)
        finally:
            svc.close()
        a5(temp)
    if FAILED:
        print("validate_drc_analysis: FAIL (%d): %s" % (len(FAILED), ", ".join(FAILED)))
        sys.exit(1)
    print("validate_drc_analysis: all green")


if __name__ == "__main__":
    main()
