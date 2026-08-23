#!/usr/bin/env python3
"""End-to-end KLayout oracle for the Rust renderer.

The geometry half reuses the parent's PX1-PX5 fixtures and fixed P-a/P-b/P-c
policy.  The styled half renders the same serialized OASIS through KLayout and
the Rust cache path, then permits RGB differences only in the union of the
per-layer 1px geometry boundary bands expanded by each device-line radius.
Speckle, custom pattern, visibility, paint order, and mono therefore have to
match exactly everywhere that cannot be touched by an accepted edge choice.
"""

import argparse
import importlib.util
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

import numpy as np
from PIL import Image


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_PARENT = ROOT


class OracleFailure(RuntimeError):
    pass


def _load_parent_goldens(parent):
    path = parent / "tools" / "validate_render_goldens.py"
    spec = importlib.util.spec_from_file_location(
        "floe_parent_render_goldens", path)
    if spec is None or spec.loader is None:
        raise OracleFailure("cannot load parent oracle: %s" % path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _run(command):
    result = subprocess.run(
        [str(item) for item in command], capture_output=True, text=True)
    if result.returncode:
        detail = "\n".join(
            part.strip() for part in (result.stdout, result.stderr)
            if part.strip())
        raise OracleFailure(
            "command failed (%d): %s%s" %
            (result.returncode, " ".join(str(item) for item in command),
             ("\n" + detail) if detail else ""))
    return result


def _require_executable(path, label):
    if not path.is_file() or not os.access(path, os.X_OK):
        raise OracleFailure("%s not executable: %s" % (label, path))


def _index(indexer, source, cache, jobs):
    _run([indexer, "vfs", source, cache, "--jobs", jobs])


def _render_rust(renderer, cache, output, view, width, height, jobs,
                 layers="1/0", styles=(), mono=False):
    command = [
        renderer, cache,
        "--view", ",".join(repr(float(value)) for value in view),
        "--width", width, "--height", height,
        "--depth", "full", "--cut-px", "0",
        "--decode-pages", "1000000000",
        "--budget-mb", "1024", "--jobs", jobs,
        "--tile-px", "128", "--frames", "off",
        "--mono", "on" if mono else "off", "--out", output,
    ]
    if layers is not None:
        command.extend(("--layers", layers))
    for style in styles:
        command.extend(("--style", style))
    _run(command)


def _geometry_oracle(work, parent_oracle, indexer, renderer, jobs):
    golden_dir = work / "px-golden"
    candidate_dir = work / "px-rust"
    candidate_dir.mkdir()
    parent_oracle.bake(str(work), str(golden_dir))

    caches = {}
    for case, _builder in parent_oracle._fixtures(
            __import__("klayout.db", fromlist=["db"])):
        source = work / (case + ".oas")
        cache = work / (case + ".oas.floe")
        _index(indexer, source, cache, jobs)
        caches[case] = cache

    failures = []
    for name, case, view, width, height in parent_oracle.RENDERS:
        output = candidate_dir / (name + ".png")
        _render_rust(
            renderer, caches[case], output, view, width, height, jobs,
            styles=("1/0,#ffffff,solid,1",))
        golden = parent_oracle.load_bin(golden_dir / (name + ".png"))
        candidate = parent_oracle.load_bin(output)
        ok, reason = parent_oracle.compare(golden, candidate)
        print("PX %-20s %s" % (name, reason))
        if not ok:
            failures.append("%s: %s" % (name, reason))
    if failures:
        raise OracleFailure("PX oracle failed:\n" + "\n".join(failures))
    return len(parent_oracle.RENDERS)


def _style_pattern():
    return "\n".join(
        "".join("*" if (x - y) % 5 in (0, 1) else "."
                for x in range(16))
        for y in range(16))


def _pattern_wire(rows):
    words = []
    for row in rows.splitlines():
        word = 0
        for pixel in row:
            word = (word << 1) | (pixel == "*")
        words.append(word)
    return "pat:" + "".join("%04X" % word for word in words)


def _build_style_fixture(source):
    import klayout.db as db

    layout = db.Layout()
    layout.dbu = 0.001
    top = layout.create_cell("TOP")
    l1 = layout.layer(1, 0)
    l2 = layout.layer(2, 0)
    l3 = layout.layer(3, 0)

    top.shapes(l1).insert(db.DBox(3.0, 3.0, 61.0, 43.0))
    top.shapes(l1).insert(db.DPolygon([
        db.DPoint(8.0, 48.0), db.DPoint(34.0, 48.0),
        db.DPoint(28.0, 57.0), db.DPoint(13.0, 55.0),
    ]))
    # Keep same-layer primitives disjoint so the solid reference mask retains
    # every primitive boundary. Cross-layer overlap still exercises paint
    # order and visibility.
    top.shapes(l2).insert(db.DBox(45.0, 13.0, 76.0, 55.0))
    top.shapes(l2).insert(db.DPath([
        db.DPoint(2.0, 8.0), db.DPoint(24.0, 26.0),
        db.DPoint(40.0, 7.0),
    ], 0.8, 0.4, 0.2, False))
    top.shapes(l3).insert(db.DPolygon([
        db.DPoint(31.0, 5.0), db.DPoint(77.0, 31.0),
        db.DPoint(42.0, 58.0), db.DPoint(37.0, 29.0),
    ]))
    top.shapes(l3).insert(db.DPath([
        db.DPoint(5.0, 35.0), db.DPoint(17.0, 35.0),
        db.DPoint(25.0, 44.0),
    ], 1.2))
    layout.write(str(source))


def _save_klayout_style(source, output, view, width, height, visible,
                        mono, fills, widths):
    import klayout.db as db

    from floe.render import Renderer

    layout = db.Layout(False)
    layout.read(str(source))
    top = layout.top_cell()
    renderer = Renderer(layout, top, {
        (1, 0): "#ef3340",
        (2, 0): "#35d04f",
        (3, 0): "#3578ff",
    })
    try:
        renderer.set_fill_patterns(fills)
        renderer.set_line_widths(widths)
        renderer.set_mono(mono)
        renderer.set_visible(visible)
        box = db.DBox(*view)
        renderer.lv.save_image_with_options(
            str(output), width, height, 1, 1, 0, box, False)
    finally:
        renderer.lv._destroy()


def _rgb(path):
    return np.asarray(Image.open(path).convert("RGB"))


def _occupied(rgb):
    return np.any(rgb != 0, axis=2)


def _write_diff(path, golden, candidate, bad):
    image = golden.copy()
    image[bad] = np.array((255, 0, 255), dtype=np.uint8)
    Image.fromarray(image, "RGB").save(path)


def _dilate(mask, radius):
    result = mask.copy()
    for dy in range(-radius, radius + 1):
        for dx in range(-radius, radius + 1):
            shifted = np.zeros_like(mask)
            ys = slice(max(dy, 0), mask.shape[0] + min(dy, 0))
            yd = slice(max(-dy, 0), mask.shape[0] + min(-dy, 0))
            xs = slice(max(dx, 0), mask.shape[1] + min(dx, 0))
            xd = slice(max(-dx, 0), mask.shape[1] + min(-dx, 0))
            shifted[yd, xd] = mask[ys, xs]
            result |= shifted
    return result


def _style_oracle(work, parent_oracle, indexer, renderer, jobs):
    source = work / "style.oas"
    cache = work / "style.oas.floe"
    golden_dir = work / "style-golden"
    candidate_dir = work / "style-rust"
    diff_dir = work / "style-diff"
    for directory in (golden_dir, candidate_dir, diff_dir):
        directory.mkdir()
    _build_style_fixture(source)
    _index(indexer, source, cache, jobs)

    pattern = _style_pattern()
    solid = "\n".join(("*" * 16,) * 16)
    fills = {(2, 0): pattern, (3, 0): solid}
    widths = {(2, 0): 4, (3, 0): 2}
    styles = (
        "1/0,#ef3340,speckle,1",
        "2/0,#35d04f,%s,4" % _pattern_wire(pattern),
        "3/0,#3578ff,solid,2",
    )
    style_widths = {(1, 0): 1, (2, 0): 4, (3, 0): 2}
    view = (0.0, 0.0, 80.0, 60.0)
    width, height = 800, 600

    # Check the actual styled silhouette and separately retain the width-1
    # geometry silhouette. A wider device outline can move an accepted
    # geometry-edge choice by its line radius, but nowhere else.
    geometry_masks = {}
    mask_checks = 0

    def mask_pair(name, case_view, layer, line_width):
        tag = "%d_%d" % layer
        stem = "mask-%s-%s-w%d" % (name, tag, line_width)
        golden_path = golden_dir / (stem + ".png")
        candidate_path = candidate_dir / (stem + ".png")
        _save_klayout_style(
            source, golden_path, case_view, width, height, [layer], False,
            {layer: solid}, {layer: line_width})
        _render_rust(
            renderer, cache, candidate_path, case_view, width, height, jobs,
            layers="%d/%d" % layer,
            styles=("%d/%d,#ffffff,solid,%d" % (
                layer[0], layer[1], line_width),))
        golden_mask = _occupied(_rgb(golden_path))
        candidate_mask = _occupied(_rgb(candidate_path))
        ok, reason = parent_oracle.compare(golden_mask, candidate_mask)
        print("STYLE mask %-14s %s" % (stem.removeprefix("mask-"), reason))
        if not ok:
            raise OracleFailure("style layer %s: %s" % (stem, reason))
        return golden_mask

    for layer in ((1, 0), (2, 0), (3, 0)):
        line_width = style_widths[layer]
        actual_mask = mask_pair("base", view, layer, line_width)
        mask_checks += 1
        if line_width == 1:
            geometry_mask = actual_mask
        else:
            geometry_mask = mask_pair("geometry", view, layer, 1)
            mask_checks += 1
        geometry_masks[(view, layer)] = geometry_mask

    cases = (
        ("all", view, None, False, None),
        ("half", (0.05, 0.05, 80.05, 60.05), None, False, None),
        ("visible", view, [(1, 0), (3, 0)], False, "1/0,3/0"),
        ("mono", view, None, True, None),
    )
    failures = []
    for name, case_view, visible, mono, layers in cases:
        case_view = tuple(case_view)
        visible_keys = visible or [(1, 0), (2, 0), (3, 0)]
        boundary = np.zeros((height, width), dtype=bool)
        for layer in visible_keys:
            mask_key = (case_view, layer)
            geometry_mask = geometry_masks.get(mask_key)
            if geometry_mask is None:
                line_width = style_widths[layer]
                actual_mask = mask_pair(name, case_view, layer, line_width)
                mask_checks += 1
                if line_width == 1:
                    geometry_mask = actual_mask
                else:
                    geometry_mask = mask_pair(
                        name + "-geometry", case_view, layer, 1)
                    mask_checks += 1
                geometry_masks[mask_key] = geometry_mask
            edge = parent_oracle._edge_band(geometry_mask)[0]
            boundary |= _dilate(edge, style_widths[layer] // 2)
        golden_path = golden_dir / (name + ".png")
        candidate_path = candidate_dir / (name + ".png")
        _save_klayout_style(
            source, golden_path, case_view, width, height, visible, mono,
            fills, widths)
        _render_rust(
            renderer, cache, candidate_path, case_view, width, height, jobs,
            layers=layers, styles=styles, mono=mono)
        golden = _rgb(golden_path)
        candidate = _rgb(candidate_path)
        occupied_diff = _occupied(golden) ^ _occupied(candidate)
        occupied_outside = occupied_diff & ~boundary
        bad = np.any(golden != candidate, axis=2) & ~boundary
        if bad.any():
            _write_diff(diff_dir / (name + ".png"), golden, candidate, bad)
            y, x = np.argwhere(bad)[0]
            reason = "RGB %d px outside style boundary (first y=%d x=%d)" % (
                int(bad.sum()), y, x)
            if occupied_outside.any():
                reason += "; occupied %d outside" % int(
                    occupied_outside.sum())
            failures.append("%s: %s" % (name, reason))
            print("STYLE %-18s FAIL %s" % (name, reason))
        else:
            total_diff = int(np.any(golden != candidate, axis=2).sum())
            print("STYLE %-18s ok (RGB diff %d, boundary-only)" %
                  (name, total_diff))
    if failures:
        raise OracleFailure(
            "style oracle failed (diffs in %s):\n%s" %
            (diff_dir, "\n".join(failures)))
    return len(cases) + mask_checks


def _parse_args():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--parent", type=Path,
        default=Path(os.environ.get("FLOE_PARENT", DEFAULT_PARENT)))
    parser.add_argument(
        "--indexer", type=Path,
        help="default: PARENT/rust/target/release/floe-index")
    parser.add_argument(
        "--renderer", type=Path,
        default=Path(os.environ.get(
            "FLOE_RENDER_CLI",
            ROOT / "rust/target/release/floe-render-cli")))
    parser.add_argument("--jobs", type=int, default=8)
    parser.add_argument("--workdir", type=Path)
    parser.add_argument("--keep", action="store_true")
    return parser.parse_args()


def main():
    args = _parse_args()
    parent = args.parent.resolve()
    indexer = (args.indexer or
               parent / "rust/target/release/floe-index").resolve()
    renderer = args.renderer.resolve()
    if args.jobs < 1 or args.jobs > 256:
        raise SystemExit("--jobs must be in 1..256")
    _require_executable(indexer, "floe-index")
    _require_executable(renderer, "floe-render-cli")
    sys.path.insert(0, str(parent))
    parent_oracle = _load_parent_goldens(parent)

    ephemeral = args.workdir is None
    work = (Path(tempfile.mkdtemp(prefix="floe-klayout-oracle-"))
            if ephemeral else args.workdir.resolve())
    if not ephemeral:
        if work.exists() and any(work.iterdir()):
            raise SystemExit("--workdir must be absent or empty: %s" % work)
        work.mkdir(parents=True, exist_ok=True)
    try:
        px_count = _geometry_oracle(
            work, parent_oracle, indexer, renderer, args.jobs)
        style_count = _style_oracle(
            work, parent_oracle, indexer, renderer, args.jobs)
        print("KLAYOUT ORACLE: ALL OK (%d PX + %d style checks)" %
              (px_count, style_count))
    except Exception:
        print("oracle artifacts retained: %s" % work, file=sys.stderr)
        raise
    else:
        if ephemeral and not args.keep:
            shutil.rmtree(work)
        else:
            print("oracle artifacts: %s" % work)


if __name__ == "__main__":
    try:
        main()
    except OracleFailure as exc:
        raise SystemExit("FAIL: %s" % exc)
