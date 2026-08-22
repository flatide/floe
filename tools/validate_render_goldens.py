"""Rasterization goldens for the rust-renderer replacement (PX1-PX5).

KLayout is the ORACLE: micro-fixtures are rendered to binary
per-layer coverage images under exactly controlled viewports. The
external rust renderer renders the same fixtures/views and is
compared under the FIXED pixel policy (docs/RENDERER-TESTS.ko.md
section 4):

  P-a  diff pixels are allowed ONLY inside the 1px boundary band of
       the golden (Chebyshev distance <= 1 from a golden edge);
       any interior diff is a hard failure
  P-b  no golden connected component may vanish (overlap >= 1px
       required, regardless of size - catches 1px features whose
       diff would otherwise hide inside the band)
  P-c  area drift |on(cand) - on(gold)| <= max(16, 0.75 x golden
       edge-pixel count) - permits a half-open fill-convention
       offset (~0.5 x perimeter), rejects systematic 1px growth on
       ALL sides (~1.3 x perimeter) and larger

Cases:
  PX1  half-pixel / quarter-pixel / negative-origin viewport
       rounding over an aligned box grid
  PX2  edge slopes: horizontal, vertical, 45 deg and arbitrary
       (atan 1/3, atan 2/7, atan 3) wedges
  PX3  1..8 px line widths, H/V/45 deg
  PX4  concave polygons: L, U, plus, comb, notch (vertex/join px)
  PX5  PATH flush/square/round/asymmetric extensions + 45/90/135
       deg bends

Goldens are NOT committed: klayout rasterization is version- and
host-dependent, so they are baked locally (manifest records the
klayout version; a version change rebakes). Self-check re-renders
and demands EXACT equality vs the baked set, then proves the policy
discriminates via synthetic perturbations (shift 1px pass / 2px
fail, dilate fail, vanished component fail).

Usage:
  validate_render_goldens.py [workdir]            bake + self-check
  validate_render_goldens.py [workdir] --candidate DIR
      compare DIR/<name>.png (external renderer output, same views)
"""
import json
import os
import sys
import tempfile

import numpy as np

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


# ------------------------------------------------------- fixtures

def _fixtures(db):
    """[(case, builder(layout, top, layer_index))] - dbu 0.001 um"""

    def px1(ly, top, l1):
        for base in ((0.0, 0.0), (-20.0, -20.0)):
            for i in range(8):
                for j in range(8):
                    x = base[0] + i * 2.0
                    y = base[1] + j * 2.0
                    top.shapes(l1).insert(
                        db.DBox(x, y, x + 1.0, y + 1.0))

    def px2(ly, top, l1):
        import math
        angs = [0.0, 90.0, 45.0,
                math.degrees(math.atan(1 / 3)),
                math.degrees(math.atan(2 / 7)),
                math.degrees(math.atan(3.0)),
                30.0, 135.0]
        for k, a in enumerate(angs):
            a0 = math.radians(a)
            a1 = math.radians(a + 5.0)
            pts = [db.DPoint(0, 0),
                   db.DPoint(10 * math.cos(a0), 10 * math.sin(a0)),
                   db.DPoint(10 * math.cos(a1), 10 * math.sin(a1))]
            top.shapes(l1).insert(db.DPolygon(pts))

    def px3(ly, top, l1):
        for k in range(1, 9):
            w = k * 0.1        # 1..8 px at 10 px/um
            y = k * 2.0
            top.shapes(l1).insert(db.DPath(
                [db.DPoint(0, y), db.DPoint(12, y)], w))
            x = k * 2.0
            top.shapes(l1).insert(db.DPath(
                [db.DPoint(x, 20), db.DPoint(x, 32)], w))
            d = k * 3.0
            top.shapes(l1).insert(db.DPath(
                [db.DPoint(14 + d, 20), db.DPoint(20 + d, 26)], w))

    def px4(ly, top, l1):
        def poly(pts, dx, dy):
            top.shapes(l1).insert(db.DPolygon(
                [db.DPoint(x + dx, y + dy) for (x, y) in pts]))
        poly([(0, 0), (6, 0), (6, 2), (2, 2), (2, 6), (0, 6)],
             0, 0)                                   # L
        poly([(0, 0), (6, 0), (6, 6), (4, 6), (4, 2), (2, 2),
              (2, 6), (0, 6)], 10, 0)                # U
        poly([(2, 0), (4, 0), (4, 2), (6, 2), (6, 4), (4, 4),
              (4, 6), (2, 6), (2, 4), (0, 4), (0, 2), (2, 2)],
             20, 0)                                  # plus
        poly([(0, 0), (10, 0), (10, 2), (8, 2), (8, 1), (6, 1),
              (6, 2), (4, 2), (4, 1), (2, 1), (2, 2), (0, 2)],
             0, 10)                                  # comb
        poly([(0, 0), (8, 0), (8, 6), (4.5, 0.8), (4, 6), (0, 6)],
             14, 10)                                 # acute notch

    def px5(ly, top, l1):
        hw = 0.25
        rows = [
            dict(b=0.0, e=0.0, rnd=False),           # flush
            dict(b=hw, e=hw, rnd=False),             # square ext
            dict(b=0.0, e=0.0, rnd=True),            # round caps
            dict(b=hw, e=0.1, rnd=False),            # asymmetric
        ]
        for k, r in enumerate(rows):
            y = k * 3.0
            top.shapes(l1).insert(db.DPath(
                [db.DPoint(0, y), db.DPoint(8, y)],
                2 * hw, r["b"], r["e"], r["rnd"]))
            # bends: 90 / 45 / 135 deg joins with the same caps
            top.shapes(l1).insert(db.DPath(
                [db.DPoint(10, y), db.DPoint(14, y),
                 db.DPoint(14, y + 2)],
                2 * hw, r["b"], r["e"], r["rnd"]))
            top.shapes(l1).insert(db.DPath(
                [db.DPoint(17, y), db.DPoint(20, y),
                 db.DPoint(22, y + 2)],
                2 * hw, r["b"], r["e"], r["rnd"]))
            top.shapes(l1).insert(db.DPath(
                [db.DPoint(25, y), db.DPoint(28, y),
                 db.DPoint(26, y + 2)],
                2 * hw, r["b"], r["e"], r["rnd"]))

    return [("px1", px1), ("px2", px2), ("px3", px3),
            ("px4", px4), ("px5", px5)]


# (name, case, view(x0,y0,x1,y1) um, width px, height px)
RENDERS = [
    # PX1: aligned / half px / quarter px / negative origin /
    # origin-crossing (10 px/um)
    ("px1_align", "px1", (0.0, 0.0, 16.0, 16.0), 160, 160),
    ("px1_half", "px1", (0.05, 0.05, 16.05, 16.05), 160, 160),
    ("px1_quarter", "px1", (0.025, 0.025, 16.025, 16.025), 160, 160),
    ("px1_neg", "px1", (-20.05, -20.05, -4.05, -4.05), 160, 160),
    ("px1_cross", "px1", (-8.0, -8.0, 8.0, 8.0), 160, 160),
    # PX2: slope fan (8 px/um)
    ("px2_fan", "px2", (-11.0, -11.0, 11.0, 11.0), 176, 176),
    ("px2_fan_half", "px2", (-10.9375, -10.9375, 11.0625, 11.0625),
     176, 176),
    # PX3: widths (10 px/um)
    ("px3_widths", "px3", (-1.0, -1.0, 39.0, 39.0), 400, 400),
    ("px3_widths_half", "px3", (-0.95, -0.95, 39.05, 39.05),
     400, 400),
    # PX4: concave (10 px/um)
    ("px4_concave", "px4", (-1.0, -1.0, 25.0, 19.0), 260, 200),
    ("px4_concave_half", "px4", (-0.95, -0.95, 25.05, 19.05),
     260, 200),
    # PX5: caps/joins (10 px/um)
    ("px5_paths", "px5", (-1.0, -1.0, 29.0, 13.0), 300, 140),
    ("px5_paths_half", "px5", (-0.95, -0.95, 29.05, 13.05),
     300, 140),
]


def bake(work, golden_dir):
    import klayout.db as db
    import klayout.lay as klay
    os.makedirs(golden_dir, exist_ok=True)
    srcs = {}
    for case, builder in _fixtures(db):
        ly = db.Layout()
        ly.dbu = 0.001
        top = ly.create_cell("TOP")
        l1 = ly.layer(1, 0)
        builder(ly, top, l1)
        p = os.path.join(work, case + ".oas")
        ly.write(p)
        srcs[case] = p
    for name, case, view, w, h in RENDERS:
        lv = klay.LayoutView()
        for k, v in (("background-color", "#000000"),
                     ("grid-visible", "false"),
                     ("grid-show-ruler", "false"),
                     ("text-visible", "false"),
                     ("cell-box-visible", "false")):
            try:
                lv.set_config(k, v)
            except Exception:
                pass
        lv.load_layout(srcs[case], 0)
        lv.max_hier()
        lv.add_missing_layers()
        it = lv.begin_layers()
        while not it.at_end():
            n = it.current()
            n.fill_color = 0xFFFFFF
            n.frame_color = 0xFFFFFF
            n.dither_pattern = 0    # solid
            n.width = 1
            n.visible = True
            it.next()
        box = db.DBox(view[0], view[1], view[2], view[3])
        out = os.path.join(golden_dir, name + ".png")
        # target-box form pins the world<->pixel mapping exactly
        # (zoom_box alone may letterbox); oversampling 1 = binary
        lv.save_image_with_options(out, w, h, 1, 1, 0, box, False)
        lv.destroy()
    import klayout
    json.dump(
        {"klayout": klayout.__version__,
         "renders": [[n, c, list(v), w, h]
                     for (n, c, v, w, h) in RENDERS]},
        open(os.path.join(golden_dir, "manifest.json"), "w"),
        indent=1)


# --------------------------------------------------------- policy

def load_bin(path):
    from PIL import Image
    return np.asarray(Image.open(path).convert("L")) > 96


def _edge_band(gold):
    """pixels whose 3x3 window mixes on/off = Chebyshev <= 1 from
    a golden edge; and the inner-perimeter pixel count"""
    on = gold
    mn = on.copy()
    mx = on.copy()
    for dy in (-1, 0, 1):
        for dx in (-1, 0, 1):
            sh = np.zeros_like(on)
            ys = slice(max(dy, 0), on.shape[0] + min(dy, 0))
            yd = slice(max(-dy, 0), on.shape[0] + min(-dy, 0))
            xs = slice(max(dx, 0), on.shape[1] + min(dx, 0))
            xd = slice(max(-dx, 0), on.shape[1] + min(-dx, 0))
            sh[yd, xd] = on[ys, xs]
            mn &= sh
            mx |= sh
    band = mx & ~mn                      # mixed neighborhood
    edge_on = on & ~mn                   # on with an off neighbor
    return band, int(edge_on.sum())


def _components(mask):
    """4-connected components as index-array list (fixtures are
    tiny; plain BFS is fine)"""
    seen = np.zeros_like(mask, dtype=bool)
    comps = []
    ys, xs = np.nonzero(mask)
    for y0, x0 in zip(ys.tolist(), xs.tolist()):
        if seen[y0, x0]:
            continue
        stack = [(y0, x0)]
        seen[y0, x0] = True
        comp = []
        while stack:
            y, x = stack.pop()
            comp.append((y, x))
            for dy, dx in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                ny, nx = y + dy, x + dx
                if (0 <= ny < mask.shape[0]
                        and 0 <= nx < mask.shape[1]
                        and mask[ny, nx] and not seen[ny, nx]):
                    seen[ny, nx] = True
                    stack.append((ny, nx))
        comps.append(comp)
    return comps


def compare(gold, cand):
    """policy P-a/b/c; returns (ok, reason)"""
    if gold.shape != cand.shape:
        return False, "size %s vs %s" % (gold.shape, cand.shape)
    band, edge_px = _edge_band(gold)
    diff = gold ^ cand
    out = diff & ~band
    if out.any():
        y, x = np.nonzero(out)
        return False, ("P-a %d diff px outside the 1px band "
                       "(first at y=%d x=%d)"
                       % (int(out.sum()), y[0], x[0]))
    dn = abs(int(cand.sum()) - int(gold.sum()))
    lim = max(16, int(0.75 * edge_px))
    if dn > lim:
        return False, ("P-c area drift %d > %d (edge px %d)"
                       % (dn, lim, edge_px))
    for comp in _components(gold):
        if not any(cand[y, x] for (y, x) in comp):
            y, x = comp[0]
            return False, ("P-b golden component vanished "
                           "(%d px at y=%d x=%d)"
                           % (len(comp), y, x))
    return True, "ok (diff %d, band-only)" % int(diff.sum())


def _shift(mask, dy, dx):
    out = np.zeros_like(mask)
    ys = slice(max(dy, 0), mask.shape[0] + min(dy, 0))
    yd = slice(max(-dy, 0), mask.shape[0] + min(-dy, 0))
    xs = slice(max(dx, 0), mask.shape[1] + min(dx, 0))
    xd = slice(max(-dx, 0), mask.shape[1] + min(-dx, 0))
    out[yd, xd] = mask[ys, xs]
    return out


def main():
    args = [a for a in sys.argv[1:]]
    cand_dir = None
    if "--candidate" in args:
        i = args.index("--candidate")
        cand_dir = args[i + 1]
        del args[i:i + 2]
    work = args[0] if args else os.path.join(
        tempfile.gettempdir(), "floe-goldens")
    os.makedirs(work, exist_ok=True)
    golden = os.path.join(work, "golden")
    bad = []

    def fail(msg):
        bad.append(msg)
        print("FAIL " + msg)

    import klayout
    man_path = os.path.join(golden, "manifest.json")
    stale = True
    if os.path.exists(man_path):
        man = json.load(open(man_path))
        stale = (man.get("klayout") != klayout.__version__
                 or [tuple(r[:1]) for r in man.get("renders", [])]
                 != [(n,) for (n, *_rest) in RENDERS])
    if stale:
        print("baking goldens (klayout %s)..." % klayout.__version__)
        bake(work, golden)

    # self-check 1: determinism - a fresh render must equal the
    # baked golden EXACTLY on the same host/version
    redo = os.path.join(work, "recheck")
    bake(work, redo)
    for name, *_ in RENDERS:
        g = load_bin(os.path.join(golden, name + ".png"))
        r = load_bin(os.path.join(redo, name + ".png"))
        if g.shape != r.shape or (g ^ r).any():
            fail("PX self-render differs from golden: %s" % name)

    # self-check 2: the policy must discriminate (synthetic
    # candidates over a concave golden)
    g = load_bin(os.path.join(golden, "px4_concave.png"))
    checks = [
        ("identity", g, True),
        ("shift1", _shift(g, 0, 1), True),        # subpixel phase
        ("shift2", _shift(g, 0, 2), False),       # P-a
        ("dilate1", _edge_band(g)[0] | g, False),  # P-c growth
    ]
    comps = _components(g)
    dropped = g.copy()
    for (y, x) in comps[0]:
        dropped[y, x] = False
    checks.append(("vanish", dropped, False))     # P-b
    for name, cand, want in checks:
        ok, why = compare(g, cand)
        if ok != want:
            fail("policy self-test %s: got %s (%s)"
                 % (name, ok, why))

    # candidate mode: the external renderer's output
    if cand_dir:
        for name, *_ in RENDERS:
            cp = os.path.join(cand_dir, name + ".png")
            if not os.path.exists(cp):
                fail("candidate missing %s" % name)
                continue
            ok, why = compare(
                load_bin(os.path.join(golden, name + ".png")),
                load_bin(cp))
            print("%-18s %s  %s" % (name, "PASS" if ok else "FAIL",
                                    why))
            if not ok:
                bad.append(name)

    print("render-goldens PX1-PX5 (%d views), failures: %d"
          % (len(RENDERS), len(bad)))
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
