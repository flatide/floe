#!/usr/bin/env python3
"""Execute GTK's real file-open policy with fake cache I/O, compare Rust policy.

No GTK runtime or actual source/cache reads. Native first-frame/worker behaviour
is separately exercised by the owner service gate.
"""
import ast
import itertools
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
from types import MethodType, SimpleNamespace
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]


def main():
    tree = ast.parse((ROOT / "floe/gui.py").read_text())
    viewer = next(n for n in tree.body if isinstance(n, ast.ClassDef) and n.name == "Viewer")
    methods = {n.name: n for n in viewer.body if isinstance(n, ast.FunctionDef)}
    functions = [methods[n] for n in ("open_file", "_set_depth")]
    scope = dict(os=os, APP="floe2", __name__="floe.gui", __package__="floe",
                 _is_deck_path=lambda p: p.endswith(".jb"))

    class Cache:
        def __init__(self, path, ids=None):
            self.src, self.ids = path, ids
        def is_stale(self):
            return False
        def exists(self):
            return True
        def load(self):
            pass

    scope["cache_mod"] = SimpleNamespace(Cache=Cache)
    exec(compile(ast.Module(body=functions, type_ignores=[]), "GTK file policy", "exec"), scope)
    writes = {n.attr for n in ast.walk(methods["_apply_cache"])
              if isinstance(n, ast.Attribute) and isinstance(n.ctx, ast.Store)
              and isinstance(n.value, ast.Name) and n.value.id == "self"}
    stable = {"depth_value", "detail", "thin_mode", "frames_on", "labels_on", "label_font_px"}
    assert not writes & stable, "GTK changed which display preferences survive cache replacement"
    mono = [n for n in methods["_apply_cache"].body if isinstance(n, ast.Assign)
            and any(isinstance(t, ast.Attribute) and t.attr in ("_mono", "_mono_saved") for t in n.targets)]
    assert len(mono) == 2
    mono_code = compile(ast.Module(body=mono, type_ignores=[]), "GTK file mono reset", "exec")
    cases = []
    for depth, detail, thin, frames, labels, font, deck, same in itertools.product(
            (0, 7, 999), (0, 1, 2), ("auto", "keep", "cull"), (False, True),
            (False, True), (6, 14, 96), (False, True), (False, True)):
        path = "/synthetic/next.jb" if deck else "/synthetic/next.oas"
        obj = SimpleNamespace(cache=Cache(path if same else "/synthetic/old.oas"),
            depth_value=depth, detail=detail, thin_mode=thin, frames_on=frames,
            labels_on=labels, label_font_px=font, _mono=True, _mono_saved=True,
            _ddlg=None, _restore_keys=lambda: None)
        def apply(c):
            obj.cache = c
            exec(mono_code, dict(self=obj))
        obj._apply_cache = apply
        for name in ("open_file", "_set_depth"):
            setattr(obj, name, MethodType(scope[name], obj))
        with patch.dict(sys.modules, {"floe.jobdeck.viewer": SimpleNamespace(DeckCache=Cache)}):
            assert obj.open_file(path) is None
        cases.append(dict(before=dict(depth=depth, detail=detail, thin=thin, frames=frames,
            labels=labels, font_px=font), deck=deck, same=same, want=dict(
            depth=None if obj.depth_value == 999 else obj.depth_value, detail=obj.detail,
            thin=obj.thin_mode, frames=obj.frames_on, labels=obj.labels_on and not deck,
            font_px=obj.label_font_px, mono=obj._mono)))
    build = subprocess.run([shutil.which("cargo"), "test", "--offline", "--locked", "-p", "floe-web",
        "--lib", "--no-run", "--message-format=json"], cwd=ROOT / "rust", text=True,
        capture_output=True, timeout=180)
    assert build.returncode == 0, build.stderr
    bins = [r["executable"] for line in build.stdout.splitlines()
            if (r := json.loads(line)).get("reason") == "compiler-artifact" and r.get("executable")]
    assert len(bins) == 1
    with tempfile.TemporaryDirectory(prefix="floe-file-display-") as td:
        path = Path(td) / "oracle.json"
        path.write_text(json.dumps(cases))
        checked = subprocess.run([bins[0], "gtk_file_display_oracle", "--ignored", "--nocapture"],
            env=dict(os.environ, PATH="", FLOE_FILE_DISPLAY_ORACLE=str(path)),
            text=True, capture_output=True, timeout=30)
        assert checked.returncode == 0, (checked.stdout, checked.stderr)
        assert f"GTK FILE DISPLAY: ALL OK ({len(cases)} " in checked.stdout
        print(checked.stdout.strip())


if __name__ == "__main__":
    main()
