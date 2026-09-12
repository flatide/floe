//! Publish only a complete artifact. Failed renders never truncate the user's
//! old PNG, nor write into the source/cache through a path alias.
use crate::{cache, catalog::Layout, check_cancelled, Error, Result};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

static SERIAL: AtomicU64 = AtomicU64::new(0);
pub fn output_path(path: &Path, layout: &Layout) -> Result<PathBuf> {
    let out = cache::absolute(path)?;
    let parent = out
        .parent()
        .ok_or_else(|| Error::input("output needs a parent directory"))?;
    let resolved = fs::canonicalize(parent)?.join(
        out.file_name()
            .ok_or_else(|| Error::input("output needs a filename"))?,
    );
    let mut lock = layout.directory.as_os_str().to_owned();
    lock.push(".index.lock");
    let lock = PathBuf::from(lock);
    let resolved_lock = fs::canonicalize(lock.parent().unwrap())?.join(lock.file_name().unwrap());
    if resolved == fs::canonicalize(&layout.source)?
        || out == layout.source
        || resolved.starts_with(fs::canonicalize(&layout.directory)?)
        || out.starts_with(&layout.directory)
        || resolved == resolved_lock
    {
        return Err(Error::input(
            "output must be outside the source and its cache",
        ));
    }
    match fs::symlink_metadata(&resolved) {
        Ok(m) if !m.is_file() => {
            return Err(Error::input(
                "output target is not a regular file (symlink unsupported)",
            ))
        }
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
        _ => (),
    }
    Ok(resolved)
}
pub fn publish(path: &Path, data: &[u8], cancelled: &AtomicUsize) -> Result<()> {
    check_cancelled(cancelled)?;
    let parent = path
        .parent()
        .ok_or_else(|| Error::input("output needs a parent directory"))?;
    let mut created = None;
    for _ in 0..128 {
        let serial = SERIAL.fetch_add(1, Ordering::Relaxed);
        let p = parent.join(format!(".floe-shot-{}-{serial}.tmp", std::process::id()));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&p)
        {
            Ok(f) => {
                created = Some((p, f));
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    let (temporary, mut file) =
        created.ok_or_else(|| Error::input("cannot allocate output staging file"))?;
    let result = (|| -> Result<()> {
        for chunk in data.chunks(1024 * 1024) {
            check_cancelled(cancelled)?;
            file.write_all(chunk)?;
        }
        file.sync_all()?;
        check_cancelled(cancelled)?;
        fs::rename(&temporary, path)?;
        // Rename is the commit point. Do not report a late signal as an
        // unpublished/cancelled artifact after this succeeds.
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}
