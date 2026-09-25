//! Opt-in repair of an exact marked default's extra staging link. No scans,
//! legacy orphan cleanup, payload rewrite, or implicit repair on ordinary read.
use super::*;
use serde::{Deserialize, Serialize};

#[cfg(target_os = "macos")]
pub(super) const MARKER: &CStr = c"com.floe.default-stage-v1";
#[cfg(not(target_os = "macos"))]
pub(super) const MARKER: &CStr = c"user.floe.default-stage-v1";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Marker {
    v: u8,
    target: Vec<u8>,
    source: Vec<u8>,
    source_file: (u64, u64),
    source_stamp: (u64, i64, i64),
    stage: String,
    directory: (u64, u64),
    file: (u64, u64),
    lock: (u64, u64),
}
pub(super) fn mark(d: &Draft, stage: &Stage, lock: (u64, u64)) -> Result<()> {
    let bytes = serde_json::to_vec(&Marker {
        v: 1,
        target: d.name.as_bytes().to_vec(),
        source: d.source.path().as_os_str().as_bytes().to_vec(),
        source_file: identity(&fs::metadata(d.source.path())?),
        source_stamp: source_stamp(d)?,
        stage: stage.name().to_str().map_err(|_| conflict())?.into(),
        directory: identity(&d.directory.file.metadata()?),
        file: identity(&stage.file.metadata()?),
        lock,
    })
    .map_err(|_| conflict())?;
    if bytes.len() > 16384 {
        return Err(Error::input("default recovery marker exceeds limit"));
    }
    security::set(&stage.file, MARKER, &bytes)?;
    if Security::read(&stage.file)?.attribute(MARKER) != Some(bytes.as_slice()) {
        return Err(unsupported("cannot preserve default recovery marker"));
    }
    Ok(())
}
fn source_stamp(d: &Draft) -> Result<(u64, i64, i64)> {
    let m = fs::metadata(d.source.path())?;
    Ok((m.len(), m.mtime(), m.mtime_nsec()))
}
pub struct Recovery {
    draft: Draft,
    stage: CString,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryState {
    Pending,
    Completed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryResult {
    Recovered { directory_synced: bool },
    Uncertain,
}
struct Lock(File, (u64, u64));
impl Lock {
    fn acquire(d: &Draft) -> Result<Self> {
        d.publisher.protect(
            &d.directory
                .path
                .join(std::ffi::OsStr::from_bytes(d.lock_name.as_bytes())),
        )?;
        let file = d.directory.open_leaf(&d.lock_name, libc::O_RDWR, 0)?;
        let m = file.metadata()?;
        if !m.is_file() || m.nlink() != 1 || m.len() != 0 {
            return Err(conflict());
        }
        // SAFETY: live regular-file fd; same stable lock as publication.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(conflict());
        }
        let lock = Self(file, identity(&m));
        lock.validate(d)?;
        Ok(lock)
    }
    fn validate(&self, d: &Draft) -> Result<()> {
        let m = self.0.metadata()?;
        d.publisher.protect(
            &d.directory
                .path
                .join(std::ffi::OsStr::from_bytes(d.lock_name.as_bytes())),
        )?;
        if !m.is_file()
            || m.nlink() != 1
            || m.len() != 0
            || identity(&m) != self.1
            || d.directory.leaf_identity(&d.lock_name)? != self.1
        {
            return Err(conflict());
        }
        Ok(())
    }
}
fn marker(d: &Draft, c: &Capture) -> Result<CString> {
    let bytes = c
        .security
        .attribute(MARKER)
        .filter(|b| b.len() <= 16384)
        .ok_or_else(|| Error::input("default has no supported recovery marker"))?;
    let m: Marker = serde_json::from_slice(bytes)
        .map_err(|_| Error::input("invalid default recovery marker"))?;
    let valid = m
        .stage
        .strip_prefix(".floe-layerprops-")
        .and_then(|s| s.strip_suffix(".tmp"))
        .and_then(|s| s.split_once('-'))
        .is_some_and(|(a, b)| {
            !a.is_empty()
                && !b.is_empty()
                && a.bytes().all(|c| c.is_ascii_digit())
                && b.bytes().all(|c| c.is_ascii_digit())
        });
    if m.v != 1
        || !valid
        || m.target != d.name.as_bytes()
        || m.source != d.source.path().as_os_str().as_bytes()
        || m.source_file != identity(&fs::metadata(d.source.path())?)
        || m.source_stamp != source_stamp(d)?
        || m.directory != identity(&d.directory.file.metadata()?)
        || m.file != c.stamp.id
        || m.lock != d.directory.leaf_identity(&d.lock_name)?
    {
        return Err(conflict());
    }
    leaf(std::ffi::OsStr::new(&m.stage))
}
impl Publisher {
    /// Read-only preview. The registered source and mode derive the only target.
    pub fn prepare_recovery(
        self: &Arc<Self>,
        source: Arc<RegisteredSource>,
        mode: Mode,
        stop: &AtomicUsize,
    ) -> Result<Recovery> {
        let _registration = self.sources.publication(stop)?;
        let draft = self.prepare_bytes(source, mode, Vec::new(), 2, stop)?;
        let lock = Lock::acquire(&draft)?;
        let before = draft.before.as_ref().ok_or_else(conflict)?;
        let document =
            layerprops::parse(std::str::from_utf8(&before.bytes).map_err(|_| conflict())?)?;
        if document.malformed != 0 || document.rows.is_empty() {
            return Err(conflict());
        }
        let stage = marker(&draft, before)?;
        let recovery = Recovery { draft, stage };
        if recovery.state(stop)? != RecoveryState::Pending {
            return Err(conflict());
        }
        lock.validate(&recovery.draft)?;
        Ok(recovery)
    }
}
impl Recovery {
    pub fn target(&self) -> &Path {
        &self.draft.target
    }
    pub fn bytes(&self) -> usize {
        self.draft.before.as_ref().unwrap().bytes.len()
    }
    fn state(&self, stop: &AtomicUsize) -> Result<RecoveryState> {
        let d = &self.draft;
        check_cancelled(stop)?;
        d.directory.validate()?;
        d.source.validate(stop)?;
        d.publisher.protect(&d.target)?;
        d.publisher.protect(
            &d.directory
                .path
                .join(std::ffi::OsStr::from_bytes(self.stage.as_bytes())),
        )?;
        let file = d.directory.open_leaf(&d.name, libc::O_RDWR, 0)?;
        let links = file.metadata()?.nlink();
        if ![1, 2].contains(&links) {
            return Err(conflict());
        }
        let c =
            Capture::read_links(&d.directory, &d.name, true, links, stop)?.ok_or_else(conflict)?;
        let before = d.before.as_ref().unwrap();
        if marker(d, &c)? != self.stage
            || c.stamp.id != before.stamp.id
            || c.bytes != before.bytes
            || c.security != before.security
            || c.stamp.modified != before.stamp.modified
            || links == 2 && !c.same(before)
            || d.directory.leaf_identity(&d.name)? != c.stamp.id
        {
            return Err(conflict());
        }
        match d.directory.open_leaf(&self.stage, libc::O_RDWR, 0) {
            Ok(stage) if links == 2 => {
                if Stamp::of(&stage.metadata()?) != c.stamp
                    || d.directory.leaf_identity(&self.stage)? != c.stamp.id
                {
                    return Err(conflict());
                }
            }
            Err(e) if links == 1 && e.kind() == std::io::ErrorKind::NotFound => (),
            _ => return Err(conflict()),
        }
        if Stamp::of(&file.metadata()?) != c.stamp {
            return Err(conflict());
        }
        Ok(if links == 2 {
            RecoveryState::Pending
        } else {
            RecoveryState::Completed
        })
    }
    /// Read-only same-operation inspection is valid after preview expiry.
    pub fn reconcile(&self, stop: &AtomicUsize) -> Result<RecoveryState> {
        let _registration = self.draft.publisher.sources.publication(stop)?;
        let lock = Lock::acquire(&self.draft)?;
        let state = self.state(stop)?;
        lock.validate(&self.draft)?;
        Ok(state)
    }
    pub fn recover(&self, stop: &AtomicUsize) -> Result<RecoveryResult> {
        self.recover_using(
            stop,
            || {
                // SAFETY: exact validated marked leaf, not a caller-provided path.
                if unsafe {
                    libc::unlinkat(
                        self.draft.directory.file.as_raw_fd(),
                        self.stage.as_ptr(),
                        0,
                    )
                } == 0
                {
                    Ok(())
                } else {
                    Err(std::io::Error::last_os_error())
                }
            },
            File::sync_all,
        )
    }
    fn recover_using(
        &self,
        stop: &AtomicUsize,
        unlink: impl FnOnce() -> std::io::Result<()>,
        sync: impl FnOnce(&File) -> std::io::Result<()>,
    ) -> Result<RecoveryResult> {
        let d = &self.draft;
        let _registration = d.publisher.sources.publication(stop)?;
        let lock = Lock::acquire(d)?;
        let state = self.state(stop)?;
        lock.validate(d)?;
        if state == RecoveryState::Completed {
            return Ok(RecoveryResult::Recovered {
                directory_synced: sync(&d.directory.file).is_ok(),
            });
        }
        if Instant::now() >= d.expires {
            return Err(conflict());
        }
        check_cancelled(stop)?;
        let _result = unlink();
        // After the attempt cancellation must not turn a commit into rollback.
        if self.state(&AtomicUsize::new(0)).ok() != Some(RecoveryState::Completed)
            || lock.validate(d).is_err()
        {
            return Ok(RecoveryResult::Uncertain);
        }
        Ok(RecoveryResult::Recovered {
            directory_synced: sync(&d.directory.file).is_ok(),
        })
    }
}

#[cfg(test)]
#[path = "recovery_tests.rs"]
mod tests;
