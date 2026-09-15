#!/usr/bin/env python3
"""Actual GTK palette methods + actual adapter versus Rust style assignments.

Synthetic rows only. Constructor file lookup and pipe publication are inert;
selection expansion, sparse maps and jobdeck submit expansion are real code.
"""
import ast
import copy
import itertools
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
from types import MethodType, SimpleNamespace
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from floe import fillpat, rust_render
from floe.jobdeck.render import DeckRenderWorker


def cases():
    methods = {"_apply_palette_color", "_apply_fill_slot", "_selected_with_folded",
               "_set_layer_width", "_step_layer_width", "_refresh_row_fills", "_push_fills"}
    tree = ast.parse((ROOT / "floe/gui.py").read_text())
    source = [n for n in ast.walk(tree) if isinstance(n, ast.FunctionDef) and n.name in methods]
    assert len(source) == len(methods)
    scope = dict(os=os, sys=sys, fillpat=fillpat)
    exec(compile(ast.Module(body=source, type_ignores=[]), "GTK palette styles", "exec"), scope)
    patterns = fillpat.default_patterns()
    output = []
    for mode in ("layout", "chip", "level"):
        pairs = [(3, 1), (3, 2), (3, 300), (7, 4), (7, 9), (9, 0)] if mode == "layout" else [
            (3, 0), (3, 1), (3, 2), (7, 0), (7, 1), (9, 0)]
        groups = {pairs[0]: pairs[1:3], pairs[3]: pairs[4:5]}
        heads = set() if mode == "layout" else set(groups)
        forced = set(groups) if mode == "level" else set()
        metadata = dict(layers=[dict(layer=p[0], datatype=p[1], color="#ffffff", jobdeck_head=p in heads)
                               for p in pairs])
        cache = SimpleNamespace(meta=metadata, src="/not-opened", dir="/not-opened")
        with patch.object(rust_render, "find_binary", lambda: "/not-spawned"), patch.object(
                rust_render.RustRenderWorker, "_init_styles", lambda self: None):
            worker = rust_render.RustRenderWorker(cache) if mode == "layout" else DeckRenderWorker(cache)
        worker.alive = lambda: True
        worker._publish_style = lambda **kw: None
        gui = SimpleNamespace(worker=worker, _layer_groups=groups, _color_epoch=0,
                              _fill_patterns=patterns, redraw=lambda **kw: None,
                              _set_live_status=lambda message: None)
        gui._layer_rows = {p: SimpleNamespace(set_color=lambda c: None, set_fill=lambda f: None) for p in pairs}
        for name in methods:
            setattr(gui, name, MethodType(scope[name], gui))
        for bits, fold, seed, (action, value) in itertools.product(
                range(1, 64), range(4), range(2),
                [("color", "#22aa88"), ("fill", "brick"), ("fill", "clear"),
                 ("width", 1), ("width", 5), ("step", -1), ("step", 1)]):
            selected = {p for i, p in enumerate(pairs) if bits & (1 << i)}
            collapsed = {p for i, p in enumerate(groups) if fold & (1 << i)}
            gui._selected_layers = selected
            gui._layer_expanded = set(groups) - collapsed - forced
            gui.meta = copy.deepcopy(metadata)
            fills = [(pairs[0], "solid"), (pairs[3], "brick")]
            widths = [(pairs[0], 6), (pairs[3], 8)]
            if seed:
                fills.append((pairs[1], "clear"))
                widths.append((pairs[1], 3))
            gui._layer_patterns = {p: fillpat.fill_index(f) for p, f in fills}
            gui._layer_widths = dict(widths)
            worker._colors = {p: "#ffffff" for p in pairs}
            gui._push_fills()
            if action == "color":
                gui._apply_palette_color(value)
            elif action == "fill":
                gui._apply_fill_slot(fillpat.fill_index(value))
            elif action == "width":
                gui._set_layer_width(value)
            else:
                gui._step_layer_width(value)
            assert worker.res.empty(), list(worker.res.queue)
            output.append(dict(pairs=pairs, heads=sorted(heads), forced=sorted(forced), selected=sorted(selected),
                collapsed=sorted((collapsed - forced) & selected), fills=fills, widths=widths, action=action, value=value,
                expected=[dict(pair=p, color=worker._colors[p], fill=worker._fills.get(p, "speckle"),
                               width=worker._widths.get(p, 1)) for p in pairs],
                assigned_fills=[(p, rust_render._pattern_fill(patterns[i])) for p, i in sorted(gui._layer_patterns.items())],
                assigned_widths=sorted(gui._layer_widths.items())))
        worker.stop()
    assert len(output) == 10584
    return output


def main():
    from validate_bitmap_slots import validate as validate_slots
    slot_cases = validate_slots()
    cargo = shutil.which("cargo") or str(Path.home() / ".cargo/bin/cargo")
    build = subprocess.run([cargo, "test", "--offline", "--locked", "-j2", "-p", "floe-app-core", "--lib",
                            "--no-run", "--message-format=json"], cwd=ROOT / "rust", capture_output=True, text=True, timeout=240)
    assert build.returncode == 0, build.stderr
    bins = [r["executable"] for line in build.stdout.splitlines()
            if (r := json.loads(line)).get("reason") == "compiler-artifact" and r.get("executable")]
    assert len(bins) == 1
    with tempfile.TemporaryDirectory(prefix="floe-palette-styles-") as td:
        oracle = Path(td) / "oracle.json"
        oracle.write_text(json.dumps(cases()))
        env = {k: v for k, v in os.environ.items() if not k.startswith("FLOE_")}
        env.update(PATH="", FLOE_PALETTE_STYLE_ORACLE=str(oracle))
        test = subprocess.run([bins[0], "view::palette_style::tests::gtk_palette_style_oracle", "--ignored", "--nocapture"],
                              env=env, capture_output=True, text=True, timeout=60)
        assert test.returncode == 0, (test.stdout, test.stderr)
        assert "GTK PALETTE STYLE: ALL OK (10584 GTK + adapter cases)" in test.stdout
        print(test.stdout.strip())
        slots = Path(td) / "slots.json"
        slots.write_text(json.dumps(slot_cases))
        env["FLOE_BITMAP_SLOT_ORACLE"] = str(slots)
        node = shutil.which("node")
        assert node, "Node is required for the development bitmap draft oracle"
        web = subprocess.run([node, str(ROOT / "rust/web/ui/fill-editor.test.cjs")],
                             env=env, capture_output=True, text=True, timeout=30)
        assert web.returncode == 0, (web.stdout, web.stderr)
        assert "GTK WEB BITMAP DRAFT: ALL OK (324 actual GTK event traces)" in web.stdout
        print(web.stdout.strip())
        test = subprocess.run([bins[0], "view::fill_slots::tests::gtk_bitmap_slots_match", "--ignored", "--nocapture"],
                              env=env, capture_output=True, text=True, timeout=30)
        assert test.returncode == 0, (test.stdout, test.stderr)
        assert "GTK BITMAP SLOTS: ALL OK (324 native cases)" in test.stdout
        print(test.stdout.strip())
        presets = Path(td) / "presets.json"
        presets.write_text(json.dumps(dict(
            colors=[dict(name=n, color=c) for n, c in fillpat.COLOR_TABLE],
            fills=[dict(name=n, rows=[int(w, 16) for w in fillpat.rows_to_hex(p).split()])
                   for n, p in fillpat.FILL_PATTERNS])))
        env["FLOE_PRESET_ORACLE"] = str(presets)
        test = subprocess.run([bins[0], "styles::presets::tests::gtk_preset_tables_match", "--ignored", "--nocapture"],
                              env=env, capture_output=True, text=True, timeout=30)
        assert test.returncode == 0, (test.stdout, test.stderr)
        assert "GTK PRESET TABLES: ALL OK" in test.stdout
        print(test.stdout.strip())


if __name__ == "__main__":
    main()
