#!/usr/bin/env python3
"""The GTK viewer's view channel (docs/SHARED_APP_LAYER.ko.md §7, P4c):
`floe2 gtk-service`'s view_open / view_edit / view_cancel / view_close over
the shared Rust ViewController, driven as the viewer will drive it
(floe/gtkservice.py ViewSession), against a real floe-renderd.

Checked: every frame the channel hands over is byte-equal to the frame the
viewer's current adapter (floe/rust_render.py RustRenderWorker) draws for
the same box, size and policy - a plain open, a snapped pan, the density
under the cut (its first round, then the frame), a margin around the
viewport (vw/vh) and a pan the margin covers (no new frame); the state the
service says (revisions, phase, density); a closed view's files go and the
service opens the next one.

P4d: snap, pick, the cell tree and a clip through the channel = the
adapter's answers; the viewer itself (floe/gui.py Viewer, in process) on
the controller's loop - its first frame, a pan, a zoom, the density, the
grayscale, each frame byte-equal to the adapter's for the view the viewer
shows, the perf line the service sends = floe/gui.py perf_status over the
frame's report (the adapter's result keys), a snap answered, a margin's pan
a crop - and on FLOE_GTK_LOOP=legacy the Python loop still draws.
"""

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
from floe.rust_render import RustRenderWorker  # noqa: E402
from floe_oracle.cache import Cache  # noqa: E402

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
              depth=None):
        self.gen += 1
        job = {"kind": "render", "gen": self.gen, "scope": "live",
               "bbox": tuple(f["bbox"]), "view": view, "w": f["width"],
               "h": f["height"], "depth": depth, "cut_px": 1.0,
               "frames": frames, "labels": labels, "label_font_px": 14,
               "lod": False, "abstract": False, "visible": None,
               "thin": "keep", "frame_cache": True, "frame_format": "raw"}
        if view is not None:
            job["bg"] = True
        if density is not None:
            job["density"] = density
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


def queries_and_cells(s, ref, frame, temp):
    """snap, pick, the cell tree and a clip through the channel = the
    viewer's adapter's answers for the same request."""
    for n, (kind, extra) in enumerate((("snap", {}), ("pick", {"nth": 0}))):
        qid = s.query(frame["id"], kind, W / 2, H / 2, 10 if kind == "snap" else 3, **extra)
        got = answer(s, "query", lambda v, qid=qid: v["id"] == qid)
        req = got["request"]
        job = {"kind": kind, "seq": 900 + n, "x": req["x"], "y": req["y"], "r": req["r"]}
        job.update(extra)
        want = reference_answer(ref, job, kind)
        keys = ("found", "x", "y", "snap") if kind == "snap" else (
            "found", "count", "index", "layer", "datatype", "lname", "cell",
            "area", "bbox", "points")
        for key in keys:
            a, b = got.get(key), want.get(key)
            if key == "points":
                a, b = [list(p) for p in a or []], [list(p) for p in b or []]
            check(a == b, "%s %s: channel %r, adapter %r" % (kind, key, a, b))
        check(got["found"], "%s at the view's centre found nothing" % kind)
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


def gui_viewer(src, ref):
    """floe/gui.py's Viewer on the controller's loop (the default)."""
    os.environ.pop("FLOE_GTK_LOOP", None)
    from floe import gui
    gui.import_gtk()
    cache = gtkservice.ServiceCache(str(src))
    cache.load()
    v = gui.Viewer(cache, detail=2)
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
        check(tuple(last["perf"]) == gui.perf_status(report, note),
              "perf line: service %r, Python %r" % (last["perf"],
                                                   gui.perf_status(report, note)))
        missing = set(ref.result) - set(report) - {"rgba", "png", "gen"}
        check(not missing, "the report lacks the adapter's keys %s" % sorted(missing))
        check(last["perf"][1] in v.pstatus.get_text(),
              "the lower bar %r is not the frame's %r" % (v.pstatus.get_text(), last["perf"][1]))
        steps = (("a zoom", lambda: v._zoom_center(0.25), {}),
                 ("a pan", lambda: v._pan_view("Right"), {}),
                 ("the density", v._toggle_density, {"density": True}),
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
                                      depth=v._depth()), what)
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
                                      "y": int(y), "r": r}, "snap")
        check(got["kind"] == "snap" and got["seq"] == 41 and all(
            got.get(k) == want.get(k) for k in ("found", "x", "y", "snap")),
            "snap: viewer %r, adapter %r" % (got, want))
        print("viewer: %d frame(s), perf %r" % (len(frames), last["perf"][1]))
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


def gui_legacy(src):
    """FLOE_GTK_LOOP=legacy: the Python loop (floe/rust_render.py) draws."""
    os.environ["FLOE_GTK_LOOP"] = "legacy"
    try:
        from floe import gui
        cache = gtkservice.ServiceCache(str(src))
        cache.load()
        v = gui.Viewer(cache, detail=2)
        try:
            pump(v, lambda: v.last_frame is not None and v._pending is None,
                 "legacy frame")
            check(not getattr(v.worker, "controller", False),
                  "FLOE_GTK_LOOP=legacy still took the controller")
        finally:
            v._quit()
            v.window.destroy()
            cache.close()
    finally:
        os.environ.pop("FLOE_GTK_LOOP", None)


def same(a, b, what):
    if a == b:
        return
    diff = sum(1 for i in range(0, min(len(a), len(b)), 4) if a[i:i + 4] != b[i:i + 4])
    raise AssertionError("%s: the channel's frame differs from the adapter's "
                         "(%d of %d pixels, sizes %d/%d)"
                         % (what, diff, len(a) // 4, len(a), len(b)))


def main():
    os.environ["FLOE_INDEX_BIN"] = str(ROOT / "rust/target/release/floe-index")
    os.environ["FLOE_RENDERD_BIN"] = str(ROOT / "rust/target/release/floe-renderd")
    os.environ.pop("FLOE_RUST_ROUND_PAGES", None)
    with tempfile.TemporaryDirectory(prefix="floe-gtk-view-") as temp:
        src = Path(temp) / "valmini.oas"
        src.write_bytes((ROOT / "data/m1/valmini.oas").read_bytes())
        done = subprocess.run([FLOE2, "index", str(src), "--jobs", "2"], cwd=ROOT,
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
            if gtk_ready():
                gui_viewer(src, ref)
                gui_legacy(src)
        finally:
            ref.stop()
            svc.close()
    print("GTK VIEW: ALL OK")


if __name__ == "__main__":
    main()
