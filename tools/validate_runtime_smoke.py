#!/usr/bin/env python3
"""Development preparation only; the compiled runtime test needs no interpreter.

This host run is not Linux acceptance unless it actually executes on Linux.
No source design or pre-generated cache is accepted as input.
"""
import argparse
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--require-linux", action="store_true")
    parser.add_argument("--binaries", type=Path, default=ROOT / "rust/target/release")
    args = parser.parse_args()
    if args.require_linux and platform.system() != "Linux":
        parser.error("Linux execution is required; this host is " + platform.system())
    binary_dir = args.binaries.resolve(strict=True)
    cargo = shutil.which("cargo") or str(Path.home() / ".cargo/bin/cargo")
    build = subprocess.run(
        [cargo, "test", "--offline", "--locked", "-j2", "-p", "floe-app-core",
         "--test", "runtime_smoke", "--no-run", "--message-format=json"],
        cwd=ROOT / "rust", capture_output=True, text=True, timeout=240)
    assert build.returncode == 0, build.stderr
    programs = [row["executable"] for line in build.stdout.splitlines()
                if (row := json.loads(line)).get("reason") == "compiler-artifact"
                and row["target"]["name"] == "runtime_smoke" and row.get("executable")]
    assert len(programs) == 1, "missing or ambiguous compiled runtime test"
    with tempfile.TemporaryDirectory(prefix="floe-runtime-gate-") as td:
        result = subprocess.run(
            [programs[0], "--ignored", "--exact", "native_runtime_without_python", "--nocapture"],
            cwd=td, env={"PATH": "", "TMPDIR": td,
                         "FLOE_RUNTIME_BIN_DIR": str(binary_dir)},
            capture_output=True, text=True, timeout=180)
        assert result.returncode == 0, (result.stdout, result.stderr)
        assert "NATIVE RUNTIME SMOKE: ALL OK" in result.stdout
        assert "1 passed; 0 failed; 0 ignored" in result.stdout
        assert not list(Path(td).iterdir()), "runtime fixture/worker cleanup incomplete"
        print(result.stdout.strip())
    print("RUNTIME SMOKE HARNESS: ALL OK (host=" + platform.system() + "/" +
          platform.machine() + "; not whole G4 or browser acceptance)")


if __name__ == "__main__":
    main()
