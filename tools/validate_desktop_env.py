"""Small deterministic gate for the read-only desktop environment inventory."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parent / "audit_desktop_env.sh"


class DesktopInventoryTests(unittest.TestCase):
    def test_fake_rhel_runtime_without_devel_packages(self):
        with tempfile.TemporaryDirectory(prefix="floe-desktop-inventory-") as tmp:
            bindir = Path(tmp)
            for name, body in {
                "uname": 'case "$1" in -s) echo Linux;; -m) echo x86_64;; *) exit 2;; esac',
                "getconf": 'echo "glibc 2.28"',
                "rpm": 'case "$4" in webkit2gtk3) echo 2.40.5-1.el8.x86_64;; gtk3) echo 3.22.30-12.el8.x86_64;; *) exit 1;; esac',
                "pkg-config": 'exit 1',
            }.items():
                path = bindir / name
                path.write_text("#!/bin/sh\n" + body + "\n", encoding="utf-8")
                path.chmod(0o700)
            env = dict(os.environ, PATH=f"{bindir}:/usr/bin:/bin",
                       DISPLAY="private-host:12345", WAYLAND_DISPLAY="")
            result = subprocess.run(["/bin/sh", str(SCRIPT)], env=env,
                                    text=True, capture_output=True, check=True)
            self.assertIn("libc=glibc 2.28", result.stdout)
            self.assertIn("rpm_webkit2gtk3=2.40.5-1.el8.x86_64", result.stdout)
            self.assertIn("devel_webkit2gtk-4.0=not_detected", result.stdout)
            self.assertIn("display=set", result.stdout)
            self.assertIn("result=inventory_only", result.stdout)
            self.assertNotIn("private-host", result.stdout + result.stderr)
            self.assertNotIn(tmp, result.stdout + result.stderr)
            self.assertEqual(result.stderr, "")

    def test_help_and_unknown_arguments(self):
        help_result = subprocess.run(["/bin/sh", str(SCRIPT), "--help"],
                                     text=True, capture_output=True, check=True)
        self.assertNotIn("rpm_", help_result.stdout)
        invalid = subprocess.run(["/bin/sh", str(SCRIPT), "--install"],
                                 text=True, capture_output=True)
        self.assertEqual(invalid.returncode, 2)
        self.assertEqual(invalid.stdout, "")


if __name__ == "__main__":
    unittest.main()
