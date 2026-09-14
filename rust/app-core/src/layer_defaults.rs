//! Explicit shared-default publication, not ordinary session settings Save.
//! Trusted callers register the entire source set and obtain a read-only draft;
//! only consuming that draft writes the exact derived sidecar. HTTP consent,
//! view/revision binding and operation receipts belong to the caller.
use crate::{
    check_cancelled,
    jobdeck::{color::Mode, dataset::props_source},
    layerprops,
    registered::{RegisteredSource, MAX_SOURCES},
    Error, ErrorKind, Result,
};
use std::{
    ffi::{CStr, CString},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt},
        },
    },
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant, SystemTime},
};

pub(crate) mod security;
use security::Security;
const DRAFT_TTL: Duration = Duration::from_secs(120);
static SERIAL: AtomicU64 = AtomicU64::new(0);

fn conflict() -> Error {
    Error::new(ErrorKind::Busy, "design default changed; prepare it again")
}
fn unsupported(message: &str) -> Error {
    Error::new(ErrorKind::Unsupported, message)
}

/// Creating this capability is an explicit local opt-in. It does not itself
/// change a source, create a lock file, or grant a browser any permission.
pub struct Publisher {
    sources: Vec<Arc<RegisteredSource>>,
    protected_files: Vec<PathBuf>,
    protected_trees: Vec<PathBuf>,
}
impl Publisher {
    pub fn new(sources: Vec<Arc<RegisteredSource>>) -> Result<Arc<Self>> {
        Self::with_protected(sources, Vec::new(), Vec::new())
    }
    /// Additional local registrations such as DRC/waive/SVRF inputs. These
    /// only restrict derived sidecar targets; they never grant output paths.
    pub fn with_protected(
        sources: Vec<Arc<RegisteredSource>>,
        protected_files: Vec<PathBuf>,
        protected_trees: Vec<PathBuf>,
    ) -> Result<Arc<Self>> {
        if sources.is_empty() || sources.len() > MAX_SOURCES {
            return Err(Error::input(
                "default publisher requires 1..32 registered sources",
            ));
        }
        if protected_files.len() > 128 || protected_trees.len() > 128 {
            return Err(Error::input("too many protected default publication paths"));
        }
        Ok(Arc::new(Self {
            sources,
            protected_files,
            protected_trees,
        }))
    }
    fn protect(&self, path: &Path) -> Result<()> {
        crate::artifact::protected_output(path, &self.protected_files, &self.protected_trees)?;
        reject_aliases(path, &self.protected_files)?;
        for source in &self.sources {
            source.protect_output(path)?;
        }
        Ok(())
    }
    pub fn prepare(
        self: &Arc<Self>,
        source: Arc<RegisteredSource>,
        mode: Mode,
        text: &str,
        stop: &AtomicUsize,
    ) -> Result<Draft> {
        check_cancelled(stop)?;
        if !self.sources.iter().any(|s| Arc::ptr_eq(s, &source))
            || (!source.deck && mode != Mode::Level)
        {
            return Err(Error::input(
                "source/mode is outside this default publisher",
            ));
        }
        let document = layerprops::parse(text)?;
        if document.malformed != 0 || document.rows.is_empty() {
            return Err(Error::input(
                "design default requires valid layer property rows",
            ));
        }
        let text = layerprops::format(&document.rows)?.into_bytes();
        source.validate(stop)?;
        let props = if source.deck {
            props_source(source.path(), mode)?
        } else {
            source.path().to_owned()
        };
        let mut target = props.into_os_string();
        target.push(".layerprops");
        let target = source.scoped_output(Path::new(&target))?;
        self.protect(&target)?;
        let directory = Directory::open(
            target
                .parent()
                .ok_or_else(|| Error::input("default has no parent"))?,
        )?;
        let name = leaf(
            target
                .file_name()
                .ok_or_else(|| Error::input("default has no name"))?,
        )?;
        let target = directory.path.join(target.file_name().unwrap());
        let lock_name = leaf(std::ffi::OsStr::from_bytes(
            &[name.as_bytes(), b".lock"].concat(),
        ))?;
        self.protect(
            &directory
                .path
                .join(std::ffi::OsStr::from_bytes(lock_name.as_bytes())),
        )?;
        let before = Capture::read(&directory, &name, false, stop)?;
        directory.validate()?;
        source.validate(stop)?;
        Ok(Draft {
            publisher: Arc::clone(self),
            source,
            directory,
            name,
            lock_name,
            target,
            text,
            before,
            expires: Instant::now() + DRAFT_TTL,
        })
    }
}
/// Case-insensitive aliases can share an inode with nlink==1. Path spelling
/// and the target's single-link guard alone are not an identity boundary.
pub(crate) fn reject_aliases(path: &Path, files: &[PathBuf]) -> Result<()> {
    let metadata = |p: &Path| match fs::metadata(p) {
        Ok(m) => Ok(Some(m)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::from(e)),
    };
    if let Some(target) = metadata(path)? {
        for file in files {
            if metadata(file)?.is_some_and(|m| identity(&m) == identity(&target)) {
                return Err(Error::input("default output aliases a registered input"));
            }
        }
    }
    Ok(())
}
pub struct Draft {
    publisher: Arc<Publisher>,
    source: Arc<RegisteredSource>,
    directory: Arc<Directory>,
    name: CString,
    lock_name: CString,
    target: PathBuf,
    text: Vec<u8>,
    before: Option<Capture>,
    expires: Instant,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Published {
    /// Rename/link already committed. A false value is a durability warning,
    /// NOT permission to retry the publication or report it as cancelled.
    pub directory_synced: bool,
}
impl Draft {
    /// Trusted local information; a gateway must expose only the basename.
    pub fn target(&self) -> &Path {
        &self.target
    }
    pub fn bytes(&self) -> usize {
        self.text.len()
    }
    pub fn replaces_existing(&self) -> bool {
        self.before.is_some()
    }
    pub fn publish(self, stop: &AtomicUsize) -> Result<Published> {
        self.publish_with(stop, || Ok(()))
    }
    fn current(&self, writable: bool, stop: &AtomicUsize) -> Result<()> {
        check_cancelled(stop)?;
        if Instant::now() >= self.expires {
            return Err(conflict());
        }
        self.directory.validate()?;
        self.source.validate(stop)?;
        self.publisher.protect(&self.target)?;
        let current = Capture::read(&self.directory, &self.name, writable, stop)?;
        if !match (&self.before, &current) {
            (None, None) => true,
            (Some(a), Some(b)) => a.same(b),
            _ => false,
        } {
            return Err(conflict());
        }
        Ok(())
    }
    fn publish_with(
        self,
        stop: &AtomicUsize,
        before_commit: impl FnOnce() -> Result<()>,
    ) -> Result<Published> {
        self.publish_using(stop, before_commit, File::sync_all)
    }
    fn publish_using(
        self,
        stop: &AtomicUsize,
        before_commit: impl FnOnce() -> Result<()>,
        sync_directory: impl FnOnce(&File) -> std::io::Result<()>,
    ) -> Result<Published> {
        self.current(false, stop)?;
        self.publisher.protect(
            &self
                .directory
                .path
                .join(std::ffi::OsStr::from_bytes(self.lock_name.as_bytes())),
        )?;
        let lock = self.directory.open_lock(&self.lock_name, 0o666)?;
        let lm = lock.metadata()?;
        if !lm.is_file() || lm.nlink() != 1 || lm.len() != 0 {
            return Err(Error::input("invalid design-default lock file"));
        }
        // SAFETY: a live regular-file descriptor; nonblocking OS advisory lock.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let e = std::io::Error::last_os_error();
            return Err(if e.kind() == std::io::ErrorKind::WouldBlock {
                Error::new(ErrorKind::Busy, "design-default publisher is busy")
            } else {
                e.into()
            });
        }
        self.current(true, stop)?; // Open-without-truncate honors existing file write access.
        let mut staged = Stage::create(Arc::clone(&self.directory), &self.publisher)?;
        for chunk in self.text.chunks(64 * 1024) {
            check_cancelled(stop)?;
            staged.file.write_all(chunk)?;
        }
        let security = self
            .before
            .as_ref()
            .map(|c| &c.security)
            .unwrap_or(&staged.creation_security);
        security.apply(&staged.file)?;
        if Security::read(&staged.file)? != *security {
            return Err(unsupported(
                "cannot preserve design-default permissions/attributes",
            ));
        }
        staged
            .file
            .set_times(fs::FileTimes::new().set_modified(SystemTime::now()))?;
        staged.file.sync_all()?;
        before_commit()?;
        self.current(true, stop)?;
        if self.directory.leaf_identity(&self.lock_name)? != identity(&lm) {
            return Err(conflict());
        }
        staged.validate()?;
        check_cancelled(stop)?;
        // All cooperating publishers hold this stable sidecar lock through
        // revalidation and commit. It is deliberately never unlinked: replacing
        // a lock inode would let two processes hold different exclusive locks.
        // Noncooperating writes after the last check are not a filesystem CAS.
        staged.commit(&self.name, self.before.is_some())?;
        // Never turn an already committed result into a cancellation/error.
        let directory_synced = sync_directory(&self.directory.file).is_ok();
        Ok(Published { directory_synced })
    }
}
pub(crate) fn leaf(name: &std::ffi::OsStr) -> Result<CString> {
    let b = name.as_bytes();
    if b.is_empty() || b == b"." || b == b".." || b.contains(&b'/') {
        return Err(Error::input("invalid sidecar leaf name"));
    }
    CString::new(b).map_err(|_| Error::input("NUL in sidecar name"))
}
pub(crate) fn identity(m: &fs::Metadata) -> (u64, u64) {
    (m.dev(), m.ino())
}
pub(crate) struct Directory {
    pub(crate) file: File,
    pub(crate) path: PathBuf,
    id: (u64, u64),
}
impl Directory {
    pub(crate) fn open(path: &Path) -> Result<Arc<Self>> {
        let path = fs::canonicalize(path)?;
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_NONBLOCK)
            .open(&path)?;
        let id = identity(&file.metadata()?);
        Ok(Arc::new(Self { file, path, id }))
    }
    pub(crate) fn validate(&self) -> Result<()> {
        if identity(&fs::symlink_metadata(&self.path)?) != self.id {
            return Err(conflict());
        }
        Ok(())
    }
    pub(crate) fn open_leaf(&self, name: &CStr, flags: i32, mode: u32) -> std::io::Result<File> {
        // SAFETY: valid dir fd and NUL-terminated basename; no paths/traversal.
        let fd = unsafe {
            libc::openat(
                self.file.as_raw_fd(),
                name.as_ptr(),
                flags | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
                mode as libc::c_uint,
            )
        };
        if fd < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(unsafe { File::from_raw_fd(fd) })
        }
    }
    pub(crate) fn open_lock(&self, name: &CStr, mode: u32) -> std::io::Result<File> {
        // Separate creation from opening a stable existing inode. Concurrent
        // O_CREAT (without O_EXCL) returned ENOENT in our macOS first-writer
        // test. Only EEXIST permits fallback; never truncate or unlink a lock.
        match self.open_leaf(name, libc::O_RDWR | libc::O_CREAT | libc::O_EXCL, mode) {
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                self.open_leaf(name, libc::O_RDWR, 0)
            }
            result => result,
        }
    }
    pub(crate) fn leaf_identity(&self, name: &CStr) -> std::io::Result<(u64, u64)> {
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: live dir fd/CString and writable stat storage; no symlink follow.
        if unsafe {
            libc::fstatat(
                self.file.as_raw_fd(),
                name.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: successful fstatat initialized the entire stat structure.
        let stat = unsafe { stat.assume_init() };
        // Darwin dev_t is signed; Linux uses unsigned platform-sized fields.
        #[allow(clippy::unnecessary_cast)]
        Ok((stat.st_dev as u64, stat.st_ino as u64))
    }
}
#[derive(PartialEq, Eq)]
pub(crate) struct Stamp {
    id: (u64, u64),
    len: u64,
    modified: (i64, i64),
    changed: (i64, i64),
    mode: u32,
    uid: u32,
    gid: u32,
    nlink: u64,
}
impl Stamp {
    pub(crate) fn of(m: &fs::Metadata) -> Self {
        Self {
            id: identity(m),
            len: m.len(),
            modified: (m.mtime(), m.mtime_nsec()),
            changed: (m.ctime(), m.ctime_nsec()),
            mode: m.mode(),
            uid: m.uid(),
            gid: m.gid(),
            nlink: m.nlink(),
        }
    }
}
struct Capture {
    stamp: Stamp,
    bytes: Vec<u8>,
    security: Security,
}
impl Capture {
    fn read(
        dir: &Directory,
        name: &CStr,
        writable: bool,
        stop: &AtomicUsize,
    ) -> Result<Option<Self>> {
        let file = match dir.open_leaf(
            name,
            if writable {
                libc::O_RDWR
            } else {
                libc::O_RDONLY
            },
            0,
        ) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let m = file.metadata()?;
        if !m.is_file() || m.nlink() != 1 || m.len() > layerprops::MAX_BYTES as u64 {
            return Err(Error::input(
                "default must be a single-link regular file of at most 4 MiB",
            ));
        }
        let stamp = Stamp::of(&m);
        let security = Security::read(&file)?;
        let mut bytes = Vec::new();
        (&file)
            .take(layerprops::MAX_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        check_cancelled(stop)?;
        if bytes.len() > layerprops::MAX_BYTES || Stamp::of(&file.metadata()?) != stamp {
            return Err(conflict());
        }
        Ok(Some(Self {
            stamp,
            bytes,
            security,
        }))
    }
    fn same(&self, other: &Self) -> bool {
        self.stamp == other.stamp && self.bytes == other.bytes && self.security == other.security
    }
}
pub(crate) struct Stage {
    directory: Arc<Directory>,
    name: CString,
    pub(crate) file: File,
    pub(crate) creation_security: Security,
    linked: bool,
    id: (u64, u64),
}
impl Stage {
    fn create(directory: Arc<Directory>, publisher: &Publisher) -> Result<Self> {
        Self::create_for(directory, "layerprops", |path| publisher.protect(path))
    }
    /// Shared descriptor-relative staging for trusted local sidecar writers.
    /// Every candidate is checked against registered inputs before O_EXCL.
    pub(crate) fn create_for(
        directory: Arc<Directory>,
        label: &str,
        mut protect: impl FnMut(&Path) -> Result<()>,
    ) -> Result<Self> {
        if label.is_empty() || !label.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(Error::input("invalid staging namespace"));
        }
        for _ in 0..128 {
            let n = SERIAL
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
                .map_err(|_| Error::input("default temporary sequence exhausted"))?;
            let name =
                CString::new(format!(".floe-{label}-{}-{n}.tmp", std::process::id())).unwrap();
            protect(
                &directory
                    .path
                    .join(std::ffi::OsStr::from_bytes(name.as_bytes())),
            )?;
            let file = match directory.open_leaf(
                &name,
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
                0o666,
            ) {
                Ok(f) => f,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            };
            let id = identity(&file.metadata()?);
            // Capture the filesystem/umask/default-ACL policy before making
            // this empty staging file private. Never read/change process umask.
            let mut stage = Self {
                directory,
                name,
                file,
                creation_security: Security::empty(),
                linked: true,
                id,
            };
            stage.creation_security = Security::read(&stage.file)?;
            Security::private(&stage.file)?;
            return Ok(stage);
        }
        Err(Error::input("cannot allocate default staging file"))
    }
    pub(crate) fn validate(&self) -> Result<()> {
        let m = self.file.metadata()?;
        if !m.is_file() || m.nlink() != 1 || self.directory.leaf_identity(&self.name)? != self.id {
            return Err(conflict());
        }
        Ok(())
    }
    /// Only the just-created private empty stage is detached. No caller path
    /// is unlinked, and failure never returns a linked descriptor as private.
    pub(crate) fn detach(mut self) -> Result<File> {
        self.validate()?;
        let file = self.file.try_clone()?;
        self.unlink();
        if self.linked || file.metadata()?.nlink() != 0 {
            return Err(Error::input("cannot unlink private transfer stage"));
        }
        Ok(file)
    }
    /// Caller holds the stable advisory lock and has revalidated input/target.
    /// A missing target uses linkat, never a clobbering rename.
    pub(crate) fn commit(&mut self, name: &CStr, replace: bool) -> Result<()> {
        // SAFETY: live directory fd and validated single-component CStrings.
        let rc = unsafe {
            if replace {
                libc::renameat(
                    self.directory.file.as_raw_fd(),
                    self.name.as_ptr(),
                    self.directory.file.as_raw_fd(),
                    name.as_ptr(),
                )
            } else {
                libc::linkat(
                    self.directory.file.as_raw_fd(),
                    self.name.as_ptr(),
                    self.directory.file.as_raw_fd(),
                    name.as_ptr(),
                    0,
                )
            }
        };
        if rc != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        if replace {
            self.linked = false;
        } else {
            self.unlink();
        }
        Ok(())
    }
    fn unlink(&mut self) {
        if self.linked {
            // Only remove our own entry; a replacement is not ours to delete.
            if self.directory.leaf_identity(&self.name).ok() == Some(self.id) {
                // SAFETY: fixed leaf in the same held directory; no recursion.
                if unsafe { libc::unlinkat(self.directory.file.as_raw_fd(), self.name.as_ptr(), 0) }
                    == 0
                {
                    self.linked = false;
                }
            }
        }
    }
}
impl Drop for Stage {
    fn drop(&mut self) {
        self.unlink();
    }
}

#[cfg(test)]
mod tests;
