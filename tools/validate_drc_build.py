#!/usr/bin/env python3
"""Explicit DRC build gate; all inputs, child faults and outputs are synthetic.

The real product uses only native binaries (PATH empty). Python below is solely
the oracle/fault injector, never a product fallback.
"""
import json
import hashlib
import os
from pathlib import Path
from cache_test_paths import drc_pack
import signal
import subprocess
import sys
import tempfile
import time

from validate_drc_ice import DB

ROOT = Path(__file__).resolve().parents[1]
CLI = ROOT / "rust/target/release/floe2-web"
NATIVE = ROOT / "rust/target/release/floe-index"


def fingerprint(path):
    return (path.read_bytes(), path.stat().st_mtime_ns, path.stat().st_ino,
            path.stat().st_mode & 0o777)


def clean_stage(root):
    assert not list(root.glob(".floe-drc-build-*")), "build left staging files"


def call(source, env, *flags, ok=True):
    p = subprocess.run([str(CLI), "drc", str(source), "--build", *flags], env=env,
                       capture_output=True, text=True, timeout=30)
    if ok:
        assert p.returncode == 0, p.stderr
        return json.loads(p.stdout)
    assert p.returncode != 0, p.stdout
    assert not p.stdout, "failure emitted success JSON"
    return p


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def parent_aliases(root, env):
    real = root / "alias target"
    real.mkdir()
    alias = root / "parent alias"
    alias.symlink_to(real, target_is_directory=True)
    source = alias / "results.db"
    source.write_text(DB)
    original = fingerprint(source)
    output = drc_pack(source)
    legacy = Path(str(source) + ".ice")
    for force in (False, False, True):
        old = fingerprint(output) if output.exists() else None
        result = call(source, env, *(["--force"] if force else []))
        assert result["migration"] is None, "parent alias was mistaken for legacy naming"
        assert result["reused"] is (old is not None and not force)
        if result["reused"]:
            assert fingerprint(output) == old
        assert fingerprint(source) == original
    for force in (False, True):
        old_bytes = output.read_bytes()
        output.rename(legacy)
        result = call(source, env, *(["--force"] if force else []))
        assert result["migration"] is not None and result["reused"] is not force
        assert output.read_bytes() == old_bytes and not legacy.exists()

    # Normalizing parents must not follow a cache leaf symlink, nor change the
    # logical basename of a source leaf symlink (cache belongs beside that name).
    for target in (output, legacy):
        output.unlink(missing_ok=True)
        target.symlink_to(source)
        call(source, env, "--force", ok=False)
        assert target.is_symlink() and fingerprint(source) == original
        target.unlink()
    linked_source = alias / "named.db"
    linked_source.symlink_to(source)
    result = call(linked_source, env)
    assert Path(result["pack"]) == real / ".named.db.tray"
    assert drc_pack(linked_source).is_file() and not output.exists()
    assert linked_source.is_symlink() and fingerprint(source) == original
    clean_stage(real)
    print("DRC BUILD PARENT ALIAS: ALL OK (fresh/reuse/force, legacy, leaf protection)")


def parallel_native(work, env):
    # Native uses at least 4 MiB per worker. A tiny --jobs=16 fixture would
    # silently run one worker and would not exercise parallel parsing at all.
    source = work / "parallel.db"
    record = b"p 1 128\n" + b"1 2\n" * 128
    with source.open("wb") as stream:
        stream.write(b"TOP 1000\n")
        for ci in range(520):
            stream.write(f"RULE.{ci}\n256 256 0\n".encode())
            stream.write(record * 256)
    assert (source.stat().st_size - len(b"TOP 1000\n")) // (4 << 20) >= 16
    expected = None
    for jobs in (1, 4, 16):
        built = call(source, env, "--force", f"--jobs={jobs}")
        assert built["errors"] == 520 * 256 and built["checks"] == 520
        actual = digest(drc_pack(source))
        if expected is None:
            expected = actual
        assert actual == expected
    clean_stage(work)


def main():
    with tempfile.TemporaryDirectory(prefix="floe-drc-build-gate-") as temp:
        root = Path(temp).resolve()
        work = root / "공백 DRC"
        work.mkdir()
        source = work / "results.db"
        source.write_bytes(DB.encode())
        output = drc_pack(source)
        golden = work / "native.ice"
        env = dict(os.environ, PATH="", FLOE_INDEX_BIN=str(NATIVE))
        parent_aliases(root, env)
        subprocess.run([str(NATIVE), "drc", str(source), str(golden), "--jobs", "1"],
                       env=env, capture_output=True, check=True, timeout=20)
        original = fingerprint(source)
        built = call(source, env, "--jobs=1")
        assert not built["reused"] and built["bytes"] == golden.stat().st_size
        assert output.read_bytes() == golden.read_bytes(), "wrapper changed native pack bytes"
        assert output.stat().st_mode & 0o777 == 0o600
        old = fingerprint(output)
        assert call(source, env)["reused"]
        assert fingerprint(output) == old
        for jobs in (4, 16):
            assert not call(source, env, "--force", f"--jobs={jobs}")["reused"]
            assert output.read_bytes() == golden.read_bytes()
        parallel_native(work, env)
        # Adjacent review/temp files never enter the native cleanup namespace.
        protected = {work / "results.db.ice.tmp-other": b"another run",
                     work / ".results.db.waive.Kim": b"review state",
                     work / ".results.db.notes.Kim.fe": b"review notes"}
        for p, b in protected.items():
            p.write_bytes(b)
        output.chmod(0o640)
        call(source, env, "--force", "--jobs=2")
        assert output.stat().st_mode & 0o777 == 0o640
        assert fingerprint(source) == original
        old = fingerprint(output)
        version = subprocess.check_output([str(NATIVE), "--version"], text=True).split()[1]
        fake = root / "fake indexer"
        fake.write_text(f'''#!{sys.executable}
import os, pathlib, signal, sys, time
if sys.argv[1:] == ["--version"]:
    print("floe-index {version}")
    sys.exit(0)
assert sys.argv[1] == "drc", sys.argv
src, dst = pathlib.Path(sys.argv[2]), pathlib.Path(sys.argv[3])
mode = os.environ.get("FLOE_DRC_FAULT", "hang")
dst.write_bytes(b"FLOEICE\\0truncated")
pathlib.Path(str(dst)+".tmpq").write_bytes(b"staging")
pathlib.Path(str(src)+".pid").write_text(str(os.getpid()))
if mode == "fail":
    sys.exit(7)
if mode == "corrupt":
    sys.exit(0)
if mode == "source":
    m = src.stat()
    os.utime(src, ns=(m.st_atime_ns, m.st_mtime_ns+20_000_000_000))
    sys.exit(0)
if mode == "target":
    dst.write_bytes(pathlib.Path(os.environ["FLOE_DRC_GOLDEN"]).read_bytes())
    src.with_name("."+src.name+".tray").write_bytes(b"external writer")
    sys.exit(0)
if mode == "hardhang":
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
print("[drc-pack enc] 1/2 checks, 123 errors, 0.0G blob", file=sys.stderr, flush=True)
while True:
    time.sleep(.01)
''')
        fake.chmod(0o700)
        faulty = dict(env, FLOE_INDEX_BIN=str(fake), FLOE_DRC_GOLDEN=str(golden))
        for mode in ("fail", "corrupt", "source"):
            call(source, dict(faulty, FLOE_DRC_FAULT=mode), "--force", ok=False)
            assert fingerprint(output) == old, mode
            clean_stage(work)
        os.utime(source, ns=(source.stat().st_atime_ns, original[1]))
        # A late uncoordinated output replacement is detected, not overwritten.
        call(source, dict(faulty, FLOE_DRC_FAULT="target"), "--force", ok=False)
        assert output.read_bytes() == b"external writer"
        call(source, env, ok=False)  # unusable existing output requires force
        call(source, env, "--force")
        old = fingerprint(output)
        # Fractional source remains available through the read-only ASCII path.
        source.write_text("TOP 1000\nR\n1 1 0\np 1 2\n1.25 2.5\n3.75 4.5\n")
        failure = call(source, env, "--force", ok=False)
        assert "integer DBU" in failure.stderr, failure.stderr
        assert fingerprint(output) == old
        source.write_bytes(original[0])
        os.utime(source, ns=(source.stat().st_atime_ns, original[1]))
        # Parent signals cancel a confirmed live child, including kill fallback.
        for sig, mode in ((signal.SIGINT, "hang"), (signal.SIGTERM, "hardhang")):
            pid_file = Path(str(source) + ".pid")
            pid_file.unlink(missing_ok=True)
            p = subprocess.Popen([str(CLI), "drc", str(source), "--build", "--force"],
                                 env=dict(faulty, FLOE_DRC_FAULT=mode),
                                 stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            try:
                deadline = time.monotonic() + 10
                while not pid_file.exists():
                    assert p.poll() is None
                    assert time.monotonic() < deadline
                    time.sleep(.005)
                child = int(pid_file.read_text())
                os.kill(child, 0)
                # The persistent OS lock prevents another CLI's replacement.
                call(source, env, "--force", ok=False)
                p.send_signal(sig)
                stdout, stderr = p.communicate(timeout=8)
                assert p.returncode == 128 + sig and not stdout, (p.returncode, stderr)
                try:
                    os.kill(child, 0)
                    raise AssertionError("native child not reaped")
                except ProcessLookupError:
                    pass
            finally:
                if p.poll() is None:
                    p.terminate()
                    p.communicate(timeout=8)
            assert fingerprint(output) == old
            clean_stage(work)
        for p, b in protected.items():
            assert p.read_bytes() == b
        assert source.read_bytes() == original[0]
        clean_stage(work)
        core = root / "core"
        core.mkdir()
        core_source = core / "core.db"
        core_source.write_bytes(original[0])
        core_alias = root / "core parent alias"
        core_alias.symlink_to(core, target_is_directory=True)
        native_env = dict(os.environ, FLOE_INDEX_BIN=str(NATIVE),
                          FLOE_DRC_BUILD_SOURCE=str(core_alias / core_source.name), FLOE_DRC_BUILD_FAKE=str(fake),
                          FLOE_DRC_FAULT="hang")
        subprocess.run(["cargo", "test", "--offline", "--locked", "-p", "floe-app-core",
                        "--test", "drc_build", "--", "--ignored"], cwd=ROOT / "rust",
                       env=native_env, check=True, timeout=90)
    print("DRC BUILD: ALL OK (native bytes j1/4/16, reuse/force, readonly sources/reviews, faults, cancellation/reap, leases)")


if __name__ == "__main__":
    main()
