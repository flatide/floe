//! Review statistics of every reviewer of one DRC result, without a viewer
//! (user 2026-11-10: a launcher - TeeBox - collects the statistics of the
//! people who review one layout's result; the viewer's CSVs needed a person
//! to load each peer by hand, and `floe2 drc --rules` gave one reviewer's
//! stored waived counters alone).
//!
//! The reviewers are found by their working files - the waive sidecar
//! `.<db>.waive.<id>` and the flags `.<db>.review.<id>.sqlite3` beside the
//! pack, and where this account's reviews go when the results folder cannot
//! be written (the temp folder's sidecar, FLOE_DRC_REVIEW_ROOT or the
//! user-data folder's flags). Each is counted from what its files hold: the
//! status bytes are read and counted again (the stored counters are only
//! compared - two sessions of one reviewer could leave them wrong before
//! 0.12.440), the flags from their bit planes. The team's numbers are the
//! reviewers window's (snapshot::aggregate, chunk by chunk): visits united,
//! a waive or an exclusion carried unless two reviewers who both visited the
//! error decided differently - that error is a conflict.
//!
//! A reviewer's reasons (`.<db>.reasons.<id>`, drc::reasons; 0.12.463) are
//! read beside the other two: each reason given with the errors that hold
//! it and are waived - or set aside - now, counted in the same pass from the
//! same status bytes and flags, so a reason never counts an error the
//! reviewer has since turned back by another road.
//!
//! Nothing is written, no snapshot is made, and the pack must be the
//! current one: a missing or stale pack is an error, never an answer of
//! zeros from the ASCII file.
//!
//! `counts` (0.12.464) is the same question asked for an answer at once
//! (user 2026-11-13: "이 통계는 실시간 요청으로 들어올 거라서 사용자가 waive
//! 할때마다 통계를 만들어두는게 좋을 것 같아"): the numbers a reviewer's
//! files KEEP - the waive sidecar's counter a rule, the flags' counters a
//! rule -, which every waive, visit and exclusion brings up to date as it
//! is written. No status byte and no bit plane is read, and the pack is
//! opened for its directory alone: the cost is the rules', not the errors'.
//! What it cannot say for that is what needs an error at a time - the
//! reasons' counts - and whether a stored counter is what its statuses count
//! (`collect` says `counters_match`). The team's union and conflicts need an
//! error at a time too, and are in the report all the same (0.12.465, user
//! 2026-11-13: ".db를 여러 사람이 검토하고 그 합집합을 실시간으로 받아야
//! 해"): kept in `.<db>.team` and brought up to date by what the reviewers'
//! change logs say since (`live`) - the one file this report writes, and
//! does without where the folder takes none.
use super::bits::{Bits, CHUNK};
use super::flags::{self, open_read_only, sql};
use super::Identity;
use crate::drc::confirm;
use crate::drc::desktop::STATUS_WAIVED;
use crate::drc::reasons::{Kind, Reasons};
use crate::drc::Pack;
use crate::{check_cancelled, Error, ErrorKind, Result};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

pub const TYPE: &str = "floe-drc-review-statistics";
pub const VERSION: u32 = 1;
/// The report of `counts`.
pub const COUNTS_TYPE: &str = "floe-drc-review-counts";

/// Which rules a `counts` report lists beside each reviewer's whole numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Listed<'a> {
    /// none: the whole result's numbers alone
    Whole,
    /// the rule of this name - each of them where a result has the name twice
    Named(&'a str),
    /// every rule
    All,
}
const READ_BUFFER: usize = 1 << 20;

/// A reviewer's working files as found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Found {
    /// the waive sidecar and where it is (`results`, `temp`)
    pub(super) waive: Option<(PathBuf, &'static str)>,
    /// the flags file and where it is (`results`, `review-root`, `user-data`)
    pub(super) review: Option<(PathBuf, &'static str)>,
    /// the reasons file and where it is (`results`, `temp`)
    pub(super) reasons: Option<(PathBuf, &'static str)>,
    /// the confirm file and where it is (`results`, `temp`)
    pub(super) confirm: Option<(PathBuf, &'static str)>,
    /// the rejects log and where it is (`results`, `temp`)
    pub(super) rejects: Option<(PathBuf, &'static str)>,
    /// the request file and where it is (`results`, `temp`): the errors
    /// whose waive this reviewer asked for (drc::confirm; 0.12.472)
    pub(super) request: Option<(PathBuf, &'static str)>,
    /// the change log, where one was listed (drc::changes): what finds the
    /// log of a reviewer who has neither a waive sidecar nor a request file
    /// to name it from - a confirmer who is no waiver (0.12.473). Alone it
    /// makes nobody a reviewer
    pub(super) changes: Option<PathBuf>,
}

// (which names are a reviewer's: the lock's reader asks the same -
// drc::confirm, which is there without the flags' store)
use crate::drc::local::reviewer_of;

/// The names in `folder` (missing: none), or the error when `must`.
fn names(folder: &Path, must: bool) -> Result<Vec<String>> {
    let entries = match std::fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(e) if !must || e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(Error::new(
                ErrorKind::Io,
                format!("{}: cannot list the reviewers' files: {e}", folder.display()),
            ))
        }
    };
    let mut out = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => {
                if let Some(name) = entry.file_name().to_str() {
                    out.push(name.to_owned());
                }
            }
            Err(e) if must => return Err(e.into()),
            Err(_) => (),
        }
    }
    Ok(out)
}

/// Every reviewer with a working file of `pack`'s result, by tag.
pub(super) fn discover(pack: &Path) -> Result<BTreeMap<String, Found>> {
    let database = crate::cache::database_path(pack)?;
    let name = database
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| Error::input("DRC filename must be UTF-8"))?
        .to_owned();
    let folder = database.parent().unwrap_or(Path::new(".")).to_path_buf();
    let mut found: BTreeMap<String, Found> = BTreeMap::new();
    let waive_prefix = format!(".{name}.waive.");
    let reasons_prefix = format!(".{name}.reasons.");
    let confirm_prefix = format!(".{name}.confirm.");
    let rejects_prefix = format!(".{name}.rejects.");
    let request_prefix = format!(".{name}.request.");
    let changes_prefix = format!(".{name}.changes.");
    let mut logs: BTreeMap<String, PathBuf> = BTreeMap::new();
    let review_prefix = format!(".{name}.review.");
    let review_tag = |file: &str| -> Option<String> {
        file.strip_prefix(&review_prefix)
            .and_then(|rest| rest.strip_suffix(".sqlite3"))
            .and_then(reviewer_of)
            .map(str::to_owned)
    };
    // beside the pack: the results folder, or its place under --workdir
    for file in names(&folder, true)? {
        if let Some(tag) = file.strip_prefix(&waive_prefix).and_then(reviewer_of) {
            found.entry(tag.to_owned()).or_default().waive = Some((folder.join(&file), "results"));
        } else if let Some(tag) = file.strip_prefix(&reasons_prefix).and_then(reviewer_of) {
            found.entry(tag.to_owned()).or_default().reasons = Some((folder.join(&file), "results"));
        } else if let Some(tag) = file.strip_prefix(&confirm_prefix).and_then(reviewer_of) {
            found.entry(tag.to_owned()).or_default().confirm = Some((folder.join(&file), "results"));
        } else if let Some(tag) = file.strip_prefix(&rejects_prefix).and_then(reviewer_of) {
            found.entry(tag.to_owned()).or_default().rejects = Some((folder.join(&file), "results"));
        } else if let Some(tag) = file.strip_prefix(&request_prefix).and_then(reviewer_of) {
            found.entry(tag.to_owned()).or_default().request = Some((folder.join(&file), "results"));
        } else if let Some(tag) = file.strip_prefix(&changes_prefix).and_then(reviewer_of) {
            logs.insert(tag.to_owned(), folder.join(&file));
        } else if let Some(tag) = review_tag(&file) {
            found.entry(tag).or_default().review = Some((folder.join(&file), "results"));
        }
    }
    // the flags' own root, when one is set: the primary place then
    let probe = flags::review_path(pack, "x")?;
    if let Some(root) = probe.parent().filter(|p| *p != folder) {
        for file in names(root, false)? {
            if let Some(tag) = review_tag(&file) {
                found.entry(tag).or_default().review = Some((root.join(&file), "review-root"));
            }
        }
    }
    // where this account's reviews go when the results folder refuses them:
    // the flags in the user-data folder (only without a primary file, as the
    // store itself reads them), the waive sidecar in the temp folder
    if let Ok(fallback) = flags::fallback_path(pack, "x") {
        if let Some(root) = fallback.parent() {
            for file in names(root, false)? {
                if let Some(tag) = review_tag(&file) {
                    let slot = &mut found.entry(tag).or_default().review;
                    if slot.is_none() {
                        *slot = Some((root.join(&file), "user-data"));
                    }
                }
            }
        }
    }
    let temp_name = crate::drc::waive_paths(pack, "x")?[1]
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_owned();
    // `.{name}.waive.x-{hash}`: the hash of this database's path
    if let Some(suffix) = temp_name.strip_prefix(&format!("{waive_prefix}x")) {
        let temp = std::env::temp_dir();
        if temp != folder && !suffix.is_empty() {
            for file in names(&temp, false)? {
                let tag = file
                    .strip_prefix(&waive_prefix)
                    .and_then(|rest| rest.strip_suffix(suffix))
                    .and_then(reviewer_of);
                if let Some(tag) = tag {
                    let slot = &mut found.entry(tag.to_owned()).or_default().waive;
                    if slot.is_none() {
                        *slot = Some((temp.join(&file), "temp"));
                    }
                }
                // (the reasons go where the waive sidecar goes)
                let tag = file
                    .strip_prefix(&reasons_prefix)
                    .and_then(|rest| rest.strip_suffix(suffix))
                    .and_then(reviewer_of);
                if let Some(tag) = tag {
                    let slot = &mut found.entry(tag.to_owned()).or_default().reasons;
                    if slot.is_none() {
                        *slot = Some((temp.join(&file), "temp"));
                    }
                }
                // (and the confirmations)
                let tag = file
                    .strip_prefix(&confirm_prefix)
                    .and_then(|rest| rest.strip_suffix(suffix))
                    .and_then(reviewer_of);
                if let Some(tag) = tag {
                    let slot = &mut found.entry(tag.to_owned()).or_default().confirm;
                    if slot.is_none() {
                        *slot = Some((temp.join(&file), "temp"));
                    }
                }
                // (and the rejections)
                let tag = file
                    .strip_prefix(&rejects_prefix)
                    .and_then(|rest| rest.strip_suffix(suffix))
                    .and_then(reviewer_of);
                if let Some(tag) = tag {
                    let slot = &mut found.entry(tag.to_owned()).or_default().rejects;
                    if slot.is_none() {
                        *slot = Some((temp.join(&file), "temp"));
                    }
                }
                // (and the requests)
                let tag = file
                    .strip_prefix(&request_prefix)
                    .and_then(|rest| rest.strip_suffix(suffix))
                    .and_then(reviewer_of);
                if let Some(tag) = tag {
                    let slot = &mut found.entry(tag.to_owned()).or_default().request;
                    if slot.is_none() {
                        *slot = Some((temp.join(&file), "temp"));
                    }
                }
                // (and the change log, where the results folder has none)
                let tag = file
                    .strip_prefix(&changes_prefix)
                    .and_then(|rest| rest.strip_suffix(suffix))
                    .and_then(reviewer_of);
                if let Some(tag) = tag {
                    logs.entry(tag.to_owned()).or_insert_with(|| temp.join(&file));
                }
            }
        }
    }
    // (a log is its reviewer's who has another file: a log left alone names
    // nobody)
    for (tag, log) in logs {
        if let Some(files) = found.get_mut(&tag) {
            files.changes = Some(log);
        }
    }
    Ok(found)
}

/// Add to `tally` - a count an action - the bits of `set` (a chunk's
/// waived or set-aside errors, the chunk starting at `start` with `count`
/// errors) that the reasons' runs `runs` cover. `cursor` walks the runs as
/// the chunks go by.
fn count_held(
    runs: &[(u64, u64, usize)],
    cursor: &mut usize,
    start: u64,
    count: u64,
    set: &Bits,
    tally: &mut [u64],
) {
    let end = start + count;
    while *cursor < runs.len() && runs[*cursor].1 <= start {
        *cursor += 1;
    }
    let mut j = *cursor;
    while j < runs.len() && runs[j].0 < end {
        let (a, b) = (runs[j].0.max(start) - start, runs[j].1.min(end) - start);
        tally[runs[j].2] += set.count_below(b) - set.count_below(a);
        if runs[j].1 > end {
            break;
        }
        j += 1;
    }
}

/// A reviewer's reasons as the report lists them: each reason given - by
/// kind and text - with the errors that hold it and are waived or set aside
/// now (`tally`, a count an action), the actions it was given in, when it
/// was first and last given, and its rules. A reason that holds no such
/// error is not listed. The waive reasons first, the most errors first.
fn reasons_json(store: &Reasons, tally: &[u64], names: &[String]) -> (Value, [u64; 3]) {
    struct Group {
        errors: u64,
        actions: u64,
        first: u64,
        last: u64,
        rules: BTreeMap<usize, u64>,
    }
    let mut groups: BTreeMap<(Kind, &str), Group> = BTreeMap::new();
    // (the errors that say why: waived, set aside, asked for)
    let mut with = [0u64; 3];
    for (entry, &errors) in store.entries().iter().zip(tally) {
        if errors == 0 {
            continue;
        }
        with[match entry.kind {
            Kind::NotRequired => 1,
            Kind::Request => 2,
            _ => 0,
        }] += errors;
        let g = groups.entry((entry.kind, entry.text.as_str())).or_insert(Group {
            errors: 0,
            actions: 0,
            first: entry.time,
            last: entry.time,
            rules: BTreeMap::new(),
        });
        g.errors += errors;
        g.actions += 1;
        g.first = g.first.min(entry.time);
        g.last = g.last.max(entry.time);
        *g.rules.entry(entry.check).or_default() += errors;
    }
    let mut listed: Vec<_> = groups.into_iter().collect();
    listed.sort_by(|a, b| (a.0 .0, std::cmp::Reverse(a.1.errors), a.0 .1).cmp(&(b.0 .0, std::cmp::Reverse(b.1.errors), b.0 .1)));
    let rows = listed
        .into_iter()
        .map(|((kind, text), g)| {
            json!({
                "kind": kind.name(),
                "text": text,
                "errors": g.errors,
                "actions": g.actions,
                "first": utc(g.first),
                "last": utc(g.last),
                "rules": g.rules.iter().map(|(&ci, &errors)| json!({"index": ci + 1, "name": names[ci], "errors": errors})).collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();
    (json!(rows), with)
}

/// When a file was last changed, UTC (`2026-11-10T07:12:45Z`).
fn modified(path: &Path) -> Option<String> {
    let at = std::fs::metadata(path).ok()?.modified().ok()?;
    let secs = at.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    Some(utc(secs))
}

/// Seconds since the epoch as UTC, ISO 8601 (days to civil: H. Hinnant).
pub(crate) fn utc(secs: u64) -> String {
    let (days, rem) = ((secs / 86_400) as i64, secs % 86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// The status bytes equal to waived, as bits (at most a chunk).
pub(super) fn waived_bits(statuses: &[u8]) -> Bits {
    let mut bits = Bits::ZERO;
    for (word, group) in statuses.chunks(64).enumerate() {
        if group.iter().all(|&b| b == 0) {
            continue;
        }
        let mut w = 0u64;
        for (i, &b) in group.iter().enumerate() {
            if b == STATUS_WAIVED {
                w |= 1 << i;
            }
        }
        bits.0[word] = w;
    }
    bits
}

/// One reviewer's counts of a rule.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Own {
    waived: u64,
    visited: u64,
    not_required: u64,
    both: u64,
    /// the errors this reviewer has confirmed (their confirm file's)
    confirmed: u64,
    /// of this reviewer's waives, the ones a confirmer has confirmed
    /// (None: not counted - the kept counters cannot say it)
    waived_confirmed: Option<u64>,
    /// the errors whose waive this reviewer has rejected and neither
    /// confirmed nor taken back since (their rejects log's)
    rejected: u64,
    /// of this reviewer's waives, the ones that are rejected: under a
    /// confirmer's rejection this reviewer has not answered by waiving
    /// again (None: not counted)
    waived_rejected: Option<u64>,
    /// the errors whose waive this reviewer asked for (their request
    /// file's; 0.12.472)
    requested: u64,
    /// of this reviewer's requests, the ones that are declined: under a
    /// waiver's decline this reviewer has not answered by asking again
    /// (None: not counted; 0.12.473)
    requested_declined: Option<u64>,
    /// the errors whose request this reviewer has declined and not taken
    /// back since (their rejects log's, a waiver's)
    declined: u64,
}
impl Own {
    fn add(&mut self, o: &Own) {
        self.requested += o.requested;
        self.declined += o.declined;
        self.waived += o.waived;
        self.visited += o.visited;
        self.not_required += o.not_required;
        self.both += o.both;
        self.confirmed += o.confirmed;
        self.rejected += o.rejected;
        let sum = |a: Option<u64>, b: Option<u64>| match (a, b) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(0) + b.unwrap_or(0)),
        };
        self.waived_confirmed = sum(self.waived_confirmed, o.waived_confirmed);
        self.waived_rejected = sum(self.waived_rejected, o.waived_rejected);
        self.requested_declined = sum(self.requested_declined, o.requested_declined);
    }
    fn any(&self) -> bool {
        self.waived + self.visited + self.not_required + self.confirmed + self.rejected + self.requested + self.declined
            > 0
    }
    /// `files`: which of the reviewer's files there are - the waive
    /// sidecar, the flags, the confirm file, the rejects log, the request
    /// file.
    fn json(&self, total: u64, files: [bool; 5]) -> Value {
        let [waive, flags, confirm, rejects, request] = files;
        let mut v = json!({});
        if request {
            v["requested"] = json!(self.requested);
            if let Some(declined) = self.requested_declined {
                v["requested_declined"] = json!(declined);
            }
        }
        // (a waiver's: the requests they declined - said where there are)
        if self.declined > 0 {
            v["declined"] = json!(self.declined);
        }
        if waive {
            v["waived"] = json!(self.waived);
            if let Some(confirmed) = self.waived_confirmed {
                v["waived_confirmed"] = json!(confirmed);
            }
            if let Some(rejected) = self.waived_rejected {
                v["waived_rejected"] = json!(rejected);
            }
        }
        if confirm {
            v["confirmed"] = json!(self.confirmed);
        }
        if rejects {
            v["rejected"] = json!(self.rejected);
        }
        if flags {
            v["visited"] = json!(self.visited);
            v["not_required"] = json!(self.not_required);
            v["visited_not_required"] = json!(self.both);
            // neither visited nor excluded: the viewer's "unvisited targets"
            v["pending"] = json!(total - self.visited - self.not_required + self.both);
        }
        v
    }
}

/// The team's counts of a rule (snapshot::Counts' numbers).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Team {
    pub visited: u64,
    pub waived: u64,
    pub not_required: u64,
    pub conflicts: u64,
    pub done: u64,
    /// the errors somebody has waived - conflicts too: what a confirmer has
    /// to look at (0.12.466)
    pub waived_any: u64,
    /// of those, the ones a confirmer has confirmed
    pub confirmed: u64,
    /// the errors whose every waive is rejected (0.12.468; 0.12.469: a
    /// waive is rejected until its reviewer waives the error again): they
    /// are waived by nobody above, and wait for their reviewers
    pub rejected: u64,
    /// the errors somebody has set aside from the review - conflicts too:
    /// the union, as `waived_any` is of the waives (0.12.470, what the
    /// brief report says)
    pub not_required_any: u64,
    /// the errors somebody asked the waive of (0.12.472: "error -> waive
    /// request -> waive -> confirm") - waived since or not. A request a
    /// waiver declined is none until its requester asks again (0.12.473)
    pub requested: u64,
    /// of those, the ones nobody has waived: they wait for a waiver (a
    /// waive that is rejected is one still - it waits for its reviewer)
    pub unwaived: u64,
    /// the errors whose every request is declined and nobody has waived
    /// (0.12.473; user 2026-11-14: "요청 거절 상태도 만들어줘"): they are
    /// asked for by nobody above, and wait for their requesters
    pub declined: u64,
}
impl Team {
    pub(super) fn add(&mut self, o: &Team) {
        self.requested += o.requested;
        self.unwaived += o.unwaived;
        self.declined += o.declined;
        self.visited += o.visited;
        self.waived += o.waived;
        self.not_required += o.not_required;
        self.conflicts += o.conflicts;
        self.done += o.done;
        self.waived_any += o.waived_any;
        self.confirmed += o.confirmed;
        self.rejected += o.rejected;
        self.not_required_any += o.not_required_any;
    }
    pub(super) fn json(&self, total: u64) -> Value {
        let mut v = json!({
            "errors": total,
            "visited": self.visited,
            "waived": self.waived,
            "not_required": self.not_required,
            "conflicts": self.conflicts,
            "pending": total.saturating_sub(self.done),
            // the waives a confirmer has confirmed, and the ones - anybody's,
            // a conflict's too - that still wait for it
            "confirmed": self.confirmed,
            "unconfirmed": self.waived_any.saturating_sub(self.confirmed),
            // the errors whose waives are all rejected: none of the numbers
            // above counts a rejected waive
            "rejected": self.rejected,
        });
        // the errors somebody asked the waive of, the ones of them nobody
        // has waived yet, and the ones whose requests are all declined:
        // said where there is a request to say (0.12.472, 0.12.473)
        if self.requested > 0 || self.declined > 0 {
            v["requested"] = json!(self.requested);
            v["unwaived"] = json!(self.unwaived);
            v["declined"] = json!(self.declined);
        }
        v
    }
}

/// One reviewer's planes of a chunk as the team counts them.
#[derive(Clone, Copy)]
pub(super) struct Planes {
    pub(super) visited: Bits,
    pub(super) waived: Bits,
    pub(super) not_required: Bits,
    /// the errors whose waive this reviewer confirmed
    pub(super) confirmed: Bits,
    /// the errors under a rejection this reviewer has not answered: a
    /// waive of theirs of one of them is rejected (`confirm::unanswered`)
    pub(super) rejected: Bits,
    /// the errors whose waive this reviewer asked for
    pub(super) requested: Bits,
    /// the errors under a decline this reviewer has not answered: a
    /// request of theirs of one of them is declined
    pub(super) declined: Bits,
    pub(super) visited_known: bool,
    pub(super) waive_known: bool,
    pub(super) not_required_known: bool,
}

/// The errors of a chunk - `count` errors from the rule's `start` - that
/// `runs` (a rule's, sorted and disjoint) cover.
pub(super) fn run_bits(runs: &[(u64, u64)], start: u64, count: u64) -> Bits {
    let end = start + count;
    let mut bits = Bits::ZERO;
    let mut i = runs.partition_point(|r| r.1 <= start);
    while let Some(&(a, b)) = runs.get(i).filter(|r| r.0 < end) {
        bits |= Bits::first(b.min(end) - start) & !Bits::first(a.max(start) - start);
        i += 1;
    }
    bits
}

/// The team's counts of one chunk from its reviewers' planes
/// (snapshot::aggregate's rule). A rejected waive is no waive: it is taken
/// out of its reviewer's before anything is counted - and an error whose
/// waives are all rejected is counted as that. A declined request is no
/// request in the same way, and an error whose requests are all declined is
/// counted as that while no waive sidecar has it.
pub(super) fn team_of(planes: &[Planes]) -> Team {
    let (mut v, mut w, mut n, mut c) = (Bits::ZERO, Bits::ZERO, Bits::ZERO, Bits::ZERO);
    let (mut wy, mut wn, mut ny, mut nn) = (Bits::ZERO, Bits::ZERO, Bits::ZERO, Bits::ZERO);
    let mut rejected = Bits::ZERO;
    let (mut asked, mut held, mut declined) = (Bits::ZERO, Bits::ZERO, Bits::ZERO);
    for p in planes {
        let waived = p.waived & !p.rejected;
        rejected |= p.waived & p.rejected;
        asked |= p.requested & !p.declined;
        declined |= p.requested & p.declined;
        held |= p.waived;
        v |= p.visited;
        w |= waived;
        n |= p.not_required;
        c |= p.confirmed;
        if p.visited_known {
            if p.waive_known {
                wy |= p.visited & waived;
                wn |= p.visited & !waived;
            }
            if p.not_required_known {
                ny |= p.visited & p.not_required;
                nn |= p.visited & !p.not_required;
            }
        }
    }
    let conflict = (wy & wn) | (ny & nn);
    let not_required_any = n.count();
    let n = n & !conflict;
    Team {
        visited: v.count(),
        waived: (w & !conflict).count(),
        not_required: n.count(),
        conflicts: conflict.count(),
        done: (v | n).count(),
        // a confirmation counts while somebody has the error waived
        waived_any: w.count(),
        confirmed: (w & c).count(),
        rejected: (rejected & !w).count(),
        not_required_any,
        requested: asked.count(),
        unwaived: (asked & !held).count(),
        declined: (declined & !asked & !held).count(),
    }
}

/// The team's counts in brief: a rule's, or the whole result's (user
/// 2026-11-14: "팀 작업 통계 cli 명령의 결과를 다음과 같이 간략하게 받고
/// 싶음. 빈번히 조회되는 것이어서 데이터량을 줄이고 ProperTee에서 별도의
/// 처리없이 넘기려는 의도임" - `{ "total": {errors, waived, confirmed,
/// rejected, visited, ignored}, "rule": [ "GRX.1": {...}, ... ] }`). Every
/// number is a union - an error somebody waived, visited, set aside - so
/// `confirmed` is never more than `waived`, and a judgement two visitors
/// differ on counts on the side of the one who made it (the full reports
/// say `conflicts`; this one has no word for them).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Brief {
    pub errors: u64,
    /// the errors somebody asked the waive of - waived since or not
    /// (0.12.472: the workflow's first step); a declined request is none
    pub requested: u64,
    /// the errors whose requests are all declined and nobody has waived
    /// (0.12.473; none of `requested`, as `rejected` is none of `waived`)
    pub declined: u64,
    /// waived by somebody - a waive that is not rejected, confirmed or not
    pub waived: u64,
    /// of those, confirmed by a confirmer
    pub confirmed: u64,
    /// the errors whose waives are all rejected (none of `waived`)
    pub rejected: u64,
    pub visited: u64,
    /// set aside from the review by somebody ("not for review")
    pub ignored: u64,
}

impl Brief {
    fn of(team: &Team, errors: u64) -> Self {
        Self {
            errors,
            requested: team.requested,
            declined: team.declined,
            waived: team.waived_any,
            confirmed: team.confirmed,
            rejected: team.rejected,
            visited: team.visited,
            ignored: team.not_required_any,
        }
    }
    fn add(&mut self, o: &Brief) {
        self.errors += o.errors;
        self.requested += o.requested;
        self.declined += o.declined;
        self.waived += o.waived;
        self.confirmed += o.confirmed;
        self.rejected += o.rejected;
        self.visited += o.visited;
        self.ignored += o.ignored;
    }
    fn write(&self, out: &mut String) {
        use std::fmt::Write;
        let _ = write!(
            out,
            "{{\"errors\":{},\"requested\":{},\"declined\":{},\"waived\":{},\"confirmed\":{},\"rejected\":{},\"visited\":{},\"ignored\":{}}}",
            self.errors,
            self.requested,
            self.declined,
            self.waived,
            self.confirmed,
            self.rejected,
            self.visited,
            self.ignored
        );
    }
}

/// What every brief report starts with - `total` first, and `errors` first
/// in it. Who keeps the report in a file knows an earlier one by it
/// (`floe2 drc --team-counts --out`).
pub const BRIEF_HEAD: &str = "{\"total\":{\"errors\":";

/// Which rules the brief report lists beside the total.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BriefRules<'a> {
    /// the rules that have errors: a rule without one has nothing to count
    /// (a field result of 17,000 rules has 1,500 of those)
    WithErrors,
    /// the rule of this name alone, with errors or without
    Named(&'a str),
    /// every rule, the ones without errors too
    All,
}

/// The brief report's text: `{"total":{…},"rule":{"NAME":{…},…}}` on one
/// line, no blank in it but a name's own - the keys in this order, the
/// rules in the result's. `total` is the whole result's; `rule` has the
/// rules `listed`. A name the result has twice is one entry, the sum of its
/// rules. `names`, `sizes` and `teams` are the rules' in the result's order.
pub fn brief_text(names: &[&str], sizes: &[u64], teams: &[Team], listed: BriefRules) -> Result<String> {
    let mut total = Brief::default();
    let mut rules: Vec<(&str, Brief)> = Vec::new();
    let mut at: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for ((name, &size), team) in names.iter().zip(sizes).zip(teams) {
        let own = Brief::of(team, size);
        total.add(&own);
        match listed {
            BriefRules::Named(only) if only != *name => continue,
            BriefRules::WithErrors if size == 0 => continue,
            _ => (),
        }
        match at.get(name) {
            Some(&i) => rules[i].1.add(&own),
            None => {
                at.insert(name, rules.len());
                rules.push((name, own));
            }
        }
    }
    if let (BriefRules::Named(only), true) = (listed, rules.is_empty()) {
        return Err(Error::input(format!("no such rule {only:?} (see --rules)")));
    }
    let mut out = String::with_capacity(96 + rules.len() * 96);
    out.push_str("{\"total\":");
    total.write(&mut out);
    out.push_str(",\"rule\":{");
    for (i, (name, own)) in rules.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&serde_json::to_string(name).map_err(|e| Error::new(ErrorKind::Io, e.to_string()))?);
        out.push(':');
        own.write(&mut out);
    }
    out.push_str("}}");
    Ok(out)
}

/// `floe2 drc DB --team-counts [--rule NAME | --all-rules]`: the team's
/// kept counts (`live::merge`, as `counts` brings them up to date) in brief
/// (`brief_text`), and what there is to say beside them - why the counts
/// do not cover every reviewer's files yet, which files were left out - for
/// the caller's stderr: the answer itself is the one line, to be handed on
/// as it is. No reviewer's own counters are read for it.
pub fn brief(source: &Path, listed: BriefRules, cancelled: &AtomicUsize) -> Result<(String, Vec<String>)> {
    let pack = crate::drc::open_pack_directory(source, cancelled)?;
    let id = Identity::of_pack(&pack, "statistics")?;
    let found = discover(&pack.path)?;
    let kept = super::live::merge_found(&pack.path, &id, &found, cancelled)?;
    let names: Vec<&str> = pack.checks.iter().map(|c| c.name.as_str()).collect();
    let text = brief_text(&names, &id.sizes, &kept.rules, listed)?;
    pack.unchanged()?;
    let mut said = kept.waiting.clone();
    said.extend(
        kept.skipped
            .iter()
            .map(|(reviewer, file, why)| format!("{reviewer}'s {} is left out: {why}", file.display())),
    );
    Ok((text, said))
}

/// A reviewer being counted: where the files are and the counts so far
/// (the open files are kept apart, in the same order).
struct Reviewer {
    tag: String,
    waive: Option<(PathBuf, &'static str)>,
    flags: Option<(PathBuf, &'static str)>,
    confirm: Option<(PathBuf, &'static str)>,
    /// the rejects log as read: the rejections this reviewer made, and
    /// the ones of their own waives they answered
    rejects: Option<((PathBuf, &'static str), Reasons)>,
    /// the request file
    request: Option<(PathBuf, &'static str)>,
    rules: Vec<Own>,
    stored: Option<Vec<u32>>,
    /// the reasons file, what it says, and the errors each of its actions
    /// holds that are waived or set aside now
    reasons: Option<((PathBuf, &'static str), Reasons, Vec<u64>)>,
}

/// The statistics of `source`'s current pack (see the module docs): every
/// reviewer found, or `only` these (an id with no file is listed as absent).
pub fn collect(source: &Path, only: Option<&[String]>, cancelled: &AtomicUsize) -> Result<Value> {
    let pack = crate::drc::open_pack(source, cancelled)?;
    let id = Identity::of_pack(&pack, "statistics")?;
    let found = discover(&pack.path)?;
    let report = collect_from(&pack, &id, found, only, cancelled)?;
    pack.unchanged()?;
    Ok(report)
}

/// The counts of `source`'s current pack as the reviewers' files keep them
/// (see the module docs): every reviewer found, or `only` these, each with
/// the whole result's errors, waived, visited and not for review - and the
/// rules `listed` with the same four; and `team`, the union of everyone's
/// review as `collect` counts it (`live::merge`), of every reviewer whoever
/// `only` names. The pack is asked for as `collect` asks (none, or one of
/// an older .db, is an error), opened for its directory alone and without a
/// reader's registration.
pub fn counts(
    source: &Path,
    only: Option<&[String]>,
    listed: Listed,
    cancelled: &AtomicUsize,
) -> Result<Value> {
    let pack = crate::drc::open_pack_directory(source, cancelled)?;
    let id = Identity::of_pack(&pack, "statistics")?;
    let found = discover(&pack.path)?;
    // the team: every reviewer's review united, from the counts kept of it
    // and brought up to date by what the reviewers' change logs say since
    // (drc::team::live; 0.12.465) - of everyone, whoever was asked for
    let kept = super::live::merge_found(&pack.path, &id, &found, cancelled)?;
    let mut report = counts_from(&pack, &id, found, only, listed, cancelled)?;
    report["team"] = team_json(&pack, &id, &kept, listed);
    pack.unchanged()?;
    Ok(report)
}

/// The kept team counts as the report says them: `collect`'s `team` keys -
/// `errors`, `visited`, `waived`, `not_required`, `conflicts`, `pending`,
/// `reviewers`, and `rules` for the rules listed - with `current` (they
/// cover every reviewer's files as they stand; `waiting` says why not),
/// `kept` (they are in the file), `counted` (the chunks counted again for
/// this answer) and `skipped` (files left out, as `collect` leaves them).
fn team_json(pack: &Pack, id: &Identity, kept: &super::live::Kept, listed: Listed) -> Value {
    let mut whole = Team::default();
    for rule in &kept.rules {
        whole.add(rule);
    }
    let mut team = whole.json(id.total());
    team["reviewers"] = json!(kept.reviewers);
    team["current"] = json!(kept.current);
    team["kept"] = json!(kept.stored);
    team["counted"] = json!(kept.counted);
    if !kept.waiting.is_empty() {
        team["waiting"] = json!(kept.waiting);
    }
    if !kept.skipped.is_empty() {
        team["skipped"] = kept
            .skipped
            .iter()
            .map(|(reviewer, file, reason)| json!({"reviewer": reviewer, "file": file, "reason": reason}))
            .collect();
    }
    let rows: Vec<usize> = match listed {
        Listed::Whole => return team,
        Listed::All => (0..pack.checks.len()).collect(),
        Listed::Named(name) => (0..pack.checks.len())
            .filter(|&ci| pack.checks[ci].name == name)
            .collect(),
    };
    team["rules"] = rows
        .into_iter()
        .map(|ci| {
            let mut row = kept.rules[ci].json(id.sizes[ci]);
            row["index"] = json!(ci + 1);
            row["name"] = json!(pack.checks[ci].name);
            row
        })
        .collect();
    team
}

/// The flags' counters a rule, as the file keeps them: (visited, not for
/// review, both) by rule, zeros for a rule without a row. Counters that
/// cannot be a rule's are an error.
pub(super) fn kept_flags(conn: &Connection, sizes: &[u64]) -> Result<Vec<(u64, u64, u64)>> {
    let mut kept = vec![(0u64, 0u64, 0u64); sizes.len()];
    let mut statement = conn
        .prepare("SELECT ci,visited,nr,both FROM rules")
        .map_err(sql)?;
    let mut rows = statement.query([]).map_err(sql)?;
    while let Some(row) = rows.next().map_err(sql)? {
        let ci: i64 = row.get(0).map_err(sql)?;
        let stored: (i64, i64, i64) = (
            row.get(1).map_err(sql)?,
            row.get(2).map_err(sql)?,
            row.get(3).map_err(sql)?,
        );
        let Some(size) = usize::try_from(ci).ok().and_then(|ci| sizes.get(ci)) else {
            return Err(Error::new(ErrorKind::Cache, "corrupt review flag counters"));
        };
        let c = flags::counts(*size, stored)?;
        kept[ci as usize] = (c.visited, c.not_required, c.visited_not_required);
    }
    Ok(kept)
}

fn counts_from(
    pack: &Pack,
    id: &Identity,
    mut found: BTreeMap<String, Found>,
    only: Option<&[String]>,
    listed: Listed,
    cancelled: &AtomicUsize,
) -> Result<Value> {
    let total = id.total();
    let names: Vec<&str> = pack.checks.iter().map(|c| c.name.as_str()).collect();
    let rows: Vec<usize> = match listed {
        Listed::Whole => Vec::new(),
        Listed::All => (0..names.len()).collect(),
        Listed::Named(name) => {
            let hits: Vec<usize> = (0..names.len()).filter(|&ci| names[ci] == name).collect();
            if hits.is_empty() {
                return Err(Error::input(format!("no such rule {name:?} (see --rules)")));
            }
            hits
        }
    };
    let mut absent = Vec::new();
    if let Some(only) = only {
        let wanted: Vec<String> = only.iter().map(|r| flags::sanitize(r.trim())).collect();
        found.retain(|tag, _| wanted.contains(tag));
        for tag in wanted {
            if !found.contains_key(&tag) && !absent.contains(&tag) {
                absent.push(tag);
            }
        }
    }
    let mut skipped = Vec::new();
    let mut people = Vec::new();
    for (tag, files) in found {
        check_cancelled(cancelled)?;
        // waived: the sidecar's counter a rule (no status byte is read)
        let mut waive = None;
        if let Some((path, place)) = files.waive {
            let kept = open_waive(&path, id).and_then(|(_, stored)| {
                if stored.iter().zip(&id.sizes).any(|(&w, &size)| u64::from(w) > size) {
                    return Err(Error::new(
                        ErrorKind::Cache,
                        "waive counters past their rules' errors (floe2 drc --review-stats counts the statuses)",
                    ));
                }
                Ok(stored)
            });
            match kept {
                Ok(stored) => waive = Some((path, place, stored)),
                Err(e) => skipped.push(json!({"reviewer": tag, "file": path, "reason": e.message})),
            }
        }
        // visited, not for review: the flags' counters a rule (no bit plane)
        let mut marks = None;
        if let Some((path, place)) = files.review {
            match open_flags(&path, id).and_then(|conn| kept_flags(&conn, &id.sizes)) {
                Ok(kept) => marks = Some((path, place, kept)),
                Err(e) => skipped.push(json!({"reviewer": tag, "file": path, "reason": e.message})),
            }
        }
        // confirmed: the confirm file's counter a rule
        let mut confirms = None;
        if let Some((path, place)) = files.confirm {
            match open_confirm(&path, id) {
                Ok((_, kept)) => confirms = Some((path, place, kept)),
                Err(e) => skipped.push(json!({"reviewer": tag, "file": path, "reason": e.message})),
            }
        }
        // rejected: the rejects log's runs a rule (a log of actions: read
        // whole, it is a line an action)
        let mut rejects = None;
        // (and the requests they declined, a waiver's: the same log's)
        let mut declines = Vec::new();
        if let Some((path, place)) = files.rejects {
            match rejects_log(&path, id) {
                Ok(Some(log)) => {
                    declines = held_of(&log, Kind::Decline, names.len());
                    rejects = Some((path, place, rejected_of(&log, names.len())));
                }
                Ok(None) => (),
                Err(e) => skipped.push(json!({"reviewer": tag, "file": path, "reason": e.message})),
            }
        }
        // requested: the request file's counter a rule
        let mut requests = None;
        if let Some((path, place)) = files.request {
            match open_request(&path, id) {
                Ok((_, kept)) => requests = Some((path, place, kept)),
                Err(e) => skipped.push(json!({"reviewer": tag, "file": path, "reason": e.message})),
            }
        }
        if waive.is_none() && marks.is_none() && confirms.is_none() && rejects.is_none() && requests.is_none() {
            continue;
        }
        let own = |ci: usize| {
            let (visited, not_required, both) = marks.as_ref().map_or((0, 0, 0), |m| m.2[ci]);
            Own {
                requested: requests.as_ref().map_or(0, |r| u64::from(r.2[ci])),
                waived: waive.as_ref().map_or(0, |w| u64::from(w.2[ci])),
                visited,
                not_required,
                both,
                confirmed: confirms.as_ref().map_or(0, |c| u64::from(c.2[ci])),
                waived_confirmed: None,
                rejected: rejects.as_ref().map_or(0, |r| r.2[ci].iter().map(|run| run.1 - run.0).sum()),
                waived_rejected: None,
                requested_declined: None,
                declined: declines.get(ci).map_or(0, |runs| runs.iter().map(|run| run.1 - run.0).sum()),
            }
        };
        let (waived, marked, confirming) = (waive.is_some(), marks.is_some(), confirms.is_some());
        let files = [waived, marked, confirming, rejects.is_some(), requests.is_some()];
        let mut all = Own::default();
        for ci in 0..names.len() {
            all.add(&own(ci));
        }
        let mut person = all.json(total, files);
        person["errors"] = json!(total);
        person["id"] = json!(tag);
        person["present"] = json!(true);
        person["waive_known"] = json!(waived);
        person["visited_known"] = json!(marked);
        person["not_required_known"] = json!(marked);
        if let Some((path, place, _)) = &confirms {
            person["confirm_file"] = json!(path);
            person["confirm_location"] = json!(place);
            person["confirm_modified"] = json!(modified(path));
        }
        if let Some((path, place, _)) = &rejects {
            person["rejects_file"] = json!(path);
            person["rejects_location"] = json!(place);
            person["rejects_modified"] = json!(modified(path));
        }
        if let Some((path, place, _)) = &requests {
            person["request_file"] = json!(path);
            person["request_location"] = json!(place);
            person["request_modified"] = json!(modified(path));
        }
        if let Some((path, place, _)) = &waive {
            person["waive_file"] = json!(path);
            person["waive_location"] = json!(place);
            person["waive_modified"] = json!(modified(path));
        }
        if let Some((path, place, _)) = &marks {
            person["review_file"] = json!(path);
            person["review_location"] = json!(place);
            person["review_modified"] = json!(modified(path));
        }
        if listed != Listed::Whole {
            // every rule listed has its row, a rule nobody touched too
            person["rules"] = rows
                .iter()
                .map(|&ci| {
                    let mut row = own(ci).json(id.sizes[ci], files);
                    row["index"] = json!(ci + 1);
                    row["name"] = json!(names[ci]);
                    row["errors"] = json!(id.sizes[ci]);
                    row
                })
                .collect();
        }
        people.push(person);
    }
    for tag in absent {
        people.push(json!({"id": tag, "present": false}));
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut report = json!({
        "type": COUNTS_TYPE,
        "version": VERSION,
        "generated": utc(now),
        "database": crate::cache::database_path(&pack.path)?,
        "pack": pack.path,
        "fingerprint": id.fingerprint,
        "cell": pack.cell,
        "errors": total,
        "rule_count": names.len(),
        "reviewers": people,
        "skipped": skipped,
    });
    if listed != Listed::Whole {
        // the rules asked about, with their errors - also when nobody has
        // reviewed yet
        report["rules"] = rows
            .iter()
            .map(|&ci| json!({"index": ci + 1, "name": names[ci], "errors": id.sizes[ci]}))
            .collect();
    }
    Ok(report)
}

pub(super) fn collect_from(
    pack: &Pack,
    id: &Identity,
    found: BTreeMap<String, Found>,
    only: Option<&[String]>,
    cancelled: &AtomicUsize,
) -> Result<Value> {
    Ok(collect_with(pack, id, found, only, cancelled)?.0)
}

/// `collect_from`, with the team's counts a rule as they were counted (what
/// the kept counts are held against, number for number).
pub(super) fn collect_with(
    pack: &Pack,
    id: &Identity,
    mut found: BTreeMap<String, Found>,
    only: Option<&[String]>,
    cancelled: &AtomicUsize,
) -> Result<(Value, Vec<Team>)> {
    let total = id.total();
    let rules = id.sizes.len();
    let mut absent = Vec::new();
    if let Some(only) = only {
        let wanted: Vec<String> = only.iter().map(|r| flags::sanitize(r.trim())).collect();
        found.retain(|tag, _| wanted.contains(tag));
        for tag in wanted {
            if !found.contains_key(&tag) && !absent.contains(&tag) {
                absent.push(tag);
            }
        }
    }
    let mut skipped = Vec::new();
    let mut skip = |reviewer: &str, file: &Path, reason: String| {
        skipped.push(json!({"reviewer": reviewer, "file": file, "reason": reason}));
    };
    let mut reviewers = Vec::new();
    let mut statuses: Vec<Option<BufReader<File>>> = Vec::new();
    let mut confirmations: Vec<Option<BufReader<File>>> = Vec::new();
    let mut requests: Vec<Option<BufReader<File>>> = Vec::new();
    let mut connections: Vec<Option<Connection>> = Vec::new();
    for (tag, files) in found {
        check_cancelled(cancelled)?;
        let mut reviewer = Reviewer {
            tag: tag.clone(),
            waive: None,
            flags: None,
            confirm: None,
            rejects: None,
            request: None,
            rules: vec![Own::default(); rules],
            stored: None,
            reasons: None,
        };
        let (mut reader, mut connection) = (None, None);
        if let Some((path, place)) = files.waive {
            match open_waive(&path, id) {
                Ok((file, stored)) => {
                    reviewer.stored = Some(stored);
                    reviewer.waive = Some((path, place));
                    reader = Some(BufReader::with_capacity(READ_BUFFER, file));
                }
                Err(e) => skip(&tag, &path, e.message),
            }
        }
        if let Some((path, place)) = files.review {
            match open_flags(&path, id) {
                Ok(conn) => {
                    reviewer.flags = Some((path, place));
                    connection = Some(conn);
                }
                Err(e) => skip(&tag, &path, e.message),
            }
        }
        // why: beside the files that say whether. One of another result is
        // left out and said, as theirs are; one that reads is counted
        // against them - alone it has nothing to be counted against
        if let Some((path, place)) = files.reasons {
            let result = crate::drc::review::Fingerprint::of(pack).tag();
            match Reasons::load(&path, &result, &id.sizes) {
                Ok(Some(store)) => {
                    let tally = vec![0u64; store.entries().len()];
                    reviewer.reasons = Some(((path, place), store, tally));
                }
                Ok(None) => (),
                Err(e) => skip(&tag, &path, e.message),
            }
        }
        // the waives this reviewer confirmed: a byte an error, as the
        // statuses are (drc::confirm)
        let mut confirming = None;
        if let Some((path, place)) = files.confirm {
            let opened = open_confirm(&path, id).and_then(|(mut file, _)| {
                file.seek(SeekFrom::Start(crate::drc::confirm::HEADER))?;
                Ok(file)
            });
            match opened {
                Ok(file) => {
                    reviewer.confirm = Some((path, place));
                    confirming = Some(BufReader::with_capacity(READ_BUFFER, file));
                }
                Err(e) => skip(&tag, &path, e.message),
            }
        }
        // the waives this reviewer rejected, and the rejections of their
        // own they answered: their rejects log (drc::reasons)
        if let Some((path, place)) = files.rejects {
            match rejects_log(&path, id) {
                Ok(Some(log)) => reviewer.rejects = Some(((path, place), log)),
                Ok(None) => (),
                Err(e) => skip(&tag, &path, e.message),
            }
        }
        // the errors whose waive this reviewer asked for: a byte an error
        // too (drc::confirm)
        let mut asking = None;
        if let Some((path, place)) = files.request {
            let opened = open_request(&path, id).and_then(|(mut file, _)| {
                file.seek(SeekFrom::Start(crate::drc::confirm::HEADER))?;
                Ok(file)
            });
            match opened {
                Ok(file) => {
                    reviewer.request = Some((path, place));
                    asking = Some(BufReader::with_capacity(READ_BUFFER, file));
                }
                Err(e) => skip(&tag, &path, e.message),
            }
        }
        if reader.is_some()
            || connection.is_some()
            || confirming.is_some()
            || reviewer.rejects.is_some()
            || asking.is_some()
        {
            reviewers.push(reviewer);
            statuses.push(reader);
            confirmations.push(confirming);
            requests.push(asking);
            connections.push(connection);
        }
    }
    // rule by rule, chunk by chunk: each reviewer's planes, then the team's
    let mut team = vec![Team::default(); rules];
    let mut buffer = vec![0u8; CHUNK as usize];
    for (ci, &size) in id.sizes.iter().enumerate() {
        let mut statements = Vec::with_capacity(reviewers.len());
        for conn in &connections {
            statements.push(match conn {
                Some(conn) => Some(
                    conn.prepare("SELECT chunk,visited,nr FROM chunks WHERE ci=? ORDER BY chunk")
                        .map_err(sql)?,
                ),
                None => None,
            });
        }
        let mut rows = Vec::with_capacity(statements.len());
        for st in statements.iter_mut() {
            rows.push(match st {
                Some(st) => Some(st.query([ci as i64]).map_err(sql)?),
                None => None,
            });
        }
        let mut heads: Vec<Option<(u64, Bits, Bits)>> = Vec::with_capacity(rows.len());
        for r in rows.iter_mut() {
            heads.push(next_flags(r)?);
        }
        let mut owns = vec![
            Own {
                waived_confirmed: Some(0),
                waived_rejected: Some(0),
                requested_declined: Some(0),
                ..Own::default()
            };
            reviewers.len()
        ];
        let mut planes = Vec::with_capacity(reviewers.len());
        // each reviewer's reasons of this rule, as runs, and where the
        // chunks have come to in them
        let held: Vec<[Vec<(u64, u64, usize)>; 3]> = reviewers
            .iter()
            .map(|r| match &r.reasons {
                Some((_, store, _)) => {
                    [store.runs(Kind::Waive, ci), store.runs(Kind::NotRequired, ci), store.runs(Kind::Request, ci)]
                }
                None => [Vec::new(), Vec::new(), Vec::new()],
            })
            .collect();
        let mut cursors = vec![[0usize; 3]; reviewers.len()];
        // every rejection of this rule that stands, and each reviewer's
        // errors under one they have not answered - a waive of theirs
        // there is rejected (the same errors for everybody who answered
        // nothing in the rule)
        let standing: Vec<confirm::Standing> = reviewers
            .iter()
            .flat_map(|r| r.rejects.iter().flat_map(|(_, log)| confirm::standing_of(&r.tag, log, ci)))
            .collect();
        let open_all = confirm::unanswered(&standing, None, ci);
        let opens: Vec<Option<Vec<(u64, u64)>>> = reviewers
            .iter()
            .map(|r| match &r.rejects {
                Some((_, log)) if !standing.is_empty() && log.kept(Kind::Answered, ci).next().is_some() => {
                    Some(confirm::unanswered(&standing, Some(log), ci))
                }
                _ => None,
            })
            .collect();
        // the declines of this rule that stand, and each reviewer's errors
        // under one they have not answered - a request of theirs there is
        // declined (0.12.473: the rejections' rule, a step earlier)
        let declines: Vec<confirm::Standing> = reviewers
            .iter()
            .flat_map(|r| r.rejects.iter().flat_map(|(_, log)| confirm::standing_by(&r.tag, log, ci, Kind::Decline)))
            .collect();
        let shut_all = confirm::unanswered_by(&declines, None, ci, Kind::Reasked);
        let shuts: Vec<Option<Vec<(u64, u64)>>> = reviewers
            .iter()
            .map(|r| match &r.rejects {
                Some((_, log)) if !declines.is_empty() && log.kept(Kind::Reasked, ci).next().is_some() => {
                    Some(confirm::unanswered_by(&declines, Some(log), ci, Kind::Reasked))
                }
                _ => None,
            })
            .collect();
        let mut start = 0u64;
        while start < size {
            if (start / CHUNK).is_multiple_of(256) {
                check_cancelled(cancelled)?;
            }
            let (k, count) = (start / CHUNK, (size - start).min(CHUNK));
            planes.clear();
            for (i, reader) in statuses.iter_mut().enumerate() {
                let r = &reviewers[i];
                let waived = match reader {
                    Some(reader) => {
                        reader.read_exact(&mut buffer[..count as usize]).map_err(|e| {
                            let path = r.waive.as_ref().map(|(p, _)| p.display().to_string());
                            Error::new(ErrorKind::Io, format!("{}: {e}", path.unwrap_or_default()))
                        })?;
                        waived_bits(&buffer[..count as usize])
                    }
                    None => Bits::ZERO,
                };
                let (mut visited, mut not_required) = (Bits::ZERO, Bits::ZERO);
                // (a stored chunk before this one would be out of order: skipped)
                while heads[i].is_some_and(|(rk, ..)| rk < k) {
                    heads[i] = next_flags(&mut rows[i])?;
                }
                if let Some((rk, v, n)) = heads[i] {
                    if rk == k {
                        (visited, not_required) = (v, n);
                        heads[i] = next_flags(&mut rows[i])?;
                    }
                }
                let limit = Bits::first(count);
                let (visited, not_required) = (visited & limit, not_required & limit);
                if let Some((_, _, tally)) = &mut reviewers[i].reasons {
                    count_held(&held[i][0], &mut cursors[i][0], start, count, &waived, tally);
                    count_held(&held[i][1], &mut cursors[i][1], start, count, &not_required, tally);
                }
                let r = &reviewers[i];
                let confirmed = match &mut confirmations[i] {
                    Some(reader) => {
                        reader.read_exact(&mut buffer[..count as usize]).map_err(|e| {
                            let path = r.confirm.as_ref().map(|(p, _)| p.display().to_string());
                            Error::new(ErrorKind::Io, format!("{}: {e}", path.unwrap_or_default()))
                        })?;
                        // (a confirmation is the byte a waive is)
                        waived_bits(&buffer[..count as usize])
                    }
                    None => Bits::ZERO,
                };
                let requested = match &mut requests[i] {
                    Some(reader) => {
                        reader.read_exact(&mut buffer[..count as usize]).map_err(|e| {
                            let path = r.request.as_ref().map(|(p, _)| p.display().to_string());
                            Error::new(ErrorKind::Io, format!("{}: {e}", path.unwrap_or_default()))
                        })?;
                        // (a request is the byte a waive is)
                        waived_bits(&buffer[..count as usize])
                    }
                    None => Bits::ZERO,
                };
                if let Some((_, _, tally)) = &mut reviewers[i].reasons {
                    count_held(&held[i][2], &mut cursors[i][2], start, count, &requested, tally);
                }
                let r = &reviewers[i];
                let rejected = run_bits(opens[i].as_deref().unwrap_or(&open_all), start, count);
                let declined = run_bits(shuts[i].as_deref().unwrap_or(&shut_all), start, count);
                let own = &mut owns[i];
                own.requested += requested.count();
                *own.requested_declined.get_or_insert(0) += (requested & declined).count();
                own.waived += waived.count();
                own.visited += visited.count();
                own.not_required += not_required.count();
                own.both += (visited & not_required).count();
                own.confirmed += confirmed.count();
                planes.push(Planes {
                    visited,
                    waived,
                    not_required,
                    confirmed,
                    rejected,
                    requested,
                    declined,
                    visited_known: r.flags.is_some(),
                    waive_known: r.waive.is_some(),
                    not_required_known: r.flags.is_some(),
                });
            }
            // of each reviewer's waives, the ones that are rejected, and of
            // the others the ones a confirmer has confirmed
            let any = planes.iter().fold(Bits::ZERO, |c, p| c | p.confirmed);
            for (own, p) in owns.iter_mut().zip(&planes) {
                *own.waived_confirmed.get_or_insert(0) += (p.waived & !p.rejected & any).count();
                *own.waived_rejected.get_or_insert(0) += (p.waived & p.rejected).count();
            }
            team[ci].add(&team_of(&planes));
            start += count;
        }
        // (the errors each reviewer's own log rejects, as a confirmer)
        for (own, r) in owns.iter_mut().zip(&reviewers) {
            if let Some((_, log)) = &r.rejects {
                own.rejected = log.runs(Kind::Reject, ci).iter().map(|&(a, b, _)| b - a).sum();
                own.declined = log.runs(Kind::Decline, ci).iter().map(|&(a, b, _)| b - a).sum();
            }
        }
        drop(heads);
        drop(rows);
        drop(statements);
        for (r, own) in reviewers.iter_mut().zip(owns) {
            r.rules[ci] = own;
        }
    }
    // the report
    let names: Vec<String> = pack.checks.iter().map(|c| c.name.clone()).collect();
    let mut people = Vec::new();
    for r in &reviewers {
        let (waive, flags, confirm) = (r.waive.is_some(), r.flags.is_some(), r.confirm.is_some());
        let files = [waive, flags, confirm, r.rejects.is_some(), r.request.is_some()];
        let mut all = Own::default();
        let mut by_rule = Vec::new();
        let mut differ = Vec::new();
        for (ci, own) in r.rules.iter().enumerate() {
            all.add(own);
            if own.any() {
                let mut row = own.json(id.sizes[ci], files);
                row["index"] = json!(ci + 1);
                row["name"] = json!(names[ci]);
                by_rule.push(row);
            }
            if let Some(stored) = &r.stored {
                if u64::from(stored[ci]) != own.waived {
                    differ.push(json!({"index": ci + 1, "name": names[ci], "stored": stored[ci], "counted": own.waived}));
                }
            }
        }
        let mut person = all.json(total, files);
        person["id"] = json!(r.tag);
        person["present"] = json!(true);
        person["waive_known"] = json!(waive);
        person["visited_known"] = json!(flags);
        person["not_required_known"] = json!(flags);
        if let Some((path, place)) = &r.confirm {
            person["confirm_file"] = json!(path);
            person["confirm_location"] = json!(place);
            person["confirm_modified"] = json!(modified(path));
        }
        if let Some(((path, place), _)) = &r.rejects {
            person["rejects_file"] = json!(path);
            person["rejects_location"] = json!(place);
            person["rejects_modified"] = json!(modified(path));
        }
        if let Some((path, place)) = &r.request {
            person["request_file"] = json!(path);
            person["request_location"] = json!(place);
            person["request_modified"] = json!(modified(path));
        }
        if let Some((path, place)) = &r.waive {
            person["waive_file"] = json!(path);
            person["waive_location"] = json!(place);
            person["waive_modified"] = json!(modified(path));
            // the sidecar's stored counters against the statuses counted
            person["counters_match"] = json!(differ.is_empty());
            if !differ.is_empty() {
                person["counter_mismatch"] = json!(differ);
            }
        }
        if let Some((path, place)) = &r.flags {
            person["review_file"] = json!(path);
            person["review_location"] = json!(place);
            person["review_modified"] = json!(modified(path));
        }
        if let Some(((path, place), store, tally)) = &r.reasons {
            let (rows, with) = reasons_json(store, tally, &names);
            person["reasons_file"] = json!(path);
            person["reasons_location"] = json!(place);
            person["reasons_modified"] = json!(modified(path));
            // of the waived (set aside) errors, those that say why
            person["waived_with_reason"] = json!(with[0]);
            person["not_required_with_reason"] = json!(with[1]);
            // (a requester's: of the errors asked for, those that say why)
            if r.request.is_some() {
                person["requested_with_reason"] = json!(with[2]);
            }
            person["reasons"] = rows;
        }
        person["rules"] = json!(by_rule);
        people.push(person);
    }
    for tag in absent {
        people.push(json!({"id": tag, "present": false, "rules": []}));
    }
    let mut whole = Team::default();
    let mut team_rules = Vec::with_capacity(rules);
    for (ci, t) in team.iter().enumerate() {
        whole.add(t);
        let mut row = t.json(id.sizes[ci]);
        row["index"] = json!(ci + 1);
        row["name"] = json!(names[ci]);
        team_rules.push(row);
    }
    let mut team_json = whole.json(total);
    team_json["reviewers"] = json!(reviewers.len());
    team_json["rules"] = json!(team_rules);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let report = json!({
        "type": TYPE,
        "version": VERSION,
        "generated": utc(now),
        "database": crate::cache::database_path(&pack.path)?,
        "pack": pack.path,
        "fingerprint": id.fingerprint,
        "cell": pack.cell,
        "errors": total,
        "rules": names
            .iter()
            .enumerate()
            .map(|(ci, name)| json!({"index": ci + 1, "name": name, "errors": id.sizes[ci]}))
            .collect::<Vec<_>>(),
        "reviewers": people,
        "team": team_json,
        "skipped": skipped,
    });
    Ok((report, team))
}

fn next_flags(rows: &mut Option<rusqlite::Rows<'_>>) -> Result<Option<(u64, Bits, Bits)>> {
    let Some(rows) = rows else { return Ok(None) };
    match rows.next().map_err(sql)? {
        None => Ok(None),
        Some(row) => {
            let k: i64 = row.get(0).map_err(sql)?;
            let v: Vec<u8> = row.get(1).map_err(sql)?;
            let n: Vec<u8> = row.get(2).map_err(sql)?;
            Ok(Some((
                k as u64,
                Bits::from_blob(&v, "working review bitmap")?,
                Bits::from_blob(&n, "working review bitmap")?,
            )))
        }
    }
}

/// A waive sidecar of this result, positioned at its statuses, and its
/// stored counters.
pub(super) fn open_waive(path: &Path, id: &Identity) -> Result<(File, Vec<u32>)> {
    waive_of(File::open(path)?, id)
}

/// `open_waive` of a sidecar already open.
pub(super) fn waive_of(mut file: File, id: &Identity) -> Result<(File, Vec<u32>)> {
    let (total, rules) = (id.total(), id.sizes.len());
    let mut head = [0u8; 40];
    let matches = file.read_exact_at(&mut head, 0).is_ok()
        && head == id.waive_header
        && file.metadata()?.len() == 40 + total + 4 * rules as u64;
    if !matches {
        return Err(Error::new(
            ErrorKind::Cache,
            "waive file of another DRC result (its header or size is not this pack's)",
        ));
    }
    let mut raw = vec![0u8; rules * 4];
    file.read_exact_at(&mut raw, 40 + total)?;
    let stored = raw
        .chunks_exact(4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    file.seek(SeekFrom::Start(40))?;
    Ok((file, stored))
}

/// A reviewer's rejects log at `path` as it reads: None without a file, an
/// error for a file that is not this result's.
pub(super) fn rejects_log(path: &Path, id: &Identity) -> Result<Option<Reasons>> {
    Reasons::load(path, &id.result_tag(), &id.sizes)
}

/// A rejects log's rejections: each rule's rejected errors as runs.
pub(super) fn rejected_of(log: &Reasons, rules: usize) -> Vec<Vec<(u64, u64)>> {
    held_of(log, Kind::Reject, rules)
}

/// What a log holds of `kind` - the rejections of a confirmer, the declines
/// of a waiver - as each rule's runs.
pub(super) fn held_of(log: &Reasons, kind: Kind, rules: usize) -> Vec<Vec<(u64, u64)>> {
    (0..rules)
        .map(|ci| {
            // (runs of two actions that touch are one run here)
            let mut runs: Vec<(u64, u64)> = Vec::new();
            for (a, b, _) in log.runs(kind, ci) {
                match runs.last_mut() {
                    Some(last) if last.1 == a => last.1 = b,
                    _ => runs.push((a, b)),
                }
            }
            runs
        })
        .collect()
}

/// A request file of this result, open to read, and its counter a rule
/// (drc::confirm: a byte an error, as a confirm file is).
pub(super) fn open_request(path: &Path, id: &Identity) -> Result<(File, Vec<u32>)> {
    crate::drc::confirm::read(
        path,
        &crate::drc::confirm::request_header_of(&id.waive_header),
        id.total(),
        id.sizes.len(),
    )
}

/// A confirm file of this result, open to read, and its counter a rule
/// (drc::confirm).
pub(super) fn open_confirm(path: &Path, id: &Identity) -> Result<(File, Vec<u32>)> {
    crate::drc::confirm::read(
        path,
        &crate::drc::confirm::header_of(&id.waive_header),
        id.total(),
        id.sizes.len(),
    )
}

/// A flags file of this result, in a read transaction.
fn open_flags(path: &Path, id: &Identity) -> Result<Connection> {
    let conn = flags_connection(path, id)?;
    conn.execute_batch("BEGIN").map_err(sql)?;
    Ok(conn)
}

/// A flags file of this result, open for reading - no transaction begun: a
/// reader that counts for long begins and ends its own, so the reviewer's
/// writes are not kept waiting (drc::team::live).
pub(super) fn flags_connection(path: &Path, id: &Identity) -> Result<Connection> {
    let conn = open_read_only(path, Duration::from_secs(10))?;
    let text: String = conn
        .query_row("SELECT value FROM metadata WHERE id=1", [], |r| r.get(0))
        .map_err(sql)?;
    let meta: Value = serde_json::from_str(&text)
        .map_err(|e| Error::new(ErrorKind::Cache, format!("review flags metadata: {e}")))?;
    let sizes: Option<Vec<u64>> = meta
        .get("sizes")
        .and_then(Value::as_array)
        .and_then(|a| a.iter().map(Value::as_u64).collect());
    if meta.get("fingerprint").and_then(Value::as_str) != Some(id.fingerprint.as_str())
        || sizes.as_deref() != Some(&id.sizes[..])
    {
        return Err(Error::new(
            ErrorKind::Cache,
            "review flags of another DRC result (their content id or rule sizes are not this pack's)",
        ));
    }
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_working_file_names_its_reviewer_and_leftovers_do_not() {
        for tag in ["kim", "Kim.Lee_corp", "10.1.2.3", "김철수", "a-b", "v1.stale", "x.tmp"] {
            assert_eq!(reviewer_of(tag), Some(tag));
        }
        for tag in ["", "kim.stale-1794300165", "kim.tmp-4242", "a/b", "a b", "kim@x"] {
            assert_eq!(reviewer_of(tag), None, "{tag}");
        }
        assert_eq!(reviewer_of(&"k".repeat(201)), None);
    }

    #[test]
    fn times_are_utc_iso8601() {
        assert_eq!(utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(utc(1_794_300_141), "2026-11-10T08:42:21Z");
        assert_eq!(utc(4_102_444_799), "2099-12-31T23:59:59Z");
    }

    #[test]
    fn waived_statuses_become_bits() {
        let mut statuses = vec![0u8; 200];
        for i in [0usize, 63, 64, 130, 199] {
            statuses[i] = 1;
        }
        statuses[5] = 2; // a reserved status is not waived
        let bits = waived_bits(&statuses);
        assert_eq!(bits.ones().collect::<Vec<_>>(), vec![0, 63, 64, 130, 199]);
        assert!(waived_bits(&[]).is_zero());
    }

    fn bits(ones: &[u64]) -> Bits {
        let mut b = Bits::ZERO;
        for &i in ones {
            b.set(i);
        }
        b
    }

    /// A reason counts the errors that hold it and are waived (or set
    /// aside) in the chunk at hand - also a run that crosses chunks.
    #[test]
    fn a_reason_counts_the_errors_that_hold_it_and_are_set_now() {
        let runs = [(0u64, 10u64, 0usize), (4090, 4100, 1), (9000, 9001, 2)];
        let mut tally = [0u64; 3];
        let mut cursor = 0;
        // errors 0..4096: 3 and 9 of the first run (10 is past it), 4090
        // and 4095 of the second
        count_held(&runs, &mut cursor, 0, 4096, &bits(&[3, 9, 10, 4090, 4095]), &mut tally);
        assert_eq!(tally, [2, 2, 0]);
        // errors 4096..8192: 4096 and 4099 of the second run, 4100 past it
        count_held(&runs, &mut cursor, 4096, 4096, &bits(&[0, 3, 4]), &mut tally);
        assert_eq!(tally, [2, 4, 0]);
        // the rule's last chunk, 900 errors: 9000 of the third run
        count_held(&runs, &mut cursor, 8192, 900, &bits(&[807, 808, 809]), &mut tally);
        assert_eq!(tally, [2, 4, 1]);
        // a whole chunk inside one run, and a chunk no run reaches
        let wide = [(0u64, 20_000u64, 0usize)];
        let (mut tally, mut cursor) = ([0u64; 1], 0);
        count_held(&wide, &mut cursor, 4096, 4096, &Bits::first(4096), &mut tally);
        count_held(&wide, &mut cursor, 8192, 4096, &bits(&[0, 4095]), &mut tally);
        count_held(&wide, &mut cursor, 20_480, 4096, &Bits::first(4096), &mut tally);
        assert_eq!(tally, [4098]);
        count_held(&[], &mut 0, 0, 4096, &Bits::first(4096), &mut []);
    }

    /// The report lists a reason once - by kind and text - with the errors
    /// of every action that gave it; one that holds no error now is left out.
    #[test]
    fn reasons_are_listed_by_what_they_say() {
        let path = std::env::temp_dir().join(format!("floe-reasons-stats-{}", std::process::id()));
        std::fs::write(
            &path,
            "floe_reasons=1\nfloe_pack=T\nw 0 100 0,10 dummy\nw 1 300 0,5 dummy\nw 0 200 20,5 seal\nn 0 50 0,3 scope\nw 1 400 9,1 nobody\n",
        )
        .unwrap();
        let store = Reasons::load(&path, "T", &[100, 100]).unwrap().unwrap();
        std::fs::remove_file(&path).unwrap();
        let names = ["A".to_string(), "B".to_string()];
        let (rows, with) = reasons_json(&store, &[7, 5, 2, 3, 0], &names);
        assert_eq!(with, [14, 3, 0]);
        assert_eq!(
            rows,
            json!([
                {"kind": "waive", "text": "dummy", "errors": 12, "actions": 2,
                 "first": "1970-01-01T00:01:40Z", "last": "1970-01-01T00:05:00Z",
                 "rules": [{"index": 1, "name": "A", "errors": 7}, {"index": 2, "name": "B", "errors": 5}]},
                {"kind": "waive", "text": "seal", "errors": 2, "actions": 1,
                 "first": "1970-01-01T00:03:20Z", "last": "1970-01-01T00:03:20Z",
                 "rules": [{"index": 1, "name": "A", "errors": 2}]},
                {"kind": "not_required", "text": "scope", "errors": 3, "actions": 1,
                 "first": "1970-01-01T00:00:50Z", "last": "1970-01-01T00:00:50Z",
                 "rules": [{"index": 1, "name": "A", "errors": 3}]},
            ])
        );
        // nothing held: nothing listed
        let (rows, with) = reasons_json(&store, &[0; 5], &names);
        assert_eq!((rows, with), (json!([]), [0, 0, 0]));
    }

    fn planes(v: &[u64], w: &[u64], n: &[u64], flags: bool, waive: bool) -> Planes {
        Planes {
            visited: bits(v),
            waived: bits(w),
            not_required: bits(n),
            confirmed: Bits::ZERO,
            rejected: Bits::ZERO,
            requested: Bits::ZERO,
            declined: Bits::ZERO,
            visited_known: flags,
            waive_known: waive,
            not_required_known: flags,
        }
    }

    /// The team's counts in brief (0.12.470, user 2026-11-14: "팀 작업 통계
    /// cli 명령의 결과를 다음과 같이 간략하게 ... 특정 룰이 옵션에 명시되면
    /// 전체와 해당 룰만 포함하고 특정 룰이 없을 경우 전체와 모든 룰을
    /// 포함해서"): one line, the keys and the rules in their order, the
    /// rules that have errors - or the one named.
    #[test]
    fn the_brief_report_is_one_line_of_the_total_and_the_rules_with_errors() {
        let team = |waived_any, confirmed, rejected, visited, not_required_any| Team {
            visited,
            waived: waived_any,
            not_required: not_required_any,
            conflicts: 0,
            done: visited,
            waived_any,
            confirmed,
            rejected,
            not_required_any,
            requested: 0,
            unwaived: 0,
            declined: 0,
        };
        let names = ["GRX.1", "CLEAN", "M1 \"S\".1", "GRX.1", "한글,룰"];
        let sizes = [100u64, 0, 7, 20, 3];
        let teams = [team(10, 5, 1, 30, 10), Team::default(), team(7, 0, 0, 0, 0), team(1, 1, 2, 3, 4), Team::default()];
        let text = brief_text(&names, &sizes, &teams, BriefRules::WithErrors).unwrap();
        // one line, no blank but a name's own; the rule without errors is
        // left out; a name the result has twice is one entry, summed, where
        // the first of them is
        assert_eq!(
            text,
            concat!(
                r#"{"total":{"errors":130,"requested":0,"declined":0,"waived":18,"confirmed":6,"rejected":3,"visited":33,"ignored":14},"#,
                r#""rule":{"GRX.1":{"errors":120,"requested":0,"declined":0,"waived":11,"confirmed":6,"rejected":3,"visited":33,"ignored":14},"#,
                r#""M1 \"S\".1":{"errors":7,"requested":0,"declined":0,"waived":7,"confirmed":0,"rejected":0,"visited":0,"ignored":0},"#,
                r#""한글,룰":{"errors":3,"requested":0,"declined":0,"waived":0,"confirmed":0,"rejected":0,"visited":0,"ignored":0}}}"#
            )
        );
        assert!(!text.contains('\n'));
        let parsed: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed["rule"]["M1 \"S\".1"]["waived"], 7);
        assert_eq!(parsed["rule"].as_object().unwrap().len(), 3);
        // a rule named: the whole result's total and that rule alone - one
        // without errors too, when it is asked for
        assert_eq!(
            brief_text(&names, &sizes, &teams, BriefRules::Named("M1 \"S\".1")).unwrap(),
            concat!(
                r#"{"total":{"errors":130,"requested":0,"declined":0,"waived":18,"confirmed":6,"rejected":3,"visited":33,"ignored":14},"#,
                r#""rule":{"M1 \"S\".1":{"errors":7,"requested":0,"declined":0,"waived":7,"confirmed":0,"rejected":0,"visited":0,"ignored":0}}}"#
            )
        );
        let clean = brief_text(&names, &sizes, &teams, BriefRules::Named("CLEAN")).unwrap();
        assert!(clean.ends_with(r#""rule":{"CLEAN":{"errors":0,"requested":0,"declined":0,"waived":0,"confirmed":0,"rejected":0,"visited":0,"ignored":0}}}"#));
        let twice: Value =
            serde_json::from_str(&brief_text(&names, &sizes, &teams, BriefRules::Named("GRX.1")).unwrap()).unwrap();
        assert_eq!((twice["rule"]["GRX.1"]["errors"].as_u64(), twice["rule"].as_object().unwrap().len()), (Some(120), 1));
        let e = brief_text(&names, &sizes, &teams, BriefRules::Named("NOPE")).unwrap_err();
        assert!(e.message.contains("no such rule \"NOPE\""), "{}", e.message);
        // every rule, asked for: the one without errors in its place
        let all = brief_text(&names, &sizes, &teams, BriefRules::All).unwrap();
        assert!(all.starts_with(&text[..text.find(r#","rule""#).unwrap()]), "the same total");
        assert!(
            all.contains(r#"},"CLEAN":{"errors":0,"requested":0,"declined":0,"waived":0,"confirmed":0,"rejected":0,"visited":0,"ignored":0},"M1 "#),
            "{all}"
        );
        let parsed: Value = serde_json::from_str(&all).unwrap();
        assert_eq!(parsed["rule"].as_object().unwrap().len(), 4);
        // a result without an error, and one without a rule
        assert_eq!(
            brief_text(&["A"], &[0], &[Team::default()], BriefRules::WithErrors).unwrap(),
            r#"{"total":{"errors":0,"requested":0,"declined":0,"waived":0,"confirmed":0,"rejected":0,"visited":0,"ignored":0},"rule":{}}"#
        );
        assert!(brief_text(&[], &[], &[], BriefRules::All).unwrap().ends_with(r#""rule":{}}"#));
        // every report starts the same way: how a file that holds an earlier
        // one is known from any other
        for listed in [BriefRules::WithErrors, BriefRules::All, BriefRules::Named("CLEAN")] {
            assert!(brief_text(&names, &sizes, &teams, listed).unwrap().starts_with(BRIEF_HEAD), "{listed:?}");
        }
        assert!(brief_text(&[], &[], &[], BriefRules::All).unwrap().starts_with(BRIEF_HEAD));
    }

    #[test]
    fn a_rules_runs_are_a_chunks_bits() {
        let runs = [(0u64, 3u64), (10, 12), (4090, 4100), (8191, 8193)];
        assert_eq!(run_bits(&runs, 0, 4096), bits(&[0, 1, 2, 10, 11, 4090, 4091, 4092, 4093, 4094, 4095]));
        assert_eq!(run_bits(&runs, 4096, 4096), bits(&[0, 1, 2, 3, 4095]));
        assert_eq!(run_bits(&runs, 8192, 100), bits(&[0]));
        assert_eq!(run_bits(&runs, 12288, 4096), Bits::ZERO);
        assert_eq!(run_bits(&[], 0, 4096), Bits::ZERO);
        assert_eq!(run_bits(&[(0, 9000)], 4096, 4096), Bits::first(4096));
        assert_eq!(run_bits(&[(0, 9000)], 8192, 808), Bits::first(808));
    }

    #[test]
    fn the_team_counts_are_the_reviewers_windows() {
        // kim visited 0..5 and waived 0..10, excluded 100..110; lee visited
        // 0, 5 and 20 and waived 5..15: error 0 is a conflict (both visited
        // it, one waived), 5..10 agree, 10..15 lee's alone (not visited by
        // kim: no conflict); park has a waive file alone (visits unknown)
        let kim = planes(&[0, 1, 2, 3, 4], &(0..10).collect::<Vec<_>>(), &(100..110).collect::<Vec<_>>(), true, true);
        let lee = planes(&[0, 5, 20], &(5..15).collect::<Vec<_>>(), &[], true, true);
        let park = planes(&[], &[], &[], false, true);
        let t = team_of(&[kim, lee, park]);
        assert_eq!(
            t,
            Team { visited: 7, waived: 14, not_required: 10, conflicts: 1, done: 17, waived_any: 15, confirmed: 0, rejected: 0, not_required_any: 10, requested: 0, unwaived: 0, declined: 0 }
        );
        // a confirmer's confirmations count on the errors somebody has
        // waived - a conflict's waive too (error 0) - and on no other: of
        // 0, 3, 12 and 50 confirmed, 50 is nobody's waive. A second
        // confirmer's same error is the same error
        let mut hong = planes(&[], &[], &[], false, false);
        hong.confirmed = bits(&[0, 3, 12, 50]);
        let mut yoon = planes(&[], &[], &[], false, false);
        yoon.confirmed = bits(&[3, 14, 60]);
        let t = team_of(&[kim, lee, park, hong, yoon]);
        assert_eq!(
            t,
            Team { visited: 7, waived: 14, not_required: 10, conflicts: 1, done: 17, waived_any: 15, confirmed: 4, rejected: 0, not_required_any: 10, requested: 0, unwaived: 0, declined: 0 }
        );
        assert_eq!(
            t.json(200),
            json!({"errors": 200, "visited": 7, "waived": 14, "not_required": 10, "conflicts": 1,
                   "pending": 183, "confirmed": 4, "unconfirmed": 11, "rejected": 0})
        );
        // a rejected waive is no waive, though its reviewer's file has it:
        // a confirmer rejects 0 (kim's waive, lee visited it and left it: the
        // conflict), 3 (kim's, confirmed by two), 12 (lee's, confirmed) and
        // 70 (nobody's) - every reviewer's errors under a rejection they have
        // not answered. The conflict is none any more, the confirmations of
        // 0, 3 and 12 count for nothing, and three errors - the ones that
        // were waived - are rejected
        let under = bits(&[0, 3, 12, 70]);
        let (mut kim, mut lee) = (kim, lee);
        (kim.rejected, lee.rejected) = (under, under);
        let t = team_of(&[kim, lee, hong, yoon]);
        assert_eq!(
            t,
            Team { visited: 7, waived: 12, not_required: 10, conflicts: 0, done: 17, waived_any: 12, confirmed: 1, rejected: 3, not_required_any: 10, requested: 0, unwaived: 0, declined: 0 }
        );
        assert_eq!(
            t.json(200),
            json!({"errors": 200, "visited": 7, "waived": 12, "not_required": 10, "conflicts": 0,
                   "pending": 183, "confirmed": 1, "unconfirmed": 11, "rejected": 3})
        );
        // a reviewer who waived a rejected error again - theirs is a waive
        // from then, whatever is rejected of the others': the error is
        // waived, by them, and not rejected (0 and 3 are confirmed still:
        // their confirmations count again)
        let mut again = planes(&[], &[0, 3], &[], false, true);
        again.rejected = bits(&[12, 70]);
        let t = team_of(&[kim, lee, hong, yoon, again]);
        assert_eq!((t.waived_any, t.confirmed, t.rejected), (14, 3, 1), "0 and 3 are waived again; 12 waits for lee");
        // (kim visited 0 and waived it, lee visited it and did not: with a
        // waive of the error that counts, theirs is no conflict - kim's own
        // waive of it is rejected, and is nobody's judgement)
        assert_eq!(t.conflicts, 0);
        // an exclusion two visitors disagree on is a conflict too, and is not
        // counted as excluded - though it is one of the errors somebody set
        // aside (`not_required_any`, the brief report's `ignored`); a
        // waive-only file never conflicts
        let a = planes(&[1], &[], &[1], true, true);
        let b = planes(&[1], &[], &[], true, true);
        let c = planes(&[], &[1], &[], false, true);
        let t = team_of(&[a, b, c]);
        assert_eq!(
            t,
            Team { visited: 1, waived: 0, not_required: 0, conflicts: 1, done: 1, waived_any: 1, confirmed: 0, rejected: 0, not_required_any: 1, requested: 0, unwaived: 0, declined: 0 }
        );
        // (the brief report says the unions: the waive a visitor differs on
        // is waived, the exclusion one differs on is ignored)
        assert_eq!(
            Brief::of(&t, 9),
            Brief { errors: 9, requested: 0, declined: 0, waived: 1, confirmed: 0, rejected: 0, visited: 1, ignored: 1 }
        );
        // nobody: nothing
        assert_eq!(team_of(&[]), Team::default());
        // one reviewer's own numbers: waived whether visited or not
        let t = team_of(&[planes(&[3], &[3, 4], &[9], true, true)]);
        assert_eq!(
            t,
            Team { visited: 1, waived: 2, not_required: 1, conflicts: 0, done: 2, waived_any: 2, confirmed: 0, rejected: 0, not_required_any: 1, requested: 0, unwaived: 0, declined: 0 }
        );
    }

    /// The requests of a team (0.12.472: "error -> waive request -> waive ->
    /// confirm"): the errors somebody asked the waive of, and the ones of
    /// them nobody has waived - a waive that is rejected is one still.
    #[test]
    fn the_team_counts_the_requests_and_the_ones_that_wait() {
        // a requester who asked for 0..6, another for 4..8; a waiver who
        // waived 0, 1, 2 and 9 (never asked for) - 2 under a rejection they
        // have not answered -, and a confirmer who confirmed 0
        let asking = |errors: &[u64]| Planes { requested: bits(errors), ..planes(&[], &[], &[], false, false) };
        let waiver = Planes { rejected: bits(&[2]), ..planes(&[], &[0, 1, 2, 9], &[], false, true) };
        let confirmer = Planes { confirmed: bits(&[0]), ..planes(&[], &[], &[], false, false) };
        let t = team_of(&[asking(&[0, 1, 2, 3, 4, 5]), asking(&[4, 5, 6, 7]), waiver, confirmer]);
        assert_eq!((t.requested, t.unwaived), (8, 5), "asked for by anybody; 3..8 wait for a waiver");
        assert_eq!((t.waived_any, t.confirmed, t.rejected), (3, 1, 1), "0, 1 and 9 waived; 2 rejected");
        // the report says them where somebody asked, and not elsewhere
        let said = t.json(10);
        assert_eq!((said["requested"].as_u64(), said["unwaived"].as_u64()), (Some(8), Some(5)));
        assert!(team_of(&[waiver, confirmer]).json(10).get("requested").is_none());
        // the brief report's are the union, the workflow's steps in order
        let brief = Brief::of(&t, 10);
        assert_eq!(
            brief,
            Brief { errors: 10, requested: 8, declined: 0, waived: 3, confirmed: 1, rejected: 1, visited: 0, ignored: 0 }
        );
        let mut text = String::new();
        brief.write(&mut text);
        assert_eq!(
            text,
            r#"{"errors":10,"requested":8,"declined":0,"waived":3,"confirmed":1,"rejected":1,"visited":0,"ignored":0}"#
        );
        assert!(text.starts_with(&BRIEF_HEAD["{\"total\":".len()..]));
    }

    /// The declined requests of a team (0.12.473; user 2026-11-14: "요청
    /// 거절 상태도 만들어줘"): a request under a decline its requester has
    /// not answered is none, and an error whose requests are all declined is
    /// counted as that while nobody has waived it.
    #[test]
    fn the_team_counts_a_declined_request_as_none() {
        // park asked for 0..6 and is under a decline of 0, 1 and 4 they
        // have not answered; choi asked for 4..8 and is under the same
        // decline of 4 - and of 8, which they never asked for; a waiver
        // waived 0, 1, 2 and 9 - 2 under a rejection -, a confirmer
        // confirmed 0
        let asking = |errors: &[u64], declined: &[u64]| Planes {
            requested: bits(errors),
            declined: bits(declined),
            ..planes(&[], &[], &[], false, false)
        };
        let park = asking(&[0, 1, 2, 3, 4, 5], &[0, 1, 4]);
        let choi = asking(&[4, 5, 6, 7], &[4, 8]);
        let waiver = Planes { rejected: bits(&[2]), ..planes(&[], &[0, 1, 2, 9], &[], false, true) };
        let confirmer = Planes { confirmed: bits(&[0]), ..planes(&[], &[], &[], false, false) };
        let t = team_of(&[park, choi, waiver, confirmer]);
        // asked for: 2, 3, 5 (park's that are not declined) and 5, 6, 7;
        // 3, 5, 6 and 7 wait for a waiver; 4 is declined for both who asked
        // - 0 and 1 are declined too, and waived: not counted as declined
        assert_eq!((t.requested, t.unwaived, t.declined), (5, 4, 1));
        assert_eq!((t.waived_any, t.confirmed, t.rejected), (3, 1, 1), "the waives are as they were");
        let said = t.json(10);
        assert_eq!(
            (said["requested"].as_u64(), said["unwaived"].as_u64(), said["declined"].as_u64()),
            (Some(5), Some(4), Some(1))
        );
        // one of two requesters answered (asked again): the error is asked
        // for, by them
        let again = asking(&[4, 5, 6, 7], &[8]);
        let t = team_of(&[park, again, waiver, confirmer]);
        assert_eq!((t.requested, t.unwaived, t.declined), (6, 5, 0));
        // nothing but declined requests: said, with no request counted
        let t = team_of(&[asking(&[3], &[3])]);
        assert_eq!((t.requested, t.unwaived, t.declined), (0, 0, 1));
        assert_eq!(
            t.json(10),
            json!({"errors": 10, "visited": 0, "waived": 0, "not_required": 0, "conflicts": 0, "pending": 10,
                   "confirmed": 0, "unconfirmed": 0, "rejected": 0, "requested": 0, "unwaived": 0, "declined": 1})
        );
        // the brief report: declined after requested, the workflow's steps
        let brief = Brief::of(&team_of(&[park, choi, waiver, confirmer]), 10);
        assert_eq!(
            brief,
            Brief { errors: 10, requested: 5, declined: 1, waived: 3, confirmed: 1, rejected: 1, visited: 0, ignored: 0 }
        );
        let mut text = String::new();
        brief.write(&mut text);
        assert_eq!(
            text,
            r#"{"errors":10,"requested":5,"declined":1,"waived":3,"confirmed":1,"rejected":1,"visited":0,"ignored":0}"#
        );
        // a reviewer's row says the requests of theirs that are declined,
        // and a waiver's the requests they declined - where there are any
        let own = Own { requested: 6, requested_declined: Some(3), ..Own::default() };
        assert_eq!(own.json(10, [false, false, false, false, true]), json!({"requested": 6, "requested_declined": 3}));
        let own = Own { declined: 4, ..Own::default() };
        assert_eq!(own.json(10, [false, false, false, true, false]), json!({"rejected": 0, "declined": 4}));
        assert!(own.any() && !Own::default().any());
        assert_eq!(Own::default().json(10, [false, false, false, true, false]), json!({"rejected": 0}));
    }
}
