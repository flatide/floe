use crate::{Error, ErrorKind, Result};
use serde::Deserialize;
use std::fs::{self, OpenOptions};
use std::io::BufReader;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

#[derive(Debug, Deserialize)]
struct SourceIdentity {
    size: u64,
    mtime: u64,
}
#[derive(Debug, Deserialize)]
struct MetaIdentity {
    version: u64,
    vfs: u64,
    src: SourceIdentity,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CacheState {
    Missing,
    Current,
    Unusable(String),
}

pub fn absolute(path: &Path) -> Result<PathBuf> {
    // Match Python abspath: do not resolve a source symlink and silently move
    // its sibling cache to the link target's directory.
    let joined = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut out = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => (),
            _ => out.push(c),
        }
    }
    utf8(&out)?;
    Ok(out)
}
pub(crate) fn utf8(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| Error::input("native indexer requires a UTF-8 path"))
}
pub fn cache_path(source: &Path) -> Result<PathBuf> {
    let mut p = absolute(source)?.into_os_string();
    p.push(".floe");
    Ok(PathBuf::from(p))
}
pub fn fingerprint(source: &Path) -> Result<(u64, u64)> {
    let m = fs::metadata(source)?;
    if !m.is_file() {
        return Err(Error::input("source is not a regular file"));
    }
    let time = m
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Error::input("source mtime predates UNIX epoch"))?;
    Ok((m.len(), time.as_secs()))
}

pub fn inspect(source: &Path, directory: &Path) -> Result<CacheState> {
    let m = match fs::symlink_metadata(directory) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(CacheState::Missing),
        Err(e) => return Err(e.into()),
    };
    if !m.is_dir() || m.file_type().is_symlink() {
        return Ok(CacheState::Unusable(
            "cache path is not a real directory (symlink unsupported)".into(),
        ));
    }
    let check = || -> Result<()> {
        let f = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(directory.join("meta.json"))?;
        if !f.metadata()?.is_file() {
            return Err(Error::input("meta.json is not a regular file"));
        }
        // Deserialize only identity. Large minimap frontiers/layer tables are
        // skipped by serde rather than materialized as a second scene in RAM.
        let meta: MetaIdentity = serde_json::from_reader(BufReader::new(f)).map_err(|e| {
            Error::new(
                ErrorKind::Cache,
                format!("cache metadata is unreadable: {e}"),
            )
        })?;
        let version: u64 = env!("FLOE_CACHE_VERSION")
            .parse()
            .expect("build cache version");
        if meta.version != version || meta.vfs != 1 {
            return Err(Error::new(
                ErrorKind::Cache,
                "cache version/type mismatch; re-index required",
            ));
        }
        let (size, mtime) = fingerprint(source)?;
        if (size, mtime) != (meta.src.size, meta.src.mtime) {
            return Err(Error::new(ErrorKind::Cache, "source fingerprint changed"));
        }
        // Canonical structural/pair validator, not a nonzero marker test.
        // Reject FIFOs/devices before the mmap reader opens them. Concurrent
        // external cache replacement remains outside this local lease model.
        for name in ["design.ovm", "design.ovp", "design.ovt"] {
            match fs::symlink_metadata(directory.join(name)) {
                Ok(m) if m.is_file() => (),
                Err(e) if name == "design.ovt" && e.kind() == std::io::ErrorKind::NotFound => (),
                _ => {
                    return Err(Error::new(
                        ErrorKind::Cache,
                        format!("{name} is not a regular cache file"),
                    ))
                }
            }
        }
        let vfs = floe_vfs::Vfs::open(utf8(directory)?).map_err(|e| {
            Error::new(
                ErrorKind::Cache,
                format!("cache commit validation failed: {e}"),
            )
        })?;
        if (vfs.ovm.src_size, vfs.ovm.src_mtime) != (size, mtime) {
            return Err(Error::new(
                ErrorKind::Cache,
                "cache marker source fingerprint mismatch",
            ));
        }
        Ok(())
    };
    Ok(match check() {
        Ok(()) => CacheState::Current,
        Err(e) => CacheState::Unusable(e.to_string()),
    })
}
