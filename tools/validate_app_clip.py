#!/usr/bin/env python3
"""Rust exact clip CLI: private synthetic inputs, byte and KLayout XOR oracles.

Python is a development oracle/fake worker only; the Rust CLI runs with PATH
empty and explicit native binary paths. No proprietary source is copied here.
"""
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time

import klayout.db as db

from validate_app_render import APP, INDEX, RENDERD, ROOT, digest, run, wait_marker


def regions(layout):
    return {(layout.get_info(li).layer, layout.get_info(li).datatype):
            db.Region(layout.top_cell().begin_shapes_rec(li))
            for li in layout.layer_indexes()}


def compare(source, work, env, name, bbox, layers=None, cell="FLOE_CLIP"):
    args = ["clip", source, "--bbox=" + ",".join(map(str, bbox)), "--cell-name", cell]
    if layers is not None:
        args += ["--layers", layers]
    outputs = [work / f"{name} {suffix}.oas" for suffix in ("j1", "j8", "python")]
    for jobs, output in zip((1, 8), outputs):
        result = run([*args, "--out", output, "--exact"], dict(env, FLOE_RUST_JOBS=str(jobs)))
        assert "clip saved:" in result.stdout
    run([*args, "--out", outputs[2]], dict(env, FLOE_RUST_JOBS="1"), python=True)
    assert outputs[0].read_bytes() == outputs[1].read_bytes() == outputs[2].read_bytes(), name
    actual = db.Layout()
    actual.read(str(outputs[0]))
    expected = db.Layout()
    expected.read(str(source))
    assert len(list(actual.each_cell())) == 1 and actual.top_cell().name == cell, name
    assert actual.dbu == expected.dbu
    a = regions(actual)
    e = regions(expected)
    b = [round(v / expected.dbu) for v in bbox]
    clip = db.Region(db.Box(min(b[0], b[2]), min(b[1], b[3]), max(b[0], b[2]), max(b[1], b[3])))
    # Name resolution parity is tested separately; these keys are explicit.
    selected = set(e) if layers is None or layers == "all" or not layers.strip(', ') else {
        tuple(map(int, token.split('/'))) for token in layers.split(',') if token}
    assert set(a) <= selected, (name, set(a), selected)
    lit = 0
    for key in selected | set(a):
        want = e.get(key, db.Region()) & clip
        got = a.get(key, db.Region())
        assert (got ^ want).is_empty(), (name, key, "Region XOR mismatch")
        lit += not got.is_empty()
    if name != "empty":
        assert lit > 0, (name, "vacuous clip oracle")
    return outputs[0]


def make_corner_fixture(path):
    layout = db.Layout()
    layout.dbu = 0.001
    leaf, top = layout.create_cell("LEAF"), layout.create_cell("TOP")
    li = layout.layer(7, 0)
    leaf.shapes(li).insert(db.Polygon([db.Point(x, y) for x, y in (
        (0, 0), (1800, 0), (1800, 1800), (1200, 1800),
        (1200, 500), (600, 500), (600, 1800), (0, 1800))]))
    leaf.shapes(layout.layer(8, 3)).insert(db.Path(
        [db.Point(0, -300), db.Point(2200, -300), db.Point(2200, 2100)], 80, 20, 70))
    for x, y in ((100, 100), (800, 300), (1600, 700), (500, 1200)):
        leaf.shapes(li).insert(db.Box(x, y, x + 50, y + 50))
    top.insert(db.CellInstArray(leaf.cell_index(), db.Trans(), db.Vector(3000, 0), db.Vector(0, 3000), 3, 2))
    top.insert(db.CellInstArray(leaf.cell_index(), db.Trans(1, True, 13000, 2000)))
    layout.write(str(path))


def failures(source, work, env, sample):
    target = work / "previous clip.oas"
    old = b"previous successful export"
    args = ["clip", source, "--bbox=0,0,1,1", "--out", target]
    for options in (["--bbox=nan,0,1,1"], ["--bbox=1e30,0,1,1"],
                    ["--bbox=0,0,0.0001,1"], ["--cell-name="],
                    ["--cell-name=a\nb"], ["--cell-name=" + "x" * 4097],
                    ["--layers=UNKNOWN"], ["--depth=0"], ["--thin=keep"], ["--lod"]):
        target.write_bytes(old)
        run([*args, *options], env, code=2)
        assert target.read_bytes() == old, options
    for variable in ("FLOE_RUST_JOBS", "FLOE_RUST_OPEN_TIMEOUT_S", "FLOE_RUST_CLIP_TIMEOUT_S"):
        run(args, dict(env, **{variable: "0"}), code=2)
        assert target.read_bytes() == old
    run(["clip", work / "missing.jb", "--bbox=0,0,1,1", "--out", target], env, code=2)
    assert target.read_bytes() == old

    version = subprocess.check_output([str(RENDERD), "--version"], text=True).split()[1]
    fake = work / "fake-renderd"
    fake.write_text(f'''#!{sys.executable}
import os, pathlib, sys, time
mode = os.environ["FAKE_MODE"]
def hold(phase):
    if mode == phase:
        pathlib.Path(os.environ["FAKE_MARKER"]).write_text(str(os.getpid()))
        time.sleep(60)
hold("ready")
print("ready version={version}", flush=True)
for line in sys.stdin:
    words = line.split()
    kind, d = words[0], dict(w.split("=", 1) for w in words[1:])
    hold(kind)
    if kind == "open": print("opened unit=1000 max_depth=6", flush=True)
    elif kind == "clip":
        if mode == "failure":
            print("error seq=" + d["seq"] + " code=clip message=ENOSPC", flush=True)
            continue
        payload = pathlib.Path(os.environ["FAKE_OASIS"]).read_bytes()
        if mode == "corrupt": payload = b"x" + payload[1:]
        pathlib.Path(d["out"]).write_bytes(payload)
        print("clip seq=" + d["seq"] + " size_bytes=" + str(len(payload)) + " records=0 rects=0 polys=0 ms=0 plan_us=0 read_us=0 decode_us=0 clip_us=0 write_us=0", flush=True)
    elif kind == "quit": break
''')
    fake.chmod(0o700)
    marker = work / "fake-phase"
    fake_env = dict(env, FLOE_RENDERD_BIN=str(fake), FAKE_MARKER=str(marker), FAKE_OASIS=str(sample))
    for mode, message in (("failure", "ENOSPC"), ("corrupt", "header mismatch")):
        target.write_bytes(old)
        result = run(args, dict(fake_env, FAKE_MODE=mode), code=1)
        assert message in result.stderr and "clip saved" not in result.stdout
        assert target.read_bytes() == old
    target.write_bytes(old)
    result = run(args, dict(fake_env, FAKE_MODE="clip", FLOE_RUST_CLIP_TIMEOUT_S="1"), code=1)
    assert "deadline exceeded" in result.stderr and target.read_bytes() == old
    assert not list(Path(env["TMPDIR"]).iterdir())
    for phase in ("ready", "open", "clip"):
        for sig in (signal.SIGINT, signal.SIGTERM):
            marker.unlink(missing_ok=True)
            target.write_bytes(old)
            proc = subprocess.Popen([str(APP), *map(str, args)], cwd=ROOT,
                                    env=dict(fake_env, FAKE_MODE=phase),
                                    stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            try:
                wait_marker(marker, proc)
                child = int(marker.read_text())
                start = time.monotonic()
                proc.send_signal(sig)
                stdout, stderr = proc.communicate(timeout=8)
                assert proc.returncode == 128 + sig, (phase, stdout, stderr)
                assert time.monotonic() - start < 5
                assert target.read_bytes() == old and "clip saved" not in stdout
                try:
                    os.kill(child, 0)
                except ProcessLookupError:
                    pass
                else:
                    raise AssertionError("clip worker not reaped")
            finally:
                if proc.poll() is None:
                    proc.kill()
                    proc.communicate()
            assert not list(Path(env["TMPDIR"]).iterdir()), "worker workspace leaked"
    assert not list(work.glob(".floe-shot-*")), "staging file leaked"


def main(fixture):
    with tempfile.TemporaryDirectory(prefix="floe-app-clip-") as td:
        work = Path(td)
        source = work / "clip 설계 with spaces.oas"
        shutil.copy2(fixture, source)
        temp = work / "worker-temp"
        temp.mkdir()
        env = {k: v for k, v in os.environ.items() if not k.startswith("FLOE_")}
        env.update(FLOE_INDEX_BIN=str(INDEX), FLOE_RENDERD_BIN=str(RENDERD), PATH="",
                   PYTHONPATH=str(ROOT), PYTHONDONTWRITEBYTECODE="1", TMPDIR=str(temp))
        assert "--bbox" in run(["clip", "--help"], env).stdout
        run(["clip"], env, code=2)
        run(["index", source, "--jobs=2"], env)
        cache = Path(str(source) + ".floe")
        before = digest(cache)
        original = source.read_bytes()
        compare(source, work, env, "layers", [0., 0., 100., 100.], "1/0,3/0,6/0", "CLI 한 글")
        compare(source, work, env, "reversed", [100., 100., 0., 0.], "1/0,3/0,6/0", "CLI 한 글")
        compare(source, work, env, "empty-selection-is-all", [0., 0., 100., 100.], ",")
        empty = compare(source, work, env, "empty", [1e6, 1e6, 1e6 + 1., 1e6 + 1.])
        # Exact has no display-setting dependency, including env knobs that
        # would be invalid for rendering. No raster/styling command is sent.
        out = work / "env exact.oas"
        run(["clip", source, "--bbox=1000000,1000000,1000001,1000001", "--out", out],
            dict(env, FLOE_RUST_RASTER_JOBS="invalid", FLOE_RUST_PAGE_HAIRLINE="cull", FLOE_RUST_RAW_FRAME="off"))
        assert out.read_bytes() == empty.read_bytes()
        default = subprocess.run([str(APP), "clip", str(source), "--bbox=1000000,1000000,1000001,1000001"],
                                 cwd=work, env=env, capture_output=True, text=True, timeout=30)
        assert default.returncode == 0, (default.stdout, default.stderr)
        assert (work / "clip.oas").read_bytes() == empty.read_bytes()
        alias = work / "cache alias"
        alias.symlink_to(cache, target_is_directory=True)
        link = work / "output-link.oas"
        link.symlink_to(source)
        for output in (source, cache / "design.ovm", cache / "new.oas",
                       Path(str(cache) + ".index.lock"), alias / "design.ovm", link, work):
            run(["clip", source, "--bbox=0,0,1,1", "--out", output], env, code=2)
        # Source aliases must not bypass output protection either.
        source_alias = work / "source-alias.oas"
        source_alias.symlink_to(source)
        run(["clip", source, "--bbox=0,0,1,1", "--out", source_alias], env, code=2)
        assert source.read_bytes() == original and digest(cache) == before
        failures(source, work, env, empty)
        assert source.read_bytes() == original and digest(cache) == before
        # Name/alias selection and warning-only stale source follow the
        # existing read CLI. Metadata mutations are confined to this fixture.
        meta = json.loads((cache / "meta.json").read_text())
        named = json.loads(json.dumps(meta))
        named["layers"][0]["aliases"] = ["공유 이름"]
        (cache / "meta.json").write_text(json.dumps(named))
        a, b = work / "named.oas", work / "explicit.oas"
        first = named["layers"][0]
        for layer, output in (("공유 이름", a), (f'{first["layer"]}/{first["datatype"]}', b)):
            run(["clip", source, "--bbox=0,0,100,100", "--layers", layer, "--out", output], env)
        assert a.read_bytes() == b.read_bytes()
        os.utime(source, (source.stat().st_atime, source.stat().st_mtime + 2))
        stale = run(["clip", source, "--bbox=0,0,1,1", "--out", out], env)
        assert "source changed; clipping cached geometry" in stale.stderr
        marker = cache / "design.ovm"
        marker.write_bytes(b"x")
        previous = out.read_bytes()
        run(["clip", source, "--bbox=0,0,1,1", "--out", out], env, code=1)
        assert out.read_bytes() == previous
        corner = work / "corners.oas"
        make_corner_fixture(corner)
        run(["index", corner, "--jobs=2"], env)
        compare(corner, work, env, "concave-path-hierarchy", [0.25, 1.1, 14., 4.7])
        compare(corner, work, env, "half-dbu", [-0.0005, -0.3015, 1.8005, 1.8015], cell="字" * 200)
        assert not list(temp.iterdir()), "private worker files leaked"
    print("RUST APP CLIP: ALL OK (Python/j1/j8 bytes + KLayout XOR, signals/timeout/protected output)")


if __name__ == "__main__":
    main(Path(sys.argv[1]))
