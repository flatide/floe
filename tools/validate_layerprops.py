#!/usr/bin/env python3
"""Layer properties: real Python codec/GTK visibility oracle, no GUI import.

All source/cache/props inputs are private synthetic copies. The Rust runtime
runs with PATH empty and must not mutate any of them. No browser file upload,
design-default publication, or user directory writes are performed.
"""
import ast
import copy
import hashlib
import json
import os
from pathlib import Path
import random
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from floe import fillpat


def gtk_view(metadata, rows):
    """Run the actual GTK methods with only their checkbox boundary mocked."""
    names = {"_apply_props_visibility", "_sync_jobdeck_groups", "_is_jobdeck_head",
             "_load_props_dialog", "_push_fills", "_refresh_row_fills"}
    tree = ast.parse((ROOT / "floe/gui.py").read_text())
    methods = [n for n in ast.walk(tree) if isinstance(n, ast.FunctionDef) and n.name in names]
    assert len(methods) == len(names)
    scope = {"fillpat": fillpat}
    exec(compile(ast.Module(body=methods, type_ignores=[]), "GTK props oracle", "exec"), scope)
    legacy = type("LegacyVisibility", (), {name: scope[name] for name in names})()
    legacy.meta = metadata
    keys = {(r["layer"], r["datatype"]) for r in metadata["layers"]}
    heads = {(r["layer"], r["datatype"]) for r in metadata["layers"] if r.get("jobdeck_head")}
    legacy.visible = set(keys)
    legacy._layer_groups = {key: sorted(k for k in keys if k[0] == key[0] and k != key) for key in heads}

    class Checkbox:
        def __init__(self, key):
            self.key = key

        def get_active(self):
            return self.key in legacy.visible

        def set_active(self, value):
            (legacy.visible.add if value else legacy.visible.discard)(self.key)

        def set_group_state(self, active, partial):
            assert active == (self.key in legacy.visible)

        def set_color(self, color):
            pass

        def set_fill(self, fill):
            pass

    legacy._layer_rows = {key: Checkbox(key) for key in keys}
    legacy._apply_props_visibility(rows)
    return legacy


def visible_layers(metadata, rows):
    gui = gtk_view(metadata, rows)
    return sorted(gui.visible - set(gui._layer_groups))


def live_properties(cache, directory):
    """Actual GTK load + actual adapter expansion; only UI/pipe I/O mocked."""
    from floe import cache as cache_mod
    from floe.rust_render import RustRenderWorker, _pattern_fill
    from floe.jobdeck.render import DeckRenderWorker
    directory.mkdir()
    props = cache_mod.load_layer_props(getattr(cache, "props_src", cache.src))[0]
    gui = gtk_view(copy.deepcopy(cache.meta), props)
    gui._layer_patterns = {}
    gui._layer_widths = {}
    gui._fill_patterns = fillpat.default_patterns()
    for key, _color, fill, _name, _visible, width in props:
        index = fillpat.fill_index(fill)
        if index is not None:
            gui._layer_patterns[tuple(key)] = index
        try:
            if int(width) > 1:
                gui._layer_widths[tuple(key)] = int(width)
        except ValueError:
            pass
    gui._color_epoch = 0
    gui.redraw = lambda **kw: None
    gui._set_live_status = lambda message: None
    worker = DeckRenderWorker(cache) if cache.meta.get("jobdeck") else RustRenderWorker(cache)
    worker.alive = lambda: True
    worker._publish_style = lambda **kw: None
    gui.worker = worker
    keys = sorted(gui._layer_rows)
    rng = random.Random(191)
    outputs = []
    wire_names = {}
    for name, bitmap in fillpat.FILL_PATTERNS:
        wire_names.setdefault(_pattern_fill(bitmap), name)
    for step in range(16):
        lines = []
        # A valid fill keeps GTK's load from taking its width-only early
        # return. That GTK bug is covered as an intentional fix in Rust units.
        lines.append("%d.%d INVALID speckle KEEP ? bad" % keys[-1])
        for _ in range(8):
            key = rng.choice(keys)
            lines.append("%d.%d %s %s MASK %s %s" % (*key, rng.choice(["red", "blue", "INVALID"]),
                         rng.choice(["solid", "clear", "diagonal_1", "brick", "INVALID"]),
                         rng.choice(["0", "1", "?"]), rng.choice(["0", "1", "3", "8", "bad"])))
        text = "\n".join(lines)
        path = directory / (str(step) + ".layerprops")
        path.write_text(text)
        gui._props_chooser = lambda save: str(path)
        gui._load_props_dialog()
        assert worker.res.empty(), list(worker.res.queue)
        outputs.append(dict(text=text, visible=sorted(gui.visible - set(gui._layer_groups)),
                            styles=[dict(layer=key, color=worker._colors[key],
                                         fill=wire_names[worker._fills.get(key, "speckle")],
                                         width=worker._widths.get(key, 1)) for key in sorted(worker._colors)]))
    worker.stop()
    return outputs


def row_dict(row):
    key, color, fill, name, visibility, width = row
    return dict(layer=key, color=color, fill=fill, name=name, visibility=visibility, width=width)


def codec_cases():
    texts = ["", "# comment\n\n", "bad\n3 red\n", "2 blue solid", "7.20 SKYBLUE diagonal_1 MASK 0 3 extra",
             "7.2 red solid X 0 -2\n7.2 unknown INVALID Y ? bad", "+0008.00.tail #1A2b3C clear 한글 1 +0009",
             "0 red solid Z 0 " + "9" * 4096]
    rng = random.Random(91)
    for _ in range(64):
        lines = []
        for _ in range(rng.randint(1, 32)):
            key = str(rng.randrange(16)) + rng.choice(["", ".0", ".20", ".20.ignored"])
            columns = [key, rng.choice(["RED", "#123456", "unknown"]),
                       rng.choice(["SOLID", "clear", "speckle", "brick", "INVALID"]),
                       rng.choice(["M1", "한글", "MASK;alias"]),
                       rng.choice(["0", "1", "?", "10"]), rng.choice(["1", "0", "-2", "8", "99", "bad"])]
            lines.append(rng.choice([" ", "\t", "  "]).join(columns[:rng.randint(3, 6)]))
        texts.append("\n".join(lines))
    cases = []
    for text in texts:
        rows = fillpat.parse_layerprops(text)
        cases.append(dict(text=text, rows=[row_dict(r) for r in rows], formatted=fillpat.format_layerprops(rows),
                          widths=[max(1, min(8, int(r[5]))) if r[5].lstrip("+-").isdigit() else None for r in rows]))
    styles = []
    for name, color in fillpat.COLOR_TABLE:
        for fill, bitmap in fillpat.FILL_PATTERNS:
            rgb = [int(color[i:i+2], 16) for i in (1, 3, 5)] + [255]
            words = [int(w, 16) for w in fillpat.rows_to_hex(bitmap).split()]
            first_fill = next(n for n, b in fillpat.FILL_PATTERNS if b == bitmap)
            row = ((7, 20), fillpat.color_name(color), first_fill, "mask with spaces", "0", "4")
            styles.append(dict(color=rgb, words=words, row=row_dict(row), formatted=fillpat.format_layerprops([row])))
    return cases, styles


def digest(directory):
    return {str(p.relative_to(directory)): (hashlib.sha256(p.read_bytes()).hexdigest(), p.stat().st_mtime_ns)
            for p in directory.rglob("*") if p.is_file()}


def main(fixture):
    cargo = os.environ.get("CARGO", shutil.which("cargo") or str(Path.home() / ".cargo/bin/cargo"))
    build = subprocess.run([cargo, "test", "--offline", "--locked", "-p", "floe-app-core", "--test", "layerprops",
                            "--no-run", "--message-format=json"], cwd=ROOT / "rust", capture_output=True, text=True, timeout=180)
    assert build.returncode == 0, build.stderr
    binaries = [r["executable"] for line in build.stdout.splitlines()
                if (r := json.loads(line)).get("reason") == "compiler-artifact"
                and r["target"]["name"] == "layerprops" and r.get("executable")]
    assert len(binaries) == 1
    with tempfile.TemporaryDirectory(prefix="floe-layerprops-") as td:
        work = Path(td)
        env = {k: v for k, v in os.environ.items() if not k.startswith("FLOE_")}
        env.update(PATH="", FLOE_INDEX_BIN=str(ROOT / "rust/target/release/floe-index"), TMPDIR=str(work))
        views = []
        for i in range(4):
            source = work / ("설계 %s.oas" % i)
            shutil.copy2(fixture, source)
            p = subprocess.run([str(ROOT / "rust/target/release/floe2-web"), "index", str(source), "--jobs", "1"],
                               env=env, capture_output=True, text=True, timeout=30)
            assert p.returncode == 0, (p.stdout, p.stderr)
            meta = json.loads(Path(str(source) + ".floe/meta.json").read_text())
            keys = [(r["layer"], r["datatype"]) for r in meta["layers"]]
            text = "".join("%s.%s red speckle MASK %s 1\n" % (*k, "0" if i == 3 or j % 2 == 0 else "1")
                           for j, k in enumerate(keys))
            if i == 0:
                text = ""  # No implicit personal palette lookup.
            elif i == 1:
                source.with_suffix(".layerprops").write_text(text)
            else:
                source.with_suffix(".layerprops").write_text("0 red solid ignored 1 1\n")
                Path(str(source) + ".layerprops").write_text(text)
            from floe.cache import Cache
            cache = Cache(str(source))
            cache.load()
            views.append(dict(source=str(source), visible=visible_layers(meta, fillpat.parse_layerprops(text)),
                              live=live_properties(cache, work / ("live-" + str(i)))))
        cases, styles = codec_cases()
        oracle = work / "oracle.json"
        oracle.write_text(json.dumps(dict(cases=cases, styles=styles, views=views)))
        before = digest(work)
        result = subprocess.run([binaries[0], "--ignored", "--nocapture"],
                                env=dict(env, FLOE_LAYERPROPS_ORACLE=str(oracle)), capture_output=True, text=True, timeout=30)
        assert result.returncode == 0, (result.stdout, result.stderr)
        assert "LAYERPROPS: ALL OK (72 documents, 980 styles, 4 native view models)" in result.stdout, result.stdout
        assert digest(work) == before, "native property reading modified an input/cache or left temporary files"
        print(result.stdout.strip())


if __name__ == "__main__":
    main(Path(sys.argv[1]).resolve())
