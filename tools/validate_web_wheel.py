#!/usr/bin/env python3
"""Run GTK's actual wheel handler as an oracle; no GTK/browser runtime."""
import ast
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
from types import MethodType, SimpleNamespace

ROOT = Path(__file__).resolve().parents[1]


def main():
    tree = ast.parse((ROOT / "floe/gui.py").read_text())
    nodes = [n for n in tree.body if isinstance(n, ast.Assign)
             and any(isinstance(t, ast.Name) and t.id == "WHEEL_ZOOM_STEP"
                     for t in n.targets)]
    functions = [n for n in ast.walk(tree) if isinstance(n, ast.FunctionDef)
                 and n.name == "_on_scroll"]
    assert len(nodes) == len(functions) == 1
    directions = SimpleNamespace(UP=1, DOWN=2, SMOOTH=3)
    scope = {"Gdk": SimpleNamespace(ScrollDirection=directions,
             ModifierType=SimpleNamespace(BUTTON1_MASK=1, BUTTON2_MASK=2, BUTTON3_MASK=4))}
    exec(compile(ast.Module(body=nodes + functions, type_ignores=[]),
                 "GTK wheel oracle", "exec"), scope)
    cases = []
    for mode in (0, 1, 2):
        for dy in (-1000, -1, -.5, -.001, 0, .001, .5, 1, 1000):
            for buttons in (0, 1, 2, 4, 5):
                calls = []
                obj = SimpleNamespace(cache=object(), _pending=None, _drag=None,
                                      _zoomdrag=None, _zoom_at=lambda *args: calls.append(args))
                ev = SimpleNamespace(direction=directions.SMOOTH, x=25, y=20, state=buttons,
                                     get_scroll_deltas=lambda: (True, 123, dy))
                MethodType(scope["_on_scroll"], obj)(None, ev)
                cases.append(dict(name=f"mode={mode}/dy={dy}/buttons={buttons}",
                                  event=dict(deltaY=dy, deltaMode=mode, buttons=buttons), expected=calls))
    # Native discrete ticks have the same capped step as one reported DOM unit.
    for direction, dy in ((directions.UP, -1), (directions.DOWN, 1)):
        calls = []
        obj = SimpleNamespace(cache=object(), _pending=None, _drag=None, _zoomdrag=None,
                              _zoom_at=lambda *args: calls.append(args))
        ev = SimpleNamespace(direction=direction, x=25, y=20, state=0)
        MethodType(scope["_on_scroll"], obj)(None, ev)
        cases.append(dict(name=f"discrete={direction}", event=dict(deltaY=dy), expected=calls))
    # GTK's pending/drag suppression is independently pinned; app.js has its
    # own full-client tests for ACK, displayed receipt and decode boundaries.
    for field in ("_pending", "_drag", "_zoomdrag"):
        calls = []
        obj = SimpleNamespace(cache=object(), _pending=None, _drag=None, _zoomdrag=None,
                              _zoom_at=lambda *args: calls.append(args))
        setattr(obj, field, object())
        ev = SimpleNamespace(direction=directions.UP, x=25, y=20, state=0)
        MethodType(scope["_on_scroll"], obj)(None, ev)
        assert not calls, field
    node = shutil.which("node")
    assert node
    with tempfile.TemporaryDirectory(prefix="floe-wheel-oracle-") as td:
        path = Path(td) / "cases.json"
        path.write_text(json.dumps(cases))
        subprocess.run([node, str(ROOT / "rust/web/ui/wheel.test.cjs"), str(path)],
                       check=True, timeout=30)


if __name__ == "__main__":
    main()
