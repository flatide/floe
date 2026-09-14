#!/usr/bin/env python3
"""Development-only GTK release → actual JS gesture → Rust coordinate oracle.

This compares input classification and pre-clamp zoom math, not a new global
camera clamp or browser/ETX acceptance. No design files or GUI runtime needed.
"""
import ast
import json
import os
from pathlib import Path
import random
import shutil
import subprocess
import tempfile
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[1]


def main():
    names = {"_on_release", "_track_band"}
    methods = [n for n in ast.walk(ast.parse((ROOT / "floe/gui.py").read_text()))
               if isinstance(n, ast.FunctionDef) and n.name in names]
    assert len(methods) == 2
    scope = {}
    exec(compile(ast.Module(body=methods, type_ignores=[]), "GTK band oracle", "exec"), scope)
    cases = []
    rng = random.Random(20260915)
    gestures = [
        ((.25, .25), [(.75, .75)]), ((.75, .75), [(.25, .25)]),
        ((.5, .25), [(.5, .75)]), ((.25, .5), [(.75, .5)]),
        ((.5, .5), [(.2, .5), (.501, .5)]),  # collapsed outward excursion
        ((.5, .5), [(.2, .5), (.51, .7)]),  # keep outward, use vertical axis
        ((.5, .5), [(.25, .5), (.75, .75)]),  # tie goes inward
        ((.5, .5), [(.5, .5)]),
    ]
    for _ in range(80):
        s = (rng.uniform(.1, .9), rng.uniform(.1, .9))
        gestures.append((s, [(s[0] + rng.uniform(-.8, .8),
                              s[1] + rng.uniform(-.8, .8)) for _ in range(3)]))
    for dpr in (1., 1.25, 2., 2.5):
        pixels = [800, 600]
        w, h = (p / dpr for p in pixels)
        for start, moves in gestures:
            start = [start[0] * w, start[1] * h]
            moves = [[x * w, y * h] for x, y in moves]
            bbox = [-10.9375, -2.375, 14.0625, 16.375]
            obj = SimpleNamespace(
                cache=object(), _zoomdrag=tuple(start),
                _band_ext=(start[0], start[0]), _band_cur=None,
                cx=(bbox[0] + bbox[2]) / 2, cy=(bbox[1] + bbox[3]) / 2,
                spp=25 / w, _viewport_size=lambda: (w, h),
                view_bbox=lambda: bbox, redraw=lambda: None,
                _display=lambda: None, _set_live_status=lambda _: None)
            for x, y in moves:
                scope["_track_band"](obj, SimpleNamespace(x=x, y=y))
            scope["_on_release"](obj, None, SimpleNamespace(
                button=3, x=moves[-1][0], y=moves[-1][1]))
            expected = [obj.cx - w * obj.spp / 2, obj.cy - h * obj.spp / 2,
                        obj.cx + w * obj.spp / 2, obj.cy + h * obj.spp / 2]
            cases.append(dict(pixels=pixels, dpr=dpr, start=start, moves=moves,
                              bbox=bbox, expected=expected))
    data = subprocess.check_output(
        ["node", str(ROOT / "tools/validate_zoom_band.cjs")],
        input=json.dumps(cases).encode(), timeout=20)
    cargo = shutil.which("cargo")
    assert cargo, "cargo is required by this development gate"
    build = subprocess.run(
        [cargo, "test", "--offline", "--locked", "-p", "floe-web", "--test",
         "zoom_band", "--no-run", "--message-format=json"],
        cwd=ROOT / "rust", capture_output=True, text=True, timeout=180)
    assert build.returncode == 0, build.stderr
    bins = [r["executable"] for line in build.stdout.splitlines()
            if (r := json.loads(line)).get("reason") == "compiler-artifact"
            and r["target"]["name"] == "zoom_band" and r.get("executable")]
    assert len(bins) == 1
    with tempfile.TemporaryDirectory(prefix="floe-band-oracle-") as td:
        path = Path(td) / "cases.json"
        path.write_bytes(data)
        run = subprocess.run(
            [bins[0], "--ignored", "--nocapture"],
            env=dict(os.environ, PATH="", FLOE_BAND_CASES=str(path)),
            capture_output=True, text=True, timeout=20)
        assert run.returncode == 0, (run.stdout, run.stderr)
        assert "ZOOM BAND GTK/JS/RUST: ALL OK" in run.stdout
        print(run.stdout.strip())


if __name__ == "__main__":
    main()
