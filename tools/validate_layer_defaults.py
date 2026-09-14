#!/usr/bin/env python3
"""Shared-default publication: GTK path/text oracle, private synthetic files.

Product execution is Rust-only with PATH empty. No real design default,
source/cache, personal preference, browser approval or remote path is written.
"""
import ast
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from floe.cache import save_shared_props, shared_props_paths
from floe.fillpat import format_layerprops
from floe.jobdeck.color import MODE_LEVEL


def props_source(source, mode):
    tree = ast.parse((ROOT / "floe/jobdeck/viewer.py").read_text())
    functions = [n for n in ast.walk(tree) if isinstance(n, ast.FunctionDef) and n.name == "props_src"]
    assert len(functions) == 1
    functions[0].decorator_list = []
    scope = {"os": os, "MODE_LEVEL": MODE_LEVEL}
    exec(compile(ast.Module(body=functions, type_ignores=[]), "GTK props_src oracle", "exec"), scope)
    return scope["props_src"](SimpleNamespace(src=str(source), mode=mode))


def digest(directory):
    return {str(p.relative_to(directory)): (hashlib.sha256(p.read_bytes()).hexdigest(), p.stat().st_mtime_ns)
            for p in directory.rglob("*") if p.is_file()}


def main(fixture):
    cargo = os.environ.get("CARGO", shutil.which("cargo") or str(Path.home() / ".cargo/bin/cargo"))
    build = subprocess.run([cargo, "test", "--offline", "--locked", "-p", "floe-app-core", "--test",
                            "layer_defaults", "--no-run", "--message-format=json"], cwd=ROOT / "rust",
                           capture_output=True, text=True, timeout=180)
    assert build.returncode == 0, build.stderr
    binaries = [r["executable"] for line in build.stdout.splitlines()
                if (r := json.loads(line)).get("reason") == "compiler-artifact"
                and r["target"]["name"] == "layer_defaults" and r.get("executable")]
    assert len(binaries) == 1
    with tempfile.TemporaryDirectory(prefix="floe-default-oracle-") as td:
        root = Path(td).resolve()
        native = root / "native"
        legacy = root / "gtk"
        native.mkdir()
        legacy.mkdir()
        cases = []
        text = format_layerprops([((3, 0), "red", "solid", "Mask 한글", "1", "3"),
                                  ((3, 300), "blue", "speckle", "Fill", "0", "2")])
        for i, name in enumerate(["layout.oas", "설계 test.oas", "mask.jb", "a.b.jb", ".mask.jb", "...jb", "마스크 파일.jb", "mask space.jb"]):
            is_deck = name.endswith(".jb")
            for mode in (("level", "chip", "layer") if is_deck else ("level",)):
                # Some all-dot stems have colliding GTK fallback names across
                # modes. Isolate cases so creating the fallback for one mode
                # cannot replace another test's pre-existing target.
                native_case = native / (str(i) + "-" + mode)
                legacy_case = legacy / (str(i) + "-" + mode)
                native_case.mkdir()
                legacy_case.mkdir()
                source = native_case / name
                old_source = legacy_case / name
                if is_deck:
                    source.write_text("MTITLE 1,Mask\nCHIP A\n$ (1,A,TC=missing.oas)\n")
                    old_source.write_bytes(source.read_bytes())
                else:
                    shutil.copy2(fixture, source)
                    shutil.copy2(fixture, old_source)
                old_props = props_source(old_source, mode) if is_deck else str(old_source)
                candidates = shared_props_paths(old_props)
                output = Path(save_shared_props(old_props, text))
                assert str(output) == candidates[0]
                target = native_case / output.name
                before = "old default" if i % 2 else None
                if before is not None:
                    target.write_text(before)
                # Stem fallback is not the publication target, even if it exists.
                if candidates[1] != candidates[0]:
                    (native_case / Path(candidates[1]).name).write_text("stem fallback must survive")
                cases.append(dict(source=str(source), mode=mode, text=output.read_text(),
                                  target=str(target), before=before))
        assert len(cases) == 20
        before = digest(native)
        oracle = root / "oracle.json"
        oracle.write_text(json.dumps(cases))
        env = {k: v for k, v in os.environ.items() if not k.startswith("FLOE_")}
        env.update(PATH="", TMPDIR=str(native), FLOE_DEFAULTS_ORACLE=str(oracle))
        result = subprocess.run([binaries[0], "--ignored", "--nocapture"], env=env,
                                capture_output=True, text=True, timeout=30)
        assert result.returncode == 0, (result.stdout, result.stderr)
        assert "LAYER DEFAULTS: ALL OK (20 GTK targets/bytes, native publication)" in result.stdout
        after = digest(native)
        outputs = {str(Path(c["target"]).relative_to(native)) for c in cases}
        locks = {name + ".lock" for name in outputs}
        for name, state in before.items():
            if name not in outputs:
                assert after[name] == state, name
        assert set(after) - set(before) <= outputs | locks
        for name in locks:
            assert (native / name).read_bytes() == b""
        print(result.stdout.strip())


if __name__ == "__main__":
    main(Path(sys.argv[1]).resolve())
