#!/usr/bin/env python3
"""Product-boundary checks for the Rust-only ``floe2`` shell."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile


ROOT = Path(__file__).resolve().parents[1]


def check(condition, message):
    if not condition:
        raise AssertionError(message)


def run(env, *args, ok=True):
    result = subprocess.run(
        [sys.executable, "-B", *map(str, args)], cwd=ROOT, env=env,
        capture_output=True, text=True, timeout=20)
    if ok and result.returncode:
        raise AssertionError(
            "command failed: %r\nstdout:\n%s\nstderr:\n%s" %
            (args, result.stdout, result.stderr))
    if not ok and not result.returncode:
        raise AssertionError("command unexpectedly succeeded: %r" % (args,))
    return result


def main():
    base = os.environ.copy()
    base.pop("FLOE_PRODUCT", None)
    base.pop("FLOE_RENDERER", None)
    base["PYTHONDONTWRITEBYTECODE"] = "1"
    base["PYTHONPATH"] = str(ROOT)

    floe = run(base, "-m", "floe", "--help")
    check("stable KLayout" in floe.stdout, "floe product description drifted")
    floe2 = run(base, "-m", "floe2", "--help")
    check("Rust-only" in floe2.stdout, "floe2 is not identified as Rust-only")
    check("profile" not in floe2.stdout,
          "floe2 exposed the legacy tile-cache profile command")
    index_help = run(base, "-m", "floe2", "index", "--help")
    for legacy in ("--legacy", "--tile-mb", "--read-mode", "KLayout"):
        check(legacy not in index_help.stdout,
              "floe2 index help exposed %s" % legacy)
    for command in ("probe", "view"):
        command_help = run(base, "-m", "floe2", command, "--help")
        check("--layout-mode" not in command_help.stdout,
              "floe2 %s help exposed a KLayout worker option" % command)

    rejected_env = dict(base, FLOE_RENDERER="klayout")
    rejected = run(rejected_env, "-m", "floe2", "--version", ok=False)
    check("Rust-only" in rejected.stderr,
          "floe2 accepted or obscured a KLayout renderer override")
    legacy = run(base, "-m", "floe2", "index", "missing.oas",
                 "--legacy", ok=False)
    check("legacy indexing belongs to floe" in legacy.stderr,
          "floe2 legacy indexing did not fail at the product boundary")

    identity = r'''import json, os
from floe.cli import _renderer_backend
from floe import instance
print(json.dumps([_renderer_backend(), instance.APP,
                  instance.socket_address(":77")]))
'''
    stable = json.loads(run(base, "-c", identity).stdout)
    rust = json.loads(run(base, "-c", "import floe2\n" + identity).stdout)
    check(stable[0:2] == ["klayout", "floe"],
          "stable floe no longer owns the KLayout/floe identity")
    check(rust[0:2] == ["rust", "floe2"],
          "floe2 did not select the Rust/floe2 identity")
    check(stable[2] != rust[2], "floe and floe2 share an instance socket")

    portable = ROOT / "tools" / "make_portable.sh"
    syntax = subprocess.run(
        ["bash", "-n", str(portable)], cwd=ROOT,
        capture_output=True, text=True)
    check(syntax.returncode == 0,
          "portable script syntax failed: %s" % syntax.stderr)
    conflict_env = dict(base, FLOE_PORTABLE_PRODUCT="floe2",
                        FLOE_PORTABLE_KLAYOUT="1")
    conflict = subprocess.run(
        ["bash", str(portable)], cwd=ROOT, env=conflict_env,
        capture_output=True, text=True, timeout=10)
    check(conflict.returncode != 0 and "Rust-only" in conflict.stdout,
          "portable allowed a floe2/KLayout product mixture")

    with tempfile.TemporaryDirectory(prefix="floe2-cli-") as td:
        work = Path(td)
        blocker = work / "blocker"
        blocker.mkdir()
        (blocker / "sitecustomize.py").write_text(
            "import builtins\n"
            "_real = builtins.__import__\n"
            "def _guard(name, *args, **kwargs):\n"
            "    if name == 'klayout' or name.startswith('klayout.'):\n"
            "        raise RuntimeError('KLayout import forbidden')\n"
            "    return _real(name, *args, **kwargs)\n"
            "builtins.__import__ = _guard\n", encoding="utf-8")
        log = work / "calls.json"
        binary = work / "floe-index"
        binary.write_text(
            "#!/usr/bin/env python3\n"
            "import json, os, pathlib, sys\n"
            "pathlib.Path(os.environ['FLOE2_CALL']).write_text("
            "json.dumps(sys.argv[1:]))\n",
            encoding="utf-8")
        binary.chmod(0o755)
        source = work / "design with spaces.oas"
        source.write_bytes(b"fixture")
        env = dict(base, FLOE_INDEX_BIN=str(binary), FLOE2_CALL=str(log))
        env["PYTHONPATH"] = os.pathsep.join((str(blocker), str(ROOT)))
        delegated = run(env, "-m", "floe2", "index", source,
                        "--jobs", "2")
        check(delegated.returncode == 0, "floe2 Rust index delegation failed")
        check(json.loads(log.read_text()) == [
            "vfs", str(source), str(source) + ".floe", "--jobs", "2",
        ], "floe2 changed the canonical Rust index argv")

    print("FLOE2 PRODUCT VALIDATION: ALL OK")


if __name__ == "__main__":
    main()
