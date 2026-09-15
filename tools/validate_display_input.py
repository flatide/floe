#!/usr/bin/env python3
"""Frozen input PNG through the actual CLI/HTTP; generated files only, no browser."""
import io
import json
import os
from pathlib import Path
import signal
import struct
import subprocess
import tempfile
import zlib

from PIL import Image, PngImagePlugin
from validate_web_cli import APP, Client, read_json, wait


def chunk(kind, data):
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))


def fixtures():
    for mode in ("1", "L", "LA", "P", "RGB", "RGBA", "I;16"):
        image = Image.new(mode, (36, 16))
        if mode == "P":
            image.putpalette([i % 256 for i in range(768)])
            image.info["transparency"] = b"\x00\xff"
        if mode in ("RGB", "RGBA", "LA"):
            color = {"RGB": (19, 111, 230), "RGBA": (19, 111, 230, 128), "LA": (77, 128)}[mode]
        else:
            color = 1
        image.paste(color, (2, 2, 30, 14))
        variants = (1, 2, 4, 8) if mode == "P" else (None,)
        for bits in variants:
            stream = io.BytesIO()
            tags = PngImagePlugin.PngInfo()
            tags.add_itxt("comment", "private synthetic test note", zip=True)
            image.save(stream, "PNG", pnginfo=tags, **({"bits": bits} if bits else {}))
            yield f"{mode}-{bits}", stream.getvalue()
    # Genuine Adam7 RGBA, with filter 0 per non-empty pass row.
    w, h = 9, 5
    samples = [[bytes((x * 20, y * 40, 100, 128 + (x % 2) * 127)) for x in range(w)] for y in range(h)]
    rows = bytearray()
    for x0, y0, dx, dy in ((0, 0, 8, 8), (4, 0, 8, 8), (0, 4, 4, 8), (2, 0, 4, 4),
                           (0, 2, 2, 4), (1, 0, 2, 2), (0, 1, 1, 2)):
        if x0 < w:
            for y in range(y0, h, dy):
                rows.append(0)
                rows.extend(b"".join(samples[y][x] for x in range(x0, w, dx)))
    adam = (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 1))
            + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))
    assert Image.open(io.BytesIO(adam)).convert("RGBA").tobytes() == b"".join(b"".join(row) for row in samples)
    yield "Adam7", adam


def main():
    cases = list(fixtures())
    with tempfile.TemporaryDirectory(prefix="floe-display-input-") as td:
        root = Path(td)
        temps = root / "temps"
        temps.mkdir()
        env = dict(os.environ, PATH="", TMPDIR=str(temps), FLOE_INDEX_BIN=str(root / "absent-index"),
                   FLOE_RENDERD_BIN=str(root / "absent-renderd"), FLOE_FIREFOX_BIN=str(root / "absent-firefox"))
        source = root / "-input 한글.png"
        session_file = root / "session.json"
        argv = [str(APP), "displaytest", "--no-open", "--session-file", str(session_file), "--", source.name]
        for name, original in cases:
            source.write_bytes(original)
            with subprocess.Popen(argv, cwd=root, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE) as proc:
                try:
                    session = wait(lambda: read_json(session_file), proc)
                    assert source.read_bytes() == original
                    client = Client(session)
                    client.call("GET", "/api/v1/display-test/input", code=401)
                    client.login()
                    meta = client.call("GET", "/api/v1/capabilities")["display_input"]
                    expected = Image.open(io.BytesIO(original))
                    assert meta == dict(width=expected.width, height=expected.height, bytes=len(original)), name
                    assert source.name not in json.dumps(session)
                    assert source.name not in json.dumps(meta)
                    saved = client.csrf
                    client.csrf = None
                    client.call("GET", "/api/v1/display-test/input", code=401)
                    client.csrf = saved
                    client.call("GET", "/api/v1/display-test/input?path=another", code=403)
                    # Never reread: even replacing/deleting the selected path cannot change the session bytes.
                    source.write_bytes(b"not a PNG any more")
                    source.unlink()
                    received = client.call("GET", "/api/v1/display-test/input")
                    assert received == original, name
                    assert Image.open(io.BytesIO(received)).convert("RGBA").tobytes() == expected.convert("RGBA").tobytes()
                    assert client.call("GET", "/api/v1/display-test/input") == original
                    client.call("DELETE", "/api/v1/session", code=204)
                    stdout, stderr = proc.communicate(timeout=10)
                    assert proc.returncode == 0 and not stdout, stderr
                    assert client.token.encode() not in stderr
                    assert not session_file.exists() and not list(temps.iterdir())
                finally:
                    if proc.poll() is None:
                        proc.send_signal(signal.SIGINT)
                        proc.communicate(timeout=10)
        original = cases[0][1]
        bad_crc = bytearray(original)
        bad_crc[29] ^= 1
        huge = bytearray(original)
        huge[16:20] = struct.pack(">I", 8193)
        huge[29:33] = struct.pack(">I", zlib.crc32(huge[12:29]))
        animation = original[:33] + chunk(b"acTL", struct.pack(">II", 2, 0)) + original[33:]
        for damaged in (b"not png", original[:30], bad_crc, huge, original + b"extra", animation):
            source.write_bytes(damaged)
            run = subprocess.run(argv, cwd=root, env=env, capture_output=True, timeout=10)
            assert run.returncode != 0 and source.read_bytes() == damaged
            assert not session_file.exists() and not list(temps.iterdir())
        with source.open("wb") as output:
            output.truncate(80 * 1024 * 1024 + 1)
        run = subprocess.run(argv, cwd=root, env=env, capture_output=True, timeout=10)
        assert run.returncode != 0 and b"80 MiB" in run.stderr
        source.unlink()
        os.mkfifo(source)
        run = subprocess.run(argv, cwd=root, env=env, capture_output=True, timeout=3)
        assert run.returncode != 0 and not session_file.exists() and not list(temps.iterdir())
        source.unlink()
    print(f"DISPLAY INPUT CLI: ALL OK ({len(cases)} PNG modes/palettes/Adam7; immutable bytes/Pillow pixels, auth, limits/corruption/FIFO, no worker or browser)")


if __name__ == "__main__":
    main()
