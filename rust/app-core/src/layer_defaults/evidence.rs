//! Immutable, inode-qualified publication evidence for filesystems without
//! user xattrs. Publish this record durably BEFORE the payload; an old target
//! never reads a new inode's record. No mutable two-file commit or directory scan.
use super::*;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::{collections::BTreeMap, io::Seek};

const MAX_BYTES: usize = 64 * 1024;
pub(crate) enum Kind {
    Default,
    Review,
}

fn key(name: &CStr) -> Result<&str> {
    let s = name.to_str().map_err(|_| conflict())?;
    let key = s
        .strip_prefix("com.floe.")
        .or_else(|| s.strip_prefix("user.floe."));
    match key {
        Some(k @ ("review-pack-v1" | "review-stage-v1" | "default-stage-v1")) => Ok(k),
        _ => Err(Error::input("not an owned publication attribute")),
    }
}
fn name(target: &CStr, inode: u64) -> CString {
    // SHA-1 is a bounded filename/change detector, not a signature. The full
    // target, inode, length and digest are all checked inside the record too.
    CString::new(format!(
        ".floe-meta-{:x}-{inode:016x}.json",
        Sha1::digest(target.to_bytes())
    ))
    .unwrap()
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    v: u8,
    target: Vec<u8>,
    inode: u64,
    len: u64,
    digest: [u8; 20],
    attributes: BTreeMap<String, Vec<u8>>,
}
struct Captured {
    stamp: Stamp,
    bytes: Vec<u8>,
    record: Record,
}
pub(crate) struct Evidence {
    directory: Arc<Directory>,
    name: CString,
    captured: Option<Box<Captured>>,
}
impl Evidence {
    pub(crate) fn read(
        directory: Arc<Directory>,
        target: &CStr,
        file: &File,
        digest: [u8; 20],
        security: &Security,
        kind: Kind,
    ) -> Result<Self> {
        let m = file.metadata()?;
        let name = name(target, m.ino());
        let captured = capture(&directory, &name)?;
        if let Some(c) = &captured {
            let r = &c.record;
            let complete = match kind {
                Kind::Default => {
                    r.attributes.len() == 1 && r.attributes.contains_key("default-stage-v1")
                }
                Kind::Review => {
                    r.attributes.len() == 2
                        && r.attributes.contains_key("review-pack-v1")
                        && r.attributes.contains_key("review-stage-v1")
                }
            };
            if r.v != 1
                || r.target != target.to_bytes()
                || r.inode != m.ino()
                || r.len != m.len()
                || r.digest != digest
                || !complete
            {
                return Err(Error::input("publication evidence does not match file"));
            }
            for (k, value) in &r.attributes {
                #[cfg(target_os = "macos")]
                let attr = CString::new(format!("com.floe.{k}"));
                #[cfg(not(target_os = "macos"))]
                let attr = CString::new(format!("user.floe.{k}"));
                let attr = attr.map_err(|_| conflict())?;
                key(&attr)?;
                if security.attribute(&attr).is_some_and(|v| v != value) {
                    return Err(Error::input("xattr and companion evidence disagree"));
                }
            }
        }
        Ok(Self {
            directory,
            name,
            captured,
        })
    }
    pub(crate) fn attribute(&self, attr: &CStr) -> Option<&[u8]> {
        self.captured
            .as_ref()?
            .record
            .attributes
            .get(key(attr).ok()?)
            .map(Vec::as_slice)
    }
    pub(crate) fn same(&self, other: &Self) -> bool {
        match (&self.captured, &other.captured) {
            (None, None) => true,
            (Some(a), Some(b)) => a.stamp == b.stamp && a.bytes == b.bytes,
            _ => false,
        }
    }
    pub(crate) fn unchanged(&self) -> Result<()> {
        let now = capture(&self.directory, &self.name)?;
        match (&self.captured, now) {
            (None, None) => Ok(()),
            (Some(a), Some(b)) if a.stamp == b.stamp && a.bytes == b.bytes => Ok(()),
            _ => Err(conflict()),
        }
    }
    /// Only a record from this exact snapshot, after payload replacement or
    /// known precommit cancellation. Never scan or clean unknown publications.
    pub(crate) fn retire(&self, protect: impl FnOnce(&Path) -> Result<()>) {
        if self.captured.is_some() && protect(&self.path()).is_ok() && self.unchanged().is_ok() {
            // SAFETY: exact held directory and validated owned leaf.
            unsafe {
                libc::unlinkat(self.directory.file.as_raw_fd(), self.name.as_ptr(), 0);
            }
        }
    }
    pub(crate) fn path(&self) -> PathBuf {
        self.directory
            .path
            .join(std::ffi::OsStr::from_bytes(self.name.to_bytes()))
    }
}

#[cfg(test)]
#[path = "evidence_tests.rs"]
mod tests;
fn capture(directory: &Directory, name: &CStr) -> Result<Option<Box<Captured>>> {
    directory.validate()?;
    let file = match directory.open_leaf(name, libc::O_RDONLY, 0) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let m = file.metadata()?;
    if !m.is_file() || m.nlink() != 1 || m.len() > MAX_BYTES as u64 || m.mode() & 0o7000 != 0 {
        return Err(Error::input("invalid publication evidence file"));
    }
    let stamp = Stamp::of(&m);
    let mut bytes = Vec::new();
    (&file).take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_BYTES
        || Stamp::of(&file.metadata()?) != stamp
        || directory.leaf_identity(name)? != identity(&m)
    {
        return Err(conflict());
    }
    let record = serde_json::from_slice(&bytes)
        .map_err(|_| Error::input("invalid publication evidence contents"))?;
    Ok(Some(Box::new(Captured {
        stamp,
        bytes,
        record,
    })))
}

impl Stage {
    pub(crate) fn set_owned(&mut self, attr: &CStr, value: &[u8]) -> Result<()> {
        let k = key(attr)?;
        if security::set_owned(&self.file, attr, value)? {
            if Security::read(&self.file)?.attribute(attr) != Some(value) {
                return Err(unsupported("cannot preserve publication attribute"));
            }
        } else {
            self.needs_evidence = true;
        }
        self.owned.insert(k.to_owned(), value.to_vec());
        Ok(())
    }
    pub(crate) fn seal_evidence(
        &mut self,
        target: &CStr,
        protect: impl Fn(&Path) -> Result<()>,
        stop: &AtomicUsize,
    ) -> Result<Option<[u8; 20]>> {
        if !self.needs_evidence {
            return Ok(None);
        }
        let m = self.file.metadata()?;
        let name = name(target, m.ino());
        let path = self
            .directory
            .path
            .join(std::ffi::OsStr::from_bytes(name.to_bytes()));
        protect(&path)?;
        let mut hash = Sha1::new();
        self.file.rewind()?;
        let mut left = m.len();
        let mut buf = [0u8; 65536];
        while left != 0 {
            check_cancelled(stop)?;
            let want = left.min(buf.len() as u64) as usize;
            let n = self.file.read(&mut buf[..want])?;
            if n == 0 {
                return Err(conflict());
            }
            hash.update(&buf[..n]);
            left -= n as u64;
        }
        if Stamp::of(&self.file.metadata()?) != Stamp::of(&m) {
            return Err(conflict());
        }
        let digest = hash.finalize().into();
        let bytes = serde_json::to_vec(&Record {
            v: 1,
            target: target.to_bytes().to_vec(),
            inode: m.ino(),
            len: m.len(),
            digest,
            attributes: self.owned.clone(),
        })
        .map_err(|_| conflict())?;
        if bytes.len() > MAX_BYTES {
            return Err(unsupported("publication evidence exceeds limit"));
        }
        check_cancelled(stop)?;
        // O_EXCL never changes an old record. A partial record is not reachable
        // via the old target because it belongs to the still-private new inode.
        let mut file =
            self.directory
                .open_leaf(&name, libc::O_RDWR | libc::O_CREAT | libc::O_EXCL, 0o600)?;
        let id = identity(&file.metadata()?);
        let result = (|| {
            Security::private(&file)?;
            file.write_all(&bytes)?;
            // Readers with payload access must also be able to read its record.
            Security::read(&self.file)?.apply(&file)?;
            file.sync_all()?;
            self.directory.file.sync_all()?;
            let e = Evidence::read(
                Arc::clone(&self.directory),
                target,
                &self.file,
                digest,
                &Security::read(&self.file)?,
                if self.owned.contains_key("review-pack-v1") {
                    Kind::Review
                } else {
                    Kind::Default
                },
            )?;
            if e.captured.as_ref().map(|c| c.stamp.id) != Some(id) {
                return Err(conflict());
            }
            e.unchanged()?;
            Ok(e)
        })();
        match result {
            Ok(e) => {
                self.evidence = Some(e);
                self.sealed_stamp = Some(Stamp::of(&m));
                Ok(Some(digest))
            }
            Err(error) => {
                if protect(&path).is_ok() && self.directory.leaf_identity(&name).ok() == Some(id) {
                    // SAFETY: only the inode created by this O_EXCL attempt.
                    unsafe {
                        libc::unlinkat(self.directory.file.as_raw_fd(), name.as_ptr(), 0);
                    }
                }
                Err(error)
            }
        }
    }
}
