#!/usr/bin/env python3
"""GTK depth-step source oracle; development only, no GTK runtime required."""
import ast
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
from types import MethodType, SimpleNamespace

ROOT = Path(__file__).resolve().parents[1]


def main():
    names = {"_depth_step", "_set_depth", "_depth"}
    funcs = [n for n in ast.walk(ast.parse((ROOT / "floe/gui.py").read_text()))
             if isinstance(n, ast.FunctionDef) and n.name in names]
    assert len(funcs) == len(names)
    scope = {}
    exec(compile(ast.Module(body=funcs, type_ignores=[]), "GTK depth oracle", "exec"), scope)
    cases = []
    for maximum in (None, 0, 1, 2, 8, 99, 998, 999, 1000, 2**64-1):
        for current in (None, 0, 1, 2, 9, 99, 998, 999, 1000, 2**32-1):
            for delta in (-1, 1):
                obj = SimpleNamespace(max_depth=maximum, depth_value=999 if current is None else current,
                                      _ddlg=None, _on_depth=lambda: None)
                for name in names:
                    setattr(obj, name, MethodType(scope[name], obj))
                obj._depth_step(delta)
                cases.append(dict(maximum=maximum, current=current, delta=delta, want=obj._depth()))
    cargo = shutil.which("cargo")
    assert cargo
    build = subprocess.run([cargo, "test", "--offline", "--locked", "-p", "floe-app-core", "--lib",
                            "--no-run", "--message-format=json"], cwd=ROOT / "rust",
                           text=True, capture_output=True, timeout=180)
    assert build.returncode == 0, build.stderr
    bins = [v["executable"] for line in build.stdout.splitlines()
            if (v := json.loads(line)).get("reason") == "compiler-artifact" and v.get("executable")]
    assert len(bins) == 1
    with tempfile.TemporaryDirectory(prefix="floe-depth-oracle-") as td:
        path = Path(td) / "cases.json"
        path.write_text(json.dumps(cases))
        result = subprocess.run([bins[0], "gtk_depth_step_oracle", "--ignored", "--nocapture"],
                                env=dict(os.environ, PATH="", FLOE_DEPTH_CASES=str(path)),
                                text=True, capture_output=True, timeout=30)
        assert result.returncode == 0, (result.stdout, result.stderr)
        assert "GTK DEPTH KEYS: ALL OK" in result.stdout
        print(result.stdout.strip())


if __name__ == "__main__":
    main()
