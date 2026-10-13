//! The analysis caches of a DRC pack (feature/jobdeck-drc's `.analysis`):
//! a rule's measurement table, its delta groups and its exact centres,
//! derived from the pack alone - never from review files - and published
//! whole (a staging folder renamed into place) or not at all.
//!
//! The folder is `.<db>.analysis` beside the results database, or
//! `FLOE_DRC_ANALYSIS_ROOT/<hash of the pack path>` (a local SSD for a
//! read-only results folder). A pack that changes - rebuilt, replaced,
//! touched - changes its identity, so a stale cache is never read.
//! Without a writable folder everything is computed in memory instead.
use crate::{check_cancelled, Error, Result};
use sha1::{Digest, Sha1};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::marker::PhantomData;
use std::os::unix::fs::{FileExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(target_endian = "big")]
compile_error!("the DRC analysis caches are little-endian");

/// The analysis folder of `pack` (see the module docs).
pub fn analysis_dir(pack: &Path) -> Result<PathBuf> {
    let pack = crate::cache::absolute(pack)?;
    if let Some(root) = std::env::var_os("FLOE_DRC_ANALYSIS_ROOT").filter(|r| !r.is_empty()) {
        let root = PathBuf::from(root);
        let root = if let Ok(rest) = root.strip_prefix("~") {
            std::env::var_os("HOME").map_or(root.clone(), |h| PathBuf::from(h).join(rest))
        } else {
            root
        };
        let key = hex(&Sha1::digest(pack.as_os_str().as_encoded_bytes()))[..20].to_string();
        return Ok(crate::cache::absolute(&root)?.join(key));
    }
    let db = crate::cache::database_path(&pack)?;
    let name = db
        .file_name()
        .ok_or_else(|| Error::input("DRC database must name a file"))?;
    let mut hidden = std::ffi::OsString::from(".");
    hidden.push(name);
    hidden.push(".analysis");
    Ok(db.with_file_name(hidden))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A digest naming a cache entry by its key.
pub fn digest(key: &serde_json::Value) -> String {
    hex(&Sha1::digest(key.to_string().as_bytes()))
}

/// The pack's identity: its file stamp and its head and tail bytes - a
/// constant-size fingerprint, changed by any rebuild or replacement.
pub fn identity(pack: &Path) -> Result<String> {
    let mut f = File::open(pack)?;
    let m = f.metadata()?;
    let mut head = vec![0u8; 40.min(m.len() as usize)];
    f.read_exact(&mut head)?;
    let tail_len = 144.min(m.len());
    let mut tail = vec![0u8; tail_len as usize];
    f.seek(SeekFrom::Start(m.len() - tail_len))?;
    f.read_exact(&mut tail)?;
    let mut h = Sha1::new();
    h.update(
        format!(
            "floe-drc-analysis-r1:{}:{}.{}:{}.{}:{}:{}:",
            m.len(),
            m.mtime(),
            m.mtime_nsec(),
            m.ctime(),
            m.ctime_nsec(),
            m.dev(),
            m.ino()
        )
        .as_bytes(),
    );
    h.update(&head);
    h.update(&tail);
    Ok(hex(&h.finalize()))
}

/// Plain little-endian numbers a column can hold.
pub trait Pod: Copy + Default + Send + Sync + 'static {}
impl Pod for u8 {}
impl Pod for i32 {}
impl Pod for u32 {}
impl Pod for i64 {}
impl Pod for u64 {}

/// An array of numbers: in memory, or a published cache file mapped.
pub enum Column<T: Pod> {
    Owned(Vec<T>),
    Mapped(memmap2::Mmap, usize, PhantomData<T>),
}

impl<T: Pod> Column<T> {
    pub fn as_slice(&self) -> &[T] {
        match self {
            Self::Owned(v) => v,
            Self::Mapped(m, n, _) => {
                if *n == 0 {
                    return &[];
                }
                // SAFETY: the map is page-aligned, holds exactly n values of
                // a plain-old-data type (checked when mapped) and is
                // read-only for its lifetime; cache files are published by
                // rename and never written in place.
                unsafe { std::slice::from_raw_parts(m.as_ptr().cast::<T>(), *n) }
            }
        }
    }
    pub fn len(&self) -> usize {
        self.as_slice().len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Map `path`, which must hold exactly `n` values.
    pub fn map(path: &Path, n: usize) -> Result<Self> {
        let f = File::open(path)?;
        let want = n
            .checked_mul(std::mem::size_of::<T>())
            .ok_or_else(|| Error::input("cache array too large"))?;
        if f.metadata()?.len() != want as u64 {
            return Err(Error::new(crate::ErrorKind::Cache, "cache array size"));
        }
        if n == 0 {
            return Ok(Self::Owned(Vec::new()));
        }
        // SAFETY: a published cache file, replaced only by rename.
        let m = unsafe { memmap2::Mmap::map(&f) }?;
        if m.as_ptr().align_offset(std::mem::align_of::<T>()) != 0 {
            return Err(Error::new(crate::ErrorKind::Cache, "cache map alignment"));
        }
        Ok(Self::Mapped(m, n, PhantomData))
    }
}

/// The bytes of a slice of numbers (little-endian host).
pub fn bytes_of<T: Pod>(values: &[T]) -> &[u8] {
    // SAFETY: plain-old-data values viewed as their bytes.
    unsafe {
        std::slice::from_raw_parts(values.as_ptr().cast::<u8>(), std::mem::size_of_val(values))
    }
}

/// Write `values` to `path` (created).
pub fn write_column<T: Pod>(path: &Path, values: &[T]) -> Result<()> {
    let mut f = std::io::BufWriter::new(File::create(path)?);
    for chunk in values.chunks(1 << 20) {
        f.write_all(bytes_of(chunk))?;
    }
    f.into_inner().map_err(|e| e.into_error())?.sync_all()?;
    Ok(())
}

/// A file of `n` numbers written by parts from several threads.
pub struct Sink<T: Pod> {
    file: File,
    _t: PhantomData<T>,
}
impl<T: Pod> Sink<T> {
    pub fn create(path: &Path, n: u64) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        file.set_len(n * std::mem::size_of::<T>() as u64)?;
        Ok(Self {
            file,
            _t: PhantomData,
        })
    }
    pub fn put(&self, at: u64, values: &[T]) -> Result<()> {
        self.file
            .write_all_at(bytes_of(values), at * std::mem::size_of::<T>() as u64)?;
        Ok(())
    }
    pub fn finish(self) -> Result<()> {
        self.file.sync_all()?;
        Ok(())
    }
}

static STAGING: AtomicUsize = AtomicUsize::new(0);

/// A cache entry being built: a staging folder beside its final name,
/// renamed into place by `publish`, removed when dropped unpublished.
pub struct Staging {
    pub dir: PathBuf,
    target: PathBuf,
    done: bool,
}
impl Staging {
    pub fn new(target: &Path) -> Result<Self> {
        let parent = target
            .parent()
            .ok_or_else(|| Error::input("cache entry needs a folder"))?;
        fs::create_dir_all(parent)?;
        let name = target.file_name().unwrap_or_default().to_string_lossy();
        // leftovers of a builder that died are reaped by name (its pid)
        if let Ok(entries) = fs::read_dir(parent) {
            for e in entries.flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                if let Some(rest) = n.strip_prefix(&format!(".{name}.build-")) {
                    let pid = rest.split('-').next().and_then(|p| p.parse::<i32>().ok());
                    // SAFETY: kill(pid, 0) only probes whether the process exists.
                    if pid.is_some_and(|p| unsafe { libc::kill(p, 0) } != 0) {
                        let _ = fs::remove_dir_all(e.path());
                    }
                }
            }
        }
        let dir = parent.join(format!(
            ".{name}.build-{}-{}",
            std::process::id(),
            STAGING.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir(&dir)?;
        Ok(Self {
            dir,
            target: target.to_path_buf(),
            done: false,
        })
    }
    /// Rename into place; a complete entry another builder published first
    /// is kept and ours dropped.
    pub fn publish(mut self) -> Result<()> {
        match fs::rename(&self.dir, &self.target) {
            Ok(()) => {
                self.done = true;
                Ok(())
            }
            Err(e) => {
                if self.target.join("complete.json").is_file() {
                    Ok(())
                } else {
                    Err(e.into())
                }
            }
        }
    }
}
impl Drop for Staging {
    fn drop(&mut self) {
        if !self.done {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }
}

/// Builders of one entry take turns (`<entry>.lock`); the lock goes with
/// the process.
pub struct BuildLock(#[allow(dead_code)] File);
impl BuildLock {
    pub fn take(target: &Path, stop: &AtomicUsize) -> Result<Self> {
        let mut path = target.as_os_str().to_owned();
        path.push(".lock");
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let f = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(PathBuf::from(path))?;
        loop {
            check_cancelled(stop)?;
            match f.try_lock() {
                Ok(()) => return Ok(Self(f)),
                Err(std::fs::TryLockError::WouldBlock) => {
                    std::thread::sleep(std::time::Duration::from_millis(25))
                }
                Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
            }
        }
    }
}

/// The manifest of a complete entry, when it says `key`.
pub fn manifest(dir: &Path, key: &serde_json::Value) -> Option<serde_json::Value> {
    let text = fs::read(dir.join("complete.json")).ok()?;
    let m: serde_json::Value = serde_json::from_slice(&text).ok()?;
    (m.get("key") == Some(key)).then_some(m)
}

pub fn write_manifest(dir: &Path, value: &serde_json::Value) -> Result<()> {
    let tmp = dir.join("complete.json.tmp");
    fs::write(&tmp, value.to_string())?;
    fs::rename(&tmp, dir.join("complete.json"))?;
    Ok(())
}
