#!/usr/bin/env python3
"""Development-only compiled-index integration fixture.

prepare NEW_DIR: print build metadata for a new synthetic installation.
run APP DIR SOURCE: copy the native test app/tools into that fixture and test
identity, all text pages, corruption, auth, selfcheck and graceful shutdown.
APP must be compiled with the digest/revision printed by prepare. Never used
by the production runtime; no source OASIS is indexed or modified here.
"""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

from validate_web_cli import Client, read_json, wait

ROOT = Path(__file__).resolve().parents[1]
REVISION = "notices-native-test"


def prepare(work):
    work.mkdir(mode=0o700)  # Refuse any existing file/directory/link.
    notices = work / "NOTICES"
    notices.mkdir()
    (notices / "INVENTORY.txt").write_text("Synthetic notice QA fixture, not distribution terms.\n")
    (notices / "long.html").write_text("<script>not_executed()</script>\n" + "한글 original source\n" * 10000)
    (notices / "binary.bin").write_bytes(bytes(range(256)))
    for i in range(67):
        (notices / ("synthetic-%02d.txt" % i)).write_text("Synthetic original %d\n" % i)
    host = next(line.split(": ", 1)[1] for line in subprocess.check_output(
        ["rustc", "-vV"], text=True).splitlines() if line.startswith("host: "))
    index = dict(format=1, source_revision=REVISION, target=host, files=[])
    for path in sorted(notices.iterdir()):
        raw = path.read_bytes()
        try:
            raw.decode("utf8")
            encoding = "utf8"
        except UnicodeDecodeError:
            encoding = "hex"
        pages = []
        at = 0
        while True:
            end = min(at + 65536, len(raw))
            if encoding == "utf8":
                while end < len(raw) and raw[end] & 0xc0 == 0x80:
                    end -= 1
            part = raw[at:end]
            pages.append(dict(offset=at, bytes=end-at, digest=hashlib.sha1(part).hexdigest()))
            at = end
            if at == len(raw):
                break
        index["files"].append(dict(name=str(path.relative_to(work)), bytes=len(raw),
                                   encoding=encoding, pages=pages))
    data = json.dumps(index, ensure_ascii=False, separators=(",", ":")).encode()
    (work / "NOTICE-INDEX.json").write_bytes(data)
    print(json.dumps(dict(digest=hashlib.sha1(data).hexdigest(), source_revision=REVISION, target=host)))


def run(app, work, source):
    index_path = work / "NOTICE-INDEX.json"
    original = index_path.read_bytes()
    index = json.loads(original)
    assert index["source_revision"] == REVISION and len(index["files"]) == 70
    for name, src in (("floe2-web", app), ("floe-index", ROOT / "rust/target/release/floe-index"),
                      ("floe-renderd", ROOT / "rust/target/release/floe-renderd")):
        dest = work / name
        assert not dest.exists() and not dest.is_symlink(), str(dest)
        shutil.copy2(src, dest)
    runtime = work / "runtime"
    runtime.mkdir(mode=0o700)
    env = dict(os.environ, PATH="", TMPDIR=str(runtime), FLOE_FILL_EDIT="",
               FLOE_INDEX_BIN=str(work / "floe-index"), FLOE_RENDERD_BIN=str(work / "floe-renderd"),
               FLOE_FIREFOX_BIN=str(work / "absent-firefox"))
    binary = work / "floe2-web"

    def command(*args, code=0):
        result = subprocess.run([str(binary), *args], env=env, capture_output=True, text=True, timeout=15)
        assert result.returncode == code, (result.stdout, result.stderr)
        return json.loads(result.stdout)

    meta = command("selfcheck", "--metadata-only")
    assert meta["notice_index"] == hashlib.sha1(original).hexdigest()
    assert meta["source_revision"] == REVISION and meta["target"] == index["target"]
    checked = command("selfcheck", "--adjacent")
    assert checked["ok"] and checked["checks"][-1]["name"] == "portable_notice_index"
    assert checked["checks"][-1]["scope"] == "index_only_not_all_file_chunks"
    for corrupt in (False, True):
        if corrupt:
            index_path.write_bytes(b"{}")
        session = work / ("session-bad.json" if corrupt else "session.json")
        proc = subprocess.Popen([str(binary), "view", str(source), "--no-open", "--session-file", str(session),
                                 "--jobs", "1", "--raster-jobs", "1"], env=env,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            client = Client(wait(lambda: read_json(session), proc))
            client.call("GET", "/api/v1/about/notices/0", code=401)
            client.login()
            info = client.call("GET", "/api/v1/about")
            assert info["notices"]["status"] == ("unavailable" if corrupt else "available")
            if corrupt:
                client.call("GET", "/api/v1/about/notices/0", code=503)
                assert command("selfcheck", "--metadata-only")["checks"] == []
                assert not command("selfcheck", "--adjacent", code=1)["ok"]
            else:
                start, files = 0, []
                while start is not None:
                    page = client.call("GET", "/api/v1/about/notices/" + str(start))
                    files.extend(page["files"])
                    start = page["next"]
                assert [f["name"] for f in files] == [f["name"] for f in index["files"]]
                for f in files:
                    rebuilt = bytearray()
                    for p in range(f["pages"]):
                        chunk = client.call("GET", "/api/v1/about/notices/%d/%d" % (f["id"], p))
                        assert chunk["offset"] == len(rebuilt) and chunk["bytes"] <= 65536
                        rebuilt.extend(chunk["text"].encode() if f["encoding"] == "utf8" else bytes.fromhex(chunk["text"]))
                    assert rebuilt == (work / f["name"]).read_bytes()
                target = work / files[0]["name"]
                before = target.read_bytes()
                try:
                    target.write_bytes(b"X" * len(before))
                    reply = client.call("GET", "/api/v1/about/notices/0/0", code=409)
                    assert "text" not in reply
                finally:
                    target.write_bytes(before)
                client.call("POST", "/api/v1/about/notices/0/0", {}, code=405)
            assert client.call("GET", "/api/v1/operations")["last_seq"] == "0"
            client.call("DELETE", "/api/v1/session", code=204)
            out, err = proc.communicate(timeout=15)
            assert proc.returncode == 0, (out, err)
            assert not session.exists()
        finally:
            if proc.poll() is None:
                proc.terminate()
                proc.communicate(timeout=15)
            index_path.write_bytes(original)
    print("COMPILED NOTICES: ALL OK (70 original files, every UTF8/hex chunk, index-only selfcheck, corruption/auth, no source operation, cleanup)")


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "prepare":
        prepare(Path(sys.argv[2]).absolute())
    elif len(sys.argv) == 5 and sys.argv[1] == "run":
        run(Path(sys.argv[2]).resolve(), Path(sys.argv[3]).resolve(), Path(sys.argv[4]).resolve())
    else:
        raise SystemExit("prepare NEW_DIR | run TEST_APP FIXTURE_DIR SOURCE")
