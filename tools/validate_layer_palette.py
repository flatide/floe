#!/usr/bin/env python3
"""GTK-source palette oracle, with inert rows and no GTK/source/cache I/O.

The targets are Rust's atomic visibility operation, read-only palette
page/range order and the browser's pure click-selection rules. Data is synthetic;
this does not replace actual browser input or visual acceptance.
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


def order_cases(scope):
    cases = []
    physical = [(3, 1), (3, 2), (3, 300), (7, 4), (7, 9), (9, 0)]
    deck = [(3, 0), (3, 1), (3, 2), (7, 0), (7, 1), (9, 0)]
    large = [(layer, dt) for layer in range(150) for dt in (2, 3, 9)]
    for pairs, synthetic, hide in [(physical, False, False), (deck, True, False),
                                   (deck, True, True), (large, False, False)]:
        groups = {}
        for layer, members in itertools.groupby(pairs, key=lambda p: p[0]):
            members = list(members)
            if len(members) > 1:
                groups[members[0]] = members[1:]
        heads = set(groups) if synthetic else set()
        hidden = {p for kids in groups.values() for p in kids} if hide else set()
        for collapsed in [set(), set(groups), set(list(groups)[::2]), {next(iter(groups))}]:
            effective = set(groups) if hide else collapsed
            o = SimpleNamespace(_layer_order=pairs, _layer_groups=groups,
                                _layer_expanded=set(groups) - effective)
            expected = scope["_selectable_layer_order"](o)
            for default in (False, True):
                exceptions = [] if hide else sorted(set(groups) - collapsed if default else collapsed)
                cases.append(dict(pairs=pairs, heads=sorted(heads), hidden=sorted(hidden),
                                  fold=dict(closed=default, exceptions=exceptions), expected=expected))
    return cases


def selection_cases(scope):
    cases = []
    for pairs in [[(3, 1), (3, 2), (3, 300), (7, 4), (7, 9), (9, 0)],
                  [(3, 0), (3, 1), (3, 2), (7, 0), (7, 1), (9, 0)]]:
        groups = {pairs[0]: pairs[1:3], pairs[3]: pairs[4:5]}
        for bits, anchor, folded in itertools.product(
                (0, 1, 3, 21, 42, 63), (None, pairs[0], pairs[1], pairs[-2]), range(4)):
            before = {p for i, p in enumerate(pairs) if bits & (1 << i)}
            expanded = {p for i, p in enumerate(groups) if not folded & (1 << i)}
            owner = SimpleNamespace(_layer_order=pairs, _layer_groups=groups, _layer_expanded=expanded)
            selectable = scope["_selectable_layer_order"](owner)
            for row in selectable:
                for detail, modifiers, button in [(d, m, 0) for d in (1, 2) for m in range(4)] + [(1, 0, 2)]:
                    owner._selected_layers = set(before)
                    owner._layer_select_anchor = anchor
                    owner._layer_rows = {p: SimpleNamespace(set_selected=lambda on: None) for p in pairs}
                    owner._popup_layer_menu = lambda event: None
                    owner._set_layer_selection = MethodType(scope["_set_layer_selection"], owner)
                    owner._selectable_layer_order = MethodType(scope["_selectable_layer_order"], owner)
                    event = SimpleNamespace(button=1 if button == 0 else 3, state=modifiers, type=detail)
                    scope["_on_layer_clicked"](owner, SimpleNamespace(key=row), event)
                    selected_range = None
                    if modifiers & 1 and anchor in selectable:
                        a, b = sorted((selectable.index(anchor), selectable.index(row)))
                        selected_range = selectable[a:b + 1]
                    key = lambda p: None if p is None else "%d/%d" % p
                    cases.append(dict(before=[key(p) for p in sorted(before)], anchor=key(anchor), row=key(row),
                                      event=dict(detail=detail, button=button, shiftKey=bool(modifiers & 1), ctrlKey=bool(modifiers & 2)),
                                      range=None if selected_range is None else [key(p) for p in selected_range],
                                      expected=dict(selected=[key(p) for p in sorted(owner._selected_layers)], anchor=key(owner._layer_select_anchor))))
    assert len(cases) == 7776
    return cases


def main():
    tree = ast.parse((ROOT / "floe/gui.py").read_text())
    viewer = next(n for n in tree.body if isinstance(n, ast.ClassDef) and n.name == "Viewer")
    names = ("_set_selected_layers", "_is_jobdeck_head", "_sync_jobdeck_groups", "_on_layer_toggled",
             "_selectable_layer_order", "_set_layer_selection", "_on_layer_clicked")
    methods = {n.name: n for n in viewer.body if isinstance(n, ast.FunctionDef)}
    scope = {"Gdk": SimpleNamespace(ModifierType=SimpleNamespace(SHIFT_MASK=1, CONTROL_MASK=2),
                                    EventType=SimpleNamespace(BUTTON_PRESS=1))}
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
        oracle.write_text(json.dumps(order_cases(scope)))
        build = subprocess.run([cargo, "test", "--offline", "--locked", "-j2", "-p", "floe-web",
            "--lib", "--no-run", "--message-format=json"], cwd=ROOT / "rust", text=True,
            capture_output=True, timeout=240)
        assert build.returncode == 0, build.stderr
        bins = [r["executable"] for line in build.stdout.splitlines()
                if (r := json.loads(line)).get("reason") == "compiler-artifact" and r.get("executable")]
        assert len(bins) == 1
        run = subprocess.run([bins[0], "layer_catalog::tests::gtk_selectable_order_oracle",
            "--ignored", "--nocapture"], env=env, text=True, capture_output=True, timeout=30)
        assert run.returncode == 0, (run.stdout, run.stderr)
        assert "GTK PALETTE ORDER: ALL OK (32 source-derived catalogue/fold cases" in run.stdout
        print(run.stdout.strip())
        oracle.write_text(json.dumps(selection_cases(scope)))
        node = shutil.which("node")
        assert node, "Node is required for the GTK/browser selection oracle (development only)"
        run = subprocess.run([node, str(ROOT / "rust/web/ui/palette.test.cjs"), str(oracle)],
                             text=True, capture_output=True, timeout=30)
        assert run.returncode == 0, (run.stdout, run.stderr)
        assert "GTK WEB PALETTE SELECTION: ALL OK (7776 source-derived clicks)" in run.stdout
        print(run.stdout.strip())


if __name__ == "__main__":
    main()
