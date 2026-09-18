#!/usr/bin/env python3
"""Source-only lock/vendor gate; no GUI, runtime Python or network access."""
import hashlib
import json
from pathlib import Path
import tomllib

ROOT = Path(__file__).resolve().parents[1]


def packages(path):
    return {(p["name"], p["version"]): p for p in
            tomllib.loads(path.read_text())["package"] if "source" in p}


def main():
    shared = packages(ROOT / "rust/Cargo.lock")
    desktop = packages(ROOT / "desktop/Cargo.lock")
    vendor = ROOT / "desktop/vendor"
    new = set()
    for (name, version), package in desktop.items():
        path = vendor / f"{name}-{version}"
        manifest = tomllib.loads((path / "Cargo.toml").read_text())["package"]
        assert (manifest["name"], manifest["version"]) == (name, version)
        checksum = json.loads((path / ".cargo-checksum.json").read_text())
        assert checksum["package"] == package["checksum"]
        if (name, version) in shared:
            assert shared[name, version]["checksum"] == package["checksum"]
            assert path.is_symlink() and path.resolve().parent == ROOT / "rust/vendor"
        else:
            new.add(name)
            assert not path.is_symlink()
            # Verify every original new-crate file, not only the manifest.
            for filename, digest in checksum["files"].items():
                assert hashlib.sha256((path / filename).read_bytes()).hexdigest() == digest
            assert manifest["license"]
    assert new == {"bitflags", "block2", "dispatch2", "objc2", "objc2-encode",
                   "objc2-foundation", "objc2-app-kit", "objc2-core-foundation", "objc2-web-kit"}
    # No dependency version drift for shared names, including multi-version syn.
    assert not ({n for n, _ in desktop} & {n for n, _ in shared} & new)
    assert "MIT supplement" in (ROOT / "desktop/NOTICES.md").read_text()
    print(f"DESKTOP VENDOR: ALL OK ({len(desktop)} locked registry packages; {len(new)} new; shared sources unchanged)")


if __name__ == "__main__":
    main()
