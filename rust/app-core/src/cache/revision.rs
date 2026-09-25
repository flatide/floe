//! Opt-in immutable cache generations. No legacy name resolution changes here.
//! Only a completed, validated private build may advance current.json. Readers
//! pin the revision directory, never reopen individual files through current.
//! Retired and failed candidates are retained: reclamation is a separate policy.
use super::{absolute, default_cache_path, utf8, validated_vfs, CacheState};
use crate::{check_cancelled, index::WriteLease, Error, ErrorKind, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::atomic::AtomicUsize,
};

const FILES: &[&str] = &[
    "meta.json",
    "design.ovm",
    "design.ovp",
    "design.ovt",
    "design.ovo",
    "design.ovr",
];
const MAX_RECORD: u64 = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stamp {
    dev: u64,
    ino: u64,
    len: u64,
    mtime: i64,
    mtime_ns: i64,
    ctime: i64,
    ctime_ns: i64,
}
impl Stamp {
    fn of(m: &fs::Metadata) -> Self {
        Self {
            dev: m.dev(),
            ino: m.ino(),
            len: m.len(),
            mtime: m.mtime(),
            mtime_ns: m.mtime_nsec(),
            ctime: m.ctime(),
            ctime_ns: m.ctime_nsec(),
        }
    }
    fn source(path: &Path) -> Result<Self> {
        let m = fs::metadata(path)?;
        if !m.is_file() {
            return Err(Error::input("revision source must be a regular file"));
        }
        Ok(Self::of(&m))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    source: PathBuf,
    revision: String,
    source_stamp: Stamp,
    files: BTreeMap<String, Stamp>,
}
impl Record {
    fn validate(&self, source: &Path) -> Result<()> {
        if self.version != 1
            || self.source != source
            || self.revision.len() != 32
            || !self
                .revision
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || self.files.keys().any(|k| !FILES.contains(&k.as_str()))
            || ["meta.json", "design.ovm", "design.ovp"]
                .iter()
                .any(|k| !self.files.contains_key(*k))
        {
            return Err(invalid("invalid revision manifest"));
        }
        Ok(())
    }
}

/// Source-bound derived store. Construction and pinning never create files.
#[derive(Clone, Debug)]
pub struct Store {
    source: PathBuf,
    root: PathBuf,
}
impl Store {
    pub fn new(source: &Path) -> Result<Self> {
        let source = absolute(source)?;
        let mut root = default_cache_path(&source)?.into_os_string();
        root.push(".revisions");
        Ok(Self {
            source,
            root: root.into(),
        })
    }
    pub fn path(&self) -> &Path {
        &self.root
    }
    pub fn pin(&self) -> Result<Option<Snapshot>> {
        match fs::symlink_metadata(&self.root) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
            Ok(_) => (),
        }
        let root = directory(&self.root)?;
        let Some(bytes) = current(&self.root)? else {
            return Ok(None);
        };
        let record: Record =
            serde_json::from_slice(&bytes).map_err(|_| invalid("unreadable revision manifest"))?;
        record.validate(&self.source)?;
        let path = self.root.join(&record.revision);
        let dir = directory(&path)?;
        if read_record(&path.join("revision.json"))? != bytes {
            return Err(invalid("revision seal and current manifest disagree"));
        }
        let snapshot = Snapshot {
            store: self.clone(),
            record,
            root_id: id(&root)?,
            dir_id: id(&dir)?,
        };
        snapshot.validate()?;
        Ok(Some(snapshot))
    }
    pub(crate) fn begin(&self, stop: &AtomicUsize) -> Result<Candidate> {
        check_cancelled(stop)?;
        let source_stamp = Stamp::source(&self.source)?;
        // This lock is separate from mutable legacy caches, which are neither
        // read nor rewritten by this full-build lane. All store writers use it.
        let lease = WriteLease::acquire(&self.root)?;
        match fs::DirBuilder::new().mode(0o700).create(&self.root) {
            Ok(()) => {
                File::open(self.root.parent().expect("absolute store"))?.sync_all()?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(e.into()),
        }
        let root = directory(&self.root)?;
        let before = current(&self.root)?;
        // Corrupt current is not silently overwritten, even by another build.
        if before.is_some() {
            self.pin()?;
        }
        check_cancelled(stop)?;
        let mut random = [0; 16];
        getrandom::fill(&mut random)
            .map_err(|_| Error::new(ErrorKind::Io, "revision entropy unavailable"))?;
        let revision = random
            .iter()
            .map(|v| format!("{v:02x}"))
            .collect::<String>();
        let path = self.root.join(&revision);
        fs::DirBuilder::new().mode(0o700).create(&path)?;
        let dir = directory(&path)?;
        root.sync_all()?;
        Ok(Candidate {
            store: self.clone(),
            revision,
            source_stamp,
            before,
            root_id: id(&root)?,
            dir_id: id(&dir)?,
            _lease: lease,
        })
    }
}

/// An immutable path plus its sealed file set. Clones keep the same revision,
/// even after another publisher advances current. No automatic garbage collector
/// is enabled; dropping a pin never removes files.
#[derive(Clone, Debug)]
pub struct Snapshot {
    store: Store,
    record: Record,
    root_id: (u64, u64),
    dir_id: (u64, u64),
}
impl Snapshot {
    pub fn id(&self) -> &str {
        &self.record.revision
    }
    pub fn source(&self) -> &Path {
        &self.store.source
    }
    pub fn directory(&self) -> PathBuf {
        self.store.root.join(self.id())
    }
    pub fn validate(&self) -> Result<()> {
        let path = self.directory();
        if id(&directory(&self.store.root)?)? != self.root_id
            || id(&directory(&path)?)? != self.dir_id
            || files(&path, false)? != self.record.files
        {
            return Err(invalid("pinned cache revision changed"));
        }
        let sealed: Record = serde_json::from_slice(&read_record(&path.join("revision.json"))?)
            .map_err(|_| invalid("unreadable revision seal"))?;
        if sealed != self.record {
            return Err(invalid("pinned revision seal changed"));
        }
        Ok(())
    }
    /// Read the exact pinned generation, not whichever cache is now current.
    pub fn open_layout(&self, stop: &AtomicUsize) -> Result<crate::catalog::Layout> {
        self.validate()?;
        let mut layout =
            crate::catalog::Layout::open_directory(self.source(), self.directory(), stop)?;
        layout.source_stale |= Stamp::source(self.source())? != self.record.source_stamp;
        self.validate()?;
        Ok(layout)
    }
}

pub struct Publication {
    pub snapshot: Snapshot,
    /// false means visible commit with uncertain crash durability, not rollback.
    pub directory_synced: bool,
}

pub(crate) struct Candidate {
    store: Store,
    revision: String,
    source_stamp: Stamp,
    before: Option<Vec<u8>>,
    root_id: (u64, u64),
    dir_id: (u64, u64),
    _lease: WriteLease,
}
impl Candidate {
    pub(crate) fn directory(&self) -> PathBuf {
        self.store.root.join(&self.revision)
    }
    pub(crate) fn publish(self, stop: &AtomicUsize) -> Result<Publication> {
        check_cancelled(stop)?;
        let path = self.directory();
        if super::inspect(&self.store.source, &path)? != CacheState::Current {
            return Err(invalid("candidate is not a complete current cache"));
        }
        let stamps = files(&path, true)?;
        let vfs = validated_vfs(&path)?;
        // Additive summaries are part of the snapshot, not optional future
        // side effects. Validate their binding before publishing the file set.
        if stamps.contains_key("design.ovo") {
            floe_vfs::occupancy::OvoFile::open(utf8(&path.join("design.ovo"))?)
                .and_then(|o| o.validate_against(&vfs.ovm))
                .map_err(invalid)?;
        }
        if stamps.contains_key("design.ovr") {
            floe_vfs::representatives::File::open(utf8(&path)?, &vfs.ovm).map_err(invalid)?;
        }
        drop(vfs);
        let record = Record {
            version: 1,
            source: self.store.source.clone(),
            revision: self.revision.clone(),
            source_stamp: self.source_stamp.clone(),
            files: stamps,
        };
        record.validate(&self.store.source)?;
        let bytes = serde_json::to_vec(&record).map_err(|e| invalid(e.to_string()))?;
        create_record(&path.join("revision.json"), &bytes)?;
        directory(&path)?.sync_all()?;
        let snapshot = Snapshot {
            store: self.store.clone(),
            record,
            root_id: self.root_id,
            dir_id: self.dir_id,
        };
        snapshot.validate()?;
        let pending = self
            .store
            .root
            .join(format!(".current-{}.tmp", self.revision));
        create_record(&pending, &bytes)?;
        let root = directory(&self.store.root)?;
        root.sync_all()?;
        if id(&root)? != self.root_id
            || current(&self.store.root)? != self.before
            || Stamp::source(&self.store.source)? != self.source_stamp
        {
            return Err(invalid("source or current revision changed during build"));
        }
        check_cancelled(stop)?;
        // Once attempted, an error is not proof that the old pointer survived.
        // Preserve the complete candidate and pending manifest as evidence.
        let directory_synced = commit_pointer(
            || fs::rename(&pending, self.store.root.join("current.json")),
            || root.sync_all(),
        )?;
        Ok(Publication {
            snapshot,
            directory_synced,
        })
    }
}

fn commit_pointer(
    rename: impl FnOnce() -> std::io::Result<()>,
    sync: impl FnOnce() -> std::io::Result<()>,
) -> Result<bool> {
    rename().map_err(|_| {
        Error::new(
            ErrorKind::PublicationUnknown,
            "revision publication outcome unknown; inspect the store before retrying",
        )
    })?;
    Ok(sync().is_ok())
}

fn invalid(message: impl Into<String>) -> Error {
    Error::new(ErrorKind::Cache, message)
}
fn directory(path: &Path) -> Result<File> {
    Ok(OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_NONBLOCK)
        .open(path)?)
}
fn id(file: &File) -> Result<(u64, u64)> {
    let m = file.metadata()?;
    Ok((m.dev(), m.ino()))
}
fn regular(path: &Path, replaceable: bool) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let m = file.metadata()?;
    // A reader may already hold the old current.json when an atomic rename
    // unlinks it. Its fd still contains one complete immutable record. Only
    // the replaceable pointer allows zero links; seals/cache files do not.
    if !m.is_file() || !(m.nlink() == 1 || (replaceable && m.nlink() == 0)) {
        return Err(invalid("revision file must be regular and unshared"));
    }
    Ok(file)
}
fn files(path: &Path, sync: bool) -> Result<BTreeMap<String, Stamp>> {
    let mut out = BTreeMap::new();
    for &name in FILES {
        match regular(&path.join(name), false) {
            Ok(file) => {
                if sync {
                    file.sync_all()?;
                }
                out.insert(name.into(), Stamp::of(&file.metadata()?));
            }
            Err(_)
                if !["meta.json", "design.ovm", "design.ovp"].contains(&name)
                    && fs::symlink_metadata(path.join(name))
                        .is_err_and(|x| x.kind() == std::io::ErrorKind::NotFound) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(out)
}
fn read_record(path: &Path) -> Result<Vec<u8>> {
    record_bytes(regular(path, false)?)
}
fn record_bytes(file: File) -> Result<Vec<u8>> {
    if file.metadata()?.len() > MAX_RECORD {
        return Err(invalid("revision manifest too large"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_RECORD + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_RECORD {
        return Err(invalid("revision manifest grew"));
    }
    Ok(bytes)
}
fn current(root: &Path) -> Result<Option<Vec<u8>>> {
    let path = root.join("current.json");
    match fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
        Ok(_) => record_bytes(regular(&path, true)?).map(Some),
    }
}
fn create_record(path: &Path, bytes: &[u8]) -> Result<()> {
    if bytes.len() as u64 > MAX_RECORD {
        return Err(invalid("revision manifest too large"));
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests;
