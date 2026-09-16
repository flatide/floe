#!/usr/bin/env python3
"""Actual HTTP/WS + native renderer gate; no Python in the tested runtime."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

from validate_view_controller import ROOT, digest


def main(fixture):
    cargo = shutil.which("cargo") or str(Path.home() / ".cargo/bin/cargo")
    build = subprocess.run([cargo, "test", "--offline", "-p", "floe-web", "--test",
                            "view_stream", "--no-run", "--message-format=json"],
                           cwd=ROOT / "rust", text=True, capture_output=True, timeout=180)
    assert build.returncode == 0, build.stderr
    tests = [r["executable"] for line in build.stdout.splitlines()
             if (r := json.loads(line)).get("reason") == "compiler-artifact"
             and r["target"]["name"] == "view_stream" and r.get("executable")]
    assert len(tests) == 1
    with tempfile.TemporaryDirectory(prefix="floe-view-stream-") as td:
        work = Path(td)
        source = work / "설계 with spaces.oas"
        second = work / "second synthetic.oas"
        for target in (source, second):
            shutil.copy2(fixture, target)
        workers = work / "workers"
        workers.mkdir()
        env = dict(os.environ, PATH="", TMPDIR=str(workers), FLOE_VIEW_FIXTURE=str(source),
                   FLOE_VIEW_SECOND_FIXTURE=str(second),
                   FLOE_RENDERD_BIN=str(ROOT / "rust/target/release/floe-renderd"),
                   FLOE_INDEX_BIN=str(ROOT / "rust/target/release/floe-index"))
        caches = [Path(str(target) + ".floe") for target in (source, second)]
        for target in (source, second):
            p = subprocess.run([str(ROOT / "rust/target/release/floe2-web"), "index", str(target),
                                "--jobs", "2"], env=env, cwd=ROOT, text=True, capture_output=True, timeout=40)
            assert p.returncode == 0, (p.stdout, p.stderr)
        before = [digest(cache) for cache in caches]
        test = subprocess.run([tests[0], "--ignored", "--nocapture"], env=env,
                              text=True, capture_output=True, timeout=90)
        assert test.returncode == 0, (test.stdout, test.stderr)
        assert "RUST VIEW STREAM: ALL OK" in test.stdout
        assert "RUST MARGIN STREAM: ALL OK" in test.stdout
        assert "RUST BAND STREAM: ALL OK" in test.stdout
        assert "RUST MINIMAP STREAM: ALL OK" in test.stdout
        assert "RUST DEPTH STREAM: ALL OK" in test.stdout
        assert "RUST PALETTE STREAM: ALL OK" in test.stdout
        assert "RUST PALETTE READ: ALL OK" in test.stdout
        assert "RUST PALETTE STYLE STREAM: ALL OK" in test.stdout
        assert "RUST FILL SLOT STREAM: ALL OK" in test.stdout
        assert "RUST LOCAL SHARE GRANTS: ALL OK" in test.stdout
        assert "RUST LOCAL FOLLOW: ALL OK" in test.stdout
        assert "RUST LOCAL FOLLOW MARGIN: ALL OK" in test.stdout
        assert "RUST LOCAL FOLLOW LIFETIME: ALL OK" in test.stdout
        assert "RUST LOCAL FOLLOW BACKPRESSURE: ALL OK" in test.stdout
        assert "RUST LOCAL EXPLORE ISOLATION: ALL OK" in test.stdout
        assert "RUST LOCAL EXPLORE SCOPE: ALL OK" in test.stdout
        assert "RUST LOCAL EXPLORE LIFETIME: ALL OK" in test.stdout
        assert "RUST LOCAL EXPLORE DECK SCOPE: ALL OK" in test.stdout
        assert "RUST GUEST QUERIES: ALL OK" in test.stdout
        assert "RUST GUEST QUERY LIFETIME: ALL OK" in test.stdout
        assert "RUST FOLLOW QUERY DENIAL: ALL OK" in test.stdout
        assert [digest(cache) for cache in caches] == before, "stream modified cache bytes/mtime"
        assert not list(workers.iterdir()), "stream worker files leaked"
        print(test.stdout.strip())


if __name__ == "__main__":
    main(Path(sys.argv[1]).resolve())
