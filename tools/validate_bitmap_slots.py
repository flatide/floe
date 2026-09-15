#!/usr/bin/env python3
"""Extract GTK bitmap SLOT behavior for the native migration oracle.

Execute the actual GTK methods with inert widgets and synthetic in-memory rows.
No GTK import, browser, worker process, design file or file publication occurs.
This is a source-side contract oracle, NOT Rust/web slot parity acceptance.
"""
import ast
import copy
from functools import lru_cache
import os
from pathlib import Path
import sys
from types import MethodType, SimpleNamespace
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from floe import fillpat


METHODS = {"_edit_fill_pattern", "_on_fill_slot_click", "_apply_fill_slot",
           "_refresh_row_fills", "_push_fills", "_props_rows"}
OK, CANCEL = -5, -6


class Widgets:
    def __init__(self, steps=()):
        self.steps = iter(steps)
        self.callbacks = {}
        self.buttons = []
        self.menus = []
        self.destroyed = False
        self.gtk = SimpleNamespace(
            Dialog=self.dialog, DrawingArea=self.area, Menu=self.menu,
            MenuItem=self.item, Align=SimpleNamespace(CENTER=0),
            ResponseType=SimpleNamespace(OK=OK, CANCEL=CANCEL))

    def dialog(self, **_kw):
        return SimpleNamespace(
            add_button=lambda label, code: self.buttons.append((label, code)),
            get_content_area=lambda: SimpleNamespace(pack_start=lambda *_args: None),
            show_all=lambda: None, run=self.run,
            destroy=lambda: setattr(self, "destroyed", True))

    def area(self):
        return SimpleNamespace(
            set_size_request=lambda *_args: None, set_halign=lambda *_args: None,
            add_events=lambda *_args: None, queue_draw=lambda: None,
            connect=lambda event, callback: self.callbacks.__setitem__(event, callback))

    def run(self):
        for event in self.steps:
            if isinstance(event, int):
                return event
            kind, x, y = event
            name = {"press": "button-press-event", "move": "motion-notify-event",
                    "release": "button-release-event"}[kind]
            self.callbacks[name](None, SimpleNamespace(x=x * 18 + 1, y=y * 18 + 1))
        raise AssertionError("dialog script needs a terminal response")

    def menu(self):
        menu = SimpleNamespace(items=[], show_all=lambda: None, popup_at_pointer=lambda _e: None)
        menu.append = menu.items.append
        self.menus.append(menu)
        return menu

    @staticmethod
    def item(**_kw):
        item = SimpleNamespace(sensitive=True, callback=None)
        item.set_sensitive = lambda value: setattr(item, "sensitive", value)
        item.connect = lambda _name, callback: setattr(item, "callback", callback)
        return item


@lru_cache(maxsize=1)
def method_code():
    tree = ast.parse((ROOT / "floe/gui.py").read_text())
    source = [n for n in ast.walk(tree) if isinstance(n, ast.FunctionDef) and n.name in METHODS]
    assert len(source) == len(METHODS)
    return compile(ast.Module(body=source, type_ignores=[]), "GTK bitmap slot methods", "exec")


def methods(widgets):
    gdk = SimpleNamespace(EventType=SimpleNamespace(BUTTON_PRESS=1), EventMask=SimpleNamespace(
        BUTTON_PRESS_MASK=1, BUTTON1_MOTION_MASK=2, BUTTON_RELEASE_MASK=4))
    scope = dict(os=os, sys=sys, fillpat=fillpat, Gtk=widgets.gtk, Gdk=gdk)
    exec(method_code(), scope)
    return {name: scope[name] for name in METHODS}


def gui_for(scope, slot, used=True):
    messages, swatches, redraws = [], {}, []
    pairs = [(3, 0), (3, 1), (7, 0), (7, 1)]
    other = (slot + 1) % 18
    gui = SimpleNamespace(
        window=None, _fill_patterns=fillpat.default_patterns(),
        _layer_patterns=({pairs[0]: slot, pairs[1]: slot, pairs[2]: other} if used else {}),
        _layer_widths={}, _color_epoch=0, _selected_layers=set(),
        _layer_groups={pairs[0]: [pairs[1]], pairs[2]: [pairs[3]]}, _layer_expanded=set(),
        visible=set(pairs), _set_live_status=lambda _message: None,
        worker=SimpleNamespace(submit=lambda message: messages.append(copy.deepcopy(message))),
        redraw=lambda **kw: redraws.append(kw),
        meta=dict(layers=[dict(layer=p[0], datatype=p[1], color="#ffffff", name="SYNTH")
                          for p in pairs]),
        _fill_slots=[SimpleNamespace(queue_draw=lambda: None) for _ in fillpat.FILL_NAMES],
        _layer_rows={p: SimpleNamespace(
            set_fill=lambda value, p=p: swatches.__setitem__(p, value)) for p in pairs})
    for name, method in scope.items():
        setattr(gui, name, MethodType(method, gui))
    return gui, messages, swatches, redraws


def words(pattern):
    return [int(w, 16) for w in fillpat.rows_to_hex(pattern).split()]


def state(gui):
    pairs = [(r["layer"], r["datatype"]) for r in gui.meta["layers"]]
    return dict(
        slots=[dict(name=name, rows=words(gui._fill_patterns[i]))
               for i, name in enumerate(fillpat.FILL_NAMES)],
        bindings=[(p, fillpat.FILL_NAMES[i]) for p, i in sorted(gui._layer_patterns.items())],
        fills=[(p, words(gui._fill_patterns[gui._layer_patterns[p]]
                        if p in gui._layer_patterns else fillpat.pattern("speckle"))) for p in pairs])


def variants(original):
    painted = original.copy()
    on = not bool(original[0] & 0x8000)
    for x, y in [(0, 0), (15, 15)]:
        mask = 1 << (15 - x)
        painted[y] = (painted[y] | mask) if on else (painted[y] & ~mask)
    return [
        ("unchanged apply", [OK], original, True),
        ("cancel", [10, CANCEL], original, False),
        ("clear", [10, OK], [0] * 16, True),
        ("solid", [11, OK], [65535] * 16, True),
        ("invert", [12, OK], [w ^ 65535 for w in original], True),
        ("reset", [10, 13, OK], original, True),
        ("drag", [("press", 0, 0), ("move", 15, 15), ("move", 0, 0),
                  ("move", -1, 0), ("move", 16, 15), ("release", 0, 0),
                  ("move", 1, 0), OK], painted, True),
        ("outside press", [("press", -1, 0), ("move", 0, 0),
                           ("press", 16, 15), OK], original, True),
    ]


def validate():
    defaults = fillpat.default_patterns()
    fixed = {i for i, name in enumerate(fillpat.FILL_NAMES) if name in fillpat.FIXED_FILLS}
    assert len(defaults) == 20 and fixed == {18, 19}, (
        "identify fixed slots by NAME, not first two indices")
    edits = 0
    cases = []
    for slot in range(18):
        for label, events, expected, applied in variants(words(defaults[slot])):
            for used in (False, True):
                widgets = Widgets(events)
                gui, messages, swatches, redraws = gui_for(methods(widgets), slot, used)
                before = copy.deepcopy(gui._layer_patterns)
                record = dict(name=fillpat.FILL_NAMES[slot], initial=state(gui), later=None, events=events)
                gui._edit_fill_pattern(slot)
                record.update(edit=words(gui._fill_patterns[slot]) if applied else None, after=state(gui))
                assert widgets.destroyed, label
                assert [name for name, _code in widgets.buttons] == [
                    "clear", "solid", "invert", "reset", "Cancel", "Apply"]
                assert words(gui._fill_patterns[slot]) == expected, (slot, label)
                assert all(p == defaults[i] for i, p in enumerate(gui._fill_patterns) if i != slot)
                assert gui._layer_patterns == before, "slot edit must not reassign selected layers"
                assert len(messages) == len(redraws) == gui._color_epoch == int(applied and used)
                if applied and used:
                    assert messages[0]["kind"] == "repattern"
                    sent = {tuple(p): words(bitmap) for p, bitmap in messages[0]["fills"]}
                    assert sent[(3, 0)] == sent[(3, 1)] == expected, "both references must change"
                    assert sent[(7, 0)] == words(defaults[(slot + 1) % 18])
                    assert swatches[(3, 0)] == swatches[(3, 1)] == gui._fill_patterns[slot]
                    assert swatches[(7, 1)] is None, "unassigned is not a matching slot reference"
                    exported = {p: name for p, _color, name, *_rest in gui._props_rows()}
                    assert exported[(3, 0)] == fillpat.FILL_NAMES[slot], (
                        "GTK exports slot NAME, not edited bits")
                # A later assignment must resolve the edited slot too. Folded
                # parent expansion is the real GTK method, not a replica here.
                gui._selected_layers = {(7, 0)}
                gui._apply_fill_slot(slot)
                record["later"] = state(gui)
                cases.append(record)
                assert gui._layer_patterns[(7, 0)] == gui._layer_patterns[(7, 1)] == slot
                sent = {tuple(p): words(bitmap) for p, bitmap in messages[-1]["fills"]}
                assert sent[(7, 0)] == sent[(7, 1)] == expected
                edits += 1
        # Equal bitmap values do not make two named slots the same reference.
        widgets = Widgets([12, OK])
        gui, messages, _swatches, _redraws = gui_for(methods(widgets), slot)
        other = (slot + 1) % 18
        gui._fill_patterns[other] = defaults[slot]
        record = dict(name=fillpat.FILL_NAMES[slot], initial=state(gui), later=None, events=[12, OK])
        gui._edit_fill_pattern(slot)
        record.update(edit=words(gui._fill_patterns[slot]), after=state(gui))
        cases.append(record)
        sent = {tuple(p): words(bitmap) for p, bitmap in messages[-1]["fills"]}
        assert sent[(3, 0)] == [w ^ 65535 for w in words(defaults[slot])]
        assert sent[(7, 0)] == words(defaults[slot])
        edits += 1
        # Reset is the bundled bitmap, NOT the already edited value at open.
        widgets = Widgets([13, OK])
        gui, _messages, _swatches, _redraws = gui_for(methods(widgets), slot)
        gui._fill_patterns[slot] = fillpat.hex_to_rows(" ".join(
            "%04X" % (w ^ 65535) for w in words(defaults[slot])))
        record = dict(name=fillpat.FILL_NAMES[slot], initial=state(gui), later=None, events=[13, OK])
        gui._edit_fill_pattern(slot)
        record.update(edit=words(gui._fill_patterns[slot]), after=state(gui))
        cases.append(record)
        assert gui._fill_patterns[slot] == defaults[slot]
        edits += 1
    menus = 0
    for flag in (None, "", "0", "1"):
        for slot in range(20):
            widgets = Widgets()
            gui, messages, _swatches, _redraws = gui_for(methods(widgets), slot, False)
            env = {} if flag is None else {"FLOE_FILL_EDIT": flag}
            with patch.dict(os.environ, env, clear=True):
                assert gui._on_fill_slot_click(slot, SimpleNamespace(type=1, button=3))
            assert len(widgets.menus) == int(bool(flag))
            if flag:
                assert len(widgets.menus[0].items) == 1
                assert widgets.menus[0].items[0].sensitive == (slot not in fixed)
            assert not messages
            menus += 1
    assert fillpat.default_patterns() == defaults, (
        "editing a session must not mutate bundled defaults")
    assert edits == 324 and menus == 80
    print("GTK BITMAP SLOT CONTRACT: ALL OK (324 edits + 80 menu gates; source oracle only)")
    assert len(cases) == 324
    return cases


if __name__ == "__main__":
    validate()
