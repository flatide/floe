#!/usr/bin/env python3
"""Generate a large synthetic OASIS test file resembling a real chip design.

Structure:
  TOP
   +- BLK_r_c (block grid)
        +- logic block: rows of std cell instances (hierarchy / placement records)
        +- ROUTE_B*: random routing wires on M2..M6 (+ vias)
        +- FILL_M*_B*: dummy metal fill (random boxes) -- dominates file size
        +- SRAM blocks: BITCELL array via regular CellInstArray (repetition records)
   +- seal ring frame, die boundary, markers with text labels (layer 63/63)

Fill tiles are generated in parallel worker processes (each writes a small
OASIS file), then merged into the master layout and instantiated.

A manifest JSON (<out>.manifest.json) records die/block/marker coordinates and
per-layer shape counts so downstream tools (clip/extract) can be verified
against known ground truth.
"""

import argparse
import gc
import json
import multiprocessing as mp
import os
import shutil
import sys
import tempfile
import time

import numpy as np
import klayout.db as db

DBU = 0.001  # 1 nm

LAYERS = [
    (0, 0, "BOUNDARY"),
    (1, 0, "NWELL"),
    (2, 0, "ACTIVE"),
    (3, 0, "POLY"),
    (4, 0, "CONT"),
    (5, 0, "M1"),
    (6, 0, "VIA1"),
    (7, 0, "M2"),
    (8, 0, "VIA2"),
    (9, 0, "M3"),
    (10, 0, "VIA3"),
    (11, 0, "M4"),
    (12, 0, "VIA4"),
    (13, 0, "M5"),
    (14, 0, "VIA5"),
    (15, 0, "M6"),
    (5, 1, "M1_FILL"),
    (7, 1, "M2_FILL"),
    (9, 1, "M3_FILL"),
    (11, 1, "M4_FILL"),
    (13, 1, "M5_FILL"),
    (15, 1, "M6_FILL"),
    (63, 63, "MARKER"),
]

FILL_WEIGHTS = {
    "M1_FILL": 0.25,
    "M2_FILL": 0.20,
    "M3_FILL": 0.20,
    "M4_FILL": 0.15,
    "M5_FILL": 0.10,
    "M6_FILL": 0.10,
}

# (name, width_nm)
STD_CELLS = [
    ("INVX1", 1000), ("INVX4", 1600), ("NAND2X1", 1400), ("NOR2X1", 1400),
    ("DFFX1", 4200), ("BUFX2", 1600), ("AOI21X1", 1800), ("OAI22X1", 2200),
    ("MUX2X1", 2600), ("XOR2X1", 2400), ("FILLCAP4", 800), ("TIEHI", 800),
]
ROW_H = 2500  # nm

FILL_WMIN, FILL_WMAX = 60, 400  # nm


def save_options():
    opt = db.SaveLayoutOptions()
    opt.format = "OASIS"
    opt.oasis_write_cblocks = True
    opt.oasis_compression_level = 2
    opt.write_context_info = False
    return opt


def make_layers(ly):
    idx = {}
    for l, d, name in LAYERS:
        idx[name] = ly.layer(db.LayerInfo(l, d, name))
    return idx


def insert_random_boxes(shapes, n, xmax, ymax, rng, chunk=500_000):
    ins = shapes.insert
    Box = db.Box
    remaining = n
    while remaining > 0:
        m = min(chunk, remaining)
        xs = rng.integers(0, xmax - FILL_WMAX, m).tolist()
        ys = rng.integers(0, ymax - FILL_WMAX, m).tolist()
        ws = rng.integers(FILL_WMIN, FILL_WMAX, m).tolist()
        hs = rng.integers(FILL_WMIN, FILL_WMAX, m).tolist()
        for x, y, w, h in zip(xs, ys, ws, hs):
            ins(Box(x, y, x + w, y + h))
        remaining -= m


def fill_tile_worker(task):
    """Generate one fill tile layout file. Runs in a worker process."""
    path, cell_name, layer, datatype, n, size_nm, seed, tile_id = task
    ly = db.Layout()
    ly.dbu = DBU
    cell = ly.create_cell(cell_name)
    li = ly.layer(layer, datatype)
    rng = np.random.default_rng([seed, tile_id])
    insert_random_boxes(cell.shapes(li), n, size_nm, size_nm, rng)
    ly.write(path, save_options())
    return path, cell_name, n


def probe_bytes_per_shape(tmpdir, seed):
    """Measure OASIS bytes per random fill box with the final write options.

    Uses a shape count close to the real per-tile count, since zlib/delta
    efficiency depends on it.
    """
    n = 1_500_000
    path = os.path.join(tmpdir, "probe.oas")
    fill_tile_worker((path, "PROBE", 5, 1, n, 1_500_000, seed, 999_999))
    bps = os.path.getsize(path) / n
    os.remove(path)
    return bps


def make_std_cell_lib(ly, L):
    cells = []
    for name, w in STD_CELLS:
        c = ly.create_cell(name)
        c.shapes(L["NWELL"]).insert(db.Box(0, 1300, w, ROW_H))
        c.shapes(L["ACTIVE"]).insert(db.Box(100, 200, w - 100, 1000))
        c.shapes(L["ACTIVE"]).insert(db.Box(100, 1500, w - 100, 2400))
        x = 300
        while x + 120 <= w - 200:
            c.shapes(L["POLY"]).insert(db.Box(x, 100, x + 120, ROW_H - 100))
            cx = x + 260
            if cx + 60 <= w - 100:
                c.shapes(L["CONT"]).insert(db.Box(cx, 550, cx + 60, 610))
                c.shapes(L["CONT"]).insert(db.Box(cx, 1900, cx + 60, 1960))
            x += 400
        c.shapes(L["M1"]).insert(db.Box(0, 0, w, 250))          # VSS rail
        c.shapes(L["M1"]).insert(db.Box(0, ROW_H - 250, w, ROW_H))  # VDD rail
        cells.append((c.cell_index(), w))
    return cells


def build_logic_block(ly, L, name, size_nm, std_cells, rng):
    """Rows of std cell instances, ~60% utilization, sparse rows."""
    blk = ly.create_cell(name)
    margin = 50_000
    row_pitch = 15_000
    n_inst = 0
    y = margin
    while y + ROW_H <= size_nm - margin:
        x = margin
        while x < size_nm - margin - 5000:
            ci, w = std_cells[rng.integers(0, len(std_cells))]
            if rng.random() < 0.6:
                blk.insert(db.CellInstArray(ci, db.Trans(db.Vector(x, y))))
                n_inst += 1
            x += w
        y += row_pitch
    return blk, n_inst


def build_route_cell(ly, L, name, size_nm, rng, n_wires=8000):
    c = ly.create_cell(name)
    metals = ["M2", "M3", "M4", "M5", "M6"]
    vias = ["VIA1", "VIA2", "VIA3", "VIA4", "VIA5"]
    weights = np.array([0.3, 0.25, 0.2, 0.15, 0.1])
    picks = rng.choice(len(metals), n_wires, p=weights)
    xs = rng.integers(0, size_nm - 100_000, n_wires)
    ys = rng.integers(0, size_nm - 100_000, n_wires)
    lens = rng.integers(5_000, 100_000, n_wires)
    wids = rng.integers(100, 500, n_wires)
    horiz = rng.random(n_wires) < 0.5
    Box = db.Box
    for i in range(n_wires):
        m, x, y, ln, w = int(picks[i]), int(xs[i]), int(ys[i]), int(lens[i]), int(wids[i])
        ml = c.shapes(L[metals[m]])
        if horiz[i]:
            ml.insert(Box(x, y, x + ln, y + w))
        else:
            ml.insert(Box(x, y, x + w, y + ln))
        vs = max(w - 40, 60)
        c.shapes(L[vias[m]]).insert(Box(x + 20, y + 20, x + 20 + vs, y + 20 + vs))
    return c, n_wires * 2


def build_sram_block(ly, L, name, size_nm):
    """2x2 SRAM macros, each a 1024x2048 bitcell array (regular repetition)."""
    bit = ly.cell("BITCELL")
    if bit is None:
        bit = ly.create_cell("BITCELL")
        bit.shapes(L["ACTIVE"]).insert(db.Box(50, 30, 250, 270))
        bit.shapes(L["ACTIVE"]).insert(db.Box(350, 30, 550, 270))
        bit.shapes(L["POLY"]).insert(db.Box(20, 100, 280, 160))
        bit.shapes(L["POLY"]).insert(db.Box(320, 100, 580, 160))
        bit.shapes(L["CONT"]).insert(db.Box(120, 40, 180, 100))
        bit.shapes(L["CONT"]).insert(db.Box(420, 190, 480, 250))
        bit.shapes(L["M1"]).insert(db.Box(100, 0, 200, 300))
        bit.shapes(L["M1"]).insert(db.Box(400, 0, 500, 300))
    macro = ly.cell("SRAM_MACRO")
    if macro is None:
        macro = ly.create_cell("SRAM_MACRO")
        nx, ny = 1024, 2048
        macro.insert(db.CellInstArray(
            bit.cell_index(), db.Trans(db.Vector(0, 0)),
            db.Vector(600, 0), db.Vector(0, 300), nx, ny))
        for i in range(16):  # power straps over the array
            x = i * 38_000
            macro.shapes(L["M4"]).insert(db.Box(x, 0, x + 4000, ny * 300))
    blk = ly.create_cell(name)
    macros = []
    msz = 1024 * 600  # 614.4 um
    for gx in range(2):
        for gy in range(2):
            ox = 80_000 + gx * 700_000
            oy = 80_000 + gy * 700_000
            blk.insert(db.CellInstArray(macro.cell_index(), db.Trans(db.Vector(ox, oy))))
            macros.append((ox, oy, ox + msz, oy + 2048 * 300))
    return blk, macros


def add_marker(top, L, name, x, y):
    top.shapes(L["MARKER"]).insert(db.Box(x - 10_000, y - 1_000, x + 10_000, y + 1_000))
    top.shapes(L["MARKER"]).insert(db.Box(x - 1_000, y - 10_000, x + 1_000, y + 10_000))
    top.shapes(L["MARKER"]).insert(db.Text(name, db.Trans(db.Vector(x, y))))
    return {"name": name, "x_nm": x, "y_nm": y, "x_um": x / 1000, "y_um": y / 1000,
            "layer": "63/63"}


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--out", default="data/testchip.oas")
    ap.add_argument("--target-gb", type=float, default=1.5)
    ap.add_argument("--seed", type=int, default=42)
    ap.add_argument("--grid", type=int, default=6, help="block grid size (NxN)")
    ap.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 4) - 1))
    ap.add_argument("--no-verify", action="store_true")
    ap.add_argument("--keep-tiles", action="store_true")
    args = ap.parse_args()

    t_start = time.perf_counter()
    out = os.path.abspath(args.out)
    os.makedirs(os.path.dirname(out), exist_ok=True)
    tmpdir = tempfile.mkdtemp(prefix="_tiles_", dir=os.path.dirname(out))

    grid = args.grid
    block = 1_500_000          # 1.5 mm block
    gap = 100_000
    margin = 500_000
    pitch = block + gap
    die = 2 * margin + grid * block + (grid - 1) * gap
    sram_blocks = {(0, 0), (grid - 1, grid - 1)}

    print(f"[gen] die {die/1e6:.1f} x {die/1e6:.1f} mm, {grid}x{grid} blocks, "
          f"target {args.target_gb} GB, jobs {args.jobs}", flush=True)

    # --- calibration ---
    t0 = time.perf_counter()
    bps = probe_bytes_per_shape(tmpdir, args.seed)
    target_bytes = args.target_gb * 1e9
    overhead_est = 30e6
    n_fill_total = max(0, int((target_bytes - overhead_est) / bps))
    print(f"[gen] probe: {bps:.2f} B/shape -> {n_fill_total/1e6:.1f}M fill shapes "
          f"({time.perf_counter()-t0:.1f}s)", flush=True)

    # --- fill tile tasks ---
    n_blocks = grid * grid
    tasks = []
    tile_id = 0
    for fname, wgt in FILL_WEIGHTS.items():
        n_per_tile = max(1, round(n_fill_total * wgt / n_blocks))
        l, d, _ = next(t for t in LAYERS if t[2] == fname)
        for r in range(grid):
            for c in range(grid):
                cell_name = f"FILL_{fname[:2]}_B{r}_{c}"
                path = os.path.join(tmpdir, f"tile_{tile_id}.oas")
                tasks.append((path, cell_name, l, d, n_per_tile, block,
                              args.seed, tile_id))
                tile_id += 1

    t0 = time.perf_counter()
    done = 0
    with mp.Pool(args.jobs) as pool:
        results = []
        for res in pool.imap_unordered(fill_tile_worker, tasks, chunksize=1):
            results.append(res)
            done += 1
            if done % 24 == 0 or done == len(tasks):
                print(f"[gen] fill tiles {done}/{len(tasks)} "
                      f"({time.perf_counter()-t0:.0f}s)", flush=True)
    total_fill = sum(n for _, _, n in results)
    print(f"[gen] fill generation done: {total_fill/1e6:.1f}M shapes "
          f"in {time.perf_counter()-t0:.0f}s", flush=True)

    # --- master layout ---
    ly = db.Layout()
    ly.dbu = DBU
    L = make_layers(ly)
    top = ly.create_cell("ZENOAS_TESTCHIP")
    rng = np.random.default_rng(args.seed)

    top.shapes(L["BOUNDARY"]).insert(db.Box(0, 0, die, die))
    for mname in ["M1", "M2", "M3", "M4", "M5", "M6"]:  # seal ring frame
        for b in (db.Box(50_000, 50_000, die - 50_000, 100_000),
                  db.Box(50_000, die - 100_000, die - 50_000, die - 50_000),
                  db.Box(50_000, 50_000, 100_000, die - 50_000),
                  db.Box(die - 100_000, 50_000, die - 50_000, die - 50_000)):
            top.shapes(L[mname]).insert(b)

    std_cells = make_std_cell_lib(ly, L)

    # --- merge fill tiles ---
    t0 = time.perf_counter()
    for i, (path, _, _) in enumerate(sorted(results)):
        ly.read(path)
        if (i + 1) % 48 == 0:
            print(f"[gen] merged {i+1}/{len(results)} tiles "
                  f"({time.perf_counter()-t0:.0f}s)", flush=True)
    print(f"[gen] merge done ({time.perf_counter()-t0:.0f}s)", flush=True)

    # --- blocks ---
    t0 = time.perf_counter()
    manifest_blocks = []
    total_inst = 0
    for r in range(grid):
        for c in range(grid):
            bx = margin + c * pitch
            by = margin + r * pitch
            blk_name = f"BLK_{r}_{c}"
            info = {"name": blk_name, "kind": "logic",
                    "bbox_nm": [bx, by, bx + block, by + block]}
            if (r, c) in sram_blocks:
                blk, macros = build_sram_block(ly, L, blk_name, block)
                info["kind"] = "sram"
                info["macros_local_nm"] = macros
            else:
                blk, n_inst = build_logic_block(ly, L, blk_name, block, std_cells, rng)
                total_inst += n_inst
                route, _ = build_route_cell(ly, L, f"ROUTE_B{r}_{c}", block, rng)
                blk.insert(db.CellInstArray(route.cell_index(), db.Trans(db.Vector(0, 0))))
            for fname in FILL_WEIGHTS:
                fc = ly.cell(f"FILL_{fname[:2]}_B{r}_{c}")
                if fc is not None:
                    blk.insert(db.CellInstArray(fc.cell_index(), db.Trans(db.Vector(0, 0))))
            top.insert(db.CellInstArray(blk.cell_index(), db.Trans(db.Vector(bx, by))))
            manifest_blocks.append(info)
    print(f"[gen] blocks built: {total_inst/1e6:.2f}M std cell instances "
          f"({time.perf_counter()-t0:.0f}s)", flush=True)

    # --- markers ---
    markers = []
    ctr = die // 2
    markers.append(add_marker(top, L, "MARK_CENTER", ctr, ctr))
    for i, (mx, my) in enumerate([(200_000, 200_000), (die - 200_000, 200_000),
                                  (200_000, die - 200_000),
                                  (die - 200_000, die - 200_000)]):
        markers.append(add_marker(top, L, f"MARK_CORNER_{i}", mx, my))
    sx = margin + 80_000 + 300_000  # inside SRAM block (0,0) macro
    markers.append(add_marker(top, L, "MARK_SRAM", sx, sx))

    # --- per-layer stored shape counts ---
    layer_counts = {}
    for lname, li in L.items():
        n = 0
        for cell in ly.each_cell():
            n += cell.shapes(li).size()
        layer_counts[lname] = n

    # --- write ---
    t0 = time.perf_counter()
    print(f"[gen] writing {out} ...", flush=True)
    ly.write(out, save_options())
    size = os.path.getsize(out)
    print(f"[gen] wrote {size/1e9:.2f} GB in {time.perf_counter()-t0:.0f}s", flush=True)

    manifest = {
        "file": os.path.basename(out),
        "size_bytes": size,
        "seed": args.seed,
        "dbu_um": DBU,
        "die_bbox_nm": [0, 0, die, die],
        "grid": grid,
        "block_size_nm": block,
        "top_cell": "ZENOAS_TESTCHIP",
        "layers": [{"layer": l, "datatype": d, "name": n} for l, d, n in LAYERS],
        "layer_stored_shape_counts": layer_counts,
        "blocks": manifest_blocks,
        "markers": markers,
        "sram": {"bitcell_pitch_nm": [600, 300], "array": [1024, 2048],
                 "macros_per_block": 4},
        "total_fill_shapes": total_fill,
        "std_cell_instances": total_inst,
    }
    with open(out + ".manifest.json", "w") as f:
        json.dump(manifest, f, indent=2)
    print(f"[gen] manifest: {out}.manifest.json", flush=True)

    if not args.keep_tiles:
        shutil.rmtree(tmpdir, ignore_errors=True)

    del ly
    gc.collect()

    if not args.no_verify:
        t0 = time.perf_counter()
        print("[gen] verifying (full re-read)...", flush=True)
        ly2 = db.Layout()
        ly2.read(out)
        tc = ly2.top_cell()
        bb = tc.bbox()
        print(f"[gen] verify OK: top={tc.name}, cells={ly2.cells()}, "
              f"bbox=({bb.left},{bb.bottom})-({bb.right},{bb.top}) dbu, "
              f"read {time.perf_counter()-t0:.0f}s", flush=True)

    print(f"[gen] total {time.perf_counter()-t_start:.0f}s", flush=True)


if __name__ == "__main__":
    main()
