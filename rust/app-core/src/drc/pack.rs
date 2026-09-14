use super::*;
use crate::{check_cancelled, Result};
use std::{
    collections::{BTreeSet, VecDeque},
    fs::{self, File, Metadata, OpenOptions},
    os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{atomic::AtomicUsize, Arc},
};

#[derive(Clone, Debug)]
pub struct Check {
    pub name: String,
    pub desc: String,
    pub declared: u64,
    pub original: u64,
    pub start: u64,
    pub count: u64,
    pub bbox: Option<[i64; 4]>,
    block_start: u64,
    block_count: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    pub kind: char,
    /// Global file-order number, ONE based, never the source record ordinal.
    pub number: u64,
    pub points: Vec<[i64; 2]>,
    pub bbox: [i64; 4],
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordInfo<B = [i64; 4]> {
    pub kind: char,
    pub number: u64,
    pub bbox: B,
    pub points: usize,
}
impl From<&Violation> for RecordInfo {
    fn from(v: &Violation) -> Self {
        Self {
            kind: v.kind,
            number: v.number,
            bbox: v.bbox,
            points: v.points.len(),
        }
    }
}
#[derive(Clone, Debug)]
pub struct PointPage {
    pub record: RecordInfo,
    pub start: usize,
    pub points: Vec<[i64; 2]>,
    pub next: Option<usize>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cursor {
    pub check: usize,
    pub error: u64,
}
#[derive(Clone, Debug)]
pub struct Hit {
    pub check: usize,
    pub local: u64,
    pub status: u8,
    pub violation: Violation,
}
#[derive(Clone, Debug)]
pub struct Page {
    pub hits: Vec<Hit>,
    /// A resumable file-order cursor. None alone means the search is complete.
    pub next: Option<Cursor>,
    pub scanned: u64,
}
#[derive(Clone, Debug)]
pub struct InfoPage<B = [i64; 4]> {
    pub hits: Vec<InfoHit<B>>,
    pub next: Option<Cursor>,
    pub scanned: u64,
}
struct QueryPage<T> {
    hits: Vec<T>,
    next: Option<Cursor>,
    scanned: u64,
}
trait QueryHit: Sized {
    const COPY_POINTS: bool;
    fn project(check: usize, local: u64, status: u8, v: &Violation) -> Self;
}
impl QueryHit for Hit {
    const COPY_POINTS: bool = true;
    fn project(check: usize, local: u64, status: u8, v: &Violation) -> Self {
        Self {
            check,
            local,
            status,
            violation: v.clone(),
        }
    }
}
impl QueryHit for InfoHit {
    const COPY_POINTS: bool = false;
    fn project(check: usize, local: u64, status: u8, v: &Violation) -> Self {
        Self {
            check,
            local,
            status,
            record: RecordInfo::from(v),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StepCursor {
    pub next: u64,
    pub remaining: u64,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct StepRequest {
    pub check: usize,
    pub backwards: bool,
    pub after: Option<u64>,
    pub cursor: Option<StepCursor>,
    pub waived: Option<bool>,
    pub bbox_um: Option<[f64; 4]>,
}
#[derive(Clone, Copy, Debug)]
pub struct InfoHit<B = [i64; 4]> {
    pub check: usize,
    pub local: u64,
    pub status: u8,
    pub record: RecordInfo<B>,
}
#[derive(Clone, Debug)]
pub struct StepPage<B = [i64; 4]> {
    pub hit: Option<InfoHit<B>>,
    /// Continue with the SAME rule, direction and filters. Only no hit AND no
    /// continuation establishes that the complete circular search is empty.
    pub next: Option<StepCursor>,
    pub scanned: u64,
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct Stamp {
    len: u64,
    dev: u64,
    ino: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}
impl From<Metadata> for Stamp {
    fn from(m: Metadata) -> Self {
        Self {
            len: m.len(),
            dev: m.dev(),
            ino: m.ino(),
            modified: (m.mtime(), m.mtime_nsec()),
            changed: (m.ctime(), m.ctime_nsec()),
        }
    }
}
pub(super) struct Input {
    pub(super) file: File,
    stamp: Stamp,
}
impl Input {
    pub(super) fn open(path: &Path) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)?;
        Self::from_file(file)
    }
    pub(super) fn from_file(file: File) -> Result<Self> {
        let meta = file.metadata()?;
        if !meta.is_file() {
            return Err(crate::Error::input("DRC input must be a regular file"));
        }
        Ok(Self {
            file,
            stamp: meta.into(),
        })
    }
    pub(super) fn unchanged(&self) -> Result<()> {
        if Stamp::from(self.file.metadata()?) != self.stamp {
            return Err(crate::Error::new(
                crate::ErrorKind::Cache,
                "DRC file changed while open; reopen explicitly",
            ));
        }
        Ok(())
    }
    pub(super) fn len(&self) -> u64 {
        self.stamp.len
    }
    pub(super) fn unchanged_at(&self, path: &Path) -> Result<()> {
        self.unchanged()?;
        if Stamp::from(fs::metadata(path)?) != self.stamp {
            return Err(crate::Error::new(
                crate::ErrorKind::Cache,
                "DRC path changed during operation",
            ));
        }
        Ok(())
    }
    fn read(&self, off: u64, out: &mut [u8]) -> Result<()> {
        if off
            .checked_add(out.len() as u64)
            .is_none_or(|n| n > self.stamp.len)
        {
            return Err(corrupt("read range"));
        }
        self.file.read_exact_at(out, off)?;
        Ok(())
    }
    fn bytes(&self, off: u64, len: usize, cap: usize) -> Result<Vec<u8>> {
        if len > cap {
            return Err(limit("section"));
        }
        let mut out = Vec::new();
        out.try_reserve_exact(len)
            .map_err(|_| limit("allocation"))?;
        out.resize(len, 0);
        self.read(off, &mut out)?;
        Ok(out)
    }
}
fn u32at(b: &[u8], p: usize) -> u32 {
    u32::from_le_bytes(b[p..p + 4].try_into().unwrap())
}
fn u64at(b: &[u8], p: usize) -> u64 {
    u64::from_le_bytes(b[p..p + 8].try_into().unwrap())
}
fn i64at(b: &[u8], p: usize) -> i64 {
    i64::from_le_bytes(b[p..p + 8].try_into().unwrap())
}
fn mul(a: u64, b: u64) -> Result<u64> {
    a.checked_mul(b)
        .ok_or_else(|| corrupt("section multiplication"))
}
fn add(a: u64, b: u64) -> Result<u64> {
    a.checked_add(b).ok_or_else(|| corrupt("section addition"))
}
fn usize_of(n: u64) -> Result<usize> {
    usize::try_from(n).map_err(|_| corrupt("section length"))
}
fn text(b: &[u8], p: u32) -> Result<String> {
    let p = p as usize;
    let size = b
        .get(
            p..p.checked_add(4)
                .ok_or_else(|| corrupt("string reference"))?,
        )
        .ok_or_else(|| corrupt("string reference"))?;
    let n = u32at(size, 0) as usize;
    let s = b
        .get(
            p + 4
                ..(p + 4)
                    .checked_add(n)
                    .ok_or_else(|| corrupt("string length"))?,
        )
        .ok_or_else(|| corrupt("string length"))?;
    if n > 1024 * 1024 {
        return Err(limit("string"));
    }
    Ok(String::from_utf8_lossy(s).into_owned())
}
fn intersects(a: [f64; 4], b: [i64; 4]) -> bool {
    a[0] <= b[2] as f64 && a[2] >= b[0] as f64 && a[1] <= b[3] as f64 && a[3] >= b[1] as f64
}
fn query_box(bbox_um: [f64; 4], precision: f64) -> Result<[f64; 4]> {
    if !bbox_um.iter().all(|v| v.is_finite()) || bbox_um[0] > bbox_um[2] || bbox_um[1] > bbox_um[3]
    {
        return Err(crate::Error::input("invalid DRC query bbox"));
    }
    let q = bbox_um.map(|v| v * precision);
    if !q.iter().all(|v| v.is_finite()) {
        return Err(crate::Error::input("DRC query coordinate overflow"));
    }
    // Broad phase only: (integer / precision) * precision can round below
    // the integer. The final predicate stays in the caller's µm domain.
    Ok([
        q[0].next_down().next_down().floor(),
        q[1].next_down().next_down().floor(),
        q[2].next_up().next_up().ceil(),
        q[3].next_up().next_up().ceil(),
    ])
}
fn bbox_valid(b: [i64; 4]) -> bool {
    b[0] <= b[2] && b[1] <= b[3]
}
fn union(a: [i64; 4], b: [i64; 4]) -> [i64; 4] {
    [
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[2].max(b[2]),
        a[3].max(b[3]),
    ]
}
fn varint(b: &[u8], p: &mut usize) -> Result<u64> {
    let mut out = 0;
    for shift in (0..70).step_by(7) {
        let v = *b
            .get(*p)
            .ok_or_else(|| corrupt("truncated coordinate varint"))?;
        *p += 1;
        if shift == 63 && v > 1 {
            return Err(corrupt("coordinate varint overflow"));
        }
        out |= u64::from(v & 127) << shift;
        if v < 128 {
            if shift > 0 && v == 0 {
                return Err(corrupt("noncanonical coordinate varint"));
            }
            return Ok(out);
        }
    }
    Err(corrupt("coordinate varint overflow"))
}
fn delta(b: &[u8], p: &mut usize, x: i64) -> Result<i64> {
    let v = varint(b, p)?;
    x.checked_add(((v >> 1) as i64) ^ -((v & 1) as i64))
        .ok_or_else(|| corrupt("coordinate delta overflow"))
}

pub struct Pack {
    pub path: PathBuf,
    pub cell: String,
    pub precision: f64,
    pub total: u64,
    pub source_size: u64,
    pub source_mtime: u64,
    pub checks: Vec<Check>,
    input: Input,
    blob_end: u64,
    qbox: u64,
    status: u64,
    wcount: u64,
    blocks: u64,
    block_count: u64,
    review: Option<Input>,
    // Snapshot-derived counts are recounted, not trusted legacy counter bytes.
    review_counts: Option<Vec<u32>>,
    cache: VecDeque<(u64, Arc<Vec<Violation>>, usize)>,
    cache_bytes: usize,
    pub decoded_blocks: u64,
}
impl Pack {
    /// Opening is read-only, including legacy embedded statuses. Sidecars are
    /// selected by the trusted local caller, never synthesized or overwritten.
    pub fn open(path: &Path, cancelled: &AtomicUsize) -> Result<Self> {
        check_cancelled(cancelled)?;
        let input = Input::open(path)?;
        if input.stamp.len < 176 {
            return Err(corrupt("truncated header/footer"));
        }
        let mut head = [0; 40];
        input.read(0, &mut head)?;
        if &head[..8] != MAGIC {
            return Err(corrupt("header magic"));
        }
        if u32at(&head, 8) != VERSION {
            return Err(crate::Error::new(
                crate::ErrorKind::Version,
                "unsupported DRC pack layout; rebuild with floe-index drc <db>",
            ));
        }
        if u32at(&head, 12) != 1 {
            return Err(corrupt("header flags"));
        }
        let precision = f64::from_le_bytes(head[16..24].try_into().unwrap());
        if !precision.is_finite() || precision <= 0.0 {
            return Err(corrupt("precision"));
        }
        let end = input.stamp.len - 136;
        let mut foot = [0; 136];
        input.read(end, &mut foot)?;
        if &foot[128..] != MAGIC || u32at(&foot, 124) != 0 {
            return Err(corrupt("footer magic/reserved"));
        }
        let f: Vec<_> = (0..15).map(|i| u64at(&foot, i * 8)).collect();
        if f[3] != mul(f[14], 4)? {
            return Err(corrupt("qbox length"));
        }
        let sections = [
            (f[0], f[1]),
            (f[2], f[3]),
            (f[4], f[14]),
            (f[5], mul(f[9], 4)?),
            (f[6], mul(f[7], 48)?),
            (f[8], mul(f[9], 64)?),
            (f[10], mul(f[11], 4)?),
            (f[12], f[13]),
        ];
        let mut cursor = 40;
        for (off, len) in sections {
            if off != cursor {
                return Err(corrupt("non-contiguous/overlapping sections"));
            }
            cursor = add(off, len)?;
            if cursor > end {
                return Err(corrupt("section out of file"));
            }
        }
        if cursor != end {
            return Err(corrupt("body end"));
        }
        let meta = add(add(mul(f[9], 64)?, mul(f[11], 4)?)?, f[13])?;
        if meta > META_BYTES as u64 {
            return Err(limit("metadata"));
        }
        let dirs = input.bytes(f[8], usize_of(mul(f[9], 64)?)?, META_BYTES)?;
        let refs = input.bytes(f[10], usize_of(mul(f[11], 4)?)?, META_BYTES)?;
        let strings = input.bytes(f[12], usize_of(f[13])?, META_BYTES)?;
        let cell = text(&strings, u32at(&foot, 120))?;
        if mul(f[9], std::mem::size_of::<Check>() as u64)? > META_BYTES as u64 {
            return Err(limit("check metadata"));
        }
        let mut checks = Vec::new();
        checks
            .try_reserve_exact(usize_of(f[9])?)
            .map_err(|_| limit("check table allocation"))?;
        let mut used = cell.len() as u64;
        let (mut es, mut bs) = (0, 0);
        for d in dirs.chunks_exact(64) {
            check_cancelled(cancelled)?;
            let (ds, dc) = (u32at(d, 4) as u64, u32at(d, 8) as u64);
            let (start, count, bstart, bcnt) =
                (u64at(d, 16), u64at(d, 24), u64at(d, 48), u64at(d, 56));
            if u32at(d, 12) != 0
                || start != es
                || bstart != bs
                || add(ds, dc)? > f[11]
                || bcnt != count.div_ceil(64)
            {
                return Err(corrupt("check spans/block count"));
            }
            es = add(es, count)?;
            bs = add(bs, bcnt)?;
            if es > f[14] || bs > f[7] {
                return Err(corrupt("check range"));
            }
            let name = text(&strings, u32at(d, 0))?;
            used = add(used, (std::mem::size_of::<Check>() + name.len()) as u64)?;
            let mut desc = String::new();
            for r in ds..ds + dc {
                let s = text(&strings, u32at(&refs, usize_of(r * 4)?))?;
                used = add(used, (s.len() + 1) as u64)?;
                if used > META_BYTES as u64 {
                    return Err(limit("decoded metadata"));
                }
                if r > ds {
                    desc.push('\n');
                }
                desc.push_str(&s);
            }
            if used > META_BYTES as u64 {
                return Err(limit("decoded metadata"));
            }
            checks.push(Check {
                name,
                desc,
                declared: u64at(d, 32),
                original: u64at(d, 40),
                start,
                count,
                bbox: None,
                block_start: bstart,
                block_count: bcnt,
            });
        }
        if es != f[14] || bs != f[7] {
            return Err(corrupt("unowned errors/blocks"));
        }
        let mut p = Self {
            path: path.to_owned(),
            cell,
            precision,
            total: f[14],
            source_size: u64at(&head, 24),
            source_mtime: u64at(&head, 32),
            input,
            blob_end: f[2],
            qbox: f[2],
            status: f[4],
            wcount: f[5],
            blocks: f[6],
            block_count: f[7],
            checks,
            review: None,
            review_counts: None,
            cache: VecDeque::new(),
            cache_bytes: 0,
            decoded_blocks: 0,
        };
        // Same check-bbox prepass as the legacy reader, but bounded 192 KiB
        // slabs instead of a resident 48B-per-block table. No coordinate decode.
        let mut next_ci = 0;
        let mut previous = None;
        for first in (0..p.block_count).step_by(4096) {
            check_cancelled(cancelled)?;
            let n = (p.block_count - first).min(4096);
            let data = p.input.bytes(
                add(p.blocks, mul(first, 48)?)?,
                usize_of(n * 48)?,
                4096 * 48,
            )?;
            for (i, b) in data.chunks_exact(48).enumerate() {
                let bi = first + i as u64;
                while next_ci < p.checks.len()
                    && bi >= p.checks[next_ci].block_start + p.checks[next_ci].block_count
                {
                    next_ci += 1;
                }
                let c = p
                    .checks
                    .get_mut(next_ci)
                    .ok_or_else(|| corrupt("unowned block"))?;
                let off = u64at(b, 0);
                let count = u32at(b, 8) as u64;
                let bbox = [i64at(b, 16), i64at(b, 24), i64at(b, 32), i64at(b, 40)];
                let expected = (c.count - (bi - c.block_start) * 64).min(64);
                if count != expected
                    || u32at(b, 12) != 0
                    || !bbox_valid(bbox)
                    || off >= p.blob_end
                    || previous.is_none_or(|last| off <= last) && bi != 0
                    || bi == 0 && off != 40
                {
                    return Err(corrupt("block metadata"));
                }
                previous = Some(off);
                c.bbox = Some(c.bbox.map_or(bbox, |a| union(a, bbox)));
            }
        }
        if p.block_count == 0 && p.blob_end != 40 {
            return Err(corrupt("unowned coordinate blob"));
        }
        for i in 0..p.checks.len() {
            if p.waived_count(i)? > p.checks[i].count {
                return Err(corrupt("waived count"));
            }
        }
        p.unchanged()?;
        Ok(p)
    }
    pub fn unchanged(&self) -> Result<()> {
        self.input.unchanged()?;
        if let Some(r) = &self.review {
            r.unchanged()?;
        }
        Ok(())
    }
    /// Review writers bind both the open inode and its registered path.
    pub(crate) fn unchanged_at(&self) -> Result<()> {
        self.unchanged()?;
        self.input.unchanged_at(&self.path)
    }
    /// Durable conservative run binding. Copying/replacing/touching a pack may
    /// require explicit import again; matching legacy header fields is not enough.
    /// Not a credential or defense against malicious same-UID metadata forgery.
    pub(crate) fn review_binding(&self) -> Result<Vec<u8>> {
        self.unchanged_at()?;
        self.geometry_binding()
    }
    /// A saved waiver replaces the old inode. Refresh checks the geometry
    /// independently, so stale review metadata cannot prevent its own recovery.
    pub(super) fn geometry_binding(&self) -> Result<Vec<u8>> {
        self.input.unchanged_at(&self.path)?;
        let s = self.input.stamp;
        Ok(format!(
            "floe-review-pack-v1:{}:{}:{}:{}:{}:{}:{}",
            s.dev, s.ino, s.len, s.modified.0, s.modified.1, s.changed.0, s.changed.1
        )
        .into_bytes())
    }
    pub(super) fn install_waives(
        &mut self,
        identity: &super::review::Identity,
        review: Option<Input>,
        counts: Vec<u32>,
        stop: &AtomicUsize,
    ) -> Result<()> {
        if self.geometry_binding()? != identity.0 {
            return Err(corrupt("waive snapshot belongs to another pack"));
        }
        if counts.len() != self.checks.len()
            || counts
                .iter()
                .zip(&self.checks)
                .any(|(&n, c)| u64::from(n) > c.count)
        {
            return Err(corrupt("waive snapshot counts"));
        }
        if let Some(r) = &review {
            r.unchanged()?;
        }
        check_cancelled(stop)?;
        // No fallible work after this point. Geometry blocks, qboxes, check
        // identity and their decoded LRU are unchanged; statuses are read apart.
        self.review = review;
        self.review_counts = Some(counts);
        Ok(())
    }
    pub(super) fn has_waives(&self) -> bool {
        self.review.is_some()
    }
    pub(crate) fn review_statuses(&self, gids: &[u64], stop: &AtomicUsize) -> Result<Vec<u8>> {
        self.unchanged_at()?;
        let result = super::review::selected_statuses(
            &self.input.file,
            self.status,
            self.total,
            gids,
            stop,
        )?;
        self.unchanged_at()?;
        Ok(result)
    }
    /// Virtual legacy sidecar seed: header + embedded status/counters, without
    /// copying all errors into RAM or ever opening the pack for writing.
    pub(crate) fn review_seed(&self) -> Result<impl std::io::Read + '_> {
        self.unchanged_at()?;
        let layout = super::review::Layout::from_pack(self)?;
        struct Section<'a> {
            file: &'a File,
            offset: u64,
            remaining: u64,
        }
        impl std::io::Read for Section<'_> {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let n = self.remaining.min(buf.len() as u64) as usize;
                if n == 0 {
                    return Ok(0);
                }
                let n = self.file.read_at(&mut buf[..n], self.offset)?;
                if n == 0 {
                    return Err(std::io::ErrorKind::UnexpectedEof.into());
                }
                self.offset += n as u64;
                self.remaining -= n as u64;
                Ok(n)
            }
        }
        use std::io::Read;
        Ok(std::io::Cursor::new(layout.header())
            .chain(Section {
                file: &self.input.file,
                offset: self.status,
                remaining: self.total,
            })
            .chain(Section {
                file: &self.input.file,
                offset: self.wcount,
                remaining: self.checks.len() as u64 * 4,
            }))
    }
    pub fn source_matches(&self, path: &Path) -> Result<bool> {
        let m = fs::metadata(path)?;
        Ok(m.len() == self.source_size && u64::try_from(m.mtime()).ok() == Some(self.source_mtime))
    }
    pub fn attach_waives(&mut self, path: &Path) -> Result<()> {
        self.unchanged()?;
        let r = Input::open(path)?;
        let mut h = [0; 40];
        r.read(0, &mut h)?;
        if &h[..8] != b"FLOEWAIV"
            || u32at(&h, 8) != 1
            || u64at(&h, 12) != self.source_size
            || u64at(&h, 20) != self.source_mtime
            || u64at(&h, 28) != self.total
            || u32at(&h, 36) as usize != self.checks.len()
            || r.stamp.len != add(add(40, self.total)?, mul(self.checks.len() as u64, 4)?)?
        {
            return Err(crate::Error::input(
                "waive sidecar does not match this DRC pack",
            ));
        }
        for (i, c) in self.checks.iter().enumerate() {
            let mut b = [0; 4];
            r.read(add(add(40, self.total)?, mul(i as u64, 4)?)?, &mut b)?;
            if u32at(&b, 0) as u64 > c.count {
                return Err(corrupt("sidecar waived count"));
            }
        }
        r.unchanged()?;
        self.review = Some(r);
        self.review_counts = None;
        Ok(())
    }
    pub fn waived_count(&self, check: usize) -> Result<u64> {
        if check >= self.checks.len() {
            return Err(crate::Error::input("DRC check index out of range"));
        }
        if let Some(counts) = &self.review_counts {
            return Ok(u64::from(counts[check]));
        }
        let (f, off) = self
            .review
            .as_ref()
            .map_or((&self.input, self.wcount), |r| (r, 40 + self.total));
        let mut b = [0; 4];
        f.read(add(off, mul(check as u64, 4)?)?, &mut b)?;
        Ok(u32at(&b, 0) as u64)
    }
    pub fn status(&self, check: usize, error: u64) -> Result<u8> {
        let c = self
            .checks
            .get(check)
            .filter(|c| error < c.count)
            .ok_or_else(|| crate::Error::input("DRC error index out of range"))?;
        let (f, off) = self
            .review
            .as_ref()
            .map_or((&self.input, self.status), |r| (r, 40));
        let mut b = [0; 1];
        f.read(add(off, c.start + error)?, &mut b)?;
        Ok(b[0])
    }
    fn block(
        &mut self,
        check: usize,
        bi: u64,
        cancelled: &AtomicUsize,
    ) -> Result<Arc<Vec<Violation>>> {
        check_cancelled(cancelled)?;
        if let Some(i) = self.cache.iter().position(|(b, _, _)| *b == bi) {
            let item = self.cache.remove(i).unwrap();
            let found = Arc::clone(&item.1);
            self.cache.push_back(item);
            return Ok(found);
        }
        let c = &self.checks[check];
        let mut b = [0; 48];
        self.input.read(add(self.blocks, mul(bi, 48)?)?, &mut b)?;
        let off = u64at(&b, 0);
        let count = u32at(&b, 8) as usize;
        if !(1..=64).contains(&count) {
            return Err(corrupt("block count changed"));
        }
        let bbox = [i64at(&b, 16), i64at(&b, 24), i64at(&b, 32), i64at(&b, 40)];
        let end = if bi + 1 < self.block_count {
            let mut n = [0; 8];
            self.input
                .read(add(self.blocks, mul(bi + 1, 48)?)?, &mut n)?;
            u64at(&n, 0)
        } else {
            self.blob_end
        };
        let length = end
            .checked_sub(off)
            .ok_or_else(|| corrupt("block offsets"))?;
        if length > BLOCK_BYTES as u64 {
            return Err(limit("coordinate block"));
        }
        let buf = self.input.bytes(off, usize_of(length)?, BLOCK_BYTES)?;
        let (mut pos, mut previous, mut total) = (0, [0i64; 2], 0);
        let mut records = Vec::with_capacity(count);
        for j in 0..count {
            check_cancelled(cancelled)?;
            let kind = varint(&buf, &mut pos)?;
            let n = usize_of(kind >> 1)?;
            if n == 0 {
                return Err(corrupt("empty geometry"));
            }
            if n > RECORD_POINTS || n > BLOCK_POINTS - total {
                return Err(limit("coordinate points"));
            }
            // At least two one-byte deltas per point must be left before any
            // allocation. Corrupt counts must not request arbitrary memory.
            if n > (buf.len() - pos) / 2 {
                return Err(corrupt("point count exceeds block bytes"));
            }
            total += n;
            let mut points = Vec::new();
            points
                .try_reserve_exact(n)
                .map_err(|_| limit("geometry allocation"))?;
            let first = [
                delta(&buf, &mut pos, previous[0])?,
                delta(&buf, &mut pos, previous[1])?,
            ];
            previous = first;
            let mut bb = [first[0], first[1], first[0], first[1]];
            points.push(first);
            let mut point = first;
            for _ in 1..n {
                if points.len() % 4096 == 0 {
                    check_cancelled(cancelled)?;
                }
                point = [
                    delta(&buf, &mut pos, point[0])?,
                    delta(&buf, &mut pos, point[1])?,
                ];
                bb = union(bb, [point[0], point[1], point[0], point[1]]);
                points.push(point);
            }
            if bb[0] < bbox[0] || bb[1] < bbox[1] || bb[2] > bbox[2] || bb[3] > bbox[3] {
                return Err(corrupt("geometry outside block bbox"));
            }
            records.push(Violation {
                kind: if kind & 1 == 1 { 'e' } else { 'p' },
                number: c.start + (bi - c.block_start) * 64 + j as u64 + 1,
                points,
                bbox: bb,
            });
        }
        if pos != buf.len() {
            return Err(corrupt("trailing coordinate bytes"));
        }
        let size = total * std::mem::size_of::<[i64; 2]>()
            + records.len() * std::mem::size_of::<Violation>();
        let records = Arc::new(records);
        while self.cache.len() >= 16 || self.cache_bytes + size > CACHE_BYTES {
            let Some((_, _, n)) = self.cache.pop_front() else {
                break;
            };
            self.cache_bytes -= n;
        }
        if size <= CACHE_BYTES {
            self.cache.push_back((bi, Arc::clone(&records), size));
            self.cache_bytes += size;
        }
        self.decoded_blocks += 1;
        Ok(records)
    }
    fn with_record<T>(
        &mut self,
        check: usize,
        error: u64,
        cancelled: &AtomicUsize,
        read: impl FnOnce(&Violation) -> Result<T>,
    ) -> Result<T> {
        self.unchanged()?;
        let c = self
            .checks
            .get(check)
            .filter(|c| error < c.count)
            .ok_or_else(|| crate::Error::input("DRC error index out of range"))?;
        let bi = c.block_start + error / 64;
        let records = self.block(check, bi, cancelled)?;
        let e = records
            .get((error % 64) as usize)
            .ok_or_else(|| corrupt("block member index"))?;
        let value = read(e)?;
        self.unchanged()?;
        Ok(value)
    }
    pub fn error(
        &mut self,
        check: usize,
        error: u64,
        cancelled: &AtomicUsize,
    ) -> Result<Violation> {
        self.with_record(check, error, cancelled, |v| Ok(v.clone()))
    }
    /// Validation still decodes the containing block, but a bbox-only consumer
    /// need not clone a large polygon's coordinates or retain a cache Arc.
    pub fn error_info(
        &mut self,
        check: usize,
        error: u64,
        cancelled: &AtomicUsize,
    ) -> Result<RecordInfo> {
        self.with_record(check, error, cancelled, |v| Ok(RecordInfo::from(v)))
    }
    /// Compute against the cached, validated record without copying a large
    /// selected polygon or exposing its cache Arc to the gateway. The rule is
    /// matched by the caller to this pack's check name, not chosen by the UI.
    pub fn constraint_comparison<'a>(
        &mut self,
        check: usize,
        error: u64,
        rule: Option<&'a crate::svrf::Rule>,
        cancelled: &AtomicUsize,
    ) -> Result<(RecordInfo, Option<crate::svrf::Comparison<'a>>)> {
        let precision = self.precision;
        self.with_record(check, error, cancelled, |v| {
            let compared = rule
                .map(|r| r.compare(v.kind, &v.points, precision, cancelled))
                .transpose()?
                .flatten();
            Ok((RecordInfo::from(v), compared))
        })
    }
    /// Copy only the requested coordinate slice. Coordinate-copy cost over
    /// all pages is O(total points), not O(record * pages). Cache eviction
    /// can still require decoding the containing block again.
    pub fn error_points(
        &mut self,
        check: usize,
        error: u64,
        start: usize,
        limit: usize,
        cancelled: &AtomicUsize,
    ) -> Result<PointPage> {
        if !(1..=2048).contains(&limit) || start > RECORD_POINTS {
            return Err(crate::Error::input("invalid point page"));
        }
        self.with_record(check, error, cancelled, |v| {
            if start > v.points.len() {
                return Err(crate::Error::input("point cursor"));
            }
            let end = (start + limit).min(v.points.len());
            Ok(PointPage {
                record: RecordInfo::from(v),
                start,
                points: v.points[start..end].to_vec(),
                next: (end < v.points.len()).then_some(end),
            })
        })
    }
    /// A single rule's file-order page, with blockwise status I/O. This is
    /// also the streamed CLI path: not two stat calls per individual error.
    pub fn errors(
        &mut self,
        check: usize,
        start: u64,
        limit: usize,
        cancelled: &AtomicUsize,
    ) -> Result<Page> {
        self.unchanged()?;
        check_cancelled(cancelled)?;
        let c = self
            .checks
            .get(check)
            .filter(|c| start <= c.count)
            .ok_or_else(|| crate::Error::input("invalid DRC error page"))?;
        if !(1..=PAGE_ITEMS).contains(&limit) {
            return Err(crate::Error::input("invalid DRC page limit"));
        }
        let (count, block_start, global_start) = (c.count, c.block_start, c.start);
        let mut page = Page {
            hits: Vec::new(),
            next: None,
            scanned: 0,
        };
        let mut ei = start;
        let mut points = 0;
        'pages: while ei < count && page.hits.len() < limit {
            check_cancelled(cancelled)?;
            let records = self.block(check, block_start + ei / 64, cancelled)?;
            let len = (count - ei)
                .min(64 - ei % 64)
                .min((limit - page.hits.len()) as u64) as usize;
            let (f, off) = self
                .review
                .as_ref()
                .map_or((&self.input, self.status), |r| (r, 40));
            let mut status = [0; 64];
            f.read(add(off, global_start + ei)?, &mut status[..len])?;
            for st in &status[..len] {
                let e = records
                    .get((ei % 64) as usize)
                    .ok_or_else(|| corrupt("page member index"))?;
                if points + e.points.len() > BLOCK_POINTS {
                    break 'pages;
                }
                points += e.points.len();
                page.hits.push(Hit {
                    check,
                    local: ei,
                    status: *st,
                    violation: e.clone(),
                });
                ei += 1;
            }
        }
        page.scanned = ei - start;
        page.next = (ei < count).then_some(Cursor { check, error: ei });
        self.unchanged()?;
        Ok(page)
    }
    pub fn bbox_um(&self, bbox: [i64; 4]) -> Result<[f64; 4]> {
        let b = bbox.map(|n| n as f64 / self.precision);
        if !b.iter().all(|x| x.is_finite()) {
            return Err(crate::Error::input("DRC micrometre coordinate overflow"));
        }
        Ok(b)
    }
    /// Next/previous matching error in ONE rule, wrapping once. Work and
    /// response size are bounded even when a sparse filter finds no hit.
    pub fn step(&mut self, request: StepRequest, cancelled: &AtomicUsize) -> Result<StepPage> {
        self.step_with_limit(request, SCAN_ITEMS, cancelled)
    }
    pub(super) fn step_with_limit(
        &mut self,
        request: StepRequest,
        limit: u64,
        cancelled: &AtomicUsize,
    ) -> Result<StepPage> {
        self.unchanged()?;
        check_cancelled(cancelled)?;
        let StepRequest {
            check,
            backwards,
            after,
            cursor,
            waived,
            bbox_um,
        } = request;
        let c = self
            .checks
            .get(check)
            .ok_or_else(|| crate::Error::input("DRC rule index"))?;
        let (count, block_start, global_start) = (c.count, c.block_start, c.start);
        if !(1..=SCAN_ITEMS).contains(&limit)
            || after.is_some_and(|i| i >= count)
            || cursor.is_some_and(|c| c.next >= count || c.remaining == 0 || c.remaining > count)
            || after.is_some() && cursor.is_some()
        {
            return Err(crate::Error::input("invalid DRC step cursor"));
        }
        let broad = bbox_um.map(|b| query_box(b, self.precision)).transpose()?;
        let mut page = StepPage {
            hit: None,
            next: None,
            scanned: 0,
        };
        if count == 0 || broad.is_some_and(|b| c.bbox.is_none_or(|c| !intersects(b, c))) {
            self.unchanged()?;
            return Ok(page);
        }
        let advance = |i: u64| {
            if backwards {
                if i == 0 {
                    count - 1
                } else {
                    i - 1
                }
            } else if i == count - 1 {
                0
            } else {
                i + 1
            }
        };
        let mut cursor = cursor.unwrap_or_else(|| StepCursor {
            next: after.map_or(if backwards { count - 1 } else { 0 }, advance),
            remaining: count,
        });
        let mut blocks = 0;
        'blocks: while cursor.remaining > 0 && page.scanned < limit && blocks < 4096 {
            blocks += 1;
            check_cancelled(cancelled)?;
            let at = cursor.next;
            let len = if backwards {
                at % 64 + 1
            } else {
                (64 - at % 64).min(count - at)
            }
            .min(cursor.remaining)
            .min(limit - page.scanned) as usize;
            let lo = if backwards { at + 1 - len as u64 } else { at };
            let bi = block_start + at / 64;
            let mut block = [0; 48];
            self.input
                .read(add(self.blocks, mul(bi, 48)?)?, &mut block)?;
            let block_bb = [
                i64at(&block, 16),
                i64at(&block, 24),
                i64at(&block, 32),
                i64at(&block, 40),
            ];
            if broad.is_some_and(|b| !intersects(b, block_bb)) {
                cursor.next = advance(if backwards { lo } else { lo + len as u64 - 1 });
                cursor.remaining -= len as u64;
                page.scanned += len as u64;
                continue;
            }
            let mut statuses = [0; 64];
            let (f, off) = self
                .review
                .as_ref()
                .map_or((&self.input, self.status), |r| (r, 40));
            f.read(add(off, global_start + lo)?, &mut statuses[..len])?;
            // Do not trust the optional waived counters to skip a block:
            // older tools can leave counters stale while statuses are valid.
            let mut records = None;
            for offset in 0..len {
                let j = if backwards { len - 1 - offset } else { offset };
                let ei = lo + j as u64;
                cursor.next = advance(ei);
                cursor.remaining -= 1;
                page.scanned += 1;
                if waived.is_some_and(|w| (statuses[j] == 1) != w) {
                    continue;
                }
                if records.is_none() {
                    records = Some(self.block(check, bi, cancelled)?);
                }
                let e = records
                    .as_ref()
                    .unwrap()
                    .get((ei % 64) as usize)
                    .ok_or_else(|| corrupt("step member index"))?;
                if let Some(b) = bbox_um {
                    let eb = self.bbox_um(e.bbox)?;
                    if b[0] > eb[2] || b[2] < eb[0] || b[1] > eb[3] || b[3] < eb[1] {
                        continue;
                    }
                }
                page.hit = Some(InfoHit {
                    check,
                    local: ei,
                    status: statuses[j],
                    record: RecordInfo::from(e),
                });
                break 'blocks;
            }
        }
        page.next = (page.hit.is_none() && cursor.remaining > 0).then_some(cursor);
        check_cancelled(cancelled)?;
        self.unchanged()?;
        Ok(page)
    }
    /// Bounded spatial page. `scanned` counts error slots, not just hits; an
    /// empty dense query therefore yields a continuation instead of hanging.
    pub fn query(
        &mut self,
        bbox_um: [f64; 4],
        checks: Option<&BTreeSet<usize>>,
        waived: Option<bool>,
        cursor: Cursor,
        limit: usize,
        cancelled: &AtomicUsize,
    ) -> Result<Page> {
        let p = self.query_page::<Hit>(bbox_um, checks, waived, cursor, limit, cancelled)?;
        Ok(Page {
            hits: p.hits,
            next: p.next,
            scanned: p.scanned,
        })
    }
    /// Same broad/exact intersection and continuation as geometry queries,
    /// without cloning vertices or imposing a copied-point limit on metadata.
    pub fn query_info(
        &mut self,
        bbox_um: [f64; 4],
        checks: Option<&BTreeSet<usize>>,
        waived: Option<bool>,
        cursor: Cursor,
        limit: usize,
        cancelled: &AtomicUsize,
    ) -> Result<InfoPage> {
        let p = self.query_page::<InfoHit>(bbox_um, checks, waived, cursor, limit, cancelled)?;
        Ok(InfoPage {
            hits: p.hits,
            next: p.next,
            scanned: p.scanned,
        })
    }
    fn query_page<T: QueryHit>(
        &mut self,
        bbox_um: [f64; 4],
        checks: Option<&BTreeSet<usize>>,
        waived: Option<bool>,
        mut cursor: Cursor,
        limit: usize,
        cancelled: &AtomicUsize,
    ) -> Result<QueryPage<T>> {
        self.unchanged()?;
        check_cancelled(cancelled)?;
        if !(1..=PAGE_ITEMS).contains(&limit)
            || !bbox_um.iter().all(|x| x.is_finite())
            || bbox_um[0] > bbox_um[2]
            || bbox_um[1] > bbox_um[3]
            || cursor.check > self.checks.len()
            || cursor.check == self.checks.len() && cursor.error != 0
            || cursor.check < self.checks.len() && cursor.error > self.checks[cursor.check].count
            || checks.is_some_and(|cs| cs.iter().any(|i| *i >= self.checks.len()))
        {
            return Err(crate::Error::input("invalid DRC query/cursor/limit"));
        }
        let q = query_box(bbox_um, self.precision)?;
        let mut page = QueryPage {
            hits: Vec::new(),
            next: None,
            scanned: 0,
        };
        let (mut steps, mut output_points) = (0, 0);
        'search: while cursor.check < self.checks.len() {
            if steps >= 4096 {
                break;
            }
            steps += 1;
            check_cancelled(cancelled)?;
            let ci = cursor.check;
            let c = &self.checks[ci];
            if checks.is_some_and(|cs| !cs.contains(&ci))
                || c.bbox.is_none_or(|b| !intersects(q, b))
                || cursor.error == c.count
            {
                cursor = Cursor {
                    check: ci + 1,
                    error: 0,
                };
                continue;
            }
            let bi = c.block_start + cursor.error / 64;
            let bb = c.bbox.unwrap();
            let quant = |v: f64, lo: i64, hi: i64, up: bool| -> u8 {
                let span = hi as i128 - lo as i128;
                if span <= 0 {
                    return if up { 255 } else { 0 };
                }
                let x = (v - lo as f64) * 255.0 / span as f64;
                (if up { x.ceil() } else { x.floor() }).clamp(0.0, 255.0) as u8
            };
            let qb = [
                quant(q[0], bb[0], bb[2], false),
                quant(q[1], bb[1], bb[3], false),
                quant(q[2], bb[0], bb[2], true),
                quant(q[3], bb[1], bb[3], true),
            ];
            let mut block = [0; 48];
            self.input
                .read(add(self.blocks, mul(bi, 48)?)?, &mut block)?;
            let block_bb = [
                i64at(&block, 16),
                i64at(&block, 24),
                i64at(&block, 32),
                i64at(&block, 40),
            ];
            let remain = (c.count - cursor.error).min(64 - cursor.error % 64);
            if !intersects(q, block_bb) {
                cursor.error += remain;
                page.scanned += remain;
            } else {
                let mut qboxes = [0; 256];
                self.input.read(
                    add(self.qbox, mul(c.start + cursor.error, 4)?)?,
                    &mut qboxes[..remain as usize * 4],
                )?;
                let mut statuses = [0; 64];
                let (f, off) = self
                    .review
                    .as_ref()
                    .map_or((&self.input, self.status), |r| (r, 40));
                f.read(
                    add(off, c.start + cursor.error)?,
                    &mut statuses[..remain as usize],
                )?;
                for j in 0..remain as usize {
                    let qbox = &qboxes[j * 4..j * 4 + 4];
                    let ei = cursor.error;
                    cursor.error += 1;
                    page.scanned += 1;
                    if qbox[0] > qbox[2] || qbox[1] > qbox[3] {
                        return Err(corrupt("quantized bbox"));
                    }
                    if waived.is_none_or(|w| (statuses[j] == 1) == w)
                        && qbox[0] <= qb[2]
                        && qbox[2] >= qb[0]
                        && qbox[1] <= qb[3]
                        && qbox[3] >= qb[1]
                    {
                        let records = self.block(ci, bi, cancelled)?;
                        let e = records
                            .get((ei % 64) as usize)
                            .ok_or_else(|| corrupt("query member index"))?;
                        let eb = self.bbox_um(e.bbox)?;
                        if bbox_um[0] <= eb[2]
                            && bbox_um[2] >= eb[0]
                            && bbox_um[1] <= eb[3]
                            && bbox_um[3] >= eb[1]
                        {
                            let points = if T::COPY_POINTS { e.points.len() } else { 0 };
                            if output_points + points > BLOCK_POINTS {
                                cursor.error = ei;
                                break 'search;
                            }
                            output_points += points;
                            page.hits.push(T::project(ci, ei, statuses[j], e));
                        }
                    }
                    if page.hits.len() == limit || page.scanned >= SCAN_ITEMS {
                        break 'search;
                    }
                }
            }
            if page.scanned >= SCAN_ITEMS {
                break;
            }
        }
        if cursor.check < self.checks.len() && cursor.error == self.checks[cursor.check].count {
            cursor = Cursor {
                check: cursor.check + 1,
                error: 0,
            };
        }
        page.next = (cursor.check < self.checks.len()).then_some(cursor);
        self.unchanged()?;
        Ok(page)
    }
}
