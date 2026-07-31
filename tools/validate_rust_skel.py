"""M3 validation: the Rust skeleton + full-text sidecar vs the
klayout-built cache.

  - meta skeleton dict (file/shapes/texts) and texts_thinned EXACT vs
    the Python meta (valmini stays under every budget, so the
    label set is the python-exact full-collection path; thinned
    layers would use each side's own sampler and are exempt).
  - skeleton.oas geometry: coverage XOR per (layer, datatype) - the
    design layers, the level-k twin layers (dt+30000k) and the
    outline layer 255/0 all included.
  - skeleton.oas texts: the (layer, dt, string, x, y) multiset must
    match exactly (outline cell names at bbox centers + budget
    labels on the level-1 twins).
  - texts.tsv sidecar: entry/member totals vs meta, per-layer
    expanded member totals vs a klayout recursive full count, and
    every python skeleton label must exist among the sidecar's
    expanded members (the sidecar is the superset that search uses).

usage: python tools/validate_rust_skel.py <src.oas> <rust_outdir>
"""
import functools
import json
import os
import sys
import time
from collections import Counter

import klayout.db as db

print = functools.partial(print, flush=True)


def fail(msgs, text):
    msgs.append(text)
    print(text)


def skel_content(path):
    """({(l,d): Region}, Counter{(l,d,string,x,y)}) of a skeleton."""
    ly = db.Layout(False)
    ly.read(path)
    tops = [c for c in ly.each_cell() if not c.parent_cells()]
    regs = {}
    texts = Counter()
    for li in ly.layer_indexes():
        info = ly.get_info(li)
        key = (info.layer, info.datatype)
        reg = db.Region()
        for top in tops:
            reg += db.Region(ly.begin_shapes(top, li))
            for cell in [top]:
                for sh in cell.shapes(li).each():
                    if sh.is_text():
                        t = sh.text
                        p = t.trans.disp
                        texts[key + (t.string, p.x, p.y)] += 1
        if not reg.is_empty():
            regs[key] = reg
    return regs, texts, ly


def parse_sidecar(path):
    """[(l, d, x, y, factors, s)] with factors as offset lists."""
    out = []
    for line in open(path, encoding="utf-8"):
        line = line.rstrip("\n")
        if not line:
            continue
        ld, x, y, reps, s = line.split("\t")
        l, d = (int(v) for v in ld.split("/"))
        s = (s.replace("\\t", "\t").replace("\\n", "\n")
              .replace("\\r", "\r").replace("\\\\", "\\"))
        factors = []
        if reps:
            for f in reps.split(";"):
                if f.startswith("g:"):
                    na, nb, vax, vay, vbx, vby = (
                        int(v) for v in f[2:].split(","))
                    factors.append([
                        (i * vax + j * vbx, i * vay + j * vby)
                        for j in range(nb) for i in range(na)])
                elif f.startswith("p:"):
                    vals = [int(v) for v in f[2:].split(" ")]
                    factors.append(list(zip(vals[::2], vals[1::2])))
                elif f == "1":
                    factors.append([(0, 0)])
                else:
                    raise ValueError("bad factor %r" % f)
        out.append((l, d, int(x), int(y), factors, s))
    return out


def expand_entry(x, y, factors):
    pts = [(x, y)]
    for offs in factors:
        pts = [(px + ox, py + oy) for px, py in pts for ox, oy in offs]
    return pts


def main():
    src = sys.argv[1]
    outdir = sys.argv[2]
    t0 = time.perf_counter()
    py = json.load(open(src + ".ice/meta.json"))
    rs = json.load(open(os.path.join(outdir, "meta.json")))
    bad = []

    if py["skeleton"] != rs["skeleton"]:
        fail(bad, "SKEL meta: py=%r rust=%r"
             % (py["skeleton"], rs["skeleton"]))
    if py.get("texts_thinned") != rs.get("texts_thinned"):
        fail(bad, "SKEL texts_thinned: py=%r rust=%r"
             % (py.get("texts_thinned"), rs.get("texts_thinned")))

    kregs, ktexts, kly = skel_content(
        os.path.join(src + ".ice", "skeleton.oas"))
    rregs, rtexts, rly = skel_content(
        os.path.join(outdir, "skeleton.oas"))
    for key in sorted(set(kregs) | set(rregs)):
        a, b = kregs.get(key), rregs.get(key)
        if a is None or b is None:
            fail(bad, "SKEL L%s: klayout=%s rust=%s"
                 % (key, a is not None, b is not None))
            continue
        x = a ^ b
        if not x.is_empty():
            fail(bad, "SKEL XOR L%s: %d polys, area %d, e.g. %s"
                 % (key, x.count(), x.area(), next(x.each()).bbox()))
    if ktexts != rtexts:
        d1 = ktexts - rtexts
        d2 = rtexts - ktexts
        fail(bad, "SKEL texts: %d only-klayout (e.g. %s), %d "
             "only-rust (e.g. %s)"
             % (sum(d1.values()), next(iter(d1), None),
                sum(d2.values()), next(iter(d2), None)))
    kly._destroy()
    rly._destroy()

    side = parse_sidecar(os.path.join(outdir, "texts.tsv"))
    sm = rs["texts_sidecar"]
    members = sum(
        len(expand_entry(0, 0, f)) if f else 1
        for _l, _d, _x, _y, f, _s in side)
    if sm["entries"] != len(side) or sm["members"] != members:
        fail(bad, "SIDECAR meta: entries %d/%d members %d/%d"
             % (sm["entries"], len(side), sm["members"], members))

    # per-layer expanded totals vs a klayout full recursive count
    per_layer = Counter()
    expanded = set()
    for l, d, x, y, factors, s in side:
        pts = expand_entry(x, y, factors)
        per_layer[(l, d)] += len(pts)
        for p in pts:
            expanded.add((l, d, s, p[0], p[1]))
    ly = db.Layout(False)
    ly.read(src)
    top = ly.top_cell()
    for li in ly.layer_indexes():
        info = ly.get_info(li)
        key = (info.layer, info.datatype)
        it = db.RecursiveShapeIterator(ly, top, [li])
        it.shape_flags = db.Shapes.STexts
        cnt = 0
        while not it.at_end():
            cnt += 1
            it.next()
        if cnt != per_layer.get(key, 0):
            fail(bad, "SIDECAR L%s: klayout %d texts vs sidecar %d"
                 % (key, cnt, per_layer.get(key, 0)))
    ly._destroy()

    # the python skeleton's labels must be a subset of the sidecar
    # (twin layer dt-30000 maps back to the design layer)
    missing = 0
    for (l, d, s, x, y), n in ktexts.items():
        if l == 255 and d == 0:
            continue  # outline cell names, not source texts
        key = (l, d - 30000, s, x, y)
        if key not in expanded:
            missing += n
            if missing <= 3:
                fail(bad, "SIDECAR missing python label %s" % (key,))
    if missing > 3:
        fail(bad, "SIDECAR missing %d python labels total" % missing)

    print("skel-checked %d geometry layers, %d label tuples, "
          "%d sidecar entries (%d members), failures: %d, %.0fs"
          % (len(set(kregs) | set(rregs)),
             sum(ktexts.values()), len(side), members, len(bad),
             time.perf_counter() - t0))
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
