#!/usr/bin/env python3
"""Development-only packager failure/cancel gates; runtime stays Rust-only.

Optional archive argument checks a REAL package made by make_web_portable.sh.
Synthetic tool doubles test refusal/cleanup, not Linux runtime acceptance.
"""
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import signal
import subprocess
import sys
import tarfile
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
PACKAGER = ROOT / "rust/target/release/floe-web-packager"


def script(path, body):
    path.write_text("#!" + str(Path(sys.executable).resolve()) + "\n" + body)
    path.chmod(0o700)
    return path


def call(repo, env, out, *args, ok=False):
    result = subprocess.run([str(PACKAGER), str(repo), "--out", str(out),
                             "--target", "x86_64-unknown-linux-musl", *args],
                            env=env, capture_output=True, text=True, timeout=20)
    assert (result.returncode == 0) == ok, (result.stdout, result.stderr)
    assert not list(out.parent.glob(".floe-web-stage-*")), "failed/finished build leaked a stage"
    return result


def fixture(work):
    repo = work / "source ZIP 경로"
    crate = repo / "rust/vendor/fixture-1.0.0"
    crate.mkdir(parents=True)
    (crate / "Cargo.toml").write_text('[package]\nname="fixture"\nversion="1.0.0"\n')
    (crate / "LICENSE").write_text("synthetic notice, not a real dependency license\n")
    (repo / "rust/Cargo.lock").write_text("synthetic lock\n")
    (repo / "rust/Cargo.toml").write_text("synthetic workspace\n")
    font = repo / "rust/render-core/assets"
    font.mkdir(parents=True)
    (font / "NotoSansMono-OFL.txt").write_text("synthetic font notice\n")
    sysroot = work / "toolchain"
    (sysroot / "lib/rustlib/x86_64-unknown-linux-musl/lib").mkdir(parents=True)
    docs = sysroot / "share/doc/rust"
    (docs / "licenses").mkdir(parents=True)
    for name in ("COPYRIGHT.html", "COPYRIGHT-library.html", "licenses/MIT.txt"):
        (docs / name).write_text("synthetic toolchain notice\n")
    packages = [{"id": name, "name": name, "source": None}
                for name in ("floe-app", "floe-index", "floe-renderd")]
    packages.append({"id": "fixture", "name": "fixture", "source": "registry+fixture",
                     "manifest_path": str(crate / "Cargo.toml")})
    nodes = [{"id": p["id"], "deps": [] if p["id"] == "fixture" else
              [{"pkg": "fixture", "dep_kinds": [{"kind": None}]}]} for p in packages]
    metadata = json.dumps({"packages": packages, "resolve": {"nodes": nodes}})
    rustc = script(work / "rustc", "import sys\nprint(%r if '--print' in sys.argv else 'rustc synthetic fixture')\n" % str(sysroot))
    marker = work / "build.marker"
    cargo = script(work / "cargo", """import os,sys,time
from pathlib import Path
if 'metadata' in sys.argv:
    print(%r)
    sys.exit(0)
assert '--offline' in sys.argv and '--locked' in sys.argv
assert os.environ['CARGO_NET_OFFLINE'] == 'true'
Path(%r).write_text(str(os.getpid()))
if os.environ.get('PACKAGER_CASE') == 'cancel':
    while True: time.sleep(.1)
if os.environ.get('PACKAGER_CASE') == 'bad-elf':
    target = Path(os.environ['CARGO_TARGET_DIR']) / 'x86_64-unknown-linux-musl/release'
    target.mkdir(parents=True)
    for name in ('floe2-web','floe-index','floe-renderd'):
        (target / name).write_bytes(b'not an ELF')
    sys.exit(0)
sys.exit(7)
""" % (metadata, str(marker)))
    env = dict(os.environ, FLOE_PACKAGER_RUSTC=str(rustc), FLOE_PACKAGER_CARGO=str(cargo),
               CARGO_TARGET_DIR=str(work / "build"), FLOE_SRC_REV="private-fixture")
    return repo, env, marker, crate


def inspect_archive(path, work):
    with tarfile.open(path, "r:gz") as archive:
        members = archive.getmembers()
        assert members and all(m.isdir() or m.isfile() for m in members)
        assert all(Path(m.name).parts[0] == "floe2-web-portable" and
                   ".." not in Path(m.name).parts and not Path(m.name).is_absolute() for m in members)
        target = work / "relocated 경로 with spaces"
        for member in members:
            output = target / member.name
            if member.isdir():
                output.mkdir(parents=True, exist_ok=True)
            else:
                output.parent.mkdir(parents=True, exist_ok=True)
                with archive.extractfile(member) as src, output.open("wb") as dst:
                    shutil.copyfileobj(src, dst)
                output.chmod(member.mode)
    bundle = target / "floe2-web-portable"
    listed = {}
    for line in (bundle / "SHA256SUMS").read_text().splitlines():
        digest, name = line.split("  ", 1)
        assert name not in listed and not Path(name).is_absolute() and ".." not in Path(name).parts
        listed[name] = digest
    files = {str(p.relative_to(bundle)) for p in bundle.rglob("*") if p.is_file()}
    assert set(listed) == files - {"SHA256SUMS"}
    for name, digest in listed.items():
        assert hashlib.sha256((bundle / name).read_bytes()).hexdigest() == digest, name
    index_bytes = (bundle / "NOTICE-INDEX.json").read_bytes()
    index_id = hashlib.sha1(index_bytes).hexdigest()
    index = json.loads(index_bytes)
    assert index["format"] == 1 and len(index_bytes) <= 2 * 1024 * 1024
    assert {f["name"] for f in index["files"]} == {n for n in listed if n.startswith("NOTICES/")}
    for f in index["files"]:
        data = (bundle / f["name"]).read_bytes()
        assert len(data) == f["bytes"]
        at = 0
        for page in f["pages"]:
            assert page["offset"] == at and 0 <= page["bytes"] <= 65536
            raw = data[at:at + page["bytes"]]
            assert hashlib.sha1(raw).hexdigest() == page["digest"]
            if f["encoding"] == "utf8":
                raw.decode("utf8")
            else:
                assert f["encoding"] == "hex"
            at += len(raw)
        assert at == len(data)
    assert index_id.encode() in (bundle / "floe2-web").read_bytes(), "catalogue ID not embedded in the app"
    assert {p.name for p in bundle.iterdir() if p.is_file() and p.read_bytes()[:4] == b'\x7fELF'} == {"floe2-web", "floe-index", "floe-renderd"}
    assert not any(p.suffix in (".py", ".so") for p in bundle.rglob("*"))
    env = dict(os.environ, FLOE_INDEX_BIN="/invalid-index", FLOE_RENDERD_BIN="/invalid-renderd")
    subprocess.run(["sh", str(bundle / "verify.sh")], env=env, check=True, capture_output=True)
    if platform.system() == "Linux" and platform.machine() == "x86_64":
        subprocess.run([str(bundle / "floe2-web"), "selfcheck", "--adjacent"], env=env,
                       check=True, capture_output=True, timeout=20)
    (bundle / "README.txt").write_text("intentional private-copy corruption\n")
    assert subprocess.run(["sh", str(bundle / "verify.sh")], capture_output=True).returncode != 0


def main():
    with tempfile.TemporaryDirectory(prefix="floe-web-portable-test-") as td:
        work = Path(td)
        repo, env, marker, crate = fixture(work)
        out = work / "new.tar.gz"
        out.write_bytes(b"existing")
        call(repo, env, out)
        assert out.read_bytes() == b"existing" and not marker.exists()
        out.unlink()
        out.symlink_to(work / "absent")
        call(repo, env, out)
        assert out.is_symlink() and not marker.exists()
        out.unlink()
        for args in (("--jobs", "0"), ("--jobs", "17"), ("--force", "1")):
            call(repo, env, out, *args)
            assert not marker.exists()
        license_file = crate / "LICENSE"
        license_file.rename(crate / "hidden-notice")
        result = call(repo, env, out)
        assert "missing notice" in result.stderr, (result.returncode, result.stdout, result.stderr)
        assert not marker.exists()
        (crate / "hidden-notice").rename(license_file)
        result = call(repo, env, out)
        assert "failed" in result.stderr, (result.returncode, result.stdout, result.stderr)
        assert marker.exists() and not out.exists()
        result = call(repo, dict(env, PACKAGER_CASE="bad-elf"), out)
        assert "ELF" in result.stderr, (result.returncode, result.stdout, result.stderr)
        marker.unlink()
        p = subprocess.Popen([str(PACKAGER), str(repo), "--out", str(out), "--target",
                              "x86_64-unknown-linux-musl"], env=dict(env, PACKAGER_CASE="cancel"),
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            until = time.monotonic() + 10
            while not marker.exists():
                assert p.poll() is None and time.monotonic() < until
                time.sleep(.01)
            p.send_signal(signal.SIGTERM)
            stdout, stderr = p.communicate(timeout=4)
            assert p.returncode == 143, (stdout, stderr)
            assert not out.exists() and not list(work.glob(".floe-web-stage-*"))
            try:
                os.kill(int(marker.read_text()), 0)
            except ProcessLookupError:
                pass
            else:
                raise AssertionError("packager build child not reaped")
        finally:
            if p.poll() is None:
                p.kill()
                p.wait()
        if len(sys.argv) > 1:
            inspect_archive(Path(sys.argv[1]), work)
    print("WEB PORTABLE: ALL OK (no-clobber, notice/build/ELF refusal, cancellation/reap/cleanup" +
          (", real archive/hash/relocation/corruption" if len(sys.argv) > 1 else "") + ")")


if __name__ == "__main__":
    main()
