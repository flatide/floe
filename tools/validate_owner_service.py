#!/usr/bin/env python3
"""Native owner HTTP service gate; no Python in the tested runtime."""
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
    build = subprocess.run([cargo, "test", "--offline", "--locked", "-p", "floe-web",
                            "--test", "owner_service", "--no-run", "--message-format=json"],
                           cwd=ROOT / "rust", text=True, capture_output=True, timeout=180)
    assert build.returncode == 0, build.stderr
    tests = [r["executable"] for line in build.stdout.splitlines()
             if (r := json.loads(line)).get("reason") == "compiler-artifact"
             and r["target"]["name"] == "owner_service" and r.get("executable")]
    assert len(tests) == 1
    with tempfile.TemporaryDirectory(prefix="floe-owner-service-") as td:
        work = Path(td)
        source = work / "설계 with spaces.oas"
        shutil.copy2(fixture, source)
        workers = work / "workers"
        workers.mkdir()
        env = dict(os.environ, PATH="", TMPDIR=str(workers), FLOE_OWNER_FIXTURE=str(source),
                   FLOE_INDEX_BIN=str(ROOT / "rust/target/release/floe-index"),
                   FLOE_RENDERD_BIN=str(ROOT / "rust/target/release/floe-renderd"))
        run = subprocess.run([tests[0], "--ignored", "--nocapture"], env=env,
                             text=True, capture_output=True, timeout=120)
        assert run.returncode == 0, (run.stdout, run.stderr)
        assert "RUST OWNER SERVICE: ALL OK" in run.stdout
        assert "RUST DRC ISOLATION: ALL OK" in run.stdout
        assert "RUST DRC BUILD IDENTITY: ALL OK" in run.stdout
        assert not list(workers.iterdir()), "owner service leaked worker files"
        print(run.stdout.strip())


if __name__ == "__main__":
    main(Path(sys.argv[1]).resolve())
