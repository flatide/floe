//! Explicit local review publication. No ambient reviewer lookup, temp fallback,
//! in-pack pwrite, automatic stale-file migration or web authority is provided.
//! Snapshots are opaque expected revisions; a draft consumes one and publishes
//! only after pack, directory, sidecar and permissions are revalidated.
use super::{rewrite_waives, ImportReport, Layout, Notes, WaiveStats, EDIT_ITEMS, SIDECAR_BYTES};
use crate::{
    artifact, check_cancelled,
    drc::{waive_paths, Pack},
    layer_defaults::{identity, leaf, reject_aliases, security::Security, Directory, Stage, Stamp},
    registered::{AccessScope, RegisteredSource},
    Error, ErrorKind, Result,
};
use sha1::{Digest, Sha1};
use std::{
    ffi::CString,
    fs::{self, File},
    io::{Read, Seek, Write},
    os::{
        fd::AsRawFd,
        unix::{ffi::OsStrExt, fs::MetadataExt},
    },
    path::{Path, PathBuf},
    sync::{atomic::AtomicUsize, Arc, Mutex},
    time::{Duration, Instant, SystemTime},
};

const TTL: Duration = Duration::from_secs(120);
#[cfg(target_os = "macos")]
const BINDING: &std::ffi::CStr = c"com.floe.review-pack-v1";
#[cfg(not(target_os = "macos"))]
const BINDING: &std::ffi::CStr = c"user.floe.review-pack-v1";
fn conflict() -> Error {
    Error::new(ErrorKind::Busy, "review changed; read and prepare again")
}
fn context<T>(phase: &str, result: Result<T>) -> Result<T> {
    result.map_err(|mut e| {
        e.message = format!("review {phase}: {}", e.message);
        e
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Waives,
    Notes,
}
/// Fixed adjacent names, without opening a pack or discovering a reviewer.
/// Used by a trusted gateway to protect future targets before enabling writes.
pub fn paths(pack: &Path, reviewer: &str, kind: Kind) -> Result<[PathBuf; 2]> {
    let waiver = waive_paths(pack, reviewer)?[0].clone();
    let target = match kind {
        Kind::Waives => waiver,
        Kind::Notes => {
            let pack = crate::cache::absolute(pack)?;
            let name = pack
                .file_name()
                .and_then(|s| s.to_str())
                .ok_or_else(|| Error::input("review name must be UTF-8"))?;
            pack.parent().unwrap().join(format!(
                ".{}.notes.{reviewer}.fe",
                name.strip_suffix(".ice").unwrap_or(name)
            ))
        }
    };
    let mut lock = target.as_os_str().to_owned();
    lock.push(".lock");
    Ok([target, lock.into()])
}
/// A local capability for exactly one pack/reviewer/kind. Reviewer is a trusted
/// caller-selected tag, not authentication. A future server must authorize it
/// before constructing this object; request text cannot choose an output path.
pub struct Store {
    scope: Arc<AccessScope>,
    pack: Mutex<Pack>,
    pack_path: PathBuf,
    layout: Layout,
    binding: Vec<u8>,
    kind: Kind,
    target: PathBuf,
    directory: Arc<Directory>,
    name: CString,
    lock_name: CString,
    protected_files: Vec<PathBuf>,
    protected_trees: Vec<PathBuf>,
    sources: Vec<Arc<RegisteredSource>>,
}
impl Store {
    /// Read-only registration. All other registered inputs/cache/private paths
    /// must be supplied by the trusted caller as additional protection.
    pub fn open(
        scope: Arc<AccessScope>,
        pack_path: &Path,
        reviewer: &str,
        kind: Kind,
        protected_files: Vec<PathBuf>,
        protected_trees: Vec<PathBuf>,
        stop: &AtomicUsize,
    ) -> Result<Arc<Self>> {
        Self::open_guarded(
            scope,
            pack_path,
            reviewer,
            kind,
            protected_files,
            protected_trees,
            vec![],
            stop,
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub fn open_guarded(
        scope: Arc<AccessScope>,
        pack_path: &Path,
        reviewer: &str,
        kind: Kind,
        protected_files: Vec<PathBuf>,
        protected_trees: Vec<PathBuf>,
        sources: Vec<Arc<RegisteredSource>>,
        stop: &AtomicUsize,
    ) -> Result<Arc<Self>> {
        check_cancelled(stop)?;
        if protected_files.len() > 128 || protected_trees.len() > 128 || sources.len() > 32 {
            return Err(Error::input("too many protected review paths"));
        }
        let pack_path = scope.check(pack_path)?;
        let pack = Pack::open(&pack_path, stop)?;
        let layout = Layout::from_pack(&pack)?;
        let binding = pack.review_binding()?;
        let [target, _] = paths(&pack_path, reviewer, kind)?;
        scope.check(&target)?;
        let directory = Directory::open(target.parent().unwrap())?;
        let name = leaf(target.file_name().unwrap())?;
        let target = directory.path.join(target.file_name().unwrap());
        let lock_name = leaf(std::ffi::OsStr::from_bytes(
            &[name.as_bytes(), b".lock"].concat(),
        ))?;
        let mut files = protected_files;
        files.push(pack_path.clone());
        let store = Arc::new(Self {
            scope,
            pack: Mutex::new(pack),
            pack_path,
            layout,
            binding,
            kind,
            target,
            directory,
            name,
            lock_name,
            protected_files: files,
            protected_trees,
            sources,
        });
        store.validate(stop)?;
        store.protect(&store.lock_path())?;
        Ok(store)
    }
    pub fn target(&self) -> &Path {
        &self.target
    }
    pub fn kind(&self) -> Kind {
        self.kind
    }
    pub fn identity(&self) -> super::Identity {
        super::Identity(self.binding.clone())
    }
    fn lock_path(&self) -> PathBuf {
        self.directory
            .path
            .join(std::ffi::OsStr::from_bytes(self.lock_name.as_bytes()))
    }
    fn protect(&self, path: &Path) -> Result<()> {
        self.scope.check(path)?;
        artifact::protected_output(path, &self.protected_files, &self.protected_trees)?;
        reject_aliases(path, &self.protected_files)?;
        for source in &self.sources {
            source.protect_output(path)?;
        }
        Ok(())
    }
    fn validate(&self, stop: &AtomicUsize) -> Result<()> {
        check_cancelled(stop)?;
        self.scope.check(&self.pack_path)?;
        self.directory.validate()?;
        self.pack.lock().unwrap().unchanged_at()?;
        self.protect(&self.target)
    }
    fn max_bytes(&self) -> u64 {
        match self.kind {
            Kind::Waives => {
                40 + self.layout.fingerprint().total + self.layout.counts.len() as u64 * 4
            }
            Kind::Notes => SIDECAR_BYTES as u64,
        }
    }
    /// Missing binding attributes are explicitly marked as legacy/unverified.
    /// A mismatching recorded binding is an error even with equal legacy headers.
    pub fn snapshot(self: &Arc<Self>, stop: &AtomicUsize) -> Result<Snapshot> {
        self.validate(stop)?;
        let before = Capture::read(self, false, stop)?;
        let mut notes = None;
        let mut report = ImportReport::default();
        let mut waives = None;
        match self.kind {
            Kind::Notes => {
                notes = Some(if let Some(c) = &before {
                    let mut text = String::new();
                    let mut input = &c.file;
                    input.rewind()?;
                    input
                        .take(SIDECAR_BYTES as u64 + 1)
                        .read_to_string(&mut text)?;
                    let (value, imported) = Notes::parse(&text, self.layout.fingerprint(), stop)?;
                    report = imported;
                    value
                } else {
                    Notes::new(self.layout.fingerprint())
                });
            }
            Kind::Waives => {
                waives = Some(if let Some(c) = &before {
                    let mut input = &c.file;
                    input.rewind()?;
                    rewrite_waives(input, std::io::sink(), &self.layout, &[], stop)?
                } else {
                    let pack = self.pack.lock().unwrap();
                    let seed = pack.review_seed()?;
                    rewrite_waives(seed, std::io::sink(), &self.layout, &[], stop)?
                });
            }
        }
        if let Some(c) = &before {
            c.unchanged()?;
        }
        self.validate(stop)?;
        Ok(Snapshot {
            store: Arc::clone(self),
            before,
            notes,
            report,
            waives,
        })
    }
}
struct Capture {
    file: File,
    stamp: Stamp,
    security: Security,
    // Bounded-memory change detection, not a signature or authorization token.
    digest: [u8; 20],
}
impl Capture {
    fn read(store: &Store, writable: bool, stop: &AtomicUsize) -> Result<Option<Self>> {
        check_cancelled(stop)?;
        let mut file = match store.directory.open_leaf(
            &store.name,
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
        let meta = file.metadata()?;
        if !meta.is_file()
            || meta.nlink() != 1
            || meta.len() > store.max_bytes()
            || store.kind == Kind::Waives && meta.len() != store.max_bytes()
        {
            return Err(Error::input(
                "review must be a bounded single-link regular file with valid length",
            ));
        }
        let stamp = Stamp::of(&meta);
        let security = Security::read(&file)?;
        if security
            .attribute(BINDING)
            .is_some_and(|v| v != store.binding)
        {
            return Err(Error::new(
                ErrorKind::Cache,
                "review belongs to another pack identity; explicit import is required",
            ));
        }
        let mut hash = Sha1::new();
        let mut bytes = [0; 64 * 1024];
        let mut len = 0u64;
        loop {
            check_cancelled(stop)?;
            let n = file.read(&mut bytes)?;
            if n == 0 {
                break;
            }
            len = len.checked_add(n as u64).ok_or_else(conflict)?;
            if len > store.max_bytes() {
                return Err(conflict());
            }
            hash.update(&bytes[..n]);
        }
        if len != meta.len()
            || Stamp::of(&file.metadata()?) != stamp
            || Security::read(&file)? != security
        {
            return Err(conflict());
        }
        check_cancelled(stop)?;
        Ok(Some(Self {
            file,
            stamp,
            security,
            digest: hash.finalize().into(),
        }))
    }
    fn unchanged(&self) -> Result<()> {
        if Stamp::of(&self.file.metadata()?) != self.stamp
            || Security::read(&self.file)? != self.security
        {
            return Err(conflict());
        }
        Ok(())
    }
    fn same(&self, other: &Self) -> bool {
        self.stamp == other.stamp && self.security == other.security && self.digest == other.digest
    }
}
/// Non-serializable expected revision, owned by one registered Store. Do not
/// expose it as filesystem metadata on a wire; a server maps its own revision.
pub struct Snapshot {
    store: Arc<Store>,
    before: Option<Capture>,
    notes: Option<Notes>,
    report: ImportReport,
    waives: Option<WaiveStats>,
}
impl Snapshot {
    /// Reads this expected snapshot, never the latest sidecar silently. Missing
    /// sidecars use embedded pack bytes. Full expected-version validation still
    /// hashes the sidecar; only the selected-status allocation/reads are bounded.
    pub fn selected_statuses(&self, gids: &[u64], stop: &AtomicUsize) -> Result<Vec<u8>> {
        if self.store.kind != Kind::Waives {
            return Err(Error::input("not a waive store"));
        }
        if gids.len() > EDIT_ITEMS {
            return Err(super::bounded("selected status count"));
        }
        if gids
            .iter()
            .any(|&gid| gid >= self.store.layout.fingerprint().total)
        {
            return Err(Error::input("review error index out of range"));
        }
        self.current(false, stop)?;
        let result = match &self.before {
            Some(c) => super::selected_statuses(
                &c.file,
                40,
                self.store.layout.fingerprint().total,
                gids,
                stop,
            )?,
            None => self
                .store
                .pack
                .lock()
                .unwrap()
                .review_statuses(gids, stop)?,
        };
        self.current(false, stop)?;
        Ok(result)
    }
    pub fn legacy_unverified(&self) -> bool {
        self.before
            .as_ref()
            .is_some_and(|c| c.security.attribute(BINDING).is_none())
    }
    pub fn notes(&self) -> Option<&Notes> {
        self.notes.as_ref()
    }
    pub fn import_report(&self) -> &ImportReport {
        &self.report
    }
    pub fn waives(&self) -> Option<&WaiveStats> {
        self.waives.as_ref()
    }
    pub fn exists(&self) -> bool {
        self.before.is_some()
    }
    fn current(&self, writable: bool, stop: &AtomicUsize) -> Result<()> {
        self.store.validate(stop)?;
        let now = Capture::read(&self.store, writable, stop)?;
        if !match (&self.before, &now) {
            (None, None) => true,
            (Some(a), Some(b)) => a.same(b),
            _ => false,
        } {
            return Err(conflict());
        }
        Ok(())
    }
    pub fn prepare_waives(self, changes: &[(u64, u8)], stop: &AtomicUsize) -> Result<Draft> {
        if self.store.kind != Kind::Waives
            || changes.len() > EDIT_ITEMS
            || changes
                .iter()
                .any(|(gid, _)| *gid >= self.store.layout.fingerprint().total)
        {
            return Err(Error::input("invalid waive edit kind/count/ID"));
        }
        self.current(false, stop)?;
        Ok(Draft {
            snapshot: self,
            change: Change::Waives(changes.to_vec()),
            expires: Instant::now() + TTL,
            accept_legacy: false,
        })
    }
    pub fn prepare_note(mut self, gids: &[u64], text: &str, stop: &AtomicUsize) -> Result<Draft> {
        self.notes
            .as_mut()
            .ok_or_else(|| Error::input("not a note store"))?
            .set(gids, text, stop)?;
        self.note_draft(stop)
    }
    /// An explicit import replaces the model, not a merge. Caller must show
    /// the report and obtain approval before consuming the returned draft.
    pub fn prepare_notes_import(
        mut self,
        text: &str,
        stop: &AtomicUsize,
    ) -> Result<(Draft, ImportReport)> {
        if self.store.kind != Kind::Notes {
            return Err(Error::input("not a note store"));
        }
        let (notes, report) = Notes::parse(text, self.store.layout.fingerprint(), stop)?;
        self.notes = Some(notes);
        Ok((self.note_draft(stop)?, report))
    }
    fn note_draft(self, stop: &AtomicUsize) -> Result<Draft> {
        self.current(false, stop)?;
        let text = {
            let mut pack = self.store.pack.lock().unwrap();
            let text = self.notes.as_ref().unwrap().serialize(
                |gid| {
                    let ci = pack.checks.partition_point(|c| c.start + c.count <= gid);
                    let c = pack
                        .checks
                        .get(ci)
                        .filter(|c| gid >= c.start)
                        .ok_or_else(|| Error::input("note error index out of range"))?;
                    let local = gid - c.start;
                    let record = pack.error_info(ci, local, stop)?;
                    let b = pack.bbox_um(record.bbox)?;
                    Ok([(b[0] + b[2]) / 2., (b[1] + b[3]) / 2.])
                },
                stop,
            )?;
            pack.unchanged_at()?;
            // An empty clear is a valid, fingerprinted tombstone, not a delete.
            // Legacy readers load it as no notes; expected-version checks survive.
            text.unwrap_or_else(|| {
                format!(
                    "# flateyes annotations\nfloe_pack={}\nppu=1\nunit=um\n",
                    self.store.layout.fingerprint().tag()
                )
            })
        };
        self.current(false, stop)?;
        Ok(Draft {
            snapshot: self,
            change: Change::Notes(text.into_bytes()),
            expires: Instant::now() + TTL,
            accept_legacy: false,
        })
    }
}
enum Change {
    Waives(Vec<(u64, u8)>),
    Notes(Vec<u8>),
}
pub struct Draft {
    snapshot: Snapshot,
    change: Change,
    expires: Instant,
    accept_legacy: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Published {
    /// Commit succeeded. False is a durability warning, never a retry request.
    pub directory_synced: bool,
}
impl Draft {
    /// Trusted caller has separately confirmed that the unbound legacy file
    /// belongs to this run. This never overrides a mismatching recorded binding.
    pub fn accept_legacy_run(mut self) -> Self {
        self.accept_legacy = true;
        self
    }
    pub fn target(&self) -> &Path {
        self.snapshot.store.target()
    }
    pub fn kind(&self) -> Kind {
        self.snapshot.store.kind
    }
    pub fn publish(self, stop: &AtomicUsize) -> Result<Published> {
        self.publish_using(stop, || Ok(()), File::sync_all)
    }
    fn publish_using(
        self,
        stop: &AtomicUsize,
        before_commit: impl FnOnce() -> Result<()>,
        sync: impl FnOnce(&File) -> std::io::Result<()>,
    ) -> Result<Published> {
        if Instant::now() >= self.expires {
            return Err(conflict());
        }
        if self.snapshot.legacy_unverified() && !self.accept_legacy {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "legacy review run is unverified; confirm before adopting it",
            ));
        }
        context("pre-lock check", self.snapshot.current(false, stop))?;
        let s = &self.snapshot.store;
        context("lock protection", s.protect(&s.lock_path()))?;
        let lock = context(
            "open lock",
            s.directory
                .open_lock(&s.lock_name, 0o600)
                .map_err(Error::from),
        )?;
        let lm = context("lock metadata", lock.metadata().map_err(Error::from))?;
        if !lm.is_file() || lm.nlink() != 1 || lm.len() != 0 {
            return Err(Error::input("invalid review lock file"));
        }
        // SAFETY: live regular file descriptor, nonblocking advisory lock.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let e = std::io::Error::last_os_error();
            return Err(if e.kind() == std::io::ErrorKind::WouldBlock {
                conflict()
            } else {
                Error::new(ErrorKind::Io, format!("review flock: {e}"))
            });
        }
        context("locked check", self.snapshot.current(true, stop))?;
        let mut staged = context(
            "staging",
            Stage::create_for(Arc::clone(&s.directory), "review", |p| s.protect(p)),
        )?;
        match &self.change {
            Change::Notes(text) => {
                for chunk in text.chunks(64 * 1024) {
                    check_cancelled(stop)?;
                    staged.file.write_all(chunk)?;
                }
            }
            Change::Waives(edits) => {
                if let Some(c) = &self.snapshot.before {
                    let mut input = &c.file;
                    input.rewind()?;
                    rewrite_waives(input, &mut staged.file, &s.layout, edits, stop)?;
                    c.unchanged()?;
                } else {
                    let pack = s.pack.lock().unwrap();
                    rewrite_waives(
                        pack.review_seed()?,
                        &mut staged.file,
                        &s.layout,
                        edits,
                        stop,
                    )?;
                    pack.unchanged_at()?;
                }
            }
        }
        if let Some(c) = &self.snapshot.before {
            c.security.apply(&staged.file)?;
            if Security::read(&staged.file)? != c.security {
                return Err(Error::new(
                    ErrorKind::Unsupported,
                    "cannot preserve review file attributes",
                ));
            }
        } else {
            Security::private(&staged.file)?;
        }
        // Bytes remain interoperable with GTK. The owned xattr atomically
        // travels with the replacement inode, unlike a second manifest file.
        context(
            "write binding",
            crate::layer_defaults::security::set(&staged.file, BINDING, &s.binding),
        )?;
        if Security::read(&staged.file)?.attribute(BINDING) != Some(s.binding.as_slice()) {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "cannot preserve review pack binding",
            ));
        }
        staged
            .file
            .set_times(fs::FileTimes::new().set_modified(SystemTime::now()))?;
        staged.file.sync_all()?;
        before_commit()?;
        if Instant::now() >= self.expires {
            return Err(conflict());
        }
        context("commit check", self.snapshot.current(true, stop))?;
        s.protect(&s.lock_path())?;
        if context(
            "lock identity",
            s.directory.leaf_identity(&s.lock_name).map_err(Error::from),
        )? != identity(&lm)
        {
            return Err(conflict());
        }
        context("stage identity", staged.validate())?;
        check_cancelled(stop)?;
        // Cooperating writers hold this stable inode through recheck+commit.
        // Never unlink the lock: different lock inodes do not exclude each other.
        // This is not a filesystem CAS against noncooperating writers (incl. GTK).
        context(
            "commit",
            staged.commit(&s.name, self.snapshot.before.is_some()),
        )?;
        // The commit wins over any later cancellation or directory-sync error.
        Ok(Published {
            directory_synced: sync(&s.directory.file).is_ok(),
        })
    }
}

#[cfg(test)]
mod tests;
