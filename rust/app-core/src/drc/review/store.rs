//! Explicit local review publication. Legacy temporary files are read-only and
//! selected by a trusted caller; no ambient reviewer, pwrite or migration.
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
mod transfer;
pub use transfer::{ExportContents, ExportInfo};
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
            let pack = crate::cache::database_path(pack)?;
            let name = pack
                .file_name()
                .and_then(|s| s.to_str())
                .ok_or_else(|| Error::input("review name must be UTF-8"))?;
            pack.parent()
                .unwrap()
                .join(format!(".{}.notes.{reviewer}.fe", name))
        }
    };
    let mut lock = target.as_os_str().to_owned();
    lock.push(".lock");
    Ok([target, lock.into()])
}
/// The only two allowed read names. The legacy hash uses lexical abspath,
/// matching GTK, not canonicalized source/pack names. No directory listing.
pub fn read_paths(pack: &Path, reviewer: &str, kind: Kind) -> Result<[PathBuf; 2]> {
    if kind == Kind::Waives {
        return waive_paths(pack, reviewer);
    }
    let adjacent = paths(pack, reviewer, kind)?[0].clone();
    let pack = crate::cache::database_path(pack)?;
    let hash = format!("{:x}", Sha1::digest(pack.as_os_str().as_bytes()));
    let name = adjacent.file_name().unwrap().to_str().unwrap();
    let temporary = std::env::temp_dir().join(format!(
        "{}-{}.fe",
        name.strip_suffix(".fe").unwrap(),
        &hash[..12]
    ));
    Ok([adjacent, temporary])
}
#[derive(Clone)]
pub struct ReadTargets {
    pub notes: PathBuf,
    pub waives: PathBuf,
}
impl ReadTargets {
    pub fn select(pack: &Path, reviewer: &str) -> Result<Self> {
        let select = |kind| -> Result<PathBuf> {
            let paths = read_paths(pack, reviewer, kind)?;
            for path in &paths {
                match fs::symlink_metadata(path) {
                    Ok(_) => return Ok(path.clone()),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                    Err(e) => return Err(e.into()),
                }
            }
            Ok(paths[0].clone())
        };
        Ok(Self {
            notes: select(Kind::Notes)?,
            waives: select(Kind::Waives)?,
        })
    }
    pub fn validate(&self, pack: &Path, reviewer: &str) -> Result<()> {
        for (kind, target) in [(Kind::Notes, &self.notes), (Kind::Waives, &self.waives)] {
            if !read_paths(pack, reviewer, kind)?.contains(target) {
                return Err(Error::input("review read target is not a derived sidecar"));
            }
        }
        Ok(())
    }
}
/// A local capability for exactly one pack/reviewer/kind. Reviewer is a trusted
/// caller-selected tag, not authentication. A future server must authorize it
/// before constructing this object; request text cannot choose an output path.
pub struct Store {
    editable: bool,
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
    sources: Arc<crate::registered::SourceSet>,
}
impl Store {
    /// Open one guarded read-only waive projection and transfer its pack into
    /// the reader. The caller holds the reader's DRC admission for this entire
    /// operation; no second pack/model or write capability is retained.
    pub fn open_readonly_database(
        scope: Arc<AccessScope>,
        pack_path: &Path,
        reviewer: &str,
        target: &Path,
        protected_files: Vec<PathBuf>,
        stop: &AtomicUsize,
    ) -> Result<crate::drc::Database> {
        let store = Self::open_readonly_catalog(
            scope,
            pack_path,
            reviewer,
            Kind::Waives,
            protected_files,
            vec![],
            crate::registered::SourceSet::new(vec![])?,
            target,
            stop,
        )?;
        let install = store.snapshot(stop)?.waive_install(stop)?;
        let store =
            Arc::try_unwrap(store).map_err(|_| Error::input("read-only pack still borrowed"))?;
        let mut pack = store.pack.into_inner().unwrap();
        pack.install_waives(&install.identity, install.input, install.counts, stop)?;
        Ok(crate::drc::Database::packed(pack))
    }
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
        Self::open_catalog(
            scope,
            pack_path,
            reviewer,
            kind,
            protected_files,
            protected_trees,
            crate::registered::SourceSet::new(sources)?,
            stop,
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub fn open_catalog(
        scope: Arc<AccessScope>,
        pack_path: &Path,
        reviewer: &str,
        kind: Kind,
        protected_files: Vec<PathBuf>,
        protected_trees: Vec<PathBuf>,
        sources: Arc<crate::registered::SourceSet>,
        stop: &AtomicUsize,
    ) -> Result<Arc<Self>> {
        Self::open_selected(
            scope,
            pack_path,
            reviewer,
            kind,
            protected_files,
            protected_trees,
            sources,
            None,
            stop,
        )
    }
    /// Fixed trusted read target, including a legacy temporary sidecar. This
    /// object can read snapshots but cannot prepare or publish any replacement.
    #[allow(clippy::too_many_arguments)]
    pub fn open_readonly_catalog(
        scope: Arc<AccessScope>,
        pack_path: &Path,
        reviewer: &str,
        kind: Kind,
        protected_files: Vec<PathBuf>,
        protected_trees: Vec<PathBuf>,
        sources: Arc<crate::registered::SourceSet>,
        target: &Path,
        stop: &AtomicUsize,
    ) -> Result<Arc<Self>> {
        Self::open_selected(
            scope,
            pack_path,
            reviewer,
            kind,
            protected_files,
            protected_trees,
            sources,
            Some(target),
            stop,
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn open_selected(
        scope: Arc<AccessScope>,
        pack_path: &Path,
        reviewer: &str,
        kind: Kind,
        protected_files: Vec<PathBuf>,
        protected_trees: Vec<PathBuf>,
        sources: Arc<crate::registered::SourceSet>,
        read_target: Option<&Path>,
        stop: &AtomicUsize,
    ) -> Result<Arc<Self>> {
        check_cancelled(stop)?;
        if protected_files.len() > 128 || protected_trees.len() > 128 {
            return Err(Error::input("too many protected review paths"));
        }
        let pack_path = scope.check(pack_path)?;
        let pack = Pack::open(&pack_path, stop)?;
        let layout = Layout::from_pack(&pack)?;
        let binding = pack.review_binding()?;
        let [target, _] = paths(&pack_path, reviewer, kind)?;
        let target = read_target
            .map(crate::cache::absolute)
            .transpose()?
            .unwrap_or(target);
        if read_target.is_some() {
            if !read_paths(&pack_path, reviewer, kind)?.contains(&target) {
                return Err(Error::input("review read target is not a derived sidecar"));
            }
        } else {
            scope.check(&target)?;
        }
        let directory = Directory::open(target.parent().unwrap())?;
        let name = leaf(target.file_name().unwrap())?;
        let target = directory.path.join(target.file_name().unwrap());
        let lock_name = leaf(std::ffi::OsStr::from_bytes(
            &[name.as_bytes(), b".lock"].concat(),
        ))?;
        let mut files = protected_files;
        files.push(pack_path.clone());
        let store = Arc::new(Self {
            editable: read_target.is_none(),
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
        if store.editable {
            store.protect(&store.lock_path())?;
        }
        Ok(store)
    }
    pub fn target(&self) -> &Path {
        &self.target
    }
    fn require_editor(&self) -> Result<()> {
        if self.editable {
            Ok(())
        } else {
            Err(Error::new(
                ErrorKind::Unsupported,
                "read-only review cannot prepare or publish edits",
            ))
        }
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
        if self.editable {
            self.scope.check(path)?;
            self.sources
                .protect_output(path, crate::registered::PublicationKind::Review)?;
        } else if path != self.target {
            return Err(Error::input("read-only review has no other path authority"));
        }
        artifact::protected_output(path, &self.protected_files, &self.protected_trees)?;
        reject_aliases(path, &self.protected_files)?;
        for source in self.sources.snapshot() {
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
/// Read-state installation, NOT a disk publication receipt or an HTTP revision.
#[derive(Debug, PartialEq, Eq)]
pub struct AppliedWaives {
    pub sidecar: bool,
    pub legacy_unverified: bool,
    pub waived: u64,
}
struct WaiveInstall {
    identity: super::Identity,
    input: Option<crate::drc::pack::Input>,
    counts: Vec<u32>,
    applied: AppliedWaives,
}
impl Snapshot {
    /// Match this captured file to an actual native commit, not merely the
    /// current contents of the same path. Not a wire token or an authorization.
    pub fn verify_published(&self, published: &Published) -> Result<()> {
        let proof = published.file.ok_or_else(conflict)?;
        let before = self.before.as_ref().ok_or_else(conflict)?;
        if self.store.kind != Kind::Waives
            || identity(&before.file.metadata()?) != proof.identity
            || before.digest != proof.digest
            || Sha1::digest(&self.store.binding).as_slice() != proof.binding
            || before.security.attribute(BINDING) != Some(self.store.binding.as_slice())
        {
            return Err(conflict());
        }
        before.unchanged()
    }
    /// Consume an expected snapshot to refresh one already-open reader without
    /// decoding its geometry again. Off-reactor: expected validation hashes the
    /// whole sidecar. No target/lock creation, path discovery or implicit writes.
    pub fn apply_waives(
        self,
        database: &mut crate::drc::Database,
        stop: &AtomicUsize,
    ) -> Result<AppliedWaives> {
        self.apply_waives_using(database, stop, || Ok(()))
    }
    pub(super) fn apply_waives_using(
        mut self,
        database: &mut crate::drc::Database,
        stop: &AtomicUsize,
        before_install: impl FnOnce() -> Result<()>,
    ) -> Result<AppliedWaives> {
        database.validate_waive_identity(&self.store.identity())?;
        let install = self.waive_install(stop)?;
        before_install()?;
        database.install_waives(&install.identity, install.input, install.counts, stop)?;
        Ok(install.applied)
    }
    fn waive_install(&mut self, stop: &AtomicUsize) -> Result<WaiveInstall> {
        if self.store.kind != Kind::Waives {
            return Err(Error::input("not a waive snapshot"));
        }
        check_cancelled(stop)?;
        let identity = self.store.identity();
        self.current(false, stop)?;
        let input = self
            .before
            .as_ref()
            .map(|c| crate::drc::pack::Input::from_file(c.file.try_clone()?))
            .transpose()?;
        // Keep the captured descriptor, not a fresh path lookup after checking.
        // A replacement/mutation during preparation cannot be silently adopted.
        self.current(false, stop)?;
        if let Some(c) = &self.before {
            c.unchanged()?;
        }
        let stats = self
            .waives
            .take()
            .ok_or_else(|| Error::input("missing waive snapshot counts"))?;
        let applied = AppliedWaives {
            sidecar: self.exists(),
            legacy_unverified: self.legacy_unverified(),
            waived: stats.waived,
        };
        Ok(WaiveInstall {
            identity,
            input,
            counts: stats.per_rule,
            applied,
        })
    }
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
    /// Read-only display guard for an already decoded note snapshot. Unlike a
    /// publication check, this does not rehash/reparse the sidecar. A changed
    /// directory/pack/file identity, metadata or security is an explicit error;
    /// this method never adopts external edits or grants publication authority.
    pub fn check_note_display(&self, stop: &AtomicUsize) -> Result<()> {
        if self.store.kind != Kind::Notes {
            return Err(Error::input("not a note snapshot"));
        }
        self.store.validate(stop)?;
        let current = match self
            .store
            .directory
            .open_leaf(&self.store.name, libc::O_RDONLY, 0)
        {
            Ok(file) => Some(file),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        match (&self.before, current) {
            (None, None) => (),
            (Some(before), Some(file)) => {
                before.unchanged()?;
                if Stamp::of(&file.metadata()?) != before.stamp
                    || Security::read(&file)? != before.security
                {
                    return Err(conflict());
                }
            }
            _ => return Err(conflict()),
        }
        check_cancelled(stop)
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
        self.store.require_editor()?;
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
            imported: false,
        })
    }
    pub fn prepare_note(mut self, gids: &[u64], text: &str, stop: &AtomicUsize) -> Result<Draft> {
        self.store.require_editor()?;
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
        self.store.require_editor()?;
        if self.store.kind != Kind::Notes {
            return Err(Error::input("not a note store"));
        }
        let (notes, report) = Notes::parse(text, self.store.layout.fingerprint(), stop)?;
        self.notes = Some(notes);
        let mut draft = self.note_draft(stop)?;
        // Portable FE has only the weak size/mtime/count tag, not proof of
        // this exact run. Explicit import confirmation is separate from the
        // target sidecar's (possibly already verified) binding.
        draft.imported = true;
        Ok((draft, report))
    }
    fn note_text(&self, stop: &AtomicUsize) -> Result<String> {
        let text = {
            let mut pack = self.store.pack.lock().unwrap();
            let text = self
                .notes
                .as_ref()
                .ok_or_else(|| Error::input("not a note snapshot"))?
                .serialize(
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
        Ok(text)
    }
    fn note_draft(self, stop: &AtomicUsize) -> Result<Draft> {
        self.current(false, stop)?;
        let text = self.note_text(stop)?;
        self.current(false, stop)?;
        Ok(Draft {
            snapshot: self,
            change: Change::Notes(text.into_bytes()),
            expires: Instant::now() + TTL,
            accept_legacy: false,
            imported: false,
        })
    }
}
enum Change {
    Waives(Vec<(u64, u8)>),
    WaivesImport(transfer::ImportedWaives),
    Notes(Vec<u8>),
}
pub struct Draft {
    snapshot: Snapshot,
    change: Change,
    expires: Instant,
    accept_legacy: bool,
    imported: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Published {
    /// Commit succeeded. False is a durability warning, never a retry request.
    pub directory_synced: bool,
    /// Present only for native waive publication; opaque and non-serializable.
    pub file: Option<PublishedFile>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PublishedFile {
    identity: (u64, u64),
    digest: [u8; 20],
    binding: [u8; 20],
}
impl Draft {
    pub fn note_counts(&self) -> Option<(usize, usize)> {
        self.snapshot
            .notes
            .as_ref()
            .map(|n| (n.groups().count(), n.member_count()))
    }
    pub fn legacy_unverified(&self) -> bool {
        self.imported || self.snapshot.legacy_unverified()
    }
    /// Trusted caller confirmed the portable import or unbound legacy target
    /// belongs to this run. Never overrides a mismatching target binding.
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
        self.snapshot.store.require_editor()?;
        let _registration = self.snapshot.store.sources.publication(stop)?;
        if Instant::now() >= self.expires {
            return Err(conflict());
        }
        if self.legacy_unverified() && !self.accept_legacy {
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
            Change::WaivesImport(input) => input.write(&mut staged.file, &s.layout, stop)?,
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
        // Fixed-memory extra read pass, before commit. SHA-1 is used here as
        // the existing revision change detector, not as a signature.
        let proof = if s.kind == Kind::Waives {
            staged.file.rewind()?;
            let mut hash = Sha1::new();
            let mut bytes = [0; 64 * 1024];
            loop {
                check_cancelled(stop)?;
                let n = staged.file.read(&mut bytes)?;
                if n == 0 {
                    break;
                }
                hash.update(&bytes[..n]);
            }
            Some(PublishedFile {
                identity: identity(&staged.file.metadata()?),
                digest: hash.finalize().into(),
                binding: Sha1::digest(&s.binding).into(),
            })
        } else {
            None
        };
        before_commit()?;
        if Instant::now() >= self.expires {
            return Err(conflict());
        }
        context("commit check", self.snapshot.current(true, stop))?;
        if let Change::WaivesImport(input) = &self.change {
            input.unchanged()?;
        }
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
            file: proof,
        })
    }
}

#[cfg(test)]
mod tests;
