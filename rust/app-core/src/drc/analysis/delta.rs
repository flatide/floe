//! CD delta groups of a DRC rule (floe_oracle/drc_delta.py `DeltaIndex`,
//! `DeltaGroups`, feature/jobdeck-drc's drc_delta_cache): every error's
//! rule-aware CD measured once into a table - five-decimal ticks, the
//! constraint it answers, whether it is an estimate - then grouped by the
//! difference to that constraint's bound, `|CD - bound|` (absolute) or
//! `|CD - bound| / |bound| * 100` (ratio), in zero-anchored half-open bins
//! of a step: by default the whole rule's widest constraint range over ten.
//! Grouping, its step, basis or cluster never measure again; group pages,
//! ranks and waive-filtered counts never list a group's members.
use super::measure::{measurement_ticks, Selector};
use super::scope::Membership;
use super::source::{parallel, Source, Statuses};
use super::store::{self, BuildLock, Column, Sink, Staging};
use super::ticks::{format_ticks, json_ticks, percent_ticks, SCALE};
use crate::{check_cancelled, Error, ErrorKind, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// No measurable CD (or no usable ratio).
pub const UNKNOWN: i32 = -1;
/// Cache tables of rules this large (smaller ones measure in a moment).
pub const CACHE_THRESHOLD: u64 = 100_000;
const VERSION: u32 = 1;
const TASK: u64 = 1 << 16;
const PAGE_CHUNK: u64 = 4096;
const DENSE_BINS: u64 = 1 << 24;

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

impl Table {
    fn key(check: usize, count: u64, constraints: &[Value], cache: &Cache) -> Value {
        json!({"version": VERSION, "engine": "rust", "pack": cache.identity,
               "rule": check, "count": count, "constraints": constraints})
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
        let bound_ticks: Vec<Option<i64>> = constraints
            .iter()
            .map(|c| c.get("value").and_then(json_ticks))
            .collect();
        let selector = Selector::new(constraints);
        let measurable = selector.can_measure();
        let base = Table {
            check,
            count,
            constraints: constraints.to_vec(),
            bound_ticks,
            values: Column::Owned(Vec::new()),
            choices: Column::Owned(Vec::new()),
            estimated: Column::Owned(Vec::new()),
            ranges: [vec![None; constraints.len()], vec![None; constraints.len()]],
            folder: None,
            measurable,
        };
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
        if ranges.iter().any(|r| r.len() != base.constraints.len()) {
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
                    let Some(pick) = selector.pick(kind, pts) else {
                        return Ok(());
                    };
                    if base.bound_ticks[pick.constraint].is_none() {
                        return Ok(());
                    }
                    let Ok((measured, _, _)) = measurement_ticks(&pick) else {
                        return Ok(());
                    };
                    part.values[j] = measured;
                    part.choices[j] = pick.constraint as i32;
                    part.estimated[j] = u8::from(pick.estimated);
                    for basis in [Basis::Absolute, Basis::Percent] {
                        if let Some(d) = difference(measured, pick.constraint as i32, &base.bound_ticks, basis) {
                            merge_range(&mut part.ranges[basis.index()][pick.constraint], d);
                        }
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
        let ranges: Vec<(i128, i128)> = self.ranges[basis.index()]
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
    /// the constraint index, UNKNOWN for the unmeasurable group (last)
    pub constraint: i32,
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

impl Groups {
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
        let key = json!({"version": VERSION, "step": step, "mode": basis.name(), "members": members_key});
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
        // (sort choice, bin) of a member: unknown sorts after every constraint
        let key_of = |ei: u64| -> (i64, i64) {
            match table.difference(ei, basis) {
                Some(d) => (i64::from(table.choice(ei)), d / step),
                None => (k, 0),
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
                if c == k {
                    unknown += 1;
                    continue;
                }
                match &mut dense[c as usize] {
                    Some((a, counts)) if !counts.is_empty() => counts[(bin - *a) as usize] += 1,
                    _ => *sparse.entry((c, bin)).or_insert(0) += 1,
                }
            }
            start += len as u64;
        }
        // the directory: constraints in order, bins ascending, unknown last
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
        if unknown > 0 {
            rows.push((k, 0, unknown));
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
                .map(|&(c, _, _)| if c == k { UNKNOWN } else { c as i32 })
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
            &json!({"key": key, "groups": b.constraint.len(), "total": b.total, "narrow": b.narrow}),
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
        if constraint < 0 {
            return (bin == 0 && c.last() == Some(&UNKNOWN)).then(|| c.len() - 1);
        }
        let known = c.len() - usize::from(c.last() == Some(&UNKNOWN));
        let lo = c[..known].partition_point(|&v| v < constraint);
        let hi = c[..known].partition_point(|&v| v <= constraint);
        let r = lo + b[lo..hi].partition_point(|&v| v < bin);
        (r < hi && b[r] == bin).then_some(r)
    }
    /// Its directory name (floe_oracle drc_delta `DeltaGroup.name`).
    pub fn name(&self, r: usize) -> String {
        let row = self.row(r);
        let name = if row.constraint < 0 {
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
