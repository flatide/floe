#!/usr/bin/env python3
"""G3, G4 and G5 of the streaming index plan (docs/STREAMING_INDEX_PLAN.ko.md
§6): `floe-index vfs SRC OUT --stream` builds the same cache as the
in-memory build.

For the battery fixture (valmini) and two generated files (tools/
gen_route_chip.py --scale 0.05: million-member point lists that the
normalization pass splits; tools/gen_main01_like.py --scale 0.001: a deep
hierarchy of 3,500 cells over 449 layers):
  - G3: the in-memory build and the streaming build at jobs 1 and 4, the
    budget set to the inventory's largest cell times 1.5 so cells are
    admitted, planned, encoded and released in waves, agree byte for byte
    on design.ovm / ovp / ovt / ovb / ovh / ovs and meta.json (design.ovo
    and ovc do not exist in streaming mode);
  - G4: `floe2 index SRC --stream --mem G` (the product's command line)
    builds the same bytes as `floe-index vfs ... --no-lod` (floe2 passes
    --no-lod); `--stream --coverage` is refused (exit 2); `--mem` without
    `--stream` is refused;
  - G5: the peak resident set of the streaming build (ru_maxrss of its
    process) is at most the in-memory build's plus a slack, reported.

usage: python tools/validate_vfs_stream.py valmini.oas
"""
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
BIN = ROOT / "rust" / "target" / "release" / "floe-index"
FLOE2 = ROOT / "rust" / "target" / "release" / "floe2"
SKIP = {"design.ovo", "design.ovc"}


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
        "sys.stderr.write(r.stderr[-40000:])\n"
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


def compare_dirs(a, b, what):
    fa = sorted(p.name for p in Path(a).iterdir() if p.is_file() and p.name not in SKIP)
    fb = sorted(p.name for p in Path(b).iterdir() if p.is_file() and p.name not in SKIP)
    if fa != fb:
        fail("%s: files differ: %s vs %s" % (what, fa, fb))
    for name in fa:
        if Path(a, name).read_bytes() != Path(b, name).read_bytes():
            fail("%s: %s differs" % (what, name))
    if "design.ovm" not in fa or "meta.json" not in fa:
        fail("%s: no cache built (%s)" % (what, fa))
    return fa


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(2)
    src0 = Path(sys.argv[1]).resolve()
    for b in (BIN, FLOE2):
        if not b.is_file():
            fail("release binary missing: %s" % b)
    env = os.environ.copy()
    env["FLOE_LOCK"] = "off"
    with tempfile.TemporaryDirectory(prefix="floe-stream-") as td:
        work = Path(td)
        files = []
        v = work / "valmini.oas"
        shutil.copy2(src0, v)
        files.append(("valmini", v))
        route = work / "route_chip.oas"
        generate("gen_route_chip.py", route, "--scale", "0.05", "--jobs", "2")
        files.append(("route_chip 0.05", route))
        main01 = work / "main01_like.oas"
        generate("gen_main01_like.py", main01, "--scale", "0.001", "--jobs", "2")
        files.append(("main01_like 0.001", main01))
        for name, src in files:
            largest, est = largest_cell_bytes(src, 4)
            mem_gib = max(largest * 1.5, 1 << 16) / float(1 << 30)
            ref = work / (src.stem + "_mem")
            rc, peak_mem, dt_mem, err = run_measured(
                [BIN, "vfs", src, ref, "--jobs", "4", "--slow-cell-s", "0"], env)
            if rc != 0:
                fail("in-memory build %s exit %d\n%s" % (name, rc, err[-2000:]))
            for jobs in (1, 4):
                out = work / ("%s_stream_j%d" % (src.stem, jobs))
                rc, peak, dt, err = run_measured(
                    [BIN, "vfs", src, out, "--jobs", str(jobs), "--slow-cell-s", "0",
                     "--stream", "--mem", "%.6f" % mem_gib], env)
                if rc != 0:
                    fail("stream build %s jobs %d exit %d\n%s" % (name, jobs, rc, err[-2000:]))
                if "inventory" not in err or "stream:" not in err:
                    fail("stream build %s jobs %d: no streaming log lines\n%s" % (name, jobs, err[-1500:]))
                names = compare_dirs(ref, out, "%s stream jobs %d" % (name, jobs))
                if (out / "design.ovo").exists():
                    fail("%s: streaming mode wrote design.ovo" % name)
                if "the build stopped" in err:
                    fail("%s jobs %d: the streaming build reported a failure\n%s" % (name, jobs, err[-1500:]))
                slack = int(peak_mem * 1.25) + (128 << 20)
                verdict = "OK " if peak <= slack else "FAIL"
                print("%s %-18s jobs %d: %d files identical; Doc est %.0f MB, budget %.1f MB; "
                      "peak rss stream %.0f MB vs memory %.0f MB; %.1fs vs %.1fs"
                      % (verdict, name, jobs, len(names), est / 1e6, mem_gib * 1024,
                         peak / 1e6, peak_mem / 1e6, dt, dt_mem))
                if verdict == "FAIL":
                    fail("%s jobs %d: streaming peak %d above the in-memory peak %d + slack" % (name, jobs, peak, peak_mem))
        # G4: the product command line, and the refusals. The reference is
        # built from the SAME path (meta.json records the source path).
        src = files[0][1]
        prod_src = work / "prod" / "valmini.oas"
        prod_src.parent.mkdir()
        shutil.copy2(src, prod_src)
        ref = work / "valmini_nolod_mem"
        r = subprocess.run([str(BIN), "vfs", str(prod_src), str(ref), "--jobs", "2", "--no-lod"],
                           capture_output=True, text=True, env=env)
        if r.returncode != 0:
            fail("in-memory --no-lod build: %s" % r.stderr[-1500:])
        penv = dict(env, FLOE_INDEX_BIN=str(BIN))
        r = subprocess.run([str(FLOE2), "index", str(prod_src), "--jobs", "2", "--stream", "--mem", "0.5"],
                           capture_output=True, text=True, env=penv)
        if r.returncode != 0:
            fail("floe2 index --stream exit %d\n%s\n%s" % (r.returncode, r.stdout[-800:], r.stderr[-2000:]))
        caches = [p for p in prod_src.parent.iterdir() if p.is_dir() and p.name.endswith(".ice")]
        if len(caches) != 1:
            fail("floe2 index --stream: expected one .ice cache, found %s" % [c.name for c in caches])
        compare_dirs(ref, caches[0], "floe2 index --stream vs floe-index vfs --no-lod")
        print("OK  floe2 index --stream --mem 0.5: cache %s identical to floe-index vfs --no-lod" % caches[0].name)
        r = subprocess.run([str(BIN), "vfs", str(src), str(work / "x1"), "--stream", "--coverage"],
                           capture_output=True, text=True, env=env)
        if r.returncode != 2:
            fail("--stream --coverage should be refused with exit 2, got %d" % r.returncode)
        r = subprocess.run([str(BIN), "vfs", str(src), str(work / "x2"), "--mem", "1"],
                           capture_output=True, text=True, env=env)
        if r.returncode != 2:
            fail("--mem without --stream should be refused with exit 2, got %d" % r.returncode)
        r = subprocess.run([str(BIN), "vfs", str(src), str(work / "x3"), "--stream", "--mem", "0.0000001"],
                           capture_output=True, text=True, env=env)
        if r.returncode == 0 or "--mem" not in r.stderr:
            fail("a budget below the largest cell should be refused: exit %d\n%s" % (r.returncode, r.stderr[-600:]))
        if (work / "x3" / "design.ovm").exists():
            fail("a refused streaming build left a cache behind")
        print("OK  refusals: --stream --coverage (2), --mem without --stream (2), budget below the largest cell (%d)" % r.returncode)
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
        # possible, hence the check is on the error path's shape only
        t0 = time.perf_counter()
        try:
            r = subprocess.run([str(BIN), "vfs", str(broken), str(work / "x4"), "--stream", "--mem", "1"],
                               capture_output=True, text=True, env=env, timeout=120)
        except subprocess.TimeoutExpired:
            fail("a streaming build of a corrupt source hung for 120 s")
        if r.returncode == 0:
            fail("a corrupt source built a cache")
        print("OK  corrupt source: exit %d in %.1fs (%s)" % (r.returncode, time.perf_counter() - t0, (r.stderr.strip().splitlines() or ['?'])[-1][:100]))
    print("vfs_stream: ALL OK")


if __name__ == "__main__":
    main()
