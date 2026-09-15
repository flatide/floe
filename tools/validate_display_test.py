#!/usr/bin/env python3
"""GTK's actual synthetic display pattern versus Rust PNG/raw and web pixels.

Development oracle only: no GTK/browser, layout, renderer or user output files.
"""
import ast
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
from types import SimpleNamespace

from PIL import Image

ROOT = Path(__file__).resolve().parents[1]


class Pixbuf:
    def __init__(self, w, h, data=None, stride=None, x=0, y=0):
        self.w, self.h, self.x, self.y = w, h, x, y
        self.stride = stride or w
        self.data = data if data is not None else bytearray(w * h * 4)

    @staticmethod
    def new(space, alpha, bits, w, h):
        assert space == "RGB" and alpha is False and bits == 8
        return Pixbuf(w, h)

    def fill(self, rgba):
        row = rgba.to_bytes(4, "big") * self.w
        for y in range(self.y, self.y + self.h):
            start = (y * self.stride + self.x) * 4
            self.data[start:start + len(row)] = row

    def get_width(self):
        return self.w

    def get_height(self):
        return self.h

    def new_subpixbuf(self, x, y, w, h):
        return Pixbuf(w, h, self.data, self.stride, self.x + x, self.y + y)


def gtk_pixels():
    cli = ast.parse((ROOT / "floe/cli.py").read_text())
    command = next(n for n in cli.body if isinstance(n, ast.FunctionDef) and n.name == "cmd_gtktest")
    synth = next(n for n in command.body if isinstance(n, ast.FunctionDef) and n.name == "synth")
    gui = ast.parse((ROOT / "floe/gui.py").read_text())
    fill = next(n for n in gui.body if isinstance(n, ast.FunctionDef) and n.name == "fill_rect")
    scope = {"GdkPixbuf": SimpleNamespace(Pixbuf=Pixbuf, Colorspace=SimpleNamespace(RGB="RGB"))}
    exec(compile(ast.Module(body=[fill, synth], type_ignores=[]), "GTK display fixture", "exec"), scope)
    scope["g"] = SimpleNamespace(fill_rect=scope["fill_rect"])
    a, b = scope["synth"](), scope["synth"]()
    assert (a.w, a.h) == (360, 160) and a.data == b.data
    return bytes(a.data)


def main():
    expected = gtk_pixels()
    cargo = shutil.which("cargo") or str(Path.home() / ".cargo/bin/cargo")
    build = subprocess.run([cargo, "test", "--offline", "--locked", "-j2", "-p", "floe-web", "--lib",
                            "--no-run", "--message-format=json"], cwd=ROOT / "rust", capture_output=True, text=True, timeout=240)
    assert build.returncode == 0, build.stderr
    bins = [r["executable"] for line in build.stdout.splitlines()
            if (r := json.loads(line)).get("reason") == "compiler-artifact" and r.get("executable")]
    assert len(bins) == 1
    node = shutil.which("node")
    assert node, "Node is required for this development oracle"
    with tempfile.TemporaryDirectory(prefix="floe-display-oracle-") as td:
        env = {k: v for k, v in os.environ.items() if not k.startswith("FLOE_")}
        env.update(PATH="", FLOE_DISPLAY_TEST_DIR=td)
        run = subprocess.run([bins[0], "display_test::tests::export_display_fixture", "--exact", "--ignored"],
                             env=env, capture_output=True, text=True, timeout=30)
        assert run.returncode == 0, (run.stdout, run.stderr)
        raw = (Path(td) / "test.raw").read_bytes()
        assert raw[:16] == b"FLOERAW1" + (360).to_bytes(4, "little") + (160).to_bytes(4, "little")
        assert raw[16:] == expected
        with Image.open(Path(td) / "test.png") as im:
            assert im.size == (360, 160) and im.convert("RGBA").tobytes() == expected
        web = subprocess.run([node, str(ROOT / "rust/web/ui/display-test.test.cjs")],
                             env=env, capture_output=True, text=True, timeout=30)
        assert web.returncode == 0, (web.stdout, web.stderr)
        assert "WEB DISPLAY TEST: ALL OK" in web.stdout
        print(web.stdout.strip())
    print("GTK DISPLAY ORACLE: ALL OK (57600 GTK/native PNG/raw pixels + web readback/crop; no browser acceptance)")


if __name__ == "__main__":
    main()
