"""V3b coverage gate: design.ovc per-layer density vs klayout.

For each source (layer, datatype) with geometry, the finest coverage
level must be non-empty, and the coverage-implied covered area must
be within a loose band of klayout's merged area (coverage is
unmerged density on coarse texels, so it runs a bit high; the band
only catches gross bugs - empty planes, wrong scale, wrong layer).

usage: python tools/validate_vfs_coverage.py <src.oas> <floe_dir>
"""
import functools
import struct
import sys

import klayout.db as db

print = functools.partial(print, flush=True)

_HDR = "<8sId4qIIII"
LO, HI = 0.5, 4.0   # allowed coverage/merged area ratio band


def read_ovc(path):
    d = open(path, "rb").read()
    (magic, ver, dbu, x0, y0, x1, y1, rx, ry, nlv, nl) = \
        struct.unpack_from(_HDR, d, 0)
    assert magic == b"FLOEOVC1" and ver == 1
    o = struct.calcsize(_HDR)
    keys = []
    for _ in range(nl):
        keys.append(struct.unpack_from("<2I", d, o))
        o += 8
    ne = struct.unpack_from("<I", d, o)[0]
    o += 4
    body = struct.unpack_from("<Q", d, o)[0]
    o += 8
    planes = {}   # (layer,dt) -> level0 (w,h,bytes)
    for _ in range(ne):
        lv = d[o]
        o += 1
        li = struct.unpack_from("<I", d, o)[0]
        o += 4
        w, h = struct.unpack_from("<2H", d, o)
        o += 4
        off = struct.unpack_from("<Q", d, o)[0]
        o += 8
        ln = struct.unpack_from("<I", d, o)[0]
        o += 4
        if lv == 0:
            planes[keys[li]] = (w, h, d[body + off:body + off + ln])
    return dict(dbu=dbu, die=(x0, y0, x1, y1), rx=rx, ry=ry,
                planes=planes)


def main():
    src, floe = sys.argv[1], sys.argv[2]
    ovc = read_ovc(floe + "/design.ovc")
    die = ovc["die"]
    dbu = ovc["dbu"]
    texw = (die[2] - die[0]) / max(1, ovc["rx"])
    texh = (die[3] - die[1]) / max(1, ovc["ry"])
    cov = {}
    for key, (w, h, b) in ovc["planes"].items():
        cov[key] = (sum(b) / 255.0) * texw * texh * (dbu ** 2)
    ly = db.Layout(False)
    ly.read(src)
    top = ly.top_cell()
    bad = []
    checked = 0
    for li in ly.layer_indexes():
        info = ly.get_info(li)
        key = (info.layer, info.datatype)
        if key == (255, 0):
            continue
        reg = db.Region(ly.begin_shapes(top, li))
        reg.merge()
        m = reg.area() * (dbu ** 2)
        if m <= 0:
            continue
        checked += 1
        c = cov.get(key, 0.0)
        if c <= 0:
            bad.append("COV L%s/%s empty, merged=%.0f" % (*key, m))
        elif not (LO <= c / m <= HI):
            bad.append("COV L%s/%s ratio %.2f (cov=%.0f merged=%.0f)"
                       % (*key, c / m, c, m))
    ly._destroy()
    for b in bad:
        print("FAIL", b)
    print("coverage-checked %d layers, failures: %d" % (checked,
                                                        len(bad)))
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
