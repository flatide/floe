#!/usr/bin/env python3
"""Explicit-index rename acceptance. Private synthetic files only."""
import hashlib
import fcntl
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time

from cache_test_paths import vfs_cache, drc_pack
from validate_drc_ice import DB

ROOT = Path(__file__).resolve().parents[1]
APP = ROOT / "rust/target/release/floe2-web"
INDEX = ROOT / "rust/target/release/floe-index"


def stamp(path):
    m = path.stat()
    return (hashlib.sha256(path.read_bytes()).hexdigest(), m.st_ino,
            m.st_mtime_ns, m.st_mode, m.st_size)


def cache_stamp(path):
    return {p.relative_to(path).as_posix(): stamp(p)
            for p in path.rglob("*") if p.is_file()}


def writer_lock(path):
    return open(str(path) + ".index.lock", "a+b")


def assert_writer_locked(path):
    with writer_lock(path) as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return
        raise AssertionError(f"writer alias is not locked: {path}")


def call(env, *args, ok=True):
    p = subprocess.run([str(APP), *map(str, args)], env=env, capture_output=True,
                       text=True, timeout=40)
    assert (p.returncode == 0) == ok, (args, p.returncode, p.stdout, p.stderr)
    return p


def main(fixture):
    with tempfile.TemporaryDirectory(prefix="floe-cache-migration-") as td:
        root = Path(td).resolve()
        source = root / "설계 with spaces.oas"
        shutil.copy2(fixture, source)
        env = dict(os.environ, PATH="", FLOE_INDEX_BIN=str(INDEX),
                   FLOE_RENDERD_BIN=str(ROOT / "rust/target/release/floe-renderd"))
        call(env, "index", source, "--jobs", "2")
        new = vfs_cache(source)
        old = Path(str(source) + ".floe")
        new.rename(old)
        before, ino = cache_stamp(old), old.stat().st_ino
        siblings = sorted(p.name for p in root.iterdir())
        call(env, "info", source, "--json")
        call(env, "probe", source)
        meta = json.loads((old / "meta.json").read_text())
        profiled = call(env, "index", source, "--profile-cell", meta["top_cell"], "--jobs", "2")
        assert json.loads(profiled.stdout)["settings"]["writes"] is False
        assert not new.exists() and cache_stamp(old) == before
        assert sorted(p.name for p in root.iterdir()) == siblings, "read/profile created files"
        for alias in (new, old):
            with writer_lock(alias) as lock:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                busy = call(env, "index", source, ok=False)
                assert "another Rust application" in busy.stderr
                assert old.exists() and not new.exists() and cache_stamp(old) == before
        # Existing lock files are writable, but the name's parent is not.
        # Root can bypass mode bits, so this is not a root/ACL permission oracle.
        if os.geteuid() != 0:
            root.chmod(0o500)
            try:
                call(env, "info", source)
                call(env, "index", source, ok=False)
                assert old.exists() and not new.exists() and cache_stamp(old) == before
            finally:
                root.chmod(0o700)
        indexed = call(env, "index", source, "--jobs", "2")
        assert "renamed legacy cache" in indexed.stderr
        assert new.stat().st_ino == ino and not old.exists()
        assert cache_stamp(new) == before, "migration rewrote cache bytes/inodes/mtimes"

        # New always wins; a legacy sibling is never deleted or silently used
        # to hide corruption in the preferred destination.
        old.mkdir()
        sentinel = old / "untouched"
        sentinel.write_bytes(b"legacy sibling is not a cleanup target")
        kept = stamp(sentinel)
        call(env, "index", source)
        assert stamp(sentinel) == kept
        marker = new / "design.ovm"
        original = marker.read_bytes()
        marker.write_bytes(b"invalid")
        call(env, "index", source, ok=False)
        assert stamp(sentinel) == kept and marker.read_bytes() == b"invalid"
        marker.write_bytes(original)
        sentinel.unlink()
        old.rmdir()

        # A selected, already indexed legacy deck source must enter the write
        # plan for its name change, even when no geometry/summary is needed.
        new.rename(old)
        before = cache_stamp(old)
        deck = root / "migration.jb"
        deck.write_text(f"CHIP C\n$ (1,A,TC='{source.name}',AD=0.001,LY={{1}},DT={{0}},UX=100,UY=100)\nROWS 0/0\n")
        call(env, "info", deck)
        assert old.exists() and not new.exists() and cache_stamp(old) == before
        call(env, "index", deck, "--no-occupancy", "--jobs", "2")
        assert not old.exists() and cache_stamp(new) == before

        # Rename commits independently of a later rebuild failure/cancellation.
        version = subprocess.check_output([str(INDEX), "--version"], text=True).split()[1]
        fake = root / "fake-indexer"
        ready = root / "fake-ready"
        fake.write_text(f'''#!{sys.executable}
import os, pathlib, signal, sys, time
if sys.argv[1:] == ["--version"]:
    print("floe-index {version}")
    sys.exit(0)
if os.environ.get("MIGRATION_FAULT") == "fail":
    sys.exit(7)
signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
pathlib.Path({str(ready)!r}).write_text("ready")
while True:
    time.sleep(.01)
''')
        fake.chmod(0o700)
        new.rename(old)
        before = cache_stamp(old)
        failed = call(dict(env, FLOE_INDEX_BIN=str(fake), MIGRATION_FAULT="fail"),
                      "index", source, "--force", "--jobs", "2", ok=False)
        assert "renamed legacy cache" in failed.stderr
        assert not old.exists() and cache_stamp(new) == before
        new.rename(old)
        proc = subprocess.Popen([str(APP), "index", str(source), "--force", "--jobs", "2"],
                                env=dict(env, FLOE_INDEX_BIN=str(fake), MIGRATION_FAULT="hang"),
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            deadline = time.monotonic() + 30
            while not ready.exists():
                assert proc.poll() is None, proc.communicate()
                assert time.monotonic() < deadline, "fake indexer did not become ready"
                time.sleep(.01)
            for alias in (new, old):
                assert_writer_locked(alias)
            proc.send_signal(signal.SIGTERM)
            stdout, stderr = proc.communicate(timeout=10)
            assert proc.returncode == 143, (proc.returncode, stdout, stderr)
            assert "renamed legacy cache" in stderr
            assert not old.exists() and cache_stamp(new) == before
        finally:
            if proc.poll() is None:
                proc.kill()
                proc.communicate(timeout=10)
        for alias in (new, old):
            with writer_lock(alias) as lock:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)

        # DRC reads keep the legacy spelling; only explicit pack building may
        # rename. Review files use the logical DB name and never move.
        db = root / "results.db"
        db.write_text(DB)
        call(env, "drc", db, "--build", "--jobs", "2")
        pack = drc_pack(db)
        legacy = Path(str(db) + ".ice")
        pack.rename(legacy)
        before = stamp(legacy)
        notes = root / ".results.db.notes.Kim.fe"
        waives = root / ".results.db.waive.Kim"
        notes.write_bytes(b"sentinel notes")
        waives.write_bytes(b"sentinel waives")
        sidecars = (stamp(notes), stamp(waives))
        call(env, "drc", db)
        assert not pack.exists() and stamp(legacy) == before
        built = json.loads(call(env, "drc", db, "--build", "--jobs", "2").stdout)
        assert built["reused"] and built["migration"] is not None
        assert not legacy.exists() and stamp(pack) == before
        assert (stamp(notes), stamp(waives)) == sidecars
        assert json.loads(call(env, "drc", db, "--build").stdout)["migration"] is None
        print("CACHE MIGRATION: ALL OK (read/profile nonwriting, layout/deck explicit rename, both writer aliases, collision/corruption, failed/cancelled rebuild receipts, DRC reuse + unchanged review sidecars)")


if __name__ == "__main__":
    main(Path(sys.argv[1]).resolve())
