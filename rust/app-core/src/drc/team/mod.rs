//! A reviewer's visited / not-required flags and the team's shared review
//! (feature/jobdeck-drc 2026-10-09..11; the reference is tools/oracle
//! floe_oracle drc_review.py, drc_team.py and drc_migrate.py):
//!
//! - `flags`: the per-reviewer SQLite store of visits (recorded when an
//!   error's own geometry was drawn) and "not for review" exclusions;
//! - `snapshot`: `.floereview` snapshots - one reviewer's visited, waived
//!   and not-required planes per 4096 errors with paging indexes -, the
//!   reading of peers' snapshots, working review files or waive files,
//!   the team aggregate and its conflicts, and applying a peer's
//!   decisions to one's own review;
//! - `migrate`: carrying review from a previous DRC run to the errors with
//!   exactly the same geometry (drc::matcher).
//!
//! SQLite (rusqlite's bundled amalgamation) behind the app-core feature
//! `review-sqlite`; floe2 turns it on.
pub mod bits;
pub mod flags;
pub mod migrate;
pub mod snapshot;

use crate::drc::desktop::{Opened, Review, StatusReader};
use crate::{Error, ErrorKind, Result};
use flags::FlagStore;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

pub use bits::{Bits, CHUNK};

/// What the team's files are checked against: one open pack's review.
#[derive(Clone, Debug)]
pub struct Identity {
    /// Pack::review_fingerprint
    pub fingerprint: String,
    pub sizes: Vec<u64>,
    /// each rule's first global error id
    pub starts: Vec<u64>,
    pub reviewer: String,
    pub pack: PathBuf,
    pub waive_path: PathBuf,
    pub waive_header: [u8; 40],
}

impl Identity {
    pub fn of(review: &Review) -> Result<Self> {
        Ok(Self {
            fingerprint: review.pack.review_fingerprint()?,
            sizes: review.pack.checks.iter().map(|c| c.count).collect(),
            starts: review.pack.checks.iter().map(|c| c.start).collect(),
            reviewer: review.reviewer.clone(),
            pack: review.pack.path.clone(),
            waive_path: review.waive_path.clone(),
            waive_header: review.waive_header(),
        })
    }
    /// A pack opened without its review (a previous run): its waive
    /// header and names for `reviewer`.
    pub fn of_pack(pack: &crate::drc::Pack, reviewer: &str) -> Result<Self> {
        Ok(Self {
            fingerprint: pack.review_fingerprint()?,
            sizes: pack.checks.iter().map(|c| c.count).collect(),
            starts: pack.checks.iter().map(|c| c.start).collect(),
            reviewer: reviewer.to_owned(),
            pack: pack.path.clone(),
            waive_path: crate::drc::waive_paths(&pack.path, reviewer)?[0].clone(),
            waive_header: crate::drc::review::Layout::from_pack(pack)?.header(),
        })
    }
    pub fn total(&self) -> u64 {
        self.sizes.iter().sum()
    }
    pub(crate) fn rule(&self, check: usize) -> Result<u64> {
        self.sizes
            .get(check)
            .copied()
            .ok_or_else(|| Error::input("DRC check index out of range"))
    }
}

/// One's own review as the team operations reach it: the review (status
/// writes, under its lock), its statuses read apart, and the flags.
pub struct Own<'a> {
    pub id: &'a Identity,
    pub opened: &'a Mutex<Opened>,
    pub statuses: &'a StatusReader,
    pub flags: &'a FlagStore,
}

impl Own<'_> {
    pub(crate) fn lock(&self) -> MutexGuard<'_, Opened> {
        self.opened.lock().unwrap_or_else(|p| p.into_inner())
    }
    /// Waived counts of every rule and the status generation, read at once.
    pub(crate) fn waived_and_generation(&self) -> Result<(Vec<u64>, u64)> {
        let mut g = self.lock();
        let r = review(&mut g)?;
        let waived = (0..self.id.sizes.len())
            .map(|c| r.status_counts(c).map(|(w, _)| w))
            .collect::<Result<Vec<_>>>()?;
        Ok((waived, r.status_generation()))
    }
    pub(crate) fn generation(&self) -> Result<u64> {
        let mut g = self.lock();
        Ok(review(&mut g)?.status_generation())
    }
}

pub(crate) fn review<'g>(g: &'g mut MutexGuard<'_, Opened>) -> Result<&'g mut Review> {
    match &mut **g {
        Opened::Pack(r) => Ok(r),
        Opened::Ascii(_) => Err(Error::new(
            ErrorKind::Unsupported,
            "review sharing needs a packed index: floe-index drc <db>",
        )),
    }
}

/// The bits of `statuses` equal to waived.
pub(crate) fn waived_bits(statuses: &[u8]) -> Bits {
    let mut b = Bits::ZERO;
    for (i, &s) in statuses.iter().enumerate() {
        if s == crate::drc::desktop::STATUS_WAIVED {
            b.set(i as u64);
        }
    }
    b
}

/// A progress callback at most every half second (and at the end).
pub struct Progress<'a> {
    callback: &'a mut dyn FnMut(u64, u64, &str),
    total: u64,
    last: Instant,
}
impl<'a> Progress<'a> {
    pub fn new(callback: &'a mut dyn FnMut(u64, u64, &str), total: u64) -> Self {
        Self {
            callback,
            total,
            last: Instant::now(),
        }
    }
    pub fn report(&mut self, done: u64) {
        self.report_text(done, self.total, "", false);
    }
    pub fn finish(&mut self, done: u64) {
        self.report_text(done, self.total, "", true);
    }
    pub fn report_text(&mut self, done: u64, total: u64, text: &str, last: bool) {
        if last || self.last.elapsed() >= Duration::from_millis(500) {
            self.last = Instant::now();
            (self.callback)(done, total, text);
        }
    }
}

/// A private folder removed with everything in it when dropped.
#[derive(Debug)]
pub struct TempDir(pub PathBuf);
impl TempDir {
    pub fn new(prefix: &str) -> Result<Self> {
        let base = std::env::temp_dir();
        for _ in 0..100 {
            let p = base.join(format!("{prefix}{}", random_hex(6)));
            match std::fs::create_dir(&p) {
                Ok(()) => return Ok(Self(p)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Err(Error::new(ErrorKind::Io, "no temporary folder could be made"))
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(crate) fn random_hex(bytes: usize) -> String {
    let mut b = vec![0u8; bytes];
    if getrandom::fill(&mut b).is_err() {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
            ^ u128::from(std::process::id());
        for (i, x) in b.iter_mut().enumerate() {
            *x = (t >> ((i % 16) * 8)) as u8;
        }
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// JSON text with every non-ASCII character escaped (Python's json.dumps
/// default, `ensure_ascii=True`).
pub(crate) fn ascii_json(value: &serde_json::Value) -> String {
    let text = value.to_string();
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            let mut buf = [0u16; 2];
            for u in c.encode_utf16(&mut buf) {
                out.push_str(&format!("\\u{u:04x}"));
            }
        }
    }
    out
}

/// A file's (size, mtime ns, ctime ns, inode): a change while reading it.
pub(crate) fn stamp(path: &Path) -> Result<(u64, i128, i128, u64)> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(path)?;
    Ok((
        m.len(),
        i128::from(m.mtime()) * 1_000_000_000 + i128::from(m.mtime_nsec()),
        i128::from(m.ctime()) * 1_000_000_000 + i128::from(m.ctime_nsec()),
        m.ino(),
    ))
}
