#!/usr/bin/env python3
"""GTK-source startup oracle + Rust CLI/native first-frame regression gate."""
import ast
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

from validate_web_cli import APP, INDEX, RENDERD, Client, read_json, wait
from validate_web_cli_inventory import legacy_parsers

ROOT = Path(__file__).resolve().parents[1]


def refinement_oracle():
    """Run the original CLI prefix and only the real worker round assignment.

    No GTK, socket, worker constructor or binary discovery is executed. This
    retains argparse's duplicate-option rule and cmd_view's stream/baseline
    precedence rather than reimplementing them in the native test driver.
    """
    sys.path.insert(0, str(ROOT))
    from floe import cli, rust_render
    parser = legacy_parsers()["view"]
    tree = ast.parse((ROOT / "floe/cli.py").read_text())
    method = next(n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name == "cmd_view")
    cut = next(i for i, n in enumerate(method.body) if isinstance(n, ast.Assign)
               and any(isinstance(t, ast.Name) and t.id == "server" for t in n.targets))
    method.body = method.body[:cut] + ast.parse("return stream_kb").body
    scope = dict(vars(cli))
    exec(compile(ast.fix_missing_locations(ast.Module(body=[method], type_ignores=[])),
                 "GTK refinement CLI prefix", "exec"), scope)
    tree = ast.parse((ROOT / "floe/rust_render.py").read_text())
    worker = next(n for n in tree.body if isinstance(n, ast.ClassDef) and n.name == "RustRenderWorker")
    init = next(n for n in worker.body if isinstance(n, ast.FunctionDef) and n.name == "__init__")
    assignment, = [n for n in init.body if isinstance(n, ast.Assign)
                   and any(isinstance(t, ast.Attribute) and t.attr == "_round_pages" for t in n.targets)]
    code = compile(ast.Module(body=[assignment], type_ignores=[]), "GTK worker round policy", "exec")
    def expected(args, env):
        with patch.dict(os.environ, env, clear=True):
            stream_kb = scope["cmd_view"](parser.parse_args(args))
            instance = SimpleNamespace()
            exec(code, dict(vars(rust_render), self=instance, stream_kb=stream_kb))
            return instance._round_pages
    return expected


def oracle(work):
    # Execute the real policy prefix, stopping before sockets, GUI or indexing.
    # Only test-owned empty paths are inspected; no monkey-patched policy math.
    sys.path.insert(0, str(ROOT))
    from floe import cli
    tree = ast.parse((ROOT / "floe/cli.py").read_text())
    method = next(n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name == "cmd_view")
    cut = next(i for i, n in enumerate(method.body) if isinstance(n, ast.Assign)
               and any(isinstance(t, ast.Name) and t.id == "server" for t in n.targets))
    method.body = method.body[:cut] + ast.parse(
        'return dict(depth=depth, frames=args.frames, labels=args.labels)').body
    scope = dict(vars(cli))
    exec(compile(ast.fix_missing_locations(ast.Module(body=[method], type_ignores=[])),
                 "GTK CLI startup policy", "exec"), scope)
    viewer = next(n for n in ast.parse((ROOT / "floe/gui.py").read_text()).body
                  if isinstance(n, ast.ClassDef) and n.name == "Viewer")
    names = {"_fit_spp", "fit", "goto", "view_bbox"}
    methods = [n for n in viewer.body if isinstance(n, ast.FunctionDef) and n.name in names]
    assert len(methods) == 4
    gui = {}
    exec(compile(ast.Module(body=methods, type_ignores=[]), "GTK camera oracle", "exec"), gui)
    init = next(n for n in viewer.body if isinstance(n, ast.FunctionDef) and n.name == "__init__")
    display = [n for n in init.body if isinstance(n, ast.Assign)
               and any(isinstance(t, ast.Attribute) and t.attr in ("frames_on", "labels_on") for t in n.targets)]
    assert len(display) == 2
    display_code = compile(ast.Module(body=display, type_ignores=[]), "GTK startup display", "exec")
    cases = []
    for deck, target, depth, frames, labels in itertools.product(
            (False, True), (None, "1.25,-2.5", "1.25,-2.5,80"),
            (None, -1, 0, 99, 999, 1000), ("on", "off"), ("on", "off")):
        source = work / ("mask.jb" if deck else "layout.oas")
        source.touch()
        baseline = len(cases) % 7 == 0
        drc = "errors.db" if len(cases) % 11 == 0 else None
        args = SimpleNamespace(src=str(source), hairline=None, thin_um=None, goto=target,
            stream_kb=None, stream_target_ms=500, label_font_px=14, perf_baseline=baseline,
            lod="on", frames=frames, labels=labels, refinement="on", frame_cache="on",
            render_debug=False, multi=True, drc=drc, detail="high", depth=depth, dump=False)
        policy = scope["cmd_view"](args)
        shown = SimpleNamespace()
        exec(display_code, dict(self=shown, frames=policy["frames"] == "on", labels=policy["labels"] == "on"))
        pixels = ([1001, 733], [800, 600], [600, 1000])[len(cases) % 3]
        bbox, dbu = [-111., 222., 8301., 7102.], .001
        obj = SimpleNamespace(cache=True, meta={"bbox":bbox}, dbu=dbu,
                              _viewport_size=lambda: pixels, redraw=lambda **_: None)
        for name in names:
            setattr(obj, name, MethodType(gui[name], obj))
        obj.fit()
        if target:
            obj.goto(*cli.parse_goto(target))
        argv = ["view", str(source), "--frames", frames, "--labels", labels]
        if target:
            argv += ["--goto", target]
        if depth is not None:
            argv += ["--depth", str(depth)]
        if drc:
            argv += ["--drc", drc]
        if baseline:
            argv += ["--perf-baseline"]
        cases.append(dict(argv=argv, deck=deck, pixels=pixels, bbox=bbox, dbu=dbu, want=dict(
            depth="full" if policy["depth"] >= 999 else str(policy["depth"]),
            frames=shown.frames_on, labels=shown.labels_on and not deck, bbox=obj.view_bbox())))
    path = work / "startup.json"
    path.write_text(json.dumps(cases))
    build = subprocess.run([shutil.which("cargo"), "test", "--offline", "--locked", "-p", "floe-app",
        "--bin", "floe2-web", "--no-run", "--message-format=json"], cwd=ROOT / "rust",
        capture_output=True, text=True, timeout=180)
    assert build.returncode == 0, (build.stdout, build.stderr)
    bins = [r["executable"] for line in build.stdout.splitlines()
            if (r := json.loads(line)).get("reason") == "compiler-artifact" and r.get("executable")]
    assert len(bins) == 1
    run = subprocess.run([bins[0], "gtk_startup_oracle", "--ignored", "--nocapture"],
        env=dict(os.environ, PATH="", FLOE_STARTUP_ORACLE=str(path)), capture_output=True, text=True, timeout=30)
    assert run.returncode == 0, (run.stdout, run.stderr)
    assert f"GTK STARTUP: ALL OK ({len(cases)} " in run.stdout
    print(run.stdout.strip())


def native(work, fixture):
    old_rounds = refinement_oracle()
    source = work / "native.oas"
    shutil.copy2(fixture, source)
    env = dict(os.environ, PATH="", FLOE_INDEX_BIN=str(INDEX), FLOE_RENDERD_BIN=str(RENDERD),
               FLOE_FILL_EDIT="", FLOE_RUST_ROUND_PAGES="1")
    env.pop("FLOE_JOBDECK_LEVELS", None)
    cache = Path(str(source) + ".floe")
    run = subprocess.run([str(INDEX), "vfs", str(source), str(cache), "--jobs", "2"],
                         env=env, capture_output=True, text=True, timeout=30)
    assert run.returncode == 0, (run.stdout, run.stderr)
    before = {p.name:p.read_bytes() for p in cache.iterdir() if p.is_file()}
    deck = work / "native.jb"
    deck.write_text("MTITLE 1,ONE\nMTITLE 2,TWO\nCHIP C\n"
                   "$ (1,P1,TC=native.oas,AD=0.001,LY={1},DT={0},UX=500,UY=500)\n"
                   "$ (2,P2,TC=native.oas,AD=0.001,LY={2},DT={0},UX=500,UY=500)\nROWS 0/0\n")
    inspection = ["--depth", "999", "--detail", "high"]
    cases = [
        (source, [], None, "0", True, True, False),
        (source, ["--goto", "1.25,-2.5"], None, "full", True, True, False),
        (source, ["--goto", "1.25,-2.5,80", "--depth", "0", "--frames", "off", "--labels", "on"], None, "0", False, False, False),
        (source, ["--perf-baseline", "--frames", "on", "--labels", "on", "--frame-cache", "on", "--depth", "999"], None, "full", False, False, False),
        (deck, [], None, "full", True, False, True),
        (deck, ["--mode", "layer"], "all", "full", True, False, False),
        (deck, [], "2", "full", True, False, False),
        (deck, ["--level", "1"], "invalid", "full", True, False, False),
        (source, ["--refinement", "on", *inspection], None, "full", True, True, False),
        (source, ["--refinement", "off", "--refinement", "on", *inspection], None, "full", True, True, False),
        (source, ["--refinement", "on", "--refinement", "off", *inspection], None, "full", True, True, False),
        (source, ["--stream-kb", "0", "--refinement", "on", *inspection], None, "full", True, True, False),
        (source, ["--perf-baseline", "--refinement", "on", *inspection], None, "full", False, False, False),
        (source, inspection, None, "full", True, True, False),
        (source, ["--refinement", "on", *inspection], None, "full", True, True, False),
    ]
    for i, (path, extra, policy, depth, frames, labels, ask) in enumerate(cases):
        temps = work / f"temps-{i}"
        temps.mkdir()
        session_file = work / f"session-{i}.json"
        child_env = dict(env, TMPDIR=str(temps))
        if i == 14:
            child_env.pop("FLOE_RUST_ROUND_PAGES")
        if policy is not None:
            child_env["FLOE_JOBDECK_LEVELS"] = policy
        # Both spellings must defeat FLOE_RUST_ROUND_PAGES=1 before
        # the first frame, including deck workers. No Python runtime fallback.
        direct_final = ([] if i >= 8 else
                        ["--stream-kb", "0"] if i % 2 else ["--refinement", "off"])
        expected_rounds = (old_rounds([str(path), *direct_final, *extra], child_env)
                           if i >= 8 else 1 << 30)
        debug = i in (1, 3, 5)
        args = [str(APP), "view", str(path), "--no-open", "--session-file", str(session_file),
                "--jobs", "1", "--raster-jobs", "1", "--frame-cache", "off"] + direct_final + extra
        if debug:
            args += ["--render-debug"]
        p = subprocess.Popen(args, env=child_env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            client = Client(wait(lambda: read_json(session_file), p))
            client.login()
            startup = client.call("GET", "/api/v1/startup")
            assert startup["confirm_levels"] is ask
            assert client.call("GET", "/api/v1/operations")["last_seq"] == "0"
            request = startup["request"]
            assert request["label_preference"] is (frames and "--labels=off" not in extra and "--no-labels" not in extra), request
            assert request["body"]["depth"] == depth
            assert request["body"]["frames"] is frames
            assert request["body"]["labels"] is labels
            if i == 6:
                assert request["levels"] == dict(mode="only", ids=["2"])
            if i == 7:
                assert request["levels"] == dict(mode="only", ids=["1"])
            if not ask:
                request["body"]["pixels"] = [257, 191]
                client.call("POST", "/api/v1/operations", request, 202)
                assert client.finished(1, p)["phase"] == "succeeded"
                view = wait(lambda: (lambda v: v if v["status"] == "idle" else None)(
                    client.call("GET", "/api/v1/view")["view"]), p)
                assert view["submitted"] == "1", view
                if expected_rounds == 1:
                    assert int(view["consumed"]) > 1, (i, expected_rounds, view)
                else:
                    assert expected_rounds == 1 << 30 and view["consumed"] == "1", (i, view)
                assert view["depth"] == depth and view["frames"] is frames and view["labels"] is labels
                assert not view["capabilities"]["margin"]
                if i == 1:
                    bbox = list(map(float, view["bbox_dbu"]))
                    dbu = float(view["dbu_um"])
                    assert abs((bbox[0] + bbox[2]) / 2 * dbu - 1.25) < 1e-9
                    assert abs((bbox[1] + bbox[3]) / 2 * dbu + 2.5) < 1e-9
            client.call("DELETE", "/api/v1/session", code=204)
            out, err = p.communicate(timeout=15)
            assert p.returncode == 0, (out, err)
            metrics = [line for line in err.splitlines() if line.startswith("[render-perf]")]
            assert len(metrics) == (1 if debug and not ask else 0), err
            assert "[render-perf]" not in out
            for line in metrics:
                assert len(line) < 4096
                fields = dict(word.split("=", 1) for word in line.split()[1:])
                assert all(value.isascii() and value.isdecimal() for value in fields.values()), line
                assert fields["gen"] == fields["round"] == fields["final"] == "1", fields
                assert fields["partial"] == fields["deferred"] == "0", fields
                assert fields["w"] == "257" and fields["h"] == "191", fields
                assert int(fields["worker_pid"]) > 0
                assert "plan_us" in fields and "raster_us" in fields, fields
                assert "path" not in fields and "view" not in fields
                assert str(work) not in line and source.name not in line and deck.name not in line
            assert not session_file.exists() and not list(temps.iterdir())
        finally:
            if p.poll() is None:
                p.terminate()
                p.communicate(timeout=15)
    assert {p.name:p.read_bytes() for p in cache.iterdir() if p.is_file()} == before
    assert source.read_bytes() == fixture.read_bytes()
    print(f"WEB STARTUP NATIVE: ALL OK ({len(cases)} launch cases, {len(cases)-1} first generations, "
          "GTK source-derived refinement/env/duplicates/zero/baseline parity, direct-final aliases, "
          "numeric debug opt-in, no implicit index, source/cache unchanged)")


def main(fixture):
    with tempfile.TemporaryDirectory(prefix="floe-web-startup-") as td:
        work = Path(td)
        oracle(work)
        native(work, fixture)


if __name__ == "__main__":
    main(Path(sys.argv[1]).resolve())
