//! Parallel, rule-local CD preprocessing from immutable v4 DRC packs.
//!
//! Geometry remains in file order. Workers dynamically claim coordinate-block
//! chunks and write disjoint original-order array spans. Memory is bounded by
//! the worker count and largest individual geometry, never by the rule size.
//! Cache identity, locking, publication, and precise Python fallback belong to
//! the Python coordinator; this command writes only a fresh staging directory.

use crate::drcmeasure_math::{measure, Chain, Plan, Predicate};
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const MAGIC: &[u8; 8] = b"FLOEICE\0";
const PLAN_MAGIC: &[u8; 8] = b"FDRCMS01";
const HEADER: usize = 40;
const FOOTER: usize = 136;
const BLOCK: usize = 64;
const BLOCK_RECORD: usize = 48;
const CHECK_RECORD: usize = 64;
const CHUNK_BLOCKS: usize = 1024;

fn bytes<const N: usize>(data: &[u8], offset: usize) -> Result<[u8; N], String> {
    let end = offset.checked_add(N).ok_or("offset overflow")?;
    data.get(offset..end)
        .ok_or_else(|| "truncated binary input".to_string())?
        .try_into()
        .map_err(|_| "invalid binary field".to_string())
}
fn u32_at(data: &[u8], offset: usize) -> Result<u32, String> {
    Ok(u32::from_le_bytes(bytes(data, offset)?))
}
fn u64_at(data: &[u8], offset: usize) -> Result<u64, String> {
    Ok(u64::from_le_bytes(bytes(data, offset)?))
}
pub(crate) fn size(value: u64) -> Result<usize, String> {
    usize::try_from(value).map_err(|_| "pack is too large for this host".into())
}
fn product(a: u64, b: u64) -> Result<u64, String> {
    a.checked_mul(b)
        .ok_or_else(|| "pack section size overflow".into())
}
fn section(off: u64, len: u64, body_end: usize) -> Result<(usize, usize), String> {
    let end = off.checked_add(len).ok_or("pack section offset overflow")?;
    if off < HEADER as u64 || end > body_end as u64 {
        return Err("pack section lies outside the file body".into());
    }
    Ok((size(off)?, size(end)?))
}

struct Cursor<'a> {
    data: &'a [u8],
    position: usize,
}
impl<'a> Cursor<'a> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N], String> {
        let out = bytes(self.data, self.position)?;
        self.position += N;
        Ok(out)
    }
    fn byte(&mut self) -> Result<u8, String> {
        Ok(self.take::<1>()?[0])
    }
    fn flag(&mut self) -> Result<bool, String> {
        match self.byte()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err("invalid plan boolean".into()),
        }
    }
    fn count(&mut self) -> Result<usize, String> {
        Ok(u32::from_le_bytes(self.take()?) as usize)
    }
    fn number(&mut self) -> Result<f64, String> {
        let value = f64::from_le_bytes(self.take()?);
        if !value.is_finite() {
            return Err("plan bound must be finite".into());
        }
        Ok(value)
    }
    fn op(&mut self) -> Result<u8, String> {
        let op = self.byte()?;
        if op > 5 {
            return Err("invalid measurement comparison operator".into());
        }
        Ok(op)
    }
}

fn parse_plan(data: &[u8]) -> Result<Plan, String> {
    let mut cursor = Cursor { data, position: 0 };
    if &cursor.take::<8>()? != PLAN_MAGIC {
        return Err("unsupported CD plan protocol".into());
    }
    let uncertain_alternatives = cursor.flag()?;
    let count = cursor.count()?;
    // Each chain has at least 28 bytes; reject counts before allocating.
    if count > data.len().saturating_sub(cursor.position) / 28 {
        return Err("truncated CD plan chains".into());
    }
    let mut chains = Vec::with_capacity(count);
    for _ in 0..count {
        let metric = cursor.byte()?;
        if metric > 7 {
            return Err("invalid CD metric".into());
        }
        let ci = i32::from_le_bytes(cursor.take()?);
        if ci < 0 {
            return Err("negative CD constraint index".into());
        }
        let op = cursor.op()?;
        let bound = cursor.number()?;
        let valid = cursor.flag()?;
        let ticks = i64::from_le_bytes(cursor.take()?);
        let options = cursor.flag()?;
        let npred = cursor.count()?;
        if npred == 0 || npred > data.len().saturating_sub(cursor.position) / 9 {
            return Err("invalid/truncated CD predicate list".into());
        }
        let mut predicates = Vec::with_capacity(npred);
        for _ in 0..npred {
            predicates.push(Predicate {
                op: cursor.op()?,
                bound: cursor.number()?,
            });
        }
        chains.push(Chain {
            metric,
            ci,
            op,
            bound,
            bound_ticks: if valid { Some(ticks) } else { None },
            options,
            predicates,
        });
    }
    if cursor.position != data.len() {
        return Err("trailing bytes in CD plan".into());
    }
    Ok(Plan {
        uncertain_alternatives,
        chains,
    })
}

pub(crate) struct Pack<'a> {
    data: &'a [u8],
    precision: f64,
    blob_start: usize,
    blob_end: usize,
    block_table: usize,
    block_total: usize,
    block_start: usize,
    pub(crate) block_count: usize,
    pub(crate) errors: usize,
}
impl<'a> Pack<'a> {
    pub(crate) fn parse(data: &'a [u8], rule: usize) -> Result<Self, String> {
        if data.len() < HEADER + FOOTER {
            return Err("truncated DRC pack".into());
        }
        if data.get(..8) != Some(MAGIC.as_slice()) || u32_at(data, 8)? != 4 {
            return Err("native DRC preprocessing requires a version 4 DRC pack".into());
        }
        if u32_at(data, 12)? != 1 {
            return Err("unsupported DRC pack flags".into());
        }
        let precision = f64::from_le_bytes(bytes(data, 16)?);
        if !precision.is_finite() || precision <= 0.0 {
            return Err("invalid DRC pack precision".into());
        }
        let end = data.len() - FOOTER;
        let footer = &data[end..];
        if footer.get(128..136) != Some(MAGIC.as_slice()) {
            return Err("invalid DRC pack footer magic".into());
        }
        let mut f = [0u64; 15];
        for (i, value) in f.iter_mut().enumerate() {
            *value = u64_at(footer, i * 8)?;
        }
        let (blob_start, blob_end) = section(f[0], f[1], end)?;
        let (_, qend) = section(f[2], f[3], end)?;
        let (_, send) = section(f[4], f[14], end)?;
        let (_, wend) = section(f[5], product(f[9], 4)?, end)?;
        let (block_table, bend) = section(f[6], product(f[7], BLOCK_RECORD as u64)?, end)?;
        let (directory, dend) = section(f[8], product(f[9], CHECK_RECORD as u64)?, end)?;
        let (refs, rend) = section(f[10], product(f[11], 4)?, end)?;
        let (strings, strend) = section(f[12], f[13], end)?;
        if blob_start != HEADER
            || f[3] != product(f[14], 4)?
            || blob_end as u64 != f[2]
            || qend as u64 != f[4]
            || send as u64 != f[5]
            || wend as u64 != f[6]
            || bend as u64 != f[8]
            || dend as u64 != f[10]
            || rend as u64 != f[12]
            || strend != end
        {
            return Err("inconsistent/overlapping DRC pack sections".into());
        }
        let check_count = size(f[9])?;
        if rule >= check_count {
            return Err(format!(
                "rule index {} is out of range ({} rules)",
                rule, check_count
            ));
        }
        let string_ref = |reference: u32| -> Result<(), String> {
            let start = (reference as usize)
                .checked_add(strings)
                .ok_or("string offset overflow")?;
            if start.checked_add(4).filter(|&v| v <= strend).is_none() {
                return Err("invalid pack string reference".into());
            }
            let stop = start
                .checked_add(4)
                .and_then(|v| v.checked_add(u32_at(data, start).ok()? as usize));
            if stop.filter(|&v| v <= strend).is_none() {
                return Err("truncated pack string".into());
            }
            Ok(())
        };
        string_ref(u32_at(footer, 120)?)?;
        for i in 0..size(f[11])? {
            string_ref(u32_at(data, refs + i * 4)?)?;
        }
        let mut expected_error = 0u64;
        let mut expected_block = 0u64;
        let mut selected = (0, 0, 0);
        for ci in 0..check_count {
            let offset = directory + ci * CHECK_RECORD;
            string_ref(u32_at(data, offset)?)?;
            let dstart = u32_at(data, offset + 4)? as u64;
            let dcount = u32_at(data, offset + 8)? as u64;
            if dstart.checked_add(dcount).filter(|&v| v <= f[11]).is_none() {
                return Err("invalid check description range".into());
            }
            let estart = u64_at(data, offset + 16)?;
            let errors = u64_at(data, offset + 24)?;
            let bstart = u64_at(data, offset + 48)?;
            let blocks = u64_at(data, offset + 56)?;
            let want_blocks = errors / BLOCK as u64 + u64::from(errors % BLOCK as u64 != 0);
            if estart != expected_error || bstart != expected_block || blocks != want_blocks {
                return Err("inconsistent check error/block directory".into());
            }
            expected_error = expected_error
                .checked_add(errors)
                .ok_or("error count overflow")?;
            expected_block = expected_block
                .checked_add(blocks)
                .ok_or("block count overflow")?;
            if expected_error > f[14] || expected_block > f[7] {
                return Err("check range exceeds pack totals".into());
            }
            if ci == rule {
                selected = (size(bstart)?, size(blocks)?, size(errors)?);
            }
        }
        if expected_error != f[14] || expected_block != f[7] {
            return Err("pack totals do not match its directory".into());
        }
        let pack = Self {
            data,
            precision,
            blob_start,
            blob_end,
            block_table,
            block_total: size(f[7])?,
            block_start: selected.0,
            block_count: selected.1,
            errors: selected.2,
        };
        // Validate target blocks before any output is created. Other rules'
        // block rows need not be paged in for a single-rule preprocessing job.
        for block in 0..pack.block_count {
            pack.block(block)?;
        }
        Ok(pack)
    }

    pub(crate) fn block(&self, relative: usize) -> Result<(&'a [u8], usize), String> {
        if relative >= self.block_count {
            return Err("block index out of range".into());
        }
        let global = self.block_start + relative;
        let rec = self.block_table + global * BLOCK_RECORD;
        let start = size(u64_at(self.data, rec)?)?;
        let stop = if global + 1 < self.block_total {
            size(u64_at(self.data, rec + BLOCK_RECORD)?)?
        } else {
            self.blob_end
        };
        let count = u32_at(self.data, rec + 8)? as usize;
        let want = (self.errors - relative * BLOCK).min(BLOCK);
        if count != want || start < self.blob_start || stop > self.blob_end || start >= stop {
            return Err("invalid coordinate block offset/count".into());
        }
        if global == 0 && start != self.blob_start {
            return Err("coordinate blob starts after its first block".into());
        }
        for (a, b) in [(16, 32), (24, 40)] {
            if i64::from_le_bytes(bytes(self.data, rec + a)?)
                > i64::from_le_bytes(bytes(self.data, rec + b)?)
            {
                return Err("invalid coordinate block bounds".into());
            }
        }
        Ok((&self.data[start..stop], count))
    }
}

pub(crate) fn varint(data: &[u8], pos: &mut usize) -> Result<u64, String> {
    let mut value = 0u64;
    for byte_index in 0..10 {
        let byte = *data.get(*pos).ok_or("truncated coordinate varint")?;
        *pos += 1;
        if byte_index == 9 && byte > 1 {
            return Err("coordinate varint overflow".into());
        }
        value |= ((byte & 0x7f) as u64) << (byte_index * 7);
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err("coordinate varint overflow".into())
}
fn unzz(value: u64) -> i64 {
    ((value >> 1) as i64) ^ -((value & 1) as i64)
}
pub(crate) fn add_coord(value: i64, delta: u64) -> Result<i64, String> {
    value
        .checked_add(unzz(delta))
        .ok_or_else(|| "coordinate delta overflow".into())
}

fn decode_block<F>(
    data: &[u8],
    count: usize,
    precision: f64,
    points: &mut Vec<(f64, f64)>,
    mut visit: F,
) -> Result<(), String>
where
    F: FnMut(u8, &[(f64, f64)]),
{
    let (mut pos, mut pfx, mut pfy) = (0usize, 0i64, 0i64);
    for _ in 0..count {
        let encoded = varint(data, &mut pos)?;
        let npts = size(encoded >> 1)?;
        if npts == 0 || npts > data.len().saturating_sub(pos) / 2 {
            return Err("invalid/truncated coordinate point count".into());
        }
        points.clear();
        points
            .try_reserve(npts)
            .map_err(|_| "insufficient memory for DRC geometry")?;
        let mut x = add_coord(pfx, varint(data, &mut pos)?)?;
        let mut y = add_coord(pfy, varint(data, &mut pos)?)?;
        pfx = x;
        pfy = y;
        for point in 0..npts {
            if point > 0 {
                x = add_coord(x, varint(data, &mut pos)?)?;
                y = add_coord(y, varint(data, &mut pos)?)?;
            }
            let (px, py) = (x as f64 / precision, y as f64 / precision);
            if !px.is_finite() || !py.is_finite() {
                return Err("non-finite physical DRC coordinate".into());
            }
            points.push((px, py));
        }
        visit((encoded & 1) as u8, points);
    }
    if pos != data.len() {
        return Err("coordinate block contains trailing or uncounted data".into());
    }
    Ok(())
}

#[cfg(unix)]
fn write_at(file: &File, mut data: &[u8], mut offset: u64) -> Result<(), String> {
    use std::os::unix::fs::FileExt;
    while !data.is_empty() {
        match file.write_at(data, offset) {
            Ok(0) => return Err("zero-length output write".into()),
            Ok(n) => {
                data = &data[n..];
                offset += n as u64;
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(format!("write measurement arrays: {}", error)),
        }
    }
    Ok(())
}
#[cfg(not(unix))]
fn write_at(_file: &File, _data: &[u8], _offset: u64) -> Result<(), String> {
    Err("parallel CD preprocessing requires positioned file writes on Unix".into())
}

struct Outputs {
    values: File,
    choices: File,
    estimated: File,
}
impl Outputs {
    fn create(folder: &Path, n: usize) -> Result<Self, String> {
        if !folder.is_dir() {
            return Err("measurement output directory must already exist".into());
        }
        // create_new never overwrites a published cache or user-owned file.
        let create = |name: &str, width: u64| -> Result<File, String> {
            let path = folder.join(name);
            let file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(|error| format!("create {}: {}", path.display(), error))?;
            file.set_len(product(n as u64, width)?)
                .map_err(|e| e.to_string())?;
            Ok(file)
        };
        Ok(Self {
            values: create("values.bin", 8)?,
            choices: create("choices.bin", 4)?,
            estimated: create("estimated.bin", 1)?,
        })
    }
}

fn report(path: Option<&Path>, done: u64, total: usize, jobs: usize) -> Result<(), String> {
    if let Some(path) = path {
        let temp = path.with_file_name(format!(
            "{}.tmp-{}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            std::process::id()
        ));
        let text = format!(
            "{{\"text\":\"CD measurement {} / {} (Rust, {} workers)\"}}",
            done, total, jobs
        );
        let mut file = File::create(&temp).map_err(|e| format!("write CD progress: {}", e))?;
        file.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
        drop(file);
        std::fs::rename(temp, path).map_err(|e| format!("publish CD progress: {}", e))?;
    }
    Ok(())
}

fn run(
    pack: &Pack<'_>,
    plan: &Plan,
    output: &Outputs,
    jobs: usize,
    progress: Option<&Path>,
) -> Result<(), String> {
    let tasks = pack.block_count / CHUNK_BLOCKS + usize::from(pack.block_count % CHUNK_BLOCKS != 0);
    let jobs = jobs.min(tasks.max(1));
    report(progress, 0, pack.errors, jobs)?;
    let next = AtomicUsize::new(0);
    let done = AtomicU64::new(0);
    let active = AtomicUsize::new(jobs);
    let failed = AtomicBool::new(false);
    struct Active<'a>(&'a AtomicUsize);
    impl Drop for Active<'_> {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::Release);
        }
    }
    let result = std::thread::scope(|scope| -> Result<(), String> {
        let mut workers = Vec::with_capacity(jobs);
        for _ in 0..jobs {
            let (next, done, active, failed) = (&next, &done, &active, &failed);
            workers.push(scope.spawn(move || -> Result<(), String> {
                let _active = Active(active);
                let result = (|| {
                    let mut points = Vec::new();
                    let mut values = Vec::with_capacity(CHUNK_BLOCKS * BLOCK * 8);
                    let mut choices = Vec::with_capacity(CHUNK_BLOCKS * BLOCK * 4);
                    let mut estimated = Vec::with_capacity(CHUNK_BLOCKS * BLOCK);
                    loop {
                        if failed.load(Ordering::Relaxed) {
                            break;
                        }
                        let task = next.fetch_add(1, Ordering::Relaxed);
                        if task >= tasks {
                            break;
                        }
                        let first = task * CHUNK_BLOCKS;
                        let last = (first + CHUNK_BLOCKS).min(pack.block_count);
                        values.clear();
                        choices.clear();
                        estimated.clear();
                        for block in first..last {
                            let (data, count) = pack.block(block)?;
                            if plan.chains.is_empty() {
                                // Match Python's unsupported-rule fast path:
                                // block headers still validate, but no error
                                // coordinates need to be paged in or decoded.
                                values.resize(values.len() + count * 8, 0);
                                choices.resize(choices.len() + count * 4, 0xff);
                                estimated.resize(estimated.len() + count, 0);
                                continue;
                            }
                            decode_block(
                                data,
                                count,
                                pack.precision,
                                &mut points,
                                |kind, points| {
                                    let value = measure(kind, points, plan);
                                    values.extend_from_slice(&value.ticks.to_le_bytes());
                                    choices.extend_from_slice(&value.choice.to_le_bytes());
                                    estimated.push(value.estimated);
                                },
                            )?;
                        }
                        let row = (first * BLOCK) as u64;
                        write_at(&output.values, &values, row * 8)?;
                        write_at(&output.choices, &choices, row * 4)?;
                        write_at(&output.estimated, &estimated, row)?;
                        done.fetch_add(estimated.len() as u64, Ordering::Relaxed);
                    }
                    Ok(())
                })();
                if result.is_err() {
                    failed.store(true, Ordering::Relaxed);
                }
                result
            }));
        }
        let mut latest = Instant::now();
        let mut report_error = None;
        while active.load(Ordering::Acquire) > 0 {
            if latest.elapsed() >= Duration::from_millis(250) {
                if let Err(error) =
                    report(progress, done.load(Ordering::Relaxed), pack.errors, jobs)
                {
                    failed.store(true, Ordering::Relaxed);
                    report_error = Some(error);
                }
                latest = Instant::now();
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        for worker in workers {
            match worker.join() {
                Ok(Ok(())) => {}
                Ok(Err(error)) => return Err(error),
                Err(_) => return Err("CD measurement worker panicked".into()),
            }
        }
        if let Some(error) = report_error {
            return Err(error);
        }
        Ok(())
    });
    result?;
    if done.load(Ordering::Relaxed) != pack.errors as u64 {
        return Err("incomplete CD measurement output".into());
    }
    report(progress, done.load(Ordering::Relaxed), pack.errors, jobs)
}

fn command(args: &[String]) -> Result<(), String> {
    if args == ["--protocol"] {
        println!("1");
        return Ok(());
    }
    let path = args.first().filter(|value| !value.starts_with("--"))
        .ok_or("usage: floe-index drc-measure PACK --rule-index N --plan PATH --out DIR [--jobs N] [--progress PATH]")?;
    let (mut rule, mut plan_path, mut output, mut progress) = (None, None, None, None);
    let mut jobs = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(4);
    let mut i = 1;
    while i < args.len() {
        let option = &args[i];
        let value = args
            .get(i + 1)
            .ok_or_else(|| format!("{} requires a value", option))?;
        match option.as_str() {
            "--rule-index" => {
                rule = Some(value.parse::<usize>().map_err(|_| "invalid rule index")?)
            }
            "--plan" => plan_path = Some(PathBuf::from(value)),
            "--out" => output = Some(PathBuf::from(value)),
            "--progress" => progress = Some(PathBuf::from(value)),
            "--jobs" => {
                jobs = value.parse().map_err(|_| "invalid CD worker count")?;
                if jobs == 0 || jobs > 256 {
                    return Err("CD worker count must be between 1 and 256".into());
                }
            }
            _ => return Err(format!("unknown drc-measure option {}", option)),
        }
        i += 2;
    }
    let rule = rule.ok_or("--rule-index is required")?;
    let plan_path = plan_path.ok_or("--plan is required")?;
    let output = output.ok_or("--out is required")?;
    let file = File::open(path).map_err(|e| format!("open DRC pack: {}", e))?;
    let map = unsafe { memmap2::Mmap::map(&file) }.map_err(|e| format!("map DRC pack: {}", e))?;
    let pack = Pack::parse(&map, rule)?;
    let plan = parse_plan(&std::fs::read(plan_path).map_err(|e| format!("read CD plan: {}", e))?)?;
    let outputs = Outputs::create(&output, pack.errors)?;
    let began = Instant::now();
    run(&pack, &plan, &outputs, jobs, progress.as_deref())?;
    eprintln!(
        "[drc-measure] {} errors in {:.3}s",
        pack.errors,
        began.elapsed().as_secs_f64()
    );
    Ok(())
}

pub fn drcmeasure_cmd(args: &[String]) {
    if let Err(error) = command(args) {
        eprintln!("[drc-measure] {}", error);
        std::process::exit(1);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) fn fixture(n: usize) -> Vec<u8> {
        let mut data = MAGIC.to_vec();
        data.extend_from_slice(&4u32.to_le_bytes());
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&1000.0f64.to_le_bytes());
        data.extend_from_slice(&0u64.to_le_bytes());
        data.extend_from_slice(&0u64.to_le_bytes());
        let mut blocks = Vec::new();
        for start in (0..n).step_by(BLOCK) {
            let count = (n - start).min(BLOCK);
            blocks.extend_from_slice(&(data.len() as u64).to_le_bytes());
            blocks.extend_from_slice(&(count as u32).to_le_bytes());
            blocks.extend_from_slice(&0u32.to_le_bytes());
            for value in [0i64, 0, 20, 100] {
                blocks.extend_from_slice(&value.to_le_bytes());
            }
            for _ in 0..count {
                // p4: (0,0), (20,0), (20,100), (0,100)
                data.extend_from_slice(&[8, 0, 0, 40, 0, 0, 200, 1, 39, 0]);
            }
        }
        let blob_end = data.len() as u64;
        for _ in 0..n {
            data.extend_from_slice(&[0, 0, 255, 255]);
        }
        let status = data.len() as u64;
        data.resize(data.len() + n, 0);
        let waived = data.len() as u64;
        data.extend_from_slice(&0u32.to_le_bytes());
        let block_table = data.len() as u64;
        let block_count = blocks.len() / BLOCK_RECORD;
        data.extend_from_slice(&blocks);
        let directory = data.len() as u64;
        for value in [7u32, 0, 0, 0] {
            data.extend_from_slice(&value.to_le_bytes());
        }
        for value in [0u64, n as u64, n as u64, n as u64, 0, block_count as u64] {
            data.extend_from_slice(&value.to_le_bytes());
        }
        let strings = data.len() as u64;
        data.extend_from_slice(&3u32.to_le_bytes());
        data.extend_from_slice(b"TOP");
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(b"R");
        for value in [
            40u64,
            blob_end - 40,
            blob_end,
            n as u64 * 4,
            status,
            waived,
            block_table,
            block_count as u64,
            directory,
            1,
            strings,
            0,
            strings,
            12,
            n as u64,
        ] {
            data.extend_from_slice(&value.to_le_bytes());
        }
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(MAGIC);
        data
    }

    #[test]
    fn packed_ranges_precision_and_block_counts_are_checked() {
        let data = fixture(65);
        let pack = Pack::parse(&data, 0).unwrap();
        assert_eq!((pack.errors, pack.block_count), (65, 2));
        assert!(Pack::parse(&data, 1).is_err());
        let mut bad = data.clone();
        bad[16..24].copy_from_slice(&f64::NAN.to_le_bytes());
        assert!(Pack::parse(&bad, 0).is_err());
        let mut bad = data.clone();
        bad[pack.block_table + 8..pack.block_table + 12].copy_from_slice(&63u32.to_le_bytes());
        assert!(Pack::parse(&bad, 0).is_err());
        let mut bad = data.clone();
        let foot = bad.len() - FOOTER;
        bad[foot + 24..foot + 32].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(Pack::parse(&bad, 0).is_err());
        let mut bad = data.clone();
        bad[pack.block_table..pack.block_table + 8].copy_from_slice(&0u64.to_le_bytes());
        assert!(Pack::parse(&bad, 0).is_err());
        assert_eq!(Pack::parse(&fixture(0), 0).unwrap().errors, 0);
    }

    #[test]
    fn parallel_outputs_are_byte_identical_and_empty_rules_work() {
        struct Temp(PathBuf);
        impl Drop for Temp {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp =
            Temp(std::env::temp_dir().join(format!("floe-cd-{}-{}", std::process::id(), stamp)));
        std::fs::create_dir(&temp.0).unwrap();
        let plan = Plan {
            uncertain_alternatives: false,
            chains: vec![Chain {
                metric: 0,
                ci: 0,
                op: 0,
                bound: 0.05,
                bound_ticks: Some(5000),
                options: false,
                predicates: vec![Predicate { op: 0, bound: 0.05 }],
            }],
        };
        let data = fixture(CHUNK_BLOCKS * BLOCK * 3 + 3);
        let pack = Pack::parse(&data, 0).unwrap();
        for jobs in [1, 4] {
            let folder = temp.0.join(jobs.to_string());
            std::fs::create_dir(&folder).unwrap();
            let output = Outputs::create(&folder, pack.errors).unwrap();
            run(&pack, &plan, &output, jobs, None).unwrap();
            assert!(Outputs::create(&folder, pack.errors).is_err());
        }
        for name in ["values.bin", "choices.bin", "estimated.bin"] {
            assert_eq!(
                std::fs::read(temp.0.join("1").join(name)).unwrap(),
                std::fs::read(temp.0.join("4").join(name)).unwrap()
            );
        }
        let folder = temp.0.join("empty");
        std::fs::create_dir(&folder).unwrap();
        let data = fixture(0);
        let pack = Pack::parse(&data, 0).unwrap();
        let output = Outputs::create(&folder, 0).unwrap();
        run(&pack, &plan, &output, 4, None).unwrap();
        for name in ["values.bin", "choices.bin", "estimated.bin"] {
            assert_eq!(std::fs::metadata(folder.join(name)).unwrap().len(), 0);
        }
    }

    #[test]
    fn unsupported_plan_skips_geometry_and_writes_unknown_measurements() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let folder = std::env::temp_dir().join(format!(
            "floe-cd-empty-plan-{}-{}",
            std::process::id(),
            stamp
        ));
        std::fs::create_dir(&folder).unwrap();
        let mut data = fixture(65);
        data[HEADER] = 0; // Invalid geometry proves this path never decodes it.
        let pack = Pack::parse(&data, 0).unwrap();
        let plan = Plan {
            uncertain_alternatives: true,
            chains: Vec::new(),
        };
        let output = Outputs::create(&folder, pack.errors).unwrap();
        run(&pack, &plan, &output, 4, None).unwrap();
        assert_eq!(
            std::fs::read(folder.join("values.bin")).unwrap(),
            vec![0; 65 * 8]
        );
        assert_eq!(
            std::fs::read(folder.join("choices.bin")).unwrap(),
            vec![0xff; 65 * 4]
        );
        assert_eq!(
            std::fs::read(folder.join("estimated.bin")).unwrap(),
            vec![0; 65]
        );
        drop(output);
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn varints_reject_truncation_and_overflow() {
        assert!(varint(&[0x80], &mut 0).is_err());
        assert!(varint(&[0xff; 10], &mut 0).is_err());
        let mut max = vec![0xff; 9];
        max.push(1);
        assert_eq!(varint(&max, &mut 0).unwrap(), u64::MAX);
        assert!(add_coord(i64::MAX, 2).is_err());
        assert_eq!(unzz(u64::MAX), i64::MIN);
    }
    #[test]
    fn plan_rejects_bad_header_counts_flags_and_trailing_data() {
        assert!(parse_plan(b"bad").is_err());
        let mut empty = PLAN_MAGIC.to_vec();
        empty.push(0);
        empty.extend_from_slice(&0u32.to_le_bytes());
        assert!(parse_plan(&empty).unwrap().chains.is_empty());
        let mut bad = empty.clone();
        bad[8] = 2;
        assert!(parse_plan(&bad).is_err());
        bad = empty.clone();
        bad[9..13].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse_plan(&bad).is_err());
        empty.push(0);
        assert!(parse_plan(&empty).is_err());
    }
    #[test]
    fn decode_resets_first_point_deltas_and_rejects_extra_bytes() {
        // Two 1-point polygons: first (1,-1), then (3,2), in dbu.
        let data = [2, 2, 1, 2, 4, 6];
        let mut points = Vec::new();
        let mut seen = Vec::new();
        decode_block(&data, 2, 10.0, &mut points, |kind, p| {
            seen.push((kind, p[0]))
        })
        .unwrap();
        assert_eq!(seen, vec![(0, (0.1, -0.1)), (0, (0.3, 0.2))]);
        assert!(decode_block(&data, 1, 10.0, &mut points, |_, _| {}).is_err());
        assert!(decode_block(&[0], 1, 1.0, &mut points, |_, _| {}).is_err());
        assert!(Pack::parse(&vec![0; HEADER + FOOTER], 0).is_err());
    }
}
