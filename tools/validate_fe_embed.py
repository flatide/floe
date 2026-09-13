#!/usr/bin/env python3
"""Native flateyes metadata CLI versus the unchanged Python byte/parser oracle.

All files are generated privately. Python/Pillow are test dependencies only;
the Rust command has an empty PATH and invalid native-worker overrides.
"""
import io
import json
import os
from pathlib import Path
import random
import resource
import signal
import struct
import subprocess
import sys
import tempfile
import time
import zlib

from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
APP = ROOT / "rust/target/release/floe2-web"
sys.path.insert(0, str(ROOT))
from floe import fe_embed as fe


def chunk(kind, data):
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))


def chunks(blob):
    assert blob.startswith(fe.PNG_SIGNATURE)
    pos = 8
    while pos < len(blob):
        n, kind = struct.unpack_from(">I4s", blob, pos)
        end = pos + 12 + n
        body = blob[pos + 8:end - 4]
        assert end <= len(blob)
        assert struct.unpack_from(">I", blob, end - 4)[0] == zlib.crc32(kind + body)
        yield kind, body, blob[pos:end]
        if kind == b"IEND":
            break
        pos = end


def rich_png():
    image = Image.new("RGBA", (19, 13))
    image.putdata([(i * 71 % 256, i * 29 % 256, i * 11 % 256, 255) for i in range(19 * 13)])
    out = io.BytesIO()
    image.save(out, format="PNG")
    parts = [fe.PNG_SIGNATURE]
    for kind, body, raw in chunks(out.getvalue()):
        if kind == b"IDAT":
            parts += [chunk(kind, body[:len(body)//2]), chunk(kind, body[len(body)//2:])]
        elif kind == b"IEND":
            parts += [chunk(b"tEXt", b"comment\0keep this"), chunk(b"iTXt", b"unrelated\0\0\0\0\0kept"), chunk(b"vpAg", b"opaque ancillary"), raw]
        else:
            parts.append(raw)
    return b"".join(parts) + b"verbatim trailer\0"


def run(args, env, code=0, stdin=None, python=False, **kwargs):
    command = [sys.executable, "-B", "-m", "floe.fe_embed"] if python else [str(APP), "fe-embed"]
    result = subprocess.run(command + list(map(str, args)), cwd=ROOT, env=env,
                            input=stdin, capture_output=True, text=True, timeout=30, **kwargs)
    assert result.returncode == code, (args, result.returncode, result.stdout, result.stderr)
    assert "panicked" not in result.stderr
    return result


def compare(work, env, base, tag, args, stdin=None):
    paths = [work / f"{tag}-{side} 한 글.png" for side in ("rust", "python")]
    for i, path in enumerate(paths):
        path.write_bytes(base)
        path.chmod(0o640)
        run([*args, path], env, stdin=stdin, python=i == 1)
        assert path.stat().st_mode & 0o777 == 0o640
    actual, expected = [p.read_bytes() for p in paths]
    assert actual == expected, (tag, fe.extract_text(actual), fe.extract_text(expected))
    assert fe.insert_text(actual, None) == fe.insert_text(base, None), (tag, "non-annotation bytes changed")
    with Image.open(io.BytesIO(actual)) as image, Image.open(io.BytesIO(base)) as before:
        assert image.convert("RGBA").tobytes() == before.convert("RGBA").tobytes()
    assert fe.read_bytes(actual) == fe.read_bytes(expected)
    dump = run(["--dump", paths[0]], env).stdout
    assert dump == (fe.extract_text(expected) if fe.extract_text(expected) is not None else "(no embedded metadata)\n")
    list(chunks(actual))
    return actual


def malformed_and_limits(work, env, base):
    path = work / "bad.png"
    valid_chunks = list(chunks(fe._sample_png()))
    raw = bytearray(base)
    raw[29] ^= 1
    malformed = [b"not a PNG", base[:20], bytes(raw), fe.PNG_SIGNATURE + chunk(b"IDAT", b"x") + chunk(b"IEND", b""),
                 fe.PNG_SIGNATURE + valid_chunks[0][2] + struct.pack(">I4s", 0xFFFFffff, b"IDAT"),
                 fe._sample_png()[:-12]]
    for body in (b"flateyes\0", b"flateyes\0\2\0\0\0text", b"flateyes\0\0\1\0\0text",
                 b"flateyes\0\1\0\0\0corrupt zlib", b"flateyes\0\0\0xx", b"flateyes\0\0\0\0\0\xff"):
        malformed.append(fe._sample_png()[:-12] + chunk(b"iTXt", body) + chunk(b"IEND", b""))
    bomb = b"flateyes\0\1\0\0\0" + zlib.compress(b"x" * (16 * 1024 * 1024 + 1))
    stream = zlib.compress(b"note=must not be partially accepted\n")
    for cut in range(len(stream)):
        body = b"flateyes\0\1\0\0\0" + stream[:cut]
        malformed.append(fe._sample_png()[:-12] + chunk(b"iTXt", body) + chunk(b"IEND", b""))
    malformed.append(fe._sample_png()[:-12] + chunk(b"iTXt", bomb) + chunk(b"IEND", b""))
    duplicate_bomb = b"flateyes\0\1\0\0\0" + zlib.compress(b"x" * (9 * 1024 * 1024))
    malformed.append(fe._sample_png()[:-12] + chunk(b"iTXt", duplicate_bomb) * 2 + chunk(b"IEND", b""))
    many = fe._sample_png()[:-12] + chunk(b"vpAg", b"") * 65536 + chunk(b"IEND", b"")
    malformed.append(many)
    for blob in malformed:
        path.write_bytes(blob)
        run(["--strip", path], env, code=1)
        assert path.read_bytes() == blob
        assert not list(work.glob(".floe-shot-*"))
    # The file limit is checked before reading/allocating its sparse body.
    with path.open("wb") as handle:
        handle.truncate(1024 * 1024 * 1024 + 1)
    run(["--strip", path], env, code=1)
    assert path.stat().st_size == 1024 * 1024 * 1024 + 1
    path.write_bytes(base)
    for args in (["--ppu=NaN", "--ruler=0,0,1,1"], ["--ppu=0", "--ruler=0,0,1,1"],
                 ["--box=0,0,1,1,0"], ["--path=0,0,NaN,1"], ["--text=0,0,size=100,x"],
                 ["--unit=x\nnote=injection", "--ppu=1"], ["--strip", "--note=x"], ["--box=0,0,1,1,red,sky,9"]):
        run([*args, path], env, code=2)
        assert path.read_bytes() == base
    bad_json = work / "bad.json"
    for value in ({"ppu": -1}, {"annotations": [{"kind": "ruler", "a": [0,0], "b": [1,1], "color": "red"}]},
                  {"extra": True}, {"legend": []}, {"note": " "}, {"annotations": "not an array"}):
        bad_json.write_text(json.dumps(value))
        run(["--json", bad_json, path], env, code=1)
        assert path.read_bytes() == base
    # Failed staging must not truncate the old PNG, even after data were copied.
    def small_file_limit():
        signal.signal(signal.SIGXFSZ, signal.SIG_IGN)
        resource.setrlimit(resource.RLIMIT_FSIZE, (128, 128))
    run(["--note=x", path], env, code=1, preexec_fn=small_file_limit)
    assert path.read_bytes() == base and not list(work.glob(".floe-shot-*"))
    link = work / "alias.png"
    link.symlink_to(path)
    assert run(["--dump", link], env).stdout == "(no embedded metadata)\n"
    dump = run(["--dump", path, link], env).stdout
    assert dump.count("(no embedded metadata)") == 2 and str(link) in dump
    run(["--strip", link], env, code=1)
    assert link.is_symlink() and path.read_bytes() == base
    link.unlink()
    os.link(path, link)
    run(["--strip", path], env, code=1)
    assert path.read_bytes() == link.read_bytes() == base
    link.unlink()
    run(["--note=x", path, path], env, code=2)
    fifo = work / "producer.png"
    os.mkfifo(fifo)
    run(["--strip", fifo], env, code=1)
    assert path.read_bytes() == base
    # A later invalid PNG reports prior commits, not a fictional all-or-nothing batch.
    later = work / "later.png"
    later.write_bytes(b"bad")
    result = run(["--note=updated", path, later], env, code=1)
    assert "1 PNG(s) already updated" in result.stderr
    assert fe.read_bytes(path.read_bytes())[3] == "updated" and later.read_bytes() == b"bad"
    assert not list(work.glob(".floe-shot-*"))


def cancellations(work, env, base):
    path = work / "cancel.png"
    for sig in (signal.SIGINT, signal.SIGTERM):
        path.write_bytes(base)
        proc = subprocess.Popen([str(APP), "fe-embed", "--json=-", str(path)], env=env,
                                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            proc.stdin.write(b"["); proc.stdin.flush()
            time.sleep(.2)
            proc.send_signal(sig)
            proc.wait(timeout=5)  # producer remains open; EOF cannot release it
            proc.stdin.close(); proc.stdin = None
            out, err = proc.communicate(timeout=1)
            assert proc.returncode == 128 + sig, (out, err)
        finally:
            if proc.poll() is None:
                proc.kill(); proc.communicate()
        assert path.read_bytes() == base
    # An opaque 64MiB ancillary chunk exercises streaming, not an image-sized
    # allocation. Catch the owned staging file before cancelling the copy/sync.
    path = work / "large.png"
    payload = b"x" * (1024 * 1024)
    crc = zlib.crc32(b"vpAg")
    for _ in range(64):
        crc = zlib.crc32(payload, crc)
    with path.open("wb") as handle:
        handle.write(fe._sample_png()[:-12])
        handle.write(struct.pack(">I4s", 64 * len(payload), b"vpAg"))
        for _ in range(64):
            handle.write(payload)
        handle.write(struct.pack(">I", crc) + chunk(b"IEND", b""))
    original = path.stat()
    for sig in (signal.SIGINT, signal.SIGTERM):
        proc = subprocess.Popen([str(APP), "fe-embed", "--note=cancelled", str(path)], env=env,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            deadline = time.monotonic() + 10
            while not list(work.glob(f".floe-shot-{proc.pid}-*.tmp")):
                assert proc.poll() is None and time.monotonic() < deadline, "never observed PNG staging"
                time.sleep(.001)
            proc.send_signal(sig)
            out, err = proc.communicate(timeout=5)
            assert proc.returncode == 128 + sig, (out, err)
        finally:
            if proc.poll() is None:
                proc.kill(); proc.communicate()
        after = path.stat()
        assert (after.st_ino, after.st_size, after.st_mtime_ns) == (original.st_ino, original.st_size, original.st_mtime_ns)
        assert not list(work.glob(".floe-shot-*"))


def main():
    with tempfile.TemporaryDirectory(prefix="floe-fe-embed-") as td:
        work = Path(td)
        env = dict(os.environ, PATH="", FLOE_RENDERD_BIN="/missing", FLOE_INDEX_BIN="/missing", PYTHONDONTWRITEBYTECODE="1")
        run(["--selftest"], env)
        base = rich_png()
        definitions = fe._sample_annos()
        # Seeded exponents, rounding boundaries and signed zero for %.10g parity.
        rng = random.Random(20260914)
        for v in [-0.0, 1e-5, 1e-4, 1e9, 1e10, 9999999999.5, 1e-307, 1e308] + [rng.uniform(-1, 1) * 10 ** rng.randrange(-307, 308) for _ in range(250)]:
            definitions.append(fe.ruler(v, -v, v/2, 0))
        doc = {"ppu": 8.5, "unit": "µm", "note": "현장 메모\\n\n확인", "legend": ["box #aBcDeF clear well", "box sky pat:" + "aaaa5555"*8 + " MASK", "line pink dash route"], "annotations": definitions}
        data = work / "주석.json"
        data.write_text(json.dumps(doc, ensure_ascii=False))
        stamped = compare(work, env, base, "json", ["--json", data])
        compare(work, env, base, "stdin", ["--json=-"], json.dumps(doc, ensure_ascii=False))
        opts = ["--box=-1,-2,30,40,red,sky,2,dashed", "--ellipse=0,0,20,30,0,#aabbcc30,3,dotted", "--line=0,0,10,10,green,8,dotted",
                "--path=0,0,10,5,20,10,pink,2,dashed", "--polygon=0,0,10,0,10,10,0,#aabbcc80:brick,2,dotted",
                "--ruler=0,0,12.25,-1", "--text=0,0,size=20,color=white,bg=#12345680,메모\\n확인,comma", "--text=1,2,text=size=10,literal",
                "--ppu=4", "--unit=nm", "--note=note\\nline"]
        compare(work, env, base, "options", opts)
        legend = work / "legend.txt"
        legend.write_text("# comment\nbox green speckle LEVEL 1\nline sky dot METAL\n")
        compare(work, env, stamped, "append", ["--append", "--json", data, "--ppu=12", "--unit=nm", "--note=override", "--legend", legend, "--ruler=1,2,3,4"])
        compare(work, env, stamped, "keep", ["--append", "--unit=ignored", "--text=0,0,bg=0,late"])
        compare(work, env, stamped, "noop-append", ["--append"])
        compare(work, env, stamped, "strip", ["--strip"])
        text = "# old\nppu=2\nunit=um\nunknown=future\nbox=0,0,1,1,#FF5040,0,1,0\n"
        compressed = b"flateyes\0\1\0en\0translated\0" + zlib.compress(text.encode())
        duplicate = base[:base.index(b"IEND")-4] + chunk(b"iTXt", compressed) + chunk(b"iTXt", b"flateyes\0\0\0\0\0note=ignored\n") + chunk(b"IEND", b"") + b"trailer"
        compare(work, env, duplicate, "compressed", ["--append", "--note=next"])
        # All PNG color modes remain byte/pixel-identical; no RGBA-only assumption.
        for mode in ("1", "L", "P", "RGB", "RGBA", "LA", "I;16"):
            stream = io.BytesIO()
            Image.new(mode, (7, 5)).save(stream, format="PNG")
            compare(work, env, stream.getvalue(), "mode-" + mode.replace(';', '-'), ["--note=mode"])
        malformed_and_limits(work, env, base)
        cancellations(work, env, base)
    print("RUST FE EMBED: ALL OK (Python bytes/metadata, seven kinds, append/strip, PNG modes, bounds/errors/signals)")


if __name__ == "__main__":
    main()
