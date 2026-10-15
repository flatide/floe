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
import shutil
import subprocess
import sys
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[1]


def app_files():
    text = (REPO / "rust/packager/src/electron.rs").read_text()
    return set(re.findall(r'"([^"]+)"', text.split("const APP_FILES:", 1)[1]
                          .split("];", 1)[0]))


class LauncherTests(unittest.TestCase):
    def test_teebox_launcher_and_readonly_preflight(self):
        # Installed /bin/sh handles fake programs; no new executable interpreter,
        # Chromium, design, connection, or real Linux/ETX assertion here.
        with tempfile.TemporaryDirectory(prefix="floe-teebox-launch-") as td:
            work = Path(td).resolve()
            bundle = work / "bundle 한글 spaces"
            bundle.mkdir()
            launcher = bundle / "floe2-teebox"
            launcher.write_text((REPO / "tools/electron-portable/teebox.sh").read_text())
            required = ("floe2-electron", "floe2-cli", "runtime/electron",
                        "rust/target/release/floe2-web", "rust/target/release/floe-index",
                        "rust/target/release/floe-renderd",
                        "electron/service/target/release/floe-electron-service",
                        "electron/service/target/release/floe-electron-download")
            for name in required:
                target = bundle / name
                target.parent.mkdir(parents=True, exist_ok=True)
                target.symlink_to("/bin/sh")
            (bundle / "runtime/libffmpeg.so").touch()
            (bundle / "verify.sh").write_text('echo verify-only; exit "${TEST_VERIFY_EXIT:-0}"\n')
            (work / "selfcheck").write_text('printf \'selfcheck %s\\n\' "$*" >&2; exit "${TEST_SELFCHECK_EXIT:-0}"\n')
            (work / "view").write_text(
                'printf \'%s\\000\' "$PWD" "$DISPLAY" "$XAUTHORITY" "$FLOE_ELECTRON_X11" "$0" "$@"\n'
                'printf \'%s\\000\' "$FLOE_ELECTRON_BIN" "$FLOE_ELECTRON_SERVICE_BIN" '
                '"$FLOE_ELECTRON_DOWNLOAD_BIN" "$FLOE_INDEX_BIN" "$FLOE_RENDERD_BIN" >&2\n'
                'exit "${TEST_VIEW_EXIT:-0}"\n')
            env = {k: v for k, v in os.environ.items()
                   if not k.startswith(("FLOE_", "NODE_", "ELECTRON_", "TEST_"))}
            env.update(DISPLAY="synthetic:7.0", XAUTHORITY="/synthetic/private auth")
            mock = '''
uname() { case "$1" in -s) echo "${TEST_OS:-Linux}";; -m) echo "${TEST_ARCH:-x86_64}";; esac; }
id() { echo "${TEST_UID:-1000}"; }
ldd() { echo "${TEST_LDD:-mock libraries resolved}"; return "${TEST_LDD_EXIT:-0}"; }
. "$0"
'''

            def call(args, extra=None):
                return subprocess.run(["sh", "-c", mock, str(launcher), *args],
                                      cwd=work, env=dict(env, **(extra or {})),
                                      capture_output=True, text=True, timeout=10)

            args = ["view", "relative 한글.oas", "--root", "/shared folder",
                    "--goto", "-1,2,500", "--multi", "--cell", "x ; $(false)"]
            result = call(args)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout.split("\0")[:-1],
                             [str(work), env["DISPLAY"], env["XAUTHORITY"], "1", *args])
            self.assertEqual(result.stderr.split("\0")[:-1], [str(bundle / name) for name in (
                "runtime/electron", "electron/service/target/release/floe-electron-service",
                "electron/service/target/release/floe-electron-download",
                "rust/target/release/floe-index", "rust/target/release/floe-renderd")])
            self.assertEqual(call(args, {"TEST_VIEW_EXIT": "17"}).returncode, 17)
            checked = call(["--check"])
            self.assertEqual(checked.returncode, 0, checked.stderr)
            self.assertIn("verify-only\n", checked.stdout)
            self.assertEqual(checked.stderr, "selfcheck --adjacent\n")
            self.assertIn("remain untested", checked.stdout)
            self.assertNotIn("\0", checked.stdout)
            for extra in ({"DISPLAY": ""}, {"TEST_OS": "Darwin"}, {"TEST_ARCH": "aarch64"},
                          {"TEST_UID": "0"}, {"FLOE_RENDERD_BIN": ""},
                          {"FLOE_INDEX_BIN": "/old/index"}, {"NODE_OPTIONS": "synthetic"}):
                with self.subTest(extra=extra):
                    denied = call(args, extra)
                    self.assertEqual(denied.returncode, 2)
                    self.assertEqual(denied.stdout, "")
            for extra in ({"TEST_LDD": "libfake => not found"}, {"TEST_LDD_EXIT": "1"},
                          {"TEST_VERIFY_EXIT": "1"}, {"TEST_SELFCHECK_EXIT": "1"}):
                denied = call(["--check"], extra)
                self.assertNotEqual(denied.returncode, 0)
                self.assertNotIn("checks passed", denied.stdout)
            for args in ([], ["index", "source.jb"], ["--check", "extra"]):
                self.assertEqual(call(args).returncode, 2)
            self.assertEqual(call(["--help"], {"DISPLAY": "", "TEST_OS": "Darwin"}).returncode, 0)
            (bundle / required[-1]).unlink()
            self.assertEqual(call(["view", "source.oas"]).returncode, 2)

    def test_bundled_cli_preserves_cwd_and_needs_no_display(self):
        with tempfile.TemporaryDirectory(prefix="floe-bundled-cli-") as td:
            work = Path(td).resolve()
            bundle = work / "bundle 한글"
            binary = bundle / "rust/target/release/floe2-web"
            binary.parent.mkdir(parents=True)
            binary.symlink_to("/bin/sh")
            for name in ("floe-index", "floe-renderd"):
                (binary.parent / name).symlink_to("/bin/sh")
            launcher = bundle / "floe2-cli"
            launcher.write_text((REPO / "tools/electron-portable/cli.sh").read_text())
            (work / "index").write_text('printf \'%s\\000\' "$PWD" "$0" "$@"\n'
                                        'printf \'%s\\000\' "$FLOE_INDEX_BIN" "$FLOE_RENDERD_BIN" >&2\n')
            env = {k: v for k, v in os.environ.items()
                   if not k.startswith("FLOE_") and k != "DISPLAY"}
            args = ["index", "relative 한글.jb", "--level", "1,3", "--jobs", "4", "--lod"]
            result = subprocess.run(["sh", str(launcher), *args], cwd=work, env=env,
                                    capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout.split("\0")[:-1], [str(work), *args])
            self.assertEqual(result.stderr.split("\0")[:-1],
                             [str(binary.parent / name) for name in ("floe-index", "floe-renderd")])
            for key in ("FLOE_INDEX_BIN", "FLOE_RENDERD_BIN"):
                denied = subprocess.run(["sh", str(launcher), *args], cwd=work,
                                        env=dict(env, **{key: ""}), capture_output=True,
                                        text=True, timeout=10)
                self.assertEqual(denied.returncode, 2)
                self.assertEqual(denied.stdout, "")
            (binary.parent / "floe-index").unlink()
            missing = subprocess.run(["sh", str(launcher), *args], cwd=work,
                                     env=env, capture_output=True, text=True, timeout=10)
            self.assertEqual(missing.returncode, 2)
            self.assertEqual(missing.stdout, "")

    def test_x11_host_policy(self):
        subprocess.run(["node", "--test", str(REPO / "electron/x11.test.cjs"),
                        str(REPO / "electron/layout-qa.test.cjs")],
                       check=True, timeout=10, capture_output=True)

    def test_development_launcher_rejects_runtime_injection_before_exec(self):
        for key in ("ELECTRON_RUN_AS_NODE", "NODE_OPTIONS", "NODE_PATH", "NODE_EXTRA_CA_CERTS"):
            env = dict(os.environ, FLOE_ELECTRON_BIN="/bin/sh", **{key: "synthetic"})
            result = subprocess.run(["sh", str(REPO / "tools/run_electron_dev.sh")],
                                    env=env, capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 2)
            self.assertEqual(result.stdout, "")
            self.assertIn("unset", result.stderr)

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
            for key in ("ELECTRON_RUN_AS_NODE", "NODE_OPTIONS", "NODE_PATH", "NODE_EXTRA_CA_CERTS"):
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
            for key in ("ELECTRON_RUN_AS_NODE", "NODE_OPTIONS", "NODE_PATH", "NODE_EXTRA_CA_CERTS"):
                result = call({key: "synthetic"})
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(result.stdout, "")

    def test_explicit_javascript_closure(self):
        listed = app_files()
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

    def test_relocated_cross_host_probe_reads_and_missing_file_failures(self):
        # Exercise the actual packaged loader, not a regex guess about dynamic
        # readFileSync paths. Stop before browser evaluation; no pixel verdict.
        node = shutil.which("node")
        self.assertIsNotNone(node, "Node is required for this development-only gate")
        script = r"""
const path=require('node:path'),vm=require('node:vm');
const {run}=require(process.argv[1]);
const stop=Error('fixture read boundary');let calls=0;
run({window:{show(){},focus(){},webContents:{focus(){}}},parityOnly:true,
  evalOwned:async code=>{new vm.Script(code);calls++;throw stop;}}).then(()=>process.exit(1),e=>{
  if(e===stop&&calls===1)console.log('PROBE_FILE_CLOSURE: OK');
  else if(e.code==='ENOENT')console.log('MISSING_PROBE: '+path.basename(e.path));
  else process.exitCode=1;
});
"""
        probes = ("layout-parity-probe.js", "frame-parity-probe.js",
                  "cross-viewport-probe.js", "frame-fingerprint-probe.js")
        with tempfile.TemporaryDirectory(prefix="floe-probe-closure-") as td:
            work = Path(td).resolve()
            for missing in (None, *probes):
                with self.subTest(missing=missing):
                    bundle = work / (missing or "complete")
                    for name in app_files():
                        if name == "desktop/ui/" + str(missing):
                            continue
                        dest = bundle / name
                        dest.parent.mkdir(parents=True, exist_ok=True)
                        shutil.copyfile(REPO / name, dest)
                    scratch = bundle / "scratch"
                    scratch.mkdir()
                    result = subprocess.run(
                        [node, "-e", script, str(bundle / "electron/layout-qa.cjs")],
                        cwd=bundle, env=dict(os.environ, PATH="", TMPDIR=str(scratch),
                                             FLOE_QA_CROSS_HOST="1"),
                        capture_output=True, text=True, timeout=10,
                    )
                    self.assertEqual(result.returncode, 0, result.stderr)
                    expected = ("MISSING_PROBE: " + missing if missing
                                else "PROBE_FILE_CLOSURE: OK")
                    self.assertEqual(result.stdout.splitlines()[-1], expected)
                    self.assertNotIn("FRAME PARITY OK", result.stdout)


def inspect_bundle(bundle):
    bundle = bundle.resolve(strict=True)
    manifest = json.loads((bundle / "BUNDLE.json").read_text())
    assert manifest["format"] == 1
    assert manifest["product"] == "floe2-electron-comparison"
    assert manifest["electron_fuses"] == {"policy": "floe-electron-fuses-v1", "schema": 1, "wire": "000000001"}
    fuse_file = bundle / "runtime" / (
        "Electron.app/Contents/Frameworks/Electron Framework.framework/Versions/A/Electron Framework"
        if manifest["target"].endswith("-apple-darwin") else "electron")
    sentinel = b"dL7pKGdnNz796PbbjQWNKmHXBZaB9tsX"
    offsets, consumed, carry = [], 0, b""
    with fuse_file.open("rb") as stream:
        while block := stream.read(1024 * 1024):
            data = carry + block
            start = 0
            while (at := data.find(sentinel, start)) >= 0:
                offsets.append(consumed - len(carry) + at + len(sentinel))
                start = at + 1
            consumed += len(block)
            carry = data[-(len(sentinel) - 1):]
        assert len(offsets) == 1, "ambiguous/missing fuse wire"
        stream.seek(offsets[0])
        assert stream.read(11) == b"\x01\x09000000001", "runtime fuse policy mismatch"
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
                 "floe2-teebox", "floe2-cli", "rust/target/release/floe2-web", "electron/x11.cjs",
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
