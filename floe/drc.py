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

Multi-hundred-GB databases go through the `.ice` INDEX SIDECAR built
by `floe-index drc results.db` instead: the .db stays the source of
truth, the sidecar holds a 16-byte record per violation (offset/len/
kind into the .db) plus the check directory and a line-deduped
string table. load_db() picks the sidecar automatically when it is
present and fresh (source size+mtime match); IceDb then mmaps both
files and decodes ONE record per access with the exact tolerant
line-parse above, so a sidecar read equals the full ASCII parse
(tools/validate_drc_ice.py locks the equivalence).
"""

import bisect
import os
import struct
import sys


class DrcError(object):
    """One violation: kind 'p' (polygon) or 'e' (edge), pts in um."""
    __slots__ = ("kind", "num", "pts")

    def __init__(self, kind, num, pts):
        self.kind = kind
        self.num = num          # ordinal within its check (1-based)
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
    """Open a DRC results database: .ice sidecar if given/available
    (mmap, lazy per-record decode), full ASCII parse otherwise.

    - path is an .ice file (by magic): its source .db must sit next
      to it (same path minus ".ice") and match size/mtime.
    - path is an ASCII .db with a fresh <path>.ice next to it: the
      sidecar is used automatically; a stale sidecar falls back to
      the full ASCII parse with a stderr note.
    """
    with open(path, "rb") as f:
        head = f.read(12)
    if head[:8] == _ICE_MAGIC:
        version = int.from_bytes(head[8:12], "little")
        if version == 2:
            return IcePack(path)
        return IceDb(path)
    side = path + ".ice"
    if os.path.exists(side):
        try:
            with open(side, "rb") as f:
                shead = f.read(12)
            if shead[:8] == _ICE_MAGIC and \
                    int.from_bytes(shead[8:12], "little") == 2:
                return IcePack(side, src_path=path, verify_src=True)
            return IceDb(side, src_path=path)
        except (ValueError, OSError) as exc:
            sys.stderr.write("[drc] %s; parsing ASCII instead "
                             "(rerun: floe-index drc %s)\n"
                             % (exc, path))
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
                check.errors.append(DrcError(kind, num, pts))
            # unknown kinds: coordinates consumed, record dropped
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


class _IceErrors(object):
    """Lazy per-check error sequence over the sidecar index.

    Supports len()/indexing/slicing/iteration; every access decodes
    exactly one record slice out of the mmapped ASCII .db."""
    __slots__ = ("_db", "_start", "_count")

    def __init__(self, db, start, count):
        self._db = db
        self._start = start
        self._count = count

    def __len__(self):
        return self._count

    def __getitem__(self, i):
        if isinstance(i, slice):
            return [self._db._decode(self._start + j)
                    for j in range(*i.indices(self._count))]
        if i < 0:
            i += self._count
        if not 0 <= i < self._count:
            raise IndexError(i)
        return self._db._decode(self._start + i)


class IceCheck(object):
    """DrcCheck twin backed by the sidecar (same attributes)."""
    __slots__ = ("name", "desc", "declared", "errors")

    def __init__(self, name, desc, declared, errors):
        self.name = name
        self.desc = desc
        self.declared = declared
        self.errors = errors


class IceDb(object):
    """mmap reader for `<db>.ice` + its source ASCII .db.

    Check names/descriptions/counts come from the sidecar alone; a
    violation's coordinates are parsed from its record slice in the
    .db on demand (identical tolerant parse to load_ascii)."""
    __slots__ = ("path", "src_path", "cell", "precision", "checks",
                 "total", "_idx", "_src")

    def __init__(self, path, src_path=None):
        import mmap
        import numpy as np
        with open(path, "rb") as f:
            head = f.read(_ICE_HEADER.size)
            if len(head) < _ICE_HEADER.size:
                raise ValueError("%s: truncated .ice header" % path)
            (magic, version, _flags, precision, src_size,
             src_mtime) = _ICE_HEADER.unpack(head)
            if magic != _ICE_MAGIC:
                raise ValueError("%s: not a floe DRC index" % path)
            if version != 1:
                raise ValueError("%s: .ice version %d (reader knows 1)"
                                 % (path, version))
            f.seek(0, os.SEEK_END)
            fsize = f.tell()
            if fsize < _ICE_HEADER.size + _ICE_FOOTER.size:
                raise ValueError("%s: truncated .ice" % path)
            f.seek(fsize - _ICE_FOOTER.size)
            (err_off, err_cnt, dir_off, check_cnt, descref_off,
             descref_cnt, str_off, str_len, cell_ref, _resv,
             fmagic) = _ICE_FOOTER.unpack(f.read(_ICE_FOOTER.size))
            if fmagic != _ICE_MAGIC:
                raise ValueError("%s: truncated .ice (bad footer)"
                                 % path)
            f.seek(dir_off)
            dirbuf = f.read(check_cnt * _ICE_CHECK.size)
            f.seek(descref_off)
            descbuf = f.read(descref_cnt * 4)
            f.seek(str_off)
            strbuf = f.read(str_len)

        if src_path is None:
            if not path.endswith(".ice"):
                raise ValueError("%s: cannot infer source .db path"
                                 % path)
            src_path = path[:-4]
        st = os.stat(src_path)  # missing source -> OSError to caller
        if st.st_size != src_size or int(st.st_mtime) != src_mtime:
            raise ValueError(
                "%s: stale index (source size/mtime changed)" % path)

        def s(ref):
            (n,) = struct.unpack_from("<I", strbuf, ref)
            return strbuf[ref + 4:ref + 4 + n].decode(
                "utf-8", errors="replace")

        self.path = path
        self.src_path = src_path
        self.cell = s(cell_ref)
        self.precision = precision
        self.total = err_cnt
        self._idx = np.memmap(
            path, mode="r", offset=err_off, shape=(err_cnt,),
            dtype=np.dtype([("off", "<u8"), ("len", "<u4"),
                            ("kind", "u1"), ("p1", "u1"),
                            ("p2", "<u2")]))
        if st.st_size:
            with open(src_path, "rb") as sf:
                self._src = mmap.mmap(sf.fileno(), 0,
                                      access=mmap.ACCESS_READ)
        else:
            self._src = b""
        self.checks = []
        drefs = struct.unpack("<%dI" % descref_cnt, descbuf)
        for ci in range(check_cnt):
            (name_ref, dstart, dcnt, _pad, estart, ecnt, declared,
             _original) = _ICE_CHECK.unpack_from(
                dirbuf, ci * _ICE_CHECK.size)
            desc = "\n".join(s(r) for r in drefs[dstart:dstart + dcnt])
            self.checks.append(IceCheck(
                s(name_ref), desc, declared,
                _IceErrors(self, estart, ecnt)))

    def _decode(self, gi):
        """One violation record slice -> DrcError (parse parity with
        load_ascii: same float() tokens, same pair packing)."""
        rec = self._idx[gi]
        off, ln = int(rec["off"]), int(rec["len"])
        buf = self._src[off:off + ln]
        lines = buf.splitlines()
        head = lines[0].split()
        kind = head[0].decode("ascii", errors="replace").lower()
        try:
            num = int(head[1])
        except (IndexError, ValueError):
            num = 0
        pts = []
        prec = self.precision
        for line in lines[1:]:
            t = line.split()
            if not t:
                continue
            try:
                nums = [float(x) for x in t]
            except ValueError:
                break
            if len(nums) < 2:
                break
            for j in range(0, len(nums) - 1, 2):
                pts.append((nums[j] / prec, nums[j + 1] / prec))
        return DrcError(kind, num, pts)


# ---- .ice v2 packed reader ----------------------------------------------

# footer: blob_off blob_len qbox_off qbox_len blk_off blk_cnt
#         dir_off check_cnt descref_off descref_cnt str_off str_len
#         err_total | u32 cell_ref | u32 reserved | magic
_ICE2_FOOTER = struct.Struct("<13QII8s")
# check dir: name_ref desc_start desc_cnt pad | err_start err_cnt
#            declared original block_start block_cnt
_ICE2_CHECK = struct.Struct("<IIII6Q")
_ICE2_BLOCK = 64


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
                 "_ecnt", "_cbb", "_cache", "_order")

    def __init__(self, path, src_path=None, verify_src=False):
        import mmap
        import numpy as np
        with open(path, "rb") as f:
            head = f.read(_ICE_HEADER.size)
            (magic, version, _flags, precision, src_size,
             src_mtime) = _ICE_HEADER.unpack(head)
            if magic != _ICE_MAGIC or version != 2:
                raise ValueError("%s: not a packed .ice v2" % path)
            f.seek(0, os.SEEK_END)
            fsize = f.tell()
            f.seek(fsize - _ICE2_FOOTER.size)
            (_blob_off, _blob_len, qbox_off, _qbox_len, blk_off,
             blk_cnt, dir_off, check_cnt, descref_off, descref_cnt,
             str_off, str_len, err_total, cell_ref, _resv,
             fmagic) = \
                _ICE2_FOOTER.unpack(f.read(_ICE2_FOOTER.size))
            if fmagic != _ICE_MAGIC:
                raise ValueError("%s: truncated .ice (bad footer)"
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
        self.checks = []
        drefs = struct.unpack("<%dI" % descref_cnt, descbuf)
        es, bs, ec = [], [], []
        cbb = np.zeros((check_cnt, 4), dtype=np.int64)
        for ci in range(check_cnt):
            (name_ref, dstart, dcnt, _pad, estart, ecnt, declared,
             _orig, bstart, bcnt) = _ICE2_CHECK.unpack_from(
                dirbuf, ci * _ICE2_CHECK.size)
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
        self._dir_es = np.array(es, dtype=np.int64)
        self._dir_bs = np.array(bs, dtype=np.int64)
        self._ecnt = np.array(ec, dtype=np.int64)
        self._cbb = cbb
        self._cache = {}       # block idx -> [DrcError]; tiny LRU
        self._order = []

    def _block(self, bi):
        got = self._cache.get(bi)
        if got is not None:
            return got
        rec = self._blk[bi]
        pos, cnt = int(rec["off"]), int(rec["cnt"])
        buf = self._map
        prec = self.precision
        errs = []
        prev_num = pfx = pfy = 0
        for _ in range(cnt):
            knpts, pos = _uv(buf, pos)
            kind = "e" if knpts & 1 else "p"
            npts = knpts >> 1
            d, pos = _uv(buf, pos)
            num = prev_num + _unzz(d)
            prev_num = num
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

    def query_rect(self, x0_um, y0_um, x1_um, y1_um, cap=2000):
        """Errors intersecting the um rect -> [(ci, ei, DrcError)].

        Two numpy stages before any varint decode: check bboxes
        prune whole rules, then the per-error [qbox] lattice (an
        outward-rounded superset) prunes down to candidate RECORDS;
        only blocks holding candidates are decoded, and the exact
        bbox test on the decoded error settles it. Stops at cap."""
        import math as _math
        import numpy as np
        prec = self.precision
        qx0, qy0 = x0_um * prec, y0_um * prec
        qx1, qy1 = x1_um * prec, y1_um * prec
        cbb = self._cbb
        hitc = np.nonzero((cbb[:, 0] <= qx1) & (cbb[:, 2] >= qx0) &
                          (cbb[:, 1] <= qy1) & (cbb[:, 3] >= qy0) &
                          (self._ecnt > 0))[0]
        out = []
        for ci in hitc:
            ci = int(ci)
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
            sl = self._qbox[es:es + n]
            cand = np.nonzero((sl[:, 0] <= qhx) & (sl[:, 2] >= qlx) &
                              (sl[:, 1] <= qhy) & (sl[:, 3] >= qly))[0]
            if not len(cand):
                continue
            bs = int(self._dir_bs[ci])
            for brel in np.unique(cand // _ICE2_BLOCK):
                base = int(brel) * _ICE2_BLOCK
                for j, e in enumerate(self._block(bs + int(brel))):
                    bb = e.bbox()
                    if bb[0] <= x1_um and bb[2] >= x0_um \
                            and bb[1] <= y1_um and bb[3] >= y0_um:
                        out.append((ci, base + j, e))
                        if len(out) >= cap:
                            return out
        return out
