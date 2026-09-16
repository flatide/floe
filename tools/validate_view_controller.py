#!/usr/bin/env python3
"""Native session controller gate; private synthetic input, empty runtime PATH.

Python only prepares the fixture and launches the Rust test, not the controller.
"""
import hashlib
import json
import os
from pathlib import Path
from cache_test_paths import vfs_cache
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def digest(directory):
    return {p.name: (hashlib.sha256(p.read_bytes()).hexdigest(), p.stat().st_mtime_ns)
            for p in directory.iterdir() if p.is_file()}


def main(fixture):
    cargo = shutil.which("cargo") or str(Path.home() / ".cargo/bin/cargo")
    build = subprocess.run([cargo, "test", "--offline", "-p", "floe-app-core",
                            "--test", "view_controller", "--no-run",
                            "--message-format=json"], cwd=ROOT / "rust",
                           capture_output=True, text=True, timeout=180)
    assert build.returncode == 0, build.stderr
    tests = [r["executable"] for line in build.stdout.splitlines()
             if (r := json.loads(line)).get("reason") == "compiler-artifact"
             and r["target"]["name"] == "view_controller" and r.get("executable")]
    assert len(tests) == 1, tests
    with tempfile.TemporaryDirectory(prefix="floe-view-controller-") as td:
        work = Path(td)
        source = work / "설계 with spaces.oas"
        shutil.copy2(fixture, source)
        workers = work / "workers"
        workers.mkdir()
        env = dict(os.environ, PATH="", TMPDIR=str(workers),
                   FLOE_VIEW_FIXTURE=str(source),
                   FLOE_RENDERD_BIN=str(ROOT / "rust/target/release/floe-renderd"),
                   FLOE_INDEX_BIN=str(ROOT / "rust/target/release/floe-index"))
        p = subprocess.run([str(ROOT / "rust/target/release/floe2-web"), "index",
                            str(source), "--jobs", "2"], env=env, cwd=ROOT,
                           text=True, capture_output=True, timeout=40)
        assert p.returncode == 0, (p.stdout, p.stderr)
        cache = vfs_cache(source)
        before = digest(cache)
        run = subprocess.run([tests[0], "--ignored", "--nocapture"], env=env,
                             text=True, capture_output=True, timeout=90)
        assert run.returncode == 0, (run.stdout, run.stderr)
        assert "15 PNG pairs including fill slots" in run.stdout, run.stdout
        assert "RUST VIEW MARGIN: ALL OK" in run.stdout, run.stdout
        assert digest(cache) == before, "view modified cache bytes/mtime"
        assert not list(workers.iterdir()), "worker temporary files leaked"
        print(run.stdout.strip())


if __name__ == "__main__":
    main(Path(sys.argv[1]).resolve())
