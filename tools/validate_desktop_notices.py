#!/usr/bin/env python3
"""Check a real built .app and corrupt only a new private copy; no UI starts.

Development-only Python gate. The installed host's diagnostic runs with no PATH,
workers, sources, network, reviewer, browser, or Python runtime dependencies.
"""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile


def check(app, ok):
    result = subprocess.run([str(app / "Contents/MacOS/floe2-desktop"), "--check-notices"],
                            env=dict(os.environ, PATH="", FLOE_INDEX_BIN="/invalid-index",
                                     FLOE_RENDERD_BIN="/invalid-renderd"),
                            capture_output=True, text=True, timeout=30)
    assert (result.returncode == 0) == ok, (result.returncode, result.stdout, result.stderr)
    if ok:
        assert "DESKTOP NOTICES: OK" in result.stdout
    else:
        assert result.stdout == ""


def main():
    if len(sys.argv) != 2:
        raise SystemExit("usage: validate_desktop_notices.py BUILT.app")
    original = Path(sys.argv[1]).resolve(strict=True)
    check(original, True)
    with tempfile.TemporaryDirectory(prefix="floe-desktop-notices-") as td:
        # Copy only the actual diagnostic host and resources, never mutate the
        # source bundle. Workers are deliberately absent from this test copy.
        app = Path(td) / "한국 경로/Floe2.app"
        binaries = app / "Contents/MacOS"
        binaries.mkdir(parents=True)
        shutil.copy2(original / "Contents/MacOS/floe2-desktop", binaries / "floe2-desktop")
        resources = app / "Contents/Resources"
        shutil.copytree(original / "Contents/Resources", resources)
        check(app, True)
        index_path = resources / "NOTICE-INDEX.json"
        raw_index = index_path.read_bytes()
        index = json.loads(raw_index)
        names = {entry["name"] for entry in index["files"]}
        required = {"NOTICES/desktop/Cargo.lock", "NOTICES/desktop/Cargo.toml",
                    "NOTICES/desktop/UPSTREAM-SUPPLEMENT.md"}
        assert required <= names
        assert any("objc2-0.6.4/UPSTREAM-SUPPLEMENT.md" in n for n in names)
        assert any("NotoSansMono-OFL.txt" in n for n in names)
        assert any("COPYRIGHT-library.html" in n for n in names)
        assert len(names) == len(index["files"])
        assert names == {str(p.relative_to(resources)) for p in (resources / "NOTICES").rglob("*")
                         if p.is_file()}
        digest = hashlib.sha1(raw_index).hexdigest()
        assert digest.encode() in (binaries / "floe2-desktop").read_bytes()
        entry = next(e for e in index["files"] if e["bytes"] > 0)
        notice = resources / entry["name"]
        data = notice.read_bytes()
        notice.write_bytes(bytes([data[0] ^ 1]) + data[1:])
        check(app, False)
        notice.unlink()
        check(app, False)
        outside = Path(td) / "same-content-outside"
        outside.write_bytes(data)
        notice.symlink_to(outside)
        check(app, False)
        notice.unlink()
        notice.write_bytes(data)
        index_path.write_bytes(raw_index + b"\n")
        check(app, False)
        index_path.write_bytes(raw_index)
        check(app, True)
        print(f"DESKTOP NOTICE GATE: OK ({len(names)} files; relocation, missing, tamper, symlink)")


if __name__ == "__main__":
    main()
