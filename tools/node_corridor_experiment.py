#!/usr/bin/env python3
"""CUT_DENSITY_DESIGN §10.2 check 1 (2026-09-23): does one summary value per
child-BVH node keep the EMPTY space inside a node the size cut prunes whole?

The child-BVH cut (hier.rs prune_size) looks at the sizes of the cells placed
below a node, not at the node's box: a node of small cells scattered wide is
cut whole, and a density pass fed by one coverage value per node would spread
that value over the whole box - an empty corridor between two blocks of small
cells is filled in. This experiment measures that on a layout built for it:
~31,000 single placements of a 1 x 1 um cell on a jittered 10 um grid in two
blocks either side of a 200 um vertical corridor, crossed by a 100 um
horizontal one (plus a sparse scatter), indexed for real (floe2 index) and
read back through `floe-index bvh --cell TOP`. What the index makes of it
matters: the indexer folds the identical placements into ONE Pts placement
record, so the cell's child BVH is a single leaf holding that record, whose
extent is the whole layout - a summary per node has nothing finer than the
record unless it subdivides it. The index already stores the record's
64-member chunks with their offset boxes (32 B each, PageIndex / PtsRef), so
those are one candidate subdivision. At the 1920 x 1080 fit view (1 px =
1.85 um, cut 3 px = 5.6 um, so every placement is under the cut), the
predicted coverage on 4 px zones is compared with the truth for:

  * node: one value for the record's extent, and a k x k grid of values over
    it (k = 4, 16, 64);
  * chunk: one value per stored chunk box, and a 4 x 4 grid per chunk;
  * morton: the same 64-member chunks after sorting the members along a
    Morton (Z-order) curve - what spatially compact chunk boxes would give
    if the indexer ordered a Pts record that way (it keeps source order);

reporting false presence (the share of EMPTY zones marked covered, and of
the corridors' zones), mean absolute coverage error, and the summary bytes
(4 B a value, k x k B a grid; chunk boxes themselves are already stored).

    .venv/bin/python tools/node_corridor_experiment.py [--out DIR]
"""
import argparse
import math
import os
import random
import subprocess
import sys
import tempfile
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))
from floe.cachepath import vfs_cache_dir  # noqa: E402

W, H = 1920, 1080
DIE = 2000.0                       # um
SMALL = 1.0                        # um, the placed cell's box
PITCH = 10.0                       # um, the placement grid
CORRIDOR_X = (900.0, 1100.0)       # um, empty vertical band
CORRIDOR_Y = (950.0, 1050.0)       # um, empty horizontal band
ZONE_PX = 4


def build(path):
    import klayout.db as kdb
    rng = random.Random(7)
    ly = kdb.Layout()
    ly.dbu = 0.001
    top = ly.create_cell('TOP')
    small = ly.create_cell('SMALL')
    small.shapes(ly.layer(1, 0)).insert(kdb.DBox(0, 0, SMALL, SMALL))
    places = []
    n = int(DIE / PITCH)
    for j in range(n):
        for i in range(n):
            x = i * PITCH + rng.uniform(0.2, 3.0)
            y = j * PITCH + rng.uniform(0.2, 3.0)
            if CORRIDOR_X[0] <= x < CORRIDOR_X[1] or CORRIDOR_Y[0] <= y < CORRIDOR_Y[1]:
                continue
            if rng.random() < 0.1:       # a sparse scatter: some grid points empty
                continue
            places.append((x, y))
    for x, y in places:
        top.insert(kdb.CellInstArray(small.cell_index(), kdb.Trans(kdb.Vector(int(round(x / ly.dbu)), int(round(y / ly.dbu))))))
    ly.write(str(path))
    return places


def read_bvh(text):
    """nodes, and per placement record: base (x, y), chunk boxes (absolute)
    and member positions (absolute)"""
    nodes, records = {}, {}
    for line in text.splitlines():
        f = line.split('\t')
        if f[0] == 'node':
            nid, parent, depth, leaf, count = int(f[1]), int(f[2]), int(f[3]), int(f[4]), int(f[5])
            bbox = tuple(float(v) for v in f[6].split(','))
            nodes[nid] = dict(parent=parent, depth=depth, leaf=bool(leaf), count=count, bbox=bbox, masked=int(f[9]), children=[])
        elif f[0] == 'place':
            assert f[3] == 'SMALL', line
            pli, x, y, kind = int(f[2]), float(f[4]), float(f[5]), int(f[8])
            records[pli] = dict(node=int(f[1]), base=(x, y), kind=kind, chunks=[], pts=[(x, y)] if kind == 0 else [])
        elif f[0] == 'chunk':
            r = records[int(f[1])]
            x0, y0, x1, y1 = (float(v) for v in f[3].split(','))
            bx, by = r['base']
            r['chunks'].append(((x0 + bx, y0 + by, x1 + bx, y1 + by), int(f[4])))
        elif f[0] == 'pt':
            r = records[int(f[1])]
            bx, by = r['base']
            r['pts'].append((bx + float(f[3]), by + float(f[4])))
    for nid, n in nodes.items():
        if n['parent'] >= 0:
            nodes[n['parent']]['children'].append(nid)
    return nodes, records


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument('--out', help='keep the layout and index here (default: a temp dir)')
    ap.add_argument('--bin', default=str(ROOT / 'rust/target/release/floe-index'))
    args = ap.parse_args(argv)
    env = dict(os.environ, FLOE_INDEX_BIN=args.bin)
    with tempfile.TemporaryDirectory(prefix='floe-corridor-') as temp:
        out = Path(args.out) if args.out else Path(temp)
        out.mkdir(parents=True, exist_ok=True)
        src = out / 'corridor.oas'
        places = build(src)
        done = subprocess.run([sys.executable, '-B', '-m', 'floe2', 'index', str(src)], cwd=ROOT, env=env,
                              capture_output=True, text=True, timeout=600)
        assert done.returncode == 0, done.stdout + done.stderr
        ice = vfs_cache_dir(str(src))
        dump = subprocess.run([args.bin, 'bvh', str(ice), '--cell', 'TOP'], capture_output=True, text=True)
        assert dump.returncode == 0, dump.stderr
        nodes, records = read_bvh(dump.stdout)
        root = next(nid for nid, n in nodes.items() if n['parent'] < 0)
        members = [p for r in records.values() for p in r['pts']]
        assert len(members) == len(places), (len(members), len(places))
        kinds = sorted(set(r['kind'] for r in records.values()))
        # the view: fit, cut 3 px
        px = min(W / DIE, H / DIE)
        cut_um = 3.0 / px
        assert SMALL < cut_um
        zone = ZONE_PX / px                          # um
        nz = int(math.ceil(DIE / zone))
        # truth per zone: placed area (1 um boxes assigned to the zone of their centre)
        truth = defaultdict(float)
        for x, y in places:
            truth[(int((x + SMALL / 2) / zone), int((y + SMALL / 2) / zone))] += SMALL * SMALL
        zone_area = zone * zone
        all_zones = [(i, j) for j in range(nz) for i in range(nz)]
        empty = [z for z in all_zones if truth.get(z, 0.0) == 0.0]
        corridor = [(i, j) for (i, j) in all_zones
                    if (CORRIDOR_X[0] <= (i + 0.5) * zone < CORRIDOR_X[1] or CORRIDOR_Y[0] <= (j + 0.5) * zone < CORRIDOR_Y[1])
                    and truth.get((i, j), 0.0) == 0.0]
        def spread(bbox, pts, k):
            """predicted coverage per zone from a k x k grid of values over a box"""
            x0, y0, x1, y1 = bbox
            cw, ch = (x1 - x0) / k, (y1 - y0) / k
            cells = defaultdict(int)
            for x, y in pts:
                ci = min(k - 1, int((x + SMALL / 2 - x0) / cw)) if cw > 0 else 0
                cj = min(k - 1, int((y + SMALL / 2 - y0) / ch)) if ch > 0 else 0
                cells[(ci, cj)] += 1
            out = defaultdict(float)
            for (ci, cj), count in cells.items():
                cx0, cy0 = x0 + ci * cw, y0 + cj * ch
                cx1, cy1 = cx0 + cw, cy0 + ch
                covered = count * SMALL * SMALL
                carea = max(cw * ch, 1e-9)
                # spread the cell's value uniformly over the zones it overlaps
                for j in range(int(cy0 / zone), int(math.ceil(cy1 / zone))):
                    for i in range(int(cx0 / zone), int(math.ceil(cx1 / zone))):
                        ox = max(0.0, min(cx1, (i + 1) * zone) - max(cx0, i * zone))
                        oy = max(0.0, min(cy1, (j + 1) * zone) - max(cy0, j * zone))
                        if ox > 0 and oy > 0:
                            out[(i, j)] += covered * (ox * oy / carea)
            return out

        n_chunks = sum(len(r['chunks']) for r in records.values())
        print('corridor experiment: %d placements of a %g um cell -> %d placement record(s) (kinds %s), %d BVH node(s), %d chunks; '
              'fit %.3f px/um, cut %.2f um, zones %d px = %.2f um (%d zones, %d empty, %d in the corridors)'
              % (len(places), SMALL, len(records), kinds, len(nodes), n_chunks, px, cut_um, ZONE_PX, zone,
                 len(all_zones), len(empty), len(corridor)))
        # the subdivisions: the record's extent (the node), and its stored chunks
        units = {}
        for r in records.values():
            x0, y0, x1, y1 = nodes[r['node']]['bbox']
            units.setdefault('node', []).append(((x0, y0, x1, y1), r['pts']))
            for (cb, count) in r['chunks']:
                pass
            # members of each chunk: consecutive slots of 64
            for k, (cb, count) in enumerate(r['chunks']):
                units.setdefault('chunk', []).append((cb, r['pts'][k * 64:k * 64 + count]))
            # the same chunks after a Morton sort of the members
            def morton(pt):
                xi, yi = int(pt[0] / DIE * 65535), int(pt[1] / DIE * 65535)
                key = 0
                for b in range(16):
                    key |= ((xi >> b) & 1) << (2 * b) | ((yi >> b) & 1) << (2 * b + 1)
                return key
            sorted_pts = sorted(r['pts'], key=morton)
            for k in range(0, len(sorted_pts), 64):
                part = sorted_pts[k:k + 64]
                cb = (min(x for x, _ in part), min(y for _, y in part), max(x for x, _ in part) + SMALL, max(y for _, y in part) + SMALL)
                units.setdefault('morton', []).append((cb, part))
        print('%-6s %-4s %8s %14s %14s %10s %10s' % ('unit', 'k', 'boxes', 'false presence', 'corridor fill', 'MAE', 'bytes'))
        rows = []
        for unit, ks in (('node', (1, 4, 16, 64)), ('chunk', (1, 4)), ('morton', (1, 4))):
            boxes = units.get(unit, [])
            for k in ks:
                pred = defaultdict(float)
                for bbox, pts in boxes:
                    for z, a in spread(bbox, pts, k).items():
                        pred[z] += a
                false_presence = sum(1 for z in empty if pred.get(z, 0.0) > 0.0) / max(1, len(empty))
                corridor_fill = sum(1 for z in corridor if pred.get(z, 0.0) > 0.0) / max(1, len(corridor))
                mae = sum(abs(pred.get(z, 0.0) - truth.get(z, 0.0)) for z in all_zones) / len(all_zones) / zone_area
                nbytes = len(boxes) * (4 if k == 1 else k * k)
                rows.append((unit, k, len(boxes), false_presence, corridor_fill, mae, nbytes))
                print('%-6s %-4d %8d %13.1f%% %13.1f%% %10.4f %10d' % (unit, k, len(boxes), 100 * false_presence, 100 * corridor_fill, mae, nbytes))
        print('== type this == (unit k boxes false-presence%% corridor-fill%% mae bytes)')
        for unit, k, n, fp, cf, mae, nbytes in rows:
            print('%s %d %d %.1f %.1f %.4f %d' % (unit, k, n, 100 * fp, 100 * cf, mae, nbytes))


if __name__ == '__main__':
    main()
