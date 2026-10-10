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

    def frame(self, f, density=None, labels=True, view=None):
        self.gen += 1
        job = {"kind": "render", "gen": self.gen, "scope": "live",
               "bbox": tuple(f["bbox"]), "view": view, "w": f["width"],
               "h": f["height"], "depth": None, "cut_px": 1.0,
               "frames": False, "labels": labels, "label_font_px": 14,
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
                return bytes(res["rgba"])
        raise AssertionError("reference render timeout")

    def stop(self):
        self.worker.stop()


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
            # a pan the controller snaps to the 16 px fill phase
            snap = s.edit(navigation={"kind": "pan", "x": 0.25, "y": 0.0, "snap": True})
            check(snap["state_rev"] == 2, "pan: %s" % snap["state_rev"])
            panned = until(s, final_frame(2))[-1]
            spp = (first["bbox"][2] - first["bbox"][0]) / W
            shift = (panned["bbox"][0] - first["bbox"][0]) / spp
            check(abs(shift - round(shift)) < 1e-6 and round(shift) % 16 == 0
                  and round(shift) > 0, "the pan moved %r px, not 16 px steps" % shift)
            same(panned["pixels"], ref.frame(panned), "the pan")
            # the density under the cut: its first round, then the frame
            on = s.edit(density=True)
            check(on["state"]["density"] is True and on["state_rev"] == 3,
                  "density: %s" % on["state"])
            got = until(s, final_frame(3))
            rounds = [f for f in got if f["state_rev"] == 3]
            # renderd shows pass 1 first (a round of its own), then the frame
            # with the density - the shared worker client takes both since
            # renderd 0.12.308 (the first round's line was not a frame line)
            check(len(rounds) >= 2 and not rounds[0]["final"]
                  and rounds[0]["fields"].get("density_round") == "1",
                  "density rounds: %s" % [(f["round"], f["final"]) for f in rounds])
            check([f["round"] for f in rounds] == sorted({f["round"] for f in rounds}),
                  "density rounds do not count up: %s" % [f["round"] for f in rounds])
            same(rounds[-1]["pixels"], ref.frame(rounds[-1], density=True), "the density")
            changed = sum(1 for i in range(0, len(panned["pixels"]), 4)
                          if panned["pixels"][i:i + 4] != rounds[-1]["pixels"][i:i + 4])
            check(changed > 0, "the density under the cut changed no pixel")
            print("density: %d round(s), %d pixel(s) changed" % (len(rounds), changed))
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
        finally:
            ref.stop()
            svc.close()
    print("GTK VIEW: ALL OK")


if __name__ == "__main__":
    main()
