//! Exact, bounded-memory review migration between immutable v4 DRC packs.
//! Hashes only locate candidates. Every candidate group is checked against
//! canonical geometry before any judgement is emitted. All old duplicates,
//! including untouched 000 states, vote independently on W and NR. Visits
//! are unioned. Disagreeing judgement fields remain unknown for the caller
//! to preserve, without discarding consensus on the other field.
//! Protocol 2 output is sorted (target rule, error): little-endian u32 rule,
//! u8 flags (V=1,W=2,NR=4), u8 disposition (0=exact,1=hash collision), u8
//! unknown judgement mask (W=2,NR=4), u8 zero, u64 error. Collision rows
//! always have zero flags and unknown mask. Scratch records are 24
//! bytes and merge fan-in is bounded. Oversized individual geometries are
//! counted as invalid instead of exceeding the configured working budget.

use crate::drcmeasure::{add_coord, size, varint, Pack};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const BLOCK: usize = 64;
const FANIN: usize = 16;
const RECORD_BYTES: usize = 24;
type Point = (i64, i64);

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
    fn write(self, out: &mut impl Write) -> Result<(), String> {
        out.write_all(&self.a.to_le_bytes())
            .and_then(|_| out.write_all(&self.b.to_le_bytes()))
            .and_then(|_| out.write_all(&self.ei.to_le_bytes()))
            .map_err(|e| format!("write sort record: {e}"))
    }
    fn read(input: &mut impl Read) -> Result<Option<Self>, String> {
        let mut data = [0u8; RECORD_BYTES];
        match input.read(&mut data[..1]) {
            Ok(0) => return Ok(None),
            Ok(_) => {}
            Err(e) => return Err(format!("read sort record: {e}")),
        }
        input
            .read_exact(&mut data[1..])
            .map_err(|e| format!("truncated sort record: {e}"))?;
        Ok(Some(Self {
            a: u64::from_le_bytes(data[..8].try_into().unwrap()),
            b: u64::from_le_bytes(data[8..16].try_into().unwrap()),
            ei: u64::from_le_bytes(data[16..].try_into().unwrap()),
        }))
    }
}

fn create(path: &Path) -> Result<File, String> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("create {}: {e}", path.display()))
}
fn map(path: &Path) -> Result<memmap2::Mmap, String> {
    let file = File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    unsafe { memmap2::Mmap::map(&file) }.map_err(|e| format!("map {}: {e}", path.display()))
}
fn u64_at(data: &[u8], at: usize) -> Result<u64, String> {
    Ok(u64::from_le_bytes(
        data.get(at..at.checked_add(8).ok_or("offset overflow")?)
            .ok_or("truncated pack metadata")?
            .try_into()
            .unwrap(),
    ))
}
fn u32_at(data: &[u8], at: usize) -> Result<u32, String> {
    Ok(u32::from_le_bytes(
        data.get(at..at.checked_add(4).ok_or("offset overflow")?)
            .ok_or("truncated pack metadata")?
            .try_into()
            .unwrap(),
    ))
}
fn footer(data: &[u8]) -> Result<usize, String> {
    if data.len() < 176
        || data.get(..8) != Some(b"FLOEICE\0")
        || u32_at(data, 8)? != 4
        || u32_at(data, 12)? != 1
        || data.get(data.len() - 8..) != Some(b"FLOEICE\0")
    {
        return Err("exact migration requires a valid version 4 DRC pack".into());
    }
    Ok(data.len() - 136)
}
fn rule_name(data: &[u8], ci: usize) -> Result<&[u8], String> {
    let f = footer(data)?;
    let count = size(u64_at(data, f + 9 * 8)?)?;
    if ci >= count {
        return Err("migration rule index is out of range".into());
    }
    let directory = size(u64_at(data, f + 8 * 8)?)?;
    let strings = size(u64_at(data, f + 12 * 8)?)?;
    let reference = u32_at(
        data,
        directory
            .checked_add(ci.checked_mul(64).ok_or("rule offset overflow")?)
            .ok_or("rule offset overflow")?,
    )? as usize;
    let start = strings
        .checked_add(reference)
        .ok_or("string offset overflow")?;
    let length = u32_at(data, start)? as usize;
    data.get(
        start + 4
            ..(start + 4)
                .checked_add(length)
                .ok_or("string length overflow")?,
    )
    .ok_or_else(|| "invalid rule name".into())
}
fn unique_name(data: &[u8], name: &[u8]) -> Result<bool, String> {
    let count = size(u64_at(data, footer(data)? + 9 * 8)?)?;
    let mut found = 0;
    for ci in 0..count {
        if rule_name(data, ci)? == name {
            found += 1;
        }
    }
    Ok(found == 1)
}

/// A deliberately small JSON grammar: [[old_ci,new_ci], ...].
fn parse_pairs(input: &[u8]) -> Result<Vec<(usize, usize)>, String> {
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
        fn token(&mut self, byte: u8) -> Result<(), String> {
            self.ws();
            if self.data.get(self.at) != Some(&byte) {
                return Err("plan must be JSON [[old_ci,new_ci], ...]".into());
            }
            self.at += 1;
            Ok(())
        }
        fn number(&mut self) -> Result<usize, String> {
            self.ws();
            let start = self.at;
            while self.data.get(self.at).is_some_and(u8::is_ascii_digit) {
                self.at += 1;
            }
            let bytes = &self.data[start..self.at];
            if bytes.is_empty() || (bytes.len() > 1 && bytes[0] == b'0') {
                return Err("invalid migration rule index".into());
            }
            let n = std::str::from_utf8(bytes)
                .unwrap()
                .parse::<u32>()
                .map_err(|_| "migration rule index overflow")?;
            Ok(n as usize)
        }
    }
    let mut p = Parser { data: input, at: 0 };
    let mut out = Vec::new();
    let mut olds = HashSet::new();
    let mut news = HashSet::new();
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
                return Err("migration plan repeats a source or target rule".into());
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
        return Err("trailing migration plan data".into());
    }
    Ok(out)
}

#[derive(Clone, Copy)]
struct Scale {
    odd: u64,
    exponent: i16,
}
impl Scale {
    fn new(precision: f64) -> Result<Self, String> {
        if !precision.is_finite() || precision <= 0.0 {
            return Err("invalid DRC precision".into());
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
        // value / precision = sign * odd_n / odd_d * 2^exponent.
        // Factored powers of two avoid all cross-product/shift overflow,
        // including i64::MIN and extreme/subnormal IEEE-754 precisions.
        let (numerator, denominator, exponent) = if value == 0 {
            (0, 1, 0)
        } else {
            let magnitude = value.unsigned_abs();
            let zeros = magnitude.trailing_zeros();
            let odd = magnitude >> zeros;
            let divisor = gcd(odd, self.odd);
            (
                odd / divisor,
                self.odd / divisor,
                zeros as i16 - self.exponent,
            )
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
    // Booth's algorithm: linear time even for repeated vertices.
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
    // Fixed candidate digest, not an identity or security claim.
    // Independent final byte equality makes collisions fail closed.
    let (mut a, mut b) = (0xcbf29ce484222325u64, 0x84222325cbf29ce4u64);
    for &v in bytes {
        a = (a ^ v as u64).wrapping_mul(0x100000001b3);
        b = (b ^ (v as u64 + 0x9e37))
            .rotate_left(11)
            .wrapping_mul(0x9e3779b185ebca87);
    }
    (a, b)
}

fn decode<F>(
    data: &[u8],
    count: usize,
    limit: usize,
    only: Option<usize>,
    points: &mut Vec<Point>,
    mut visit: F,
) -> Result<(), String>
where
    F: FnMut(usize, u8, Option<&mut Vec<Point>>) -> Result<(), String>,
{
    let (mut pos, mut pfx, mut pfy) = (0, 0, 0);
    for ei in 0..count {
        let encoded = varint(data, &mut pos)?;
        let n = size(encoded >> 1)?;
        if n > data.len().saturating_sub(pos) / 2 {
            return Err("truncated coordinate record".into());
        }
        let wanted = only.is_none_or(|which| which == ei);
        let collect = wanted && n <= limit;
        points.clear();
        if collect {
            points
                .try_reserve(n)
                .map_err(|_| "insufficient geometry memory")?;
        }
        let (mut x, mut y) = (pfx, pfy);
        for i in 0..n {
            x = add_coord(x, varint(data, &mut pos)?)?;
            y = add_coord(y, varint(data, &mut pos)?)?;
            if i == 0 {
                pfx = x;
                pfy = y;
            }
            if collect {
                points.push((x, y));
            }
        }
        if wanted {
            visit(
                ei,
                (encoded & 1) as u8,
                if collect { Some(points) } else { None },
            )?;
        }
        // Targeted verification can stop here: full blocks were validated in
        // the indexing pass, and the input packs remain immutable throughout.
        if only == Some(ei) {
            return Ok(());
        }
    }
    if pos != data.len() {
        return Err("coordinate block has trailing data".into());
    }
    Ok(())
}
fn geometry(pack: &Pack<'_>, ei: u64, limit: usize) -> Result<Vec<u8>, String> {
    let ei = size(ei)?;
    if ei >= pack.errors {
        return Err("candidate error index exceeds rule".into());
    }
    let (data, count) = pack.block(ei / BLOCK)?;
    let scale = Scale::new(pack.precision)?;
    let (mut points, mut bytes, mut edges) = (Vec::new(), Vec::new(), Vec::new());
    let mut valid = false;
    decode(
        data,
        count,
        limit,
        Some(ei % BLOCK),
        &mut points,
        |_, kind, points| {
            if let Some(points) = points {
                valid = canonical(kind, points, scale, &mut bytes, &mut edges);
            }
            Ok(())
        },
    )?;
    if !valid {
        return Err("candidate geometry does not validate".into());
    }
    Ok(bytes)
}

struct Progress {
    state: Mutex<(Option<File>, Instant)>,
}
impl Progress {
    fn new(path: Option<&Path>) -> Result<Self, String> {
        Ok(Self {
            state: Mutex::new((
                path.map(create).transpose()?,
                Instant::now() - Duration::from_secs(1),
            )),
        })
    }
    fn send(&self, phase: &str, processed: u64, total: u64, force: bool) -> Result<(), String> {
        let mut state = self.state.lock().map_err(|_| "progress lock poisoned")?;
        if !force && state.1.elapsed() < Duration::from_millis(500) {
            return Ok(());
        }
        state.1 = Instant::now();
        if let Some(file) = state.0.as_mut() {
            let text =
                format!("{{\"phase\":\"{phase}\",\"processed\":{processed},\"total\":{total}}}\n");
            file.seek(SeekFrom::Start(0))
                .and_then(|_| file.write_all(text.as_bytes()))
                .and_then(|_| file.set_len(text.len() as u64))
                .map_err(|e| format!("write migration progress: {e}"))?;
        }
        Ok(())
    }
}
struct Scratch(PathBuf);
impl Scratch {
    fn new(parent: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create migration work directory: {e}"))?;
        let epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "invalid system clock")?
            .as_nanos();
        let path = parent.join(format!("drc-match-{}-{epoch}", std::process::id()));
        std::fs::create_dir(&path)
            .map_err(|e| format!("create migration scratch {}: {e}", path.display()))?;
        Ok(Self(path))
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn save_run(records: &mut Vec<Record>, path: &Path) -> Result<(), String> {
    records.sort_unstable();
    let mut out = BufWriter::new(create(path)?);
    for &record in records.iter() {
        record.write(&mut out)?;
    }
    out.flush().map_err(|e| format!("flush sorted run: {e}"))?;
    records.clear();
    Ok(())
}
struct Stream {
    input: BufReader<File>,
    head: Option<Record>,
    offset: u64,
}
impl Stream {
    fn new(path: &Path) -> Result<Self, String> {
        let file = File::open(path).map_err(|e| format!("open sorted run: {e}"))?;
        let mut input = BufReader::with_capacity(32768, file);
        let head = Record::read(&mut input)?;
        Ok(Self {
            input,
            head,
            offset: 0,
        })
    }
    fn pop(&mut self) -> Result<Option<Record>, String> {
        let old = self.head;
        if old.is_some() {
            self.head = Record::read(&mut self.input)?;
            self.offset += RECORD_BYTES as u64;
        }
        Ok(old)
    }
    fn seek(&mut self, offset: u64) -> Result<(), String> {
        self.input
            .seek(SeekFrom::Start(offset))
            .map_err(|e| format!("seek sorted run: {e}"))?;
        self.offset = offset;
        self.head = Record::read(&mut self.input)?;
        Ok(())
    }
}
fn merge(paths: &[PathBuf], output: &Path, progress: &Progress) -> Result<(), String> {
    let mut streams = paths
        .iter()
        .map(|path| Stream::new(path))
        .collect::<Result<Vec<_>, _>>()?;
    let mut heap = BinaryHeap::new();
    for (i, stream) in streams.iter().enumerate() {
        if let Some(row) = stream.head {
            heap.push(Reverse((row, i)));
        }
    }
    let mut out = BufWriter::new(create(output)?);
    let total = streams
        .iter()
        .map(|stream| {
            stream
                .input
                .get_ref()
                .metadata()
                .map(|meta| meta.len() / RECORD_BYTES as u64)
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?
        .into_iter()
        .sum();
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
    out.flush().map_err(|e| format!("flush merged run: {e}"))
}
fn collapse(
    mut paths: Vec<PathBuf>,
    directory: &Path,
    prefix: &str,
    progress: &Progress,
) -> Result<PathBuf, String> {
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
                std::fs::remove_file(path).map_err(|e| format!("remove merged run: {e}"))?;
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
fn index(
    pack: &Pack<'_>,
    states: Option<&[u8]>,
    directory: &Path,
    prefix: &str,
    jobs: usize,
    budget: usize,
    point_limit: usize,
    progress: &Progress,
) -> Result<Indexed, String> {
    let jobs = jobs.min(pack.block_count.max(1));
    let capacity = (budget / jobs / 2 / RECORD_BYTES).max(BLOCK);
    let scale = Scale::new(pack.precision)?;
    let next = AtomicUsize::new(0);
    let done = AtomicU64::new(0);
    let invalid = AtomicU64::new(0);
    let work = AtomicU64::new(0);
    let failed = AtomicBool::new(false);
    progress.send(prefix, 0, pack.errors as u64, true)?;
    let runs = std::thread::scope(|scope| -> Result<Vec<PathBuf>, String> {
        let mut workers = Vec::new();
        for worker in 0..jobs {
            let (next, done, invalid, work, failed) = (&next, &done, &invalid, &work, &failed);
            workers.push(scope.spawn(move || -> Result<Vec<PathBuf>, String> {
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
                        if block >= pack.block_count {
                            break;
                        }
                        let (data, count) = pack.block(block)?;
                        decode(
                            data,
                            count,
                            point_limit,
                            None,
                            &mut points,
                            |local, kind, points| {
                                let ei = block * BLOCK + local;
                                if let Some(states) = states {
                                    let flags = *states
                                        .get(pack.error_start + ei)
                                        .ok_or("source flags index out of range")?;
                                    if flags > 7 {
                                        return Err("source flags contain unknown bits".into());
                                    }
                                    if flags != 0 {
                                        work.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                                if let Some(points) = points {
                                    if canonical(kind, points, scale, &mut bytes, &mut edges) {
                                        let (a, b) = digest(&bytes);
                                        rows.push(Record {
                                            a,
                                            b,
                                            ei: ei as u64,
                                        });
                                    } else {
                                        invalid.fetch_add(1, Ordering::Relaxed);
                                    }
                                } else {
                                    invalid.fetch_add(1, Ordering::Relaxed);
                                }
                                if rows.len() >= capacity {
                                    let path =
                                        directory.join(format!("{prefix}-{worker}-{serial}"));
                                    serial += 1;
                                    save_run(&mut rows, &path)?;
                                    paths.push(path);
                                }
                                Ok(())
                            },
                        )?;
                        let done = done.fetch_add(count as u64, Ordering::Relaxed) + count as u64;
                        progress.send(prefix, done, pack.errors as u64, false)?;
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
                    if error.is_none() {
                        error = Some(e);
                    }
                }
                Err(_) => {
                    if error.is_none() {
                        error = Some("geometry indexing worker panicked".into());
                    }
                }
            }
        }
        if let Some(e) = error {
            Err(e)
        } else {
            Ok(runs)
        }
    })?;
    progress.send(prefix, pack.errors as u64, pack.errors as u64, true)?;
    let path = collapse(runs, directory, &format!("{prefix}-sorted"), progress)?;
    Ok(Indexed {
        path,
        invalid: invalid.load(Ordering::Relaxed),
        work: work.load(Ordering::Relaxed),
    })
}

#[derive(Default, Debug)]
struct Stats {
    source_work: u64,
    matched_target: u64,
    ambiguous_target: u64,
    matched_source_work: u64,
    ambiguous_source_work: u64,
    partial_target: u64,
    partial_source_work: u64,
    invalid_old: u64,
    invalid_new: u64,
    hash_collision_groups: u64,
}
impl Stats {
    fn json(&self) -> String {
        format!("{{\"protocol\":2,\"source_work\":{},\"matched_target\":{},\"ambiguous_target\":{},\"matched_source_work\":{},\"ambiguous_source_work\":{},\"partial_target\":{},\"partial_source_work\":{},\"unmatched_source_work\":{},\"invalid_old\":{},\"invalid_new\":{},\"hash_collision_groups\":{}}}\n",
            self.source_work,self.matched_target,self.ambiguous_target,self.matched_source_work,
            self.ambiguous_source_work,self.partial_target,self.partial_source_work,
            self.source_work-self.matched_source_work-(self.ambiguous_source_work-self.partial_source_work),
            self.invalid_old,self.invalid_new,self.hash_collision_groups)
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
        let capacity = (budget / 4 / RECORD_BYTES).max(BLOCK);
        Self {
            rows: Vec::with_capacity(capacity),
            paths: Vec::new(),
            capacity,
            directory: directory.to_owned(),
        }
    }
    fn push(
        &mut self,
        ci: usize,
        ei: u64,
        flags: u8,
        collision: bool,
        unknown: u8,
    ) -> Result<(), String> {
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
    fn flush(&mut self) -> Result<(), String> {
        if self.rows.is_empty() {
            return Ok(());
        }
        let path = self
            .directory
            .join(format!("output-run-{}", self.paths.len()));
        save_run(&mut self.rows, &path)?;
        self.paths.push(path);
        Ok(())
    }
    fn finish(mut self, path: &Path, progress: &Progress) -> Result<(), String> {
        self.flush()?;
        let sorted = collapse(self.paths, &self.directory, "output", progress)?;
        let mut input = Stream::new(&sorted)?;
        let mut out = BufWriter::new(create(path)?);
        let total = input
            .input
            .get_ref()
            .metadata()
            .map_err(|e| e.to_string())?
            .len()
            / RECORD_BYTES as u64;
        let mut done = 0;
        while let Some(row) = input.pop()? {
            let mut bytes = [0u8; 16];
            bytes[..4].copy_from_slice(&(row.a as u32).to_le_bytes());
            bytes[4] = row.ei as u8;
            bytes[5] = (row.ei >> 8) as u8;
            bytes[6] = (row.ei >> 16) as u8;
            bytes[8..].copy_from_slice(&row.b.to_le_bytes());
            out.write_all(&bytes)
                .map_err(|e| format!("write migration output: {e}"))?;
            done += 1;
            progress.send("sort-output", done, total, false)?;
        }
        out.flush()
            .map_err(|e| format!("flush migration output: {e}"))?;
        progress.send("done", done, total, true)
    }
}

#[allow(clippy::too_many_arguments)]
fn match_rule(
    old: &Pack<'_>,
    new: &Pack<'_>,
    old_sorted: &Path,
    new_sorted: &Path,
    states: &[u8],
    new_ci: usize,
    point_limit: usize,
    output: &mut OutputRuns,
    stats: &mut Stats,
    progress: &Progress,
) -> Result<(), String> {
    let mut left = Stream::new(old_sorted)?;
    let mut right = Stream::new(new_sorted)?;
    let mut processed = 0;
    while let Some(first) = left.head {
        let key = first.key();
        while right.head.is_some_and(|row| row.key() < key) {
            right.pop()?;
            processed += 1;
            if processed & 4095 == 0 {
                progress.send("match", processed, new.errors as u64, false)?;
            }
        }
        let mut any = 0u8;
        let mut all = 6u8;
        let mut collision = false;
        let mut work = 0;
        let has_targets = right.head.is_some_and(|row| row.key() == key);
        let old_begin = left.offset;
        while left.head.is_some_and(|row| row.key() == key) {
            let row = left.pop()?.unwrap();
            let value = states[old.error_start + row.ei as usize];
            if value != 0 {
                work += 1;
            }
            any |= value;
            all &= value;
            if left.offset / RECORD_BYTES as u64 & 4095 == 0 {
                progress.send(
                    "scan-source",
                    left.offset / RECORD_BYTES as u64,
                    old.errors as u64,
                    false,
                )?;
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
                    progress.send("match", processed, new.errors as u64, false)?;
                }
            }
            continue;
        }
        // Untouched groups need no exact decode. For a group with work, replay
        // every old occurrence, including 000, without retaining an ID list.
        left.seek(old_begin)?;
        let mut representative = None;
        while left.head.is_some_and(|row| row.key() == key) {
            let row = left.pop()?.unwrap();
            let canonical = geometry(old, row.ei, point_limit)?;
            if let Some(previous) = &representative {
                collision |= previous != &canonical;
            } else {
                representative = Some(canonical);
            }
            progress.send(
                "verify-source",
                left.offset / RECORD_BYTES as u64,
                old.errors as u64,
                false,
            )?;
        }
        let begin = right.offset;
        while right.head.is_some_and(|row| row.key() == key) {
            let row = right.pop()?.unwrap();
            collision |= Some(geometry(new, row.ei, point_limit)?) != representative;
            progress.send(
                "verify-target",
                right.offset / RECORD_BYTES as u64,
                new.errors as u64,
                false,
            )?;
        }
        // V is evidence that any exact old occurrence was seen. W and NR
        // are separate judgements: both 0 and 1 vote, including untouched
        // duplicates. A disagreement blocks only that particular field.
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
        // Replay a same-hash target group after verification. This keeps even
        // a billion identical targets bounded and avoids a Cartesian product.
        right.seek(begin)?;
        while right.head.is_some_and(|row| row.key() == key) {
            let row = right.pop()?.unwrap();
            output.push(new_ci, row.ei, flags, collision, unknown)?;
            if ambiguous {
                stats.ambiguous_target += 1;
            }
            if !collision {
                stats.matched_target += 1;
            }
            if partial {
                stats.partial_target += 1;
            }
            processed += 1;
            progress.send("match", processed, new.errors as u64, false)?;
        }
    }
    progress.send("match", new.errors as u64, new.errors as u64, true)
}

fn target(path: &Path) -> Result<PathBuf, String> {
    if path.exists() {
        return std::fs::canonicalize(path).map_err(|e| e.to_string());
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    Ok(std::fs::canonicalize(parent)
        .map_err(|e| e.to_string())?
        .join(path.file_name().ok_or("missing output filename")?))
}
fn command(args: &[String]) -> Result<(), String> {
    if args == ["--protocol"] {
        println!("2");
        return Ok(());
    }
    if args.len() < 2 || args[0].starts_with("--") || args[1].starts_with("--") {
        return Err("usage: floe-index drc-match OLD NEW --plan PAIRS.json --states FLAGS.u8 --out RECORDS.bin --work-dir DIR [--jobs N] [--memory-mib N] [--progress PATH] [--report PATH]".into());
    }
    let (mut plan, mut states, mut out, mut workdir, mut progress, mut report) =
        (None, None, None, None, None, None);
    let mut jobs = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(4);
    let mut memory = 64usize;
    let mut i = 2;
    while i < args.len() {
        let value = args
            .get(i + 1)
            .ok_or_else(|| format!("{} requires a value", args[i]))?;
        match args[i].as_str() {
            "--plan" => plan = Some(Path::new(value)),
            "--states" => states = Some(Path::new(value)),
            "--out" => out = Some(Path::new(value)),
            "--work-dir" => workdir = Some(Path::new(value)),
            "--progress" => progress = Some(Path::new(value)),
            "--report" => report = Some(Path::new(value)),
            "--jobs" => jobs = value.parse().map_err(|_| "invalid match worker count")?,
            "--memory-mib" => memory = value.parse().map_err(|_| "invalid match memory budget")?,
            option => return Err(format!("unknown drc-match option {option}")),
        }
        i += 2;
    }
    if jobs == 0 || jobs > 256 || !(4..=65536).contains(&memory) {
        return Err("jobs must be 1..256 and memory-mib 4..65536".into());
    }
    let plan = plan.ok_or("--plan is required")?;
    let states = states.ok_or("--states is required")?;
    let out = out.ok_or("--out is required")?;
    let workdir = workdir.ok_or("--work-dir is required")?;
    let inputs = [Path::new(&args[0]), Path::new(&args[1]), plan, states];
    let identities = inputs
        .iter()
        .map(|path| target(path))
        .collect::<Result<Vec<_>, _>>()?;
    let mut outputs = HashSet::new();
    for path in [Some(out), progress, report].into_iter().flatten() {
        let identity = target(path)?;
        if identities.contains(&identity) || !outputs.insert(identity) || path.exists() {
            return Err(
                "migration outputs must be fresh files distinct from all inputs and each other"
                    .into(),
            );
        }
    }
    let oldmap = map(inputs[0])?;
    let newmap = map(inputs[1])?;
    let oldtotal = size(u64_at(&oldmap, footer(&oldmap)? + 14 * 8)?)?;
    footer(&newmap)?;
    let statefile = File::open(states).map_err(|e| format!("open source flags: {e}"))?;
    if statefile.metadata().map_err(|e| e.to_string())?.len() != oldtotal as u64 {
        return Err("source flags length does not match old pack".into());
    }
    let statemap = if oldtotal == 0 {
        None
    } else {
        Some(unsafe { memmap2::Mmap::map(&statefile) }.map_err(|e| e.to_string())?)
    };
    let states = statemap.as_deref().unwrap_or(&[]);
    let planbytes = std::fs::read(plan).map_err(|e| format!("read migration plan: {e}"))?;
    let pairs = parse_pairs(&planbytes)?;
    // Validate every selected rule and identity before creating any output.
    for &(a, b) in &pairs {
        Pack::parse(&oldmap, a)?;
        Pack::parse(&newmap, b)?;
        let name = rule_name(&oldmap, a)?;
        if name != rule_name(&newmap, b)?
            || !unique_name(&oldmap, name)?
            || !unique_name(&newmap, name)?
        {
            return Err("migration rules must have the same unique exact name".into());
        }
    }
    let budget = memory * 1024 * 1024;
    let jobs = jobs.min((budget / (1024 * 1024)).max(1));
    let point_limit = (budget / jobs / 4 / 128).max(1024);
    let scratch = Scratch::new(workdir)?;
    let progress = Progress::new(progress)?;
    let mut stats = Stats::default();
    // Output runs consume at most a quarter of the configured memory, leaving
    // the remaining worker budget for fixed records and one geometry each.
    let mut output = OutputRuns::new(&scratch.0, budget);
    for (ordinal, &(a, b)) in pairs.iter().enumerate() {
        let old = Pack::parse(&oldmap, a)?;
        let new = Pack::parse(&newmap, b)?;
        let dir = scratch.0.join(format!("pair-{ordinal}"));
        std::fs::create_dir(&dir).map_err(|e| e.to_string())?;
        let left = index(
            &old,
            Some(states),
            &dir,
            "hash-old",
            jobs,
            budget * 3 / 4,
            point_limit,
            &progress,
        )?;
        let right = index(
            &new,
            None,
            &dir,
            "hash-new",
            jobs,
            budget * 3 / 4,
            point_limit,
            &progress,
        )?;
        stats.source_work += left.work;
        stats.invalid_old += left.invalid;
        stats.invalid_new += right.invalid;
        match_rule(
            &old,
            &new,
            &left.path,
            &right.path,
            states,
            b,
            point_limit,
            &mut output,
            &mut stats,
            &progress,
        )?;
        std::fs::remove_dir_all(&dir).map_err(|e| format!("remove rule scratch: {e}"))?;
    }
    output.finish(out, &progress)?;
    if let Some(path) = report {
        create(path)?
            .write_all(stats.json().as_bytes())
            .map_err(|e| format!("write match report: {e}"))?;
    }
    eprintln!(
        "[drc-match] {} matched target errors; {} ambiguous; {} invalid old, {} invalid new",
        stats.matched_target, stats.ambiguous_target, stats.invalid_old, stats.invalid_new
    );
    Ok(())
}

pub fn drcmatch_cmd(args: &[String]) {
    if let Err(error) = command(args) {
        eprintln!("[drc-match] {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            static SERIAL: AtomicU64 = AtomicU64::new(0);
            let time = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "floe-match-{}-{time}-{}",
                std::process::id(),
                SERIAL.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn file(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn canon(kind: u8, points: &[Point], precision: f64) -> Vec<u8> {
        let mut out = Vec::new();
        assert!(canonical(
            kind,
            &mut points.to_vec(),
            Scale::new(precision).unwrap(),
            &mut out,
            &mut Vec::new()
        ));
        out
    }
    fn pack(temp: &Temp, name: &str, source: &str) -> PathBuf {
        let src = temp.file(&format!("{name}.db"));
        let dst = temp.file(&format!("{name}.tray"));
        std::fs::write(&src, source).unwrap();
        crate::drcpack::pack(src.to_str().unwrap(), dst.to_str().unwrap(), 2).unwrap();
        dst
    }
    fn args(
        temp: &Temp,
        old: &Path,
        new: &Path,
        pairs: &str,
        states: &[u8],
        stem: &str,
    ) -> Vec<String> {
        let plan = temp.file(&format!("{stem}.json"));
        let flags = temp.file(&format!("{stem}.u8"));
        std::fs::write(&plan, pairs).unwrap();
        std::fs::write(&flags, states).unwrap();
        vec![
            old.to_str().unwrap().into(),
            new.to_str().unwrap().into(),
            "--plan".into(),
            plan.to_str().unwrap().into(),
            "--states".into(),
            flags.to_str().unwrap().into(),
            "--out".into(),
            temp.file(&format!("{stem}.out")).to_str().unwrap().into(),
            "--work-dir".into(),
            temp.file("new-work-directory").to_str().unwrap().into(),
            "--report".into(),
            temp.file(&format!("{stem}.report"))
                .to_str()
                .unwrap()
                .into(),
            "--progress".into(),
            temp.file(&format!("{stem}.progress"))
                .to_str()
                .unwrap()
                .into(),
            "--jobs".into(),
            "2".into(),
            "--memory-mib".into(),
            "4".into(),
        ]
    }
    fn rows(path: &Path) -> Vec<(u32, u64, u8, u8, u8)> {
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(bytes.len() % 16, 0);
        bytes
            .chunks_exact(16)
            .map(|row| {
                assert_eq!(row[7], 0);
                (
                    u32::from_le_bytes(row[..4].try_into().unwrap()),
                    u64::from_le_bytes(row[8..].try_into().unwrap()),
                    row[4],
                    row[5],
                    row[6],
                )
            })
            .collect()
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
            let scaled = points
                .iter()
                .map(|&(x, y)| (x * 3, y * 3))
                .collect::<Vec<_>>();
            assert_eq!(canon(0, &scaled, 3000.0), expected);
        }
        assert_ne!(canon(0, &polygon, 1000.0000000001), expected);
        assert_ne!(
            canon(
                0,
                &[(-9, -7), (0, -7), (11, -7), (11, 15), (-9, 15)],
                1000.0
            ),
            expected
        );
    }
    #[test]
    fn edge_endpoint_pairing_is_preserved_but_order_is_irrelevant() {
        let shape = [(-9, -7), (11, -7), (11, 15), (-9, 15)];
        let expected = canon(1, &shape, 1000.0);
        assert_eq!(
            canon(1, &[shape[3], shape[2], shape[1], shape[0]], 1000.0),
            expected
        );
        assert_ne!(
            canon(1, &[shape[0], shape[2], shape[1], shape[3]], 1000.0),
            expected
        );
        assert_ne!(canon(0, &shape, 1000.0), expected);
    }
    #[test]
    fn rational_coordinates_do_not_round_large_or_extreme_values() {
        assert_ne!(
            canon(0, &[(1 << 60, 0)], 1000.0),
            canon(0, &[((1 << 60) + 1, 0)], 1000.0)
        );
        assert_eq!(
            canon(0, &[(i64::MIN, 0)], 2.0),
            canon(0, &[(i64::MIN / 2, 0)], 1.0)
        );
        assert_eq!(
            canon(0, &[(3, -9)], f64::from_bits(1)),
            canon(0, &[(6, -18)], f64::from_bits(2))
        );
        assert_ne!(canon(0, &[(1, 0)], f64::MAX), canon(0, &[(2, 0)], f64::MAX));
        assert_eq!(
            canon(0, &[(0, 0)], f64::MAX),
            canon(0, &[(0, 0)], f64::from_bits(1))
        );
    }
    #[test]
    fn strict_pair_plan_rejects_duplicate_or_invalid_indices() {
        assert_eq!(
            parse_pairs(b" [ [ 0, 2 ],[1,0] ] ").unwrap(),
            vec![(0, 2), (1, 0)]
        );
        assert!(parse_pairs(b"[]").unwrap().is_empty());
        for bad in [
            "[[0,1],[0,2]]",
            "[[0,1],[2,1]]",
            "[[01,2]]",
            "[[-1,2]]",
            "[[0,2],]",
            "[[0,4294967296]]",
            "[[0,1]]x",
        ] {
            assert!(parse_pairs(bad.as_bytes()).is_err(), "{bad}");
        }
    }
    #[test]
    fn parallel_command_sorts_reordered_rules_and_preserves_duplicate_zeros() {
        let temp = Temp::new();
        let old=pack(&temp,"old","TOP 1000\nA\n5 5 0\np 1 1\n10 -5\np 2 1\n20 3\np 3 1\n10 -5\np 4 1\n30 4\np 5 1\n30 4\nB\n1 1 0\ne 1 1\n-2 0 2 0\n");
        let new=pack(&temp,"new","TOP 2000\nB\n1 1 0\ne 1 1\n4 0 -4 0\nA\n5 5 0\np 1 1\n60 8\np 2 1\n40 6\np 3 1\n20 -10\np 4 1\n22 -10\np 5 1\n20 -10\n");
        let mut run = args(
            &temp,
            &old,
            &new,
            "[[0,1],[1,0]]",
            &[3, 4, 0, 5, 5, 2],
            "parallel",
        );
        command(&run).unwrap();
        let expected = vec![
            (0, 0, 2, 0, 0),
            (1, 0, 5, 0, 0),
            (1, 1, 4, 0, 0),
            (1, 2, 1, 0, 2),
            (1, 4, 1, 0, 2),
        ];
        assert_eq!(rows(&temp.file("parallel.out")), expected);
        let report = std::fs::read_to_string(temp.file("parallel.report")).unwrap();
        assert!(report.contains("\"source_work\":5"));
        assert!(report.contains("\"matched_target\":5"));
        assert!(report.contains("\"ambiguous_target\":2"));
        assert!(report.contains("\"ambiguous_source_work\":1"));
        assert!(report.contains("\"partial_target\":2"));
        assert!(report.contains("\"partial_source_work\":1"));
        assert!(report.contains("\"unmatched_source_work\":0"));
        assert_eq!(
            std::fs::read_dir(temp.file("new-work-directory"))
                .unwrap()
                .count(),
            0
        );
        run = args(
            &temp,
            &old,
            &new,
            "[[1,0],[0,1]]",
            &[3, 4, 0, 5, 5, 2],
            "serial",
        );
        run[15] = "1".into();
        command(&run).unwrap();
        assert_eq!(rows(&temp.file("serial.out")), expected);
    }
    #[test]
    fn existing_files_and_unknown_states_and_rule_names_are_rejected() {
        let temp = Temp::new();
        let bytes = crate::drcmeasure::tests::fixture(1);
        let old = temp.file("old.tray");
        let new = temp.file("new.tray");
        std::fs::write(&old, &bytes).unwrap();
        std::fs::write(&new, &bytes).unwrap();
        let mut run = args(&temp, &old, &new, "[[0,0]]", &[1], "safe");
        run[7] = old.to_str().unwrap().into();
        assert!(command(&run).unwrap_err().contains("fresh"));
        assert_eq!(std::fs::read(&old).unwrap(), bytes);
        let run = args(&temp, &old, &new, "[[0,0]]", &[8], "bad-flags");
        assert!(command(&run).unwrap_err().contains("unknown bits"));
        let run = args(&temp, &old, &new, "[[0,0]]", &[], "bad-size");
        assert!(command(&run).unwrap_err().contains("length"));
        let other = pack(&temp, "other", "TOP 1000\nDIFFERENT\n1 1 0\np 1 1\n1 1\n");
        let run = args(&temp, &old, &other, "[[0,0]]", &[1], "bad-name");
        assert!(command(&run).unwrap_err().contains("unique exact name"));
        let duplicate = pack(
            &temp,
            "duplicate",
            "TOP 1000\nR\n1 1 0\np 1 1\n1 1\nR\n1 1 0\np 1 1\n1 1\n",
        );
        let run = args(&temp, &old, &duplicate, "[[0,0]]", &[1], "duplicate-name");
        assert!(command(&run).unwrap_err().contains("unique exact name"));
    }
    #[test]
    fn empty_rules_produce_empty_fresh_output() {
        let temp = Temp::new();
        let path = temp.file("empty.tray");
        std::fs::write(&path, crate::drcmeasure::tests::fixture(0)).unwrap();
        command(&args(&temp, &path, &path, "[[0,0]]", &[], "empty")).unwrap();
        assert!(rows(&temp.file("empty.out")).is_empty());
        assert!(std::fs::read_to_string(temp.file("empty.progress"))
            .unwrap()
            .contains("\"phase\":\"done\""));
    }
    #[test]
    fn duplicate_visits_union_and_each_judgement_has_independent_consensus() {
        let temp = Temp::new();
        let old = temp.file("old.tray");
        let new = temp.file("new.tray");
        std::fs::write(&old, crate::drcmeasure::tests::fixture(2)).unwrap();
        std::fs::write(&new, crate::drcmeasure::tests::fixture(1)).unwrap();
        // Expected flags and unknown judgement mask. Known false is encoded
        // by an absent flag with the corresponding unknown bit also absent.
        for (states, flags, unknown) in [
            ([1, 0], 1, 0), // Visits differ; both judgements agree false.
            ([3, 1], 1, 2), // W differs, NR agrees false.
            ([3, 2], 3, 0), // Visits differ, W agrees true.
            ([7, 3], 3, 4), // NR differs, W agrees true.
            ([5, 1], 1, 4), // NR differs, W agrees false.
            ([2, 0], 0, 2), // No visits and no consensus true: still emit W conflict.
            ([4, 0], 0, 4), // The analogous NR-only conflict.
            ([6, 2], 2, 4),
            ([2, 4], 0, 6), // Both judgements disagree, without a visit.
            ([7, 0], 1, 6), // Untouched duplicates vote on both fields.
            ([7, 7], 7, 0),
            ([0, 0], 0, 0), // Entirely untouched geometry is not a transfer.
        ] {
            let stem = format!("states-{}-{}", states[0], states[1]);
            command(&args(&temp, &old, &new, "[[0,0]]", &states, &stem)).unwrap();
            let actual = rows(&temp.file(&format!("{stem}.out")));
            let work = states.iter().filter(|&&value| value != 0).count();
            let expected = if work == 0 {
                vec![]
            } else {
                vec![(0, 0, flags, 0, unknown)]
            };
            assert_eq!(actual, expected, "states={states:?}");
            let report = std::fs::read_to_string(temp.file(&format!("{stem}.report"))).unwrap();
            let partial = usize::from(unknown != 0);
            for text in [
                "\"protocol\":2".to_owned(),
                format!("\"matched_target\":{}", usize::from(work != 0)),
                format!("\"matched_source_work\":{work}"),
                format!("\"ambiguous_target\":{partial}"),
                format!("\"ambiguous_source_work\":{}", partial * work),
                format!("\"partial_target\":{partial}"),
                format!("\"partial_source_work\":{}", partial * work),
                "\"unmatched_source_work\":0".to_owned(),
            ] {
                assert!(
                    report.contains(&text),
                    "states={states:?}: {text} missing from {report}"
                );
            }
        }
    }
    #[test]
    fn malformed_blocks_abort_and_semantic_invalid_shapes_are_skipped() {
        let mut points = Vec::new();
        for bad in [&[2, 0][..], &[2, 0, 0, 0][..], &[255; 10][..]] {
            assert!(decode(bad, 1, 100, None, &mut points, |_, _, _| Ok(())).is_err());
        }
        let mut valid = Vec::new();
        // p0, odd e1, then p2 exceeding the deliberate per-error budget.
        decode(
            &[0, 3, 0, 0, 4, 0, 0, 2, 2],
            3,
            1,
            None,
            &mut points,
            |_, kind, points| {
                valid.push(points.is_some_and(|points| {
                    canonical(
                        kind,
                        points,
                        Scale::new(1000.0).unwrap(),
                        &mut Vec::new(),
                        &mut Vec::new(),
                    )
                }));
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(valid, vec![false, false, false]);
        let temp = Temp::new();
        let path = temp.file("bad.tray");
        let mut bytes = crate::drcmeasure::tests::fixture(1);
        bytes[40] = 255;
        std::fs::write(&path, bytes).unwrap();
        assert!(command(&args(&temp, &path, &path, "[[0,0]]", &[1], "bad")).is_err());
        assert!(!temp.file("bad.out").exists());
        assert_eq!(
            std::fs::read_dir(temp.file("new-work-directory"))
                .unwrap()
                .count(),
            0
        );
    }
    #[test]
    fn bounded_fanin_merge_and_target_output_sort() {
        let temp = Temp::new();
        let progress = Progress::new(None).unwrap();
        let mut paths = Vec::new();
        for index in 0..FANIN + 3 {
            let path = temp.file(&format!("run-{index}"));
            save_run(
                &mut vec![
                    Record {
                        a: 1,
                        b: index as u64,
                        ei: 2,
                    },
                    Record {
                        a: 0,
                        b: index as u64,
                        ei: 1,
                    },
                ],
                &path,
            )
            .unwrap();
            paths.push(path);
        }
        let sorted = collapse(paths, &temp.0, "test", &progress).unwrap();
        let mut input = Stream::new(&sorted).unwrap();
        let mut actual = Vec::new();
        while let Some(row) = input.pop().unwrap() {
            actual.push(row);
        }
        assert_eq!(actual.len(), 2 * (FANIN + 3));
        assert!(actual.windows(2).all(|rows| rows[0] <= rows[1]));
        let mut output = OutputRuns::new(&temp.0, 4096);
        output.capacity = 1;
        for ci in (0..20).rev() {
            output.push(ci, 7, 5, false, 2).unwrap();
            output.push(ci, 1, 0, true, 0).unwrap();
        }
        let path = temp.file("final");
        output.finish(&path, &progress).unwrap();
        let actual = rows(&path);
        assert_eq!(actual.len(), 40);
        assert!(actual
            .windows(2)
            .all(|rows| (rows[0].0, rows[0].1) < (rows[1].0, rows[1].1)));
    }
    #[test]
    fn forced_digest_collision_is_ambiguous_and_never_copies_judgement() {
        let temp = Temp::new();
        let old = std::fs::read(pack(
            &temp,
            "old",
            "TOP 1000\nA\n2 2 0\np 1 1\n1 1\np 2 1\n2 2\n",
        ))
        .unwrap();
        let new = std::fs::read(pack(&temp, "new", "TOP 1000\nA\n1 1 0\np 1 1\n1 1\n")).unwrap();
        let left = temp.file("left");
        let right = temp.file("right");
        save_run(
            &mut vec![Record { a: 1, b: 2, ei: 0 }, Record { a: 1, b: 2, ei: 1 }],
            &left,
        )
        .unwrap();
        save_run(&mut vec![Record { a: 1, b: 2, ei: 0 }], &right).unwrap();
        let mut output = OutputRuns::new(&temp.0, 4096);
        let mut stats = Stats::default();
        let progress = Progress::new(None).unwrap();
        match_rule(
            &Pack::parse(&old, 0).unwrap(),
            &Pack::parse(&new, 0).unwrap(),
            &left,
            &right,
            &[3, 4],
            0,
            1024,
            &mut output,
            &mut stats,
            &progress,
        )
        .unwrap();
        assert_eq!(stats.hash_collision_groups, 1);
        assert_eq!(stats.ambiguous_source_work, 2);
        let path = temp.file("out");
        output.finish(&path, &progress).unwrap();
        assert_eq!(stats.matched_target, 0);
        assert_eq!(stats.partial_target, 0);
        assert_eq!(rows(&path), vec![(0, 0, 0, 1, 0)]);
    }
    #[test]
    fn several_workers_and_sort_runs_keep_all_block_ids() {
        let temp = Temp::new();
        let bytes = crate::drcmeasure::tests::fixture(257);
        let pack = Pack::parse(&bytes, 0).unwrap();
        let progress = Progress::new(None).unwrap();
        let indexed = index(
            &pack,
            Some(&vec![3; 257]),
            &temp.0,
            "parallel",
            3,
            512,
            1024,
            &progress,
        )
        .unwrap();
        assert_eq!((indexed.work, indexed.invalid), (257, 0));
        let mut stream = Stream::new(&indexed.path).unwrap();
        let mut count = 0;
        while let Some(row) = stream.pop().unwrap() {
            assert_eq!(row.ei, count);
            count += 1;
        }
        assert_eq!(count, 257);
    }
}
