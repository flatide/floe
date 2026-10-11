//! An index pinned into the executable binds original notice files/chunks.
//! SHA-1 here is a content/change identifier, NOT publisher authentication.
//! Package SHA256SUMS and external provenance verification remain separate.
#![deny(unsafe_op_in_unsafe_fn)]
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::{
    collections::BTreeSet,
    ffi::CString,
    fs::{self, File, OpenOptions},
    io,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{FileExt, MetadataExt, OpenOptionsExt},
    },
    path::Path,
    sync::atomic::{AtomicUsize, Ordering},
};

pub const INDEX_NAME: &str = "NOTICE-INDEX.json";
pub const CHUNK_BYTES: usize = 65536;
pub const INDEX_BYTES: usize = 2 * 1024 * 1024;
pub const TOTAL_BYTES: u64 = 128 * 1024 * 1024;
pub const FILES: usize = 4096;
pub const LIST_SIZE: usize = 64;

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "notice index or file changed/invalid",
    )
}
fn check(cancel: &AtomicUsize) -> io::Result<()> {
    if cancel.load(Ordering::Relaxed) != 0 {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "notice read cancelled",
        ))
    } else {
        Ok(())
    }
}
pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha1::digest(bytes))
}
fn hash_valid(s: &str) -> bool {
    s.len() == 40
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn safe_name(s: &str) -> bool {
    s.starts_with("NOTICES/")
        && s.len() <= 1024
        && !s.chars().any(|c| c.is_control() || c == '\\')
        && s.split('/').count() <= 16
        && s.split('/').all(|c| !c.is_empty() && c != "." && c != "..")
}
fn root(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(fs::canonicalize(path)?)
}
// Each component is resolved relative to an already-open directory, without
// following symlinks. Renaming the installation cannot retarget this reader.
fn relative(root: &File, name: &str) -> io::Result<File> {
    let mut parent = root.try_clone()?;
    let parts: Vec<_> = name.split('/').collect();
    for (i, part) in parts.iter().enumerate() {
        let part = CString::new(*part).map_err(|_| invalid())?;
        let directory = if i + 1 < parts.len() {
            libc::O_DIRECTORY
        } else {
            0
        };
        // SAFETY: live directory fd, NUL-terminated component and read-only flags.
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                part.as_ptr(),
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK | directory,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: openat returned a new, owned fd.
        parent = unsafe { File::from_raw_fd(fd) };
    }
    if !parent.metadata()?.is_file() {
        return Err(invalid());
    }
    Ok(parent)
}
fn stamp(m: &fs::Metadata) -> (u64, u64, u64, i64, i64, i64, i64) {
    (
        m.dev(),
        m.ino(),
        m.len(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
    )
}
fn read(
    file: &File,
    offset: u64,
    count: usize,
    size: u64,
    cancel: &AtomicUsize,
) -> io::Result<Vec<u8>> {
    check(cancel)?;
    let before = file.metadata()?;
    if !before.is_file()
        || before.len() != size
        || offset.checked_add(count as u64).is_none_or(|n| n > size)
    {
        return Err(invalid());
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(count)
        .map_err(|_| io::Error::other("notice allocation limit"))?;
    bytes.resize(count, 0);
    let mut pos = 0;
    while pos < count {
        check(cancel)?;
        match file.read_at(&mut bytes[pos..], offset + pos as u64) {
            Ok(0) => return Err(invalid()),
            Ok(n) => pos += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    check(cancel)?;
    if stamp(&before) != stamp(&file.metadata()?) {
        return Err(invalid());
    }
    Ok(bytes)
}
#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Encoding {
    Utf8,
    Hex,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Page {
    offset: u64,
    bytes: usize,
    digest: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    name: String,
    bytes: u64,
    encoding: Encoding,
    pages: Vec<Page>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Index {
    format: u32,
    source_revision: String,
    target: String,
    files: Vec<Entry>,
}
impl Index {
    fn validate(&self) -> io::Result<u64> {
        if self.format != 1
            || self.source_revision.is_empty()
            || self.source_revision.len() > 128
            || self.target.is_empty()
            || self.target.len() > 128
            || self.files.is_empty()
            || self.files.len() > FILES
        {
            return Err(invalid());
        }
        let mut names = BTreeSet::new();
        let mut total = 0_u64;
        for f in &self.files {
            if !safe_name(&f.name)
                || !names.insert(&f.name)
                || f.bytes > TOTAL_BYTES
                || f.pages.is_empty()
                || f.pages.len() > (TOTAL_BYTES as usize / (CHUNK_BYTES - 3)) + 1
            {
                return Err(invalid());
            }
            total = total
                .checked_add(f.bytes)
                .filter(|n| *n <= TOTAL_BYTES)
                .ok_or_else(invalid)?;
            let mut end = 0_u64;
            for (i, p) in f.pages.iter().enumerate() {
                if p.offset != end
                    || p.bytes > CHUNK_BYTES
                    || !hash_valid(&p.digest)
                    || (i + 1 < f.pages.len() && p.bytes < CHUNK_BYTES - 3)
                    || (p.bytes == 0 && (f.bytes != 0 || f.pages.len() != 1))
                {
                    return Err(invalid());
                }
                end = end.checked_add(p.bytes as u64).ok_or_else(invalid)?;
            }
            if end != f.bytes {
                return Err(invalid());
            }
        }
        Ok(total)
    }
}

/// Development packager only. Original files are not rewritten; UTF-8 chunk
/// boundaries preserve every character, other files have an explicit hex view.
pub fn build_index(
    path: &Path,
    names: &[String],
    revision: &str,
    target: &str,
    cancel: &AtomicUsize,
) -> io::Result<Vec<u8>> {
    if names.is_empty() || names.len() > FILES {
        return Err(invalid());
    }
    let root = root(path)?;
    let mut index = Index {
        format: 1,
        source_revision: revision.into(),
        target: target.into(),
        files: Vec::new(),
    };
    let mut total = 0_u64;
    for name in names {
        check(cancel)?;
        if !safe_name(name) {
            return Err(invalid());
        }
        let file = relative(&root, name)?;
        let size = file.metadata()?.len();
        total = total
            .checked_add(size)
            .filter(|n| *n <= TOTAL_BYTES)
            .ok_or_else(invalid)?;
        let bytes = read(&file, 0, size as usize, size, cancel)?;
        let utf8 = std::str::from_utf8(&bytes).ok();
        let mut pages = Vec::new();
        let mut at = 0;
        loop {
            check(cancel)?;
            let mut end = (at + CHUNK_BYTES).min(bytes.len());
            if let Some(s) = utf8 {
                while !s.is_char_boundary(end) {
                    end -= 1;
                }
            }
            pages.push(Page {
                offset: at as u64,
                bytes: end - at,
                digest: digest(&bytes[at..end]),
            });
            at = end;
            if at == bytes.len() {
                break;
            }
        }
        index.files.push(Entry {
            name: name.clone(),
            bytes: size,
            encoding: if utf8.is_some() {
                Encoding::Utf8
            } else {
                Encoding::Hex
            },
            pages,
        });
    }
    index.validate()?;
    let bytes = serde_json::to_vec(&index).map_err(|_| invalid())?;
    if bytes.len() > INDEX_BYTES {
        return Err(invalid());
    }
    Ok(bytes)
}
pub struct Catalog {
    root: File,
    index: Index,
    id: String,
    total: u64,
}
#[derive(Serialize)]
pub struct FileInfo {
    pub id: usize,
    pub name: String,
    pub bytes: u64,
    pub pages: usize,
    pub encoding: Encoding,
}
#[derive(Serialize)]
pub struct Listing {
    pub index_id: String,
    pub start: usize,
    pub next: Option<usize>,
    pub total: usize,
    pub files: Vec<FileInfo>,
}
#[derive(Serialize)]
pub struct Chunk {
    pub index_id: String,
    pub file: FileInfo,
    pub page: usize,
    pub offset: u64,
    pub bytes: usize,
    pub text: String,
}
impl Catalog {
    pub fn open(
        path: &Path,
        expected: &str,
        revision: &str,
        target: &str,
        cancel: &AtomicUsize,
    ) -> io::Result<Self> {
        check(cancel)?;
        if !hash_valid(expected) {
            return Err(invalid());
        }
        let root = root(path)?;
        let file = relative(&root, INDEX_NAME)?;
        let size = file.metadata()?.len();
        if size > INDEX_BYTES as u64 {
            return Err(invalid());
        }
        let bytes = read(&file, 0, size as usize, size, cancel)?;
        if digest(&bytes) != expected {
            return Err(invalid());
        }
        let index: Index = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        let total = index.validate()?;
        if index.source_revision != revision || index.target != target {
            return Err(invalid());
        }
        Ok(Self {
            root,
            index,
            id: expected.into(),
            total,
        })
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn len(&self) -> usize {
        self.index.files.len()
    }
    pub fn is_empty(&self) -> bool {
        self.index.files.is_empty()
    }
    pub fn total_bytes(&self) -> u64 {
        self.total
    }
    fn info(&self, id: usize) -> io::Result<FileInfo> {
        let e = self.index.files.get(id).ok_or_else(invalid)?;
        Ok(FileInfo {
            id,
            name: e.name.clone(),
            bytes: e.bytes,
            pages: e.pages.len(),
            encoding: e.encoding,
        })
    }
    pub fn list(&self, start: usize) -> io::Result<Listing> {
        if start >= self.len() || !start.is_multiple_of(LIST_SIZE) {
            return Err(invalid());
        }
        let end = (start + LIST_SIZE).min(self.len());
        Ok(Listing {
            index_id: self.id.clone(),
            start,
            next: (end < self.len()).then_some(end),
            total: self.len(),
            files: (start..end)
                .map(|i| self.info(i))
                .collect::<io::Result<_>>()?,
        })
    }
    pub fn page(&self, id: usize, page: usize, cancel: &AtomicUsize) -> io::Result<Chunk> {
        let e = self.index.files.get(id).ok_or_else(invalid)?;
        let p = e.pages.get(page).ok_or_else(invalid)?;
        check(cancel)?;
        let file = relative(&self.root, &e.name)?;
        let raw = read(&file, p.offset, p.bytes, e.bytes, cancel)?;
        if digest(&raw) != p.digest {
            return Err(invalid());
        }
        let text = match e.encoding {
            Encoding::Utf8 => String::from_utf8(raw).map_err(|_| invalid())?,
            Encoding::Hex => {
                use std::fmt::Write;
                let mut text = String::with_capacity(raw.len() * 3);
                for (i, b) in raw.iter().enumerate() {
                    write!(
                        &mut text,
                        "{b:02x}{}",
                        if (i + 1).is_multiple_of(16) {
                            '\n'
                        } else {
                            ' '
                        }
                    )
                    .unwrap();
                }
                text
            }
        };
        check(cancel)?;
        Ok(Chunk {
            index_id: self.id.clone(),
            file: self.info(id)?,
            page,
            offset: p.offset,
            bytes: p.bytes,
            text,
        })
    }
}

#[cfg(test)]
mod tests;
