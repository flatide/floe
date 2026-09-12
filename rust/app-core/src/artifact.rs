//! Publish only a complete artifact. Failed renders never truncate the user's
//! old PNG, nor write into the source/cache through a path alias.
use crate::{cache, catalog::Layout, check_cancelled, Error, Result};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

static SERIAL: AtomicU64 = AtomicU64::new(0);
/// Resolve the existing prefix, preserving a missing source/cache suffix.
/// This permits reports for missing sources without allowing those reports
/// to replace the very source/cache paths they diagnose.
fn resolve_prefix(path: &Path) -> Result<PathBuf> {
    let absolute = cache::absolute(path)?;
    let mut prefix = absolute.as_path();
    let mut suffix = Vec::new();
    loop {
        match fs::canonicalize(prefix) {
            Ok(mut resolved) => {
                for s in suffix.iter().rev() {
                    resolved.push(s);
                }
                return Ok(resolved);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                suffix.push(
                    prefix
                        .file_name()
                        .ok_or_else(|| Error::input("unresolvable protected path"))?,
                );
                prefix = prefix
                    .parent()
                    .ok_or_else(|| Error::input("unresolvable protected parent"))?;
            }
            Err(e) => return Err(e.into()),
        }
    }
}
pub fn protected_output(path: &Path, files: &[PathBuf], trees: &[PathBuf]) -> Result<PathBuf> {
    let out = cache::absolute(path)?;
    let parent = out
        .parent()
        .ok_or_else(|| Error::input("output needs a parent directory"))?;
    let resolved = fs::canonicalize(parent)?.join(
        out.file_name()
            .ok_or_else(|| Error::input("output needs a filename"))?,
    );
    for file in files {
        if out == cache::absolute(file)? || resolved == resolve_prefix(file)? {
            return Err(Error::input(
                "output would replace a source, color input, or cache lock",
            ));
        }
    }
    for tree in trees {
        if out.starts_with(cache::absolute(tree)?) || resolved.starts_with(resolve_prefix(tree)?) {
            return Err(Error::input("output must be outside source caches"));
        }
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
/// Explicit, cancellable artifact size guard, not a truncated JSON document.
pub fn json_bytes(value: &serde_json::Value, cancelled: &AtomicUsize) -> Result<Vec<u8>> {
    struct Bounded<'a> {
        bytes: Vec<u8>,
        cancelled: &'a AtomicUsize,
    }
    impl Write for Bounded<'_> {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            if self.cancelled.load(Ordering::Relaxed) != 0 {
                return Err(std::io::Error::other("operation cancelled"));
            }
            if self
                .bytes
                .len()
                .checked_add(b.len())
                .is_none_or(|n| n > 128 * 1024 * 1024)
            {
                return Err(std::io::Error::other("JSON artifact exceeds 128 MiB"));
            }
            self.bytes.extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = Bounded {
        bytes: Vec::new(),
        cancelled,
    };
    let result = serde_json::to_writer_pretty(&mut writer, value);
    check_cancelled(cancelled)?;
    result.map_err(|e| Error::input(e.to_string()))?;
    Ok(writer.bytes)
}
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
