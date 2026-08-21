"""Calibre DRC ASCII results database (.db) parser + .ice sidecar.

The format Calibre writes with `DRC RESULTS DATABASE "out.db" ASCII`:

    <top-cell> <precision>
    <rulecheck name>
    <n-results> <n-original> <n-desc-lines> <timestamp ...>
    <rule text line 1..n-desc-lines>
    p <ordinal> <n-vertices>
    <x> <y>                     (integers; um = value / precision)
    ...
    e <ordinal> <n-edges>
    <x1> <y1> <x2> <y2>         (edge records: one whole edge per line)
    <next rulecheck name>
    ...

DRC results contain only 'p' (polygon) and 'e' (edge) primitives:
'p' carries one vertex (2 ints) per line, 'e' one edge (4 ints) per
line — klayout's Calibre reader confirms this split. The parser is
deliberately tolerant of real-world variation: blank lines and CRLF
are ignored, declared counts are advisory, unknown record letters
are skipped with their block, and a truncated file yields whatever
parsed cleanly instead of raising.

Multi-hundred-GB databases go through the self-contained `.ice`
PACK (v2, layout 4) built by `floe-index drc results.db`: varint
delta coordinate blocks in file order, a qbox lattice + block bbox
table for spatial queries, a per-error [status] byte + per-check
[wcount] for waive review. load_db() picks a fresh pack
automatically; pack contents equal the full ASCII parse
(tools/validate_drc_ice.py locks the equivalence). The v1 OFFSET
sidecar was retired 2026-08-19 (no status storage, no spatial
index, .db required alongside) - v1 files get a stderr note and
the ASCII fallback until re-packed.
"""

import bisect
import math
import os
import struct
import sys


EDGE_RULER_OFFSET_PX = 14


def offset_screen_segment(a, b, distance=EDGE_RULER_OFFSET_PX):
    """Move a screen-space segment along a stable normal.

    Prefer the upper side, with the right side as the tie-break for a
    vertical segment.  The rule is independent of endpoint order so the
    viewer and rendered snapshot place a one-edge CD ruler identically.
    """
    dx, dy = b[0] - a[0], b[1] - a[1]
    length = math.hypot(dx, dy)
    if length <= 0:
        return a, b
    nx, ny = -dy / length, dx / length
    if ny > 1e-12 or (abs(ny) <= 1e-12 and nx < 0):
        nx, ny = -nx, -ny
    ox, oy = nx * distance, ny * distance
    return ((a[0] + ox, a[1] + oy),
            (b[0] + ox, b[1] + oy))


class DrcError(object):
    """One violation: kind 'p' (polygon) or 'e' (edge), pts in um."""
    __slots__ = ("kind", "num", "pts")

    def __init__(self, kind, num, pts):
        self.kind = kind
        # GLOBAL 1-based file-order sequence across the whole db
        # (Calibre RVE numbering, user call 2026-08-13); the per-
        # record ordinal token in the ASCII file is ignored
        self.num = num
        self.pts = pts          # [(x_um, y_um), ...]

    def bbox(self):
        xs = [p[0] for p in self.pts]
        ys = [p[1] for p in self.pts]
        return (min(xs), min(ys), max(xs), max(ys))

    def center(self):
        b = self.bbox()
        return ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0)


class DrcCheck(object):
    """One rule check: name, rule text, and its violations."""
    __slots__ = ("name", "desc", "declared", "errors")

    def __init__(self, name, desc="", declared=0):
        self.name = name
        self.desc = desc        # rule text ('' if none)
        self.declared = declared
        self.errors = []


class DrcDb(object):
    __slots__ = ("path", "cell", "precision", "checks")

    def __init__(self, path, cell, precision, checks):
        self.path = path
        self.cell = cell
        self.precision = precision
        self.checks = checks

    @property
    def total(self):
        return sum(len(c.errors) for c in self.checks)


def cd_segments(e):
    """CD ruler segments for SIMPLE violations, as um 4-tuples -
    shared by the viewer's auto rulers and the CLI snapshot embeds
    (keep them identical):

    * single edge = its length;
    * facing edge pair = the closest gap, anchored at the midpoint
      when the edges face in parallel (the common spacing shape);
    * parallel edge pair with diagonal closest endpoints = the primary
      diagonal ruler followed by horizontal and vertical components;
    * axis-aligned rectangle = both spans through the middle.

    Complex polygons and edge sets get none.
    """
    pts = e.pts   # um

    def dist(p, q):
        return math.hypot(p[0] - q[0], p[1] - q[1])

    def foot(p, a, b):
        ax, ay = a
        dx, dy = b[0] - ax, b[1] - ay
        l2 = dx * dx + dy * dy
        if l2 <= 0:
            return a
        t = ((p[0] - ax) * dx + (p[1] - ay) * dy) / l2
        t = max(0.0, min(1.0, t))
        return (ax + t * dx, ay + t * dy)

    def seg(p, q):
        return (p[0], p[1], q[0], q[1])

    if e.kind == "e" and len(pts) == 2:
        if dist(pts[0], pts[1]) <= 0:
            return []
        return [seg(pts[0], pts[1])]
    if e.kind == "e" and len(pts) == 4:
        a0, a1, b0, b1 = pts

        def orient(a, b, c):
            return ((b[0] - a[0]) * (c[1] - a[1])
                    - (b[1] - a[1]) * (c[0] - a[0]))
        if (orient(a0, a1, b0) > 0) != (orient(a0, a1, b1) > 0) \
                and (orient(b0, b1, a0) > 0) != (orient(b0, b1,
                                                        a1) > 0):
            return []   # edges properly cross: gap is zero
        cand = [(p, foot(p, b0, b1)) for p in (a0, a1)]
        cand += [(foot(p, a0, a1), p) for p in (b0, b1)]
        dmin = min(dist(p, q) for p, q in cand)
        if dmin <= 0:
            return []   # touching edges: no gap
        mid = ((a0[0] + a1[0]) / 2.0, (a0[1] + a1[1]) / 2.0)
        fm = foot(mid, b0, b1)
        if dist(mid, fm) <= dmin * 1.0001:
            return [seg(mid, fm)]
        p, q = min(cand, key=lambda c: dist(c[0], c[1]))
        out = [seg(p, q)]

        # Disjoint projections of parallel edges produce a diagonal
        # endpoint-to-endpoint minimum.  Keep that true minimum first,
        # then expose its X/Y components as an L between the same points.
        # The sorted endpoints make the elbow independent of record order.
        adx, ady = a1[0] - a0[0], a1[1] - a0[1]
        bdx, bdy = b1[0] - b0[0], b1[1] - b0[1]
        al2, bl2 = adx * adx + ady * ady, bdx * bdx + bdy * bdy
        cross = adx * bdy - ady * bdx
        parallel = (al2 > 0 and bl2 > 0
                    and abs(cross) <= 1e-12 * math.sqrt(al2 * bl2))
        if parallel and p[0] != q[0] and p[1] != q[1]:
            lo, hi = sorted((p, q))
            elbow = (hi[0], lo[1])
            out.append(seg(lo, elbow))       # horizontal component
            out.append(seg(elbow, hi))       # vertical component
        return out
    if e.kind == "p" and len(pts) == 4:
        xs = sorted(set(x for x, _ in pts))
        ys = sorted(set(y for _, y in pts))
        if len(xs) != 2 or len(ys) != 2:
            return []   # not an axis-aligned rectangle
        if set(pts) != {(x, y) for x in xs for y in ys}:
            return []
        w, h = xs[1] - xs[0], ys[1] - ys[0]
        out = []
        if w > 0:   # width span through the middle
            my = (ys[0] + ys[1]) / 2.0
            out.append(seg((xs[0], my), (xs[1], my)))
        if h > 0:   # height span through the middle
            mx = (xs[0] + xs[1]) / 2.0
            out.append(seg((mx, ys[0]), (mx, ys[1])))
        return out
    return []


def _is_rve_name(name):
    """RVE-internal dunder bookkeeping section (__RVE_ERROR_TAG2__
    etc.) - tag data, never violations (field 2026-08-20)."""
    return name.startswith("__RVE_") and name.endswith("__")


def _ints_prefix(tokens):
    """Leading integers of a token list (check header: ints, then a
    textual timestamp)."""
    out = []
    for t in tokens:
        try:
            out.append(int(t))
        except ValueError:
            break
    return out


def _is_geom_header(tokens):
    return (len(tokens) >= 3 and len(tokens[0]) == 1
            and tokens[0].isalpha()
            and tokens[1].lstrip("-").isdigit()
            and tokens[2].lstrip("-").isdigit())


def load_db(path):
    """Open a DRC results database: packed .ice (v2) if given or
    fresh next to the .db, full ASCII parse otherwise.

    The v1 offset sidecar is retired (user call 2026-08-19 - no
    review-status storage, no spatial index, and it kept the huge
    .db as a required companion): a v1 file gets a stderr note and
    the ASCII fallback; `floe-index drc` re-packs it.
    """
    with open(path, "rb") as f:
        head = f.read(12)
    if head[:8] == _ICE_MAGIC:
        version = int.from_bytes(head[8:12], "little")
        if version >= 2:
            return IcePack(path)
        raise ValueError(
            "%s is a retired v1 offset sidecar - re-run: "
            "floe-index drc <db>" % path)
    side = path + ".ice"
    if os.path.exists(side):
        try:
            with open(side, "rb") as f:
                shead = f.read(12)
            if shead[:8] == _ICE_MAGIC and \
                    int.from_bytes(shead[8:12], "little") >= 2:
                return IcePack(side, src_path=path, verify_src=True)
            raise ValueError("v1 offset sidecar is retired")
        except (ValueError, OSError) as exc:
            sys.stderr.write("[drc] %s: %s; parsing ASCII instead "
                             "(rerun: floe-index drc %s)\n"
                             % (side, exc, path))
    return load_ascii(path)


def load_ascii(path):
    """Parse a Calibre ASCII DRC results database -> DrcDb."""
    with open(path, "r", errors="replace") as f:
        lines = [ln.rstrip("\r\n") for ln in f]
    # skip blank leading lines; header = "<cell> <precision>"
    i = 0
    while i < len(lines) and not lines[i].strip():
        i += 1
    if i >= len(lines):
        raise ValueError("%s: empty file" % path)
    head = lines[i].split()
    i += 1
    cell = head[0] if head else os.path.basename(path)
    try:
        precision = float(head[1])
    except (IndexError, ValueError):
        precision = 1000.0
    if precision <= 0:
        precision = 1000.0

    checks = []
    gnum = 0                    # global file-order error number
    n = len(lines)
    while i < n:
        name = lines[i].strip()
        i += 1
        if not name:
            continue
        check = DrcCheck(name)
        # header: "<results> <original> <desc-lines> <date...>"
        if i < n:
            ints = _ints_prefix(lines[i].split())
            if ints:
                i += 1
                check.declared = ints[0]
                dlines = ints[2] if len(ints) >= 3 else 0
                desc = []
                for _ in range(dlines):
                    if i < n and not _is_geom_header(lines[i].split()):
                        desc.append(lines[i].strip())
                        i += 1
                check.desc = "\n".join(desc)
        # geometry records until the next check name
        while i < n:
            tok = lines[i].split()
            if not tok:
                i += 1
                continue
            if not _is_geom_header(tok):
                break  # next rulecheck name
            kind, num, nv = tok[0].lower(), int(tok[1]), int(tok[2])
            i += 1
            # nv coordinate LINES follow; every integer pair on a line
            # is one point ('p': 2 ints = 1 vertex, 'e': 4 ints = the
            # edge's two endpoints)
            pts = []
            got = 0
            while got < nv and i < n:
                ct = lines[i].split()
                if not ct:
                    i += 1
                    continue  # stray blank inside a record
                try:
                    nums = [float(t) for t in ct]
                except ValueError:
                    break  # next check name: record truncated here
                if len(nums) < 2:
                    break
                i += 1
                got += 1
                for j in range(0, len(nums) - 1, 2):
                    pts.append((nums[j] / precision,
                                nums[j + 1] / precision))
            if pts and kind in ("p", "e"):
                gnum += 1
                check.errors.append(DrcError(kind, gnum, pts))
            # unknown kinds: coordinates consumed, record dropped
        # RVE-internal bookkeeping sections (__RVE_ERROR_TAG2__
        # etc., dunder-named) carry tag data, not violations
        # (field report 2026-08-20): ALWAYS dropped - records too,
        # and those never consume global file-order numbers, so
        # the numbering matches what RVE displays
        if _is_rve_name(check.name):
            gnum -= len(check.errors)
            continue
        # administrative tail sections (DENSITY_RDBS,
        # NET_AREA_RATIO_RDBS, DFM_RDBS, LAYOUT_INPUT_EXCEPTION_RDBS)
        # list rdb files, not violations: drop them - but only when
        # empty, so a real check that happens to end in _RDBS can
        # never lose its errors (the rust indexer mirrors this)
        if check.name.endswith("_RDBS") and not check.errors:
            continue
        checks.append(check)
    return DrcDb(path, cell, precision, checks)


# ---- .ice index sidecar reader -----------------------------------------

_ICE_MAGIC = b"FLOEICE\x00"
# header: magic | u32 version | u32 flags | f64 precision
#         | u64 src_size | u64 src_mtime
_ICE_HEADER = struct.Struct("<8sIIdQQ")
# footer: err_off err_cnt dir_off check_cnt descref_off descref_cnt
#         str_off str_len | u32 cell_ref | u32 reserved | magic
_ICE_FOOTER = struct.Struct("<8QII8s")
# check dir: name_ref desc_start desc_cnt pad | err_start err_cnt
#            declared original
_ICE_CHECK = struct.Struct("<IIII4Q")


class IceCheck(object):
    """DrcCheck twin backed by the sidecar (same attributes)."""
    __slots__ = ("name", "desc", "declared", "errors")

    def __init__(self, name, desc, declared, errors):
        self.name = name
        self.desc = desc
        self.declared = declared
        self.errors = errors


# ---- .ice v2 packed reader ----------------------------------------------

# footer: blob_off blob_len qbox_off qbox_len status_off
#         wcount_off blk_off blk_cnt dir_off check_cnt descref_off
#         descref_cnt str_off str_len err_total | u32 cell_ref
#         | u32 reserved | magic
_ICE2_FOOTER = struct.Struct("<15QII8s")

# per-error review status byte ([status] section; app-defined >2)
STATUS_NONE = 0
STATUS_WAIVED = 1
STATUS_RESERVED = 2
# check dir: name_ref desc_start desc_cnt pad | err_start err_cnt
#            declared original block_start block_cnt
_ICE2_CHECK = struct.Struct("<IIII6Q")
_ICE2_BLOCK = 64
# status scan granularity: rank/page jumps read cached per-chunk
# waived counts and touch at most ONE chunk of status bytes
_STATUS_CHUNK = 1 << 22


def _uv(buf, pos):
    val = 0
    shift = 0
    while True:
        b = buf[pos]
        pos += 1
        val |= (b & 0x7F) << shift
        if not b & 0x80:
            return val, pos
        shift += 7


def _unzz(u):
    return (u >> 1) ^ -(u & 1)


class _PackErrors(object):
    """Lazy per-check error sequence over decoded blocks."""
    __slots__ = ("_db", "_start", "_count")

    def __init__(self, db, start, count):
        self._db = db
        self._start = start
        self._count = count

    def __len__(self):
        return self._count

    def __getitem__(self, i):
        if isinstance(i, slice):
            return [self._db._perr(self._start + j)
                    for j in range(*i.indices(self._count))]
        if i < 0:
            i += self._count
        if not 0 <= i < self._count:
            raise IndexError(i)
        return self._db._perr(self._start + i)


class IcePack(object):
    """mmap reader for a PACKED .ice v2 (self-contained: rule table
    + delta/varint coordinate blocks + per-block bbox table that
    doubles as the spatial index; the source .db is not needed)."""
    __slots__ = ("path", "cell", "precision", "checks", "total",
                 "_map", "_blk", "_qbox", "_dir_es", "_dir_bs",
                 "_ecnt", "_cbb", "_cache", "_order",
                 "_status", "_status_off", "_wfd",
                 "_wcount", "_wcount_off", "_wchunk")

    def __init__(self, path, src_path=None, verify_src=False):
        self._wfd = None
        self._map = None
        try:
            self._load(path, src_path, verify_src)
        except (struct.error, IndexError, OverflowError) as exc:
            # every parse mishap on a damaged file normalizes to
            # ONE catchable story: corrupt pack -> rebuild
            self.close()
            raise ValueError(
                "%s: corrupt packed .ice (%s) - re-run: "
                "floe-index drc <db>" % (path, exc))
        except Exception:
            self.close()
            raise

    def close(self):
        """Release the pwrite fd and the coordinate mmap
        (idempotent; the object is unusable afterwards). The
        np.memmap section views hold their own mappings and are
        freed by GC."""
        wfd = getattr(self, "_wfd", None)
        self._wfd = None
        if wfd is not None:
            try:
                os.close(wfd)
            except OSError:
                pass
        m = getattr(self, "_map", None)
        self._map = None
        if m is not None:
            try:
                m.close()
            except (OSError, ValueError):
                pass

    def __del__(self):
        try:
            self.close()
        except Exception:
            pass

    def _load(self, path, src_path, verify_src):
        import mmap
        import numpy as np
        with open(path, "rb") as f:
            head = f.read(_ICE_HEADER.size)
            if len(head) != _ICE_HEADER.size:
                raise ValueError(
                    "%s: truncated .ice - re-run: floe-index drc "
                    "<db>" % path)
            (magic, version, _flags, precision, src_size,
             src_mtime) = _ICE_HEADER.unpack(head)
            if magic != _ICE_MAGIC or version < 2:
                raise ValueError("%s: not a packed .ice" % path)
            if version != 4:
                # the packed layout revved (file order, status,
                # wcount, footer size); a stale pack misaligns
                # sections and SILENTLY breaks queries - refuse it
                raise ValueError(
                    "%s: packed .ice layout %d is from an older "
                    "build - re-run: floe-index drc <db> --pack"
                    % (path, version))
            f.seek(0, os.SEEK_END)
            fsize = f.tell()
            if fsize < _ICE_HEADER.size + _ICE2_FOOTER.size:
                raise ValueError(
                    "%s: truncated .ice - re-run: floe-index drc "
                    "<db>" % path)
            f.seek(fsize - _ICE2_FOOTER.size)
            (_blob_off, _blob_len, qbox_off, _qbox_len,
             status_off, wcount_off, blk_off, blk_cnt, dir_off,
             check_cnt, descref_off, descref_cnt, str_off, str_len,
             err_total, cell_ref, _resv, fmagic) = \
                _ICE2_FOOTER.unpack(f.read(_ICE2_FOOTER.size))
            if fmagic != _ICE_MAGIC:
                raise ValueError("%s: truncated .ice (bad footer)"
                                 % path)
            # every section must land inside the file body, or the
            # reads below decode garbage / explode arbitrarily
            body_end = fsize - _ICE2_FOOTER.size
            for off, ln in ((qbox_off, err_total * 4),
                            (status_off, err_total),
                            (wcount_off, check_cnt * 4),
                            (blk_off, blk_cnt * 48),
                            (dir_off, check_cnt * _ICE2_CHECK.size),
                            (descref_off, descref_cnt * 4),
                            (str_off, str_len)):
                if off < _ICE_HEADER.size or off + ln > body_end:
                    raise ValueError(
                        "%s: corrupt packed .ice (section out of "
                        "range) - re-run: floe-index drc <db>"
                        % path)
            f.seek(dir_off)
            dirbuf = f.read(check_cnt * _ICE2_CHECK.size)
            f.seek(descref_off)
            descbuf = f.read(descref_cnt * 4)
            f.seek(str_off)
            strbuf = f.read(str_len)
        if verify_src and src_path is not None:
            st = os.stat(src_path)
            if st.st_size != src_size or int(st.st_mtime) != src_mtime:
                raise ValueError(
                    "%s: stale pack (source size/mtime changed)"
                    % path)

        def s(ref):
            (n,) = struct.unpack_from("<I", strbuf, ref)
            return strbuf[ref + 4:ref + 4 + n].decode(
                "utf-8", errors="replace")

        self.path = path
        self.cell = s(cell_ref)
        self.precision = precision
        self.total = err_total
        with open(path, "rb") as f:
            self._map = mmap.mmap(f.fileno(), 0,
                                  access=mmap.ACCESS_READ)
        self._blk = np.memmap(
            path, mode="r", offset=blk_off, shape=(blk_cnt,),
            dtype=np.dtype([("off", "<u8"), ("cnt", "<u4"),
                            ("pad", "<u4"), ("x0", "<i8"),
                            ("y0", "<i8"), ("x1", "<i8"),
                            ("y1", "<i8")]))
        self._qbox = (np.memmap(path, mode="r", offset=qbox_off,
                                shape=(err_total, 4), dtype=np.uint8)
                      if err_total else
                      np.zeros((0, 4), dtype=np.uint8))
        # review status bytes: shared read mapping stays coherent
        # with in-place pwrite updates (set_status)
        self._status = (np.memmap(path, mode="r", offset=status_off,
                                  shape=(err_total,), dtype=np.uint8)
                        if err_total else
                        np.zeros(0, dtype=np.uint8))
        self._status_off = status_off
        # per-rule waived counters: O(1) filter counts, kept in
        # sync by set_status (no [status] rescans)
        self._wcount = (np.memmap(path, mode="r",
                                  offset=wcount_off,
                                  shape=(check_cnt,),
                                  dtype="<u4")
                        if check_cnt else
                        np.zeros(0, dtype="<u4"))
        self._wcount_off = wcount_off
        self._wfd = None
        self.checks = []
        drefs = struct.unpack("<%dI" % descref_cnt, descbuf)
        es, bs, ec = [], [], []
        cbb = np.zeros((check_cnt, 4), dtype=np.int64)
        for ci in range(check_cnt):
            (name_ref, dstart, dcnt, _pad, estart, ecnt, declared,
             _orig, bstart, bcnt) = _ICE2_CHECK.unpack_from(
                dirbuf, ci * _ICE2_CHECK.size)
            if estart + ecnt > err_total or bstart + bcnt > blk_cnt \
                    or dstart + dcnt > descref_cnt:
                raise ValueError(
                    "%s: corrupt packed .ice (check %d out of "
                    "range) - re-run: floe-index drc <db>"
                    % (path, ci))
            desc = "\n".join(s(r) for r in drefs[dstart:dstart + dcnt])
            es.append(estart)
            bs.append(bstart)
            ec.append(ecnt)
            if bcnt:
                sl = self._blk[bstart:bstart + bcnt]
                cbb[ci] = (sl["x0"].min(), sl["y0"].min(),
                           sl["x1"].max(), sl["y1"].max())
            else:
                cbb[ci] = (1, 1, 0, 0)   # empty: never intersects
            self.checks.append(IceCheck(
                s(name_ref), desc, declared,
                _PackErrors(self, estart, ecnt)))
        # OLD packs (built before 0.11.40) still carry RVE-internal
        # sections baked in - the freshness check pins source
        # size/mtime, not indexer fixes, so they stay "fresh"
        # forever. TRAILING ones (the real-deck shape) drop safely
        # at open: every ci-indexed structure keeps its stored
        # prefix meaning and no global number shifts (nothing
        # follows them). A mid-file one is kept - dropping it would
        # shift the stored numbering - with a re-pack hint.
        kept = check_cnt
        while kept and _is_rve_name(self.checks[kept - 1].name):
            kept -= 1
        if kept < check_cnt:
            self.total -= sum(ec[kept:])
            del self.checks[kept:], es[kept:], bs[kept:], ec[kept:]
            cbb = cbb[:kept]
            self._wcount = self._wcount[:kept]
            sys.stderr.write(
                "[drc] %s: dropped %d RVE-internal tail section(s) "
                "baked into an old pack (re-run: floe-index drc "
                "<db> to slim it)\n" % (path, check_cnt - kept))
        if any(_is_rve_name(c.name) for c in self.checks):
            sys.stderr.write(
                "[drc] %s: mid-pack RVE-internal section kept "
                "(stored numbering) - re-run: floe-index drc <db>\n"
                % path)
        self._dir_es = np.array(es, dtype=np.int64)
        self._dir_bs = np.array(bs, dtype=np.int64)
        self._ecnt = np.array(ec, dtype=np.int64)
        self._cbb = cbb
        self._cache = {}       # block idx -> [DrcError]; tiny LRU
        self._order = []
        self._wchunk = {}      # ci -> per-chunk waived counts

    def _block(self, bi):
        got = self._cache.get(bi)
        if got is not None:
            return got
        rec = self._blk[bi]
        pos, cnt = int(rec["off"]), int(rec["cnt"])
        # global number base: blocks store no ordinal - storage is
        # file order, so the number is err_start + offset + 1
        ci = bisect.bisect_right(self._dir_bs, bi) - 1
        base = int(self._dir_es[ci]) \
            + (bi - int(self._dir_bs[ci])) * _ICE2_BLOCK
        buf = self._map
        prec = self.precision
        errs = []
        pfx = pfy = 0
        for j in range(cnt):
            knpts, pos = _uv(buf, pos)
            kind = "e" if knpts & 1 else "p"
            npts = knpts >> 1
            num = base + j + 1
            d, pos = _uv(buf, pos)
            x = pfx + _unzz(d)
            d, pos = _uv(buf, pos)
            y = pfy + _unzz(d)
            pfx, pfy = x, y
            pts = [(x / prec, y / prec)]
            for _ in range(npts - 1):
                d, pos = _uv(buf, pos)
                x += _unzz(d)
                d, pos = _uv(buf, pos)
                y += _unzz(d)
                pts.append((x / prec, y / prec))
            errs.append(DrcError(kind, num, pts))
        self._cache[bi] = errs
        self._order.append(bi)
        if len(self._order) > 16:
            self._cache.pop(self._order.pop(0), None)
        return errs

    def get_status(self, ci, ei):
        """Review status byte of check ci's error ei (0=none,
        1=waived, 2=reserved, >2 app-defined)."""
        return int(self._status[int(self._dir_es[ci]) + ei])

    def set_status(self, ci, ei, value):
        """Set the status byte IN PLACE (pwrite; the read mapping
        is coherent) and keep the rule's [wcount] waived counter in
        sync. Raises OSError if the .ice is not writable."""
        gid = int(self._dir_es[ci]) + ei
        if not 0 <= gid < self.total:
            raise IndexError((ci, ei))
        value = int(value) & 0xFF
        old = int(self._status[gid])
        if old == value:
            return
        if self._wfd is None:
            self._wfd = os.open(self.path, os.O_RDWR)
        os.pwrite(self._wfd, bytes((value,)),
                  self._status_off + gid)
        was = old == STATUS_WAIVED
        now = value == STATUS_WAIVED
        if was != now:
            cur = int(self._wcount[ci]) + (1 if now else -1)
            os.pwrite(self._wfd, struct.pack("<I", max(0, cur)),
                      self._wcount_off + 4 * ci)
            ch = self._wchunk.get(ci)
            if ch is not None:
                ch[ei // _STATUS_CHUNK] += 1 if now else -1

    def status_counts(self, ci):
        """(waived, total) of one rule - O(1) via [wcount]."""
        return (int(self._wcount[ci]), int(self._ecnt[ci]))

    def status_eis(self, ci, waived):
        """Rule-local error indices whose waived-ness matches.
        MATERIALIZES the full list - use status_page/status_rank
        for huge rules (the viewer does)."""
        import numpy as np
        es = int(self._dir_es[ci])
        n = int(self._ecnt[ci])
        if n == 0:
            return []
        sl = self._status[es:es + n]
        mask = ((sl == STATUS_WAIVED) if waived
                else (sl != STATUS_WAIVED))
        return np.nonzero(mask)[0].tolist()

    def _wf_chunks(self, ci):
        """Per-_STATUS_CHUNK WAIVED counts of one rule (lazy: built
        on the first filtered access with one status pass, then
        kept in sync incrementally by set_status). Rank and page
        jumps read these whole-chunk counts and scan at most ONE
        chunk of status bytes - n/p on a 100M-error rule used to
        re-sum status from index 0 (review 2026-08-18)."""
        got = self._wchunk.get(ci)
        if got is not None:
            return got
        import numpy as np
        es = int(self._dir_es[ci])
        n = int(self._ecnt[ci])
        nch = -(-n // _STATUS_CHUNK) if n else 0
        arr = np.zeros(nch, dtype=np.int64)
        for k in range(nch):
            a = k * _STATUS_CHUNK
            b = min(a + _STATUS_CHUNK, n)
            arr[k] = int(
                (self._status[es + a:es + b]
                 == STATUS_WAIVED).sum())
        self._wchunk[ci] = arr
        return arr

    def status_page(self, ci, waived, start, limit):
        """The start..start+limit-th MATCHING rule-local indices -
        whole chunks before `start` are skipped via the cached
        counts (no status reads), and a filter page over a
        100M-error rule never builds the full filtered list."""
        import numpy as np
        es = int(self._dir_es[ci])
        n = int(self._ecnt[ci])
        if n == 0 or limit <= 0:
            return []
        ch = self._wf_chunks(ci)
        out = []
        seen = 0
        for k in range(len(ch)):
            a = k * _STATUS_CHUNK
            b = min(a + _STATUS_CHUNK, n)
            cnt = int(ch[k]) if waived else (b - a) - int(ch[k])
            if seen + cnt <= start:
                seen += cnt
                continue
            sl = self._status[es + a:es + b]
            m = ((sl == STATUS_WAIVED) if waived
                 else (sl != STATUS_WAIVED))
            idx = np.nonzero(m)[0]
            lo = max(0, start - seen)
            for v in idx[lo:lo + (limit - len(out))]:
                out.append(int(v) + a)
            seen += cnt
            if len(out) >= limit:
                break
        return out

    def status_rank(self, ci, waived, ei):
        """Rank of ei among the rule's matching errors (None when
        ei itself does not match) - cached chunk counts + a partial
        scan of ei's own chunk only."""
        es = int(self._dir_es[ci])
        st = int(self._status[es + ei])
        match = ((st == STATUS_WAIVED) if waived
                 else (st != STATUS_WAIVED))
        if not match:
            return None
        ch = self._wf_chunks(ci)
        k = ei // _STATUS_CHUNK
        wbefore = int(ch[:k].sum())
        a = k * _STATUS_CHUNK
        w_in = int((self._status[es + a:es + ei]
                    == STATUS_WAIVED).sum())
        if waived:
            return wbefore + w_in
        return (a - wbefore) + (ei - a - w_in)

    def _perr(self, gid):
        """Global error id -> DrcError via its 256-record block."""
        ci = bisect.bisect_right(self._dir_es, gid) - 1
        # empty checks share err_start with their successor: walk
        # back to the check that actually owns this id
        while len(self.checks[ci].errors) == 0:
            ci -= 1
        rel = gid - int(self._dir_es[ci])
        bi = int(self._dir_bs[ci]) + rel // _ICE2_BLOCK
        return self._block(bi)[rel % _ICE2_BLOCK]

    def query_rect(self, x0_um, y0_um, x1_um, y1_um, cap=2000,
                   checks=None, waived=None):
        """Errors intersecting the um rect -> [(ci, ei, DrcError)].

        STREAMING with early exit (2026-08-14): per check the BLOCK
        table is scanned in chunks (48B/block - 1/64th of qbox);
        only blocks whose bbox intersects get their 64 qbox rows
        tested, and only blocks holding candidates are decoded. A
        wide view fills `cap` within the first blocks and never
        touches the rest of a huge rule - the previous full-rule
        qbox scan cost O(rule size) per call (seconds + a cold
        page-in of hundreds of MB on 100M-error rules).
        `checks` restricts the query to those rules; None = all.
        `waived` = None/True/False filters by review status INSIDE
        the query, before the cap - a caller-side post-filter over
        a capped result silently drops matches past the cap."""
        import math as _math
        import numpy as np
        prec = self.precision
        qx0, qy0 = x0_um * prec, y0_um * prec
        qx1, qy1 = x1_um * prec, y1_um * prec
        cbb = self._cbb
        mask = ((cbb[:, 0] <= qx1) & (cbb[:, 2] >= qx0) &
                (cbb[:, 1] <= qy1) & (cbb[:, 3] >= qy0) &
                (self._ecnt > 0))
        if checks is not None:
            only = np.zeros(len(self.checks), dtype=bool)
            for c in checks:
                if 0 <= int(c) < len(only):
                    only[int(c)] = True
            mask &= only
        hitc = np.nonzero(mask)[0]
        out = []
        for ci in hitc:
            ci = int(ci)
            if waived is True and int(self._wcount[ci]) == 0:
                continue   # O(1) skip: rule has no waived errors
            cx0, cy0, cx1, cy1 = (int(v) for v in cbb[ci])
            sx, sy = cx1 - cx0, cy1 - cy0

            def q(v, c0, span, up):
                if span <= 0:
                    return 255 if up else 0
                d = (v - c0) * 255.0 / span
                r = _math.ceil(d) if up else _math.floor(d)
                return max(0, min(255, r))

            qlx = q(qx0, cx0, sx, False)
            qhx = q(qx1, cx0, sx, True)
            qly = q(qy0, cy0, sy, False)
            qhy = q(qy1, cy0, sy, True)
            es = int(self._dir_es[ci])
            n = int(self._ecnt[ci])
            bs = int(self._dir_bs[ci])
            nblocks = -(-n // _ICE2_BLOCK)
            B = _ICE2_BLOCK
            step = 1 << 16          # blocks per vectorized chunk
            for c0_ in range(0, nblocks, step):
                c1_ = min(c0_ + step, nblocks)
                bsl = self._blk[bs + c0_:bs + c1_]
                bm = ((bsl["x0"] <= qx1) & (bsl["x1"] >= qx0) &
                      (bsl["y0"] <= qy1) & (bsl["y1"] >= qy0))
                bhits = np.nonzero(bm)[0]
                if not len(bhits):
                    continue
                if len(bhits) <= 64:
                    # sparse: touch only the hit blocks' qbox rows
                    for brel in bhits:
                        brel = int(brel) + c0_
                        base = brel * B
                        rcnt = min(B, n - base)
                        qs = self._qbox[es + base:es + base + rcnt]
                        qm = ((qs[:, 0] <= qhx) &
                              (qs[:, 2] >= qlx) &
                              (qs[:, 1] <= qhy) &
                              (qs[:, 3] >= qly))
                        if waived is not None:
                            ssl = self._status[es + base:
                                               es + base + rcnt]
                            qm &= ((ssl == STATUS_WAIVED) if waived
                                   else (ssl != STATUS_WAIVED))
                        if not qm.any():
                            continue
                        errs = self._block(bs + brel)
                        for j in np.nonzero(qm)[0]:
                            e = errs[int(j)]
                            bb = e.bbox()
                            if bb[0] <= x1_um and bb[2] >= x0_um \
                                    and bb[1] <= y1_um \
                                    and bb[3] >= y0_um:
                                out.append((ci, base + int(j), e))
                                if len(out) >= cap:
                                    return out
                    continue
                # dense (file-order block bboxes are wide): one
                # vectorized qbox pass over the chunk's row span
                # beats thousands of per-block python iterations
                r0 = c0_ * B
                rcnt = min(c1_ * B, n) - r0
                qs = self._qbox[es + r0:es + r0 + rcnt]
                qm = ((qs[:, 0] <= qhx) & (qs[:, 2] >= qlx) &
                      (qs[:, 1] <= qhy) & (qs[:, 3] >= qly))
                if waived is not None:
                    ssl = self._status[es + r0:es + r0 + rcnt]
                    qm &= ((ssl == STATUS_WAIVED) if waived
                           else (ssl != STATUS_WAIVED))
                errs = None
                cur = -1
                for r in np.nonzero(qm)[0]:
                    grel = r0 + int(r)
                    brel = grel // B
                    if brel != cur:
                        cur = brel
                        errs = self._block(bs + brel)
                    e = errs[grel - brel * B]
                    bb = e.bbox()
                    if bb[0] <= x1_um and bb[2] >= x0_um \
                            and bb[1] <= y1_um and bb[3] >= y0_um:
                        out.append((ci, grel, e))
                        if len(out) >= cap:
                            return out
        return out
