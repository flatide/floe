//! The team's waives as a confirmer reads them (user 2026-11-13: "사용자중
//! 팀의 모든 waive를 뷰어에서 보고 confirm하는 역할이 필요함"): every error
//! somebody has waived, who waived it and why, who confirmed it - read from
//! the reviewers' own files as they stand (the waive sidecars, the confirm
//! files of `drc::confirm`, the reasons), found as the statistics find them
//! (`stats::discover`). Nobody's file is written here: a confirmation is
//! written by the confirmer's session into its own file (`confirm_chunk`,
//! through `desktop::Review`).
//!
//! A confirmer's other verdict is the rejection (0.12.468): the errors whose
//! waive a confirmer rejected are read from the confirmers' rejects logs
//! (`drc::reasons`, `Kind::Reject`). A rejected error is no waive in any of
//! the lists but its own (`Kind::Rejected`: every rejection that stands,
//! with who still has the error waived, who rejected it and why);
//! `reject_runs` says what a rejection by this session's confirmer is of.
//!
//! A `TeamWaives` is the files as they were opened: a list is paged against
//! one state, and opened anew to see what changed since.
use super::bits::{Bits, CHUNK};
use super::stats::{discover, open_confirm, open_waive, rejected_of, run_bits, waived_bits, Found};
use super::Identity;
use crate::drc::analysis::scope::Membership;
use crate::drc::confirm::{self, CONFIRMED};
use crate::drc::desktop::Review;
use crate::drc::reasons::{Kind as Reason, Reasons};
use crate::{check_cancelled, Error, ErrorKind, Result};
use std::collections::BTreeMap;
use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;

/// Which of the team's waives a list shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// every error somebody has waived
    Waived,
    /// ... that no confirmer has confirmed yet
    Unconfirmed,
    /// ... that a confirmer has confirmed
    Confirmed,
    /// every error whose waive a confirmer has rejected (none of them is in
    /// the lists above)
    Rejected,
}

impl Kind {
    pub fn parse(text: &str) -> Result<Self> {
        Ok(match text {
            "waived" => Self::Waived,
            "unconfirmed" => Self::Unconfirmed,
            "confirmed" => Self::Confirmed,
            "rejected" => Self::Rejected,
            _ => return Err(Error::input(format!("unsupported team waive list: {text}"))),
        })
    }
}

/// The team's waives of a rule, or of the result: waived by somebody,
/// confirmed of them, and the rest - and the errors whose waive is
/// rejected, which are none of those (said when there are any).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Counts {
    pub waived: u64,
    pub confirmed: u64,
    pub unconfirmed: u64,
    #[serde(skip_serializing_if = "super::is_zero")]
    pub rejected: u64,
}

/// One error of the list.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Row {
    pub error: u64,
    /// the reviewers who have it waived
    pub waived_by: Vec<String>,
    /// the reviewers who confirmed its waive
    pub confirmed_by: Vec<String>,
    /// why, as each reviewer who waived it said (a reviewer who gave no
    /// reason has no line)
    pub reasons: Vec<Why>,
    /// the confirmers who rejected its waive, each with why (the text is
    /// empty where they did not say)
    pub rejected_by: Vec<Why>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Why {
    pub reviewer: String,
    pub text: String,
    /// seconds since the epoch
    pub time: u64,
}

struct Person {
    tag: String,
    /// the waive sidecar and its waived counter a rule
    waive: Option<(File, Vec<u32>)>,
    /// the confirm file and its confirmed counter a rule
    confirm: Option<(File, Vec<u32>)>,
    reasons_path: Option<PathBuf>,
    /// read at the first row that asks (Some(None): none that reads)
    reasons: Option<Option<Reasons>>,
    /// the rejects log as read, and each rule's rejected errors as runs
    rejects: Option<(Reasons, Vec<Vec<(u64, u64)>>)>,
}

/// See the module docs.
pub struct TeamWaives {
    sizes: Vec<u64>,
    starts: Vec<u64>,
    /// the reasons files' `floe_pack=` value of this result
    tag: String,
    people: Vec<Person>,
    /// a rule's chunks: the errors somebody waived, the confirmed of
    /// them, and the rejected - counted at the first ask
    scans: BTreeMap<usize, Vec<(u16, u16, u16)>>,
    /// the files left out: reviewer, file, why
    pub skipped: Vec<(String, PathBuf, String)>,
}

/// The reviewers' working files of a result as they were found (the
/// folders listed once): what a host that asks often - an error's detail at
/// every selection - keeps for a moment, the listing being the cost of
/// finding them (a temp folder of 19,000 names: 8 ms).
pub struct Files(BTreeMap<String, Found>);

impl Files {
    pub fn find(pack: &Path) -> Result<Self> {
        Ok(Self(discover(pack)?))
    }
}

fn plane(file: &File, at: u64, buffer: &mut [u8]) -> Result<Bits> {
    file.read_exact_at(buffer, at)?;
    Ok(waived_bits(buffer))
}

impl TeamWaives {
    /// Every reviewer's waives and confirmations of the result at `pack`
    /// (`id`), as their files stand now.
    pub fn open(pack: &Path, id: &Identity) -> Result<Self> {
        Ok(Self::of(id, &Files::find(pack)?))
    }

    /// `open` of the files `files` found: opened and read now. A reviewer
    /// found by their waive sidecar has their confirm file and their
    /// reasons looked for by name as well - files made since the folders
    /// were listed (a first confirmation, a first reason) are read.
    pub fn of(id: &Identity, files: &Files) -> Self {
        let hex: String = id.waive_header[12..28].iter().map(|b| format!("{b:02x}")).collect();
        let mut me = Self {
            sizes: id.sizes.clone(),
            starts: id.starts.clone(),
            tag: format!("{hex},{}", id.total()),
            people: Vec::new(),
            scans: BTreeMap::new(),
            skipped: Vec::new(),
        };
        for (tag, files) in &files.0 {
            let beside = |from: &str, to: &str| {
                files.waive.as_ref().map(|(path, _)| {
                    let name = path.file_name().unwrap_or_default().to_string_lossy();
                    path.with_file_name(name.replacen(from, to, 1))
                })
            };
            let mut person = Person {
                tag: tag.clone(),
                waive: None,
                confirm: None,
                reasons_path: files
                    .reasons
                    .as_ref()
                    .map(|(path, _)| path.clone())
                    .or_else(|| beside(".waive.", ".reasons.")),
                reasons: None,
                rejects: None,
            };
            if let Some((path, _)) = &files.waive {
                match open_waive(path, id) {
                    Ok(waive) => person.waive = Some(waive),
                    Err(e) => me.skipped.push((tag.clone(), path.clone(), e.message)),
                }
            }
            let listed = files.confirm.as_ref().map(|(path, _)| path.clone());
            if let Some(path) = listed.clone().or_else(|| beside(".waive.", ".confirm.")) {
                match open_confirm(&path, id) {
                    Ok(confirm) => person.confirm = Some(confirm),
                    // (looked for by name, and not there: no confirmation)
                    Err(_) if listed.is_none() && !path.exists() => (),
                    Err(e) => me.skipped.push((tag.clone(), path, e.message)),
                }
            }
            let listed = files.rejects.as_ref().map(|(path, _)| path.clone());
            if let Some(path) = listed.or_else(|| beside(".waive.", ".rejects.")) {
                match Reasons::load(&path, &me.tag, &me.sizes) {
                    Ok(Some(log)) => {
                        let runs = rejected_of(&log, me.sizes.len());
                        person.rejects = Some((log, runs));
                    }
                    Ok(None) => (),
                    Err(e) => me.skipped.push((tag.clone(), path, e.message)),
                }
            }
            if person.waive.is_some() || person.confirm.is_some() || person.rejects.is_some() {
                me.people.push(person);
            }
        }
        me
    }

    /// The reviewers with a waive sidecar that reads.
    pub fn reviewers(&self) -> Vec<&str> {
        self.people.iter().filter(|p| p.waive.is_some()).map(|p| p.tag.as_str()).collect()
    }

    /// The reviewers who have confirmed anything.
    pub fn confirmers(&self) -> Vec<&str> {
        self.people
            .iter()
            .filter(|p| p.confirm.as_ref().is_some_and(|(_, counts)| counts.iter().any(|&c| c > 0)))
            .map(|p| p.tag.as_str())
            .collect()
    }

    fn rule(&self, check: usize) -> Result<u64> {
        self.sizes
            .get(check)
            .copied()
            .ok_or_else(|| Error::input("DRC check index out of range"))
    }

    /// Whether anybody has waived anything in rule `check`, or rejected
    /// anything in it.
    fn any(&self, check: usize) -> bool {
        self.people.iter().any(|p| {
            p.waive.as_ref().is_some_and(|(_, stored)| stored[check] > 0)
                || p.rejects.as_ref().is_some_and(|(_, runs)| !runs[check].is_empty())
        })
    }

    /// Chunk `k` of rule `check`: the errors somebody's file has waived,
    /// the ones somebody has confirmed (waived or not), and the ones whose
    /// waive somebody has rejected.
    fn planes(&self, check: usize, k: u64, buffer: &mut [u8]) -> Result<(Bits, Bits, Bits)> {
        let size = self.rule(check)?;
        if k >= size.div_ceil(CHUNK) {
            return Ok((Bits::ZERO, Bits::ZERO, Bits::ZERO));
        }
        let count = (size - k * CHUNK).min(CHUNK) as usize;
        let at = self.starts[check] + k * CHUNK;
        let (mut waived, mut confirmed, mut rejected) = (Bits::ZERO, Bits::ZERO, Bits::ZERO);
        for person in &self.people {
            if let Some((file, stored)) = &person.waive {
                if stored[check] > 0 {
                    waived |= plane(file, 40 + at, &mut buffer[..count])?;
                }
            }
            if let Some((file, stored)) = &person.confirm {
                if stored[check] > 0 {
                    confirmed |= plane(file, confirm::HEADER + at, &mut buffer[..count])?;
                }
            }
            if let Some((_, runs)) = &person.rejects {
                rejected |= run_bits(&runs[check], k * CHUNK, count as u64);
            }
        }
        Ok((waived, confirmed, rejected))
    }

    /// The errors of rule `check`'s chunk `k` that somebody has waived -
    /// a waive: the ones whose waive is rejected are not among them.
    pub fn waived(&self, check: usize, k: u64) -> Result<Bits> {
        if !self.any(check) {
            return Ok(Bits::ZERO);
        }
        let mut buffer = vec![0u8; CHUNK as usize];
        let (waived, _, rejected) = self.planes(check, k, &mut buffer)?;
        Ok(waived & !rejected)
    }

    /// The errors of rule `check`'s chunk `k` somebody's file has waived,
    /// rejected or not: what a rejection is of.
    fn held(&self, check: usize, k: u64) -> Result<Bits> {
        if !self.any(check) {
            return Ok(Bits::ZERO);
        }
        let mut buffer = vec![0u8; CHUNK as usize];
        Ok(self.planes(check, k, &mut buffer)?.0)
    }

    /// Rule `check`'s chunks, counted once: the waived by somebody, the
    /// confirmed of them, the rejected.
    fn scan(&mut self, check: usize, cancelled: &AtomicUsize) -> Result<&Vec<(u16, u16, u16)>> {
        if !self.scans.contains_key(&check) {
            let chunks = self.rule(check)?.div_ceil(CHUNK);
            let mut counts = Vec::new();
            if self.any(check) {
                counts.reserve(chunks as usize);
                let mut buffer = vec![0u8; CHUNK as usize];
                for k in 0..chunks {
                    if k % 256 == 255 {
                        check_cancelled(cancelled)?;
                    }
                    let (waived, confirmed, rejected) = self.planes(check, k, &mut buffer)?;
                    let waived = waived & !rejected;
                    counts.push((
                        waived.count() as u16,
                        (waived & confirmed).count() as u16,
                        rejected.count() as u16,
                    ));
                }
            }
            self.scans.insert(check, counts);
        }
        Ok(&self.scans[&check])
    }

    /// The team's waives of rule `check`, or of the whole result.
    pub fn counts(&mut self, check: Option<usize>, cancelled: &AtomicUsize) -> Result<Counts> {
        let rules: Vec<usize> = match check {
            Some(ci) => {
                self.rule(ci)?;
                vec![ci]
            }
            None => (0..self.sizes.len()).collect(),
        };
        let mut all = Counts::default();
        for ci in rules {
            for &(waived, confirmed, rejected) in self.scan(ci, cancelled)? {
                all.waived += u64::from(waived);
                all.confirmed += u64::from(confirmed);
                all.rejected += u64::from(rejected);
            }
        }
        all.unconfirmed = all.waived - all.confirmed;
        Ok(all)
    }

    /// The rules somebody has waived anything in, or rejected anything in,
    /// with their counts.
    pub fn rules(&mut self, cancelled: &AtomicUsize) -> Result<Vec<(usize, Counts)>> {
        let mut rules = Vec::new();
        for check in 0..self.sizes.len() {
            if self.any(check) {
                let counts = self.counts(Some(check), cancelled)?;
                if counts.waived > 0 || counts.rejected > 0 {
                    rules.push((check, counts));
                }
            }
        }
        Ok(rules)
    }

    /// The `limit` errors of rule `check`'s `kind` list from its `start`-th
    /// on, in the rule's order, and how many the list has.
    pub fn page(
        &mut self,
        check: usize,
        kind: Kind,
        start: u64,
        limit: u64,
        cancelled: &AtomicUsize,
    ) -> Result<(u64, Vec<u64>)> {
        let of = |&(waived, confirmed, rejected): &(u16, u16, u16)| -> u64 {
            match kind {
                Kind::Waived => u64::from(waived),
                Kind::Confirmed => u64::from(confirmed),
                Kind::Unconfirmed => u64::from(waived - confirmed),
                Kind::Rejected => u64::from(rejected),
            }
        };
        let per_chunk: Vec<u64> = self.scan(check, cancelled)?.iter().map(of).collect();
        let total: u64 = per_chunk.iter().sum();
        let mut ids = Vec::new();
        let mut skip = start;
        let mut buffer = vec![0u8; CHUNK as usize];
        for (k, &here) in per_chunk.iter().enumerate() {
            if ids.len() as u64 >= limit {
                break;
            }
            if here <= skip {
                skip -= here;
                continue;
            }
            let (waived, confirmed, rejected) = self.planes(check, k as u64, &mut buffer)?;
            let waived = waived & !rejected;
            let listed = match kind {
                Kind::Waived => waived,
                Kind::Confirmed => waived & confirmed,
                Kind::Unconfirmed => waived & !confirmed,
                Kind::Rejected => rejected,
            };
            for i in listed.ones() {
                if skip > 0 {
                    skip -= 1;
                } else if (ids.len() as u64) < limit {
                    ids.push(k as u64 * CHUNK + i);
                }
            }
        }
        Ok((total, ids))
    }

    /// Who has rule `check`'s `errors` waived, why, who confirmed them, and
    /// who rejected their waive and why.
    pub fn rows(&mut self, check: usize, errors: &[u64]) -> Result<Vec<Row>> {
        let size = self.rule(check)?;
        if errors.iter().any(|&e| e >= size) {
            return Err(Error::input("DRC error index out of range"));
        }
        let first = self.starts[check];
        let mut rows: Vec<Row> = errors
            .iter()
            .map(|&error| Row {
                error,
                waived_by: Vec::new(),
                confirmed_by: Vec::new(),
                reasons: Vec::new(),
                rejected_by: Vec::new(),
            })
            .collect();
        let (tag, sizes) = (self.tag.clone(), self.sizes.clone());
        let mut skipped = Vec::new();
        for person in &mut self.people {
            let mut byte = [0u8; 1];
            let waiving = person.waive.as_ref().filter(|(_, stored)| stored[check] > 0);
            let confirming = person.confirm.as_ref().filter(|(_, stored)| stored[check] > 0);
            for row in &mut rows {
                if let Some((file, _)) = waiving {
                    file.read_exact_at(&mut byte, 40 + first + row.error)?;
                    if byte[0] == crate::drc::desktop::STATUS_WAIVED {
                        row.waived_by.push(person.tag.clone());
                    }
                }
                if let Some((file, _)) = confirming {
                    file.read_exact_at(&mut byte, confirm::HEADER + first + row.error)?;
                    if byte[0] == CONFIRMED {
                        row.confirmed_by.push(person.tag.clone());
                    }
                }
            }
            // who rejected, and why as they said
            if let Some((log, _)) = &person.rejects {
                for row in &mut rows {
                    if let Some(entry) = log.of(Reason::Reject, check, row.error) {
                        row.rejected_by.push(Why {
                            reviewer: person.tag.clone(),
                            text: entry.text.clone(),
                            time: entry.time,
                        });
                    }
                }
            }
            // why: the reasons of a reviewer who waived any of them, read once
            if !rows.iter().any(|row| row.waived_by.last() == Some(&person.tag)) {
                continue;
            }
            if person.reasons.is_none() {
                person.reasons = Some(match &person.reasons_path {
                    None => None,
                    Some(path) => match Reasons::load(path, &tag, &sizes) {
                        Ok(reasons) => reasons,
                        Err(e) => {
                            skipped.push((person.tag.clone(), path.clone(), e.message));
                            None
                        }
                    },
                });
            }
            if let Some(Some(reasons)) = &person.reasons {
                for row in &mut rows {
                    if row.waived_by.last() != Some(&person.tag) {
                        continue;
                    }
                    if let Some(entry) = reasons.of(Reason::Waive, check, row.error) {
                        row.reasons.push(Why {
                            reviewer: person.tag.clone(),
                            text: entry.text.clone(),
                            time: entry.time,
                        });
                    }
                }
            }
        }
        self.skipped.append(&mut skipped);
        Ok(rows)
    }
}

/// One chunk of a confirmation by `review`'s reviewer: of rule `check`'s
/// chunk `k`, the errors `members` names (None: every one). Confirming takes
/// the ones somebody has waived - as `view` read them - and this reviewer
/// has not confirmed yet; taking back takes the ones this reviewer has
/// confirmed. (changed, asked to confirm and nobody's waive). The caller
/// ends its request or job with `Review::sync_waives`.
pub fn confirm_chunk(
    review: &mut Review,
    view: &TeamWaives,
    check: usize,
    k: u64,
    members: Option<&dyn Membership>,
    on: bool,
) -> Result<(u64, u64)> {
    let size = view.rule(check)?;
    if k >= size.div_ceil(CHUNK) {
        return Ok((0, 0));
    }
    let count = (size - k * CHUNK).min(CHUNK);
    let asked = match members {
        None => Bits::first(count),
        Some(members) => {
            let mut mask = vec![false; count as usize];
            members.mask(k * CHUNK, &mut mask);
            Bits::from_mask(&mask)
        }
    };
    if asked.is_zero() {
        return Ok((0, 0));
    }
    if !review.confirmer() {
        return Err(Error::new(
            ErrorKind::Unsupported,
            "confirming waives is the confirmer's: start the viewer with --confirmer",
        ));
    }
    let before = review.confirms.statuses(check, k * CHUNK, count)?;
    let mine = waived_bits(&before);
    let (target, skipped) = if on {
        let waived = view.waived(check, k)?;
        // (errors named one by one and nobody's waive are said; "every one"
        // names none)
        let skipped = if members.is_some() { (asked & !waived).count() } else { 0 };
        (asked & waived & !mine, skipped)
    } else {
        (asked & mine, 0)
    };
    let (Some(lo), Some(hi)) = (target.ones().next(), target.ones().last()) else {
        return Ok((0, skipped));
    };
    let (lo, hi) = (lo as usize, hi as usize);
    let mut after = before[lo..=hi].to_vec();
    for i in target.ones() {
        after[i as usize - lo] = if on { CONFIRMED } else { 0 };
    }
    let changed = review.confirm_span(check, k * CHUNK + lo as u64, &before[lo..=hi], &after)?;
    Ok((changed, skipped))
}

/// What a rejection by `review`'s confirmer - or its taking back - is of:
/// of rule `check`'s `errors`, the ones somebody has waived (as `view` read
/// the files; a waive another confirmer rejected and its reviewer has yet to
/// turn back is one) and this confirmer has not rejected yet; taking back,
/// the ones this confirmer has rejected. (the runs, the errors asked to
/// reject that are nobody's waive.) The caller hands the runs to
/// `Review::reject_set` or `reject_drop`, which is one line of the
/// confirmer's log whatever their number.
pub fn reject_runs(
    review: &Review,
    view: &TeamWaives,
    check: usize,
    errors: &[u64],
    on: bool,
) -> Result<(Vec<(u64, u64)>, u64)> {
    let size = view.rule(check)?;
    if errors.iter().any(|&e| e >= size) {
        return Err(Error::input("DRC error index out of range"));
    }
    let mut sorted = errors.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let (mut runs, mut skipped): (Vec<(u64, u64)>, u64) = (Vec::new(), 0);
    let mut i = 0;
    while i < sorted.len() {
        let k = sorted[i] / CHUNK;
        let mut asked = Bits::ZERO;
        while i < sorted.len() && sorted[i] / CHUNK == k {
            asked.set(sorted[i] % CHUNK);
            i += 1;
        }
        let count = (size - k * CHUNK).min(CHUNK);
        let mine: Vec<(u64, u64)> = review.rejects.held(Reason::Reject, check, &[(k * CHUNK, k * CHUNK + count)]);
        let mine = run_bits(&mine, k * CHUNK, count);
        let target = if on {
            let waived = view.held(check, k)?;
            skipped += (asked & !waived & !mine).count();
            asked & waived & !mine
        } else {
            asked & mine
        };
        for j in target.ones() {
            let error = k * CHUNK + j;
            match runs.last_mut() {
                Some(last) if last.1 == error => last.1 = error + 1,
                _ => runs.push((error, error + 1)),
            }
        }
    }
    Ok((runs, skipped))
}
