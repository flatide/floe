//! Publish only a complete artifact. Failed exports never truncate the user's
//! old output, nor write into the source/cache through a path alias.
use crate::{cache, catalog::Layout, check_cancelled, Error, Result};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

static SERIAL: AtomicU64 = AtomicU64::new(0);
/// Resolve the existing prefix, preserving a missing source/cache suffix.
/// This permits reports for missing sources without allowing those reports
/// to replace the very source/cache paths they diagnose.
pub(crate) fn resolve_prefix(path: &Path) -> Result<PathBuf> {
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
    protected_output_mode(path, files, trees, false)
}
pub(crate) fn protected_output_mode(
    path: &Path,
    files: &[PathBuf],
    trees: &[PathBuf],
    planned: bool,
) -> Result<PathBuf> {
    let out = cache::absolute(path)?;
    let parent = out
        .parent()
        .ok_or_else(|| Error::input("output needs a parent directory"))?;
    let resolved = (if planned {
        resolve_prefix(parent)?
    } else {
        fs::canonicalize(parent)?
    })
    .join(
        out.file_name()
            .ok_or_else(|| Error::input("output needs a filename"))?,
    );
    for file in files {
        if out == cache::absolute(file)?
            || resolved == resolve_prefix(file)?
            || fs::canonicalize(&resolved).ok().as_ref() == Some(&resolve_prefix(file)?)
        {
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
    layout_output_mode(path, layout, false)
}
pub(crate) fn layout_output_mode(path: &Path, layout: &Layout, planned: bool) -> Result<PathBuf> {
    let out = cache::absolute(path)?;
    let parent = out
        .parent()
        .ok_or_else(|| Error::input("output needs a parent directory"))?;
    let resolved = (if planned {
        resolve_prefix(parent)?
    } else {
        fs::canonicalize(parent)?
    })
    .join(
        out.file_name()
            .ok_or_else(|| Error::input("output needs a filename"))?,
    );
    let mut lock = layout.directory.as_os_str().to_owned();
    lock.push(".index.lock");
    let lock = PathBuf::from(lock);
    let resolved_lock = fs::canonicalize(lock.parent().unwrap())?.join(lock.file_name().unwrap());
    if resolved == fs::canonicalize(&layout.source)?
        || fs::canonicalize(&resolved).ok() == Some(fs::canonicalize(&layout.source)?)
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
    // Keep existing in-memory PNG/report publication free of extra copies.
    publish_with(path, cancelled, |file| {
        for chunk in data.chunks(1024 * 1024) {
            check_cancelled(cancelled)?;
            file.write_all(chunk)?;
        }
        Ok(())
    })
}

/// Stream an owned complete artifact without a geometry-sized payload copy.
/// The advertised byte count must match exactly; short/growing input cannot
/// replace an old output. As with publish(), rename is the commit point.
pub fn publish_reader(
    path: &Path,
    reader: &mut impl Read,
    length: u64,
    cancelled: &AtomicUsize,
) -> Result<()> {
    publish_with(path, cancelled, |file| {
        let mut buffer = vec![0; (1024 * 1024).min(length.max(1)) as usize];
        let mut remaining = length;
        while remaining != 0 {
            let limit = remaining.min(buffer.len() as u64) as usize;
            let n = read_cancellable(reader, &mut buffer[..limit], cancelled)?;
            if n == 0 {
                return Err(Error::input("artifact shorter than advertised"));
            }
            file.write_all(&buffer[..n])?;
            remaining -= n as u64;
        }
        if read_cancellable(reader, &mut buffer[..1], cancelled)? != 0 {
            return Err(Error::input("artifact longer than advertised"));
        }
        Ok(())
    })
}

fn read_cancellable(
    reader: &mut impl Read,
    buffer: &mut [u8],
    cancelled: &AtomicUsize,
) -> Result<usize> {
    loop {
        check_cancelled(cancelled)?;
        match reader.read(buffer) {
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            result => return result.map_err(Into::into),
        }
    }
}

fn publish_with(
    path: &Path,
    cancelled: &AtomicUsize,
    write: impl FnOnce(&mut std::fs::File) -> Result<()>,
) -> Result<()> {
    StagedArtifact::write(path, cancelled, write)?.commit(cancelled)
}

/// Stage one complete export without replacing its target yet. A mosaic keeps
/// at most five of these until all four renders succeed; Drop removes only
/// the create-new temporary, never a user's output or directory.
pub struct StagedArtifact {
    temporary: Option<PathBuf>,
    target: PathBuf,
}
impl StagedArtifact {
    pub fn write(
        path: &Path,
        cancelled: &AtomicUsize,
        write: impl FnOnce(&mut std::fs::File) -> Result<()>,
    ) -> Result<Self> {
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
        let staged = Self {
            temporary: Some(temporary),
            target: path.to_owned(),
        };
        write(&mut file)?;
        file.sync_all()?;
        check_cancelled(cancelled)?;
        Ok(staged)
    }
    pub fn commit(mut self, cancelled: &AtomicUsize) -> Result<()> {
        check_cancelled(cancelled)?;
        fs::rename(
            self.temporary.as_ref().expect("uncommitted stage"),
            &self.target,
        )?;
        self.temporary = None;
        // Rename is the commit point; no late signal can unpublish it.
        Ok(())
    }
}
impl Drop for StagedArtifact {
    fn drop(&mut self) {
        if let Some(path) = &self.temporary {
            let _ = fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "floe-stream-test-{}-{}",
                std::process::id(),
                SERIAL.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&dir).unwrap();
            Self(dir)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    #[test]
    fn staged_files_publish_only_on_commit() {
        let dir = Temp::new();
        let output = dir.0.join("mosaic.png");
        let flag = AtomicUsize::new(0);
        fs::write(&output, b"old").unwrap();
        let stage = || {
            StagedArtifact::write(&output, &flag, |f| {
                f.write_all(b"new")?;
                Ok(())
            })
            .unwrap()
        };
        let pending = stage();
        assert_eq!(fs::read(&output).unwrap(), b"old");
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 2);
        drop(pending);
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
        assert!(StagedArtifact::write(&output, &flag, |f| {
            f.write_all(b"failed")?;
            Err(Error::input("encode failed"))
        })
        .is_err());
        assert_eq!(fs::read(&output).unwrap(), b"old");
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
        let pending = stage();
        flag.store(2, Ordering::Relaxed);
        assert_eq!(
            pending.commit(&flag).unwrap_err().kind,
            crate::ErrorKind::Cancelled
        );
        assert_eq!(fs::read(&output).unwrap(), b"old");
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
        flag.store(0, Ordering::Relaxed);
        stage().commit(&flag).unwrap();
        assert_eq!(fs::read(&output).unwrap(), b"new");
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
    }
    #[test]
    fn stream_is_exact_bounded_and_atomic() {
        let dir = Temp::new();
        let output = dir.0.join("clip.oas");
        let flag = AtomicUsize::new(0);
        fs::write(&output, b"old export").unwrap();
        for length in [1, 3] {
            assert!(publish_reader(&output, &mut Cursor::new(b"ab"), length, &flag).is_err());
            assert_eq!(fs::read(&output).unwrap(), b"old export");
            assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
        }
        struct Input<'a> {
            remaining: u64,
            flag: &'a AtomicUsize,
            cancel: bool,
        }
        impl Read for Input<'_> {
            fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
                assert!(b.len() <= 1024 * 1024);
                let n = self.remaining.min(b.len() as u64) as usize;
                b[..n].fill(123);
                self.remaining -= n as u64;
                if self.cancel {
                    self.flag.store(2, Ordering::Relaxed);
                }
                Ok(n)
            }
        }
        let len = 3 * 1024 * 1024 + 9;
        let mut input = Input {
            remaining: len,
            flag: &flag,
            cancel: true,
        };
        assert_eq!(
            publish_reader(&output, &mut input, len, &flag)
                .unwrap_err()
                .kind,
            crate::ErrorKind::Cancelled
        );
        assert_eq!(fs::read(&output).unwrap(), b"old export");
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
        flag.store(0, Ordering::Relaxed);
        let mut input = Input {
            remaining: len,
            flag: &flag,
            cancel: false,
        };
        publish_reader(&output, &mut input, len, &flag).unwrap();
        let bytes = fs::read(&output).unwrap();
        assert_eq!(bytes.len() as u64, len);
        assert!(bytes.iter().all(|b| *b == 123));
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
        struct Failed;
        impl Read for Failed {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("read failed"))
            }
        }
        assert!(publish_reader(&output, &mut Failed, 5, &flag).is_err());
        assert_eq!(fs::read(&output).unwrap(), bytes);
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
    }

    #[test]
    fn interrupted_read_retries_or_cancels_without_publishing() {
        let dir = Temp::new();
        let output = dir.0.join("out");
        let flag = AtomicUsize::new(0);
        struct Interrupted<'a> {
            first: bool,
            cancel: bool,
            flag: &'a AtomicUsize,
        }
        impl Read for Interrupted<'_> {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                if self.first {
                    self.first = false;
                    if self.cancel {
                        self.flag.store(2, Ordering::Relaxed);
                    }
                    Err(std::io::ErrorKind::Interrupted.into())
                } else {
                    Ok(0)
                }
            }
        }
        let mut input = Interrupted {
            first: true,
            cancel: false,
            flag: &flag,
        };
        publish_reader(&output, &mut input, 0, &flag).unwrap();
        assert!(fs::read(&output).unwrap().is_empty());
        publish(&output, b"old", &flag).unwrap();
        let mut input = Interrupted {
            first: true,
            cancel: true,
            flag: &flag,
        };
        assert_eq!(
            publish_reader(&output, &mut input, 0, &flag)
                .unwrap_err()
                .kind,
            crate::ErrorKind::Cancelled
        );
        assert_eq!(fs::read(&output).unwrap(), b"old");
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
    }
}
