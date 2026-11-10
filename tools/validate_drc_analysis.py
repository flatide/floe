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
      the selected scope. Whole-rule CD group counts are prepared before
      selecting a rule and reused on reopen without full membership caches;
      the grouped-rule filter excludes unmeasurable and empty rules and
      follows the basis and manual step. Grouping off does not prefill.
      Coincident, overlapping, touching and crossing edge pairs measure zero
      unless the SVRF range excludes it; service/group/detail/ruler agree.
      CD-unresolved single edges have exact five-decimal marker-length
      groups without inventing a CD, delta or ratio; pagination, cluster
      scopes, waives and cached CD-only counts remain consistent.

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

    def job(self, handle, op, events=None, **fields):
        self.svc.drc_watch(handle)
        job = self.svc.request(op, handle=handle, **fields)["job"]
        deadline = time.monotonic() + 600
        while time.monotonic() < deadline:
            for e in self.svc.drc_events(handle):
                if e["job"] != job:
                    gtkservice._discard_files([e])
                    continue
                if events is not None:
                    events.append(e)
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
    cached_checks = [dict(check=ci, constraints=rules["checks"][c.name]["constraints"], step=None)
                     for ci, c in enumerate(od.checks)]
    cached = svc.job(h, "drc_delta_cached", checks=cached_checks, basis="absolute")
    check(cached["state"] == "done" and len(cached["rows"]) == len(od.checks)
          and all(r["count"] is None for r in cached["rows"])
          and not list(Path(opened["analysis"]).glob("delta/*/measure/complete.json")),
          "cached group summary leaves unprepared rules unknown without measuring")
    events = []
    prepared = svc.job(h, "drc_delta_cached", checks=cached_checks, basis="absolute",
                       prepare=True, events=events)
    snapshots = [{r["check"]: r["count"] for r in e["rows"] if r["count"] is not None}
                 for e in events if "rows" in e]
    expected = {ci: count for ci, count in enumerate((5, 1, 2, 2, 2, 2, 2, 3))}
    check(prepared["state"] == "done" and snapshots and snapshots[0] == {}
          and snapshots[-1] == expected
          and any(0 < len(s) < len(expected) for s in snapshots)
          and all(a.items() <= b.items() for a, b in zip(snapshots, snapshots[1:]))
          and any(e["state"] == "progress" and any(r["count"] is not None for r in e.get("rows", []))
                  for e in events),
          "cold prepare:true publishes cumulative rule counts before the final done event", snapshots)
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
                            or e["estimated_total"] != og.estimated_total \
                            or e["cd_groups"] != sum(g.key[0] >= 0 for g in og.page(0, 10 ** 6)):
                        bad.append((c.name, basis, step, cl, want[:2], got[:2]))
    check(not bad, "delta groups: rules x bases x steps x clusters (+ filters)", bad[:2])
    # Small live groups stay in memory; the cache-only summary reads completed
    # disk groups, which drc-prepare publishes even below CACHE_THRESHOLD.
    r = run([INDEX, "drc-prepare", str(d / "sample.db"), "--svrf",
             str(d / "sample.svrf.rules.json"), "--delta-only", "--jobs", "2"])
    check(r.returncode == 0, "publish small-rule caches for summary lookup", r.stderr[-800:])
    for basis in ("absolute", "percent"):
        cached = svc.job(h, "drc_delta_cached", checks=cached_checks, basis=basis)
        expected = []
        for item in cached_checks:
            groups = DeltaIndex(od, item["check"], item["constraints"]).group(mode=basis)
            expected.append((item["check"], sum(g.key[0] >= 0 for g in groups.page(0, 1000)),
                             groups.step_ticks))
        check([(r["check"], r["count"], r["step"]) for r in cached["rows"]] == expected,
              "cached whole-rule group counts and resolved steps (%s)" % basis, cached)
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


def analysis_dir(path):
    svc = Svc()
    try:
        opened = svc("drc_open", path=str(path), mode="load")
        svc("drc_close", handle=opened["handle"])
        return Path(opened["analysis"])
    finally:
        svc.close()


def cd_part(v):
    """(controls, list): what of the DRC pane's CD part is in sight."""
    w = v._drcwin
    return w._delta_controls.get_visible(), w._delta_list.get_visible()


def step_dialog(v, text=None):
    """The step button's modal (user 2026-11-10: "step 입력창을 제거하고 set
    step 버튼을 step 으로 변경하고 누르면 step을 입력받는 작은 모달이 뜨도록"):
    the button pressed when the dialog is not up, `text` typed in its entry
    and Enter. What the dialog says in refusal - it stays up then - or None
    when it took the step and closed. Without `text` the dialog's named
    widgets, nothing typed."""
    from floe import gui
    Gtk = gui.Gtk
    if v._sdlg is None:
        v._drcwin._delta_step.clicked()

    def walk(w):
        yield w
        if isinstance(w, Gtk.Container):
            for k in w.get_children():
                for kid in walk(k):
                    yield kid
    named = dict((k.get_name(), k) for k in walk(v._sdlg))
    if text is None:
        return named
    named["drc-step"].set_text(text)
    named["drc-step"].emit("activate")
    return None if v._sdlg is None else named["drc-step-why"].get_text()


def grouped_rules_ui(v, d, pump):
    """Prepared, unmeasurable, unprepared and empty rules in the same tree."""
    from floe import gui
    db = d / "groups.db"
    edge = "e %d 2\n0 0 10000 0\n0 %d 10000 %d\n"
    db.write_text("MAIN 100000\nGROUP.KNOWN\n3 3 0\n"
                  + "".join(edge % (i, gap, gap) for i, gap in enumerate((1000, 2000, 3000), 1))
                  + "GROUP.UNKNOWN\n1 1 0\n" + edge % (1, 1000, 1000)
                  + "GROUP.UNPREPARED\n1 1 0\n" + edge % (1, 1000, 1000)
                  + "GROUP.ZERO\n0 0 0\n")
    space = {"metric": "space", "op": "<", "value": 0.05, "text": "EXTERNAL M1 < 0.05"}
    metadata = d / "groups.db.rules.json"
    metadata.write_text(json.dumps({"format": "floe-svrf-rules", "version": 1, "checks": {
        name: {"constraints": [({"metric": "area", "op": "<", "value": 0.01,
                                 "text": "AREA M1 < 0.01"} if name == "GROUP.UNKNOWN" else space)]}
        for name in ("GROUP.KNOWN", "GROUP.UNKNOWN", "GROUP.UNPREPARED", "GROUP.ZERO")}}))
    (d / "groups.db.clusters").write_text("[GROUP.KNOWN]\nfirst = 1\n")
    r = run([INDEX, "drc", str(db)])
    check(r.returncode == 0, "group-filter fixture packed", r.stderr)
    r = run([INDEX, "drc-prepare", str(db), "--svrf", str(metadata), "--delta-only",
             "--rule", "GROUP.KNOWN", "--rule", "GROUP.UNKNOWN", "--jobs", "2"])
    check(r.returncode == 0, "positive and unmeasurable caches prepared", r.stderr[-800:])
    analysis = analysis_dir(db)
    v._drcwin._delta_mode.set_active_id("off")
    check(v.load_drc(str(db)), "load mixed group-count fixture")
    v._drcwin._wf.set_active_id("all")  # include zero-error rule metadata
    check(v._drc_group_cache_key is None and not v._drc_group_counts
          and len(list(analysis.glob("delta/*/measure/complete.json"))) == 2,
          "loading metadata with grouping off does not prepare missing rules")
    v._drcwin._delta_mode.set_active_id("absolute")
    pump(v, lambda: v._drc_group_cache_key is None, "mixed prepared group counts", 60)
    check([v._drc_group_count(ci) for ci in range(3)] == [3, 0, 1]
          and not v._drc_group_count(3) and v._drc_open is None and not v._drc_delta_busy
          and [r[4] for r in v._drcwin._rstore] == ["✓ (3) GROUP.KNOWN", "✓ GROUP.UNKNOWN",
                                                  "✓ (1) GROUP.UNPREPARED", "✓ GROUP.ZERO"],
          "background preparation fills missing positive counts and omits zero parentheses")
    # A cancelled prefill can leave a cache-miss row from its first progress
    # snapshot. Model that state directly, without depending on worker timing.
    v._drc_group_counts[(2, "absolute", None)] = None
    v._drcwin._delta_mode.set_active_id("off")
    v._drcwin._delta_mode.set_active_id("absolute")
    check(v._drc_group_cache_key is not None and v._drc_open is None,
          "re-enabling grouping retries a pending None count without a rule click")
    pump(v, lambda: v._drc_group_cache_key is None, "resumed pending count", 60)
    check(v._drc_group_count(2) == 1, "resumed prefill replaces the pending count")
    v._drcwin._tf.set_active_id("__grouped__")
    check(v._drcwin._tf.get_active_text() == "CD grouped (2)"
          and [r[0] for r in v._drcwin._rstore] == ["GROUP.KNOWN", "GROUP.UNPREPARED"]
          and all(c[4] == c[0] for c in v._drcwin._rstore[0].iterchildren())
          and v._drc_open is None and not v._drc_delta_busy,
          "CD grouped filter includes the newly prepared rule before any rule selection")
    # Complete another rule while reviewing a later page inside a cluster.
    # Inject the count transition so this cannot race the tiny fixture's job.
    v._drc_group_counts[(2, "absolute", None)] = None
    v._drc_group_ui_refresh()
    page = gui.DRC_PAGE
    gui.DRC_PAGE = 1
    try:
        store = v._drcwin._rstore
        parent_ref = gui.Gtk.TreeRowReference.new(store, store[0].path)
        cluster = next(c[3] for c in store[0].iterchildren() if c[0] == "Unclustered")
        v._drc_select_row(0, cluster)
        pump(v, lambda: v._drc_delta_groups is not None, "cluster before count arrival", 60)
        v._drc_goto_cell(0, 2)
        v._drc_sel_click(0, 0, 0, 0, 2, False)
        pump(v, lambda: v._drc_selection_key is None, "error selection before count arrival", 60)
        v._drc_jump(0, 2)
        selected, mark, rulers, grid = v._drc_sel, v.drc_mark, v._drc_ruler, v._drc_grid_map
        cursor = (v._drc_open, v._drc_page, v._drc_pos, v._drc_cell, v._drc_focus)
        v._drc_group_counts[(2, "absolute", None)] = 1
        v._drc_group_ui_refresh()
        check(parent_ref.valid() and [r[2] for r in store] == [0, 2]
              and v._drc_cluster is cluster and selected is not None and mark is not None and rulers
              and v._drc_sel is selected and v.drc_mark is mark and v._drc_ruler is rulers
              and v._drc_grid_map is grid and list(grid) == [2] and v._drc_page == 1
              and (v._drc_open, v._drc_page, v._drc_pos, v._drc_cell, v._drc_focus) == cursor,
              "a newly grouped rule is inserted without resetting the current error, cluster or page")
    finally:
        gui.DRC_PAGE = page
    v._drc_select_row(0)
    pump(v, lambda: v._drc_delta_groups is not None, "known whole-rule groups", 60)
    refused = step_dialog(v, "0.05000")
    check(refused is None and v._drc_group_count(0) is None and v._drc_open == 0,
          "a new uncached step drops the old count and keeps the active grouped rule", refused)
    pump(v, lambda: not v._drc_delta_busy and v._drc_group_cache_key is None,
         "manual absolute group count", 60)
    check(v._drc_group_count(0) == 1 and v._drcwin._rstore[0][4] == "✓ (1) GROUP.KNOWN",
          "completed whole-rule grouping updates the count for the manual step")
    v._drcwin._delta_mode.set_active_id("percent")
    pump(v, lambda: not v._drc_delta_busy and v._drc_group_cache_key is None,
         "ratio group count", 60)
    check(v._drc_group_count(0) == 3 and v._drcwin._rstore[0][4] == "✓ (3) GROUP.KNOWN"
          and v._drc_delta_steps.get((0, "absolute")) == 5000,
          "changing basis uses its own group count and preserves the other basis's step")
    v._drcwin._delta_mode.set_active_id("absolute")
    pump(v, lambda: not v._drc_delta_busy and v._drc_group_cache_key is None,
         "restored manual absolute count", 60)
    check(v._drc_group_count(0) == 1, "returning to the prior basis restores its manual-step count")
    v._drcwin._delta_mode.set_active_id("off")
    check(v._drc_tfilter == "all" and len(v._drcwin._rstore) == 4
          and all(r[4] == "✓ " + r[0] for r in v._drcwin._rstore)
          and all(row[1] != "__grouped__" for row in v._drcwin._tf.get_model()),
          "grouping off removes count prefixes and the grouped-only filter")
    v._drcwin._delta_mode.set_active_id("absolute")
    pump(v, lambda: not v._drc_delta_busy and v._drc_group_cache_key is None,
         "grouping re-enabled", 60)
    check(v._drc_rules_load(str(metadata)), "reload group-count metadata")
    pump(v, lambda: not v._drc_delta_busy and v._drc_group_cache_key is None,
         "counts after metadata reload", 60)
    check(v._drc_delta_steps == {} and v._drc_group_count(0) == 3
          and v._drcwin._rstore[0][4] == "✓ (3) GROUP.KNOWN",
          "metadata reload clears manual settings and restores automatic cached counts")
    v._drc_select_row(1)
    pump(v, lambda: v._drc_delta_groups is not None, "unmeasurable-only groups", 60)
    check(len(v._drc_delta_groups) == 1 and v._drc_delta_groups[0].key[0] < 0
          and v._drc_group_count(1) == 0 and v._drcwin._rstore[1][4] == "✓ GROUP.UNKNOWN"
          and cd_part(v) == (False, False),
          "a completed Unmeasurable-only group does not count as a CD group")


def zero_gap_ui(v, d, pump):
    """Zero is a measurable distance, distinct from unknown and one DBU."""
    from floe import gui
    db, deck = d / "zero-gap.db", d / "zero-gap.svrf"
    metadata = d / "zero-gap.db.rules.json"
    first = (100000, 100000, 110000, 100000)
    seconds = [first, (110000, 100000, 100000, 100000),
               (105000, 100000, 115000, 100000), (110000, 100000, 110000, 105000),
               (105000, 95000, 105000, 105000), (100000, 100001, 110000, 100001)]
    records = "".join("e %d 2\n%s\n%s\n" % (
        i, " ".join(map(str, first)), " ".join(map(str, second)))
        for i, second in enumerate(seconds, 1))
    names = ("ZERO.SPACE", "ZERO.STRICT")
    db.write_text("SAMPLE 100000\n" + "".join(name + "\n6 6 0\n" + records for name in names))
    deck.write_text("PRECISION 100000\nLAYER M1 1\n"
                    "ZERO.SPACE {\n EXTERNAL M1 < 0.02750\n}\n"
                    "ZERO.STRICT {\n EXTERNAL M1 > 0 < 0.02750\n}\n")
    for args in ([INDEX, "drc", str(db)],
                 [INDEX, "svrf", str(deck), "--no-env-switches", "-o", str(metadata)]):
        r = run(args)
        check(r.returncode == 0, "zero-gap fixture indexed/converted", r.stderr)
    rules = json.loads(metadata.read_text())["checks"]
    constraints = [rules[name]["constraints"] for name in names]
    svc = Svc()
    try:
        opened = svc("drc_open", path=str(db), mode="load")
        h = opened["handle"]
        info = [svc("drc_cd_info", handle=h, check=0, error=ei, constraints=constraints[0])
                for ei in range(6)]
        check(all(row["pick"] is not None and row["pick"]["ticks"] == [0, 2750, 2750]
                  and row["pick"]["ratio"] == 10000000
                  and any(s[:2] == s[2:] for s in row["rulers"]) for row in info[:5]),
              "identical/reversed/overlapping/touching/crossing edges have CD 0, delta .02750, ratio 100%",
              info[:5])
        check(info[5]["pick"] is not None and info[5]["pick"]["ticks"] == [1, 2750, 2749]
              and info[5]["pick"]["ratio"] == 9996364,
              "a nearby .00001 um gap stays positive", info[5])
        strict = [svc("drc_cd_info", handle=h, check=1, error=ei, constraints=constraints[1])
                  for ei in range(6)]
        check(all(row["pick"] is None for row in strict[:5])
              and strict[5]["pick"] is not None and strict[5]["pick"]["ticks"] == [1, 2750, 2749],
              "the combined > 0 and < .02750 range excludes zero but includes one DBU", strict)
        checks = [dict(check=ci, constraints=cons, step=None) for ci, cons in enumerate(constraints)]
        for basis in ("absolute", "percent"):
            prepared = svc.job(h, "drc_delta_cached", checks=checks, basis=basis, prepare=True)
            cached = svc.job(h, "drc_delta_cached", checks=checks, basis=basis)
            check([r["count"] for r in prepared["rows"]] == [2, 1]
                  and cached["rows"] == prepared["rows"],
                  "zero-gap prefill and warm summary counts agree (%s)" % basis, (prepared, cached))
            for ci, cons in enumerate(constraints):
                event = svc.job(h, "drc_delta", check=ci, constraints=cons, basis=basis)
                rows = svc("drc_delta_rows", handle=h, group=event["group"], start=0, limit=100)
                unknown = [r for r in rows if r["key"][0] < 0]
                members = [svc("drc_scope_page", handle=h, check=ci,
                               scope={"group": event["group"], "row": r["row"]}, start=0, limit=10)
                           for r in rows]
                check(event["cd_groups"] == (2 if ci == 0 else 1)
                      and sorted(members) == [[0, 1, 2, 3, 4], [5]]
                      and (not unknown if ci == 0 else len(unknown) == 1 and unknown[0]["total"] == 5),
                      "zero-gap live groups honor the SVRF range (%s, %s)" % (names[ci], basis), rows)
        svc("drc_close", handle=h)
    finally:
        svc.close()
    check(v.load_drc(str(db)), "load zero-gap DB with SVRF")
    pump(v, lambda: v._drc_group_cache_key is None, "zero-gap cached counts", 60)
    check([v._drc_group_count(ci) for ci in range(2)] == [2, 1] and v._drc_open is None,
          "zero-gap cached counts populate the GUI before selection")
    v._drc_select_row(0)
    pump(v, lambda: v._drc_delta_groups is not None, "zero-gap GUI groups", 60)
    e = v._drc.checks[0].errors[0]
    lines = v._drc_meta_lines("ZERO.SPACE", e)
    v._drc_goto_cell(0, 0)
    v._drc_jump(0, 0)
    check(any("0.00000 um vs < 0.02750 · Δ 0.02750 (100.00000%)" in line for line in lines)
          and any(isinstance(r, gui._DrcAreaLabel) and r.text == "0.00000 um" for r in v._drc_ruler),
          "GUI detail and the point ruler display zero CD with the full delta",
          (lines, [getattr(r, "text", None) for r in v._drc_ruler]))
    for basis in ("percent", "absolute"):
        v._drcwin._delta_mode.set_active_id(basis)
        pump(v, lambda: not v._drc_delta_busy and v._drc_group_cache_key is None,
             "zero-gap GUI %s groups" % basis, 60)
        check(v._drc_group_count(0) == 2 and v._drc_delta_groups.cd_count == 2
              and all(g.key[0] >= 0 for g in v._drc_delta_groups.page(0, 10)),
              "GUI zero distances stay in measurable groups (%s)" % basis)


def marker_length_groups(svc, temp):
    """Two user-visible length families stay separate without a width CD."""
    print("[A4b] CD-unresolved single-edge length groups")
    d = temp / "marker-length"
    d.mkdir()
    db, deck, metadata = (d / "length.db", d / "length.cal", d / "length.db.rules.json")
    records = []
    for i, length in enumerate([23750] * 1001 + [25750] * 2 + [23751], 1):
        y = 151707250 + i
        a, b = (42567350, y), (42567350 + length, y)
        if i % 2 == 0:
            a, b = b, a
        records.append("e %d 1\n%d %d %d %d\n" % (i, *a, *b))
    records += ["e 1005 2\n0 0 23750 0\n0 7900 23750 7900\n",
                "p 1006 3\n0 0\n1000 0\n0 1000\n"]
    db.write_text("MAIN 100000\nEDGE.UNKNOWN\n1006 1006 0\n" + "".join(records))
    deck.write_text("PRECISION 100000\nLAYER dGRM1.xx 1\nLAYER d_gen_pedge 2\n"
                    "EDGE.UNKNOWN {\n ANGLE dGRM1.xx == 0\n"
                    " INT (d_gen_pedge) == 0.079 ABUT < 90 OPPOSITE SYMMETRIC\n}\n")
    (d / "length.db.clusters").write_text("[EDGE.UNKNOWN]\nlast = 1000-1004\n")
    for args in ([INDEX, "drc", str(db)],
                 [INDEX, "svrf", str(deck), "--no-env-switches", "-o", str(metadata)]):
        r = run(args)
        check(r.returncode == 0, "single-edge fixture indexed/converted", r.stderr)
    cons = json.loads(metadata.read_text())["checks"]["EDGE.UNKNOWN"]["constraints"]
    check([(c["metric"], c["op"], c["value"]) for c in cons]
          == [("angle", "==", 0), ("width", "==", 0.079)],
          "ABUT angle is not mistaken for a second CD constraint", cons)
    opened = svc("drc_open", path=str(db), mode="load")
    h = opened["handle"]
    svc("drc_clusters_load", handle=h)
    info = [svc("drc_cd_info", handle=h, check=0, error=ei, constraints=cons)
            for ei in (0, 1001, 1003)]
    check(all(i["pick"] is None for i in info)
          and [i["marker_length_ticks"] for i in info] == [23750, 25750, 23751],
          "marker lengths are available while width CD stays unresolved", info)
    checks = [dict(check=0, constraints=cons, step=None)]
    for basis in ("absolute", "percent"):
        prepared = svc.job(h, "drc_delta_cached", checks=checks, basis=basis, prepare=True)
        check(prepared["state"] == "done" and prepared["rows"][0]["count"] == 1,
              "background count excludes marker-length groups (%s)" % basis, prepared)
        for step in (None, 100000):
            event = svc.job(h, "drc_delta", check=0, constraints=cons, basis=basis, step=step)
            rows = svc("drc_delta_rows", handle=h, group=event["group"], start=0, limit=50)
            geometry = [r for r in rows if r["key"][0] == -2]
            check(event["cd_groups"] == 1 and event["geometry_groups"] == 3
                  and [(r["key"][1], r["total"]) for r in geometry]
                  == [(23750, 1001), (23751, 1), (25750, 2)]
                  and [r["label"] for r in geometry] == ["C? #1", "C? #2", "C? #3"]
                  and all("CD unresolved" in r["name"] and "edge length" in r["name"]
                          and "um" in r["name"] and "%" not in r["name"] for r in geometry)
                  and rows[-1]["key"] == [-1, 0] and rows[-1]["total"] == 1,
                  "exact lengths survive basis/step changes (%s, %s)" % (basis, step), rows)
            first = geometry[0]
            scope = {"group": event["group"], "row": first["row"]}
            check(svc("drc_scope_page", handle=h, check=0, scope=scope, start=0, limit=1000)
                  == list(range(1000))
                  and svc("drc_scope_page", handle=h, check=0, scope=scope, start=1000, limit=1000)
                  == [1000]
                  and svc("drc_delta_find", handle=h, group=event["group"], key=first["key"])
                  == first["row"],
                  "1,000-error pages and stable negative-key lookup (%s, %s)" % (basis, step))
        svc("drc_set_status", handle=h, check=0, errors=[0, 1000, 1001], status=1)
        check(svc("drc_scope_count", handle=h, check=0, scope=scope, waived=True) == 2
              and svc("drc_scope_page", handle=h, check=0, scope=scope,
                      start=0, limit=1000, waived=False) == list(range(1, 1000))
              and svc("drc_scope_rank", handle=h, check=0, scope=scope, error=1000) == 1000,
              "length-group waive counts, filtered pages and ranks (%s)" % basis)
        event = svc.job(h, "drc_delta", check=0, constraints=cons, basis=basis, cluster=0)
        rows = svc("drc_delta_rows", handle=h, group=event["group"], start=0, limit=50)
        check(event["total"] == 5 and event["cd_groups"] == 0 and event["geometry_groups"] == 3
              and [(r["key"][1], r["total"]) for r in rows] == [(23750, 2), (23751, 1), (25750, 2)],
              "cluster intersection keeps only its exact length members (%s)" % basis, rows)
        cached = svc.job(h, "drc_delta_cached", checks=checks, basis=basis)
        check(cached["rows"] == prepared["rows"], "warm summary keeps CD-only count (%s)" % basis)
    svc("drc_close", handle=h)


def explicit_rules_ui(v, d, pump):
    folder = d / "explicit rules"
    folder.mkdir()
    rules = folder / "deck.cal.rules.json"
    original = json.loads((d / "sample.svrf.rules.json").read_text())
    rules.write_text(json.dumps({"format": "floe-svrf-rules", "version": 1,
        "checks": {"M1.NOTCH": original["checks"]["M1.NOTCH"]}}))
    db = d / "sample.db"
    v._drc_open_db(str(db), rules=str(rules))
    check(v._drc_rmatch == (1, 8) and set(v._drc_rmeta["checks"]) == {"M1.NOTCH"}
          and not (folder / "deck.cal").exists(),
          "explicit rules JSON overrides automatic metadata without a source deck")
    ci = next(i for i, c in enumerate(v._drc.checks) if c.name == "M1.NOTCH")
    v._drc_select_row(ci)
    pump(v, lambda: v._drc_delta_groups is not None, "explicit rules CD groups", 60)
    check(v._drc_cd_info(v._drc.checks[ci].errors[0], "M1.NOTCH")["pick"]["ticks"]
          == [2000, 5000, 3000], "explicit rules supply the measured CD constraints")
    previous_db, previous_meta = v._drc, v._drc_rmeta
    raw = folder / "raw.cal"
    raw.write_text("M1.NOTCH { INT M1 < 0.03 }\n")
    check(not v.load_drc(str(db), rules=str(raw)) and v._drc is previous_db
          and v._drc_rmeta is previous_meta
          and v.status.get_text().startswith("rules metadata load failed:")
          and not (folder / "raw.cal.rules.json").exists(),
          "raw SVRF is refused without indexing or replacing the current review")
    check(v.load_drc(str(db)) and v._drc_rmatch == (8, 8),
          "omitting explicit rules keeps automatic JSON discovery")


def marker_length_ui(v, temp, pump):
    db = temp / "marker-length" / "length.db"
    v._drcwin._delta_mode.set_active_id("absolute")
    check(v.load_drc(str(db)), "load single-edge length groups in the viewer")
    v._drcwin._wf.set_active_id("all")  # include waived length-group members
    v._drc_select_row(0)
    pump(v, lambda: v._drc_delta_groups is not None and v._drc_group_cache_key is None,
         "single-edge groups", 60)
    groups = v._drc_delta_groups
    first = next(g for g in groups.page(0, 50) if g.key == (-2, 23750))
    check(groups.cd_count == 1 and groups.geometry_count == 3
          and "3 edge-length groups" in v._drcwin._delta_store[0][3],
          "viewer distinguishes marker-length groups from CD groups")
    lines = v._drc_meta_lines("EDGE.UNKNOWN", v._drc.checks[0].errors[0])
    check(any("CD unavailable" in line for line in lines)
          and "Marker edge length: 0.23750 um (CD unresolved)" in lines
          and not any("Δ" in line for line in lines),
          "detail displays marker length without a false CD delta", lines)
    v._drc_delta_choose(first)
    pump(v, lambda: len(v._drc_grid_map) == 1000, "first length-group page", 60)
    check(v._drc_grid_map == list(range(1000)), "length group shows its first 1,000 errors")
    v._drc_page_step(1)
    pump(v, lambda: v._drc_grid_map == [1000], "second length-group page", 60)
    check(v._drc_page == 1, "length group has a final one-error page")
    v._drcwin._delta_mode.set_active_id("percent")
    pump(v, lambda: not v._drc_delta_busy and v._drc_group_cache_key is None,
         "length-group ratio switch", 60)
    check(v._drc_delta_group is not None and v._drc_delta_group.key == (-2, 23750)
          and v._drc_delta_group.total == 1001,
          "switching delta basis preserves the selected marker-length group")
    # edge-length groups are groups to pick: a cluster with nothing else
    # keeps the pane's CD part
    cluster = next(c[3] for c in v._drcwin._rstore[0].iterchildren() if c[0] == "last")
    v._drc_select_row(0, cluster)
    pump(v, lambda: v._drc_delta_groups is not None and not v._drc_delta_busy,
         "length cluster groups", 60)
    check(v._drc_delta_groups.cd_count == 0 and v._drc_delta_groups.geometry_count == 3
          and cd_part(v) == (True, True),
          "a cluster with edge-length groups alone keeps the CD part",
          (v._drc_delta_groups.cd_count, v._drc_delta_groups.geometry_count, cd_part(v)))
    # the group menu's `waive all` on a scope past a page of 1,000: the
    # service's review job on the scope's selection, any filter
    def done():
        return (v._drc_selection_key is None and v._drc_review_job is None
                and not v._drc_delta_busy and v._drc_group_cache_key is None)
    v._drc_select_row(0)
    pump(v, lambda: v._drc_cluster is None and done(), "the whole length rule", 60)
    waived = v._drc.status_counts(0)[0]
    v._drc_scope_all("waive", True)
    job = v._drc_selection_then
    pump(v, done, "waive all of 1,006 errors", 60)
    said = v.status.get_text()
    all_waived = v._drc.status_counts(0)[0]
    v._drc_scope_all("waive", False)
    pump(v, done, "unwaive all of 1,006 errors", 60)
    check(job == ("waive", True) and all_waived == 1006 and "1006" in said
          and v._drc.status_counts(0)[0] == 0 and v._drc_sel is None and 0 < waived < 1006,
          "waive all / unwaive all of a rule past a page are the service's review job",
          (job, waived, all_waived, said, v._drc.status_counts(0)[0]))


def pane_ui(v, d, pump):
    """The DRC pane's lower half and its lists (user 2026-11-11, six calls:
    the filters on one line with `Conflict only` out of sight; a menu on
    the group rows - select all / deselect all / waive all / unwaive all /
    mark not for review all / clear not for review all; the group pager
    only for a second page, low, `1/3`; the error pager under its list,
    low; the group list's and the error list's heights by a divider; the
    SVRF lines of an error out of the detail pane, in a window)."""
    from floe import gui
    Gtk, Gdk = gui.Gtk, gui.Gdk
    db = d / "pane.db"
    pair = "e %d 2\n0 0 10000 0\n0 %d 10000 %d\n"
    db.write_text(
        "MAIN 100000\nPANE.MANY\n120 120 0\n"
        + "".join(pair % (i, 1000 + i, 1000 + i) for i in range(1, 121))
        + "PANE.FEW\n4 4 0\n"
        + "".join(pair % (i, gap, gap) for i, gap in enumerate((1000, 1000, 2000, 3000), 1))
        + "PANE.NOMETA\n1 1 0\n" + pair % (1, 1000, 1000))
    space = {"metric": "space", "op": "<", "value": 0.05, "text": "EXTERNAL M1_drawn < 0.05"}
    (d / "pane.db.rules.json").write_text(json.dumps({
        "format": "floe-svrf-rules", "version": 1,
        "checks": {"PANE.MANY": {"constraints": [space]},
                   "PANE.FEW": {"constraints": [space], "layers": ["M1_drawn"],
                                "source_gds": [[1, 0]]}},
        "derived": {"M1_drawn": "M1 NOT FILL"}}))
    (d / "pane.db.clusters").write_text("[PANE.FEW]\nfirst = 1-2\n")
    r = run([INDEX, "drc", str(db)])
    check(r.returncode == 0, "pane fixture packed", r.stderr)
    w = v._drcwin
    w._delta_mode.set_active_id("absolute")
    check(v.load_drc(str(db)), "load the pane fixture")

    def done():
        return (v._drc_selection_key is None and v._drc_review_job is None
                and not v._drc_delta_busy and v._drc_group_cache_key is None)

    def settle(what):
        pump(v, done, what, 60)

    def pick(ci, cluster=None):
        v._drc_select_row(ci, cluster)
        pump(v, lambda: v._drc_open == ci and v._drc_cluster is cluster and done(),
             "pane: %s" % v._drc.checks[ci].name, 60)

    def detail():
        buf = w._detail
        return buf.get_text(buf.get_start_iter(), buf.get_end_iter(), False)

    settle("pane counts")
    check(v._drc_wfilter == "notwaived" and [v._drc_group_count(ci) for ci in range(3)][2] is None,
          "the pane fixture opens under Not Waived")

    # ---- 1. the filters: one line, `Conflict only` in no row
    nav = w._hl.get_parent().get_parent()
    kids = [c.get_child() for c in nav.get_children()]
    check(kids == [w._hl, w._selv, w._tf, w._wf] and isinstance(nav, Gtk.FlowBox)
          and nav.get_halign() == Gtk.Align.START
          and w._conflict.get_parent() is None and not w._conflict.get_mapped()
          and (w._hl.get_label(), w._selv.get_label()) == ("in view", "selected"),
          "the filter row holds in view, selected and the two combos; Conflict only is in no row",
          [type(k).__name__ for k in kids])
    pump(v, lambda: w._wf.get_allocation().width > 1, "the filter row laid out", 10)
    cells = [k.get_parent().get_allocation() for k in kids]
    need = nav.get_preferred_width()[1]
    room = nav.get_parent().get_allocated_width()
    check(room < need
          or (len(set(c.y for c in cells)) == 1 and cells[0].x == 0
              and all(b.x == a.x + a.width + 6 for a, b in zip(cells, cells[1:]))
              and [c.width for c in cells] == [k.get_preferred_width()[1] for k in kids]
              and nav.get_allocated_width() == need),
          "the four stand on one line from the left, at their own widths, 6 px apart",
          ([(c.x, c.y, c.width) for c in cells], need, room))
    check(nav.get_preferred_width()[0] < gui.LEFT_PANE_PX,
          "(narrower than the four it wraps: it asks the left pane for less than its start width)",
          tuple(nav.get_preferred_width()))

    # ---- 3-5. the pagers and the divider
    pick(1)
    plain = Gtk.Button(label="◀")
    plain.show()
    tall = plain.get_preferred_height()[1]
    ebox = w._pprev.get_parent().get_parent()
    order = [type(k).__name__ for k in ebox.get_children()]
    pump(v, lambda: w._pprev.get_allocation().height > 1, "the error pager laid out", 10)
    check(order == ["ScrolledWindow", "Box"] and ebox.get_children()[0].get_child() is w._grid
          and ebox.get_children()[1] is w._pprev.get_parent()
          and w._plabel.get_text() == "1/1"
          and w._pprev.get_allocated_height() < tall and w._pnext.get_allocated_height() < tall,
          "the error pager is under its grid, its arrows lower than a plain button, `1/1`",
          (order, w._plabel.get_text(), w._pprev.get_allocated_height(), tall))
    split = w._vsplit
    check(isinstance(split, Gtk.Paned) and split.get_orientation() == Gtk.Orientation.VERTICAL
          and split.get_child1() is w._delta_list and split.get_child2() is ebox
          and split.get_position() == gui.DRC_GROUPS_PX
          and w._delta_list.get_allocated_height() == gui.DRC_GROUPS_PX,
          "the group list stands over the error grid in a divider, %d px at first"
          % gui.DRC_GROUPS_PX, (split.get_position(), w._delta_list.get_allocated_height()))
    whole = split.get_allocated_height()
    split.set_position(180)
    pump(v, lambda: w._delta_list.get_allocated_height() == 180, "the divider moved", 10)
    grown = (w._delta_list.get_allocated_height(), ebox.get_allocated_height())
    split.set_position(5)
    pump(v, lambda: w._delta_list.get_allocated_height() < 100, "the divider moved up", 10)
    least = w._delta_list.get_allocated_height()
    split.set_position(gui.DRC_GROUPS_PX)
    pump(v, lambda: w._delta_list.get_allocated_height() == gui.DRC_GROUPS_PX, "the divider back", 10)
    check(grown[0] == 180 and 0 < grown[1] < whole - 180 and 30 < least < gui.DRC_GROUPS_PX,
          "the divider gives the group list more or less of the height, never under two rows",
          (grown, least, whole))
    pick(2)
    pump(v, lambda: ebox.get_allocated_height() == split.get_allocated_height(),
         "the grid alone", 10)
    check(cd_part(v) == (False, False)
          and ebox.get_allocated_height() == split.get_allocated_height(),
          "a rule without CD groups: the error grid has the divider's whole height")
    pick(1)
    check(cd_part(v) == (True, True) and split.get_position() == gui.DRC_GROUPS_PX
          and not w._delta_bar.get_visible() and w._delta_label.get_text() == "1/1"
          and len(v._drc_delta_groups) == 3,
          "three groups: no group pager", (len(v._drc_delta_groups), w._delta_label.get_text()))
    pick(0)
    check(not w._delta_bar.get_visible() and len(v._drc_delta_groups) <= 50,
          "the automatic step's groups are a page: no group pager", len(v._drc_delta_groups))
    check(v._drc_delta_set_step("0.00001") is None, "a step that makes 120 groups")
    settle("120 groups")
    pump(v, lambda: w._delta_prev.get_allocation().height > 1, "the group pager laid out", 10)
    first = (len(v._drc_delta_groups), w._delta_bar.get_visible(), w._delta_label.get_text(),
             len(w._delta_store), w._delta_prev.get_sensitive(), w._delta_next.get_sensitive())
    low = w._delta_prev.get_allocated_height()
    tip = w._delta_store[0][3]
    w._delta_next.clicked()
    second = (w._delta_label.get_text(), w._delta_store[1][0], w._delta_prev.get_sensitive())
    w._delta_next.clicked()
    third = (w._delta_label.get_text(), len(w._delta_store), w._delta_next.get_sensitive())
    check(first == (120, True, "1/3", 51, False, True) and low < tall
          and second[0] == "2/3" and second[1].startswith("C1 #51 ") and second[2]
          and third == ("3/3", 21, False)
          and tip == "All errors · 120 groups · step 0.00001",
          "more than a page of groups: the pager, low, reads page/pages; the rest is the tooltip",
          (first, low, tall, second, third, tip))
    check(v._drc_delta_set_step("auto") is None, "the automatic step again")
    settle("few groups again")
    check(not w._delta_bar.get_visible() and w._delta_label.get_text() == "1/1",
          "back to one page: the pager is gone")

    # ---- 2. the menu on the group rows
    def right_click(row):
        """A right click on a row of the group list: the menu's labels,
        the menu left up for an item to be picked."""
        tree = w._delta_tree
        path = Gtk.TreePath.new_from_indices([row])
        pump(v, lambda: tree.get_cell_area(path, tree.get_column(0)).height > 1,
             "the group list laid out", 10)
        area = tree.get_cell_area(path, tree.get_column(0))
        e = Gdk.Event.new(Gdk.EventType.BUTTON_PRESS)
        e.window = tree.get_bin_window()
        e.send_event = True
        e.x, e.y = area.x + 3, area.y + area.height / 2
        e.button, e.state = 3, Gdk.ModifierType(0)
        e.set_device(Gdk.Display.get_default().get_default_seat().get_pointer())
        v._drc_menu = None
        tree.event(e)
        menu = v._drc_menu
        return menu, ([] if menu is None else
                      ["-" if isinstance(i, Gtk.SeparatorMenuItem) else i.get_label()
                       for i in menu.get_children()])

    def choose(row, label, what):
        menu, labels = right_click(row)
        next(i for i in menu.get_children()
             if not isinstance(i, Gtk.SeparatorMenuItem) and i.get_label() == label).activate()
        menu.popdown()
        settle(what)

    pick(1)
    rows = [(r[0].split(" ")[1] if r[2] is not None else r[0], r[1]) for r in w._delta_store]
    check(rows == [("All errors", "4"), ("#1", "1"), ("#2", "1"), ("#3", "2")]
          and v._drc_delta_group is None and v._drc_grid_map == [0, 1, 2, 3],
          "the fixture's groups", rows)
    menu, labels = right_click(3)
    scope = (None if v._drc_delta_group is None else v._drc_delta_group.total, v._drc_grid_map)
    if menu is not None:
        menu.popdown()
    check(labels == ["select all", "deselect all", "-", "waive all", "unwaive all", "-",
                     "mark not for review all", "clear not for review all"]
          and scope == (2, [0, 1]) and w._delta_tree.get_selection().get_selected()[1] is not None
          and w._delta_store[w._delta_tree.get_selection().get_selected()[1]][0].startswith("C1 #3"),
          "a right click on a group row makes it the grid's scope and offers the six commands",
          (labels, scope))
    choose(3, "select all", "select all")
    sel = v._drc_sel
    check(sel is not None and sel[0] == 1 and list(sel[1]) == [0, 1]
          and v.status.get_text() == "error select PANE.FEW: 2 selected",
          "select all: the group's errors are the selection", v.status.get_text())
    choose(3, "deselect all", "deselect all")
    check(v._drc_sel is None and v.status.get_text() == "error select PANE.FEW: none selected",
          "deselect all drops it", v.status.get_text())
    choose(3, "waive all", "waive all")
    rows = [r[1] for r in w._delta_store]
    check(v._drc.status_counts(1)[0] == 2 and [v._drc.get_status(1, ei) for ei in range(4)]
          == [1, 1, 0, 0] and rows == ["2", "1", "1", "0"] and v._drc_grid_map == []
          and v._drc_sel is None and v.status.get_text() == "waived 2 error(s) in PANE.FEW",
          "waive all: every error of the group is waived (Not Waived lists none of it now)",
          (rows, v.status.get_text()))
    choose(3, "unwaive all", "unwaive all")
    rows = [r[1] for r in w._delta_store]
    check(v._drc.status_counts(1)[0] == 0 and rows == ["4", "1", "1", "2"]
          and v._drc_grid_map == [0, 1]
          and v.status.get_text() == "unwaived 2 error(s) in PANE.FEW",
          "unwaive all undoes it under the same filter: the scope's errors, not the listed ones",
          (rows, v.status.get_text()))
    choose(3, "mark not for review all", "mark not for review all")
    marked = (v._drc.review_counts(1)["not_required"],
              [v._drc.review_flags(1, ei) & gtkservice.NOT_REQUIRED for ei in range(4)])
    choose(3, "clear not for review all", "clear not for review all")
    check(marked == (2, [gtkservice.NOT_REQUIRED, gtkservice.NOT_REQUIRED, 0, 0])
          and v._drc.review_counts(1)["not_required"] == 0,
          "mark / clear not for review all: the group's errors", marked)
    # `All errors`: the rule; the selection in the grid is not the command's
    w._wf.set_active_id("all")
    pick(1)
    menu, _labels = right_click(0)       # (the scope: the rule, not the group)
    menu.popdown()
    settle("the rule's scope")
    v._drc_selection_submit(1, None, gtkservice.ListMembers([2]), "replace")
    settle("one error selected")
    check(v._drc_delta_group is None and v._drc_sel is not None and list(v._drc_sel[1]) == [2],
          "(one error selected in the rule's scope)")
    choose(0, "waive all", "waive all of the rule")
    sel = v._drc_sel
    check(v._drc_delta_group is None and v._drc.status_counts(1)[0] == 4
          and sel is not None and list(sel[1]) == [2],
          "waive all on `All errors`: the whole rule; the selection in the grid stays as it was",
          (v._drc.status_counts(1)[0], None if sel is None else list(sel[1])))
    choose(0, "unwaive all", "unwaive all of the rule")
    choose(0, "deselect all", "deselect all")
    check(v._drc.status_counts(1)[0] == 0 and v._drc_sel is None, "(and undone)")
    # a cluster picked: `All errors` is the cluster
    clusters = dict((c[0], c[3]) for c in w._rstore[1].iterchildren())
    pick(1, clusters["first"])
    choose(0, "waive all", "waive all of the cluster")
    in_cluster = [v._drc.get_status(1, ei) for ei in range(4)]
    choose(0, "unwaive all", "unwaive all of the cluster")
    check(in_cluster == [1, 1, 0, 0] and v._drc.status_counts(1)[0] == 0,
          "with a cluster picked `All errors` is the cluster's errors", in_cluster)
    w._wf.set_active_id("notwaived")
    pick(1)

    # ---- 6. the SVRF lines: out of the detail pane, in a window
    v._drc_goto_cell(1, 2)
    v._drc_show_detail(1, 2)
    pane = detail()
    errors = v._drc.checks[1].errors
    heads = ["PANE.FEW  #%d(%d)" % (ei + 1, errors[ei].num) for ei in range(4)]
    block = v._drc_meta_lines("PANE.FEW", errors[2])
    check(pane.startswith("#3(%d)  [none]\nrule: PANE.FEW\nSVRF: applied (rule name matched)\n"
                          % errors[2].num)
          and "edge (um):" in pane and v._drc_detail_at == (1, 2)
          and not any(word in pane for word in ("constraint:", "measured", "layers:", "gds:",
                                                "derivation:"))
          and block[1:] == ["constraint: EXTERNAL M1_drawn < 0.05",
                            "measured: 0.02000 um vs < 0.05000 · Δ 0.03000 (60.00000%)",
                            "layers: M1_drawn", "gds: 1/0", "derivation:",
                            "  M1_drawn = M1 NOT FILL"],
          "the detail pane of an error has no SVRF lines", (pane, block))
    check(v._svrfdlg is None, "(no SVRF window until it is asked for)")
    v._drc_svrf_dialog()
    dlg = v._svrfdlg

    def shown():
        buf = dlg._buffer
        return buf.get_text(buf.get_start_iter(), buf.get_end_iter(), False).split("\n")
    check(dlg is not None and dlg.get_title() == "SVRF info" and not dlg.get_modal()
          and dlg.get_transient_for() is v.window
          and shown() == [heads[2]] + block[1:],
          "the SVRF window shows them, and does not block", shown())
    v._drc_svrf_dialog()
    check(v._svrfdlg is dlg, "asked again: the same window")
    v._drc_show_detail(1, 3)
    follows = shown()
    v._drc_show_rule(1)
    rule = shown()
    pick(2)
    none = shown()
    check(follows[0] == heads[3]
          and follows[2] == "measured: 0.03000 um vs < 0.05000 · Δ 0.02000 (40.00000%)"
          and rule == ["PANE.FEW", "constraint: EXTERNAL M1_drawn < 0.05", "layers: M1_drawn",
                       "gds: 1/0", "derivation:", "  M1_drawn = M1 NOT FILL"]
          and none[0] == "PANE.NOMETA" and none[1].startswith("SVRF: no matching rule"),
          "it follows the error in the pane; a rule alone has no measurement; one without "
          "metadata says so", (follows, rule, none))
    # the viewer's keys work from it: `.` steps to the next error
    pick(1)
    v._drc_goto_cell(1, 0)
    v._drc_jump(1, 0)

    def key(name):
        e = Gdk.Event.new(Gdk.EventType.KEY_PRESS)
        e.window = dlg.get_window()
        e.keyval = Gdk.keyval_from_name(name)
        return dlg.emit("key-press-event", e)
    stepped = (key("period"), v._drc_detail_at, shown()[0])
    check(stepped == (True, (1, 1), heads[1]),
          "a key pressed in the SVRF window is the viewer's: `.` steps, the window follows",
          stepped)
    closed = (key("Escape"), v._svrfdlg)
    v._drc_svrf_dialog(toggle=True)
    opened = v._svrfdlg
    v._drc_svrf_dialog(toggle=True)
    check(closed == (True, None) and opened is not None and v._svrfdlg is None,
          "Esc closes it; i opens and closes it")

    # ---- the pane has no line for a measurement in the making: a slow
    # one says where it is in the status line, a quick one nothing
    class Slow(object):
        progress = "CD measurement 5 / 10 (Rust, up to 2 workers)"

        def poll(self):
            return None

        def cancel(self):
            pass

        def close(self):
            pass
    worker, busy, since = v._drc_delta_worker, v._drc_delta_busy, v._drc_delta_since
    try:
        v._drc_delta_worker, v._drc_delta_busy = Slow(), True
        v._drc_delta_progress = None
        v._set_live_status("-")
        v._drc_delta_since = time.monotonic()
        v._drc_delta_poll()
        quick = v.status.get_text()
        v._drc_delta_progress = None
        v._drc_delta_since = time.monotonic() - 5.0
        v._drc_delta_poll()
        slow = (v.status.get_text(), v._drc_delta_said)
    finally:
        v._drc_delta_worker, v._drc_delta_busy, v._drc_delta_since = worker, busy, since
        v._drc_delta_said = False
    check(quick == "-" and slow == ("CD groups: CD measurement 5 / 10 (Rust, up to 2 workers)", True),
          "a CD measurement that takes its time says where it is in the status line", (quick, slow))


def cd_part_ui(v, d, pump):
    """The pane's CD part (user 2026-11-10: "CD 델타 컨트롤 각 항목이 너무
    자리를 많이 차지하니 absolute/ratio 콤보박스의 문구를 단순화하여 크기를
    줄이고 step 입력창을 제거하고 set step 버튼을 step 으로 변경하고 누르면
    step을 입력받는 작은 모달이 뜨도록 변경해줘. CD 그룹이 없는 에러는 CD 델타
    컨트롤과 그룹 목록을 모두 제거해줘."): two small controls on a line, the
    step asked in a modal, and neither controls nor list for a rule with no
    CD group."""
    from floe import gui
    Gtk = gui.Gtk
    db = d / "cdpart.db"
    pair = "e %d 2\n0 0 10000 0\n0 %d 10000 %d\n"
    names = ("PART.KNOWN", "PART.UNKNOWN", "PART.NOMETA", "PART.ZEROBOUND",
             "PART.EDGES", "PART.MIXED", "PART.EMPTY")
    db.write_text(
        "MAIN 100000\nPART.KNOWN\n3 3 0\n"
        + "".join(pair % (i, gap, gap) for i, gap in enumerate((1000, 2000, 3000), 1))
        # an AREA constraint cannot measure an edge pair
        + "PART.UNKNOWN\n1 1 0\n" + pair % (1, 1000, 1000)
        # no metadata of this one
        + "PART.NOMETA\n1 1 0\n" + pair % (1, 1000, 1000)
        # touching edges against a bound of zero: a CD, and no ratio
        + "PART.ZEROBOUND\n2 2 0\n" + pair % (1, 0, 0) + pair % (2, 0, 0)
        # single edges: no CD, their lengths
        + "PART.EDGES\n2 2 0\ne 1 1\n0 0 23750 0\ne 2 1\n0 100 25750 100\n"
        # one error measured, one that cannot be
        + "PART.MIXED\n2 2 0\n" + pair % (1, 1000, 1000) + "p 2 3\n0 0\n1000 0\n0 1000\n"
        + "PART.EMPTY\n0 0 0\n")
    space = {"metric": "space", "op": "<", "value": 0.05, "text": "EXTERNAL M1 < 0.05"}
    meta = {"PART.KNOWN": [space],
            "PART.UNKNOWN": [{"metric": "area", "op": "<", "value": 0.01, "text": "AREA M1 < 0.01"}],
            "PART.ZEROBOUND": [{"metric": "space", "op": "<=", "value": 0,
                                "text": "EXTERNAL M1 <= 0"}],
            "PART.EDGES": [space], "PART.MIXED": [space], "PART.EMPTY": [space]}
    (d / "cdpart.db.rules.json").write_text(json.dumps({
        "format": "floe-svrf-rules", "version": 1,
        "checks": {name: {"constraints": cons} for name, cons in meta.items()}}))
    (d / "cdpart.db.clusters").write_text("[PART.MIXED]\npoly = 2\n")
    r = run([INDEX, "drc", str(db)])
    check(r.returncode == 0, "CD part fixture packed", r.stderr)
    # what the service says of each rule: groups, CD groups, edge-length
    # groups, and whether a CD was measured at all
    svc = Svc()
    try:
        h = svc("drc_open", path=str(db), mode="load", reviewer="drc-analysis-service")["handle"]
        said = {}
        for basis in ("absolute", "percent"):
            for ci, name in enumerate(names):
                if name in meta:
                    e = svc.job(h, "drc_delta", check=ci, constraints=meta[name], basis=basis)
                    said[(name, basis)] = (e.get("groups"), e.get("cd_groups"),
                                           e.get("geometry_groups"), e.get("has_cd"))
        svc("drc_close", handle=h)
    finally:
        svc.close()
    both = {"PART.KNOWN": (3, 3, 0, True), "PART.UNKNOWN": (1, 0, 0, False),
            "PART.EDGES": (2, 0, 2, False), "PART.MIXED": (2, 1, 0, True),
            "PART.EMPTY": (0, 0, 0, False)}
    expected = {(name, basis): row for name, row in both.items()
                for basis in ("absolute", "percent")}
    expected[("PART.ZEROBOUND", "absolute")] = (1, 1, 0, True)
    expected[("PART.ZEROBOUND", "percent")] = (1, 0, 0, True)
    check(said == expected,
          "the service says whether a rule's CD was measured, whatever the basis makes of it",
          sorted(said.items()))

    w = v._drcwin
    w._delta_mode.set_active_id("absolute")
    check(v.load_drc(str(db)), "load the CD part fixture")
    w._wf.set_active_id("all")      # the rule without errors is listed
    pump(v, lambda: v._drc_group_cache_key is None, "CD part counts", 60)
    check([v._drc_group_count(ci) for ci in range(7)] == [3, 0, None, 1, 0, 1, None]
          and v._drc_open is None, "the fixture's whole-rule counts",
          [v._drc_group_count(ci) for ci in range(7)])
    # the controls: a combo of three short words and a button
    kids = w._delta_controls.get_children()
    check([type(k).__name__ for k in kids] == ["ComboBoxText", "Button"]
          and kids[0] is w._delta_mode and kids[1] is w._delta_step
          and [(row[1], row[0]) for row in w._delta_mode.get_model()]
          == [("absolute", "absolute"), ("percent", "ratio"), ("off", "off")]
          and w._delta_step.get_label() == "step"
          and isinstance(w._delta_controls, Gtk.Box)
          and w._delta_controls.get_orientation() == Gtk.Orientation.HORIZONTAL,
          "the CD controls are a basis combo and a step button on one line",
          [type(k).__name__ for k in kids])
    check(cd_part(v) == (False, False), "no rule picked: no CD part", cd_part(v))

    def pick(ci, cluster=None):
        """The CD part right after the pick - the rule being measured - and
        once its groups are there."""
        v._drc_select_row(ci, cluster)
        busy, early = v._drc_delta_busy, cd_part(v)
        pump(v, lambda: v._drc_open == ci and v._drc_cluster is cluster
             and not v._drc_delta_busy, "CD part of %s" % names[ci], 60)
        return busy, early, cd_part(v)

    on, off, bar = (True, True), (False, False), (True, False)
    # the first visit: a rule known to have groups is in sight from the
    # pick, one known to have none is never in sight, and one whose count
    # says none but has edge-length groups comes in sight with them
    first = [pick(ci) for ci in range(7)]
    check(first == [(True, on, on), (True, off, off), (False, off, off), (True, on, on),
                    (True, off, on), (True, on, on), (True, off, off)],
          "CD part by rule: with CD or edge-length groups, and not without", first)
    check([(r[0], r[1]) for r in w._delta_store] == [("All errors", "0")]
          and w._delta_store[0][3].startswith("All errors · 0 groups"),
          "(the list of the rule without errors, out of sight)")
    # the second visit: as it was, from the pick on
    again = [pick(ci) for ci in range(7)]
    check(again == [(True, on, on), (True, off, off), (False, off, off), (True, on, on),
                    (True, on, on), (True, on, on), (True, off, off)],
          "a rule seen before keeps its CD part while it is measured again", again)
    # a cluster is judged by its own errors
    v._drcwin._rules.expand_row(w._rstore[5].path, False)
    clusters = dict((c[0], c[3]) for c in w._rstore[5].iterchildren())
    check(sorted(clusters) == ["Unclustered", "poly"], "the fixture's clusters", sorted(clusters))
    check(pick(5, clusters["poly"])[2] == off and len(v._drc_delta_groups) == 1
          and v._drc_delta_groups.has_cd
          and pick(5, clusters["Unclustered"])[2] == on and pick(5)[2] == on,
          "a cluster whose errors have no CD has no CD part, its rule and the other cluster have")
    # the ratio basis: no group of a zero bound, and the rule keeps the
    # part - its CD is measured, and the combo is the way back
    w._delta_mode.set_active_id("percent")
    pump(v, lambda: not v._drc_delta_busy and v._drc_group_cache_key is None, "ratio counts", 60)
    ratio = [pick(ci)[2] for ci in range(7)]
    check(ratio == [on, off, off, on, on, on, off], "CD part by rule in the ratio basis", ratio)
    pick(3)
    check(v._drc_group_count(3) == 0 and v._drc_delta_groups.cd_count == 0
          and v._drc_delta_groups.has_cd and cd_part(v) == on
          and [(r[0], r[1]) for r in w._delta_store] == [("All errors", "2"), ("Unmeasurable", "2")]
          and w._delta_store[1][3].endswith("Unmeasurable / ratio unavailable"),
          "a rule measured against a zero bound keeps the CD part in the ratio basis",
          [(r[0], r[1], r[3]) for r in w._delta_store])
    check(pick(5, clusters["poly"])[2] == off, "(a cluster without CD: none in the ratio basis either)")
    # grouping off: nothing to list; the controls - the way to turn it on
    # again - for a rule that was not seen to have no group, the step
    # button of no use
    w._delta_mode.set_active_id("off")
    quiet = []
    for ci in range(7):
        v._drc_select_row(ci)
        quiet.append((v._drc_delta_busy, cd_part(v)))
    check(quiet == [(False, bar), (False, off), (False, off), (False, bar),
                    (False, bar), (False, bar), (False, off)]
          and not w._delta_step.get_sensitive(),
          "grouping off: the controls alone, for a rule that has groups", quiet)
    w._delta_step.clicked()
    check(v._sdlg is None, "grouping off: the step button opens nothing")
    w._delta_mode.set_active_id("absolute")
    pump(v, lambda: not v._drc_delta_busy and v._drc_group_cache_key is None, "absolute again", 60)

    # the step's modal
    check(pick(0)[2] == on and w._delta_step.get_sensitive() and v._drc_delta_steps == {},
          "a rule with groups: the step button is live")
    # side by side at their own widths where the pane has the room (the
    # FlowBox there was stretched each over the pane, one a line when narrow)
    pump(v, lambda: w._delta_step.get_allocation().width > 1, "the controls laid out", 10)
    a, b = w._delta_mode.get_allocation(), w._delta_step.get_allocation()
    row = w._delta_controls.get_allocation()
    need = w._delta_controls.get_preferred_width()[1]
    check(row.width < need
          or (a.y == b.y and a.x == row.x and b.x == a.x + a.width + 4
              and a.width == w._delta_mode.get_preferred_width()[1]
              and b.width == w._delta_step.get_preferred_width()[1]
              and a.width + 4 + b.width == need and need < row.width),
          "the two controls stand side by side at the left, at their own widths",
          ((a.x, a.y, a.width), (b.x, b.y, b.width), (row.x, row.width), need))
    # ... and on a pane narrower than the two their texts shorten: the
    # line asks for less than it would like, so the pane is not cut off
    least = w._delta_controls.get_preferred_width()[0]
    check(w._delta_mode.get_cells()[0].get_property("ellipsize") == gui.Pango.EllipsizeMode.END
          and w._delta_step.get_child().get_ellipsize() == gui.Pango.EllipsizeMode.END
          and 0 < least < need,
          "on a narrow pane the controls' texts shorten instead of the line being cut off",
          (least, need))
    auto = gui.format_ticks(v._drc_delta_groups.step_ticks)
    label = w._delta_store[0][3]
    check("· auto step %s" % auto in label
          and w._delta_step.get_tooltip_text().startswith("CD group step: %s (automatic)." % auto),
          "the automatic step is read in the `All errors` row's tooltip and the button's",
          (label, w._delta_step.get_tooltip_text()[:50]))
    named = step_dialog(v)
    dlg = v._sdlg
    check(dlg is not None and dlg.get_modal() and dlg.get_title() == "CD group step"
          and dlg.get_transient_for() is v.window and not dlg.get_resizable()
          and named["drc-step"].get_text() == auto
          and named["drc-step-unit"].get_text() == "um"
          and "auto" in named["drc-step-why"].get_text(),
          "the step button opens a small modal that starts from the step in use",
          (named["drc-step"].get_text(), named["drc-step-why"].get_text()))
    w._delta_step.clicked()
    check(v._sdlg is dlg, "pressed again: the same dialog")
    refusals = [step_dialog(v, text) for text in ("abc", "0", "-1", "0.000001")]
    check(refusals == ["invalid grouping step", "step must be a positive finite number",
                       "step must be a positive finite number",
                       "step supports at most five decimal places"]
          and v._sdlg is dlg and v._drc_delta_steps == {} and not v._drc_delta_busy,
          "a step that is refused is said in the dialog, which stays; nothing changes", refusals)
    took = step_dialog(v, "0.05")
    check(took is None and v._sdlg is None and v._drc_delta_steps == {(0, "absolute"): 5000}
          and v._drc_delta_busy and cd_part(v) == on,
          "Enter sets the step and closes the dialog; the CD part stays while it is grouped",
          (took, v._drc_delta_steps, cd_part(v)))
    pump(v, lambda: not v._drc_delta_busy and v._drc_group_cache_key is None, "the step set", 60)
    label = w._delta_store[0][3]
    check(v._drc_delta_groups.step_ticks == 5000 and not v._drc_delta_groups.auto_step
          and "· step 0.05000" in label and "auto step" not in label
          and w._delta_step.get_tooltip_text().startswith("CD group step: 0.05000. ")
          and v._drc_group_count(0) == 1 and cd_part(v) == on,
          "a step of one's own is read in the `All errors` row's tooltip and the button's",
          (label, w._delta_step.get_tooltip_text()[:40]))
    named = step_dialog(v)
    check(named["drc-step"].get_text() == "0.05000", "the dialog starts from the step set",
          named["drc-step"].get_text())
    named["drc-step"].set_text("0.01")
    key = gui.Gdk.EventKey.new(gui.Gdk.EventType.KEY_PRESS)
    key.keyval = gui.Gdk.KEY_Escape
    v._sdlg.emit("key-press-event", key)
    check(v._sdlg is None and v._drc_delta_steps == {(0, "absolute"): 5000}
          and not v._drc_delta_busy, "Esc closes the dialog and leaves the step")
    took = step_dialog(v, " AUTO ")
    pump(v, lambda: not v._drc_delta_busy and v._drc_group_cache_key is None, "automatic again", 60)
    check(took is None and v._drc_delta_steps == {}
          and "· auto step %s" % auto in w._delta_store[0][3]
          and v._drc_group_count(0) == 3,
          "auto (or nothing) is the automatic step again", w._delta_store[0][3])
    took = step_dialog(v, "")
    check(took is None and v._sdlg is None and v._drc_delta_steps == {} and not v._drc_delta_busy,
          "the automatic step asked again changes nothing")
    # the unit is the basis'
    w._delta_mode.set_active_id("percent")
    pump(v, lambda: not v._drc_delta_busy and v._drc_group_cache_key is None, "ratio", 60)
    named = step_dialog(v)
    unit = named["drc-step-unit"].get_text()
    v._sdlg.destroy()
    check(unit == "%" and v._sdlg is None, "the dialog's unit in the ratio basis", unit)
    w._delta_mode.set_active_id("absolute")
    pump(v, lambda: not v._drc_delta_busy and v._drc_group_cache_key is None, "absolute", 60)


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
    analysis = analysis_dir(d / "sample.db")
    check(not list(analysis.glob("delta/*/measure/complete.json")), "sample has no prepared CD cache")
    cache = gtkservice.ServiceCache(str(d / "sample.oas"))
    cache.load()
    v = gui.Viewer(cache, detail=2)
    try:
        pump(v, lambda: v.last_frame is not None, "first frame")
        check(v.load_drc(str(d / "sample.db")), "load_drc")
        pump(v, lambda: v._drc_group_cache_key is None, "background group counts", 60)
        expected_counts = [5, 1, 2, 2, 2, 2, 2, 3]
        check([v._drc_group_count(ci) for ci in range(8)] == expected_counts
              and v._drc_open is None and v._drc_delta_worker is None
              and not v._drc_delta_busy,
              "cold-open counts appear for every rule before any rule is selected")
        check(len(list(analysis.glob("delta/*/measure/complete.json"))) == 8
              and len(list(analysis.glob("delta/*/summaries/*/complete.json"))) == 8
              and not list(analysis.glob("delta/*/groups/*/complete.json")),
              "background prefill persists small-rule measurements and tiny summaries, not full groups")
        before = {str(p): (p.stat().st_size, p.stat().st_mtime_ns)
                  for p in analysis.rglob("*") if p.is_file()}
        check(v.load_drc(str(d / "sample.db")), "reopen DB with summary caches")
        v._drcwin._wf.set_active_id("all")  # inspect the rule after waiving it entirely
        pump(v, lambda: v._drc_group_cache_key is None, "warm group counts", 60)
        after = {str(p): (p.stat().st_size, p.stat().st_mtime_ns)
                 for p in analysis.rglob("*") if p.is_file()}
        check([v._drc_group_count(ci) for ci in range(8)] == expected_counts
              and before == after and v._drc_open is None and v._drc_delta_worker is None,
              "warm reopen reuses unchanged summaries before selecting any rule")
        store = v._drcwin._rstore
        name_column = v._drcwin._rules.get_column(0)
        name_column.cell_set_cell_data(store, store.get_iter_first(), False, False)
        check(len(v._drcwin._rules.get_columns()) == 2
              and [r[0] for r in store] == [c.name for c in v._drc.checks]
              and all(r[4] == "✓ (%d) %s" % (expected_counts[r[2]], r[0])
                      and r[0] in r[5] for r in store)
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
              and rows[1][0] == "C1 #1 <=0.02200"
              and v._drcwin._delta_store[1][3].startswith(gui.GLib.markup_escape_text(
                  "Group #1 · condition 1 · space < 0.05000: [0.02000, 0.02200) um")),
              "the delta list", rows[:2])
        label = v._drcwin._delta_store[0][3]
        check(label == "All errors · 5 groups · auto step 0.00200 · 20 estimated (all statuses)"
              and v._drcwin._delta_label.get_text() == "1/1"
              and not v._drcwin._delta_bar.get_visible(),
              "the grouping in a line is the `All errors` row's tooltip; one page: no pager",
              label)
        check(cd_part(v) == (True, True), "a rule with CD groups shows the pane's CD part")
        v._display()
        pump(v, lambda: v._drc_marker_result is not None and not v._drc_marker_busy, "markers", 60)
        m = v._drc_marker_result[1]
        check((m.visible_count, m.total_count) == (20, 20), "the marker picture",
              (m.visible_count, m.total_count))
        # the panel has no info line (2026-11-10): a picture long enough in
        # the making to show a part says so in the status line and then
        # that it is done - a part must not pass for the whole; a quick one
        # says nothing

        class Replies(object):
            def __init__(self, replies):
                self.replies = list(replies)

            def poll(self):
                return self.replies.pop(0) if self.replies else None
        worker, key = v._drc_marker_worker, v._drc_marker_key
        v._drc_marker_worker = Replies([(key, gtkservice.MarkerProgress(m), None),
                                        (key, m, None), (key, m, None)])
        try:
            v._drc_marker_poll()
            part, busy = v.status.get_text(), v._drc_marker_busy
            v._drc_marker_poll()
            done = v.status.get_text()
            v._set_live_status("-")
            v._drc_marker_poll()
            quick = v.status.get_text()
        finally:
            v._drc_marker_worker = worker
        check(part == "DRC markers: %d / 20 errors processed (partial)…" % m.processed_count
              and busy and done == "DRC markers: 20 errors / %d markers in view" % m.occupied_count
              and quick == "-" and not v._drc_marker_busy and v._drc_marker_result[1] is m,
              "a marker picture in the making says so in the status line", (part, done, quick))
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
        check(v._drc_group_count(0) == 5 and store[0][4] == "✓ (5) M1.SPACE.1"
              and len(v._drc_delta_groups) != 5,
              "cluster grouping does not replace the parent rule's whole-rule count")
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
            pump(v, lambda: v._drc_delta_groups is not None and v._drc_group_cache_key is None,
                 "retained cluster groups and cached counts", 60)
            notch = next(r for r in store if r[0] == "M1.NOTCH")
            label, explanation = v._drc_svrf_status("M1.NOTCH")
            check(label == ("✓" if enabled else "")
                  and notch[4] == ("✓ (2) M1.NOTCH" if enabled else "M1.NOTCH")
                  and v._drc_group_count(notch[2]) == (2 if enabled else None)
                  and "M1.NOTCH" in notch[5] and explanation in notch[5]
                  and (enabled or "#IFDEF" in explanation)
                  and all(r[4] == "✓ (%d) %s" % (expected_counts[r[2]], r[0])
                          for r in store if r[0] != "M1.NOTCH")
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
        pump(v, lambda: v._drc_group_cache_key is None, "empty constraint group counts", 60)
        check(next(r for r in store if r[0] == "M1.NOTCH")[4] == "✓ M1.NOTCH"
              and v._drc_svrf_status("M1.NOTCH")[0] == "✓"
              and v._drc_delta_groups is None and not v._drc_group_count(notch_ci),
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
              and v._drc_meta_lines("M1.NOTCH", e) == []
              and cd_part(v) == (False, False),
              "GUI does not infer CD or delta groups from a rule description")
        check(len(v._drcwin._rules.get_columns()) == 2
              and all(r[4] == r[0] and r[0] in r[5] for r in v._drcwin._rstore)
              and v._drc_svrf_status("M1.NOTCH") == ("", "SVRF: not loaded")
              and not v._drc_group_counts,
              "without metadata rule names have no checkmarks or extra column")
        svc = Svc()
        try:
            # (the viewer above has this reviewer's review of the database
            # open: another process opens it as another reviewer - one
            # session a reviewer and result, 0.12.440)
            h = svc("drc_open", path=str(db), mode="load",
                    reviewer="drc-analysis-service")["handle"]
            info = svc("drc_cd_info", handle=h, check=ci, error=0)
            event = svc.job(h, "drc_delta", check=ci, basis="absolute")
            check(info["pick"] is None and event["state"] == "done"
                  and event.get("measurable") is False and "group" not in event,
                  "service leaves description-only CD/delta unmeasurable", (info, event))
            svc("drc_close", handle=h)
        finally:
            svc.close()
        check(v._drc_rules_load(str(d / "sample.svrf.rules.json")), "load SVRF for the same DB")
        pump(v, lambda: v._drc_delta_groups is not None and v._drc_group_cache_key is None,
             "SVRF groups restored", 60)
        check(v._drc_cd_info(e, "M1.NOTCH")["pick"]["ticks"] == [2000, 5000, 3000]
              and len(v._drcwin._delta_store) > 1
              and next(r for r in v._drcwin._rstore if r[2] == ci)[4] == "✓ (2) M1.NOTCH"
              and all(r[4] == "✓ (%d) %s" % (expected_counts[r[2]], r[0])
                      for r in v._drcwin._rstore),
              "SVRF metadata restores CD measurement and delta groups")
        check(cd_part(v) == (True, True), "and the pane's CD part with them")
        explicit_rules_ui(v, d, pump)
        grouped_rules_ui(v, d, pump)
        zero_gap_ui(v, d, pump)
        marker_length_ui(v, temp, pump)
        cd_part_ui(v, d, pump)
        pane_ui(v, d, pump)
        small = d / "small.db"
        small.write_text("MAIN 100000\nSMALL.ONE\n0 0 0\nSMALL.TWO\n0 0 0\n")
        (d / "small.db.rules.json").write_text(json.dumps({
            "format": "floe-svrf-rules", "version": 1,
            "checks": {"SMALL.ONE": {}, "SMALL.TWO": {}}}))
        r = run([INDEX, "drc", str(small)])
        check(r.returncode == 0, "smaller DB fixture packed", r.stderr)
        small_loaded = v.load_drc(str(small))
        v._drcwin._wf.set_active_id("all")  # inspect both zero-error rules
        check(small_loaded and len(v._drcwin._rstore) == 2
              and all(r[4] == "✓ " + r[0] and r[0] in r[5] for r in v._drcwin._rstore)
              and not v._drc_group_counts,
              "switching to a smaller DB safely refreshes auto-loaded SVRF statuses")
        v._drc_release()
        check(v._drc is None and len(v._drcwin._rstore) == 0
              and len(v._drcwin._rules.get_columns()) == 2 and not v._drc_group_counts,
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
            marker_length_groups(svc, temp)
        finally:
            svc.close()
        a5(temp)
    if FAILED:
        print("validate_drc_analysis: FAIL (%d): %s" % (len(FAILED), ", ".join(FAILED)))
        sys.exit(1)
    print("validate_drc_analysis: all green")


if __name__ == "__main__":
    main()
