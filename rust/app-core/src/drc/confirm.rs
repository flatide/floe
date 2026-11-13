//! Confirmations of waives: `.<db>.confirm.<reviewer>` - which errors'
//! waives a confirmer has confirmed (user 2026-11-13: "사용자중 팀의 모든
//! waive를 뷰어에서 보고 confirm하는 역할이 필요함. 현재 confirm 상태는
//! 없는것 같은데 confirm 상태를 추가해야 함.").
//!
//! A confirmation is the confirmer's own mark on an error - "its waive is
//! confirmed" - kept in a file of the confirmer's own, beside their waive
//! sidecar and written by their session alone, as every review file is: a
//! byte an error (1 = confirmed) and a confirmed counter a rule, the waive
//! sidecar's layout under a magic of its own.
//!
//! ```text
//! header, 40 bytes: "FLOECONF" | u32 1 | 16 B content id prefix | u64 errors
//!                   | u32 rules
//! errors bytes     : 1 = this reviewer confirmed the error's waive
//! rules x u32      : the confirmed errors of each rule
//! ```
//!
//! The file says who confirmed, never whether an error is waived: that is
//! the waive sidecars' - anyone's on the team. A confirmation counts while
//! somebody has the error waived; turned back by its reviewers it counts
//! nothing, and stands again should the error be waived again. Its readers
//! (the team's counts, the confirmer's list) ask the waives besides.
//!
//! The file is made at the first confirmation: a reviewer who confirms
//! nothing has none. Who may confirm is the launcher's word (`floe2 view
//! --confirmer`), kept by `desktop::Review`.
//!
//! A confirmed waive is locked (0.12.467; user 2026-11-13: "confirm된
//! waive는 되돌릴 수 없게 잠구고 confirmer만 되돌릴 수 있게 해줘"): the
//! session of a reviewer who is no confirmer does not turn it back, nor
//! change its reason. `Locks` is how that session knows - every confirmer's
//! file of the result, read as the statistics find them - and
//! `desktop::Review`'s status writers are where it is kept.
//!
//! A confirmer's other verdict is the rejection (0.12.468; user 2026-11-14:
//! "confirmer가 다른 리뷰어의 waive를 반려할 수 있게 해줘"): the errors
//! whose waive the confirmer rejected are a log of the confirmer's own,
//! `.<db>.rejects.<reviewer>` (`drc::reasons`, `Kind::Reject`: which errors,
//! when, why - an action a line). No waive of a rejected error counts, the
//! sessions of the reviewers who waived it turn their waive back, and no
//! session waives it again until its confirmer takes the rejection back.
//! `Locks` reads those logs too.
use super::local::aside_path;
use super::reasons::{Kind as Reason, Reasons};
use crate::{Error, ErrorKind, Result};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const MAGIC: &[u8; 8] = b"FLOECONF";
pub const VERSION: u32 = 1;
pub const HEADER: u64 = 40;
/// The byte of an error whose waive this reviewer confirmed.
pub const CONFIRMED: u8 = 1;

/// The confirm file's header of the result whose waive sidecars start with
/// `waive`: the same identity (content id prefix, errors, rules) under this
/// file's magic and version - a confirm file is never taken for a waive
/// file, nor the other way round.
pub fn header_of(waive: &[u8; 40]) -> [u8; 40] {
    let mut header = *waive;
    header[..8].copy_from_slice(MAGIC);
    header[8..12].copy_from_slice(&VERSION.to_le_bytes());
    header
}

/// The file's name from the waive sidecar's (same folder, reviewer and - in
/// the temp folder - path hash); never the file it was named from.
pub fn path_beside(waive: &Path) -> PathBuf {
    let name = waive.file_name().unwrap_or_default().to_string_lossy();
    if name.contains(".waive.") {
        waive.with_file_name(name.replacen(".waive.", ".confirm.", 1))
    } else {
        waive.with_file_name(format!("{name}.confirm"))
    }
}

/// A confirmer's rejects log's name (drc::reasons: the errors whose waive
/// the confirmer rejected, an action a line), from the waive sidecar's as
/// `path_beside`.
pub fn rejects_beside(waive: &Path) -> PathBuf {
    let name = waive.file_name().unwrap_or_default().to_string_lossy();
    if name.contains(".waive.") {
        waive.with_file_name(name.replacen(".waive.", ".rejects.", 1))
    } else {
        waive.with_file_name(format!("{name}.rejects"))
    }
}

fn foreign() -> Error {
    Error::new(
        ErrorKind::Cache,
        "confirmations of another DRC result (their header or size is not this pack's)",
    )
}

/// A confirm file of the result `header` (`header_of`) with `total` errors
/// in `rules` rules, open to read: the file and its counter a rule. Another
/// result's is ErrorKind::Cache.
pub fn read_of(file: File, header: &[u8; 40], total: u64, rules: usize) -> Result<(File, Vec<u32>)> {
    let mut head = [0u8; 40];
    let matches = file.read_exact_at(&mut head, 0).is_ok()
        && head == *header
        && file.metadata()?.len() == HEADER + total + 4 * rules as u64;
    if !matches {
        return Err(foreign());
    }
    let mut raw = vec![0u8; rules * 4];
    file.read_exact_at(&mut raw, HEADER + total)?;
    let counts = raw
        .chunks_exact(4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    Ok((file, counts))
}

/// `read_of` the file at `path`.
pub fn read(path: &Path, header: &[u8; 40], total: u64, rules: usize) -> Result<(File, Vec<u32>)> {
    read_of(File::open(path)?, header, total, rules)
}

/// A confirmer's own confirmations, as their session keeps them.
pub struct Confirms {
    pub path: PathBuf,
    header: [u8; 40],
    total: u64,
    /// each rule's first error and its errors
    rules: Vec<(u64, u64)>,
    /// the file, once there is one (opened to write at the first write)
    file: Option<File>,
    writable: bool,
    counts: Vec<u32>,
    unsynced: bool,
    sync: bool,
    /// nothing can be kept (the review writes no file of its own)
    off: bool,
    pub notices: Vec<String>,
}

impl Confirms {
    /// The confirmations at `path` of the result `header` (`header_of`),
    /// whose rules start at `rules.0` and have `rules.1` errors. Nothing is
    /// made by the open; a file of another result is moved aside
    /// (`.stale-<time>`), as the reviewer's other files are.
    pub fn open(path: PathBuf, header: [u8; 40], rules: Vec<(u64, u64)>, sync: bool) -> Self {
        let total = rules.iter().map(|r| r.1).sum();
        let mut me = Self {
            path,
            header,
            total,
            counts: vec![0; rules.len()],
            rules,
            file: None,
            writable: false,
            unsynced: false,
            sync,
            off: false,
            notices: Vec::new(),
        };
        match File::open(&me.path) {
            Ok(file) => match read_of(file, &me.header, me.total, me.rules.len()) {
                Ok((file, counts)) => {
                    me.file = Some(file);
                    me.counts = counts;
                }
                Err(e) if e.kind == ErrorKind::Cache => {
                    let aside = aside_path(&me.path);
                    match fs::rename(&me.path, &aside) {
                        Ok(()) => me.notices.push(format!(
                            "[drc] confirmations do not match this pack (DRC re-run?); moved aside: {}",
                            aside.display()
                        )),
                        Err(e) => {
                            me.notices.push(format!(
                                "[drc] confirmations of another DRC result cannot be moved aside ({}): {e}; nothing can be confirmed here",
                                me.path.display()
                            ));
                            me.off = true;
                        }
                    }
                }
                Err(e) => {
                    me.notices.push(format!(
                        "[drc] confirmations cannot be read ({}): {}; nothing can be confirmed here",
                        me.path.display(),
                        e.message
                    ));
                    me.off = true;
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => {
                me.notices.push(format!(
                    "[drc] confirmations cannot be read ({}): {e}; nothing can be confirmed here",
                    me.path.display()
                ));
                me.off = true;
            }
        }
        me
    }

    /// The confirmations of a review that writes no file of its own (the
    /// pack's embedded statuses shown read-only): none, and none can be made.
    pub fn none(rules: usize) -> Self {
        Self {
            path: PathBuf::new(),
            header: [0; 40],
            total: 0,
            rules: vec![(0, 0); rules],
            file: None,
            writable: false,
            counts: vec![0; rules],
            unsynced: false,
            sync: false,
            off: true,
            notices: Vec::new(),
        }
    }

    /// Whether there is a file (this reviewer has confirmed something, once).
    pub fn present(&self) -> bool {
        self.file.is_some()
    }

    /// The confirmed errors of each rule.
    pub fn counts(&self) -> &[u32] {
        &self.counts
    }

    /// Turn the flushes on or off (as the review's waive writes are).
    pub fn set_sync(&mut self, on: bool) {
        self.sync = on;
    }

    fn rule(&self, check: usize) -> Result<(u64, u64)> {
        self.rules
            .get(check)
            .copied()
            .ok_or_else(|| Error::input("DRC check index out of range"))
    }

    /// Rule `check`'s bytes `start..start+count` (clamped to the rule; zeros
    /// while there is no file).
    pub fn statuses(&self, check: usize, start: u64, count: u64) -> Result<Vec<u8>> {
        let (first, n) = self.rule(check)?;
        let start = start.min(n);
        let mut out = vec![0u8; count.min(n - start) as usize];
        if let Some(file) = &self.file {
            file.read_exact_at(&mut out, HEADER + first + start)?;
        }
        Ok(out)
    }

    /// Whether this reviewer confirmed rule `check`'s error `error`.
    pub fn get(&self, check: usize, error: u64) -> Result<bool> {
        let (_, n) = self.rule(check)?;
        if error >= n {
            return Err(Error::input("DRC error index out of range"));
        }
        Ok(self.statuses(check, error, 1)?[0] == CONFIRMED)
    }

    /// The file open to write, made when there is none yet: the header, and
    /// zeros (never written: a hole) for the errors and the counters.
    fn writer(&mut self) -> Result<&File> {
        if self.off {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "this review is read-only here: nothing can be confirmed",
            ));
        }
        if !self.writable {
            if self.file.is_none() {
                let mut name = self.path.as_os_str().to_owned();
                name.push(format!(".tmp-{}", std::process::id()));
                let tmp = PathBuf::from(name);
                let made = (|| -> std::io::Result<()> {
                    let mut out = File::create(&tmp)?;
                    out.write_all(&self.header)?;
                    out.set_len(HEADER + self.total + 4 * self.rules.len() as u64)?;
                    out.sync_all()?;
                    fs::rename(&tmp, &self.path)
                })();
                if let Err(e) = made {
                    let _ = fs::remove_file(&tmp);
                    return Err(Error::new(
                        ErrorKind::Io,
                        format!("{}: {e}", self.path.display()),
                    ));
                }
            }
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&self.path)
                .map_err(|e| Error::new(ErrorKind::Io, format!("{}: {e}", self.path.display())))?;
            self.file = Some(file);
            self.writable = true;
        }
        Ok(self.file.as_ref().expect("opened"))
    }

    fn count_by(&mut self, check: usize, delta: i64) -> Result<()> {
        if delta == 0 {
            return Ok(());
        }
        let count = (i64::from(self.counts[check]) + delta).max(0) as u32;
        let at = HEADER + self.total + 4 * check as u64;
        self.writer()?.write_all_at(&count.to_le_bytes(), at)?;
        self.counts[check] = count;
        Ok(())
    }

    /// Confirm (`on`) rule `check`'s `errors`, or take their confirmation
    /// back: written in place, the rule's counter in step; the errors it
    /// changed. Flushed by `flush`.
    pub fn set(&mut self, check: usize, errors: &[u64], on: bool) -> Result<Vec<u64>> {
        let (first, n) = self.rule(check)?;
        if errors.iter().any(|&e| e >= n) {
            return Err(Error::input("DRC error index out of range"));
        }
        let value = if on { CONFIRMED } else { 0 };
        let mut changed = Vec::new();
        for &error in errors {
            let old = match &self.file {
                Some(file) => {
                    let mut byte = [0u8; 1];
                    file.read_exact_at(&mut byte, HEADER + first + error)?;
                    byte[0]
                }
                None => 0,
            };
            if old == value {
                continue;
            }
            self.writer()?.write_all_at(&[value], HEADER + first + error)?;
            changed.push(error);
        }
        let delta = changed.len() as i64 * if on { 1 } else { -1 };
        self.count_by(check, delta)?;
        self.unsynced |= !changed.is_empty();
        Ok(changed)
    }

    /// Write rule `check`'s bytes `start..` from `before` (as read) to
    /// `after` in one span, the counter in step; the bytes that changed.
    pub fn write_span(&mut self, check: usize, start: u64, before: &[u8], after: &[u8]) -> Result<u64> {
        let (first, n) = self.rule(check)?;
        if before.len() != after.len() || start + after.len() as u64 > n {
            return Err(Error::input("confirmation span outside its rule"));
        }
        let changed = before.iter().zip(after).filter(|(b, a)| b != a).count() as u64;
        if changed == 0 {
            return Ok(0);
        }
        let confirmed = |s: &[u8]| s.iter().filter(|&&v| v == CONFIRMED).count() as i64;
        let delta = confirmed(after) - confirmed(before);
        self.writer()?.write_all_at(after, HEADER + first + start)?;
        self.count_by(check, delta)?;
        self.unsynced = true;
        Ok(changed)
    }

    /// Flush what was written since the last flush to the disk (the end of
    /// a request or a job; FLOE_WAIVE_SYNC, as the waive writes).
    pub fn flush(&mut self) -> Result<()> {
        if !std::mem::take(&mut self.unsynced) || !self.sync {
            return Ok(());
        }
        let Some(file) = &self.file else { return Ok(()) };
        match file.sync_data() {
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
}

/// How long what `Locks` read of the confirmers' files is used before the
/// files are found and opened anew.
pub const LOCK_REFRESH: Duration = Duration::from_secs(1);

/// Which waives of a result are locked: the confirmations of every
/// confirmer, as the session of a reviewer reads them before it turns a
/// waive back. An error is locked while a confirmer's file has it confirmed;
/// whether the reviewer has it waived is the caller's to ask.
///
/// The files are the ones the statistics count (drc::team::stats `discover`):
/// `.<db>.confirm.<reviewer>` beside the results, and the temp folder's
/// path-hashed names. They are found and opened anew when what is held is
/// older than `LOCK_REFRESH` - a job of minutes lists the folders once a
/// second, not once a chunk - and their bytes are read at each ask. An open
/// asks an NFS server for the file as it is, a read of a file held open may
/// not: so a confirmation made on another host locks within about that
/// second, and a confirmer's first one, which makes their file, as late as
/// the mount's directory cache shows the name. A file that is not this
/// result's, or reads no more, locks nothing.
pub struct Locks {
    /// where confirm files are and how they are named: the folder, the
    /// name's start and what follows the reviewer in it; and the name's
    /// start of a rejects log there
    places: Vec<(PathBuf, String, String, String)>,
    header: [u8; 40],
    total: u64,
    /// each rule's first error and its errors
    rules: Vec<(u64, u64)>,
    /// the result as the rejects logs name it (`floe_pack=`)
    tag: String,
    /// the confirmers' files as last found: whose, and the file
    files: Vec<(String, File)>,
    /// the confirmers' rejects logs as last found and read
    rejects: Vec<Rejects>,
    read: Option<Instant>,
    refresh: Duration,
}

/// One confirmer's rejects log as it was read.
struct Rejects {
    tag: String,
    path: PathBuf,
    /// the file's time and size then: read again when they are others
    stamp: (u64, u64),
    log: Reasons,
}

/// Who rejected an error's waive, and why as they said.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Rejected {
    pub reviewer: String,
    pub text: String,
    /// seconds since the epoch
    pub time: u64,
}

fn stamp_of(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let meta = fs::metadata(path).ok()?;
    Some((
        (meta.mtime() as u64).wrapping_mul(1_000_000_000).wrapping_add(meta.mtime_nsec() as u64),
        meta.len(),
    ))
}

impl Locks {
    /// The locks of the result whose confirm files start with `header`
    /// (`header_of`), for a review whose waive sidecar is `waive[0]` beside
    /// the results or `waive[1]` in the temp folder (`drc::waive_paths` of
    /// `reviewer`): every reviewer's confirm file is named as this one's.
    ///
    /// `tag`: the result as the rejects logs name it (the reasons' tag).
    pub fn beside(
        waive: &[PathBuf; 2],
        reviewer: &str,
        header: [u8; 40],
        rules: Vec<(u64, u64)>,
        tag: String,
    ) -> Self {
        let name = |p: PathBuf| p.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let (near, far) = (name(path_beside(&waive[0])), name(path_beside(&waive[1])));
        let logs = name(rejects_beside(&waive[0]));
        let mut places = Vec::new();
        if let (Some(start), Some(log), Some(folder)) =
            (near.strip_suffix(reviewer), logs.strip_suffix(reviewer), waive[0].parent())
        {
            places.push((folder.to_path_buf(), start.to_owned(), String::new(), log.to_owned()));
            // the temp folder's names end in a hash of the database's path
            if let (Some(end), Some(temp)) = (far.strip_prefix(near.as_str()), waive[1].parent()) {
                if !end.is_empty() && temp != folder {
                    places.push((temp.to_path_buf(), start.to_owned(), end.to_owned(), log.to_owned()));
                }
            }
        }
        Self {
            places,
            header,
            total: rules.iter().map(|r| r.1).sum(),
            rules,
            tag,
            files: Vec::new(),
            rejects: Vec::new(),
            read: None,
            refresh: LOCK_REFRESH,
        }
    }

    /// The locks of a review that writes nothing: none.
    pub fn none(rules: usize) -> Self {
        Self {
            places: Vec::new(),
            header: [0; 40],
            total: 0,
            rules: vec![(0, 0); rules],
            tag: String::new(),
            files: Vec::new(),
            rejects: Vec::new(),
            read: None,
            refresh: LOCK_REFRESH,
        }
    }

    /// How old what was read may be before it is read anew (`LOCK_REFRESH`;
    /// zero: at each ask - the tests').
    pub fn set_refresh(&mut self, every: Duration) {
        self.refresh = every;
        self.read = None;
    }

    fn rule(&self, check: usize) -> Result<(u64, u64)> {
        self.rules
            .get(check)
            .copied()
            .ok_or_else(|| Error::input("DRC check index out of range"))
    }

    /// Every reviewer's confirm file and rejects log, by reviewer: the
    /// results folder's before the temp folder's, leftovers (`.stale-N`,
    /// `.tmp-N`) left out.
    fn found(&self) -> [BTreeMap<String, PathBuf>; 2] {
        let mut found = [BTreeMap::new(), BTreeMap::new()];
        for (folder, start, end, log) in &self.places {
            let Ok(entries) = fs::read_dir(folder) else { continue };
            for entry in entries.flatten() {
                let name = entry.file_name();
                let Some(text) = name.to_str() else { continue };
                for (slot, start) in [(0, start), (1, log)] {
                    let tag = text
                        .strip_prefix(start.as_str())
                        .and_then(|n| n.strip_suffix(end.as_str()))
                        .and_then(super::local::reviewer_of);
                    if let Some(tag) = tag {
                        found[slot].entry(tag.to_owned()).or_insert_with(|| folder.join(&name));
                    }
                }
            }
        }
        found
    }

    /// The files found and opened anew, when what is held is too old.
    fn fresh(&mut self) {
        if self.read.is_some_and(|at| at.elapsed() < self.refresh) {
            return;
        }
        let (header, total, rules) = (self.header, self.total, self.rules.len());
        let [files, logs] = self.found();
        self.files = files
            .into_iter()
            .filter_map(|(tag, path)| read(&path, &header, total, rules).ok().map(|(file, _)| (tag, file)))
            .collect();
        // a log that stands as it was read is kept as read
        let mut held = std::mem::take(&mut self.rejects);
        let sizes: Vec<u64> = self.rules.iter().map(|r| r.1).collect();
        for (tag, path) in logs {
            let Some(stamp) = stamp_of(&path) else { continue };
            match held.iter().position(|r| r.path == path && r.stamp == stamp) {
                Some(i) => self.rejects.push(held.swap_remove(i)),
                None => {
                    // (another result's, or one that does not read: no rejection)
                    if let Ok(Some(log)) = Reasons::load(&path, &self.tag, &sizes) {
                        self.rejects.push(Rejects { tag, path, stamp, log });
                    }
                }
            }
        }
        self.rejects.sort_by(|a, b| a.tag.cmp(&b.tag));
        self.read = Some(Instant::now());
    }

    /// The logs held are read again where their file is another now (a
    /// confirmer's next rejection is a line more).
    fn reread(&mut self) {
        let sizes: Vec<u64> = self.rules.iter().map(|r| r.1).collect();
        let tag = self.tag.clone();
        self.rejects.retain_mut(|r| match stamp_of(&r.path) {
            Some(stamp) if stamp == r.stamp => true,
            Some(stamp) => match Reasons::load(&r.path, &tag, &sizes) {
                Ok(Some(log)) => {
                    (r.stamp, r.log) = (stamp, log);
                    true
                }
                _ => false,
            },
            None => false,
        });
    }

    /// The rejects logs as they stand: read again where they changed, and
    /// - `look` - looked for anew when what is held is too old.
    fn logs(&mut self, look: bool) -> &[Rejects] {
        let listed = self.read;
        if look {
            self.fresh();
        }
        if listed == self.read {
            self.reread();
        }
        &self.rejects
    }

    /// Rule `check`'s errors `start..start+len` (clamped): true where a
    /// confirmer has the error's waive rejected. None when none is.
    ///
    /// `waiving`: asked before an error is waived - which lists no folder:
    /// the logs are the ones found by the last look (`rejections`, which
    /// the reviewer's host asks as often as it follows the confirmers, or a
    /// lock's), read again where they changed. A waive click costs no
    /// listing, and does not keep a list that the lock's next ask would
    /// take for fresh. A confirmer's first rejection - a new file - is
    /// therefore known from the next look on; a waive that slips past it
    /// is turned back by that look.
    pub fn rejected(&mut self, check: usize, start: u64, len: u64, waiving: bool) -> Result<Option<Vec<bool>>> {
        let (_, n) = self.rule(check)?;
        let start = start.min(n);
        let len = len.min(n - start);
        let mut out: Option<Vec<bool>> = None;
        for r in self.logs(!waiving) {
            for (a, b) in r.log.held(Reason::Reject, check, &[(start, start + len)]) {
                let out = out.get_or_insert_with(|| vec![false; len as usize]);
                out[(a - start) as usize..(b - start) as usize].fill(true);
            }
        }
        Ok(out)
    }

    /// Who has rejected the waive of any of rule `check`'s errors `runs`,
    /// and why as they said of the first of them (the logs found by the
    /// last look: no folder is listed for it).
    pub fn rejected_by(&mut self, check: usize, runs: &[(u64, u64)]) -> Result<Vec<Rejected>> {
        self.rule(check)?;
        if runs.is_empty() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for r in self.logs(false) {
            let held = r.log.held(Reason::Reject, check, runs);
            if let Some(entry) = held.first().and_then(|first| r.log.of(Reason::Reject, check, first.0)) {
                out.push(Rejected { reviewer: r.tag.clone(), text: entry.text.clone(), time: entry.time });
            }
        }
        Ok(out)
    }

    /// Every rejection that stands: rule by rule, the rejected errors as
    /// runs - looked for anew now, whatever is held - and what the logs
    /// were when they said so (their names, times and sizes: the same
    /// answer while it is the same).
    pub fn rejections(&mut self) -> (Vec<(usize, Vec<(u64, u64)>)>, Vec<(String, u64, u64)>) {
        // (this look is what finds a confirmer's first rejection - a new
        // file - for the writers, who list nothing when they waive)
        self.read = None;
        self.fresh();
        let mut rules: BTreeMap<usize, Vec<(u64, u64)>> = BTreeMap::new();
        for r in &self.rejects {
            for check in r.log.checks(Reason::Reject) {
                let runs = rules.entry(check).or_default();
                runs.extend(r.log.runs(Reason::Reject, check).into_iter().map(|(a, b, _)| (a, b)));
            }
        }
        for runs in rules.values_mut() {
            // (two confirmers' runs: sorted, and joined where they meet)
            runs.sort_unstable();
            let mut joined: Vec<(u64, u64)> = Vec::with_capacity(runs.len());
            for &(a, b) in runs.iter() {
                match joined.last_mut() {
                    Some(last) if a <= last.1 => last.1 = last.1.max(b),
                    _ => joined.push((a, b)),
                }
            }
            *runs = joined;
        }
        let seen = self.rejects.iter().map(|r| (r.tag.clone(), r.stamp.0, r.stamp.1)).collect();
        (rules.into_iter().collect(), seen)
    }

    /// Rule `check`'s errors `start..start+len` (clamped to the rule): true
    /// where a confirmer has the error confirmed. None when none of them is.
    pub fn mask(&mut self, check: usize, start: u64, len: u64) -> Result<Option<Vec<bool>>> {
        let (first, n) = self.rule(check)?;
        let start = start.min(n);
        let len = len.min(n - start) as usize;
        self.fresh();
        let mut out: Option<Vec<bool>> = None;
        let mut bytes = vec![0u8; len];
        for (_, file) in &self.files {
            if file.read_exact_at(&mut bytes, HEADER + first + start).is_err() {
                continue;
            }
            if bytes.contains(&CONFIRMED) {
                let out = out.get_or_insert_with(|| vec![false; len]);
                for (o, b) in out.iter_mut().zip(&bytes) {
                    *o |= *b == CONFIRMED;
                }
            }
        }
        Ok(out)
    }

    /// Who has rule `check`'s error `error` confirmed.
    pub fn by(&mut self, check: usize, error: u64) -> Result<Vec<String>> {
        let (first, n) = self.rule(check)?;
        if error >= n {
            return Err(Error::input("DRC error index out of range"));
        }
        self.fresh();
        let mut byte = [0u8; 1];
        Ok(self
            .files
            .iter()
            .filter(|(_, file)| file.read_exact_at(&mut byte, HEADER + first + error).is_ok() && byte[0] == CONFIRMED)
            .map(|(tag, _)| tag.clone())
            .collect())
    }

    /// Whether a confirmer has rule `check`'s error `error` confirmed.
    pub fn confirmed(&mut self, check: usize, error: u64) -> Result<bool> {
        Ok(!self.by(check, error)?.is_empty())
    }

    /// Who has any of rule `check`'s errors `runs` (start, stop) confirmed:
    /// whom a reviewer asks about the waives a writer left locked.
    pub fn who(&mut self, check: usize, runs: &[(u64, u64)]) -> Result<Vec<String>> {
        const STEP: u64 = 1 << 20;
        let (first, n) = self.rule(check)?;
        if runs.is_empty() {
            // (nothing asked: no folder is listed for it)
            return Ok(Vec::new());
        }
        self.fresh();
        let mut out = Vec::new();
        let mut bytes = Vec::new();
        for (tag, file) in &self.files {
            let found = runs.iter().any(|&(start, stop)| {
                let (mut at, stop) = (start.min(n), stop.min(n));
                while at < stop {
                    bytes.resize((stop - at).min(STEP) as usize, 0);
                    if file.read_exact_at(&mut bytes, HEADER + first + at).is_err() {
                        return false;
                    }
                    if bytes.contains(&CONFIRMED) {
                        return true;
                    }
                    at += bytes.len() as u64;
                }
                false
            });
            if found {
                out.push(tag.clone());
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SERIAL: AtomicU64 = AtomicU64::new(0);

    fn folder() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "floe-confirm-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        dir
    }

    fn waive_header(total: u64, rules: u32) -> [u8; 40] {
        let mut h = [0xabu8; 40];
        h[..8].copy_from_slice(b"FLOEWAIV");
        h[8..12].copy_from_slice(&2u32.to_le_bytes());
        h[28..36].copy_from_slice(&total.to_le_bytes());
        h[36..40].copy_from_slice(&rules.to_le_bytes());
        h
    }

    #[test]
    fn a_confirmation_is_a_byte_of_the_confirmers_own_file_and_a_count() {
        let dir = folder();
        let path = dir.join(".x.db.confirm.lee");
        assert_eq!(path_beside(&dir.join(".x.db.waive.lee")), path);
        assert_eq!(
            path_beside(Path::new("/tmp/.r.db.waive.lee-0123456789ab")),
            Path::new("/tmp/.r.db.confirm.lee-0123456789ab")
        );
        assert_eq!(path_beside(Path::new("/r/.r.db.tray")), Path::new("/r/.r.db.tray.confirm"));
        // rules of 5, 0 and 9000 errors
        let rules = vec![(0u64, 5u64), (5, 0), (5, 9000)];
        let header = header_of(&waive_header(9005, 3));
        assert_eq!((&header[..8], header[8], &header[12..]), (&MAGIC[..], 1, &waive_header(9005, 3)[12..]));
        let mut c = Confirms::open(path.clone(), header, rules.clone(), false);
        assert!(!c.present() && !path.exists() && c.notices.is_empty(), "nothing is made by the open");
        assert_eq!((c.counts(), c.get(2, 8999).unwrap()), (&[0u32, 0, 0][..], false));
        assert_eq!(c.statuses(2, 8990, 100).unwrap(), vec![0u8; 10]);
        // nothing changes: nothing made
        assert!(c.set(0, &[1, 2], false).unwrap().is_empty() && !path.exists());
        // the first confirmation makes the file: its header, a byte an
        // error, a counter a rule
        assert_eq!(c.set(2, &[0, 4096, 8999, 4096], true).unwrap(), [0, 4096, 8999]);
        assert_eq!(c.set(0, &[4], true).unwrap(), [4]);
        c.flush().unwrap();
        let bytes = fs::read(&path).unwrap();
        assert_eq!(bytes.len(), 40 + 9005 + 12);
        assert_eq!(&bytes[..40], &header[..]);
        assert_eq!((bytes[40 + 4], bytes[40 + 5], bytes[40 + 5 + 4096], bytes[40 + 5 + 8999]), (1, 1, 1, 1));
        assert_eq!(bytes[40..40 + 9005].iter().filter(|&&b| b == 1).count(), 4);
        assert_eq!(&bytes[40 + 9005..], &[1, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0]);
        assert_eq!(c.counts(), &[1, 0, 3]);
        assert!(c.get(2, 4096).unwrap() && !c.get(2, 4095).unwrap());
        // again: nothing changes; taken back: the byte and the counter
        assert!(c.set(2, &[0, 8999], true).unwrap().is_empty());
        assert_eq!(c.set(2, &[0, 7], false).unwrap(), [0]);
        assert_eq!(c.counts(), &[1, 0, 2]);
        // a span, as a job writes a chunk: what changed, the counter by the difference
        let before = c.statuses(2, 4090, 10).unwrap();
        let mut after = before.clone();
        after[0] = CONFIRMED;
        after[6] = 0; // error 4096
        after[9] = CONFIRMED;
        assert_eq!(c.write_span(2, 4090, &before, &after).unwrap(), 3);
        assert_eq!(c.write_span(2, 4090, &after, &after).unwrap(), 0);
        assert_eq!((c.counts(), c.get(2, 4096).unwrap(), c.get(2, 4099).unwrap()), (&[1u32, 0, 3][..], false, true));
        assert!(c.write_span(2, 8995, &[0; 6], &[1; 6]).is_err() && c.set(2, &[9000], true).is_err());
        assert!(c.get(1, 0).is_err() && c.statuses(3, 0, 1).is_err());
        drop(c);
        // read back: by the next session and by a reader
        let c = Confirms::open(path.clone(), header, rules.clone(), false);
        assert!(c.present() && c.counts() == [1, 0, 3] && c.get(0, 4).unwrap());
        let (file, counts) = read(&path, &header, 9005, 3).unwrap();
        assert_eq!(counts, [1, 0, 3]);
        let mut byte = [0u8];
        file.read_exact_at(&mut byte, 40 + 5 + 4090).unwrap();
        assert_eq!(byte[0], 1);
        drop(c);
        // not this result's: a reader says so, the session moves it aside
        let other = header_of(&waive_header(9005, 4));
        assert_eq!(read(&path, &other, 9005, 4).unwrap_err().kind, ErrorKind::Cache);
        assert_eq!(read(&path, &waive_header(9005, 3), 9005, 3).unwrap_err().kind, ErrorKind::Cache, "a waive header");
        let mut c = Confirms::open(path.clone(), other, vec![(0, 5), (5, 0), (5, 9000), (9005, 0)], false);
        assert!(!c.present() && !path.exists());
        assert!(c.notices.len() == 1 && c.notices[0].contains("moved aside"), "{:?}", c.notices);
        let aside: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(aside.len() == 1 && aside[0].starts_with(".x.db.confirm.lee.stale-"), "{aside:?}");
        assert_eq!(c.set(0, &[0], true).unwrap(), [0]);
        assert!(path.exists());
        // a review that writes nothing of its own confirms nothing
        let mut none = Confirms::none(3);
        assert!(!none.present() && none.counts() == [0, 0, 0] && none.flush().is_ok());
        assert!(none.set(0, &[0], true).is_err() && none.write_span(0, 0, &[], &[]).unwrap() == 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_locks_are_every_confirmers_confirmations_read_anew() {
        let (dir, temp) = (folder(), folder());
        let rules = vec![(0u64, 5u64), (5, 0), (5, 9000)];
        let header = header_of(&waive_header(9005, 3));
        let waive = [dir.join(".x.db.waive.kim"), temp.join(".x.db.waive.kim-0123456789ab")];
        let confirms = |folder: &Path, name: &str| Confirms::open(folder.join(name), header, rules.clone(), false);
        let tag = "abababababababababababababababab,9005".to_owned();
        let mut locks = Locks::beside(&waive, "kim", header, rules.clone(), tag.clone());
        locks.set_refresh(Duration::ZERO);
        // nobody has confirmed: nothing is locked, and nothing is made
        assert_eq!(locks.mask(2, 0, 9000).unwrap(), None);
        assert!(!locks.confirmed(2, 7).unwrap() && locks.by(0, 4).unwrap().is_empty());
        assert!(fs::read_dir(&dir).unwrap().next().is_none());
        // two confirmers beside the results, one in the temp folder (its
        // names end in the path's hash), the reviewer's own of another day
        let mut hong = confirms(&dir, ".x.db.confirm.hong");
        hong.set(2, &[7, 4096], true).unwrap();
        let mut park = confirms(&dir, ".x.db.confirm.park");
        park.set(2, &[7, 8999], true).unwrap();
        park.set(0, &[4], true).unwrap();
        let mut choi = confirms(&temp, ".x.db.confirm.choi-0123456789ab");
        choi.set(2, &[100], true).unwrap();
        let mut kim = confirms(&dir, ".x.db.confirm.kim");
        kim.set(2, &[200], true).unwrap();
        let ones = |mask: Option<Vec<bool>>| -> Vec<usize> {
            mask.map(|m| (0..m.len()).filter(|&j| m[j]).collect()).unwrap_or_default()
        };
        assert_eq!(ones(locks.mask(2, 0, 9000).unwrap()), [7, 100, 200, 4096, 8999]);
        assert_eq!(ones(locks.mask(2, 4090, 10).unwrap()), [6], "a span is its own");
        assert_eq!(ones(locks.mask(2, 8990, 100).unwrap()), [9], "clamped to the rule");
        assert_eq!(locks.mask(2, 300, 3000).unwrap(), None);
        assert_eq!((ones(locks.mask(0, 0, 5).unwrap()), locks.mask(1, 0, 5).unwrap()), (vec![4], None));
        assert_eq!(locks.by(2, 7).unwrap(), ["hong", "park"]);
        assert_eq!(locks.who(2, &[(0, 8), (8999, 9000)]).unwrap(), ["hong", "park"]);
        assert_eq!(locks.who(2, &[(8, 4096), (4097, 8999)]).unwrap(), ["choi", "kim"]);
        assert_eq!(locks.who(2, &[(0, 9999)]).unwrap(), ["choi", "hong", "kim", "park"]);
        assert!(locks.who(2, &[]).unwrap().is_empty() && locks.who(2, &[(8, 100)]).unwrap().is_empty());
        // asking about no error looks for no file: a waive that meets no
        // waived error must not list the folders (nor keep what it found)
        let mut idle = Locks::beside(&waive, "kim", header, rules.clone(), tag.clone());
        idle.set_refresh(Duration::from_secs(3600));
        assert!(idle.who(2, &[]).unwrap().is_empty() && idle.read.is_none());
        assert_eq!(idle.by(2, 7).unwrap(), ["hong", "park"]);
        assert!(idle.read.is_some());
        assert_eq!((locks.by(2, 100).unwrap(), locks.by(2, 200).unwrap()), (vec!["choi".to_owned()], vec!["kim".to_owned()]));
        assert!(locks.confirmed(2, 8999).unwrap() && !locks.confirmed(2, 8998).unwrap());
        assert!(locks.mask(3, 0, 1).is_err() && locks.by(2, 9000).is_err());
        // taken back by its confirmer: no lock; by one of two: the other's stays
        hong.set(2, &[7, 4096], false).unwrap();
        assert_eq!(ones(locks.mask(2, 0, 9000).unwrap()), [7, 100, 200, 8999]);
        assert_eq!(locks.by(2, 7).unwrap(), ["park"]);
        // what is no confirm file of this result locks nothing: a leftover
        // moved aside or being made, another result's, a file cut short, the
        // temp folder's name of another path, a waive file
        fs::write(dir.join(".x.db.confirm.lee.stale-1700000000"), fs::read(dir.join(".x.db.confirm.park")).unwrap()).unwrap();
        fs::write(dir.join(".x.db.confirm.lee.tmp-77"), fs::read(dir.join(".x.db.confirm.park")).unwrap()).unwrap();
        let mut other = Confirms::open(dir.join(".x.db.confirm.yoon"), header_of(&waive_header(9005, 4)), vec![(0, 5), (5, 0), (5, 9000), (9005, 0)], false);
        other.set(2, &[300], true).unwrap();
        fs::write(dir.join(".x.db.confirm.cut"), &fs::read(dir.join(".x.db.confirm.park")).unwrap()[..5000]).unwrap();
        fs::write(temp.join(".x.db.confirm.jung-ba9876543210"), fs::read(dir.join(".x.db.confirm.park")).unwrap()).unwrap();
        fs::write(dir.join(".x.db.waive.jung"), fs::read(dir.join(".x.db.confirm.park")).unwrap()).unwrap();
        assert_eq!(ones(locks.mask(2, 0, 9000).unwrap()), [7, 100, 200, 8999]);
        // a file gone since it was found locks nothing more once looked for
        // again; held open it is read as it was (and it was not changed)
        drop(park);
        fs::remove_file(dir.join(".x.db.confirm.park")).unwrap();
        assert_eq!(ones(locks.mask(2, 0, 9000).unwrap()), [100, 200]);
        // read anew only when what is held is old: a confirmer's first
        // confirmation - a new file - is seen at the next look, the bytes of
        // a file already held at once
        locks.set_refresh(Duration::from_secs(3600));
        assert_eq!(ones(locks.mask(2, 0, 9000).unwrap()), [100, 200]);
        let mut shin = confirms(&dir, ".x.db.confirm.shin");
        shin.set(2, &[1], true).unwrap();
        choi.set(2, &[101], true).unwrap();
        assert_eq!(ones(locks.mask(2, 0, 9000).unwrap()), [100, 101, 200]);
        locks.set_refresh(Duration::ZERO);
        assert_eq!(ones(locks.mask(2, 0, 9000).unwrap()), [1, 100, 101, 200]);
        // the confirmers' rejects logs (0.12.468), found beside the confirm
        // files - the temp folder's by its hashed name - and read again
        // when they are others
        let sizes = vec![5u64, 0, 9000];
        let rejects = |folder: &Path, name: &str| {
            Reasons::open_as("# floe drc waive rejections", [folder.join(name), temp.join("unused")], tag.clone(), sizes.clone(), false)
        };
        assert!(locks.rejected(2, 0, 9000, false).unwrap().is_none() && locks.rejections().0.is_empty());
        let mut hong_log = rejects(&dir, ".x.db.rejects.hong");
        hong_log.set(Reason::Reject, 2, &[(7, 9), (4095, 4097)], "not a dummy", 100).unwrap();
        let mut choi_log = rejects(&temp, ".x.db.rejects.choi-0123456789ab");
        choi_log.set(Reason::Reject, 2, &[(8, 10)], "", 200).unwrap();
        choi_log.set(Reason::Reject, 0, &[(4, 5)], "another rule", 300).unwrap();
        // (asked before a waive, no folder is listed: the logs are the
        // ones the last look found - none yet)
        assert!(locks.rejected(2, 0, 9000, true).unwrap().is_none() && locks.rejected_by(2, &[(8, 9)]).unwrap().is_empty());
        assert_eq!(ones(locks.rejected(2, 0, 9000, false).unwrap()), [7, 8, 9, 4095, 4096]);
        assert_eq!(ones(locks.rejected(2, 4090, 10, true).unwrap()), [5, 6]);
        assert_eq!((ones(locks.rejected(0, 0, 5, false).unwrap()), locks.rejected(1, 0, 5, false).unwrap()), (vec![4], None));
        let by = |found: Vec<Rejected>| found.into_iter().map(|r| (r.reviewer, r.text, r.time)).collect::<Vec<_>>();
        assert_eq!(by(locks.rejected_by(2, &[(8, 9)]).unwrap()), [("choi".into(), String::new(), 200), ("hong".into(), "not a dummy".into(), 100)]);
        assert_eq!(by(locks.rejected_by(2, &[(0, 7), (9, 10)]).unwrap()), [("choi".into(), String::new(), 200)]);
        assert!(locks.rejected_by(2, &[]).unwrap().is_empty() && locks.rejected_by(2, &[(100, 200)]).unwrap().is_empty());
        let (rules, seen) = locks.rejections();
        assert_eq!(rules, [(0, vec![(4, 5)]), (2, vec![(7, 10), (4095, 4097)])], "two confirmers' runs are joined");
        assert_eq!(seen.iter().map(|s| s.0.as_str()).collect::<Vec<_>>(), ["choi", "hong"]);
        assert_eq!(locks.rejections().1, seen, "the same answer while the logs stand");
        // a rejection taken back, and another made: a line more, read at the next ask
        hong_log.clear(Reason::Reject, 2, &[(7, 8)]);
        hong_log.flush(400).unwrap();
        hong_log.set(Reason::Reject, 2, &[(20, 21)], "later", 500).unwrap();
        assert_eq!(ones(locks.rejected(2, 0, 9000, false).unwrap()), [8, 9, 20, 4095, 4096]);
        assert_ne!(locks.rejections().1, seen);
        // read again where the list is not looked for anew: a log held is
        // asked its time and size at each ask
        locks.set_refresh(Duration::from_secs(3600));
        assert_eq!(ones(locks.rejected(2, 0, 30, false).unwrap()), [8, 9, 20]);
        hong_log.set(Reason::Reject, 2, &[(25, 26)], "", 600).unwrap();
        assert_eq!(ones(locks.rejected(2, 0, 30, false).unwrap()), [8, 9, 20, 25]);
        let mut kim_log = rejects(&dir, ".x.db.rejects.kim");
        kim_log.set(Reason::Reject, 2, &[(1, 2)], "", 700).unwrap();
        assert_eq!(ones(locks.rejected(2, 0, 30, false).unwrap()), [8, 9, 20, 25], "a new log is found at the next look");
        locks.set_refresh(Duration::ZERO);
        assert_eq!(ones(locks.rejected(2, 0, 30, false).unwrap()), [1, 8, 9, 20, 25]);
        // what is no rejects log of this result rejects nothing
        let mut other = Reasons::open_as("# floe drc waive rejections", [dir.join(".x.db.rejects.yoon"), temp.join("unused")], "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd,9005".to_owned(), sizes.clone(), false);
        other.set(Reason::Reject, 2, &[(50, 60)], "another result's", 800).unwrap();
        fs::write(dir.join(".x.db.rejects.lee.stale-1700000000"), fs::read(dir.join(".x.db.rejects.hong")).unwrap()).unwrap();
        fs::write(dir.join(".x.db.rejects.cut"), b"not a log").unwrap();
        fs::write(temp.join(".x.db.rejects.jung-ba9876543210"), fs::read(dir.join(".x.db.rejects.hong")).unwrap()).unwrap();
        assert_eq!(ones(locks.rejected(2, 0, 9000, false).unwrap()), [1, 8, 9, 20, 25, 4095, 4096]);
        // a log gone rejects nothing more
        drop(kim_log);
        fs::remove_file(dir.join(".x.db.rejects.kim")).unwrap();
        assert_eq!(ones(locks.rejected(2, 0, 30, false).unwrap()), [8, 9, 20, 25]);
        // a review that writes nothing has no locks to ask
        let mut none = Locks::none(3);
        assert!(none.mask(2, 0, 10).unwrap().is_none() && none.by(0, 0).is_err());
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&temp);
    }
}
