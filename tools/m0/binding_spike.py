#!/usr/bin/env python
"""M0 binding spike (VFS_HIER.md par.5 M0 / par.3.3).

Feeds gen-delta OASIS files (m0_gen gens output) into ONE viewer-mode
Layout the way VfsMosaic will in V4:

  gen 1:    defines page cells + its WC cells
  gen g>=2: defines only its WC cells (+1 new page); PLACEMENTs
            reference resident page cells with NO definition in file

Per gen it verifies:
  A. name binding - reading the delta adds exactly the cells the file
     defines (no '$' conflict variants, no extra ghost cells), and the
     geometry under FLOE_WS equals a manifest-built expectation
     (region XOR per layer) - i.e. placements bound to the resident
     concrete cells, not to fresh empty ones.
  B. shallow delete_cells(prev gen WCs) - pages survive (including
     the page only that gen referenced), geometry unchanged.
  C. book-keeping - cell count = FLOE_WS + base pages + g new pages
     + 2 live WCs; RSS logged for leak trend.

Usage: binding_spike.py <dir> [--mode plain|addtocell] [--gens N]
Exit 0 = all green.
"""

import json
import os
import subprocess
import sys
import time

import klayout.db as db


def rss_mb():
    out = subprocess.run(["ps", "-o", "rss=", "-p", str(os.getpid())],
                         capture_output=True, text=True).stdout.strip()
    return int(out) / 1024.0 if out else -1.0


def layer_index(ly, l, dt):
    for li in ly.layer_indexes():
        inf = ly.get_info(li)
        if inf.layer == l and inf.datatype == dt:
            return li
    return None


def load_manifests(d):
    pages = {}
    with open(os.path.join(d, "gens_pages.tsv")) as f:
        for line in f:
            name, l, dt, x, y, w, h = line.split("\t")
            pages[name] = (int(l), int(dt), int(x), int(y), int(w), int(h))
    gens = {}
    with open(os.path.join(d, "gens_index.tsv")) as f:
        for line in f:
            g, fname, top, wcs, newp = line.rstrip("\n").split("\t")
            gens[int(g)] = dict(file=fname, top=top,
                                wcs=wcs.split(","), new_pages=newp.split(","))
    hier = {}
    with open(os.path.join(d, "gens_hier.tsv")) as f:
        for line in f:
            g, parent, kind, target, x, y, rot, flip, rep = \
                line.rstrip("\n").split("\t")
            hier.setdefault((int(g), parent), []).append(
                (kind, target, int(x), int(y), int(rot), int(flip), rep))
    return pages, gens, hier


def rep_offsets(rep):
    if rep == "-":
        return [(0, 0)]
    p = rep.split(":")
    if p[0] == "g":
        na, nb, vax, vay, vbx, vby = map(int, p[1:])
        return [(i * vax + j * vbx, i * vay + j * vby)
                for i in range(na) for j in range(nb)]
    if p[0] == "p":
        v = list(map(int, p[1:]))
        return list(zip(v[0::2], v[1::2]))
    raise ValueError(rep)


def expected_regions(pages, hier, g, cell, memo):
    """(layer,dt) -> Region, in `cell` local frame."""
    key = (g, cell)
    if key in memo:
        return memo[key]
    acc = {}

    def add(ld, region):
        if ld in acc:
            acc[ld] += region
        else:
            acc[ld] = region

    if cell in pages:
        l, dt, x, y, w, h = pages[cell]
        add((l, dt), db.Region(db.Box(x, y, x + w, y + h)))
    else:
        for kind, target, x, y, rot, flip, rep in hier.get(key, []):
            if kind == "frame":
                l, dt = map(int, target.split(":"))
                _, w, h = rep.split(":")
                add((l, dt),
                    db.Region(db.Box(x, y, x + int(w), y + int(h))))
                continue
            sub = expected_regions(pages, hier, g, target, memo)
            for ox, oy in rep_offsets(rep):
                t = db.Trans(rot, bool(flip), db.Vector(x + ox, y + oy))
                for ld, r in sub.items():
                    add(ld, r.transformed(t))
    memo[key] = acc
    return acc


def region_under(ly, cell, li):
    return db.Region(db.RecursiveShapeIterator(ly, cell, li))


def main():
    d = sys.argv[1]
    mode = "plain"
    max_gens = None
    if "--mode" in sys.argv:
        mode = sys.argv[sys.argv.index("--mode") + 1]
    if "--gens" in sys.argv:
        max_gens = int(sys.argv[sys.argv.index("--gens") + 1])

    pages, gens, hier = load_manifests(d)
    order = sorted(gens)
    if max_gens:
        order = order[:max_gens]

    opts = None
    if mode == "addtocell":
        opts = db.LoadLayoutOptions()
        try:
            opts.cell_conflict_resolution = \
                db.LoadLayoutOptions.CellConflictResolution.AddToCell
        except Exception as e:
            print(json.dumps({"fatal": "no cell_conflict_resolution API",
                              "err": str(e)}))
            sys.exit(3)

    ly = db.Layout(False)  # viewer mode, same as viewport.py:43
    ws = ly.create_cell("FLOE_WS")
    known = {"FLOE_WS"}
    fails = []
    rows = []
    prev_wcs = []
    prev_new_page = None

    for g in order:
        info = gens[g]
        path = os.path.join(d, info["file"])
        t0 = time.perf_counter()
        if opts is not None:
            ly.read(path, opts)
        else:
            ly.read(path)
        t_read = (time.perf_counter() - t0) * 1000

        now = {c.name for c in ly.each_cell()}
        expected_defined = set(info["wcs"]) | set(info["new_pages"])
        if g == 1:
            expected_defined |= {p for p in pages if not p.startswith("P9")}
        extra = now - known - expected_defined
        dollar = {n for n in now if "$" in n}
        if dollar:
            fails.append(f"gen{g}: conflict variants {sorted(dollar)}")
        if extra:
            fails.append(f"gen{g}: unexpected new cells {sorted(extra)}")
        known = now

        ghosts = []
        for pname in pages:
            c = ly.cell(pname)
            if c is None:
                continue
            try:
                if c.is_ghost_cell():
                    ghosts.append(pname)
            except AttributeError:
                pass
        if ghosts:
            fails.append(f"gen{g}: resident pages became ghosts {ghosts}")

        # link current top under FLOE_WS (viewport pattern)
        top = ly.cell(info["top"])
        if top is None:
            fails.append(f"gen{g}: top {info['top']} missing")
            break
        ws.clear_insts()
        ws.insert(db.CellInstArray(top.cell_index(), db.Trans(0, 0)))

        # geometry: expectation from manifest vs layout reality
        memo = {}
        exp = expected_regions(pages, hier, g, info["top"], memo)
        geo_bad = []
        for (l, dt), er in sorted(exp.items()):
            li = layer_index(ly, l, dt)
            if li is None:
                geo_bad.append(f"L{l}/{dt}:missing")
                continue
            ar = region_under(ly, ws, li)
            if not (ar ^ er).is_empty():
                geo_bad.append(
                    f"L{l}/{dt}:xor a={ar.area()} e={er.area()}")
        if geo_bad:
            fails.append(f"gen{g}: geometry mismatch {geo_bad}")

        # shallow delete of previous gen's WCs
        t_del = 0.0
        if prev_wcs:
            idxs = []
            for w in prev_wcs:
                c = ly.cell(w)
                if c is None:
                    fails.append(f"gen{g}: prev WC {w} already gone")
                else:
                    idxs.append(c.cell_index())
            t1 = time.perf_counter()
            try:
                ly.delete_cells(idxs)
                del_api = "delete_cells"
            except AttributeError:
                for i in idxs:
                    ly.delete_cell(i)
                del_api = "delete_cell loop"
            t_del = (time.perf_counter() - t1) * 1000
            for w in prev_wcs:
                if ly.cell(w) is not None:
                    fails.append(f"gen{g}: prev WC {w} survived delete")
            known = {c.name for c in ly.each_cell()}
            # every page must survive, including prev gen's private page
            for pname in list(pages)[:4] + ([prev_new_page] if prev_new_page else []):
                c = ly.cell(pname)
                if c is None:
                    fails.append(f"gen{g}: page {pname} deleted by shallow delete")
                else:
                    nsh = sum(c.shapes(li).size()
                              for li in ly.layer_indexes())
                    if nsh != 1:
                        fails.append(f"gen{g}: page {pname} shapes={nsh}")
            # geometry survives the delete
            for (l, dt), er in sorted(exp.items()):
                li = layer_index(ly, l, dt)
                ar = region_under(ly, ws, li) if li is not None else db.Region()
                if not (ar ^ er).is_empty():
                    fails.append(f"gen{g}: geometry changed after delete L{l}/{dt}")
                    break
        else:
            del_api = "-"

        # ly.cells() is the index-table size (holes from the reader's
        # transient forward-reference cells stay allocated) - live
        # count is what the leak check must use
        live = sum(1 for _ in ly.each_cell())
        n_base = 4
        n_p9 = sum(1 for gg in order if gg <= g)
        exp_live = 1 + n_base + n_p9 + 2
        if live != exp_live:
            fails.append(f"gen{g}: live cells={live} expected={exp_live}")

        rows.append(dict(gen=g, read_ms=round(t_read, 2),
                         del_ms=round(t_del, 2), live_cells=live,
                         index_slots=ly.cells(),
                         rss_mb=round(rss_mb(), 1), del_api=del_api))
        prev_wcs = info["wcs"]
        prev_new_page = info["new_pages"][0]

    # final: drop the last gen's WCs too -> only pages remain
    idxs = [ly.cell(w).cell_index() for w in prev_wcs if ly.cell(w)]
    try:
        ly.delete_cells(idxs)
    except AttributeError:
        for i in idxs:
            ly.delete_cell(i)
    for pname in pages:
        if ly.cell(pname) is None:
            fails.append(f"final: page {pname} gone after last delete")
    final_cells = sum(1 for _ in ly.each_cell())
    exp_final = 1 + 4 + len(order)
    if final_cells != exp_final:
        fails.append(f"final: live cells={final_cells} expected={exp_final}")

    print(json.dumps(dict(
        mode=mode,
        klayout=getattr(__import__("klayout"), "__version__", "?"),
        gens=len(order),
        rows=rows,
        final_cells=final_cells,
        fails=fails,
        verdict="GREEN" if not fails else "RED",
    ), indent=1))
    sys.exit(0 if not fails else 1)


if __name__ == "__main__":
    main()
