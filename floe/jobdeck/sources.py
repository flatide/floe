"""Source catalog: the dbu of every TC from its file header, nothing else.

The OASIS START record and the GDS UNITS record sit in the first bytes of
a file and carry the dbu. Reading them is a sub-millisecond pure-Python
operation independent of file size, so the deck grid and the whole-deck
extent are known before any geometry is touched, and a source outside
the selection is never opened beyond one page. gzip is inflated only as
far as that page.

Every source ends with a status; the report lists them all so a skipped
entry is never silent. floe2 draws from `<src>.floe` caches (M2+), so the
catalog also says whether that cache exists (`indexed`).
"""

from __future__ import annotations

import gzip
import io
import os
import struct
import time
from dataclasses import dataclass, asdict

STATUS_OK = "ok"
STATUS_MISSING = "missing"
STATUS_UNREADABLE = "unreadable"
STATUS_UNKNOWN = "unknown_format"

PROBE_OASIS_HEADER = "oasis-start-record"
PROBE_GDS_HEADER = "gds-units-record"
PROBE_NONE = "not-probed"

OASIS_MAGIC = b"%SEMI-OASIS\r\n"
GZIP_MAGIC = b"\x1f\x8b"
GDS_MAGIC = b"\x00\x06\x00\x02"       # HEADER record: length 6, type 0, i16
GDS_UNITS, GDS_ENDLIB, GDS_BGNSTR = 0x03, 0x04, 0x05
GDS_HEADER_CAP = 64 * 1024
PROBE_BYTES = 4096

FORMAT_OASIS = "oasis"
FORMAT_GDS = "gds"


def _oasis_uint(fh) -> int:
    v, shift = 0, 0
    while True:
        b = fh.read(1)
        if not b:
            raise ValueError("truncated OASIS header")
        v |= (b[0] & 0x7F) << shift
        shift += 7
        if not (b[0] & 0x80):
            return v


def _oasis_real(fh) -> float:
    t = _oasis_uint(fh)
    if t == 0:
        return float(_oasis_uint(fh))
    if t == 1:
        return -float(_oasis_uint(fh))
    if t == 2:
        return 1.0 / _oasis_uint(fh)
    if t == 3:
        return -1.0 / _oasis_uint(fh)
    if t in (4, 5):
        a = _oasis_uint(fh)
        b = _oasis_uint(fh)
        return (a / b) if t == 4 else -(a / b)
    if t == 6:
        return struct.unpack("<f", fh.read(4))[0]
    if t == 7:
        return struct.unpack("<d", fh.read(8))[0]
    raise ValueError("bad OASIS real type %d" % t)


def _oasis_start(fh):
    """(dbu_um, version) from a stream positioned after the magic."""
    if _oasis_uint(fh) != 1:
        raise ValueError("OASIS file does not begin with a START record")
    n = _oasis_uint(fh)
    version = fh.read(n).decode("ascii", "replace")
    unit = _oasis_real(fh)              # grid steps per micron
    if unit <= 0:
        raise ValueError("OASIS unit %r is not positive" % unit)
    return 1.0 / unit, version


def _gds_real(b: bytes) -> float:
    sign = -1.0 if b[0] & 0x80 else 1.0
    exp = (b[0] & 0x7F) - 64
    mant = int.from_bytes(b[1:8], "big")
    return sign * (mant / 2.0 ** 56) * (16.0 ** exp)


def _gds_units(fh):
    """(dbu_um, version) from a stream positioned after the HEADER magic."""
    version = str(struct.unpack(">h", fh.read(2))[0])
    walked = len(GDS_MAGIC) + 2
    while True:
        head = fh.read(4)
        if len(head) < 4:
            raise ValueError("GDS library header ends before UNITS")
        length, rtype = struct.unpack(">H", head[:2])[0], head[2]
        if length < 4:
            raise ValueError("GDS record type %#04x has length %d"
                             % (rtype, length))
        body = fh.read(length - 4)
        walked += length
        if rtype == GDS_UNITS:
            if len(body) != 16:
                raise ValueError("GDS UNITS record has %d bytes, not 16"
                                 % len(body))
            meters = _gds_real(body[8:16])
            if meters <= 0:
                raise ValueError("GDS database unit %r is not positive"
                                 % meters)
            return meters * 1e6, version
        if rtype in (GDS_BGNSTR, GDS_ENDLIB):
            raise ValueError("GDS library has no UNITS record before its "
                             "first structure")
        if walked > GDS_HEADER_CAP:
            raise ValueError("no GDS UNITS record within %d bytes"
                             % GDS_HEADER_CAP)


def file_header(path: str):
    """(format, dbu_um, version, gzipped) from the first bytes of a source,
    or (None, None, "", gzipped) when they match no known format. Raises
    ValueError on a file that starts like a known format but whose header
    is broken (UNREADABLE, not unknown)."""
    with open(path, "rb") as raw:
        head = raw.read(PROBE_BYTES)
        gz = head[:len(GZIP_MAGIC)] == GZIP_MAGIC
        if gz:
            with gzip.open(path, "rb") as fh:
                head = fh.read(PROBE_BYTES)
                more = fh.read(GDS_HEADER_CAP) if head[:4] == GDS_MAGIC \
                    else b""
        else:
            more = raw.read(GDS_HEADER_CAP) if head[:4] == GDS_MAGIC \
                else b""
    if head.startswith(OASIS_MAGIC):
        dbu, version = _oasis_start(io.BytesIO(head[len(OASIS_MAGIC):]))
        return FORMAT_OASIS, dbu, version, gz
    if head[:len(GDS_MAGIC)] == GDS_MAGIC:
        dbu, version = _gds_units(io.BytesIO(head[len(GDS_MAGIC):] + more))
        return FORMAT_GDS, dbu, version, gz
    return None, None, "", gz


@dataclass
class SourceInfo:
    tc: str
    path: str
    status: str
    dbu: float | None = None
    version: str = ""
    format: str = ""
    gzipped: bool = False
    bytes: int = 0
    probe: str = PROBE_NONE
    probe_s: float = 0.0
    error: str = ""
    indexed: bool = False      # <path>.floe exists and is not stale
    cache_dir: str = ""

    def ok(self) -> bool:
        return self.status == STATUS_OK and self.dbu is not None


def _cache_state(path: str):
    """(indexed, cache_dir) for a source, via the floe cache layout."""
    try:
        from ..cache import Cache
        c = Cache(path)
        if not c.exists():
            return False, c.dir
        c.load()
        return bool(c.meta.get("vfs") and not c.is_stale()), c.dir
    except Exception:
        return False, ""


class SourceCatalog:
    """Resolve and probe the source files a deck names."""

    def __init__(self, sources_dir: str = "."):
        self.dir = sources_dir
        self.infos: dict = {}

    def resolve(self, tc: str) -> str:
        return tc if os.path.isabs(tc) else os.path.join(self.dir, tc)

    def probe(self, tc: str) -> SourceInfo:
        if tc in self.infos:
            return self.infos[tc]
        path = self.resolve(tc)
        t0 = time.time()
        if not os.path.isfile(path):
            info = SourceInfo(tc, path, STATUS_MISSING, error="file not found")
        else:
            info = SourceInfo(tc, path, STATUS_OK, bytes=os.path.getsize(path))
            try:
                fmt, dbu, version, gz = file_header(path)
                info.format = fmt or ""
                info.gzipped = gz
                if fmt is not None:
                    info.dbu, info.version = dbu, version
                    info.probe = (PROBE_OASIS_HEADER if fmt == FORMAT_OASIS
                                  else PROBE_GDS_HEADER)
                else:
                    info.status = STATUS_UNKNOWN
                    info.error = "no OASIS or GDS header" + \
                        (" (gzip)" if gz else "")
            except Exception as e:
                info.status = STATUS_UNREADABLE
                info.error = str(e)
            info.indexed, info.cache_dir = _cache_state(path)
        info.probe_s = time.time() - t0
        self.infos[tc] = info
        return info

    def probe_all(self, tcs) -> dict:
        for tc in tcs:
            self.probe(tc)
        return self.infos

    def dbus(self) -> dict:
        return {tc: i.dbu for tc, i in self.infos.items() if i.ok()}

    def bad(self) -> dict:
        """{tc: (status, error, 'probe')} for every source without a dbu."""
        return {tc: (i.status, i.error, "probe")
                for tc, i in self.infos.items() if not i.ok()}

    def unindexed(self) -> list:
        """Probed-ok sources that have no usable <src>.floe cache yet."""
        return [tc for tc, i in sorted(self.infos.items())
                if i.ok() and not i.indexed]

    def report(self) -> dict:
        infos = [asdict(self.infos[tc]) for tc in sorted(self.infos)]
        return {
            "dir": self.dir,
            "probed": len(infos),
            "ok": sum(1 for i in infos if i["status"] == STATUS_OK),
            "indexed": sum(1 for i in infos if i["indexed"]),
            "probe_s": round(sum(i["probe_s"] for i in infos), 4),
            "note": "dbu comes from the OASIS START / GDS UNITS record in "
                    "the first bytes of each file; no geometry is read. "
                    "indexed = a fresh <src>.floe cache exists.",
            "files": infos,
        }
