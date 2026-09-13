#!/usr/bin/env python3
"""Rust dataset/deck read and worker parity, using generated inputs only."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from unittest.mock import patch

from validate_app_render import ROOT, INDEX, RENDERD, compare as compare_pair, digest, fake_worker_tests, run
from validate_app_jobdeck_plan import difference
from validate_jobdeck import (DECK, DENSE_DECK, FRAMES_DECK, MISSING_LAYER_DECK,
                             THIN_DECK, build_dense_oas, build_hier2_oas,
                             build_oas, build_thin_oas)
from floe.jobdeck.viewer import DeckCache
from floe.jobdeck.render import DeckRenderWorker
from floe.shots import ShotRunner
from floe import fillpat
from validate_layerprops import visible_layers, live_properties


def compare(source, work, env, tag, args, code=0):
    return compare_pair(source, work, env, tag, args, code,
                        python_env=dict(env, TMPDIR=str(work / "oracle-temp")))


def main():
    # Legacy Python's line protocol cannot use a whitespace TMPDIR. Keep its
    # workspace wire-safe; the source itself still exercises UTF-8/spaces.
    with tempfile.TemporaryDirectory(prefix="floe-app-deck-read-") as td:
        work = Path(td)
        temp = work / "worker-temp"
        temp.mkdir()
        (work / "oracle-temp").mkdir()
        env = {k: v for k, v in os.environ.items() if not k.startswith("FLOE_")}
        env.update(PATH="", FLOE_INDEX_BIN=str(INDEX), FLOE_RENDERD_BIN=str(RENDERD),
                   FLOE_PRODUCT="floe2",
                   PYTHONPATH=str(ROOT), PYTHONDONTWRITEBYTECODE="1", TMPDIR=str(temp))
        for name, dbu, w, h, layers in [
                ("chipA.oas", 5e-5, 2000, 2550, [(123, 43), (456, 0)]),
                ("chipB.oas", .001, 1000, 1000, [(456, 0), (7, 2)]),
                ("mark.oas", .002, 100, 100, [(999, 0)])]:
            build_oas(work / name, dbu, w, h, layers, "TOP")
        build_hier2_oas(work / "hier2.oas")
        build_thin_oas(work / "thin.oas")
        build_dense_oas(work / "dense.oas")
        for source in sorted(work.glob("*.oas")):
            run(["index", source, "--jobs", "2"], env)
        snapshots = {p: digest(p) for p in work.glob("*.floe")}
        decks = {}
        for name, text in (("read", DECK), ("frames", FRAMES_DECK),
                           ("thin", THIN_DECK), ("dense", DENSE_DECK),
                           ("missing-layer", MISSING_LAYER_DECK),
                           ("missing-source", DECK.replace("TC=mark.oas", "TC=absent.oas"))):
            path = work / (name + " 한 글.jb")
            path.write_text(text)
            decks[name] = path
        deck = decks["read"]
        no_worker = dict(env, FLOE_RENDERD_BIN="/invalid/not/needed", FLOE_INDEX_BIN="/invalid/not/needed")
        for levels in ([], ["--level", "1"], ["--level", "3,2"]):
            actual = run(["info", deck, *levels], no_worker).stdout
            expected = run(["info", deck, *levels], dict(env, TMPDIR=str(work / "oracle-temp")), python=True).stdout
            assert actual == expected, (actual, expected)
            info = json.loads(run(["info", deck, *levels, "--json"], no_worker).stdout)
            cache = DeckCache(str(deck), ids=None if not levels else [int(x) for x in levels[1].split(",")])
            try:
                cache.load()
                expected = json.loads(json.dumps(cache.meta))
                assert info["metadata"] == expected, difference(info["metadata"], expected)
                assert info["cache"] is None and info["source_stale"] is False
            finally:
                cache.close()
        png = compare(deck, work, env, "fit", ["--px", "128x128"])
        for name, args in [
                ("load-one", ["--level", "1"]), ("load-two", ["--level", "3,2"]),
                ("level-name", ["--layers", "METAL1"]),
                ("level-old", ["--layers", "$1 METAL1"]),
                ("source-name", ["--layers", "chipA.oas"]),
                ("head", ["--layers", "1/0"]), ("leaf", ["--layers", "1/1"]),
                ("none", ["--layers", ","]), ("all", ["--layers", "all"]),
                ("half", ["--bbox", "-10.9375,-10.9375,100000,100000", "--stretch"]),
                ("outside", ["--bbox", "1000000,1000000,1000100,1000100"])]:
            compare(deck, work, env, name, ["--px", "101x83", *args])
        for depth in ("0", "1", "999"):
            compare(decks["frames"], work, env, "frames-"+depth,
                    ["--px", "129x111", "--depth", depth, "--frames"])
        for thin in ("auto", "keep", "cull"):
            compare(decks["thin"], work, env, "thin-"+thin,
                    ["--px", "200x200", "--detail", "high", "--thin", thin])
        for name in ("missing-layer", "missing-source"):
            compare(decks[name], work, env, name, ["--px", "128x128"], code=3)
            p = run(["probe", decks[name]], env, code=3)
            assert "[probe] OK" not in p.stdout and "incomplete" in p.stderr
        whole = compare(decks["dense"], work, env, "dense", ["--px", "120x120"])
        streamed = compare(decks["dense"], work, dict(env, FLOE_RUST_BUDGET_MB="1"),
                           "dense-streamed", ["--px", "120x120"])
        assert whole.read_bytes() == streamed.read_bytes()
        partial = compare(decks["dense"], work,
                          dict(env, FLOE_RUST_BUDGET_MB="1", FLOE_RUST_DECK_STREAM="off"),
                          "dense-partial", ["--px", "120x120"], code=3)
        assert partial.read_bytes() != whole.read_bytes()

        # Mode-specific props, stable leaf IDs and separate fill/width head
        # inheritance. Runtime Rust API must not read the old CHIP namespace.
        cases = []
        with patch.dict(os.environ, env, clear=True):
            for mode in ("level", "chip", "layer"):
                for selection in (None, [2, 3]):
                    cache = DeckCache(str(deck), mode=mode, ids=selection)
                    props = Path(cache.props_src + ".layerprops")
                    property_lines = ["1.0 red diagonal_1 HEAD 0 8", "1.1 skyblue INVALID CHILD 1 1",
                                      "2.0 yellow plus TWO 0 5", "2.1 cyan clear LEAF 0 2",
                                      "2.1 cyan INVALID LEAF 1 1", "123.43 orange brick RAW 0 3"]
                    if selection is not None:
                        # Explicit child flags win even when a hidden head
                        # appears later in the input file.
                        property_lines.sort(key=lambda s: s.split()[0] in ("1.0", "2.0"))
                    props.write_text("\n".join(property_lines) + "\n")
                    try:
                        cache.load()
                        worker = DeckRenderWorker(cache)
                        styles = [{"layer": list(k), "color": worker._colors[k],
                                   "fill": worker._fills.get(k, "speckle"),
                                   "width": worker._widths.get(k, 1)}
                                  for k in sorted(worker._colors)]
                        box = [v * cache.meta["dbu"] for v in cache.meta["bbox"]]
                        visible = visible_layers(cache.meta, fillpat.parse_layerprops(props.read_text()))
                        runner = ShotRunner(cache)
                        try:
                            out = work / (mode + str(selection) + "-api.png")
                            data, result = runner.capture(box, 103, 91)
                            assert not result.get("over_budget_pages") and not result.get("labels_truncated")
                            out.write_bytes(data)
                            view_out = work / (mode + str(selection) + "-view-api.png")
                            view_data, view_result = runner.capture(box, 103, 91, layers=visible)
                            assert not view_result.get("over_budget_pages") and not view_result.get("labels_truncated")
                            view_out.write_bytes(view_data)
                        finally:
                            runner.stop()
                        cases.append({"source": str(deck), "mode": mode, "levels": selection,
                                      "metadata": cache.meta, "styles": styles,
                                      "bbox": box, "png": str(out), "view_png": str(view_out), "visible": visible,
                                      "live": live_properties(cache, work / (mode + str(selection) + "-live"))})
                    finally:
                        cache.close()
        oracle = work / "dataset-oracle.json"
        oracle.write_text(json.dumps(cases))
        cargo = os.environ.get("CARGO", str(Path.home() / ".cargo/bin/cargo"))
        p = subprocess.run([cargo, "test", "--offline", "-p", "floe-app-core", "--test",
                            "jobdeck_dataset", "--", "--ignored", "--nocapture"],
                           cwd=ROOT / "rust", env=dict(os.environ, FLOE_APP_DATASET_ORACLE=str(oracle),
                                                     FLOE_RENDERD_BIN=str(RENDERD)),
                           text=True, capture_output=True, timeout=90)
        assert p.returncode == 0, p.stdout + p.stderr
        assert "RUST APP DECK DATASET: ALL OK (6 cases)" in p.stdout
        assert "+ 6 managed controllers" in p.stdout
        for props in work.glob("*.layerprops"):
            props.unlink()

        # Explicit unsupported/invalid requests fail before worker spawn or
        # file creation, even when the configured binary is invalid.
        for args in (["--labels"], ["--level", "999"], ["--layers", "unknown"],
                     ["--level", "NaN"], ["--mode", "chip"]):
            target = work / "invalid.png"
            run(["render", deck, *args, "--out", target], no_worker, code=2)
            assert not target.exists()
        run(["info", work / "chipA.oas", "--level", "1"], env, code=2)
        for target in (deck, work / "chipA.oas", work / "chipA.oas.floe/design.ovm",
                       work / "absent.oas", Path(str(deck)+".layerprops")):
            run(["render", decks["missing-source"] if target.name == "absent.oas" else deck,
                 "--out", target], no_worker, code=2)
        result = run(["probe", deck], env)
        assert result.stdout.count("frame OK") == 2 and "[probe] OK" in result.stdout
        info = json.loads(run(["info", deck, "--json"], no_worker).stdout)
        fake_worker_tests(deck, work, env, png, info["metadata"]["dbu"], deck=True)
        assert not list(temp.iterdir()), "worker workspace leaked"
        assert all(digest(path) == before for path, before in snapshots.items())
    print("RUST APP DECK READ: ALL OK (23 PNG/report pairs + 6 API cases + 8 signal phases)")


if __name__ == "__main__":
    main()
