use crate::{Error, FrameFormat, Result};
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

pub(crate) struct Workspace(pub PathBuf);
impl Workspace {
    pub fn create(root: &Path) -> Result<Self> {
        let root = fs::canonicalize(root)?;
        wire_path(&root)?;
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        for _ in 0..64 {
            let serial = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
            let path = root.join(format!(
                "floe-worker-{}-{stamp}-{serial}",
                std::process::id()
            ));
            match DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Err(Error::input("could not allocate private worker directory"))
    }
    pub fn output(&self, gen: u64, format: FrameFormat) -> PathBuf {
        self.0.join(format!("frame-{gen}.{}", format.wire()))
    }
    pub fn frame_path(
        &self,
        reported: &str,
        gen: u64,
        round: u64,
        final_frame: bool,
        format: FrameFormat,
    ) -> Result<PathBuf> {
        let base = self.output(gen, format);
        let expected = if final_frame {
            base
        } else {
            PathBuf::from(format!(
                "{}.gen-{gen}.round-{round}.partial.{}",
                wire_path(&base)?,
                format.wire()
            ))
        };
        // Exact filename equality, not a prefix/starts_with check. Never let
        // a daemon-supplied path turn cleanup into an arbitrary-file operation.
        if Path::new(reported) != expected {
            return Err(Error::protocol("frame path outside issued output slot"));
        }
        Ok(expected)
    }
    pub fn write_style(&self, epoch: u64, text: &str) -> Result<PathBuf> {
        let path = self.0.join(format!("style-{epoch}.tsv"));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&path)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        Ok(path)
    }
    pub fn cleanup(&self) -> Result<()> {
        match fs::remove_dir_all(&self.0) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        // Only the private directory created by create(), never a caller's
        // cache. remove_dir_all unlinks the cache alias without following it.
        let _ = self.cleanup();
    }
}

pub(crate) fn wire_path(path: &Path) -> Result<&str> {
    path.to_str()
        .filter(|s| !s.chars().any(|c| c.is_whitespace() || c.is_control()))
        .ok_or_else(|| {
            Error::input("internal wire path must be UTF-8 without whitespace/control characters")
        })
}

fn frame_file(path: &Path) -> Result<File> {
    // O_NONBLOCK avoids hanging on a FIFO; O_NOFOLLOW closes the leaf symlink
    // check/open race. libc constants do not require unsafe Rust.
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.nlink() != 1 {
        return Err(Error::protocol("frame is not a private regular file"));
    }
    Ok(file)
}

/// Validate the version-pinned native writer's envelope, not arbitrary OASIS
/// semantics. Geometry correctness is covered by the clip Region XOR oracle.
/// Only the issued slot is read/unlinked; no response-supplied path is used.
pub(crate) fn consume_clip(path: &Path, size: u64, unit: f64, name: &str) -> Result<File> {
    let result = (|| {
        let mut file = frame_file(path)?;
        let mut prefix = b"%SEMI-OASIS\r\n\x01\x031.0\x07".to_vec();
        prefix.extend_from_slice(&unit.to_le_bytes());
        prefix.extend_from_slice(&[0; 13]);
        prefix.push(14); // inline CELL name
        let mut n = name.len() as u64;
        while n >= 128 {
            prefix.push((n as u8 & 127) | 128);
            n >>= 7;
        }
        prefix.push(n as u8);
        prefix.extend_from_slice(name.as_bytes());
        if file.metadata()?.len() != size || size < prefix.len() as u64 + 257 {
            return Err(Error::protocol("clip length mismatch"));
        }
        let mut actual = vec![0; prefix.len()];
        file.read_exact(&mut actual)?;
        if actual != prefix {
            return Err(Error::protocol("clip magic/unit/cell header mismatch"));
        }
        let mut end = [0; 256];
        file.seek(SeekFrom::End(-256))?;
        file.read_exact(&mut end)?;
        if end[..3] != [2, 252, 1] || end[3..].iter().any(|b| *b != 0) {
            return Err(Error::protocol("clip END envelope mismatch"));
        }
        file.rewind()?;
        Ok(file)
    })();
    let removed = fs::remove_file(path).map_err(Error::from);
    match result {
        Err(e) => Err(e),
        Ok(file) => {
            removed?;
            Ok(file)
        }
    }
}

pub(crate) fn consume(
    path: &Path,
    max_bytes: usize,
    expected: Option<(FrameFormat, u32, u32)>,
) -> Result<Option<Vec<u8>>> {
    let result = (|| {
        let file = frame_file(path)?;
        if file.metadata()?.len() > max_bytes as u64 {
            return Err(Error::protocol("frame byte limit exceeded"));
        }
        let Some((format, width, height)) = expected else {
            return Ok(None);
        };
        let mut bytes = Vec::new();
        file.take(max_bytes as u64 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > max_bytes {
            return Err(Error::protocol("frame grew past byte limit"));
        }
        validate_payload(&bytes, format, width, height)?;
        Ok(Some(bytes))
    })();
    // Even malformed or stale owned outputs are reclaimed. A rejected path
    // never reaches this function; unlinking a leaf cannot follow a symlink.
    let removed = fs::remove_file(path).map_err(Error::from);
    match result {
        Err(e) => Err(e),
        Ok(value) => {
            removed?;
            Ok(value)
        }
    }
}

pub(crate) fn validate_payload(bytes: &[u8], format: FrameFormat, w: u32, h: u32) -> Result<()> {
    match format {
        FrameFormat::Raw => {
            let expected = u64::from(w)
                .checked_mul(u64::from(h))
                .and_then(|v| v.checked_mul(4))
                .and_then(|v| v.checked_add(16));
            if expected != Some(bytes.len() as u64)
                || bytes.get(..8) != Some(b"FLOERAW1")
                || bytes.get(8..12) != Some(w.to_le_bytes().as_slice())
                || bytes.get(12..16) != Some(h.to_le_bytes().as_slice())
            {
                return Err(Error::protocol(
                    "raw frame dimensions/magic/length mismatch",
                ));
            }
        }
        FrameFormat::Png => validate_png(bytes, w, h)?,
    }
    Ok(())
}

fn validate_png(bytes: &[u8], w: u32, h: u32) -> Result<()> {
    let bad = || Error::protocol("invalid native RGBA PNG frame");
    if bytes.get(..8) != Some(b"\x89PNG\r\n\x1a\n") {
        return Err(bad());
    }
    let mut offset = 8usize;
    // Native renderer contract: RGBA8, non-interlaced, single IDAT. Validate
    // CRCs and chunk boundaries without re-decoding PNG on the hot path.
    for kind in [b"IHDR", b"IDAT", b"IEND"] {
        let header = bytes
            .get(offset..offset.checked_add(8).ok_or_else(bad)?)
            .ok_or_else(bad)?;
        let len = u32::from_be_bytes(header[..4].try_into().unwrap()) as usize;
        if &header[4..] != kind {
            return Err(bad());
        }
        let end = offset
            .checked_add(8)
            .and_then(|v| v.checked_add(len))
            .ok_or_else(bad)?;
        let data = bytes.get(offset + 8..end).ok_or_else(bad)?;
        let crc_end = end.checked_add(4).ok_or_else(bad)?;
        let crc = bytes.get(end..crc_end).ok_or_else(bad)?;
        if crc32fast::hash(&bytes[offset + 4..end]).to_be_bytes() != crc {
            return Err(bad());
        }
        match kind {
            b"IHDR"
                if data.len() != 13
                    || data[..4] != w.to_be_bytes()
                    || data[4..8] != h.to_be_bytes()
                    || data[8..] != [8, 6, 0, 0, 0] =>
            {
                return Err(bad())
            }
            b"IDAT" if data.is_empty() => return Err(bad()),
            b"IEND" if !data.is_empty() => return Err(bad()),
            _ => {}
        }
        offset = crc_end;
    }
    if offset != bytes.len() {
        return Err(bad());
    }
    Ok(())
}
