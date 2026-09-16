#!/usr/bin/env python3
"""Managed exact clip core: private fixtures, native bytes and lifecycle faults.

No HTTP/auth/download API is implied by this core gate. Python/KLayout create
oracles and controlled worker faults only; the product test runs PATH-empty.
"""
import json
import os
from pathlib import Path
from cache_test_paths import vfs_cache
import shutil
import subprocess
import sys
import tempfile

from validate_app_clip import compare, make_corner_fixture
from validate_app_render import INDEX, RENDERD, ROOT, digest, run


def main():
    cargo = shutil.which("cargo") or str(Path.home() / ".cargo/bin/cargo")
    built = subprocess.run(
        [cargo, "test", "--offline", "--locked", "-p", "floe-app-core", "--test",
         "managed_clip", "--no-run", "--message-format=json"], cwd=ROOT / "rust",
        capture_output=True, text=True, timeout=180)
    assert built.returncode == 0, built.stderr
    binaries = [r["executable"] for line in built.stdout.splitlines()
                if (r := json.loads(line)).get("reason") == "compiler-artifact"
                and r["target"]["name"] == "managed_clip" and r.get("executable")]
    assert len(binaries) == 1
    with tempfile.TemporaryDirectory(prefix="floe-managed-clip-") as td:
        root = Path(td).resolve()
        workers = root / "workers"
        workers.mkdir()
        env = {k: v for k, v in os.environ.items() if not k.startswith("FLOE_")}
        env.update(PATH="", TMPDIR=str(workers), PYTHONPATH=str(ROOT),
                   PYTHONDONTWRITEBYTECODE="1", FLOE_INDEX_BIN=str(INDEX),
                   FLOE_RENDERD_BIN=str(RENDERD), FLOE_MANAGED_EXPORT_ROOT=str(root))
        source = root / "layout 설계.oas"
        make_corner_fixture(source)
        originals = {}
        for p in [source, *[root / f"{name}.oas" for name in
                           ("stale", "alternate", "changed", "cache-change")]]:
            if p != source:
                shutil.copy2(source, p)
            run(["index", p, "--jobs=2"], env)
            originals[p] = (p.read_bytes(), digest(vfs_cache(p)))
        bbox = [0.25, 1.1, 14., 4.7]
        for name, layers in [("all", None), ("selected", "7/0"), ("none", None)]:
            path = compare(source, root, env, "empty" if name == "none" else name,
                           [1e6, 1e6, 1e6 + 1, 1e6 + 1] if name == "none" else bbox,
                           layers, "MANAGED_CLIP")
            shutil.copy2(path, root / f"golden-{name}.oas")
        stale = root / "stale.oas"
        stat = stale.stat()
        os.utime(stale, ns=(stat.st_atime_ns, stat.st_mtime_ns + 2_000_000_000))
        (root / "test.jb").write_text(
            "MTITLE 1,Mask\nCHIP C\n"
            "$ (1,A,AD=0.001,TC='layout 설계.oas',LY={7},DT={0},UX=14000,UY=5000)\nROWS 0/0\n")
        fake = root / "fake"
        fake.mkdir()
        version = subprocess.check_output([str(RENDERD), "--version"], text=True).split()[1]
        script = f'''#!{sys.executable}
import os, pathlib, sys, time
binary = pathlib.Path(sys.argv[0])
mode = binary.name
root = pathlib.Path(os.environ["FLOE_MANAGED_EXPORT_ROOT"])
def hold(phase):
    if mode.startswith(phase + "_") or (mode == "failure_cancel" and phase == "quit") or (mode == "timeout" and phase == "clip"):
        pathlib.Path(str(binary) + ".started").write_text(str(os.getpid()))
        time.sleep(60)
hold("ready")
print("ready version={version}", flush=True)
for line in sys.stdin:
    words = line.split()
    kind, d = words[0], dict(w.split("=", 1) for w in words[1:])
    hold(kind)
    if kind == "open": print("opened unit=1000 max_depth=6", flush=True)
    elif kind == "clip":
        if mode.startswith("failure"):
            print("error seq=" + d["seq"] + " code=clip message=ENOSPC", flush=True)
            continue
        payload = (root / "golden-all.oas").read_bytes()
        if mode == "corrupt": payload = b"x" + payload[1:]
        pathlib.Path(d["out"]).write_bytes(payload)
        if mode == "source-change":
            path = root / "changed.oas"
            st = path.stat()
            os.utime(path, ns=(st.st_atime_ns, st.st_mtime_ns + 2_000_000_000))
        if mode == "cache-change":
            path = pathlib.Path({str(vfs_cache(root / "cache-change.oas") / "design.ovm")!r})
            st = path.stat()
            with path.open("r+b") as out: out.write(b"X")
            os.utime(path, ns=(st.st_atime_ns, st.st_mtime_ns))
        print("clip seq=" + d["seq"] + " size_bytes=" + str(len(payload)) + " records=1 rects=1 polys=0 ms=0 plan_us=0 read_us=0 decode_us=0 clip_us=0 write_us=0", flush=True)
    elif kind == "quit": break
'''
        names = [f"{phase}_{action}" for phase in ("ready", "open", "clip", "quit")
                 for action in ("cancel", "drop", "close")]
        for name in names + ["failure", "corrupt", "timeout", "source-change",
                             "cache-change", "failure_cancel"]:
            p = fake / name
            p.write_text(script)
            p.chmod(0o700)
        checked = subprocess.run([binaries[0], "--ignored", "--nocapture"], env=env,
                                 capture_output=True, text=True, timeout=90)
        assert checked.returncode == 0, (checked.stdout, checked.stderr)
        assert "RUST MANAGED CLIP: ALL OK" in checked.stdout
        assert (vfs_cache(root / "cache-change.oas") / "design.ovm").read_bytes()[:1] == b"X", (
            "cache-change fault was not injected")
        for name in ("all", "selected", "none"):
            for jobs in (1, 8):
                assert (root / f"managed-{name}-j{jobs}.oas").read_bytes() == (
                    root / f"golden-{name}.oas").read_bytes()
        assert (root / "managed-stale.oas").read_bytes() == (root / "golden-all.oas").read_bytes()
        for p, (content, cache) in originals.items():
            assert p.read_bytes() == content
            if p.name != "cache-change.oas":
                assert digest(vfs_cache(p)) == cache, p
        assert not list(workers.iterdir()), "managed clip leaked native files"
        print(checked.stdout.strip())
    print("MANAGED CLIP GATE: ALL OK (Python/j1/j8 bytes + KLayout XOR, lifecycle/admission/faults)")


if __name__ == "__main__":
    main()
