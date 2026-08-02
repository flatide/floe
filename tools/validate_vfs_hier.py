"""M2 gate: vfsd hier protocol (rust/VFS_HIER.md par.3.2/3.5/3.7).

Drives one daemon run end to end and checks, against a live klayout
layout, the properties the viewer (M3) will rely on:

  H1  gen1 delta applies; top=W1_*; names= table arrives exactly once
  H2  gen2 incremental delta: resident page refs bind BY NAME to the
      cells gen1 defined (no '$' conflict variants), shallow
      delete_cells of the W1_* cells keeps pages alive
  H3  stale drop: a response that is never acked is rolled back and
      its new pages are re-sent on the next request
  H4  duplicate gen -> protocol error; reset=1 recovers with a full
      resend (new == pages)
  H5  hier_probe delta at cut=0 renders IDENTICAL geometry to the
      source inside the window (region XOR per layer; frames layer
      excluded - none at cut 0)

usage: python tools/validate_vfs_hier.py <src.oas> <outdir.floe> [bin]
"""
import functools
import os
import shutil
import subprocess
import sys
import tempfile

import klayout.db as db

print = functools.partial(print, flush=True)


def rpc(p, line):
    p.stdin.write(line + "\n")
    p.stdin.flush()
    r = p.stdout.readline().strip()
    assert r, "daemon died on: " + line
    if r.startswith("error="):
        return {"error": r[len("error="):]}
    return dict(t.split("=", 1) for t in r.split())


def region_under(ly, cell, l, dt, box):
    for li in ly.layer_indexes():
        info = ly.get_info(li)
        if (info.layer, info.datatype) == (l, dt):
            r = db.Region(
                db.RecursiveShapeIterator(ly, cell, li, box))
            return r & db.Region(box)
    return db.Region()


def main():
    src, out = sys.argv[1], sys.argv[2]
    fi = sys.argv[3] if len(sys.argv) > 3 else os.path.join(
        os.path.dirname(__file__), "..",
        "rust/target/release/floe-index")
    bad = []

    def chk(cond, msg):
        if not cond:
            bad.append(msg)
            print("FAIL", msg)

    tmp = tempfile.mkdtemp(prefix="floe_hier_")
    p = subprocess.Popen(
        [fi, "vfsd", out, "--budget-mb", "1024"],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL, text=True)
    try:
        base = ("px=5 cut=2 depth=full layers=all out=%s mode=hier"
                % tmp)
        # H1: first gen defines pages + WCs, names once
        r1 = rpc(p, "gen=1 view=0,0,200,200 ack=0 " + base)
        chk("error" not in r1, "H1 %s" % r1)
        chk(r1["top"].startswith("W1_"), "H1 top %s" % r1.get("top"))
        chk(r1["names"] != "-" and os.path.exists(r1["names"]),
            "H1 names table")
        chk(int(r1["new"]) > 0 and os.path.exists(r1["delta"]),
            "H1 delta")
        names = {}
        with open(r1["names"]) as f:
            for line in f:
                ci, nm = line.rstrip("\n").split("\t")
                names[int(ci)] = nm
        chk(len(names) > 0, "H1 names rows")
        os.unlink(r1["names"])  # client contract: load once, delete

        ly = db.Layout(False)
        ws = ly.create_cell("FLOE_WS")
        ly.read(r1["delta"])
        t1 = ly.cell(r1["top"])
        chk(t1 is not None, "H1 top cell in layout")
        ws.insert(db.CellInstArray(t1.cell_index(), db.Trans(0, 0)))

        # H2: incremental delta binds resident refs, shallow delete
        r2 = rpc(p, "gen=2 view=100,100,380,380 ack=1 " + base)
        chk("error" not in r2, "H2 %s" % r2)
        chk(r2["names"] == "-", "H2 names resent")
        pages_before = {c.name for c in ly.each_cell()
                        if c.name.startswith("P")}
        if r2["delta"] != "-":
            ly.read(r2["delta"])
        chk(not any("$" in c.name for c in ly.each_cell()),
            "H2 conflict variants")
        t2 = ly.cell(r2["top"])
        chk(t2 is not None, "H2 top")
        ws.clear_insts()
        ws.insert(db.CellInstArray(t2.cell_index(), db.Trans(0, 0)))
        w1 = [c.cell_index() for c in ly.each_cell()
              if c.name.startswith("W1_")]
        ly.delete_cells(w1)
        chk(not any(c.name.startswith("W1_")
                    for c in ly.each_cell()), "H2 W1 gone")
        pages_after = {c.name for c in ly.each_cell()
                       if c.name.startswith("P")}
        chk(pages_before <= pages_after,
            "H2 shallow delete lost pages")

        # H3: stale drop -> rollback -> resend
        r3 = rpc(p, "gen=3 view=0,0,60,60 ack=2 " + base)
        chk("error" not in r3, "H3 %s" % r3)
        n3 = int(r3["new"])
        r4 = rpc(p, "gen=4 view=0,0,60,60 ack=2 " + base)
        chk("error" not in r4, "H3b %s" % r4)
        chk(int(r4["new"]) == n3,
            "H3 resend new=%s want %d" % (r4.get("new"), n3))

        # H4: duplicate gen -> error; reset recovers fully
        r5 = rpc(p, "gen=4 view=0,0,60,60 ack=4 " + base)
        chk("error" in r5, "H4 dup gen accepted %s" % r5)
        r6 = rpc(p, "gen=6 view=0,0,200,200 ack=0 reset=1 " + base)
        chk("error" not in r6, "H4 reset %s" % r6)
        chk(r6["new"] == r6["pages"],
            "H4 reset resend %s/%s" % (r6.get("new"),
                                       r6.get("pages")))

        # H5: probe at cut=0 == source geometry in the window
        srcly = db.Layout(False)
        srcly.read(src)
        stop = srcly.top_cell()
        dbu = srcly.dbu
        for (x0, y0, x1, y1) in [(20, 20, 120, 120),
                                 (150, 150, 260, 240)]:
            r7 = rpc(p, "gen=9 view=%g,%g,%g,%g ack=0 px=5 cut=0 "
                     "depth=full layers=all out=%s mode=hier_probe"
                     % (x0, y0, x1, y1, tmp))
            chk("error" not in r7 and r7["delta"] != "-",
                "H5 probe %s" % r7)
            ply = db.Layout(False)
            ply.read(r7["delta"])
            ptop = ply.cell(r7["top"])
            chk(ptop is not None, "H5 top")
            box = db.Box(int(x0 / dbu), int(y0 / dbu),
                         int(x1 / dbu), int(y1 / dbu))
            for li in srcly.layer_indexes():
                info = srcly.get_info(li)
                if info.layer >= 255:
                    continue
                rs = region_under(srcly, stop, info.layer,
                                  info.datatype, box)
                rp = region_under(ply, ptop, info.layer,
                                  info.datatype, box)
                if not (rs ^ rp).is_empty():
                    chk(False, "H5 xor L%d/%d view %g,%g"
                        % (info.layer, info.datatype, x0, y0))
            ply._destroy()
        srcly._destroy()

        p.stdin.write("quit\n")
        p.stdin.flush()
    finally:
        try:
            p.kill()
        except Exception:
            pass
        shutil.rmtree(tmp, ignore_errors=True)

    print("vfs-hier-checked H1-H5, failures: %d" % len(bad))
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
