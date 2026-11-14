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
//! (`drc::reasons`, `Kind::Reject`). A rejection is of each reviewer's waive
//! until that reviewer answers it by waiving again (0.12.469,
//! `confirm::unanswered`): a rejected waive is no waive in any of the lists,
//! and an error whose waives are all rejected is in the list of its own
//! (`Kind::Rejected`) - who has it waived, who rejected it and why. What a
//! reviewer's own rejects log keeps of an error - the reasons its waive had,
//! the rejections that were answered - is the row's `history`.
//! `reject_runs` says what a rejection by this session's confirmer is of.
//!
//! Before a waive there may be a request (0.12.472; the roles' workflow):
//! the errors a requester asked the waive of are read from the requesters'
//! request files (`drc::confirm`), and the ones nobody has waived yet are a
//! list of their own (`Kind::Requested`) - what waits for a waiver; a row
//! says who asked and why (`requested_by`).
//!
//! A waiver's verdict on a request is the decline (0.12.473; user
//! 2026-11-14: "요청 거절 상태도 만들어줘"), read from the waivers' rejects
//! logs (`Kind::Decline`) as a confirmer's rejection is: a decline is of each
//! requester's request until that requester answers it by asking again, a
//! declined request is none in the list of the requested, and an error whose
//! requests are all declined - and nobody has waived - is in the list of its
//! own (`Kind::Declined`): who asked, who declined and why, and what the
//! requesters' logs keep of their requests. `decline_runs` says what a
//! decline by this session's waiver is of.
//!
//! A `TeamWaives` is the files as they were opened: a list is paged against
//! one state, and opened anew to see what changed since.
use super::bits::{Bits, CHUNK};
use super::stats::{discover, open_confirm, open_request, open_waive, run_bits, waived_bits, Found};
use super::Identity;
use crate::drc::analysis::scope::Membership;
use crate::drc::confirm::{self, CONFIRMED};
use crate::drc::desktop::{history_of, Event, Review};
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
    /// every error whose waives are all rejected (none of them is in the
    /// lists above): what waits for its reviewers
    Rejected,
    /// every error somebody asked the waive of and nobody has waived (none
    /// of them is in the lists above either): what waits for a waiver. A
    /// request a waiver declined is none
    Requested,
    /// every error whose requests are all declined and nobody has waived:
    /// what waits for its requesters
    Declined,
}

impl Kind {
    pub fn parse(text: &str) -> Result<Self> {
        Ok(match text {
            "waived" => Self::Waived,
            "unconfirmed" => Self::Unconfirmed,
            "confirmed" => Self::Confirmed,
            "rejected" => Self::Rejected,
            "requested" => Self::Requested,
            "declined" => Self::Declined,
            _ => return Err(Error::input(format!("unsupported team waive list: {text}"))),
        })
    }
}

/// The team's waives of a rule, or of the result: waived by somebody,
/// confirmed of them, and the rest - and the errors whose waives are all
/// rejected, which are none of those (said when there are any).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Counts {
    pub waived: u64,
    pub confirmed: u64,
    pub unconfirmed: u64,
    #[serde(skip_serializing_if = "super::is_zero")]
    pub rejected: u64,
    /// the errors asked for and waived by nobody (said when there are any)
    #[serde(skip_serializing_if = "super::is_zero")]
    pub requested: u64,
    /// the errors whose requests are all declined, waived by nobody (said
    /// when there are any)
    #[serde(skip_serializing_if = "super::is_zero")]
    pub declined: u64,
}

/// One error of the list.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Row {
    pub error: u64,
    /// the reviewers whose file has it waived - a rejected waive too
    pub waived_by: Vec<String>,
    /// of those, the ones whose waive is rejected
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rejected_of: Vec<String>,
    /// the reviewers who confirmed its waive
    pub confirmed_by: Vec<String>,
    /// why, as each reviewer who waived it said (a reviewer who gave no
    /// reason has no line)
    pub reasons: Vec<Why>,
    /// the confirmers whose rejection of its waive stands and one of its
    /// reviewers has not answered, each with why (the text is empty where
    /// they did not say)
    pub rejected_by: Vec<Why>,
    /// what the logs of the reviewers who have it waived - or asked for -
    /// keep of it: the reasons the waive had and the rejections that were
    /// answered, the reasons the request had and the declines that were
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<Past>,
    /// the requesters who asked for its waive, each with why (the text is
    /// empty where they did not say) - a declined request too
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub requested_by: Vec<Why>,
    /// of those, the ones whose request is declined
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub declined_of: Vec<String>,
    /// the waivers whose decline of its request stands and one of its
    /// requesters has not answered, each with why
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub declined_by: Vec<Why>,
}

/// One kept event of a reviewer's waive of an error (`desktop::Event`).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Past {
    /// whose waive
    pub reviewer: String,
    #[serde(flatten)]
    pub event: Event,
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
    /// the request file and its requested counter a rule
    request: Option<(File, Vec<u32>)>,
    reasons_path: Option<PathBuf>,
    /// read at the first row that asks (Some(None): none that reads)
    reasons: Option<Option<Reasons>>,
    /// the rejects log as read: the rejections this reviewer made, and
    /// the ones of their own waives they answered
    rejects: Option<Reasons>,
}

/// See the module docs.
pub struct TeamWaives {
    sizes: Vec<u64>,
    starts: Vec<u64>,
    /// the reasons files' `floe_pack=` value of this result
    tag: String,
    people: Vec<Person>,
    /// a rule's chunks: the errors somebody waived, the confirmed of
    /// them, the rejected, the ones asked for and waived by nobody, and
    /// the declined - counted at the first ask
    scans: BTreeMap<usize, Vec<[u16; 5]>>,
    /// a rule's rejections that stand, and each reviewer's errors under one
    /// they have not answered (in `people`'s order) - made at the first ask
    opens: BTreeMap<usize, (Vec<confirm::Standing>, Vec<Vec<(u64, u64)>>)>,
    /// the same of the rule's declines: each reviewer's errors under one
    /// they have not answered - a request of theirs there is declined
    shuts: BTreeMap<usize, (Vec<confirm::Standing>, Vec<Vec<(u64, u64)>>)>,
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
            opens: BTreeMap::new(),
            shuts: BTreeMap::new(),
            skipped: Vec::new(),
        };
        for (tag, files) in &files.0 {
            // (a reviewer's other files are named from their waive sidecar
            // - or, for one who only asks, from their request file)
            let beside = |to: &str| {
                let named = |path: &PathBuf, from: &str| {
                    let name = path.file_name().unwrap_or_default().to_string_lossy();
                    path.with_file_name(name.replacen(from, to, 1))
                };
                match (&files.waive, &files.request) {
                    (Some((path, _)), _) => Some(named(path, ".waive.")),
                    (None, Some((path, _))) => Some(named(path, ".request.")),
                    (None, None) => None,
                }
            };
            let mut person = Person {
                tag: tag.clone(),
                waive: None,
                confirm: None,
                request: None,
                reasons_path: files
                    .reasons
                    .as_ref()
                    .map(|(path, _)| path.clone())
                    .or_else(|| beside(".reasons.")),
                reasons: None,
                rejects: None,
            };
            let listed = files.request.as_ref().map(|(path, _)| path.clone());
            if let Some(path) = listed.clone().or_else(|| beside(".request.")) {
                match open_request(&path, id) {
                    Ok(request) => person.request = Some(request),
                    // (looked for by name, and not there: nothing asked)
                    Err(_) if listed.is_none() && !path.exists() => (),
                    Err(e) => me.skipped.push((tag.clone(), path, e.message)),
                }
            }
            if let Some((path, _)) = &files.waive {
                match open_waive(path, id) {
                    Ok(waive) => person.waive = Some(waive),
                    Err(e) => me.skipped.push((tag.clone(), path.clone(), e.message)),
                }
            }
            let listed = files.confirm.as_ref().map(|(path, _)| path.clone());
            if let Some(path) = listed.clone().or_else(|| beside(".confirm.")) {
                match open_confirm(&path, id) {
                    Ok(confirm) => person.confirm = Some(confirm),
                    // (looked for by name, and not there: no confirmation)
                    Err(_) if listed.is_none() && !path.exists() => (),
                    Err(e) => me.skipped.push((tag.clone(), path, e.message)),
                }
            }
            let listed = files.rejects.as_ref().map(|(path, _)| path.clone());
            if let Some(path) = listed.or_else(|| beside(".rejects.")) {
                match Reasons::load(&path, &me.tag, &me.sizes) {
                    Ok(Some(log)) => person.rejects = Some(log),
                    Ok(None) => (),
                    Err(e) => me.skipped.push((tag.clone(), path, e.message)),
                }
            }
            if person.waive.is_some()
                || person.confirm.is_some()
                || person.rejects.is_some()
                || person.request.is_some()
            {
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

    /// Whether anybody has waived, or asked for, anything in rule `check`.
    fn any(&self, check: usize) -> bool {
        self.people
            .iter()
            .any(|p| p.waive.iter().chain(&p.request).any(|(_, stored)| stored[check] > 0))
    }

    /// Rule `check`'s rejections that stand - every reviewer's log's - and
    /// each reviewer's errors under one they have not answered: a waive of
    /// theirs there is rejected.
    fn rejections(&mut self, check: usize) -> &(Vec<confirm::Standing>, Vec<Vec<(u64, u64)>>) {
        if !self.opens.contains_key(&check) {
            let standing: Vec<confirm::Standing> = self
                .people
                .iter()
                .flat_map(|p| p.rejects.iter().flat_map(|log| confirm::standing_of(&p.tag, log, check)))
                .collect();
            let own = self
                .people
                .iter()
                .map(|p| match standing.is_empty() {
                    true => Vec::new(),
                    false => confirm::unanswered(&standing, p.rejects.as_ref(), check),
                })
                .collect();
            self.opens.insert(check, (standing, own));
        }
        &self.opens[&check]
    }

    /// Rule `check`'s declines that stand - every waiver's log's - and each
    /// reviewer's errors under one they have not answered: a request of
    /// theirs there is declined.
    fn declines(&mut self, check: usize) -> &(Vec<confirm::Standing>, Vec<Vec<(u64, u64)>>) {
        if !self.shuts.contains_key(&check) {
            let standing: Vec<confirm::Standing> = self
                .people
                .iter()
                .flat_map(|p| p.rejects.iter().flat_map(|log| confirm::standing_by(&p.tag, log, check, Reason::Decline)))
                .collect();
            let own = self
                .people
                .iter()
                .map(|p| match standing.is_empty() {
                    true => Vec::new(),
                    false => confirm::unanswered_by(&standing, p.rejects.as_ref(), check, Reason::Reasked),
                })
                .collect();
            self.shuts.insert(check, (standing, own));
        }
        &self.shuts[&check]
    }

    /// Chunk `k` of rule `check`: the errors somebody has waived - a waive
    /// that is not rejected -, the ones somebody has confirmed (waived or
    /// not), the ones whose waives are all rejected, the ones somebody
    /// asked the waive of - a request that is not declined - that nobody
    /// has waived, and the ones whose requests are all declined that nobody
    /// has waived.
    fn planes(&mut self, check: usize, k: u64, buffer: &mut [u8]) -> Result<[Bits; 5]> {
        let size = self.rule(check)?;
        if k >= size.div_ceil(CHUNK) {
            return Ok([Bits::ZERO; 5]);
        }
        let count = (size - k * CHUNK).min(CHUNK) as usize;
        let at = self.starts[check] + k * CHUNK;
        self.rejections(check);
        self.declines(check);
        let open = &self.opens[&check].1;
        let shut = &self.shuts[&check].1;
        let (mut waived, mut confirmed, mut rejected) = (Bits::ZERO, Bits::ZERO, Bits::ZERO);
        let (mut asked, mut any, mut declined) = (Bits::ZERO, Bits::ZERO, Bits::ZERO);
        for (i, person) in self.people.iter().enumerate() {
            if let Some((file, stored)) = &person.waive {
                if stored[check] > 0 {
                    let held = plane(file, 40 + at, &mut buffer[..count])?;
                    let gone = run_bits(&open[i], k * CHUNK, count as u64);
                    waived |= held & !gone;
                    rejected |= held & gone;
                    any |= held;
                }
            }
            if let Some((file, stored)) = &person.confirm {
                if stored[check] > 0 {
                    confirmed |= plane(file, confirm::HEADER + at, &mut buffer[..count])?;
                }
            }
            if let Some((file, stored)) = &person.request {
                if stored[check] > 0 {
                    let held = plane(file, confirm::HEADER + at, &mut buffer[..count])?;
                    let gone = run_bits(&shut[i], k * CHUNK, count as u64);
                    asked |= held & !gone;
                    declined |= held & gone;
                }
            }
        }
        Ok([waived, confirmed, rejected & !waived, asked & !any, declined & !asked & !any])
    }

    /// The errors of rule `check`'s chunk `k` that somebody has waived -
    /// a waive: a rejected one is none.
    pub fn waived(&mut self, check: usize, k: u64) -> Result<Bits> {
        if !self.any(check) {
            return Ok(Bits::ZERO);
        }
        let mut buffer = vec![0u8; CHUNK as usize];
        Ok(self.planes(check, k, &mut buffer)?[0])
    }

    /// The errors of rule `check`'s chunk `k` somebody asked the waive of
    /// and nobody has waived - a request: a declined one is none.
    pub fn requested(&mut self, check: usize, k: u64) -> Result<Bits> {
        if !self.any(check) {
            return Ok(Bits::ZERO);
        }
        let mut buffer = vec![0u8; CHUNK as usize];
        Ok(self.planes(check, k, &mut buffer)?[3])
    }

    /// Rule `check`'s chunks, counted once: the waived by somebody, the
    /// confirmed of them, the rejected, the asked for that wait, the
    /// declined.
    fn scan(&mut self, check: usize, cancelled: &AtomicUsize) -> Result<&Vec<[u16; 5]>> {
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
                    let [waived, confirmed, rejected, asked, declined] = self.planes(check, k, &mut buffer)?;
                    counts.push([
                        waived.count() as u16,
                        (waived & confirmed).count() as u16,
                        rejected.count() as u16,
                        asked.count() as u16,
                        declined.count() as u16,
                    ]);
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
            for &[waived, confirmed, rejected, asked, declined] in self.scan(ci, cancelled)? {
                all.waived += u64::from(waived);
                all.confirmed += u64::from(confirmed);
                all.rejected += u64::from(rejected);
                all.requested += u64::from(asked);
                all.declined += u64::from(declined);
            }
        }
        all.unconfirmed = all.waived - all.confirmed;
        Ok(all)
    }

    /// The rules somebody has waived anything in, rejected anything in, or
    /// asked for anything in, with their counts.
    pub fn rules(&mut self, cancelled: &AtomicUsize) -> Result<Vec<(usize, Counts)>> {
        let mut rules = Vec::new();
        for check in 0..self.sizes.len() {
            if self.any(check) {
                let counts = self.counts(Some(check), cancelled)?;
                if counts.waived > 0 || counts.rejected > 0 || counts.requested > 0 || counts.declined > 0 {
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
        let of = |&[waived, confirmed, rejected, asked, declined]: &[u16; 5]| -> u64 {
            match kind {
                Kind::Waived => u64::from(waived),
                Kind::Confirmed => u64::from(confirmed),
                Kind::Unconfirmed => u64::from(waived - confirmed),
                Kind::Rejected => u64::from(rejected),
                Kind::Requested => u64::from(asked),
                Kind::Declined => u64::from(declined),
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
            let [waived, confirmed, rejected, asked, declined] = self.planes(check, k as u64, &mut buffer)?;
            let listed = match kind {
                Kind::Waived => waived,
                Kind::Confirmed => waived & confirmed,
                Kind::Unconfirmed => waived & !confirmed,
                Kind::Rejected => rejected,
                Kind::Requested => asked,
                Kind::Declined => declined,
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

    /// Who has rule `check`'s `errors` waived - and whose waive of them is
    /// rejected -, why, who confirmed them, who rejected their waive and
    /// why, and what is kept of their waives; who asked for their waive -
    /// and whose request is declined -, who declined and why.
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
                rejected_of: Vec::new(),
                confirmed_by: Vec::new(),
                reasons: Vec::new(),
                rejected_by: Vec::new(),
                history: Vec::new(),
                requested_by: Vec::new(),
                declined_of: Vec::new(),
                declined_by: Vec::new(),
            })
            .collect();
        let (tag, sizes) = (self.tag.clone(), self.sizes.clone());
        let mut skipped = Vec::new();
        let (standing, open) = self.rejections(check).clone();
        let (declines, shut) = self.declines(check).clone();
        let holds = |runs: &[(u64, u64)], error: u64| {
            let i = runs.partition_point(|r| r.1 <= error);
            runs.get(i).is_some_and(|r| r.0 <= error)
        };
        for (i, person) in self.people.iter_mut().enumerate() {
            let mut byte = [0u8; 1];
            let waiving = person.waive.as_ref().filter(|(_, stored)| stored[check] > 0);
            let confirming = person.confirm.as_ref().filter(|(_, stored)| stored[check] > 0);
            let asking = person.request.as_ref().filter(|(_, stored)| stored[check] > 0);
            // (the rows this reviewer asked the waive of)
            let mut asked: Vec<usize> = Vec::new();
            for (at, row) in rows.iter_mut().enumerate() {
                // (what this reviewer's log keeps of the error is said once,
                // for their request of it or their waive)
                let mut kept = false;
                if let Some((file, _)) = asking {
                    file.read_exact_at(&mut byte, confirm::HEADER + first + row.error)?;
                    if byte[0] == crate::drc::desktop::STATUS_WAIVED {
                        asked.push(at);
                        kept = true;
                        if holds(&shut[i], row.error) {
                            // declined: by the declines that stand on it
                            // and this requester has not answered
                            row.declined_of.push(person.tag.clone());
                            let at = [(row.error, row.error + 1)];
                            let by = confirm::rejecting_by(&declines, person.rejects.as_ref(), check, &at, Reason::Reasked);
                            for by in by {
                                if !row.declined_by.iter().any(|w| w.reviewer == by.reviewer && w.time == by.time) {
                                    row.declined_by.push(Why { reviewer: by.reviewer, text: by.text, time: by.time });
                                }
                            }
                        }
                    }
                }
                if let Some((file, _)) = waiving {
                    file.read_exact_at(&mut byte, 40 + first + row.error)?;
                    if byte[0] == crate::drc::desktop::STATUS_WAIVED {
                        row.waived_by.push(person.tag.clone());
                        if holds(&open[i], row.error) {
                            // rejected: by the rejections that stand on it
                            // and this reviewer has not answered
                            row.rejected_of.push(person.tag.clone());
                            let at = [(row.error, row.error + 1)];
                            for by in confirm::rejecting(&standing, person.rejects.as_ref(), check, &at) {
                                if !row.rejected_by.iter().any(|w| w.reviewer == by.reviewer && w.time == by.time) {
                                    row.rejected_by.push(Why { reviewer: by.reviewer, text: by.text, time: by.time });
                                }
                            }
                        }
                        kept = true;
                    }
                }
                // what is kept of this reviewer's waive of it, or request
                if let Some(log) = person.rejects.as_ref().filter(|_| kept) {
                    row.history.extend(
                        history_of(log, check, row.error)
                            .into_iter()
                            .map(|event| Past { reviewer: person.tag.clone(), event }),
                    );
                }
                if let Some((file, _)) = confirming {
                    file.read_exact_at(&mut byte, confirm::HEADER + first + row.error)?;
                    if byte[0] == CONFIRMED {
                        row.confirmed_by.push(person.tag.clone());
                    }
                }
            }
            // why: the reasons of a reviewer who waived any of them, or
            // asked for the waive of any, read once
            if asked.is_empty() && !rows.iter().any(|row| row.waived_by.last() == Some(&person.tag)) {
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
            // (who asked is said whether they said why or not)
            for &at in &asked {
                let why = match &person.reasons {
                    Some(Some(reasons)) => reasons.of(Reason::Request, check, rows[at].error),
                    _ => None,
                };
                rows[at].requested_by.push(Why {
                    reviewer: person.tag.clone(),
                    text: why.map(|entry| entry.text.clone()).unwrap_or_default(),
                    time: why.map_or(0, |entry| entry.time),
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
        // (a row's history is a reviewer's after the other, each in the
        // order things happened in)
        for row in &mut rows {
            row.rejected_by.sort_by(|a, b| (&a.reviewer, a.time).cmp(&(&b.reviewer, b.time)));
            row.declined_by.sort_by(|a, b| (&a.reviewer, a.time).cmp(&(&b.reviewer, b.time)));
        }
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
    view: &mut TeamWaives,
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
            "confirming waives is the confirmer's: start the viewer with --role confirmer",
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

/// The errors a verdict is asked of: named one by one, or a set - a
/// selection, a scope - whose every chunk is asked.
enum Asked<'a> {
    Named(Vec<u64>),
    Set(&'a dyn Membership),
}

/// Each chunk of a rule of `size` errors that has any of `asked`, with the
/// errors asked in it: `each(chunk, the chunk's errors, the asked ones)`.
fn asked_chunks(
    size: u64,
    asked: &Asked,
    cancelled: &AtomicUsize,
    each: &mut dyn FnMut(u64, u64, Bits) -> Result<()>,
) -> Result<()> {
    match asked {
        Asked::Named(errors) => {
            let mut i = 0;
            while i < errors.len() {
                let k = errors[i] / CHUNK;
                let mut bits = Bits::ZERO;
                while i < errors.len() && errors[i] / CHUNK == k {
                    bits.set(errors[i] % CHUNK);
                    i += 1;
                }
                each(k, (size - k * CHUNK).min(CHUNK), bits)?;
            }
        }
        Asked::Set(members) => {
            let mut mask = vec![false; CHUNK as usize];
            for k in 0..size.div_ceil(CHUNK) {
                if k % 256 == 255 {
                    check_cancelled(cancelled)?;
                }
                let count = (size - k * CHUNK).min(CHUNK);
                members.mask(k * CHUNK, &mut mask[..count as usize]);
                let bits = Bits::from_mask(&mask[..count as usize]);
                if !bits.is_zero() {
                    each(k, count, bits)?;
                }
            }
        }
    }
    Ok(())
}

/// The errors named, sorted and each once - refused when one is past the
/// rule.
fn named(view: &TeamWaives, check: usize, errors: &[u64]) -> Result<Vec<u64>> {
    let size = view.rule(check)?;
    if errors.iter().any(|&e| e >= size) {
        return Err(Error::input("DRC error index out of range"));
    }
    let mut sorted = errors.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    Ok(sorted)
}

fn push(runs: &mut Vec<(u64, u64)>, k: u64, target: Bits) {
    for j in target.ones() {
        let error = k * CHUNK + j;
        match runs.last_mut() {
            Some(last) if last.1 == error => last.1 = error + 1,
            _ => runs.push((error, error + 1)),
        }
    }
}

fn reject_of(
    review: &Review,
    view: &mut TeamWaives,
    check: usize,
    asked: Asked,
    on: bool,
    cancelled: &AtomicUsize,
) -> Result<(Vec<(u64, u64)>, u64)> {
    let size = view.rule(check)?;
    let (mut runs, mut skipped): (Vec<(u64, u64)>, u64) = (Vec::new(), 0);
    asked_chunks(size, &asked, cancelled, &mut |k, count, asked| {
        let target = if on {
            let waived = view.waived(check, k)?;
            skipped += (asked & !waived).count();
            asked & waived
        } else {
            let mine: Vec<(u64, u64)> = review.rejects.held(Reason::Reject, check, &[(k * CHUNK, k * CHUNK + count)]);
            asked & run_bits(&mine, k * CHUNK, count)
        };
        push(&mut runs, k, target);
        Ok(())
    })?;
    Ok((runs, skipped))
}

/// What a rejection by `review`'s confirmer - or its taking back - is of:
/// of rule `check`'s `errors`, the ones somebody has waived (as `view` read
/// the files: a waive that is not rejected - an error whose waives are all
/// rejected waits for its reviewers, and is not rejected again); taking
/// back, the ones this confirmer's log holds. (the runs, the errors asked
/// to reject that are nobody's waive.) The caller hands the runs to
/// `Review::reject_set` or `reject_drop`, which is one line of the
/// confirmer's log whatever their number.
pub fn reject_runs(
    review: &Review,
    view: &mut TeamWaives,
    check: usize,
    errors: &[u64],
    on: bool,
) -> Result<(Vec<(u64, u64)>, u64)> {
    let asked = Asked::Named(named(view, check, errors)?);
    reject_of(review, view, check, asked, on, &AtomicUsize::new(0))
}

/// `reject_runs`, the errors being a set - a selection of any size, a scope
/// (0.12.474; user 2026-11-14: "그룹으로 한번에 waive된 것을 하나씩 확인하기는
/// 매우 어려울 것으로 예상되므로 해당 그룹도 한번에 확인하고 confirm 할 수
/// 있는"): one line of the confirmer's log for the whole of it.
pub fn reject_members(
    review: &Review,
    view: &mut TeamWaives,
    check: usize,
    members: &dyn Membership,
    on: bool,
    cancelled: &AtomicUsize,
) -> Result<(Vec<(u64, u64)>, u64)> {
    reject_of(review, view, check, Asked::Set(members), on, cancelled)
}

fn decline_of(
    review: &Review,
    view: &mut TeamWaives,
    check: usize,
    asked: Asked,
    on: bool,
    cancelled: &AtomicUsize,
) -> Result<(Vec<(u64, u64)>, u64)> {
    let size = view.rule(check)?;
    let (mut runs, mut skipped): (Vec<(u64, u64)>, u64) = (Vec::new(), 0);
    asked_chunks(size, &asked, cancelled, &mut |k, count, asked| {
        let target = if on {
            let waiting = view.requested(check, k)?;
            skipped += (asked & !waiting).count();
            asked & waiting
        } else {
            let mine: Vec<(u64, u64)> = review.rejects.held(Reason::Decline, check, &[(k * CHUNK, k * CHUNK + count)]);
            asked & run_bits(&mine, k * CHUNK, count)
        };
        push(&mut runs, k, target);
        Ok(())
    })?;
    Ok((runs, skipped))
}

/// What a decline by `review`'s waiver - or its taking back - is of: of rule
/// `check`'s `errors`, the ones somebody asked the waive of and nobody has
/// waived (as `view` read the files: a request that is not declined - an
/// error whose requests are all declined waits for its requesters, and is
/// not declined again); taking back, the ones this waiver's log holds. (the
/// runs, the errors asked to decline that have no request waiting.) The
/// caller hands the runs to `Review::decline_set` or `decline_drop`, which
/// is one line of the waiver's log whatever their number.
pub fn decline_runs(
    review: &Review,
    view: &mut TeamWaives,
    check: usize,
    errors: &[u64],
    on: bool,
) -> Result<(Vec<(u64, u64)>, u64)> {
    let asked = Asked::Named(named(view, check, errors)?);
    decline_of(review, view, check, asked, on, &AtomicUsize::new(0))
}

/// `decline_runs`, the errors being a set (`reject_members`).
pub fn decline_members(
    review: &Review,
    view: &mut TeamWaives,
    check: usize,
    members: &dyn Membership,
    on: bool,
    cancelled: &AtomicUsize,
) -> Result<(Vec<(u64, u64)>, u64)> {
    decline_of(review, view, check, Asked::Set(members), on, cancelled)
}

/// The errors of a rule that one reviewer marked with one action - what was
/// waived, or asked for, together (0.12.474): the errors their reasons file
/// keeps under one line, an action being a line whatever its size. Marks
/// made without a reason have no line: they are one group a reviewer and
/// rule, the ones that say no why.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Group {
    /// "waive": a waiver's waives; "request": a requester's requests
    pub stage: &'static str,
    /// whose marks
    pub reviewer: String,
    /// why, as they said for the action (empty: the marks without a reason)
    pub text: String,
    /// when (seconds since the epoch; 0 for the marks without a reason)
    pub time: u64,
    /// the group's errors that are marked now (sorted runs)
    #[serde(skip)]
    pub runs: Vec<(u64, u64)>,
}

impl TeamWaives {
    /// The group rule `check`'s error `error` was marked with (`Group`):
    /// a waive's before a request's - what a confirmer looks at -, a mark
    /// that counts before one sent back, the first reviewer of several.
    /// None for an error nobody has waived and nobody asked the waive of.
    pub fn group_of(&mut self, check: usize, error: u64, cancelled: &AtomicUsize) -> Result<Option<Group>> {
        const STEP: u64 = 1 << 20;
        let size = self.rule(check)?;
        if error >= size {
            return Err(Error::input("DRC error index out of range"));
        }
        let first = self.starts[check];
        self.rejections(check);
        self.declines(check);
        let (open, shut) = (&self.opens[&check].1, &self.shuts[&check].1);
        let holds = |runs: &[(u64, u64)]| {
            let i = runs.partition_point(|r| r.1 <= error);
            runs.get(i).is_some_and(|r| r.0 <= error)
        };
        // whose mark of it: (what it ranks, the person, the stage)
        let mut pick: Option<(u8, usize, &'static str)> = None;
        let mut byte = [0u8; 1];
        for (i, person) in self.people.iter().enumerate() {
            let files = [(&person.waive, 40, "waive", &open[i]), (&person.request, confirm::HEADER, "request", &shut[i])];
            for (file, head, stage, back) in files {
                let Some((file, _)) = file.as_ref().filter(|(_, stored)| stored[check] > 0) else {
                    continue;
                };
                file.read_exact_at(&mut byte, head + first + error)?;
                if byte[0] != crate::drc::desktop::STATUS_WAIVED {
                    continue;
                }
                let rank = 2 * u8::from(stage == "waive") + u8::from(!holds(back));
                if pick.is_none_or(|(best, ..)| rank > best) {
                    pick = Some((rank, i, stage));
                }
            }
        }
        let Some((_, i, stage)) = pick else {
            return Ok(None);
        };
        let (tag, sizes) = (self.tag.clone(), self.sizes.clone());
        let person = &mut self.people[i];
        if person.reasons.is_none() {
            person.reasons = Some(match &person.reasons_path {
                None => None,
                Some(path) => match Reasons::load(path, &tag, &sizes) {
                    Ok(reasons) => reasons,
                    Err(e) => {
                        self.skipped.push((person.tag.clone(), path.clone(), e.message));
                        None
                    }
                },
            });
        }
        let kind = if stage == "waive" { Reason::Waive } else { Reason::Request };
        let (file, head) = match stage {
            "waive" => (&person.waive.as_ref().expect("picked by it").0, 40),
            _ => (&person.request.as_ref().expect("picked by it").0, confirm::HEADER),
        };
        // the reviewer's marks of the rule inside `runs`, as their file has them
        let marked = |runs: &[(u64, u64)]| -> Result<Vec<(u64, u64)>> {
            let mut out: Vec<(u64, u64)> = Vec::new();
            let mut buffer = vec![0u8; STEP as usize];
            for &(a, b) in runs {
                let mut at = a;
                while at < b {
                    check_cancelled(cancelled)?;
                    let len = (b - at).min(STEP);
                    let bytes = &mut buffer[..len as usize];
                    file.read_exact_at(bytes, head + first + at)?;
                    for (j, &s) in bytes.iter().enumerate() {
                        if s == crate::drc::desktop::STATUS_WAIVED {
                            let e = at + j as u64;
                            match out.last_mut() {
                                Some(last) if last.1 == e => last.1 = e + 1,
                                _ => out.push((e, e + 1)),
                            }
                        }
                    }
                    at += len;
                }
            }
            Ok(out)
        };
        let reasons = person.reasons.as_ref().and_then(Option::as_ref);
        let line = reasons.and_then(|r| r.parts(kind, check, &[(error, error + 1)]).first().map(|part| part.2));
        let (text, time, runs) = match (reasons, line) {
            (Some(reasons), Some(entry)) => {
                // the action's line: every run of it in the rule
                let mut of: Vec<(u64, u64)> = Vec::new();
                for (a, b, e) in reasons.runs(kind, check) {
                    if e == entry {
                        match of.last_mut() {
                            Some(last) if last.1 == a => last.1 = b,
                            _ => of.push((a, b)),
                        }
                    }
                }
                let said = &reasons.entries()[entry];
                (said.text.clone(), said.time, marked(&of)?)
            }
            _ => {
                // no line: the reviewer's marks of the rule that say no why
                let all = marked(&[(0, size)])?;
                let said: Vec<(u64, u64)> = match reasons {
                    Some(reasons) => {
                        let mut runs: Vec<(u64, u64)> = Vec::new();
                        for (a, b, _) in reasons.runs(kind, check) {
                            match runs.last_mut() {
                                Some(last) if last.1 == a => last.1 = b,
                                _ => runs.push((a, b)),
                            }
                        }
                        runs
                    }
                    None => Vec::new(),
                };
                (String::new(), 0, crate::drc::reasons::minus(&all, &said))
            }
        };
        Ok(Some(Group { stage, reviewer: person.tag.clone(), text, time, runs }))
    }
}
