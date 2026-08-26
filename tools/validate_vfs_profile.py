#!/usr/bin/env python3
"""Non-publishing single-cell VFS profiler contract gate."""

import json
from pathlib import Path
import subprocess
import sys
import tempfile


ROOT = Path(__file__).resolve().parents[1]
INDEX = ROOT / "rust" / "target" / "release" / "floe-index"


def run(src, outdir, *options, ok=True):
    result = subprocess.run(
        [str(INDEX), "vfs", str(src), str(outdir), *map(str, options)],
        cwd=ROOT, capture_output=True, text=True)
    if ok and result.returncode:
        raise AssertionError(
            "profile failed\nstdout:\n%s\nstderr:\n%s" %
            (result.stdout, result.stderr))
    if not ok and not result.returncode:
        raise AssertionError("invalid profile unexpectedly succeeded")
    return result


def main():
    src = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(
        tempfile.gettempdir()) / "floe-valmini" / "valmini.oas"
    if not INDEX.is_file():
        raise AssertionError("release floe-index is not built")
    if not src.is_file():
        raise AssertionError("profile fixture is missing: %s" % src)

    with tempfile.TemporaryDirectory(prefix="floe-vfs-profile-") as td:
        outdir = Path(td) / "must-not-touch.floe"
        outdir.mkdir()
        sentinel = outdir / "sentinel"
        sentinel.write_bytes(b"unchanged")

        result = run(src, outdir, "--jobs", 2, "--no-lod",
                     "--profile-cell-ci", 0)
        report = json.loads(result.stdout)
        assert report["mode"] == "vfs-cell-profile"
        assert report["cell"]["index"] == 0
        assert report["settings"]["jobs"] == 2
        assert report["settings"]["lod"] is False
        assert report["settings"]["writes"] is False
        assert report["timing_s"]["plan"] >= 0
        assert set(report["phase_s"]) == {
            "bvh", "assemble", "split", "lod", "pts", "sink"}
        assert isinstance(report["layers"], list)
        assert "wrote 0 cache files" in result.stderr
        assert list(outdir.iterdir()) == [sentinel]
        assert sentinel.read_bytes() == b"unchanged"

        # Exact-name selection, including spaces/UTF-8-safe argv handling,
        # resolves to the same parsed cell without publishing anything.
        by_name = run(src, outdir, "--jobs", 1, "--profile-cell",
                      report["cell"]["name"])
        named = json.loads(by_name.stdout)
        assert named["cell"] == report["cell"]
        assert list(outdir.iterdir()) == [sentinel]

        bad = run(src, outdir, "--profile-cell-ci", 999999999, ok=False)
        assert "out of range" in bad.stderr
        assert list(outdir.iterdir()) == [sentinel]

    print("VFS CELL PROFILE VALIDATION: ALL OK")


if __name__ == "__main__":
    main()
