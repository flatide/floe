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
use super::local::aside_path;
use crate::{Error, ErrorKind, Result};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};

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
}
