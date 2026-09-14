#!/usr/bin/env python3
"""Actual GTK minimap pixel/coordinate oracle; no GTK or customer input needed."""
import ast
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
from types import MethodType, SimpleNamespace

ROOT = Path(__file__).resolve().parents[1]
PALETTE = [0x000000FF, 0x141414FF, 0x666666FF, 0x46565FFF, 0x8ECDF5FF]


class Pixels:
    def __init__(self, width=180, height=180, parent=None, offset=(0, 0)):
        self.width, self.height, self.parent, self.offset = width, height, parent, offset
        self.data = bytearray(b"0" * (width * height)) if parent is None else None

    def get_width(self):
        return self.width

    def get_height(self):
        return self.height

    def fill(self, rgba):
        root = self.parent or self
        x, y = self.offset
        for row in range(y, y + self.height):
            root.data[row * root.width + x:row * root.width + x + self.width] = (
                str(PALETTE.index(rgba)).encode() * self.width)

    def new_subpixbuf(self, x, y, w, h):
        return Pixels(w, h, self, (x, y))

    def copy(self):
        out = Pixels(self.width, self.height)
        out.data[:] = self.data
        return out


def main():
    names = {"fill_rect", "frame_rect", "_minimap_geom", "_minimap_frontier_depth",
             "_minimap_world_point", "_on_minimap_click", "_minimap_base", "_update_minimap"}
    funcs = [n for n in ast.walk(ast.parse((ROOT / "floe/gui.py").read_text()))
             if isinstance(n, ast.FunctionDef) and n.name in names]
    assert len(funcs) == len(names)
    scope = dict(MINIMAP_PX=180, MINIMAP_DOT_MIN=6, BLACK=PALETTE[0],
                 MINIMAP_BG=PALETTE[1], MINIMAP_EDGE=PALETTE[2],
                 MINIMAP_FRONT=PALETTE[3], MINIMAP_VIEW=PALETTE[4],
                 Gdk=SimpleNamespace(EventType=SimpleNamespace(BUTTON_PRESS=1)),
                 GdkPixbuf=SimpleNamespace(Colorspace=SimpleNamespace(RGB=0),
                     Pixbuf=SimpleNamespace(new=lambda _c, _a, _b, w, h: Pixels(w, h))),
                 os=SimpleNamespace(environ={}))
    exec(compile(ast.Module(body=funcs, type_ignores=[]), "GTK overview oracle", "exec"), scope)
    cases = []
    for bbox in ([0., 0., 1000., 1000.], [-10.9375, -20.5, 700., 1400.],
                 [-4000., 70., 4000., 80.], [0., 0., 1., 1000.],
                 [-1e8, 1e8, -1e8 + 50000., 1e8 + 60000.]):
        a, b, c, d = bbox
        w, h = c - a, d - b
        rows = [[a + i*w/15, b + i*h/15, a + (i+1)*w/15, b + (i+1)*h/15, i % 3]
                for i in range(14)]
        rows += [[a, b, c, b + h/1000, 1], [a, d, a, d, 0]]
        frontier = {"depths": [rows, []]}
        for depth in (None, 0, 1, 9):
            for fraction in (1., .2, .01):
                for point in ([90., 90.], [2., 2.], [179., 179.]):
                    span = max(w, h) * fraction
                    cx, cy = a + w*.43, b + h*.57
                    view = [cx-span/2, cy-span*.75/2, cx+span/2, cy+span*.75/2]
                    image = SimpleNamespace(translate_coordinates=lambda *_: (7, 13))
                    image.set_from_pixbuf = lambda pixels: setattr(image, "pixels", pixels)
                    obj = SimpleNamespace(meta={"bbox": bbox}, cache=object(),
                        _frontier_depths=frontier["depths"], _minimap_bases={},
                        depth_value=999 if depth is None else depth,
                        _minimap_image=image, _minimap_event=object(), cx=cx, cy=cy,
                        spp=span/800, redraw=lambda **_: None)
                    for name in names:
                        if name.startswith("_"):
                            setattr(obj, name, MethodType(scope[name], obj))
                    base = obj._minimap_base(obj._minimap_frontier_depth()).data.decode()
                    obj._update_minimap(view)
                    rendered = image.pixels.data.decode()
                    obj._on_minimap_click(None, SimpleNamespace(type=1, button=1,
                        x=point[0]+7, y=point[1]+13))
                    after = [obj.cx-span/2, obj.cy-span*.75/2,
                             obj.cx+span/2, obj.cy+span*.75/2]
                    cases.append(dict(bbox=bbox, frontier=frontier, depth=depth,
                                      view=view, base=base, pixels=rendered, point=point, after=after))
    cargo = shutil.which("cargo")
    assert cargo
    build = subprocess.run([cargo, "test", "--offline", "--locked", "-p", "floe-app-core",
                            "--lib", "--no-run", "--message-format=json"],
                           cwd=ROOT / "rust", text=True, capture_output=True, timeout=180)
    assert build.returncode == 0, build.stderr
    bins = [v["executable"] for line in build.stdout.splitlines()
            if (v := json.loads(line)).get("reason") == "compiler-artifact" and v.get("executable")]
    assert len(bins) == 1
    with tempfile.TemporaryDirectory(prefix="floe-minimap-oracle-") as td:
        path = Path(td) / "cases.json"
        path.write_text(json.dumps(cases))
        result = subprocess.run([bins[0], "view::minimap::tests::gtk_overview_pixels_and_snapped_navigation_match",
                                 "--ignored", "--nocapture"],
                                env=dict(os.environ, PATH="", FLOE_MINIMAP_CASES=str(path)),
                                text=True, capture_output=True, timeout=30)
        assert result.returncode == 0, (result.stdout, result.stderr)
        assert "MINIMAP GTK/RUST: ALL OK" in result.stdout
        print(result.stdout.strip())


if __name__ == "__main__":
    main()
