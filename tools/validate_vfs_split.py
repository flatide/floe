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

Usage: validate_vfs_split.py [workdir]  (default $TMPDIR/floe-valsplit)
"""
import json
import os
import random
import shutil
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FI = os.path.join(ROOT, "rust", "target", "release", "floe-index")
TOTAL_MEMBERS = 1_150_050  # 500x2000 pts + 150k grid + 50 ones


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
    gen_marker = src + ".gen3"
    if not os.path.exists(src) or not os.path.exists(gen_marker):
        gen(src)
        open(gen_marker, "w").write("ok")
    bad = []

    def fail(msg):
        bad.append(msg)
        print("FAIL " + msg)

    out = os.path.join(work, "repfloor.oas.floe")
    shutil.rmtree(out, ignore_errors=True)
    subprocess.run([FI, "vfs", src, out, "--jobs", "4"],
                   capture_output=True, check=True)

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

    # S4: thread-count determinism
    out1 = os.path.join(work, "repfloor_j1.floe")
    shutil.rmtree(out1, ignore_errors=True)
    subprocess.run([FI, "vfs", src, out1, "--jobs", "1"],
                   capture_output=True, check=True)
    for f in ("design.ovm", "design.ovp"):
        a = open(os.path.join(out, f), "rb").read()
        b = open(os.path.join(out1, f), "rb").read()
        if a != b:
            fail("S4 %s differs between --jobs 4 and --jobs 1" % f)
    shutil.rmtree(out1, ignore_errors=True)

    print("vfs-split-checked S1-S4, failures: %d" % len(bad))
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
