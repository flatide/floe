//! A reviewer's change log `.<db>.changes.<reviewer>`: which errors of which
//! rule the reviewer's review changed, said after each change - so that the
//! team's counts (the union of every reviewer's review: drc::team::live) can
//! be brought up to date by reading what changed and nothing else (user
//! 2026-11-13: ".db를 여러 사람이 검토하고 그 합집합을 실시간으로 받아야 해").
//!
//! A reviewer's own counts are kept by the review files themselves (the waive
//! sidecar's counter a rule, the flags' counters a rule). The team's are not a
//! sum of those - an error two reviewers waived is one - so they need each
//! error's bits of every reviewer, and counting them all again for every
//! answer is the cost `floe2 drc --review-stats` has. This log makes the
//! difference small: a chunk of 4,096 errors whose bits did not change keeps
//! the team count it had.
//!
//! One writer - the reviewer's session - appends to its own file; nobody
//! else writes it, so there is nothing shared to lock between reviewers.
//!
//! ```text
//! header, 48 bytes: "FLOECHGS" | u32 1 | 16 B content id prefix | u64 errors
//!                   | u32 rules | u64 log id
//! records, 24 bytes each: u32 kind | u32 rule | u64 a | u64 b
//!   1 TOUCH  rule's chunks a..=b may have changed (a chunk = 4,096 errors)
//!   2 STAMP  the reviewer's files as these changes left them: a = the waive
//!            sidecar's time, b = the flags file's (ns since the epoch, 0 =
//!            no file)
//!   3 RESET  the files changed without being said here (a session of an
//!            older build, a run that ended before it could say): whoever
//!            reads counts this reviewer's rules again
//!   4 CSTAMP the reviewer's confirm file as these changes left it: a = its
//!            time (0 = no file). Said after a STAMP, and only when the time
//!            is another than the one said last - a reviewer who confirms
//!            nothing never says it (0.12.466; a reader of 0.12.465 passes
//!            it over). b = the time of the reviewer's rejects log likewise
//!            (0.12.468; 0 until then, and to a reader of before)
//!   5 QSTAMP the reviewer's request file as these changes left it: a = its
//!            time (0 = no file). Said likewise, only when the time is
//!            another - a reviewer who asks for no waive never says it
//!            (0.12.472; a reader of before passes it over)
//! ```
//!
//! The records of a change are appended when the change is settled - the end
//! of the request or, in a long job, about once a second - after the bits
//! are on the disk, with the STAMP that says how the files stand then:
//! statuses are said by the review's own settle, which follows their flush
//! (`touched`, `settle`), flags once their transaction is committed
//! (`committed`, said by whichever settle comes first). A reader that finds
//! the files newer than the last STAMP and no record to explain it waits a
//! little for the record (a writer between its two writes), and counts again
//! when none comes: the log makes answers cheap, it is never what makes them
//! right.
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

pub const MAGIC: &[u8; 8] = b"FLOECHGS";
pub const VERSION: u32 = 1;
pub const HEADER: u64 = 48;
pub const RECORD: u64 = 24;
pub const TOUCH: u32 = 1;
pub const STAMP: u32 = 2;
pub const RESET: u32 = 3;
pub const CSTAMP: u32 = 4;
pub const QSTAMP: u32 = 5;
/// The errors of one chunk (the review flags' and the team counts' unit).
pub const CHUNK: u64 = 4096;
/// A log past this size is started again at the next open (its readers count
/// its reviewer's rules once more).
const ROTATE: u64 = 16 * 1024 * 1024;

/// FLOE_TEAM_KEEP: on unless `off` (`0`, `no`, `false`) - the kill switch of
/// what 0.12.465 writes for the team's counts. Off, a session says nothing
/// in a change log and removes the one it has (its reviewer is then one
/// without a log: counted by their files' times), and a merge keeps nothing
/// (`team::live`): the team is counted for each answer.
pub fn keep_of(value: Option<&str>) -> bool {
    !matches!(
        value.unwrap_or_default().trim().to_ascii_lowercase().as_str(),
        "off" | "0" | "no" | "false"
    )
}

/// `keep_of` what FLOE_TEAM_KEEP says here.
pub fn enabled() -> bool {
    keep_of(std::env::var("FLOE_TEAM_KEEP").ok().as_deref())
}

/// A file's time as a stamp says it: ns since the epoch, 0 for no file.
pub fn stamp_of(meta: &fs::Metadata) -> u64 {
    let ns = i128::from(meta.mtime()) * 1_000_000_000 + i128::from(meta.mtime_nsec());
    u64::try_from(ns).unwrap_or(0).max(1)
}

/// `stamp_of` the file at `path` (0: none).
pub fn stamp_at(path: &Path) -> u64 {
    fs::metadata(path).map(|m| stamp_of(&m)).unwrap_or(0)
}

/// A reviewer's files' times as the log says them: the waive sidecar's,
/// the flags file's, the confirm file's, the rejects log's, the request
/// file's (0: no file).
pub type Stamps = [u64; 5];

/// The times the log has said once `records` follow a log that had said
/// `base`: the last STAMP's two, the last CSTAMP's two and the last
/// QSTAMP's.
pub fn stamps_after(base: Stamps, records: &[Record]) -> Stamps {
    let mut said = base;
    for record in records {
        match record.kind {
            STAMP => (said[0], said[1]) = (record.a, record.b),
            CSTAMP => (said[2], said[3]) = (record.a, record.b),
            QSTAMP => said[4] = record.a,
            _ => (),
        }
    }
    said
}

/// One record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record {
    pub kind: u32,
    pub rule: u32,
    pub a: u64,
    pub b: u64,
}

impl Record {
    fn bytes(self) -> [u8; RECORD as usize] {
        let mut out = [0u8; RECORD as usize];
        out[0..4].copy_from_slice(&self.kind.to_le_bytes());
        out[4..8].copy_from_slice(&self.rule.to_le_bytes());
        out[8..16].copy_from_slice(&self.a.to_le_bytes());
        out[16..24].copy_from_slice(&self.b.to_le_bytes());
        out
    }
    fn of(b: &[u8]) -> Self {
        Self {
            kind: u32::from_le_bytes(b[0..4].try_into().unwrap()),
            rule: u32::from_le_bytes(b[4..8].try_into().unwrap()),
            a: u64::from_le_bytes(b[8..16].try_into().unwrap()),
            b: u64::from_le_bytes(b[16..24].try_into().unwrap()),
        }
    }
}

/// The log's name, from the waive sidecar's (same folder, reviewer and - in
/// the temp folder - path hash). A name that is no waive sidecar's gets
/// `.changes` after it: the log is never the file it was named from.
///
/// A request file's name gives the same log (a reviewer who only asks has no
/// waive sidecar to name it from): one log a reviewer, whatever they do.
pub fn path_beside(waive: &Path) -> PathBuf {
    let name = waive.file_name().unwrap_or_default().to_string_lossy();
    if name.contains(".waive.") {
        waive.with_file_name(name.replacen(".waive.", ".changes.", 1))
    } else if name.contains(".request.") {
        waive.with_file_name(name.replacen(".request.", ".changes.", 1))
    } else {
        waive.with_file_name(format!("{name}.changes"))
    }
}

/// What a reader takes from a log: its id, how far it reads (whole records),
/// the records after `from`, and the last STAMP among them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tail {
    pub log_id: u64,
    pub len: u64,
    pub records: Vec<Record>,
}

/// Read the log at `path` of the result `identity` (the waive header's bytes
/// 12..40: content id prefix, errors, rules) from offset `from` on - the
/// whole of it when `from` is not a record boundary inside it or the log is
/// not `log_id`'s (then `Tail::records` starts at the first record).
/// None: no log, or not this result's.
pub fn read(path: &Path, identity: &[u8; 28], known: Option<(u64, u64)>) -> Option<Tail> {
    let mut file = File::open(path).ok()?;
    let mut head = [0u8; HEADER as usize];
    file.read_exact(&mut head).ok()?;
    if &head[..8] != MAGIC
        || u32::from_le_bytes(head[8..12].try_into().unwrap()) != VERSION
        || head[12..40] != identity[..]
    {
        return None;
    }
    let log_id = u64::from_le_bytes(head[40..48].try_into().unwrap());
    let size = file.metadata().ok()?.len();
    let len = HEADER + (size.saturating_sub(HEADER) / RECORD) * RECORD;
    let from = match known {
        Some((id, at)) if id == log_id && at >= HEADER && at <= len && (at - HEADER) % RECORD == 0 => at,
        _ => HEADER,
    };
    let mut bytes = vec![0u8; (len - from) as usize];
    file.seek(SeekFrom::Start(from)).ok()?;
    file.read_exact(&mut bytes).ok()?;
    Some(Tail {
        log_id,
        len,
        records: bytes.chunks_exact(RECORD as usize).map(Record::of).collect(),
    })
}

/// Hand what was appended to the file system: what makes a record readable
/// from another host of an NFS folder now, not at the kernel's next
/// writeback. The log needs no more - a record a host loses is one its
/// readers count again, the files' times say so - so on macOS this is the
/// plain fsync and not the drive flush `File::sync_data` asks for there
/// (about 10 ms a record: a visit, a click).
fn flush(file: &File) -> std::io::Result<()> {
    #[cfg(target_vendor = "apple")]
    {
        use std::os::unix::io::AsRawFd;
        if unsafe { libc::fsync(file.as_raw_fd()) } == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
    #[cfg(not(target_vendor = "apple"))]
    {
        file.sync_data()
    }
}

/// The writer's side: a reviewer's session saying what it changed.
pub struct ChangeLog {
    pub path: PathBuf,
    identity: [u8; 28],
    /// the files a stamp says the times of
    waive: PathBuf,
    flags: Option<PathBuf>,
    confirm: PathBuf,
    /// the reviewer's rejects log (drc::reasons, a confirmer's)
    rejects: PathBuf,
    /// the reviewer's request file (drc::confirm, a requester's)
    request: PathBuf,
    file: Option<File>,
    /// statuses written and not said yet - they are said once flushed, by
    /// `settle`: rule -> inclusive chunk ranges
    written: BTreeMap<u32, Vec<(u64, u64)>>,
    /// flags committed and not said yet: said by the next settle of either
    committed: BTreeMap<u32, Vec<(u64, u64)>>,
    /// a RESET to say with the next records
    reset: bool,
    /// the times said last, or found said at the open (None: no STAMP yet)
    last: Option<Stamps>,
    /// flush what is said to the disk (FLOE_WAIVE_SYNC, as the waive writes)
    sync: bool,
    /// nothing is said (the review is read-only: no file of its own)
    off: bool,
    pub notices: Vec<String>,
}

impl ChangeLog {
    /// The log beside the waive sidecar `waive` of the result `identity`.
    /// Nothing is made by the open. A log of another result, a torn one or
    /// one grown past its size is removed: its readers count again.
    pub fn open(waive: &Path, identity: [u8; 28], sync: bool) -> Self {
        let path = path_beside(waive);
        let mut me = Self {
            path: path.clone(),
            identity,
            waive: waive.to_owned(),
            flags: None,
            confirm: super::confirm::path_beside(waive),
            rejects: super::confirm::rejects_beside(waive),
            request: super::confirm::request_beside(waive),
            file: None,
            written: BTreeMap::new(),
            committed: BTreeMap::new(),
            reset: false,
            last: None,
            sync,
            off: false,
            notices: Vec::new(),
        };
        match fs::metadata(&path) {
            Ok(m) if m.len() <= ROTATE => match read(&path, &identity, None) {
                Some(tail) => {
                    if tail.len != m.len() {
                        // a torn last record: cut, the records before it stand
                        if let Ok(f) = OpenOptions::new().write(true).open(&path) {
                            let _ = f.set_len(tail.len);
                        }
                    }
                    if tail.records.iter().any(|r| r.kind == STAMP) {
                        me.last = Some(stamps_after([0; 5], &tail.records));
                    }
                }
                None => {
                    let _ = fs::remove_file(&path);
                }
            },
            Ok(_) => {
                let _ = fs::remove_file(&path);
            }
            Err(_) => (),
        }
        me
    }

    /// A review that writes nothing of its own (the pack's embedded statuses
    /// shown read-only) says nothing.
    pub fn off(mut self) -> Self {
        self.off = true;
        self
    }

    /// The log of a session that is not to keep one (FLOE_TEAM_KEEP=off):
    /// nothing is said, and a log already beside `waive` is removed - left
    /// there it would say less than its files do from now on, and its
    /// readers would wait for records that never come.
    pub fn silent(waive: &Path) -> Self {
        let _ = fs::remove_file(path_beside(waive));
        Self::none()
    }

    /// The log of a review without a sidecar of its own: no file is looked
    /// at, made or removed, and nothing is said.
    pub fn none() -> Self {
        Self {
            path: PathBuf::new(),
            identity: [0; 28],
            waive: PathBuf::new(),
            flags: None,
            confirm: PathBuf::new(),
            rejects: PathBuf::new(),
            request: PathBuf::new(),
            file: None,
            written: BTreeMap::new(),
            committed: BTreeMap::new(),
            reset: false,
            last: None,
            sync: false,
            off: true,
            notices: Vec::new(),
        }
    }

    /// The reviewer's rejects log is at `rejects` (it goes to the temp
    /// folder when the results folder refuses it): where the next stamp
    /// looks.
    pub fn rejects_at(&mut self, rejects: &Path) {
        if !self.off && self.rejects != rejects {
            self.rejects = rejects.to_owned();
        }
    }

    /// Turn the flushes on or off (as the review's waive writes are).
    pub fn set_sync(&mut self, on: bool) {
        self.sync = on;
    }

    fn stamps(&self) -> Stamps {
        [
            stamp_at(&self.waive),
            self.flags.as_deref().map_or(0, stamp_at),
            stamp_at(&self.confirm),
            stamp_at(&self.rejects),
            stamp_at(&self.request),
        ]
    }

    /// The session knows its flags file now (`flags`), before it has written
    /// anything: files that are not as the last STAMP left them were changed
    /// without being said here - by a session of an older build, by a run
    /// that ended before it could say - and a RESET says so at once, so the
    /// next reader counts this reviewer's rules again. A reviewer without a
    /// log has nothing to compare: their first record starts one, which a
    /// reader takes for a new log.
    pub fn attach_flags(&mut self, flags: &Path) {
        self.flags = Some(flags.to_owned());
        if self.off {
            return;
        }
        if let Some(last) = self.last {
            if last != self.stamps() {
                self.reset = true;
                self.settle();
            }
        }
    }

    /// The flags file is at `flags` (the store may have gone to its durable
    /// fallback at its first write): where the next stamp looks.
    pub fn flags_at(&mut self, flags: &Path) {
        if self.flags.as_deref() != Some(flags) {
            self.flags = Some(flags.to_owned());
        }
    }

    fn note(ranges: &mut Vec<(u64, u64)>, first: u64, last: u64) {
        match ranges.last_mut() {
            // (the usual case: a job walking a rule upwards)
            Some(r) if first <= r.1.saturating_add(1) && last >= r.0 => {
                r.0 = r.0.min(first);
                r.1 = r.1.max(last);
            }
            _ => ranges.push((first, last)),
        }
    }

    /// Statuses of rule `rule`'s chunks `first..=last` were written: said
    /// at `settle`, which the review calls once they are flushed.
    pub fn touched(&mut self, rule: usize, first: u64, last: u64) {
        if !self.off && first <= last {
            Self::note(self.written.entry(rule as u32).or_default(), first, last);
        }
    }

    /// Flags of rule `rule`'s chunks `first..=last` were changed, and their
    /// transaction is committed: said at the next settle, whoever's.
    pub fn committed(&mut self, rule: usize, first: u64, last: u64) {
        if !self.off && first <= last {
            Self::note(self.committed.entry(rule as u32).or_default(), first, last);
        }
    }

    /// Rule `rule`'s errors `errors` were changed.
    pub fn touched_errors(&mut self, rule: usize, errors: &[u64]) {
        for &e in errors {
            self.touched(rule, e / CHUNK, e / CHUNK);
        }
    }

    /// Everything may have changed (a saved waive file loaded over the
    /// review): the readers count this reviewer's rules again.
    pub fn everything(&mut self) {
        if !self.off {
            self.reset = true;
        }
    }

    /// Whether anything waits to be said.
    pub fn unsettled(&self) -> bool {
        self.reset || !self.written.is_empty() || !self.committed.is_empty()
    }

    /// Say what was changed since the last settle, with the STAMP of the
    /// files as they stand - the review has flushed its statuses first. A
    /// log that cannot be written is said once in `notices` and stops
    /// nothing: the readers find the files newer than the log says and
    /// count again.
    pub fn settle(&mut self) {
        self.say(true);
    }

    /// Say the flags committed since the last settle (the end of a flags
    /// write that is a request of its own: a visit, a job's exclusions).
    /// Statuses written meanwhile wait for their flush and the review's own
    /// `settle`: a record must not be read before its bits can be.
    pub fn settle_committed(&mut self) {
        self.say(false);
    }

    fn say(&mut self, all: bool) {
        if !(all && self.unsettled() || !self.committed.is_empty()) {
            return;
        }
        let reset = all && std::mem::take(&mut self.reset);
        let mut pending = std::mem::take(&mut self.committed);
        if all {
            for (rule, ranges) in std::mem::take(&mut self.written) {
                pending.entry(rule).or_default().extend(ranges);
            }
        }
        let stamps = self.stamps();
        let mut bytes = Vec::new();
        if reset {
            bytes.extend(Record { kind: RESET, rule: 0, a: 0, b: 0 }.bytes());
        }
        for (rule, mut ranges) in pending {
            ranges.sort_unstable();
            let mut merged: Vec<(u64, u64)> = Vec::with_capacity(ranges.len());
            for (a, b) in ranges {
                match merged.last_mut() {
                    Some(m) if a <= m.1.saturating_add(1) => m.1 = m.1.max(b),
                    _ => merged.push((a, b)),
                }
            }
            for (a, b) in merged {
                bytes.extend(Record { kind: TOUCH, rule, a, b }.bytes());
            }
        }
        bytes.extend(Record { kind: STAMP, rule: 0, a: stamps[0], b: stamps[1] }.bytes());
        let last = self.last.unwrap_or([0; 5]);
        if stamps[2..4] != last[2..4] {
            bytes.extend(Record { kind: CSTAMP, rule: 0, a: stamps[2], b: stamps[3] }.bytes());
        }
        if stamps[4] != last[4] {
            bytes.extend(Record { kind: QSTAMP, rule: 0, a: stamps[4], b: 0 }.bytes());
        }
        match self.append(&bytes) {
            Ok(()) => self.last = Some(stamps),
            Err(e) => {
                if self.notices.is_empty() {
                    self.notices.push(format!(
                        "[drc] the review's change log could not be written ({}): {e}; the team's counts are counted again where this review changes",
                        self.path.display()
                    ));
                }
                self.file = None;
            }
        }
    }

    fn append(&mut self, records: &[u8]) -> std::io::Result<()> {
        if self.file.is_none() {
            let mut file = OpenOptions::new().append(true).create(true).open(&self.path)?;
            let len = file.metadata()?.len();
            if len < HEADER {
                // a new log (or one cut inside its header): its id is new
                file.set_len(0)?;
                let id = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos() as u64)
                    .unwrap_or(0)
                    ^ (u64::from(std::process::id()) << 40);
                let mut head = Vec::with_capacity(HEADER as usize);
                head.extend(MAGIC);
                head.extend(VERSION.to_le_bytes());
                head.extend(self.identity);
                head.extend(id.max(1).to_le_bytes());
                file.write_all(&head)?;
            } else if (len - HEADER) % RECORD != 0 {
                file.set_len(HEADER + (len - HEADER) / RECORD * RECORD)?;
            }
            self.file = Some(file);
        }
        let mut file = self.file.as_ref().expect("opened");
        file.write_all(records)?;
        if self.sync {
            match flush(file) {
                Err(e)
                    if !matches!(
                        e.kind(),
                        std::io::ErrorKind::Unsupported | std::io::ErrorKind::InvalidInput
                    ) =>
                {
                    return Err(e)
                }
                _ => (),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SERIAL: AtomicU64 = AtomicU64::new(0);
    const ID: [u8; 28] = [7; 28];

    struct Folder(PathBuf);
    impl Folder {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "floe-changes-{}-{}",
                std::process::id(),
                SERIAL.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&dir).unwrap();
            fs::write(dir.join(".x.db.waive.kim"), b"waive").unwrap();
            Self(dir)
        }
        fn waive(&self) -> PathBuf {
            self.0.join(".x.db.waive.kim")
        }
        fn log(&self) -> PathBuf {
            self.0.join(".x.db.changes.kim")
        }
        fn open(&self) -> ChangeLog {
            ChangeLog::open(&self.waive(), ID, false)
        }
        fn records(&self) -> Vec<(u32, u32, u64, u64)> {
            read(&self.log(), &ID, None)
                .unwrap()
                .records
                .iter()
                .map(|r| (r.kind, r.rule, r.a, r.b))
                .collect()
        }
    }
    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn what_was_touched_is_said_when_it_is_settled() {
        let f = Folder::new();
        assert_eq!(path_beside(&f.waive()), f.log());
        assert_eq!(
            path_beside(Path::new("/tmp/.r.db.waive.kim-0123456789ab")),
            Path::new("/tmp/.r.db.changes.kim-0123456789ab")
        );
        // never the file it was named from (a pack shown read-only)
        assert_eq!(path_beside(Path::new("/r/.r.db.tray")), Path::new("/r/.r.db.tray.changes"));
        let mut none = ChangeLog::none();
        none.touched(0, 0, 0);
        none.committed(0, 0, 0);
        none.everything();
        none.attach_flags(Path::new("/r/x"));
        none.settle();
        assert!(!none.unsettled() && none.notices.is_empty());
        let mut log = f.open();
        assert!(!log.unsettled() && !f.log().exists(), "nothing is made by the open");
        log.settle();
        assert!(!f.log().exists(), "nothing touched: nothing said");
        // a job walking a rule upwards is one range; errors far apart are
        // ranges of their own; another rule is another record
        log.touched(3, 0, 15);
        log.touched(3, 16, 31);
        log.touched(3, 20, 40);
        log.touched_errors(3, &[4096 * 100, 4096 * 100 + 5, 4096 * 101]);
        log.touched_errors(1, &[9]);
        log.touched(3, 7, 2); // not a range
        assert!(log.unsettled());
        log.settle();
        assert!(!log.unsettled());
        let stamp = stamp_at(&f.waive());
        assert!(stamp > 1);
        assert_eq!(
            f.records(),
            [(TOUCH, 1, 0, 0), (TOUCH, 3, 0, 40), (TOUCH, 3, 100, 101), (STAMP, 0, stamp, 0)]
        );
        let head = fs::read(f.log()).unwrap();
        assert_eq!(head.len() as u64, HEADER + 4 * RECORD);
        assert_eq!((&head[..8], &head[12..40]), (&MAGIC[..], &ID[..]));
        // ranges out of order are said in order, joined where they meet
        log.touched(2, 50, 60);
        log.touched(2, 10, 20);
        log.touched(2, 21, 30);
        log.touched(2, 55, 70);
        log.settle();
        assert_eq!(
            f.records()[4..],
            [(TOUCH, 2, 10, 30), (TOUCH, 2, 50, 70), (STAMP, 0, stamp, 0)]
        );
        // everything changed (a saved file loaded over the review)
        log.everything();
        log.touched(0, 1, 1);
        log.settle();
        assert_eq!(f.records()[7..], [(RESET, 0, 0, 0), (TOUCH, 0, 1, 1), (STAMP, 0, stamp, 0)]);
        // flags committed are said by their own settle; statuses written
        // meanwhile - and a RESET - wait for the review's, after its flush
        log.touched(5, 2, 2);
        log.everything();
        log.settle_committed();
        assert_eq!(f.records().len(), 10, "nothing committed: nothing said");
        log.committed(5, 9, 9);
        log.committed(4, 0, 1);
        log.settle_committed();
        assert_eq!(f.records()[10..], [(TOUCH, 4, 0, 1), (TOUCH, 5, 9, 9), (STAMP, 0, stamp, 0)]);
        assert!(log.unsettled());
        log.committed(5, 3, 3);
        log.settle();
        assert_eq!(
            f.records()[13..],
            [(RESET, 0, 0, 0), (TOUCH, 5, 2, 3), (STAMP, 0, stamp, 0)]
        );
        assert!(!log.unsettled());
        let said = f.records().len();
        assert_eq!(said, 16);
        // a reader that knows where it was reads what came after
        let all = read(&f.log(), &ID, None).unwrap();
        let tail = read(&f.log(), &ID, Some((all.log_id, HEADER + 7 * RECORD))).unwrap();
        assert_eq!((tail.len, tail.records.len()), (all.len, said - 7));
        assert!(read(&f.log(), &ID, Some((all.log_id, all.len))).unwrap().records.is_empty());
        // ... and the whole log when its place is not one of this log's
        for known in [(all.log_id + 1, all.len), (all.log_id, all.len + RECORD), (all.log_id, HEADER + 5), (all.log_id, 3)] {
            assert_eq!(read(&f.log(), &ID, Some(known)).unwrap().records.len(), said, "{known:?}");
        }
        // another result's log, or no log, is none
        assert!(read(&f.log(), &[8; 28], None).is_none());
        assert!(read(&f.0.join("none"), &ID, None).is_none());
    }

    /// A requester's request file is one of the reviewer's files (0.12.472):
    /// its time is said in a record of its own, when it is another - and the
    /// log is the one the request file's name gives too.
    #[test]
    fn the_request_files_time_is_said_when_it_is_another() {
        let f = Folder::new();
        let request = f.0.join(".x.db.request.kim");
        assert_eq!(path_beside(&request), f.log());
        let mut log = f.open();
        log.touched(0, 0, 0);
        log.settle();
        assert_eq!(f.records().len(), 2, "no request file: no word of one");
        fs::write(&request, b"request").unwrap();
        log.touched(0, 1, 1);
        log.settle();
        let (waive, said) = (stamp_at(&f.waive()), stamp_at(&request));
        assert_eq!(f.records()[2..], [(TOUCH, 0, 1, 1), (STAMP, 0, waive, 0), (QSTAMP, 0, said, 0)]);
        log.touched(0, 2, 2);
        log.settle();
        assert_eq!(f.records()[5..], [(TOUCH, 0, 2, 2), (STAMP, 0, waive, 0)]);
        let all = read(&f.log(), &ID, None).unwrap().records;
        assert_eq!(stamps_after([0; 5], &all), [waive, 0, 0, 0, said]);
        drop(log);
        // changed without a word: a RESET at the next open, and its time
        let flags = f.0.join(".x.db.review.kim.sqlite3");
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
        File::options().write(true).open(&request).unwrap().set_modified(later).unwrap();
        let mut log = f.open();
        log.attach_flags(&flags);
        let now = f.records();
        assert_eq!(now.len(), 10);
        assert_eq!((now[7], now[9]), ((RESET, 0, 0, 0), (QSTAMP, 0, stamp_at(&request), 0)));
        drop(log);
        // ... and as it was said: no word
        let mut log = f.open();
        log.attach_flags(&flags);
        assert_eq!(f.records().len(), 10);
    }

    #[test]
    fn the_confirm_files_time_is_said_when_it_is_another() {
        let f = Folder::new();
        let confirm = f.0.join(".x.db.confirm.kim");
        let mut log = f.open();
        log.touched(0, 0, 0);
        log.settle();
        assert_eq!(f.records().len(), 2, "no confirm file: no word of one");
        // the file comes (the reviewer's first confirmation): its time after
        // the STAMP, once
        fs::write(&confirm, b"confirm").unwrap();
        log.touched(0, 1, 1);
        log.settle();
        let (waive, said) = (stamp_at(&f.waive()), stamp_at(&confirm));
        assert_eq!(f.records()[2..], [(TOUCH, 0, 1, 1), (STAMP, 0, waive, 0), (CSTAMP, 0, said, 0)]);
        log.touched(0, 2, 2);
        log.settle();
        assert_eq!(f.records()[5..], [(TOUCH, 0, 2, 2), (STAMP, 0, waive, 0)]);
        let all = read(&f.log(), &ID, None).unwrap().records;
        assert_eq!(stamps_after([0; 5], &all), [waive, 0, said, 0, 0]);
        assert_eq!(stamps_after([7, 8, 9, 6, 5], &all[..1]), [7, 8, 9, 6, 5]);
        assert_eq!(stamps_after([7, 8, 9, 6, 5], &all[..2]), [waive, 0, 9, 6, 5]);
        drop(log);
        // the next session finds the three as they were said: no word
        let flags = f.0.join(".x.db.review.kim.sqlite3");
        let mut log = f.open();
        log.attach_flags(&flags);
        assert_eq!(f.records().len(), 7);
        drop(log);
        // the confirm file changed without a word: a RESET, and its time
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
        File::options().write(true).open(&confirm).unwrap().set_modified(later).unwrap();
        let mut log = f.open();
        log.attach_flags(&flags);
        let now = f.records();
        assert_eq!(now.len(), 10);
        assert_eq!((now[7], now[9]), ((RESET, 0, 0, 0), (CSTAMP, 0, stamp_at(&confirm), 0)));
        drop(log);
        // ... and gone: said as no file
        fs::remove_file(&confirm).unwrap();
        let mut log = f.open();
        log.attach_flags(&flags);
        assert_eq!(f.records()[10..], [(RESET, 0, 0, 0), (STAMP, 0, waive, 0), (CSTAMP, 0, 0, 0)]);
        // the rejects log is the fourth file (0.12.468): its time is the
        // same record's second number, said when either of the two is
        // another - beside the confirm file's, and alone
        let rejects = f.0.join(".x.db.rejects.kim");
        assert_eq!(crate::drc::confirm::rejects_beside(&f.waive()), rejects);
        fs::write(&rejects, b"# rejections\n").unwrap();
        log.touched(0, 3, 3);
        log.settle();
        let said = stamp_at(&rejects);
        assert_eq!(f.records()[13..], [(TOUCH, 0, 3, 3), (STAMP, 0, waive, 0), (CSTAMP, 0, 0, said)]);
        log.touched(0, 3, 3);
        log.settle();
        assert_eq!(f.records()[16..], [(TOUCH, 0, 3, 3), (STAMP, 0, waive, 0)], "no word of what stands");
        fs::write(&confirm, b"confirm").unwrap();
        log.touched(0, 4, 4);
        log.settle();
        assert_eq!(f.records()[20], (CSTAMP, 0, stamp_at(&confirm), said));
        let all = read(&f.log(), &ID, None).unwrap().records;
        assert_eq!(stamps_after([0; 5], &all), [waive, 0, stamp_at(&confirm), said, 0]);
        drop(log);
        // changed without a word (a session that ended before it could
        // say): the next one's RESET, with its time
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(9);
        File::options().write(true).open(&rejects).unwrap().set_modified(later).unwrap();
        let mut log = f.open();
        log.attach_flags(&flags);
        let now = f.records();
        assert_eq!((now.len(), now[21], now[23]), (24, (RESET, 0, 0, 0), (CSTAMP, 0, stamp_at(&confirm), stamp_at(&rejects))));
        // moved to the temp folder (the results folder refused a line):
        // the stamp looks where it is
        let other = f.0.join(".x.db.rejects.kim-0123456789ab");
        fs::write(&other, b"# rejections, elsewhere\n").unwrap();
        log.rejects_at(&other);
        log.touched(0, 5, 5);
        log.settle();
        assert_eq!(f.records()[26], (CSTAMP, 0, stamp_at(&confirm), stamp_at(&other)));
    }

    #[test]
    fn a_session_that_keeps_no_log_removes_the_one_it_has() {
        for (value, on) in [
            (None, true),
            (Some(""), true),
            (Some("on"), true),
            (Some("1"), true),
            (Some("off"), false),
            (Some(" OFF "), false),
            (Some("0"), false),
            (Some("no"), false),
            (Some("false"), false),
        ] {
            assert_eq!(keep_of(value), on, "{value:?}");
        }
        let f = Folder::new();
        let mut log = f.open();
        log.touched(0, 0, 0);
        log.settle();
        drop(log);
        assert!(f.log().exists());
        let mut log = ChangeLog::silent(&f.waive());
        assert!(!f.log().exists() && f.waive().exists());
        log.touched(0, 0, 0);
        log.committed(0, 1, 1);
        log.everything();
        log.attach_flags(&f.0.join("flags"));
        log.settle();
        log.settle_committed();
        assert!(!f.log().exists() && !log.unsettled() && log.notices.is_empty());
    }

    #[test]
    fn files_changed_without_a_word_are_said_at_the_next_open() {
        let f = Folder::new();
        let flags = f.0.join(".x.db.review.kim.sqlite3");
        let mut log = f.open();
        log.attach_flags(&flags);
        assert!(!f.log().exists(), "a reviewer without a log has nothing to compare");
        log.touched(0, 0, 0);
        log.settle();
        let said = f.records();
        assert_eq!(said.len(), 2);
        drop(log);
        // the next session finds the files as they were left: nothing said
        let mut log = f.open();
        log.attach_flags(&flags);
        assert_eq!(f.records(), said);
        drop(log);
        // the flags file appears (an older build's session visited an error)
        fs::write(&flags, b"flags").unwrap();
        let mut log = f.open();
        log.attach_flags(&flags);
        let now = f.records();
        assert_eq!(now.len(), 4);
        assert_eq!(now[2], (RESET, 0, 0, 0));
        assert_eq!((now[3].0, now[3].2, now[3].3), (STAMP, stamp_at(&f.waive()), stamp_at(&flags)));
        drop(log);
        // ... said once: the stamp it left is how the files stand
        let mut log = f.open();
        log.attach_flags(&flags);
        assert_eq!(f.records().len(), 4);
        // the waive sidecar changed (another time)
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
        File::options().write(true).open(f.waive()).unwrap().set_modified(later).unwrap();
        drop(log);
        let mut log = f.open();
        log.attach_flags(&flags);
        assert_eq!(f.records().len(), 6);
        assert_eq!(f.records()[4], (RESET, 0, 0, 0));
        drop(log);
        // a review that writes nothing of its own says nothing
        let mut log = f.open().off();
        File::options().write(true).open(f.waive()).unwrap().set_modified(later + std::time::Duration::from_secs(5)).unwrap();
        log.attach_flags(&flags);
        log.touched(0, 0, 0);
        log.everything();
        log.settle();
        assert!(!log.unsettled());
        assert_eq!(f.records().len(), 6);
    }

    #[test]
    fn a_torn_foreign_or_overgrown_log_starts_again() {
        let f = Folder::new();
        let mut log = f.open();
        log.touched(0, 0, 0);
        log.settle();
        drop(log);
        let whole = fs::read(f.log()).unwrap();
        let id = read(&f.log(), &ID, None).unwrap().log_id;
        // a torn last record is cut at the open; the records before it stand
        let mut torn = whole.clone();
        torn.extend_from_slice(&[1, 2, 3, 4, 5]);
        fs::write(f.log(), &torn).unwrap();
        assert_eq!(read(&f.log(), &ID, None).unwrap().records.len(), 2, "a reader passes it over");
        let mut log = f.open();
        assert_eq!(fs::read(f.log()).unwrap(), whole);
        log.touched(0, 5, 5);
        log.settle();
        assert_eq!(f.records().len(), 4);
        assert_eq!(read(&f.log(), &ID, None).unwrap().log_id, id, "the same log goes on");
        drop(log);
        // another result's log is removed, and the next record starts a new one
        let mut other = whole.clone();
        other[20] ^= 1;
        fs::write(f.log(), &other).unwrap();
        let mut log = f.open();
        assert!(!f.log().exists());
        log.touched(0, 1, 1);
        log.settle();
        let new = read(&f.log(), &ID, None).unwrap();
        assert_eq!(new.records.len(), 2);
        assert_ne!(new.log_id, id, "a new log has an id of its own");
        drop(log);
        // not a log at all
        fs::write(f.log(), b"junk").unwrap();
        assert!(read(&f.log(), &ID, None).is_none());
        drop(f.open());
        assert!(!f.log().exists());
        // grown past its size: started again at the open
        let mut big = whole.clone();
        big.resize((ROTATE + HEADER + RECORD) as usize / RECORD as usize * RECORD as usize, 0);
        fs::write(f.log(), &big).unwrap();
        drop(f.open());
        assert!(!f.log().exists());
    }
}
