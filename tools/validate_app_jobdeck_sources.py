#!/usr/bin/env python3
"""Rust jobdeck source/index migration gate; private generated files only."""
import gzip
import hashlib
import json
import os
from pathlib import Path
from cache_test_paths import vfs_cache
import shutil
import signal
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from floe.jobdeck.sources import SourceCatalog, file_header
from validate_jobdeck import DECK, FORMAT_DECK, build_oas, build_thin_oas
from validate_vfs import read_ovm

APP = ROOT / "rust/target/release/floe2-web"
INDEX = ROOT / "rust/target/release/floe-index"
PARTS = ("meta.json", "design.ovm", "design.ovp", "design.ovt")
NAMES = ("chipA.oas", "chipB.oas", "mark.oas")


def run(args, env, code=0, python=False):
    command = [sys.executable, "-B", "-m", "floe2"] if python else [str(APP)]
    p = subprocess.run(command + list(map(str, args)), cwd=ROOT, env=env,
                       text=True, capture_output=True, timeout=60)
    assert p.returncode == code, (args, p.returncode, p.stdout, p.stderr)
    return p


def digest(cache):
    return {p.name: (hashlib.sha256(p.read_bytes()).hexdigest(), p.stat().st_mtime_ns)
            for p in cache.iterdir() if p.is_file()}


def header_gate(work, env):
    work.mkdir()
    build_oas(work / "chipA.oas", 5e-5, 20, 30, [(1, 0)], "TOP")
    build_oas(work / "chipA.gds", 5e-5, 20, 30, [(1, 0)], "TOP")
    for name in ("chipA.oas", "chipA.gds"):
        (work / (name + ".gz")).write_bytes(gzip.compress((work / name).read_bytes()))
    (work / "junk.bin").write_bytes(b"not a layout")
    (work / "empty").write_bytes(b"")
    (work / "truncated.oas").write_bytes(b"%SEMI-OASIS\r\n\x01\x03")
    # First ten continuation groups contain zero; this used to panic the
    # shared Rust uint cursor at a shift >=64 instead of reporting overflow.
    (work / "varint.oas").write_bytes(b"%SEMI-OASIS\r\n" + b"\x80" * 200)
    (work / "badgzip").write_bytes(b"\x1f\x8b\x08\x00")
    os.mkfifo(work / "fifo")
    shutil.copy2(work / "chipA.oas", work / "unselected.oas")
    bad_cache = vfs_cache(work / "unselected.oas")
    bad_cache.mkdir()
    os.mkfifo(bad_cache / "meta.json")
    names = ["chipA.oas", "chipA.gds", "chipA.oas.gz", "chipA.gds.gz",
             "junk.bin", "empty", "truncated.oas", "varint.oas", "badgzip", "absent", "fifo"]
    catalog = SourceCatalog(str(work))
    catalog.probe_all(names)
    headers = []
    for name in names:
        if name in ("fifo", "absent"):
            headers.append({"path": str(work / name), "error": True})
            continue
        try:
            fmt, dbu, version, gzipped = file_header(str(work / name))
            # Python's unlimited int accepts the malformed varint only if
            # terminated; this fixture is truncated and both must fail.
            header = dict(format=fmt, dbu=dbu, version=version, gzipped=gzipped)
            headers.append({"path": str(work / name), "error": False, "header": header})
        except Exception:
            headers.append({"path": str(work / name), "error": True})
    oracle = work / "oracle.json"
    def canonical_report(catalog):
        report = catalog.report()
        # The legacy Cache constructor reports an absent .tiles fallback when
        # no VFS exists yet. Rust names the future canonical destination.
        for row in report["files"]:
            if row["cache_dir"].endswith(".tiles"):
                row["cache_dir"] = str(vfs_cache(row["path"]))
        return report

    payload = {"directory": str(work), "names": names, "headers": headers,
               "catalog": canonical_report(catalog),
               "header_dbus": {name: catalog.header_dbu(name) for name in names},
               "unselected_dbu": file_header(str(work / "unselected.oas"))[1]}
    for indexed in (False, True):
        if indexed:
            run(["index", work / "chipA.oas", "--jobs", "2"], env)
            catalog = SourceCatalog(str(work))
            catalog.probe_all(names)
            payload["catalog"] = canonical_report(catalog)
        oracle.write_text(json.dumps(payload, allow_nan=False))
        subprocess.run(["cargo", "test", "--offline", "--locked", "-p", "floe-app-core",
                        "--test", "jobdeck_sources", "--", "--ignored", "--nocapture"],
                       cwd=ROOT / "rust", env=dict(os.environ, FLOE_APP_SOURCE_ORACLE=str(oracle)),
                       check=True, timeout=90)


def signal_gate(work, env):
    directory = work / "signals"
    directory.mkdir()
    for name in NAMES:
        build_thin_oas(directory / name)
    deck = directory / "batch.jb"
    deck.write_text(DECK)
    fake = directory / "fake-index"
    version = subprocess.check_output([str(INDEX), "--version"], text=True).split()[1]
    fake.write_text(f'''#!{sys.executable}
import json,os,pathlib,signal,sys,time
a=sys.argv[1:]
if a==["--version"]:
    print("floe-index {version}")
    sys.exit(0)
with open(os.environ["CALLS"],"a") as f: f.write(json.dumps(a)+"\\n")
if os.environ.get("WAIT"):
    pathlib.Path(a[2]).mkdir(exist_ok=True)
    pathlib.Path(a[2],"design.ovo.tmp").write_bytes(b"partial")
    def stop(n,_): sys.exit(128+n)
    signal.signal(signal.SIGINT,stop)
    signal.signal(signal.SIGTERM,stop)
    pathlib.Path(os.environ["READY"]).write_text(str(os.getpid()))
    while True: time.sleep(.01)
sys.exit(7 if pathlib.Path(a[1]).name=="chipA.oas" else 0)
''')
    fake.chmod(0o700)
    calls = directory / "calls.jsonl"
    ready = directory / "ready"
    fake_env = dict(env, FLOE_INDEX_BIN=str(fake), CALLS=str(calls), READY=str(ready))
    p = run(["index", deck, "--jobs", "3", "--lod", "--page-target-mb", "2",
             "--slow-cell-s", "10", "--p2-shard-limit-mb", "32"], fake_env, code=2)
    assert "2 built, 1 failed, 0 kept" in p.stdout
    rows = [json.loads(s) for s in calls.read_text().splitlines()]
    assert [Path(a[1]).name for a in rows] == list(NAMES)
    for a in rows:
        assert a[a.index("--jobs") + 1] == "3"
        assert a[a.index("--page-target-mb") + 1] == "2"
        assert "--slow-cell-s" in a and "--p2-shard-limit-mb" in a
        assert "--no-lod" not in a
    for sig in (signal.SIGINT, signal.SIGTERM):
        calls.unlink()
        ready.unlink(missing_ok=True)
        proc = subprocess.Popen([str(APP), "index", str(deck), "--force", "--occupancy", "--jobs", "2"],
                                env=dict(fake_env, WAIT="1"), stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                text=True, start_new_session=True)
        try:
            end = time.monotonic() + 6
            while not ready.exists():
                assert proc.poll() is None, proc.communicate()
                assert time.monotonic() < end
                time.sleep(.01)
            child = int(ready.read_text())
            proc.send_signal(sig)  # parent only, not process group
            out, err = proc.communicate(timeout=5)
            assert proc.returncode == 128 + sig, (out, err)
            assert len(calls.read_text().splitlines()) == 1, "next source started after cancellation"
            try:
                os.kill(child, 0)
            except ProcessLookupError:
                pass
            else:
                raise AssertionError("native child not reaped")
            assert not list(directory.glob(".*.ice/design.ovo.tmp"))
        finally:
            if proc.poll() is None:
                proc.kill()
                proc.communicate()


def index_gate(work, env):
    actual, reference = work / "actual", work / "reference"
    actual.mkdir()
    reference.mkdir()
    for name in NAMES:
        build_thin_oas(actual / name)
        shutil.copy2(actual / name, reference / name)
    decks = [d / "test.jb" for d in (actual, reference)]
    for deck in decks:
        deck.write_text(DECK)

    def cache(directory, name):
        return vfs_cache(directory / name)

    def compare(args):
        for deck, python in zip(decks, (False, True)):
            result = run(["index", deck, "--jobs", "2", *args], env, python=python)
            for line in result.stdout.splitlines():
                if line.startswith("[jobdeck]") and " : " in line and " ok " in line:
                    assert "elapsed, ~" in line and " left)" in line and "(" in line.split(" ok ")[0], line
        for name in NAMES:
            if not cache(actual, name).exists():
                assert not cache(reference, name).exists()
                continue
            for part in PARTS[1:]:
                assert (cache(actual, name) / part).read_bytes() == (cache(reference, name) / part).read_bytes(), (name, part, args)

    compare(["--level", "3", "--lod"])
    assert not cache(actual, "chipA.oas").exists()
    assert not cache(actual, "chipB.oas").exists()
    compare([])
    assert all((cache(actual, name) / "design.ovo").is_file() for name in NAMES)
    for parent, python in zip((actual, reference), (False, True)):
        optout = parent / "no-summary"
        optout.mkdir()
        build_thin_oas(optout / "mark.oas")
        (optout / "test.jb").write_text(DECK)
        run(["index", optout / "test.jb", "--level", "3", "--no-occupancy", "--jobs", "2"], env, python=python)
        assert not (cache(optout, "mark.oas") / "design.ovo").exists(), "deck lost explicit opt-out"
        before_add = digest(cache(optout, "mark.oas"))
        run(["index", optout / "test.jb", "--level", "3", "--jobs", "2"], env, python=python)
        assert (cache(optout, "mark.oas") / "design.ovo").is_file()
        after_add = digest(cache(optout, "mark.oas"))
        assert {k: after_add[k] for k in before_add} == before_add
    before = {name: digest(cache(actual, name)) for name in NAMES}
    compare(["--lod"])
    assert before == {name: digest(cache(actual, name)) for name in NAMES}, "current cache changed"
    compare(["--force", "--lod"])
    assert all(any(p[-1] for p in read_ovm(str(cache(actual, name) / "design.ovm"))["pages"]) for name in NAMES)
    compare(["--occupancy", "--occupancy-um", "4"])
    before = {name: digest(cache(actual, name)) for name in NAMES}
    compare(["--occupancy-only", "--occupancy-um", "8", "--lod"])
    for name in NAMES:
        now = digest(cache(actual, name))
        assert {k: now[k] for k in PARTS} == {k: before[name][k] for k in PARTS}
    before = {name: digest(cache(actual, name)) for name in NAMES}
    run(["index", decks[0], "--level", "999"], env, code=2)
    run(["index", decks[0], "--profile-cell", "TOP"], env, code=2)
    assert before == {name: digest(cache(actual, name)) for name in NAMES}
    # No implicit stale replacement, including when batch occupancy-only wants
    # to build an unindexed source. Other source jobs may still finish normally.
    meta = cache(actual, "chipA.oas") / "meta.json"
    old = meta.read_bytes()
    meta.write_text(json.dumps(dict(json.loads(old), version=7)))
    damaged = digest(cache(actual, "chipA.oas"))
    run(["index", decks[0]], env, code=2)
    assert digest(cache(actual, "chipA.oas")) == damaged
    meta.write_bytes(old)
    empty = work / "occupancy-new"
    empty.mkdir()
    build_thin_oas(empty / "mark.oas")
    (empty / "test.jb").write_text(DECK)
    run(["index", empty / "test.jb", "--level", "3", "--occupancy-only", "--jobs", "2"], env)
    assert (cache(empty, "mark.oas") / "design.ovo").is_file()
    # Lexical source aliases share one destination; source symlinks do not.
    alias = actual / "alias.jb"
    alias.write_text(DECK.replace("TC=mark.oas", "TC=./chipA.oas"))
    result = run(["index", alias, "--force", "--jobs", "2"], env)
    assert "2 built, 0 failed, 0 kept" in result.stdout
    assert "aliases" in result.stdout
    missing = actual / "missing.jb"
    missing.write_text(FORMAT_DECK)
    result = run(["index", missing, "--level", "8"], env)
    assert "MISSING" in result.stdout and "0 built, 0 failed, 0 kept" in result.stdout
    # Indexing does not create a merged cache beside the deck.
    assert not vfs_cache(decks[0]).exists()


def main():
    with tempfile.TemporaryDirectory(prefix="floe-app-deck sources 한 글 ") as td:
        work = Path(td)
        env = {k: v for k, v in os.environ.items() if not k.startswith("FLOE_")}
        env.update(FLOE_INDEX_BIN=str(INDEX), PATH="", PYTHONPATH=str(ROOT), PYTHONDONTWRITEBYTECODE="1")
        header_gate(work / "headers", env)
        index_gate(work, env)
        signal_gate(work, env)
        print("RUST APP JOBDECK INDEX: ALL OK")


if __name__ == "__main__":
    main()
