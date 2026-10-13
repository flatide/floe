//! Exact, bounded-memory matching of review state between two DRC runs
//! (feature/jobdeck-drc's floe-index drc-match, 2026-10-11, on app-core's
//! pack reader). For each pair of same-named rules every error's geometry
//! is put in a canonical form - polygons from their least rotation in
//! either direction, closing points dropped; edges as sorted endpoint
//! pairs; each coordinate the exact rational value/precision - and hashed;
//! the hashes are sorted externally (bounded runs, 16-way merges). A hash
//! group of the old run with review work is verified byte for byte against
//! every target of the group (a hash collision makes the group ambiguous,
//! never a transfer). All old duplicates, untouched ones included, vote on
//! waived and not-required separately; visits are united; a field the
//! duplicates disagree on stays unknown.
//!
//! Output (protocol 2), sorted by (target rule, error), 16 bytes a row:
//! u32 rule, u8 flags (V=1, W=2, NR=4), u8 disposition (0 exact, 1 hash
//! collision), u8 unknown judgement mask (W=2, NR=4), u8 0, u64 error.
use crate::drc::Pack;
use crate::{check_cancelled, Error, ErrorKind, Result};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const PROTOCOL: u32 = 2;
const BLOCK: u64 = 64;
const FANIN: usize = 16;
const RECORD_BYTES: usize = 24;
/// a hash group up to this many records is replayed from memory
const GROUP_BUFFER: usize = 4096;
type Point = (i64, i64);

fn fail(message: impl Into<String>) -> Error {
    Error::new(ErrorKind::Io, message)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct Record {
    a: u64,
    b: u64,
    ei: u64,
}
impl Record {
    fn key(self) -> (u64, u64) {
        (self.a, self.b)
    }
    fn write(self, out: &mut impl Write) -> Result<()> {
        out.write_all(&self.a.to_le_bytes())
            .and_then(|_| out.write_all(&self.b.to_le_bytes()))
            .and_then(|_| out.write_all(&self.ei.to_le_bytes()))
            .map_err(|e| fail(format!("write sort record: {e}")))
    }
    fn read(input: &mut impl Read) -> Result<Option<Self>> {
        let mut data = [0u8; RECORD_BYTES];
        match input.read(&mut data[..1]) {
            Ok(0) => return Ok(None),
            Ok(_) => {}
            Err(e) => return Err(fail(format!("read sort record: {e}"))),
        }
        input
            .read_exact(&mut data[1..])
            .map_err(|e| fail(format!("truncated sort record: {e}")))?;
        Ok(Some(Self {
            a: u64::from_le_bytes(data[..8].try_into().unwrap()),
            b: u64::from_le_bytes(data[8..16].try_into().unwrap()),
            ei: u64::from_le_bytes(data[16..].try_into().unwrap()),
        }))
    }
}

fn create(path: &Path) -> Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| fail(format!("create {}: {e}", path.display())))
}

/// `[[old_ci,new_ci], ...]`, each rule at most once on either side.
pub fn parse_pairs(input: &[u8]) -> Result<Vec<(usize, usize)>> {
    struct Parser<'a> {
        data: &'a [u8],
        at: usize,
    }
    impl Parser<'_> {
        fn ws(&mut self) {
            while self.data.get(self.at).is_some_and(u8::is_ascii_whitespace) {
                self.at += 1;
            }
        }
        fn token(&mut self, byte: u8) -> Result<()> {
            self.ws();
            if self.data.get(self.at) != Some(&byte) {
                return Err(Error::input("plan must be JSON [[old_ci,new_ci], ...]"));
            }
            self.at += 1;
            Ok(())
        }
        fn number(&mut self) -> Result<usize> {
            self.ws();
            let start = self.at;
            while self.data.get(self.at).is_some_and(u8::is_ascii_digit) {
                self.at += 1;
            }
            let bytes = &self.data[start..self.at];
            if bytes.is_empty() || (bytes.len() > 1 && bytes[0] == b'0') {
                return Err(Error::input("invalid migration rule index"));
            }
            let n = std::str::from_utf8(bytes)
                .unwrap()
                .parse::<u32>()
                .map_err(|_| Error::input("migration rule index overflow"))?;
            Ok(n as usize)
        }
    }
    let mut p = Parser { data: input, at: 0 };
    let mut out = Vec::new();
    let (mut olds, mut news) = (HashSet::new(), HashSet::new());
    p.token(b'[')?;
    p.ws();
    if p.data.get(p.at) != Some(&b']') {
        loop {
            p.token(b'[')?;
            let old = p.number()?;
            p.token(b',')?;
            let new = p.number()?;
            p.token(b']')?;
            if !olds.insert(old) || !news.insert(new) {
                return Err(Error::input("migration plan repeats a source or target rule"));
            }
            out.push((old, new));
            p.ws();
            if p.data.get(p.at) != Some(&b',') {
                break;
            }
            p.at += 1;
        }
    }
    p.token(b']')?;
    p.ws();
    if p.at != input.len() {
        return Err(Error::input("trailing migration plan data"));
    }
    Ok(out)
}

#[derive(Clone, Copy)]
struct Scale {
    odd: u64,
    exponent: i16,
}
impl Scale {
    fn new(precision: f64) -> Result<Self> {
        if !precision.is_finite() || precision <= 0.0 {
            return Err(Error::input("invalid DRC precision"));
        }
        let bits = precision.to_bits();
        let exp = ((bits >> 52) & 0x7ff) as i16;
        let mantissa = (bits & ((1u64 << 52) - 1)) | if exp == 0 { 0 } else { 1u64 << 52 };
        let zeros = mantissa.trailing_zeros() as i16;
        Ok(Self {
            odd: mantissa >> zeros,
            exponent: (if exp == 0 { -1074 } else { exp - 1023 - 52 }) + zeros,
        })
    }
    fn coordinate(self, value: i64, out: &mut Vec<u8>) {
        // value / precision = sign * odd_n / odd_d * 2^exponent: factored
        // powers of two never overflow, i64::MIN and subnormals included
        let (numerator, denominator, exponent) = if value == 0 {
            (0, 1, 0)
        } else {
            let magnitude = value.unsigned_abs();
            let zeros = magnitude.trailing_zeros();
            let odd = magnitude >> zeros;
            let divisor = gcd(odd, self.odd);
            (odd / divisor, self.odd / divisor, zeros as i16 - self.exponent)
        };
        out.push(u8::from(value < 0));
        out.extend_from_slice(&numerator.to_le_bytes());
        out.extend_from_slice(&denominator.to_le_bytes());
        out.extend_from_slice(&exponent.to_le_bytes());
    }
    fn point(self, point: Point, out: &mut Vec<u8>) {
        self.coordinate(point.0, out);
        self.coordinate(point.1, out);
    }
}
fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let next = a % b;
        a = b;
        b = next;
    }
    a
}
fn point_at(points: &[Point], at: usize, reverse: bool) -> Point {
    let at = at % points.len();
    points[if reverse { points.len() - 1 - at } else { at }]
}
fn rotation(points: &[Point], reverse: bool) -> usize {
    // Booth's algorithm: linear even for repeated vertices
    let n = points.len();
    let (mut a, mut b, mut k) = (0, 1, 0);
    while a < n && b < n && k < n {
        let x = point_at(points, a + k, reverse);
        let y = point_at(points, b + k, reverse);
        if x == y {
            k += 1;
            continue;
        }
        if x > y {
            a += k + 1;
            if a == b {
                a += 1;
            }
        } else {
            b += k + 1;
            if a == b {
                b += 1;
            }
        }
        k = 0;
    }
    a.min(b) % n
}
/// The canonical bytes of one error (kind 0 polygon, 1 edges); false for
/// a shape that cannot be one (no points, an odd edge list).
fn canonical(
    kind: u8,
    points: &mut Vec<Point>,
    scale: Scale,
    out: &mut Vec<u8>,
    edges: &mut Vec<(Point, Point)>,
) -> bool {
    out.clear();
    if points.is_empty() || (kind == 1 && points.len() % 2 != 0) {
        return false;
    }
    if kind == 0 {
        while points.len() > 1 && points.first() == points.last() {
            points.pop();
        }
    }
    out.push(kind);
    out.extend_from_slice(&(points.len() as u64).to_le_bytes());
    if kind == 1 {
        edges.clear();
        for pair in points.chunks_exact(2) {
            edges.push(if pair[0] <= pair[1] {
                (pair[0], pair[1])
            } else {
                (pair[1], pair[0])
            });
        }
        edges.sort_unstable();
        for &(a, b) in edges.iter() {
            scale.point(a, out);
            scale.point(b, out);
        }
    } else {
        let a = rotation(points, false);
        let b = rotation(points, true);
        let reverse = (0..points.len())
            .map(|i| point_at(points, b + i, true))
            .cmp((0..points.len()).map(|i| point_at(points, a + i, false)))
            .is_lt();
        let start = if reverse { b } else { a };
        for i in 0..points.len() {
            scale.point(point_at(points, start + i, reverse), out);
        }
    }
    true
}
fn digest(bytes: &[u8]) -> (u64, u64) {
    // a fixed candidate digest, not an identity: byte equality decides
    let (mut a, mut b) = (0xcbf29ce484222325u64, 0x84222325cbf29ce4u64);
    for &v in bytes {
        a = (a ^ v as u64).wrapping_mul(0x100000001b3);
        b = (b ^ (v as u64 + 0x9e37))
            .rotate_left(11)
            .wrapping_mul(0x9e3779b185ebca87);
    }
    (a, b)
}

/// The pack's bytes, mapped read-only for the match: its reader lock is
/// held throughout (a re-pack waits), as floe-index drc-match mapped it.
struct Mapped {
    map: memmap2::Mmap,
    blocks: u64,
    block_count: u64,
    blob_end: u64,
}
impl Mapped {
    fn new(pack: &Pack) -> Result<Self> {
        let file = File::open(&pack.path)?;
        // SAFETY: a read-only mapping of the pack while its reader lock is
        // held (Pack::hold); every access below is bounds-checked.
        let map = unsafe { memmap2::Mmap::map(&file) }?;
        let (blocks, block_count, blob_end, _) = pack.block_layout();
        Ok(Self {
            map,
            blocks,
            block_count,
            blob_end,
        })
    }
    fn u64_at(&self, at: u64) -> Result<u64> {
        let at = usize::try_from(at).map_err(|_| fail("pack offset overflow"))?;
        self.map
            .get(at..at + 8)
            .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
            .ok_or_else(|| fail("truncated pack block table"))
    }
    /// Block `bi`'s coordinate bytes and member count.
    fn block(&self, bi: u64) -> Result<(&[u8], u64)> {
        if bi >= self.block_count {
            return Err(fail("DRC block index out of range"));
        }
        let entry = self.blocks + bi * 48;
        let off = self.u64_at(entry)?;
        let count = self.u64_at(entry + 8)? & 0xffff_ffff;
        let end = if bi + 1 < self.block_count {
            self.u64_at(entry + 48)?
        } else {
            self.blob_end
        };
        if !(1..=64).contains(&count) || end < off || end > self.blob_end {
            return Err(fail("corrupt DRC block table"));
        }
        let data = self
            .map
            .get(off as usize..end as usize)
            .ok_or_else(|| fail("truncated coordinate block"))?;
        Ok((data, count))
    }
}

fn varint(b: &[u8], p: &mut usize) -> Result<u64> {
    let mut out = 0;
    for shift in (0..70).step_by(7) {
        let v = *b.get(*p).ok_or_else(|| fail("truncated coordinate varint"))?;
        *p += 1;
        if shift == 63 && v > 1 {
            return Err(fail("coordinate varint overflow"));
        }
        out |= u64::from(v & 127) << shift;
        if v < 128 {
            if shift > 0 && v == 0 {
                return Err(fail("noncanonical coordinate varint"));
            }
            return Ok(out);
        }
    }
    Err(fail("coordinate varint overflow"))
}
fn delta(b: &[u8], p: &mut usize, x: i64) -> Result<i64> {
    let v = varint(b, p)?;
    x.checked_add(((v >> 1) as i64) ^ -((v & 1) as i64))
        .ok_or_else(|| fail("coordinate delta overflow"))
}

/// One rule of a pack.
struct Rule<'a> {
    pack: &'a Mapped,
    errors: u64,
    /// the rule's first global error (the states file is global)
    start: u64,
    first_block: u64,
    scale: Scale,
}
impl Rule<'_> {
    fn block_count(&self) -> u64 {
        self.errors.div_ceil(BLOCK)
    }
    /// `visit(local, kind, points)` for the members of block `block` up to
    /// `only` (all when None); `points` is None for a record over `limit`.
    fn decode(
        &self,
        block: u64,
        limit: usize,
        only: Option<u64>,
        points: &mut Vec<Point>,
        visit: &mut dyn FnMut(u64, u8, Option<&mut Vec<Point>>) -> Result<()>,
    ) -> Result<()> {
        if block >= self.block_count() {
            return Err(fail("DRC block index out of range"));
        }
        let (data, count) = self.pack.block(self.first_block + block)?;
        let expected = (self.errors - block * BLOCK).min(BLOCK);
        if count != expected {
            return Err(fail("block count changed"));
        }
        let (mut pos, mut first) = (0usize, (0i64, 0i64));
        for j in 0..count {
            let kind = varint(data, &mut pos)?;
            let n = usize::try_from(kind >> 1).map_err(|_| fail("coordinate count overflow"))?;
            if n == 0 {
                return Err(fail("empty geometry"));
            }
            if n > (data.len() - pos) / 2 {
                return Err(fail("point count exceeds block bytes"));
            }
            let wanted = only.is_none_or(|w| w == j);
            let collect = wanted && n <= limit;
            points.clear();
            let mut p = (delta(data, &mut pos, first.0)?, delta(data, &mut pos, first.1)?);
            first = p;
            if collect {
                points.push(p);
            }
            for _ in 1..n {
                p = (delta(data, &mut pos, p.0)?, delta(data, &mut pos, p.1)?);
                if collect {
                    points.push(p);
                }
            }
            if wanted {
                visit(j, (kind & 1) as u8, if collect { Some(points) } else { None })?;
            }
            if only == Some(j) {
                return Ok(());
            }
        }
        if pos != data.len() {
            return Err(fail("trailing coordinate bytes"));
        }
        Ok(())
    }
    /// Error `ei`'s canonical bytes, in `scratch.bytes`.
    fn geometry<'s>(&self, ei: u64, limit: usize, scratch: &'s mut Canon) -> Result<&'s [u8]> {
        if ei >= self.errors {
            return Err(fail("candidate error index exceeds rule"));
        }
        let Canon { points, bytes, edges } = scratch;
        let mut valid = false;
        self.decode(ei / BLOCK, limit, Some(ei % BLOCK), points, &mut |_, kind, pts| {
            if let Some(pts) = pts {
                valid = canonical(kind, pts, self.scale, bytes, edges);
            }
            Ok(())
        })?;
        if !valid {
            return Err(fail("candidate geometry does not validate"));
        }
        Ok(&scratch.bytes)
    }
}

/// Reused buffers of a canonical form.
#[derive(Default)]
struct Canon {
    points: Vec<Point>,
    bytes: Vec<u8>,
    edges: Vec<(Point, Point)>,
}
impl Rule<'_> {
}

/// Progress as (phase, processed, total), at most every half second.
struct Progress<'a> {
    state: Mutex<(Instant, &'a mut (dyn FnMut(&str, u64, u64) + Send))>,
    cancelled: &'a AtomicUsize,
}
impl Progress<'_> {
    fn send(&self, phase: &str, processed: u64, total: u64, force: bool) -> Result<()> {
        check_cancelled(self.cancelled)?;
        let mut state = self.state.lock().map_err(|_| fail("progress lock poisoned"))?;
        if !force && state.0.elapsed() < Duration::from_millis(500) {
            return Ok(());
        }
        state.0 = Instant::now();
        (state.1)(phase, processed, total);
        Ok(())
    }
}
struct Scratch(PathBuf);
impl Scratch {
    fn new(parent: &Path) -> Result<Self> {
        std::fs::create_dir_all(parent)
            .map_err(|e| fail(format!("create migration work directory: {e}")))?;
        let epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| fail("invalid system clock"))?
            .as_nanos();
        let path = parent.join(format!("drc-match-{}-{epoch}", std::process::id()));
        std::fs::create_dir(&path)
            .map_err(|e| fail(format!("create migration scratch {}: {e}", path.display())))?;
        Ok(Self(path))
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn save_run(records: &mut Vec<Record>, path: &Path) -> Result<()> {
    records.sort_unstable();
    let mut out = BufWriter::new(create(path)?);
    for &record in records.iter() {
        record.write(&mut out)?;
    }
    out.flush().map_err(|e| fail(format!("flush sorted run: {e}")))?;
    records.clear();
    Ok(())
}
struct Stream {
    input: BufReader<File>,
    head: Option<Record>,
    offset: u64,
}
impl Stream {
    fn new(path: &Path) -> Result<Self> {
        let file = File::open(path).map_err(|e| fail(format!("open sorted run: {e}")))?;
        let mut input = BufReader::with_capacity(32768, file);
        let head = Record::read(&mut input)?;
        Ok(Self {
            input,
            head,
            offset: 0,
        })
    }
    fn pop(&mut self) -> Result<Option<Record>> {
        let old = self.head;
        if old.is_some() {
            self.head = Record::read(&mut self.input)?;
            self.offset += RECORD_BYTES as u64;
        }
        Ok(old)
    }
    fn seek(&mut self, offset: u64) -> Result<()> {
        self.input
            .seek(SeekFrom::Start(offset))
            .map_err(|e| fail(format!("seek sorted run: {e}")))?;
        self.offset = offset;
        self.head = Record::read(&mut self.input)?;
        Ok(())
    }
    fn records(&self) -> Result<u64> {
        Ok(self.input.get_ref().metadata()?.len() / RECORD_BYTES as u64)
    }
}
fn merge(paths: &[PathBuf], output: &Path, progress: &Progress) -> Result<()> {
    let mut streams = paths.iter().map(|p| Stream::new(p)).collect::<Result<Vec<_>>>()?;
    let mut heap = BinaryHeap::new();
    for (i, stream) in streams.iter().enumerate() {
        if let Some(row) = stream.head {
            heap.push(Reverse((row, i)));
        }
    }
    let mut out = BufWriter::new(create(output)?);
    let total: u64 = streams.iter().map(Stream::records).sum::<Result<u64>>()?;
    let mut done = 0;
    while let Some(Reverse((row, i))) = heap.pop() {
        row.write(&mut out)?;
        streams[i].pop()?;
        if let Some(row) = streams[i].head {
            heap.push(Reverse((row, i)));
        }
        done += 1;
        if done & 4095 == 0 {
            progress.send("merge-runs", done, total, false)?;
        }
    }
    out.flush().map_err(|e| fail(format!("flush merged run: {e}")))
}
fn collapse(mut paths: Vec<PathBuf>, directory: &Path, prefix: &str, progress: &Progress) -> Result<PathBuf> {
    if paths.is_empty() {
        let out = directory.join(format!("{prefix}-empty"));
        create(&out)?;
        return Ok(out);
    }
    let mut level = 0;
    while paths.len() > 1 {
        let mut next = Vec::new();
        for (i, group) in paths.chunks(FANIN).enumerate() {
            let out = directory.join(format!("{prefix}-merge-{level}-{i}"));
            merge(group, &out, progress)?;
            for path in group {
                std::fs::remove_file(path).map_err(|e| fail(format!("remove merged run: {e}")))?;
            }
            next.push(out);
        }
        paths = next;
        level += 1;
    }
    Ok(paths.pop().unwrap())
}

struct Indexed {
    path: PathBuf,
    invalid: u64,
    work: u64,
}
#[allow(clippy::too_many_arguments)]
fn index(
    rule: &Rule,
    states: Option<&[u8]>,
    directory: &Path,
    prefix: &str,
    jobs: usize,
    budget: usize,
    point_limit: usize,
    progress: &Progress,
) -> Result<Indexed> {
    let blocks = rule.block_count();
    let jobs = jobs.min(blocks.max(1) as usize);
    let capacity = (budget / jobs / 2 / RECORD_BYTES).max(BLOCK as usize);
    let next = AtomicU64::new(0);
    let done = AtomicU64::new(0);
    let invalid = AtomicU64::new(0);
    let work = AtomicU64::new(0);
    let failed = AtomicBool::new(false);
    progress.send(prefix, 0, rule.errors, true)?;
    let runs = std::thread::scope(|scope| -> Result<Vec<PathBuf>> {
        let mut workers = Vec::new();
        for worker in 0..jobs {
            let (next, done, invalid, work, failed) = (&next, &done, &invalid, &work, &failed);
            workers.push(scope.spawn(move || -> Result<Vec<PathBuf>> {
                let result = (|| {
                    let mut rows = Vec::with_capacity(capacity);
                    let (mut points, mut bytes, mut edges) = (Vec::new(), Vec::new(), Vec::new());
                    let mut paths = Vec::new();
                    let mut serial = 0;
                    loop {
                        if failed.load(Ordering::Relaxed) {
                            break;
                        }
                        let block = next.fetch_add(1, Ordering::Relaxed);
                        if block >= blocks {
                            break;
                        }
                        let mut count = 0u64;
                        rule.decode(block, point_limit, None, &mut points, &mut |local, kind, pts| {
                            count += 1;
                            let ei = block * BLOCK + local;
                            if let Some(states) = states {
                                let flags = *states
                                    .get((rule.start + ei) as usize)
                                    .ok_or_else(|| fail("source flags index out of range"))?;
                                if flags > 7 {
                                    return Err(Error::input("source flags contain unknown bits"));
                                }
                                if flags != 0 {
                                    work.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                            let valid = match pts {
                                Some(pts) => canonical(kind, pts, rule.scale, &mut bytes, &mut edges),
                                None => false,
                            };
                            if valid {
                                let (a, b) = digest(&bytes);
                                rows.push(Record { a, b, ei });
                            } else {
                                invalid.fetch_add(1, Ordering::Relaxed);
                            }
                            if rows.len() >= capacity {
                                let path = directory.join(format!("{prefix}-{worker}-{serial}"));
                                serial += 1;
                                save_run(&mut rows, &path)?;
                                paths.push(path);
                            }
                            Ok(())
                        })?;
                        let done = done.fetch_add(count, Ordering::Relaxed) + count;
                        progress.send(prefix, done, rule.errors, false)?;
                    }
                    if !rows.is_empty() {
                        let path = directory.join(format!("{prefix}-{worker}-{serial}"));
                        save_run(&mut rows, &path)?;
                        paths.push(path);
                    }
                    Ok(paths)
                })();
                if result.is_err() {
                    failed.store(true, Ordering::Relaxed);
                }
                result
            }));
        }
        let mut runs = Vec::new();
        let mut error = None;
        for thread in workers {
            match thread.join() {
                Ok(Ok(paths)) => runs.extend(paths),
                Ok(Err(e)) => {
                    error.get_or_insert(e);
                }
                Err(_) => {
                    error.get_or_insert(fail("geometry indexing worker panicked"));
                }
            }
        }
        match error {
            Some(e) => Err(e),
            None => Ok(runs),
        }
    })?;
    progress.send(prefix, rule.errors, rule.errors, true)?;
    let path = collapse(runs, directory, &format!("{prefix}-sorted"), progress)?;
    Ok(Indexed {
        path,
        invalid: invalid.load(Ordering::Relaxed),
        work: work.load(Ordering::Relaxed),
    })
}

/// What a match found.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Stats {
    pub source_work: u64,
    pub matched_target: u64,
    pub ambiguous_target: u64,
    pub matched_source_work: u64,
    pub ambiguous_source_work: u64,
    pub partial_target: u64,
    pub partial_source_work: u64,
    pub invalid_old: u64,
    pub invalid_new: u64,
    pub hash_collision_groups: u64,
}
impl Stats {
    pub fn unmatched_source_work(&self) -> u64 {
        self.source_work - self.matched_source_work - (self.ambiguous_source_work - self.partial_source_work)
    }
    /// The branch's report line (protocol 2).
    pub fn json(&self) -> String {
        format!(
            "{{\"protocol\":2,\"source_work\":{},\"matched_target\":{},\"ambiguous_target\":{},\"matched_source_work\":{},\"ambiguous_source_work\":{},\"partial_target\":{},\"partial_source_work\":{},\"unmatched_source_work\":{},\"invalid_old\":{},\"invalid_new\":{},\"hash_collision_groups\":{}}}\n",
            self.source_work,
            self.matched_target,
            self.ambiguous_target,
            self.matched_source_work,
            self.ambiguous_source_work,
            self.partial_target,
            self.partial_source_work,
            self.unmatched_source_work(),
            self.invalid_old,
            self.invalid_new,
            self.hash_collision_groups
        )
    }
}
struct OutputRuns {
    rows: Vec<Record>,
    paths: Vec<PathBuf>,
    capacity: usize,
    directory: PathBuf,
}
impl OutputRuns {
    fn new(directory: &Path, budget: usize) -> Self {
        let capacity = (budget / 4 / RECORD_BYTES).max(BLOCK as usize);
        Self {
            rows: Vec::with_capacity(capacity),
            paths: Vec::new(),
            capacity,
            directory: directory.to_owned(),
        }
    }
    fn push(&mut self, ci: usize, ei: u64, flags: u8, collision: bool, unknown: u8) -> Result<()> {
        self.rows.push(Record {
            a: ci as u64,
            b: ei,
            ei: flags as u64 | (u64::from(collision) << 8) | ((unknown as u64) << 16),
        });
        if self.rows.len() >= self.capacity {
            self.flush()?;
        }
        Ok(())
    }
    fn flush(&mut self) -> Result<()> {
        if self.rows.is_empty() {
            return Ok(());
        }
        let path = self.directory.join(format!("output-run-{}", self.paths.len()));
        save_run(&mut self.rows, &path)?;
        self.paths.push(path);
        Ok(())
    }
    fn finish(mut self, path: &Path, progress: &Progress) -> Result<()> {
        self.flush()?;
        let sorted = collapse(std::mem::take(&mut self.paths), &self.directory, "output", progress)?;
        let mut input = Stream::new(&sorted)?;
        let mut out = BufWriter::new(create(path)?);
        let total = input.records()?;
        let mut done = 0;
        while let Some(row) = input.pop()? {
            let mut bytes = [0u8; 16];
            bytes[..4].copy_from_slice(&(row.a as u32).to_le_bytes());
            bytes[4] = row.ei as u8;
            bytes[5] = (row.ei >> 8) as u8;
            bytes[6] = (row.ei >> 16) as u8;
            bytes[8..].copy_from_slice(&row.b.to_le_bytes());
            out.write_all(&bytes)
                .map_err(|e| fail(format!("write migration output: {e}")))?;
            done += 1;
            progress.send("sort-output", done, total, false)?;
        }
        out.flush().map_err(|e| fail(format!("flush migration output: {e}")))?;
        progress.send("done", done, total, true)
    }
}

/// A hash group with review work, held for a batched verification.
struct Group {
    old: Vec<u64>,
    new: Vec<u64>,
    any: u8,
    all: u8,
    work: u64,
}

/// Whether each group's members differ (a hash collision): its old
/// occurrences against the first, its targets against it - on `jobs`
/// threads, a part of the batch each.
fn verify(old: &Rule, new: &Rule, groups: &[Group], point_limit: usize, jobs: usize) -> Result<Vec<bool>> {
    if groups.is_empty() {
        return Ok(Vec::new());
    }
    let part = groups.len().div_ceil(jobs.max(1)).max(1);
    std::thread::scope(|scope| {
        let workers: Vec<_> = groups
            .chunks(part)
            .map(|slice| {
                scope.spawn(move || -> Result<Vec<bool>> {
                    let (mut a, mut b) = (Canon::default(), Canon::default());
                    slice
                        .iter()
                        .map(|g| {
                            let mut representative: Option<Vec<u8>> = None;
                            let mut collision = false;
                            for &ei in &g.old {
                                let c = old.geometry(ei, point_limit, &mut a)?;
                                match &representative {
                                    Some(r) => collision |= r.as_slice() != c,
                                    None => representative = Some(c.to_vec()),
                                }
                            }
                            for &ei in &g.new {
                                let c = new.geometry(ei, point_limit, &mut b)?;
                                collision |= representative.as_deref() != Some(c);
                            }
                            Ok(collision)
                        })
                        .collect()
                })
            })
            .collect();
        let mut out = Vec::with_capacity(groups.len());
        for w in workers {
            out.extend(w.join().map_err(|_| fail("verification worker panicked"))??);
        }
        Ok(out)
    })
}

/// The decision of one verified group, its targets written.
#[allow(clippy::too_many_arguments)]
fn decide(
    any: u8,
    all: u8,
    work: u64,
    collision: bool,
    targets: &mut dyn Iterator<Item = Result<u64>>,
    new_ci: usize,
    output: &mut OutputRuns,
    stats: &mut Stats,
    processed: &mut u64,
) -> Result<()> {
    // V: any exact old occurrence was seen; W and NR are separate
    // votes of every duplicate, a disagreement blocks that field only
    let unknown = if collision { 0 } else { (any ^ all) & 6 };
    let flags = if collision { 0 } else { (any & 1) | (all & 6) };
    let partial = unknown != 0;
    let ambiguous = partial || collision;
    if collision {
        stats.hash_collision_groups += 1;
    }
    if ambiguous {
        stats.ambiguous_source_work += work;
    }
    if !collision {
        stats.matched_source_work += work;
    }
    if partial {
        stats.partial_source_work += work;
    }
    for ei in targets {
        output.push(new_ci, ei?, flags, collision, unknown)?;
        if ambiguous {
            stats.ambiguous_target += 1;
        }
        if !collision {
            stats.matched_target += 1;
        }
        if partial {
            stats.partial_target += 1;
        }
        *processed += 1;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn match_rule(
    old: &Rule,
    new: &Rule,
    old_sorted: &Path,
    new_sorted: &Path,
    states: &[u8],
    new_ci: usize,
    point_limit: usize,
    jobs: usize,
    output: &mut OutputRuns,
    stats: &mut Stats,
    progress: &Progress,
) -> Result<()> {
    const BATCH_RECORDS: usize = 1 << 16;
    let mut left = Stream::new(old_sorted)?;
    let mut right = Stream::new(new_sorted)?;
    let mut processed = 0;
    let mut batch: Vec<Group> = Vec::new();
    let mut batch_records = 0;
    let flush = |batch: &mut Vec<Group>,
                 output: &mut OutputRuns,
                 stats: &mut Stats,
                 processed: &mut u64|
     -> Result<()> {
        let collisions = verify(old, new, batch, point_limit, jobs)?;
        for (g, collision) in batch.drain(..).zip(collisions) {
            let mut targets = g.new.iter().map(|&e| Ok(e));
            decide(g.any, g.all, g.work, collision, &mut targets, new_ci, output, stats, processed)?;
        }
        progress.send("match", *processed, new.errors, false)
    };
    while let Some(first) = left.head {
        let key = first.key();
        while right.head.is_some_and(|row| row.key() < key) {
            right.pop()?;
            processed += 1;
            if processed & 4095 == 0 {
                progress.send("match", processed, new.errors, false)?;
            }
        }
        let (mut any, mut all, mut work) = (0u8, 6u8, 0u64);
        let has_targets = right.head.is_some_and(|row| row.key() == key);
        let old_begin = left.offset;
        // a small group is verified from memory in a batch; a huge one
        // replays its runs (bounded however many duplicates)
        let mut old_rows = Vec::new();
        let mut old_spilled = false;
        while left.head.is_some_and(|row| row.key() == key) {
            let row = left.pop()?.unwrap();
            if !old_spilled {
                if old_rows.len() < GROUP_BUFFER {
                    old_rows.push(row.ei);
                } else {
                    old_spilled = true;
                    old_rows = Vec::new();
                }
            }
            let value = states[(old.start + row.ei) as usize];
            if value != 0 {
                work += 1;
            }
            any |= value;
            all &= value;
            if left.offset / RECORD_BYTES as u64 & 4095 == 0 {
                progress.send("scan-source", left.offset / RECORD_BYTES as u64, old.errors, false)?;
            }
        }
        if !has_targets {
            continue;
        }
        if work == 0 {
            while right.head.is_some_and(|row| row.key() == key) {
                right.pop()?;
                processed += 1;
                if processed & 4095 == 0 {
                    progress.send("match", processed, new.errors, false)?;
                }
            }
            continue;
        }
        let begin = right.offset;
        let mut new_rows = Vec::new();
        let mut new_spilled = false;
        while right.head.is_some_and(|row| row.key() == key) {
            let row = right.pop()?.unwrap();
            if !new_spilled {
                if new_rows.len() < GROUP_BUFFER {
                    new_rows.push(row.ei);
                } else {
                    new_spilled = true;
                    new_rows = Vec::new();
                }
            }
        }
        if !old_spilled && !new_spilled {
            batch_records += old_rows.len() + new_rows.len();
            batch.push(Group {
                old: old_rows,
                new: new_rows,
                any,
                all,
                work,
            });
            if batch_records >= BATCH_RECORDS {
                flush(&mut batch, output, stats, &mut processed)?;
                batch_records = 0;
            }
            continue;
        }
        // a huge group, on its own: every old occurrence, untouched ones
        // too, then every target, replayed from the runs
        let (mut a, mut b) = (Canon::default(), Canon::default());
        let mut representative: Option<Vec<u8>> = None;
        let mut collision = false;
        left.seek(old_begin)?;
        while left.head.is_some_and(|row| row.key() == key) {
            let row = left.pop()?.unwrap();
            let c = old.geometry(row.ei, point_limit, &mut a)?;
            match &representative {
                Some(r) => collision |= r.as_slice() != c,
                None => representative = Some(c.to_vec()),
            }
            progress.send("verify-source", left.offset / RECORD_BYTES as u64, old.errors, false)?;
        }
        right.seek(begin)?;
        while right.head.is_some_and(|row| row.key() == key) {
            let row = right.pop()?.unwrap();
            let c = new.geometry(row.ei, point_limit, &mut b)?;
            collision |= representative.as_deref() != Some(c);
            progress.send("verify-target", right.offset / RECORD_BYTES as u64, new.errors, false)?;
        }
        // replay the target group after verification: no Cartesian product
        right.seek(begin)?;
        let mut targets = std::iter::from_fn(|| match right.head {
            Some(row) if row.key() == key => Some(right.pop().map(|r| r.unwrap().ei)),
            _ => None,
        });
        decide(any, all, work, collision, &mut targets, new_ci, output, stats, &mut processed)?;
    }
    flush(&mut batch, output, stats, &mut processed)?;
    progress.send("match", new.errors, new.errors, true)
}

/// How a match runs.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub jobs: usize,
    pub memory_mib: usize,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            jobs: std::thread::available_parallelism().map_or(1, |n| n.get()).min(4),
            memory_mib: 64,
        }
    }
}

/// Match rule pairs `pairs` (old, new) of the packs `old` and `new`;
/// `states` = the old run's flags, one byte per global error (V=1, W=2,
/// NR=4). Writes the records to `out` (a fresh file) using a scratch
/// folder under `work_dir`; `progress(phase, processed, total)`.
#[allow(clippy::too_many_arguments)]
pub fn match_packs(
    old: &Pack,
    new: &Pack,
    pairs: &[(usize, usize)],
    states: &[u8],
    out: &Path,
    work_dir: &Path,
    options: Options,
    cancelled: &AtomicUsize,
    progress: &mut (dyn FnMut(&str, u64, u64) + Send),
) -> Result<Stats> {
    if options.jobs == 0 || options.jobs > 256 || !(4..=65536).contains(&options.memory_mib) {
        return Err(Error::input("jobs must be 1..256 and memory-mib 4..65536"));
    }
    if states.len() as u64 != old.total {
        return Err(Error::input("source flags length does not match old pack"));
    }
    // every rule and name checked before any output
    let unique = |p: &Pack, name: &str| p.checks.iter().filter(|c| c.name == name).count() == 1;
    for &(a, b) in pairs {
        let (Some(x), Some(y)) = (old.checks.get(a), new.checks.get(b)) else {
            return Err(Error::input("migration rule index is out of range"));
        };
        if x.name != y.name || !unique(old, &x.name) || !unique(new, &y.name) {
            return Err(Error::input("migration rules must have the same unique exact name"));
        }
    }
    let (old_map, new_map) = (Mapped::new(old)?, Mapped::new(new)?);
    let (old_first, new_first) = (old.block_layout().3, new.block_layout().3);
    fn rule<'b>(p: &Pack, map: &'b Mapped, first: &[u64], ci: usize) -> Result<Rule<'b>> {
        let c = &p.checks[ci];
        Ok(Rule {
            pack: map,
            errors: c.count,
            start: c.start,
            first_block: first[ci],
            scale: Scale::new(p.precision)?,
        })
    }
    let budget = options.memory_mib * 1024 * 1024;
    let jobs = options.jobs.min((budget / (1024 * 1024)).max(1));
    let point_limit = (budget / jobs / 4 / 128).max(1024);
    let scratch = Scratch::new(work_dir)?;
    let progress = Progress {
        state: Mutex::new((Instant::now() - Duration::from_secs(1), progress)),
        cancelled,
    };
    let mut stats = Stats::default();
    // output runs take at most a quarter of the memory
    let mut output = OutputRuns::new(&scratch.0, budget);
    for (ordinal, &(a, b)) in pairs.iter().enumerate() {
        let o = rule(old, &old_map, &old_first, a)?;
        let n = rule(new, &new_map, &new_first, b)?;
        let dir = scratch.0.join(format!("pair-{ordinal}"));
        std::fs::create_dir(&dir)?;
        let left = index(&o, Some(states), &dir, "hash-old", jobs, budget * 3 / 4, point_limit, &progress)?;
        let right = index(&n, None, &dir, "hash-new", jobs, budget * 3 / 4, point_limit, &progress)?;
        stats.source_work += left.work;
        stats.invalid_old += left.invalid;
        stats.invalid_new += right.invalid;
        match_rule(&o, &n, &left.path, &right.path, states, b, point_limit, jobs, &mut output, &mut stats, &progress)?;
        std::fs::remove_dir_all(&dir).map_err(|e| fail(format!("remove rule scratch: {e}")))?;
    }
    output.finish(out, &progress)?;
    Ok(stats)
}

/// One output row: (rule, error, flags, disposition, unknown).
pub type Row = (u32, u64, u8, u8, u8);

/// Parse a records file's 16-byte rows.
pub fn rows(bytes: &[u8]) -> Result<Vec<Row>> {
    if bytes.len() % 16 != 0 {
        return Err(Error::input("truncated geometry match records"));
    }
    bytes
        .chunks_exact(16)
        .map(|r| {
            if r[7] != 0 {
                return Err(Error::input("invalid geometry match record"));
            }
            Ok((
                u32::from_le_bytes(r[..4].try_into().unwrap()),
                u64::from_le_bytes(r[8..].try_into().unwrap()),
                r[4],
                r[5],
                r[6],
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canon(kind: u8, points: &[Point], precision: f64) -> Vec<u8> {
        let mut out = Vec::new();
        assert!(canonical(kind, &mut points.to_vec(), Scale::new(precision).unwrap(), &mut out, &mut Vec::new()));
        out
    }

    #[test]
    fn polygon_cycle_reverse_closure_and_exact_precision() {
        let polygon = [(-9, -7), (11, -7), (11, 15), (-9, 15)];
        let expected = canon(0, &polygon, 1000.0);
        for start in 0..4 {
            let mut points = (0..4).map(|i| polygon[(start + i) % 4]).collect::<Vec<_>>();
            assert_eq!(canon(0, &points, 1000.0), expected);
            points.reverse();
            points.push(points[0]);
            assert_eq!(canon(0, &points, 1000.0), expected);
            let scaled = points.iter().map(|&(x, y)| (x * 3, y * 3)).collect::<Vec<_>>();
            assert_eq!(canon(0, &scaled, 3000.0), expected);
        }
        assert_ne!(canon(0, &polygon, 1000.0000000001), expected);
        assert_ne!(canon(0, &[(-9, -7), (0, -7), (11, -7), (11, 15), (-9, 15)], 1000.0), expected);
    }

    #[test]
    fn edge_endpoint_pairing_is_preserved_but_order_is_irrelevant() {
        let shape = [(-9, -7), (11, -7), (11, 15), (-9, 15)];
        let expected = canon(1, &shape, 1000.0);
        assert_eq!(canon(1, &[shape[3], shape[2], shape[1], shape[0]], 1000.0), expected);
        assert_ne!(canon(1, &[shape[0], shape[2], shape[1], shape[3]], 1000.0), expected);
        assert_ne!(canon(0, &shape, 1000.0), expected);
    }

    #[test]
    fn rational_coordinates_do_not_round_large_or_extreme_values() {
        assert_ne!(canon(0, &[(1 << 60, 0)], 1000.0), canon(0, &[((1 << 60) + 1, 0)], 1000.0));
        assert_eq!(canon(0, &[(i64::MIN, 0)], 2.0), canon(0, &[(i64::MIN / 2, 0)], 1.0));
        assert_eq!(canon(0, &[(3, -9)], f64::from_bits(1)), canon(0, &[(6, -18)], f64::from_bits(2)));
        assert_ne!(canon(0, &[(1, 0)], f64::MAX), canon(0, &[(2, 0)], f64::MAX));
        assert_eq!(canon(0, &[(0, 0)], f64::MAX), canon(0, &[(0, 0)], f64::from_bits(1)));
    }

    #[test]
    fn strict_pair_plan_rejects_duplicate_or_invalid_indices() {
        assert_eq!(parse_pairs(b" [ [ 0, 2 ],[1,0] ] ").unwrap(), vec![(0, 2), (1, 0)]);
        assert!(parse_pairs(b"[]").unwrap().is_empty());
        for bad in ["[[0,1],[0,2]]", "[[0,1],[2,1]]", "[[01,2]]", "[[-1,2]]", "[[0,2],]", "[[0,4294967296]]", "[[0,1]]x"] {
            assert!(parse_pairs(bad.as_bytes()).is_err(), "{bad}");
        }
    }

    #[test]
    fn bounded_fanin_merge_sorts() {
        let dir = std::env::temp_dir().join(format!("floe-match-merge-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut sink = |_: &str, _: u64, _: u64| {};
        let stop = AtomicUsize::new(0);
        let progress = Progress {
            state: Mutex::new((Instant::now(), &mut sink)),
            cancelled: &stop,
        };
        let mut paths = Vec::new();
        for index in 0..FANIN + 3 {
            let path = dir.join(format!("run-{index}"));
            save_run(
                &mut vec![Record { a: 1, b: index as u64, ei: 2 }, Record { a: 0, b: index as u64, ei: 1 }],
                &path,
            )
            .unwrap();
            paths.push(path);
        }
        let sorted = collapse(paths, &dir, "test", &progress).unwrap();
        let mut input = Stream::new(&sorted).unwrap();
        let mut actual = Vec::new();
        while let Some(row) = input.pop().unwrap() {
            actual.push(row);
        }
        assert_eq!(actual.len(), 2 * (FANIN + 3));
        assert!(actual.windows(2).all(|r| r[0] <= r[1]));
        let mut output = OutputRuns::new(&dir, 4096);
        output.capacity = 1;
        for ci in (0..20).rev() {
            output.push(ci, 7, 5, false, 2).unwrap();
            output.push(ci, 1, 0, true, 0).unwrap();
        }
        let path = dir.join("final");
        output.finish(&path, &progress).unwrap();
        let actual = rows(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(actual.len(), 40);
        assert!(actual.windows(2).all(|r| (r[0].0, r[0].1) < (r[1].0, r[1].1)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
