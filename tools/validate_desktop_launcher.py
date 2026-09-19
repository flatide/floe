"""No GUI, workers, indexing or user files: exercise the actual shell launchers."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

TOOLS = Path(__file__).resolve().parent


class DesktopLauncherTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="floe-desktop-launch-")
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name) / "한국 repo with spaces"
        self.caller = Path(self.tmp.name) / "caller"
        self.caller.mkdir()
        (self.root / "tools").mkdir(parents=True)
        for name in ("run_desktop_macos_dev.sh", "build_desktop_macos_dev.sh"):
            shutil.copyfile(TOOLS / name, self.root / "tools" / name)
        self.fake = self.root / "fakebin"
        self.fake.mkdir()
        self.write_executable(self.fake / "uname", 'printf "%s\\n" "${TEST_OS-Darwin}"')
        self.env = dict(os.environ, PATH=f"{self.fake}:/usr/bin:/bin")
        for name in ("FLOE_INDEX_BIN", "FLOE_RENDERD_BIN"):
            self.env.pop(name, None)
        for profile in ("debug", "release"):
            self.write_executable(self.root / f"desktop/target/{profile}/floe2-desktop",
                                  'printf "%s\\n" "$PWD" "$0" "$FLOE_INDEX_BIN" "$FLOE_RENDERD_BIN" "$@"\nexit "${TEST_EXIT-0}"')

    def write_executable(self, path, body):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("#!/bin/sh\nset -eu\n" + body + "\n", encoding="utf-8")
        path.chmod(0o700)

    def run_script(self, *args, script="run_desktop_macos_dev.sh"):
        return subprocess.run(["/bin/sh", str(self.root / "tools" / script), *args],
                              cwd=self.caller, env=self.env, text=True, capture_output=True)

    def test_relative_paths_spaces_and_options_are_not_rewritten(self):
        args = ["view", "칩 하나.oas", "--root", "../mask files", "--jobs", "2"]
        result = self.run_script(*args)
        self.assertEqual(result.returncode, 0, result.stderr)
        lines = result.stdout.splitlines()
        self.assertEqual(lines[0], str(self.caller.resolve()))
        self.assertEqual(lines[1], str(self.root.resolve() / "desktop/target/debug/floe2-desktop"))
        self.assertEqual(lines[2:4], [str(self.root.resolve() / f"rust/target/release/{n}")
                                    for n in ("floe-index", "floe-renderd")])
        self.assertEqual(lines[4:], args)

    def test_release_and_exit_status(self):
        self.env["TEST_EXIT"] = "17"
        result = self.run_script("--release", "view", "./oas")
        self.assertEqual(result.returncode, 17)
        self.assertIn("/target/release/floe2-desktop\n", result.stdout)
        self.assertEqual(result.stdout.splitlines()[4:], ["view", "./oas"])

    def test_explicit_overrides_are_not_replaced(self):
        for value in ("", "../missing binary"):
            self.env.update(FLOE_INDEX_BIN=value, FLOE_RENDERD_BIN=value)
            result = self.run_script("view", "a.oas")
            self.assertEqual(result.returncode, 0)
            self.assertEqual(result.stdout.splitlines()[2:4], [value, value])

    def test_missing_build_is_actionable(self):
        (self.root / "desktop/target/debug/floe2-desktop").unlink()
        result = self.run_script()
        self.assertEqual(result.returncode, 1)
        self.assertIn("build_desktop_macos_dev.sh", result.stderr)
        self.assertEqual(result.stdout, "")

    def test_non_macos_is_explicit(self):
        self.env["TEST_OS"] = "Linux"
        result = self.run_script()
        self.assertEqual(result.returncode, 2)
        self.assertIn("macOS preview only", result.stderr)

    def test_packager_profiles_and_fresh_output(self):
        (self.root / "rust").mkdir()
        (self.root / "docs").mkdir()
        (self.root / "docs/WEBUI_DESKTOP.ko.md").write_text("preview", encoding="utf-8")
        (self.root / "desktop/Info.plist").write_text("plist", encoding="utf-8")
        (self.root / "desktop/NOTICES.md").write_text("notices", encoding="utf-8")
        self.write_executable(self.fake / "cargo", '''
case "$PWD" in
    */rust)
        [ "$*" = 'build --release --offline --locked -p floe-index -p floe-renderd' ]
        mkdir -p target/release
        printf index > target/release/floe-index
        printf renderd > target/release/floe-renderd ;;
    */desktop)
        case "$*" in
            'build --offline --locked') profile=debug ;;
            'build --release --offline --locked') profile=release ;;
            *) exit 42 ;;
        esac
        printf '%s' "$profile" > "target/$profile/floe2-desktop" ;;
    *) exit 43 ;;
esac''')
        self.write_executable(self.fake / "plutil", '[ "$1" = -lint ]\n[ -f "$2" ]')
        outputs = []
        for args, expected in (([], "debug"), (["--release"], "release"), ([], "debug")):
            result = self.run_script(*args, script="build_desktop_macos_dev.sh")
            self.assertEqual(result.returncode, 0, result.stderr)
            app = Path(result.stdout.strip()).resolve()
            self.assertTrue(app.is_relative_to(self.root.resolve()))
            self.assertEqual((app / "Contents/MacOS/floe2-desktop").read_text(), expected)
            self.assertEqual((app / "Contents/MacOS/floe-renderd").read_text(), "renderd")
            self.assertTrue((app / "Contents/Resources/DESKTOP-NOTICES.md").is_file())
            outputs.append(app)
        self.assertEqual(len(set(outputs)), 3)

    def test_packager_rejects_extra_arguments(self):
        result = self.run_script("--release", "--unknown", script="build_desktop_macos_dev.sh")
        self.assertEqual(result.returncode, 2)
        self.assertIn("usage:", result.stderr)


if __name__ == "__main__":
    unittest.main()
