"""VFS render-path parity: the working set a vfsd probe builds for a
viewport must contain EXACTLY the source geometry inside that
viewport (cut=0, full depth). This exercises the whole render path -
plan spatial query, placement transforms, delta byte-splice - and
XORs the result against klayout's own view of the source.

For each test viewport and each (layer, datatype):
  region(VFS working set, flattened) ^ region(source) , both
  clipped to the view box, must be empty.

Pages are unclipped whole cells, so the working set may reach
outside the view; clipping both sides to the view box is what makes
the equality exact.

usage: python tools/validate_vfs_render.py <src.oas> <floe_dir>
       (needs rust/target/release/floe-index for the daemon)
"""
import functools
import json
import os
import sys

import klayout.db as db

sys.path.insert(0, os.path.dirname(os.path.dirname(
    os.path.abspath(__file__))))
from floe.vfsclient import VfsClient           # noqa: E402
from floe.viewport import VfsMosaic            # noqa: E402
from floe import cache as cm                   # noqa: E402

print = functools.partial(print, flush=True)
FRAME_LAYER = (255, 0)


def source_regions(src):
    ly = db.Layout(False)
    ly.read(src)
    top = ly.top_cell()
    bbox = top.bbox()
    regs = {}
    for li in ly.layer_indexes():
        info = ly.get_info(li)
        reg = db.Region(ly.begin_shapes(top, li))
        reg.merged_semantics = False
        regs[(info.layer, info.datatype)] = reg
    return regs, (bbox.left, bbox.bottom, bbox.right, bbox.top), ly


def vfs_regions(mosaic):
    ly = mosaic.ly
    top = mosaic.top
    regs = {}
    for li in ly.layer_indexes():
        info = ly.get_info(li)
        key = (info.layer, info.datatype)
        if key == FRAME_LAYER:
            continue
        reg = db.Region(ly.begin_shapes(top, li))
        reg.merged_semantics = False
        if not reg.is_empty():
            regs[key] = reg
    return regs


def main():
    src, floe_dir = sys.argv[1], sys.argv[2]
    # load meta straight from the given floe dir (it may not sit next
    # to the source, so cache_dir_for would miss it)
    cache = cm.Cache(src)
    cache.dir = floe_dir
    cache.meta = json.load(open(os.path.join(floe_dir, "meta.json")))
    dbu = cache.meta["dbu"]
    sregs, (bx0, by0, bx1, by1), sly = source_regions(src)

    # test viewports (dbu): whole, center quarter, each corner half
    w, h = bx1 - bx0, by1 - by0
    views = [
        (bx0, by0, bx1, by1),
        (bx0 + w // 4, by0 + h // 4, bx1 - w // 4, by1 - h // 4),
        (bx0, by0, bx0 + w // 2, by0 + h // 2),
        (bx1 - w // 2, by1 - h // 2, bx1, by1),
    ]

    client = VfsClient(floe_dir)
    bad = []
    try:
        for vi, (x0, y0, x1, y1) in enumerate(views):
            r = client.request(
                0, (x0 * dbu, y0 * dbu, x1 * dbu, y1 * dbu),
                1.0, 0.0, None, None, probe=True)
            mosaic = VfsMosaic(cache)
            mosaic.apply(r["delta"], r["mats"], [], r["frames"])
            vregs = vfs_regions(mosaic)
            clip = db.Region(db.Box(x0, y0, x1, y1))
            keys = set(sregs) | set(vregs)
            for key in sorted(keys):
                a = sregs.get(key, db.Region()) & clip
                b = vregs.get(key, db.Region()) & clip
                x = a ^ b
                if not x.is_empty():
                    bad.append((vi, key, x.count(), x.area()))
                    print("FAIL view %d L%s/%s: %d polys, area %d, "
                          "e.g. %s" % (vi, key[0], key[1], x.count(),
                                       x.area(), next(x.each()).bbox()))
            mosaic.ly._destroy()
    finally:
        client.stop()
    sly._destroy()
    print("vfs-render-checked %d views x %d layers, failures: %d"
          % (len(views), len(sregs), len(bad)))
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
