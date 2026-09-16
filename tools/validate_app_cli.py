#!/usr/bin/env python3
"""M1a-2 layout-index gate. Python is a test/oracle, never the new runtime.

Usage: python tools/validate_app_cli.py SYNTHETIC_VALMINI.oas
Uses only private copies; do not feed a real-chip milestone source here.
"""
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
APP = ROOT / "rust/target/release/floe2-web"
INDEX = ROOT / "rust/target/release/floe-index"
PYTHON = Path(sys.executable)
PARTS = ("design.ovm", "design.ovp", "design.ovt")


def run(*args, env=None, code=0, python=False):
    command = ([str(PYTHON), "-B", "-m", "floe2"] if python else [str(APP)])
    result = subprocess.run(command + list(map(str, args)), cwd=ROOT,
                            env=env, capture_output=True, text=True, timeout=40)
    assert result.returncode == code, (command, args, result.returncode,
                                       result.stdout, result.stderr)
    return result


def digest(directory, names=None):
    return {p.name: (hashlib.sha256(p.read_bytes()).hexdigest(), p.stat().st_mtime_ns)
            for p in directory.iterdir() if p.is_file() and
            (names is None or p.name in names)}


def wait_file(path, process):
    end = time.monotonic() + 5
    while True:
        try:
            text = path.read_text()
        except FileNotFoundError:
            text = ""
        # Creation is not publication: write_text creates an empty file first.
        # Return the complete record we read, not a second racing read.
        if text.endswith("\n"):
            assert text[:-1].isascii() and text[:-1].isdigit() and int(text) > 0
            return int(text)
        assert process.poll() is None, process.communicate()
        assert time.monotonic() < end, "fake worker did not become ready"
        time.sleep(0.01)


def main(fixture):
    assert APP.is_file() and INDEX.is_file() and fixture.is_file()
    version = subprocess.check_output([str(INDEX), "--version"], text=True).split()[1]
    with tempfile.TemporaryDirectory(prefix="floe-app-cli-") as td:
        work = Path(td)
        source = work / "설계 with spaces.oas"
        oracle = work / "oracle.oas"
        shutil.copy2(fixture, source)
        shutil.copy2(fixture, oracle)
        cache = Path(str(source) + ".floe")
        # No Python/shell can be discovered by the Rust runtime under this PATH.
        env = dict(os.environ, FLOE_INDEX_BIN=str(INDEX), PATH="",
                   PYTHONPATH=str(ROOT), PYTHONDONTWRITEBYTECODE="1")
        assert "Rust application migration CLI" in run("--help", env=env).stdout
        assert "selfcheck" in run("--help", env=env).stdout
        assert version in run("--version", env=env).stdout
        assert "--no-open" in run("view", "--help", env=env).stdout
        assert "--stream-kb N" in run("view", "--help", env=env).stdout
        assert "not a byte budget" in run("view", "--stream-kb", "1", "--help", env=env).stdout
        assert "never writes server /tmp PNGs" in run("view", "--help", env=env).stdout
        for args, text in [
            (("--stream-kb", "-1"), "must be >= 0"),
            (("--stream-kb", "NaN"), "requires a decimal integer"),
            (("--stream-kb", "1", "--refinement", "off"), "conflicts with nonzero"),
            (("--perf-baseline", "--stream-kb", "1"), "conflicts with nonzero"),
            (("--stream-target-ms", "500"), "unused by Rust renderd"),
            (("--hairline", "0"), "legacy KLayout planner"),
            (("--thin-um", "0"), "not an equivalent frame control"),
            (("--lod", "off"), "not sent to Rust renderd"),
            (("--dump=false",), "--dump takes no value"),
            (("--floe-reviewer", "tag"), "require --drc"),
        ]:
            assert text in run("view", source, *args, env=env, code=2).stderr
        run("view", env=env, code=2)
        assert "--bbox" in run("clip", "--help", env=env).stdout
        for tail in ((), ("--help",), (str(work / "missing.png"),)):
            rejected = run("gtktest", *tail, env=env, code=2)
            assert "replaced by displaytest [PNG]" in rejected.stderr
            assert "no alias" in rejected.stderr and "Run display test" in rejected.stderr
            assert "not yet ported" not in rejected.stderr and not rejected.stdout
        assert "--follow-verbatim" in run("svrf", "--help", env=env).stdout
        assert "requires DECK" in run("svrf", env=env, code=2).stderr
        deck = work / "rules.svrf"
        deck.write_text("LAYER M 7\nR { INT M < .05 }\n")
        assert "checks 1" in run("svrf", deck, "--scan", env=env).stdout
        assert not Path(str(deck) + ".rules.json").exists()
        run("svrf", deck, env=env)
        rules = json.loads(Path(str(deck) + ".rules.json").read_text())
        assert rules["checks"]["R"]["source_gds"] == [[7, None]]
        for option in ("--coverage", "--legacy", "--tile-mb", "--level"):
            run("index", source, option, env=env, code=2)
        for option in ("--jobs", "--page-target-mb", "--profile-repeat"):
            run("index", source, option, "0", env=env, code=2)
        for value in ("nan", "inf", "-1"):
            run("index", source, "--occupancy-um", value, env=env, code=2)
        assert not cache.exists()

        run("index", source, "--jobs", "2", env=env)
        run("index", oracle, "--jobs", "2", env=env, python=True)
        oracle_cache = Path(str(oracle) + ".floe")
        assert (cache / "design.ovo").is_file(), "default build omitted summary"
        assert (cache / "design.ovo").read_bytes() == (oracle_cache / "design.ovo").read_bytes()
        for part in PARTS:
            assert (cache / part).read_bytes() == (oracle_cache / part).read_bytes(), part
        before = digest(cache)
        assert "up to date" in run("index", source, "--lod", env=env).stdout
        assert digest(cache) == before, "reuse changed data or mtime"

        # Actual marker and JSON corruption, never just a fake nonzero marker.
        marker = (cache / "design.ovm").read_bytes()
        (cache / "design.ovm").write_bytes(b"x")
        damaged = digest(cache)
        assert "refusing" in run("index", source, env=env, code=1).stderr
        assert digest(cache) == damaged
        (cache / "design.ovm").write_bytes(marker)
        meta_path = cache / "meta.json"
        meta = json.loads(meta_path.read_text())
        for change in ({"version": 7}, {"src": dict(meta["src"], size=0)}):
            meta_path.write_text(json.dumps(dict(meta, **change)))
            bad = digest(cache)
            run("index", source, env=env, code=1)
            assert digest(cache) == bad
        meta_path.write_text(json.dumps(meta))
        # A FIFO must fail without hanging on read/open.
        meta_bytes = meta_path.read_bytes()
        meta_path.unlink()
        os.mkfifo(meta_path)
        run("index", source, env=env, code=1)
        meta_path.unlink()
        meta_path.write_bytes(meta_bytes)
        stat = source.stat()
        os.utime(source, ns=(stat.st_atime_ns, stat.st_mtime_ns + 2_000_000_000))
        run("index", source, env=env, code=1)
        os.utime(source, ns=(stat.st_atime_ns, stat.st_mtime_ns))
        run("index", source, "--force", "--lod", "--jobs", "2", env=env)
        assert (cache / "design.ovm").read_bytes() != marker, "LOD opt-in ignored"
        run("index", oracle, "--force", "--lod", "--jobs", "2", env=env, python=True)
        for part in PARTS:
            assert (cache / part).read_bytes() == (oracle_cache / part).read_bytes(), part
        print("app CLI: real cache bytes, reuse, force, corruption and LOD parity ok")

        older = work / "older.oas"
        shutil.copy2(fixture, older)
        older_cache = Path(str(older) + ".floe")
        run("index", older, "--no-occupancy", "--jobs", "2", env=env)
        assert not (older_cache / "design.ovo").exists()
        legacy = digest(older_cache)
        run("index", older, "--no-occupancy", env=env)
        assert digest(older_cache) == legacy
        run("index", older, "--jobs", "2", env=env)
        assert (older_cache / "design.ovo").is_file()
        assert digest(older_cache, legacy) == legacy, "default summary replaced legacy cache"
        summary_kept = digest(older_cache)
        run("index", older, "--no-occupancy", env=env)
        assert digest(older_cache) == summary_kept, "opt-out removed existing summary"
        before = digest(cache, (*PARTS, "meta.json"))
        run("index", source, "--occupancy", "--occupancy-um", "4", "--jobs", "2", env=env)
        assert digest(cache, before) == before, "additive summary changed base cache"
        summary = (cache / "design.ovo").read_bytes()
        kept = digest(cache)
        assert "already present" in run("index", source, "--occupancy-um", "2", env=env).stdout
        assert digest(cache) == kept
        run("index", source, "--occupancy-only", "--occupancy-um", "2", "--no-lod", "--jobs", "2", env=env)
        assert digest(cache, before) == before
        assert (cache / "design.ovo").read_bytes() != summary
        print("app CLI: occupancy add/rebuild preserves OVM/OVP/OVT and metadata ok")

        before = digest(cache)
        top = meta["top_cell"]
        snapshot = work / "profile 한 글.bin"
        profile_args = ("index", source, "--profile-cell", top, "--jobs", "2")
        series = json.loads(run(*profile_args, "--profile-jobs", "1,2", "--profile-repeat", "2",
                                "--profile-snapshot", snapshot, env=env).stdout)
        assert [r["settings"]["jobs"] for r in series] == [1, 1, 2, 2]
        assert all(r["settings"]["writes"] is False for r in series)
        loaded = json.loads(run(*profile_args, "--profile-snapshot", snapshot, env=env).stdout)
        assert loaded["snapshot"]["state"] == "loaded"
        assert digest(cache) == before
        run(*profile_args, "--profile-snapshot", cache / "design.ovm",
            "--profile-snapshot-refresh", env=env, code=2)
        assert digest(cache) == before
        fresh_profile = work / "profile_only.oas"
        shutil.copy2(fixture, fresh_profile)
        run("index", fresh_profile, "--profile-cell", top, "--jobs", "2", env=env)
        assert not Path(str(fresh_profile) + ".floe").exists()
        assert not Path(str(fresh_profile) + ".floe.index.lock").exists()
        print("app CLI: profile JSON, repeated jobs, snapshot reuse and cache non-mutation ok")

        missing_env = dict(env, FLOE_INDEX_BIN=str(work / "missing"))
        assert "FLOE_INDEX_BIN" in run("index", source, env=missing_env, code=2).stderr
        link_source = work / "linked.oas"
        link_source.symlink_to(source)
        linked_cache = Path(str(link_source) + ".floe")
        linked_cache.symlink_to(cache, target_is_directory=True)
        run("index", link_source, "--force", env=env, code=2)
        assert digest(cache) == before
        linked_cache.unlink()
        run("index", link_source, "--jobs", "2", env=env)
        assert linked_cache.is_dir() and not linked_cache.is_symlink()
        assert digest(cache) == before, "source alias selected target cache instead"

        # Controlled child for argv/errors/cancellation. Its Python interpreter
        # is explicit and exists only in this development test, not the app.
        fake = work / "fake indexer"
        log = work / "calls.jsonl"
        ready = work / "fake.ready"
        stopped = work / "signal.txt"
        fake.write_text("#!" + str(PYTHON) + "\n" + '''
import json, os, pathlib, signal, sys, time
a = sys.argv[1:]
if a == ["--version"]:
    print("floe-index " + os.environ["FAKE_VERSION"] + " (test)")
    sys.exit(0)
with open(os.environ["FAKE_LOG"], "a") as f:
    f.write(json.dumps(a) + "\\n")
if os.environ.get("FAKE_CLEANUP_ERROR"):
    (pathlib.Path(a[2]) / "design.ovo.tmp").mkdir()
    sys.exit(7)
if os.environ.get("FAKE_WAIT"):
    out = pathlib.Path(a[2])
    (out / "design.ovo.tmp").write_bytes(b"partial")
    def interrupted(n, _):
        pathlib.Path(os.environ["FAKE_STOPPED"]).write_text(str(n))
        sys.exit(128 + n)
    signal.signal(signal.SIGINT, interrupted)
    signal.signal(signal.SIGTERM, interrupted)
    print("heartbeat", file=sys.stderr, flush=True)
    ready = pathlib.Path(os.environ["FAKE_READY"])
    ready.write_text("")
    time.sleep(0.02)  # Reproduce the created-but-not-yet-published interval.
    ready.write_text(str(os.getpid()) + "\\n")
    while True: time.sleep(0.01)
sys.exit(int(os.environ.get("FAKE_EXIT", "0")))
''')
        fake.chmod(0o700)
        fake_env = dict(env, FLOE_INDEX_BIN=str(fake), FAKE_VERSION=version,
                        FAKE_LOG=str(log), FAKE_READY=str(ready), FAKE_STOPPED=str(stopped))
        run("index", source, env=dict(fake_env, FAKE_VERSION="0.0.0"), code=1)
        assert not log.exists(), "mismatched indexer executed a command"
        fake_source = work / "argument test.oas"
        fake_source.write_bytes(b"only fake indexer reads this")
        run("index", fake_source, "--jobs", "16", "--page-target-mb", "2", "--slow-cell-s", "0",
            "--p2-shard-limit-mb", "1024", "--occupancy-um", "4", env=dict(fake_env, FAKE_EXIT="7"), code=7)
        call = json.loads(log.read_text().splitlines()[-1])
        assert call[:3] == ["vfs", str(fake_source), str(fake_source) + ".floe"]
        assert call[3:] == ["--jobs", "16", "--page-target-mb", "2", "--occupancy", "--occupancy-um", "4",
                            "--no-lod", "--slow-cell-s", "0", "--p2-shard-limit-mb", "1024"]
        bad_cleanup = run("index", source, "--occupancy-only",
                          env=dict(fake_env, FAKE_CLEANUP_ERROR="1"), code=7)
        assert "cannot clean" in bad_cleanup.stderr
        (cache / "design.ovo.tmp").rmdir()
        for sig in (signal.SIGINT, signal.SIGTERM):
            ready.unlink(missing_ok=True)
            stopped.unlink(missing_ok=True)
            before_cancel = digest(cache)
            p = subprocess.Popen([str(APP), "index", str(source), "--occupancy-only"],
                                 env=dict(fake_env, FAKE_WAIT="1"), stdout=subprocess.PIPE,
                                 stderr=subprocess.PIPE, text=True, start_new_session=True)
            try:
                child_pid = wait_file(ready, p)
                assert os.getpgid(child_pid) != os.getpgid(p.pid), "child SIGINT not isolated"
                busy = run("index", source, "--force", env=fake_env, code=1)
                assert "another Rust application" in busy.stderr
                p.send_signal(sig)
                out, err = p.communicate(timeout=5)
                assert p.returncode == 128 + sig, (out, err, p.returncode)
                assert "heartbeat" in err and "discarded" in err
                assert int(stopped.read_text()) == sig, "signal did not reach the owned child"
            finally:
                if p.poll() is None:
                    p.send_signal(signal.SIGTERM)
                    p.communicate(timeout=5)
            assert digest(cache) == before_cancel, "cancel changed committed summary/cache"
            assert not (cache / "design.ovo.tmp").exists()
        assert "up to date" in run("index", source, env=env).stdout, "lease leaked after cancel"
        print("app CLI: override/version, argv, exit status, writer lock and SIGINT/SIGTERM cleanup ok")
    print("RUST APP CLI: ALL OK")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit(__doc__)
    main(Path(sys.argv[1]).resolve())
