#!/usr/bin/env python3
"""Native owner HTTP service gate; no Python in the tested runtime."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

from validate_app_clip import compare
from validate_app_render import RENDERD, digest, run

ROOT = Path(__file__).resolve().parents[1]


def main(fixture):
    cargo = shutil.which("cargo") or str(Path.home() / ".cargo/bin/cargo")
    build = subprocess.run([cargo, "test", "--offline", "--locked", "-p", "floe-web",
                            "--test", "owner_service", "--no-run", "--message-format=json"],
                           cwd=ROOT / "rust", text=True, capture_output=True, timeout=180)
    assert build.returncode == 0, build.stderr
    tests = [r["executable"] for line in build.stdout.splitlines()
             if (r := json.loads(line)).get("reason") == "compiler-artifact"
             and r["target"]["name"] == "owner_service" and r.get("executable")]
    assert len(tests) == 1
    with tempfile.TemporaryDirectory(prefix="floe-owner-service-") as td:
        work = Path(td)
        source = work / "설계 with spaces.oas"
        shutil.copy2(fixture, source)
        workers = work / "workers"
        workers.mkdir()
        env = dict(os.environ, PATH="", TMPDIR=str(workers), FLOE_OWNER_FIXTURE=str(source),
                   FLOE_INDEX_BIN=str(ROOT / "rust/target/release/floe-index"),
                   FLOE_RENDERD_BIN=str(ROOT / "rust/target/release/floe-renderd"))
        exports = work / "exports"
        exports.mkdir()
        export_source = exports / "layout.oas"
        shutil.copy2(fixture, export_source)
        oracle_env = dict(env, PYTHONPATH=str(ROOT), PYTHONDONTWRITEBYTECODE="1")
        run(["index", export_source, "--jobs=2"], oracle_env)
        before = digest(Path(str(export_source) + ".floe"))
        for name, layers in [("all", None), ("visible", "1/0"), ("none", None)]:
            golden = compare(export_source, exports, oracle_env, "empty" if name == "none" else name,
                             [1e6, 1e6, 1e6+1, 1e6+1] if name == "none" else [0, 0, 100, 100], layers, "WEB_CLIP")
            shutil.copy2(golden, exports / f"golden-{name}.oas")
        version = subprocess.check_output([str(RENDERD), "--version"], text=True).split()[1]
        for mode in ("cancel", "logout", "failure"):
            fake = exports / f"fake-{mode}"
            fake.write_text(f'''#!{sys.executable}
import pathlib,sys,os,time,struct,zlib
def chunk(tag,data):
    return struct.pack(">I",len(data))+tag+data+struct.pack(">I",zlib.crc32(tag+data))
def png(w,h):
    return b"\\x89PNG\\r\\n\\x1a\\n"+chunk(b"IHDR",struct.pack(">IIBBBBB",w,h,8,6,0,0,0))+chunk(b"IDAT",zlib.compress((b"\\0"+b"\\0\\0\\0\\xff"*w)*h))+chunk(b"IEND",b"")
print("ready version={version}",flush=True)
for line in sys.stdin:
    words=line.split();kind=words[0];d=dict(w.split("=",1) for w in words[1:])
    if kind=="open":print("opened unit=1000 max_depth=6",flush=True)
    elif kind=="style":print("styled epoch="+d["epoch"],flush=True)
    elif kind=="render":
        pathlib.Path(d["out"]).write_bytes(png(int(d["w"]),int(d["h"])))
        print("frame gen="+d["gen"]+" round=1 final=1 partial=0 deferred=0 labels_truncated=0 style_epoch="+d["style_epoch"]+" format=png png="+d["out"]+" scene_gen="+d["gen"]+" scene_round=1 scene_complete=0 scene_summary=1",flush=True)
    elif kind=="clip":
        if {mode!r}=="failure":print("error seq="+d["seq"]+" code=clip message=ENOSPC_private_path",flush=True)
        else:
            pathlib.Path(sys.argv[0]).with_suffix(".pid").write_text(str(os.getpid()))
            time.sleep(60)
    elif kind=="quit":break
''')
            fake.chmod(0o700)
        env["FLOE_OWNER_EXPORT_FIXTURE"] = str(export_source)
        mode_dir = work / "mode"
        mode_dir.mkdir()
        mode_caches = {}
        for name in ("A", "B"):
            mode_source = mode_dir / (name + ".oas")
            shutil.copy2(fixture, mode_source)
            run(["index", mode_source, "--jobs=2"], oracle_env)
            cache = Path(str(mode_source) + ".floe")
            mode_caches[cache] = digest(cache)
        env["FLOE_OWNER_MODE_FIXTURE"] = str(mode_dir / "A.oas")
        checked = subprocess.run([tests[0], "--ignored", "--nocapture"], env=env,
                             text=True, capture_output=True, timeout=120)
        assert checked.returncode == 0, (checked.stdout, checked.stderr)
        assert "RUST OWNER SERVICE: ALL OK" in checked.stdout
        assert "RUST DRC ISOLATION: ALL OK" in checked.stdout
        assert "RUST GUEST DRC: ALL OK" in checked.stdout
        assert "RUST DRC BUILD IDENTITY: ALL OK" in checked.stdout
        assert "RUST DRC WAIVE REVISION: ALL OK" in checked.stdout
        assert "RUST OWNER EXPORT: ALL OK" in checked.stdout
        assert "RUST OWNER EXPORT LIFECYCLE: ALL OK" in checked.stdout
        assert "RUST OWNER SETTINGS: ALL OK" in checked.stdout
        assert "RUST OWNER DEFAULTS: ALL OK" in checked.stdout
        assert "RUST OWNER DEFAULT PROTECTION: ALL OK" in checked.stdout
        assert "RUST OWNER DYNAMIC CATALOG: ALL OK" in checked.stdout
        assert "RUST OWNER LAUNCH: ALL OK" in checked.stdout
        assert "RUST WINDOW DISPLAY: ALL OK" in checked.stdout
        assert "RUST INDEX OPEN NATIVE: ALL OK" in checked.stdout
        assert "RUST INDEX OPEN DECK: ALL OK" in checked.stdout
        assert "RUST INDEX OPEN RACES: ALL OK" in checked.stdout
        assert "RUST LIVE DECK INDEX: ALL OK" in checked.stdout
        assert "RUST DECK LEVELS: ALL OK" in checked.stdout
        assert "RUST DECK LEVELS INDEX: ALL OK" in checked.stdout
        assert "RUST OWNER DECK DEFAULTS: ALL OK" in checked.stdout
        assert "RUST OWNER DECK MODES: ALL OK (5 native cutovers, one reservation" in checked.stdout
        assert all(digest(path) == before for path, before in mode_caches.items())
        for marker in exports.glob("fake-*.pid"):
            try:
                os.kill(int(marker.read_text()), 0)
            except ProcessLookupError:
                pass
            else:
                raise AssertionError("export worker was not reaped")
        assert digest(Path(str(export_source) + ".floe")) == before
        assert not list(workers.iterdir()), "owner service leaked worker files"
        print(checked.stdout.strip())


if __name__ == "__main__":
    main(Path(sys.argv[1]).resolve())
