//! A run's own folder for the caches of sources that have none beside them
//! (`floe2 render --temp-index`, user 2026-10-28: "인덱싱이 되어 있지 않더라도
//! 캡쳐할 수 있게"): the index a read command builds first goes under it
//! instead of beside the source - a folder that cannot be written, a one-off
//! capture that should leave nothing behind - and the folder goes with the
//! command (Drop). A cache beside the source is opened as it is, as the
//! auto-index does.
use super::{absolute, cache_path, inspect, CacheState};
use crate::{Error, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// What a source has of a cache in a directory, for its index first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Found {
    /// none under either name
    None,
    /// a real folder without its commit (no meta.json): a build that
    /// stopped half-way left it - or one runs now (the locks tell)
    Incomplete,
    /// one: current, of a changed source, or one this build cannot read
    There,
}
/// `source`'s cache in `directory`, as the auto-index judges it.
pub fn found(source: &Path, directory: &Path) -> Result<Found> {
    if inspect(source, directory)? == CacheState::Missing {
        return Ok(Found::None);
    }
    let real =
        fs::symlink_metadata(directory).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink());
    let uncommitted = matches!(
        fs::symlink_metadata(directory.join("meta.json")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound
    );
    Ok(if real && uncommitted {
        Found::Incomplete
    } else {
        Found::There
    })
}

/// FNV-1a over the bytes: the folder name a source's path gets under the
/// run's root (two sources of one name in different folders stay apart).
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// `source`'s cache folder under `root`: `<hash of the absolute path>/.NAME.ice`
/// (the `.NAME.ice` spelling keeps the lock's subject readable; the cache's
/// meta.json names the source, so the folder may be anywhere).
pub fn cache_dir_under(root: &Path, source: &Path) -> Result<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let source = absolute(source)?;
    let name = source
        .file_name()
        .ok_or_else(|| Error::input("source must name a file"))?;
    let mut folder = std::ffi::OsString::from(".");
    folder.push(name);
    folder.push(".ice");
    Ok(root
        .join(format!("{:016x}", fnv1a(source.as_os_str().as_bytes())))
        .join(folder))
}

/// Where `source`'s cache is read and written under a temporary index:
/// the one beside it when a real one is there (whatever its state - it is
/// opened as it is, as the auto-index does) or another run writes it now
/// (the open waits for it or says who); else the run's own folder
/// (`cache_dir_under`) - true.
pub fn resolve_under(root: &Path, source: &Path) -> Result<(PathBuf, bool)> {
    let beside = cache_path(source)?;
    let key = floe_vfs::lock::key(floe_vfs::lock::Kind::Vfs, &beside.to_string_lossy());
    if found(source, &beside)? == Found::There || floe_vfs::lock::writing_refusal(&key).is_some() {
        return Ok((beside, false));
    }
    Ok((cache_dir_under(root, source)?, true))
}

/// The run's temporary index: a fresh folder `<program>-temp-index-<pid>-<n>`
/// under the base given (`--temp-index=DIR`) or the system's temporary
/// directory, removed with everything under it when dropped (after the
/// dataset and the renderer that read it). A run killed outright leaves it
/// for the host's cleaning.
#[derive(Debug)]
pub struct TempIndex {
    root: PathBuf,
}
impl TempIndex {
    pub fn create(base: Option<&Path>) -> Result<Self> {
        let base = match base {
            Some(b) => absolute(b)?,
            None => std::env::temp_dir(),
        };
        if !base.is_dir() {
            return Err(Error::input(format!(
                "--temp-index: {} is not a directory",
                base.display()
            )));
        }
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        for n in 0..16u32 {
            let root = base.join(format!(
                "{}-temp-index-{pid}-{:x}",
                crate::program(),
                u64::from(nanos).wrapping_add(u64::from(n))
            ));
            match fs::create_dir(&root) {
                Ok(()) => return Ok(Self { root }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => {
                    return Err(Error::new(
                        crate::ErrorKind::Io,
                        format!("--temp-index: cannot create {}: {e}", root.display()),
                    ))
                }
            }
        }
        Err(Error::input(format!(
            "--temp-index: no free folder name under {}",
            base.display()
        )))
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
}
impl Drop for TempIndex {
    fn drop(&mut self) {
        // what this run made, and nothing else: the root did not exist
        // before create_dir made it
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_temporary_index_names_its_folders_and_goes_with_the_run() {
        let base =
            std::env::temp_dir().join(format!("floe-temp-index-test-{}", std::process::id()));
        fs::create_dir(&base).unwrap();
        let source = base.join("a.oas");
        fs::write(&source, b"x").unwrap();
        let other = base.join("sub");
        fs::create_dir(&other).unwrap();
        fs::write(other.join("a.oas"), b"y").unwrap();
        let root = {
            let temp = TempIndex::create(Some(&base)).unwrap();
            let root = temp.root().to_path_buf();
            assert!(root.is_dir() && root.starts_with(&base));
            assert!(root
                .file_name()
                .unwrap()
                .to_string_lossy()
                .contains("-temp-index-"));
            // the source's folder: hashed path, the .NAME.ice spelling
            let dir = cache_dir_under(&root, &source).unwrap();
            assert_eq!(dir.file_name().unwrap().to_string_lossy(), ".a.oas.ice");
            assert_eq!(dir.parent().unwrap().parent().unwrap(), root);
            assert_eq!(dir.parent().unwrap().file_name().unwrap().len(), 16);
            assert_ne!(dir, cache_dir_under(&root, &other.join("a.oas")).unwrap());
            assert_eq!(dir, cache_dir_under(&root, &base.join("./a.oas")).unwrap());
            // no cache beside the source: the run's own folder
            assert_eq!(resolve_under(&root, &source).unwrap(), (dir.clone(), true));
            assert_eq!(found(&source, &dir).unwrap(), Found::None);
            // a folder without its commit beside it: still the run's own
            let beside = cache_path(&source).unwrap();
            fs::create_dir(&beside).unwrap();
            assert_eq!(found(&source, &beside).unwrap(), Found::Incomplete);
            assert_eq!(resolve_under(&root, &source).unwrap(), (dir.clone(), true));
            // a committed cache beside it (whatever its state): opened as it is
            fs::write(beside.join("meta.json"), b"{}").unwrap();
            assert_eq!(found(&source, &beside).unwrap(), Found::There);
            assert_eq!(
                resolve_under(&root, &source).unwrap(),
                (beside.clone(), false)
            );
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("x"), b"x").unwrap();
            root
        };
        assert!(!root.exists(), "the temporary index was not removed");
        assert!(source.is_file() && cache_path(&source).unwrap().is_dir());
        let e = TempIndex::create(Some(&base.join("absent"))).unwrap_err();
        assert!(e.message.contains("is not a directory"), "{}", e.message);
        fs::remove_dir_all(&base).unwrap();
    }
}
