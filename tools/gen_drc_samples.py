#!/usr/bin/env python3
"""Sample data for trying every DRC feature of floe2 by hand: a layout with
the violating shapes, an SVRF deck and its rules sidecar, a current and a
previous DRC run, user clusters, and review files of three other reviewers
and of the user's own previous review - then the numbers the viewer should
show, measured through floe2 gtk-service, in GUIDE.ko.md.

The review files are bound to their pack's fingerprint (its mtime in ns
among others), so the set is made on the machine it is used on, after the
packs - copying it elsewhere drops the reviews (the viewer refuses them as
"another DRC result"). It needs Python 3.6+ and the built binaries only (no
KLayout, no NumPy):

    python3 tools/gen_drc_samples.py                     # -> data/drc_samples
    python3 tools/gen_drc_samples.py --heavy 2000000     # a 2M-error rule
    python3 tools/gen_drc_samples.py --out /tmp/s --force --me kim

The layout's unit and the results' precision are 1000 per um (1 nm).
"""

import argparse
import json
import os
import random
import shutil
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PREC = 1000          # dbu per um, layout and results
DECK_PATH = "/calibre/run/drc/deck.svrf"   # as a Calibre run records it
M1, M2, VIA1 = (31, 0), (32, 0), (51, 0)
DATE = "Oct 12 09:00:00 2026"


# ---------------------------------------------------------------- binaries
def binary(name, env, given):
    if given:
        return os.path.abspath(given)
    if os.environ.get(env):
        return os.environ[env]
    local = os.path.join(ROOT, "rust", "target", "release", name)
    if os.path.exists(local):
        return local
    found = shutil.which(name)
    if found:
        return found
    sys.exit("%s not found: build it (cd rust && cargo build --release) or pass --%s"
             % (name, name))


def run(args):
    r = subprocess.run(args, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                       universal_newlines=True)
    if r.returncode:
        sys.exit("%s failed (%d):\n%s%s" % (" ".join(args), r.returncode, r.stdout, r.stderr))
    return r


# ---------------------------------------------------------------- geometry
# One error = (kind, points): 'e' points are edge endpoints in pairs, 'p' a
# polygon. `shapes` = the layout shapes under it: (layer, [points]).

def space(x, y, gap, length=200, width=100, layer=M1):
    """Two facing lines `gap` apart: the error is their facing edges."""
    err = ("e", [(x, y), (x + length, y), (x, y + gap), (x + length, y + gap)])
    shapes = [(layer, rect(x, y - width, length, width)),
              (layer, rect(x, y + gap, length, width))]
    return err, shapes


def rect(x, y, w, h):
    return [(x, y), (x + w, y), (x + w, y + h), (x, y + h)]


def width(x, y, w, length=300, vertical=False):
    pts = rect(x, y, length, w) if not vertical else rect(x, y, w, length)
    return ("p", pts), [(M1, pts)]


def area(x, y, kind, s):
    """A small M1 island; `s` scales it (area < 0.01 um^2)."""
    if kind == 0:
        pts = rect(x, y, int(80 * s), 100)
    elif kind == 1:
        pts = [(x, y), (x + int(120 * s), y), (x, y + 120)]
    else:
        a, b = int(100 * s), 50
        pts = [(x, y), (x + a, y), (x + a, y + b), (x + b, y + b), (x + b, y + a), (x, y + a)]
    return ("p", pts), [(M1, pts)]


def enclosure(x, y, e, side):
    """A 50 nm via in an M1 pad, enclosed by `e` on one side."""
    v = 50
    big = 60
    left = e if side == 0 else big
    right = e if side == 1 else big
    via = rect(x, y, v, v)
    pad = rect(x - left, y - big, v + left + right, v + 2 * big)
    if side == 0:
        err = [(x - e, y), (x - e, y + v), (x, y), (x, y + v)]
    else:
        err = [(x + v, y), (x + v, y + v), (x + v + e, y), (x + v + e, y + v)]
    return ("e", err), [(M1, pad), (VIA1, via)]


def notch(x, y, n):
    """An M2 U whose prongs are `n` apart."""
    pts = [(x, y), (x + 200 + n, y), (x + 200 + n, y + 300), (x + 100 + n, y + 300),
           (x + 100 + n, y + 100), (x + 100, y + 100), (x + 100, y + 300), (x, y + 300)]
    err = ("e", [(x + 100, y + 100), (x + 100, y + 300),
                 (x + 100 + n, y + 100), (x + 100 + n, y + 300)])
    return err, [(M2, pts)]


def overlap(x, y, o):
    """M1 and M2 overlapping by `o`."""
    err = ("e", [(x + 300 - o, y), (x + 300 - o, y + 100), (x + 300, y), (x + 300, y + 100)])
    return err, [(M1, rect(x, y, 300, 100)), (M2, rect(x + 300 - o, y, 300, 100))]


def grid(n, x0, y0, cols, pitch=2000):
    for i in range(n):
        yield i, x0 + (i % cols) * pitch, y0 + (i // cols) * pitch


# ---------------------------------------------------------------- the runs
class Run(object):
    """Rules in order: (name, title lines, [(error, shapes)])."""

    def __init__(self):
        self.rules = []

    def add(self, name, lines, errors):
        self.rules.append((name, lines, errors))

    def write(self, path):
        with open(path, "w") as out:
            out.write("CHIP %d\n" % PREC)
            for name, lines, errors in self.rules:
                desc = ["Rule File Pathname: %s" % DECK_PATH,
                        "Rule File Title: floe2 DRC sample deck"] + lines
                out.write("%s\n%d %d %d %s\n" % (name, len(errors), len(errors), len(desc), DATE))
                for line in desc:
                    out.write(line + "\n")
                for i, ((kind, pts), _shapes) in enumerate(errors):
                    if kind == "e":
                        out.write("e %d %d\n" % (i + 1, len(pts) // 2))
                        for j in range(0, len(pts), 2):
                            out.write("%d %d %d %d\n" % (pts[j] + pts[j + 1]))
                    else:
                        out.write("p %d %d\n" % (i + 1, len(pts)))
                        for p in pts:
                            out.write("%d %d\n" % p)


def make_runs(heavy, seed):
    """run2 (current) and run1 (previous): run1 = run2 with fixes, moves,
    duplicates, reordering and renamed rules, so that migration finds exact,
    partial, ambiguous and missing matches. The review plans name errors by
    their run-local index."""
    rng = random.Random(seed)
    s1 = [space(x, y, 50 + i % 20) for i, x, y in grid(3000, 100000, 100000, 60)]
    w1 = [width(x, y, 40 + i % 20, vertical=bool(i % 4 == 3))
          for i, x, y in grid(1500, 250000, 100000, 50)]
    s2 = [space(x, y, 60 + i % 20, length=300, layer=M2)
          for i, x, y in grid(heavy, 100000, 400000, 500)]
    en = [enclosure(x, y, 10 + i % 20, i % 2) for i, x, y in grid(800, 400000, 100000, 40)]
    ar = [area(x, y, i % 3, 0.6 + (i % 10) * 0.04) for i, x, y in grid(400, 500000, 100000, 20)]
    nt = [notch(x, y, 20 + i % 30) for i, x, y in grid(300, 560000, 100000, 20)]
    ov = [overlap(x, y, 10 + i % 30) for i, x, y in grid(200, 620000, 100000, 10)]
    dup = [space(x, y, 90) for i, x, y in grid(5, 700000, 100000, 5)]
    new = [space(x, y, 55) for i, x, y in grid(20, 720000, 100000, 10)]
    old = [space(x, y, 45) for i, x, y in grid(30, 740000, 100000, 10)]

    run2 = Run()
    run2.add("M1.S.1", ["Metal1 spacing < 0.070 um; gaps 0.050..0.069 cycle (CD delta groups).",
                        "Clusters: run2/results.db.clusters."], s1)
    run2.add("M1.W.1", ["Metal1 width < 0.060 um; widths 0.040..0.059, every 4th vertical."], w1)
    run2.add("M2.S.1", ["Metal2 spacing < 0.080 um; the large rule (markers, selection,",
                        "drc-prepare)."], s2)
    run2.add("V1.EN.1", ["Via1 enclosure by Metal1 < 0.030 um, left or right side."], en)
    run2.add("M1.A.1", ["Metal1 area < 0.010 um^2: rectangles, triangles, L-shapes."], ar)
    run2.add("M2.NOTCH.1", ["Metal2 notch < 0.050 um."], nt)
    run2.add("M1.M2.OVL", ["Metal1/Metal2 overlap < 0.040 um."], ov)
    run2.add("DUP.NAME", ["The same rule name twice: never matched by migration."], dup)
    run2.add("DUP.NAME", ["The same rule name twice: never matched by migration."], dup)
    run2.add("NEW.RULE", ["Only in this run."], new)
    run2.add("EMPTY.RULE", ["No result in this run."], [])

    # run1: M1.S.1 - the first 2800 the same (shuffled), the last 200 at
    # their old place (moved since), 150 fixed since, the first 10 twice
    keep = list(range(2800))
    rng.shuffle(keep)
    s1_old = [(s1[i], ("same", i)) for i in keep]
    s1_old += [(space(e[0][1][0][0] + 7, e[0][1][0][1], 50 + i % 20), ("moved", i))
               for i, e in enumerate(s1[2800:], 2800)]
    s1_old += [(space(x, y, 65), ("fixed", i)) for i, x, y in grid(150, 100000, 220000, 60)]
    s1_old += [(s1[i], ("dup", i)) for i in range(10)]
    rng.shuffle(s1_old)
    # M2.S.1: reversed, every 100th fixed since, 500 new since
    s2_old = [(e, ("same", i)) for i, e in enumerate(s2) if i % 100 != 99][::-1]
    below = 400000 + (heavy + 499) // 500 * 2000 + 100000   # past the large rule
    s2_old += [(space(x, y, 70, length=300, layer=M2), ("fixed", i))
               for i, x, y in grid(500, 100000, below, 500)]
    run1 = Run()
    run1.add("M2.S.1", ["Metal2 spacing < 0.080 um."], [e for e, _ in s2_old])
    run1.add("M1.S.1", ["Metal1 spacing < 0.070 um."], [e for e, _ in s1_old])
    run1.add("M1.W.1", ["Metal1 width < 0.060 um."], w1)
    run1.add("V1.EN.1", ["Via1 enclosure by Metal1 < 0.030 um."], en)
    run1.add("M1.A.1", ["Metal1 area < 0.010 um^2."], ar)
    run1.add("M2.NOTCH.1", ["Metal2 notch < 0.050 um."], nt)
    run1.add("M1.M2.OVL", ["Metal1/Metal2 overlap < 0.040 um."], ov)
    run1.add("DUP.NAME", ["The same rule name twice."], dup)
    run1.add("DUP.NAME", ["The same rule name twice."], dup)
    run1.add("OLD.ONLY", ["Only in the previous run (renamed or dropped since)."], old)
    tags = {"M1.S.1": [t for _, t in s1_old], "M2.S.1": [t for _, t in s2_old]}
    return run2, run1, tags


# ---------------------------------------------------------------- the deck
DECK = """// floe2 DRC sample deck (tools/gen_drc_samples.py) - not a Calibre deck.
// floe-index svrf deck.svrf -> deck.svrf.rules.json (copied next to each
// results.db: the viewer finds it by the deck's name).
PRECISION 1000

LAYER M1 31
LAYER M2 32
LAYER VIA1 51

M1.S.1 {
    @ Metal1 spacing.
    EXTERNAL M1 < 0.070
}

M1.W.1 {
    @ Metal1 minimum width: the smaller violating side is the CD.
    INTERNAL M1 < 0.060
}

M2.S.1 {
    @ Metal2 spacing (the large rule).
    EXTERNAL M2 < 0.080
}

V1.EN.1 {
    @ Via1 enclosure by Metal1.
    ENCLOSURE VIA1 M1 < 0.030
}

M1.A.1 {
    @ Metal1 minimum area: the polygon area is labelled.
    AREA M1 < 0.010
}

M2.NOTCH.1 {
    @ Metal2 notch.
    EXTERNAL M2 < 0.050
        NOTCH
}

M1.M2.OVL {
    @ Two-layer INTERNAL is an overlap.
    INTERNAL M1 M2 < 0.040
}

NEW.RULE {
    @ Only in the current run.
    EXTERNAL M1 < 0.060
}
"""


def clusters_text(heavy):
    rows = max(1, heavy // 500)
    half = max(1, rows // 2) * 500
    return """# floe2 DRC sample clusters (tools/gen_drc_samples.py).
# Numbers are the rule-local error numbers (from 1); ranges are inclusive.
# The errors no cluster names are "Unclustered".

[M1.S.1]
Block A rows 1-17 = 1-1000
Block B rows 18-34 = 1001-2000
Edge cases = 2001-2100, 2501-2600, 2777, 2877, 2977

[M2.S.1]
Upper half = 1-%d
""" % half


# ---------------------------------------------------------------- the layout
def uint(v):
    out = bytearray()
    while True:
        b = v & 0x7F
        v >>= 7
        if v:
            out.append(b | 0x80)
        else:
            out.append(b)
            return bytes(out)


def sint(v):
    return uint((abs(v) << 1) | (1 if v < 0 else 0))


def bstr(b):
    return uint(len(b)) + b


def gdelta(dx, dy):
    if dy == 0:
        return uint((abs(dx) << 4) | ((2 if dx < 0 else 0) << 1))
    if dx == 0:
        return uint((abs(dy) << 4) | ((3 if dy < 0 else 1) << 1))
    return uint((abs(dx) << 2) | (2 if dx < 0 else 0) | 1) + sint(dy)


def write_layout(path, shapes):
    """A one-cell OASIS file (unit 1000/um): rectangles as RECTANGLE,
    others as POLYGON with a g-delta point list."""
    with open(path, "wb") as out:
        out.write(b"%SEMI-OASIS\r\n")
        out.write(uint(1) + bstr(b"1.0") + uint(0) + uint(PREC) + uint(0) + uint(0) * 12)
        out.write(uint(3) + bstr(b"CHIP"))
        for name, (layer, dt) in (("M1", M1), ("M2", M2), ("VIA1", VIA1)):
            out.write(uint(11) + bstr(name.encode()) + uint(3) + uint(layer) + uint(3) + uint(dt))
        out.write(uint(13) + uint(0))
        buf = []
        for (layer, dt), pts in shapes:
            xs = [p[0] for p in pts]
            ys = [p[1] for p in pts]
            if len(pts) == 4 and len(set(xs)) == 2 and len(set(ys)) == 2:
                x0, y0 = min(xs), min(ys)
                buf.append(uint(20) + b"\x7b" + uint(layer) + uint(dt) + uint(max(xs) - x0)
                           + uint(max(ys) - y0) + sint(x0) + sint(y0))
            else:
                deltas = b"".join(gdelta(b[0] - a[0], b[1] - a[1]) for a, b in zip(pts, pts[1:]))
                buf.append(uint(21) + b"\x3b" + uint(layer) + uint(dt) + uint(4)
                           + uint(len(pts) - 1) + deltas + sint(pts[0][0]) + sint(pts[0][1]))
            if len(buf) >= 65536:
                out.write(b"".join(buf))
                buf = []
        out.write(b"".join(buf))
        end = uint(2)
        end += bstr(b"\0" * (256 - len(end) - 2 - 1)) + uint(0)
        out.write(end)


# ---------------------------------------------------------------- the service
class Service(object):
    """`floe2 gtk-service`: one JSON line a request, replies and events."""

    def __init__(self, floe2):
        env = dict(os.environ)
        env.pop("FLOE_DRC_REVIEW_ROOT", None)
        self.proc = subprocess.Popen([floe2, "gtk-service"], stdin=subprocess.PIPE,
                                     stdout=subprocess.PIPE, universal_newlines=True,
                                     bufsize=1, env=env)
        self.seq = 0
        self.events = []

    def _line(self):
        line = self.proc.stdout.readline()
        if not line:
            sys.exit("floe2 gtk-service ended")
        return json.loads(line)

    def __call__(self, op, **fields):
        self.seq += 1
        fields.update(id=self.seq, op=op)
        self.proc.stdin.write(json.dumps(fields) + "\n")
        self.proc.stdin.flush()
        while True:
            m = self._line()
            if "event" in m:
                self.events.append(m)
            elif m.get("id") == self.seq:
                if m.get("error"):
                    raise RuntimeError("%s: %s" % (op, m["error"].get("message")))
                return m.get("result")

    def job(self, handle, op, **fields):
        job = self(op, handle=handle, **fields)["job"]
        while True:
            for i, e in enumerate(self.events):
                if e.get("job") == job and e.get("state") in ("done", "error", "cancelled"):
                    del self.events[i]
                    if e["state"] != "done":
                        raise RuntimeError("%s: %s" % (op, e.get("message")))
                    return e
            self.events.append(self._line())

    def visit(self, handle, check, errors):
        for ei in errors:
            self("drc_visit", handle=handle, check=check, error=ei)
        want = len(errors)
        while want:
            got = [e for e in self.events if e.get("kind") == "visit"]
            for e in got:
                self.events.remove(e)
                if e.get("state") != "done":
                    raise RuntimeError("visit: %s" % e.get("message"))
            want -= len(got)
            if want:
                self.events.append(self._line())

    def close(self):
        self.proc.stdin.close()
        self.proc.wait()


def review(svc, db, reviewer, plan):
    """Apply `plan` {rule: (waive, visit, not-required)} as `reviewer`."""
    opened = svc("drc_open", path=db, mode="load", reviewer=reviewer)
    h = opened["handle"]
    index = {}
    for ci, c in enumerate(opened["checks"]):
        index.setdefault(c["name"], ci)
    for name, (waive, visit, nr) in plan.items():
        ci = index[name]
        for start in range(0, len(waive), 50000):
            svc("drc_set_status", handle=h, check=ci, errors=waive[start:start + 50000], status=1)
        svc.visit(h, ci, visit)
        if nr:
            svc.job(h, "drc_review", check=ci, action="not-required", errors=nr, on=True)
    return h, opened


# ---------------------------------------------------------------- main
def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--out", default=os.path.join(ROOT, "data", "drc_samples"))
    ap.add_argument("--heavy", type=int, default=200000,
                    help="errors of the large rule M2.S.1 (default 200000)")
    ap.add_argument("--me", help="your reviewer tag (default: the one floe2 picks here)")
    ap.add_argument("--seed", type=int, default=12)
    ap.add_argument("--force", action="store_true", help="replace an existing --out")
    ap.add_argument("--no-index", action="store_true", help="do not index chip.oas")
    ap.add_argument("--floe2")
    ap.add_argument("--floe-index")
    a = ap.parse_args()
    if a.heavy < 1000:
        sys.exit("--heavy must be at least 1000")
    floe2 = binary("floe2", "FLOE2_BIN", a.floe2)
    index = binary("floe-index", "FLOE_INDEX_BIN", a.floe_index)
    out = os.path.abspath(a.out)
    if os.path.exists(out):
        if not a.force:
            sys.exit("%s exists: pass --force to replace it" % out)
        shutil.rmtree(out)
    run1_dir, run2_dir = os.path.join(out, "run1"), os.path.join(out, "run2")
    peers = os.path.join(run2_dir, "peers")
    for d in (run1_dir, peers):
        os.makedirs(d)
    t0 = time.time()

    def say(text):
        print("[%5.1f s] %s" % (time.time() - t0, text), flush=True)

    run2, run1, tags = make_runs(a.heavy, a.seed)
    deck = os.path.join(out, "deck.svrf")
    with open(deck, "w") as f:
        f.write(DECK)
    run([index, "svrf", deck])
    for d in (run1_dir, run2_dir):
        shutil.copy(deck + ".rules.json", os.path.join(d, "deck.svrf.rules.json"))
    say("deck.svrf and its rules sidecar")
    db2, db1 = os.path.join(run2_dir, "results.db"), os.path.join(run1_dir, "results.db")
    run2.write(db2)
    run1.write(db1)
    with open(db2 + ".clusters", "w") as f:
        f.write(clusters_text(a.heavy))
    for db in (db2, db1):
        run([index, "drc", db])
    say("run2/results.db (current), run1/results.db (previous), packed")
    shapes = [s for _n, _l, errors in run2.rules for _e, sh in errors for s in sh]
    chip = os.path.join(out, "chip.oas")
    write_layout(chip, shapes)
    if not a.no_index:
        run([floe2, "index", chip])
    say("chip.oas (%d shapes)%s" % (len(shapes), "" if a.no_index else ", indexed"))

    svc = Service(floe2)
    try:
        me = a.me or svc("drc_open", path=db2, mode="load")["reviewer"]
        # the previous run, reviewed by me
        t1 = tags["M1.S.1"]
        first = {i: k for k, (kind, i) in enumerate(t1) if kind == "same" and i < 1200}
        dup = {i: k for k, (kind, i) in enumerate(t1) if kind == "dup"}
        s1_visit = sorted(first.values()) + sorted(dup.values())
        s1_waive = [first[i] for i in sorted(first) if i % 3 == 0]
        # duplicates of 0..4 disagree on the waive (partial consensus)
        s1_waive += [dup[i] for i in sorted(dup) if (i % 3 == 0) != (i < 5)]
        s1_nr = [first[i] for i in sorted(first) if 1000 <= i < 1100]
        moved = [k for k, (kind, i) in enumerate(t1) if kind == "moved" and i < 2900]
        fixed = [k for k, (kind, i) in enumerate(t1) if kind == "fixed"][:50]
        s1_visit += moved + fixed
        s1_waive += moved + fixed
        n2 = len(tags["M2.S.1"])
        plan1 = {
            "M1.S.1": (sorted(set(s1_waive)), sorted(set(s1_visit)), s1_nr),
            "M2.S.1": (list(range(50000, min(n2, 60000))), list(range(200)),
                       list(range(min(n2, 50000)))),
            "M1.W.1": (list(range(100)), list(range(50)), list(range(100, 150))),
            "OLD.ONLY": (list(range(10)), list(range(5)), []),
            "DUP.NAME": ([0, 1], [0], []),
        }
        h, _ = review(svc, db1, me, plan1)
        svc.job(h, "drc_team_export", path=os.path.join(run1_dir, "my-review.floereview"))
        svc("drc_waive_export", handle=h, path=os.path.join(run1_dir, "my-waives.waive"))
        svc("drc_close", handle=h)
        say("run1: %s's previous review (+ my-review.floereview, my-waives.waive)" % me)
        # the current run, reviewed by alice, bob and carol
        heavy = a.heavy
        plan_a = {"M1.S.1": ([i for i in range(800) if i % 2 == 0], list(range(800)),
                             list(range(700, 800))),
                  "M2.S.1": ([], list(range(100)), list(range(min(heavy, 10000))))}
        plan_b = {"M1.S.1": ([i for i in range(500, 1300) if i % 3 == 0], list(range(500, 1300)),
                             list(range(750, 850))),
                  "M2.S.1": (list(range(50)), list(range(100)), [])}
        plan_c = {"M1.S.1": (list(range(1900, 2000)), [], [])}
        h, _ = review(svc, db2, "alice", plan_a)
        svc.job(h, "drc_team_export", path=os.path.join(peers, "alice.floereview"))
        svc("drc_close", handle=h)
        h, _ = review(svc, db2, "bob", plan_b)
        svc("drc_close", handle=h)
        h, _ = review(svc, db2, "carol", plan_c)
        svc("drc_close", handle=h)
        for name in (".results.db.review.bob.sqlite3", ".results.db.waive.bob",
                     ".results.db.waive.carol"):
            shutil.copy2(os.path.join(run2_dir, name), os.path.join(peers, name[1:]))
        say("run2: alice (.floereview), bob (review SQLite + waive), carol (waive only)")

        # what the viewer should show: the reviewers window and migration
        opened = svc("drc_open", path=db2, mode="load", reviewer=me)
        h = opened["handle"]
        names = [c["name"] for c in opened["checks"]]
        loaded = svc.job(h, "drc_team_load", paths=[
            os.path.join(peers, "alice.floereview"),
            os.path.join(peers, "results.db.review.bob.sqlite3"),
            os.path.join(peers, "results.db.waive.carol")])
        sids = [x["snapshot"] for x in loaded["loaded"]]
        agg = svc.job(h, "drc_team_aggregate", sources=sids)
        team = [svc("drc_snapshot_counts", handle=h, snapshot=agg["snapshot"], check=ci)
                for ci in range(len(names))]
        team_db = svc("drc_snapshot_counts", handle=h, snapshot=agg["snapshot"])
        plan = svc.job(h, "drc_migrate_prepare", previous=db1)
        summary = plan["summary"]
        per_rule = [svc("drc_plan_counts", handle=h, plan=plan["plan"], check=ci)
                    for ci in range(len(names))]
        svc("drc_close", handle=h)
    finally:
        svc.close()
    # opening as `me` made an empty waive autosave; drop it (fresh start)
    for name in os.listdir(run2_dir):
        if name in (".results.db.waive.%s" % me, ".results.db.review.%s.sqlite3" % me):
            os.unlink(os.path.join(run2_dir, name))
    say("measured: %d conflicts, migration matched %d" % (team_db["conflicts"], summary["matched"]))
    write_guide(out, me, a.heavy, names, team, team_db, summary, per_rule)
    with open(os.path.join(out, "expected.json"), "w") as f:
        json.dump({"me": me, "heavy": a.heavy, "team": team_db, "migration": summary,
                   "rules": [{"name": n, "team": team[ci], "migration": per_rule[ci]}
                             for ci, n in enumerate(names)]}, f, indent=1)
    say("GUIDE.ko.md, expected.json")
    print("\nsamples in %s - open them with:\n  floe2 view %s --drc %s"
          % (out, chip, db2))


def write_guide(out, me, heavy, names, team, team_db, summary, per_rule):
    def row(ci):
        t, p = team[ci], per_rule[ci]
        return "| %s | %d | %d | %d | %d | %d | %d | %d | %d |" % (
            names[ci], t["total"], t["visited"], t["waived"], t["not_required"], t["conflicts"],
            p["matched"], p["partial"], p["ambiguous"])
    rows = "\n".join(row(ci) for ci in range(len(names)))
    text = GUIDE.format(me=me, heavy=heavy, rows=rows, conflicts=team_db["conflicts"],
                        visited=team_db["visited"], reviewers=team_db.get("reviewers", 0),
                        **summary)
    with open(os.path.join(out, "GUIDE.ko.md"), "w") as f:
        f.write(text)


GUIDE = """# floe2 DRC 샘플 (tools/gen_drc_samples.py)

이 폴더는 생성한 머신에서 그대로 쓴다. 리뷰 파일은 pack 지문(파일 mtime 포함)에
묶여 있어 다른 곳으로 복사하면 "another DRC result"로 거절된다 - 그때는 그 머신에서
다시 생성한다. 내 리뷰어 태그: `{me}` (이 이름으로 이전 실행 리뷰를 만들었다; 다르면
`FLOE_REVIEWER={me}`로 실행하거나 `--me`로 다시 생성).

```sh
floe2 view chip.oas --drc run2/results.db          # 현재 실행
floe2 view chip.oas --drc run1/results.db          # 이전 실행
```

| 파일 | 내용 |
|---|---|
| chip.oas | M1(31/0)·M2(32/0)·VIA1(51/0) - 에러 자리의 실제 도형 |
| deck.svrf, run*/deck.svrf.rules.json | 룰 제약(뷰어가 덱 이름으로 자동 연결) |
| run2/results.db | 현재 실행: 11개 룰(이름 중복 1쌍, 결과 없는 룰 1개 포함) |
| run2/results.db.clusters | M1.S.1·M2.S.1 클러스터 |
| run2/peers/ | 동료 리뷰: alice.floereview, bob의 review SQLite + waive, carol의 waive만 |
| run1/results.db | 이전 실행(이동·수정·중복·순서·이름이 바뀐 룰) |
| run1/my-review.floereview, my-waives.waive | 내 이전 리뷰를 따로 보관한 파일 |

## 1. 탐색·CD·면적·클러스터·델타

- 룰 목록: M1.S.1 3000, M1.W.1 1500, M2.S.1 {heavy}, V1.EN.1 800, M1.A.1 400,
  M2.NOTCH.1 300, M1.M2.OVL 200, DUP.NAME 5 × 2, NEW.RULE 20, EMPTY.RULE 0.
- 더블클릭·`.`/`,`: 에러로 이동, CD 룰러와 상세의 `measured CD … vs < 0.070 · Δ … (%)`.
  M1.W.1은 4개 중 1개가 세로 사각형, M1.A.1은 면적 라벨(`area 0.00… µm²`),
  V1.EN.1은 좌/우 한쪽 enclosure, M1.M2.OVL은 두 레이어 overlap.
- 클러스터: M1.S.1 아래 Block A/B, Edge cases, Unclustered; M2.S.1 아래 Upper half.
- CD 델타: M1.S.1(간격 0.050~0.069의 20단계)에서 절대/비율 그룹, 자동/수동 스텝.

## 2. 대용량: 마커·박스 선택·대량 waive·drc-prepare (M2.S.1, {heavy}개)

- 룰을 고르면 전체 범위 원형 마커가 그려지고 정보줄에 처리 수가 보인다.
- `e`로 박스 선택(개수 제한 없음), `w`로 선택 전체 waive(서비스 작업).
- `floe2 drc-prepare run2/results.db --svrf run2/deck.svrf.rules.json`로 중심·CD 캐시를
  미리 만든 뒤 다시 열면 바로 그려진다.

## 3. 방문·검토 제외·통계 (현재 실행, 내 리뷰는 비어 있다)

- 에러를 열어 도형이 화면에 그려지면 목록 번호 앞에 `V`, 우클릭
  `mark not for review`로 `X`. Tab으로 오버레이를 숨긴 채 연 에러는 방문이 아니다.
- 패널의 Whole DB / Whole rule 통계, `export review statistics…`(CSV).

## 4. 공동 검토 (DRC > reviewers…)

`Load peer work…`로 `run2/peers/`의 세 파일을 불러온다(숨김 파일이 아니다).
bob의 SQLite는 옆의 `results.db.waive.bob`과 짝지어 읽히고, carol은 waive만 있어
V/NR이 `—`(모름)이다. `Refresh`(나 + 3명, 내 리뷰가 비어 있을 때) 기대값:

- 전체: reviewers {reviewers} · visited {visited} · conflicts {conflicts}
- M1.S.1에서 alice(0~799 방문, 짝수 waive, 700~799 제외)와 bob(500~1299 방문,
  3의 배수 waive, 750~849 제외)이 겹치는 500~799가 conflict 후보다.
- `Apply all peer work safely` 뒤 내가 방문한 에러의 다른 판단은 건너뛴다(먼저 몇 개를
  열고 waive해 두면 import protected가 생긴다). 메인 목록 `Conflict only`.

| 룰 | 전체 | 팀 visited | waived | 제외 | conflicts | 승계 일치 | 부분 합의 | 모호 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
{rows}

## 5. 이전 실행 리뷰 승계 (DRC > review from previous run…)

`Previous results…`에서 `run1/results.db`를 고르고(검토 파일은 자동: `{me}`의 파일)
`Compare geometry`. 기대값(내 리뷰가 비어 있을 때):

- 일치(exact) {matched} · 부분 합의 {partial} · 모호 {ambiguous} · 옮길 이전 리뷰가
  없는 에러 {unmatched}(이전 리뷰가 없던 에러, 이동·새로 생긴 에러)
- 이름이 없거나 중복이라 옮기지 않는 이전 작업 {no_rule_match_source_work}
  (OLD.ONLY, DUP.NAME), 이전 리뷰 작업 전체 {source_work}
- M1.S.1: 0~1199 중 이전 실행에 그대로 있던 것이 일치, 0~4는 이전 중복끼리 waive가
  달라 부분 합의(`—`), 2800~2899는 이동해서 대응 없음.
- `Apply exact matches` 뒤 방문(V)·waive·제외가 새 번호로 옮겨진다.
- `Review file…`로 `run1/my-review.floereview`나 `run1/my-waives.waive`(waive만,
  V/NR 모름)를 지정해도 된다.

명령줄의 같은 매처: `floe2 drc-match`(SPEC-FORMATS `.floereview` 절).
"""


if __name__ == "__main__":
    main()
