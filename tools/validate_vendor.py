#!/usr/bin/env python3
"""Check vendor bytes AND Git inclusion; ignored local files must not hide holes."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


def audit(root):
    root = root.resolve()
    tracked = None
    if (root / ".git").exists():
        output = subprocess.check_output(
            ["git", "ls-files", "-z"], cwd=root)
        tracked = {name.decode("utf-8") for name in output.split(b"\0") if name}
    errors = []
    manifests = files = 0
    for directory in (root / "rust/vendor", root / "desktop/vendor"):
        if not directory.is_dir():
            errors.append(f"missing vendor directory: {directory.relative_to(root)}")
            continue
        checksums = sorted(directory.glob("*/.cargo-checksum.json"))
        if not checksums:
            errors.append(f"no vendor manifests: {directory.relative_to(root)}")
        for manifest in checksums:
            manifests += 1
            # Desktop shared packages are symlinks into rust/vendor. Check both
            # the symlink and its canonical manifest/files in the Git index.
            if tracked is not None:
                required = [manifest.resolve()]
                if manifest.parent.is_symlink():
                    required.append(manifest.parent)
                for path in required:
                    name = path.relative_to(root).as_posix()
                    if name not in tracked:
                        errors.append(f"not tracked by Git: {name}")
            for name, expected in json.loads(manifest.read_text())["files"].items():
                files += 1
                path = manifest.parent / name
                relative = path.resolve().relative_to(root).as_posix()
                if tracked is not None and relative not in tracked:
                    errors.append(f"not tracked by Git: {relative}")
                if not path.is_file():
                    errors.append(f"missing file: {relative}")
                elif hashlib.sha256(path.read_bytes()).hexdigest() != expected:
                    errors.append(f"checksum mismatch: {relative}")
    return sorted(set(errors)), manifests, files, tracked is not None


class AuditTests(unittest.TestCase):
    def test_ignored_missing_changed_and_exported_files(self):
        with tempfile.TemporaryDirectory(prefix="floe-vendor-gate-") as tmp:
            root = Path(tmp)
            for area in ("rust", "desktop"):
                crate = root / area / "vendor/example"
                (crate / "tests/data").mkdir(parents=True)
                (crate / "tests/data/vector.blb").write_bytes(b"original")
                (crate / ".cargo-checksum.json").write_text(json.dumps({
                    "files": {"tests/data/vector.blb":
                              hashlib.sha256(b"original").hexdigest()}}))
            # A source archive has no Git dependency but still checks all bytes.
            self.assertEqual(audit(root), ([], 2, 2, False))
            subprocess.run(["git", "init", "-q", str(root)], check=True)
            (root / ".gitignore").write_text("data/\n")
            subprocess.run(["git", "add", "."], cwd=root, check=True)
            errors, _, _, tracked = audit(root)
            self.assertTrue(tracked)
            self.assertEqual(len(errors), 2)
            self.assertTrue(all("not tracked by Git" in e for e in errors))
            subprocess.run(["git", "add", "-f", "rust/vendor", "desktop/vendor"],
                           cwd=root, check=True)
            self.assertEqual(audit(root)[0], [])
            vector = root / "rust/vendor/example/tests/data/vector.blb"
            vector.write_bytes(b"changed")
            self.assertIn("checksum mismatch", audit(root)[0][0])
            vector.unlink()
            self.assertIn("missing file", audit(root)[0][0])


def main():
    result = unittest.TextTestRunner().run(
        unittest.defaultTestLoader.loadTestsFromTestCase(AuditTests))
    if not result.wasSuccessful():
        return 1
    errors, manifests, files, tracked = audit(ROOT)
    if errors:
        for error in errors:
            print(f"VENDOR FAIL: {error}", file=sys.stderr)
        return 1
    mode = "Git index + SHA-256" if tracked else "source archive SHA-256"
    print(f"VENDOR: ALL OK ({manifests} manifests; {files} entries; {mode})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
