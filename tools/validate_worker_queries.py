#!/usr/bin/env python3
"""Scene-pinned native query gate. Synthetic data only; runtime PATH is empty."""
import ast
import hashlib
import json
import math
import os
from pathlib import Path
from cache_test_paths import vfs_cache
import random
import shutil
import subprocess
import tempfile
from types import SimpleNamespace

import klayout.db as db

ROOT = Path(__file__).resolve().parents[1]


def fingerprints(directory):
    return {p.name: (hashlib.sha256(p.read_bytes()).hexdigest(), p.stat().st_mtime_ns)
            for p in directory.iterdir() if p.is_file()}


def ruler_oracle():
    # Compile ONLY the GTK pure bbox rule: no GUI import or copied oracle.
    tree = ast.parse((ROOT / "floe/gui.py").read_text())
    functions = [n for n in ast.walk(tree) if isinstance(n, ast.FunctionDef)
                 and n.name == "_measure_selection"]
    assert len(functions) == 1
    namespace = {"math": math}
    exec(compile(ast.Module(body=functions, type_ignores=[]), "gtk-ruler-oracle", "exec"), namespace)
    rng = random.Random(6746)
    cases = [[], [[0, 0, 1, 1]], [[0, 0, 2, 2], [2, 0, 4, 2]],
             [[0, 0, 1, 1], [3, 4, 5, 6]],
             [[0, 0, 1, 1], [2, 0, 3, 1], [2, 0, 3, 1]]]
    for count in [2, 3, 8, 16, 32, 64] * 3:
        boxes = []
        for _ in range(count):
            x, y = rng.randrange(-2000, 2000), rng.randrange(-2000, 2000)
            boxes.append([x, y, x + rng.randrange(0, 80), y + rng.randrange(0, 80)])
        cases.append(boxes)
    result = []
    for boxes in cases:
        view = SimpleNamespace(selections=[{"bbox": b} for b in boxes], _auto_rulers=[], rulers=[])
        namespace["_measure_selection"](view)
        result.append({"boxes": [[str(n) for n in b] for b in boxes], "rulers": view.rulers})
    return json.dumps(result, separators=(",", ":"))


def main():
    cargo = shutil.which("cargo") or str(Path.home() / ".cargo/bin/cargo")
    binaries = []
    for package, target in [("floe-worker-client", "real_queries"), ("floe-app-core", "view_queries"),
                            ("floe-web", "query_stream")]:
        build = subprocess.run([cargo, "test", "--offline", "--locked", "-p", package,
                                "--test", target, "--no-run", "--message-format=json"],
                               cwd=ROOT / "rust", text=True, capture_output=True, timeout=180)
        assert build.returncode == 0, build.stderr
        found = [r["executable"] for line in build.stdout.splitlines()
                 if (r := json.loads(line)).get("reason") == "compiler-artifact"
                 and r["target"]["name"] == target and r.get("executable")]
        assert len(found) == 1
        binaries.extend(found)
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
        cache = vfs_cache(source)
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
                   FLOE_QUERY_GAPS=ruler_oracle(),
                   FLOE_QUERY_SOURCE=str(source),
                   FLOE_VIEW_FIXTURE=str(source),
                   FLOE_QUERY_RENDERD=str(ROOT / "rust/target/release/floe-renderd"),
                   FLOE_RENDERD_BIN=str(ROOT / "rust/target/release/floe-renderd"),
                   FLOE_INDEX_BIN=indexer,
                   FLOE_QUERY_POLYGON_AREA=str(polygon.area()),
                   FLOE_RUST_OCCUPANCY="on", FLOE_RUST_OCCUPANCY_PX="1",
                   FLOE_RUST_PAN_REUSE="on", FLOE_RUST_RETAINED_MB="64", FLOE_RUST_QUERY_INLINE="0")
        for binary, marker in zip(binaries, ["RUST PINNED QUERIES: ALL OK", "RUST VIEW QUERIES: ALL OK",
                                            "WEB QUERY GEOMETRY: ALL OK"]):
            run = subprocess.run([binary, "--ignored", "--nocapture"], env=env,
                                 text=True, capture_output=True, timeout=60)
            assert run.returncode == 0, (run.stdout, run.stderr)
            assert marker in run.stdout
            if marker == "RUST VIEW QUERIES: ALL OK":
                assert "RUST CAPTURE QUERY GUARD: ALL OK" in run.stdout
            if marker == "WEB QUERY GEOMETRY: ALL OK":
                assert "WEB QUERY LIFECYCLE: ALL OK" in run.stdout
                assert "WEB QUERY REJECTIONS: ALL OK" in run.stdout
                assert "WEB MANUAL RULERS: ALL OK" in run.stdout
                assert "WEB SELECTION RULERS: ALL OK" in run.stdout
            print(run.stdout.strip())
        assert fingerprints(cache) == before, "queries modified index/summary bytes or mtime"
        assert not list(workers.iterdir()), "query workers leaked private files"


if __name__ == "__main__":
    main()
