#!/usr/bin/env python3
"""Rust jobdeck analysis/spec parity. Generated sources; Python is oracle only."""
import gzip
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from floe.jobdeck import color, plan, render
from floe.jobdeck.viewer import DeckCache, level_rows
from validate_jobdeck import DECK, FORMAT_DECK, MISSING_LAYER_DECK, build_oas

APP = ROOT / "rust/target/release/floe2-web"
INDEX = ROOT / "rust/target/release/floe-index"
RENDERD = ROOT / "rust/target/release/floe-renderd"


def difference(a, b, path=""):
    if isinstance(a, dict) and isinstance(b, dict):
        if a.keys() != b.keys():
            return path, sorted(a), sorted(b)
        for k in a:
            if a[k] != b[k]:
                return difference(a[k], b[k], path + "/" + k)
    elif isinstance(a, list) and isinstance(b, list):
        if len(a) != len(b):
            return path, len(a), len(b)
        for i, (x, y) in enumerate(zip(a, b)):
            if x != y:
                return difference(x, y, path + "/" + str(i))
    return path, a, b


def normalized_report(report):
    report = json.loads(json.dumps(report, allow_nan=False))
    sources = report["plan"]["sources"]
    sources.pop("probe_s")
    for row in sources["files"]:
        row.pop("probe_s")
        # M1a-3b's explicit diagnostic delta: native always reports the .floe
        # destination of a regular source, including a corrupt cache for which
        # the Python try/load block leaves cache_dir empty.
        if row["cache_dir"].endswith(".tiles") or (not row["cache_dir"] and Path(row["path"]).is_file()):
            row["cache_dir"] = os.path.abspath(row["path"]) + ".floe"
    return report


def spec_rows(text):
    rows = []
    for line in text.splitlines():
        if not line.strip() or line.startswith("#"):
            continue
        head, *fields = line.split()
        row = dict(field.split("=", 1) for field in fields)
        # Transport float spelling is not the contract: Rust Debug and Python
        # repr both round-trip, with different exponent zero padding.
        for key in ("unit", "scale"):
            if key in row:
                row[key] = float(row[key])
        rows.append((head, row))
    return rows


def run(args, env, code=0):
    p = subprocess.run([str(APP)] + list(map(str, args)), cwd=ROOT, env=env,
                       capture_output=True, text=True, timeout=60)
    assert p.returncode == code, (args, p.returncode, p.stdout, p.stderr)
    return p


def digest(directory):
    return {str(p.relative_to(directory)): (hashlib.sha256(p.read_bytes()).hexdigest(), p.stat().st_mtime_ns)
            for p in directory.rglob("*") if p.is_file() and (".floe" in str(p) or p.suffix in (".oas", ".jb"))}


def main():
    with tempfile.TemporaryDirectory(prefix="floe-deck-analysis 한 글-") as td:
        work = Path(td)
        sources = work / "source dir"
        sources.mkdir()
        for name, dbu, w, h, layers in [
                ("chipA.oas", 5e-5, 2000, 2550, [(123, 43), (456, 0)]),
                ("chipB.oas", .001, 1000, 1000, [(456, 0), (7, 2)]),
                ("mark.oas", .002, 100, 100, [(999, 0)])]:
            build_oas(sources / name, dbu, w, h, layers, "TOP")
        build_oas(sources / "chipA.gds", 5e-5, 2000, 2550, [(123, 43)], "TOP")
        for name in ("chipA.oas", "chipA.gds"):
            (sources / (name + ".gz")).write_bytes(gzip.compress((sources / name).read_bytes()))
        (sources / "junk.bin").write_bytes(b"unknown format")
        deck = work / "analysis deck.jb"
        deck.write_text(DECK)
        # Index sources separately: deck CLI has no --sources override (legacy).
        index_env = dict(os.environ, PATH="", FLOE_INDEX_BIN=str(INDEX))
        env = dict(index_env, FLOE_INDEX_BIN="/invalid/index/must/not/run",
                   FLOE_RENDERD_BIN="/invalid/renderd/must/not/run",
                   PYTHONDONTWRITEBYTECODE="1", PYTHONPATH=str(ROOT))
        for name in ("chipA.oas", "chipB.oas", "mark.oas"):
            run(["index", sources / name, "--jobs", "2"], index_env)
        saved = work / "palette.json"
        scheme_json = {"palette": "reserve", "mode": "chip", "cross_ly_dt": False,
                       "overrides": {"1": "#ABCDEF", "ID001": "#123456", "456/0": "#ffeedd"}}
        saved.write_text(json.dumps(scheme_json))
        oracle_cases = []
        expected_pngs = []
        report_count = 0
        spec_count = 0
        before = digest(sources)

        def check(name, text, mode="level", selected=None, load=None, palette=False, spec=True, code=0, strict=True):
            nonlocal report_count, spec_count
            deck.write_text(text)
            scheme = color.ColorScheme.load(saved) if palette else None
            old = plan.plan_deck(str(deck), sources_dir=str(sources), ids=selected,
                                 load_ids=load, mode=mode, scheme=scheme, strict=strict,
                                 missing="skip", cross=True)
            d, catalog, placements, stats, scheme, colors = old
            report = plan.report_dict(d, placements, stats)
            rows = render.view_layers(d, stats, scheme, colors)
            lines, ledger = render.deck_spec_lines(d, placements, stats, scheme, colors, catalog)
            oracle_cases.append({"name": name, "path": str(deck), "text": text, "sources": str(sources),
                                 "mode": mode, "ids": selected, "load_ids": load, "strict": strict,
                                 "scheme": scheme_json if palette else None,
                                 "report": normalized_report(report), "rows": rows,
                                 "levels": level_rows(d), "spec": "\n".join(lines)+"\n", "ledger": ledger})
            # Load selection belongs to view/info/render, not analysis CLI.
            if load is not None:
                return
            out = work / (name + ".json")
            sp = work / (name + ".spec")
            args = ["jobdeck", deck, "--sources", sources, "--mode", mode, "--report", out, "--placements"]
            if selected:
                args += ["--id", ",".join(map(str, selected))]
            if palette:
                # Saved scheme wins cross/zip, while command mode wins mode.
                args += ["--colors", saved, "--ly-dt", "cross"]
            if not strict:
                args += ["--lenient"]
            if spec:
                args += ["--spec", sp]
            p = run(args, env, code)
            actual, expected = normalized_report(json.loads(out.read_text())), normalized_report(report)
            assert actual == expected, (name, difference(actual, expected))
            report_count += 1
            if spec:
                assert spec_rows(sp.read_text()) == spec_rows("\n".join(lines)), name
                spec_count += 1
                if name in ("level-all", "chip-all", "layer-all"):
                    expected_pngs.append((sp, "\n".join(lines)+"\n", stats, render.deck_layers_meta(d, stats, scheme, colors, placements)))
            assert "instances :" in p.stdout and "view      :" in p.stdout

        for mode in ("level", "chip", "layer"):
            for ids, name in ((None, "all"), ([1], "one"), ([2, 3], "two"), ([99], "unknown")):
                check(mode + "-" + name, DECK, mode, ids, spec=ids != [99])
            check(mode + "-pins", DECK, mode, [2, 3], palette=True)
            check(mode + "-load", DECK, mode, load=[2, 3], palette=True)
        check("formats", FORMAT_DECK, code=3)
        check("empty-layer", MISSING_LAYER_DECK, code=3)
        # Preserve first-use ordinal, normalized source aliases, duplicate CHIP
        # IDs and repeated leaves. It is not one leaf per CHIP block.
        alias = DECK.replace("TC=chipA.oas", "TC=./chipA.oas", 1).replace("CHIP ID003", "CHIP ID001")
        check("aliases", alias, "chip")
        # Lenient syntax errors must be visible but never produce placements.
        check("lenient", DECK.replace("ROWS 85120.0/45020.0", "SF=2\nROWS 85120.0/45020.0", 1), strict=False)
        # An unselected source contributes header DBU even with corrupt cache.
        mark_cache = Path(str(sources / "mark.oas") + ".floe")
        original_meta = (mark_cache / "meta.json").read_bytes()
        (mark_cache / "meta.json").write_bytes(b"bad metadata")
        check("unselected-cache", DECK, load=[1])
        check("not-indexed", DECK, code=3)
        (mark_cache / "meta.json").write_bytes(original_meta)
        # Save per-case deck files: analysis service opens paths, not strings.
        for i, case in enumerate(oracle_cases):
            path = work / ("case-%d.jb" % i)
            path.write_text(case.pop("text"))
            case["path"] = str(path)
            case["report"]["deck"]["path"] = str(path)
            case["bad_mark_cache"] = case["name"] in ("unselected-cache", "not-indexed")
        oracle = work / "oracle.json"
        oracle.write_text(json.dumps({"cases": oracle_cases, "mark_cache": str(mark_cache)}, allow_nan=False))
        subprocess.run(["cargo", "test", "--offline", "--locked", "-p", "floe-app-core",
                        "--test", "jobdeck_analysis", "--", "--ignored", "--nocapture"], cwd=ROOT / "rust",
                       env=dict(os.environ, FLOE_APP_ANALYSIS_ORACLE=str(oracle)), check=True, timeout=120)

        # Both generated specs go through the real daemon; color/order/grid
        # mistakes must not hide behind a text-only equality oracle.
        render_env = dict(os.environ, FLOE_RENDERD_BIN=str(RENDERD), FLOE_RUST_JOBS="2")
        previous = dict(os.environ)
        os.environ.update(render_env)
        try:
            for i, (rust_spec, old_spec_text, stats, layers) in enumerate(expected_pngs):
                py_spec = work / ("python-%d.spec" % i)
                py_spec.write_text(old_spec_text)
                bbox = [v / stats["dbu"] for v in render.fit_bbox_to_pixels(stats["bbox_um"], 320, 300)]
                pngs = []
                for n, sp in enumerate((rust_spec, py_spec)):
                    out = work / ("pixel-%d-%d.png" % (i, n))
                    result = render.render_deck_png(str(sp), str(deck), stats["dbu"], layers, bbox, 320, 300, str(out))
                    # Python's public result maps native final to refining=0
                    # and deferred to over_budget_pages; it has no final key.
                    assert result["kind"] == "frame" and not result.get("refining")
                    assert not result.get("over_budget_pages") and not result.get("labels_truncated")
                    pngs.append(out.read_bytes())
                assert pngs[0] == pngs[1], ("native composite PNG", i)
        finally:
            os.environ.clear()
            os.environ.update(previous)

        deck.write_text(DECK)
        # Invalid input / protected exports never alter old output artifacts.
        report, spec = work / "preserved.json", work / "preserved.spec"
        report.write_bytes(b"old report")
        spec.write_bytes(b"old spec")
        common = ["jobdeck", deck, "--sources", sources]
        for args in (["--level", ""], ["--on-missing", "guess"], ["--mode", "no"], ["--colors", saved, "--colors", "absent"],
                     ["--report", report, "--spec", report], ["--report", report, "--spec", sources / "chipA.oas"]):
            # Missing color file is I/O(1); malformed CLI/spec targets input(2).
            run(common + args, env, 1 if args[-1] == "absent" else 2)
            assert report.read_bytes() == b"old report" and spec.read_bytes() == b"old spec"
        targets = [deck, sources / "chipA.oas", sources / "chipA.oas.floe/meta.json",
                   sources / "chipA.oas.floe.index.lock", saved]
        linked = work / "cache alias"
        linked.symlink_to(sources / "chipA.oas.floe", target_is_directory=True)
        targets.append(linked / "protected.json")
        target_link = work / "output-link"
        target_link.symlink_to(report)
        targets.append(target_link)
        for target in targets:
            run(common + ["--colors", saved, "--report", target], env, 2)
        missing_deck = DECK.replace("TC=mark.oas", "TC=absent.oas")
        deck.write_text(missing_deck)
        run(common + ["--on-missing", "fail", "--report", report], env, 2)
        run(common + ["--report", sources / "absent.oas"], env, 2)
        run(common + ["--report", report], env, 3)
        assert json.loads(report.read_text())["plan"]["skipped"]
        bad_color = work / "bad-color.json"
        for saved_bad in ({"palette": []}, {"palette": ["#000000\nsource"]}, {"palette": ["\u001b"]}, {"palette": "unknown"}):
            bad_color.write_text(json.dumps(saved_bad))
            run(common + ["--colors", bad_color, "--report", report], env, 2)
        # Restore meta mtime comparison is intentionally excluded; no source or
        # geometry cache contents are allowed to change from read operations.
        after = digest(sources)
        assert {k: v[0] for k, v in after.items()} == {k: v[0] for k, v in before.items()}
        for k in before:
            if not k.endswith("mark.oas.floe/meta.json"):
                assert before[k] == after[k], k
        assert not list(work.rglob(".floe-shot-*.tmp"))
        print("RUST APP JOBDECK ANALYSIS: ALL OK (%d reports, %d specs, 3 native PNG pairs)" % (report_count, spec_count))


if __name__ == "__main__":
    main()
