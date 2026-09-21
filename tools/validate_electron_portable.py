#!/usr/bin/env python3
"""Development-only bundle checks; no GUI, clipboard or real designs.

--self-test uses a fresh synthetic launcher. --bundle DIR reads a relocated bundle.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[1]


class LauncherTests(unittest.TestCase):
    def test_caller_cwd_arguments_overrides_and_no_fallback(self):
        with tempfile.TemporaryDirectory(prefix="floe-electron-launch-") as td:
            root = Path(td).resolve()
            bundle = root / "bundle 한글 with spaces"
            bundle.mkdir()
            # Use an installed interpreter, not a newly executable shebang
            # file that can trigger unrelated macOS first-launch scanning.
            (bundle / "electron").write_text('printf \'%s\\000\' "$PWD" "$0" "$@"\n')
            runtime = bundle / "runtime" / "mock runtime"
            runtime.parent.mkdir()
            runtime.symlink_to("/bin/sh")
            launcher = bundle / "floe2-electron"
            launcher.write_text((REPO / "tools/electron-portable/launch.sh")
                                .read_text().replace("@RUNTIME@", "mock runtime"))
            env = {k: v for k, v in os.environ.items() if not k.startswith("FLOE_")}
            for key in ("ELECTRON_RUN_AS_NODE", "NODE_OPTIONS", "NODE_PATH"):
                env.pop(key, None)

            def call(extra=None):
                return subprocess.run(
                    ["sh", str(launcher), "view", "relative 한글.oas", "--cell", "x ; $(false)"],
                    env=dict(env, **(extra or {})), cwd=root, capture_output=True,
                    text=True, timeout=10,
                )

            result = call()
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout.split("\0")[:-1], [str(root), str(bundle / "electron"),
                             "view", "relative 한글.oas", "--cell", "x ; $(false)"])
            self.assertEqual(call({"FLOE_ELECTRON_BIN": str(runtime)}).returncode, 0)
            for value in ("", "relative", str(root / "missing"), str(root)):
                self.assertNotEqual(call({"FLOE_ELECTRON_BIN": value}).returncode, 0)
            for key in ("ELECTRON_RUN_AS_NODE", "NODE_OPTIONS", "NODE_PATH"):
                result = call({key: "synthetic"})
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(result.stdout, "")

    def test_explicit_javascript_closure(self):
        text = (REPO / "rust/packager/src/electron.rs").read_text()
        listed = set(re.findall(r'"([^"]+)"', text.split("const APP_FILES:", 1)[1]
                                .split("];", 1)[0]))
        self.assertIn("electron/main.cjs", listed)
        for name in listed:
            source = REPO / name
            self.assertTrue(source.is_file(), name)
            if source.suffix not in (".cjs", ".js"):
                continue
            for relative in re.findall(r"require\(['\"](\.[^'\"]+)['\"]\)", source.read_text()):
                candidate = (source.parent / relative).resolve().relative_to(REPO)
                self.assertIn(str(candidate), listed, (name, relative))
        for shared in ("menu-action.js", "recovery-status.js", "frame-parity-probe.js"):
            self.assertIn("desktop/ui/" + shared, listed)


def inspect_bundle(bundle):
    bundle = bundle.resolve(strict=True)
    manifest = json.loads((bundle / "BUNDLE.json").read_text())
    assert manifest["format"] == 1
    assert manifest["product"] == "floe2-electron-comparison"
    expected = manifest["files"]
    found = set()
    for directory, dirs, files in os.walk(bundle, followlinks=False):
        for name in dirs + files:
            path = Path(directory) / name
            relative = str(path.relative_to(bundle))
            if relative == "BUNDLE.json":
                continue
            found.add(relative)
            entry = expected[relative]
            if path.is_symlink():
                assert entry == {"kind": "symlink", "target": os.readlink(path)}
                assert not Path(entry["target"]).is_absolute()
                assert path.resolve(strict=True).is_relative_to(bundle)
            else:
                assert entry["mode"] == path.stat().st_mode & 0o7777
                if path.is_dir():
                    assert entry["kind"] == "directory"
                else:
                    assert entry["kind"] == "file" and entry["bytes"] == path.stat().st_size
                    with path.open("rb") as stream:
                        assert hashlib.file_digest(stream, "sha256").hexdigest() == entry["sha256"]
    assert found == set(expected)
    for name in ("runtime/LICENSE", "runtime/LICENSES.chromium.html", "NOTICES/INVENTORY.txt",
                 "electron/main.cjs", "rust/target/release/floe-index",
                 "rust/target/release/floe-renderd",
                 "electron/service/target/release/floe-electron-service",
                 "electron/service/target/release/floe-electron-download"):
        assert expected[name]["kind"] == "file"
    assert not any(n.endswith((".oas", ".py")) or ".ice/" in n or "node_modules/" in n for n in found)
    result = subprocess.run(["sh", str(bundle / "verify.sh")], capture_output=True,
                            text=True, timeout=120)
    assert result.returncode == 0, result.stderr
    print(result.stdout.strip())
    print("ELECTRON PORTABLE: independent inventory verified; no GUI/clipboard test")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--self-test", action="store_true")
    mode.add_argument("--bundle", type=Path)
    args = parser.parse_args()
    if args.self_test:
        unittest.main(argv=[sys.argv[0]])
    else:
        inspect_bundle(args.bundle)
