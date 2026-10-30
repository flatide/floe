//! CD delta groups of a DRC rule (floe_oracle/drc_delta.py `DeltaIndex`,
//! `DeltaGroups`, feature/jobdeck-drc's drc_delta_cache): every error's
//! rule-aware CD measured once into a table - five-decimal ticks, the
//! constraint it answers, whether it is an estimate - then grouped by the
//! difference to that constraint's bound, `|CD - bound|` (absolute) or
//! `|CD - bound| / |bound| * 100` (ratio), in zero-anchored half-open bins
//! of a step: by default the whole rule's widest constraint range over ten.
//! Unresolved single output edges retain their geometric length in separate,
//! exact five-decimal groups; those values are never CD deltas or ratios.
//! Grouping, its step, basis or cluster never measure again; group pages,
//! ranks and waive-filtered counts never list a group's members.
use super::measure::{marker_length_ticks, measurement_ticks, Selector};
use super::scope::Membership;
use super::source::{parallel, Source, Statuses};
use super::store::{self, BuildLock, Column, Sink, Staging};
use super::ticks::{format_ticks, json_ticks, percent_ticks, SCALE};
use crate::{check_cancelled, Error, ErrorKind, Result};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// No measurable CD (or no usable ratio).
pub const UNKNOWN: i32 = -1;
/// An output edge's length when the rule-aware CD could not be determined.
pub const GEOMETRY_LENGTH: i32 = -2;
/// Cache tables of rules this large (smaller ones measure in a moment).
pub const CACHE_THRESHOLD: u64 = 100_000;
// v3 preserves single-edge lengths separately from unmeasurable CD values.
const VERSION: u32 = 3;
const TASK: u64 = 1 << 16;
const PAGE_CHUNK: u64 = 4096;
const DENSE_BINS: u64 = 1 << 24;
const SUMMARY_SPARSE_BINS: usize = 1 << 18;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Basis {
    Absolute,
    Percent,
}
impl Basis {
    pub fn parse(text: &str) -> Result<Self> {
        match text {
            "absolute" => Ok(Self::Absolute),
            "percent" => Ok(Self::Percent),
            other => Err(Error::input(format!("unknown grouping basis {other:?}"))),
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Absolute => "absolute",
            Self::Percent => "percent",
        }
    }
    fn index(self) -> usize {
        match self {
            Self::Absolute => 0,
            Self::Percent => 1,
        }
    }
}

/// Where a pack's analysis caches go (None: memory only).
#[derive(Clone, Debug)]
pub struct Cache {
    pub dir: PathBuf,
    pub identity: String,
}
impl Cache {
    pub fn of_pack(pack: &Path) -> Result<Self> {
        Ok(Self {
            dir: store::analysis_dir(pack)?,
            identity: store::identity(pack)?,
        })
    }
}

/// The difference of one error to its constraint's bound, in ticks.
fn difference(value: i64, choice: i32, bounds: &[Option<i64>], basis: Basis) -> Option<i64> {
    if choice < 0 {
        return None;
    }
    let bound = bounds.get(choice as usize).copied().flatten().unwrap_or(0);
    let delta = (i128::from(value) - i128::from(bound)).abs();
    let delta = i64::try_from(delta).ok()?;
    match basis {
        Basis::Absolute => Some(delta),
        Basis::Percent => percent_ticks(delta, bound),
    }
}

/// A rule's measurement table.
pub struct Table {
    pub check: usize,
    pub count: u64,
    pub constraints: Vec<Value>,
    pub bound_ticks: Vec<Option<i64>>,
    values: Column<i64>,
    choices: Column<i32>,
    estimated: Column<u8>,
    /// per basis, per constraint: the (lowest, highest) difference
    ranges: [Vec<Option<(i64, i64)>>; 2],
    /// where the table is kept (its groups go beside it), when cached
    folder: Option<PathBuf>,
    pub measurable: bool,
}

/// One task's results.
struct Part {
    values: Vec<i64>,
    choices: Vec<i32>,
    estimated: Vec<u8>,
    ranges: [Vec<Option<(i64, i64)>>; 2],
}

fn merge_range(into: &mut Option<(i64, i64)>, v: i64) {
    *into = Some(match *into {
        None => (v, v),
        Some((lo, hi)) => (lo.min(v), hi.max(v)),
    });
}

fn auto_step_ranges(ranges: &[Option<(i64, i64)>]) -> i64 {
    let ranges: Vec<(i128, i128)> = ranges
        .iter()
        .flatten()
        .map(|&(lo, hi)| (i128::from(lo), i128::from(hi)))
        .collect();
    let mut step = ranges
        .iter()
        .map(|(lo, hi)| (hi - lo + 10) / 10)
        .max()
        .unwrap_or(1)
        .max(1);
    if ranges.iter().any(|(lo, hi)| hi / step - lo / step + 1 > 10) {
        step = ranges
            .iter()
            .map(|(lo, hi)| (hi - lo + 8) / 9)
            .max()
            .unwrap_or(1)
            .max(1);
    }
    i64::try_from(step).unwrap_or(i64::MAX)
}

impl Table {
    fn key(check: usize, count: u64, constraints: &[Value], cache: &Cache) -> Value {
        json!({"version": VERSION, "engine": "rust", "pack": cache.identity,
               "rule": check, "count": count, "constraints": constraints})
    }

    fn empty(check: usize, count: u64, constraints: &[Value]) -> Self {
        Self {
            check, count, constraints: constraints.to_vec(),
            bound_ticks: constraints.iter().map(|c| c.get("value").and_then(json_ticks)).collect(),
            values: Column::Owned(Vec::new()), choices: Column::Owned(Vec::new()),
            estimated: Column::Owned(Vec::new()),
            ranges: [vec![None; constraints.len()], vec![None; constraints.len()]],
            folder: None, measurable: Selector::new(constraints).can_measure(),
        }
    }

    /// Existing measurement only; never decode geometry on a cache miss.
    pub fn cached(cache: &Cache, check: usize, count: u64, constraints: &[Value]) -> Option<Self> {
        let base = Self::empty(check, count, constraints);
        let key = Self::key(check, count, constraints, cache);
        let folder = cache.dir.join("delta").join(store::digest(&key));
        let table = Self::load(&base, &folder.join("measure"), &key)?;
        Some(Self { folder: Some(folder), ..table })
    }

    /// Background preparation requires disk-backed columns. An unwritable
    /// cache is an error, never a fallback allocating arrays for every error.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_cached(
        source: &dyn Source, check: usize, constraints: &[Value], cache: &Cache,
        jobs: usize, stop: &AtomicUsize, progress: &(dyn Fn(String) + Sync),
    ) -> Result<Self> {
        check_cancelled(stop)?;
        let count = source.count(check);
        let base = Self::empty(check, count, constraints);
        if !base.measurable || count == 0 {
            return Ok(base);
        }
        if let Some(table) = Self::cached(cache, check, count, constraints) {
            return Ok(table);
        }
        let key = Self::key(check, count, constraints, cache);
        let folder = cache.dir.join("delta").join(store::digest(&key));
        let selector = Selector::new(constraints);
        let table = Self::build_cached(&base, &selector, source, &folder.join("measure"),
                                      &key, jobs.clamp(1, 4), stop, progress)?;
        Ok(Self { folder: Some(folder), ..table })
    }

    /// Measure rule `check` (or read its cached table): parallel on `jobs`
    /// threads; `progress` hears how far it got.
    pub fn measure(
        source: &dyn Source,
        check: usize,
        constraints: &[Value],
        cache: Option<&Cache>,
        store_new: bool,
        jobs: usize,
        stop: &AtomicUsize,
        progress: &(dyn Fn(String) + Sync),
    ) -> Result<Table> {
        let count = source.count(check);
        let selector = Selector::new(constraints);
        let measurable = selector.can_measure();
        let base = Self::empty(check, count, constraints);
        if let Some(cache) = cache.filter(|_| count > 0 && measurable) {
            let key = Self::key(check, count, constraints, cache);
            let folder = cache.dir.join("delta").join(store::digest(&key));
            let dir = folder.join("measure");
            if let Some(t) = Self::load(&base, &dir, &key) {
                return Ok(Table {
                    folder: Some(folder),
                    ..t
                });
            }
            if store_new {
                match Self::build_cached(&base, &selector, source, &dir, &key, jobs, stop, progress) {
                    Ok(t) => {
                        return Ok(Table {
                            folder: Some(folder),
                            ..t
                        })
                    }
                    Err(e) if e.kind == ErrorKind::Cancelled => return Err(e),
                    // a folder that cannot be written: measure in memory
                    Err(e) if e.kind == ErrorKind::Io => {
                        progress(format!("analysis cache unavailable ({e}); measuring in memory"))
                    }
                    Err(e) => return Err(e),
                }
            }
        }
        let n = count as usize;
        let out = Mutex::new((vec![0i64; n], vec![UNKNOWN; n], vec![0u8; n]));
        let ranges = Self::run(&base, &selector, source, jobs, stop, progress, &|start, part| {
            let mut o = out.lock().unwrap();
            let s = start as usize;
            o.0[s..s + part.values.len()].copy_from_slice(&part.values);
            o.1[s..s + part.choices.len()].copy_from_slice(&part.choices);
            o.2[s..s + part.estimated.len()].copy_from_slice(&part.estimated);
            Ok(())
        })?;
        let (values, choices, estimated) = out.into_inner().unwrap();
        Ok(Table {
            values: Column::Owned(values),
            choices: Column::Owned(choices),
            estimated: Column::Owned(estimated),
            ranges,
            ..base
        })
    }

    fn load(base: &Table, dir: &Path, key: &Value) -> Option<Table> {
        let m = store::manifest(dir, key)?;
        let n = base.count as usize;
        let ranges = serde_json::from_value::<[Vec<Option<(i64, i64)>>; 2]>(m.get("ranges")?.clone()).ok()?;
        if ranges.iter().any(|r| r.len() != base.constraints.len()
            || r.iter().flatten().any(|&(lo, hi)| lo < 0 || hi < lo)) {
            return None;
        }
        Some(Table {
            check: base.check,
            count: base.count,
            constraints: base.constraints.clone(),
            bound_ticks: base.bound_ticks.clone(),
            values: Column::map(&dir.join("values.bin"), n).ok()?,
            choices: Column::map(&dir.join("choices.bin"), n).ok()?,
            estimated: Column::map(&dir.join("estimated.bin"), n).ok()?,
            ranges,
            folder: None,
            measurable: base.measurable,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn build_cached(
        base: &Table,
        selector: &Selector,
        source: &dyn Source,
        dir: &Path,
        key: &Value,
        jobs: usize,
        stop: &AtomicUsize,
        progress: &(dyn Fn(String) + Sync),
    ) -> Result<Table> {
        let _lock = BuildLock::take(dir, stop)?;
        if let Some(t) = Self::load(base, dir, key) {
            return Ok(t);
        }
        let staging = Staging::new(dir)?;
        let n = base.count;
        let values = Sink::<i64>::create(&staging.dir.join("values.bin"), n)?;
        let choices = Sink::<i32>::create(&staging.dir.join("choices.bin"), n)?;
        let estimated = Sink::<u8>::create(&staging.dir.join("estimated.bin"), n)?;
        let ranges = Self::run(base, selector, source, jobs, stop, progress, &|start, part| {
            values.put(start, &part.values)?;
            choices.put(start, &part.choices)?;
            estimated.put(start, &part.estimated)
        })?;
        values.finish()?;
        choices.finish()?;
        estimated.finish()?;
        check_cancelled(stop)?;
        store::write_manifest(
            &staging.dir,
            &json!({"key": key, "ranges": ranges, "backend": "rust"}),
        )?;
        staging.publish()?;
        Self::load(base, dir, key)
            .ok_or_else(|| Error::new(ErrorKind::Cache, "completed CD measurement cache could not be read"))
    }

    /// Measure every error on `jobs` threads, handing each task's part to
    /// `deliver`; the per-constraint difference ranges of both bases.
    fn run(
        base: &Table,
        selector: &Selector,
        source: &dyn Source,
        jobs: usize,
        stop: &AtomicUsize,
        progress: &(dyn Fn(String) + Sync),
        deliver: &(dyn Fn(u64, &Part) -> Result<()> + Sync),
    ) -> Result<[Vec<Option<(i64, i64)>>; 2]> {
        let n = base.count;
        let k = base.constraints.len();
        let ranges = Mutex::new([vec![None; k], vec![None; k]]);
        let done = AtomicU64::new(0);
        let last = Mutex::new(std::time::Instant::now());
        progress(format!("CD measurement 0 / {n} (Rust, up to {jobs} workers)"));
        if base.measurable && n > 0 {
            let tasks = n.div_ceil(TASK) as usize;
            parallel(tasks, jobs, stop, &|i| {
                let start = i as u64 * TASK;
                let end = (start + TASK).min(n);
                let len = (end - start) as usize;
                let mut part = Part {
                    values: vec![0; len],
                    choices: vec![UNKNOWN; len],
                    estimated: vec![0; len],
                    ranges: [vec![None; k], vec![None; k]],
                };
                source.visit(base.check, start, end, stop, &mut |ei, kind, pts| {
                    let j = (ei - start) as usize;
                    if let Some((pick, measured)) = selector.pick(kind, pts).and_then(|pick| {
                        base.bound_ticks[pick.constraint]?;
                        let (measured, _, _) = measurement_ticks(&pick).ok()?;
                        Some((pick, measured))
                    }) {
                        part.values[j] = measured;
                        part.choices[j] = pick.constraint as i32;
                        part.estimated[j] = u8::from(pick.estimated);
                        for basis in [Basis::Absolute, Basis::Percent] {
                            if let Some(d) = difference(measured, pick.constraint as i32, &base.bound_ticks, basis) {
                                merge_range(&mut part.ranges[basis.index()][pick.constraint], d);
                            }
                        }
                    } else if let Some(length) = marker_length_ticks(kind, pts) {
                        part.values[j] = length;
                        part.choices[j] = GEOMETRY_LENGTH;
                    }
                    Ok(())
                })?;
                deliver(start, &part)?;
                {
                    let mut r = ranges.lock().unwrap();
                    for b in 0..2 {
                        for (c, v) in part.ranges[b].iter().enumerate() {
                            if let Some((lo, hi)) = v {
                                merge_range(&mut r[b][c], *lo);
                                merge_range(&mut r[b][c], *hi);
                            }
                        }
                    }
                }
                let d = done.fetch_add(len as u64, Ordering::Relaxed) + len as u64;
                let mut l = last.lock().unwrap();
                if l.elapsed().as_millis() >= 500 || d == n {
                    *l = std::time::Instant::now();
                    progress(format!("CD measurement {d} / {n} (Rust, up to {jobs} workers)"));
                }
                Ok(())
            })?;
        } else {
            for start in (0..n).step_by(TASK as usize) {
                check_cancelled(stop)?;
                let len = (n - start).min(TASK) as usize;
                deliver(
                    start,
                    &Part {
                        values: vec![0; len],
                        choices: vec![UNKNOWN; len],
                        estimated: vec![0; len],
                        ranges: [vec![None; k], vec![None; k]],
                    },
                )?;
            }
        }
        Ok(ranges.into_inner().unwrap())
    }

    pub fn value(&self, ei: u64) -> i64 {
        self.values.as_slice()[ei as usize]
    }
    pub fn choice(&self, ei: u64) -> i32 {
        self.choices.as_slice()[ei as usize]
    }
    pub fn estimated(&self, ei: u64) -> bool {
        self.estimated.as_slice()[ei as usize] != 0
    }
    pub fn difference(&self, ei: u64, basis: Basis) -> Option<i64> {
        difference(self.value(ei), self.choice(ei), &self.bound_ticks, basis)
    }

    /// The whole rule's automatic step: the widest constraint's range over
    /// ten, widened when zero-anchored bins would make an eleventh.
    pub fn auto_step(&self, basis: Basis) -> i64 {
        auto_step_ranges(&self.ranges[basis.index()])
    }
}

/// Group member ids: u32 when the rule allows.
enum Ids {
    Narrow(Column<u32>),
    Wide(Column<u64>),
}
impl Ids {
    fn get(&self, i: u64) -> u64 {
        match self {
            Self::Narrow(c) => u64::from(c.as_slice()[i as usize]),
            Self::Wide(c) => c.as_slice()[i as usize],
        }
    }
    fn search(&self, lo: u64, hi: u64, ei: u64) -> u64 {
        // first position in lo..hi whose id >= ei
        match self {
            Self::Narrow(c) => {
                lo + c.as_slice()[lo as usize..hi as usize].partition_point(|&v| u64::from(v) < ei) as u64
            }
            Self::Wide(c) => lo + c.as_slice()[lo as usize..hi as usize].partition_point(|&v| v < ei) as u64,
        }
    }
}

/// One group row of the directory.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Row {
    /// Constraint index, then GEOMETRY_LENGTH rows, then UNKNOWN (last).
    pub constraint: i32,
    /// Delta/step for CD rows; raw length ticks for GEOMETRY_LENGTH.
    pub bin: i64,
    pub total: u64,
    pub estimated: u64,
}

/// A rule's (or a cluster's) groups for one basis and step.
pub struct Groups {
    pub table: Arc<Table>,
    pub step: i64,
    pub basis: Basis,
    pub auto: bool,
    pub total: u64,
    constraint: Column<i32>,
    bins: Column<i64>,
    offsets: Column<u64>,
    estimated: Column<u64>,
    ids: Ids,
    row_of: Column<i32>,
}

/// A cached whole-rule directory summary. None is unavailable, not zero:
/// callers must not present an unprepared rule as a measured zero-group rule.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct CachedGroupSummary {
    pub count: Option<u64>,
    pub step: Option<i64>,
}

/// Check a published column's length without mapping or reading its contents.
fn cached_column_len(dir: &Path, name: &str, count: u64, width: u64) -> Option<()> {
    let metadata = std::fs::metadata(dir.join(name)).ok()?;
    (metadata.is_file() && metadata.len() == count.checked_mul(width)?).then_some(())
}

fn cached_i32(file: &File, index: u64) -> Option<i32> {
    let mut bytes = [0u8; 4];
    file.read_exact_at(&mut bytes, index.checked_mul(4)?).ok()?;
    Some(i32::from_le_bytes(bytes))
}

fn cached_u64(file: &File, index: u64) -> Option<u64> {
    let mut bytes = [0u8; 8];
    file.read_exact_at(&mut bytes, index.checked_mul(8)?).ok()?;
    Some(u64::from_le_bytes(bytes))
}

struct SummaryBins {
    dense: Vec<Option<(i64, u64, Vec<u64>)>>,
    sparse: HashSet<(usize, i64)>,
    sparse_limit: usize,
    count: u64,
    step: i64,
}

impl SummaryBins {
    fn new(ranges: &[Option<(i64, i64)>], step: i64, sparse_limit: usize) -> Self {
        let mut bits = 0u64;
        let dense = ranges.iter().map(|range| {
            let (lo, hi) = (*range)?;
            let first = lo / step;
            let length = (hi / step - first) as u64 + 1;
            if length > DENSE_BINS.saturating_sub(bits) {
                return None;
            }
            bits += length;
            Some((first, length, vec![0u64; length.div_ceil(64) as usize]))
        }).collect();
        Self { dense, sparse: HashSet::new(), sparse_limit, count: 0, step }
    }

    fn insert(&mut self, constraint: usize, delta: i64) -> Result<()> {
        let bin = delta / self.step;
        let slot = self.dense.get_mut(constraint)
            .ok_or_else(|| Error::new(ErrorKind::Cache, "invalid cached CD constraint index"))?;
        let inserted = if let Some((first, length, bits)) = slot {
            let offset = bin.checked_sub(*first).filter(|&v| v >= 0).map(|v| v as u64)
                .filter(|&v| v < *length)
                .ok_or_else(|| Error::new(ErrorKind::Cache, "cached CD difference lies outside its range"))?;
            let mask = 1u64 << (offset % 64);
            let word = &mut bits[(offset / 64) as usize];
            let fresh = *word & mask == 0;
            *word |= mask;
            fresh
        } else {
            let key = (constraint, bin);
            if self.sparse.contains(&key) {
                false
            } else {
                if self.sparse.len() >= self.sparse_limit {
                    return Err(Error::new(ErrorKind::Unsupported,
                        "too many sparse CD bins for background preparation; use a larger grouping step"));
                }
                self.sparse.insert(key)
            }
        };
        self.count += u64::from(inserted);
        Ok(())
    }
}

impl Groups {
    fn key(step: i64, basis: Basis, members_key: &str) -> Value {
        json!({"version": VERSION, "step": step, "mode": basis.name(), "members": members_key})
    }

    /// Read only an already-published whole-rule grouping. This deliberately
    /// never calls Table::measure/Groups::build: their cache-miss paths scan
    /// every error. Only manifests, file lengths and a few directory endpoints
    /// are read, with no per-error columns mapped and no review state opened.
    pub fn cached_summary(
        cache: &Cache,
        check: usize,
        count: u64,
        constraints: &[Value],
        step: Option<i64>,
        basis: Basis,
    ) -> CachedGroupSummary {
        let mut summary = CachedGroupSummary { count: None, step };
        summary.count = (|| -> Option<u64> {
            if step.is_some_and(|s| s <= 0) {
                return None;
            }
            let table_key = Table::key(check, count, constraints, cache);
            let folder = cache.dir.join("delta").join(store::digest(&table_key));
            let measure = folder.join("measure");
            let manifest = store::manifest(&measure, &table_key)?;
            let ranges = serde_json::from_value::<[Vec<Option<(i64, i64)>>; 2]>(
                manifest.get("ranges")?.clone(),
            ).ok()?;
            if ranges.iter().any(|r| r.len() != constraints.len()
                || r.iter().flatten().any(|&(lo, hi)| lo < 0 || hi < lo)) {
                return None;
            }
            for (name, width) in [("values.bin", 8), ("choices.bin", 4), ("estimated.bin", 1)] {
                cached_column_len(&measure, name, count, width)?;
            }
            let resolved = step.unwrap_or_else(|| auto_step_ranges(&ranges[basis.index()]));
            summary.step = Some(resolved);
            let key = Self::key(resolved, basis, "all");
            let summary_dir = folder.join("summaries").join(store::digest(&key));
            if let Some(saved) = store::manifest(&summary_dir, &key) {
                if saved.get("table") == Some(&table_key)
                    && saved.get("total").and_then(Value::as_u64) == Some(count) {
                    if let Some(known) = saved.get("cd_groups").and_then(Value::as_u64).filter(|&n| n <= count) {
                        return Some(known);
                    }
                }
            }
            let dir = folder.join("groups").join(store::digest(&key));
            let manifest = store::manifest(&dir, &key)?;
            let groups = manifest.get("groups")?.as_u64()?;
            let total = manifest.get("total")?.as_u64()?;
            let narrow = manifest.get("narrow")?.as_bool()?;
            if total != count || groups > count || (count > 0 && groups == 0)
                || narrow != (count <= u64::from(u32::MAX)) {
                return None;
            }
            for (name, n, width) in [
                ("constraint.bin", groups, 4), ("bins.bin", groups, 8),
                ("offsets.bin", groups.checked_add(1)?, 8), ("estimated.bin", groups, 8),
                ("ids.bin", total, if narrow { 4 } else { 8 }), ("rows.bin", count, 4),
            ] {
                cached_column_len(&dir, name, n, width)?;
            }
            let offsets = File::open(dir.join("offsets.bin")).ok()?;
            if cached_u64(&offsets, 0)? != 0 || cached_u64(&offsets, groups)? != total {
                return None;
            }
            if groups == 0 {
                return Some(0);
            }
            let constraints_file = File::open(dir.join("constraint.bin")).ok()?;
            let last = cached_i32(&constraints_file, groups - 1)?;
            if last != UNKNOWN && last != GEOMETRY_LENGTH
                && (last < 0 || last as usize >= constraints.len()) {
                return None;
            }
            // Older manifests lack cd_groups. Find the nonnegative prefix
            // with O(log groups) reads, without mapping a large directory.
            let known = if let Some(known) = manifest.get("cd_groups") {
                known.as_u64().filter(|&n| n <= groups)?
            } else {
                let (mut lo, mut hi) = (0, groups);
                while lo < hi {
                    let mid = lo + (hi - lo) / 2;
                    if cached_i32(&constraints_file, mid)? >= 0 {
                        lo = mid + 1;
                    } else {
                        hi = mid;
                    }
                }
                lo
            };
            if known > 0 {
                let first = cached_i32(&constraints_file, 0)?;
                let final_known = cached_i32(&constraints_file, known - 1)?;
                if first < 0 || final_known < first || final_known as usize >= constraints.len() {
                    return None;
                }
            }
            if known < groups {
                let first_geometry = cached_i32(&constraints_file, known)?;
                let geometry_end = groups - u64::from(last == UNKNOWN);
                if known == geometry_end {
                    if first_geometry != UNKNOWN { return None; }
                } else if first_geometry != GEOMETRY_LENGTH
                    || cached_i32(&constraints_file, geometry_end - 1)? != GEOMETRY_LENGTH {
                    return None;
                }
            }
            Some(known)
        })();
        summary
    }

    /// Prepare only a whole-rule CD-bin count, preserving the measured table
    /// for a later selection. No group IDs, row membership or status columns
    /// are created. Automatic steps need at most ten occupied bins per bound.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_summary(
        source: &dyn Source, check: usize, constraints: &[Value], cache: Option<&Cache>,
        step: Option<i64>, basis: Basis, jobs: usize, stop: &AtomicUsize,
        progress: &(dyn Fn(String) + Sync),
    ) -> Result<CachedGroupSummary> {
        check_cancelled(stop)?;
        if step.is_some_and(|s| s <= 0) {
            return Err(Error::input("grouping step must be positive fixed-precision ticks"));
        }
        let count = source.count(check);
        if count == 0 || !Selector::new(constraints).can_measure() {
            return Ok(CachedGroupSummary { count: Some(0), step: Some(step.unwrap_or(1)) });
        }
        let cache = cache.ok_or_else(|| Error::new(ErrorKind::Unsupported,
            "background CD preparation needs a writable analysis cache"))?;
        let ready = Self::cached_summary(cache, check, count, constraints, step, basis);
        if ready.count.is_some() {
            return Ok(ready);
        }
        let table = Table::prepare_cached(source, check, constraints, cache, jobs, stop, progress)?;
        let resolved = step.unwrap_or_else(|| table.auto_step(basis));
        let key = Self::key(resolved, basis, "all");
        let folder = table.folder.as_ref()
            .ok_or_else(|| Error::new(ErrorKind::Cache, "CD measurement cache is unavailable"))?;
        let dir = folder.join("summaries").join(store::digest(&key));
        let _lock = BuildLock::take(&dir, stop)?;
        let ready = Self::cached_summary(cache, check, count, constraints, step, basis);
        if ready.count.is_some() {
            return Ok(ready);
        }
        let mut bins = SummaryBins::new(&table.ranges[basis.index()], resolved, SUMMARY_SPARSE_BINS);
        let mut last = std::time::Instant::now();
        progress(format!("Counting CD groups 0 / {count}"));
        for start in (0..count).step_by(TASK as usize) {
            check_cancelled(stop)?;
            let end = (start + TASK).min(count);
            for ei in start..end {
                if let Some(delta) = table.difference(ei, basis) {
                    bins.insert(table.choice(ei) as usize, delta)?;
                }
            }
            if last.elapsed().as_millis() >= 500 || end == count {
                progress(format!("Counting CD groups {end} / {count}"));
                last = std::time::Instant::now();
            }
        }
        check_cancelled(stop)?;
        std::fs::create_dir_all(&dir)?;
        store::write_manifest(&dir, &json!({
            "key":key, "table":Table::key(check, count, constraints, cache),
            "cd_groups":bins.count, "total":count,
        }))?;
        Ok(CachedGroupSummary { count: Some(bins.count), step: Some(resolved) })
    }

    /// Group `table` (restricted to `members` when given; `members_key` names
    /// that restriction for the cache) by `step` ticks (None: the automatic
    /// step) and `basis`.
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        table: Arc<Table>,
        step: Option<i64>,
        basis: Basis,
        members: Option<&dyn Membership>,
        members_key: &str,
        stop: &AtomicUsize,
    ) -> Result<Groups> {
        let auto = step.is_none();
        let step = match step {
            Some(s) if s > 0 => s,
            Some(_) => return Err(Error::input("grouping step must be positive fixed-precision ticks")),
            None => table.auto_step(basis),
        };
        let key = Self::key(step, basis, members_key);
        let folder = table
            .folder
            .as_ref()
            .map(|f| f.join("groups").join(store::digest(&key)));
        if let Some(dir) = &folder {
            if let Some(g) = Self::load(&table, dir, &key, step, basis, auto) {
                return Ok(g);
            }
            if let Ok(_lock) = BuildLock::take(dir, stop) {
                if let Some(g) = Self::load(&table, dir, &key, step, basis, auto) {
                    return Ok(g);
                }
                let built = Self::compute(&table, step, basis, members, stop)?;
                match Self::save(&built, dir, &key, stop) {
                    Ok(()) => {
                        if let Some(g) = Self::load(&table, dir, &key, step, basis, auto) {
                            return Ok(g);
                        }
                    }
                    Err(e) if e.kind == ErrorKind::Cancelled => return Err(e),
                    Err(_) => (),
                }
                return Ok(built.into_groups(table, step, basis, auto));
            }
        }
        Ok(Self::compute(&table, step, basis, members, stop)?.into_groups(table, step, basis, auto))
    }

    fn compute(
        table: &Table,
        step: i64,
        basis: Basis,
        members: Option<&dyn Membership>,
        stop: &AtomicUsize,
    ) -> Result<Built> {
        let n = table.count;
        let k = table.constraints.len() as i64;
        // CD bins, then exact output-edge lengths, then unknown.
        let key_of = |ei: u64| -> (i64, i64) {
            match table.difference(ei, basis) {
                Some(d) => (i64::from(table.choice(ei)), d / step),
                None if table.choice(ei) == GEOMETRY_LENGTH => (k, table.value(ei)),
                None => (k + 1, 0),
            }
        };
        // dense bin counts per constraint where the rule's range allows
        let range = &table.ranges[basis.index()];
        let mut dense: Vec<Option<(i64, Vec<u64>)>> = Vec::new();
        let mut size = 0u64;
        for r in range.iter() {
            match r {
                Some((lo, hi)) => {
                    let (a, b) = (lo / step, hi / step);
                    let len = (b - a + 1) as u64;
                    if size + len <= DENSE_BINS {
                        size += len;
                        dense.push(Some((a, vec![0u64; len as usize])));
                    } else {
                        dense.push(None);
                    }
                }
                None => dense.push(Some((0, Vec::new()))),
            }
        }
        let mut sparse: HashMap<(i64, i64), u64> = HashMap::new();
        let mut unknown = 0u64;
        let mut mask = vec![true; TASK as usize];
        let mut total = 0u64;
        let mut start = 0;
        while start < n {
            check_cancelled(stop)?;
            let len = (n - start).min(TASK) as usize;
            if let Some(m) = members {
                m.mask(start, &mut mask[..len]);
            }
            for j in 0..len {
                if members.is_some() && !mask[j] {
                    continue;
                }
                total += 1;
                let (c, bin) = key_of(start + j as u64);
                if c == k + 1 {
                    unknown += 1;
                    continue;
                }
                if c == k {
                    *sparse.entry((c, bin)).or_insert(0) += 1;
                    continue;
                }
                match &mut dense[c as usize] {
                    Some((a, counts)) if !counts.is_empty() => counts[(bin - *a) as usize] += 1,
                    _ => *sparse.entry((c, bin)).or_insert(0) += 1,
                }
            }
            start += len as u64;
        }
        // The directory keeps all real CD rows in a nonnegative prefix.
        let mut rows: Vec<(i64, i64, u64)> = Vec::new();
        for (c, d) in dense.iter().enumerate() {
            match d {
                Some((a, counts)) if !counts.is_empty() => {
                    for (i, &cnt) in counts.iter().enumerate() {
                        if cnt > 0 {
                            rows.push((c as i64, a + i as i64, cnt));
                        }
                    }
                }
                _ => {
                    let mut bins: Vec<(i64, u64)> = sparse
                        .iter()
                        .filter(|((cc, _), _)| *cc == c as i64)
                        .map(|((_, b), &cnt)| (*b, cnt))
                        .collect();
                    bins.sort_unstable();
                    rows.extend(bins.into_iter().map(|(b, cnt)| (c as i64, b, cnt)));
                }
            }
        }
        let mut lengths: Vec<(i64, u64)> = sparse.iter()
            .filter(|((c, _), _)| *c == k)
            .map(|((_, length), &count)| (*length, count)).collect();
        lengths.sort_unstable();
        rows.extend(lengths.into_iter().map(|(length, count)| (k, length, count)));
        if unknown > 0 {
            rows.push((k + 1, 0, unknown));
        }
        if rows.len() as u64 >= i32::MAX as u64 {
            return Err(Error::input("too many distinct delta groups for this rule"));
        }
        let index: HashMap<(i64, i64), usize> = rows
            .iter()
            .enumerate()
            .map(|(r, &(c, b, _))| ((c, b), r))
            .collect();
        let mut offsets = Vec::with_capacity(rows.len() + 1);
        let mut at = 0u64;
        for &(_, _, cnt) in &rows {
            offsets.push(at);
            at += cnt;
        }
        offsets.push(at);
        let mut cursor: Vec<u64> = offsets[..rows.len()].to_vec();
        let mut ids = vec![0u64; total as usize];
        let mut row_of = vec![-1i32; n as usize];
        let mut estimated = vec![0u64; rows.len()];
        let mut start = 0;
        while start < n {
            check_cancelled(stop)?;
            let len = (n - start).min(TASK) as usize;
            if let Some(m) = members {
                m.mask(start, &mut mask[..len]);
            }
            for j in 0..len {
                if members.is_some() && !mask[j] {
                    continue;
                }
                let ei = start + j as u64;
                let r = index[&key_of(ei)];
                ids[cursor[r] as usize] = ei;
                cursor[r] += 1;
                row_of[ei as usize] = r as i32;
                estimated[r] += u64::from(table.estimated(ei));
            }
            start += len as u64;
        }
        Ok(Built {
            constraint: rows
                .iter()
                .map(|&(c, _, _)| if c == k { GEOMETRY_LENGTH } else if c == k + 1 { UNKNOWN } else { c as i32 })
                .collect(),
            bins: rows.iter().map(|&(_, b, _)| b).collect(),
            offsets,
            estimated,
            ids,
            row_of,
            total,
            narrow: n <= u64::from(u32::MAX),
        })
    }

    fn save(b: &Built, dir: &Path, key: &Value, stop: &AtomicUsize) -> Result<()> {
        let staging = Staging::new(dir)?;
        store::write_column(&staging.dir.join("constraint.bin"), &b.constraint)?;
        store::write_column(&staging.dir.join("bins.bin"), &b.bins)?;
        store::write_column(&staging.dir.join("offsets.bin"), &b.offsets)?;
        store::write_column(&staging.dir.join("estimated.bin"), &b.estimated)?;
        if b.narrow {
            let narrow: Vec<u32> = b.ids.iter().map(|&v| v as u32).collect();
            store::write_column(&staging.dir.join("ids.bin"), &narrow)?;
        } else {
            store::write_column(&staging.dir.join("ids.bin"), &b.ids)?;
        }
        store::write_column(&staging.dir.join("rows.bin"), &b.row_of)?;
        check_cancelled(stop)?;
        store::write_manifest(
            &staging.dir,
            &json!({"key": key, "groups": b.constraint.len(), "total": b.total, "narrow": b.narrow,
                "cd_groups": b.constraint.partition_point(|&c| c >= 0)}),
        )?;
        staging.publish()
    }

    fn load(table: &Arc<Table>, dir: &Path, key: &Value, step: i64, basis: Basis, auto: bool) -> Option<Groups> {
        let m = store::manifest(dir, key)?;
        let g = m.get("groups")?.as_u64()? as usize;
        let total = m.get("total")?.as_u64()?;
        let narrow = m.get("narrow")?.as_bool()?;
        let n = table.count as usize;
        let groups = Groups {
            table: Arc::clone(table),
            step,
            basis,
            auto,
            total,
            constraint: Column::map(&dir.join("constraint.bin"), g).ok()?,
            bins: Column::map(&dir.join("bins.bin"), g).ok()?,
            offsets: Column::map(&dir.join("offsets.bin"), g + 1).ok()?,
            estimated: Column::map(&dir.join("estimated.bin"), g).ok()?,
            ids: if narrow {
                Ids::Narrow(Column::map(&dir.join("ids.bin"), total as usize).ok()?)
            } else {
                Ids::Wide(Column::map(&dir.join("ids.bin"), total as usize).ok()?)
            },
            row_of: Column::map(&dir.join("rows.bin"), n).ok()?,
        };
        let o = groups.offsets.as_slice();
        (o.first() == Some(&0) && o.last() == Some(&total)).then_some(groups)
    }

    pub fn len(&self) -> usize {
        self.constraint.len()
    }
    /// Actual CD bins, excluding geometric lengths and unknown values.
    pub fn cd_group_count(&self) -> usize {
        self.constraint.as_slice().partition_point(|&c| c >= 0)
    }
    pub fn geometry_group_count(&self) -> usize {
        self.len() - self.cd_group_count()
            - usize::from(self.constraint.as_slice().last() == Some(&UNKNOWN))
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn row(&self, r: usize) -> Row {
        let o = self.offsets.as_slice();
        Row {
            constraint: self.constraint.as_slice()[r],
            bin: self.bins.as_slice()[r],
            total: o[r + 1] - o[r],
            estimated: self.estimated.as_slice()[r],
        }
    }
    pub fn estimated_total(&self) -> u64 {
        self.estimated.as_slice().iter().sum()
    }
    /// The row of `(constraint, bin)` - a group kept across rebuilds.
    pub fn find(&self, constraint: i32, bin: i64) -> Option<usize> {
        let c = self.constraint.as_slice();
        let b = self.bins.as_slice();
        if constraint == UNKNOWN {
            return (bin == 0 && c.last() == Some(&UNKNOWN)).then(|| c.len() - 1);
        }
        let known = self.cd_group_count();
        let (lo, hi) = if constraint == GEOMETRY_LENGTH {
            (known, known + self.geometry_group_count())
        } else if constraint >= 0 {
            (c[..known].partition_point(|&v| v < constraint),
             c[..known].partition_point(|&v| v <= constraint))
        } else {
            return None;
        };
        let r = lo + b[lo..hi].partition_point(|&v| v < bin);
        (r < hi && b[r] == bin).then_some(r)
    }
    /// Compact upper-bound label; numbering is local to the condition.
    pub fn label(&self, r: usize) -> String {
        let constraints = self.constraint.as_slice();
        let constraint = constraints[r];
        if constraint == GEOMETRY_LENGTH {
            return format!("C? #{}", r - self.cd_group_count() + 1);
        }
        if constraint < 0 {
            return "Unmeasurable".into();
        }
        let known = self.cd_group_count();
        let first = constraints[..known].partition_point(|&c| c < constraint);
        let upper = self.bins.as_slice()[r]
            .saturating_mul(self.step)
            .saturating_add(self.step);
        format!(
            "C{} #{} <={}",
            constraint as usize + 1,
            r - first + 1,
            format_ticks(upper, false)
        )
    }
    /// Its directory name (floe_oracle drc_delta `DeltaGroup.name`).
    pub fn name(&self, r: usize) -> String {
        let row = self.row(r);
        let name = if row.constraint == GEOMETRY_LENGTH {
            format!("{} um · edge length · CD unresolved", format_ticks(row.bin, false))
        } else if row.constraint < 0 {
            match self.basis {
                Basis::Percent => "Unmeasurable / ratio unavailable".to_string(),
                Basis::Absolute => "Unmeasurable".to_string(),
            }
        } else {
            let ci = row.constraint as usize;
            let con = &self.table.constraints[ci];
            let metric = match con.get("metric") {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Null) | None => "None".into(),
                Some(v) => v.to_string(),
            };
            let op = match con.get("op") {
                Some(Value::String(s)) => s.clone(),
                None => "?".into(),
                Some(Value::Null) => "None".into(),
                Some(v) => v.to_string(),
            };
            let unit = match self.basis {
                Basis::Percent => "%",
                Basis::Absolute if metric == "area" => "um2",
                Basis::Absolute => "um",
            };
            let low = row.bin.saturating_mul(self.step);
            format!(
                "condition {} · {metric} {op} {}: [{}, {}) {unit}",
                ci + 1,
                self.table.bound_ticks[ci].map_or("?".into(), |t| format_ticks(t, false)),
                format_ticks(low, false),
                format_ticks(low.saturating_add(self.step), false)
            )
        };
        let mut name = format!("Group #{} · {name}", r + 1);
        if row.estimated > 0 {
            name.push_str(&format!(" · estimated {} total", row.estimated));
        }
        name
    }
    pub fn row_of(&self, ei: u64) -> Option<usize> {
        self.row_of
            .as_slice()
            .get(ei as usize)
            .and_then(|&r| (r >= 0).then_some(r as usize))
    }
    fn span(&self, r: usize) -> (u64, u64) {
        let o = self.offsets.as_slice();
        (o[r], o[r + 1])
    }
    /// A group's membership, for workers.
    pub fn members(self: &Arc<Self>, r: usize) -> GroupMembers {
        GroupMembers {
            groups: Arc::clone(self),
            row: r as i32,
        }
    }
}

struct Built {
    constraint: Vec<i32>,
    bins: Vec<i64>,
    offsets: Vec<u64>,
    estimated: Vec<u64>,
    ids: Vec<u64>,
    row_of: Vec<i32>,
    total: u64,
    narrow: bool,
}
impl Built {
    fn into_groups(self, table: Arc<Table>, step: i64, basis: Basis, auto: bool) -> Groups {
        Groups {
            table,
            step,
            basis,
            auto,
            total: self.total,
            constraint: Column::Owned(self.constraint),
            bins: Column::Owned(self.bins),
            offsets: Column::Owned(self.offsets),
            estimated: Column::Owned(self.estimated),
            ids: if self.narrow {
                Ids::Narrow(Column::Owned(self.ids.iter().map(|&v| v as u32).collect()))
            } else {
                Ids::Wide(Column::Owned(self.ids))
            },
            row_of: Column::Owned(self.row_of),
        }
    }
}

/// One group's members (a worker's snapshot).
#[derive(Clone)]
pub struct GroupMembers {
    groups: Arc<Groups>,
    row: i32,
}
impl Membership for GroupMembers {
    fn mask(&self, start: u64, out: &mut [bool]) {
        let rows = self.groups.row_of.as_slice();
        for (i, o) in out.iter_mut().enumerate() {
            *o = rows.get(start as usize + i) == Some(&self.row);
        }
    }
    fn contains(&self, ei: u64) -> bool {
        self.groups.row_of.as_slice().get(ei as usize) == Some(&self.row)
    }
}

/// The review side of a grouping: each group's waived count and per
/// 4096-member running counts, so filtered pages and ranks read one chunk
/// (floe_oracle drc_delta `reset_status`, `status_changed`).
pub struct GroupStatus {
    pub groups: Arc<Groups>,
    waived: Vec<u64>,
    /// per row: where its chunk counts start in `prefix`
    chunks: Vec<u64>,
    prefix: Vec<u64>,
    bits: Vec<u64>,
}

impl GroupStatus {
    pub fn new(groups: Arc<Groups>, statuses: &Statuses, stop: &AtomicUsize) -> Result<Self> {
        let g = groups.len();
        let mut chunks = Vec::with_capacity(g + 1);
        let mut at = 0u64;
        for r in 0..g {
            chunks.push(at);
            at += groups.row(r).total.div_ceil(PAGE_CHUNK);
        }
        chunks.push(at);
        let mut s = Self {
            groups,
            waived: vec![0; g],
            chunks,
            prefix: vec![0; at as usize],
            bits: Vec::new(),
        };
        s.reset(statuses, stop)?;
        Ok(s)
    }

    /// Recount from the stored statuses (after a bulk change).
    pub fn reset(&mut self, statuses: &Statuses, stop: &AtomicUsize) -> Result<()> {
        let groups = Arc::clone(&self.groups);
        let n = groups.table.count;
        let check = groups.table.check;
        self.waived.iter_mut().for_each(|w| *w = 0);
        self.prefix.iter_mut().for_each(|p| *p = 0);
        self.bits = vec![0u64; n.div_ceil(64) as usize];
        let mut position = vec![0u64; groups.len()];
        let mut start = 0;
        while start < n {
            check_cancelled(stop)?;
            let len = (n - start).min(TASK);
            let st = statuses.read(check, start, len)?;
            for (j, s) in st.iter().enumerate() {
                let ei = start + j as u64;
                let waived = *s == 1;
                if waived {
                    self.bits[(ei / 64) as usize] |= 1 << (ei % 64);
                }
                if let Some(r) = groups.row_of(ei) {
                    if waived {
                        self.waived[r] += 1;
                        self.prefix[(self.chunks[r] + position[r] / PAGE_CHUNK) as usize] += 1;
                    }
                    position[r] += 1;
                }
            }
            start += len;
        }
        for r in 0..groups.len() {
            let (a, b) = (self.chunks[r] as usize, self.chunks[r + 1] as usize);
            for i in a + 1..b {
                self.prefix[i] += self.prefix[i - 1];
            }
        }
        Ok(())
    }

    fn waived_bit(&self, ei: u64) -> bool {
        (self.bits[(ei / 64) as usize] >> (ei % 64)) & 1 == 1
    }

    /// Error `ei`'s status became `waived`.
    pub fn changed(&mut self, ei: u64, waived: bool) {
        if ei >= self.groups.table.count || self.waived_bit(ei) == waived {
            return;
        }
        self.bits[(ei / 64) as usize] ^= 1 << (ei % 64);
        let Some(r) = self.groups.row_of(ei) else {
            return;
        };
        let (lo, hi) = self.groups.span(r);
        let pos = self.groups.ids.search(lo, hi, ei) - lo;
        let (a, b) = (self.chunks[r] as usize, self.chunks[r + 1] as usize);
        for p in &mut self.prefix[a + (pos / PAGE_CHUNK) as usize..b] {
            if waived {
                *p += 1;
            } else {
                *p -= 1;
            }
        }
        if waived {
            self.waived[r] += 1;
        } else {
            self.waived[r] -= 1;
        }
    }

    pub fn count(&self, r: usize, waived: Option<bool>) -> u64 {
        let total = self.groups.row(r).total;
        match waived {
            None => total,
            Some(true) => self.waived[r],
            Some(false) => total - self.waived[r],
        }
    }

    /// Running counts of row `r`'s chunks for `waived`.
    fn running(&self, r: usize, waived: bool) -> Vec<u64> {
        let (a, b) = (self.chunks[r] as usize, self.chunks[r + 1] as usize);
        let total = self.groups.row(r).total;
        self.prefix[a..b]
            .iter()
            .enumerate()
            .map(|(i, &w)| {
                if waived {
                    w
                } else {
                    ((i as u64 + 1) * PAGE_CHUNK).min(total) - w
                }
            })
            .collect()
    }

    /// The members `start..start+limit` of row `r` (rule order) whose
    /// waived-ness is `waived` (None: all).
    pub fn page(&self, r: usize, start: u64, limit: u64, waived: Option<bool>) -> Vec<u64> {
        let (lo, hi) = self.groups.span(r);
        let ids = &self.groups.ids;
        let Some(w) = waived else {
            return (lo + start..(lo + start + limit).min(hi)).map(|i| ids.get(i)).collect();
        };
        let mut out = Vec::new();
        if limit == 0 || start >= self.count(r, waived) {
            return out;
        }
        let prefix = self.running(r, w);
        let mut chunk = prefix.partition_point(|&p| p <= start);
        let mut before = if chunk > 0 { prefix[chunk - 1] } else { 0 };
        while chunk < prefix.len() && (out.len() as u64) < limit {
            let a = lo + chunk as u64 * PAGE_CHUNK;
            let b = (a + PAGE_CHUNK).min(hi);
            let skip = start.saturating_sub(before);
            out.extend(
                (a..b)
                    .map(|i| ids.get(i))
                    .filter(|&ei| self.waived_bit(ei) == w)
                    .skip(skip as usize)
                    .take((limit - out.len() as u64) as usize),
            );
            before = prefix[chunk];
            chunk = prefix.partition_point(|&p| p <= before);
        }
        out
    }

    /// The rank of `ei` in row `r` among `waived` (None: all) members.
    pub fn rank(&self, r: usize, ei: u64, waived: Option<bool>) -> Option<u64> {
        if self.groups.row_of(ei) != Some(r) {
            return None;
        }
        let (lo, hi) = self.groups.span(r);
        let rank = self.groups.ids.search(lo, hi, ei) - lo;
        let Some(w) = waived else {
            return Some(rank);
        };
        if self.waived_bit(ei) != w {
            return None;
        }
        let prefix = self.running(r, w);
        let chunk = rank / PAGE_CHUNK;
        let before = if chunk > 0 { prefix[(chunk - 1) as usize] } else { 0 };
        let n = (lo + chunk * PAGE_CHUNK..lo + rank)
            .filter(|&i| self.waived_bit(self.groups.ids.get(i)) == w)
            .count() as u64;
        Some(before + n)
    }
}

/// `SCALE` re-exported for callers formatting steps.
pub const TICKS_PER_UNIT: i64 = SCALE;

#[cfg(test)]
mod cached_summary_tests {
    use super::*;
    use crate::drc::analysis::source::ErrorBox;

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    struct Fixture {
        root: PathBuf,
        cache: Cache,
        constraints: Vec<Value>,
        count: u64,
        measure: PathBuf,
        groups: PathBuf,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn sparse(path: &Path, count: u64, width: u64) {
        File::create(path).unwrap().set_len(count * width).unwrap();
    }

    impl Fixture {
        fn new(count: u64, rows: &[i32], members: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "floe-cached-cd-summary-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed),
            ));
            std::fs::create_dir(&root).unwrap();
            let cache = Cache { dir: root.join("analysis"), identity: "pack-one".into() };
            let constraints = vec![json!({"metric":"width", "op":"<", "value":0.05, "text":"INT M1 < 0.05"})];
            let key = Table::key(0, count, &constraints, &cache);
            let folder = cache.dir.join("delta").join(store::digest(&key));
            let measure = folder.join("measure");
            std::fs::create_dir_all(&measure).unwrap();
            let known = rows.iter().any(|&r| r >= 0);
            let ranges = if known {
                [vec![Some((100, 199))], vec![Some((1000, 1999))]]
            } else {
                [vec![None], vec![None]]
            };
            store::write_manifest(&measure, &json!({"key":key,"ranges":ranges,"backend":"rust"})).unwrap();
            for (name, width) in [("values.bin",8), ("choices.bin",4), ("estimated.bin",1)] {
                sparse(&measure.join(name), count, width);
            }
            let group_key = Groups::key(if known { 10 } else { 1 }, Basis::Absolute, members);
            let groups = folder.join("groups").join(store::digest(&group_key));
            std::fs::create_dir_all(&groups).unwrap();
            store::write_column(&groups.join("constraint.bin"), rows).unwrap();
            store::write_column(&groups.join("bins.bin"), &vec![0i64; rows.len()]).unwrap();
            store::write_column(&groups.join("estimated.bin"), &vec![0u64; rows.len()]).unwrap();
            let mut offsets: Vec<u64> = (0..rows.len() as u64).collect();
            offsets.push(count);
            store::write_column(&groups.join("offsets.bin"), &offsets).unwrap();
            let narrow = count <= u64::from(u32::MAX);
            sparse(&groups.join("ids.bin"), count, if narrow { 4 } else { 8 });
            sparse(&groups.join("rows.bin"), count, 4);
            // The old published format has no extra cd_groups summary field.
            store::write_manifest(&groups, &json!({"key":group_key,"groups":rows.len(),"total":count,"narrow":narrow})).unwrap();
            Self { root, cache, constraints, count, measure, groups }
        }

        fn summary(&self, step: Option<i64>, basis: Basis) -> CachedGroupSummary {
            Groups::cached_summary(&self.cache, 0, self.count, &self.constraints, step, basis)
        }
    }

    #[test]
    fn old_cache_summary_excludes_unknown_without_reading_error_columns() {
        // Sparse multi-GB columns contain no actual measured values or IDs.
        // A summary needs only their lengths and the tiny directory endpoints.
        let f = Fixture::new(200_000_000, &[0, 0, UNKNOWN], "all");
        assert_eq!(f.summary(None, Basis::Absolute), CachedGroupSummary { count: Some(2), step: Some(10) });
        assert_eq!(f.summary(Some(10), Basis::Absolute).count, Some(2));
        let unknown = Fixture::new(10, &[UNKNOWN], "all");
        assert_eq!(unknown.summary(None, Basis::Absolute), CachedGroupSummary { count: Some(0), step: Some(1) });
        for rows in [vec![0, 0, GEOMETRY_LENGTH, GEOMETRY_LENGTH, UNKNOWN],
                     vec![0, 0, GEOMETRY_LENGTH], vec![GEOMETRY_LENGTH, UNKNOWN],
                     vec![GEOMETRY_LENGTH, GEOMETRY_LENGTH]] {
            let f = Fixture::new(200_000_000, &rows, "all");
            let known = rows.iter().filter(|&&c| c >= 0).count() as u64;
            assert_eq!(f.summary(None, Basis::Absolute).count, Some(known));
        }
    }

    #[test]
    fn summary_requires_exact_basis_step_identity_constraints_and_whole_rule_scope() {
        let f = Fixture::new(10, &[0, UNKNOWN], "all");
        assert_eq!(f.summary(Some(20), Basis::Absolute), CachedGroupSummary { count: None, step: Some(20) });
        assert_eq!(f.summary(None, Basis::Percent), CachedGroupSummary { count: None, step: Some(100) });
        let other = Cache { identity: "pack-replaced".into(), ..f.cache.clone() };
        assert_eq!(Groups::cached_summary(&other, 0, f.count, &f.constraints, None, Basis::Absolute).count, None);
        let mut changed = f.constraints.clone();
        changed[0]["text"] = json!("INT M2 < 0.05");
        assert_eq!(Groups::cached_summary(&f.cache, 0, f.count, &changed, None, Basis::Absolute).count, None);
        assert_eq!(Groups::cached_summary(&f.cache, 1, f.count, &f.constraints, None, Basis::Absolute).count, None);
        assert_eq!(Groups::cached_summary(&f.cache, 0, f.count + 1, &f.constraints, None, Basis::Absolute).count, None);
        let cluster = Fixture::new(10, &[0], "cluster-subset");
        assert_eq!(cluster.summary(None, Basis::Absolute).count, None);
    }

    #[test]
    fn summary_refuses_missing_truncated_and_incomplete_cache_without_building() {
        let f = Fixture::new(10, &[0, UNKNOWN], "all");
        std::fs::remove_file(f.groups.join("rows.bin")).unwrap();
        assert_eq!(f.summary(None, Basis::Absolute).count, None);
        assert!(!f.groups.join("rows.bin").exists());
        sparse(&f.groups.join("rows.bin"), f.count, 4);
        File::options().write(true).open(f.measure.join("values.bin")).unwrap().set_len(1).unwrap();
        assert_eq!(f.summary(None, Basis::Absolute).count, None);
        sparse(&f.measure.join("values.bin"), f.count, 8);
        std::fs::remove_file(f.measure.join("complete.json")).unwrap();
        assert_eq!(f.summary(None, Basis::Absolute), CachedGroupSummary { count: None, step: None });
        assert!(!f.measure.join("complete.json").exists());
        let missing = Cache { dir: f.root.join("missing"), ..f.cache.clone() };
        assert_eq!(Groups::cached_summary(&missing, 0, f.count, &f.constraints, None, Basis::Absolute).count, None);
        assert!(!missing.dir.exists());
    }

    #[test]
    fn live_cd_group_count_excludes_the_unmeasurable_row() {
        let table = Arc::new(Table {
            check: 0, count: 3,
            constraints: vec![json!({"metric":"width","op":"<","value":0.05})],
            bound_ticks: vec![Some(5000)],
            values: Column::Owned(vec![1000, 2000, 0]),
            choices: Column::Owned(vec![0, 0, UNKNOWN]),
            estimated: Column::Owned(vec![0, 0, 0]),
            ranges: [vec![Some((3000, 4000))], vec![Some((6_000_000, 8_000_000))]],
            folder: None, measurable: true,
        });
        let groups = Groups::build(table, Some(1000), Basis::Absolute, None, "all", &AtomicUsize::new(0)).unwrap();
        assert_eq!(groups.len(), 3);
        assert_eq!(groups.cd_group_count(), 2);
        assert_eq!(groups.row(2).constraint, UNKNOWN);
    }

    struct GeometrySource {
        shapes: Vec<(char, Vec<[f64; 2]>)>,
        visits: AtomicUsize,
    }

    impl GeometrySource {
        fn new(shapes: Vec<(char, Vec<[f64; 2]>)>) -> Self {
            Self { shapes, visits: AtomicUsize::new(0) }
        }
    }

    impl Source for GeometrySource {
        fn check_count(&self) -> usize { 1 }
        fn count(&self, _: usize) -> u64 { self.shapes.len() as u64 }
        fn precision(&self) -> f64 { 100000.0 }
        fn visit(&self, _: usize, start: u64, end: u64, stop: &AtomicUsize,
                 visit: &mut dyn FnMut(u64, char, &[[f64; 2]]) -> Result<()>) -> Result<()> {
            self.visits.fetch_add(1, Ordering::Relaxed);
            for ei in start..end {
                check_cancelled(stop)?;
                let (kind, points) = &self.shapes[ei as usize];
                visit(ei, *kind, points)?;
            }
            Ok(())
        }
        fn visit_boxes(&self, _: usize, _: u64, _: u64, _: &AtomicUsize,
                       _: &mut dyn FnMut(u64, ErrorBox) -> Result<()>) -> Result<()> {
            panic!("geometry grouping must not revisit marker boxes")
        }
    }

    fn single_edge(length: f64) -> (char, Vec<[f64; 2]>) {
        ('e', vec![[0.0, 0.0], [length, 0.0]])
    }

    fn measured_pair() -> (char, Vec<[f64; 2]>) {
        ('e', vec![[0.0, 0.0], [0.1, 0.0], [0.0, 0.02], [0.1, 0.02]])
    }

    fn unknown_triangle() -> (char, Vec<[f64; 2]>) {
        ('p', vec![[0.0, 0.0], [0.1, 0.0], [0.0, 0.1]])
    }

    fn width_constraint() -> Vec<Value> {
        vec![json!({"metric":"width","op":"<","value":0.05,"text":"INT M1 < 0.05"})]
    }

    #[test]
    fn fallback_lengths_keep_five_decimals_without_replacing_valid_cd() {
        let a = [425.6735, 1517.0725];
        let b = [425.9110, 1517.0725];
        let source = GeometrySource::new(vec![('e', vec![a, b]), ('e', vec![b, a]),
            single_edge(0.237509), single_edge(0.25750), measured_pair(), unknown_triangle()]);
        let stop = AtomicUsize::new(0);
        let table = Table::measure(&source, 0, &width_constraint(), None, false, 2, &stop, &|_| {}).unwrap();
        assert_eq!(table.values.as_slice(), &[23750, 23750, 23751, 25750, 2000, 0]);
        assert_eq!(table.choices.as_slice(), &[GEOMETRY_LENGTH, GEOMETRY_LENGTH,
            GEOMETRY_LENGTH, GEOMETRY_LENGTH, 0, UNKNOWN]);
        for ei in 0..4 {
            assert_eq!(table.difference(ei, Basis::Absolute), None);
            assert_eq!(table.difference(ei, Basis::Percent), None);
            assert!(!table.estimated(ei));
        }
        assert_eq!(table.difference(4, Basis::Absolute), Some(3000));
        assert_eq!(table.ranges[0], vec![Some((3000, 3000))]);
        let length_constraint = vec![json!({"metric":"length","op":"<","value":0.3})];
        let length_cd = Table::measure(&source, 0, &length_constraint, None, false, 1, &stop, &|_| {}).unwrap();
        for ei in 0..4 {
            assert_eq!(length_cd.choice(ei), 0);
            assert!(length_cd.difference(ei, Basis::Absolute).is_some());
        }
        assert_eq!(marker_length_ticks('e', &[[0.0, 0.0], [0.3, 0.4]]), Some(50000));
        assert_eq!(marker_length_ticks('e', &[[0.0, 0.0], [f64::NAN, 0.0]]), None);
        assert_eq!(marker_length_ticks('e', &[[0.0, 0.0], [1.0e14, 0.0]]), None);
        assert_eq!(marker_length_ticks('p', &[[0.0, 0.0], [0.2, 0.0]]), None);
        assert_eq!(marker_length_ticks('e', &measured_pair().1), None);
    }

    #[test]
    fn length_directory_sorts_over_fifty_rows_and_preserves_scope_status_and_basis() {
        use crate::drc::analysis::scope::Runs;
        let mut shapes: Vec<_> = (0..61).rev().map(|i| single_edge((10000 + i) as f64 / 100000.0)).collect();
        shapes.extend([single_edge(0.1), measured_pair(), unknown_triangle()]);
        let source = GeometrySource::new(shapes);
        let stop = AtomicUsize::new(0);
        let table = Arc::new(Table::measure(&source, 0, &width_constraint(), None, false, 1, &stop, &|_| {}).unwrap());
        let groups = Arc::new(Groups::build(Arc::clone(&table), Some(5000), Basis::Absolute, None, "all", &stop).unwrap());
        assert_eq!((groups.len(), groups.cd_group_count(), groups.geometry_group_count()), (63, 1, 61));
        for i in 0..61 {
            let row = i + 1;
            assert_eq!(groups.find(GEOMETRY_LENGTH, 10000 + i as i64), Some(row));
            assert_eq!(groups.row(row).constraint, GEOMETRY_LENGTH);
            assert_eq!(groups.row(row).bin, 10000 + i as i64);
            assert!(groups.name(row).starts_with(&format!("Group #{} · {} um", row + 1,
                format_ticks(10000 + i as i64, false))));
            assert!(groups.name(row).ends_with("edge length · CD unresolved"));
        }
        assert_eq!(groups.find(GEOMETRY_LENGTH, 9999), None);
        assert_eq!(groups.find(UNKNOWN, 0), Some(62));
        assert_eq!(groups.find(-3, 0), None);
        let percent = Groups::build(Arc::clone(&table), Some(1), Basis::Percent, None, "all", &stop).unwrap();
        for row in 1..62 {
            assert_eq!(percent.row(row), groups.row(row));
            assert_eq!(percent.name(row), groups.name(row));
        }
        let mut status = GroupStatus::new(Arc::clone(&groups), &Statuses::None, &stop).unwrap();
        assert_eq!(status.page(1, 0, 1000, None), vec![60, 61]);
        status.changed(60, true);
        assert_eq!(status.count(1, Some(true)), 1);
        assert_eq!(status.page(1, 0, 1000, Some(false)), vec![61]);
        assert_eq!(status.rank(1, 60, Some(true)), Some(0));
        let membership = groups.members(1);
        assert!(membership.contains(60) && membership.contains(61));
        assert!(!membership.contains(0));
        let cluster = Runs::new(vec![0, 61], vec![1, 62]);
        let subset = Groups::build(table, Some(100000), Basis::Percent, Some(&cluster), "cluster", &stop).unwrap();
        assert_eq!((subset.total, subset.cd_group_count(), subset.geometry_group_count()), (2, 0, 2));
        assert_eq!(subset.find(GEOMETRY_LENGTH, 10000), Some(0));
        assert_eq!(subset.find(GEOMETRY_LENGTH, 10060), Some(1));
        assert_eq!(subset.row_of(0), Some(1));
        assert_eq!(subset.row_of(1), None);
    }

    #[test]
    fn cached_geometry_lengths_never_inflate_cd_summaries_or_repeat_geometry_visits() {
        for mixed in [false, true] {
            let mut shapes = vec![single_edge(0.2375), single_edge(0.2575)];
            if mixed { shapes.extend([measured_pair(), unknown_triangle()]); }
            let source = GeometrySource::new(shapes);
            let count = source.count(0);
            let f = Fixture::new(count, &[UNKNOWN], "all");
            std::fs::remove_dir_all(&f.cache.dir).unwrap();
            let stop = AtomicUsize::new(0);
            let summary = Groups::prepare_summary(&source, 0, &f.constraints, Some(&f.cache),
                None, Basis::Absolute, 1, &stop, &|_| {}).unwrap();
            assert_eq!(summary.count, Some(u64::from(mixed)));
            let visits = source.visits.load(Ordering::Relaxed);
            assert!(!f.measure.parent().unwrap().join("groups").exists());
            let table = Arc::new(Table::cached(&f.cache, 0, count, &f.constraints).unwrap());
            assert_eq!((table.value(0), table.choice(0)), (23750, GEOMETRY_LENGTH));
            assert_eq!(table.difference(0, Basis::Absolute), None);
            let groups = Groups::build(Arc::clone(&table), None, Basis::Absolute, None, "all", &stop).unwrap();
            assert_eq!(groups.cd_group_count(), usize::from(mixed));
            assert_eq!(groups.geometry_group_count(), 2);
            std::fs::remove_dir_all(f.measure.parent().unwrap().join("summaries")).unwrap();
            assert_eq!(f.summary(None, Basis::Absolute).count, Some(u64::from(mixed)));
            let group_dir = f.measure.parent().unwrap().join("groups")
                .join(store::digest(&Groups::key(groups.step, Basis::Absolute, "all")));
            let mut manifest = store::manifest(&group_dir, &Groups::key(groups.step, Basis::Absolute, "all")).unwrap();
            manifest.as_object_mut().unwrap().remove("cd_groups");
            store::write_manifest(&group_dir, &manifest).unwrap();
            assert_eq!(f.summary(None, Basis::Absolute).count, Some(u64::from(mixed)));
            let warm = Groups::prepare_summary(&source, 0, &f.constraints, Some(&f.cache),
                None, Basis::Absolute, 1, &stop, &|_| panic!("warm grouping must not remeasure")).unwrap();
            assert_eq!(warm, summary);
            let loaded = Groups::build(table, None, Basis::Absolute, None, "all", &stop).unwrap();
            assert_eq!(loaded.find(GEOMETRY_LENGTH, 25750), Some(usize::from(mixed) + 1));
            assert_eq!(source.visits.load(Ordering::Relaxed), visits);
        }
    }

    #[test]
    fn v2_unknown_single_edges_are_remeasured_as_geometry_lengths() {
        let f = Fixture::new(2, &[UNKNOWN], "all");
        store::write_column(&f.measure.join("choices.bin"), &[UNKNOWN, UNKNOWN]).unwrap();
        let mut old_key = Table::key(0, 2, &f.constraints, &f.cache);
        old_key["version"] = json!(2);
        store::write_manifest(&f.measure,
            &json!({"key":old_key,"ranges":[[null],[null]],"backend":"rust"})).unwrap();
        let old_folder = f.cache.dir.join("delta").join(store::digest(&old_key));
        std::fs::rename(f.measure.parent().unwrap(), &old_folder).unwrap();
        let old_measure = old_folder.join("measure").join("complete.json");
        let old_bytes = std::fs::read(&old_measure).unwrap();
        assert!(Table::cached(&f.cache, 0, 2, &f.constraints).is_none());
        assert_eq!(f.summary(None, Basis::Absolute).count, None);
        let source = GeometrySource::new(vec![single_edge(0.2375), single_edge(0.2575)]);
        let summary = Groups::prepare_summary(&source, 0, &f.constraints, Some(&f.cache),
            None, Basis::Absolute, 1, &AtomicUsize::new(0), &|_| {}).unwrap();
        assert_eq!(summary.count, Some(0));
        let table = Table::cached(&f.cache, 0, 2, &f.constraints).unwrap();
        assert_eq!(table.choices.as_slice(), &[GEOMETRY_LENGTH, GEOMETRY_LENGTH]);
        assert_eq!(table.values.as_slice(), &[23750, 25750]);
        assert_eq!(std::fs::read(old_measure).unwrap(), old_bytes);
    }

    struct SyntheticSource {
        count: u64,
        visits: AtomicUsize,
    }

    impl Source for SyntheticSource {
        fn check_count(&self) -> usize { 2 }
        fn count(&self, _: usize) -> u64 { self.count }
        fn precision(&self) -> f64 { 100000.0 }
        fn visit(&self, _: usize, start: u64, end: u64, stop: &AtomicUsize,
                 visit: &mut dyn FnMut(u64, char, &[[f64; 2]]) -> Result<()>) -> Result<()> {
            self.visits.fetch_add(1, Ordering::Relaxed);
            for ei in start..end {
                check_cancelled(stop)?;
                if ei % 4 == 3 {
                    visit(ei, 'p', &[[0.0, 0.0], [0.1, 0.0], [0.0, 0.1]])?;
                } else {
                    let width = [0.02, 0.03, 0.04][(ei % 4) as usize];
                    visit(ei, 'p', &[[0.0, 0.0], [width, 0.0], [width, 0.1], [0.0, 0.1]])?;
                }
            }
            Ok(())
        }
        fn visit_boxes(&self, _: usize, _: u64, _: u64, _: &AtomicUsize,
                       _: &mut dyn FnMut(u64, ErrorBox) -> Result<()>) -> Result<()> {
            panic!("CD summary must not visit marker boxes")
        }
    }

    #[test]
    fn cold_summary_persists_small_tables_without_group_members_and_warm_never_visits() {
        let f = Fixture::new(8, &[0], "all");
        std::fs::remove_dir_all(&f.cache.dir).unwrap();
        let source = SyntheticSource { count: 8, visits: AtomicUsize::new(0) };
        let stop = AtomicUsize::new(0);
        let summary = Groups::prepare_summary(&source, 0, &f.constraints, Some(&f.cache),
            None, Basis::Absolute, 4, &stop, &|_| {}).unwrap();
        assert_eq!(summary, CachedGroupSummary { count: Some(3), step: Some(223) });
        assert!(source.visits.load(Ordering::Relaxed) > 0);
        let visits = source.visits.load(Ordering::Relaxed);
        let folder = f.measure.parent().unwrap();
        assert!(f.measure.join("values.bin").is_file());
        assert!(!folder.join("groups").exists());
        let table = Table::cached(&f.cache, 0, 8, &f.constraints).unwrap();
        assert!(matches!(table.values, Column::Mapped(..)));
        let summaries = folder.join("summaries");
        assert_eq!(std::fs::read_dir(&summaries).unwrap()
            .filter(|entry| entry.as_ref().unwrap().file_type().unwrap().is_dir()).count(), 1);
        let dir = summaries.join(store::digest(&Groups::key(223, Basis::Absolute, "all")));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let warm = Groups::prepare_summary(&source, 0, &f.constraints, Some(&f.cache),
            None, Basis::Absolute, 4, &stop, &|_| panic!("warm summary has no work")).unwrap();
        assert_eq!(warm, summary);
        assert_eq!(source.visits.load(Ordering::Relaxed), visits);
        assert_eq!(Groups::prepare_summary(&source, 0, &f.constraints, Some(&f.cache),
            Some(5000), Basis::Absolute, 4, &stop, &|_| {}).unwrap().count, Some(1));
        assert_eq!(Groups::prepare_summary(&source, 0, &f.constraints, Some(&f.cache),
            None, Basis::Percent, 4, &stop, &|_| {}).unwrap().count, Some(3));
        assert_eq!(source.visits.load(Ordering::Relaxed), visits);
        assert!(!folder.join("groups").exists());
    }

    #[test]
    fn v1_unknown_measurements_groups_and_summaries_are_not_reused_for_zero_gap() {
        struct ContactSource(AtomicUsize);
        impl Source for ContactSource {
            fn check_count(&self) -> usize {
                1
            }
            fn count(&self, _: usize) -> u64 {
                1
            }
            fn precision(&self) -> f64 {
                100000.0
            }
            fn visit(
                &self,
                _: usize,
                start: u64,
                end: u64,
                stop: &AtomicUsize,
                visit: &mut dyn FnMut(u64, char, &[[f64; 2]]) -> Result<()>,
            ) -> Result<()> {
                check_cancelled(stop)?;
                self.0.fetch_add(1, Ordering::Relaxed);
                for ei in start..end {
                    visit(ei, 'e', &[[0.0, 0.0], [1.0, 0.0], [1.0, 0.0], [2.0, 0.0]])?;
                }
                Ok(())
            }
            fn visit_boxes(
                &self,
                _: usize,
                _: u64,
                _: u64,
                _: &AtomicUsize,
                _: &mut dyn FnMut(u64, ErrorBox) -> Result<()>,
            ) -> Result<()> {
                panic!("CD preparation must not visit marker boxes")
            }
        }

        let f = Fixture::new(1, &[UNKNOWN], "all");
        let current_folder = f.measure.parent().unwrap();
        let mut old_table_key = Table::key(0, 1, &f.constraints, &f.cache);
        old_table_key["version"] = json!(1);
        let mut old_group_key = Groups::key(1, Basis::Absolute, "all");
        old_group_key["version"] = json!(1);
        let old_folder = f
            .cache
            .dir
            .join("delta")
            .join(store::digest(&old_table_key));
        store::write_column(&f.measure.join("choices.bin"), &[UNKNOWN]).unwrap();
        store::write_manifest(
            &f.measure,
            &json!({
                "key":old_table_key,"ranges":[[null],[null]],"backend":"rust",
            }),
        )
        .unwrap();
        store::write_manifest(
            &f.groups,
            &json!({
                "key":old_group_key,"groups":1,"total":1,"narrow":true,
            }),
        )
        .unwrap();
        let old_group_dir = current_folder
            .join("groups")
            .join(store::digest(&old_group_key));
        std::fs::rename(&f.groups, &old_group_dir).unwrap();
        let old_summary = current_folder
            .join("summaries")
            .join(store::digest(&old_group_key));
        std::fs::create_dir_all(&old_summary).unwrap();
        store::write_manifest(
            &old_summary,
            &json!({
                "key":old_group_key,"table":old_table_key,"cd_groups":0,"total":1,
            }),
        )
        .unwrap();
        std::fs::rename(current_folder, &old_folder).unwrap();
        let old_measure = old_folder.join("measure").join("complete.json");
        let old_bytes = std::fs::read(&old_measure).unwrap();

        assert!(Table::cached(&f.cache, 0, 1, &f.constraints).is_none());
        assert_eq!(f.summary(None, Basis::Absolute).count, None);
        let source = ContactSource(AtomicUsize::new(0));
        let summary = Groups::prepare_summary(
            &source,
            0,
            &f.constraints,
            Some(&f.cache),
            None,
            Basis::Absolute,
            1,
            &AtomicUsize::new(0),
            &|_| {},
        )
        .unwrap();
        assert_eq!(summary.count, Some(1));
        assert!(source.0.load(Ordering::Relaxed) > 0);
        let table = Table::cached(&f.cache, 0, 1, &f.constraints).unwrap();
        assert_eq!((table.value(0), table.choice(0)), (0, 0));
        assert_eq!(table.difference(0, Basis::Absolute), Some(5000));
        assert_eq!(table.difference(0, Basis::Percent), Some(10_000_000));
        assert_eq!(f.summary(None, Basis::Absolute).count, Some(1));
        assert_eq!(std::fs::read(old_measure).unwrap(), old_bytes);
    }

    #[test]
    fn cancellation_keeps_completed_rule_and_does_not_publish_incomplete_summary() {
        let f = Fixture::new(8, &[0], "all");
        std::fs::remove_dir_all(&f.cache.dir).unwrap();
        let source = SyntheticSource { count: 8, visits: AtomicUsize::new(0) };
        let stop = AtomicUsize::new(0);
        Groups::prepare_summary(&source, 0, &f.constraints, Some(&f.cache),
            None, Basis::Absolute, 1, &stop, &|_| {}).unwrap();
        let result = Groups::prepare_summary(&source, 1, &f.constraints, Some(&f.cache),
            None, Basis::Absolute, 1, &stop, &|text| {
                if text.starts_with("Counting CD groups 0") {
                    stop.store(1, Ordering::Relaxed);
                }
            });
        assert!(matches!(result, Err(Error { kind: ErrorKind::Cancelled, .. })));
        assert_eq!(Groups::cached_summary(&f.cache, 0, 8, &f.constraints, None, Basis::Absolute).count, Some(3));
        assert_eq!(Groups::cached_summary(&f.cache, 1, 8, &f.constraints, None, Basis::Absolute).count, None);
        assert!(Table::cached(&f.cache, 1, 8, &f.constraints).is_some());
    }

    #[test]
    fn unsupported_empty_and_unwritable_preparation_never_decode_or_allocate_per_error_arrays() {
        let f = Fixture::new(8, &[0], "all");
        let source = SyntheticSource { count: 200_000_000, visits: AtomicUsize::new(0) };
        let stop = AtomicUsize::new(0);
        let unsupported = vec![json!({"metric":"density","op":"<","value":1})];
        assert_eq!(Groups::prepare_summary(&source, 0, &unsupported, None,
            None, Basis::Absolute, 4, &stop, &|_| {}).unwrap().count, Some(0));
        let empty = SyntheticSource { count: 0, visits: AtomicUsize::new(0) };
        assert_eq!(Groups::prepare_summary(&empty, 0, &f.constraints, None,
            None, Basis::Absolute, 4, &stop, &|_| {}).unwrap().count, Some(0));
        let path = f.root.join("not-a-directory");
        File::create(&path).unwrap();
        let blocked = Cache { dir: path, ..f.cache.clone() };
        assert!(Groups::prepare_summary(&source, 0, &f.constraints, Some(&blocked),
            None, Basis::Absolute, 4, &stop, &|_| {}).is_err());
        assert!(Groups::prepare_summary(&source, 0, &f.constraints, None,
            None, Basis::Absolute, 4, &stop, &|_| {}).is_err());
        assert_eq!(source.visits.load(Ordering::Relaxed), 0);
        assert_eq!(empty.visits.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn summary_occupancy_counts_holes_and_bounds_sparse_manual_bins() {
        let mut dense = SummaryBins::new(&[Some((100, 190))], 10, 2);
        for delta in [100, 100, 130, 190] { dense.insert(0, delta).unwrap(); }
        assert_eq!(dense.count, 3);
        let mut sparse = SummaryBins::new(&[Some((0, (DENSE_BINS * 10) as i64))], 1, 2);
        for delta in [0, 0, 1] { sparse.insert(0, delta).unwrap(); }
        assert_eq!(sparse.count, 2);
        assert!(sparse.insert(0, 2).is_err());
        assert_eq!(sparse.sparse.len(), 2);
    }
}
