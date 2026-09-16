#!/usr/bin/env python3
"""Audit the actual legacy argparse surface against native parser preflight.

This gate does not dispatch a Python command or start a native session. A native
option is placed BEFORE --help so the real parser must consume it. Acceptance
is NOT semantic/file/pixel/browser parity; the linked integration gates own that.
All temporary paths are synthetic. No user configuration or design is inspected.
"""
import argparse
from collections import Counter
import copy
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
APP = ROOT / "rust/target/release/floe2-web"

# Each token is one action: aliases separated by |, optional =preflight value.
# This is deliberately explicit: new/removed flags must trigger another audit.
SURFACE = {
    "index": """--level=1 --force --jobs=2 --page-target-mb=1
        --occupancy --no-occupancy --occupancy-only --occupancy-um=4 --occupancy-balance=1 --no-lod --lod
        --slow-cell-s=0 --p2-shard-limit-mb=0 --profile-cell=TOP --profile-cell-ci=0
        --profile-jobs=1,2 --profile-repeat=2 --profile-snapshot=snapshot
        --profile-snapshot-refresh""",
    "info": "--level=1",
    "render": """--level=1 --bbox=0,0,10,10 --layers=7/0 --px=128x96 --out=out.png
        --at=1,2 --size=10,10 --anchor=center --stretch --mosaic-at=0,1;1,1;1,0;0,0
        --corners=0,0,10,10 --line=2 --line-color=#ffffff --keep-tiles
        --batch=batch --report=report.json --depth=99 --thin=keep --detail=high
        --frames --labels --label-font-px=14 --drc=errors.db --drc-rule=RULE
        --drc-err=all --drc-cap=200 --drc-frac=0.3 --drc-rules=rules.json
        --floe-reviewer=reviewer""",
    "clip": "--bbox=0,0,10,10 --layers=7/0 --out=clip.oas --cell-name=CLIP --exact",
    "probe": "",
    "drc": "--list --rules --errs=RULE --floe-reviewer=reviewer",
    "svrf": """--out|-o=rules.json --scan --define|-D=NAME --include-dir|-I=include
        --follow-verbatim --no-env-switches""",
    "gtktest": "",
    "view": """--level=1 --multi --goto=1,2,10 --drc=errors.db --detail=high --depth=99
        --thin=keep --lod=on --refinement=off --frame-cache=on --perf-baseline
        --frames=on --labels=on --label-font-px=14 --stream-kb=0 --stream-target-ms=500
        --render-debug --hairline=0.5 --thin-um=7 --dump --floe-reviewer=reviewer""",
    "jobdeck": """--sources=sources --level|--id=1 --mode=level --colors=colors
        --ly-dt=cross --on-missing=skip --lenient --placements --report=report --spec=spec""",
    "fe-embed": """--box=0,0,10,10 --ellipse=0,0,10,10 --line=0,0,10,10
        --path=0,0,10,10 --polygon=0,0,10,0,0,10 --ruler=0,0,10,10 --text=0,0,text
        --json=annotations.json --legend=legend --note=note --ppu=1 --unit=um
        --append --dump --strip --selftest""",
}
HIDDEN = {
    "index": set("""--legacy --tile-mb --skeleton-only --texts-only --merge-only --merge
        --mem --mem-floor --no-gov --text-cap --text-tile-cap --skel-texts --tile-tgt
        --bands --read-mode""".split()),
    "view": {"--layout-mode"}, "probe": {"--layout-mode"},
}
# Defaults not listed here are None (value options) or False (switches).
# argparse occupancy=None is later resolved by cmd_index: layout off, deck on.
DEFAULTS = {
    "index": {"--jobs": 12, "--profile-repeat": 1, "--occupancy": None, "--no-occupancy": None},
    "render": {"--px": "1200", "--out": "view.png", "--anchor": "center", "--line": 2.,
               "--line-color": "#ffffff", "--detail": "exact", "--label-font-px": 14,
               "--drc-err": "all", "--drc-cap": 200, "--drc-frac": .3},
    "clip": {"--out": "clip.oas", "--cell-name": "FLOE_CLIP"},
    "svrf": {"--define": [], "--include-dir": []},
    "view": {"--lod": "on", "--refinement": "on", "--frame-cache": "on", "--frames": "on",
             "--labels": "on", "--label-font-px": 14, "--stream-target-ms": 500},
    "jobdeck": {"--mode": "level", "--ly-dt": "cross", "--on-missing": "skip"},
    "fe-embed": {"--" + k: [] for k in ("box", "ellipse", "line", "path", "polygon", "ruler", "text")},
}
CHOICES = {
    ("index", "--occupancy-balance"): (0, 1),
    ("render", "--anchor"): ("center", "lb"),
    ("render", "--thin"): ("auto", "keep", "cull"),
    ("render", "--detail"): ("exact", "low", "medium", "high"),
    ("view", "--detail"): ("low", "medium", "high"),
    ("view", "--thin"): ("auto", "keep", "cull"),
    **{("view", flag): ("on", "off") for flag in
       ("--lod", "--refinement", "--frame-cache", "--frames", "--labels")},
    ("jobdeck", "--mode"): ("level", "chip", "layer", "identifier"),
    ("jobdeck", "--ly-dt"): ("cross", "zip"),
    ("jobdeck", "--on-missing"): ("skip", "fail"),
}
INACTIVE = {
    "--lod": "not sent to Rust renderd", "--stream-target-ms": "unused by Rust renderd",
    "--hairline": "legacy KLayout planner", "--thin-um": "not an equivalent frame control",
}
EVIDENCE = {
    "index": ("validate_app_cli.py", "validate_app_jobdeck_sources.py"),
    "info": ("validate_app_render.py", "validate_app_deck_render.py"),
    "render": ("validate_app_render.py", "validate_app_captures.py", "validate_drc_captures.py"),
    "clip": ("validate_app_clip.py",), "probe": ("validate_app_render.py", "validate_app_deck_render.py"),
    "drc": ("validate_app_drc.py",), "svrf": ("validate_app_svrf.py",),
    "gtktest": ("validate_display_test.py", "validate_display_cli.py", "validate_display_input.py"),
    "view": ("validate_web_cli.py", "validate_web_startup.py", "validate_web_handoff.py",
             "validate_web_read_reviewer.py", "validate_web_ui.cjs"),
    "jobdeck": ("validate_app_jobdeck.py", "validate_app_jobdeck_plan.py"),
    "fe-embed": ("validate_fe_embed.py",),
}


def specs():
    return {cmd: [(tuple(names.split("|")), value if eq else None)
                  for names, eq, value in (token.partition("=") for token in text.split())]
            for cmd, text in SURFACE.items()}


def legacy_parsers():
    # Capture the actual rust_only constructor, stopping before dispatch. This
    # keeps helper-defined flags and conditional exclusions in the inventory.
    sys.path.insert(0, str(ROOT))
    from floe.cli import main
    class Captured(BaseException):
        pass
    found = []
    def capture(parser, *_args, **_kwargs):
        found.append(parser)
        raise Captured
    with patch.dict(os.environ, FLOE_RENDERER="rust"), patch.object(
            argparse.ArgumentParser, "parse_args", capture):
        try:
            main([], prog="floe2", rust_only=True)
        except Captured:
            pass
    parser, = found
    assert {tuple(a.option_strings) for a in parser._actions if a.option_strings} == {
        ("-h", "--help"), ("--version",)}
    parsers = dict(next(a for a in parser._actions if isinstance(a, argparse._SubParsersAction)).choices)
    assert set(parsers) == set(SURFACE) - {"fe-embed"}, "Public command set changed"
    from floe.fe_embed import build_parser
    parsers["fe-embed"] = build_parser()
    return parsers


def validate(parsers):
    assert set(parsers) == set(SURFACE), "Public commands changed; audit the new scope"
    total = Counter()
    for cmd, rows in specs().items():
        actions = parsers[cmd]._actions
        visible = [a for a in actions if a.option_strings and a.help != argparse.SUPPRESS
                   and not isinstance(a, argparse._HelpAction)]
        expected = {frozenset(names): (names[0], value) for names, value in rows}
        assert {frozenset(a.option_strings) for a in visible} == set(expected), cmd
        hidden = [a for a in actions if a.help == argparse.SUPPRESS]
        assert {s for a in hidden for s in a.option_strings} == HIDDEN.get(cmd, set()), cmd
        position, = [a for a in actions if not a.option_strings]
        assert position.dest == {"drc": "db", "jobdeck": "deck", "svrf": "deck", "gtktest": "png", "fe-embed": "png"}.get(cmd, "src")
        assert position.nargs == {"gtktest": "?", "view": "?", "fe-embed": "*"}.get(cmd)
        assert position.required == (cmd not in ("gtktest", "view", "fe-embed"))
        assert position.default is None
        for a in visible:
            name, value = expected[frozenset(a.option_strings)]
            assert a.nargs == (0 if value is None else None), (cmd, name, "arity")
            default = DEFAULTS.get(cmd, {}).get(name, False if value is None else None)
            assert type(a.default) is type(default) and a.default == default, (cmd, name, "default", a.default, default)
            assert (None if a.choices is None else tuple(a.choices)) == CHOICES.get((cmd, name)), (cmd, name, "choices")
            assert a.required == (cmd == "clip" and name == "--bbox"), (cmd, name, "required")
        for gate in EVIDENCE[cmd]:
            assert (ROOT / "tools" / gate).is_file(), (cmd, gate)
        total.update(public=len(visible), hidden=len(hidden))
    return total


def selftest(parsers):
    # Mutation checks ensure inventory/default drift is not a silent PASS.
    for change in ("added", "missing", "alias", "default"):
        broken = copy.deepcopy(parsers)
        if change == "added":
            broken["view"].add_argument("--not-audited")
        elif change == "missing":
            del broken["clip"]
        else:
            a = next(a for a in broken["view"]._actions if "--frames" in a.option_strings)
            if change == "alias":
                a.option_strings.append("--another-name")
            else:
                a.default = "off"
        try:
            validate(broken)
        except AssertionError:
            continue
        raise AssertionError(f"Inventory missed {change} mutation")


def native(parsers):
    count = 0
    with tempfile.TemporaryDirectory(prefix="floe-cli-inventory-") as td:
        root = Path(td)
        env = {k: v for k, v in os.environ.items() if not k.startswith("FLOE_")}
        env.update(PATH="", FLOE_INDEX_BIN=str(root / "no-index"),
                   FLOE_RENDERD_BIN=str(root / "no-renderd"), FLOE_FIREFOX_BIN=str(root / "no-browser"))
        def call(args, code=0, text="Usage:"):
            nonlocal count
            result = subprocess.run([str(APP), *args], cwd=root, env=env,
                                    capture_output=True, text=True, timeout=5)
            assert result.returncode == code and text in result.stdout + result.stderr, (
                args, result.returncode, result.stdout, result.stderr)
            assert not list(root.iterdir()), "Parser preflight wrote files"
            count += 1
        for cmd, rows in specs().items():
            if cmd == "gtktest":
                call([cmd, "--help"], 2, "displaytest [PNG]")
                continue
            call([cmd, "--help"])
            call([cmd, "--not-audited", "--help"], 2, "option")
            for names, value in rows:
                for flag in names:
                    values = CHOICES.get((cmd, names[0]), (value,))
                    for v in values:
                        args = [cmd, "synthetic", flag] + ([] if v is None else [str(v)]) + ["--help"]
                        rejected = INACTIVE.get(flag) if cmd == "view" else None
                        call(args, 2 if rejected else 0, rejected or "Usage:")
            for action in parsers[cmd]._actions:
                if action.help != argparse.SUPPRESS:
                    continue
                value = [] if action.nargs == 0 else [str(next(iter(action.choices), "1")) if action.choices else "1"]
                call([cmd, "synthetic", *action.option_strings[:1], *value, "--help"], 2, "option")
        call(["view", "synthetic", "--stream-kb", "1", "--help"])
        call(["--help"])
        call(["--version"], text="floe2-web")
    return count


def main():
    parsers = legacy_parsers()
    totals = validate(parsers)
    selftest(parsers)
    count = native(parsers)
    print(f"WEB CLI INVENTORY: ALL OK ({len(parsers) - 1} commands + fe-embed, {totals['public']} public options, "
          f"{totals['hidden']} rejected hidden options; {count} native parser probes)")
    print("Known boundaries: gtktest/PNG diagnostic replacement; stream-kb is legacy page-round compatibility, not a byte budget.")
    print("Parser surface/default drift checked, NOT command semantics or full G4/browser acceptance.")


if __name__ == "__main__":
    main()
