"""v5 text-index gates (VFS_TEXT_PLAN.md par.8): the cell-local
text index and the label planner vs the klayout source oracle.

  X1  counts: per-layer text member sums decoded from the ovm
      TEXTS/TREPS sections == klayout's expanded text count
  X2  raw candidates: `plan --labels raw` == flat klayout text
      enumeration inside fixed viewports (identity multiset XOR 0),
      full depth AND depth 0, plus a layer-visibility slice
  X3  declutter: `--labels sel` deterministic across runs, row
      count <= budget, txt rows a subset of raw
  X4  corrupt: truncated design.ovt refuses to open
  X5  determinism: --jobs 1 vs 4 builds byte-identical ovm/ovt
  X6  daemon: labels=<gen file> arrives with rows parsing back to
      the selection; nolabels=1, probes and labels=0 suppress

usage: python tools/validate_vfs_text.py [workdir]
"""
import functools
import json
import os
import shutil
import struct
import subprocess
import sys
import tempfile

import klayout.db as db

print = functools.partial(print, flush=True)

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FI = os.path.join(ROOT, "rust", "target", "release", "floe-index")

HEADER = 312
TEXT_LEN = 80
TREP_LEN = 64


def gen(src):
    ly = db.Layout()
    ly.dbu = 0.001
    top = ly.create_cell("TOP")
    mid = ly.create_cell("MID")
    sub = ly.create_cell("SUB")
    l7 = ly.layer(7, 0)
    l9 = ly.layer(9, 1)
    l255 = ly.layer(255, 0)
    # SUB: distinct pin names, one PIN string scattered 200x (the
    # writer folds it into a Pts repetition), a regular 20x10 "G"
    # grid (Grid repetition), and geometry so pages exist
    for i in range(10):
        sub.shapes(l7).insert(db.Text("A%d" % i, i * 40, 25))
    import random
    rnd = random.Random(11)
    for _ in range(200):
        sub.shapes(l7).insert(
            db.Text("PIN", rnd.randint(0, 900), rnd.randint(0, 900)))
    for i in range(20):
        for j in range(10):
            sub.shapes(l9).insert(
                db.Text("G", 10 + i * 45, 12 + j * 90))
    sub.shapes(l7).insert(db.Box(0, 0, 950, 950))
    # a string exercising the TSV escaper
    sub.shapes(l9).insert(db.Text("WE\tIRD\\X", 500, 500))
    # MID: SUB straight + rotated 90 + mirrored
    mid.insert(db.CellInstArray(sub.cell_index(), db.Trans(0, False, 0, 0)))
    mid.insert(db.CellInstArray(sub.cell_index(), db.Trans(1, False, 2400, 0)))
    mid.insert(db.CellInstArray(sub.cell_index(), db.Trans(0, True, 4200, 0)))
    mid.shapes(l255).insert(db.Text("MIDLBL", 2000, 1500))
    # TOP: MID twice, SUB as a 3x2 array, own label
    top.insert(db.CellInstArray(mid.cell_index(), db.Trans(0, False, 0, 0)))
    top.insert(db.CellInstArray(mid.cell_index(), db.Trans(2, False, 9000, 8000)))
    top.insert(db.CellInstArray(
        sub.cell_index(), db.Trans(0, False, 0, 3000),
        db.Vector(1200, 0), db.Vector(0, 1500), 3, 2))
    top.shapes(l255).insert(db.Text("TOPLBL", 4000, 7000))
    top.shapes(l255).insert(db.Box(0, 0, 100, 100))
    opt = db.SaveLayoutOptions()
    opt.format = "OASIS"
    opt.oasis_compression_level = 10
    ly.write(src, opt)
    ly._destroy()


def read_text_sections(ovm_path):
    """per-layer (records, members) decoded from TEXTS + TREPS"""
    d = open(ovm_path, "rb").read()
    assert d[:8] == b"FLOEOVM1" and struct.unpack_from("<I", d, 8)[0] == 5
    n_layers = struct.unpack_from("<I", d, 44)[0]
    secs = [struct.unpack_from("<QQ", d, 88 + 16 * i) for i in range(14)]
    layers = []
    names = d[secs[0][0]:secs[0][0] + secs[0][1]]
    for i in range(n_layers):
        o = secs[1][0] + 32 * i
        l, dt = struct.unpack_from("<II", d, o)
        layers.append((l, dt))
    to, tl = secs[9]
    ro, rl = secs[12]
    out = {}
    for i in range(tl // TEXT_LEN):
        o = to + TEXT_LEN * i
        li = struct.unpack_from("<I", d, o + 4)[0]
        rep = struct.unpack_from("<I", d, o + 36)[0]
        if rep == 0xFFFFFFFF:
            m = 1
        else:
            ro_i = ro + TREP_LEN * rep
            kind = d[ro_i]
            na, nb = struct.unpack_from("<II", d, ro_i + 4)
            m = na * nb if kind == 1 else na
        r, mm = out.get(layers[li], (0, 0))
        out[layers[li]] = (r + 1, mm + m)
    return out


def unesc(s):
    out, i = [], 0
    while i < len(s):
        c = s[i]
        if c == "\\" and i + 1 < len(s):
            out.append({"t": "\t", "n": "\n", "r": "\r",
                        "\\": "\\"}.get(s[i + 1], s[i + 1]))
            i += 2
        else:
            out.append(c)
            i += 1
    return "".join(out)


def plan_labels(outdir, view, mode, depth="full", px=5.0,
                layers=None):
    args = [FI, "plan", outdir, "--view",
            ",".join("%f" % v for v in view), "--px-per-um",
            str(px), "--cut-px", "0", "--depth", str(depth),
            "--labels", mode]
    if layers:
        args += ["--layers", ",".join(layers)]
    r = subprocess.run(args, capture_output=True, check=True,
                       text=True)
    rows = []
    for ln in r.stdout.splitlines():
        p = ln.split("\t")
        if len(p) < 5:
            continue
        if p[0] == "txt":
            l, _, dd = p[1].partition("/")
            rows.append(("txt", int(l), int(dd), int(p[2]),
                         int(p[3]), unesc(p[4])))
        elif p[0] == "blk":
            # v6 rows append the tone column before the text
            text_col = (6 if len(p) >= 7
                        else 5 if len(p) >= 6 else 4)
            rows.append(("blk", -1, -1, int(p[2]), int(p[3]),
                         unesc(p[text_col])))
    return rows


def oracle(ly, top_ci, view_dbu, depth_full=True, only=None):
    """flat text anchors inside the viewport (anchor containment,
    closed box), as (l, d, x, y, s) multiset"""
    box = db.Box(*view_dbu)
    out = []
    for li in ly.layer_indexes():
        info = ly.get_info(li)
        if only is not None and (info.layer, info.datatype) not in only:
            continue
        if depth_full:
            it = ly.begin_shapes_touching(top_ci, li, box)
            while not it.at_end():
                sh = it.shape()
                if sh.is_text():
                    t = sh.text.transformed(it.trans())
                    x, y = t.trans.disp.x, t.trans.disp.y
                    if box.contains(db.Point(x, y)):
                        out.append((info.layer, info.datatype,
                                    x, y, sh.text.string))
                it.next()
        else:
            for sh in ly.cell(top_ci).shapes(li).each():
                if sh.is_text():
                    x, y = sh.text.trans.disp.x, sh.text.trans.disp.y
                    if box.contains(db.Point(x, y)):
                        out.append((info.layer, info.datatype,
                                    x, y, sh.text.string))
    out.sort()
    return out


def main():
    work = sys.argv[1] if len(sys.argv) > 1 else os.path.join(
        tempfile.gettempdir(), "floe-valtext")
    os.makedirs(work, exist_ok=True)
    src = os.path.join(work, "textmini.oas")
    marker = src + ".gen1"
    if not os.path.exists(src) or not os.path.exists(marker):
        gen(src)
        open(marker, "w").write("ok")
    bad = []

    def fail(msg):
        bad.append(msg)
        print("FAIL " + msg)

    out = os.path.join(work, "textmini.oas.floe")
    shutil.rmtree(out, ignore_errors=True)
    subprocess.run([FI, "vfs", src, out, "--jobs", "4"],
                   capture_output=True, check=True)

    ly = db.Layout(False)
    ly.read(src)
    top_ci = ly.top_cell().cell_index()

    # X1: per-layer member sums vs klayout expansion
    got = read_text_sections(os.path.join(out, "design.ovm"))
    for li in ly.layer_indexes():
        info = ly.get_info(li)
        key = (info.layer, info.datatype)
        want = 0
        for cell in ly.each_cell():
            for sh in cell.shapes(li).each():
                if sh.is_text():
                    want += 1
        have = got.get(key, (0, 0))[1]
        if have != want:
            fail("X1 layer %s members ovm=%d klayout=%d"
                 % (key, have, want))

    # X2: raw candidates == oracle over fixed viewports
    views = [
        (0.0, 0.0, 20.0, 20.0),        # everything
        (0.0, 0.0, 1.0, 1.0),          # SUB at identity
        (2.3, -0.2, 3.6, 1.2),         # rotated SUB inside MID
        (0.0, 2.8, 4.5, 6.2),          # SUB array band
        (8.0, 6.5, 10.0, 9.0),         # rotated MID at TOP
    ]
    for view in views:
        vd = tuple(int(v * 1000) for v in view)
        want = oracle(ly, top_ci, vd, True)
        rows = plan_labels(out, view, "raw")
        have = sorted((l, d, x, y, s)
                      for (k, l, d, x, y, s) in rows if k == "txt")
        if have != want:
            miss = [t for t in want if t not in have]
            extra = [t for t in have if t not in want]
            fail("X2 view %s: %d vs oracle %d (miss %s / extra %s)"
                 % (view, len(have), len(want), miss[:3], extra[:3]))
    # depth 0: TOP's own texts only
    vd = (0, 0, 20000, 20000)
    want = oracle(ly, top_ci, vd, False)
    rows = plan_labels(out, (0.0, 0.0, 20.0, 20.0), "raw", depth=0)
    have = sorted((l, d, x, y, s)
                  for (k, l, d, x, y, s) in rows if k == "txt")
    if have != want:
        fail("X2 depth-0 %d vs oracle %d" % (len(have), len(want)))
    # visibility slice: only 9/1
    want = oracle(ly, top_ci, (0, 0, 20000, 20000), True,
                  only={(9, 1)})
    rows = plan_labels(out, (0.0, 0.0, 20.0, 20.0), "raw",
                       layers=["9/1"])
    have = sorted((l, d, x, y, s)
                  for (k, l, d, x, y, s) in rows if k == "txt")
    if have != want:
        fail("X2 layers=9/1 %d vs oracle %d"
             % (len(have), len(want)))

    # X3: declutter selection - budget, subset, determinism
    raw = set((l, d, x, y, s) for (k, l, d, x, y, s)
              in plan_labels(out, (0.0, 0.0, 20.0, 20.0), "raw")
              if k == "txt")
    sel1 = plan_labels(out, (0.0, 0.0, 20.0, 20.0), "sel", px=100)
    sel2 = plan_labels(out, (0.0, 0.0, 20.0, 20.0), "sel", px=100)
    if sel1 != sel2:
        fail("X3 selection not deterministic")
    if len(sel1) > 400:
        fail("X3 selection %d rows > budget" % len(sel1))
    if not sel1:
        fail("X3 empty selection")
    for (k, l, d, x, y, s) in sel1:
        if k == "txt" and (l, d, x, y, s) not in raw:
            fail("X3 selected %r not a raw candidate"
                 % ((l, d, x, y, s),))
    # a probe-style px=0 run must select nothing
    r = subprocess.run(
        [FI, "plan", out, "--view", "0,0,20,20", "--px-per-um",
         "5", "--cut-px", "0", "--depth", "full", "--lod", "0",
         "--labels", "sel"],
        capture_output=True, check=True, text=True)
    if any(ln.startswith(("txt", "blk"))
           for ln in r.stdout.splitlines()):
        fail("X3 --lod 0 (px 0) still selected labels")

    # X4: truncated ovt refuses to open
    ovt = os.path.join(out, "design.ovt")
    blob = open(ovt, "rb").read()
    open(ovt, "wb").write(blob[:-1])
    r = subprocess.run([FI, "plan", out, "--view", "0,0,1,1"],
                       capture_output=True, text=True)
    if r.returncode == 0 or "corrupt cache" not in r.stderr:
        fail("X4 truncated ovt opened (rc=%d)" % r.returncode)
    open(ovt, "wb").write(blob)

    # X5: jobs determinism (ovm + ovt bytes)
    out1 = os.path.join(work, "textmini_j1.floe")
    shutil.rmtree(out1, ignore_errors=True)
    subprocess.run([FI, "vfs", src, out1, "--jobs", "1"],
                   capture_output=True, check=True)
    for f in ("design.ovm", "design.ovt", "design.ovp",
              "meta.json"):
        if open(os.path.join(out, f), "rb").read() != \
           open(os.path.join(out1, f), "rb").read():
            fail("X5 %s differs between --jobs 4 and 1" % f)
    shutil.rmtree(out1, ignore_errors=True)

    # X6: daemon labels lifecycle
    def start():
        return subprocess.Popen(
            [FI, "vfsd", out], stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            text=True, bufsize=1)

    tmp = tempfile.mkdtemp(prefix="floe_valtext_")

    def ask(p, line):
        p.stdin.write(line + "\n")
        p.stdin.flush()
        return dict(t.partition("=")[::2]
                    for t in p.stdout.readline().split())

    p = start()
    r1 = ask(p, "gen=1 view=0,0,20,20 px=100 cut=0 depth=full "
                "layers=all out=%s ack=0" % tmp)
    metrics = ("labels_truncated", "text_bvh_nodes",
               "text_place_bvh_nodes", "text_place_records",
               "text_members_tested", "text_members_visible",
               "text_plan_ms")
    missing = [k for k in metrics if k not in r1]
    if missing:
        fail("X6 live response missing metrics: %s" % ",".join(missing))
    if int(r1.get("labels_truncated", 1)) != 0:
        fail("X6 ordinary fixture unexpectedly truncated labels")
    if r1.get("labels", "-") == "-" or int(r1.get("nlabels", 0)) < 1:
        fail("X6 no labels in live response: %r" % r1)
    else:
        n = 0
        with open(r1["labels"]) as f:
            for ln in f:
                if ln.startswith(("txt\t", "blk\t")):
                    n += 1
        if n != int(r1["nlabels"]):
            fail("X6 nlabels=%s but %d rows" % (r1["nlabels"], n))
    r2 = ask(p, "gen=2 view=0,0,20,20 px=100 cut=0 depth=full "
                "layers=all out=%s ack=1 nolabels=1" % tmp)
    if r2.get("labels", "-") != "-":
        fail("X6 nolabels round still shipped labels")
    r3 = ask(p, "gen=3 view=0,0,20,20 px=100 cut=0 depth=full "
                "layers=all out=%s mode=probe" % tmp)
    if r3.get("labels", "-") != "-":
        fail("X6 probe shipped labels")
    p.stdin.write("quit\n")
    p.stdin.flush()
    p.wait(timeout=5)
    p2 = start()
    r4 = ask(p2, "gen=1 view=0,0,20,20 px=100 cut=0 depth=full "
                 "layers=all labels=0 out=%s ack=0" % tmp)
    if r4.get("labels", "-") != "-":
        fail("X6 labels=0 shipped labels")
    p2.stdin.write("quit\n")
    p2.stdin.flush()
    p2.wait(timeout=5)
    shutil.rmtree(tmp, ignore_errors=True)

    ly._destroy()
    print("vfs-text-checked X1-X6, failures: %d" % len(bad))
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
