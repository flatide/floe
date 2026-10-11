//! Review from a previous DRC run, carried to the errors of this run with
//! exactly the same geometry (floe_oracle drc_migrate.py): the previous
//! result and its review files are only read; `prepare` matches them
//! (drc::matcher) into sparse snapshots - what an exact match carries, the
//! ambiguous targets, and per target its eligibility and unknown fields -
//! and `apply` writes them in bounded batches. One's own visited errors
//! with a different decision are protected; disputed fields keep their
//! values; a visit is carried only for a proven unchanged error.
use super::bits::{Bits, CHUNK};
use super::flags::{exists, fallback_path, review_path, NOT_REQUIRED, VISITED};
use super::snapshot::{open_review, own_bits, Members, Snapshot, Writer};
use super::{review, Identity, Own, Progress, TempDir};
use crate::drc::analysis::store::identity as pack_identity;
use crate::drc::desktop::{STATUS_NONE, STATUS_WAIVED};
use crate::drc::matcher;
use crate::drc::Pack;
use crate::{check_cancelled, Error, ErrorKind, Result};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

/// The previous run's state of one error as it would carry: None for an
/// unknown or disputed field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Carried {
    pub visited: Option<bool>,
    pub waived: Option<bool>,
    pub not_required: Option<bool>,
}

/// A prepared migration (see the module docs).
pub struct Plan {
    pub mapped: Snapshot,
    pub ambiguous: Snapshot,
    pub eligibility: Snapshot,
    pub conflicts: Option<Snapshot>,
    pub protected: Option<Snapshot>,
    pub summary: Map<String, Value>,
    pub reviewer: String,
    target_identity: String,
    serial: u64,
    // dropped last: the snapshots' folder
    owner: Arc<TempDir>,
}

/// Rule counts of a plan.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct RuleCounts {
    pub total: u64,
    pub matched: u64,
    pub ambiguous: u64,
    pub partial: u64,
    pub conflict: u64,
    pub protected: u64,
    pub unmatched: u64,
}

/// What an application changed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Applied {
    pub changed_visited: u64,
    pub changed_waived: u64,
    pub changed_not_required: u64,
    pub conflicts_skipped: u64,
    pub applied: u64,
    pub processed: u64,
}

fn different(s: &Snapshot, ow: Bits, on: Bits, w: Bits, n: Bits, uw: Bits, un: Bits) -> Bits {
    (if s.waive_known { (ow ^ w) & !uw } else { Bits::ZERO })
        | (if s.not_required_known { (on ^ n) & !un } else { Bits::ZERO })
}

/// The pack of a previous result (a .db's hidden pack, or a pack itself)
/// and the .db when one was given; no legacy rename.
fn resolve_pack(path: &Path) -> Result<(PathBuf, Option<PathBuf>)> {
    use std::io::Read;
    let path = crate::cache::absolute(path)?;
    let mut magic = [0u8; 8];
    let n = std::fs::File::open(&path)?.read(&mut magic)?;
    if n == 8 && &magic == crate::drc::MAGIC {
        return Ok((path, None));
    }
    for candidate in crate::cache::pack_paths(&path)? {
        if exists(&candidate)? {
            return Ok((candidate, Some(path)));
        }
    }
    Err(Error::input(
        "previous DRC result needs an existing valid .tray pack; index it first",
    ))
}

fn existing(primary: &Path, fallback: Option<&Path>) -> Result<Option<PathBuf>> {
    if exists(primary)? {
        return Ok(Some(primary.to_owned()));
    }
    match fallback {
        Some(f) if exists(f)? => Ok(Some(f.to_owned())),
        _ => Ok(None),
    }
}

/// The previous review: the chosen file, else the reviewer's own working
/// files of that run, else the waives embedded in its pack.
#[allow(clippy::too_many_arguments)]
fn prior_source(
    old: &Pack,
    old_id: &Identity,
    state: Option<&Path>,
    reviewer: &str,
    folder: &Path,
    cancelled: &AtomicUsize,
    progress: &mut dyn FnMut(u64, u64, &str),
) -> Result<Snapshot> {
    let mut read = |d: u64, t: u64, _: &str| progress(d, t, "Read previous review");
    if let Some(s) = state {
        return open_review(old_id, s, None, None, cancelled, &mut read);
    }
    let mut raw = existing(&review_path(&old.path, reviewer)?, None)?;
    if raw.is_none() && std::env::var("FLOE_DRC_REVIEW_ROOT").map_or(true, |r| r.is_empty()) {
        raw = existing(&fallback_path(&old.path, reviewer)?, None)?;
    }
    let [beside, temp] = crate::drc::waive_paths(&old.path, reviewer)?;
    let waive = existing(&beside, Some(&temp))?;
    if let Some(first) = raw.clone().or(waive.clone()) {
        return open_review(old_id, &first, waive.as_deref(), Some(reviewer), cancelled, &mut read);
    }
    // a retired pack may hold the only saved waives; visits and
    // exclusions are unknown without a personal file
    let any = (0..old.checks.len())
        .map(|c| old.embedded_waived(c))
        .collect::<Result<Vec<_>>>()?;
    if !any.iter().any(|&w| w > 0) {
        return Err(Error::input(format!(
            "no saved review state for reviewer {reviewer} in previous DRC result; choose a review snapshot"
        )));
    }
    let path = folder.join("embedded-review.sqlite3");
    let extra = vec![
        ("visited_known".into(), json!(false)),
        ("not_required_known".into(), json!(false)),
        ("waive_known".into(), json!(true)),
        ("source_path".into(), json!(old.path)),
    ];
    let mut writer = Writer::new(old_id, &path, reviewer, extra)?;
    for (ci, c) in old.checks.iter().enumerate() {
        if any[ci] == 0 {
            continue;
        }
        let mut start = 0;
        while start < c.count {
            check_cancelled(cancelled)?;
            let values = old.embedded_statuses(ci, start, 65536)?;
            for (j, piece) in values.chunks(CHUNK as usize).enumerate() {
                let w = super::waived_bits(piece);
                if !w.is_zero() {
                    writer.add(ci, start / CHUNK + j as u64, Bits::ZERO, w, Bits::ZERO, Bits::ZERO, None)?;
                }
            }
            read(c.start + start + values.len() as u64, old_id.total(), "");
            start += values.len() as u64;
        }
    }
    writer.finish()?;
    let mut snap = Snapshot::open(old_id, &path, None)?;
    snap.original_path = old.path.clone();
    Ok(snap)
}

/// A consistent copy of a source read through its validated connection
/// (a replaceable path is never opened again).
fn freeze(old_id: &Identity, source: Snapshot, path: &Path) -> Result<Snapshot> {
    let original = source.original_path.clone();
    {
        let conn = super::flags::open_read_only(&source.path, std::time::Duration::from_secs(10))?;
        conn.execute("VACUUM INTO ?", [path.to_string_lossy()])
            .map_err(super::flags::sql)?;
    }
    drop(source);
    let mut frozen = Snapshot::open(old_id, path, None)?;
    frozen.original_path = original;
    Ok(frozen)
}

fn write_states(old: &Pack, source: &Snapshot, cancelled: &AtomicUsize) -> Result<Vec<u8>> {
    let mut states = vec![0u8; old.total as usize];
    let mut stream = source.chunks(None)?;
    while let Some((ci, k, [v, w, n, _, _])) = stream.next()? {
        check_cancelled(cancelled)?;
        let size = old.checks[ci].count;
        let count = size.saturating_sub(k * CHUNK).min(CHUNK);
        if count == 0 || v.beyond(count) || w.beyond(count) || n.beyond(count) {
            return Err(Error::input("previous review bitmap is outside its rule"));
        }
        let start = (old.checks[ci].start + k * CHUNK) as usize;
        for i in 0..count {
            states[start + i as usize] = u8::from(v.get(i)) | u8::from(w.get(i)) << 1 | u8::from(n.get(i)) << 2;
        }
    }
    Ok(states)
}

/// Build the mapped, ambiguous and eligibility snapshots from the match
/// records (sorted, unique, validated).
fn mapped_snapshots(
    target: &Identity,
    source: &Snapshot,
    rows: &[matcher::Row],
    folder: &Path,
    cancelled: &AtomicUsize,
) -> Result<(Snapshot, Snapshot, Snapshot)> {
    let meta = vec![
        ("visited_known".into(), json!(source.visited_known)),
        ("waive_known".into(), json!(source.waive_known)),
        ("not_required_known".into(), json!(source.not_required_known)),
        ("migration".into(), json!(true)),
        ("prior_fingerprint".into(), json!(source.fingerprint)),
        ("prior_snapshot_id".into(), json!(source.snapshot_id)),
    ];
    let paths = [
        folder.join("mapped.sqlite3"),
        folder.join("ambiguous.sqlite3"),
        folder.join("eligibility.sqlite3"),
    ];
    let mut matched = Writer::new(target, &paths[0], &source.reviewer, meta)?;
    let mut ambiguous = Writer::new(target, &paths[1], "ambiguous-prior", vec![("aggregate".into(), json!(true))])?;
    let mut eligible = Writer::new(
        target,
        &paths[2],
        "migration-eligibility",
        vec![("aggregate".into(), json!(true))],
    )?;
    // planes: V/W/NR carried, ambiguity, exact, unknown W, unknown NR, partial
    let mut current: Option<(usize, u64)> = None;
    let mut planes = [Bits::ZERO; 8];
    let mut previous: Option<(u32, u64)> = None;
    let flush = |current: Option<(usize, u64)>,
                 p: &[Bits; 8],
                 m: &mut Writer,
                 a: &mut Writer,
                 e: &mut Writer|
     -> Result<()> {
        if let Some((ci, k)) = current {
            let [v, w, n, amb, exact, uw, un, partial] = *p;
            m.add(ci, k, v, w, n, Bits::ZERO, None)?;
            a.add(ci, k, amb, Bits::ZERO, Bits::ZERO, amb, Some(amb))?;
            // internal membership and masks, never actual review flags
            e.add(ci, k, exact, uw, un, partial, Some(exact))?;
        }
        Ok(())
    };
    for (i, &(ci, ei, flags, disposition, unknown)) in rows.iter().enumerate() {
        if i % 65536 == 0 {
            check_cancelled(cancelled)?;
        }
        let ci_u = ci as usize;
        if ci_u >= target.sizes.len()
            || flags > 7
            || disposition > 1
            || unknown & !6 != 0
            || unknown & flags != 0
            || (disposition == 1 && (flags | unknown) != 0)
        {
            return Err(Error::input("invalid geometry match record"));
        }
        if ei >= target.sizes[ci_u] {
            return Err(Error::input("geometry match error is outside target rule"));
        }
        if previous.is_some_and(|p| (ci, ei) <= p) {
            return Err(Error::input("geometry match records are not unique and sorted"));
        }
        previous = Some((ci, ei));
        let key = (ci_u, ei / CHUNK);
        if current != Some(key) {
            flush(current, &planes, &mut matched, &mut ambiguous, &mut eligible)?;
            current = Some(key);
            planes = [Bits::ZERO; 8];
        }
        let local = ei % CHUNK;
        let agreed = disposition == 0;
        if agreed && (flags | unknown) == 0 {
            return Err(Error::input("empty previous state cannot be transferred"));
        }
        let take = [
            agreed && flags & 1 != 0,
            agreed && flags & 2 != 0,
            agreed && flags & 4 != 0,
            !agreed || unknown != 0,
            agreed,
            agreed && unknown & 2 != 0,
            agreed && unknown & 4 != 0,
            agreed && unknown != 0,
        ];
        for (p, t) in planes.iter_mut().zip(take) {
            if t {
                p.set(local);
            }
        }
    }
    flush(current, &planes, &mut matched, &mut ambiguous, &mut eligible)?;
    check_cancelled(cancelled)?;
    matched.finish()?;
    ambiguous.finish()?;
    eligible.finish()?;
    Ok((
        Snapshot::open(target, &paths[0], None)?,
        Snapshot::open(target, &paths[1], None)?,
        Snapshot::open(target, &paths[2], None)?,
    ))
}

impl Plan {
    fn validate(&self, own: &Own) -> Result<()> {
        if self.mapped.fingerprint != own.id.fingerprint || self.mapped.sizes != own.id.sizes {
            return Err(Error::new(
                ErrorKind::Cache,
                "review source belongs to another DRC result; source preserved",
            ));
        }
        if pack_identity(&own.id.pack)? != self.target_identity {
            return Err(Error::new(ErrorKind::Busy, "target DRC pack changed; prepare migration again"));
        }
        Ok(())
    }

    /// One's own visited errors whose decision differs from what would be
    /// carried, merged with the ambiguous targets: the conflicts.
    pub fn refresh_conflicts(
        &mut self,
        own: &Own,
        cancelled: &AtomicUsize,
        progress: &mut dyn FnMut(u64, u64, &str),
    ) -> Result<()> {
        self.validate(own)?;
        self.serial += 1;
        let protected_path = self.owner.path().join(format!("protected-{}.sqlite3", self.serial));
        let total = self.eligibility.counts(None)?.visited;
        let mut report = Progress::new(progress, total);
        let mut writer = Writer::new(
            own.id,
            &protected_path,
            "protected-current-review",
            vec![("aggregate".into(), json!(true))],
        )?;
        let mut processed = 0;
        let mut rows = self.eligibility.chunks(None)?;
        while let Some((ci, k, [eligible, uw, un, _, _])) = rows.next()? {
            check_cancelled(cancelled)?;
            let [_v, w, n, _, _] = self.mapped.chunk(ci, k)?;
            let conflict = {
                let mut g = own.lock();
                let r = review(&mut g)?;
                let _guard = own.flags.write_guard();
                let (ov, ow, on, _) = own_bits(own, r, ci, k)?;
                eligible & ov & different(&self.mapped, ow, on, w, n, uw, un)
            };
            if !conflict.is_zero() {
                writer.add(ci, k, conflict, Bits::ZERO, Bits::ZERO, conflict, Some(conflict))?;
            }
            processed += eligible.count();
            report.report_text(processed, total, "Check current review conflicts", false);
        }
        check_cancelled(cancelled)?;
        drop(rows);
        writer.finish()?;
        let protected = Snapshot::open(own.id, &protected_path, None)?;
        // conflicts = ambiguous ∪ protected, merged by key
        let path = self.owner.path().join(format!("conflicts-{}.sqlite3", self.serial));
        let mut merged = Writer::new(own.id, &path, "migration-conflicts", vec![("aggregate".into(), json!(true))])?;
        let (mut left, mut right) = (self.ambiguous.chunks(None)?, protected.chunks(None)?);
        let (mut a, mut b) = (left.next()?, right.next()?);
        while a.is_some() || b.is_some() {
            check_cancelled(cancelled)?;
            let key = match (&a, &b) {
                (Some(x), Some(y)) => (x.0, x.1).min((y.0, y.1)),
                (Some(x), None) => (x.0, x.1),
                (None, Some(y)) => (y.0, y.1),
                _ => unreachable!(),
            };
            let mut bits = Bits::ZERO;
            if a.as_ref().is_some_and(|x| (x.0, x.1) == key) {
                bits |= a.as_ref().unwrap().2[3];
                a = left.next()?;
            }
            if b.as_ref().is_some_and(|y| (y.0, y.1) == key) {
                bits |= b.as_ref().unwrap().2[3];
                b = right.next()?;
            }
            merged.add(key.0, key.1, bits, Bits::ZERO, Bits::ZERO, bits, Some(bits))?;
        }
        drop((left, right));
        merged.finish()?;
        let conflicts = Snapshot::open(own.id, &path, None)?;
        self.summary.insert("conflict".into(), json!(conflicts.counts(None)?.conflicts));
        self.summary.insert("protected".into(), json!(protected.counts(None)?.conflicts));
        for old in [self.conflicts.replace(conflicts), self.protected.replace(protected)].into_iter().flatten() {
            let p = old.path.clone();
            drop(old);
            let _ = std::fs::remove_file(p);
        }
        Ok(())
    }

    pub fn rule_counts(&self, check: Option<usize>) -> Result<RuleCounts> {
        let e = self.eligibility.counts(check)?;
        let ambiguous = self.ambiguous.counts(check)?.conflicts;
        let conflict = match &self.conflicts {
            Some(c) => c.counts(check)?.conflicts,
            None => ambiguous,
        };
        let protected = match &self.protected {
            Some(p) => p.counts(check)?.conflicts,
            None => 0,
        };
        Ok(RuleCounts {
            total: e.total,
            matched: e.visited,
            ambiguous,
            partial: e.conflicts,
            conflict,
            protected,
            unmatched: e.total + e.conflicts - e.visited - ambiguous,
        })
    }

    /// matched | partial | ambiguous | conflict | protected
    pub fn members(&self, check: usize, kind: &str) -> Result<Members<'_>> {
        match kind {
            "matched" => self.eligibility.members(check, "visited"),
            "partial" => self.eligibility.members(check, "conflict"),
            "ambiguous" => self.ambiguous.members(check, "conflict"),
            "conflict" => self.conflicts.as_ref().unwrap_or(&self.ambiguous).members(check, "conflict"),
            "protected" => match &self.protected {
                Some(p) => p.members(check, "conflict"),
                None => Err(Error::input("current review conflicts have not been prepared")),
            },
            _ => Err(Error::input("unsupported migration membership")),
        }
    }

    /// What would carry to error `error` of rule `check` (None: an
    /// ambiguous geometry match).
    pub fn get(&self, check: usize, error: u64) -> Result<Option<Carried>> {
        let size = self.eligibility.rule(check)?;
        if error >= size {
            return Err(Error::input("DRC error index out of range"));
        }
        let [eligible, uw, un, _, _] = self.eligibility.chunk(check, error / CHUNK)?;
        let i = error % CHUNK;
        if !eligible.get(i) && self.ambiguous.members(check, "conflict")?.contains(error)? {
            return Ok(None);
        }
        let v = self.mapped.get(check, error)?;
        Ok(Some(Carried {
            visited: self.mapped.visited_known.then_some(v.visited),
            waived: (self.mapped.waive_known && !uw.get(i)).then_some(v.waived),
            not_required: (self.mapped.not_required_known && !un.get(i)).then_some(v.not_required),
        }))
    }
}

/// Match the previous result at `old_path` (a .db or its pack) with this
/// review's pack and read its review (`state`, or the reviewer's own files
/// of that run) without applying anything.
pub fn prepare(
    own: &Own,
    old_path: &Path,
    state: Option<&Path>,
    work_dir: Option<&Path>,
    options: matcher::Options,
    cancelled: &AtomicUsize,
    progress: &mut (dyn FnMut(u64, u64, &str) + Send),
) -> Result<Plan> {
    check_cancelled(cancelled)?;
    let target_identity = pack_identity(&own.id.pack)?;
    let (old_pack, source_db) = resolve_pack(old_path)?;
    if super::flags::realpath(&old_pack) == super::flags::realpath(&own.id.pack) {
        return Err(Error::input("choose a previous DRC result, not the currently open pack"));
    }
    let owner = Arc::new(TempDir::new("floe-review-migration-")?);
    let lock = crate::drc::local::pack_reader(&old_pack)?;
    let mut old = Pack::open(&old_pack, cancelled)?;
    old.hold(lock);
    if let Some(db) = &source_db {
        if !old.source_matches(db)? {
            return Err(Error::new(
                ErrorKind::Cache,
                format!("{}: stale pack (source size/mtime changed)", old_pack.display()),
            ));
        }
    }
    let old_identity = pack_identity(&old_pack)?;
    let old_id = Identity::of_pack(&old, &own.id.reviewer)?;
    if old_id.fingerprint == own.id.fingerprint {
        return Err(Error::input("previous and current packs describe the same DRC result"));
    }
    let source = prior_source(&old, &old_id, state, &own.id.reviewer, owner.path(), cancelled, progress)?;
    let source = freeze(&old_id, source, &owner.path().join("prior.sqlite3"))?;
    let mut old_names: HashMap<&str, usize> = HashMap::new();
    for c in &old.checks {
        *old_names.entry(&c.name).or_default() += 1;
    }
    let mut new_names: HashMap<String, (usize, usize)> = HashMap::new();
    let target_names = {
        let mut g = own.lock();
        review(&mut g)?.pack.checks.iter().map(|c| c.name.clone()).collect::<Vec<_>>()
    };
    for (ci, name) in target_names.iter().enumerate() {
        let e = new_names.entry(name.clone()).or_default();
        e.0 += 1;
        e.1 = ci;
    }
    let mut pairs = Vec::new();
    for (ci, c) in old.checks.iter().enumerate() {
        if old_names[c.name.as_str()] == 1
            && new_names.get(&c.name).is_some_and(|e| e.0 == 1)
            && source.counts(Some(ci))?.work > 0
        {
            pairs.push((ci, new_names[&c.name].1));
        }
    }
    let eligible: u64 = pairs
        .iter()
        .map(|&(ci, _)| source.counts(Some(ci)).map(|c| c.work))
        .sum::<Result<u64>>()?;
    let mut stats = matcher::Stats::default();
    let rows = if pairs.is_empty() {
        Vec::new()
    } else {
        let states = write_states(&old, &source, cancelled)?;
        let records = owner.path().join("matches.bin");
        let target_pack = {
            let lock = crate::drc::local::pack_reader(&own.id.pack)?;
            let mut p = Pack::open(&own.id.pack, cancelled)?;
            p.hold(lock);
            p
        };
        let mut phase = |p: &str, d: u64, t: u64| progress(d, t, p);
        stats = matcher::match_packs(
            &old,
            &target_pack,
            &pairs,
            &states,
            &records,
            &work_dir.map_or_else(|| owner.path().join("sort"), Path::to_owned),
            options,
            cancelled,
            &mut phase,
        )?;
        let rows = matcher::rows(&std::fs::read(&records)?)?;
        let _ = std::fs::remove_file(&records);
        rows
    };
    check_cancelled(cancelled)?;
    if pack_identity(&old_pack)? != old_identity || pack_identity(&own.id.pack)? != target_identity {
        return Err(Error::new(
            ErrorKind::Busy,
            "DRC pack changed while matching; prepare migration again",
        ));
    }
    let (mapped, ambiguous, eligibility) = mapped_snapshots(own.id, &source, &rows, owner.path(), cancelled)?;
    drop(rows);
    let total = own.id.total();
    let matched = eligibility.counts(None)?.visited;
    let partial = eligibility.counts(None)?.conflicts;
    let ambiguous_count = ambiguous.counts(None)?.conflicts;
    if !pairs.is_empty()
        && (stats.matched_target != matched
            || stats.ambiguous_target != ambiguous_count
            || stats.partial_target != partial)
    {
        return Err(Error::new(ErrorKind::Cache, "geometry match counts do not match their records"));
    }
    let source_work = source.counts(None)?.work;
    let mut summary: Map<String, Value> = Map::new();
    if !pairs.is_empty() {
        if let Value::Object(native) = serde_json::from_str(stats.json().trim()).unwrap_or(Value::Null) {
            summary.extend(native);
        }
    }
    let fields: BTreeMap<&str, Value> = [
        ("total", json!(total)),
        ("matched", json!(matched)),
        ("ambiguous", json!(ambiguous_count)),
        ("partial", json!(partial)),
        ("unmatched", json!(total + partial - matched - ambiguous_count)),
        ("conflict", json!(ambiguous_count)),
        ("source_work", json!(source_work)),
        ("eligible_source_work", json!(eligible)),
        ("no_rule_match_source_work", json!(source_work - eligible)),
        ("source_path", json!(crate::cache::absolute(old_path)?)),
        ("old_pack", json!(old_pack)),
        ("state_path", json!(source.original_path)),
        ("reviewer", json!(source.reviewer)),
        ("prior_fingerprint", json!(source.fingerprint)),
        ("prior_snapshot_id", json!(source.snapshot_id)),
        ("rules", json!(pairs.len())),
    ]
    .into_iter()
    .collect();
    for (k, v) in fields {
        summary.insert(k.to_owned(), v);
    }
    drop(source);
    drop(old);
    let mut plan = Plan {
        mapped,
        ambiguous,
        eligibility,
        conflicts: None,
        protected: None,
        summary,
        reviewer: own.id.reviewer.clone(),
        target_identity,
        serial: 0,
        owner,
    };
    plan.refresh_conflicts(own, cancelled, progress)?;
    check_cancelled(cancelled)?;
    progress(total, total, "Previous review matches ready");
    Ok(plan)
}

/// Apply a plan's exact matches: waived and not-required first, then
/// visits, a batch of at most 16 chunks of a rule a time; one's own
/// visited errors with a different decision are kept. `changed(rule)` for
/// each rule whose statuses moved.
pub fn apply(
    own: &Own,
    plan: &mut Plan,
    cancelled: &AtomicUsize,
    progress: &mut dyn FnMut(u64, u64, &str),
    changed: &mut dyn FnMut(usize),
) -> Result<Applied> {
    check_cancelled(cancelled)?;
    plan.validate(own)?;
    let total = plan.eligibility.counts(None)?.visited;
    let mut result = Applied::default();
    {
        let mut report = Progress::new(progress, total);
        let mut rows = plan.eligibility.chunks(None)?;
        let mut batch: Vec<(u64, Bits, Bits, Bits)> = Vec::new();
        let mut batch_ci = None;
        let source = &plan.mapped;
        let mut run = |ci: usize, batch: &[(u64, Bits, Bits, Bits)], result: &mut Applied| -> Result<()> {
            check_cancelled(cancelled)?;
            plan.validate(own)?;
            let mut g = own.lock();
            let r = review(&mut g)?;
            let mut guard = own.flags.write_guard();
            let (mut set_nr, mut clear_nr, mut visits) = (Vec::new(), Vec::new(), Vec::new());
            for &(k, work, uw, un) in batch {
                let [v, w, n, _, _] = source.chunk(ci, k)?;
                let (ov, ow, on, before) = own_bits(own, r, ci, k)?;
                let conflicts = ov & work & different(source, ow, on, w, n, uw, un);
                let eligible = work & !conflicts;
                result.conflicts_skipped += conflicts.count();
                if source.waive_known {
                    let change_w = eligible & !uw & (ow ^ w);
                    if !change_w.is_zero() {
                        let mut after = before.clone();
                        for i in change_w.ones() {
                            after[i as usize] = if w.get(i) { STATUS_WAIVED } else { STATUS_NONE };
                        }
                        r.write_status_span(ci, k * CHUNK, &before, &after)?;
                        result.changed_waived += change_w.count();
                        changed(ci);
                    }
                }
                if source.not_required_known {
                    set_nr.push((k, eligible & !un & n & !on));
                    clear_nr.push((k, eligible & !un & on & !n));
                }
                if source.visited_known {
                    visits.push((k, eligible & v & !ov));
                }
                result.applied += eligible.count();
                result.processed += work.count();
            }
            result.changed_not_required += own.flags.change_many(&mut guard, ci, &set_nr, NOT_REQUIRED, true)?;
            result.changed_not_required += own.flags.change_many(&mut guard, ci, &clear_nr, NOT_REQUIRED, false)?;
            result.changed_visited += own.flags.change_many(&mut guard, ci, &visits, VISITED, true)?;
            Ok(())
        };
        while let Some((ci, k, [work, uw, un, _, _])) = rows.next()? {
            if !batch.is_empty() && (batch_ci != Some(ci) || batch.len() >= 16) {
                run(batch_ci.unwrap(), &batch, &mut result)?;
                batch.clear();
                report.report_text(result.processed, total, "Apply unchanged-error review", false);
            }
            batch_ci = Some(ci);
            batch.push((k, work, uw, un));
        }
        if !batch.is_empty() {
            run(batch_ci.unwrap(), &batch, &mut result)?;
            report.report_text(result.processed, total, "Apply unchanged-error review", false);
        }
        check_cancelled(cancelled)?;
    }
    plan.refresh_conflicts(own, cancelled, progress)?;
    progress(total, total, "Previous review applied");
    Ok(result)
}
