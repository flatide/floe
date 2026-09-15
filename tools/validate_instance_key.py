#!/usr/bin/env python3
"""Execute GTK's real DISPLAY normalizer and compare the Rust local IPC key input."""
import ast
import itertools
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def main():
    tree = ast.parse((ROOT / "floe/instance.py").read_text())
    method = next(n for n in tree.body if isinstance(n, ast.FunctionDef)
                  and n.name == "normalize_display")
    scope = {}
    exec(compile(ast.Module(body=[method], type_ignores=[]), "GTK instance key", "exec"), scope)
    inputs = [f"{pad}{host}:{number}{suffix}{pad}"
              for host, number, suffix, pad in itertools.product(
                  ("", "localhost", "unix", "teebox.domain", "[::1]", "사용자"),
                  ("0", "1", "99", "32768"), ("", ".0", ".1", ".12"), ("", " "))]
    inputs += ["", "host", " host ", "a.b", ":", " : .0", "a:b:c.1"]
    cases = [(s, scope["normalize_display"](s)) for s in inputs]
    build = subprocess.run([shutil.which("cargo"), "test", "--offline", "--locked", "-p",
                            "floe-app-core", "--lib", "--no-run", "--message-format=json"],
                           cwd=ROOT / "rust", text=True, capture_output=True, timeout=180)
    assert build.returncode == 0, (build.stdout, build.stderr)
    binaries = [row["executable"] for line in build.stdout.splitlines()
                if (row := json.loads(line)).get("reason") == "compiler-artifact"
                and row.get("executable")]
    assert len(binaries) == 1
    with tempfile.TemporaryDirectory(prefix="floe-instance-key-") as directory:
        fixture = Path(directory) / "cases.json"
        fixture.write_text(json.dumps(cases))
        result = subprocess.run([binaries[0], "gtk_display_key_oracle", "--ignored", "--nocapture"],
                                env=dict(os.environ, PATH="", FLOE_INSTANCE_ORACLE=str(fixture)),
                                text=True, capture_output=True, timeout=30)
        assert result.returncode == 0, (result.stdout, result.stderr)
        assert f"GTK INSTANCE KEY: ALL OK ({len(cases)} " in result.stdout
        print(result.stdout.strip())


if __name__ == "__main__":
    main()
