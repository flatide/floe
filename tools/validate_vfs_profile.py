#!/usr/bin/env python3
"""Non-publishing single-cell VFS profiler contract gate."""

import json
import os
from pathlib import Path
import shutil
import stat
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
        # Work on a copy so stale-fingerprint tests never mutate the shared
        # deterministic fixture used by the rest of validate_rust.sh.
        profile_src = Path(td) / src.name
        shutil.copy2(src, profile_src)
        outdir = Path(td) / "must-not-touch.floe"
        outdir.mkdir()
        sentinel = outdir / "sentinel"
        sentinel.write_bytes(b"unchanged")

        result = run(profile_src, outdir, "--jobs", 2, "--no-lod",
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
        by_name = run(profile_src, outdir, "--jobs", 1, "--profile-cell",
                      report["cell"]["name"])
        named = json.loads(by_name.stdout)
        assert named["cell"] == report["cell"]
        assert list(outdir.iterdir()) == [sentinel]

        bad = run(profile_src, outdir, "--profile-cell-ci", 999999999,
                  ok=False)
        assert "out of range" in bad.stderr
        assert list(outdir.iterdir()) == [sentinel]
        bad_jobs = run(
            profile_src, outdir, "--profile-cell-ci", 0,
            "--profile-jobs", "0", ok=False)
        assert "positive comma-separated" in bad_jobs.stderr
        assert "panicked" not in bad_jobs.stderr
        bad_refresh = run(
            profile_src, outdir, "--profile-cell-ci", 0,
            "--profile-snapshot-refresh", ok=False)
        assert "requires --profile-snapshot" in bad_refresh.stderr

        # One parse/prepare feeds the full sequential jobs/repeat matrix. A
        # newly named snapshot is atomically published outside the VFS cache.
        snapshot = Path(td) / "selected-cell.profile-snapshot"
        series_result = run(
            profile_src, outdir, "--jobs", 2, "--no-lod",
            "--profile-cell", "VALMINI_TOP", "--profile-jobs", "1,2",
            "--profile-repeat", 2, "--profile-snapshot", snapshot)
        series = json.loads(series_result.stdout)
        assert isinstance(series, list) and len(series) == 4
        assert [item["settings"]["jobs"] for item in series] == [1, 1, 2, 2]
        assert [item["settings"]["repeat"] for item in series] == [1, 2, 1, 2]
        assert all(item["snapshot"]["state"] == "saved" for item in series)
        assert all(item["cell"]["name"] == "VALMINI_TOP"
                   for item in series)
        assert series_result.stderr.count("parsed 7 cells") == 1
        assert snapshot.is_file() and snapshot.stat().st_size > 0
        if os.name == "posix":
            assert stat.S_IMODE(snapshot.stat().st_mode) == 0o600
        assert list(outdir.iterdir()) == [sentinel]

        # A second process reconstructs the prepared selected-cell input from
        # the explicit snapshot: no source read/parse or recursive bbox pass.
        loaded_result = run(
            profile_src, outdir, "--jobs", 2, "--no-lod",
            "--profile-cell", "VALMINI_TOP",
            "--profile-snapshot", snapshot)
        loaded = json.loads(loaded_result.stdout)
        assert loaded["snapshot"]["state"] == "loaded"
        assert loaded["timing_s"]["read"] == 0
        assert loaded["timing_s"]["parse"] == 0
        assert loaded["timing_s"]["prepare"] >= 0
        assert loaded["work"] == series[0]["work"]
        assert "[vfs] reading" not in loaded_result.stderr
        assert list(outdir.iterdir()) == [sentinel]

        wrong_cell = run(
            profile_src, outdir, "--profile-cell-ci", 1,
            "--profile-snapshot", snapshot, ok=False)
        assert "not the requested cell" in wrong_cell.stderr

        corrupt = Path(td) / "corrupt.profile-snapshot"
        damaged = bytearray(snapshot.read_bytes())
        damaged[len(damaged) // 2] ^= 0x40
        corrupt.write_bytes(damaged)
        broken = run(
            profile_src, outdir, "--profile-cell-ci", 0,
            "--profile-snapshot", corrupt, ok=False)
        assert "checksum mismatch" in broken.stderr
        assert "panicked" not in broken.stderr

        # Source changes are never silently accepted. Refresh explicitly
        # replaces the scratch snapshot, after which it is reusable again.
        before = profile_src.stat()
        os.utime(profile_src, ns=(before.st_atime_ns,
                                  before.st_mtime_ns + 2_000_000_000))
        stale = run(
            profile_src, outdir, "--profile-cell-ci", 0,
            "--profile-snapshot", snapshot, ok=False)
        assert "source fingerprint changed" in stale.stderr
        refreshed_result = run(
            profile_src, outdir, "--profile-cell-ci", 0,
            "--profile-snapshot", snapshot,
            "--profile-snapshot-refresh")
        refreshed = json.loads(refreshed_result.stdout)
        assert refreshed["snapshot"]["state"] == "saved"
        reloaded = json.loads(run(
            profile_src, outdir, "--profile-cell-ci", 0,
            "--profile-snapshot", snapshot).stdout)
        assert reloaded["snapshot"]["state"] == "loaded"
        assert list(outdir.iterdir()) == [sentinel]

    print("VFS CELL PROFILE VALIDATION: ALL OK")


if __name__ == "__main__":
    main()
