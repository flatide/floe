#!/usr/bin/env python3
"""Perf line parity gate (step P4b of moving the GTK render loop onto the
shared Rust ViewController, docs/SHARED_APP_LAYER.ko.md §7).

The viewer's status bar and terminal log show a perf line that field
engineers paste: `perf_status` (floe/gui.py's until P4f, now
tools/oracle/floe_oracle/perf_line.py - the product's is the Rust port, sent
by floe2 gtk-service with each frame) over the result dict that
floe_oracle/rust_render.py `_emit_frame` adds up over a generation's refinement
rounds. rust/app-core/src/view/perf.rs ports both - `FrameReport` (the
result, a JSON object) and `perf_status` / `fmt_count` / `occ_note` /
`load_note` - and this gate holds the port to the Python, exactly:

  * synthetic: result dicts that reach every key and branch perf_status and
    occ_note read - a hand-made full frame changed one key at a time through
    each key's values (missing, None, zero, edges of fmt_count's units and of
    round()'s half-to-even, huge, fractional), then random dicts (fixed seed)
    mixing ints, floats and bools, density/fit/deck/labels/evict/summary
    variants, FLOE_RUST_DENSITY_ONLY both ways; fmt_count's unit edges;
    Viewer._load_note on fixed clocks. Python's (full, brief) and Rust's must
    be the same bytes (a dict Python itself refuses - a TypeError - is left
    out and counted); every line of perf_status, occ_note and fmt_count must
    have run for a case compared (a line trace);
  * real renders: the GTK adapter (RustRenderWorker / DeckRenderWorker)
    renders a synthetic MAIN01-class chip (tools/gen_main01_like.py) and a
    jobdeck over a small layout with FLOE_RUST_RECORD set, so every emitted
    frame is recorded - the frame line's fields, the job and adapter values
    the result reads, the adapter's measured times and the result: detail
    low/medium/high and exact, depth full/0/2/5, labels and frames on/off,
    the frame cache's reuse and a pan's tile reuse, a margin (`bg`) around a
    viewport, the density on and off (pass 2 by the occupancy density and,
    with FLOE_RUST_DENSITY_OCC=off, by the plans), a 48 MB decode budget that
    fits the frame (fit fields), multi-round refinement
    (FLOE_RUST_ROUND_PAGES=16), render probes and a deck's composite frames;
    the gate checks the renders still reach each of those;
  * fuzzed frame lines: the adapter's own _submit_render / _emit_frame (no
    renderd) on made-up lines - every field _emit_frame reads (scanned from
    its source), odd integers (`1_000`, `+5`, ` 7`, `-`, `abc`), density
    lists of each length renderd ever sent and wrong ones, broken
    place_walks, probes, decks, refining and settled rounds - recorded the
    same way.
    Every record replays through the Rust FrameReport, generation by
    generation in record order: the Rust result must equal Python's in
    keys, value types and values (a float by its repr), and both perf lines
    must match.

The Rust side is an ignored integration test (rust/app-core/tests/
perf_parity.rs) run on the corpus written here; it asserts and writes its
outputs, which are compared here once more with Python's types.

    .venv/bin/python tools/validate_perf_parity.py [--keep DIR]
"""
import argparse
import copy
import json
import math
import os
from pathlib import Path
import random
import subprocess
import sys
import tempfile
import time
import types

ROOT = Path(__file__).resolve().parents[1]
FLOE2 = os.environ.get("FLOE2_BIN") or str(ROOT / "rust" / "target" / "release" / "floe2")
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT / "tools" / "oracle"))  # floe_oracle (P3)
# perf_status reads it (renderd's diagnostic); unset unless a case sets it
os.environ.pop("FLOE_RUST_DENSITY_ONLY", None)
import floe.gui as gui  # noqa: E402  (headless-safe: GTK loads lazily)
from floe_oracle import perf_line, rust_render  # noqa: E402

SEED = 20261010
# the fuzzed frame lines' generations (the renders' count from 1)
FUZZ_GEN = 100000
MISSING = object()

# ---- synthetic result dicts ---------------------------------------------

COUNT = [0, 1, 2, 7, 99, 100, 101, 999, 1000, 1500, 2500, 9999, 10000, 10499, 10500,
         12500, 13500, 999499, 999500, 999999, 1000000, 1049999, 1050000, 1250000,
         1350000, 999949999, 999950000, 999999999, 10 ** 9, 1250000000, 2 ** 53 + 1,
         10 ** 15 + 500, 10 ** 18, 2 ** 62]
COUNT_ODD = [9999.5, 12500.0, 0.0, 1.5, 2.5, 10499.5, 999999.5, True, False]
MS = [0.0, 0.04, 0.05, 0.06, 0.25, 0.35, 0.45, 0.5, 0.55, 1.5, 2.5, 99.5, 99.96, 100.0,
      100.5, 199.5, 200.0, 200.5, 201.0, 999.5, 1500.25, 12345.678, 1e-05, 1e-07,
      123456.789, 1e16, 1.7e17, 0.125]
MS_ODD = [0, 3, 100, 200, 201, 250, True]
US = [0, 1, 499, 500, 501, 1500, 2500, 3500, 999500, 1000500, 2 ** 53 + 1001,
      10 ** 18 + 500, 2500.0, 1500.5, 4999.999]
PCT = [0, 1, 50, 99, 100, 101, 150, 250, 333, 12345, 99.9, 150.7]
FLAG = [0, 1, 2, True, False]
CUT_UM = [0, 0.0, 0.001, 0.0005, 0.00012345, 0.1235, 0.25, 1.0, 1.5, 1234.5, 1e-05, 99950.0,
          3, 0.0625]
EDGE = ["14/367", "1/0", "7/59", ""]
NONE_WHY = ["-", "policy", "exact", "off", "nofile", "layers", "near", "work"]


def pick(rng, values, odd=(), p_odd=0.15):
    if odd and rng.random() < p_odd:
        return rng.choice(odd)
    return rng.choice(values)


def rand_count(rng):
    r = rng.random()
    if r < 0.45:
        return pick(rng, COUNT, COUNT_ODD)
    if r < 0.75:
        return rng.randrange(0, 10 ** rng.randrange(1, 13))
    return 0


def rand_ms(rng):
    r = rng.random()
    if r < 0.4:
        return pick(rng, MS, MS_ODD)
    if r < 0.8:
        return round(rng.uniform(0, 10 ** rng.randrange(0, 7)), rng.randrange(0, 5))
    return rng.randrange(0, 50) + rng.choice((0.5, 0.05, 0.25, 0.75, 0.0))


def rand_us(rng):
    if rng.random() < 0.4:
        return rng.choice(US)
    return rng.randrange(0, 10 ** rng.randrange(1, 11)) + rng.choice((0, 500, 499, 501))


# the kinds of the result keys perf_status reads (the nested dicts below)
TOP = {
    "tiles": "count!", "ms": "msint!", "new": "count", "load_ms": "countnone",
    "phase_apply": "countnone", "phase_plan": "count", "phase_delta": "count",
    "draw_ms": "count!", "text_plan_ms": "ms", "fit_probe_ms": "ms", "other_ms": "msnum",
    "wait_ms": "msnum", "cut_um": "cut", "drawn": "countnone", "refining": "count",
    "plan_ms": "msnone", "frame_rects": "count", "fit_probe_walk": "flag",
    "text_place_records": "count", "png_ms": "msnone", "frame_format": "format",
    "publish_ms": "ms", "raster_jobs": "count", "render_tiles": "count",
    "tile_px": "countany", "frame_width": "countany", "frame_height": "countany",
    "rounds": "count", "decode_sum_ms": "ms", "decode_max_ms": "ms", "index_ms": "ms",
    "raster_tile_max_ms": "ms", "tiles_reused": "count", "work_bin_items": "count",
    "work_bin_defer_rep": "count", "work_bin_defer_single": "count",
    "work_bin_defer_wmax": "count", "work_bin_overflow_items": "count",
    "member_paints": "count", "cache_evicted": "count", "retained_mb": "ms",
    "hier_cells_visited": "count", "subtrees_pruned": "count", "once_full_tiles": "count",
    "once_passes_skipped": "count", "once_items_skipped": "count",
    "labels_truncated": "flag", "over_budget_pages": "count", "workers": "count",
    "density_block": "msnone", "density_floor": "msnone",
}
CULLS = {k: "count" for k in (
    "pages_size", "page_bvh", "child_bvh", "children_size", "layer", "washed",
    "lod_swapped", "thin_frames", "thin_pages", "sub_cut_washes", "sub_cut_sparse",
    "sub_cut_sparse_over", "sub_cut_wash_over", "rep_kept", "rep_washed", "rep_children",
    "rep_page_level", "rep_level", "fit_fixed", "fit_refits", "sub_cut_boxes",
    "sub_cut_box_over", "sub_cut_box_unsure", "stored_rep_points", "stored_rep_tested",
    "stored_rep_nodes", "stored_rep_proxies", "stored_rep_bytes", "stored_rep_pixels",
    "stored_rep_spans", "stored_rep_painted_pixels")}
CULLS.update({
    "fit_pct": "pct", "fit_full_pct": "pct", "fit_none_pct": "pct", "fit_thin": "thin",
    "fit_cull": "flag", "fit_over": "flag", "fit_redecided": "flag", "fit_ranked": "flag",
    "fit_layers_whole": "small", "fit_layer_edge": "edge", "fit_layers_out": "small",
    "fit_scale": "scale", "sub_cut_box_level": "level", "shape_cut": "flag",
    "shape_cut_max": "flag", "stored_rep_limited": "flag",
})
SUMMARY = {"layers": "small", "cells": "count", "level": "small", "cell_um": "ms",
           "none": "why", "pixels": "count", "pages_skipped": "count"}
DECK = {k: "count" for k in (
    "passes", "frame_passes", "passes_skipped", "scene_reuses", "unique_pages",
    "pages_summed", "pass_workers", "batches", "streamed_passes", "slices",
    "wide_washes", "summary_passes", "summary_cells", "summary_none_passes",
    "over_budget_pages")}
DECK.update({k: "us" for k in (
    "scene_us", "frame_raster_us", "composite_us", "raster_wall_us", "raster_us",
    "pass_bytes_max", "batch_bytes_max")})
DECK_REQUIRED = ("passes", "frame_passes", "passes_skipped", "unique_pages",
                 "pages_summed", "scene_us", "frame_raster_us", "composite_us",
                 "pass_bytes_max")
PLAN2 = {k: "count" for k in rust_render.DENSITY_PLAN2}
PLAN2.update({"probe_us": "us", "fit_us": "us", "threads": "threads", "passes": "small",
              "dot_gain_milli": "gain", "dot_gate_min": "gate", "bright_milli": "bright",
              "pattern": "flag", "cell_cover": "flag", "occ_layers": "small",
              "occ_cell_nm": "cellnm", "occ_made": "small", "occ_cache_kb": "count"})
PLAN2_REQUIRED = ("probe_us", "probes", "fit_us", "passes", "regions", "nodes",
                  "page_nodes", "page_candidates")
DUS = {k: "us" for k in rust_render.DENSITY_TIMES}
DPAGES = {k: "count" for k in rust_render.DENSITY_PAGE_COUNTS}
STACK = {k: "count" for k in rust_render.DENSITY_STACK_COUNTS}
DOTS = {k: "count" for k in rust_render.DENSITY_DOTS}

KIND_VALUES = {
    "count": COUNT + COUNT_ODD, "count!": COUNT + [9999.5, 2.5],
    "msint!": [0, 1, 52, 250, 1499, 99999, 10 ** 12, 2.5, 52.7],
    "countnone": [None] + COUNT[:12] + [2.5, 99.5],
    "countany": [0, 384, 1920, 384.0, 1919.5, 1e16, True],
    "ms": MS + MS_ODD, "msnum": [0, 0.0, 199.9, 200, 200.0, 200.5, 201, 2000.7, 1e16],
    "msnone": [None] + MS + MS_ODD, "cut": CUT_UM, "flag": FLAG,
    "format": ["raw", "png", None, "PNG"], "pct": PCT,
    "thin": [0, 1, 2, 5, 29, 30, 31, 40, 255, 2.7, True], "small": [0, 1, 2, 3, 12, 400, 2.5],
    "edge": [None] + EDGE, "scale": [0, 999, 1000, 1001, 1500, 2345, 12345, 1000.5],
    "level": [0, 1, 2, 3, 20, 62, 63, 64, 126, 127, 130, True], "why": [None] + NONE_WHY + [0],
    "us": US, "threads": [0, 1, 2, 8, 1.5, 3.7, True], "gain": [None, 0, 1, 500, 999, 999.5,
                                                               1000, 1500, -5, 0.5],
    "gate": [0, 1, 2, 3, 2.5, 9, True], "bright": [0, 1, 333, 1500, 2000, 999.5],
    "cellnm": [0, 1, 250, 1000, 2500, 12345, 999.5, 123456789],
}


def rand_value(rng, kind):
    if kind in ("count", "count!"):
        return rand_count(rng)
    if kind in ("ms", "msnone") and rng.random() < 0.5:
        return rand_ms(rng)
    if kind == "us" and rng.random() < 0.5:
        return rand_us(rng)
    return rng.choice(KIND_VALUES[kind])


def rand_dict(rng, schema, p_present, required=()):
    out = {}
    for key, kind in schema.items():
        if key in required or rng.random() < p_present:
            out[key] = rand_value(rng, kind)
    return out


def rand_result(rng):
    """A random result dict: each key present or not, values of its kind or
    an odd type now and then; the nested dicts sparse like real frames."""
    res = {}
    density = rng.random() < 0.5
    for key, kind in TOP.items():
        if kind.endswith("!") or rng.random() < 0.55:
            res[key] = rand_value(rng, kind)
    if rng.random() < 0.9:
        res["plan_culls"] = rand_dict(rng, CULLS, rng.choice((0.0, 0.05, 0.2, 0.6)))
        if rng.random() < 0.15:
            res["plan_culls"] = {k: 0 for k in CULLS} | {"fit_layer_edge": None}
    if rng.random() < 0.7:
        res["summary"] = rand_dict(rng, SUMMARY, 0.7, required=("layers", "cells", "level",
                                                                "cell_um"))
    if rng.random() < 0.25:
        res["deck"] = rand_dict(rng, DECK, 0.6, required=DECK_REQUIRED)
    if density:
        res["density_stack"] = rand_dict(rng, STACK, 0.8)
        if rng.random() < 0.7:
            res["density_dots"] = rand_dict(rng, DOTS, 0.9)
        if rng.random() < 0.8:
            res["density_plan2"] = rand_dict(rng, PLAN2, rng.choice((0.1, 0.4, 0.9)),
                                             required=PLAN2_REQUIRED)
        if rng.random() < 0.8:
            res["density_us"] = rand_dict(rng, DUS, 0.7)
        if rng.random() < 0.7:
            res["density_pages"] = rand_dict(rng, DPAGES, 0.8)
    else:
        for key in ("density_stack", "density_dots", "density_plan2", "density_us",
                    "density_pages"):
            if rng.random() < 0.5:
                res[key] = None
    return res


def base_result():
    """A full frame, every key there, the density on: the systematic cases
    change it one key at a time."""
    res = {
        "kind": "frame", "frame_format": "raw", "gen": 7, "tiles": 1234, "new": 56,
        "scope": "live", "bg": False, "load_ms": 345, "phase_plan": 12, "phase_delta": 33,
        "phase_apply": 300, "draw_ms": 210, "read_ms": 33.2, "decode_ms": 250.5,
        "scene_ms": 49.5, "raster_ms": 210.25, "png_ms": 0.5, "publish_ms": 1.25,
        "cache_hit": 10, "cache_miss": 56, "cache_evicted": 12500, "frame_cache_hit": 0,
        "retained_mb": 12.5, "resident_mb": 100.25, "decode_workers": 8, "workers": 8,
        "raster_jobs": 4, "render_tiles": 45, "tile_px": 384, "frame_width": 1920,
        "frame_height": 1080, "wait_ms": 250, "queue_ms": 0.0, "wall_ms": 900.5,
        "other_ms": 201, "ms": 1203, "plan_ms": 12.25, "fit_probe_ms": 150.5,
        "fit_probe_walk": True, "wc_cells": 0, "inst_edges": 0, "frame_rects": 1250000,
        "text_plan_ms": 120.5, "text_place_records": 99999, "labels": 12,
        "rounds": 3, "decode_sum_ms": 1999.5, "decode_max_ms": 300.5, "index_ms": 2.5,
        "raster_tile_max_ms": 30.5, "tiles_reused": 9, "work_bin_items": 1500000,
        "work_bin_overflow_items": 0, "work_bin_defer_rep": 12, "work_bin_defer_single": 10500,
        "work_bin_defer_wmax": 999999, "member_paints": 2500000, "hier_cells_visited": 13500,
        "subtrees_pruned": 9999, "once_full_tiles": 3, "once_passes_skipped": 4,
        "once_items_skipped": 12345, "drawn": 10500, "refining": 2, "cut_um": 0.1235,
        "labels_truncated": True, "over_budget_pages": 3,
        "plan_culls": {k: 0 for k in CULLS},
        "summary": {"layers": 3, "cells": 12500, "pixels": 99, "level": 2, "cell_um": 0.5,
                    "none": "-", "pages_skipped": 7},
        "deck": {k: 0 for k in DECK},
        "density_stack": {"lit": 123456, "top": 1, "lower": 2, "covered": 3, "claimed": 4},
        "density_dots": {"items": 10, "over": 0},
        "density_block": 2.0, "density_floor": 0.25,
        "density_plan2": {k: 0 for k in rust_render.DENSITY_PLAN2},
        "density_us": {k: 1500 for k in rust_render.DENSITY_TIMES},
        "density_pages": {"planned": 9, "in_hand": 3, "decoded": 5, "over_budget": 2},
    }
    res["plan_culls"].update({
        "pages_size": 3, "fit_pct": 150, "fit_thin": 3, "fit_full_pct": 250,
        "fit_none_pct": 1250, "fit_ranked": 1, "fit_layers_whole": 2,
        "fit_layer_edge": "14/367", "fit_layers_out": 1, "fit_scale": 1500, "shape_cut": 1,
        "shape_cut_max": 1, "sub_cut_boxes": 12, "sub_cut_box_level": 2,
        "sub_cut_box_over": 5, "sub_cut_box_unsure": 1, "rep_kept": 4, "rep_children": 2,
        "rep_level": 3, "rep_page_level": 1, "stored_rep_points": 10500,
        "stored_rep_tested": 999, "stored_rep_limited": 1, "thin_pages": 2,
        "sub_cut_washes": 1, "sub_cut_sparse_over": 1})
    res["deck"].update({"passes": 12, "frame_passes": 2, "passes_skipped": 1,
                        "unique_pages": 30, "pages_summed": 45, "scene_us": 2500,
                        "frame_raster_us": 3500, "composite_us": 1500, "raster_wall_us": 4500,
                        "pass_workers": 4, "batches": 2, "pass_bytes_max": 2500000,
                        "batch_bytes_max": 3500000, "streamed_passes": 1, "slices": 4,
                        "wide_washes": 12500, "summary_passes": 2, "summary_cells": 99,
                        "summary_none_passes": 1})
    res["density_plan2"].update({
        "probe_us": 2500, "fit_us": 3500, "probes": 2, "passes": 2, "regions": 4,
        "nodes": 12500, "page_nodes": 99, "page_candidates": 1000, "threads": 4,
        "reads": 1500000, "items": 2500, "by_nodes": 1, "by_list_chunks": 3,
        "by_chunk_members": 12500, "by_pages": 4, "occ_pages": 2, "full_chunks": 1,
        "sampled_chunks": 2, "sampled_members": 9, "map_updates": 3, "stages": 2,
        "mask_tests": 10, "mask_pruned": 5, "reserve_mb": 64, "bright_milli": 1500,
        "dot_gain_milli": 500, "dot_gate_min": 3, "dot_gated": 12, "free_top": 10500,
        "free_others": 3, "probes_over": 1, "thinned": 1, "stood_in": 2})
    return res


# the systematic variations: (path, values); MISSING removes the key
def variations():
    out = []
    for key, kind in TOP.items():
        out.append(((key,), [MISSING] + KIND_VALUES[kind]))
    for key, kind in CULLS.items():
        out.append((("plan_culls", key), [MISSING] + KIND_VALUES[kind]))
    for key, kind in SUMMARY.items():
        out.append((("summary", key), [MISSING] + KIND_VALUES[kind]))
    for key, kind in DECK.items():
        out.append((("deck", key), [MISSING] + KIND_VALUES[kind]))
    for key, kind in PLAN2.items():
        out.append((("density_plan2", key), [MISSING] + KIND_VALUES[kind]))
    for key in rust_render.DENSITY_TIMES:
        out.append((("density_us", key), [MISSING, None] + US))
    for key in rust_render.DENSITY_PAGE_COUNTS:
        out.append((("density_pages", key), [MISSING] + KIND_VALUES["count"]))
    out.append((("density_stack", "lit"), [MISSING] + KIND_VALUES["count"]))
    for key in ("plan_culls", "summary", "deck", "density_stack", "density_dots",
                "density_plan2", "density_us", "density_pages"):
        out.append(((key,), [MISSING, None, {}]))
    return out


def with_value(res, path, value):
    res = copy.deepcopy(res)
    node = res
    for key in path[:-1]:
        if not isinstance(node.get(key), dict):
            node[key] = {}
        node = node[key]
    if value is MISSING:
        node.pop(path[-1], None)
    else:
        node[path[-1]] = value
    return res


class LineTrace:
    """The lines of `functions` (their nested generator code too) that the
    cases Python answered ran: a case Python refused (an exception) counts
    nothing, it is not compared."""

    def __init__(self, *functions):
        self.codes = set()
        for function in functions:
            stack = [function.__code__]
            while stack:
                code = stack.pop()
                self.codes.add(code)
                stack.extend(c for c in code.co_consts if isinstance(c, types.CodeType))
        self.hit = set()
        self._case = set()
        self._saved = None

    def _local(self, frame, event, arg):
        if event == "line":
            self._case.add(frame.f_lineno)
        return self._local

    def _global(self, frame, event, arg):
        return self._local if frame.f_code in self.codes else None

    def __enter__(self):
        self._case = set()
        self._saved = sys.gettrace()
        sys.settrace(self._global)
        return self

    def __exit__(self, kind, value, tb):
        sys.settrace(self._saved)
        if kind is None:
            self.hit |= self._case
        return False

    def lines(self):
        return {line for code in self.codes for _, _, line in code.co_lines()
                if line is not None and line != code.co_firstlineno}

    def missing(self):
        return sorted(self.lines() - self.hit)

    def covered(self):
        return len(self.lines() & self.hit), len(self.lines())


def roundtrip(value):
    """What the Rust side reads: the JSON of it."""
    return json.loads(json.dumps(value, allow_nan=False))


def python_status(res, depth_note, density_only):
    if density_only:
        os.environ["FLOE_RUST_DENSITY_ONLY"] = "on"
    try:
        return perf_line.perf_status(res, depth_note)
    finally:
        os.environ.pop("FLOE_RUST_DENSITY_ONLY", None)


def synthetic_cases():
    rng = random.Random(SEED)
    results = []
    base = base_result()
    results.append(base)
    # the base with its density by the occupancy density, and one without
    occ = with_value(base, ("density_plan2", "occ_layers"), 37)
    occ["density_plan2"].update({"occ_cell_nm": 2500, "occ_made": 3, "occ_cache_kb": 12345})
    results.append(occ)
    plain = copy.deepcopy(base)
    for key in ("density_stack", "density_dots", "density_plan2", "density_us",
                "density_pages", "deck"):
        plain[key] = None
    results.append(plain)
    for path, values in variations():
        for start in (base, occ, plain) if path[0].startswith("density") else (base, plain):
            for value in values:
                results.append(with_value(start, path, value))
    for _ in range(4000):
        results.append(rand_result(rng))
    status, refused = [], 0
    occ_cases = []
    trace = LineTrace(perf_line.perf_status, perf_line.occ_note, gui.fmt_count)
    for i, res in enumerate(results):
        try:
            res = roundtrip(res)
        except ValueError:
            refused += 1
            continue
        depth_note = rng.choice(("", ", depth 3", ", depth 0", ", depth 12"))
        density_only = rng.random() < 0.15
        try:
            with trace:
                full, brief = python_status(res, depth_note, density_only)
        except (TypeError, KeyError, ValueError, AttributeError, OverflowError):
            refused += 1
            continue
        status.append({"res": res, "depth_note": depth_note, "density_only": density_only,
                       "full": full, "brief": brief})
        for full_note in (False, True):
            try:
                with trace:
                    note = perf_line.occ_note(res, full=full_note)
            except (TypeError, KeyError, ValueError, AttributeError):
                continue
            if note or i % 7 == 0:
                occ_cases.append({"res": res, "full": full_note, "out": note})
    counts = []
    values = (COUNT + COUNT_ODD + [n + d for n in (9999, 10499, 999499, 999949999, 10 ** 9)
                                  for d in (0.25, 0.5, 0.75, 1)]
              + [-1, -10000, -2.5, -1e9, 1e12, 1.25e9, 1.35e6, 10500.0, 11500.0, 0.0, -0.0])
    values += [rand_count(rng) for _ in range(500)] + [rand_ms(rng) for _ in range(500)]
    for n in values:
        with trace:
            counts.append({"n": n, "out": gui.fmt_count(n)})
    missing = trace.missing()
    assert not missing, "the synthetic cases compared never reach floe/gui.py lines %s" % (
        ", ".join(map(str, missing)))
    return status, refused, occ_cases, counts, len(results), trace.covered()


def load_cases():
    """Viewer._load_note on fixed clocks (its only impurities: the marks it
    is handed and time.monotonic)."""
    cases = []
    real_time = gui.time
    try:
        for marks in (None, {"t0": 100.0}, {"t0": 100.0, "cache": 101.25},
                      {"t0": 100.0, "cache": 101.25, "service": 103.05},
                      {"t0": 100.0, "cache": 101.25, "service": 103.05, "open": {}},
                      {"t0": 100.0, "cache": 101.25, "service": 103.05, "open": None},
                      {"t0": 100.0, "cache": 101.25, "service": 103.05,
                       "open": {"renderd_open_ms": None}},
                      {"t0": 100.0, "cache": 101.25, "service": 103.05,
                       "open": {"renderd_open_ms": 1450.0}},
                      {"t0": 0.125, "cache": 0.375, "service": 10.625,
                       "open": {"renderd_open_ms": 9250.5}}):
            for res in ({}, {"refining": 1}, {"refining": 0}):
                for now in (103.05, 104.5, 110.125, 1e6 + 0.05):
                    stub = types.SimpleNamespace(_load_marks=copy.deepcopy(marks))
                    gui.time = types.SimpleNamespace(monotonic=lambda now=now: now)
                    full, brief = gui.Viewer._load_note(stub, res)
                    ready = marks is not None and "service" in marks
                    rust_marks = None if not ready else {
                        "t0": marks["t0"], "cache": marks["cache"], "service": marks["service"],
                        "renderd_open_ms": (marks.get("open") or {}).get("renderd_open_ms")}
                    cases.append({"marks": rust_marks, "res": res, "now": now,
                                  "full": full, "brief": brief})
    finally:
        gui.time = real_time
    return cases


# ---- real renders --------------------------------------------------------

def run(argv, timeout=900):
    done = subprocess.run([str(a) for a in argv], cwd=ROOT, env=os.environ,
                          capture_output=True, text=True, timeout=timeout)
    assert done.returncode == 0, "%s: %s%s" % (argv[:3], done.stdout[-2000:],
                                               done.stderr[-2000:])


def make_worker(src, env=None, deck=False):
    saved = {name: os.environ.get(name) for name in (env or {})}
    os.environ.update(env or {})
    try:
        if deck:
            from floe_oracle.jobdeck.viewer import DeckCache
            cache = DeckCache(str(src), mode="level")
            cache.load()
            worker = rust_render.DeckRenderWorker(cache)
        else:
            from floe_oracle.cache import Cache
            cache = Cache(str(src))
            cache.load()
            worker = rust_render.RustRenderWorker(cache)
        worker.start()
        return worker
    finally:
        for name, value in saved.items():
            if value is None:
                os.environ.pop(name, None)
            else:
                os.environ[name] = value


class Session:
    """The renders of one worker; every result the queue gave is kept (the
    pixels dropped) to check the records against."""

    gen = 0
    emitted = []

    def __init__(self, worker):
        self.worker = worker
        self.meta = worker.cache.meta

    def job(self, bbox, w, h, **kw):
        Session.gen += 1
        job = {"kind": "render", "gen": Session.gen, "scope": "live",
               "t_sub": time.time(), "bbox": tuple(float(v) for v in bbox),
               "view": tuple(float(v) for v in bbox), "w": int(w), "h": int(h),
               "depth": None, "root": None, "cut_px": 3.0, "lod": False, "thin": "keep",
               "frames": True, "labels": True, "label_font_px": 14, "frame_cache": True,
               "abstract": False, "visible": None}
        job.update(kw)
        return job

    def render(self, job, timeout=600):
        """Submit `job`; its rounds up to the settled one (a dropped margin:
        none)."""
        self.worker.submit(job)
        deadline = time.monotonic() + timeout
        rounds = []
        while True:
            res = self.worker.res.get(timeout=max(0.1, deadline - time.monotonic()))
            kind = res.get("kind")
            if kind == "error":
                raise AssertionError("render error: %s" % res.get("msg"))
            if res.get("gen") != job["gen"]:
                continue
            if kind in ("dropped", "cancelled"):
                return rounds
            if kind in ("frame", "probe_frame"):
                res = {k: v for k, v in res.items() if k not in ("rgba", "png")}
                rounds.append(res)
                Session.emitted.append(res)
                if kind == "probe_frame" or not res.get("refining"):
                    return rounds


def view_box(meta, fx0, fy0, fx1, fy1):
    x0, y0, x1, y1 = (float(v) for v in meta["bbox"])
    return (x0 + (x1 - x0) * fx0, y0 + (y1 - y0) * fy0,
            x0 + (x1 - x0) * fx1, y0 + (y1 - y0) * fy1)


def pan(box, w, dx_px, dy_px):
    spp = (box[2] - box[0]) / w
    return (box[0] + dx_px * spp, box[1] + dy_px * spp,
            box[2] + dx_px * spp, box[3] + dy_px * spp)


def margin_job(session, viewport, w, h, **kw):
    """The viewer's margin: the viewport and as much around it, at the
    viewport's scale (`bg`, its `view` the viewport)."""
    sx = (viewport[2] - viewport[0]) / w
    sy = (viewport[3] - viewport[1]) / h
    ex, ey = w // 2, h // 2
    eb = (viewport[0] - ex * sx, viewport[1] - ey * sy,
          viewport[2] + ex * sx, viewport[3] + ey * sy)
    job = session.job(eb, w + 2 * ex, h + 2 * ey, bg=True, **kw)
    job["view"] = tuple(float(v) for v in viewport)
    return job


def chip_scenarios(src):
    """The renders, by worker: (name, rounds)."""
    done = []
    W, H = 640, 480
    plain = Session(make_worker(src))
    try:
        meta = plain.meta
        whole = view_box(meta, 0, 0, 1, 1)
        mid = view_box(meta, 0.40, 0.40, 0.46, 0.48)
        near = view_box(meta, 0.42, 0.42, 0.4225, 0.423)
        keys = [(int(l["layer"]), int(l["datatype"])) for l in meta["layers"]]
        for cut in (5.0, 3.0, 1.0, 0.0):
            done.append(("detail cut %g" % cut, plain.render(plain.job(mid, W, H, cut_px=cut))))
        for depth in (0, 2, 5):
            done.append(("depth %d" % depth, plain.render(plain.job(mid, W, H, depth=depth))))
        done.append(("labels off, frames off", plain.render(
            plain.job(near, W, H, labels=False, frames=False))))
        done.append(("labels on near", plain.render(plain.job(near, W, H, cut_px=1.0))))
        done.append(("whole chip", plain.render(plain.job(whole, W, H))))
        done.append(("frame cache again", plain.render(plain.job(whole, W, H))))
        done.append(("pan 16 px", plain.render(plain.job(pan(whole, W, 16, 0), W, H))))
        done.append(("pan 32 px", plain.render(plain.job(pan(whole, W, 32, 16), W, H))))
        done.append(("margin", plain.render(margin_job(plain, mid, W, H))))
        done.append(("viewport in its margin", plain.render(plain.job(mid, W, H))))
        done.append(("density on", plain.render(plain.job(whole, W, H, cut_px=3.0,
                                                          density=True))))
        done.append(("density on mid", plain.render(plain.job(mid, W, H, cut_px=5.0,
                                                              density=True))))
        done.append(("density off", plain.render(plain.job(whole, W, H, density=False))))
        done.append(("a few layers", plain.render(plain.job(mid, W, H, visible=keys[:7]))))
        done.append(("no layers", plain.render(plain.job(mid, W, H, visible=[]))))
        done.append(("thin cull", plain.render(plain.job(whole, W, H, thin="cull",
                                                         frame_cache=False))))
        done.append(("png frame", plain.render(plain.job(near, 200, 150,
                                                         frame_format="png"))))
        for mode in ("baseline", "ordered"):
            probe = plain.job(mid, W, H, density=False)
            probe.update(kind="render_probe", mode=mode)
            done.append(("probe %s" % mode, plain.render(probe)))
    finally:
        plain.worker.stop()
    # the plans' pass 2 (the occupancy density's kill switch)
    plans = Session(make_worker(src, {"FLOE_RUST_DENSITY_OCC": "off"}))
    try:
        whole = view_box(plans.meta, 0, 0, 1, 1)
        mid = view_box(plans.meta, 0.40, 0.40, 0.46, 0.48)
        done.append(("density by the plans", plans.render(plans.job(whole, W, H, density=True))))
        done.append(("density by the plans mid", plans.render(
            plans.job(mid, W, H, cut_px=5.0, density=True))))
    finally:
        plans.worker.stop()
    # streamed rounds: sixteen pages a round
    rounds = Session(make_worker(src, {"FLOE_RUST_ROUND_PAGES": "16"}))
    try:
        mid = view_box(rounds.meta, 0.30, 0.30, 0.60, 0.62)
        done.append(("rounds mid density", rounds.render(
            rounds.job(mid, W, H, cut_px=3.0, density=True))))
        done.append(("rounds mid", rounds.render(rounds.job(mid, W, H, cut_px=1.0,
                                                            density=False))))
        done.append(("rounds again", rounds.render(rounds.job(mid, W, H, cut_px=1.0,
                                                              density=False))))
    finally:
        rounds.worker.stop()
    # a decode budget the wide view does not fit (validate_fit_budget's)
    tight = Session(make_worker(src, {"FLOE_RUST_BUDGET_MB": "48"}))
    try:
        whole = view_box(tight.meta, 0, 0, 1, 1)
        mid = view_box(tight.meta, 0.25, 0.25, 0.75, 0.75)
        done.append(("budget fit", tight.render(tight.job(whole, 1920, 1920, cut_px=1.0,
                                                          density=False, labels=False,
                                                          frames=False))))
        done.append(("budget fit again", tight.render(tight.job(whole, 1920, 1920, cut_px=1.0,
                                                                density=False, labels=False,
                                                                frames=False))))
        done.append(("budget fit density", tight.render(tight.job(mid, 1920, 1920, cut_px=1.0,
                                                                  density=True))))
        done.append(("budget margin", tight.render(margin_job(tight, mid, 960, 960,
                                                              cut_px=1.0))))
    finally:
        tight.worker.stop()
    return done


DECK_SPEC = """SLICE 1,17
RETICLE
* perf.jb
OPTION PA, AA=0.0200, BA=0.002000, SA=80
MTITLE 1,PERF
*PLACE-INFO
*
CHIP ID001, * MAIN 1.0000
*
$ (1, PERF, AD=0.00020, SF=1, TC=parts.oas, LY={1,2}, DT={0}, BX=0.0, BY=0.0, UX=400.0, UY=400.0 )
ROWS 100.0/100.0
*END-PLACE
END
"""


def deck_layout(path):
    """400 x 400 um: 1/0 boxes in a cell placed 16 times, 2/0 thin lines."""
    import klayout.db as kdb
    ly = kdb.Layout()
    ly.dbu = 0.001
    top = ly.create_cell("PARTS")
    cell = ly.create_cell("BLOCK")
    l1, l2 = ly.layer(1, 0), ly.layer(2, 0)
    for j in range(20):
        for i in range(20):
            x, y = i * 4.0 + 0.3 * ((i + j) % 3), j * 4.0 + 0.2 * ((i * 3 + j) % 4)
            cell.shapes(l1).insert(kdb.DBox(x, y, x + 1.5 + 0.1 * (i % 5), y + 1.5))
    for j in range(4):
        for i in range(4):
            top.insert(kdb.DCellInstArray(cell.cell_index(),
                                          kdb.DTrans(kdb.DVector(i * 100.0, j * 100.0))))
    for k in range(200):
        top.shapes(l2).insert(kdb.DBox(k * 2.0, 0, k * 2.0 + 0.1, 400.0))
    top.shapes(l1).insert(kdb.DText("PERF", kdb.DTrans(kdb.DVector(10.0, 10.0))))
    ly.write(str(path))


def deck_scenarios(deck_path):
    done = []
    deck = Session(make_worker(deck_path, deck=True))
    try:
        whole = view_box(deck.meta, 0, 0, 1, 1)
        part = view_box(deck.meta, 0.1, 0.1, 0.35, 0.3)
        done.append(("deck whole", deck.render(deck.job(whole, 640, 640, cut_px=1.0))))
        done.append(("deck part", deck.render(deck.job(part, 640, 480, cut_px=3.0,
                                                       labels=False))))
        done.append(("deck density", deck.render(deck.job(whole, 640, 640, cut_px=5.0,
                                                          density=True))))
        done.append(("deck depth 0", deck.render(deck.job(whole, 640, 640, depth=0))))
    finally:
        deck.worker.stop()
    return done


# ---- the frame line fuzzed --------------------------------------------

def wire_names():
    """Every frame-line field _emit_frame reads, from its source: a field the
    adapter learns is fuzzed (and must be ported) without this list."""
    import inspect
    import re
    source = inspect.getsource(rust_render.RustRenderWorker._emit_frame)
    names = set(re.findall(r'fields,\s*"([a-z0-9_]+)"', source))
    names |= set(re.findall(r'fields\.get\(\s*"([a-z0-9_]+)"', source))
    names -= {"gen", "png", "out", "format", "final"}
    assert len(names) > 150, "the field scan found only %d names" % len(names)
    return sorted(names)


def wire_int(rng):
    r = rng.random()
    if r < 0.55:
        return str(rng.choice((0, 0, 1, 2, 3, 499, 500, 501, 1500, 2500, 3500, 999999,
                               1000500, 2 ** 53 + 1, 10 ** 15 + 500, 2 ** 62)))
    if r < 0.85:
        return str(rng.randrange(0, 10 ** rng.randrange(1, 12)))
    # what Python's int() makes of odd text (or refuses: the default)
    return rng.choice(("", "-", "abc", "1_000", "+5", "-3", " 7", "7 ", "1.5", "0x10",
                       "1__0", "_1", "1_", "007", "-0", "+", "9" * 18))


def wire_counts(rng, n):
    if rng.random() < 0.15:
        return rng.choice(("-", "", "1/2", "a/b", "/".join(["1"] * (n + 1)),
                           "/".join(["1"] * max(1, n - 1)) + "/x"))
    return "/".join(wire_int(rng) if rng.random() < 0.05 else
                    str(rng.choice((0, 0, 1, 7, 500, 1500, 2500, 12345, 10 ** 9)))
                    for _ in range(n))


def wire_float(rng):
    return rng.choice(("-", "", "0", "0.25", "1", "2.0", "0.5", "1e-3", "1_0.5", "3.75",
                       "x", "4", "0.125", "+2", "1E2", ".5", "5."))


def wire_fields(rng, names, gen, final, probe):
    fields = {}
    for name in names:
        if rng.random() < 0.6:
            fields[name] = wire_int(rng)
    for name, n in (("density_stack", 5), ("density_pages", 4), ("density_us", 6),
                    ("density_bin", 3), ("density_dots", 2)):
        if rng.random() < 0.5:
            fields[name] = wire_counts(rng, n)
    if rng.random() < 0.6:
        fields["density_plan2"] = wire_counts(rng, rng.choice((48, 46, 44, 40, 39, 47, 50)))
    for name in ("density_floor", "density_block", "summary_cell_um"):
        if rng.random() < 0.6:
            fields[name] = wire_float(rng)
    if rng.random() < 0.6:
        parts = [rng.choice(("kept1", "culled2", "deep1", "")) + ":" + rng.choice(
            ("3/4", "1/1", "0/9", "a/2", "5", "1/2/3", "+2/7", "1_0/2")) for _ in range(
            rng.randrange(1, 5))]
        fields["place_walks"] = rng.choice((",".join(parts), "-", "", "x", "y:1"))
    for name, values in (("summary_none", ("-", "policy", "exact", "off", "layers", "")),
                         ("fit_layer_edge", ("-", "14/367", "1/0", "")),
                         ("mode", ("baseline", "ordered", ""))):
        if rng.random() < 0.5:
            fields[name] = rng.choice(values)
    for name in ("passes", "labels_truncated", "density_round", "deferred"):
        if rng.random() < 0.5:
            fields.pop(name, None)
    if rng.random() < 0.3:
        fields["deferred"] = str(rng.choice((0, 1, 3, 12)))
    if "sub_cut_box_level" in fields:
        # the bar's `x2^N coarser`: Python builds 1 << N whole (a fuzzed
        # 10^11 would take its memory); its 4300 digits' edge
        fields["sub_cut_box_level"] = rng.choice(
            ("0", "1", "3", "20", "62", "63", "64", "126", "127", "130", "14284", "14285",
             "-1", "x", "2_0"))
    fields["gen"] = str(gen)
    if not probe:
        if final:
            fields["final"] = "1"
        elif rng.random() < 0.7:
            fields["final"] = "0"
    return fields


def fuzz_rounds(temp, generations=400):
    """The adapter's own _submit_render / _emit_frame on frame lines made up
    here (no renderd: a worker without a process, its frame files written
    here), recorded like a render's. Returns the rounds emitted."""
    import queue
    import threading
    rng = random.Random(SEED + 1)
    names = wire_names()
    work = temp / "fuzz"
    work.mkdir(exist_ok=True)
    w = rust_render.RustRenderWorker.__new__(rust_render.RustRenderWorker)
    w.cache = types.SimpleNamespace(meta={"dbu": 0.001, "layers": []}, dir=str(work))
    w.res = queue.Queue()
    w._jobs, w._jobs_lock = {}, threading.Lock()
    w._work_dir = str(work)
    w._label_font_px, w._raw_frames, w._thin_default = 14, True, "keep"
    w._mono, w._style_epoch, w._jobs_count, w._tile_px = False, 0, 8, 384
    w._round_pages = rust_render._NO_REFINEMENT_ROUND_PAGES
    w._record_path = os.environ["FLOE_RUST_RECORD"]
    w._send = lambda command: None
    emitted = 0
    for k in range(generations):
        gen = FUZZ_GEN + k
        w._raster_jobs_count = rng.choice((1, 4, 8))
        w._max_depth = rng.choice((None, 0, 11))
        w.cache.meta["dbu"] = rng.choice((0.001, 0.00025, 0.005))
        width, height = rng.randrange(1, 6), rng.randrange(1, 5)
        x0, y0 = rng.choice((0, -1500.5, 12345)), rng.choice((0, 7.25, -10))
        span = rng.choice((1, 999.5, 1e6, 3.3e7))
        job = {"kind": "render", "gen": gen, "bbox": rng.choice((
                   (float(x0), float(y0), x0 + span, y0 + span),
                   [x0, y0, x0 + 1000, y0 + 750])), "w": width, "h": height,
               "depth": None, "cut_px": rng.choice((0, 0.0, 1, 3.0, 5.0, 0.37, -2.0, None)),
               "frames": True, "labels": True, "visible": None}
        if rng.random() < 0.7:
            job["scope"] = rng.choice(("live", "headless", "preview"))
        if rng.random() < 0.4:
            job["bg"] = rng.choice((True, False, 0, 1))
        probe = rng.random() < 0.1
        w._submit_render(job, probe="baseline" if probe else None)
        state = w._jobs[gen]
        rounds = 1 if probe else rng.choice((1, 1, 2, 3, 5))
        for r in range(rounds):
            final = r == rounds - 1
            fields = wire_fields(rng, names, gen, final, probe)
            fmt = "raw" if rng.random() < 0.85 else "png"
            path = state["output"] if fmt == "raw" else state["output"] + ".gen-%d.png" % r
            with open(path, "wb") as fh:
                if fmt == "raw":
                    fh.write(b"FLOERAW1" + width.to_bytes(4, "little") +
                             height.to_bytes(4, "little") + bytes(width * height * 4))
                else:
                    fh.write(b"\x89PNG\r\n\x1a\n" + bytes(16))
            fields["format"] = fmt
            fields["out" if probe else "png"] = path
            w._emit_frame(fields, probe=probe)
            res = w.res.get_nowait()
            assert res.get("kind") in ("frame", "probe_frame"), res
            Session.emitted.append({k: v for k, v in res.items() if k not in ("rgba", "png")})
            emitted += 1
        w._jobs.pop(gen, None)
    return emitted


def recorded_rounds(record_path):
    records = [json.loads(line) for line in
               Path(record_path).read_text(encoding="utf-8").splitlines() if line]
    emitted = Session.emitted
    assert len(records) == len(emitted), (
        "%d frames emitted, %d recorded" % (len(emitted), len(records)))
    # the record holds the result the queue gave - but for place_walks: the
    # adapter's result shares its [walks, members] lists with the
    # generation's sums (a shallow dict copy), so an earlier round's dict
    # shows the later rounds' counts once they came; the record is written
    # as it was emitted (and the Rust result is a copy)
    for record, res in zip(sorted(records, key=lambda r: (r["result"]["gen"],
                                                          r["result"]["rounds"])),
                           sorted(emitted, key=lambda r: (r["gen"], r["rounds"]))):
        mine = {k: v for k, v in record["result"].items() if k != "place_walks"}
        theirs = roundtrip({k: v for k, v in res.items() if k != "place_walks"})
        assert mine == theirs, "the record of gen %d is not the emitted result" % res["gen"]
    rounds = []
    for i, record in enumerate(records):
        record["depth_note"] = "" if i % 3 else ", depth %d" % (i % 16)
        result = roundtrip(record["result"])
        try:
            record["full"], record["brief"] = perf_line.perf_status(result, record["depth_note"])
        except (TypeError, ValueError, OverflowError, MemoryError):
            # a fuzzed value Python itself refuses (a sub-cut box level past
            # 4300 digits of 1 << N, a negative shift): no line to compare
            assert result["gen"] >= FUZZ_GEN, "perf_status refused a real render's result"
            record["full"] = record["brief"] = None
        rounds.append(record)
    return rounds


def coverage(rounds):
    """What the recorded rounds reached (the gate's own check that its
    scenarios still exercise what they are there for)."""
    seen = {
        "refining rounds": any(r["result"].get("refining") for r in rounds),
        "multi-round generations": any(r["result"].get("rounds", 0) > 1 for r in rounds),
        "labels": any(r["result"].get("labels") for r in rounds),
        # (renderd's frame cache is its retained frames: frame_cache_hit
        # stays 0, the reuse is the tiles)
        "frame cache reuse (tiles reused)": any(r["result"].get("tiles_reused")
                                                for r in rounds),
        "margin": any(r["result"].get("bg") for r in rounds),
        "density stack": any(r["result"].get("density_stack") for r in rounds),
        "density off": any(r["result"].get("density_stack") is None for r in rounds),
        "pass 2 by occupancy": any(
            (r["result"].get("density_plan2") or {}).get("occ_layers") for r in rounds),
        "pass 2 by the plans": any(
            r["result"].get("density_plan2") and
            not r["result"]["density_plan2"].get("occ_layers") for r in rounds),
        "budget fit": any((r["result"].get("plan_culls") or {}).get("fit_thin") or
                          (r["result"].get("plan_culls") or {}).get("fit_ranked")
                          for r in rounds),
        "probe": any(r["probe"] for r in rounds),
        "deck": any(r["result"].get("deck") for r in rounds),
        "cut_um": any(r["result"].get("cut_um") for r in rounds),
        "png frame": any(r["result"].get("frame_format") == "png" for r in rounds),
    }
    missing = [k for k, v in seen.items() if not v]
    assert not missing, "the recorded renders no longer reach: %s" % ", ".join(missing)
    return seen


# ---- the comparison ------------------------------------------------------

def same(a, b, path="result"):
    """Equal in value AND type (an int is not a float, True is not 1; a
    float by its repr, so -0.0 is not 0.0); the first difference or None."""
    if type(a) is not type(b):
        return "%s: rust %r (%s) python %r (%s)" % (path, a, type(a).__name__, b,
                                                    type(b).__name__)
    if isinstance(a, dict):
        for key in sorted(set(a) | set(b)):
            if key not in a or key not in b:
                return "%s.%s: rust %r python %r" % (path, key, a.get(key, "<missing>"),
                                                     b.get(key, "<missing>"))
            diff = same(a[key], b[key], "%s.%s" % (path, key))
            if diff:
                return diff
        return None
    if isinstance(a, list):
        if len(a) != len(b):
            return "%s: rust %r python %r" % (path, a, b)
        for i, (x, y) in enumerate(zip(a, b)):
            diff = same(x, y, "%s[%d]" % (path, i))
            if diff:
                return diff
        return None
    if isinstance(a, float):
        return None if repr(a) == repr(b) or (math.isnan(a) and math.isnan(b)) else \
            "%s: rust %r python %r" % (path, a, b)
    return None if a == b else "%s: rust %r python %r" % (path, a, b)


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--keep", help="write the corpus and outputs here (kept)")
    parser.add_argument("--synthetic-only", action="store_true",
                        help="skip the real renders")
    args = parser.parse_args()
    t0 = time.monotonic()
    os.environ["FLOE_INDEX_BIN"] = str(ROOT / "rust" / "target" / "release" / "floe-index")
    os.environ["FLOE_RENDERD_BIN"] = str(ROOT / "rust" / "target" / "release" / "floe-renderd")
    status, refused, occ_cases, counts, made, (hit, lines) = synthetic_cases()
    loads = load_cases()
    print("== synthetic: %d result dicts made, %d perf lines (%d refused by Python itself), "
          "%d occupancy notes, %d counts, %d load notes; %d/%d lines of perf_status, "
          "occ_note and fmt_count reached (%.1f s)" % (
              made, len(status), refused, len(occ_cases), len(counts), len(loads), hit, lines,
              time.monotonic() - t0), flush=True)
    assert len(status) > made * 0.6, "too many synthetic dicts refused: %d" % refused
    with tempfile.TemporaryDirectory(prefix="floe-perf-") as temp:
        temp = Path(args.keep) if args.keep else Path(temp)
        temp.mkdir(parents=True, exist_ok=True)
        rounds = []
        if not args.synthetic_only:
            t1 = time.monotonic()
            record = temp / "rounds.jsonl"
            if record.exists():
                record.unlink()
            chip = temp / "chip.oas"
            run([sys.executable, "-B", ROOT / "tools" / "gen_main01_like.py", chip,
                 "--scale", "0.003", "--jobs", "2", "--geometry", "legacy"])
            run([FLOE2, "index", chip, "--jobs", "2", "--force"])
            deck_dir = temp / "deck"
            deck_dir.mkdir(exist_ok=True)
            deck_layout(deck_dir / "parts.oas")
            (deck_dir / "perf.jb").write_text(DECK_SPEC)
            run([FLOE2, "index", deck_dir / "perf.jb", "--jobs", "2", "--force"])
            print("== fixtures: chip + jobdeck indexed (%.1f s)" % (time.monotonic() - t1),
                  flush=True)
            t2 = time.monotonic()
            os.environ["FLOE_RUST_RECORD"] = str(record)
            os.environ["FLOE_RUST_RETAINED_MB"] = "256"
            try:
                scenarios = chip_scenarios(chip) + deck_scenarios(deck_dir / "perf.jb")
                fuzzed = fuzz_rounds(temp)
            finally:
                os.environ.pop("FLOE_RUST_RECORD", None)
            rounds = recorded_rounds(record)
            real = [r for r in rounds if r["result"]["gen"] < FUZZ_GEN]
            seen = coverage(real)
            print("== real renders: %d scenarios, %d rounds in %d generations recorded "
                  "(and %d fuzzed frame lines through the adapter in %d generations); "
                  "reached %s (%.1f s)" % (
                      len(scenarios), len(real), len({r["state"] for r in real}), fuzzed,
                      len({r["state"] for r in rounds}) - len({r["state"] for r in real}),
                      ", ".join(seen), time.monotonic() - t2), flush=True)
        corpus = temp / "corpus.json"
        out = temp / "rust_out.json"
        corpus.write_text(json.dumps({"status": status, "count": counts, "occ": occ_cases,
                                      "load": loads, "rounds": rounds}, allow_nan=False))
        t3 = time.monotonic()
        env = dict(os.environ, FLOE_PERF_CORPUS=str(corpus), FLOE_PERF_OUT=str(out),
                   PATH=str(Path.home() / ".cargo" / "bin") + os.pathsep +
                   os.environ.get("PATH", ""))
        test = subprocess.run(
            ["cargo", "test", "--release", "--offline", "-p", "floe-app-core", "--test",
             "perf_parity", "--", "--ignored", "--nocapture"],
            cwd=ROOT / "rust", env=env, capture_output=True, text=True, timeout=1800)
        tail = (test.stdout + test.stderr).splitlines()
        for line in tail:
            if line.startswith(("perf parity", "MISMATCH", "  rust", "  python")):
                print(line)
        assert test.returncode == 0, "the Rust perf parity test failed:\n%s" % "\n".join(
            tail[-60:])
        # once more here, with Python's types
        rust = json.loads(out.read_text())
        failures = []
        for case, got in zip(status, rust["status"]):
            if [case["full"], case["brief"]] != got:
                failures.append("status: %r != %r" % (got, [case["full"], case["brief"]]))
        for case, got in zip(counts, rust["count"]):
            if case["out"] != got:
                failures.append("fmt_count(%r): %r != %r" % (case["n"], got, case["out"]))
        for case, got in zip(occ_cases, rust["occ"]):
            if case["out"] != got:
                failures.append("occ_note: %r != %r" % (got, case["out"]))
        for case, got in zip(loads, rust["load"]):
            if [case["full"], case["brief"]] != got:
                failures.append("load_note: %r != %r" % (got, [case["full"], case["brief"]]))
        for case, got in zip(rounds, rust["rounds"]):
            diff = same(got["result"], case["result"])
            if diff:
                failures.append("round gen %d: %s" % (case["result"]["gen"], diff))
            if case["full"] is not None and [got["full"], got["brief"]] != [case["full"],
                                                                             case["brief"]]:
                failures.append("round gen %d perf line:\n  rust   %s\n  python %s" % (
                    case["result"]["gen"], got["full"], case["full"]))
        lengths = [(len(rust[k]), len(v)) for k, v in (
            ("status", status), ("count", counts), ("occ", occ_cases), ("load", loads),
            ("rounds", rounds))]
        assert all(a == b for a, b in lengths), "Rust answered %s" % lengths
        for failure in failures[:20]:
            print("MISMATCH " + failure)
        assert not failures, "%d perf parity mismatches" % len(failures)
        print("== rust replay: %.1f s" % (time.monotonic() - t3))
    synthetic = len(status) + len(counts) + len(occ_cases) + len(loads)
    print("PERF PARITY OK: %d synthetic cases (%d perf lines, %d counts, %d occupancy notes, "
          "%d load notes), %d recorded rounds - all equal (%.0f s)" % (
              synthetic, len(status), len(counts), len(occ_cases), len(loads), len(rounds),
              time.monotonic() - t0))


if __name__ == "__main__":
    main()
