"""Rep-split build gates (ovm v3): a synthetic rep-flood asset -
the miniature of the 9.8G field pathology - must produce spatially
honest pages.

  S1  floor collapse: an interior POINT view at depth 0 (cut off)
      selects a small fraction of the layer's pages/bytes (the v2
      build selected 100% of them for ANY view)
  S2  conservation: full-view plan members == source member count
  S3  G5 gates on the fragmented asset: page-by-page klayout member
      recount + layer sums (validate_vfs.py reused - this is the
      end-to-end proof that fragment REBASE re-encodes correctly)
  S4  determinism: --jobs 1 and --jobs 4 builds are byte-identical
      (with TWO Pts-flood layers this also proves the per-layer
      arena slots of the #60 P1 split fanout never cross)
  S5  LOD variants (M7): dense layers grow lod pages; the planner
      never selects them (pranges cover exact pages only); their
      payloads pass the same klayout recount via S3
  S6  split fanout (#60 P1): the --jobs 4 build's slow-cell log
      (FLOE_SLOW_CELL_S=0) shows >1 split thread and the per-layer
      top list on the multi-layer cell, and the parallel build's
      peak child RSS stays within 1.5x + 512MB of the serial one

Usage: validate_vfs_split.py [workdir]  (default $TMPDIR/floe-valsplit)
"""
import json
import os
import random
import re
import resource
import shutil
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FI = os.path.join(ROOT, "rust", "target", "release", "floe-index")
# 500x2000 + 200x1500 pts + 150k grid + 50 ones
TOTAL_MEMBERS = 1_450_050


def gen(src):
    import klayout.db as db
    ly = db.Layout()
    ly.dbu = 0.001
    top = ly.create_cell("TOP")
    l1 = ly.layer(1, 0)
    l2 = ly.layer(2, 0)
    rnd = random.Random(7)
    die = 1_000_000
    # 500 distinct fill boxes x 1000 scattered positions: klayout's
    # OASIS writer folds them into whole-die Pts repetitions
    for _ in range(500):
        w = rnd.randint(80, 200)
        h = rnd.randint(80, 200)
        for _ in range(2000):
            x = rnd.randint(0, die - w)
            y = rnd.randint(0, die - h)
            top.shapes(l1).insert(db.Box(x, y, x + w, y + h))
    # die-tall grid strips (writer folds into vertical Grid reps)
    for k in range(150):
        x = rnd.randint(0, die - 200)
        for j in range(1000):
            top.shapes(l2).insert(
                db.Box(x, j * 1000, x + 150, j * 1000 + 400))
    # unique wide spines (Rep::One quarantine path)
    for k in range(50):
        y = rnd.randint(0, die - 60)
        top.shapes(l2).insert(db.Box(k * 17, y, die - k * 13, y + 50))
    # SECOND Pts-flood layer (#60 P1): both layer arenas start at
    # local index 0, so a slot mixup between them corrupts pages
    # that S3's recount and S4's byte compare then catch
    l3 = ly.layer(3, 0)
    for _ in range(200):
        w = rnd.randint(80, 200)
        h = rnd.randint(80, 200)
        for _ in range(1500):
            x = rnd.randint(0, die - w)
            y = rnd.randint(0, die - h)
            top.shapes(l3).insert(db.Box(x, y, x + w, y + h))
    opt = db.SaveLayoutOptions()
    opt.format = "OASIS"
    opt.oasis_compression_level = 10
    ly.write(src, opt)


def plan(outdir, view, ppu, depth=0, cut=0.0):
    r = subprocess.run(
        [FI, "plan", outdir, "--mode", "hier",
         "--view", ",".join("%f" % v for v in view),
         "--px-per-um", str(ppu), "--cut-px", str(cut),
         "--depth", str(depth)],
        capture_output=True, check=True)
    return json.loads(r.stdout)


def main():
    work = sys.argv[1] if len(sys.argv) > 1 else os.path.join(
        tempfile.gettempdir(), "floe-valsplit")
    os.makedirs(work, exist_ok=True)
    src = os.path.join(work, "repfloor.oas")
    gen_marker = src + ".gen4"
    if not os.path.exists(src) or not os.path.exists(gen_marker):
        gen(src)
        open(gen_marker, "w").write("ok")
    bad = []

    def fail(msg):
        bad.append(msg)
        print("FAIL " + msg)

    def child_maxrss():
        # macOS reports bytes, Linux kilobytes
        mult = 1 if sys.platform == "darwin" else 1024
        return resource.getrusage(
            resource.RUSAGE_CHILDREN).ru_maxrss * mult

    # serial build first: S4's byte reference AND the RSS baseline
    # (RUSAGE_CHILDREN.ru_maxrss is a running max over reaped
    # children, so the serial peak must be sampled first)
    out1 = os.path.join(work, "repfloor_j1.floe")
    shutil.rmtree(out1, ignore_errors=True)
    subprocess.run([FI, "vfs", src, out1, "--jobs", "1"],
                   capture_output=True, check=True)
    rss_j1 = child_maxrss()

    out = os.path.join(work, "repfloor.oas.floe")
    shutil.rmtree(out, ignore_errors=True)
    env = dict(os.environ, FLOE_SLOW_CELL_S="0")
    r = subprocess.run([FI, "vfs", src, out, "--jobs", "4"],
                       capture_output=True, check=True, env=env)
    rss_j4 = child_maxrss()

    # S6: the multi-layer cell fans its split out and says so
    slow = [ln for ln in r.stderr.decode().splitlines()
            if "slow cell TOP " in ln]
    m = re.search(r"split [0-9.]+/(\d+)t", slow[0]) if slow else None
    if not m:
        fail("S6 no slow-cell line for TOP (FLOE_SLOW_CELL_S=0)")
    else:
        if int(m.group(1)) < 2:
            fail("S6 split fanout never engaged: %s" % slow[0])
        if "top L" not in slow[0]:
            fail("S6 per-layer top list missing: %s" % slow[0])
    if rss_j4 > rss_j1 * 1.5 + (512 << 20):
        fail("S6 parallel build peak RSS %.0fMB vs serial %.0fMB"
             % (rss_j4 / 2**20, rss_j1 / 2**20))
    print("split-rss serial %.0fMB parallel-peak %.0fMB"
          % (rss_j1 / 2**20, rss_j4 / 2**20))

    # S2 first (full view = whole-asset reference)
    full = plan(out, (0, 0, 1000, 1000), 1.047, cut=0.0)
    if full["members"] != TOTAL_MEMBERS:
        fail("S2 full members=%d want %d"
             % (full["members"], TOTAL_MEMBERS))

    # S1: interior points far from the die center planes
    worst = 0.0
    for (cx, cy) in ((222.3, 333.7), (777.1, 111.9), (60.2, 940.4)):
        pt = plan(out, (cx - 0.0005, cy - 0.0005,
                        cx + 0.0005, cy + 0.0005), 1e6, cut=0.0)
        frac = pt["encoded_bytes"] / max(1, full["encoded_bytes"])
        worst = max(worst, frac)
        if pt["pages"] >= full["pages"]:
            fail("S1 point (%s,%s) selects ALL %d pages"
                 % (cx, cy, full["pages"]))
    # v2 selected ~100% of bytes at any point; honest pages must
    # stay well under half even with oversize spines resident
    if worst > 0.35:
        fail("S1 point view still pulls %.0f%% of layer bytes"
             % (100 * worst))

    # S3: page-by-page klayout recount on the FRAGMENTED asset
    r = subprocess.run(
        [sys.executable, os.path.join(ROOT, "tools",
                                      "validate_vfs.py"), src, out],
        capture_output=True)
    sys.stdout.write(r.stdout.decode())
    if r.returncode != 0:
        sys.stderr.write(r.stderr.decode())
        fail("S3 validate_vfs.py failed on rep-split asset")

    # S5: LOD variants exist for the dense layer and stay out of
    # every plan (until M7-B the planner is exact-only; after M7-B
    # exact stays the default and LOD needs the density trigger)
    import struct
    d = open(os.path.join(out, "design.ovm"), "rb").read()
    secs = [struct.unpack_from("<QQ", d, 88 + 16 * i)
            for i in range(9)]
    po, pl = secs[6]
    n_pages = pl // 104  # v6 page stride
    n_lod = sum(1 for i in range(n_pages)
                if d[po + 104 * i + 12] != 0)
    if n_lod < 1:
        fail("S5 no lod pages on a rep-flood asset")
    if full["pages"] != n_pages - n_lod:
        fail("S5 plan selects %d pages but exact pages are %d"
             % (full["pages"], n_pages - n_lod))
    # M7-B: the density gate swaps dense pages at wide zoom, and
    # --lod 0 (or a probe) must render exact
    # px 0.1/um puts the whole die on ~100 px: quadrant pages hold
    # ~250k members vs ~2.5k px^2 -> far past the k=4 gate
    wide = plan(out, (0, 0, 1000, 1000), 0.1, depth=0, cut=0.0)
    if wide.get("lod_pages", 0) < 1:
        fail("S5 density gate never fired on a rep-flood asset")
    r = subprocess.run(
        [FI, "plan", out, "--mode", "hier",
         "--view", "0,0,1000,1000", "--px-per-um", "0.1",
         "--cut-px", "0", "--depth", "0", "--lod", "0"],
        capture_output=True, check=True)
    exact = json.loads(r.stdout)
    if exact.get("lod_pages", 0) != 0:
        fail("S5 --lod 0 still swapped %d pages"
             % exact["lod_pages"])

    # S4: thread-count determinism (serial reference built above)
    for f in ("design.ovm", "design.ovp", "design.ovt"):
        a = open(os.path.join(out, f), "rb").read()
        b = open(os.path.join(out1, f), "rb").read()
        if a != b:
            fail("S4 %s differs between --jobs 4 and --jobs 1" % f)
    shutil.rmtree(out1, ignore_errors=True)

    print("vfs-split-checked S1-S6, failures: %d" % len(bad))
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
