#!/usr/bin/env python3
"""GTK-source palette oracle, with inert rows and no GTK/source/cache I/O.

The target is Rust's atomic visibility operation, not the still-unmigrated
browser selection/collapse UI. Test data is synthetic and private.
"""
import ast
import itertools
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
from types import MethodType, SimpleNamespace

ROOT = Path(__file__).resolve().parents[1]


def main():
    tree = ast.parse((ROOT / "floe/gui.py").read_text())
    viewer = next(n for n in tree.body if isinstance(n, ast.ClassDef) and n.name == "Viewer")
    names = ("_set_selected_layers", "_is_jobdeck_head", "_sync_jobdeck_groups", "_on_layer_toggled")
    methods = {n.name: n for n in viewer.body if isinstance(n, ast.FunctionDef)}
    scope = {}
    exec(compile(ast.Module(body=[methods[n] for n in names], type_ignores=[]),
                 "GTK palette batch oracle", "exec"), scope)

    class Row:
        def __init__(self, owner, key, on):
            self.owner, self.key, self.on = owner, key, on
        def get_active(self):
            return self.on
        def set_active(self, on):
            if self.on != on:
                self.on = on
                self.owner._on_layer_toggled(self, self.key)
        def set_group_state(self, on, partial=False):
            self.on = on

    cases = []
    for deck in (False, True):
        pairs = [(3, 0), (3, 1), (3, 2), (7, 0), (7, 1), (9, 0)] if deck else [
            (3, 1), (3, 2), (3, 300), (7, 4), (7, 9), (9, 0)]
        groups = {pairs[0]: pairs[1:3], pairs[3]: pairs[4:5]}
        heads = set(groups) if deck else set()
        for bits, show, fold, action in itertools.product(
                range(1, 64), (0, 1, 2, 3, 21, 42, 62, 63), range(4), ("show", "hide", "toggle")):
            selected = {p for i, p in enumerate(pairs) if bits & (1 << i)}
            collapsed = {p for i, p in enumerate(groups) if fold & (1 << i)}
            visible = {p for i, p in enumerate(pairs) if show & (1 << i)} - heads
            o = SimpleNamespace(visible=set(visible), selections=[], _layers_batch=False,
                _layer_order=pairs, _layer_groups=groups, _layer_expanded=set(groups) - collapsed,
                _selected_layers=selected, draws=0,
                meta={"layers": [dict(layer=p[0], datatype=p[1], jobdeck_head=p in heads) for p in pairs]})
            o._layer_rows = {p: Row(o, p, p in visible) for p in pairs}
            for name in names:
                setattr(o, name, MethodType(scope[name], o))
            def redraw(immediate=False):
                assert immediate
                o.draws += 1
            o.redraw = redraw
            o._sync_jobdeck_groups()
            o._set_selected_layers(action)
            assert o.draws == 1
            cases.append(dict(pairs=pairs, heads=sorted(heads), selected=sorted(selected),
                collapsed=sorted(collapsed & selected), visible=sorted(visible),
                expected=sorted(o.visible - heads), action=action))
    assert len(cases) == 12096
    cargo = shutil.which("cargo") or str(Path.home() / ".cargo/bin/cargo")
    build = subprocess.run([cargo, "test", "--offline", "--locked", "-j2", "-p", "floe-app-core",
        "--lib", "--no-run", "--message-format=json"], cwd=ROOT / "rust", text=True,
        capture_output=True, timeout=240)
    assert build.returncode == 0, build.stderr
    bins = [r["executable"] for line in build.stdout.splitlines()
            if (r := json.loads(line)).get("reason") == "compiler-artifact" and r.get("executable")]
    assert len(bins) == 1
    with tempfile.TemporaryDirectory(prefix="floe-palette-") as td:
        oracle = Path(td) / "oracle.json"
        oracle.write_text(json.dumps(cases))
        env = {k: v for k, v in os.environ.items() if not k.startswith("FLOE_")}
        env.update(PATH="", FLOE_LAYER_PALETTE_ORACLE=str(oracle))
        run = subprocess.run([bins[0], "view::palette::tests::gtk_palette_batch_oracle",
            "--ignored", "--nocapture"], env=env, text=True, capture_output=True, timeout=30)
        assert run.returncode == 0, (run.stdout, run.stderr)
        assert "GTK PALETTE: ALL OK (12096 source-derived batch cases)" in run.stdout
        print(run.stdout.strip())


if __name__ == "__main__":
    main()
