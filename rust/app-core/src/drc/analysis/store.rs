//! The analysis caches of a DRC pack (feature/jobdeck-drc's `.analysis`):
//! a rule's measurement table, its delta groups and its exact centres,
//! derived from the pack alone - never from review files - and published
//! whole (a staging folder renamed into place) or not at all.
//!
//! The folder is `.<db>.analysis` beside the results database, or
//! `FLOE_DRC_ANALYSIS_ROOT/<hash of the pack path>` (a local SSD for a
//! read-only results folder). The caches are bound to what the pack's
//! errors are (`content_identity`, 0.12.459): a pack made again of the same
//! results, a copy of it and one whose times changed keep them, other
//! errors are another identity - so a stale cache is never read. The caches
//! of other identities are taken away by `floe2 drc-prepare`
//! (`sweep_outdated`). Without a writable folder everything is computed in
//! memory instead.
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

/// The pack FILE's identity: its file stamp and its head and tail bytes - a
/// constant-size fingerprint, changed by any rebuild or replacement. What a
/// long operation compares to know that the pack it works on is still the
/// file it opened (the review migration). The analysis caches were bound to
/// it until 0.12.458; they bind to `content_identity` now.
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

/// What the identity of the analysis caches starts with (r1 was the file
/// identity above).
const CONTENT_TAG: &[u8] = b"floe-drc-analysis-r2:";

/// The identity the analysis caches bind to: what the pack's errors ARE -
/// its content id (floe-index drcpack.rs: the precision, the cell, the
/// coordinate blob as stored, each check's name and error count), under a
/// tag of this cache's own. A pack made again of the same results, a copy
/// of it in another folder and one whose times changed keep their caches;
/// other errors are another identity. (0.12.459, user 2026-11-12: until
/// then the pack file's stamp - `identity` -, so a re-pack, a copy or a
/// touch had every cache made again and left the old ones behind. Review
/// files bind to the content id since 0.12.441.)
pub fn content_identity(pack: &Path) -> Result<String> {
    let unusable = |what: &str| {
        Error::new(
            crate::ErrorKind::Cache,
            format!("{}: not a DRC pack this build reads ({what})", pack.display()),
        )
    };
    let mut f = File::open(pack)?;
    let len = f.metadata()?.len();
    let footer = crate::drc::FOOTER_BYTES;
    if len < 40 + footer {
        return Err(unusable("truncated header/footer"));
    }
    let mut head = [0u8; 12];
    f.read_exact(&mut head)?;
    let mut foot = vec![0u8; footer as usize];
    f.seek(SeekFrom::Start(len - footer))?;
    f.read_exact(&mut foot)?;
    if &head[..8] != crate::drc::MAGIC || &foot[160..] != crate::drc::MAGIC {
        return Err(unusable("magic"));
    }
    if u32::from_le_bytes(head[8..12].try_into().unwrap()) != crate::drc::VERSION {
        return Err(unusable("layout"));
    }
    let mut h = Sha1::new();
    h.update(CONTENT_TAG);
    h.update(&foot[128..160]);
    Ok(hex(&h.finalize()))
}

/// The file in a pack's analysis folder that names the rules sidecar its
/// caches were prepared with: one line, the sidecar's absolute path.
const RULES_NOTE: &str = "rules.path";
/// A path a note holds at most.
const RULES_NOTE_BYTES: u64 = 8192;

/// Note `sidecar` as the rules sidecar of `pack`'s results - the file
/// `floe2 drc-prepare --svrf` was given - in the pack's analysis folder
/// (made if need be), whole or not at all (a temporary renamed into place).
/// The viewer's load of the database and a DRC capture look there first
/// (svrf::rules_candidates; 0.12.460, user 2026-11-12: a sidecar at none of
/// the places a database's name and its recorded deck path give was found
/// by neither, though the caches had been prepared with it). A note that
/// says this path already is left as it is - a run that prepares nothing
/// new writes nothing. The path as noted, absolute.
pub fn note_rules(pack: &Path, sidecar: &Path) -> Result<PathBuf> {
    let sidecar = crate::cache::absolute(sidecar)?;
    if noted_rules(pack).as_deref() == Some(sidecar.as_path()) {
        return Ok(sidecar);
    }
    let text = sidecar
        .to_str()
        .filter(|s| !s.contains('\n') && (s.len() as u64) < RULES_NOTE_BYTES)
        .ok_or_else(|| Error::input("a rules sidecar's path must be one line of UTF-8"))?;
    let dir = analysis_dir(pack)?;
    fs::create_dir_all(&dir)?;
    let path = dir.join(RULES_NOTE);
    let tmp = dir.join(format!("{RULES_NOTE}.tmp-{}", std::process::id()));
    let written = fs::write(&tmp, format!("{text}\n")).and_then(|()| fs::rename(&tmp, &path));
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(sidecar)
}

/// The rules sidecar noted for `pack`'s results (`note_rules`), when one
/// is: an absolute path - the file it names may be gone, as any place a
/// sidecar is looked for may be empty. A note that is not one line naming
/// an absolute path says nothing.
pub fn noted_rules(pack: &Path) -> Option<PathBuf> {
    let path = analysis_dir(pack).ok()?.join(RULES_NOTE);
    let meta = fs::symlink_metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > RULES_NOTE_BYTES {
        return None;
    }
    let text = fs::read_to_string(&path).ok()?;
    let line = text.strip_suffix('\n').unwrap_or(&text);
    (!line.is_empty() && !line.contains('\n') && Path::new(line).is_absolute()).then(|| PathBuf::from(line))
}

/// What `sweep_outdated` found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Outdated {
    /// cache entries of other identities (a rule's centres, a rule's
    /// measurement table with its groups)
    pub entries: u64,
    /// the bytes of their files
    pub bytes: u64,
}

/// A centre entry's name, `centers-r<N>-<identity>-<rule>`: its identity.
fn center_identity(name: &str) -> Option<&str> {
    let rest = name.strip_prefix("centers-r")?;
    let (revision, rest) = rest.split_once('-')?;
    let (identity, rule) = rest.split_once('-')?;
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    (digits(revision) && digits(rule) && hex_name(identity)).then_some(identity)
}

/// A digest as this store names things: 40 lower-case hex digits.
fn hex_name(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The bytes of the regular files under `path` (links not followed).
fn tree_bytes(path: &Path) -> u64 {
    let Ok(m) = fs::symlink_metadata(path) else { return 0 };
    if m.is_file() {
        return m.len();
    }
    if !m.is_dir() {
        return 0;
    }
    fs::read_dir(path)
        .map(|entries| entries.flatten().map(|e| tree_bytes(&e.path())).sum())
        .unwrap_or(0)
}

/// The entries of the analysis folder `dir` that are of another identity
/// than `identity` - caches no pack at this place reads any more: of the
/// file identities of 0.12.458 and before, of results that were replaced
/// under the same name. With `remove` they are taken away and what was
/// taken is said; without, what is there is said.
///
/// Only what is certainly this store's and certainly of another identity:
/// a real folder named as a centre entry with another identity in its name
/// (with its `.lock`, and a dead builder's staging folder of it), and a
/// `delta/<digest>` folder whose complete measurement table states another
/// pack in its key. Everything else stays: an entry that states nothing (a
/// table still being measured), files of other tools, links. The caller
/// asks first that no one has the pack open (`drc::pack_idle`) - a reader
/// of an older build maps its entries, which are of another identity here.
pub fn sweep_outdated(dir: &Path, identity: &str, remove: bool) -> Outdated {
    let mut found = Outdated::default();
    let real_dir = |p: &Path| fs::symlink_metadata(p).is_ok_and(|m| m.is_dir());
    let take = |path: &Path, entry: bool, found: &mut Outdated| {
        let bytes = tree_bytes(path);
        let gone = !remove
            || if real_dir(path) {
                fs::remove_dir_all(path).is_ok()
            } else {
                fs::remove_file(path).is_ok()
            };
        if gone {
            found.entries += u64::from(entry);
            found.bytes += bytes;
        }
    };
    let Ok(entries) = fs::read_dir(dir) else { return found };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let path = e.path();
        if let Some(id) = center_identity(&name) {
            if id != identity && real_dir(&path) {
                take(&path, true, &mut found);
            }
        } else if let Some(id) = name.strip_suffix(".lock").and_then(center_identity) {
            if id != identity && fs::symlink_metadata(&path).is_ok_and(|m| m.is_file()) {
                take(&path, false, &mut found);
            }
        } else if let Some((entry, build)) = name.strip_prefix('.').and_then(|n| n.split_once(".build-")) {
            // a staging folder (`Staging::new`) of an entry of another
            // identity, its builder gone
            let pid = build.split('-').next().and_then(|p| p.parse::<i32>().ok());
            // SAFETY: kill(pid, 0) only probes whether the process exists.
            let dead = pid.is_some_and(|p| unsafe { libc::kill(p, 0) } != 0);
            if center_identity(entry).is_some_and(|id| id != identity) && dead && real_dir(&path) {
                take(&path, false, &mut found);
            }
        }
    }
    let delta = dir.join("delta");
    if real_dir(&delta) {
        for e in fs::read_dir(&delta).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let path = e.path();
            if !hex_name(&name) || !real_dir(&path) {
                continue;
            }
            let stated = fs::read(path.join("measure").join("complete.json"))
                .ok()
                .and_then(|text| serde_json::from_slice::<serde_json::Value>(&text).ok())
                .and_then(|m| m.get("key")?.get("pack")?.as_str().map(str::to_string));
            if stated.is_some_and(|pack| pack != identity) {
                take(&path, true, &mut found);
            }
        }
    }
    found
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

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);
    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("floe-analysis-store-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// As much of a pack as an identity reads: its header's magic and layout
    /// and its footer, with `content` as the content id; `body` between.
    fn pack_bytes(content: [u8; 32], body: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(crate::drc::MAGIC);
        bytes.extend_from_slice(&crate::drc::VERSION.to_le_bytes());
        bytes.resize(40, 7);
        bytes.extend_from_slice(body);
        let mut foot = vec![3u8; crate::drc::FOOTER_BYTES as usize];
        foot[128..160].copy_from_slice(&content);
        foot[160..].copy_from_slice(crate::drc::MAGIC);
        bytes.extend_from_slice(&foot);
        bytes
    }

    #[test]
    fn the_caches_are_bound_to_what_the_errors_are() {
        let s = Scratch::new("identity");
        let (a, b, other) = (s.0.join("a.tray"), s.0.join("b.tray"), s.0.join("other.tray"));
        fs::write(&a, pack_bytes([1; 32], b"errors")).unwrap();
        // the same content id in another file - other bytes around it,
        // another length, other times
        fs::write(&b, pack_bytes([1; 32], b"the same errors, packed again")).unwrap();
        fs::write(&other, pack_bytes([2; 32], b"errors")).unwrap();
        let id = content_identity(&a).unwrap();
        assert!(hex_name(&id), "{id}");
        assert_eq!(content_identity(&b).unwrap(), id);
        assert_ne!(content_identity(&other).unwrap(), id);
        // the file's identity is another thing: of the file, not of its errors
        assert_ne!(identity(&a).unwrap(), identity(&b).unwrap());
        assert_ne!(identity(&a).unwrap(), id);
        let before = identity(&a).unwrap();
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(90);
        File::options().write(true).open(&a).unwrap().set_modified(later).unwrap();
        assert_ne!(identity(&a).unwrap(), before);
        assert_eq!(content_identity(&a).unwrap(), id);
        // what is not a pack of this layout states nothing
        let bad = s.0.join("bad.tray");
        for (what, change) in [
            ("truncated", (&|v: &mut Vec<u8>| v.truncate(100)) as &dyn Fn(&mut Vec<u8>)),
            ("magic", &|v: &mut Vec<u8>| v[0] ^= 1),
            ("magic", &|v: &mut Vec<u8>| {
                let n = v.len();
                v[n - 1] ^= 1
            }),
            ("layout", &|v: &mut Vec<u8>| v[8] ^= 1),
        ] {
            let mut bytes = pack_bytes([1; 32], b"errors");
            change(&mut bytes);
            fs::write(&bad, bytes).unwrap();
            let e = content_identity(&bad).unwrap_err();
            assert!(e.message.contains(what), "{what}: {}", e.message);
        }
        assert!(content_identity(&s.0.join("absent.tray")).is_err());
    }

    #[test]
    fn a_prepared_sidecar_is_noted_in_the_analysis_folder() {
        let s = Scratch::new("note");
        let pack = s.0.join(".r.db.tray");
        let folder = analysis_dir(&pack).unwrap();
        let _ = fs::remove_dir_all(&folder);
        // nothing noted, no folder
        assert_eq!(noted_rules(&pack), None);
        // noted: the folder made, the path absolute, one line of it
        let sidecar = s.0.join("rules").join("deck.cal.rules.json");
        assert_eq!(note_rules(&pack, &sidecar).unwrap(), sidecar);
        assert_eq!(fs::read_to_string(folder.join("rules.path")).unwrap(), format!("{}\n", sidecar.display()));
        assert_eq!(noted_rules(&pack), Some(sidecar.clone()));
        // (the file it names need not be there - a place like the others)
        assert!(!sidecar.exists());
        // a relative path is noted as the place it means from here, dots resolved
        let relative = note_rules(&pack, Path::new("some/../other.rules.json")).unwrap();
        assert!(relative.is_absolute() && relative.ends_with("other.rules.json") && !relative.to_string_lossy().contains(".."));
        assert_eq!(noted_rules(&pack), Some(relative));
        // noted again: the last one, no temporary left
        note_rules(&pack, &sidecar).unwrap();
        assert_eq!(noted_rules(&pack), Some(sidecar.clone()));
        // the same path noted again: the file as it was, not written anew
        use std::os::unix::fs::MetadataExt;
        let stamp = |p: &Path| fs::metadata(p).map(|m| (m.ino(), m.mtime(), m.mtime_nsec())).unwrap();
        let before = stamp(&folder.join("rules.path"));
        note_rules(&pack, &sidecar).unwrap();
        assert_eq!(stamp(&folder.join("rules.path")), before);
        let names: Vec<String> = fs::read_dir(&folder).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["rules.path"]);
        // a note that is not one line naming an absolute path says nothing
        let long = format!("/{}\n", "x".repeat(9000));
        for junk in ["", "\n", "relative/deck.rules.json\n", "/a.rules.json\n/b.rules.json\n", long.as_str()] {
            fs::write(folder.join("rules.path"), junk).unwrap();
            assert_eq!(noted_rules(&pack), None, "{:?}", &junk[..junk.len().min(40)]);
        }
        fs::remove_file(folder.join("rules.path")).unwrap();
        fs::create_dir(folder.join("rules.path")).unwrap();
        assert_eq!(noted_rules(&pack), None);
        // a path of two lines is not noted
        assert!(note_rules(&pack, Path::new("/a\nb.rules.json")).is_err());
        // the sweep of other identities leaves a note be
        fs::remove_dir(folder.join("rules.path")).unwrap();
        note_rules(&pack, &sidecar).unwrap();
        assert_eq!(sweep_outdated(&folder, &"a".repeat(40), true), Outdated::default());
        assert_eq!(noted_rules(&pack), Some(sidecar));
    }

    #[test]
    fn the_caches_of_other_identities_are_swept_and_nothing_else() {
        let s = Scratch::new("sweep");
        let dir = s.0.join(".r.db.analysis");
        let (mine, old) = ("a".repeat(40), "b".repeat(40));
        let entry = |name: &str, files: &[(&str, usize)]| {
            let d = dir.join(name);
            fs::create_dir_all(&d).unwrap();
            for (file, len) in files {
                fs::write(d.join(file), vec![0u8; *len]).unwrap();
            }
            d
        };
        let table = |digest: &str, pack: Option<&str>| {
            let d = entry(&format!("delta/{digest}/measure"), &[("values.bin", 800), ("choices.bin", 400)]);
            if let Some(pack) = pack {
                fs::write(d.join("complete.json"), format!("{{\"key\":{{\"pack\":\"{pack}\",\"rule\":0}}}}")).unwrap();
            }
            entry(&format!("delta/{digest}/groups/{}", "9".repeat(40)), &[("ids.bin", 100)]);
            fs::write(dir.join("delta").join(digest).join("measure.lock"), b"").unwrap();
        };
        // this pack's: a rule's centres, a table with its groups
        entry(&format!("centers-r1-{mine}-0"), &[("centers.bin", 160), ("complete.json", 10)]);
        fs::write(dir.join(format!("centers-r1-{mine}-0.lock")), b"").unwrap();
        table(&"1".repeat(40), Some(&mine));
        // another identity's: two rules' centres with their locks, a dead
        // builder's staging folder, a table with its groups
        entry(&format!("centers-r1-{old}-0"), &[("centers.bin", 1600), ("complete.json", 10)]);
        fs::write(dir.join(format!("centers-r1-{old}-0.lock")), b"").unwrap();
        entry(&format!("centers-r1-{old}-12"), &[("centers.bin", 320)]);
        entry(&format!(".centers-r1-{old}-3.build-{}-0", i32::MAX), &[("centers.bin", 64)]);
        table(&"2".repeat(40), Some(&old));
        // what states nothing, or is not this store's, or is no real folder
        entry(&format!(".centers-r1-{old}-4.build-{}-0", std::process::id()), &[("centers.bin", 64)]);
        table(&"3".repeat(40), None);
        entry("delta/not-a-digest/measure", &[("complete.json", 0)]);
        fs::write(dir.join("delta").join("4".repeat(40)), b"a file").unwrap();
        entry(&format!("centers-v3-{}-0", "c".repeat(64)), &[("centers.bin", 16)]);
        // (not a digest of this store: its names are lower-case hex - said
        // by a name of other letters, an upper-case one being the same
        // folder where the file system folds case)
        entry(&format!("centers-r1-{}-0", "g".repeat(40)), &[("centers.bin", 16)]);
        assert!(hex_name(&old) && !hex_name(&"B".repeat(40)) && !hex_name(&"b".repeat(39)));
        entry(&format!("centers-r1-{old}-x"), &[("centers.bin", 16)]);
        fs::write(dir.join("bounds-1.npy"), b"other tool").unwrap();
        let elsewhere = entry("../elsewhere", &[("keep", 5)]);
        std::os::unix::fs::symlink(&elsewhere, dir.join(format!("centers-r1-{old}-7"))).unwrap();
        let listed = |d: &Path| -> Vec<String> {
            let mut names = Vec::new();
            let mut stack = vec![d.to_path_buf()];
            while let Some(p) = stack.pop() {
                for e in fs::read_dir(&p).unwrap().flatten() {
                    let path = e.path();
                    if fs::symlink_metadata(&path).unwrap().is_dir() {
                        stack.push(path.clone());
                    }
                    names.push(path.strip_prefix(d).unwrap().to_string_lossy().into_owned());
                }
            }
            names.sort();
            names
        };
        let before = listed(&dir);
        // asked alone: counted, nothing taken
        let outdated = Outdated { entries: 3, bytes: 1610 + 320 + 64 + 800 + 400 + 31 + 100 };
        let key_len = format!("{{\"key\":{{\"pack\":\"{old}\",\"rule\":0}}}}").len() as u64;
        let outdated = Outdated { bytes: outdated.bytes - 31 + key_len, ..outdated };
        assert_eq!(sweep_outdated(&dir, &mine, false), outdated);
        assert_eq!(listed(&dir), before);
        // taken away: the other identity's and nothing else
        assert_eq!(sweep_outdated(&dir, &mine, true), outdated);
        let after = listed(&dir);
        let gone: Vec<&String> = before.iter().filter(|n| !after.contains(n)).collect();
        assert!(!gone.is_empty() && gone.iter().all(|n| {
            n.starts_with(&format!("centers-r1-{old}-0"))
                || n.starts_with(&format!("centers-r1-{old}-12"))
                || n.starts_with(&format!(".centers-r1-{old}-3.build-"))
                || n.starts_with(&format!("delta/{}", "2".repeat(40)))
        }), "{gone:?}");
        for kept in [
            format!("centers-r1-{mine}-0/centers.bin"),
            format!("centers-r1-{mine}-0.lock"),
            format!("delta/{}/measure/complete.json", "1".repeat(40)),
            format!("delta/{}/groups/{}/ids.bin", "1".repeat(40), "9".repeat(40)),
            format!(".centers-r1-{old}-4.build-{}-0/centers.bin", std::process::id()),
            format!("delta/{}/measure/values.bin", "3".repeat(40)),
            "delta/not-a-digest/measure/complete.json".to_string(),
            format!("delta/{}", "4".repeat(40)),
            format!("centers-v3-{}-0/centers.bin", "c".repeat(64)),
            format!("centers-r1-{}-0/centers.bin", "g".repeat(40)),
            format!("centers-r1-{old}-x/centers.bin"),
            "bounds-1.npy".to_string(),
            format!("centers-r1-{old}-7"),
        ] {
            assert!(after.contains(&kept), "{kept} is gone");
        }
        assert!(elsewhere.join("keep").is_file());
        for name in [format!("centers-r1-{old}-0"), format!("centers-r1-{old}-0.lock"), format!("centers-r1-{old}-12")] {
            assert!(!dir.join(&name).exists(), "{name} is left");
        }
        // nothing more to take; a folder that is not there has nothing
        assert_eq!(sweep_outdated(&dir, &mine, true), Outdated::default());
        assert_eq!(sweep_outdated(&s.0.join("absent"), &mine, true), Outdated::default());
        // the other way round the rest goes, this identity's being the old one's now
        assert_eq!(sweep_outdated(&dir, &old, true).entries, 2);
    }
}
