//! The team's counts, kept: the union of every reviewer's review of one DRC
//! result - visited, waived, not for review, conflicts, pending, whole and by
//! rule, as `stats::collect` counts them - answered at once (user
//! 2026-11-13: ".db를 여러 사람이 검토하고 그 합집합을 실시간으로 받아야 해").
//!
//! The team's numbers are no sum of the reviewers' own: an error two people
//! waived is one, and one two visitors judged differently is a conflict. They
//! need each error's bits of every reviewer, which is what makes the exact
//! report cost a status byte an error a reviewer. What is kept here makes the
//! answer cost what CHANGED:
//!
//! - `.<db>.team` beside the pack holds the team's counts a rule and a chunk
//!   (4,096 errors; only chunks that count anything), and for each reviewer
//!   how far their change log was read and how their files stood then;
//! - `merge` reads what each reviewer's change log (`drc::changes`) says
//!   since, counts those chunks again from every reviewer's bits, and puts
//!   the differences into the rules' totals. With nothing changed it reads
//!   the file, a few file times and nothing else: no lock, no write.
//!
//! Nothing here is shared between writers: a reviewer's session appends to
//! its own log, and `merge` - whoever runs it: the command that answers, a
//! viewer after a job that changed many errors - writes the whole file anew
//! and renames it into place. Two merges at once are each right for the logs
//! as they read them; the one that lands last stands, and what its cursors
//! did not reach is read by the next. A lock keeps a second merge from
//! doing a long count twice; where no lock can be had both run.
//!
//! The order of the reads is what makes a record enough. A session puts its
//! bits on the disk, then says so in its log, with the files' times as they
//! stood (STAMP). A merge reads a reviewer's log first and opens their files
//! after - an open asks the server, so what the records it read say is in
//! what it reads - and asks the files' times before their bytes. `judge`
//! then says which of four things holds for a reviewer:
//!
//! - *said*: the files are as the log's last word left them (or nothing
//!   changed at all): the chunks said are counted, and the counts are theirs;
//! - *ahead*: the files are newer than the last word - a job still going, a
//!   click between its bits and its record: what was said is counted, the
//!   rest waits for its record (`current` is false meanwhile);
//! - *unsaid*: the files changed and no record says how, or the log says more
//!   than the files show yet: nothing is taken; a record, or the files, are
//!   waited for a short grace (FLOE_TEAM_GRACE_S, 10 s);
//! - *again*: the reviewer's rules are counted whole - they are new to the
//!   counts, their log is another one or says RESET, a file of theirs came or
//!   went, they have no log to wait for (a build before 0.12.465), or the
//!   grace ended.
//!
//! So the logs make the answer cheap; they are never what makes it right.
use super::bits::{Bits, CHUNK};
use super::stats::{
    discover, flags_connection, rejects_log, run_bits, team_of, waive_of, waived_bits, Found, Planes, Team,
};
use super::Identity;
use crate::drc::changes::{self, stamp_of, stamps_after, Stamps, RESET, TOUCH};
use crate::drc::confirm;
use crate::drc::reasons::{Kind, Reasons};
use crate::{check_cancelled, Error, ErrorKind, Result};
use floe_vfs::lock::{self, Busy, Key, Mode};
use rusqlite::{Connection, OptionalExtension};
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Write;
use std::os::unix::fs::{FileExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime};

const MAGIC: &[u8; 8] = b"FLOETEAM";
/// 2 (0.12.466): the waives anybody made and the confirmed ones of them are
/// counted too, and a reviewer's confirm file is one of their files.
/// 3 (0.12.468): the rejected errors are counted, and a reviewer's rejects
/// log is one of their files.
/// 4 (0.12.469): a rejection is of each reviewer's waive until they answer
/// it - the rejected count is the errors whose waives are all rejected.
const VERSION: u32 = 4;
/// How long a reviewer's files may differ from what their log says before
/// their rules are counted again (FLOE_TEAM_GRACE_S; seconds, 0: at once).
const GRACE: u64 = 10;
/// How long a merge waits for another one to finish.
const WAIT: Duration = Duration::from_secs(3);
/// How long a merge waits, once, for the record of bits it already sees.
const RECORD_WAIT: Duration = Duration::from_millis(40);
/// A temporary of a merge that did not end is removed after this long.
const LEFTOVER: Duration = Duration::from_secs(600);

/// The counts kept of a chunk and of a rule.
const KEPT: usize = 8;

/// A chunk's team counts: visited, waived, not for review, conflicts, done,
/// waived by anybody, confirmed, rejected.
type Counts = [u16; KEPT];

fn counts_of(t: &Team) -> Counts {
    [
        t.visited as u16,
        t.waived as u16,
        t.not_required as u16,
        t.conflicts as u16,
        t.done as u16,
        t.waived_any as u16,
        t.confirmed as u16,
        t.rejected as u16,
    ]
}

/// What is kept of one reviewer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Seen {
    /// their change log (0: none) and how far it was read
    log_id: u64,
    cursor: u64,
    /// the times the log has said as far as it was read (0: none)
    log_stamp: Stamps,
    /// their files' times (waive, flags, confirm, rejects) the kept
    /// counts cover
    stamps: Stamps,
    /// since when the files are not as the log says (0: they are)
    suspect: u64,
    /// bit 0: a waive file that reads, bit 1: a flags file that reads,
    /// bit 2: a confirm file that reads, bit 3: a rejects log that reads
    known: u8,
    /// the rules they may have bits in
    active: BTreeSet<u32>,
}

/// The file `.<db>.team` (see the module docs).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Cache {
    /// per rule: the counts
    rules: Vec<[u64; KEPT]>,
    reviewers: BTreeMap<String, Seen>,
    /// (rule, chunk) -> counts, for the chunks that count anything
    chunks: BTreeMap<(u32, u32), Counts>,
}

fn put32(out: &mut Vec<u8>, v: u32) {
    out.extend(v.to_le_bytes());
}
fn put64(out: &mut Vec<u8>, v: u64) {
    out.extend(v.to_le_bytes());
}

/// A reader over bytes that says None past their end.
struct Bytes<'a>(&'a [u8]);
impl<'a> Bytes<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.0.len() < n {
            return None;
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Some(head)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }
    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|b| u16::from_le_bytes(b.try_into().unwrap()))
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    }
    fn u64(&mut self) -> Option<u64> {
        self.take(8).map(|b| u64::from_le_bytes(b.try_into().unwrap()))
    }
}

impl Cache {
    fn empty(rules: usize) -> Self {
        Self {
            rules: vec![[0; KEPT]; rules],
            ..Self::default()
        }
    }

    /// The file's bytes: a head (the rules' counts, the reviewers) and the
    /// chunks, each closed by its CRC-32.
    fn bytes(&self, content: &str) -> Vec<u8> {
        let mut out = Vec::with_capacity(64 + self.rules.len() * 8 * KEPT + self.chunks.len() * (8 + 2 * KEPT));
        out.extend(MAGIC);
        put32(&mut out, VERSION);
        put32(&mut out, self.rules.len() as u32);
        put32(&mut out, content.len() as u32);
        out.extend(content.as_bytes());
        put32(&mut out, self.reviewers.len() as u32);
        for counts in &self.rules {
            for &c in counts {
                put64(&mut out, c);
            }
        }
        for (tag, s) in &self.reviewers {
            out.extend((tag.len() as u16).to_le_bytes());
            out.extend(tag.as_bytes());
            for v in [s.log_id, s.cursor].into_iter().chain(s.log_stamp).chain(s.stamps).chain([s.suspect]) {
                put64(&mut out, v);
            }
            out.push(s.known);
            put32(&mut out, s.active.len() as u32);
            for &rule in &s.active {
                put32(&mut out, rule);
            }
        }
        put64(&mut out, self.chunks.len() as u64);
        let crc = crc32fast::hash(&out);
        put32(&mut out, crc);
        let at = out.len();
        for (&(rule, chunk), counts) in &self.chunks {
            put32(&mut out, rule);
            put32(&mut out, chunk);
            for &c in counts {
                out.extend(c.to_le_bytes());
            }
        }
        let crc = crc32fast::hash(&out[at..]);
        put32(&mut out, crc);
        out
    }

    /// The cache `bytes` hold, when it is this result's (`content`, `rules`)
    /// and whole - both sums right, no count past a chunk, every rule's
    /// total the sum of its chunks; anything else is no cache.
    fn of(bytes: &[u8], content: &str, rules: usize) -> Option<Self> {
        let mut b = Bytes(bytes);
        if b.take(8)? != MAGIC || b.u32()? != VERSION || b.u32()? as usize != rules {
            return None;
        }
        let n = b.u32()? as usize;
        if b.take(n)? != content.as_bytes() {
            return None;
        }
        let reviewers = b.u32()?;
        let mut me = Self::empty(rules);
        for counts in &mut me.rules {
            for c in counts {
                *c = b.u64()?;
            }
        }
        for _ in 0..reviewers {
            let n = b.u16()? as usize;
            let tag = String::from_utf8(b.take(n)?.to_vec()).ok()?;
            let mut s = Seen {
                log_id: b.u64()?,
                cursor: b.u64()?,
                log_stamp: [b.u64()?, b.u64()?, b.u64()?, b.u64()?],
                stamps: [b.u64()?, b.u64()?, b.u64()?, b.u64()?],
                suspect: b.u64()?,
                known: b.u8()?,
                active: BTreeSet::new(),
            };
            for _ in 0..b.u32()? {
                s.active.insert(b.u32()?);
            }
            me.reviewers.insert(tag, s);
        }
        let chunks = b.u64()?;
        let head = bytes.len() - b.0.len();
        if b.u32()? != crc32fast::hash(&bytes[..head]) {
            return None;
        }
        let body = usize::try_from(chunks).ok()?.checked_mul(8 + 2 * KEPT)?;
        let raw = b.take(body)?;
        if b.u32()? != crc32fast::hash(raw) || !b.0.is_empty() {
            return None;
        }
        let mut c = Bytes(raw);
        let mut sums = vec![[0u64; KEPT]; rules];
        for _ in 0..chunks {
            let key = (c.u32()?, c.u32()?);
            let mut counts = [0u16; KEPT];
            for v in &mut counts {
                *v = c.u16()?;
            }
            let sum = sums.get_mut(key.0 as usize)?;
            for i in 0..KEPT {
                if u64::from(counts[i]) > CHUNK {
                    return None;
                }
                sum[i] += u64::from(counts[i]);
            }
            if counts == [0; KEPT] || me.chunks.insert(key, counts).is_some() {
                return None;
            }
        }
        (sums == me.rules).then_some(me)
    }

    /// A chunk's counts become `new`: the rule's totals follow.
    fn set(&mut self, rule: usize, chunk: u64, new: Counts) {
        let key = (rule as u32, chunk as u32);
        let old = if new == [0; KEPT] {
            self.chunks.remove(&key)
        } else {
            self.chunks.insert(key, new)
        }
        .unwrap_or([0; KEPT]);
        for i in 0..KEPT {
            self.rules[rule][i] = (self.rules[rule][i] + u64::from(new[i])).saturating_sub(u64::from(old[i]));
        }
    }

    /// The chunks of `rule` that are kept now.
    fn kept(&self, rule: usize) -> Vec<u64> {
        let rule = rule as u32;
        self.chunks
            .range((rule, 0)..=(rule, u32::MAX))
            .map(|(&(_, chunk), _)| u64::from(chunk))
            .collect()
    }

    fn teams(&self) -> Vec<Team> {
        self.rules
            .iter()
            .map(|c| Team {
                visited: c[0],
                waived: c[1],
                not_required: c[2],
                conflicts: c[3],
                done: c[4],
                waived_any: c[5],
                confirmed: c[6],
                rejected: c[7],
            })
            .collect()
    }
}

/// The cache's place: beside the pack, by the database's name.
pub fn path_of(pack: &Path) -> Result<PathBuf> {
    let database = crate::cache::database_path(pack)?;
    let name = database.file_name().unwrap_or_default().to_string_lossy();
    Ok(database.with_file_name(format!(".{name}.team")))
}

/// What the kept counts say after a merge.
#[derive(Clone, Debug)]
pub struct Kept {
    /// per rule
    pub rules: Vec<Team>,
    /// the reviewers the counts are of
    pub reviewers: usize,
    /// the counts cover every reviewer's files as they stand
    pub current: bool,
    /// why not, when they do not: a line each
    pub waiting: Vec<String>,
    /// the files left out of the counts - another result's, ones that cannot
    /// be read: reviewer, file, why (as the exact report leaves them out)
    pub skipped: Vec<(String, PathBuf, String)>,
    /// the counts are kept in the file (false: the folder would not take it,
    /// and they were counted for this answer alone)
    pub stored: bool,
    /// the chunks (4,096 errors) counted again for this answer
    pub counted: u64,
}

/// What a reviewer's change log and files say against what is kept of them
/// (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    Said,
    Ahead,
    Unsaid,
    Again,
}

/// A file of `a` is older than `b` says.
fn older(a: Stamps, b: Stamps) -> bool {
    a.iter().zip(b).any(|(&a, b)| a < b)
}

/// `kept`: what the counts hold of the reviewer (None: nothing yet); `tail`:
/// their log read from where the counts left it (None: no log of this
/// result); `known` and `stamps`: their files as they open now - opened
/// after the log was read.
fn judge(kept: Option<&Seen>, tail: Option<&changes::Tail>, known: u8, stamps: Stamps) -> Verdict {
    let Some(kept) = kept else {
        return Verdict::Again; // new to the counts
    };
    if kept.known != known {
        return Verdict::Again; // a file of theirs came, went, or reads now
    }
    let Some(tail) = tail else {
        // no log: nothing to wait for (a build before 0.12.465)
        return if kept.log_id == 0 && kept.stamps == stamps {
            Verdict::Said
        } else {
            Verdict::Again
        };
    };
    if kept.log_id != tail.log_id || tail.records.iter().any(|r| r.kind == RESET) {
        return Verdict::Again; // their first log, another one, or one that asks
    }
    // the counts were made of files older than the log had said by then:
    // which of its records they hold cannot be told, so any change of the
    // files counts again
    let behind = older(kept.stamps, kept.log_stamp);
    if tail.records.is_empty() {
        return if stamps == kept.stamps {
            Verdict::Said
        } else if behind {
            Verdict::Again
        } else {
            Verdict::Unsaid
        };
    }
    if behind {
        return Verdict::Again;
    }
    let said = stamps_after(kept.log_stamp, &tail.records);
    if stamps == said {
        Verdict::Said
    } else if older(stamps, said) {
        Verdict::Unsaid // the log says more than the files show yet
    } else {
        Verdict::Ahead
    }
}

/// One reviewer's files as a merge reads them: opened by `load`, after the
/// reviewer's change log was read.
struct Source {
    tag: String,
    waive_path: Option<PathBuf>,
    flags_path: Option<PathBuf>,
    confirm_path: Option<PathBuf>,
    rejects_path: Option<PathBuf>,
    log: Option<PathBuf>,
    /// the waive sidecar and its stored counter a rule
    waive: Option<(File, Vec<u32>)>,
    /// the confirm file and its stored counter a rule
    confirm: Option<(File, Vec<u32>)>,
    /// the rejects log as read: the rejections this reviewer made, and the
    /// ones of their own waives they answered
    rejects: Option<Reasons>,
    /// the flags read (bit 1 of `known`); their connection is opened when a
    /// plane is first asked
    flags_ok: bool,
    flags: Option<Connection>,
    /// a read transaction is open on the flags (ended every few chunks: a
    /// reader's lock held for a long count would keep the reviewer's own
    /// writes waiting)
    reading: bool,
    /// the files' times (waive, flags, confirm, rejects), asked before
    /// their bytes
    stamps: Stamps,
    /// the files left out: which, and why
    skipped: Vec<(PathBuf, String)>,
    /// the flags are held by their writer past the wait
    busy: Option<String>,
}

impl Source {
    fn new(tag: &str, files: &Found) -> Self {
        let waive_path = files.waive.as_ref().map(|(path, _)| path.clone());
        Self {
            tag: tag.to_owned(),
            log: waive_path.as_deref().map(changes::path_beside),
            waive_path,
            flags_path: files.review.as_ref().map(|(path, _)| path.clone()),
            confirm_path: files.confirm.as_ref().map(|(path, _)| path.clone()),
            rejects_path: files.rejects.as_ref().map(|(path, _)| path.clone()),
            waive: None,
            confirm: None,
            rejects: None,
            flags_ok: false,
            flags: None,
            reading: false,
            stamps: [0; 4],
            skipped: Vec::new(),
            busy: None,
        }
    }

    /// Open the files as they are now. A file is opened to ask its time: an
    /// open asks the server, a stat by name may answer from a cache; and the
    /// time is asked before the bytes, so a file at least as new as a record
    /// says holds what the record says. With `trust`, flags that read at the
    /// last merge and have the time they had then are taken to read still -
    /// their connection is opened when a plane is asked.
    fn load(&mut self, id: &Identity, kept: Option<&Seen>, trust: bool) {
        self.release();
        (self.waive, self.confirm, self.flags, self.flags_ok) = (None, None, None, false);
        (self.stamps, self.busy, self.rejects) = ([0; 4], None, None);
        self.skipped.clear();
        if let Some(path) = &self.rejects_path {
            // (a log of actions, read whole: a line an action)
            let stamp = File::open(path).and_then(|file| Ok(stamp_of(&file.metadata()?)));
            match stamp.map_err(Error::from).and_then(|stamp| Ok((rejects_log(path, id)?, stamp))) {
                Ok((Some(log), stamp)) => {
                    self.rejects = Some(log);
                    self.stamps[3] = stamp;
                }
                // (gone between its time and its bytes: no file)
                Ok((None, _)) => (),
                Err(e) => self.skipped.push((path.clone(), e.message)),
            }
        }
        if let Some(path) = &self.confirm_path {
            let opened = File::open(path).and_then(|file| {
                let stamp = stamp_of(&file.metadata()?);
                Ok((file, stamp))
            });
            let header = confirm::header_of(&id.waive_header);
            match opened
                .map_err(Error::from)
                .and_then(|(file, stamp)| Ok((confirm::read_of(file, &header, id.total(), id.sizes.len())?, stamp)))
            {
                Ok((confirm, stamp)) => {
                    self.confirm = Some(confirm);
                    self.stamps[2] = stamp;
                }
                Err(e) => self.skipped.push((path.clone(), e.message)),
            }
        }
        if let Some(path) = &self.waive_path {
            let opened = File::open(path).and_then(|file| {
                let stamp = stamp_of(&file.metadata()?);
                Ok((file, stamp))
            });
            match opened
                .map_err(Error::from)
                .and_then(|(file, stamp)| Ok((waive_of(file, id)?, stamp)))
            {
                Ok((waive, stamp)) => {
                    self.waive = Some(waive);
                    self.stamps[0] = stamp;
                }
                Err(e) => self.skipped.push((path.clone(), e.message)),
            }
        }
        if let Some(path) = &self.flags_path {
            match File::open(path).and_then(|file| file.metadata()) {
                Ok(meta) => {
                    let stamp = stamp_of(&meta);
                    if trust && kept.is_some_and(|k| k.known & 2 != 0 && k.stamps[1] == stamp) {
                        (self.flags_ok, self.stamps[1]) = (true, stamp);
                    } else {
                        match flags_connection(path, id) {
                            Ok(conn) => {
                                self.flags = Some(conn);
                                (self.flags_ok, self.stamps[1]) = (true, stamp);
                            }
                            Err(e) if e.kind == ErrorKind::Busy => self.busy = Some(e.message),
                            Err(e) => self.skipped.push((path.clone(), e.message)),
                        }
                    }
                }
                Err(e) => self.skipped.push((path.clone(), e.to_string())),
            }
        }
    }

    /// Bit 0: a waive file that reads; bit 1: flags that read; bit 2: a
    /// confirm file that reads; bit 3: a rejects log that reads.
    fn known(&self) -> u8 {
        u8::from(self.waive.is_some())
            | u8::from(self.flags_ok) << 1
            | u8::from(self.confirm.is_some()) << 2
            | u8::from(self.rejects.is_some()) << 3
    }

    fn named(&self, e: Error) -> Error {
        Error::new(e.kind, format!("{}: {}", self.tag, e.message))
    }

    /// The flags' connection inside a read transaction (None: no flags).
    fn flags(&mut self, id: &Identity) -> Result<Option<&Connection>> {
        if !self.flags_ok {
            return Ok(None);
        }
        if self.flags.is_none() {
            let path = self.flags_path.as_deref().expect("flags that read have a path");
            self.flags = Some(flags_connection(path, id).map_err(|e| self.named(e))?);
        }
        let conn = self.flags.as_ref().expect("opened");
        if !self.reading {
            conn.execute_batch("BEGIN")
                .map_err(|e| self.named(super::flags::sql(e)))?;
            self.reading = true;
        }
        Ok(self.flags.as_ref())
    }

    /// End the read transaction (the next read starts another).
    fn release(&mut self) {
        if std::mem::take(&mut self.reading) {
            if let Some(conn) = &self.flags {
                let _ = conn.execute_batch("COMMIT");
            }
        }
    }

    /// The rules this reviewer has anything in: the ones their waive counter
    /// says, and the ones their flags have a row of (a seek a rule).
    fn active(&mut self, id: &Identity) -> Result<BTreeSet<u32>> {
        let mut rules = BTreeSet::new();
        for (_, stored) in self.waive.iter().chain(&self.confirm) {
            rules.extend((0..stored.len()).filter(|&ci| stored[ci] > 0).map(|ci| ci as u32));
        }
        if let Some(log) = &self.rejects {
            rules.extend(log.checks(Kind::Reject).into_iter().map(|ci| ci as u32));
            rules.extend(log.kept_checks(Kind::Answered).into_iter().map(|ci| ci as u32));
        }
        let tag = self.tag.clone();
        let sql = |e| {
            let e = super::flags::sql(e);
            Error::new(e.kind, format!("{tag}: {}", e.message))
        };
        if let Some(conn) = self.flags(id)? {
            let mut next = conn
                .prepare("SELECT min(ci) FROM chunks WHERE ci>?")
                .map_err(sql)?;
            let mut at = -1i64;
            loop {
                let found: Option<i64> = next.query_row([at], |row| row.get(0)).map_err(sql)?;
                match found {
                    Some(ci) if ci > at => {
                        if let Ok(ci) = u32::try_from(ci) {
                            rules.insert(ci);
                        }
                        at = ci;
                    }
                    _ => break,
                }
            }
        }
        Ok(rules)
    }

    /// This reviewer's planes of rule `ci`'s chunk `k` (`count` errors).
    /// `open`: the rule's errors under a rejection this reviewer has not
    /// answered (`count`'s `opens`).
    fn planes(
        &mut self,
        id: &Identity,
        ci: usize,
        k: u64,
        count: u64,
        open: &[(u64, u64)],
        buffer: &mut [u8],
    ) -> Result<Planes> {
        let waived = match &self.waive {
            // (a rule the reviewer waived nothing in has no bit to read)
            Some((file, stored)) if stored[ci] > 0 => {
                let buffer = &mut buffer[..count as usize];
                file.read_exact_at(buffer, 40 + id.starts[ci] + k * CHUNK)
                    .map_err(|e| self.named(e.into()))?;
                waived_bits(buffer)
            }
            _ => Bits::ZERO,
        };
        let confirmed = match &self.confirm {
            // (a confirmation is the byte a waive is)
            Some((file, stored)) if stored[ci] > 0 => {
                let buffer = &mut buffer[..count as usize];
                file.read_exact_at(buffer, confirm::HEADER + id.starts[ci] + k * CHUNK)
                    .map_err(|e| self.named(e.into()))?;
                waived_bits(buffer)
            }
            _ => Bits::ZERO,
        };
        let rejected = run_bits(open, k * CHUNK, count);
        let waive_known = self.waive.is_some();
        let marked = self.flags_ok;
        let (mut visited, mut not_required) = (Bits::ZERO, Bits::ZERO);
        let tag = self.tag.clone();
        let named = |e: Error| Error::new(e.kind, format!("{tag}: {}", e.message));
        if let Some(conn) = self.flags(id)? {
            let row: Option<(Vec<u8>, Vec<u8>)> = conn
                .query_row(
                    "SELECT visited,nr FROM chunks WHERE ci=? AND chunk=?",
                    rusqlite::params![ci as i64, k as i64],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|e| named(super::flags::sql(e)))?;
            if let Some((v, n)) = row {
                let limit = Bits::first(count);
                visited = Bits::from_blob(&v, "working review bitmap").map_err(named)? & limit;
                not_required = Bits::from_blob(&n, "working review bitmap").map_err(named)? & limit;
            }
        }
        Ok(Planes {
            visited,
            waived,
            not_required,
            confirmed,
            rejected,
            visited_known: marked,
            waive_known,
            not_required_known: marked,
        })
    }

    /// The chunks of rule `ci` this reviewer may have a bit in: every chunk
    /// when they waived anything in the rule (None), else the chunks their
    /// flags have rows for and the ones their rejections lie in.
    fn chunks(&mut self, id: &Identity, ci: usize) -> Result<Option<Vec<u64>>> {
        if self.waive.as_ref().is_some_and(|(_, stored)| stored[ci] > 0) {
            return Ok(None);
        }
        let mut found = Vec::new();
        if let Some(log) = &self.rejects {
            for (a, b, _) in log.runs(Kind::Reject, ci) {
                found.extend(a / CHUNK..=(b - 1) / CHUNK);
            }
        }
        let tag = self.tag.clone();
        let sql = |e| {
            let e = super::flags::sql(e);
            Error::new(e.kind, format!("{tag}: {}", e.message))
        };
        if let Some(conn) = self.flags(id)? {
            let mut statement = conn
                .prepare("SELECT chunk FROM chunks WHERE ci=? ORDER BY chunk")
                .map_err(sql)?;
            let mut rows = statement.query([ci as i64]).map_err(sql)?;
            while let Some(row) = rows.next().map_err(sql)? {
                let k: i64 = row.get(0).map_err(sql)?;
                found.push(k as u64);
            }
        }
        Ok(Some(found))
    }
}

/// The merge's lock: beside the cache, named by it.
fn lock_key(cache: &Path) -> Key {
    let folder = match cache.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let name = cache
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = name.trim_start_matches('.');
    Key {
        dir: folder.join(lock::DIR),
        name: name.to_owned(),
        subject: format!("the team's counts of {}", name.trim_end_matches(".team")),
    }
}

fn grace() -> u64 {
    std::env::var("FLOE_TEAM_GRACE_S")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(GRACE)
}

static SAVES: AtomicU64 = AtomicU64::new(0);

/// Write the cache whole through a temporary beside it - made with `mode`,
/// the pack's: whoever reads the pack reads its counts - and rename it into
/// place (not flushed to the disk: what is lost with a host is counted
/// again). A temporary an ended merge left is removed on the way.
fn save(path: &Path, bytes: &[u8], mode: u32) -> std::io::Result<()> {
    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let prefix = format!("{name}.tmp-");
    let tmp = path.with_file_name(format!(
        "{prefix}{}-{}",
        std::process::id(),
        SAVES.fetch_add(1, Ordering::Relaxed)
    ));
    let result = File::create(&tmp)
        .and_then(|mut f| {
            f.write_all(bytes)?;
            f.set_permissions(fs::Permissions::from_mode(mode))
        })
        .and_then(|()| fs::rename(&tmp, path));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    if let Some(entries) = path.parent().and_then(|folder| fs::read_dir(folder).ok()) {
        for entry in entries.flatten() {
            if !entry.file_name().to_string_lossy().starts_with(&prefix) {
                continue;
            }
            let old = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|at| at.elapsed().ok())
                .is_some_and(|age| age >= LEFTOVER);
            if old {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    result
}

fn load(path: &Path, id: &Identity) -> Option<(Cache, Vec<u8>)> {
    let bytes = fs::read(path).ok()?;
    Cache::of(&bytes, &id.fingerprint, id.sizes.len()).map(|cache| (cache, bytes))
}

/// The team's counts of the result at `pack` (`id`), brought up to date (see
/// the module docs) and kept.
pub fn merge(pack: &Path, id: &Identity, cancelled: &AtomicUsize) -> Result<Kept> {
    let found = discover(pack)?;
    merge_found(pack, id, &found, cancelled)
}

/// `merge` of the reviewers' files `found` (`stats::discover`).
pub(super) fn merge_found(
    pack: &Path,
    id: &Identity,
    found: &BTreeMap<String, Found>,
    cancelled: &AtomicUsize,
) -> Result<Kept> {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    merge_at(pack, id, found, now, grace(), changes::enabled(), cancelled)
}

/// `keep` false (FLOE_TEAM_KEEP=off): nothing is written and no lock taken -
/// what the file holds is brought up to date for this answer alone.
pub(super) fn merge_at(
    pack: &Path,
    id: &Identity,
    found: &BTreeMap<String, Found>,
    now: u64,
    grace: u64,
    keep: bool,
    cancelled: &AtomicUsize,
) -> Result<Kept> {
    let path = path_of(pack)?;
    // the kept file is read by whoever reads the pack
    let mode = fs::metadata(pack).ok().map(|m| m.permissions().mode() & 0o666 | 0o600);
    let tripped = Cell::new(false);
    let pass = |work: Option<Option<u32>>, trust: bool| {
        pass(&path, id, found, now, grace, cancelled, work, trust, &tripped)
    };
    // a pass that counts; where a file it took to read did not (flags that
    // read at the last merge, their time as it was), once more with every
    // reviewer's files opened anew: what reads no more is then left out and
    // said, as the exact report leaves it out
    let counted = |save_as: Option<u32>| -> Result<Kept> {
        let kept = pass(Some(save_as), true)?.expect("a pass that may work answers");
        if !tripped.replace(false) {
            return Ok(kept);
        }
        Ok(pass(Some(save_as), false)?.expect("a pass that may work answers"))
    };
    // with nothing to count and nothing to note the answer is the file's:
    // no lock, no write
    if let Some(kept) = pass(None, true)? {
        return Ok(kept);
    }
    if !keep {
        return counted(None);
    }
    // one merge at a time, where a lock can be had: a second one waits a
    // little, then answers with what is kept and says it is being made
    let key = lock_key(&path);
    let started = Instant::now();
    let guard = loop {
        match lock::writer(&key, Mode::Additive, &format!("{} review counts", crate::program())) {
            Ok(guard) => break Some(guard),
            Err(Busy::Indexing { holder, .. }) => {
                if started.elapsed() >= WAIT {
                    let cache = load(&path, id).map_or_else(|| Cache::empty(id.sizes.len()), |(cache, _)| cache);
                    return Ok(Kept {
                        rules: cache.teams(),
                        reviewers: cache.reviewers.len(),
                        current: false,
                        waiting: vec![format!(
                            "the team's counts are being brought up to date by {}",
                            holder.map_or_else(|| "another process".to_string(), |h| h.to_string())
                        )],
                        skipped: Vec::new(),
                        stored: true,
                        counted: 0,
                    });
                }
                check_cancelled(cancelled)?;
                std::thread::sleep(Duration::from_millis(20));
            }
            // no lock to be had here: merge without one
            Err(_) => break None,
        }
    };
    let kept = counted(Some(mode.unwrap_or(0o644)))?;
    drop(guard);
    Ok(kept)
}

/// One pass over the reviewers (see the module docs). `work` None: only
/// look - the answer is None when there is anything to count, to wait for
/// or to note. Some: count; and with a mode, keep what was counted in the
/// file, made with that mode. `trust`: see `Source::load`; `tripped` is set
/// when the count could not read a reviewer's file.
#[allow(clippy::too_many_arguments)]
fn pass(
    path: &Path,
    id: &Identity,
    found: &BTreeMap<String, Found>,
    now: u64,
    grace: u64,
    cancelled: &AtomicUsize,
    work: Option<Option<u32>>,
    trust: bool,
    tripped: &Cell<bool>,
) -> Result<Option<Kept>> {
    let (work, save_as) = (work.is_some(), work.flatten());
    let rules = id.sizes.len();
    let (mut cache, loaded) = load(path, id).unwrap_or_else(|| (Cache::empty(rules), Vec::new()));
    let identity: [u8; 28] = id.waive_header[12..40].try_into().expect("28 of 40 bytes");
    let mut sources: Vec<Source> = found.iter().map(|(tag, files)| Source::new(tag, files)).collect();
    let mut waiting: Vec<String> = Vec::new();
    let mut skipped: Vec<(String, PathBuf, String)> = Vec::new();
    // rule -> chunk ranges to count again; rules to count whole
    let mut dirty: BTreeMap<usize, Vec<(u64, u64)>> = BTreeMap::new();
    let mut whole: BTreeSet<usize> = BTreeSet::new();
    let mut next: BTreeMap<String, Seen> = BTreeMap::new();
    // what is kept, should this pass not come to its end: nothing is kept of
    // a count that could not read, the next one starts where this did
    let stands = |cache: &Cache, why: String, skipped: Vec<(String, PathBuf, String)>| Kept {
        rules: cache.teams(),
        reviewers: cache.reviewers.len(),
        current: false,
        waiting: vec![why],
        skipped,
        stored: true,
        counted: 0,
    };
    for src in sources.iter_mut() {
        check_cancelled(cancelled)?;
        let kept = cache.reviewers.get(&src.tag);
        let mut waited = false;
        let (verdict, tail) = loop {
            // their log first, their files after
            let known = kept.filter(|k| k.log_id != 0).map(|k| (k.log_id, k.cursor));
            let tail = src.log.as_deref().and_then(|log| changes::read(log, &identity, known));
            src.load(id, kept, trust);
            let mut verdict = judge(kept, tail.as_ref(), src.known(), src.stamps);
            if src.busy.is_some() || src.known() == 0 {
                break (verdict, tail);
            }
            if matches!(verdict, Verdict::Ahead | Verdict::Unsaid) {
                let since = kept.map_or(0, |k| k.suspect);
                if grace == 0 || (verdict == Verdict::Unsaid && since != 0 && now.saturating_sub(since) >= grace) {
                    verdict = Verdict::Again;
                } else if since == 0 && !waited {
                    // a writer between its bits and its record: look once more
                    if !work {
                        return Ok(None);
                    }
                    waited = true;
                    std::thread::sleep(RECORD_WAIT);
                    continue;
                }
            }
            break (verdict, tail);
        };
        skipped.extend(src.skipped.drain(..).map(|(file, why)| (src.tag.clone(), file, why)));
        if let Some(why) = src.busy.take() {
            return Ok(Some(stands(&cache, format!("{}'s review flags are busy: {why}", src.tag), skipped)));
        }
        if src.known() == 0 {
            continue; // nothing of theirs reads: no reviewer of this result
        }
        let said = |seen: &mut Seen, dirty: &mut BTreeMap<usize, Vec<(u64, u64)>>| {
            let Some(tail) = &tail else { return };
            for record in &tail.records {
                if record.kind == TOUCH && (record.rule as usize) < rules {
                    dirty.entry(record.rule as usize).or_default().push((record.a, record.b));
                    seen.active.insert(record.rule);
                }
            }
            seen.log_stamp = stamps_after(seen.log_stamp, &tail.records);
            (seen.log_id, seen.cursor) = (tail.log_id, tail.len);
        };
        let seen = match verdict {
            Verdict::Again => {
                if !work {
                    return Ok(None);
                }
                // every rule they have, or had, anything in is counted again
                let active = match src.active(id) {
                    Ok(active) => active,
                    Err(e) if e.kind == ErrorKind::Cancelled => return Err(e),
                    Err(e) => {
                        tripped.set(true);
                        return Ok(Some(stands(&cache, e.message, skipped)));
                    }
                };
                src.release();
                whole.extend(
                    active
                        .iter()
                        .chain(kept.iter().flat_map(|k| k.active.iter()))
                        .map(|&ci| ci as usize)
                        .filter(|&ci| ci < rules),
                );
                let same_log = kept.is_some_and(|k| tail.as_ref().is_some_and(|t| t.log_id == k.log_id));
                Seen {
                    log_id: tail.as_ref().map_or(0, |t| t.log_id),
                    cursor: tail.as_ref().map_or(0, |t| t.len),
                    // (a log read from its start says its times whole; one
                    // read on from where it was left adds to what it said)
                    log_stamp: tail.as_ref().map_or([0; 4], |t| {
                        stamps_after(kept.filter(|_| same_log).map_or([0; 4], |k| k.log_stamp), &t.records)
                    }),
                    stamps: src.stamps,
                    suspect: 0,
                    known: src.known(),
                    active,
                }
            }
            Verdict::Said => {
                let mut seen = kept.cloned().expect("judged against what is kept");
                said(&mut seen, &mut dirty);
                (seen.stamps, seen.suspect) = (src.stamps, 0);
                seen
            }
            Verdict::Ahead => {
                let mut seen = kept.cloned().expect("judged against what is kept");
                said(&mut seen, &mut dirty);
                (seen.stamps, seen.suspect) = (seen.log_stamp, now.max(1));
                waiting.push(format!(
                    "{} is changing their review: counted as far as their change log says",
                    src.tag
                ));
                seen
            }
            Verdict::Unsaid => {
                let mut seen = kept.cloned().expect("judged against what is kept");
                if seen.suspect == 0 {
                    seen.suspect = now.max(1);
                }
                waiting.push(format!(
                    "{}'s review files and their change log do not agree yet: counted again if no record comes",
                    src.tag
                ));
                seen
            }
        };
        next.insert(src.tag.clone(), seen);
    }
    // a reviewer whose files are gone takes their bits along
    for (tag, seen) in &cache.reviewers {
        if !next.contains_key(tag) {
            whole.extend(seen.active.iter().map(|&ci| ci as usize).filter(|&ci| ci < rules));
        }
    }
    if !work {
        if next != cache.reviewers || !whole.is_empty() || !dirty.is_empty() {
            return Ok(None);
        }
        return Ok(Some(Kept {
            rules: cache.teams(),
            reviewers: cache.reviewers.len(),
            current: waiting.is_empty(),
            waiting,
            skipped,
            stored: true,
            counted: 0,
        }));
    }
    let before = (cache.teams(), cache.reviewers.len());
    cache.reviewers = next;
    let counted = count(&mut cache, &mut sources, id, &whole, dirty, cancelled);
    sources.iter_mut().for_each(Source::release);
    let counted = match counted {
        Ok(counted) => counted,
        Err(e) if e.kind == ErrorKind::Cancelled => return Err(e),
        Err(e) => {
            tripped.set(true);
            return Ok(Some(Kept {
                rules: before.0,
                reviewers: before.1,
                current: false,
                waiting: vec![e.message],
                skipped,
                stored: true,
                counted: 0,
            }))
        }
    };
    let bytes = cache.bytes(&id.fingerprint);
    let stored = bytes == loaded || save_as.is_some_and(|mode| save(path, &bytes, mode).is_ok());
    Ok(Some(Kept {
        rules: cache.teams(),
        reviewers: cache.reviewers.len(),
        current: waiting.is_empty(),
        waiting,
        skipped,
        stored,
        counted,
    }))
}

/// Count the rules `whole` and the chunk ranges `dirty` again from every
/// reviewer's bits, into `cache`; the chunks counted.
fn count(
    cache: &mut Cache,
    sources: &mut [Source],
    id: &Identity,
    whole: &BTreeSet<usize>,
    dirty: BTreeMap<usize, Vec<(u64, u64)>>,
    cancelled: &AtomicUsize,
) -> Result<u64> {
    let mut buffer = vec![0u8; CHUNK as usize];
    let mut planes: Vec<Planes> = Vec::with_capacity(sources.len());
    let mut counted = 0u64;
    // a rule's errors under a rejection each reviewer has not answered - a
    // waive of theirs there is rejected: made once a rule, from every
    // rejection of it that stands (the same errors for everybody who
    // answered nothing in the rule)
    let mut opens: Option<(usize, Vec<(u64, u64)>, Vec<Option<Vec<(u64, u64)>>>)> = None;
    let mut chunk = |cache: &mut Cache, sources: &mut [Source], ci: usize, k: u64| -> Result<()> {
        let size = id.sizes[ci];
        if k >= size.div_ceil(CHUNK) {
            return Ok(());
        }
        counted += 1;
        if counted.is_multiple_of(256) {
            check_cancelled(cancelled)?;
            sources.iter_mut().for_each(Source::release);
        }
        if opens.as_ref().is_none_or(|(rule, ..)| *rule != ci) {
            let standing: Vec<confirm::Standing> = sources
                .iter()
                .filter(|src| src.known() != 0)
                .flat_map(|src| src.rejects.iter().flat_map(|log| confirm::standing_of(&src.tag, log, ci)))
                .collect();
            let own = sources
                .iter()
                .map(|src| match &src.rejects {
                    Some(log) if !standing.is_empty() && log.kept(Kind::Answered, ci).next().is_some() => {
                        Some(confirm::unanswered(&standing, Some(log), ci))
                    }
                    _ => None,
                })
                .collect();
            opens = Some((ci, confirm::unanswered(&standing, None, ci), own));
        }
        let (_, open_all, own) = opens.as_ref().expect("made above");
        let errors = (size - k * CHUNK).min(CHUNK);
        planes.clear();
        for (i, src) in sources.iter_mut().enumerate() {
            if src.known() != 0 {
                let open = own[i].as_deref().unwrap_or(open_all);
                planes.push(src.planes(id, ci, k, errors, open, &mut buffer)?);
            }
        }
        cache.set(ci, k, counts_of(&team_of(&planes)));
        Ok(())
    };
    for &ci in whole {
        check_cancelled(cancelled)?;
        // the chunks anyone may have a bit in, and the ones kept so far
        let mut every = false;
        let mut some: BTreeSet<u64> = cache.kept(ci).into_iter().collect();
        for src in sources.iter_mut() {
            if src.known() == 0 {
                continue;
            }
            match src.chunks(id, ci)? {
                None => every = true,
                Some(chunks) => some.extend(chunks),
            }
        }
        if every {
            for k in 0..id.sizes[ci].div_ceil(CHUNK) {
                chunk(cache, sources, ci, k)?;
            }
        } else {
            for k in some {
                chunk(cache, sources, ci, k)?;
            }
        }
    }
    for (ci, mut ranges) in dirty {
        if whole.contains(&ci) {
            continue;
        }
        ranges.sort_unstable();
        let chunks = id.sizes[ci].div_ceil(CHUNK);
        let mut at = 0u64;
        for (a, b) in ranges {
            let b = b.min(chunks.saturating_sub(1));
            let mut k = a.max(at);
            while k <= b && k < chunks {
                chunk(cache, sources, ci, k)?;
                k += 1;
            }
            at = at.max(b.saturating_add(1));
        }
    }
    Ok(counted)
}

#[cfg(test)]
mod tests;
