#!/usr/bin/env python3
"""Scene-pinned native query gate. Synthetic data only; runtime PATH is empty."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

import klayout.db as db

ROOT = Path(__file__).resolve().parents[1]


def fingerprints(directory):
    return {p.name: (hashlib.sha256(p.read_bytes()).hexdigest(), p.stat().st_mtime_ns)
            for p in directory.iterdir() if p.is_file()}


def main():
    cargo = shutil.which("cargo") or str(Path.home() / ".cargo/bin/cargo")
    build = subprocess.run([cargo, "test", "--offline", "--locked", "-p", "floe-worker-client",
                            "--test", "real_queries", "--no-run", "--message-format=json"],
                           cwd=ROOT / "rust", text=True, capture_output=True, timeout=180)
    assert build.returncode == 0, build.stderr
    binaries = [r["executable"] for line in build.stdout.splitlines()
                if (r := json.loads(line)).get("reason") == "compiler-artifact"
                and r["target"]["name"] == "real_queries" and r.get("executable")]
    assert len(binaries) == 1
    with tempfile.TemporaryDirectory(prefix="floe-query-gate-") as td:
        root = Path(td)
        source = root / "query 한글.oas"
        layout = db.Layout()
        layout.dbu = 0.001
        top = layout.create_cell("TOP_QUERY")
        a, b = layout.layer(7, 0), layout.layer(8, 0)
        top.shapes(a).insert(db.Box(0, 0, 10000, 10000))
        top.shapes(b).insert(db.Box(-1000, -1000, 11000, 11000))
        points = [(20000, 0), (50000, 0), (50000, 10000)]
        points += [(50000 - i * 40, 9000 if i % 2 else 10000) for i in range(600)]
        points += [(20000, 10000)]
        polygon = db.Polygon([db.Point(x, y) for x, y in points])
        assert polygon.num_points_hull() > 512
        top.shapes(a).insert(polygon)
        layout.write(str(source))
        cache = Path(str(source) + ".floe")
        indexer = str(ROOT / "rust/target/release/floe-index")
        index = subprocess.run([indexer, "vfs", str(source), str(cache), "--jobs", "2",
                                "--no-lod", "--occupancy", "--occupancy-um", "1"],
                               text=True, capture_output=True, timeout=45)
        assert index.returncode == 0, (index.stdout, index.stderr)
        assert (cache / "design.ovo").is_file(), (index.stdout, index.stderr)

        def listing():
            p = subprocess.run([indexer, "occupancy", str(cache)], text=True,
                               capture_output=True, timeout=10)
            assert p.returncode == 0, (p.stdout, p.stderr)
            return {r["ld"]: r for line in p.stdout.splitlines() if line.startswith("layer ")
                    if (r := dict(word.split("=", 1) for word in line.split()[1:]))}

        rows = listing()
        assert rows["7/0"]["status"] == rows["8/0"]["status"] == "ok", rows
        assert int(rows["7/0"]["work"]) > int(rows["8/0"]["work"]), rows
        # Produce a genuine mixed summary via the existing native safety valve,
        # without patching cache bytes or depending on a guessed work counter.
        mixed = subprocess.run([indexer, "vfs", str(source), str(cache), "--jobs", "2",
                                "--occupancy-only", "--occupancy-um", "1",
                                "--occupancy-max-work", rows["8/0"]["work"]],
                               text=True, capture_output=True, timeout=45)
        assert mixed.returncode == 0, (mixed.stdout, mixed.stderr)
        rows = listing()
        assert rows["7/0"]["status"] == "none:work" and rows["8/0"]["status"] == "ok", rows
        workers = root / "workers"
        workers.mkdir()
        before = fingerprints(cache)
        env = dict(os.environ, PATH="", TMPDIR=str(workers), FLOE_QUERY_CACHE=str(cache),
                   FLOE_QUERY_RENDERD=str(ROOT / "rust/target/release/floe-renderd"),
                   FLOE_QUERY_POLYGON_AREA=str(polygon.area()),
                   FLOE_RUST_OCCUPANCY="on", FLOE_RUST_OCCUPANCY_PX="1",
                   FLOE_RUST_PAN_REUSE="on", FLOE_RUST_RETAINED_MB="64")
        run = subprocess.run([binaries[0], "--ignored", "--nocapture"], env=env,
                             text=True, capture_output=True, timeout=60)
        assert run.returncode == 0, (run.stdout, run.stderr)
        assert "RUST PINNED QUERIES: ALL OK" in run.stdout
        assert fingerprints(cache) == before, "queries modified index/summary bytes or mtime"
        assert not list(workers.iterdir()), "query workers leaked private files"
        print(run.stdout.strip())


if __name__ == "__main__":
    main()
