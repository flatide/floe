#!/usr/bin/env python3
"""Synthetic MAIN01 generator gate (tools/gen_main01_like.py).

The generated file is the reference layout while the real chips are out of
reach (user, 2026-09-19), so what it promises is pinned:

  * `--geometry legacy` is the file of 2026-09-17..18 byte for byte (every
    measurement recorded before 2026-09-19 was made on it);
  * `--geometry chip` (the default) is deterministic - the same bytes for any
    --jobs - reads back in KLayout with the planned cells and 449 layers,
    indexes with floe-index, and has the shape of a chip: wires that are thin
    and long, several size classes in one cell, library cells far smaller
    than the die (under 1/500 of it even at this scale, where they are
    largest; 0.25-3 um at scale 1).

    .venv/bin/python tools/validate_gen_main01.py
"""
import hashlib
import os
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
LEGACY_SHA256 = '254141b600578da8afe27ea4721bf3a9d247b65336a8848661843384632f20f1'


def generate(out, *extra):
    done = subprocess.run([sys.executable, '-B', str(ROOT / 'tools/gen_main01_like.py'), str(out),
                           '--scale', '0.001', *extra], cwd=ROOT, capture_output=True, text=True, timeout=600)
    assert done.returncode == 0, done.stdout + done.stderr
    return hashlib.sha256(Path(out).read_bytes()).hexdigest()


def main():
    import klayout.db as db
    with tempfile.TemporaryDirectory(prefix='floe-gen-') as temp:
        temp = Path(temp)
        assert generate(temp / 'legacy.oas', '--geometry', 'legacy', '--jobs', '2') == LEGACY_SHA256, (
            'the legacy geometry is no longer the file of 2026-09-17')
        one = generate(temp / 'chip1.oas', '--jobs', '1')
        assert one == generate(temp / 'chip.oas', '--jobs', '3'), 'the chip geometry depends on --jobs'
        assert one != LEGACY_SHA256
        layout = db.Layout()
        layout.read(str(temp / 'chip.oas'))
        assert layout.cells() == 3521 and layout.layers() == 449 and layout.top_cell().name == 'TOP', (
            layout.cells(), layout.layers())
        # the shape of a chip, read off the top cell and the library cells
        longest, thin_long, classes = 0, 0, set()
        top = layout.top_cell()
        for li in layout.layer_indexes():
            for shape in top.shapes(li).each():
                box = shape.bbox()
                short, long = sorted((box.width(), box.height()))
                longest = max(longest, long)
                thin_long += long >= 50 * max(1, short)
                classes.add(len(str(max(1, long))))
        extent = max(top.bbox().width(), top.bbox().height())
        assert thin_long > 1000 and longest > extent // 4, (thin_long, longest, extent)
        assert len(classes) >= 4, 'one size class: %s' % sorted(classes)
        leaves = [c for c in layout.each_cell() if c.name.startswith('LF')]
        widest = max(max(c.bbox().width(), c.bbox().height()) for c in leaves)
        assert leaves and widest * 500 < extent, 'a library cell of %.1f um in a %.0f um die' % (
            widest * layout.dbu, extent * layout.dbu)
        index = subprocess.run([str(ROOT / 'rust/target/release/floe-index'), 'vfs', str(temp / 'chip.oas')],
                               capture_output=True, text=True, timeout=600)
        assert index.returncode == 0, index.stderr[-2000:]
        print('gen main01: legacy byte-identical, chip deterministic; top cell: %d thin long shapes, longest '
              '%.0f um of %.0f um, %d size decades; library cells <= %.1f um; indexed'
              % (thin_long, longest * layout.dbu, extent * layout.dbu, len(classes), widest * layout.dbu))
    print('GEN MAIN01: ALL OK')


if __name__ == '__main__':
    main()
