#!/usr/bin/env python3
"""G3, G4 and G5 of the streaming index plan (docs/STREAMING_INDEX_PLAN.ko.md
§6) and the switch-over (§10-§11, 0.12.370): the streaming build is the
default of `floe-index vfs SRC OUT` and builds the same cache as the
in-memory build (`--in-memory`).

For the battery fixture (valmini) and two generated files (tools/
gen_route_chip.py --scale 0.05: million-member point lists that the
normalization pass splits; tools/gen_main01_like.py --scale 0.001: a deep
hierarchy of 3,500 cells over 449 layers):
  - G3: the in-memory build (--in-memory) and the streaming build at jobs 1
    and 4, the budget set to the inventory's largest cell times 1.5 so cells
    are admitted, planned, encoded and released in waves, agree byte for
    byte on design.ovm / ovp / ovt / ovb / ovh / ovs / ovo and meta.json
    (both with --occupancy; design.ovc is not offered in streaming mode);
    the default command line (no mode flag, no --mem) streams and agrees
    too; the exact occupancy walk (--occupancy-prune 0) at a finer cell too;
    `--occupancy-only` on a cache without design.ovo streams (the
    switch-over) and writes the in-memory run's bytes;
  - G4: `floe2 index SRC` (the product's command line, default mode) and
    `floe2 index SRC --stream --mem G` build the same bytes as `floe-index
    vfs ... --in-memory --no-lod` (floe2 passes --no-lod), `floe2 index
    --in-memory` builds in memory; the refusals: `--stream --coverage` (2),
    `--in-memory --mem` (2), `--stream --in-memory` (2), a --mem below the
    largest cell (71); `--coverage` without a mode flag builds in memory;
    FLOE_INDEX_STREAM=off builds in memory, an unknown value notes and
    streams; the default budget rises to the file's largest cell when the
    host has it (FLOE_MEM_AVAILABLE_GB) and is refused with 71 when it has
    not; a file the inventory refuses (a CELLNAME reference number defined
    twice) falls back to the in-memory build in the default mode, exits 1
    under --stream; the S6 guards (the memory ceiling FLOE_RSS_CEILING, the
    size pre-check of the in-memory build) stop with exit 71 and the cache
    as it was;
  - G5: the peak resident set of the streaming build (ru_maxrss of its
    process) is bounded against the in-memory build's: the main build
    alone (no --occupancy, jobs 1) at 1.25 x the in-memory peak + 128 MB;
    with the occupancy pass (S5) at that plus half the budget - the pass
    keeps a quarter of the budget in planes and a quarter in resident
    cells, and it runs after the main build whose freed heap the
    allocator may keep (macOS does, so the two add up). The bound is
    enforced at jobs 1; the jobs 4 peak is reported (the parallel main
    build's peak varies by about 100 MB run to run).

usage: python tools/validate_vfs_stream.py valmini.oas
       python tools/validate_vfs_stream.py --compare FILE.oas [--jobs N]
           (the field check of §10.4: the in-memory and the default build
           of one file, compared byte for byte; nothing else runs)
"""
import json
import os
import re
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
BIN = ROOT / "rust" / "target" / "release" / "floe-index"
FLOE2 = ROOT / "rust" / "target" / "release" / "floe2"
# design.ovc is not offered in streaming mode; design.inv and design.seg
# (the inventory and the segment directory as files, DIRECT_READ_PLAN
# L1 / L2) are written by streaming builds only and checked on their
# own below
SKIP = {"design.ovc", "design.inv", "design.seg"}


def fail(msg):
    print("FAIL: %s" % msg)
    sys.exit(1)


def generate(script, out, *args):
    cmd = [sys.executable, "-B", str(ROOT / "tools" / script), str(out)] + list(args)
    r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True)
    if r.returncode != 0 or not Path(out).is_file():
        fail("%s: %s\n%s" % (script, r.stdout[-800:], r.stderr[-1200:]))


def run_measured(cmd, env=None):
    """run `cmd` in a child Python that reports the peak RSS of its own
    children (bytes), portable across macOS (bytes) and Linux (KiB)"""
    wrapper = (
        "import resource, subprocess, sys\n"
        "r = subprocess.run(sys.argv[1:], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)\n"
        "m = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss\n"
        "if sys.platform != 'darwin': m *= 1024\n"
        "sys.stdout.write(str(m) + '\\n')\n"
        "sys.stderr.write(r.stderr[-200000:])\n"
        "sys.exit(r.returncode)\n"
    )
    t0 = time.perf_counter()
    r = subprocess.run([sys.executable, "-c", wrapper] + [str(c) for c in cmd],
                       capture_output=True, text=True, env=env)
    dt = time.perf_counter() - t0
    peak = 0
    try:
        peak = int(r.stdout.strip().splitlines()[-1])
    except (ValueError, IndexError):
        pass
    return r.returncode, peak, dt, r.stderr


def largest_cell_bytes(src, jobs):
    r = subprocess.run([str(BIN), "inventory", str(src), "--jobs", str(jobs)],
                       capture_output=True, text=True)
    if r.returncode != 0:
        fail("inventory %s: %s" % (src.name, r.stderr[-800:]))
    j = json.loads(r.stdout)
    return int(j["largest_cell"]["doc_bytes"]), int(j["doc_bytes_est"])


def compare_dirs(a, b, what, skip=SKIP):
    fa = sorted(p.name for p in Path(a).iterdir() if p.is_file() and p.name not in skip)
    fb = sorted(p.name for p in Path(b).iterdir() if p.is_file() and p.name not in skip)
    if fa != fb:
        fail("%s: files differ: %s vs %s" % (what, fa, fb))
    for name in fa:
        if Path(a, name).read_bytes() != Path(b, name).read_bytes():
            fail("%s: %s differs" % (what, name))
    if "design.ovm" not in fa or "meta.json" not in fa:
        fail("%s: no cache built (%s)" % (what, fa))
    return fa


def _size(s):
    """a fmt_size string (517B, 1.3K, 4.6M, 58M, 1.2G) in bytes"""
    m = re.match(r"^(\d+(?:\.\d+)?)([BKMGT])$", s)
    if not m:
        fail("not a size: %r" % s)
    return float(m.group(1)) * (1024 ** "BKMGT".index(m.group(2)))


def check_resident_peak(name, label, err):
    """the markers' resident peak of every group stays within the resident
    budget (markpass.rs admission; 0.12.393: a set's ranges walked one at
    a time within the multi-definition span, the cells complete released -
    0.12.392 parsed a set at once and held a chain whole); on the chain
    fixture it is a fraction of the file's Doc estimate"""
    budgets = re.findall(r"resident budget (\S+) \(", err)
    peaks = re.findall(r"resident peak (\S+);", err)
    if not budgets and not peaks:
        return
    if not budgets or not peaks:
        fail("%s %s: resident budget %s / peak lines %s\n%s" % (name, label, budgets, peaks, err[-1500:]))
    budget = _size(budgets[0])
    worst = max(_size(p) for p in peaks)
    # (fmt_size rounds to two digits: a tenth of slack)
    if worst > budget * 1.1 + 4096:
        fail("%s %s: the markers' resident peak %s passed the resident budget %s\n%s"
             % (name, label, max(peaks, key=_size), budgets[0], err[-1500:]))
    if name == "cblock chain":
        est = re.findall(r"Doc estimate (\S+);", err)
        if not est:
            fail("%s %s: no Doc estimate line\n%s" % (name, label, err[-1200:]))
        if worst * 4 > _size(est[0]):
            fail("%s %s: resident peak %s against the file's Doc estimate %s - the chain's set was held whole"
                 % (name, label, max(peaks, key=_size), est[0]))
    return worst


def streamed(err):
    """the streaming build's log lines: the inventory pass's (its first),
    the main build's last, the done line's tag - a long log (every cell
    logged under --slow-cell-s 0) may be cut at the front"""
    return ("stream: inventory" in err or "stream: build complete" in err
            or "stream: placements" in err or "(streaming" in err)


# --- a minimal OASIS stream the inventory refuses: two CELLNAME records
# with the same reference number (the geometry parser resolves the
# reference by record position; a cell-wise build cannot reproduce that)

def _uint(v):
    out = bytearray()
    while True:
        b = v & 0x7F
        v >>= 7
        if v:
            out.append(b | 0x80)
        else:
            out.append(b)
            return bytes(out)


def _sint(v):
    return _uint((abs(v) << 1) | (1 if v < 0 else 0))


def _string(s):
    return _uint(len(s)) + s


def dup_cellname_oas():
    b = bytearray(b"%SEMI-OASIS\r\n")
    # START: version, unit 1000/um (real type 0), offset-flag 0 + 6 zero pairs
    b += bytes([1]) + _string(b"1.0") + bytes([0]) + _uint(1000) + _uint(0) + _uint(0) * 12
    b += bytes([4]) + _string(b"TOP") + _uint(0)
    b += bytes([4]) + _string(b"TOP2") + _uint(0)
    b += bytes([13]) + _uint(0)
    # RECTANGLE, info-byte W H X Y D L
    b += bytes([20, 0x7B]) + _uint(1) + _uint(0) + _uint(2000) + _uint(1000) + _sint(100) + _sint(200)
    # END: a 256-byte record, validation scheme 0
    b += bytes([2]) + _string(b"\0" * 252) + bytes([0])
    return bytes(b)


def compare_file(src, jobs):
    """--compare FILE: the field check of the switch-over - the in-memory
    build and the default (streaming) build of one file, byte for byte"""
    env = dict(os.environ, FLOE_LOCK="off")
    with tempfile.TemporaryDirectory(prefix="floe-stream-cmp-") as td:
        work = Path(td)
        for label, out, extra in (("in-memory", work / "mem", ["--in-memory"]), ("default", work / "def", [])):
            rc, peak, dt, err = run_measured(
                [BIN, "vfs", src, out, "--jobs", str(jobs), "--occupancy"] + extra, env)
            if rc != 0:
                fail("%s build exit %d\n%s" % (label, rc, err[-3000:]))
            if (label == "default") != streamed(err):
                fail("%s build: unexpected mode\n%s" % (label, err[-1500:]))
            print("%-9s build: %.1fs, peak rss %.0f MB" % (label, dt, peak / 1e6))
        names = compare_dirs(work / "mem", work / "def", "%s default vs in-memory" % src.name)
        print("OK  %s: %d files identical (%s)" % (src.name, len(names), ", ".join(names)))


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(2)
    for b in (BIN, FLOE2):
        if not b.is_file():
            fail("release binary missing: %s" % b)
    if sys.argv[1] == "--compare":
        if len(sys.argv) < 3:
            fail("--compare needs a file")
        jobs = 4
        if "--jobs" in sys.argv:
            jobs = int(sys.argv[sys.argv.index("--jobs") + 1])
        compare_file(Path(sys.argv[2]).resolve(), jobs)
        print("vfs_stream --compare: ALL OK")
        return
    src0 = Path(sys.argv[1]).resolve()
    env = os.environ.copy()
    env["FLOE_LOCK"] = "off"
    with tempfile.TemporaryDirectory(prefix="floe-stream-") as td:
        work = Path(td)
        files = []
        v = work / "valmini.oas"
        shutil.copy2(src0, v)
        files.append(("valmini", v))
        # L2: --no-seg leaves the segment directory out and the inventory in
        out = work / "valmini_noseg"
        r = subprocess.run([str(BIN), "vfs", str(v), str(out), "--jobs", "2", "--no-seg"],
                           capture_output=True, text=True, env=env)
        if r.returncode != 0 or (out / "design.seg").exists() or not (out / "design.inv").is_file():
            fail("--no-seg: exit %d, design.seg %s, design.inv %s\n%s" % (
                r.returncode, (out / "design.seg").exists(), (out / "design.inv").exists(), r.stderr[-800:]))
        print("OK  valmini            --no-seg: design.inv written, design.seg left out")
        route = work / "route_chip.oas"
        generate("gen_route_chip.py", route, "--scale", "0.05", "--jobs", "2")
        files.append(("route_chip 0.05", route))
        main01 = work / "main01_like.oas"
        generate("gen_main01_like.py", main01, "--scale", "0.001", "--jobs", "2")
        files.append(("main01_like 0.001", main01))
        # 24 cells defined inside one CBLOCK of another cell's range: the
        # marking pass parses that range once for all of them (0.12.391);
        # design.ovo must still be the in-memory build's
        cc = work / "cblock_cells.oas"
        r = subprocess.run([str(BIN.parent / "floe-dbg"), "fixture", "cblock-cells", str(cc)],
                           capture_output=True, text=True)
        if r.returncode != 0 or not cc.is_file():
            fail("floe-dbg fixture cblock-cells: exit %d\n%s" % (r.returncode, r.stderr[-800:]))
        files.append(("cblock cells", cc))
        # the same 24 cells defined on their own first and again in that
        # CBLOCK (the review of 0.12.391): one set of 25 cells and 25 ranges
        # parsed once (0.12.392)
        cc2 = work / "cblock_cells_twice.oas"
        r = subprocess.run([str(BIN.parent / "floe-dbg"), "fixture", "cblock-cells-twice", str(cc2)],
                           capture_output=True, text=True)
        if r.returncode != 0 or not cc2.is_file():
            fail("floe-dbg fixture cblock-cells-twice: exit %d\n%s" % (r.returncode, r.stderr[-800:]))
        files.append(("cblock cells twice", cc2))
        # a chain of 32 cells, each one's CBLOCK defining the next (the
        # review of 0.12.392): one set of 32 cells and 32 ranges - the file -
        # which the markers walk range by range within the multi-definition
        # span instead of holding whole (0.12.393); its budget below is
        # three cells, the span, where the others take one and a half
        chain = work / "cblock_chain.oas"
        r = subprocess.run([str(BIN.parent / "floe-dbg"), "fixture", "cblock-chain", str(chain)],
                           capture_output=True, text=True)
        if r.returncode != 0 or not chain.is_file():
            fail("floe-dbg fixture cblock-chain: exit %d\n%s" % (r.returncode, r.stderr[-800:]))
        files.append(("cblock chain", chain))
        largest_of = {}
        for name, src in files:
            largest, est = largest_cell_bytes(src, 4)
            largest_of[name] = largest
            mem_gib = max(largest * (3.0 if name == "cblock chain" else 1.5), 1 << 16) / float(1 << 30)
            budget = int(mem_gib * (1 << 30))
            ref = work / (src.stem + "_mem")
            rc, peak_mem, dt_mem, err = run_measured(
                [BIN, "vfs", src, ref, "--in-memory", "--jobs", "4", "--slow-cell-s", "0", "--occupancy"], env)
            if rc != 0:
                fail("in-memory build %s exit %d\n%s" % (name, rc, err[-2000:]))
            if streamed(err) or "[vfs] done in" not in err:
                fail("--in-memory %s did not build in memory\n%s" % (name, err[-1500:]))
            # G5, the main build alone: no occupancy pass, jobs 1
            out = work / (src.stem + "_stream_noocc")
            rc, peak0, dt0, err = run_measured(
                [BIN, "vfs", src, out, "--jobs", "1", "--slow-cell-s", "0",
                 "--stream", "--mem", "%.6f" % mem_gib], env)
            if rc != 0:
                fail("stream build %s (no occupancy) exit %d\n%s" % (name, rc, err[-2000:]))
            names = compare_dirs(ref, out, "%s stream (no occupancy) jobs 1" % name,
                                 skip=SKIP | {"design.ovo"})
            if (out / "design.ovo").exists():
                fail("%s: a streaming build without --occupancy wrote design.ovo" % name)
            bound0 = int(peak_mem * 1.25) + (128 << 20)
            verdict = "OK " if peak0 <= bound0 else "FAIL"
            print("%s %-18s jobs 1, no ovo: %d files identical; peak rss stream %.0f MB vs memory %.0f MB "
                  "(bound %.0f MB); %.1fs vs %.1fs"
                  % (verdict, name, len(names), peak0 / 1e6, peak_mem / 1e6, bound0 / 1e6, dt0, dt_mem))
            if verdict == "FAIL":
                fail("%s: streaming peak %d above the in-memory peak %d + slack" % (name, peak0, peak_mem))
            for jobs in (1, 4):
                out = work / ("%s_stream_j%d" % (src.stem, jobs))
                rc, peak, dt, err = run_measured(
                    [BIN, "vfs", src, out, "--jobs", str(jobs), "--slow-cell-s", "0",
                     "--occupancy", "--stream", "--mem", "%.6f" % mem_gib], env)
                if rc != 0:
                    fail("stream build %s jobs %d exit %d\n%s" % (name, jobs, rc, err[-2000:]))
                if not streamed(err):
                    fail("stream build %s jobs %d: no streaming log lines\n%s" % (name, jobs, err[-1500:]))
                names = compare_dirs(ref, out, "%s stream jobs %d" % (name, jobs))
                if not (out / "design.ovo").exists():
                    fail("%s: the streaming build wrote no design.ovo" % name)
                if "the build stopped" in err:
                    fail("%s jobs %d: the streaming build reported a failure\n%s" % (name, jobs, err[-1500:]))
                peak_resident = check_resident_peak(name, "jobs %d" % jobs, err)
                if name == "cblock chain" and jobs == 4:
                    print("OK  %-18s jobs 4: the markers' resident peak %s within the resident budget (a chain of 32 cells, one set)"
                          % (name, "%.1f MB" % (peak_resident / 1e6) if peak_resident else "?"))
                # with the occupancy pass: half the budget more (planes +
                # resident cells of the pass, on top of the main build's
                # kept heap); enforced at jobs 1, reported at jobs 4
                bound = int(peak_mem * 1.25) + (128 << 20) + budget // 2
                verdict = "OK " if (peak <= bound or jobs != 1) else "FAIL"
                print("%s %-18s jobs %d: %d files identical; Doc est %.0f MB, budget %.1f MB; "
                      "peak rss stream %.0f MB vs memory %.0f MB (bound %.0f MB%s); %.1fs vs %.1fs"
                      % (verdict, name, jobs, len(names), est / 1e6, mem_gib * 1024,
                         peak / 1e6, peak_mem / 1e6, bound / 1e6,
                         "" if jobs == 1 else ", reported", dt, dt_mem))
                if verdict == "FAIL":
                    fail("%s jobs %d: streaming peak %d above the in-memory peak %d + slack" % (name, jobs, peak, peak_mem))
            # the switch-over: the default command line (no mode flag, no
            # --mem) streams and builds the same bytes
            out = work / (src.stem + "_default")
            r = subprocess.run([str(BIN), "vfs", str(src), str(out), "--jobs", "4", "--slow-cell-s", "0", "--occupancy"],
                               capture_output=True, text=True, env=env)
            if r.returncode != 0:
                fail("default build %s exit %d\n%s" % (name, r.returncode, r.stderr[-2000:]))
            if not streamed(r.stderr):
                fail("default build %s did not stream\n%s" % (name, r.stderr[-1500:]))
            r_default_err = r.stderr
            check_resident_peak(name, "default", r_default_err)
            names = compare_dirs(ref, out, "%s default" % name)
            # L1 (DIRECT_READ_PLAN §8.2): the streaming build leaves the
            # inventory as design.inv - the bytes `floe-index inventory --out`
            # writes, identical to a fresh inventory of the source; the
            # in-memory build has no inventory and writes none
            inv = out / "design.inv"
            if not inv.is_file() or "stream: design.inv" not in r.stderr:
                fail("%s default: no design.inv\n%s" % (name, r.stderr[-800:]))
            if (ref / "design.inv").exists():
                fail("%s: the in-memory build wrote design.inv" % name)
            j4 = work / ("%s_stream_j4" % src.stem) / "design.inv"
            if not j4.is_file() or j4.read_bytes() != inv.read_bytes():
                fail("%s: design.inv differs between the default and the jobs-4 streaming build" % name)
            alone = work / (src.stem + "_alone.inv")
            r = subprocess.run([str(BIN), "inventory", str(src), "--jobs", "4", "--out", str(alone)],
                               capture_output=True, text=True)
            if r.returncode != 0 or alone.read_bytes() != inv.read_bytes():
                fail("%s: design.inv differs from floe-index inventory --out (exit %d)\n%s" % (name, r.returncode, r.stderr[-800:]))
            r = subprocess.run([str(BIN), "inventory", "--load", str(inv), str(src), "--jobs", "4"],
                               capture_output=True, text=True)
            if r.returncode != 0 or '"identical": true' not in r.stdout:
                fail("%s: design.inv --load beside the source (exit %d)\n%s" % (name, r.returncode, r.stderr[-800:]))
            # L2 (DIRECT_READ_PLAN §8.3): design.seg beside it - the bytes
            # `floe-index inventory --seg` writes, paired with design.inv,
            # identical to a fresh directory of the source
            seg = out / "design.seg"
            if not seg.is_file() or "stream: design.seg" not in r_default_err:
                fail("%s default: no design.seg\n%s" % (name, r_default_err[-800:]))
            if (ref / "design.seg").exists():
                fail("%s: the in-memory build wrote design.seg" % name)
            if (j4.parent / "design.seg").read_bytes() != seg.read_bytes():
                fail("%s: design.seg differs between the default and the jobs-4 streaming build" % name)
            alone_seg = work / (src.stem + "_alone.seg")
            r = subprocess.run([str(BIN), "inventory", str(src), "--jobs", "4", "--out", str(alone), "--seg", str(alone_seg)],
                               capture_output=True, text=True)
            if r.returncode != 0 or alone_seg.read_bytes() != seg.read_bytes():
                fail("%s: design.seg differs from floe-index inventory --seg (exit %d)\n%s" % (name, r.returncode, r.stderr[-800:]))
            r = subprocess.run([str(BIN), "inventory", "--load", str(inv), "--load-seg", str(seg), str(src), "--jobs", "4"],
                               capture_output=True, text=True)
            if r.returncode != 0 or '"pair_ok": true, "identical": true' not in r.stdout:
                fail("%s: design.seg --load-seg beside design.inv and the source (exit %d)\n%s" % (name, r.returncode, r.stderr[-800:]))
            # L2 (DIRECT_READ_PLAN §8.3): design.seg beside it - the bytes
            # `floe-index inventory --seg` writes, paired with design.inv,
            # identical to a fresh directory of the source
            seg = out / "design.seg"
            if not seg.is_file() or "stream: design.seg" not in r_default_err:
                fail("%s default: no design.seg\n%s" % (name, r_default_err[-800:]))
            if (ref / "design.seg").exists():
                fail("%s: the in-memory build wrote design.seg" % name)
            if (j4.parent / "design.seg").read_bytes() != seg.read_bytes():
                fail("%s: design.seg differs between the default and the jobs-4 streaming build" % name)
            alone_seg = work / (src.stem + "_alone.seg")
            r = subprocess.run([str(BIN), "inventory", str(src), "--jobs", "4", "--out", str(alone), "--seg", str(alone_seg)],
                               capture_output=True, text=True)
            if r.returncode != 0 or alone_seg.read_bytes() != seg.read_bytes():
                fail("%s: design.seg differs from floe-index inventory --seg (exit %d)\n%s" % (name, r.returncode, r.stderr[-800:]))
            r = subprocess.run([str(BIN), "inventory", "--load", str(inv), "--load-seg", str(seg), str(src), "--jobs", "4"],
                               capture_output=True, text=True)
            if r.returncode != 0 or '"pair_ok": true, "identical": true' not in r.stdout:
                fail("%s: design.seg --load-seg beside design.inv and the source (exit %d)\n%s" % (name, r.returncode, r.stderr[-800:]))
            # the switch-over: --occupancy-only on a cache without design.ovo
            # streams (inventory, placements pass, occupancy passes) and
            # writes the in-memory run's bytes; a stale design.inv planted
            # before the in-memory build is cleared with the other outputs
            out = work / (src.stem + "_occonly")
            out.mkdir(exist_ok=True)
            (out / "design.inv").write_bytes(b"stale")
            (out / "design.seg").write_bytes(b"stale")
            r = subprocess.run([str(BIN), "vfs", str(src), str(out), "--in-memory", "--jobs", "4", "--slow-cell-s", "0"],
                               capture_output=True, text=True, env=env)
            if r.returncode != 0 or (out / "design.ovo").exists():
                fail("cache without design.ovo %s: exit %d\n%s" % (name, r.returncode, r.stderr[-1500:]))
            if (out / "design.inv").exists() or (out / "design.seg").exists():
                fail("%s: a stale design.inv / design.seg survived the in-memory rebuild" % name)
            t0 = time.perf_counter()
            r = subprocess.run([str(BIN), "vfs", str(src), str(out), "--occupancy-only", "--jobs", "4"],
                               capture_output=True, text=True, env=env)
            dt_oo = time.perf_counter() - t0
            if r.returncode != 0:
                fail("--occupancy-only %s exit %d\n%s" % (name, r.returncode, r.stderr[-2000:]))
            if not streamed(r.stderr) or "placements of" not in r.stderr or "occupancy streaming" not in r.stderr:
                fail("--occupancy-only %s did not stream\n%s" % (name, r.stderr[-1500:]))
            compare_dirs(ref, out, "%s --occupancy-only (streaming)" % name)
            print("OK  %-18s default command line streams (%d files identical; design.inv = inventory --out, design.seg = inventory --seg and paired, both absent in memory, stale ones cleared); --occupancy-only streams design.ovo (identical, %.1fs)"
                  % (name, len(names), dt_oo))
        # the exact walk (--occupancy-prune 0) and a finer cell on valmini
        src = files[0][1]
        ref = work / "valmini_exact_mem"
        out = work / "valmini_exact_stream"
        for target, extra in ((ref, ["--in-memory"]), (out, ["--stream", "--mem", "0.5"])):
            r = subprocess.run([str(BIN), "vfs", str(src), str(target), "--jobs", "2", "--occupancy",
                                "--occupancy-prune", "0", "--occupancy-um", "1"] + extra,
                               capture_output=True, text=True, env=env)
            if r.returncode != 0:
                fail("exact-walk build %s: %s" % (target.name, r.stderr[-1500:]))
        compare_dirs(ref, out, "valmini exact walk (prune 0, 1 um)")
        print("OK  valmini --occupancy-prune 0 --occupancy-um 1: streaming identical")
        # G4: the product command line, and the refusals. The reference is
        # built from the SAME path (meta.json records the source path).
        src = files[0][1]
        penv = dict(env, FLOE_INDEX_BIN=str(BIN))
        for label, extra, expect_stream in (("--stream --mem 0.5", ["--stream", "--mem", "0.5"], True),
                                            ("(default)", [], True),
                                            ("--in-memory", ["--in-memory"], False)):
            prod_src = work / ("prod_" + str(len(label))) / "valmini.oas"
            prod_src.parent.mkdir()
            shutil.copy2(src, prod_src)
            ref = prod_src.parent / "ref_nolod_mem"
            r = subprocess.run([str(BIN), "vfs", str(prod_src), str(ref), "--in-memory", "--jobs", "2", "--no-lod"],
                               capture_output=True, text=True, env=env)
            if r.returncode != 0:
                fail("in-memory --no-lod build: %s" % r.stderr[-1500:])
            r = subprocess.run([str(FLOE2), "index", str(prod_src), "--jobs", "2"] + extra,
                               capture_output=True, text=True, env=penv)
            if r.returncode != 0:
                fail("floe2 index %s exit %d\n%s\n%s" % (label, r.returncode, r.stdout[-800:], r.stderr[-2000:]))
            if streamed(r.stderr + r.stdout) != expect_stream:
                fail("floe2 index %s: unexpected mode\n%s\n%s" % (label, r.stdout[-800:], r.stderr[-1500:]))
            caches = [p for p in prod_src.parent.iterdir() if p.is_dir() and p.name.endswith(".ice")]
            if len(caches) != 1:
                fail("floe2 index %s: expected one .ice cache, found %s" % (label, [c.name for c in caches]))
            compare_dirs(ref, caches[0], "floe2 index %s vs floe-index vfs --in-memory --no-lod" % label)
            print("OK  floe2 index %s: cache %s identical to floe-index vfs --in-memory --no-lod" % (label, caches[0].name))
        r = subprocess.run([str(BIN), "vfs", str(src), str(work / "x1"), "--stream", "--coverage"],
                           capture_output=True, text=True, env=env)
        if r.returncode != 2:
            fail("--stream --coverage should be refused with exit 2, got %d" % r.returncode)
        r = subprocess.run([str(BIN), "vfs", str(src), str(work / "x2"), "--in-memory", "--mem", "1"],
                           capture_output=True, text=True, env=env)
        if r.returncode != 2:
            fail("--in-memory --mem should be refused with exit 2, got %d" % r.returncode)
        r = subprocess.run([str(BIN), "vfs", str(src), str(work / "x2b"), "--stream", "--in-memory"],
                           capture_output=True, text=True, env=env)
        if r.returncode != 2:
            fail("--stream --in-memory should be refused with exit 2, got %d" % r.returncode)
        r = subprocess.run([str(BIN), "vfs", str(src), str(work / "x3"), "--mem", "0.0000001"],
                           capture_output=True, text=True, env=env)
        if r.returncode != 71 or "--mem" not in r.stderr:
            fail("a --mem below the largest cell should be refused with exit 71: exit %d\n%s" % (r.returncode, r.stderr[-600:]))
        if (work / "x3" / "design.ovm").exists():
            fail("a refused streaming build left a cache behind")
        r = subprocess.run([str(BIN), "vfs", str(src), str(work / "x4"), "--jobs", "2", "--coverage"],
                           capture_output=True, text=True, env=env)
        if r.returncode != 0 or streamed(r.stderr) or not (work / "x4" / "design.ovc").exists():
            fail("--coverage without a mode flag should build in memory: exit %d\n%s" % (r.returncode, r.stderr[-1000:]))
        print("OK  refusals: --stream --coverage (2), --in-memory --mem (2), --stream --in-memory (2), --mem below the largest cell (71); --coverage builds in memory")
        # FLOE_INDEX_STREAM: off = the in-memory build, an unknown value notes
        # and streams
        ref = work / "valmini_mem"
        r = subprocess.run([str(BIN), "vfs", str(src), str(work / "envoff"), "--jobs", "2", "--occupancy"],
                           capture_output=True, text=True, env=dict(env, FLOE_INDEX_STREAM="off"))
        if r.returncode != 0 or streamed(r.stderr) or "[vfs] reading" not in r.stderr:
            fail("FLOE_INDEX_STREAM=off should build in memory: exit %d\n%s" % (r.returncode, r.stderr[-1000:]))
        compare_dirs(ref, work / "envoff", "FLOE_INDEX_STREAM=off")
        r = subprocess.run([str(BIN), "vfs", str(src), str(work / "envbad"), "--jobs", "2"],
                           capture_output=True, text=True, env=dict(env, FLOE_INDEX_STREAM="maybe"))
        if r.returncode != 0 or not streamed(r.stderr) or "FLOE_INDEX_STREAM=maybe" not in r.stderr:
            fail("FLOE_INDEX_STREAM=maybe should note and stream: exit %d\n%s" % (r.returncode, r.stderr[-1000:]))
        print("OK  FLOE_INDEX_STREAM=off builds in memory (identical); an unknown value notes and streams")
        # the default budget rises to the file's largest cell when the host
        # has it (route_chip: a 117 MB top cell; MemAvailable 1.5 x it ->
        # the default budget is half of it, below the cell), and is refused
        # with 71 when the host has not (0.5 x)
        rsrc = files[1][1]
        largest = largest_of[files[1][0]]
        ref = work / (rsrc.stem + "_mem")
        r = subprocess.run([str(BIN), "vfs", str(rsrc), str(work / "raise"), "--jobs", "2", "--slow-cell-s", "0", "--occupancy"],
                           capture_output=True, text=True,
                           env=dict(env, FLOE_MEM_AVAILABLE_GB="%.9f" % (largest * 1.5 / 1e9)))
        if r.returncode != 0 or "the budget is raised to" not in r.stderr:
            fail("the default budget should rise to the largest cell: exit %d\n%s" % (r.returncode, r.stderr[-1500:]))
        compare_dirs(ref, work / "raise", "route_chip, budget raised")
        r = subprocess.run([str(BIN), "vfs", str(rsrc), str(work / "noraise"), "--jobs", "2"],
                           capture_output=True, text=True,
                           env=dict(env, FLOE_MEM_AVAILABLE_GB="%.9f" % (largest * 0.5 / 1e9)))
        if r.returncode != 71 or "cannot rise" not in r.stderr or (work / "noraise").exists():
            fail("a need past the host should be refused with 71: exit %d\n%s" % (r.returncode, r.stderr[-1500:]))
        print("OK  default budget: raised to the largest cell within the host (identical); refused (71) past it")
        # S6 guards (STREAMING_INDEX_PLAN §5): the memory ceiling stops a
        # build at its first check, the size pre-check refuses an in-memory
        # build of a file larger than MemAvailable - exit 71, the cache as
        # it was, one line saying what to do; off, the build runs
        keep = work / "valmini_keep"
        r = subprocess.run([str(BIN), "vfs", str(src), str(keep), "--jobs", "2"],
                           capture_output=True, text=True, env=env)
        if r.returncode != 0:
            fail("guard reference build: %s" % r.stderr[-1500:])
        snapshot = lambda: {p.name: (p.stat().st_mtime_ns, p.read_bytes()) for p in keep.iterdir() if p.is_file()}
        before = snapshot()
        for label, extra, more, needle in (
                ("memory ceiling, in-memory build", ["--in-memory"], {"FLOE_RSS_CEILING": "1M"}, "memory ceiling"),
                ("memory ceiling, streaming build", ["--stream", "--mem", "0.5"], {"FLOE_RSS_CEILING": "1M"}, "memory ceiling"),
                ("memory ceiling, default build", [], {"FLOE_RSS_CEILING": "1M"}, "memory ceiling"),
                ("size pre-check", ["--in-memory"], {"FLOE_MEM_AVAILABLE_GB": "0.000001"}, "index with --stream")):
            r = subprocess.run([str(BIN), "vfs", str(src), str(keep), "--jobs", "2"] + extra,
                               capture_output=True, text=True, env=dict(env, **more), timeout=120)
            if r.returncode != 71:
                fail("%s: exit %d, want 71\n%s" % (label, r.returncode, r.stderr[-1500:]))
            if needle not in r.stderr or "the cache is unchanged" not in r.stderr and label != "size pre-check":
                fail("%s: the stop line is missing\n%s" % (label, r.stderr[-1500:]))
            if snapshot() != before:
                fail("%s: the cache changed" % label)
        r = subprocess.run([str(BIN), "vfs", str(src), str(keep), "--jobs", "2"],
                           capture_output=True, text=True, env=dict(env, FLOE_RSS_CEILING="off"))
        if r.returncode != 0:
            fail("FLOE_RSS_CEILING=off: the build should run: exit %d\n%s" % (r.returncode, r.stderr[-1500:]))
        r = subprocess.run([str(BIN), "vfs", str(src), str(keep), "--jobs", "2"],
                           capture_output=True, text=True, env=dict(env, FLOE_RSS_CEILING="12x"))
        if r.returncode != 0 or "the default ceiling applies" not in r.stderr:
            fail("FLOE_RSS_CEILING=12x: the build should run with a note: exit %d\n%s" % (r.returncode, r.stderr[-1500:]))
        print("OK  guards: memory ceiling (in-memory, streaming, default) and the size pre-check exit 71 and keep the cache; off and a bad setting build")
        # a file the inventory refuses (a CELLNAME reference number defined
        # twice): --in-memory builds it, --stream exits 1 before any output,
        # the default falls back to the in-memory build with the same bytes
        dup = work / "dup_cellname.oas"
        dup.write_bytes(dup_cellname_oas())
        r = subprocess.run([str(BIN), "vfs", str(dup), str(work / "dup_mem"), "--in-memory", "--jobs", "1"],
                           capture_output=True, text=True, env=env)
        if r.returncode != 0:
            fail("dup-cellname file: the in-memory build should run: exit %d\n%s" % (r.returncode, r.stderr[-1500:]))
        r = subprocess.run([str(BIN), "vfs", str(dup), str(work / "dup_stream"), "--stream", "--jobs", "1"],
                           capture_output=True, text=True, env=env)
        if r.returncode != 1 or "inventory" not in r.stderr or (work / "dup_stream").exists():
            fail("dup-cellname file: --stream should exit 1 before any output: exit %d\n%s" % (r.returncode, r.stderr[-1500:]))
        r = subprocess.run([str(BIN), "vfs", str(dup), str(work / "dup_default"), "--jobs", "1"],
                           capture_output=True, text=True, env=env)
        if r.returncode != 0 or "the in-memory build takes over" not in r.stderr:
            fail("dup-cellname file: the default should fall back: exit %d\n%s" % (r.returncode, r.stderr[-1500:]))
        compare_dirs(work / "dup_mem", work / "dup_default", "dup-cellname fallback")
        print("OK  a file the inventory refuses: --in-memory builds, --stream exits 1, the default falls back (identical)")
        # a source that changes under the reader: one line, exit 1, no hang
        broken = work / "broken.oas"
        data = bytearray(src.read_bytes())
        inv = subprocess.run([str(BIN), "inventory", str(src), "--cells"], capture_output=True, text=True)
        j = json.loads(inv.stdout)
        starts = [c["ranges"][0][0] for c in j["cells_detail"] if c["ranges"]]
        data[starts[1]] = 99
        broken.write_bytes(bytes(data))
        # the inventory refuses the broken record itself; so corrupt a copy
        # AFTER the inventory would run: emulate by patching a byte deep
        # inside a cell's range that the inventory does not inspect is not
        # possible, hence the check is on the error path's shape only (the
        # default mode falls back to the in-memory parse, which refuses too)
        for label, extra in (("--stream", ["--stream", "--mem", "1"]), ("default", [])):
            t0 = time.perf_counter()
            try:
                r = subprocess.run([str(BIN), "vfs", str(broken), str(work / ("x5_" + label.strip("-"))), "--jobs", "2"] + extra,
                                   capture_output=True, text=True, env=env, timeout=120)
            except subprocess.TimeoutExpired:
                fail("a %s build of a corrupt source hung for 120 s" % label)
            if r.returncode == 0:
                fail("a corrupt source built a cache (%s)" % label)
            print("OK  corrupt source, %s: exit %d in %.1fs (%s)" % (label, r.returncode, time.perf_counter() - t0, (r.stderr.strip().splitlines() or ['?'])[-1][:100]))
    print("vfs_stream: ALL OK")


if __name__ == "__main__":
    main()
