#!/usr/bin/env python3
"""Source-derived GTK menu inventory, NOT a functionality/browser parity oracle.

No GTK import or design data. Unknown callbacks and broken evidence links fail.
--require-complete additionally fails on acknowledged, unimplemented menu paths.
Real semantics remain the responsibility of the linked native/HTTP/UI gates.
"""
import argparse
import ast
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
UI = ROOT / "rust/web/ui"


def linked(control, module="app", test="client", count=1):
    return (count, "linked", control, module, test)


# One entry per GTK handler family; count = menu call sites, not expanded loops.
# Thin modes and jobdeck modes each have one source call inside a three-item loop.
POLICIES = {
    "_load_layout_dialog": linked("browse-open", "browse", "browse"),
    "_load_jobdeck_dialog": linked("browse-open", "browse", "browse"),
    "_clip_dialog": linked("clip-open", "clip", "clip"),
    "_copy_view": linked("snapshot-copy", "snapshot", "snapshot"),
    "_confirm_quit": linked("logout", "session-exit", "session-exit"),
    "fit": linked("fit"),
    "_zoom_center": linked("viewport", count=2),
    "_goto_dialog": linked("goto-form"),
    "_detail_dialog": linked("detail"),
    "_label_font_dialog": linked("font-px"),
    "_depth_step": linked("depth", count=2),
    "_set_depth": linked("depth"),
    "_set_frames": linked("frames"),
    "_toggle_abstract": (1, "excluded", "Rust never supported abstract", None, None),
    "_toggle_coverage": (1, "excluded", "Retired density coverage; not occupancy", None, None),
    "_set_lod": (1, "inactive", "GTK Rust wire does not carry the LOD toggle", None, None),
    "_set_thin": linked("thin"),
    "_set_mono": linked("mono"),
    "_toggle_overlays": linked("overlays"),
    "_toggle_ruler": linked("ruler-mode", "measure", "measure"),
    "_toggle_snap": linked("ruler-snap", "measure", "measure"),
    "_ruler_pop": linked("ruler-pop", "measure", "measure"),
    "_rulers_clear": linked("ruler-clear", "measure", "measure"),
    "_drc_open_dialog": linked("drc-open", "browse", "browse"),
    "_drc_rules_dialog": linked("drc-rules-load", "browse", "browse"),
    "_drc_step": linked("drc-step-next", "drc", "drc-navigation", count=2),
    "_drc_waive_key": linked("waives-action", "drc-waives", "drc-waives"),
    "_drc_waive_save_dialog": linked("transfer-export", "drc-transfer", "drc-transfer"),
    "_drc_waive_load_dialog": linked("transfer-import", "drc-transfer", "drc-transfer"),
    "_drc_note_key": linked("notes-read", "drc-notes", "drc-notes"),
    "_drc_note_clear": linked("notes-text", "drc-notes", "drc-notes"),
    "_drc_note_save_dialog": linked("transfer-export", "drc-transfer", "drc-transfer"),
    "_drc_note_load_dialog": linked("transfer-import", "drc-transfer", "drc-transfer"),
    "_esel_toggle": linked("drc-box", "drc", "drc-box"),
    "_jobdeck_set_mode": linked("live-mode"),
    "_jobdeck_toggle_view": linked("live-mode"),
    "_jobdeck_reselect_levels": (1, "open", "Reselect loaded levels without changing camera", None, None),
    "_about_dialog": linked("about-open", "about", "about"),
    "_licenses_dialog": linked("about-open", "about", "about"),
}


def inventory(source):
    tree = ast.parse(source)
    functions = [n for n in ast.walk(tree)
                 if isinstance(n, ast.FunctionDef) and n.name == "_build_menubar"]
    assert len(functions) == 1, "GTK menu constructor changed; audit it"
    rows = []
    for call in sorted(ast.walk(functions[0]), key=lambda n: getattr(n, "lineno", 0)):
        if not (isinstance(call, ast.Call) and isinstance(call.func, ast.Name)
                and call.func.id in ("item", "check")):
            continue
        assert len(call.args) >= 3, "GTK menu helper signature changed"
        callback = call.args[2]
        if isinstance(callback, ast.Lambda):
            assert isinstance(callback.body, ast.Call), "Audit new menu lambda"
            callback = callback.body.func
        assert (isinstance(callback, ast.Attribute) and isinstance(callback.value, ast.Name)
                and callback.value.id == "self"), "Audit new GTK callback form"
        rows.append((callback.attr, call.lineno, ast.unparse(call.args[1])))
    assert rows, "No GTK menu actions found"
    return rows


def validate(rows, html, read, exists):
    counts = Counter(handler for handler, _, _ in rows)
    expected = Counter({name: row[0] for name, row in POLICIES.items()})
    assert counts == expected, f"Menu inventory changed: {counts - expected}; removed: {expected - counts}"
    for handler, (_, status, evidence, module, test) in POLICIES.items():
        assert status in ("linked", "open", "excluded", "inactive"), handler
        if status == "linked":
            assert f'id="{evidence}"' in html, f"Missing web control: {handler} / {evidence}"
            code = read(module + ".js")
            assert any(q + evidence + q in code for q in ("'", '"')), f"Missing control reference: {handler}"
            assert exists(test + ".test.cjs"), f"Missing evidence test: {handler}"


def selftest(rows, html):
    read = lambda name: (UI / name).read_text()
    exists = lambda name: (UI / name).is_file()
    validate(rows, html, read, exists)
    for altered, markup, probe in [
        (rows + [("_unreviewed_menu", 0, "new")], html, exists),
        (rows[1:], html, exists),
        (rows, html.replace('id="fit"', 'id="gone"'), exists),
        (rows, html, lambda name: False),
    ]:
        try:
            validate(altered, markup, read, probe)
        except AssertionError:
            continue
        raise AssertionError("Inventory failed to detect a broken evidence link")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--require-complete", action="store_true")
    args = parser.parse_args()
    rows = inventory((ROOT / "floe/gui.py").read_text())
    selftest(rows, (UI / "index.html").read_text())
    totals = Counter(POLICIES[handler][1] for handler, _, _ in rows)
    print(f"GTK MENU INVENTORY: {len(rows)} call sites / {len(POLICIES)} handlers; {dict(totals)}")
    for handler, line, _ in rows:
        policy = POLICIES[handler]
        if policy[1] == "open":
            print(f"OPEN floe/gui.py:{line} {handler}: {policy[2]}")
    print("Inventory links checked, NOT menu semantics, browser acceptance or G4 completion.")
    return 1 if args.require_complete and totals["open"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
