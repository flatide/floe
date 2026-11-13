#!/usr/bin/env python3
"""The GTK viewer's view channel (docs/SHARED_APP_LAYER.ko.md §7, P4c):
`floe2 gtk-service`'s view_open / view_edit / view_cancel / view_close over
the shared Rust ViewController, driven as the viewer drives it
(floe/gtkservice.py ViewSession), against a real floe-renderd.

Checked: every frame the channel hands over is byte-equal to the frame the
Python renderd adapter (tools/oracle/floe_oracle/rust_render.py
RustRenderWorker, the viewer's until P4f) draws for the same box, size and
policy - a plain open, a snapped pan, the density under the cut (its first
round, then the frame), a margin around the viewport (vw/vh) and a pan the
margin covers (no new frame); the state the service says (revisions, phase,
density); a closed view's files go and the service opens the next one.

P4d-P4f: snap, pick, the cell tree and a clip through the channel = the
adapter's answers, cell questions while the view still opens answered once
it is open; the viewer itself (floe/gui.py Viewer, in process, pycairo
blocked as on its hosts) - its first frame, a pan, a zoom, the density, the
grayscale, each frame byte-equal to the adapter's for the view the viewer
shows, the perf line the service sends = the Python perf_status
(floe_oracle/perf_line.py) over the frame's report (the adapter's result
keys), a snap answered, a margin's pan a crop, its navigations the
controller's = its own maths, its minimap the service's bake, the hidden
layer's strike and the palettes drawn as images.

`floe2 render` with its defaults = the viewer's pixels for the views it
settles on (capture_parity, 2026-10-12).

The scale border (2026-10-13): the band along the frame's edges is the
service's (app-core view::border) - segments, tones, rule, the corners'
chips -, it follows a pan, goes with `s` and under Tab's clean look
(border_parity); view_border with a box given, and refused.

The cell tree's status line (2026-10-15): a selected cell says the
service's cell_info text (app-core view::cell_info) - a cell placed once
its place, one placed 2.25 million times its count -, behind the cursor's
coordinates as it moves; Esc clears it (cell_status).

The file dialogs' browser (2026-10-26): a symbolic link is a row `name@`
under the filters its own name passes, among the files by name, its own
text the tooltip; a link to a folder is entered, one to a file picked
(browse_dialog). The jobdeck level dialog labels a level `level N`, its
MTITLE after it (levels_dialog).

The file loads turned off (2026-11-11, `floe2 view --no-file-load`): the
viewer loads what its start names and nothing picked in it - the eight
load commands of the File and DRC menus insensitive, each handler
refusing without a dialog, the layer menu's load, the reviewers window's
load button, the window of a previous run not opened; a plain viewer
beside it loads as ever (file_loads_off).

One window a display (2026-11-11): a later launch's request - layout, DRC
results, reviewer, file loads off - handed over the instance socket to a
real window, which switches to it without asking; results already open
stay; a request waits while a dialog is open or a review write runs (a
note typed meanwhile lands on the results it was typed for); the review
written before a switch is what `floe2 drc --review-stats` reads
(forwarded_requests).
"""


import math
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
FLOE2 = os.environ.get("FLOE2_BIN") or str(
    ROOT / "rust" / "target" / "release" / "floe2")
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT / "tools" / "oracle"))  # floe_oracle (P3)
from floe import gtkservice  # noqa: E402
from floe_oracle.rust_render import RustRenderWorker  # noqa: E402
from floe_oracle.cache import Cache  # noqa: E402
from floe_oracle import overlay_snap  # noqa: E402
from floe_oracle.perf_line import perf_status  # noqa: E402

W, H = 400, 300


def check(condition, message):
    if not condition:
        raise AssertionError(message)


def until(session, want, deadline_s=120):
    """Events until `want(kind, value)` says stop; the frames read."""
    deadline = time.monotonic() + deadline_s
    frames = []
    while time.monotonic() < deadline:
        for kind, value in session.events():
            if kind == "frame":
                data, w, h, fmt = gtkservice.read_frame(value)
                value = dict(value, pixels=data)
                frames.append(value)
            if want(kind, value):
                return frames
        time.sleep(0.005)
    raise AssertionError("view channel: no %s in %d s" % (want, deadline_s))


def final_frame(state_rev, purpose="foreground"):
    return lambda kind, v: (kind == "frame" and v["purpose"] == purpose
                            and v["final"] and v["state_rev"] == state_rev)


class Reference:
    """The viewer's current adapter on the same cache."""

    def __init__(self, src):
        cache = Cache(str(src))
        cache.load()
        self.worker = RustRenderWorker(cache)
        self.worker.start()
        self.gen = 0

    def frame(self, f, density=None, labels=True, view=None, frames=False,
              depth=None, occupancy=None, cut_px=1.0):
        self.gen += 1
        job = {"kind": "render", "gen": self.gen, "scope": "live",
               "bbox": tuple(f["bbox"]), "view": view, "w": f["width"],
               "h": f["height"], "depth": depth, "cut_px": cut_px,
               "frames": frames, "labels": labels, "label_font_px": 14,
               "lod": False, "abstract": False, "visible": None,
               "thin": "keep", "frame_cache": True, "frame_format": "raw"}
        if view is not None:
            job["bg"] = True
        if density is not None:
            job["density"] = density
        if occupancy is not None:
            job["occupancy"] = occupancy
        self.worker.submit(job)
        deadline = time.monotonic() + 120
        while time.monotonic() < deadline:
            res = self.worker.res.get(timeout=max(0.1, deadline - time.monotonic()))
            check(res.get("kind") != "error", "reference render: %s" % res)
            if res.get("kind") == "frame" and res.get("gen") == self.gen \
                    and not res.get("refining"):
                self.result = res
                return bytes(res["rgba"])
        raise AssertionError("reference render timeout")

    def stop(self):
        self.worker.stop()


def answer(session, kind, test, deadline_s=60):
    deadline = time.monotonic() + deadline_s
    while time.monotonic() < deadline:
        for k, v in session.events():
            if k == "frame":
                gtkservice.read_frame(v)
            elif k == kind and test(v):
                return v
        time.sleep(0.005)
    raise AssertionError("view channel: no %s answer" % kind)


def reference_answer(ref, job, kind):
    ref.worker.submit(job)
    deadline = time.monotonic() + 60
    while time.monotonic() < deadline:
        res = ref.worker.res.get(timeout=max(0.1, deadline - time.monotonic()))
        check(res.get("kind") != "error", "reference %s: %s" % (kind, res))
        # the adapter's clip answer carries no seq: one clip at a time
        if res.get("kind") == kind and (kind == "clip" or res.get("seq") == job.get("seq")):
            return res
    raise AssertionError("reference %s timeout" % kind)


def overlay_snaps(s, frame):
    """A snap takes what the viewer draws over the frame too (0.12.454, user
    2026-11-12: "룰러의 스냅이 에러 표시 도형에는 적용되지 않는데 적용되게 할
    수 있을까?"): the segments of an error's shape go with the question
    (`overlay`) and are judged with the layout's answer by its own rule, a
    vertex before an edge and then the nearer - = the reference's
    (floe_oracle/overlay_snap.py) for the same segments and that answer."""
    def snap(**more):
        qid = s.query(frame["id"], "snap", W / 2, H / 2, 10, **more)
        return answer(s, "query", lambda v, qid=qid: v["id"] == qid)

    def same(got, want):
        if not want["found"]:
            return got.get("found") is False and "overlay" not in got
        return all(got.get(k) == want.get(k)
                   for k in ("found", "x", "y", "snap", "overlay"))
    scene = snap()
    x, y, r = (scene["request"][k] for k in ("x", "y", "r"))
    check(scene["found"] and "overlay" not in scene and r >= 8,
          "the fixture's snap at the centre: %r" % scene)
    um = 0.001   # a host holds its shapes in micrometres: not whole dbu again
    cases = (
        ("a segment out of reach",
         [[x + 3 * r, y + 3 * r, x + 5 * r, y + 3 * r]]),
        ("an end beside the cursor, off the grid",
         [[x + 0.25, y - 0.5, x + 4 * r, y - 0.5]]),
        ("an end at the reach's rim", [[x + r, y, x + 3 * r, y]]),
        ("an end a dbu past the rim", [[x + r + 1, y, x + 3 * r, y]]),
        ("a segment through the reach, its ends out of it",
         [[x - 3 * r, y + 1.5, x + 3 * r, y + 1.5]]),
        ("a slanted segment",
         [[x - 2 * r, y - 2 * r + 2, x + 2 * r, y + 2 * r + 2]]),
        ("a point from micrometres",
         [[(x + 2) * um / um, (y + 3) * um / um] * 2]),
        ("several, the nearest end of them",
         [[x + 3 * r, y, x + 5 * r, y], [x - 4, y - 3, x - 4, y - 3 * r],
          [x - 3 * r, y + 1.5, x + 3 * r, y + 1.5], [x + 2, y + 3, x + r, y + 3]]),
    )
    with_layout, alone = [], []
    for what, segments in cases:
        got = snap(overlay=segments)
        want = overlay_snap.snap(segments, x, y, r, scene)
        check(same(got, want), "snap with %s: channel %r, reference %r (the layout's %r)"
              % (what, got, want, scene))
        with_layout.append(got)
        # the layout not asked (a view too far out for its geometry)
        got = snap(overlay=segments, layout=False)
        want = overlay_snap.snap(segments, x, y, r)
        check(same(got, want), "snap with %s alone: channel %r, reference %r"
              % (what, got, want))
        alone.append(got)
    check(same(with_layout[0], scene) and same(with_layout[3], scene),
          "a segment out of reach leaves the layout's answer: %r" % with_layout[:4])
    check(with_layout[1].get("overlay") and with_layout[1]["snap"] == "vertex"
          and (with_layout[1]["x"], with_layout[1]["y"]) == (x + 0.25, y - 0.5),
          "an end beside the cursor is the snap, as it is: %r" % with_layout[1])
    check([(a["found"], a.get("snap"), a.get("x"), a.get("y")) for a in alone] == [
        (False, "", 0, 0), (True, "vertex", x + 0.25, y - 0.5), (True, "vertex", x + r, y),
        (False, "", 0, 0), (True, "edge", x, y + 1.5), (True, "edge", x - 1, y + 1),
        (True, "vertex", x + 2, y + 3), (True, "vertex", x + 2, y + 3)]
        and all(a.get("overlay") for a in alone if a["found"]),
        "the segments alone: %r" % alone)
    check(snap(layout=False)["found"] is False, "nothing asked, nothing found")
    check(same(snap(overlay=[[x + 2, y + 3, x + r, y + 3]] * 256, layout=False), alone[-1]),
          "256 segments are taken")

    def refused(kind="snap", **fields):
        try:
            s._svc.request("view_query", view=s.view, frame=int(frame["id"]), kind=kind,
                           x=W / 2.0, y=H / 2.0, r_px=10.0, **fields)
        except gtkservice.ServiceError as exc:
            return "%s: %s" % (exc.kind, exc)
        return None
    one = [x, y, x + 1, y]
    for what, fields in (("no list", {"overlay": "near"}),
                         ("three numbers", {"overlay": [one[:3]]}),
                         ("a word among them", {"overlay": [one[:3] + ["x"]]}),
                         ("a number out of range", {"overlay": [[1e300, 0, 1, 1]]}),
                         ("more than 256", {"overlay": [one] * 257}),
                         ("a layout neither on nor off", {"layout": "no"})):
        said = refused(**fields)
        check(said is not None and said.startswith("input: "),
              "view_query with %s: %r" % (what, said))
    said = refused(kind="pick", overlay=[one])
    check(said is not None and "only a snap takes overlay segments" in said,
          "a pick with overlay segments: %r" % said)
    print("overlay snaps: %d cases with the layout's answer %s (%s, %s), reach %d dbu"
          % (len(cases), scene["snap"], scene["x"], scene["y"], r))


def pick_layout(temp):
    """Shapes over one another, a layer each, to pick among and snap to
    (layers in paint order, the last on top): two plates over the die (1,
    2), a 1 um square (2/5) under a 6 um square (3) that has a 2 um square
    on it (4), a 30 um square over them all (5), and beside the die a square
    of its own (6)."""
    import klayout.db as db
    d = Path(temp) / "pick"
    d.mkdir()
    layout = db.Layout()
    layout.dbu = 0.001
    top = layout.create_cell("TOP")
    for n, box in ((1, (0, 0, 40000, 40000)), (2, (0, 0, 40000, 40000)),
                   (3, (17000, 17000, 23000, 23000)), (4, (19000, 19000, 21000, 21000)),
                   (5, (5000, 5000, 35000, 35000)), (6, (42000, 5000, 48000, 35000))):
        top.shapes(layout.layer(db.LayerInfo(n, 0))).insert(db.Box(*box))
    top.shapes(layout.layer(db.LayerInfo(2, 5))).insert(db.Box(21500, 21500, 22500, 22500))
    src = d / "plates.oas"
    layout.write(str(src))
    done = subprocess.run([FLOE2, "index", str(src), "--jobs", "1"], cwd=ROOT,
                          capture_output=True, text=True, timeout=600)
    check(done.returncode == 0, done.stdout + done.stderr)
    return src


# pick_layout's fills: the plates and the small square the default speckle,
# the 6 um square solid, the two large squares their outlines alone
PICK_FILLS = [{"pair": [3, 0], "fill": {"kind": "solid"}},
              {"pair": [5, 0], "fill": {"kind": "clear"}},
              {"pair": [6, 0], "fill": {"kind": "clear"}}]
# a snap at a point (um) of that view: the answer with the solid square on,
# with every shape snapped to, and the answer with the solid square off
SNAP_CASES = [
    # the corner and an edge of the 1 um square under the solid one: passed
    # by while it is covered, snapped to when the solid square is off
    ("a covered corner", (21.6, 21.6), None, ("vertex", 21500, 21500), ("vertex", 21500, 21500)),
    ("a covered edge", (22.0, 21.45), None, ("edge", 22000, 21500), ("edge", 22000, 21500)),
    # the small square on top and the solid square itself show
    ("a corner on top", (21.05, 21.05), ("vertex", 21000, 21000), ("vertex", 21000, 21000),
     ("vertex", 21000, 21000)),
    ("the solid square's corner", (22.9, 22.9), ("vertex", 23000, 23000),
     ("vertex", 23000, 23000), None),
    ("the solid square's edge", (23.1, 20.0), ("edge", 23000, 20000), ("edge", 23000, 20000),
     None),
]
# a point (um), the layers picked there from the smallest, the shapes over
# the point that do not show - in a view 20 um wide around the die's middle
PICK_CASES = [
    # the small square, and between its speckles the solid square under it;
    # the plates are under the solid one, the 30 um square's outline far off
    ("the middle", (20.0, 20.0), [4, 3], 3),
    # on the solid square beside the small one
    ("the solid square", (22.0, 20.0), [3], 3),
    # off it: the upper plate - the lower one has the same speckle and
    # lights no pixel the upper one left
    ("the plates", (27.0, 20.0), [2], 2),
]


def pick_order(src, svc):
    """A pick takes what is seen (0.12.456). User 2026-11-12, after 0.12.455
    left shapes larger than the view out of a pick: "화면 밖에 걸쳐있는 도형을
    제외해 달라는 얘기였는데 생각해보니 이러면 명백하게 보이는 도형도 선택이 안되는
    문제가 있을 것 같음. 그리고 가능하면 현재 눈에 보이는 도형을 선택하도록 해야 함.
    레이어가 켜져 있더라도 가려진 레이어의 경우에는 선택되지 않는게 좋을 것 같음."
    Of the shapes over the point the candidates are those that show in the
    frame there - the renderer paints the corner of the frame around the
    point again and reads which layers wrote its pixels -, the smallest
    first; the others are counted (`hidden`). No size keeps a shape out."""
    def cycle(s, frame, at):
        """The picks at a point (um) from nth 0 until the cycle is round."""
        box = frame["bbox"]
        px = (at[0] * 1000 - box[0]) / (box[2] - box[0]) * W
        py = (box[3] - at[1] * 1000) / (box[3] - box[1]) * H
        out = []
        for nth in range(70):
            qid = s.query(frame["id"], "pick", px, py, 3, nth=nth)
            got = answer(s, "query", lambda v, qid=qid: v["id"] == qid)
            if not got["found"] or (nth and got["index"] == 0):
                return out, got
            out.append(got)
        raise AssertionError("pick: the cycle did not come round: %r" % out[-1])

    def view(s, width_um, **more):
        snap = s.edit(navigation={"kind": "goto", "center_um": [20.0, 20.0],
                                  "width_um": width_um}, **more)
        return until(s, final_frame(snap["state_rev"]))[-1]

    def said(picks, last):
        return ([a["layer"] for a in picks], (picks[0] if picks else last)["hidden"])

    def snap(s, frame, at):
        """A snap at a point (um), 10 px around it: (kind, x, y) or None."""
        box = frame["bbox"]
        qid = s.query(frame["id"], "snap",
                      (at[0] * 1000 - box[0]) / (box[2] - box[0]) * W,
                      (box[3] - at[1] * 1000) / (box[3] - box[1]) * H, 10)
        got = answer(s, "query", lambda v, qid=qid: v["id"] == qid)
        return ((got["snap"], got["x"], got["y"]) if got["found"] else None,
                got["request"]["seen"])
    s = gtkservice.ViewSession(src, W, H, patch={"detail": "high"}, svc=svc)
    until(s, final_frame(1))
    frame = view(s, 20.0, style_deltas=PICK_FILLS)
    for what, at, layers, hidden in PICK_CASES:
        picks, again = cycle(s, frame, at)
        check(said(picks, again) == (layers, hidden)
              and all(a["count"] == len(layers) and a["hidden"] == hidden
                      and a["request"]["seen"] is True for a in picks)
              and again["layer"] == layers[0]
              and [a["area"] for a in picks] == sorted(a["area"] for a in picks),
              "a pick at %s %r: %r" % (what, at, said(picks, again)))
    # a snap is of what shows too (0.12.457, user 2026-11-12: "스냅도 가려진
    # 도형에는 붙지 않도록 할 수 있을까?"): the corner and the edge of a shape
    # under the solid square are passed by; what is on top and the solid
    # square itself are snapped to as ever
    for what, at, shown, _every, _open in SNAP_CASES:
        got = snap(s, frame, at)
        check(got == (shown, True), "a snap at %s %r: %r" % (what, at, got))
    # the pick there: the solid square, and four shapes under it
    picks, again = cycle(s, frame, (22.0, 22.0))
    check(said(picks, again) == ([3], 4), "a pick over the covered square: %r"
          % (said(picks, again),))
    # the 30 um square is its outline: on it, and within the few pixels
    # around it that a pick looks at, it is picked - and the plate with it -,
    # further inside it is not
    frame = view(s, 44.0)
    for at, layers, hidden in (((35.0, 20.0), [5, 2], 1), ((34.5, 20.0), [5, 2], 1),
                               ((33.0, 20.0), [2], 2)):
        picks, again = cycle(s, frame, at)
        check(said(picks, again) == (layers, hidden),
              "a pick at %r by the outline: %r" % (at, said(picks, again)))
    # a square that is its outline alone, nothing under it: inside it there
    # is nothing to see, and the answer says that a shape is there
    picks, none = cycle(s, frame, (40.9, 20.0))
    check(picks == [] and none["found"] is False and none["count"] == 0
          and none["hidden"] == 0 and "err" not in none,
          "a pick beside the die: %r" % none)
    frame = view(s, 60.0)
    picks, none = cycle(s, frame, (45.0, 20.0))
    check(picks == [] and none["found"] is False and none["hidden"] == 1,
          "a pick inside an outline alone: %r" % none)
    picks, again = cycle(s, frame, (48.0, 20.0))
    check(said(picks, again) == ([6], 0), "a pick on that outline: %r" % (said(picks, again),))
    # the upper plate off: the lower one shows and is picked
    off = s.edit(layers=[[1, 0], [2, 5], [3, 0], [4, 0], [5, 0], [6, 0]])
    frame = until(s, final_frame(off["state_rev"]))[-1]
    frame = view(s, 20.0)
    picks, again = cycle(s, frame, (27.0, 20.0))
    check(said(picks, again) == ([1], 1), "the upper plate off: %r" % (said(picks, again),))
    # the solid square off: what it covered shows, and is snapped to
    off = s.edit(layers=[[1, 0], [2, 0], [2, 5], [4, 0], [5, 0], [6, 0]])
    frame = until(s, final_frame(off["state_rev"]))[-1]
    for what, at, _shown, _every, opened in SNAP_CASES:
        got = snap(s, frame, at)
        check(got == (opened, True), "the solid square off, a snap at %s %r: %r"
              % (what, at, got))
    s.close()
    until(s, lambda kind, v: kind == "closed", 30)
    # FLOE_PICK_VISIBLE=off: every shape over the point, the smallest first;
    # FLOE_SNAP_VISIBLE=off: every corner and edge
    os.environ["FLOE_PICK_VISIBLE"] = os.environ["FLOE_SNAP_VISIBLE"] = "off"
    every = gtkservice.Service(FLOE2)
    try:
        t = gtkservice.ViewSession(src, W, H, patch={"detail": "high"}, svc=every)
        until(t, final_frame(1))
        frame = view(t, 20.0, style_deltas=PICK_FILLS)
        picks, again = cycle(t, frame, (20.0, 20.0))
        check(said(picks, again) == ([4, 3, 5, 1, 2], 0)
              and all(a["request"]["seen"] is False for a in picks),
              "every shape over the point: %r" % (said(picks, again),))
        for what, at, _shown, all_shapes, _open in SNAP_CASES:
            got = snap(t, frame, at)
            check(got == (all_shapes, False), "every shape, a snap at %s %r: %r"
                  % (what, at, got))
        t.close()
    finally:
        os.environ.pop("FLOE_PICK_VISIBLE", None)
        os.environ.pop("FLOE_SNAP_VISIBLE", None)
        every.close()
    print("pick order: what shows at %d points, the smallest first; the rest counted; "
          "snaps at %d points pass by what is covered" % (len(PICK_CASES) + 7, len(SNAP_CASES)))


def pick_order_ui(src):
    """pick_order in the real viewer: a click picks the smallest shape that
    shows under the pointer, the same spot the next one and round again;
    what is under other layers is not picked; where shapes are and none
    shows the status line says so."""
    from types import SimpleNamespace
    from floe import gui
    cache = gtkservice.ServiceCache(str(src))
    cache.load()
    v = gui.Viewer(cache, None)
    frames, answers = [], []
    take, took = v._ctl_frame, v._on_pick_result
    v._ctl_frame = lambda f: (frames.append(f), take(f))
    v._on_pick_result = lambda res: (answers.append(res), took(res))[1]

    def shown(step, what):
        # (a pick is asked of the frame on the screen: the view's own, final)
        rev = v.worker.snapshot["render_rev"]
        step()
        pump(v, lambda: settled(v, frames) and v.worker.snapshot["render_rev"] != rev, what)

    def click(x_um, y_um):
        """A plain click at a point of the layout: the layer picked (None:
        nothing) and the status line."""
        box = v.view_bbox()
        del answers[:]
        v._pick_click(SimpleNamespace(x=(x_um * 1000 - box[0]) / v.spp,
                                      y=(box[3] - y_um * 1000) / v.spp,
                                      state=gui.Gdk.ModifierType(0)))
        try:
            pump(v, lambda: bool(answers), "a pick's answer", 10)
        except AssertionError:
            raise AssertionError("no pick answer at (%s, %s): status %r, view %r, shown %r" % (
                x_um, y_um, v.status.get_text(), box, v.worker.shown))
        sel = v.selection
        return (None if sel is None else sel["layer"]), v.status.get_text()
    try:
        pump(v, lambda: settled(v, frames), "the pick layout's first frame")
        shown(lambda: v.worker.edit(style_deltas=PICK_FILLS), "the fills")
        shown(lambda: v.goto(20.0, 20.0, 20.0), "the 20 um view")
        seen = [click(20.0, 20.0) for _ in range(3)]
        check([layer for layer, _ in seen] == [4, 3, 4]
              and [said.rsplit(" · ", 1)[-1] for _, said in seen] == ["1/2", "2/2", "1/2"]
              and "2.000 x 2.000 um" in seen[0][1] and "6.000 x 6.000 um" in seen[1][1],
              "clicks at the middle: %r" % seen)
        # another spot starts anew: the solid square alone, the plate alone
        beside = [click(22.0, 20.0), click(27.0, 20.0)]
        check([layer for layer, _ in beside] == [3, 2]
              and all(said.endswith(" · 1/1") for _, said in beside),
              "clicks beside it: %r" % beside)
        # shapes there, none of them to be seen: the selection goes and the
        # line says how many; nothing there at all: the plain words
        shown(lambda: v.goto(30.0, 20.0, 60.0), "the 60 um view")
        layer, said = click(45.0, 20.0)
        check(layer is None and said == "no visible object here (1 hidden)",
              "a click inside an outline alone: %r %r" % (layer, said))
        layer, said = click(40.9, 20.0)
        check(layer is None and said == "no object here",
              "a click on nothing: %r %r" % (layer, said))
        # the ruler's snap passes by a covered corner and takes one that shows
        shown(lambda: v.goto(20.0, 20.0, 20.0), "the 20 um view again")
        heard = []
        handle = v._handle_result
        v._handle_result = lambda res: (heard.append(res), handle(res))[1]
        v.mode = "ruler"

        def hover(x_um, y_um):
            v._cursor = (x_um * 1000, y_um * 1000)
            v._snap_sent = 0.0
            del heard[:]
            v._request_snap()
            pump(v, lambda: any(a.get("kind") in ("snap", "error") for a in heard),
                 "a snap's answer", 30)
            res = v._snap_res
            return None if res is None else (res["snap"], res["x"], res["y"])
        snaps = [hover(21.6, 21.6), hover(22.9, 22.9)]
        check(snaps == [None, ("vertex", 23000, 23000)],
              "the ruler's snap at a covered corner and at one that shows: %r" % snaps)
        v.mode = "normal"
        v._snap_res = None
        print("pick clicks: %s, then %s" % (
            ", ".join(said.rsplit(" · ", 1)[-1] for _, said in seen),
            "no visible object here (1 hidden)"))
    finally:
        v._quit()
        v.window.destroy()
        cache.close()


def queries_and_cells(s, ref, frame, temp):
    """snap, pick, the cell tree and a clip through the channel = the
    viewer's adapter's answers for the same request."""
    for n, (kind, extra) in enumerate((("snap", {}), ("pick", {"nth": 0}))):
        qid = s.query(frame["id"], kind, W / 2, H / 2, 10 if kind == "snap" else 3, **extra)
        got = answer(s, "query", lambda v, qid=qid: v["id"] == qid)
        req = got["request"]
        job = {"kind": kind, "seq": 900 + n, "x": req["x"], "y": req["y"], "r": req["r"]}
        # a query is of what shows (a pick 0.12.456, a snap 0.12.457): the
        # adapter asks its own frame of the same view the same
        check(req.get("seen") is True, "%s's request: %r" % (kind, req))
        job["seen"] = True
        job.update(extra)
        want = reference_answer(ref, job, kind)
        keys = ("found", "x", "y", "snap") if kind == "snap" else (
            "found", "count", "index", "layer", "datatype", "lname", "cell",
            "area", "bbox", "points", "hidden")
        for key in keys:
            a, b = got.get(key), want.get(key)
            if key == "points":
                a, b = [list(p) for p in a or []], [list(p) for p in b or []]
            check(a == b, "%s %s: channel %r, adapter %r" % (kind, key, a, b))
        check(got["found"], "%s at the view's centre found nothing" % kind)
    overlay_snaps(s, frame)
    for n, (kind, fields) in enumerate((
            ("cell_sources", {}), ("cells", {"src": 0}),
            ("cell_find", {"src": -1, "pattern": "", "limit": 50}))):
        s.cells(kind, 700 + n, **fields)
        got = answer(s, "cells", lambda v, n=n: v["seq"] == 700 + n)
        want = reference_answer(ref, dict(fields, kind=kind, seq=800 + n), kind)
        got = dict(got, seq=None)
        want = dict(want, seq=None)
        if kind == "cell_sources":
            # the channel's worker opened the same cache under another name
            for rows in (got, want):
                for row in rows.get("sources") or []:
                    row["path"] = os.path.realpath(row["path"]) if row["path"] else ""
        check(got == want, "%s: channel %r, adapter %r" % (kind, got, want))
    bbox = [int(v) for v in frame["bbox"]]
    s.clip(600, bbox, temp / "channel.oas")
    got = answer(s, "clip", lambda v: v.get("seq") == 600, 120)
    check(got["kind"] == "clip", "clip: %s" % got)
    want = reference_answer(ref, {"kind": "clip", "seq": -1, "bbox": bbox,
                                  "out": str(temp / "adapter.oas")}, "clip")
    a, b = (temp / "channel.oas").read_bytes(), (temp / "adapter.oas").read_bytes()
    check(a == b, "clip: the channel's OASIS (%d B) differs from the adapter's (%d B)"
          % (len(a), len(b)))


class Event:
    """A stand-in GDK event (the attributes the handlers read)."""

    def __init__(self, **fields):
        self.__dict__.update(fields)


def band(v, start, end):
    """A right-button band from `start` to `end` (viewport px), released."""
    v._zoomdrag = start
    v._band_cur = end
    v._band_ext = (min(start[0], end[0]), max(start[0], end[0]))
    v._on_release(None, Event(button=3, x=end[0], y=end[1]))


def minimap_click(v, fx, fy):
    """A click on the minimap at a fraction of the die's box there."""
    from floe import gui
    _scale, x0, y0, mw, mh = v._minimap_geom()
    off = v._minimap_image.translate_coordinates(v._minimap_event, 0, 0)
    ox, oy = off if off else (0, 0)
    v._on_minimap_click(None, Event(
        type=gui.Gdk.EventType.BUTTON_PRESS, button=1,
        x=x0 + mw * fx + ox, y=y0 + mh * fy + oy))


def navigation_parity(v):
    """P4e: every navigation of the viewer is the view controller's (the
    shared Viewport::navigate) and lands where the viewer's own maths - the
    Python loop's, kept as its fallback while the view opens - put it."""
    from floe import gui
    w, h = v._viewport_size()
    bb = v._die_bbox()
    um = lambda fx, fy: ((bb[0] + fx * (bb[2] - bb[0])) * v.dbu,
                         (bb[1] + fy * (bb[3] - bb[1])) * v.dbu)
    ops = (
        ("Ctrl+Z", lambda: v._zoom_center(gui.CAL_ZOOM_IN)),
        ("the wheel in at a point",
         lambda: v._zoom_at(w * 0.3, h * 0.7, gui.WHEEL_ZOOM_STEP)),
        ("the wheel out at a point",
         lambda: v._zoom_at(w * 0.8, h * 0.2, 1 / gui.WHEEL_ZOOM_STEP)),
        ("Ctrl+Z again", lambda: v._zoom_center(gui.CAL_ZOOM_IN)),
        ("an arrow right", lambda: v._pan_view("Right")),
        ("a fine arrow up",
         lambda: v._pan_view("Up", gui.KEY_PAN_FRACTION_FINE)),
        ("a band in", lambda: band(v, (0.2 * w, 0.3 * h), (0.5 * w, 0.45 * h))),
        ("a thin band", lambda: band(v, (0.1 * w, 0.5 * h), (0.7 * w, 0.5 * h + 2))),
        ("a band out", lambda: band(v, (0.6 * w, 0.5 * h), (0.4 * w, 0.6 * h))),
        ("a minimap click", lambda: minimap_click(v, 0.3, 0.6)),
        ("goto with a window", lambda: v.goto(*um(0.4, 0.55),
                                              (bb[2] - bb[0]) * v.dbu * 0.05)),
        ("goto at the zoom", lambda: v.goto(*um(0.6, 0.45))),
        ("fit", v.fit),
    )
    for what, op in ops:
        before = (v.cx, v.cy, v.spp)
        # the viewer's own maths, not drawn
        v._ctl_navigate = lambda nav: False
        v.redraw = lambda **kw: None
        try:
            op()
        finally:
            del v._ctl_navigate
            del v.redraw
        v._clamp_view()
        want = (v.cx, v.cy, v.spp)
        v.cx, v.cy, v.spp = before
        rev = v.worker.snapshot["state_rev"]
        op()
        got = (v.cx, v.cy, v.spp)
        check(v.worker.snapshot["state_rev"] != rev,
              "%s did not reach the controller" % what)
        check(abs(got[2] / want[2] - 1) < 1e-9
              and abs(got[0] - want[0]) < 1e-4 * want[2]
              and abs(got[1] - want[1]) < 1e-4 * want[2],
              "%s: the controller's view %r, the viewer's maths %r"
              % (what, got, want))
        check(v._ctl_same_view(v.worker.snapshot["state"]["viewport"]),
              "%s: the viewer did not take the controller's view" % what)
    print("navigation: %d moves as the viewer's maths" % len(ops))


def minimap_parity(v):
    """P4e: the minimap's bases and the die's place come from the service
    (app-core view::minimap) and are the viewer's own bake byte for byte."""
    depths = [None] + list(range(len(v._frontier_depths)))
    v._minimap_bases = {}
    geom = v._minimap_geom()
    check(("ctl", None) in v._minimap_bases, "the minimap did not ask the service")
    ours = {d: bytes(v._minimap_base(d).get_pixels()) for d in depths}
    v._ctl_minimap = lambda d: None
    try:
        v._minimap_bases = {}
        mine = v._minimap_geom()
        check(geom[1:] == mine[1:] and abs(geom[0] / mine[0] - 1) < 1e-12,
              "minimap geometry: service %r, viewer %r" % (geom, mine))
        for d in depths:
            check(ours[d] == bytes(v._minimap_base(d).get_pixels()),
                  "minimap base at depth %s differs from the viewer's bake" % d)
    finally:
        del v._ctl_minimap
        v._minimap_bases = {}
    print("minimap: %d base(s) as the viewer's bake" % len(depths))


def border_parity(v):
    """The scale border (user 2026-10-13; app-core view::border): the band
    along the frame's four edges is the service's - its segments alternate
    the two tones at the service's px, each a 1-2-5 length of 60 px or
    more, their edges the world's round coordinates (moved by a pan as the
    layout is), the rule between band and layout -, the corners' chips say
    the viewport's corners in um to the service's decimals and the top one
    the length; the same view is asked once; off (`s`) the frame shows
    where the band was, and under Tab's clean look nothing of it shows."""
    from floe import gui
    pump(v, lambda: settled(v), "a settled view")
    v._display()
    border = v._border_shown
    check(border is not None, "the viewer drew no scale border")
    w, h = v._viewport_size()
    vb = v.view_bbox()
    um_px = v.spp * v.dbu
    step = border["step_um"]
    exponent = math.floor(math.log10(step))
    mantissa = step / 10 ** exponent
    check(any(abs(mantissa - m) < 1e-9 for m in (1, 2, 5)),
          "the step %r um is not 1, 2 or 5 x 10^n" % step)
    check(60 <= border["step_px"] <= 150 + 1e-9
          and abs(border["step_px"] - step / um_px) < 1e-6,
          "the step %r um is %r px at %r um/px" % (step, border["step_px"], um_px))
    check(border["label"] == "%.*f um" % (max(0, -exponent), step),
          "the label %r is not the step %r" % (border["label"], step))
    for axis, segs, length, edge_um in (
            ("x", border["x"], w, lambda s: (vb[0] + s["from"] * v.spp) * v.dbu),
            ("y", border["y"], h, lambda s: (vb[3] - s["from"] * v.spp) * v.dbu)):
        check(segs and segs[0]["from"] == 0 and abs(segs[-1]["to"] - length) < 1e-9,
              "%s: the band does not span the frame: %s" % (axis, segs))
        for a, b in zip(segs, segs[1:]):
            check(abs(a["to"] - b["from"]) < 1e-9 and a["even"] != b["even"],
                  "%s: segments %s and %s" % (axis, a, b))
        for s in segs[1:-1]:
            check(abs(s["to"] - s["from"] - border["step_px"]) < 1e-6,
                  "%s: an inner segment %s is not the step" % (axis, s))
        for s in segs[1:]:
            # each inner edge is a multiple of the step from the origin; the
            # ordinal counts the segment on the origin's side of the edge
            k = edge_um(s) / step
            check(abs(k - round(k)) < 1e-6, "%s: the edge at %r um is not a "
                  "multiple of %r" % (axis, edge_um(s), step))
            ordinal = round(k) if axis == "x" else round(k) - 1
            check((ordinal % 2 == 0) == s["even"],
                  "%s: the segment from %r um has the wrong tone" % (axis, edge_um(s)))
    # the pixels: the tones at the segments' middles, the rule, the corners
    disp = v.image.get_pixbuf()
    pixels, stride, channels = bytes(disp.get_pixels()), disp.get_rowstride(), disp.get_n_channels()

    def at(x, y):
        i = int(y) * stride + int(x) * channels
        return pixels[i:i + 3]

    def rgb(c):
        return bytes(((c >> 24) & 255, (c >> 16) & 255, (c >> 8) & 255))

    band = gui.BORDER_PX
    for s in border["x"]:
        mid = (s["from"] + s["to"]) // 2
        if band <= mid < w - band and s["to"] - s["from"] >= 3:
            tone = rgb(gui.BORDER_EVEN if s["even"] else gui.BORDER_ODD)
            check(at(mid, 2) == tone and at(mid, h - 3) == tone,
                  "the top/bottom band at x %d is not its tone" % mid)
    for s in border["y"]:
        mid = (s["from"] + s["to"]) // 2
        if band <= mid < h - band and s["to"] - s["from"] >= 3:
            tone = rgb(gui.BORDER_EVEN if s["even"] else gui.BORDER_ODD)
            check(at(2, mid) == tone and at(w - 3, mid) == tone,
                  "the left/right band at y %d is not its tone" % mid)
    rule = rgb(gui.BORDER_RULE)
    check(at(w // 2, band - 1) == rule and at(w // 2, h - band) == rule
          and at(band - 1, h // 2) == rule and at(w - band, h // 2) == rule,
          "the rule between the band and the layout is missing")
    check(at(2, 2) == rgb(gui.BORDER_ODD) and at(w - 3, h - 3) == rgb(gui.BORDER_ODD),
          "the corner squares are not filled")
    # the chips: the service's text, the viewport's corners in um
    labels = v._border_labels
    decimals = border["decimals"]
    check(3 <= decimals <= 5, "decimals %r" % decimals)
    for key, (cx, cy) in (("top_left", (vb[0], vb[3])), ("top_right", (vb[2], vb[3])),
                          ("bottom_left", (vb[0], vb[1])), ("bottom_right", (vb[2], vb[1]))):
        corner = border[key]
        check(labels[key].get_visible() and labels[key].get_text()
              == " %s, %s " % (corner["x"], corner["y"]),
              "the %s chip says %r, the service %r" % (key, labels[key].get_text(), corner))
        for shown, world in ((corner["x"], cx), (corner["y"], cy)):
            check(len(shown.split(".")[1]) == decimals
                  and abs(float(shown) - world * v.dbu) <= 0.5 * 10 ** -decimals + 1e-9,
                  "%s: %r is not %r um" % (key, shown, world * v.dbu))
    check(labels["scale"].get_visible()
          and labels["scale"].get_text() == " scale %s " % border["label"],
          "the scale chip says %r" % labels["scale"].get_text())
    asked = []
    original = v.worker.border

    def counted(*args):
        asked.append(args)
        return original(*args)

    v.worker.border = counted
    try:
        v._display()
        check(not asked, "the same view asked the service for its border again")
        # a pan moves the band with the layout: every inner edge still on
        # screen moved by the pan's px
        edges = [s["from"] for s in border["x"][1:]]
        v._pan_view("Right", 0.1)
        pump(v, lambda: settled(v) and v.view_bbox()[0] != vb[0], "a pan")
        v._display()
        moved = v._border_shown
        shift = (v.view_bbox()[0] - vb[0]) / v.spp
        check(shift > 0 and asked, "the pan moved %r px; asked %d" % (shift, len(asked)))
        now = [s["from"] for s in moved["x"][1:]]
        for e in edges:
            if 0 < e - shift < w:
                check(any(abs(e - shift - f) < 1e-6 for f in now),
                      "the edge at %r px did not move to %r px with the pan" % (e, e - shift))
    finally:
        v.worker.border = original
    # off: the frame's own pixels where the band was, no chips
    frame = v.last_frame[0]
    v._toggle_border()
    check(not v.border_on and v._border_shown is None
          and not any(l.get_visible() for l in labels.values()),
          "the border did not go with `s`")
    disp = v.image.get_pixbuf()
    fpx, fstride, fch = bytes(frame.get_pixels()), frame.get_rowstride(), frame.get_n_channels()
    for x, y in ((w // 2, 2), (2, h // 2), (w - 3, h - 3)):
        shown = bytes(disp.get_pixels())[y * disp.get_rowstride() + x * disp.get_n_channels():][:3]
        own = fpx[y * fstride + x * fch:][:3]
        check(shown == own, "off: the pixel at %d,%d is %r, the frame's %r" % (x, y, shown, own))
    v._toggle_border()
    check(v.border_on and v._border_shown is not None and labels["scale"].get_visible(),
          "the border did not come back")
    # Tab's clean look hides it, Tab again shows it
    v._toggle_overlays()
    v._toggle_overlays()
    check(v.overlay_mode == 2 and v._border_shown is None
          and not labels["top_left"].get_visible(), "the border survived Tab's clean look")
    v._toggle_overlays()
    check(v.overlay_mode == 0 and v._border_shown is not None, "the border did not return")
    print("scale border: %d + %d segments of %s, the corners' chips, a pan, off, Tab"
          % (len(border["x"]), len(border["y"]), border["label"]))


def error_shape_snaps(v, answers, x, y, scene):
    """The ruler's snap takes the error shape drawn over the frame (0.12.454):
    the jumped error's edges - the viewer's own drawing, no part of the
    layout a snap is answered from - go with the snap's question where they
    are drawn as the shape, and the answer is the one snap of the two
    (floe_oracle/overlay_snap.py). `scene`: the layout's own answer at the
    view's centre (x, y), just asked."""
    from floe import gui
    X, Y = int(x), int(y)
    r = max(1, int(10 * v.spp))
    # the whole dbu and the reach the controller asks about for this pixel
    RX, RY, reach = (scene["request"][k] for k in ("x", "y", "r"))
    check(r >= 4 and abs(RX - X) <= 1 and abs(RY - Y) <= 1,
          "the fixture's snap: reach %d dbu, asked at (%d, %d) for (%d, %d)" % (r, RX, RY, X, Y))
    check(v.drc_mark is None and v._snap_shapes(x, y, r) == [],
          "no error jumped to: nothing drawn to snap to")
    sent = []
    submit = v.worker.submit
    v.worker.submit = lambda job: (sent.append(job), submit(job))[1]

    def ask(expect_sent=True):
        """One hover at the centre: the job sent and its answer."""
        v._cursor = (x, y)
        v._snap_sent = 0.0
        del sent[:], answers[:]
        seq = v._snap_seq
        v._request_snap()
        if not expect_sent:
            return (sent[:], seq)
        check(len(sent) == 1 and v._snap_seq == seq + 1, "a snap asked: %r" % sent)
        pump(v, lambda: any(a.get("kind") in ("snap", "error") for a in answers),
             "snap answer", 60)
        return sent[0], [a for a in answers if a.get("kind") in ("snap", "error")][-1]

    def same(got, want):
        if got.get("kind") != "snap":
            return False
        if not want["found"]:
            return got.get("found") is False and "overlay" not in got
        return all(got.get(k) == want.get(k)
                   for k in ("found", "x", "y", "snap", "overlay"))
    try:
        # an edge pair: one edge ends beside the cursor, the other is away
        near = (X - 2 * r, Y + 1.5, X + 0.25, Y + 1.5)
        v.drc_mark = {"kind": "e", "pts": [near[:2], near[2:], (X + 3 * r, Y - 3 * r),
                                           (X + 3 * r, Y + 3 * r)]}
        check(v._snap_shapes(x, y, r) == [near],
              "the error's edges in reach of the snap: %r" % v._snap_shapes(x, y, r))
        job, got = ask()
        want = overlay_snap.snap([near], RX, RY, reach, scene)
        check(job.get("overlay") == [near] and "layout" not in job and same(got, want)
              and got.get("overlay") and (got["x"], got["y"]) == (X + 0.25, Y + 1.5)
              and v._snap_res is got and v._cursor_snapped() == (X + 0.25, Y + 1.5),
              "the snap lands on the error edge's end: job %r, answer %r, reference %r"
              % (job, got, want))
        # a polygon: its outline, the closing edge with it
        tip, a, b = (X + 0.25, Y + 1.5), (X + 3 * r, Y + 3 * r), (X - 3 * r, Y + 3 * r)
        v.drc_mark = {"kind": "p", "pts": [tip, a, b]}
        shapes = v._snap_shapes(x, y, r)
        check(shapes == [tip + a, b + tip], "a polygon's edges in reach: %r" % shapes)
        job, got = ask()
        check(job.get("overlay") == shapes and same(got, overlay_snap.snap(shapes, RX, RY, reach, scene)),
              "the snap on a polygon's corner: %r" % got)
        # too far out for the layout's geometry: the shape alone is asked
        tiles = v.tiles_spanned
        v.tiles_spanned = lambda box: 99
        try:
            job, got = ask()
            check(job.get("layout") is False and job.get("overlay") == shapes
                  and same(got, overlay_snap.snap(shapes, RX, RY, reach)),
                  "too far out: the error's shape alone: job %r, answer %r" % (job, got))
            # ... and with nothing drawn in reach nothing is asked, and no
            # snap is left behind
            v.drc_mark = None
            check(v._snap_res is not None, "a snap is held")
            jobs, seq = ask(expect_sent=False)
            check(jobs == [] and v._snap_res is None and v._snap_seq == seq + 1,
                  "too far out and nothing drawn: jobs %r, held %r" % (jobs, v._snap_res))
        finally:
            v.tiles_spanned = tiles
        # an error drawn as its marker circle is no shape to snap to; nor
        # is one hidden with the overlays (Tab)
        v.drc_mark = {"kind": "e", "pts": [(X, Y), (X + (gui.DRC_MARK_PX - 1) * v.spp, Y)]}
        check(v._snap_shapes(x, y, r) == [], "an error drawn as its marker is not offered")
        v.drc_mark = {"kind": "e", "pts": [(X, Y), (X + (gui.DRC_MARK_PX + 1) * v.spp, Y)]}
        check(len(v._snap_shapes(x, y, r)) == 1, "one drawn as its shape is")
        v.overlay_mode = 2
        check(v._snap_shapes(x, y, r) == [], "overlays off: nothing drawn to snap to")
        job, got = ask()
        check("overlay" not in job and same(got, scene),
              "overlays off: the layout's answer as it was: %r" % got)
        v.overlay_mode = 0
    finally:
        v.worker.submit = submit
        v.drc_mark = None
        v.overlay_mode = 0
        v._snap_res = None


def cell_status(v, answers):
    """User 2026-10-15 ("셀 트리에서 셀을 선택하면 셀의 관련정보(좌표등)을
    상태줄에 표시해줘"): a cell selected in the tree says the service's
    cell_info text on the status line - a cell placed once its origin and
    box, one placed 2.25 million times its count and extent -, the cursor's move
    puts its coordinates in front and keeps it, Esc takes it away."""
    v.mode = "normal"   # the snap's ruler mode before
    w = v._cellwin
    store = w._store

    def children():
        top = store.get_iter_first()
        if top is None:
            return {}
        out, it = {}, store.iter_children(top)
        while it is not None:
            if store[it][5] == "cell":
                out[store[it][0]] = it
            it = store.iter_next(it)
        return out

    pump(v, lambda: {"DEEPA", "MEGAFILL"} <= set(children()), "the cell tree", 60)
    for name, says in (("DEEPA", "DEEPA · 1 placement at ("),
                       ("MEGAFILL", "MEGAFILL · 2,250,000 placements · extent (")):
        started = time.monotonic()
        w._tree.get_selection().select_iter(children()[name])
        pump(v, lambda: any(a.get("kind") == "cell_info" and a.get("name") == name
                            for a in answers), "%s's facts" % name, 60)
        took = time.monotonic() - started
        got = [a for a in answers if a.get("kind") == "cell_info"][-1]
        check(got["found"] and got["text"].startswith(says),
              "%s: cell_info said %r" % (name, got.get("text")))
        check(v.status.get_text().startswith(got["text"]),
              "%s: the status line %r, the service %r" % (name, v.status.get_text(), got["text"]))
        print("cell status (%.2f s): %s" % (took, v.status.get_text()))
    # the cursor's coordinates in front, the cell kept
    v._cursor = (0, 0)
    v._hover(Event(state=0))
    check(v.status.get_text().startswith("x 0.000  y 0.000 um   |   " + got["text"]),
          "the cursor's line %r" % v.status.get_text())
    v._cell_hl_clear(display=False)
    v._hover(Event(state=0))
    check(v.status.get_text() == "x 0.000  y 0.000 um",
          "Esc left %r on the status line" % v.status.get_text())


def browse_dialog(v):
    """User 2026-10-26 ("필터가 .jb 혹은 다른 것일때는 보이지 않아야 하며 해당할
    경우에도 @file 혹은 file@ 형태로 보이면 좋겠음"): the GTK dialog of
    gui.py `_browse_file_dialog` - under the .jb filter the links named
    *.jb alone, as `name@` among the files by name, `name -> target` the
    tooltip (markup-escaped: an & in a name); under all files every link,
    the one to a folder too; activating that one enters the folder, a
    link to a deck is picked as the link's path. The dialog's run() is
    the gate's while it looks (the rows are the store's)."""
    from floe import gui
    Gtk = gui.Gtk
    with tempfile.TemporaryDirectory(prefix="floe-browse-") as d:
        os.mkdir(os.path.join(d, "Sub"))
        for name in ("deck.jb", "a.oas", os.path.join("Sub", "inner.jb")):
            with open(os.path.join(d, name), "w") as fh:
                fh.write("x")
        for name, target in (("linked.jb", "deck.jb"), ("R&D.jb", "deck.jb"),
                             ("current", "deck.jb"), ("to_a.oas", "a.oas"),
                             ("to_sub", "Sub")):
            os.symlink(target, os.path.join(d, name))
        filters = [("jobdecks (*.jb)", ("*.jb",)), ("all files", ("*",))]
        seen = []

        def parts(dlg):
            kids = dlg.get_content_area().get_children()
            view = [k for k in kids if isinstance(k, Gtk.ScrolledWindow)][0]
            combo = [k for k in kids if isinstance(k, Gtk.ComboBoxText)][0]
            return view.get_child(), combo

        def rows(view):
            tip = view.get_tooltip_column()
            return [(r[0], r[tip]) for r in view.get_model()]

        def activate(view, display):
            for i, r in enumerate(view.get_model()):
                if r[0] == display:
                    path = Gtk.TreePath.new_from_indices([i])
                    view.get_selection().select_path(path)
                    view.row_activated(path, view.get_column(0))
                    return
            check(False, "no row %r in %s" % (display, rows(view)))

        def look(dlg):
            view, combo = parts(dlg)
            seen.append(rows(view))
            combo.set_active(1)
            seen.append(rows(view))
            activate(view, "to_sub@")
            seen.append(rows(view))
            return Gtk.ResponseType.CANCEL

        def pick(dlg):
            activate(parts(dlg)[0], "linked.jb@")
            return Gtk.ResponseType.OK

        run = Gtk.Dialog.run
        try:
            Gtk.Dialog.run = look
            none = v._browse_file_dialog("browse", d, filters)
            Gtk.Dialog.run = pick
            picked = v._browse_file_dialog("browse", d, filters)
        finally:
            Gtk.Dialog.run = run
        check(none is None and len(seen) == 3, "the dialog's runs: %r %r" % (none, seen))
        check(seen[0] == [("Sub/", "Sub/"), ("deck.jb", "deck.jb"),
                          ("linked.jb@", "linked.jb -&gt; deck.jb"),
                          ("R&D.jb@", "R&amp;D.jb -&gt; deck.jb")],
              "the .jb filter's rows: %s" % seen[0])
        check([r[0] for r in seen[1]] == ["Sub/", "a.oas", "current@", "deck.jb",
                                          "linked.jb@", "R&D.jb@", "to_a.oas@",
                                          "to_sub@"],
              "all files' rows: %s" % seen[1])
        check(seen[2] == [("inner.jb", "inner.jb")],
              "the folder behind to_sub@: %s" % seen[2])
        check(picked == os.path.join(d, "linked.jb"), "picked %r" % picked)
    print("file dialog: %d rows under *.jb, %d under all files, a folder link "
          "entered, a deck link picked" % (len(seen[0]), len(seen[1])))


def levels_dialog(v):
    """User 2026-10-26 ("jobdeck을 로드할 때 레벨 선택 다이얼로그에서 레벨이 $1 $2로
    표시되는데 이 부분을 level 1, level 2로 수정해줘"): the GTK dialog of gui.py
    `_jobdeck_levels_dialog` labels a mask level `level N`, the deck's
    MTITLE after it when there is one; the levels left checked are the
    answer. The dialog's run() is the gate's while it looks."""
    from floe import gui
    Gtk = gui.Gtk
    rows = [{"level": 1, "name": "", "chips": ["ID001"], "instances": 2,
             "sources": ["/data/a.oas"]},
            {"level": 2, "name": "METAL1", "chips": ["ID001", "ID002"],
             "instances": 3, "sources": ["/data/a.oas", "/data/b.oas"]},
            {"level": 13, "name": "VIA 1", "chips": ["ID002"], "instances": 1,
             "sources": ["/data/b.oas"]}]
    labels = []

    def look(dlg):
        kids = dlg.get_content_area().get_children()
        grid = [k for k in kids if isinstance(k, Gtk.ScrolledWindow)][0].get_child()
        if not isinstance(grid, Gtk.Grid):
            grid = grid.get_child()        # the viewport GTK puts between
        checks = [grid.get_child_at(0, r) for r in range(len(rows))]
        labels.extend(c.get_label() for c in checks)
        checks[1].set_active(False)
        return Gtk.ResponseType.OK

    run = Gtk.Dialog.run
    try:
        Gtk.Dialog.run = look
        picked = v._jobdeck_levels_dialog("/data/deck.jb", rows, None)
    finally:
        Gtk.Dialog.run = run
    check(labels == ["level 1", "level 2 METAL1", "level 13 VIA 1"],
          "the level dialog's labels: %s" % labels)
    check(picked == [1, 13], "the levels left checked: %r" % (picked,))
    print("level dialog: %s" % ", ".join(labels))


def own_cut(v, text):
    """The `d` dialog's own cut (user 2026-10-28: "테스트를 위해 detail cut을
    입력받을 수 있게 수정해줘. 0.1px까지 테스트해보려고 함"): `text` typed in
    its entry and Enter. The level buttons and the refusal line after it."""
    from floe import gui
    Gtk = gui.Gtk
    v._detail_dialog()

    def walk(w):
        yield w
        if isinstance(w, Gtk.Container):
            for k in w.get_children():
                for d in walk(k):
                    yield d
    kids = list(walk(v._cdlg))
    named = dict((k.get_name(), k) for k in kids)
    named["detail-cut"].set_text(text)
    named["detail-cut"].emit("activate")
    levels = [k for k in kids if isinstance(k, Gtk.ToggleButton)]
    return levels, named["detail-cut-why"].get_text()


def gtk_ready():
    """Whether the viewer's windows can open here (GuiSmokeTests' rule)."""
    try:
        import gi
        gi.require_version("Gtk", "3.0")
        from gi.repository import Gtk  # noqa: F401
    except (ImportError, ValueError):
        print("viewer: skipped (PyGObject/GTK is not importable)")
        return False
    if sys.platform.startswith("linux") and not os.environ.get(
            "DISPLAY") and not os.environ.get("WAYLAND_DISPLAY"):
        print("viewer: skipped (no display)")
        return False
    return True


def pump(viewer, want, what, deadline_s=120):
    """The viewer's main loop until `want()`."""
    from floe import gui
    ctx = gui.GLib.MainContext.default()
    deadline = time.monotonic() + deadline_s
    while time.monotonic() < deadline:
        while ctx.iteration(False):
            pass
        if want():
            return
        time.sleep(0.005)
    raise AssertionError("viewer: no %s in %d s" % (what, deadline_s))


def shown(viewer):
    """The viewer's frame on screen: (RGBA bytes, the frame's dict)."""
    pix, fb, _fspp, _key = viewer.last_frame
    return bytes(pix.get_pixels()), {
        "bbox": list(fb), "width": pix.get_width(), "height": pix.get_height()}


def settled(viewer, frames=None):
    """The controller is idle at the viewer's view, its final frame of
    that state shown (when `frames` are the foreground ones seen)."""
    snap = viewer.worker.snapshot
    if (viewer.last_frame is None or snap is None or snap["phase"] != "idle"
            or viewer._pending is not None
            or getattr(viewer, "_refining", False)
            or not viewer._ctl_same_view(snap["state"]["viewport"])):
        return False
    if frames is None:
        return True
    fg = [f for f in frames if f["purpose"] == "foreground"]
    return bool(fg) and fg[-1]["render_rev"] == snap["render_rev"] \
        and fg[-1]["final"]


class NoCairo:
    """pycairo unimportable, as on the target hosts (floe/gui.py's module
    docstring): a GTK "draw" handler gets no cairo context there and its
    widget stays blank (field 2026-10-10)."""

    def find_spec(self, name, path=None, target=None):
        if name in ("cairo", "gi._gi_cairo") or name.startswith("cairo."):
            raise ImportError("no pycairo on the viewer's hosts: %s" % name)
        return None


def strike_and_palettes(v):
    """A hidden layer's row shows the strike - one line across the row -
    and the colour and fill palettes have their images, without pycairo."""
    from floe import gui
    key = sorted(v._layer_rows)[0]
    row = v._layer_rows[key]
    row.set_active(False)
    pump(v, lambda: row._strike.get_visible() and
         row._strike.get_pixbuf() is not None, "the strike", 30)
    pb = row._strike.get_pixbuf()
    alloc = row.widget.get_allocation()
    y = max(0, (alloc.height - row._row_pad) // 2)
    # an opaque 1 px line the row's width at the line's height: nothing
    # transparent over the row (an overlay child's own window may paint
    # an opaque background - a row-sized image hid the row, 2026-10-10)
    check(not pb.get_has_alpha() and (pb.get_width(), pb.get_height())
          == (alloc.width, 1) and bytes(pb.get_pixels())[:3 * alloc.width]
          == bytes(gui.LAYER_STRIKE_RGB) * alloc.width,
          "the strike is not one opaque line the row's width (%dx%d, alpha %s)"
          % (pb.get_width(), pb.get_height(), pb.get_has_alpha()))
    check(row._strike.get_margin_top() == y,
          "the strike sits at %d, not the row's middle %d"
          % (row._strike.get_margin_top(), y))
    salloc = row._strike.get_allocation()
    check(salloc.height == 1, "the strike covers %d px of the row" % salloc.height)
    # an overlay child has a GDK window of its own: it must take no input
    # (the row's clicks; the palette's pick reads the grid's coordinates)
    check(row._strike.get_window().get_pass_through(),
          "the strike's window takes the row's clicks")
    row.set_active(True)
    pump(v, lambda: not row._strike.get_visible(), "the strike gone", 30)
    for cell in v._fill_slots:
        check(cell.image.get_pixbuf() is not None,
              "a fill slot has no image")
        # (the palette tab may not be realized yet: the overlay's word,
        # which its child window takes when it is)
        check(cell.widget.get_overlay_pass_through(cell.image),
              "a fill slot's image takes the palette's clicks")
    print("strike and palettes: drawn without pycairo")


def gui_viewer(src, ref):
    """floe/gui.py's Viewer on the controller's loop (the default)."""
    sys.meta_path.insert(0, NoCairo())
    from floe import gui
    gui.import_gtk()
    cache = gtkservice.ServiceCache(str(src))
    cache.load()
    # the density starts off here so `v` turns it on below (the viewer's
    # own start, on since 2026-10-12, is capture_parity's)
    v = gui.Viewer(cache, detail=2, density=False)
    frames, answers = [], []
    take, handle = v._ctl_frame, v._handle_result
    v._ctl_frame = lambda f: (frames.append(f), take(f))
    v._handle_result = lambda res: (answers.append(res), handle(res))
    try:
        pump(v, lambda: settled(v, frames), "first frame")
        check(v.worker.controller, "the viewer did not take the controller's loop")
        w, h = v._viewport_size()
        on_screen, f = shown(v)
        vb = v.view_bbox()
        check((f["width"], f["height"]) == (w, h) and all(
            abs(a - b) < 1e-6 * v.spp for a, b in zip(f["bbox"], vb)),
            "the frame %s is not the viewer's view %s" % (f, vb))
        same(on_screen, ref.frame(f, density=False, frames=True, depth=v._depth()),
             "the viewer's open")
        # the perf line: the service's = the Python perf_status over the report
        last = [f for f in frames if f["purpose"] == "foreground"][-1]
        report = last["report"]
        note = "" if last.get("depth") is None else ", depth %d" % last["depth"]
        check(tuple(last["perf"]) == perf_status(report, note),
              "perf line: service %r, Python %r" % (last["perf"],
                                                   perf_status(report, note)))
        missing = set(ref.result) - set(report) - {"rgba", "png", "gen"}
        check(not missing, "the report lacks the adapter's keys %s" % sorted(missing))
        check(last["perf"][1] in v.pstatus.get_text(),
              "the lower bar %r is not the frame's %r" % (v.pstatus.get_text(), last["perf"][1]))
        minimap_parity(v)
        border_parity(v)
        navigation_parity(v)
        pump(v, lambda: settled(v, frames), "the fit after the moves")
        steps = (("a zoom", lambda: v._zoom_center(0.25), {}),
                 ("a pan", lambda: v._pan_view("Right"), {}),
                 ("the density", v._toggle_density, {"density": True}),
                 # the occupancy summary toggle (2026-10-20): the state and
                 # the request carry it (this layout has no design.ovo, so
                 # the picture is the page path's either way)
                 ("the occupancy summary", v._toggle_occupancy,
                  {"occupancy": True}),
                 # the `d` dialog's own cut (2026-10-28)
                 ("a cut of one's own", lambda: own_cut(v, "0.1"),
                  {"cut_px": 0.1}),
                 ("the grayscale", lambda: v._set_mono(True, announce=False),
                  {"mono": True}))
        for what, step, policy in steps:
            rev = v.worker.snapshot["render_rev"]
            step()
            pump(v, lambda: settled(v, frames)
                 and v.worker.snapshot["render_rev"] != rev, what)
            on_screen, f = shown(v)
            vb = v.view_bbox()
            check(all(abs(a - b) < 1e-6 * v.spp for a, b in zip(f["bbox"], vb)),
                  "%s: the frame %s is not the view %s" % (what, f["bbox"], vb))
            if policy.get("mono"):
                ref.worker.submit({"kind": "mono", "on": True})
            same(on_screen, ref.frame(f, density=v.density_on, frames=True,
                                      depth=v._depth(),
                                      occupancy=v.occupancy_on,
                                      cut_px=v._effective_cut_px()), what)
            if policy.get("mono"):
                ref.worker.submit({"kind": "mono", "on": False})
            for key, value in policy.items():
                check(v.worker.snapshot["state"][key] == value,
                      "%s: the controller's %s is %r" % (what, key,
                                                        v.worker.snapshot["state"][key]))
        # a snap at the view's centre, answered as the adapter answers it
        v.mode = "ruler"
        v._snap_seq = 41
        x, y = (vb[0] + vb[2]) / 2, (vb[1] + vb[3]) / 2
        r = max(1, int(10 * v.spp))
        v.worker.submit({"kind": "snap", "seq": 41, "x": int(x), "y": int(y), "r": r,
                         "layers": v._layers_arg()})
        pump(v, lambda: any(a.get("kind") in ("snap", "error") for a in answers),
             "snap answer", 60)
        got = [a for a in answers if a.get("kind") in ("snap", "error")][-1]
        want = reference_answer(ref, {"kind": "snap", "seq": 1042, "x": int(x),
                                      "y": int(y), "r": r, "seen": True}, "snap")
        check(got["kind"] == "snap" and got["seq"] == 41 and all(
            got.get(k) == want.get(k) for k in ("found", "x", "y", "snap")),
            "snap: viewer %r, adapter %r" % (got, want))
        error_shape_snaps(v, answers, x, y, got)
        print("viewer: %d frame(s), perf %r" % (len(frames), last["perf"][1]))
        # the own cut: no level lit, the status line says it; a refused
        # entry keeps it; a level button ends it
        levels, why = own_cut(v, "abc")
        check(not any(b.get_active() for b in levels) and v.cut_custom == 0.1
              and why.startswith("not a number")
              and "detail: 0.1px" in v.dstatus.get_text(),
              "the own cut: levels %s, refusal %r, status %r" % (
                  [b.get_active() for b in levels], why, v.dstatus.get_text()))
        levels, why = own_cut(v, "200")
        check(v.cut_custom == 0.1 and "at most 100" in why, "200 px: %r" % why)
        rev = v.worker.snapshot["render_rev"]
        levels[2].set_active(True)
        pump(v, lambda: settled(v, frames)
             and v.worker.snapshot["render_rev"] != rev, "the level after the own cut")
        check(v.cut_custom is None and v.worker.snapshot["state"]["cut_px"] is None
              and "detail: high" in v.dstatus.get_text(),
              "high after the own cut: %r, state %r" % (
                  v.dstatus.get_text(), v.worker.snapshot["state"]["cut_px"]))
        v._cdlg.destroy()
        print("detail dialog: a 0.1 px cut of its own, refusals said, high ends it")
        strike_and_palettes(v)
        cell_status(v, answers)
        browse_dialog(v)
        levels_dialog(v)
    finally:
        v._quit()
        v.window.destroy()
    # a margin: the arrow step inside it is a crop - nothing drawn
    m = gui.Viewer(cache, detail=2, margin=True)
    try:
        pump(m, lambda: settled(m) and m._margin_frame is not None, "margin")
        m._zoom_center(0.25)
        pump(m, lambda: settled(m) and m._margin_frame is not None
             and abs(m._margin_frame[2] / m.spp - 1) < 1e-9, "zoomed margin")
        crops = m.worker.snapshot["crop_hits"]
        submitted = m.worker.snapshot["submitted"]
        m._pan_view("Right", 0.1)
        pump(m, lambda: m.worker.snapshot["crop_hits"] > crops, "crop", 30)
        time.sleep(0.3)
        pump(m, lambda: True, "events")
        check(m.worker.snapshot["submitted"] == submitted,
              "a pan inside the margin rendered: %s" % m.worker.snapshot)
    finally:
        m._quit()
        m.window.destroy()
    cache.close()


def file_loads_off(src, temp):
    """User 2026-11-11 ("TeeBox에서 floe를 열 경우 사용자가 임의의 파일을 로드할 수
    없게 하려고 함. 메뉴에서 레이아웃, db등 파일 로드 관련을 disable 상태로 만들기
    위한 옵션을 추가해줘"): floe/gui.py's Viewer as `floe2 view --no-file-load`
    starts it (file_load=False), beside a plain one. What the start names
    is loaded by both - the layout, the DRC results as --drc opens them,
    and by their names the rules metadata and the clusters beside them.
    The locked one: the load commands of the File and DRC menus are
    insensitive with the reason as their tooltip, the rest of the two
    menus as in the plain viewer (the saves among them); every load
    handler says the refusal in the status line and makes no dialog; the
    layer menu's `load layer properties…` is insensitive, its save is not;
    `review from previous run…` opens no window, `load waives…` neither;
    the reviewers window opens with `Load peer work…` insensitive - and
    still so after a job has made its controls sensitive again - and its
    loader makes no chooser. The dialogs' run() is the gate's (a dialog
    made is a dialog counted)."""
    from floe import gui
    Gtk, Gdk = gui.Gtk, gui.Gdk
    d = Path(temp) / "no-file-load"
    d.mkdir()
    for name in ("sample.db", "sample.db.clusters", "sample.svrf.rules.json"):
        (d / name).write_bytes((ROOT / name).read_bytes())
    db = d / "sample.db"
    done = subprocess.run([os.environ["FLOE_INDEX_BIN"], "drc", str(db)], cwd=ROOT,
                          capture_output=True, text=True, timeout=300)
    check(done.returncode == 0, done.stdout + done.stderr)
    loads = {"File": ["load layout…", "load jobdeck…"],
             "DRC": ["open results .db…", "load SVRF rules…", "load clusters…",
                     "load waives…", "review from previous run…", "load notes…"]}
    handlers = (("load layout", "_load_layout_dialog"),
                ("load jobdeck", "_load_jobdeck_dialog"),
                ("open DRC results (.db)", "_drc_open_dialog"),
                ("open rule metadata (*.rules.json)", "_drc_rules_dialog"),
                ("load DRC clusters", "_drc_clusters_dialog"),
                ("load notes", "_drc_note_load_dialog"),
                ("load layer properties", "_load_props_dialog"))
    off = gui.NO_FILE_LOAD

    def menus(v):
        return {root.get_label(): {
            item.get_label().split("\t")[0]: (item.get_sensitive(), item.get_tooltip_text())
            for item in root.get_submenu().get_children()
            if not isinstance(item, Gtk.SeparatorMenuItem)}
            for root in v._menubar.get_children()}

    def quiet(v):
        return (v._drc_selection_key is None and v._drc_review_job is None
                and not v._drc_delta_busy and v._drc_group_cache_key is None)

    def layer_menu(v):
        e = Gdk.Event.new(Gdk.EventType.BUTTON_PRESS)
        e.window = v.window.get_window()
        e.send_event = True
        e.x, e.y = 5, 5
        e.button, e.state = 3, Gdk.ModifierType(0)
        e.set_device(Gdk.Display.get_default().get_default_seat().get_pointer())
        v._popup_layer_menu(e)
        items = {i.get_label(): i.get_sensitive() for i in v._layer_menu.get_children()
                 if not isinstance(i, Gtk.SeparatorMenuItem)}
        v._layer_menu.popdown()
        return items

    def buttons(widget, out):
        if isinstance(widget, Gtk.Button):
            out[widget.get_label()] = widget
        elif isinstance(widget, Gtk.Container):
            widget.forall(lambda child: buttons(child, out))
        return out

    made, plain = [], {}
    run = Gtk.Dialog.run
    Gtk.Dialog.run = lambda dlg: (made.append(dlg.get_title()), Gtk.ResponseType.CANCEL)[1]
    try:
        for locked in (False, True):
            cache = gtkservice.ServiceCache(str(src))
            cache.load()
            v = gui.Viewer(cache, file_load=not locked)
            try:
                pump(v, lambda: settled(v), "first frame")
                check(v._file_load is (not locked), "the switch: %r" % v._file_load)
                # the start's own loads: the results as --drc opens them, the
                # rules metadata and the clusters found beside them by name
                v._drc_open_db(str(db))
                pump(v, lambda: v._drc is not None and quiet(v), "the results", 60)
                check(v._drc_total > 0 and v._drc_rmeta is not None
                      and v._drc_clusters is not None and not made,
                      "the start's loads (locked %s): %s errors, rules %s, clusters %s, dialogs %s"
                      % (locked, v._drc_total, v._drc_rmeta is not None,
                         v._drc_clusters is not None, made))
                bar, layer = menus(v), layer_menu(v)
                if not locked:
                    plain = bar
                    check(all(bar[m][label] == (True, None)
                              for m, labels in loads.items() for label in labels),
                          "a plain viewer's load commands: %s" % {m: bar[m] for m in loads})
                    check(layer["load layer properties…"] and layer["save layer properties…"],
                          "a plain viewer's layer menu: %s" % layer)
                    v._load_layout_dialog()
                    check(made == ["load layout"] and v.status.get_text() != "load layout: " + off,
                          "a plain viewer's load dialog: %s, %r" % (made, v.status.get_text()))
                    del made[:]
                    w = v._drc_team_dialog()
                    load = buttons(w.window, {})["Load peer work…"]
                    check(load.get_sensitive() and load in w.controls,
                          "a plain viewer's reviewers window: its load button")
                    w.window.destroy()
                    continue
                for m, labels in loads.items():
                    for label, state in bar[m].items():
                        want = (False, off) if label in labels else plain[m][label]
                        check(state == want, "%s > %s: %s, not %s" % (m, label, state, want))
                    check(set(labels) <= set(bar[m]), "%s lacks %s" % (m, set(labels) - set(bar[m])))
                check(all(bar[m][label][0] for m, label in (
                    ("File", "clip region…"), ("DRC", "save waives as…"),
                    ("DRC", "save notes as…"), ("DRC", "reviewers…"))),
                      "the saves and the reviewers window are not this switch's: %s" % bar["DRC"])
                check(layer["load layer properties…"] is False
                      and layer["save layer properties…"] is True,
                      "the layer menu: %s" % layer)
                for what, name in handlers:
                    v._set_live_status("")
                    getattr(v, name)()
                    check(not made and v.status.get_text() == "%s: %s" % (what, off),
                          "%s: dialogs %s, status %r" % (name, made, v.status.get_text()))
                # the two commands that open a window first
                v._set_live_status("")
                check(v._drc_migration_dialog() is None
                      and getattr(v, "_drc_migration", None) is None
                      and v.status.get_text() == "review from previous run: " + off,
                      "the window of a previous run: %r" % v.status.get_text())
                v._drc_waive_load_dialog()
                check(getattr(v, "_drc_team", None) is None and not made
                      and v.status.get_text() == "load waives: " + off,
                      "load waives: %r" % v.status.get_text())
                # the reviewers window: exports and statistics, no load
                w = v._drc_team_dialog()
                check(w is not None, "the reviewers window did not open")
                b = buttons(w.window, {})
                load = b["Load peer work…"]
                others = [b[k] for k in ("Export my review…", "Refresh", "Statistics CSV…")]
                check(not load.get_sensitive() and load not in w.controls
                      and load.get_tooltip_text() == off
                      and all(o.get_sensitive() for o in others)
                      and w.status.get_text()
                      == "Loading peer work is off in this viewer (--no-file-load).",
                      "the reviewers window: load %s, the others %s, %r" % (
                          load.get_sensitive(), [o.get_sensitive() for o in others],
                          w.status.get_text()))
                w._load_dialog()
                check(not made and w.status.get_text()
                      == "Load peer review bundles or waiver files: " + off,
                      "the reviewers window's loader: %s, %r" % (made, w.status.get_text()))
                w._refresh()
                check(w.job is not None, "the reviewers window's refresh did not start")
                pump(v, lambda: w.job is None, "the reviewers window's refresh", 60)
                check(not load.get_sensitive() and all(o.get_sensitive() for o in others),
                      "after a job: load %s, the others %s" % (
                          load.get_sensitive(), [o.get_sensitive() for o in others]))
                w.window.destroy()
            finally:
                v._drc_release()
                v._quit()
                v.window.destroy()
                cache.close()
    finally:
        Gtk.Dialog.run = run
    print("file loads off: %d menu commands insensitive, %d handlers refuse, the layer "
          "menu's load, the reviewers window's load button; the start's loads and the "
          "saves as in a plain viewer" % (sum(len(v) for v in loads.values()),
                                          len(handlers) + 2))


def forwarded_requests(src, temp):
    """User 2026-11-11 ("사용자가 TeeBox를 이용해서 floe를 열 경우, 이미 떠있는
    floe 창을 이용하는게 좋을 것 같음 ... 자동저장 방식이라면 전환 확인을 물을
    필요는 없음. DISPLAY가 동일할 경우 전환하는 flateyes 방식을 따르면 될 것
    같음. 통계는 별도의 cli 명령으로"): floe/gui.py's Viewer owning a real
    instance socket (floe/instance.py), the requests of later launches
    sent as floe/viewcli.py sends them. The window: leaves results already
    open as they are; opens other results in their place, nothing asked,
    its title naming them with their folder; serves the reviewer a request
    states and turns its file loads off with a request that has them off,
    for good; opens another layout and the results after it; holds a
    request back while a dialog is open - the note typed in that dialog is
    written to the results it was typed for, not to those of the request -
    and while a review write runs; presents itself alone for a bare launch
    (no index question: the first option was once taken for a path); opens
    a plain layout before it answers. The waives made before a switch are
    what `floe2 drc DB --review-stats` reads while the viewer runs."""
    import json
    import threading
    from floe import gui, instance
    Gtk = gui.Gtk
    d = Path(temp) / "requests"
    for run in ("run1", "run2"):
        (d / run).mkdir(parents=True)
        for name in ("sample.db", "sample.db.clusters", "sample.svrf.rules.json"):
            (d / run / name).write_bytes((ROOT / name).read_bytes())
        done = subprocess.run([os.environ["FLOE_INDEX_BIN"], "drc", str(d / run / "sample.db")],
                              cwd=ROOT, capture_output=True, text=True, timeout=300)
        check(done.returncode == 0, done.stdout + done.stderr)
    other = d / "other.oas"
    other.write_bytes(Path(src).read_bytes())
    done = subprocess.run([FLOE2, "index", str(other), "--jobs", "2"], cwd=ROOT,
                          capture_output=True, text=True, timeout=600)
    check(done.returncode == 0, done.stdout + done.stderr)
    db1, db2 = str(d / "run1" / "sample.db"), str(d / "run2" / "sample.db")
    addr = instance.socket_address("gate-%d" % os.getpid())
    server = instance.try_bind(addr)
    check(server is not None, "the gate's instance socket %r is taken" % addr)
    cache = gtkservice.ServiceCache(str(src))
    cache.load()
    v = gui.Viewer(cache, server)
    asked = []
    v._ask_yes_no = lambda question: (asked.append(question), False)[1]

    def quiet():
        return (v._drc_selection_key is None and v._drc_review_job is None
                and not v._drc_delta_busy and v._drc_group_cache_key is None)

    def send(layout, **fields):
        """A later launch's request, as viewcli sends it; its exit code."""
        request = "\t".join([str(layout)] + ["%s=%s" % kv for kv in fields.items()])
        got = []
        thread = threading.Thread(
            target=lambda: got.append(instance.try_forward(addr, request)))
        thread.start()
        pump(v, lambda: not thread.is_alive(), "the answer to %r" % request, 30)
        return got[0]

    def showing(layout, results, what):
        pump(v, lambda: v.cache is not None and v.cache.src == str(layout)
             and v._drc_source == results and v._drc is not None and quiet()
             and v._request_waiting is None and settled(v), what, 60)

    def stats(results, who):
        out = subprocess.run([FLOE2, "drc", results, "--review-stats", "--floe-reviewer", who],
                             cwd=ROOT, capture_output=True, text=True, timeout=120)
        check(out.returncode == 0, out.stdout + out.stderr)
        return json.loads(out.stdout)["reviewers"][0]

    run = Gtk.Dialog.run
    try:
        pump(v, lambda: settled(v), "first frame")
        v._drc_open_db(db1)
        showing(src, db1, "the start's results")
        who = v._drc._reviewer
        title = v.window.get_title()
        check(title.endswith(" · DRC run1/sample.db"), "the title of a review: %r" % title)
        v._drc_set_waived(0, [0, 1], True, reason="")    # (asked why, none given)
        # the same results again: the window as it is
        held = v._drc
        check(send(src, depth=999, drc=db1) == 0, "the same results were refused")
        pump(v, lambda: v.status.get_text() == "DRC sample.db: already open",
             "results already open", 30)
        check(v._drc is held and v._drc_source == db1, "results already open were opened again")
        # other results: in their place, nothing asked; the review made is on the disk
        check(send(src, depth=999, drc=db2) == 0, "other results were refused")
        showing(src, db2, "the other results")
        check(v._drc is not held and v.window.get_title().endswith(" · DRC run2/sample.db")
              and not asked, "the switch: %r, asked %s" % (v.window.get_title(), asked))
        mine = stats(db1, who)
        check(mine["waived"] == 2 and mine["counters_match"],
              "the review left behind, read by the statistics command: %s"
              % {k: mine.get(k) for k in ("waived", "counters_match")})
        # a request's reviewer, and its file loads off - which stay off
        check(v._file_load and all(i.get_sensitive() for i in v._load_items),
              "the window's loads before a request turns them off")
        check(send(src, depth=999, drc=db2, userid="lee", noload=1) == 0, "refused")
        pump(v, lambda: v._drc is not None and v._drc._reviewer == "lee" and quiet(),
             "the request's reviewer", 60)
        check((d / "run2" / ".sample.db.waive.lee").is_file() and v._drc_source == db2,
              "the review of the reviewer the request states")
        check(not v._file_load and v._load_items
              and not any(i.get_sensitive() for i in v._load_items)
              and all(i.get_tooltip_text() == gui.NO_FILE_LOAD for i in v._load_items),
              "a request with the file loads off left the window loading")
        # another layout, then its results (no noload: the loads stay off)
        check(send(other, depth=999, drc=db1, userid="lee") == 0, "refused")
        showing(other, db1, "the other layout and its results")
        title = v.window.get_title()
        check(title.startswith("%s - requests/other.oas · " % gui.APP)
              and title.endswith(" · DRC run1/sample.db") and not v._file_load,
              "another layout: %r, loads %s" % (title, v._file_load))
        # a dialog open: the request waits for it
        v.window.grab_add()
        check(send(other, depth=999, drc=db2, userid="lee") == 0, "a waiting request's answer")
        check(v._request_waiting is not None and v._drc_source == db1
              and v.status.get_text().startswith(
                  "open request waiting (a dialog or a menu is open): sample.db"),
              "a request under a grab: %r, %r" % (v._request_waiting, v.status.get_text()))
        v.window.grab_remove()
        showing(other, db2, "the request after the grab")
        # the note dialog: what is typed while a request waits is written to
        # the results it was typed for
        v._drcwin._wf.set_active_id("all")
        v._drc_select_row(0)
        pump(v, quiet, "the rule", 60)
        v._drc_selection_submit(0, None, gtkservice.ListMembers([2]), "replace")
        pump(v, lambda: v._drc_sel is not None and quiet(), "an error selected", 60)
        seen = {}

        def note_run(dlg):
            seen["grab"] = Gtk.grab_get_current() is not None
            seen["answer"] = send(other, depth=999, drc=db1, userid="lee")
            seen["waiting"] = v._request_waiting is not None and v._drc_source == db2
            scroll = [k for k in dlg.get_content_area().get_children()
                      if isinstance(k, Gtk.ScrolledWindow)][0]
            scroll.get_child().get_buffer().set_text("typed while a request waited")
            return Gtk.ResponseType.OK

        Gtk.Dialog.run = note_run
        v._drc_note_key()
        Gtk.Dialog.run = run
        check(seen == {"grab": True, "answer": 0, "waiting": True},
              "a request while the note dialog was open: %s" % seen)
        note = d / "run2" / ".sample.db.notes.lee.fe"
        check(note.is_file() and "typed while a request waited" in note.read_text(),
              "the note typed for run2's results is not in their notes")
        showing(other, db1, "the request after the note dialog")
        check(not (d / "run1" / ".sample.db.notes.lee.fe").exists(),
              "the note followed the request to the other results")
        # a review write running: the request waits for its end
        v._drc_review_job = dict(db=v._drc, ci=0, targets=[0], operation="waive", on=True)
        check(send(other, depth=999, drc=db2, userid="lee") == 0, "a waiting request's answer")
        check(v._request_waiting is not None and v._drc_source == db1
              and "a review update is running" in v.status.get_text(),
              "a request beside a review write: %r" % v.status.get_text())
        v._drc_review_job = None
        showing(other, db2, "the request after the review write")
        # a bare launch: the window comes forward, nothing else
        held, text = v._drc, v.status.get_text()
        check(send("", detail="medium", depth=0, frames="on") == 0, "a bare launch's answer")
        end = time.monotonic() + 0.5
        pump(v, lambda: time.monotonic() > end, "a bare launch")
        check(v._drc is held and v.cache.src == str(other) and v._request_waiting is None
              and v.status.get_text() == text and not asked,
              "a bare launch changed the window: %r, asked %s" % (v.status.get_text(), asked))
        # a plain indexed layout: opened before the answer; the results of
        # the layout before go with it
        check(send(src, detail="medium", depth=0) == 0, "a plain layout's answer")
        check(v.cache.src == str(src), "a plain layout was not open when its request was answered")
        pump(v, lambda: settled(v), "the plain layout")
        check(v._drc is None and v._drc_source is None
              and " · DRC " not in v.window.get_title() and not asked,
              "after a plain layout: %r, asked %s" % (v.window.get_title(), asked))
    finally:
        Gtk.Dialog.run = run
        v._drc_release()
        v._quit()
        v.window.destroy()
        cache.close()
        if not addr.startswith("\0") and os.path.exists(addr):
            os.unlink(addr)
    print("one window a display: results already open kept, others opened in their place, "
          "a reviewer and the loads off taken from a request, another layout, a request "
          "held under a dialog and a review write, a note typed meanwhile on its own "
          "results, a bare launch, a plain layout; the review left behind read by "
          "--review-stats")


def cells_while_opening(src, temp):
    """The cell tree's first question comes as soon as the view exists,
    while its worker still opens (renderd on a big cache takes seconds):
    the service waits for the open and answers (2026-10-10: main01's tree
    stayed at "loading..." on `view is still opening`)."""
    slow = temp / "slow-renderd"
    slow.write_text('#!/bin/sh\nsleep 1\nexec "%s" "$@"\n'
                    % os.environ["FLOE_RENDERD_BIN"])
    slow.chmod(0o755)
    saved = os.environ["FLOE_RENDERD_BIN"]
    os.environ["FLOE_RENDERD_BIN"] = str(slow)
    svc = gtkservice.Service(FLOE2)
    try:
        s = gtkservice.ViewSession(src, W, H, svc=svc)
        check(s.snapshot["phase"] == "opening",
              "the slow worker was open at once: %s" % s.snapshot["phase"])
        s.cells("cell_sources", 1)
        s.cells("cells", 2, src=0)
        got = answer(s, "cells", lambda v: v["seq"] == 2, 60)
        check(got.get("found"), "a cell question while the view opened: %s" % got)
        s.close()
    finally:
        os.environ["FLOE_RENDERD_BIN"] = saved
        svc.close()
    print("cells while opening: answered once the view opened")


def layer_properties_option(src, temp):
    """User 2026-11-11, after a second launch had opened /path/2/layout.oas
    in the window showing /path/1/layout.oas: "창 제목에 폴더도 함께 표시해줘.
    그리고 layer property 파일도 실행 옵션으로 지정할 수 있게 해줘." A real
    window owning a real instance socket; later launches are the real
    command line. The title names the layout with its folder. `floe2 view
    SRC --layerprops FILE`: the window that starts with the file has its
    colours, fills, widths and visibility on the layer table and in its
    frames - the first one included -, says so, and keeps them when the
    layout's table is built again; the same file named again leaves what
    the user changed since, a changed file is applied in place; another
    layout takes its own file, opened before the launch is answered; a
    launch without the option opens a layout by its defaults; a link is read
    through; a file that cannot be read, one that is not there and the
    option without a layout end the launch with 1 and its reason, the window
    as it was."""
    import threading
    from floe import gui, instance
    Gtk = gui.Gtk
    d = Path(temp) / "props"
    one, two = d / "1" / "layout.oas", d / "2" / "layout.oas"
    for layout in (one, two):
        layout.parent.mkdir(parents=True)
        layout.write_bytes(Path(src).read_bytes())
        done = subprocess.run([FLOE2, "index", str(layout), "--jobs", "2"], cwd=ROOT,
                              capture_output=True, text=True, timeout=600)
        check(done.returncode == 0, done.stdout + done.stderr)
    a, b = d / "a.layerprops", d / "b.layerprops"
    # (77.0: a layer the layout does not have)
    a.write_text("# the launch's\n63.63 #ff00ff diagonal_1 MARK 1 3\n"
                 "7.0 blue speckle M7 0 1\n77.0 green speckle none 1 1\n")
    b.write_text("63.63 #00ffff wave MARK 1 2\n")
    link, bad = d / "link.layerprops", d / "bad.layerprops"
    os.symlink(str(b), str(link))
    # (the window names a file by its real path: the temporary folder is
    # itself behind a link on macOS)
    real_a, real_b, real_bad = (os.path.realpath(str(f)) for f in (a, b, bad))
    bad.write_bytes(b"63.63 red speckle \xff\xfe\n")
    display = ":gate-props-%d" % os.getpid()
    addr = instance.socket_address(instance.normalize_display(display))
    server = instance.try_bind(addr)
    check(server is not None, "the gate's instance socket %r is taken" % addr)
    plain = gtkservice.ServiceCache(str(one))
    plain.load()
    default = {(l["layer"], l["datatype"]): l["color"] for l in plain.meta["layers"]}
    plain.close()
    cache = gtkservice.ServiceCache(str(one))
    cache.load()
    v = gui.Viewer(cache, server, layerprops=str(a))
    asked = []
    v._ask_yes_no = lambda question, *_a, **_k: (asked.append(question), False)[1]
    mark, m7 = (63, 63), (7, 0)

    def colour(key):
        return [l["color"] for l in v.meta["layers"]
                if (l["layer"], l["datatype"]) == key][0]

    def painted(rgb):
        """Whether the frame shown has a pixel of exactly this colour."""
        pix = v.last_frame[0]
        n, stride, w = pix.get_n_channels(), pix.get_rowstride(), pix.get_width()
        data, want = bytes(pix.get_pixels()), bytes(rgb)
        for y in range(pix.get_height()):
            row = data[y * stride: y * stride + w * n]
            if any(row[x:x + 3] == want for x in range(0, w * n, n)):
                return True
        return False

    def rgb(text):
        return tuple(int(text[i:i + 2], 16) for i in (1, 3, 5))

    def launch(words, env_display=display):
        """A later launch, the real command line: (exit code, its output)."""
        got = []

        def run():
            env = dict(os.environ, DISPLAY=env_display)
            env.pop("FLOE_GUI_SMOKE_MS", None)
            done = subprocess.run([FLOE2, "view"] + [str(w) for w in words], cwd=ROOT, env=env,
                                  capture_output=True, text=True, timeout=120)
            got.append((done.returncode, (done.stdout + done.stderr).strip()))
        thread = threading.Thread(target=run)
        thread.start()
        pump(v, lambda: not thread.is_alive(), "the answer to %s" % (words,), 60)
        return got[0]

    def showing(layout, what):
        pump(v, lambda: v.cache is not None and v.cache.src == str(layout)
             and v._request_waiting is None and settled(v), what, 60)

    try:
        # the view was opened without the layer the file hides: its first
        # frame is not drawn with every layer and then again
        opened = v.worker._args[2]["layers"]
        check(isinstance(opened, list) and [7, 0] not in opened and [63, 63] in opened,
              "the layers the view was opened with: %s" % (opened,))
        pump(v, lambda: settled(v), "the first frame")
        # the title: the layout with its folder
        title = v.window.get_title()
        check(title.startswith("%s - 1/layout.oas · " % gui.APP),
              "the title of a layout: %r" % title)
        # the file is on the table ...
        check(colour(mark) == "#ff00ff" and colour(m7) == "#0000ff",
              "the file's colours: %s %s" % (colour(mark), colour(m7)))
        check(gui.fillpat.FILL_NAMES[v._layer_patterns[mark]] == "diagonal_1"
              and v._layer_widths == {mark: 3},
              "the file's fill and width: %s %s" % (v._layer_patterns, v._layer_widths))
        check(m7 not in v.visible and mark in v.visible
              and not v._layer_rows[m7].get_active(),
              "the file's visibility: %s" % sorted(v.visible))
        check(v.status.get_text() == "layer properties loaded: %s (2 colors, 2 fills)" % real_a,
              "what the status line says of it: %r" % v.status.get_text())
        check(v._props_bound is not None and v._props_bound["path"] == real_a
              and v._props_bound["src"] == str(one), "the file is the layout's: %s" % v._props_bound)
        # ... and in the frames: the marker layer in the file's colour, none in
        # its default; the hidden layer not asked for
        layers = v.worker.snapshot["state"]["layers"]
        check(isinstance(layers, list) and [7, 0] not in layers and [63, 63] in layers,
              "the view's layers: %s" % (layers,))
        check(painted((255, 0, 255)) and not painted(rgb(default[mark])),
              "the frame is not in the file's colours (default %s)" % default[mark])
        # the layout's table built again (a reload): the file is on it again
        v._layer_rows[m7].set_active(True)
        pump(v, lambda: m7 in v.visible, "the row shown by hand")
        v._apply_cache(v.cache)
        pump(v, lambda: settled(v), "the reload")
        check(colour(mark) == "#ff00ff" and m7 not in v.visible and v._layer_widths == {mark: 3}
              and painted((255, 0, 255)),
              "a reload lost the launch's layer properties: %s %s" % (colour(mark), sorted(v.visible)))
        # the same launch again: nothing is applied - what the user changed stays
        v._layer_rows[mark].set_active(False)
        pump(v, lambda: mark not in v.visible and settled(v), "the row hidden by hand")
        held = v.cache
        code, text = launch([one, "--layerprops", a])
        check(code == 0 and not text, "the same launch again: %s %r" % (code, text))
        end = time.monotonic() + 0.4
        pump(v, lambda: time.monotonic() > end, "the same launch")
        check(v.cache is held and mark not in v.visible and m7 not in v.visible,
              "the same file named again changed the table: %s" % sorted(v.visible))
        # the file changed: applied in place
        time.sleep(0.02)
        a.write_text("63.63 #ffff00 speckle MARK 1 1\n7.0 blue speckle M7 1 1\n")
        code, text = launch([one, "--layerprops", a])
        check(code == 0 and not text, "a changed file: %s %r" % (code, text))
        pump(v, lambda: colour(mark) == "#ffff00" and mark in v.visible and m7 in v.visible
             and settled(v) and painted((255, 255, 0)), "the changed file", 30)
        check(v.cache is held and v._layer_widths == {} and not painted((255, 0, 255)),
              "the changed file in place: %s" % v._layer_widths)
        # another layout with its own file, through a link: opened before
        # the launch is answered, the title its folder's
        code, text = launch([two, "--layerprops", link])
        check(code == 0 and not text, "another layout: %s %r" % (code, text))
        check(v.cache.src == str(two), "another layout was not open when its launch was answered")
        showing(two, "the other layout")
        title = v.window.get_title()
        check(title.startswith("%s - 2/layout.oas · " % gui.APP), "its title: %r" % title)
        check(colour(mark) == "#00ffff" and colour(m7) == default[m7]
              and v._layer_widths == {mark: 2} and m7 in v.visible
              and v._props_bound["path"] == real_b and painted((0, 255, 255)),
              "the other layout's file (the link's target): %s %s" % (colour(mark), v._props_bound))
        check(v.status.get_text() == "layer properties loaded: %s (1 colors, 1 fills)" % real_b,
              "the status line: %r" % v.status.get_text())
        # a file that cannot be read, one that is not there, no layout: the
        # launch says so with 1 and the window is as it was
        held = v.cache
        for words, reason in (([one, "--layerprops", bad],
                               "ERR layer properties file %s: " % real_bad),
                              ([one, "--layerprops", d / "none.layerprops"],
                               "floe2: no such layer properties file: "),
                              (["--layerprops", a], "floe2: --layerprops requires a layout")):
            code, text = launch(words)
            check(code == 1 and text.startswith(reason), "%s: %s %r" % (words, code, text))
        end = time.monotonic() + 0.3
        pump(v, lambda: time.monotonic() > end, "the refused launches")
        check(v.cache is held and colour(mark) == "#00ffff" and v._props_bound["path"] == real_b,
              "a refused launch changed the window")
        # a layout without the option: its defaults, no file
        code, text = launch([one])
        check(code == 0 and not text, "a plain launch: %s %r" % (code, text))
        showing(one, "the first layout by itself")
        check(colour(mark) == default[mark] and v._props_bound is None and v._layer_widths == {}
              and painted(rgb(default[mark])) and not asked,
              "a layout opened without the option: %s %s, asked %s"
              % (colour(mark), v._props_bound, asked))
        print("layer properties option: the title's folder, the file on the table and in the "
              "frames, the same / a changed / another file, the refusals")
    finally:
        v._quit()
        v.window.destroy()
        cache.close()
        if not addr.startswith("\0") and os.path.exists(addr):
            os.unlink(addr)


def same(a, b, what):
    if a == b:
        return
    diff = sum(1 for i in range(0, min(len(a), len(b)), 4) if a[i:i + 4] != b[i:i + 4])
    raise AssertionError("%s: the channel's frame differs from the adapter's "
                         "(%d of %d pixels, sizes %d/%d)"
                         % (what, diff, len(a) // 4, len(a), len(b)))


def capture_parity(src, temp):
    """`floe2 render` with its defaults draws what the viewer shows (user
    2026-10-12: "the viewer's default mode has thin keep, the density and so
    on; a capture must come out the same"): floe/gui.py's Viewer as `floe2
    view` starts it (detail medium, thin keep, frames and labels on, the
    density under the cut on - user 2026-10-12: "turn the density on by
    default"), and for each view it settles on - the plain open (depth 0), a
    zoom, full depth, a zoom out, the density turned off (`v`), the occupancy
    summary turned on (`o`, 2026-10-20; the fixture is indexed with its
    summary, so the summary itself is compared) - the capture of its box at
    its size, its depth and, for the last two, --density off / --occupancy on:
    the same RGB, pixel for pixel."""
    from PIL import Image
    from floe import gui
    cache = gtkservice.ServiceCache(str(src))
    cache.load()
    dbu = float(cache.meta["dbu"])
    v = gui.Viewer(cache)
    frames = []
    take = v._ctl_frame
    v._ctl_frame = lambda f: (frames.append(f), take(f))

    def compare(what, *extra):
        on_screen, f = shown(v)
        out = Path(temp) / ("capture %s.png" % what)
        argv = [FLOE2, "render", str(src), "--bbox",
                ",".join("%.17g" % (c * dbu) for c in f["bbox"]),
                "--px", "%dx%d" % (f["width"], f["height"]), "--stretch",
                "--out", str(out), *extra]
        if v._depth() is not None:
            argv += ["--depth", str(v._depth())]
        done = subprocess.run(argv, cwd=ROOT, capture_output=True, text=True, timeout=300)
        check(done.returncode == 0, "capture %s: %s" % (what, done.stdout + done.stderr))
        captured = Image.open(out).convert("RGBA").tobytes()
        differ = sum(1 for i in range(0, len(on_screen), 4)
                     if on_screen[i:i + 3] != captured[i:i + 3])
        check(len(captured) == len(on_screen) and differ == 0,
              "%s: the capture differs from the viewer in %d of %d pixels"
              % (what, differ, len(on_screen) // 4))

    try:
        pump(v, lambda: settled(v, frames), "first frame")
        check(v.density_on and v.detail == 1 and v._effective_thin() == "keep",
              "the viewer's start: density %s, detail %s, thin %s"
              % (v.density_on, v.detail, v._effective_thin()))
        compare("the open")
        steps = (("a zoom", lambda: v._zoom_center(0.25), ()),
                 ("full depth", lambda: v._set_depth(999), ()),
                 ("a zoom out", lambda: v._zoom_center(4.0), ()),
                 ("the density off", v._toggle_density, ("--density", "off")),
                 ("the occupancy summary on", v._toggle_occupancy,
                  ("--density", "off", "--occupancy", "on")))
        for what, step, extra in steps:
            rev = v.worker.snapshot["render_rev"]
            step()
            pump(v, lambda: settled(v, frames)
                 and v.worker.snapshot["render_rev"] != rev, what)
            compare(what, *extra)
        print("capture parity: floe2 render's defaults = the viewer's pixels "
              "(open, zoom, full depth, zoom out with the density on; density off; "
              "occupancy summary on)")
    finally:
        v._quit()
        v.window.destroy()


def light_status(src, temp):
    """A layout opened from a light cache (DIRECT_READ_PLAN L6): the depth
    label says `cache:light`, its frame line carries the light fill rounds,
    and no summary banner shows (a layout has none)."""
    from floe import gui
    light_src = Path(temp) / "light" / src.name
    light_src.parent.mkdir()
    light_src.write_bytes(src.read_bytes())
    done = subprocess.run([FLOE2, "index", str(light_src), "--light", "--jobs", "2"],
                          cwd=ROOT, capture_output=True, text=True, timeout=600)
    check(done.returncode == 0, done.stdout + done.stderr)
    cache = gtkservice.ServiceCache(str(light_src))
    cache.load()
    check(cache.meta.get("cache") == "light",
          "the light cache's meta says %r" % cache.meta.get("cache"))
    v = gui.Viewer(cache)
    frames = []
    take = v._ctl_frame
    v._ctl_frame = lambda f: (frames.append(f), take(f))
    try:
        pump(v, lambda: settled(v, frames), "first frame")
        label = v.dstatus.get_text()
        check("cache:light" in label, "the depth label of a light cache: %r" % label)
        last = [f for f in frames if f["purpose"] == "foreground"][-1]
        fields = last.get("fields") or {}
        check("light_rounds" in fields and fields.get("light_pending") == "0",
              "the settled frame of a light cache: %s"
              % {k: fields.get(k) for k in ("light_rounds", "light_pending")})
        check(not v._summary_banner.get_visible(), "a layout showed the summary banner")
        print("light cache: %r, %s fill round(s)" % (label, fields.get("light_rounds")))
    finally:
        v._quit()
        v.window.destroy()


def main():
    os.environ["FLOE_INDEX_BIN"] = str(ROOT / "rust/target/release/floe-index")
    os.environ["FLOE_RENDERD_BIN"] = str(ROOT / "rust/target/release/floe-renderd")
    os.environ.pop("FLOE_RUST_ROUND_PAGES", None)
    with tempfile.TemporaryDirectory(prefix="floe-gtk-view-") as temp:
        src = Path(temp) / "valmini.oas"
        src.write_bytes((ROOT / "data/m1/valmini.oas").read_bytes())
        # (with its occupancy summary: capture_parity's last step turns the
        # summary on in the viewer and captures with --occupancy on, which
        # since 0.12.388 builds a missing summary first - the fixture has it
        # from the start, so both draw the same summary)
        done = subprocess.run([FLOE2, "index", str(src), "--occupancy", "--jobs", "2"], cwd=ROOT,
                              capture_output=True, text=True, timeout=600)
        check(done.returncode == 0, done.stdout + done.stderr)
        svc = gtkservice.Service(FLOE2)
        ref = Reference(src)
        try:
            # a plain open: detail high, the controller's initial fit
            s = gtkservice.ViewSession(src, W, H, patch={"detail": "high"}, svc=svc)
            check(s.model["deck"] is False and s.snapshot["state_rev"] == 1,
                  "view_open: %s" % s.snapshot)
            first = until(s, final_frame(1))[-1]
            check((first["width"], first["height"], first["format"]) == (W, H, "raw"),
                  "first frame: %s" % {k: first[k] for k in ("width", "height", "format")})
            same(first["pixels"], ref.frame(first), "the open")
            queries_and_cells(s, ref, first, Path(temp))
            # the scale border of the view's viewport, of a box given, and
            # a box the viewport refuses
            b = s.border()
            check(b["step_px"] >= 60 and b["x"][0]["from"] == 0
                  and abs(b["x"][-1]["to"] - W) < 1e-9 and b["y"][0]["from"] == 0
                  and abs(b["y"][-1]["to"] - H) < 1e-9 and 3 <= b["decimals"] <= 5,
                  "view_border: %s" % b)
            dbu = s.model["dbu"]
            given = s.border([0.0, 0.0, 1000.0, 750.0], W, H)
            decimals = min(5, max(3, math.ceil(-math.log10(2.5 * dbu))))
            check(given["bottom_left"] == {"x": "%.*f" % (decimals, 0), "y": "%.*f" % (decimals, 0)}
                  and given["top_right"] == {"x": "%.*f" % (decimals, 1000 * dbu),
                                             "y": "%.*f" % (decimals, 750 * dbu)}
                  and abs(given["step_px"] - given["step_um"] / (2.5 * dbu)) < 1e-6,
                  "view_border of a box: %s" % given)
            try:
                s.border([0.0, 0.0, 1000.0, 750.0], 0, H)
                check(False, "view_border took a zero width")
            except gtkservice.ServiceError as exc:
                check(exc.kind == "input", "view_border's refusal: %s %s" % (exc.kind, exc))
            print("view_border: %s, %d + %d segments" % (b["label"], len(b["x"]), len(b["y"])))
            cells_while_opening(src, Path(temp))
            # the density under the cut: its first round, then the frame
            on = s.edit(density=True)
            rev = on["state_rev"]
            check(on["state"]["density"] is True, "density: %s" % on["state"])
            got = until(s, final_frame(rev))
            rounds = [f for f in got if f["state_rev"] == rev]
            # renderd shows pass 1 first (a round of its own), then the frame
            # with the density - the shared worker client takes both since
            # renderd 0.12.308 (the first round's line was not a frame line)
            # (the controller keeps the latest frame only: a quick first round
            # may be passed by the frame - the frame's round says it came)
            check(rounds[-1]["round"] >= 2 and all(
                not f["final"] and f["fields"].get("density_round") == "1"
                for f in rounds[:-1]),
                "density rounds: %s" % [(f["round"], f["final"]) for f in rounds])
            check([f["round"] for f in rounds] == sorted({f["round"] for f in rounds}),
                  "density rounds do not count up: %s" % [f["round"] for f in rounds])
            same(rounds[-1]["pixels"], ref.frame(rounds[-1], density=True), "the density")
            changed = sum(1 for i in range(0, len(first["pixels"]), 4)
                          if first["pixels"][i:i + 4] != rounds[-1]["pixels"][i:i + 4])
            check(changed > 0, "the density under the cut changed no pixel")
            print("density: %d round(s), %d pixel(s) changed" % (len(rounds), changed))
            off = s.edit(density=False)
            until(s, final_frame(off["state_rev"]))
            # a cut of the viewer's own (the `d` dialog's entry, 2026-10-28):
            # in place of the level's until a level is set again
            own = s.edit(cut_px=0.1)
            check(own["state"]["cut_px"] == 0.1 and own["state"]["detail"] == "high",
                  "cut_px: %s" % own["state"])
            fine = until(s, final_frame(own["state_rev"]))[-1]
            same(fine["pixels"], ref.frame(fine, cut_px=0.1), "the 0.1 px cut")
            finer = sum(1 for i in range(0, len(first["pixels"]), 4)
                        if first["pixels"][i:i + 4] != fine["pixels"][i:i + 4])
            check(finer > 0, "the 0.1 px cut drew what the 1 px cut drew")
            for refused in (0, -1, 101, "0.1"):
                try:
                    s.edit(cut_px=refused)
                    check(False, "cut_px %r was taken" % (refused,))
                except gtkservice.ServiceError as exc:
                    check(exc.kind == "input", "cut_px %r: %s %s" % (refused, exc.kind, exc))
            back = s.edit(detail="high")
            check(back["state"]["cut_px"] is None and back["state"]["detail"] == "high",
                  "a level after the cut: %s" % back["state"])
            until(s, final_frame(back["state_rev"]))
            print("cut_px: 0.1 px changed %d pixel(s) of the 1 px fit" % finer)
            # the desktop's bounds (gui.py _clamp_view): the fit view is wider
            # than the die and 10% around it, so a pan there stays centred
            still = s.edit(navigation={"kind": "pan", "x": 0.25, "y": 0.0, "snap": True})
            check(all(abs(a - b) < 1e-6 * (first["bbox"][2] - first["bbox"][0])
                      for a, b in zip(still["state"]["viewport"]["bbox"], first["bbox"])),
                  "a pan of the too-wide fit view moved it: %s" % still["state"]["viewport"])
            # zoomed in, a pan the controller snaps to the 16 px fill phase
            zoomed = s.edit(navigation={"kind": "zoom", "factor": 0.25, "anchor": [0.5, 0.5]})
            rev = zoomed["state_rev"]
            near = until(s, final_frame(rev))[-1]
            same(near["pixels"], ref.frame(near), "the zoom")
            snap = s.edit(navigation={"kind": "pan", "x": 0.25, "y": 0.0, "snap": True})
            rev = snap["state_rev"]
            panned = until(s, final_frame(rev))[-1]
            spp = (near["bbox"][2] - near["bbox"][0]) / W
            shift = (panned["bbox"][0] - near["bbox"][0]) / spp
            check(abs(shift - round(shift)) < 1e-6 and round(shift) % 16 == 0
                  and round(shift) > 0, "the pan moved %r px, not 16 px steps" % shift)
            same(panned["pixels"], ref.frame(panned), "the pan")
            s.close()
            until(s, lambda kind, v: kind == "closed", 30)
            # a margin around the viewport, then a pan it covers
            m = gtkservice.ViewSession(src, W, H, patch={"detail": "high"},
                                       margin=True, svc=svc)
            check(m.view != s.view and m.snapshot["margin_enabled"],
                  "margin view: %s" % m.snapshot)
            got = until(m, final_frame(1, "margin"))
            margin = got[-1]
            check(margin["viewport"] == [W, H] and margin["width"] > W,
                  "the margin frame: %sx%s around %s" % (
                      margin["width"], margin["height"], margin["viewport"]))
            fg = [f for f in got if f["purpose"] == "foreground"][-1]
            vbox = fg["bbox"]
            same(margin["pixels"], ref.frame(margin, view=vbox), "the margin")
            covered = m.edit(navigation={"kind": "pan", "x": 0.1, "y": 0.0, "snap": True})
            check(covered["margin"] and covered["margin"]["crop_safe"],
                  "a pan inside the margin: %s" % covered["margin"])
            time.sleep(0.3)
            more = [(k, v) for k, v in m.events() if k == "frame"]
            check(not more and m.snapshot["crop_hits"] >= 1,
                  "a covered pan rendered again: %s crop_hits %s"
                  % (len(more), m.snapshot["crop_hits"]))
            m.close()
            until(m, lambda kind, v: kind == "closed", 30)
            # the service's frame folders went with their views
            stay = list(Path(tempfile.gettempdir()).glob(
                "floe2-view-%d-*" % svc._proc.pid))
            check(not stay, "closed views left their folders: %s" % stay)
            plates = pick_layout(temp)
            pick_order(plates, svc)
            if gtk_ready():
                gui_viewer(src, ref)
                pick_order_ui(plates)
                capture_parity(src, temp)
                light_status(src, temp)
                file_loads_off(src, temp)
                forwarded_requests(src, temp)
                layer_properties_option(src, temp)
        finally:
            ref.stop()
            svc.close()
    print("GTK VIEW: ALL OK")


if __name__ == "__main__":
    main()
