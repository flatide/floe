#!/usr/bin/env python3
"""Generate a deterministic OASIS fixture for monster-cell indexing.

The expensive cell contains many differently-sized rectangles.  Each size is
repeated at scattered, die-wide positions, so KLayout's OASIS writer stores a
small number of explicit-point repetition records while the VFS page splitter
must partition millions of members.  This is the on-disk counterpart of the
``monster_cell_p2_bench`` ignored Rust benchmark.

Examples:

    .venv/bin/python tools/gen_monster_cell.py data/monster_cell.oas
    .venv/bin/python tools/gen_monster_cell.py data/monster_field.oas \
        --preset field

``standard`` is suitable for routine profiling.  ``field`` reproduces the
500 x 50,000 Pts-flood shape used by the heavyweight benchmark and needs much
more generation time and memory.
"""

import argparse
from pathlib import Path
import random
import time

import klayout.db as db


PRESETS = {
    # All presets cross the 8 MiB P2 eligibility floor (four estimated bytes
    # per explicit point) while leaving increasingly long observation windows.
    "quick": (128, 16_000, 250_000, 16),
    "standard": (400, 20_000, 1_000_000, 48),
    "field": (500, 50_000, 0, 96),
}
GRID_SIDE = 8192
GRID_PITCH = 1000
GRID_SLOTS = GRID_SIDE * GRID_SIDE


def positions(seed, record, count):
    """Yield unique scattered positions without a full-domain bitmap."""
    # Independent random samples avoid the spatial correlation of an affine
    # permutation.  That correlation makes KLayout split a logical repeated
    # size into many OASIS geometry records, shifting the workload from page
    # splitting to assembly.  sample(range(...)) keeps only O(count) indices
    # and guarantees that identical boxes never collapse as duplicates.
    rng = random.Random(seed * 1_000_003 + record * 97_409)
    for slot in rng.sample(range(GRID_SLOTS), count):
        yield ((slot % GRID_SIDE) * GRID_PITCH,
               (slot // GRID_SIDE) * GRID_PITCH)


def save_options():
    options = db.SaveLayoutOptions()
    options.format = "OASIS"
    options.oasis_write_cblocks = True
    options.oasis_compression_level = 2
    options.write_context_info = False
    return options


def insert_pts_flood(shapes, seed, record, count, width, height):
    for x, y in positions(seed, record, count):
        shapes.insert(db.Box(x, y, x + width, y + height))


def generate(output, reps, members, giant_members, fillers, seed):
    layout = db.Layout()
    layout.dbu = 0.001
    dominant_layer = layout.layer(db.LayerInfo(1, 0, "MONSTER_PTS"))
    minor_layer = layout.layer(db.LayerInfo(2, 0, "MINOR_GRID"))
    boundary_layer = layout.layer(db.LayerInfo(255, 0, "BOUNDARY"))

    # Create lightweight cells first.  KLayout writes these sibling
    # dependencies after MONSTER, keeping the expensive cell at the ordered
    # commit head while later planners fill the admission window and lend
    # their shared #76 slots.
    filler_cells = []
    for index in range(fillers):
        cell = layout.create_cell("FILLER_%04d" % index)
        x = index * 20
        cell.shapes(minor_layer).insert(db.Box(x, 0, x + 10, 10))
        filler_cells.append(cell)

    monster = layout.create_cell("MONSTER")
    flood = monster.shapes(dominant_layer)
    started = time.perf_counter()
    report_every = max(1, reps // 20)
    for record in range(reps):
        # Width is unique, forcing distinct repetition signatures.  Heights
        # vary independently to avoid an unrealistically modal-only stream.
        width = 80 + record
        height = 90 + (record * 37) % 211
        insert_pts_flood(flood, seed, record, members, width, height)
        if (record + 1) % report_every == 0 or record + 1 == reps:
            done = (record + 1) * members
            print("[gen] Pts records %d/%d, members %s (%.1fs)" %
                  (record + 1, reps, format(done, ","),
                   time.perf_counter() - started), flush=True)

    if giant_members:
        insert_pts_flood(flood, seed ^ 0xC0FFEE, reps + 17,
                         giant_members, 800, 800)
        print("[gen] giant Pts member count %s" %
              format(giant_members, ","), flush=True)

    # A small second layer keeps the dominant-layer decision honest instead
    # of taking a special one-layer path.  Its regular grids are cheap and
    # remain far below 40% of estimated split bytes.
    minor = monster.shapes(minor_layer)
    die = GRID_SIDE * GRID_PITCH
    for column in range(100):
        x = (column * 7919) % (die - 200)
        for row in range(200):
            y = row * (die // 200)
            minor.insert(db.Box(x, y, x + 150, y + 400))

    top = layout.create_cell("TOP")
    for cell in filler_cells:
        top.insert(db.CellInstArray(cell.cell_index(), db.Trans()))
    top.insert(db.CellInstArray(monster.cell_index(), db.Trans()))
    top.shapes(boundary_layer).insert(db.Box(0, 0, die, die))

    print("[gen] writing OASIS...", flush=True)
    layout.write(str(output), save_options())
    total = reps * members + giant_members
    return total, die, time.perf_counter() - started


def main():
    parser = argparse.ArgumentParser(
        description="generate a Pts-repetition monster-cell OASIS fixture")
    parser.add_argument("out", type=Path)
    parser.add_argument("--preset", choices=sorted(PRESETS),
                        default="standard")
    parser.add_argument("--reps", type=int,
                        help="number of die-wide Pts records")
    parser.add_argument("--members-per-rep", type=int,
                        help="members in each Pts record")
    parser.add_argument("--giant-members", type=int,
                        help="members in one additional giant Pts record")
    parser.add_argument("--fillers", type=int,
                        help="small cells after MONSTER in commit order")
    parser.add_argument("--seed", type=int, default=11)
    parser.add_argument("--force", action="store_true",
                        help="replace an existing output file")
    args = parser.parse_args()

    preset = PRESETS[args.preset]
    reps = args.reps if args.reps is not None else preset[0]
    members = (args.members_per_rep if args.members_per_rep is not None
               else preset[1])
    giant_members = (args.giant_members if args.giant_members is not None
                     else preset[2])
    fillers = args.fillers if args.fillers is not None else preset[3]
    for name, value in (("reps", reps), ("members-per-rep", members),
                        ("giant-members", giant_members),
                        ("fillers", fillers)):
        if value < 0 or (name in ("reps", "members-per-rep") and value == 0):
            parser.error("--%s must be %s" %
                         (name, "positive" if name in
                          ("reps", "members-per-rep") else "non-negative"))
    if max(members, giant_members) > GRID_SLOTS:
        parser.error("a repetition cannot exceed %s unique lattice slots" %
                     format(GRID_SLOTS, ","))
    if args.out.exists() and not args.force:
        parser.error("output exists; pass --force to replace it: %s" %
                     args.out)
    args.out.parent.mkdir(parents=True, exist_ok=True)

    total = reps * members + giant_members
    estimated_ram_gib = total * 46 / (1 << 30)
    estimated_rep_mib = total * 4 / (1 << 20)
    print("[gen] preset=%s reps=%d x %s + giant %s" %
          (args.preset, reps, format(members, ","),
           format(giant_members, ",")))
    print("[gen] %s repetition members, estimated editable-generation "
          "RAM %.2f GiB, VFS split input %.1f MiB" %
          (format(total, ","), estimated_ram_gib, estimated_rep_mib))
    if estimated_rep_mib < 8:
        print("[gen] warning: below the production P2 eligibility floor")

    total, die, elapsed = generate(
        args.out, reps, members, giant_members, fillers, args.seed)
    size = args.out.stat().st_size
    print("[gen] wrote %s: %.1f MiB, die %.3f mm, %s rep members "
          "in %.1fs" %
          (args.out, size / (1 << 20), die / 1_000_000,
           format(total, ","), elapsed))
    print("[gen] profile with: floe2 index %s --jobs N "
          "--profile-cell MONSTER > monster-profile.json" % args.out)


if __name__ == "__main__":
    main()
