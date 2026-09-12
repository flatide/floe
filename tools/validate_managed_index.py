#!/usr/bin/env python3
"""Registered/managed index gate; all runtime services run with PATH empty."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def main(fixture):
    cargo = shutil.which("cargo") or str(Path.home() / ".cargo/bin/cargo")
    build = subprocess.run([cargo, "test", "--offline", "--locked", "-p", "floe-app-core",
                            "--test", "managed_index", "--no-run", "--message-format=json"],
                           cwd=ROOT / "rust", text=True, capture_output=True, timeout=180)
    assert build.returncode == 0, build.stderr
    tests = [r["executable"] for line in build.stdout.splitlines()
             if (r := json.loads(line)).get("reason") == "compiler-artifact"
             and r["target"]["name"] == "managed_index" and r.get("executable")]
    assert len(tests) == 1
    with tempfile.TemporaryDirectory(prefix="floe-managed-index-") as td:
        work = Path(td)
        source = work / "설계 with spaces.oas"
        shutil.copy2(fixture, source)
        workers = work / "workers"
        workers.mkdir()
        env = dict(os.environ, PATH="", TMPDIR=str(workers), FLOE_MANAGED_FIXTURE=str(source),
                   FLOE_INDEX_BIN=str(ROOT / "rust/target/release/floe-index"))
        run = subprocess.run([tests[0], "--ignored", "--nocapture"], env=env,
                             text=True, capture_output=True, timeout=180)
        assert run.returncode == 0, (run.stdout, run.stderr)
        assert "RUST MANAGED INDEX: ALL OK" in run.stdout
        assert not list(workers.iterdir()), "managed index leaked worker files"
        print(run.stdout.strip())


if __name__ == "__main__":
    main(Path(sys.argv[1]).resolve())
