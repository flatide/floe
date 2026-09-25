//! Explicit recovery of ONE completed DRC inode with its own staging link.
//! The marker travels with the inode, so no directory scan or second journal
//! publication is needed. It is provenance/change detection, not authorization.
//! The gateway requires separate explicit approval; never use automatic repair.
use super::*;
use serde::{Deserialize, Serialize};
use std::ffi::{CStr, OsStr};

#[cfg(target_os = "macos")]
pub(super) const MARKER: &CStr = c"com.floe.review-stage-v1";
#[cfg(not(target_os = "macos"))]
pub(super) const MARKER: &CStr = c"user.floe.review-stage-v1";
const MARKER_BYTES: usize = 4096;

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Marker {
    v: u8,
    target: Vec<u8>,
    stage: String,
    directory: (u64, u64),
    file: (u64, u64),
    lock: (u64, u64),
}
pub(super) fn mark(store: &Store, stage: &mut Stage, lock: (u64, u64)) -> Result<()> {
    let marker = Marker {
        v: 1,
        target: store.name.as_bytes().to_vec(),
        stage: stage.name().to_str().map_err(|_| conflict())?.into(),
        directory: identity(&store.directory.file.metadata()?),
        file: identity(&stage.file.metadata()?),
        lock,
    };
    let bytes = serde_json::to_vec(&marker).map_err(|_| conflict())?;
    if bytes.len() > MARKER_BYTES {
        return Err(Error::input("review recovery marker exceeds limit"));
    }
    stage.set_owned(MARKER, &bytes)
}

/// Opaque, expiring approval preview tied to one registered writer and inode.
/// A gateway must authorize recovery separately from read or autosave. Never
/// serialize paths, marker data, descriptors or this capability to a client.
pub struct Recovery {
    store: Arc<Store>,
    before: Capture,
    stage: CString,
    bytes: u64,
    expires: Instant,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryState {
    Pending,
    Completed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryResult {
    Recovered {
        already_completed: bool,
        directory_synced: bool,
    },
    /// An unlink attempt was made but its result could not be proven. Keep
    /// THIS operation and reconcile it; do not issue a fresh save or repair.
    Uncertain,
}

struct Lock(File, (u64, u64));
impl Lock {
    fn acquire(s: &Store) -> Result<Self> {
        s.protect(&s.lock_path())?;
        // Recovery never creates a lock: the original publisher created it
        // before staging. Opening for write also honors existing permissions.
        let file = s.directory.open_leaf(&s.lock_name, libc::O_RDWR, 0)?;
        let m = file.metadata()?;
        if !m.is_file() || m.nlink() != 1 || m.len() != 0 {
            return Err(Error::input("invalid review recovery lock"));
        }
        // SAFETY: live regular-file fd; same stable lock as publication.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let error = std::io::Error::last_os_error();
            return Err(if error.kind() == std::io::ErrorKind::WouldBlock {
                conflict()
            } else {
                error.into()
            });
        }
        let lock = Self(file, identity(&m));
        lock.validate(s)?;
        Ok(lock)
    }
    fn validate(&self, s: &Store) -> Result<()> {
        let m = self.0.metadata()?;
        s.protect(&s.lock_path())?;
        if !m.is_file()
            || m.nlink() != 1
            || m.len() != 0
            || identity(&m) != self.1
            || s.directory.leaf_identity(&s.lock_name)? != self.1
        {
            return Err(conflict());
        }
        Ok(())
    }
}

fn marker(s: &Store, capture: &Capture) -> Result<CString> {
    if capture.attribute(BINDING) != Some(s.binding.as_slice()) {
        return Err(Error::input(
            "review recovery requires the exact pack binding",
        ));
    }
    let bytes = capture
        .attribute(MARKER)
        .filter(|v| v.len() <= MARKER_BYTES)
        .ok_or_else(|| Error::input("review has no supported recovery marker"))?;
    let marker: Marker = serde_json::from_slice(bytes)
        .map_err(|_| Error::input("invalid review recovery marker"))?;
    // Only names emitted by Stage::create_for(review) are candidates. This is
    // a syntax check, not proof: identity, content and permissions follow.
    let parts = marker
        .stage
        .strip_prefix(".floe-review-")
        .and_then(|v| v.strip_suffix(".tmp"))
        .and_then(|v| v.split_once('-'));
    let valid_name = parts.is_some_and(|(pid, serial)| {
        !pid.is_empty()
            && !serial.is_empty()
            && pid.bytes().all(|b| b.is_ascii_digit())
            && serial.bytes().all(|b| b.is_ascii_digit())
    });
    if marker.v != 1
        || marker.target != s.name.as_bytes()
        || !valid_name
        || marker.directory != identity(&s.directory.file.metadata()?)
        || marker.file != identity(&capture.file.metadata()?)
        || marker.lock != s.directory.leaf_identity(&s.lock_name)?
    {
        return Err(Error::input("review recovery marker does not match target"));
    }
    leaf(OsStr::new(&marker.stage))
}

// Descriptor-relative inspection of exactly the marked sibling, never a scan.
fn pair(s: &Store, c: &Capture, stage: &CStr, links: u64) -> Result<()> {
    let id = identity(&c.file.metadata()?);
    s.protect(&s.directory.path.join(OsStr::from_bytes(stage.to_bytes())))?;
    if s.directory.leaf_identity(&s.name)? != id {
        return Err(conflict());
    }
    match s.directory.open_leaf(stage, libc::O_RDWR, 0) {
        Ok(file) if links == 2 => {
            let m = file.metadata()?;
            if !m.is_file()
                || m.nlink() != 2
                || identity(&m) != id
                || Stamp::of(&m) != c.stamp
                || s.directory.leaf_identity(stage)? != id
            {
                return Err(conflict());
            }
        }
        Err(e) if links == 1 && e.kind() == std::io::ErrorKind::NotFound => (),
        _ => return Err(conflict()),
    }
    c.unchanged()
}

impl Store {
    /// Read-only preview; requires a writer capability, existing stable lock,
    /// the exact pack binding and a new-version marker. Old orphans are refused.
    pub fn prepare_recovery(self: &Arc<Self>, stop: &AtomicUsize) -> Result<Recovery> {
        self.require_editor()?;
        let _registration = self.sources.publication(stop)?;
        self.validate(stop)?;
        let lock = Lock::acquire(self)?;
        let before = Capture::read_links(self, true, 2, stop)?.ok_or_else(conflict)?;
        let stage = marker(self, &before)?;
        pair(self, &before, &stage, 2)?;
        // Validate the completed payload too; the marker alone is not enough.
        let mut input = &before.file;
        input.rewind()?;
        match self.kind {
            Kind::Notes => {
                let mut text = String::new();
                input
                    .take(SIDECAR_BYTES as u64 + 1)
                    .read_to_string(&mut text)?;
                Notes::parse(&text, self.layout.fingerprint(), stop)?;
            }
            Kind::Waives => {
                rewrite_waives(input, std::io::sink(), &self.layout, &[], stop)?;
            }
        }
        before.unchanged()?;
        self.validate(stop)?;
        lock.validate(self)?;
        pair(self, &before, &stage, 2)?;
        let bytes = before.file.metadata()?.len();
        Ok(Recovery {
            store: Arc::clone(self),
            before,
            stage,
            bytes,
            expires: Instant::now() + TTL,
        })
    }
}

impl Recovery {
    /// Trusted caller information; a UI should show only the registered target
    /// label, never the internal staging filename or filesystem identities.
    pub fn kind(&self) -> Kind {
        self.store.kind
    }
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
    fn state(&self, stop: &AtomicUsize) -> Result<RecoveryState> {
        let s = &self.store;
        s.require_editor()?;
        s.validate(stop)?;
        // Read nlink from the captured inode; Capture then verifies the named
        // current file. A replacement with identical bytes is not this commit.
        let links = self.before.file.metadata()?.nlink();
        if ![1, 2].contains(&links) {
            return Err(conflict());
        }
        let current = Capture::read_links(s, true, links, stop)?.ok_or_else(conflict)?;
        if marker(s, &current)? != self.stage
            || identity(&current.file.metadata()?) != identity(&self.before.file.metadata()?)
            || current.digest != self.before.digest
            || current.security != self.before.security
            || !current.evidence.same(&self.before.evidence)
            || links == 2 && !current.same(&self.before)
        {
            return Err(conflict());
        }
        pair(s, &current, &self.stage, links)?;
        s.validate(stop)?;
        Ok(if links == 2 {
            RecoveryState::Pending
        } else {
            RecoveryState::Completed
        })
    }
    /// Read-only check of the SAME operation, including after approval expiry.
    /// An error means unknown/conflicting, not permission for a new publication.
    pub fn reconcile(&self, stop: &AtomicUsize) -> Result<RecoveryState> {
        let _registration = self.store.sources.publication(stop)?;
        let lock = Lock::acquire(&self.store)?;
        let state = self.state(stop)?;
        lock.validate(&self.store)?;
        Ok(state)
    }
    /// Explicit owner-approved repair: only unlink the exact extra stage name.
    /// Never rewrite/re-publish the payload, remove the target, or clear xattrs.
    pub fn recover(&self, stop: &AtomicUsize) -> Result<RecoveryResult> {
        self.recover_using(
            stop,
            || {
                // SAFETY: validated single-component marked name and held directory.
                let rc = unsafe {
                    libc::unlinkat(
                        self.store.directory.file.as_raw_fd(),
                        self.stage.as_ptr(),
                        0,
                    )
                };
                if rc == 0 {
                    Ok(())
                } else {
                    Err(std::io::Error::last_os_error())
                }
            },
            File::sync_all,
        )
    }
    pub(super) fn recover_using(
        &self,
        stop: &AtomicUsize,
        unlink: impl FnOnce() -> std::io::Result<()>,
        sync: impl FnOnce(&File) -> std::io::Result<()>,
    ) -> Result<RecoveryResult> {
        let s = &self.store;
        let _registration = s.sources.publication(stop)?;
        let lock = Lock::acquire(s)?;
        let state = self.state(stop)?;
        lock.validate(s)?;
        if state == RecoveryState::Completed {
            return Ok(RecoveryResult::Recovered {
                already_completed: true,
                directory_synced: sync(&s.directory.file).is_ok(),
            });
        }
        if Instant::now() >= self.expires {
            return Err(conflict());
        }
        self.before.unchanged()?;
        s.validate(stop)?;
        lock.validate(s)?;
        pair(s, &self.before, &self.stage, 2)?;
        check_cancelled(stop)?;
        // Like publication, this is not an atomic CAS against noncooperating
        // local writers. Stable flock excludes cooperating Floe publishers.
        let _result = unlink();
        // On NFS, a reported syscall failure may have committed. Ignore later
        // cancellation, verify this inode, and never automatically retry unlink.
        let post = AtomicUsize::new(0);
        if self.state(&post).ok() != Some(RecoveryState::Completed) || lock.validate(s).is_err() {
            return Ok(RecoveryResult::Uncertain);
        }
        Ok(RecoveryResult::Recovered {
            already_completed: false,
            directory_synced: sync(&s.directory.file).is_ok(),
        })
    }
}

#[cfg(test)]
#[path = "recovery_tests.rs"]
mod tests;
