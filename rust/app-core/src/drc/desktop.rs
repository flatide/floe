//! The desktop viewer's DRC review: floe/drc.py IcePack's review state, for
//! `floe2 gtk-service` (docs/SHARED_APP_LAYER.ko.md P2b). The files are the
//! review store's (`review`): the per-reviewer waive sidecar
//! `.<db>.waive.<tag>` (FLOEWAIV, version 2) and the notes `.<db>.notes.<tag>.fe`
//! beside the pack, byte for byte. What differs from the web's transactional
//! store is how they are kept, as the GTK viewer always kept them:
//!
//! - every waive click is written at once - one status byte and the rule's
//!   waived counter, in place (a sidecar of 100M errors is never rewritten) -
//!   and flushed to the disk before the request answers (`sync_waives`,
//!   0.12.446: the statistics are read by a command of their own while the
//!   viewer runs, on any host; FLOE_WAIVE_SYNC=off leaves the bytes to the
//!   kernel's writeback as before);
//! - a sidecar of another DRC run (header or size) is moved aside
//!   (`.stale-<epoch>`) and a fresh one seeded from the pack's embedded
//!   sections; the first open creates it;
//! - a results folder that cannot be written keeps the review in the system
//!   temp folder (path-hashed names); when that fails too the pack's own
//!   embedded statuses are shown read-only - one reviewer never changes a
//!   shared result (feature/jobdeck-drc 2026-10-09);
//! - notes are rewritten whole on every edit and the file removed when the
//!   last note goes; the reader takes what Python wrote (lenient lines);
//! - why errors were waived or set aside from the review is said once for
//!   the action, in `.<db>.reasons.<tag>` (`reasons`, 0.12.463): a line an
//!   action, whatever its size. The status writers here drop the reason of
//!   an error they turn back or change without one (`reason_drop`), so a
//!   reason is only ever held by an error that is waived;
//! - a waive a confirmer has confirmed is locked (`confirm::Locks`,
//!   0.12.467): the status writers here leave it as it is - waived, with the
//!   reason it has - unless the session is a confirmer's, and say which
//!   errors they left. A confirmer who turns a waive back takes their own
//!   confirmation of it back with it;
//! - a waive a confirmer has rejected is no waive (the confirmers' rejects
//!   logs, 0.12.468): the status writers here do not waive a rejected error
//!   - whoever's the session is - and say which they left; this reviewer's
//!   waives that are rejected are turned back by `take_rejected`, which a
//!   host calls as often as it wants the review to follow (the viewer every
//!   couple of seconds); a confirmer rejects with `reject_set` and takes it
//!   back with `reject_drop`.
//!
//! A sidecar is bound to the result's content (the pack's content id and
//! error count - what the errors are), not to a file's time or inode: a
//! re-pack, a copy of the folder, a .db whose time alone changed and a
//! re-run with the same errors keep their review (0.12.441).
use super::{
    changes::{ChangeLog, CHUNK as CHANGE_CHUNK},
    confirm::{self, Confirms, Locks, Rejected},
    local::{aside_path, pack_reader, required_reviewer, waive_paths, ReviewerSource},
    reasons::{self, Kind, Reasons, Run},
    review::Layout,
    session, Pack, Violation,
};
use crate::{annotations, check_cancelled, Error, ErrorKind, Result};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};

pub const STATUS_NONE: u8 = 0;
pub const STATUS_WAIVED: u8 = 1;
/// Status scan granularity: rank and page jumps read cached per-chunk
/// waived counts and touch at most one chunk of status bytes.
const STATUS_CHUNK: u64 = 1 << 22;
/// The colour of a note's flateyes text annotation.
const NOTE_COLOR: &str = "#FFD819";
const HEADER: u64 = 40;

/// What a status write left as it was: the errors whose waive is locked (a
/// confirmer has confirmed it, and the session is no confirmer's), and the
/// errors that were not waived because a confirmer has rejected their waive.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Left<T> {
    pub locked: Vec<T>,
    pub rejected: Vec<T>,
}

impl<T> Left<T> {
    pub fn is_empty(&self) -> bool {
        self.locked.is_empty() && self.rejected.is_empty()
    }
}

/// `Left`, counted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Kept {
    pub locked: u64,
    pub rejected: u64,
}

/// Waives of this reviewer that `take_rejected` turned back: a rule's, with
/// who rejected them and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Returned {
    pub check: usize,
    pub runs: Vec<Run>,
    pub count: u64,
    pub by: Vec<Rejected>,
}

fn push_run(runs: &mut Vec<Run>, error: u64) {
    match runs.last_mut() {
        Some(last) if last.1 == error => last.1 = error + 1,
        _ => runs.push((error, error + 1)),
    }
}

/// One note shared by its member errors (global ids).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub text: String,
    pub members: BTreeSet<u64>,
}

/// An open pack and its review (see the module docs).
pub struct Review {
    pub pack: Pack,
    header: [u8; 40],
    /// the waive store: a sidecar (statuses at 40), or the pack itself
    pub waive_path: PathBuf,
    status_off: u64,
    wcount_off: u64,
    wcount: Vec<u32>,
    reader: File,
    writer: Option<File>,
    chunks: BTreeMap<usize, Vec<u64>>,
    notes: BTreeMap<u64, Note>,
    note_of: HashMap<u64, u64>,
    note_next: u64,
    pub note_path: PathBuf,
    note_beside: PathBuf,
    note_temp: PathBuf,
    note_tag: String,
    /// why errors were waived or set aside, an action at a time (`reasons`)
    pub reasons: Reasons,
    /// the waives this reviewer has confirmed (`confirm`; 0.12.466): a file
    /// of their own, made at the first confirmation
    pub confirms: Confirms,
    /// whether this session may confirm: the launcher's word (`floe2 view
    /// --confirmer`), false until the host says so
    confirmer: bool,
    /// every confirmer's confirmations, as the status writers read them:
    /// a waive one of them has confirmed is locked for a session that is
    /// no confirmer's (0.12.467)
    locks: Locks,
    /// the waives this reviewer has rejected as a confirmer (0.12.468): a
    /// log of their own, `.<db>.rejects.<reviewer>` - which errors, when,
    /// why (`reasons`, `Kind::Reject`)
    pub rejects: Reasons,
    /// what the confirmers' rejects logs and this review's statuses were
    /// when `take_rejected` last looked: nothing to do while they are
    rejections_seen: Option<(Vec<(String, u64, u64)>, u64)>,
    /// which chunks of which rule this review changed, said as each change
    /// is settled (`changes`; 0.12.465): what the team's kept counts are
    /// brought up to date from. Shared with the flags store, whose changes
    /// it says too (drc::team).
    pub changes: Arc<Mutex<ChangeLog>>,
    /// what the open had to say (floe/drc.py wrote these to stderr)
    pub notices: Vec<String>,
    /// the reviewer tag the sidecars are named by (drc::team names the
    /// visited/not-required store and snapshots by it too)
    pub reviewer: String,
    /// how that tag was found: stated (`--userid`, FLOE_REVIEWER) or taken
    /// from where the viewer runs - the viewer shows it (2026-11-10)
    pub reviewer_source: ReviewerSource,
    /// this review's hold on its waive sidecar (drc::session): a second
    /// session of the reviewer on this result is refused while it lives
    _session: Option<std::sync::Arc<session::Session>>,
    /// bumped on every status write (a worker reading statuses apart
    /// knows whether they moved under it)
    generation: u64,
    /// whether the in-place waive writes are flushed to the disk at the end
    /// of the request or job that made them (FLOE_WAIVE_SYNC at the open)
    sync: bool,
    /// status bytes written since the last flush
    unsynced: bool,
    /// flushes made (what the tests count)
    syncs: u64,
}

/// FLOE_WAIVE_SYNC: on unless `off` (`0`, `no`, `false`) - the kill switch of
/// the flush after each waive request (0.12.446).
pub(super) fn waive_sync_of(value: Option<&str>) -> bool {
    !matches!(
        value.unwrap_or_default().trim().to_ascii_lowercase().as_str(),
        "off" | "0" | "no" | "false"
    )
}

impl Drop for Review {
    /// What an error left unflushed goes to the disk with the review.
    fn drop(&mut self) {
        let _ = self.sync_waives();
    }
}

fn io(e: Error) -> std::io::Error {
    std::io::Error::other(e.message)
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// Write `bytes` to `path` through `.<name>.tmp-<pid>` in its folder, fsync,
/// rename (floe/drc.py `_note_atomic_write`, `waive_export`).
fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let folder = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let name = path
        .file_name()
        .ok_or_else(|| std::io::Error::other("not a file name"))?
        .to_string_lossy();
    let tmp = folder.join(format!(".{name}.tmp-{}", std::process::id()));
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

/// The notes sidecar names beside the pack and in the temp folder, from the
/// waive names (same database name, reviewer and path hash).
fn note_paths(waive: &[PathBuf; 2]) -> [PathBuf; 2] {
    let rename = |p: &Path, fe: bool| {
        let name = p.file_name().unwrap_or_default().to_string_lossy();
        let name = name.replacen(".waive.", ".notes.", 1);
        p.with_file_name(if fe { format!("{name}.fe") } else { name })
    };
    [rename(&waive[0], true), rename(&waive[1], true)]
}

/// The rejects log's names, from the waive names likewise.
fn reject_paths(waive: &[PathBuf; 2]) -> [PathBuf; 2] {
    [confirm::rejects_beside(&waive[0]), confirm::rejects_beside(&waive[1])]
}

/// The reasons file's names, from the waive names likewise.
fn reason_paths(waive: &[PathBuf; 2]) -> [PathBuf; 2] {
    let rename = |p: &Path| {
        let name = p.file_name().unwrap_or_default().to_string_lossy();
        p.with_file_name(name.replacen(".waive.", ".reasons.", 1))
    };
    [rename(&waive[0]), rename(&waive[1])]
}

/// Seconds since the epoch: an action's time.
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Create or validate the waive sidecar at `side`; returns it. Another run's
/// file is moved aside, a new one seeded from the pack's sections.
fn ensure_sidecar(
    side: &Path,
    pack: &Pack,
    header: &[u8; 40],
    notices: &mut Vec<String>,
) -> std::io::Result<PathBuf> {
    let size = HEADER + pack.total + 4 * pack.checks.len() as u64;
    match File::open(side) {
        Ok(mut f) => {
            let mut head = [0u8; 40];
            let ok =
                f.read_exact(&mut head).is_ok() && head == *header && f.metadata()?.len() == size;
            if ok {
                return Ok(side.to_path_buf());
            }
            let aside = aside_path(side);
            fs::rename(side, &aside)?;
            notices.push(format!(
                "[drc] waive autosave does not match this pack (DRC re-run?); moved aside: {}",
                aside.display()
            ));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let tmp = with_suffix(side, &format!(".tmp-{}", std::process::id()));
    let result = (|| {
        let mut out = File::create(&tmp)?;
        let copied = std::io::copy(&mut pack.review_seed().map_err(io)?, &mut out)?;
        if copied != size {
            return Err(std::io::Error::other("pack status sections truncated"));
        }
        out.sync_all()?;
        fs::rename(&tmp, side)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map(|_| side.to_path_buf())
}

impl Review {
    /// Open `pack_path` for review: its reader lock held while open (a
    /// re-pack refuses it), `source` - the .db - checked against the pack's
    /// fingerprint when given, the reviewer `reviewer` (else FLOE_REVIEWER,
    /// the DISPLAY host, the ssh client, the account).
    pub fn open(
        pack_path: &Path,
        source: Option<&Path>,
        reviewer: Option<&str>,
        cancelled: &AtomicUsize,
    ) -> Result<Self> {
        Self::open_with(pack_path, source, reviewer, cancelled, false)
    }
    /// `open`. `whole`: a source whose time changed is read and compared
    /// whatever its size (Pack::source_current) - the load that would parse
    /// the ASCII file were the pack stale. Without it the read stops at the
    /// inline limit (Pack::source_matches): the open of a pack chosen with
    /// its .db, where a stale pack is asked about instead.
    fn open_with(
        pack_path: &Path,
        source: Option<&Path>,
        reviewer: Option<&str>,
        cancelled: &AtomicUsize,
        whole: bool,
    ) -> Result<Self> {
        let lock = pack_reader(pack_path)?;
        let mut pack = Pack::open(pack_path, cancelled)?;
        pack.hold(lock);
        if let Some(source) = source {
            let current = if whole {
                pack.source_current(source, cancelled)?
            } else {
                pack.source_matches(source)?
            };
            if !current {
                return Err(Error::new(
                    ErrorKind::Cache,
                    format!(
                        "{}: stale pack (the source is not the file that was indexed)",
                        pack_path.display()
                    ),
                ));
            }
        }
        let header = Layout::from_pack(&pack)?.header();
        // (a stated id where FLOE_REVIEWER_REQUIRED asks for one)
        let who = required_reviewer(reviewer)?;
        let tag = who.tag.clone();
        let waive = waive_paths(&pack.path, &tag)?;
        let [note_beside, note_temp] = note_paths(&waive);
        let mut notices = Vec::new();
        let (status_section, wcount_section) = pack.review_sections();
        let (waive_path, status_off, wcount_off) = match ensure_sidecar(
            &waive[0],
            &pack,
            &header,
            &mut notices,
        ) {
            Ok(p) => (p, HEADER, HEADER + pack.total),
            Err(_) => match ensure_sidecar(&waive[1], &pack, &header, &mut notices) {
                Ok(p) => {
                    notices.push(format!(
                        "[drc] results folder not writable; waive autosave in {} (save waives as… to keep the review)",
                        p.display()
                    ));
                    (p, HEADER, HEADER + pack.total)
                }
                Err(e) => {
                    notices.push(format!(
                        "[drc] waive autosave unavailable ({e}); embedded statuses are read-only; no shared pack changes will be made"
                    ));
                    (pack.path.clone(), status_section, wcount_section)
                }
            },
        };
        // one session a reviewer and result: the sidecar's counters are kept
        // in memory by the session that writes them (drc::session); the
        // pack's own embedded statuses are read-only and need none
        let session = if waive_path == pack.path {
            None
        } else {
            let database = crate::cache::database_path(&pack.path)?;
            session::hold(
                &waive_path,
                &tag,
                &database.file_name().unwrap_or_default().to_string_lossy(),
                &floe_vfs::lock::reader_label(&format!("{} drc review", crate::program())),
                &mut notices,
            )?
        };
        let reader = File::open(&waive_path)?;
        let mut raw = vec![0u8; pack.checks.len() * 4];
        reader.read_exact_at(&mut raw, wcount_off)?;
        let wcount = raw
            .chunks_exact(4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        // (the result's content, as the waive file's header: review.rs)
        let note_tag = super::review::Fingerprint::of(&pack).tag();
        let sync = waive_sync_of(std::env::var("FLOE_WAIVE_SYNC").ok().as_deref());
        // the reasons: beside the waive sidecar in use, bound to the result
        // as the notes are; the pack shown read-only has none to write
        let mut reasons = Reasons::open(
            reason_paths(&waive),
            note_tag.clone(),
            pack.checks.iter().map(|c| c.count).collect(),
            sync,
        );
        notices.append(&mut reasons.notices);
        // the rejections this reviewer made as a confirmer: a log of the
        // same shape, in a file of its own
        let mut rejects = Reasons::open_as(
            "# floe drc waive rejections",
            reject_paths(&waive),
            note_tag.clone(),
            pack.checks.iter().map(|c| c.count).collect(),
            sync,
        );
        notices.append(&mut rejects.notices);
        // the confirmations: beside the sidecar in use (a file of another
        // result is moved aside, as the sidecar's was); the pack shown
        // read-only has none to write
        let mut confirms = if waive_path == pack.path {
            Confirms::none(pack.checks.len())
        } else {
            Confirms::open(
                confirm::path_beside(&waive_path),
                confirm::header_of(&header),
                pack.checks.iter().map(|c| (c.start, c.count)).collect(),
                sync,
            )
        };
        notices.append(&mut confirms.notices);
        // which waives are locked: every confirmer's file of this result,
        // named as this reviewer's would be (the read-only pack writes
        // nothing, so it asks nothing)
        let locks = if waive_path == pack.path {
            Locks::none(pack.checks.len())
        } else {
            Locks::beside(
                &waive,
                &tag,
                confirm::header_of(&header),
                pack.checks.iter().map(|c| (c.start, c.count)).collect(),
                note_tag.clone(),
            )
        };
        // the change log: beside the sidecar in use; the pack shown
        // read-only has no file of its own and says nothing
        let changes = if waive_path == pack.path {
            ChangeLog::none()
        } else if !super::changes::enabled() {
            ChangeLog::silent(&waive_path)
        } else {
            let identity: [u8; 28] = header[12..40].try_into().expect("28 of 40 bytes");
            ChangeLog::open(&waive_path, identity, sync)
        };
        let mut review = Self {
            pack,
            header,
            waive_path,
            status_off,
            wcount_off,
            wcount,
            reader,
            writer: None,
            chunks: BTreeMap::new(),
            notes: BTreeMap::new(),
            note_of: HashMap::new(),
            note_next: 1,
            note_path: note_beside.clone(),
            note_beside,
            note_temp,
            note_tag,
            reasons,
            confirms,
            confirmer: false,
            locks,
            rejects,
            rejections_seen: None,
            changes: Arc::new(Mutex::new(changes)),
            notices,
            reviewer: tag,
            reviewer_source: who.source,
            _session: session,
            generation: 0,
            sync,
            unsynced: false,
            syncs: 0,
        };
        review.load_notes();
        Ok(review)
    }

    fn check(&self, check: usize) -> Result<(u64, u64)> {
        self.pack
            .checks
            .get(check)
            .map(|c| (c.start, c.count))
            .ok_or_else(|| Error::input("DRC check index out of range"))
    }

    /// Global 0-based error id (the status byte's and the notes' key).
    pub fn error_gid(&self, check: usize, error: u64) -> Result<u64> {
        let (start, count) = self.check(check)?;
        if error >= count {
            return Err(Error::input("DRC error index out of range"));
        }
        Ok(start + error)
    }

    /// Check `check`'s status bytes `start..start+count` (clamped).
    pub fn statuses(&self, check: usize, start: u64, count: u64) -> Result<Vec<u8>> {
        let (first, n) = self.check(check)?;
        let start = start.min(n);
        let count = count.min(n - start);
        let mut out = vec![0u8; count as usize];
        self.reader
            .read_exact_at(&mut out, self.status_off + first + start)?;
        Ok(out)
    }

    pub fn status(&self, check: usize, error: u64) -> Result<u8> {
        let gid = self.error_gid(check, error)?;
        let mut b = [0u8; 1];
        self.reader.read_exact_at(&mut b, self.status_off + gid)?;
        Ok(b[0])
    }

    /// A status reader of its own for worker threads (drc::analysis): the
    /// same store, positional reads; the review's writes are seen as made.
    pub fn status_reader(&self) -> Result<StatusReader> {
        Ok(StatusReader {
            file: std::sync::Arc::new(self.reader.try_clone()?),
            status_off: self.status_off,
            checks: std::sync::Arc::new(
                self.pack.checks.iter().map(|c| (c.start, c.count)).collect(),
            ),
        })
    }

    /// (waived, total) of one rule, from the stored counters.
    pub fn status_counts(&self, check: usize) -> Result<(u64, u64)> {
        let (_, n) = self.check(check)?;
        Ok((u64::from(self.wcount[check]), n))
    }

    /// The waive store is the pack's embedded sections, shown read-only
    /// (no sidecar could be made).
    pub fn read_only(&self) -> bool {
        self.waive_path == self.pack.path
    }

    /// The 40-byte header of this pack's waive files (FLOEWAIV, version 2).
    pub fn waive_header(&self) -> [u8; 40] {
        self.header
    }

    /// Bumped by every status write.
    pub fn status_generation(&self) -> u64 {
        self.generation
    }

    fn writer(&mut self) -> Result<&File> {
        if self.read_only() {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "per-reviewer waive storage unavailable; shared pack is read-only",
            ));
        }
        self.generation += 1;
        self.unsynced = true;
        if self.writer.is_none() {
            self.writer = Some(
                OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&self.waive_path)?,
            );
        }
        Ok(self.writer.as_ref().expect("opened"))
    }

    /// Flush the waive writes made since the last flush to the disk: the
    /// request or the job that made them ends with this (user 2026-11-11:
    /// "waive 처리 작업을 매번 자동 저장" - every click was written at once
    /// but stayed in the page cache until the kernel wrote it back, which a
    /// host that goes down loses and a reader on another host of an NFS
    /// folder does not see for tens of seconds; the statistics are read by a
    /// command of their own while the viewer runs). One flush a request or a
    /// job, whatever its size. FLOE_WAIVE_SYNC=off: none, as before 0.12.446.
    pub fn sync_waives(&mut self) -> Result<()> {
        // (the reasons a job took back with the statuses: said in their
        // file now, one line a rule, and flushed with them)
        if let Err(e) = self.reasons.flush(now()) {
            self.notices.push(format!(
                "[drc] review reasons: {} ({})",
                e.message,
                self.reasons.path.display()
            ));
        }
        self.notices.append(&mut self.reasons.notices);
        // (and the rejections taken back: a line a rule of the rejects log)
        if let Err(e) = self.rejects.flush(now()) {
            self.notices.push(format!(
                "[drc] waive rejections: {} ({})",
                e.message,
                self.rejects.path.display()
            ));
        }
        self.notices.append(&mut self.rejects.notices);
        let flushed = self.flush_statuses().and(self.confirms.flush());
        // what this review changed - its statuses and, through the flags
        // store that shares the log, its flags - is said now that the bits
        // are on the disk (`changes`): a line of its own file, so the
        // team's counts are brought up to date from what changed
        let mut log = self.changes.lock().unwrap_or_else(|p| p.into_inner());
        log.rejects_at(&self.rejects.path);
        log.settle();
        self.notices.append(&mut log.notices);
        flushed
    }

    fn flush_statuses(&mut self) -> Result<()> {
        if !std::mem::take(&mut self.unsynced) || !self.sync {
            return Ok(());
        }
        let Some(writer) = &self.writer else {
            return Ok(());
        };
        match writer.sync_data() {
            Ok(()) => {
                self.syncs += 1;
                Ok(())
            }
            // a file system without the call writes back in its own time
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::Unsupported | std::io::ErrorKind::InvalidInput
                ) =>
            {
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Rule `check`'s errors `first..=last` changed their status: said in
    /// the change log at the next `sync_waives`.
    fn touched(&self, check: usize, first: u64, last: u64) {
        self.changes
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .touched(check, first / CHANGE_CHUNK, last / CHANGE_CHUNK);
    }

    /// The flushes `sync_waives` made.
    pub fn waive_syncs(&self) -> u64 {
        self.syncs
    }

    /// Turn the flushes on or off (FLOE_WAIVE_SYNC decides at the open).
    pub fn set_waive_sync(&mut self, on: bool) {
        self.sync = on;
        self.reasons.set_sync(on);
        self.rejects.set_sync(on);
        self.confirms.set_sync(on);
        self.changes.lock().unwrap_or_else(|p| p.into_inner()).set_sync(on);
    }

    // ---- confirmations (0.12.466) ---------------------------------------

    /// Whether this session may confirm waives: what the launcher said
    /// (`floe2 view --confirmer`, FLOE_CONFIRMER) - a role, not a check of
    /// who the reviewer is (the ids are the launcher's too). The role is
    /// also what turns a confirmed waive back: no lock holds its session.
    pub fn set_confirmer(&mut self, on: bool) {
        self.confirmer = on;
    }

    pub fn confirmer(&self) -> bool {
        self.confirmer
    }

    // ---- the lock of a confirmed waive (0.12.467) ------------------------

    /// Rule `check`'s errors `start..start+len` (clamped): true where a
    /// confirmer has the error confirmed, which locks this reviewer's waive
    /// of it - not turned back, its reason not changed, by any writer here.
    /// None when none is, and for a confirmer's session, which no lock
    /// holds (user 2026-11-13: "confirm된 waive는 되돌릴 수 없게 잠구고
    /// confirmer만 되돌릴 수 있게 해줘"). Whether the error is waived here
    /// is the caller's to ask: an error this reviewer has not waived has
    /// nothing to lock.
    ///
    /// A waive a confirmer has rejected is never locked, whoever confirmed
    /// it: it is to be turned back.
    pub fn locked(&mut self, check: usize, start: u64, len: u64) -> Result<Option<Vec<bool>>> {
        if self.confirmer {
            return Ok(None);
        }
        let Some(mut locked) = self.locks.mask(check, start, len)? else {
            return Ok(None);
        };
        if let Some(rejected) = self.rejected_at(check, start, len, false)? {
            for (l, r) in locked.iter_mut().zip(rejected) {
                *l &= !r;
            }
        }
        Ok(locked.contains(&true).then_some(locked))
    }

    // ---- the rejection of a waive (0.12.468) ------------------------------

    /// Rule `check`'s errors `start..start+len` (clamped): true where a
    /// confirmer has rejected the error's waive - no session waives it
    /// (user 2026-11-14: "confirmer가 다른 리뷰어의 waive를 반려할 수 있게
    /// 해줘"). None when none is.
    pub fn rejected(&mut self, check: usize, start: u64, len: u64) -> Result<Option<Vec<bool>>> {
        self.rejected_at(check, start, len, true)
    }

    /// `rejected`: what the confirmers' logs say as the locks read them,
    /// and what this reviewer's own log holds - which is known here at
    /// once, where another's first rejection (a new file) is found at the
    /// next look for files.
    fn rejected_at(&mut self, check: usize, start: u64, len: u64, waiving: bool) -> Result<Option<Vec<bool>>> {
        let mut out = self.locks.rejected(check, start, len, waiving)?;
        let (_, n) = self.check(check)?;
        let start = start.min(n);
        let len = len.min(n - start);
        for (a, b) in self.rejects.held(Kind::Reject, check, &[(start, start + len)]) {
            let out = out.get_or_insert_with(|| vec![false; len as usize]);
            out[(a - start) as usize..(b - start) as usize].fill(true);
        }
        Ok(out)
    }

    /// Who has rejected the waive of any of rule `check`'s errors `runs`,
    /// and why as they said: what the answer of a write that left rejected
    /// errors says.
    pub fn rejected_by(&mut self, check: usize, runs: &[Run]) -> Result<Vec<Rejected>> {
        let mut by = self.locks.rejected_by(check, runs)?;
        // (this reviewer's own, where the locks have not found its file yet)
        if !by.iter().any(|r| r.reviewer == self.reviewer) {
            let held = self.rejects.held(Kind::Reject, check, runs);
            if let Some(entry) = held.first().and_then(|first| self.rejects.of(Kind::Reject, check, first.0)) {
                by.push(Rejected { reviewer: self.reviewer.clone(), text: entry.text.clone(), time: entry.time });
                by.sort_by(|a, b| a.reviewer.cmp(&b.reviewer));
            }
        }
        Ok(by)
    }

    fn may_reject(&self) -> Result<()> {
        if !self.confirmer {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "rejecting waives is the confirmer's: start the viewer with --confirmer",
            ));
        }
        if self.read_only() {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "this review is read-only here: nothing can be rejected",
            ));
        }
        Ok(())
    }

    /// Reject the waive of rule `check`'s `runs` (sorted, disjoint, not
    /// touching), `text` saying why (it may be empty): one line of this
    /// confirmer's rejects log, whatever the number of errors. Their own
    /// waive of those errors is turned back and their own confirmation of
    /// them taken back with it; the other reviewers' sessions turn theirs
    /// back when they look (`take_rejected`). Whose waive the errors are is
    /// the caller's to ask (drc::team::waives): a rejection is the error's.
    pub fn reject_set(&mut self, check: usize, runs: &[Run], text: &str) -> Result<()> {
        self.may_reject()?;
        let (_, n) = self.check(check)?;
        if runs.is_empty() {
            return Ok(());
        }
        if runs.last().is_some_and(|r| r.1 > n) {
            return Err(Error::input("DRC error index out of range"));
        }
        let said = self.rejects.set(Kind::Reject, check, runs, text, now());
        self.notices.append(&mut self.rejects.notices);
        said.map_err(|e| Error::new(e.kind, format!("{} ({})", e.message, self.rejects.path.display())))?;
        // (the log's change is a change of every chunk it names: the
        // team's counts follow)
        for &(a, b) in runs {
            self.touched(check, a, b - 1);
        }
        self.turn_back(check, runs)?;
        self.unconfirm_runs(check, runs)?;
        self.sync_waives()
    }

    /// This confirmer's own confirmations of rule `check`'s `runs` are
    /// taken back: a rejection is their other verdict on the same errors.
    fn unconfirm_runs(&mut self, check: usize, runs: &[Run]) -> Result<()> {
        const STEP: u64 = 1 << 16;
        if !self.confirms.present() || self.confirms.counts()[check] == 0 {
            return Ok(());
        }
        for &(a, b) in runs {
            let mut at = a;
            while at < b {
                let len = (b - at).min(STEP);
                let was = self.confirms.statuses(check, at, len)?;
                if was.contains(&confirm::CONFIRMED) {
                    self.confirms.write_span(check, at, &was, &vec![0u8; was.len()])?;
                }
                at += len;
            }
        }
        Ok(())
    }

    /// Take this confirmer's rejection of rule `check`'s `runs` back:
    /// whether any of them was rejected. The errors are anybody's to waive
    /// again, and a waive never turned back since stands again.
    pub fn reject_drop(&mut self, check: usize, runs: &[Run]) -> Result<bool> {
        self.may_reject()?;
        self.check(check)?;
        let held = self.rejects.held(Kind::Reject, check, runs);
        if held.is_empty() {
            return Ok(false);
        }
        self.rejects.clear(Kind::Reject, check, &held);
        for &(a, b) in &held {
            self.touched(check, a, b - 1);
        }
        self.sync_waives()?;
        Ok(true)
    }

    /// Turn this reviewer's waives among rule `check`'s `runs` back: the
    /// runs that were waived. (No lock holds a rejected waive.)
    fn turn_back(&mut self, check: usize, runs: &[Run]) -> Result<Vec<Run>> {
        const STEP: u64 = 1 << 16;
        let mut back = Vec::new();
        if self.wcount[check] == 0 {
            return Ok(back);
        }
        for &(a, b) in runs {
            let mut at = a;
            while at < b {
                let len = (b - at).min(STEP);
                let before = self.statuses(check, at, len)?;
                if before.contains(&STATUS_WAIVED) {
                    let after: Vec<u8> = before
                        .iter()
                        .map(|&s| if s == STATUS_WAIVED { STATUS_NONE } else { s })
                        .collect();
                    self.write_span(check, at, &before, &after)?;
                    for (j, &s) in before.iter().enumerate() {
                        if s == STATUS_WAIVED {
                            push_run(&mut back, at + j as u64);
                        }
                    }
                }
                at += len;
            }
        }
        Ok(back)
    }

    /// This reviewer's waives that a confirmer has rejected are turned
    /// back - status and reason, as any waive turned back - and said: rule
    /// by rule what was turned back, who rejected it and why. A host calls
    /// this when it wants the review to follow the confirmers (at the open,
    /// and then as often as it likes: the look lists the folders the
    /// confirmers' files are in and reads the logs that changed, and does
    /// nothing more while they and this review's statuses are as they were).
    pub fn take_rejected(&mut self) -> Result<Vec<Returned>> {
        if self.read_only() {
            return Ok(Vec::new());
        }
        let (rules, seen) = self.locks.rejections();
        if self.rejections_seen.as_ref().is_some_and(|(logs, generation)| *logs == seen && *generation == self.generation) {
            return Ok(Vec::new());
        }
        let mut returned = Vec::new();
        for (check, runs) in rules {
            let back = self.turn_back(check, &runs)?;
            if !back.is_empty() {
                let by = self.locks.rejected_by(check, &back)?;
                returned.push(Returned {
                    check,
                    count: back.iter().map(|r| r.1 - r.0).sum(),
                    runs: back,
                    by,
                });
            }
        }
        if !returned.is_empty() {
            self.sync_waives()?;
        }
        self.rejections_seen = Some((seen, self.generation));
        Ok(returned)
    }

    /// Who has rule `check`'s error `error` confirmed.
    pub fn confirmed_by(&mut self, check: usize, error: u64) -> Result<Vec<String>> {
        self.locks.by(check, error)
    }

    /// Who has any of rule `check`'s errors `runs` confirmed: what the
    /// answer of a write that left locked waives says, so their reviewer
    /// knows whom to ask.
    pub fn confirmers_of(&mut self, check: usize, runs: &[Run]) -> Result<Vec<String>> {
        self.locks.who(check, runs)
    }

    /// How old what was read of the confirmers' files may be before it is
    /// read anew (`confirm::LOCK_REFRESH`; the tests ask at each look).
    pub fn set_lock_refresh(&mut self, every: std::time::Duration) {
        self.locks.set_refresh(every);
    }

    /// A confirmer turned rule `check`'s waives `back` (not empty; within a
    /// span the caller wrote) back: their own confirmation of them goes
    /// with it. Left, it would count again - and lock - the moment anybody
    /// waived the error anew, unseen by its confirmer.
    fn unconfirm(&mut self, check: usize, back: &[u64]) -> Result<()> {
        if !self.confirmer || !self.confirms.present() {
            return Ok(());
        }
        let (Some(&first), Some(&last)) = (back.iter().min(), back.iter().max()) else {
            return Ok(());
        };
        if (last - first) / 8 > back.len() as u64 {
            // scattered: an error at a time
            self.confirms.set(check, back, false)?;
            return Ok(());
        }
        let was = self.confirms.statuses(check, first, last - first + 1)?;
        if !was.contains(&confirm::CONFIRMED) {
            return Ok(());
        }
        let mut now = was.clone();
        for &error in back {
            now[(error - first) as usize] = 0;
        }
        self.confirms.write_span(check, first, &was, &now)?;
        Ok(())
    }

    fn may_confirm(&self) -> Result<()> {
        if self.confirmer {
            Ok(())
        } else {
            Err(Error::new(
                ErrorKind::Unsupported,
                "confirming waives is the confirmer's: start the viewer with --confirmer",
            ))
        }
    }

    /// Confirm (`on`) the waives of rule `check`'s `errors`, or take this
    /// reviewer's confirmation of them back: written in place and flushed;
    /// the changed count. Whether somebody has the errors waived is the
    /// caller's to ask (drc::team::waives): a confirmation counts while
    /// somebody does.
    pub fn confirm_set(&mut self, check: usize, errors: &[u64], on: bool) -> Result<u64> {
        self.may_confirm()?;
        let changed = self.confirms.set(check, errors, on)?;
        for &error in &changed {
            self.touched(check, error, error);
        }
        self.sync_waives()?;
        Ok(changed.len() as u64)
    }

    /// One span of a job that confirms: rule `check`'s confirmation bytes
    /// `start..` from `before` (as read) to `after`; the changed count. The
    /// job ends with `sync_waives`.
    pub fn confirm_span(&mut self, check: usize, start: u64, before: &[u8], after: &[u8]) -> Result<u64> {
        self.may_confirm()?;
        let changed = self.confirms.write_span(check, start, before, after)?;
        if changed > 0 {
            self.touched(check, start, start + after.len() as u64 - 1);
        }
        Ok(changed)
    }

    // ---- reasons (0.12.463) --------------------------------------------

    /// The reason of an action - `kind` - on rule `check`'s `runs`: every
    /// error of them holds it from now, in place of what it held; written
    /// and flushed. The caller has made the errors waived (or set them
    /// aside) first: a reason that cannot be kept leaves that done, and the
    /// error says so.
    pub fn reason_set(&mut self, kind: Kind, check: usize, runs: &[Run], text: &str) -> Result<()> {
        let result = self.reasons.set(kind, check, runs, text, now());
        self.notices.append(&mut self.reasons.notices);
        result.map_err(|e| {
            Error::new(
                e.kind,
                format!("{} ({})", e.message, self.reasons.path.display()),
            )
        })
    }

    /// The reasons of rule `check`'s `runs` are gone: the errors were turned
    /// back, or changed by something that gave no reason. No longer shown
    /// from now; said in the file by the caller's `sync_waives` - the end
    /// of its request or job -, where a file that cannot be written is said
    /// in the notices and stops nothing (the readers ask the status besides).
    pub fn reason_drop(&mut self, kind: Kind, check: usize, runs: &[Run]) {
        if !runs.is_empty() {
            self.reasons.clear(kind, check, runs);
        }
    }

    /// Why rule `check`'s error `error` is waived: None when it is not
    /// waived, or was waived without a reason.
    pub fn waive_reason(&self, check: usize, error: u64) -> Result<Option<&reasons::Entry>> {
        if self.status(check, error)? != STATUS_WAIVED {
            return Ok(None);
        }
        Ok(self.reasons.of(Kind::Waive, check, error))
    }

    /// Set the status of check `check`'s errors `errors` to `value`, in
    /// place, keeping the rule's waived counter in step; flushed. The
    /// errors it changes lose their waive reason: turned back they have
    /// none, and waived here they are waived without one until the caller
    /// gives it (`reason_set`, which is every error's of the action then).
    ///
    /// The errors of `errors` this write leaves as they are come back,
    /// each list sorted: the ones whose waive is locked (`locked`) - not
    /// turned back, and, waived again, not the caller's to give a reason to
    /// - and the ones a confirmer has rejected the waive of (`rejected`),
    /// which are not waived here.
    pub fn set_status(&mut self, check: usize, errors: &[u64], value: u8) -> Result<Left<u64>> {
        let (start, n) = self.check(check)?;
        if errors.iter().any(|&e| e >= n) {
            return Err(Error::input("DRC error index out of range"));
        }
        let mut changed = Vec::new();
        let (mut left, mut back) = (Left::default(), Vec::new());
        for &error in errors {
            let gid = start + error;
            let mut old = [0u8; 1];
            self.reader.read_exact_at(&mut old, self.status_off + gid)?;
            if value == STATUS_WAIVED && self.rejected_at(check, error, 1, true)?.is_some() {
                // (waived here or not: a rejected error is not this write's)
                left.rejected.push(error);
                continue;
            }
            if old[0] == STATUS_WAIVED && self.locked(check, error, 1)?.is_some() {
                left.locked.push(error);
                continue;
            }
            if old[0] == value {
                continue;
            }
            if old[0] == STATUS_WAIVED {
                back.push(error);
            }
            changed.push(error);
            let (status_off, wcount_off) = (self.status_off, self.wcount_off);
            self.writer()?.write_all_at(&[value], status_off + gid)?;
            let (was, now) = (old[0] == STATUS_WAIVED, value == STATUS_WAIVED);
            if was != now {
                let cur = i64::from(self.wcount[check]) + if now { 1 } else { -1 };
                let cur = cur.max(0) as u32;
                self.writer()?
                    .write_all_at(&cur.to_le_bytes(), wcount_off + 4 * check as u64)?;
                self.wcount[check] = cur;
                if let Some(chunks) = self.chunks.get_mut(&check) {
                    let k = (error / STATUS_CHUNK) as usize;
                    if now {
                        chunks[k] += 1;
                    } else {
                        chunks[k] = chunks[k].saturating_sub(1);
                    }
                }
            }
        }
        if self.reasons.any(Kind::Waive, check) {
            self.reason_drop(Kind::Waive, check, &reasons::runs_of(&changed));
        }
        for &error in &changed {
            self.touched(check, error, error);
        }
        self.unconfirm(check, &back)?;
        self.sync_waives()?;
        for list in [&mut left.locked, &mut left.rejected] {
            list.sort_unstable();
            list.dedup();
        }
        Ok(left)
    }

    /// One chunk of `set_status_members`: errors `a..a+len` of rule `check`;
    /// the changed count. The members this write leaves as they are - the
    /// ones whose waive is locked (`locked`), the ones not waived because
    /// their waive is rejected (`rejected`) - are added to `left` as runs,
    /// in order when the chunks are asked in order.
    pub fn set_status_range(
        &mut self,
        check: usize,
        members: &dyn super::analysis::scope::Membership,
        value: u8,
        a: u64,
        len: u64,
        left: &mut Left<Run>,
    ) -> Result<u64> {
        let (start, n) = self.check(check)?;
        let len = len.min(n.saturating_sub(a));
        let mut mask = vec![false; len as usize];
        members.mask(a, &mut mask);
        if !mask.iter().any(|&m| m) {
            return Ok(0);
        }
        let before = self.statuses(check, a, len)?;
        if value == STATUS_WAIVED {
            if let Some(rejected) = self.rejected_at(check, a, len, true)? {
                for j in (0..len as usize).filter(|&j| rejected[j]) {
                    if std::mem::take(&mut mask[j]) {
                        push_run(&mut left.rejected, a + j as u64);
                    }
                }
            }
        }
        if (0..len as usize).any(|j| mask[j] && before[j] == STATUS_WAIVED) {
            if let Some(locked) = self.locked(check, a, len)? {
                for j in (0..len as usize).filter(|&j| locked[j] && before[j] == STATUS_WAIVED) {
                    if std::mem::take(&mut mask[j]) {
                        push_run(&mut left.locked, a + j as u64);
                    }
                }
            }
        }
        let positions: Vec<usize> = (0..len as usize)
            .filter(|&j| mask[j] && before[j] != value)
            .collect();
        let (Some(&lo), Some(&hi)) = (positions.first(), positions.last()) else {
            return Ok(0);
        };
        let mut after = before[lo..=hi].to_vec();
        for &j in &positions {
            after[j - lo] = value;
        }
        let delta: i64 = positions
            .iter()
            .map(|&j| i64::from(value == STATUS_WAIVED) - i64::from(before[j] == STATUS_WAIVED))
            .sum();
        let (status_off, wcount_off) = (self.status_off, self.wcount_off);
        self.writer()?
            .write_all_at(&after, status_off + start + a + lo as u64)?;
        if delta != 0 {
            let cur = (i64::from(self.wcount[check]) + delta).max(0) as u32;
            self.writer()?
                .write_all_at(&cur.to_le_bytes(), wcount_off + 4 * check as u64)?;
            self.wcount[check] = cur;
        }
        // (as `set_status`: what changed holds no reason until one is given)
        if self.reasons.any(Kind::Waive, check) {
            let changed: Vec<u64> = positions.iter().map(|&j| a + j as u64).collect();
            self.reason_drop(Kind::Waive, check, &reasons::runs_of(&changed));
        }
        self.touched(check, a + lo as u64, a + hi as u64);
        if self.confirmer && value != STATUS_WAIVED {
            let back: Vec<u64> = positions
                .iter()
                .filter(|&&j| before[j] == STATUS_WAIVED)
                .map(|&j| a + j as u64)
                .collect();
            self.unconfirm(check, &back)?;
        }
        if let Some(chunks) = self.chunks.get_mut(&check) {
            for &j in &positions {
                let k = ((a + j as u64) / STATUS_CHUNK) as usize;
                if value == STATUS_WAIVED {
                    chunks[k] += 1;
                } else if before[j] == STATUS_WAIVED {
                    chunks[k] = chunks[k].saturating_sub(1);
                }
            }
        }
        Ok(positions.len() as u64)
    }

    /// Set the status of every member of `members` in rule `check` to
    /// `value`, a 65536-error chunk at a time (feature/jobdeck-drc
    /// `set_status_members`): each chunk's changed span written in place,
    /// the counter after it; `progress(done, total)` after each chunk;
    /// cancelling leaves the chunks written so far. The changed count; the
    /// members left as they were - locked, rejected - are added to `left`.
    pub fn set_status_members(
        &mut self,
        check: usize,
        members: &dyn super::analysis::scope::Membership,
        value: u8,
        cancelled: &AtomicUsize,
        progress: &mut dyn FnMut(u64, u64),
        left: &mut Left<Run>,
    ) -> Result<u64> {
        const CHUNK: u64 = 1 << 16;
        let (_, n) = self.check(check)?;
        let mut changed = 0u64;
        let mut a = 0;
        while a < n {
            if let Err(e) = check_cancelled(cancelled) {
                // the chunks written so far stay: flushed with the rest
                self.sync_waives()?;
                return Err(e);
            }
            let len = (n - a).min(CHUNK);
            changed += self.set_status_range(check, members, value, a, len, left)?;
            a += len;
            progress(a, n);
        }
        self.sync_waives()?;
        Ok(changed)
    }

    /// Write rule `check`'s statuses `start..start+after.len()` from
    /// `before` (as read) to `after` in one span, the counters in step
    /// (floe_oracle drc.py `_write_status_span`). A waive `after` would
    /// turn back stays waived when it is locked (`locked`), and an error
    /// `after` would waive stays as it is when its waive is rejected
    /// (`rejected`): how many of each.
    pub fn write_status_span(
        &mut self,
        check: usize,
        start: u64,
        before: &[u8],
        after: &[u8],
    ) -> Result<Kept> {
        let (_, n) = self.check(check)?;
        if before.len() != after.len() || start + after.len() as u64 > n {
            return Err(Error::input("status span outside its rule"));
        }
        let turned = |b: u8, a: u8| b == STATUS_WAIVED && a != STATUS_WAIVED;
        let mut held: Option<Vec<u8>> = None;
        let mut kept = Kept::default();
        if before.iter().zip(after).any(|(b, a)| turned(*b, *a)) {
            if let Some(locked) = self.locked(check, start, after.len() as u64)? {
                let now = held.get_or_insert_with(|| after.to_vec());
                for j in 0..now.len() {
                    if locked[j] && turned(before[j], after[j]) {
                        now[j] = STATUS_WAIVED;
                        kept.locked += 1;
                    }
                }
            }
        }
        if before.iter().zip(after).any(|(b, a)| turned(*a, *b)) {
            if let Some(rejected) = self.rejected_at(check, start, after.len() as u64, true)? {
                let now = held.get_or_insert_with(|| after.to_vec());
                for j in 0..now.len() {
                    if rejected[j] && turned(after[j], before[j]) {
                        now[j] = before[j];
                        kept.rejected += 1;
                    }
                }
            }
        }
        self.write_span(check, start, before, held.as_deref().unwrap_or(after))?;
        Ok(kept)
    }

    /// `write_status_span`'s write, nothing asked: the bytes, the rule's
    /// counter, the reasons of what changed, the change log's word - and a
    /// confirmer's own confirmation of what they turn back.
    fn write_span(&mut self, check: usize, start: u64, before: &[u8], after: &[u8]) -> Result<()> {
        let (first, _) = self.check(check)?;
        let turned = |b: u8, a: u8| b == STATUS_WAIVED && a != STATUS_WAIVED;
        let any_back = before.iter().zip(after).any(|(b, a)| turned(*b, *a));
        let waived = |s: &[u8]| s.iter().filter(|&&v| v == STATUS_WAIVED).count() as i64;
        let delta = waived(after) - waived(before);
        let (status_off, wcount_off) = (self.status_off, self.wcount_off);
        self.writer()?.write_all_at(after, status_off + first + start)?;
        // a peer's decision or a previous run's carries no reason of mine
        if self.reasons.any(Kind::Waive, check) {
            let changed: Vec<u64> = before
                .iter()
                .zip(after)
                .enumerate()
                .filter(|(_, (b, a))| b != a)
                .map(|(j, _)| start + j as u64)
                .collect();
            self.reason_drop(Kind::Waive, check, &reasons::runs_of(&changed));
        }
        if before != after && !after.is_empty() {
            self.touched(check, start, start + after.len() as u64 - 1);
        }
        if self.confirmer && any_back {
            let back: Vec<u64> = (0..after.len())
                .filter(|&j| turned(before[j], after[j]))
                .map(|j| start + j as u64)
                .collect();
            self.unconfirm(check, &back)?;
        }
        if delta != 0 {
            let cur = (i64::from(self.wcount[check]) + delta).max(0) as u32;
            self.writer()?
                .write_all_at(&cur.to_le_bytes(), wcount_off + 4 * check as u64)?;
            self.wcount[check] = cur;
            if let Some(chunks) = self.chunks.get_mut(&check) {
                for (j, (b, a)) in before.iter().zip(after).enumerate() {
                    let k = ((start + j as u64) / STATUS_CHUNK) as usize;
                    if *a == STATUS_WAIVED && *b != STATUS_WAIVED {
                        chunks[k] += 1;
                    } else if *b == STATUS_WAIVED && *a != STATUS_WAIVED {
                        chunks[k] = chunks[k].saturating_sub(1);
                    }
                }
            }
        }
        Ok(())
    }

    /// Per-chunk waived counts of one rule: built on the first filtered
    /// access with one pass, then kept in step by set_status.
    fn wf_chunks(&mut self, check: usize) -> Result<&Vec<u64>> {
        if !self.chunks.contains_key(&check) {
            let (_, n) = self.check(check)?;
            let mut counts = Vec::new();
            let mut a = 0;
            while a < n {
                let b = (a + STATUS_CHUNK).min(n);
                let st = self.statuses(check, a, b - a)?;
                counts.push(st.iter().filter(|&&s| s == STATUS_WAIVED).count() as u64);
                a = b;
            }
            self.chunks.insert(check, counts);
        }
        Ok(&self.chunks[&check])
    }

    /// The `start`..`start+limit`-th errors of one rule whose waived-ness
    /// is `waived`, as rule-local indices.
    pub fn status_page(
        &mut self,
        check: usize,
        waived: bool,
        start: u64,
        limit: u64,
    ) -> Result<Vec<u64>> {
        let (_, n) = self.check(check)?;
        if n == 0 || limit == 0 {
            return Ok(Vec::new());
        }
        let chunks = self.wf_chunks(check)?.clone();
        let mut out = Vec::new();
        let mut seen = 0u64;
        for (k, &w) in chunks.iter().enumerate() {
            let a = k as u64 * STATUS_CHUNK;
            let b = (a + STATUS_CHUNK).min(n);
            let cnt = if waived { w } else { (b - a) - w };
            if seen + cnt <= start {
                seen += cnt;
                continue;
            }
            let st = self.statuses(check, a, b - a)?;
            let skip = start.saturating_sub(seen);
            for (i, _) in st
                .iter()
                .enumerate()
                .filter(|(_, &s)| (s == STATUS_WAIVED) == waived)
                .skip(skip as usize)
                .take((limit - out.len() as u64) as usize)
            {
                out.push(a + i as u64);
            }
            seen += cnt;
            if out.len() as u64 >= limit {
                break;
            }
        }
        Ok(out)
    }

    /// Rank of `error` among the rule's errors whose waived-ness is
    /// `waived` (None when it does not match).
    pub fn status_rank(&mut self, check: usize, waived: bool, error: u64) -> Result<Option<u64>> {
        if (self.status(check, error)? == STATUS_WAIVED) != waived {
            return Ok(None);
        }
        let k = error / STATUS_CHUNK;
        let before: u64 = self.wf_chunks(check)?[..k as usize].iter().sum();
        let a = k * STATUS_CHUNK;
        let w_in = self
            .statuses(check, a, error - a)?
            .iter()
            .filter(|&&s| s == STATUS_WAIVED)
            .count() as u64;
        Ok(Some(if waived {
            before + w_in
        } else {
            (a - before) + (error - a - w_in)
        }))
    }

    /// `query` restricted to `members` too (a cluster, a delta group, a
    /// selection - of the rule the query is limited to), before the cap.
    pub fn query_scoped(
        &mut self,
        bbox_um: [f64; 4],
        cap: usize,
        checks: Option<&BTreeSet<usize>>,
        waived: Option<bool>,
        members: Option<&dyn super::analysis::scope::Membership>,
        cancelled: &AtomicUsize,
    ) -> Result<Vec<(usize, u64, Violation)>> {
        let Some(members) = members else {
            return self.query(bbox_um, cap, checks, waived, cancelled);
        };
        let mut out = Vec::new();
        let mut cursor = super::Cursor::default();
        while out.len() < cap {
            check_cancelled(cancelled)?;
            let page = self.pack.query(bbox_um, checks, None, cursor, super::PAGE_ITEMS, cancelled)?;
            for hit in page.hits {
                if !members.contains(hit.local) {
                    continue;
                }
                if let Some(w) = waived {
                    if (self.status(hit.check, hit.local)? == STATUS_WAIVED) != w {
                        continue;
                    }
                }
                out.push((hit.check, hit.local, hit.violation));
                if out.len() >= cap {
                    break;
                }
            }
            match page.next {
                Some(next) => cursor = next,
                None => break,
            }
        }
        Ok(out)
    }

    /// Errors of `checks` (None: all) whose geometry meets the um rect, at
    /// most `cap`, file order; `waived` filters by review status before the
    /// cap (floe/drc.py query_rect).
    pub fn query(
        &mut self,
        bbox_um: [f64; 4],
        cap: usize,
        checks: Option<&BTreeSet<usize>>,
        waived: Option<bool>,
        cancelled: &AtomicUsize,
    ) -> Result<Vec<(usize, u64, Violation)>> {
        let mut out = Vec::new();
        let mut cursor = super::Cursor::default();
        let checks = match (waived, checks) {
            // a rule with no waived error has nothing to give
            (Some(true), only) => Some(
                (0..self.pack.checks.len())
                    .filter(|c| only.is_none_or(|o| o.contains(c)) && self.wcount[*c] > 0)
                    .collect::<BTreeSet<_>>(),
            ),
            (_, only) => only.cloned(),
        };
        while out.len() < cap {
            check_cancelled(cancelled)?;
            let page = self.pack.query(
                bbox_um,
                checks.as_ref(),
                None,
                cursor,
                super::PAGE_ITEMS,
                cancelled,
            )?;
            for hit in page.hits {
                if let Some(w) = waived {
                    if (self.status(hit.check, hit.local)? == STATUS_WAIVED) != w {
                        continue;
                    }
                }
                out.push((hit.check, hit.local, hit.violation));
                if out.len() >= cap {
                    break;
                }
            }
            match page.next {
                Some(next) => cursor = next,
                None => break,
            }
        }
        Ok(out)
    }

    // ---- notes ---------------------------------------------------------

    /// The note shared by global error `gid`, if any.
    pub fn note(&self, gid: u64) -> Option<&str> {
        self.note_of
            .get(&gid)
            .map(|nid| self.notes[nid].text.as_str())
    }

    /// Every note with members, in note order.
    pub fn notes(&self) -> Vec<&Note> {
        self.notes
            .values()
            .filter(|n| !n.members.is_empty())
            .collect()
    }

    fn detach(&mut self, gids: &[u64]) {
        for g in gids {
            if let Some(nid) = self.note_of.remove(g) {
                let empty = {
                    let note = self.notes.get_mut(&nid).expect("note");
                    note.members.remove(g);
                    note.members.is_empty()
                };
                if empty {
                    self.notes.remove(&nid);
                }
            }
        }
    }

    /// One shared note on `gids` (each detached from its earlier note
    /// first); empty text only clears. Ids out of range are dropped.
    pub fn set_note(&mut self, gids: &[u64], text: &str, cancelled: &AtomicUsize) -> Result<()> {
        let gids: Vec<u64> = gids
            .iter()
            .copied()
            .filter(|&g| g < self.pack.total)
            .collect();
        self.detach(&gids);
        let text = text.trim();
        if !text.is_empty() && !gids.is_empty() {
            let nid = self.note_next;
            self.note_next += 1;
            self.notes.insert(
                nid,
                Note {
                    text: text.to_string(),
                    members: gids.iter().copied().collect(),
                },
            );
            for g in gids {
                self.note_of.insert(g, nid);
            }
        }
        self.write_notes(cancelled)
    }

    pub fn clear_note(&mut self, gids: &[u64], cancelled: &AtomicUsize) -> Result<()> {
        let gids: Vec<u64> = gids
            .iter()
            .copied()
            .filter(|&g| g < self.pack.total)
            .collect();
        self.detach(&gids);
        self.write_notes(cancelled)
    }

    /// The um centre of an error, the anchor of its note's text line.
    fn center(&mut self, gid: u64, cancelled: &AtomicUsize) -> [f64; 2] {
        let found = self
            .pack
            .checks
            .iter()
            .enumerate()
            .rfind(|(_, c)| c.start <= gid && gid < c.start + c.count)
            .map(|(i, c)| (i, gid - c.start));
        let precision = self.pack.precision;
        found
            .and_then(|(ci, local)| self.pack.error(ci, local, cancelled).ok())
            .map(|v| {
                let x0 = v
                    .points
                    .iter()
                    .map(|p| p[0] as f64 / precision)
                    .fold(f64::INFINITY, f64::min);
                let x1 = v
                    .points
                    .iter()
                    .map(|p| p[0] as f64 / precision)
                    .fold(f64::NEG_INFINITY, f64::max);
                let y0 = v
                    .points
                    .iter()
                    .map(|p| p[1] as f64 / precision)
                    .fold(f64::INFINITY, f64::min);
                let y1 = v
                    .points
                    .iter()
                    .map(|p| p[1] as f64 / precision)
                    .fold(f64::NEG_INFINITY, f64::max);
                [(x0 + x1) / 2., (y0 + y1) / 2.]
            })
            .filter(|c| c.iter().all(|v| v.is_finite()))
            .unwrap_or([0., 0.])
    }

    /// The notes' flateyes .fe text, None when there are none
    /// (floe/drc.py `_serialize_notes`, byte for byte).
    pub fn serialize_notes(&mut self, cancelled: &AtomicUsize) -> Result<Option<String>> {
        let notes: Vec<Note> = self.notes().into_iter().cloned().collect();
        if notes.is_empty() {
            return Ok(None);
        }
        let mut out = format!(
            "# flateyes annotations\nfloe_pack={}\nppu=1\nunit=um\n",
            self.note_tag
        );
        for note in &notes {
            let ids: Vec<String> = note.members.iter().map(u64::to_string).collect();
            out.push_str(&format!(
                "floe_note={}|{}\n",
                ids.join(","),
                annotations::escape(&note.text)
            ));
        }
        for note in &notes {
            for &gid in &note.members {
                check_cancelled(cancelled)?;
                let [x, y] = self.center(gid, cancelled);
                out.push_str(&format!(
                    "text={},{},16,{NOTE_COLOR},#00000059,{}\n",
                    annotations::coord(x),
                    annotations::coord(y),
                    annotations::escape(&note.text)
                ));
            }
        }
        Ok(Some(out))
    }

    /// Persist the notes (or remove the file when none remain); a results
    /// folder that refuses switches once to the temp folder.
    fn write_notes(&mut self, cancelled: &AtomicUsize) -> Result<()> {
        let Some(text) = self.serialize_notes(cancelled)? else {
            let _ = fs::remove_file(&self.note_path);
            return Ok(());
        };
        if let Err(e) = atomic_write(&self.note_path, text.as_bytes()) {
            if self.note_path != self.note_beside {
                return Err(e.into());
            }
            self.note_path = self.note_temp.clone();
            atomic_write(&self.note_path, text.as_bytes())?;
            self.notices.push(format!(
                "[drc] results folder not writable; note autosave in {} (save notes explicitly to keep them)",
                self.note_path.display()
            ));
        }
        Ok(())
    }

    /// The notes of `text`: false when its pack fingerprint is another
    /// pack's (floe/drc.py `_parse_notes` - malformed lines skipped).
    fn parse_notes(&mut self, text: &str) -> bool {
        let mut notes = BTreeMap::new();
        let mut note_of = HashMap::new();
        let mut nid = 1u64;
        let mut tag_ok = true;
        for raw in text.lines() {
            let line = raw.trim();
            if let Some(tag) = line.strip_prefix("floe_pack=") {
                tag_ok = tag == self.note_tag;
            } else if let Some(body) = line.strip_prefix("floe_note=") {
                let Some((ids, txt)) = body.split_once('|') else {
                    continue;
                };
                let members: BTreeSet<u64> = ids
                    .split(',')
                    .map(str::trim)
                    .filter(|t| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()))
                    .filter_map(|t| t.parse::<u64>().ok())
                    .filter(|&g| g < self.pack.total)
                    .collect();
                let txt = annotations::unescape(txt).trim().to_string();
                if members.is_empty() || txt.is_empty() {
                    continue;
                }
                for &g in &members {
                    note_of.insert(g, nid);
                }
                notes.insert(nid, Note { text: txt, members });
                nid += 1;
            }
        }
        if !tag_ok {
            return false;
        }
        self.notes = notes;
        self.note_of = note_of;
        self.note_next = nid;
        true
    }

    /// The notes autosave: beside the pack, else the temp folder's; another
    /// pack's file is moved aside.
    fn load_notes(&mut self) {
        for path in [self.note_beside.clone(), self.note_temp.clone()] {
            let Ok(text) = fs::read(&path) else { continue };
            let text = String::from_utf8_lossy(&text).into_owned();
            if self.parse_notes(&text) {
                self.note_path = path;
            } else {
                let aside = aside_path(&path);
                if fs::rename(&path, &aside).is_ok() {
                    self.notices.push(format!(
                        "[drc] note sidecar does not match this pack (DRC re-run?); moved aside: {}",
                        aside.display()
                    ));
                }
                self.notes.clear();
                self.note_of.clear();
                self.note_next = 1;
            }
            return;
        }
    }

    /// Save the notes to `dst` (an empty set removes it).
    pub fn note_export(&mut self, dst: &Path, cancelled: &AtomicUsize) -> Result<()> {
        match self.serialize_notes(cancelled)? {
            Some(text) => Ok(atomic_write(dst, text.as_bytes())?),
            None => {
                let _ = fs::remove_file(dst);
                Ok(())
            }
        }
    }

    /// Replace the notes from a saved .fe; another pack's file is refused.
    /// The note count.
    pub fn note_import(&mut self, src: &Path, cancelled: &AtomicUsize) -> Result<usize> {
        let text = String::from_utf8_lossy(&fs::read(src)?).into_owned();
        if !self.parse_notes(&text) {
            return Err(Error::input(format!(
                "{}: note file does not match this pack (different DRC run / re-packed db?)",
                src.display()
            )));
        }
        self.write_notes(cancelled)?;
        Ok(self.notes.len())
    }

    // ---- waive files ---------------------------------------------------

    /// Save the review state to `dst`, the autosave's format.
    pub fn waive_export(&self, dst: &Path) -> Result<()> {
        let mut data = self.header.to_vec();
        let mut status = vec![0u8; self.pack.total as usize];
        self.reader.read_exact_at(&mut status, self.status_off)?;
        data.extend_from_slice(&status);
        for w in &self.wcount {
            data.extend_from_slice(&w.to_le_bytes());
        }
        Ok(atomic_write(dst, &data)?)
    }

    /// Replace the review state from a saved file of this pack (header and
    /// size); the counters recomputed. The waived count, and what stays as
    /// it was: the waives the file would have turned back that are locked
    /// (`locked`), and the errors it would have waived whose waive is
    /// rejected (`rejected`).
    pub fn waive_import(&mut self, src: &Path) -> Result<(u64, Kept)> {
        let data = fs::read(src)?;
        let n = self.pack.total as usize;
        let checks = self.pack.checks.len();
        if data.len() != HEADER as usize + n + 4 * checks || data[..HEADER as usize] != self.header
        {
            return Err(Error::input(format!(
                "{}: waive file does not match this pack (different DRC run / re-packed db?)",
                src.display()
            )));
        }
        let mut status = data[HEADER as usize..HEADER as usize + n].to_vec();
        // what the file would turn back: a locked waive stays, and a
        // confirmer's own confirmation goes with the waive it turns back
        let mut current = vec![0u8; n];
        self.reader.read_exact_at(&mut current, self.status_off)?;
        let rules: Vec<(usize, usize)> = self
            .pack
            .checks
            .iter()
            .map(|c| (c.start as usize, c.count as usize))
            .collect();
        let mut kept = Kept::default();
        for (check, &(first, count)) in rules.iter().enumerate() {
            // an error the file would waive whose waive is rejected stays
            // as it is (a rejected waive this review has yet to turn back
            // is `take_rejected`'s)
            if (first..first + count).any(|g| status[g] == STATUS_WAIVED) {
                if let Some(rejected) = self.rejected_at(check, 0, count as u64, true)? {
                    for j in 0..count {
                        if rejected[j] && status[first + j] == STATUS_WAIVED && current[first + j] != STATUS_WAIVED {
                            status[first + j] = current[first + j];
                            kept.rejected += 1;
                        }
                    }
                }
            }
            let turned = |g: usize, status: &[u8]| current[g] == STATUS_WAIVED && status[g] != STATUS_WAIVED;
            if !(first..first + count).any(|g| turned(g, &status)) {
                continue;
            }
            if self.confirmer {
                if self.confirms.present() && self.confirms.counts()[check] > 0 {
                    let was = self.confirms.statuses(check, 0, count as u64)?;
                    let mut now = was.clone();
                    for j in (0..count).filter(|&j| turned(first + j, &status)) {
                        now[j] = 0;
                    }
                    self.confirms.write_span(check, 0, &was, &now)?;
                }
            } else if let Some(locked) = self.locked(check, 0, count as u64)? {
                for j in 0..count {
                    if locked[j] && turned(first + j, &status) {
                        status[first + j] = STATUS_WAIVED;
                        kept.locked += 1;
                    }
                }
            }
        }
        drop(current);
        let status = &status[..];
        let counts: Vec<u32> = rules
            .iter()
            .map(|&(first, count)| {
                status[first..first + count]
                    .iter()
                    .filter(|&&s| s == STATUS_WAIVED)
                    .count() as u32
            })
            .collect();
        let (status_off, wcount_off) = (self.status_off, self.wcount_off);
        let w = self.writer()?;
        if n > 0 {
            w.write_all_at(status, status_off)?;
        }
        let raw: Vec<u8> = counts.iter().flat_map(|c| c.to_le_bytes()).collect();
        if !raw.is_empty() {
            w.write_all_at(&raw, wcount_off)?;
        }
        self.wcount = counts;
        self.chunks.clear();
        // a reason stays with an error the loaded file has waived too, and
        // goes with one it turns back
        for check in self.reasons.checks(Kind::Waive) {
            let first = self.pack.checks[check].start as usize;
            let mut back = Vec::new();
            for (a, b, _) in self.reasons.runs(Kind::Waive, check) {
                back.extend((a..b).filter(|&e| status[first + e as usize] != STATUS_WAIVED));
            }
            self.reason_drop(Kind::Waive, check, &reasons::runs_of(&back));
        }
        // (every status may have changed: the team counts this review again)
        self.changes.lock().unwrap_or_else(|p| p.into_inner()).everything();
        self.sync_waives()?;
        Ok((status.iter().filter(|&&s| s == STATUS_WAIVED).count() as u64, kept))
    }
}

/// Status bytes read apart from the review (worker threads).
#[derive(Clone)]
pub struct StatusReader {
    file: std::sync::Arc<File>,
    status_off: u64,
    checks: std::sync::Arc<Vec<(u64, u64)>>,
}
impl StatusReader {
    /// Check `check`'s status bytes `start..start+count` (clamped).
    pub fn statuses(&self, check: usize, start: u64, count: u64) -> Result<Vec<u8>> {
        let &(first, n) = self
            .checks
            .get(check)
            .ok_or_else(|| Error::input("DRC check index out of range"))?;
        let start = start.min(n);
        let count = count.min(n - start);
        let mut out = vec![0u8; count as usize];
        self.file
            .read_exact_at(&mut out, self.status_off + first + start)?;
        Ok(out)
    }
}

/// One rule as the viewer lists it.
#[derive(Clone, Debug, serde::Serialize)]
pub struct CheckRow {
    pub name: String,
    pub desc: String,
    /// the count the results file declares (a number where it is one)
    pub declared: serde_json::Value,
    pub count: u64,
    /// the global id of its first error
    pub start: u64,
}

/// One error: kind ('p' polygon, 'e' edge), its global 1-based number and
/// its points in micrometres (floe/drc.py DrcError).
#[derive(Clone, Debug, PartialEq)]
pub struct ErrorRow {
    pub kind: char,
    pub number: u64,
    pub points: Vec<[f64; 2]>,
}

fn row(v: &Violation, precision: f64) -> ErrorRow {
    ErrorRow {
        kind: v.kind,
        number: v.number,
        points: v
            .points
            .iter()
            .map(|p| [p[0] as f64 / precision, p[1] as f64 / precision])
            .collect(),
    }
}

/// An open results database: a pack under review, or an ASCII file read
/// whole (no review state, no spatial query - as floe/drc.py DrcDb).
pub enum Opened {
    Pack(Box<Review>),
    Ascii(std::sync::Arc<super::Ascii>),
}

impl Opened {
    /// floe/drc.py load_db: a pack given (by its magic), or the .db's pack
    /// when it is current, else the ASCII file itself - the reason said.
    pub fn load(
        path: &Path,
        reviewer: Option<&str>,
        cancelled: &AtomicUsize,
    ) -> Result<(Self, Vec<String>)> {
        let mut magic = [0u8; 8];
        let packed = File::open(path)?.read_exact(&mut magic).is_ok() && &magic == super::MAGIC;
        if packed {
            return Ok((
                Self::Pack(Box::new(Review::open(path, None, reviewer, cancelled)?)),
                Vec::new(),
            ));
        }
        let mut warnings = Vec::new();
        let side = crate::cache::pack_path(path)?;
        if side.exists() {
            match Review::open_with(&side, Some(path), reviewer, cancelled, true) {
                Ok(review) => return Ok((Self::Pack(Box::new(review)), Vec::new())),
                // a pack being rebuilt, a review open in another session, a
                // reviewer id required and not given: said as they are, never
                // answered with the ASCII file (which has no review)
                Err(e)
                    if matches!(
                        e.kind,
                        ErrorKind::Busy | ErrorKind::Cancelled | ErrorKind::Admission
                    ) =>
                {
                    return Err(e)
                }
                Err(e) => warnings.push(format!(
                    "[drc] {}: {e}; parsing ASCII instead (rerun: floe-index drc {})",
                    side.display(),
                    path.display()
                )),
            }
        }
        Ok((
            Self::Ascii(std::sync::Arc::new(super::Ascii::open(path, cancelled)?)),
            warnings,
        ))
    }

    pub fn path(&self) -> &Path {
        match self {
            Self::Pack(r) => &r.pack.path,
            Self::Ascii(a) => &a.path,
        }
    }
    pub fn cell(&self) -> &str {
        match self {
            Self::Pack(r) => &r.pack.cell,
            Self::Ascii(a) => &a.cell,
        }
    }
    pub fn precision(&self) -> f64 {
        match self {
            Self::Pack(r) => r.pack.precision,
            Self::Ascii(a) => a.precision,
        }
    }
    pub fn total(&self) -> u64 {
        match self {
            Self::Pack(r) => r.pack.total,
            Self::Ascii(a) => a.total,
        }
    }

    pub fn checks(&self) -> Vec<CheckRow> {
        let declared = |text: &str| {
            text.trim()
                .parse::<i64>()
                .map(serde_json::Value::from)
                .unwrap_or_else(|_| serde_json::Value::from(text))
        };
        match self {
            Self::Pack(r) => r
                .pack
                .checks
                .iter()
                .map(|c| CheckRow {
                    name: c.name.clone(),
                    desc: c.desc.clone(),
                    declared: serde_json::Value::from(c.declared),
                    count: c.count,
                    start: c.start,
                })
                .collect(),
            Self::Ascii(a) => a
                .checks
                .iter()
                .map(|c| CheckRow {
                    name: c.name.clone(),
                    desc: c.desc.clone(),
                    declared: declared(&c.declared),
                    count: c.count,
                    start: c.start,
                })
                .collect(),
        }
    }

    fn count(&self, check: usize) -> Result<u64> {
        let count = match self {
            Self::Pack(r) => r.pack.checks.get(check).map(|c| c.count),
            Self::Ascii(a) => a.checks.get(check).map(|c| c.count),
        };
        count.ok_or_else(|| Error::input("DRC check index out of range"))
    }

    /// Check `check`'s errors `start..start+count` (clamped), in order.
    pub fn errors(
        &mut self,
        check: usize,
        start: u64,
        count: u64,
        cancelled: &AtomicUsize,
    ) -> Result<Vec<ErrorRow>> {
        let n = self.count(check)?;
        let end = start.saturating_add(count).min(n);
        let mut out = Vec::new();
        let mut i = start.min(n);
        while i < end {
            check_cancelled(cancelled)?;
            match self {
                Self::Pack(r) => {
                    let precision = r.pack.precision;
                    let limit = ((end - i) as usize).min(super::PAGE_ITEMS);
                    let page = r.pack.errors(check, i, limit, cancelled)?;
                    if page.hits.is_empty() {
                        break;
                    }
                    for hit in page.hits {
                        if hit.local >= end {
                            break;
                        }
                        out.push(row(&hit.violation, precision));
                        i = hit.local + 1;
                    }
                }
                Self::Ascii(a) => {
                    let v = a.error(check, i, cancelled)?;
                    out.push(ErrorRow {
                        kind: v.kind,
                        number: v.number,
                        points: v.points_um,
                    });
                    i += 1;
                }
            }
        }
        Ok(out)
    }

    /// The CD ruler segments of one simple error, um (floe/drc.py
    /// cd_segments): none for complex shapes.
    pub fn cd(
        &mut self,
        check: usize,
        error: u64,
        cancelled: &AtomicUsize,
    ) -> Result<Vec<[f64; 4]>> {
        let segments = match self {
            Self::Pack(r) => {
                let v = r.pack.error(check, error, cancelled)?;
                super::cd_segments(v.kind, &v.points, r.pack.precision)?
            }
            Self::Ascii(a) => {
                let v = a.error(check, error, cancelled)?;
                super::cd_segments_um(v.kind, &v.points_um, cancelled)?
            }
        };
        Ok(segments
            .iter()
            .map(|s| {
                [
                    s.endpoints_um[0][0],
                    s.endpoints_um[0][1],
                    s.endpoints_um[1][0],
                    s.endpoints_um[1][1],
                ]
            })
            .collect())
    }
}

/// The ErrorRow of a query hit.
pub fn query_rows(
    review: &Review,
    hits: Vec<(usize, u64, Violation)>,
) -> Vec<(usize, u64, ErrorRow)> {
    let precision = review.pack.precision;
    hits.into_iter()
        .map(|(c, e, v)| (c, e, row(&v, precision)))
        .collect()
}
