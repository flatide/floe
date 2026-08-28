#!/usr/bin/env python3
"""KLayout-free contract checks for ``floe index`` Rust delegation."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile


ROOT = Path(__file__).resolve().parents[1]


def check(condition, message):
    if not condition:
        raise AssertionError(message)


def run(env, *args, ok=True):
    result = subprocess.run(
        [sys.executable, "-B", "-m", "floe", "index", *map(str, args)],
        cwd=ROOT, env=env, capture_output=True, text=True)
    if ok and result.returncode != 0:
        raise AssertionError(
            "command failed: %s\nstdout:\n%s\nstderr:\n%s" %
            (args, result.stdout, result.stderr))
    if not ok and result.returncode == 0:
        raise AssertionError("command unexpectedly succeeded: %s" % (args,))
    return result


def calls(path):
    if not path.exists():
        return []
    return [json.loads(line) for line in path.read_text().splitlines()]


def build_calls(path):
    return [call for call in calls(path) if call and call[0] == "vfs"]


def validate_real_marker():
    binary = ROOT / "rust" / "target" / "release" / "floe-index"
    fixture = ROOT / "data" / "m1" / "valmini.oas"
    check(binary.is_file(), "release floe-index is not built")
    check(fixture.is_file(), "valmini fixture is missing")
    with tempfile.TemporaryDirectory(prefix="floe-index-marker-") as td:
        src = Path(td) / "valmini.oas"
        shutil.copy2(fixture, src)
        env = os.environ.copy()
        env.update({
            "FLOE_INDEX_BIN": str(binary),
            "PYTHONDONTWRITEBYTECODE": "1",
            "PYTHONPATH": str(ROOT),
        })
        run(env, src, "--jobs", "2")
        (Path(str(src) + ".floe") / "design.ovm").write_bytes(b"x")
        rejected = run(env, src, ok=False)
        check("commit validation failed" in rejected.stderr,
              "real Vfs::open accepted a partial OVM marker")


def main():
    with tempfile.TemporaryDirectory(prefix="floe-index-cli-") as td:
        work = Path(td)
        binary_dir = work / "bin with space"
        binary_dir.mkdir()
        binary = binary_dir / "floe-index"
        log = work / "calls.jsonl"
        blocker = work / "no-klayout"
        blocker.mkdir()
        (blocker / "sitecustomize.py").write_text("""import builtins
real_import = builtins.__import__
def checked_import(name, *args, **kwargs):
    if name == "klayout" or name.startswith("klayout."):
        raise RuntimeError("KLayout import forbidden by index CLI gate")
    return real_import(name, *args, **kwargs)
builtins.__import__ = checked_import
""", encoding="utf-8")
        binary.write_text("""#!/usr/bin/env python3
import json, os, pathlib, sys
args = sys.argv[1:]
with open(os.environ["FLOE_INDEX_TEST_LOG"], "a", encoding="utf-8") as f:
    f.write(json.dumps(args) + "\\n")
if len(args) == 2 and args[0] == "vfsd":
    marker = pathlib.Path(args[1]) / "design.ovm"
    sys.exit(0 if marker.read_bytes() == b"committed" else 1)
if args and args[0] == "vfs" and ("--profile-cell" in args or
                                  "--profile-cell-ci" in args):
    print(json.dumps({"mode": "vfs-cell-profile", "writes": False}))
    sys.exit(0)
if len(args) < 3 or args[0] != "vfs":
    sys.exit(2)
src, out = pathlib.Path(args[1]), pathlib.Path(args[2])
out.mkdir(parents=True, exist_ok=True)
if "--coverage-only" in args:
    (out / "design.ovc").write_bytes(b"coverage")
else:
    st = src.stat()
    meta = {"version": 8, "vfs": 1,
            "src": {"path": str(src), "size": st.st_size,
                    "mtime": int(st.st_mtime)}}
    (out / "meta.json").write_text(json.dumps(meta), encoding="utf-8")
    (out / "design.ovm").write_bytes(b"committed")
    if "--coverage" in args:
        (out / "design.ovc").write_bytes(b"coverage")
""", encoding="utf-8")
        binary.chmod(0o755)
        env = os.environ.copy()
        env.update({
            "FLOE_INDEX_BIN": str(binary),
            "FLOE_INDEX_TEST_LOG": str(log),
            "PYTHONDONTWRITEBYTECODE": "1",
            "PYTHONPATH": os.pathsep.join((str(blocker), str(ROOT))),
        })

        src = work / "design.oas"
        src.write_bytes(b"fixture")
        run(env, src, "--jobs", "3", "--page-target-mb", "2",
            "--coverage", "--no-lod", "--slow-cell-s", "0",
            "--p2-shard-limit-mb", "0")
        expected = [
            "vfs", str(src), str(src) + ".floe", "--jobs", "3",
            "--page-target-mb", "2", "--coverage", "--no-lod",
            "--slow-cell-s", "0.0", "--p2-shard-limit-mb", "0",
        ]
        check(build_calls(log) == [expected],
              "Rust VFS options were not forwarded")

        reused = run(env, src)
        check("cache up to date" in reused.stdout, "current cache not reused")
        check(len(build_calls(log)) == 1,
              "reuse unexpectedly launched an index build")
        check(calls(log)[-1] == ["vfsd", str(src) + ".floe"],
              "reuse did not validate the committed Rust cache")

        src.write_bytes(b"fixture changed")
        stale = run(env, src, ok=False)
        check("--force" in stale.stderr, "stale-cache error omitted --force")
        check(len(build_calls(log)) == 1,
              "stale cache was replaced without --force")
        run(env, src, "--force")
        check(len(build_calls(log)) == 2,
              "--force did not launch a rebuild")

        additive = work / "additive.oas"
        additive.write_bytes(b"fixture 2")
        run(env, additive)
        run(env, additive, "--coverage")
        check(build_calls(log)[-1] == [
            "vfs", str(additive), str(additive) + ".floe",
            "--jobs", "12", "--coverage-only",
        ], "coverage was not added non-destructively")

        before = len(build_calls(log))
        legacy = run(env, additive, "--tile-mb", "4", ok=False)
        check("--legacy" in legacy.stderr, "legacy option was not gated")
        check(len(build_calls(log)) == before,
              "legacy option unexpectedly launched the Rust indexer")

        cache = Path(str(additive) + ".floe")
        meta_path = cache / "meta.json"
        meta = json.loads(meta_path.read_text())
        meta["version"] = 7
        meta_path.write_text(json.dumps(meta))
        old = run(env, additive, ok=False)
        check("cache version" in old.stderr,
              "old cache version was incorrectly reused")
        meta["version"] = 8
        meta_path.write_text(json.dumps(meta))

        marker = cache / "design.ovm"
        marker.write_bytes(b"x")
        partial = run(env, additive, ok=False)
        check("commit validation failed" in partial.stderr,
              "partial OVM marker was incorrectly reused")
        marker.write_bytes(b"committed")

        # A cell profile bypasses cache reuse and does not even pass the cache
        # path to floe-index. Relevant planner knobs still propagate.
        cache_snapshot = {p.name: p.read_bytes() for p in cache.iterdir()
                          if p.is_file()}
        profile_snapshot = work / "scratch profile.bin"
        profiled = run(env, additive, "--jobs", "7", "--no-lod",
                       "--page-target-mb", "3", "--profile-cell",
                       "MONSTER CELL", "--profile-jobs", "8,12,16",
                       "--profile-repeat", "2", "--profile-snapshot",
                       profile_snapshot, "--profile-snapshot-refresh")
        check(json.loads(profiled.stdout)["mode"] == "vfs-cell-profile",
              "profile stdout was not one standalone JSON object")
        check("[floe]" in profiled.stderr,
              "profile command provenance was not kept on stderr")
        check(calls(log)[-1] == [
            "vfs", str(additive), "--jobs", "7",
            "--page-target-mb", "3", "--no-lod",
            "--profile-cell", "MONSTER CELL",
            "--profile-jobs", "8,12,16", "--profile-repeat", "2",
            "--profile-snapshot", str(profile_snapshot),
            "--profile-snapshot-refresh",
        ], "cell profile argv is not the non-publishing form")
        check(cache_snapshot == {
            p.name: p.read_bytes() for p in cache.iterdir() if p.is_file()},
              "cell profile modified an existing cache")
        conflict = run(env, additive, "--force", "--profile-cell-ci", "0",
                       ok=False)
        check("cannot be combined" in conflict.stderr,
              "profile accepted a destructive --force combination")
        tuning_without_cell = run(
            env, additive, "--profile-jobs", "8,16", ok=False)
        check("require --profile-cell" in tuning_without_cell.stderr,
              "profile tuning was accepted without a cell selector")

        invalid_env = env.copy()
        invalid_env["FLOE_INDEX_BIN"] = str(work / "missing-floe-index")
        invalid = run(invalid_env, additive, ok=False)
        check("FLOE_INDEX_BIN is set" in invalid.stderr,
              "invalid explicit binary silently fell through")

    validate_real_marker()
    print("INDEX CLI VALIDATION: ALL OK")


if __name__ == "__main__":
    main()
