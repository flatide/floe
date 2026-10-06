"""Reproducible full-preparation benchmark on scattered rectangle errors.

Usage: python tools/bench_drc_prepare.py --counts 10000 1000000 --jobs 1 4
       python tools/bench_drc_prepare.py --counts 10000000 --native-only
Each cold run has its own analysis cache. OS disk caches are not flushed.
Fixture generation/packing is separate from measured preprocessing time.
"""

import argparse
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))


def fixture(folder, count):
    from floe.vfsclient import find_binary
    source, pack, rules = [folder / name for name in ("data.db", "data.tray", "rules.json")]
    with source.open("w") as stream:
        stream.write("BENCH 100000\nWIDTH\n%d %d 0\n" % (count, count))
        for start in range(0, count, 4096):
            lines = []
            for ei in range(start, min(start + 4096, count)):
                location = ei * 7919 % count
                x, y = location % 1000 * 20000, location // 1000 * 20000
                width = 1000 + ei % 101 * 30
                lines.append("p %d 4\n%d %d\n%d %d\n%d %d\n%d %d\n" %
                             (ei + 1, x, y, x + width, y, x + width, y + 10000,
                              x, y + 10000))
            stream.write("".join(lines))
    rules.write_text(json.dumps({"format": "floe-svrf-rules", "version": 1,
        "checks": {"WIDTH": {"constraints": [
            {"metric": "width", "op": "<", "value": 0.05,
             "text": "INTERNAL M1 < 0.05"}]}}}))
    began = time.perf_counter()
    subprocess.run([find_binary(), "drc", str(source), str(pack)],
                   check=True, capture_output=True)
    return pack, rules, time.perf_counter() - began


def worker(args):
    from floe import drc_delta_cache, drc_prepare, drc_spatial
    stages = []

    def timed(module, name, stage):
        original = getattr(module, name)
        def call(*pos, **kw):
            began = time.perf_counter()
            result = original(*pos, **kw)
            stages.append({"stage": kw.get("mode", stage),
                           "seconds": time.perf_counter() - began})
            return result
        setattr(module, name, call)

    timed(drc_spatial, "prepare_rule", "spatial")
    timed(drc_delta_cache, "process_measure", "cd")
    timed(drc_delta_cache, "prepare_group_cache", "group")
    began = time.perf_counter()
    drc_prepare.prepare(args.pack, args.rules, backend=args.backend,
                        jobs=args.worker_jobs, report=lambda _: None)
    value = {"prepare_seconds": time.perf_counter() - began, "stages": stages}
    # Check fixture correctness after the clock, including every measurement.
    validation_started = time.perf_counter()
    import numpy as np
    from floe import cachepath, drc_delta, svrf
    db = drc_prepare.open_pack(args.pack)
    try:
        index = drc_delta.DeltaIndex(db, 0, svrf.load_rules(args.rules)["checks"]["WIDTH"]["constraints"])
        assert drc_delta_cache.load_measurements(index)
        for start in range(0, len(index.measured_ticks), 65536):
            stop = min(start + 65536, len(index.measured_ticks))
            expected = 1000 + np.arange(start, stop) % 101 * 30
            np.testing.assert_array_equal(index.measured_ticks[start:stop], expected)
            assert np.all(index.constraint_indices[start:stop] == 0)
            assert np.all(index.estimated_flags[start:stop])
        folder = drc_delta_cache._rule_dir(index)[0]
        manifest = json.loads((Path(folder) / "measure" / "complete.json").read_text())
        value["measurement_backend"] = manifest.get("backend")
        value["precision_fallback_count"] = manifest.get("precision_fallback_count")
        directory = Path(cachepath.drc_analysis_dir(db.path))
        value["analysis_bytes"] = sum(path.stat().st_size for path in directory.rglob("*") if path.is_file())
    finally:
        db.close()
    value["validation_seconds"] = time.perf_counter() - validation_started
    print(json.dumps(value), flush=True)


def run(args):
    print(json.dumps({"platform": platform.platform(), "python": sys.version.split()[0],
                      "logical_cpus": os.cpu_count(), "fixture": "scattered rectangles, one width predicate",
                      "note": "Fresh analysis caches; OS caches retained; pack build excluded."}), flush=True)
    with tempfile.TemporaryDirectory(prefix="floe-native-benchmark-") as temporary:
        for count in args.counts:
            folder = Path(temporary) / str(count)
            folder.mkdir()
            pack, rules, pack_seconds = fixture(folder, count)
            cases = ([] if args.native_only else [("python", 1)]) + [("rust", jobs) for jobs in args.jobs]
            for backend, jobs in cases:
                env = dict(os.environ, FLOE_DRC_ANALYSIS_ROOT=str(folder / ("%s-%d" % (backend, jobs))))
                command = [sys.executable, __file__, "--worker", "--pack", str(pack),
                           "--rules", str(rules), "--backend", backend, "--worker-jobs", str(jobs)]
                for phase in ("cold", "warm"):
                    began = time.perf_counter()
                    result = subprocess.run(command, cwd=ROOT, env=env, check=True,
                                            capture_output=True, text=True)
                    value = json.loads(result.stdout)
                    elapsed = time.perf_counter() - began
                    value.update(count=count, backend=backend, jobs=jobs, phase=phase,
                                 pack_seconds=pack_seconds,
                                 wall_seconds=elapsed - value["validation_seconds"])
                    print(json.dumps(value), flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--counts", nargs="+", type=int, default=[10000, 1000000])
    parser.add_argument("--jobs", nargs="+", type=int, default=[1, 4])
    parser.add_argument("--native-only", action="store_true")
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--pack", help=argparse.SUPPRESS)
    parser.add_argument("--rules", help=argparse.SUPPRESS)
    parser.add_argument("--backend", help=argparse.SUPPRESS)
    parser.add_argument("--worker-jobs", type=int, help=argparse.SUPPRESS)
    args = parser.parse_args()
    if any(count <= 0 for count in args.counts):
        parser.error("counts must be positive")
    worker(args) if args.worker else run(args)
