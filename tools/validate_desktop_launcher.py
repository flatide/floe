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
        for name in ("FLOE_INDEX_BIN", "FLOE_RENDERD_BIN", "CARGO_BUILD_TARGET"):
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

    def test_checked_app_receipt_keeps_cwd_and_matched_workers(self):
        app = self.root / "desktop/target/macos-dev.checked/Floe2.app"
        binary = app / "Contents/MacOS/floe2-desktop"
        self.write_executable(binary,
                              'printf "%s\\n" "$PWD" "$0" "$FLOE_INDEX_BIN" "$FLOE_RENDERD_BIN" "$@"')
        resources = app / "Contents/Resources"
        resources.mkdir()
        (resources / "NOTICE-INDEX.json").write_text("index")
        receipt = self.root / "desktop/target/macos-preview-debug.path"
        receipt.write_text(str(app.resolve()) + "\n")
        result = self.run_script("view", "relative.oas")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.splitlines(), [str(self.caller.resolve()),
                         str(binary.resolve()), str(binary.resolve().parent / "floe-index"),
                         str(binary.resolve().parent / "floe-renderd"), "view", "relative.oas"])
        self.env.update(FLOE_INDEX_BIN="", FLOE_RENDERD_BIN="../override")
        self.assertEqual(self.run_script().stdout.splitlines()[2:4], ["", "../override"])
        for bad in ("/other/Floe2.app", str(app.resolve()) + "\nother",
                    str(app.resolve()).replace("macos-dev.checked", "macos-dev../outside"),
                    str(app.resolve()).replace("macos-dev.checked", "macos-dev.missing"),
                    "x" * 8192):
            receipt.write_text(bad)
            result = self.run_script()
            self.assertEqual(result.returncode, 1)
            self.assertEqual(result.stdout, "")
            self.assertIn("Invalid/stale", result.stderr)
        receipt.unlink()
        receipt.symlink_to(self.root / "absent")
        self.assertEqual(self.run_script().returncode, 1)

    def test_packager_profiles_and_fresh_output(self):
        (self.root / "rust").mkdir(exist_ok=True)
        (self.root / "docs").mkdir()
        (self.root / "docs/WEBUI_DESKTOP.ko.md").write_text("preview", encoding="utf-8")
        (self.root / "desktop/Info.plist").write_text("plist", encoding="utf-8")
        (self.root / "desktop/NOTICES.md").write_text("notices", encoding="utf-8")
        self.write_executable(self.fake / "rustc", 'exit 0')
        self.write_executable(self.root / "rust/target/release/floe-web-packager", '''
if [ "$2" = --desktop-receipt ]; then
    [ "$#" = 4 ]
    [ "${TEST_RECEIPT_FAIL-0}" = 0 ] || exit 34
    printf '%s\\n' "$4" > "$1/desktop/target/macos-preview-$3.path"
    exit 0
fi
[ "$2" = --desktop-notices ]
[ ! -e "$3" ]
[ "${TEST_NOTICE_FAIL-0}" = 0 ] || exit 33
mkdir -p "$3/NOTICES"
printf original > "$3/NOTICES/fixture.txt"
printf index > "$3/NOTICE-INDEX.json"
printf 'source_revision=test-revision\\nnotice_index_sha1=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\\n'
''')
        self.write_executable(self.fake / "cargo", '''
case "$PWD" in
    */rust)
        [ "$*" = "build --release --offline --locked -p floe-index -p floe-renderd -p floe-web-packager --target-dir $PWD/target" ]
        mkdir -p target/release
        printf index > target/release/floe-index
        printf renderd > target/release/floe-renderd ;;
    */desktop)
        case "$*" in
            "build --offline --locked --target-dir $PWD/target") profile=debug ;;
            "build --release --offline --locked --target-dir $PWD/target") profile=release ;;
            *) exit 42 ;;
        esac
        [ "$FLOE_SRC_REV" = test-revision ]
        [ "$FLOE_NOTICE_INDEX_SHA1" = aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa ]
        printf '#!/bin/sh\\n# %s\\n[ "$1" = --check-notices ]\\nexit "${TEST_CHECK_FAIL-0}"\\n' "$profile" > "target/$profile/floe2-desktop"
        chmod +x "target/$profile/floe2-desktop" ;;
    *) exit 43 ;;
esac''')
        self.write_executable(self.fake / "plutil", '[ "$1" = -lint ]\n[ -f "$2" ]')
        outputs = []
        for args, expected in (([], "debug"), (["--release"], "release"), ([], "debug")):
            result = self.run_script(*args, script="build_desktop_macos_dev.sh")
            self.assertEqual(result.returncode, 0, result.stderr)
            app = Path(result.stdout.strip()).resolve()
            self.assertTrue(app.is_relative_to(self.root.resolve()))
            self.assertIn(f"# {expected}\n", (app / "Contents/MacOS/floe2-desktop").read_text())
            self.assertEqual((app / "Contents/MacOS/floe-renderd").read_text(), "renderd")
            self.assertTrue((app / "Contents/Resources/DESKTOP-NOTICES.md").is_file())
            self.assertEqual((app / "Contents/Resources/NOTICES/fixture.txt").read_text(), "original")
            self.assertEqual((self.root / f"desktop/target/macos-preview-{expected}.path").read_text(),
                             str(app) + "\n")
            outputs.append(app)
        self.assertEqual(len(set(outputs)), 3)
        receipt = self.root / "desktop/target/macos-preview-debug.path"
        previous = receipt.read_text()
        for setting in ("TEST_NOTICE_FAIL", "TEST_CHECK_FAIL", "TEST_RECEIPT_FAIL"):
            self.env[setting] = "1"
            result = self.run_script(script="build_desktop_macos_dev.sh")
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(result.stdout, "")
            self.assertEqual(receipt.read_text(), previous)
            del self.env[setting]

    def test_packager_rejects_cross_target(self):
        self.env["CARGO_BUILD_TARGET"] = "x86_64-unknown-linux-musl"
        result = self.run_script(script="build_desktop_macos_dev.sh")
        self.assertEqual(result.returncode, 2)
        self.assertIn("unset CARGO_BUILD_TARGET", result.stderr)

    def test_packager_rejects_extra_arguments(self):
        result = self.run_script("--release", "--unknown", script="build_desktop_macos_dev.sh")
        self.assertEqual(result.returncode, 2)
        self.assertIn("usage:", result.stderr)


if __name__ == "__main__":
    unittest.main()
