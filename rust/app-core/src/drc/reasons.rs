//! Review reasons: why a reviewer waived errors, or set errors aside from the
//! review ("not for review") - said once for the action that did it, however
//! many errors the action took (user 2026-11-13: "waive와 not for waive 처리할
//! 때 사유를 입력할 수 있게 입력창이 뜨도록 ... 복수개의 error를 waive할 경우
//! error마다 동일한 note가 생겨서 수백만개의 에러를 waive할 경우 note가 너무
//! 많아지는 등 부작용"). Until 0.12.463 a reason was a note, and a note is
//! kept an error at a time: one `text=` annotation an error in its file, every
//! member's id in a line, each member's geometry decoded for the annotation's
//! place, the whole file made again at every edit and the whole member list
//! handed to the viewer at every open.
//!
//! A reason belongs to the action. The file `.<db>.reasons.<reviewer>` beside
//! the waive sidecar is a log of actions, a line each:
//!
//! ```text
//! # floe drc review reasons
//! floe_reasons=1
//! floe_pack=<content id prefix, 32 hex>,<errors>
//! w <rule> <time> <runs> <text>     waived, with this reason
//! n <rule> <time> <runs> <text>     set aside from the review, with it
//! W <rule> <time> <runs>            these errors' waive reason is gone
//! N <rule> <time> <runs>            these errors' exclusion reason is gone
//! q <rule> <time> <runs> <text>     their waive was asked for, with this
//!                                   reason (a requester's; 0.12.472)
//! Q <rule> <time> <runs>            these errors' request reason is gone
//! ```
//!
//! `<rule>` is the rule's index in the pack (from 0), `<time>` seconds since
//! the epoch, `<runs>` the errors as rule-local half-open runs written as
//! `gap,length` pairs - the first gap from 0, each next from the end of the
//! run before - so a whole rule is `0,N` and a million scattered errors are a
//! few bytes each; `<text>` is the rest of the line (`\` and a line break
//! escaped as the notes escape them). A later line wins where two meet.
//!
//! What is kept in memory is the same shape: the actions that still hold
//! errors, and a rule's errors as sorted runs that name their action - a
//! lookup is a binary search, an action over k runs a merge. Nothing is an
//! error long.
//!
//! The file says why, not whether: an error's status byte (the waive
//! sidecar) and its flag (the review flags) say whether, and a reason is
//! shown only of an error that is waived or set aside now. Every writer of
//! those drops the reason of an error it turns back (`clear`), so the two
//! agree; a file an older build wrote under, or a run that ended between the
//! two writes, can leave a reason on an error that is not waived - which is
//! why the readers ask the status too.
//!
//! A line is appended and flushed with the action that made it. What a job
//! takes back - a chunk of statuses at a time - is dropped from memory at
//! once and said when the job ends, one line a rule (`clear`, `flush`). A
//! torn last line (no line break) is passed over and cut before the next
//! one; a file much larger than what it still says is written again at the
//! open.
//!
//! A confirmer's rejections are a log of the same shape in a file of its
//! own, `.<db>.rejects.<reviewer>` (0.12.468; user 2026-11-14: "confirmer가
//! 다른 리뷰어의 waive를 반려할 수 있게 해줘"):
//!
//! ```text
//! # floe drc waive rejections
//! r <rule> <time> <runs> [<text>]   these errors' waive is rejected, why
//! R <rule> <time> <runs>            their rejection is taken back
//! ```
//!
//! There the log says whether, not only why: an error's waive is rejected
//! while its confirmer's log holds it, with or without a reason
//! (`Kind::Reject`). Who reads it and what follows is `confirm::Locks` and
//! `desktop::Review`'s.
//!
//! A rejection does not take the waive away (0.12.469; user 2026-11-14:
//! "리뷰어가 사유를 수정해서 다시 waive를 할 수 있게 하되, 이 때 이전의
//! waive사유나 반려사유는 계속 유지되어야"): the waive stays in its
//! reviewer's file, rejected, until the reviewer waives the error again with
//! a reason - which answers the rejection. What was is kept then, in the
//! rejects log of the reviewer who answered (every reviewer has one from
//! their first answer on, a confirmer or not):
//!
//! ```text
//! floe_reasons=2
//! j <rule> <time> <runs> <who> [<text>]   answered: <who> rejected these
//!                                         errors' waive at <time>, why
//! p <rule> <time> <runs> <text>           the waive reason they had before
//!                                         (given at <time>)
//! ```
//!
//! These are kept records (`Kind::Answered`, `Kind::Earlier`): a line takes
//! nothing from an earlier one and is never taken back - an error waived
//! again three times has three of each. A `j` line is also what says that
//! its reviewer's waive is no longer under that rejection: a rejection is
//! named by who made it and when (a confirmer's log gives every rejection a
//! time of its own), and a waive is rejected while a confirmer's log holds
//! the error under a rejection its reviewer has not answered. The version
//! is 2 so that a build of 0.12.468 - which turned a rejected waive back -
//! passes the log over; a log of version 1 is read, and written again as 2
//! when its reviewer's session opens it.
use super::analysis::scope::Membership;
use super::local::aside_path;
use crate::{annotations, Error, ErrorKind, Result};
use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// The file's format version (`floe_reasons=`).
pub const VERSION: u32 = 1;
/// The rejects log's: 2 since it keeps what its reviewer answered (0.12.469).
pub const REJECTS_VERSION: u32 = 2;
/// The longest reason, in characters.
pub const TEXT_LIMIT: usize = 1000;
/// A file this much larger than twice what it still says is written again.
const SLACK: u64 = 64 * 1024;

/// Rule-local errors `start..stop`.
pub type Run = (u64, u64);

/// What a reason is the reason of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Waive,
    NotRequired,
    /// a confirmer's rejection of the errors' waive (the rejects log)
    Reject,
    /// a rejection the log's reviewer answered by waiving the errors again:
    /// who rejected, when, why - kept (the rejects log of who answered)
    Answered,
    /// the waive reason the errors had before their reviewer waived them
    /// again: kept there too
    Earlier,
    /// a requester's request of the errors' waive (0.12.472): why they
    /// asked - the reviewer's reasons file, beside their waive reasons
    Request,
}

impl Kind {
    pub const ALL: [Kind; 6] =
        [Kind::Waive, Kind::NotRequired, Kind::Reject, Kind::Answered, Kind::Earlier, Kind::Request];
    /// The kinds an error holds one reason of (the others are kept records).
    pub const HELD: [Kind; 4] = [Kind::Waive, Kind::NotRequired, Kind::Reject, Kind::Request];

    fn slot(self) -> usize {
        match self {
            Kind::Waive => 0,
            Kind::NotRequired => 1,
            Kind::Reject => 2,
            Kind::Answered => 3,
            Kind::Earlier => 4,
            Kind::Request => 5,
        }
    }
    /// A line's first word: with a reason, and the reason gone (a kept
    /// record is never gone: its second word is no line's).
    fn tags(self) -> (&'static str, &'static str) {
        match self {
            Kind::Waive => ("w", "W"),
            Kind::NotRequired => ("n", "N"),
            Kind::Reject => ("r", "R"),
            Kind::Answered => ("j", "J"),
            Kind::Earlier => ("p", "P"),
            Kind::Request => ("q", "Q"),
        }
    }
    /// A line of this kind is an action even without a text: a rejection
    /// stands whether its confirmer said why or not.
    fn bare(self) -> bool {
        matches!(self, Kind::Reject | Kind::Answered)
    }
    /// A record of this kind is kept as it was: it takes no error from an
    /// earlier one and is not taken back (see the module docs).
    pub fn kept(self) -> bool {
        matches!(self, Kind::Answered | Kind::Earlier)
    }
    /// As the statistics name it.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Waive => "waive",
            Kind::NotRequired => "not_required",
            Kind::Reject => "reject",
            Kind::Answered => "answered",
            Kind::Earlier => "earlier",
            Kind::Request => "request",
        }
    }
    /// From a request's word (the review job's actions, the statistics').
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "waive" => Some(Kind::Waive),
            "not-required" | "not_required" => Some(Kind::NotRequired),
            "reject" => Some(Kind::Reject),
            "request" => Some(Kind::Request),
            _ => None,
        }
    }
}

/// One action's reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub kind: Kind,
    pub check: usize,
    /// seconds since the epoch
    pub time: u64,
    pub text: String,
    /// who rejected (`Kind::Answered`); empty for the other kinds, whose
    /// actions are the log's reviewer's own
    pub by: String,
}

/// Errors `start..stop` of a rule under the action `entry`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Seg {
    start: u64,
    stop: u64,
    entry: u32,
}

/// A reason as it is kept: one line - control characters are spaces -,
/// trimmed, at most `TEXT_LIMIT` characters.
pub fn clean(text: &str) -> Result<String> {
    let text: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let text = text.trim();
    if text.chars().count() > TEXT_LIMIT {
        return Err(Error::input(format!(
            "a review reason is at most {TEXT_LIMIT} characters"
        )));
    }
    Ok(text.to_owned())
}

/// The runs of a list of errors (any order, repeats allowed).
pub fn runs_of(errors: &[u64]) -> Vec<Run> {
    let mut sorted = errors.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut runs: Vec<Run> = Vec::new();
    for e in sorted {
        match runs.last_mut() {
            Some(last) if last.1 == e => last.1 = e + 1,
            _ => runs.push((e, e + 1)),
        }
    }
    runs
}

/// The runs of `members` below `upto`, read a bounded range at a time.
pub fn member_runs(members: &dyn Membership, upto: u64) -> Vec<Run> {
    const STEP: u64 = 1 << 16;
    let mut runs: Vec<Run> = Vec::new();
    let mut mask = vec![false; STEP as usize];
    let mut a = 0;
    while a < upto {
        let len = (upto - a).min(STEP) as usize;
        members.mask(a, &mut mask[..len]);
        let mut i = 0;
        while i < len {
            if !mask[i] {
                i += 1;
                continue;
            }
            let first = i;
            while i < len && mask[i] {
                i += 1;
            }
            let (start, stop) = (a + first as u64, a + i as u64);
            match runs.last_mut() {
                Some(last) if last.1 == start => last.1 = stop,
                _ => runs.push((start, stop)),
            }
        }
        a += len as u64;
    }
    runs
}

/// `runs` without the errors of `without` (both sorted and disjoint): what
/// an action's reason is said of when some of its errors were not the
/// action's to change (a confirmed waive, `confirm::Locks`).
pub fn minus(runs: &[Run], without: &[Run]) -> Vec<Run> {
    let mut out = Vec::with_capacity(runs.len());
    let mut j = 0;
    for &(start, stop) in runs {
        let mut at = start;
        while j < without.len() && without[j].1 <= at {
            j += 1;
        }
        let mut k = j;
        while k < without.len() && without[k].0 < stop {
            if without[k].0 > at {
                out.push((at, without[k].0));
            }
            at = at.max(without[k].1);
            k += 1;
        }
        if at < stop {
            out.push((at, stop));
        }
    }
    out
}

/// Runs as a line says them: `gap,length` pairs.
pub fn encode(runs: &[Run]) -> String {
    let mut out = String::with_capacity(runs.len() * 4);
    let mut at = 0;
    for (i, &(start, stop)) in runs.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&(start - at).to_string());
        out.push(',');
        out.push_str(&(stop - start).to_string());
        at = stop;
    }
    out
}

/// `encode`'s runs, inside a rule of `size` errors; None for anything else.
/// Runs that touch are one.
fn decode(text: &str, size: u64) -> Option<Vec<Run>> {
    let mut runs: Vec<Run> = Vec::new();
    let mut at = 0u64;
    let mut numbers = text.split(',');
    while let Some(gap) = numbers.next() {
        let gap = number(gap)?;
        let length = number(numbers.next()?)?;
        let start = at.checked_add(gap)?;
        let stop = start.checked_add(length)?;
        if length == 0 || stop > size {
            return None;
        }
        match runs.last_mut() {
            Some(last) if last.1 == start => last.1 = stop,
            _ => runs.push((start, stop)),
        }
        at = stop;
    }
    (!runs.is_empty()).then_some(runs)
}

/// A plain decimal number (no sign, no blank).
fn number(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// The parts of `segs` inside `runs` (both sorted and disjoint), parts that
/// touch joined.
fn overlap(segs: &[Seg], runs: &[Run]) -> Vec<Run> {
    let mut out: Vec<Run> = Vec::new();
    for &(a, b) in runs {
        let mut i = segs.partition_point(|s| s.stop <= a);
        while let Some(s) = segs.get(i) {
            if s.start >= b {
                break;
            }
            let piece = (s.start.max(a), s.stop.min(b));
            match out.last_mut() {
                Some(last) if last.1 == piece.0 => last.1 = piece.1,
                _ => out.push(piece),
            }
            i += 1;
        }
    }
    out
}

/// `runs` (sorted, disjoint) taken out of `segs`, and given to `entry` when
/// there is one. Whether anything held was taken from what held it.
fn assign(segs: &mut Vec<Seg>, runs: &[Run], entry: Option<u32>) -> bool {
    let (Some(first), Some(last)) = (runs.first(), runs.last()) else {
        return false;
    };
    // the segments that may meet the runs
    let lo = segs.partition_point(|s| s.stop <= first.0);
    let hi = segs.partition_point(|s| s.start < last.1).max(lo);
    let mut kept: Vec<Seg> = Vec::new();
    let mut r = 0;
    for s in &segs[lo..hi] {
        let mut at = s.start;
        while r < runs.len() && runs[r].1 <= at {
            r += 1;
        }
        let mut j = r;
        while j < runs.len() && runs[j].0 < s.stop {
            if runs[j].0 > at {
                kept.push(Seg { start: at, stop: runs[j].0, entry: s.entry });
            }
            at = at.max(runs[j].1);
            if runs[j].1 >= s.stop {
                break;
            }
            j += 1;
        }
        if at < s.stop {
            kept.push(Seg { start: at, stop: s.stop, entry: s.entry });
        }
    }
    let held = |segs: &[Seg]| segs.iter().map(|s| s.stop - s.start).sum::<u64>();
    let taken = held(&kept) != held(&segs[lo..hi]);
    let merged = match entry {
        None => kept,
        Some(entry) => {
            let mut merged = Vec::with_capacity(kept.len() + runs.len());
            let (mut i, mut j) = (0, 0);
            while i < kept.len() || j < runs.len() {
                if j >= runs.len() || (i < kept.len() && kept[i].start < runs[j].0) {
                    merged.push(kept[i]);
                    i += 1;
                } else {
                    merged.push(Seg { start: runs[j].0, stop: runs[j].1, entry });
                    j += 1;
                }
            }
            merged
        }
    };
    segs.splice(lo..hi, merged);
    taken
}

/// What a file turned out to be.
enum Read {
    /// this result's: the bytes that are whole lines, the lines passed over
    Ours { whole: u64, skipped: u64 },
    /// not this result's, or not a file this build reads
    Other(&'static str),
}

/// A reviewer's reasons of one result (see the module docs).
pub struct Reasons {
    /// the file in use
    pub path: PathBuf,
    /// where the reasons go when `path`'s folder refuses them (once)
    other: Option<PathBuf>,
    /// the result: `floe_pack=`'s value
    tag: String,
    /// each rule's error count
    sizes: Vec<u64>,
    /// the actions read and made, in their order (some hold no error now)
    entries: Vec<Entry>,
    /// per kind and rule: sorted runs naming their action
    segs: [BTreeMap<usize, Vec<Seg>>; 6],
    /// the kept records (`Kind::kept`) per rule, in their order: the errors
    /// and the record
    kept: BTreeMap<(Kind, usize), Vec<(Vec<Run>, u32)>>,
    /// the file's first line (a comment: what the log is of)
    title: &'static str,
    /// the format version this log is written in (`floe_reasons=`); a file
    /// of an earlier one is read
    version: u32,
    /// the file read is of an earlier version: written again at the open
    outdated: bool,
    /// what `clear` dropped and the file does not say yet, per kind and rule
    cleared: BTreeMap<(Kind, usize), Vec<Run>>,
    /// the file as read says more than what is held: a line took errors
    /// from an earlier one, or took reasons back (what `tidy` asks before
    /// it weighs a rewrite - a log that only ever added is as short as it
    /// can be)
    superseded: bool,
    /// the file's bytes that are whole lines: where the next line goes
    written: u64,
    /// the file ends in a torn line, to be cut before the next one
    torn: bool,
    file: Option<File>,
    /// flush the lines to the disk (FLOE_WAIVE_SYNC, as the waive writes)
    sync: bool,
    /// lines written since the last flush
    unsynced: bool,
    /// what there was to say (the review hands them on)
    pub notices: Vec<String>,
}

impl Reasons {
    fn empty(path: PathBuf, other: Option<PathBuf>, tag: String, sizes: Vec<u64>, sync: bool) -> Self {
        Self {
            path,
            other,
            tag,
            sizes,
            entries: Vec::new(),
            segs: Default::default(),
            kept: BTreeMap::new(),
            title: "# floe drc review reasons",
            version: VERSION,
            outdated: false,
            cleared: BTreeMap::new(),
            superseded: false,
            written: 0,
            torn: false,
            file: None,
            sync,
            unsynced: false,
            notices: Vec::new(),
        }
    }

    /// The same store under another first line and version (the rejects
    /// log's).
    pub fn titled(mut self, title: &'static str, version: u32) -> Self {
        self.title = title;
        self.version = version;
        self
    }

    fn reset(&mut self) {
        self.entries.clear();
        self.segs = Default::default();
        self.kept.clear();
        self.outdated = false;
        self.cleared.clear();
        self.superseded = false;
        self.written = 0;
        self.torn = false;
    }

    /// A reviewer's reasons for writing: the file beside the waive sidecar
    /// (`paths[0]`), else the temp folder's (`paths[1]`), else none yet. A
    /// file of another result is moved aside as the reviewer's other files
    /// are; one that cannot be read is said and left.
    pub fn open(paths: [PathBuf; 2], tag: String, sizes: Vec<u64>, sync: bool) -> Self {
        Self::open_as("# floe drc review reasons", VERSION, paths, tag, sizes, sync)
    }

    /// `open`, of a log whose first line is `title` and whose version is
    /// `version` (the rejects log).
    pub fn open_as(
        title: &'static str,
        version: u32,
        paths: [PathBuf; 2],
        tag: String,
        sizes: Vec<u64>,
        sync: bool,
    ) -> Self {
        let [beside, temp] = paths;
        let mut me = Self::empty(beside.clone(), Some(temp.clone()), tag, sizes, sync).titled(title, version);
        for (i, path) in [beside, temp].into_iter().enumerate() {
            let bytes = match fs::read(&path) {
                Ok(bytes) => bytes,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    me.notices.push(format!(
                        "[drc] review reasons unreadable ({}): {e}",
                        path.display()
                    ));
                    continue;
                }
            };
            if i == 1 {
                me.path = path.clone();
                me.other = None;
            }
            match me.parse(&bytes) {
                Read::Ours { whole, skipped } => {
                    me.written = whole;
                    me.torn = whole != bytes.len() as u64;
                    if skipped > 0 {
                        me.notices.push(format!(
                            "[drc] review reasons: {skipped} line(s) of {} are not read",
                            path.display()
                        ));
                    }
                    me.tidy(bytes.len() as u64);
                }
                Read::Other(_) => {
                    me.reset();
                    let aside = aside_path(&path);
                    if fs::rename(&path, &aside).is_ok() {
                        me.notices.push(format!(
                            "[drc] review reasons do not match this pack (DRC re-run?); moved aside: {}",
                            aside.display()
                        ));
                    }
                }
            }
            break;
        }
        me
    }

    /// A reviewer's reasons - or rejects log - as their file says them,
    /// for reading alone (the statistics): None without a file, an error
    /// for a file of another result. Nothing is written, moved or tidied.
    pub fn load(path: &Path, tag: &str, sizes: &[u64]) -> Result<Option<Self>> {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let mut me = Self::empty(path.to_owned(), None, tag.to_owned(), sizes.to_vec(), false);
        // (either log's file: the rejects log's version is the later one)
        me.version = REJECTS_VERSION;
        match me.parse(&bytes) {
            Read::Ours { whole, .. } => {
                me.written = whole;
                Ok(Some(me))
            }
            Read::Other(why) => Err(Error::new(
                ErrorKind::Cache,
                format!("review reasons of {why} (not this pack's)"),
            )),
        }
    }

    /// Take a file's whole lines.
    fn parse(&mut self, bytes: &[u8]) -> Read {
        let whole = bytes.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
        let text = String::from_utf8_lossy(&bytes[..whole]);
        let (mut version, mut pack) = (false, false);
        let mut skipped = 0;
        for line in text.lines() {
            let line = line.trim_end_matches('\r');
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(value) = line.strip_prefix("floe_reasons=") {
                match value.trim().parse::<u32>().ok() {
                    Some(v) if v == self.version => {}
                    // (an earlier version's lines are this one's too)
                    Some(v) if v >= 1 && v < self.version => self.outdated = true,
                    _ => return Read::Other("another format version"),
                }
                version = true;
            } else if let Some(value) = line.strip_prefix("floe_pack=") {
                if value.trim() != self.tag {
                    return Read::Other("another DRC result");
                }
                pack = true;
            } else if !(version && pack) {
                return Read::Other("an unknown format");
            } else if self.record(line).is_none() {
                skipped += 1;
            }
        }
        Read::Ours { whole: whole as u64, skipped }
    }

    /// Take one action line; None for a line that is none.
    fn record(&mut self, line: &str) -> Option<()> {
        let mut parts = line.splitn(5, ' ');
        let tag = parts.next()?;
        let check = parts.next()?.parse::<usize>().ok()?;
        let time = number(parts.next()?)?;
        let runs = parts.next()?;
        let text = parts.next();
        let (kind, set) = Kind::ALL.into_iter().find_map(|kind| {
            let (with, gone) = kind.tags();
            if tag == with {
                Some((kind, true))
            } else if tag == gone {
                Some((kind, false))
            } else {
                None
            }
        })?;
        let runs = decode(runs, *self.sizes.get(check)?)?;
        if kind.kept() {
            if !set {
                return None;
            }
            // (an answered rejection: who made it, then why)
            let (by, text) = match kind {
                Kind::Answered => {
                    let rest = text?;
                    let (by, why) = rest.split_once(' ').unwrap_or((rest, ""));
                    (super::local::reviewer_of(by)?.to_owned(), why)
                }
                _ => (String::new(), text?),
            };
            let text = annotations::unescape(text).trim().to_owned();
            if text.is_empty() && !kind.bare() {
                return None;
            }
            self.entries.push(Entry { kind, check, time, text, by });
            let entry = self.entries.len() as u32 - 1;
            self.kept.entry((kind, check)).or_default().push((runs, entry));
            return Some(());
        }
        let entry = if set {
            let text = match text {
                Some(text) => annotations::unescape(text).trim().to_owned(),
                None if kind.bare() => String::new(),
                None => return None,
            };
            if text.is_empty() && !kind.bare() {
                return None;
            }
            self.entries.push(Entry { kind, check, time, text, by: String::new() });
            Some(self.entries.len() as u32 - 1)
        } else {
            None
        };
        let segs = self.segs[kind.slot()].entry(check).or_default();
        // (a line that takes reasons back is itself more than what is held)
        self.superseded |= assign(segs, &runs, entry) || !set;
        if segs.is_empty() {
            self.segs[kind.slot()].remove(&check);
        }
        Some(())
    }

    fn header(&self) -> String {
        format!("{}\nfloe_reasons={}\nfloe_pack={}\n", self.title, self.version, self.tag)
    }

    /// The actions that still hold errors with those errors, in their order.
    pub fn live(&self) -> Vec<(&Entry, Vec<Run>)> {
        let mut held: BTreeMap<u32, Vec<Run>> = BTreeMap::new();
        for rules in &self.segs {
            for segs in rules.values() {
                for s in segs {
                    held.entry(s.entry).or_default().push((s.start, s.stop));
                }
            }
        }
        held.into_iter()
            .map(|(entry, runs)| (&self.entries[entry as usize], runs))
            .collect()
    }

    /// The file that says what is held now - and what is kept - and
    /// nothing else, in the actions' order.
    fn serialize(&self) -> String {
        let mut lines: BTreeMap<u32, Vec<Vec<Run>>> = BTreeMap::new();
        for rules in &self.segs {
            for segs in rules.values() {
                for s in segs {
                    let runs = lines.entry(s.entry).or_insert_with(|| vec![Vec::new()]);
                    runs[0].push((s.start, s.stop));
                }
            }
        }
        for records in self.kept.values() {
            for (runs, entry) in records {
                lines.entry(*entry).or_default().push(runs.clone());
            }
        }
        let mut out = self.header();
        for (entry, runs) in lines {
            let entry = &self.entries[entry as usize];
            for runs in runs {
                out.push_str(&line_of(entry.kind, entry.check, entry.time, &runs, &entry.by, &entry.text));
            }
        }
        out
    }

    /// At the open: a file whose reasons are all gone is removed, and one
    /// much larger than what it still says is written again (the actions
    /// that hold nothing and the lines that took reasons back drop out).
    fn tidy(&mut self, len: u64) {
        if self.is_empty() {
            if fs::remove_file(&self.path).is_ok() {
                self.written = 0;
                self.torn = false;
            }
            return;
        }
        // (a file of an earlier version is this version's from now: a
        // build that reads the earlier one alone passes it over)
        if !self.outdated && (!self.superseded || len <= SLACK) {
            return;
        }
        let text = self.serialize();
        if (self.outdated || len > 2 * text.len() as u64 + SLACK) && write_whole(&self.path, text.as_bytes()).is_ok() {
            self.written = text.len() as u64;
            self.torn = false;
            self.superseded = false;
            self.outdated = false;
        }
    }

    /// Turn the flushes on or off (as the review's waive writes are).
    pub fn set_sync(&mut self, on: bool) {
        self.sync = on;
    }

    /// What `clear` dropped since it was last said: one line a kind and
    /// rule, whatever number of chunks a job dropped it in. A file that
    /// cannot say it is not asked again - the reasons are gone from memory,
    /// and the readers ask the status besides.
    fn say_cleared(&mut self, time: u64) -> Result<()> {
        while let Some(((kind, check), mut pieces)) = self.cleared.pop_first() {
            pieces.sort_unstable();
            let mut runs: Vec<Run> = Vec::with_capacity(pieces.len());
            for (a, b) in pieces {
                match runs.last_mut() {
                    Some(last) if last.1 >= a => last.1 = last.1.max(b),
                    _ => runs.push((a, b)),
                }
            }
            let line = format!("{} {check} {time} {}\n", kind.tags().1, encode(&runs));
            if let Err(e) = self.append(&line) {
                self.cleared.clear();
                return Err(e);
            }
        }
        Ok(())
    }

    /// The end of an action or a job: what it took back is said (`time`,
    /// when), and the lines written since the last flush reach the disk.
    pub fn flush(&mut self, time: u64) -> Result<()> {
        self.say_cleared(time)?;
        if !std::mem::take(&mut self.unsynced) || !self.sync {
            return Ok(());
        }
        let Some(file) = &self.file else {
            return Ok(());
        };
        match file.sync_data() {
            // a file system without the call writes back in its own time
            Err(e)
                if !matches!(
                    e.kind(),
                    std::io::ErrorKind::Unsupported | std::io::ErrorKind::InvalidInput
                ) =>
            {
                Err(e.into())
            }
            _ => Ok(()),
        }
    }

    /// Write one line at the file's end, the header before the first.
    fn write(&mut self, line: &str) -> std::io::Result<()> {
        if self.file.is_none() {
            let file = OpenOptions::new().append(true).create(true).open(&self.path)?;
            let len = file.metadata()?.len();
            if self.torn && len > self.written {
                file.set_len(self.written)?;
            }
            self.torn = false;
            if len == 0 {
                self.written = 0;
            }
            self.file = Some(file);
        }
        let mut data = String::new();
        if self.written == 0 {
            data.push_str(&self.header());
        }
        data.push_str(line);
        let mut file = self.file.as_ref().expect("opened");
        match file.write_all(data.as_bytes()) {
            Ok(()) => {
                self.written += data.len() as u64;
                self.unsynced = true;
                Ok(())
            }
            Err(e) => {
                // what reached the file of this line is cut before the next
                self.file = None;
                self.torn = true;
                Err(e)
            }
        }
    }

    /// `write`, and where the folder refuses it, once: everything held and
    /// this line to the other place (the temp folder), which is the file
    /// from then on.
    fn append(&mut self, line: &str) -> Result<()> {
        let Err(first) = self.write(line) else {
            return Ok(());
        };
        let Some(other) = self.other.take() else {
            return Err(first.into());
        };
        let text = self.serialize() + line;
        write_whole(&other, text.as_bytes()).map_err(|e| {
            Error::new(
                ErrorKind::Io,
                format!("{}: {first}; {}: {e}", self.path.display(), other.display()),
            )
        })?;
        self.notices.push(format!(
            "[drc] results folder not writable; review reasons in {}",
            other.display()
        ));
        self.path = other;
        self.file = None;
        self.torn = false;
        self.unsynced = false;
        self.written = text.len() as u64;
        Ok(())
    }

    fn size(&self, check: usize) -> Result<u64> {
        self.sizes
            .get(check)
            .copied()
            .ok_or_else(|| Error::input("DRC check index out of range"))
    }

    fn inside(&self, check: usize, runs: &[Run]) -> Result<()> {
        let size = self.size(check)?;
        let ordered = runs.windows(2).all(|w| w[0].1 < w[1].0);
        if !ordered || runs.iter().any(|r| r.0 >= r.1) || runs.last().is_some_and(|r| r.1 > size) {
            return Err(Error::input("review reason runs outside their rule"));
        }
        Ok(())
    }

    /// One action's reason on rule `check`'s `runs` (sorted, disjoint, not
    /// touching): written, then held. An empty reason or no run is nothing -
    /// but a rejection, which is an action without a reason too.
    pub fn set(&mut self, kind: Kind, check: usize, runs: &[Run], text: &str, time: u64) -> Result<()> {
        if kind.kept() {
            return Err(Error::input("a kept record is kept, not set"));
        }
        let text = clean(text)?;
        self.inside(check, runs)?;
        if (text.is_empty() && !kind.bare()) || runs.is_empty() {
            return Ok(());
        }
        // (what was taken back before this is said before it: a later line
        // wins, and this reason must not be taken back by an earlier drop)
        self.say_cleared(time)?;
        let line = line_of(kind, check, time, runs, "", &text);
        self.append(&line)?;
        self.entries.push(Entry { kind, check, time, text, by: String::new() });
        let entry = self.entries.len() as u32 - 1;
        assign(self.segs[kind.slot()].entry(check).or_default(), runs, Some(entry));
        // (an action's reason is on the disk when the action answers)
        self.flush(time)
    }

    /// Keep a record of rule `check`'s `runs` (sorted, disjoint, not
    /// touching) as it was: `Kind::Answered` - `by` rejected their waive at
    /// `time`, why `text` (it may be empty), and this log's reviewer has
    /// waived them again - or `Kind::Earlier` - the waive reason `text`,
    /// given at `time`, that they had before. A line of the log; flushed by
    /// the `flush` its action ends with. Nothing is taken from an earlier
    /// record. No run, or no reason to keep, is nothing.
    pub fn keep(&mut self, kind: Kind, check: usize, runs: &[Run], time: u64, by: &str, text: &str) -> Result<()> {
        if !kind.kept() || (kind == Kind::Answered) != super::local::reviewer_of(by).is_some() {
            return Err(Error::input("not a record to keep"));
        }
        let text = clean(text)?;
        self.inside(check, runs)?;
        if (text.is_empty() && !kind.bare()) || runs.is_empty() {
            return Ok(());
        }
        let line = line_of(kind, check, time, runs, by, &text);
        self.append(&line)?;
        self.entries.push(Entry { kind, check, time, text, by: by.to_owned() });
        let entry = self.entries.len() as u32 - 1;
        self.kept.entry((kind, check)).or_default().push((runs.to_vec(), entry));
        Ok(())
    }

    /// The records of `kind` kept for rule `check`, in their order: the
    /// errors and the record.
    pub fn kept(&self, kind: Kind, check: usize) -> impl Iterator<Item = (&[Run], &Entry)> {
        self.kept
            .get(&(kind, check))
            .into_iter()
            .flatten()
            .map(|(runs, entry)| (runs.as_slice(), &self.entries[*entry as usize]))
    }

    /// The records of `kind` kept for rule `check`'s error `error`, in
    /// their order.
    pub fn kept_of(&self, kind: Kind, check: usize, error: u64) -> Vec<&Entry> {
        self.kept(kind, check)
            .filter(|(runs, _)| {
                let i = runs.partition_point(|r| r.1 <= error);
                runs.get(i).is_some_and(|r| r.0 <= error)
            })
            .map(|(_, entry)| entry)
            .collect()
    }

    /// Everything kept of rule `check`'s error `error`, in the order it
    /// was kept - which is the order things happened in: at each waiving
    /// again, the reason the waive had, then the rejection it answers.
    pub fn kept_at(&self, check: usize, error: u64) -> Vec<&Entry> {
        let mut found: Vec<u32> = Vec::new();
        for kind in [Kind::Answered, Kind::Earlier] {
            for (runs, entry) in self.kept.get(&(kind, check)).into_iter().flatten() {
                let i = runs.partition_point(|r| r.1 <= error);
                if runs.get(i).is_some_and(|r| r.0 <= error) {
                    found.push(*entry);
                }
            }
        }
        found.sort_unstable();
        found.into_iter().map(|entry| &self.entries[entry as usize]).collect()
    }

    /// The rules that have a kept record of `kind`.
    pub fn kept_checks(&self, kind: Kind) -> Vec<usize> {
        self.kept.keys().filter(|(k, _)| *k == kind).map(|(_, check)| *check).collect()
    }

    /// Rule `check`'s errors whose rejection by `by` at `time` this log's
    /// reviewer has answered (sorted, joined where they meet).
    pub fn answered(&self, check: usize, by: &str, time: u64) -> Vec<Run> {
        let mut all: Vec<Run> = self
            .kept(Kind::Answered, check)
            .filter(|(_, entry)| entry.time == time && entry.by == by)
            .flat_map(|(runs, _)| runs.iter().copied())
            .collect();
        all.sort_unstable();
        let mut joined: Vec<Run> = Vec::with_capacity(all.len());
        for (a, b) in all {
            match joined.last_mut() {
                Some(last) if a <= last.1 => last.1 = last.1.max(b),
                _ => joined.push((a, b)),
            }
        }
        joined
    }

    /// The latest time of an action of `kind` this log has read or made
    /// (0: none) - a confirmer's next rejection is given a later one, so
    /// that each is named by its time.
    pub fn latest(&self, kind: Kind) -> u64 {
        self.entries.iter().filter(|e| e.kind == kind).map(|e| e.time).max().unwrap_or(0)
    }

    /// The reasons of rule `check`'s `runs` are gone (the errors were turned
    /// back, or changed by something that gave no reason): dropped at once
    /// - only what was held, so a rule without reasons costs nothing - and
    /// said in the file by the `flush` the caller's action or job ends with
    /// (a job takes reasons back a chunk of statuses at a time: one line
    /// for all of it). Whether anything was held.
    pub fn clear(&mut self, kind: Kind, check: usize, runs: &[Run]) -> bool {
        let Some(segs) = self.segs[kind.slot()].get_mut(&check) else {
            return false;
        };
        let pieces = overlap(segs, runs);
        if pieces.is_empty() {
            return false;
        }
        assign(segs, &pieces, None);
        if segs.is_empty() {
            self.segs[kind.slot()].remove(&check);
        }
        self.cleared.entry((kind, check)).or_default().extend(pieces);
        true
    }

    /// Whether rule `check` holds any reason of `kind`.
    pub fn any(&self, kind: Kind, check: usize) -> bool {
        self.segs[kind.slot()].contains_key(&check)
    }

    /// The rules that hold a reason of `kind`.
    pub fn checks(&self, kind: Kind) -> Vec<usize> {
        self.segs[kind.slot()].keys().copied().collect()
    }

    /// Whether anything is held - or kept - at all.
    pub fn is_empty(&self) -> bool {
        self.segs.iter().all(|rules| rules.is_empty()) && self.kept.is_empty()
    }

    /// The reason held for rule `check`'s error `error`.
    pub fn of(&self, kind: Kind, check: usize, error: u64) -> Option<&Entry> {
        let segs = self.segs[kind.slot()].get(&check)?;
        let i = segs.partition_point(|s| s.stop <= error);
        segs.get(i)
            .filter(|s| s.start <= error)
            .map(|s| &self.entries[s.entry as usize])
    }

    /// The parts of `runs` that hold a reason.
    pub fn held(&self, kind: Kind, check: usize, runs: &[Run]) -> Vec<Run> {
        self.segs[kind.slot()]
            .get(&check)
            .map_or_else(Vec::new, |segs| overlap(segs, runs))
    }

    /// The parts of `runs` that hold a reason, each with its action's index
    /// in `entries` (in order; parts of one action are not joined).
    pub fn parts(&self, kind: Kind, check: usize, runs: &[Run]) -> Vec<(u64, u64, usize)> {
        let Some(segs) = self.segs[kind.slot()].get(&check) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for &(a, b) in runs {
            let mut i = segs.partition_point(|s| s.stop <= a);
            while let Some(s) = segs.get(i) {
                if s.start >= b {
                    break;
                }
                out.push((s.start.max(a), s.stop.min(b), s.entry as usize));
                i += 1;
            }
        }
        out
    }

    /// The one reason every error of `runs` holds, when there is one.
    pub fn common(&self, kind: Kind, check: usize, runs: &[Run]) -> Option<&str> {
        let segs = self.segs[kind.slot()].get(&check)?;
        let mut text: Option<&str> = None;
        for &(a, b) in runs {
            let mut i = segs.partition_point(|s| s.stop <= a);
            let mut at = a;
            while at < b {
                let s = segs.get(i).filter(|s| s.start <= at)?;
                let this = self.entries[s.entry as usize].text.as_str();
                if *text.get_or_insert(this) != this {
                    return None;
                }
                at = s.stop;
                i += 1;
            }
        }
        text
    }

    /// The reasons of `kind` given so far, each once, the latest first.
    pub fn texts(&self, kind: Kind, limit: usize) -> Vec<&str> {
        let mut seen = HashSet::new();
        self.entries
            .iter()
            .rev()
            .filter(|e| e.kind == kind && !e.text.is_empty())
            .map(|e| e.text.as_str())
            .filter(|text| seen.insert(*text))
            .take(limit)
            .collect()
    }

    /// The actions as read and made (an index of `runs`).
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Rule `check`'s runs of `kind` in order, each with its action's index
    /// in `entries`.
    pub fn runs(&self, kind: Kind, check: usize) -> Vec<(u64, u64, usize)> {
        self.segs[kind.slot()].get(&check).map_or_else(Vec::new, |segs| {
            segs.iter().map(|s| (s.start, s.stop, s.entry as usize)).collect()
        })
    }
}

/// An action's line: `<tag> <rule> <time> <runs> [<by>] <text>`, the text
/// left out when there is none (a rejection without a reason); `by` - who
/// rejected - is an answered rejection's alone.
fn line_of(kind: Kind, check: usize, time: u64, runs: &[Run], by: &str, text: &str) -> String {
    let mut line = format!("{} {check} {time} {}", kind.tags().0, encode(runs));
    if !by.is_empty() {
        line.push(' ');
        line.push_str(by);
    }
    if !text.is_empty() {
        line.push(' ');
        line.push_str(&annotations::escape(text));
    }
    line.push('\n');
    line
}

/// Write a whole file through a temporary beside it: flushed, then renamed.
fn write_whole(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".tmp-{}", std::process::id()));
    let tmp = PathBuf::from(name);
    let result = (|| {
        let mut f = File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests;
