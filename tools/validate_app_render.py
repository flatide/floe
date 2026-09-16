#!/usr/bin/env python3
"""Rust application read-path oracle; synthetic private inputs only.

Python/KLayout are development oracles. floe2-web always runs with PATH empty.
"""
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
from floe.cache import Cache

APP = ROOT / "rust/target/release/floe2-web"
INDEX = ROOT / "rust/target/release/floe-index"
RENDERD = ROOT / "rust/target/release/floe-renderd"


def run(args, env, code=0, python=False):
    cmd = ([sys.executable, "-B", "-m", "floe2"] if python else [str(APP)])
    p = subprocess.run(cmd + list(map(str, args)), cwd=ROOT, env=env,
                       text=True, capture_output=True, timeout=50)
    assert p.returncode == code, (cmd, args, p.returncode, p.stdout, p.stderr)
    return p


def digest(cache):
    return {p.name: (hashlib.sha256(p.read_bytes()).hexdigest(), p.stat().st_mtime_ns)
            for p in cache.iterdir() if p.is_file()}


def compare(source, work, env, tag, args, code=0, python_env=None):
    out = work / (tag + " rust.png")
    ref = work / (tag + " python.png")
    report = work / (tag + " rust.json")
    ref_report = work / (tag + " python.json")
    run(["render", source, *args, "--out", ref, "--report", ref_report],
        env if python_env is None else python_env, code=code, python=True)
    run(["render", source, *args, "--out", out, "--report", report], env, code=code)
    if out.read_bytes() != ref.read_bytes():
        from PIL import Image, ImageChops
        import collections
        a, b = Image.open(out).convert("RGB"), Image.open(ref).convert("RGB")
        raise AssertionError(("PNG bytes: " + tag, a.size, b.size,
            ImageChops.difference(a, b).getbbox(),
            collections.Counter(a.getdata()).most_common(8),
            collections.Counter(b.getdata()).most_common(8),
            json.loads(report.read_text()), json.loads(ref_report.read_text())))
    actual, expected = json.loads(report.read_text()), json.loads(ref_report.read_text())
    for doc in (actual, expected):
        for row in doc["shots"]:
            for key in ("ms", "out", "name"):
                row.pop(key)
    assert actual == expected, (tag, actual, expected)
    return out


def wait_marker(marker, proc):
    until = time.monotonic() + 8
    while not marker.exists():
        assert proc.poll() is None, proc.communicate()
        assert time.monotonic() < until, "fake renderd did not enter phase"
        time.sleep(0.01)


def fake_worker_tests(source, work, env, png, unit, deck=False):
    version = subprocess.check_output([str(RENDERD), "--version"], text=True).split()[1]
    fake = work / "fake-renderd"
    fake.write_text(f'''#!{sys.executable}
import os, pathlib, sys, time
mode = os.environ["FAKE_MODE"]
def ready(phase):
    if mode == phase:
        pathlib.Path(os.environ["FAKE_MARKER"]).write_text(str(os.getpid()))
        time.sleep(60)
ready("ready")
print("ready version={version}", flush=True)
for line in sys.stdin:
    parts = line.split()
    if not parts: continue
    kind = parts[0]
    d = dict(p.split("=", 1) for p in parts[1:])
    ready(kind)
    if kind == "open": print("opened unit=" + str({unit} * (2 if mode == "unit" else 1)) + " max_depth=6", flush=True)
    elif kind == "style": print("styled epoch=" + d["epoch"], flush=True)
    elif kind == "render":
        if mode == "failure":
            print("error gen=" + d["gen"] + " code=render message=ENOSPC", flush=True)
            continue
        pathlib.Path(d["out"]).write_bytes(pathlib.Path(os.environ["FAKE_PNG"]).read_bytes())
        scene = ("scene_gen=0 scene_round=0 scene_complete=0 scene_summary=0" if {deck!r}
                 else "scene_gen=" + d["gen"] + " scene_round=1 scene_complete=" + ("0" if mode in ("partial", "deferred") else "1") + " scene_summary=0")
        print("frame gen=" + d["gen"] + " round=1 final=1 partial=" + ("1" if mode in ("partial", "deferred") else "0") + " deferred=" + ("2" if mode == "deferred" else "0") + " labels_truncated=" + ("1" if mode == "labels" else "0") + " style_epoch=" + d["style_epoch"] + " format=png png=" + d["out"] + " " + scene, flush=True)
    elif kind == "quit": break
''')
    fake.chmod(0o700)
    target = work / "protected.png"
    marker = work / "fake-phase"
    fake_env = dict(env, FLOE_RENDERD_BIN=str(fake), FAKE_MARKER=str(marker), FAKE_PNG=str(png))
    args = ["render", source, "--px", "128x128", "--out", target]
    for mode, code in [("failure", 1), ("unit", 1), ("partial", 3), ("labels", 3)]:
        target.write_bytes(b"keep previous successful export")
        result = run(args, dict(fake_env, FAKE_MODE=mode), code=code)
        assert target.read_bytes() == b"keep previous successful export"
        expected = {"failure": "ENOSPC", "unit": "units differ"}.get(mode, "incomplete")
        assert expected in result.stderr
    if deck:
        report = work / "fake-partial.json"
        result = run([*args, "--report", report], dict(fake_env, FAKE_MODE="deferred"), code=3)
        assert target.read_bytes() == png.read_bytes()
        doc = json.loads(report.read_text())
        assert not doc["complete"] and not doc["jobdeck"]["complete"]
        assert doc["jobdeck"]["over_budget_pages"] == 2
        assert "INCOMPLETE" in result.stderr
        target.write_bytes(b"keep previous successful export")
    for phase in ("ready", "open", "style", "render"):
        for sig in (signal.SIGINT, signal.SIGTERM):
            marker.unlink(missing_ok=True)
            proc = subprocess.Popen([str(APP), *map(str, args)], cwd=ROOT,
                                    env=dict(fake_env, FAKE_MODE=phase),
                                    stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            try:
                wait_marker(marker, proc)
                child_pid = int(marker.read_text())
                started = time.monotonic()
                proc.send_signal(sig)  # parent only, NOT renderd's process group
                stdout, stderr = proc.communicate(timeout=8)
                assert proc.returncode == 128 + sig, (phase, stdout, stderr)
                assert time.monotonic() - started < 5
                assert target.read_bytes() == b"keep previous successful export"
                try:
                    os.kill(child_pid, 0)
                except ProcessLookupError:
                    pass
                else:
                    raise AssertionError("renderd not reaped")
            finally:
                if proc.poll() is None:
                    proc.kill()
                    proc.communicate()
            assert not list(Path(env["TMPDIR"]).iterdir()), ("private frame workspace leaked", list(Path(env["TMPDIR"]).iterdir()))


def main(fixture):
    with tempfile.TemporaryDirectory(prefix="floe-app-read-") as td:
        work = Path(td)
        source = work / "테스트 with spaces.oas"
        shutil.copy2(fixture, source)
        cache = vfs_cache(source)
        temp = work / "worker-temp"
        temp.mkdir()
        env = {k: v for k, v in os.environ.items() if not k.startswith("FLOE_")}
        env.update(FLOE_INDEX_BIN=str(INDEX), FLOE_RENDERD_BIN=str(RENDERD),
                   PATH="", PYTHONPATH=str(ROOT), PYTHONDONTWRITEBYTECODE="1",
                   TMPDIR=str(temp))
        for command in ("info", "render", "probe"):
            assert "Usage:" in run([command, "--help"], env).stdout
            run([command], env, code=2)
        run(["index", source, "--jobs", "2"], env)
        before = digest(cache)
        actual = run(["info", source], env).stdout
        expected = run(["info", source], env, python=True).stdout
        assert actual == expected, (actual, expected)
        meta = json.loads((cache / "meta.json").read_text())
        info = json.loads(run(["info", source, "--json"], env).stdout)
        assert info["source_stale"] is False
        c = Cache(str(source))
        c.load()
        for key in ("src", "dbu", "top_cell", "bbox", "grid", "layers"):
            assert info["metadata"][key] == c.meta[key], key
        bb = [n * meta["dbu"] for n in meta["bbox"]]
        box = ",".join(map(str, bb))
        default_png = compare(source, work, env, "fit", ["--px", "128x128"])
        compare(source, work, env, "all", ["--px", "128x128", "--layers", "all"])
        compare(source, work, env, "width", ["--px", "123", "--bbox", box])
        compare(source, work, env, "units", ["--px", "97x81", "--at", "5um,3000nm", "--size", "12µm,9μm", "--anchor", "lb"])
        compare(source, work, env, "stretch", ["--px", "101x67", "--bbox", "12,10,-2,-4", "--stretch"])
        compare(source, work, env, "half", ["--px", "64x64", "--bbox", "-10.9375,-10.9375,89.0625,89.0625"])
        for detail, depth in (("high", "0"), ("medium", "1"), ("low", "999")):
            compare(source, work, env, detail, ["--px", "128x128", "--detail", detail, "--depth", depth, "--thin", "keep", "--frames", "--labels", "--label-font-px", "18"])
        first = meta["layers"][0]
        key = f'{first["layer"]}/{first["datatype"]}'
        compare(source, work, env, "selected", ["--px", "96x96", "--layers", key + "," + key])
        # A legitimate empty plan must publish a complete blank image. Turning
        # design layers off must not hide the independent structural frontier.
        from PIL import Image
        blank = compare(source, work, env, "none", ["--px", "96x96", "--layers", ","])
        assert Image.open(blank).convert("RGB").getbbox() is None
        framed = compare(source, work, env, "none-frames", ["--px", "96x96", "--layers", ",", "--depth", "0", "--frames"])
        assert Image.open(framed).convert("RGB").getbbox() is not None
        outside = compare(source, work, env, "outside", ["--px", "96x96", "--bbox", "1000000,1000000,1000100,1000100"])
        assert Image.open(outside).convert("RGB").getbbox() is None
        # Design colour wins over old cached colour, but archival fill/width
        # remain solid/1 regardless of live personalization.
        props = Path(str(source) + ".layerprops")
        props.write_text(f'{first["layer"]}.{first["datatype"]} skyblue clear CUSTOM 0 8\n')
        compare(source, work, env, "properties", ["--px", "128x128"])
        props.unlink()
        # Names may alias more than one datatype; selection order is stable.
        named = json.loads(json.dumps(meta))
        for layer in named["layers"][:2]:
            layer["aliases"] = ["SHARED 이름"]
        (cache / "meta.json").write_text(json.dumps(named))
        compare(source, work, env, "alias", ["--px", "96x96", "--layers", "SHARED 이름"])
        (cache / "meta.json").write_text(json.dumps(meta))
        before = digest(cache)
        result = run(["probe", source], env)
        assert result.stdout.count("frame OK") == 2 and "[probe] OK" in result.stdout
        # A real cold multiround request: final-only export must match baseline.
        compare(source, work, dict(env, FLOE_RUST_ROUND_PAGES="1"), "rounds", ["--px", "128x128"])
        assert digest(cache) == before
        for args in [["--bbox", "nan,0,1,1"], ["--px", "999999x999999"],
                     ["--bbox", "1,1,1,2"], ["--label-font-px", "100"],
                     ["--layers", "UNKNOWN_LAYER"], ["--frames=true"], ["--mosaic-at", "1,2"]]:
            run(["render", source, *args, "--out", work / "invalid.png"], env, code=2)
            assert not (work / "invalid.png").exists()
        for path in (source, cache / "design.ovm"):
            run(["render", source, "--out", path], env, code=2)
        alias = work / "cache-alias"
        alias.symlink_to(cache, target_is_directory=True)
        run(["render", source, "--out", alias / "design.ovm"], env, code=2)
        run(["render", source, "--out", default_png, "--report", default_png], env, code=2)
        # Stale source is warning-only for a read, unlike destructive index
        # reuse. Mixed-version/marker corruption is not warning-only.
        os.utime(source, (source.stat().st_atime, source.stat().st_mtime + 2))
        stale = run(["info", source, "--json"], env)
        assert json.loads(stale.stdout)["source_stale"] and "source changed" in stale.stderr
        os.utime(source, (source.stat().st_atime, meta["src"]["mtime"]))
        marker = cache / "design.ovm"
        old = marker.read_bytes()
        marker.write_bytes(b"x")
        run(["render", source, "--out", default_png], env, code=1)
        marker.write_bytes(old)
        bad = dict(meta, dbu=0)
        (cache / "meta.json").write_text(json.dumps(bad))
        run(["info", source], env, code=1)
        (cache / "meta.json").write_text(json.dumps(meta))
        for value in ("", "/no/renderd"):
            run(["render", source], dict(env, FLOE_RENDERD_BIN=value), code=2)
        fake_worker_tests(source, work, env, default_png, 1 / meta["dbu"])
        assert not list(temp.iterdir()), "private worker files leaked"
        assert not list(work.glob(".floe-shot-*")), "staged export leaked"
    print("RUST APP READ: ALL OK")


if __name__ == "__main__":
    main(Path(sys.argv[1]))
