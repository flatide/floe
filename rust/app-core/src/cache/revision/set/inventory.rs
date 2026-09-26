//! Bounded, read-only accounting. No directory from a manifest is followed.
//! A row is an observation, NEVER authority to reclaim it later. Reclamation
//! must independently lock/revalidate the current pointer, ownership and pins.
use super::*;
use crate::registered::RegisteredSource;

const MAX_ENTRIES: usize = 4096;
const MAX_ROWS: usize = 256;
const MAX_STORES: usize = 256;
const MAX_SEAL_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Serialize)]
pub struct Inventory {
    /// Decimal string: JSON must not round byte counts above 2^53.
    pub logical_bytes: String,
    pub rows: Vec<Entry>,
    pub stores_scanned: usize,
    pub unknown_entries: usize,
    pub unavailable_entries: usize,
    pub partial: bool,
}
#[derive(Debug, Serialize)]
pub struct Entry {
    pub kind: &'static str,
    /// Stable ordinal in this registered source's sorted dependency list;
    /// filesystem paths are deliberately absent from the wire representation.
    pub source_number: usize,
    pub revision: String,
    pub logical_bytes: String,
    pub format: Option<u32>,
    pub current: bool,
    pub current_unknown: bool,
    pub seal: &'static str,
    pub readers: &'static str,
    pub owner: &'static str,
    pub set_revision: Option<String>,
    pub members: Option<usize>,
    pub extra_entries: bool,
}
struct Scan {
    result: Inventory,
    bytes: u64,
    remaining: usize,
    seal_bytes_left: u64,
}
impl Scan {
    fn add_bytes(&mut self, n: u64) -> Result<()> {
        self.bytes = self
            .bytes
            .checked_add(n)
            .ok_or_else(|| invalid("inventory size overflow"))?;
        Ok(())
    }
    fn names(&mut self, path: &Path, stop: &AtomicUsize) -> Result<Vec<std::ffi::OsString>> {
        let mut names = Vec::new();
        for entry in fs::read_dir(path)? {
            check_cancelled(stop)?;
            if self.remaining == 0 {
                self.result.partial = true;
                break;
            }
            self.remaining -= 1;
            names.push(entry?.file_name());
        }
        names.sort();
        Ok(names)
    }
    fn payload(&mut self, path: &Path, is_set: bool, stop: &AtomicUsize) -> Result<(u64, bool)> {
        let mut bytes = 0u64;
        let mut extra = false;
        for name in self.names(path, stop)? {
            let known = name == "revision.json"
                || (!is_set && name.to_str().is_some_and(|n| FILES.contains(&n)));
            match fs::symlink_metadata(path.join(&name)) {
                Ok(m) if m.is_file() && m.nlink() == 1 => {
                    self.add_bytes(m.len())?;
                    bytes = bytes
                        .checked_add(m.len())
                        .ok_or_else(|| invalid("inventory size overflow"))?;
                }
                Ok(_) => {
                    // No symlink, hardlink or nested-directory traversal.
                    extra = true;
                    self.result.partial = true;
                    self.result.unavailable_entries += 1;
                }
                Err(_) => {
                    extra = true;
                    self.result.unavailable_entries += 1;
                    self.result.partial = true;
                }
            }
            if !known {
                self.result.unknown_entries += 1;
                extra = true;
            }
        }
        Ok((bytes, extra))
    }
    fn store(
        &mut self,
        store: &super::super::Store,
        is_set: bool,
        source_number: usize,
        registered: &RegisteredSource,
        expected: &BTreeSet<PathBuf>,
        stop: &AtomicUsize,
    ) -> Result<()> {
        check_cancelled(stop)?;
        if self.remaining == 0 || self.result.rows.len() >= MAX_ROWS {
            self.result.partial = true;
            return Ok(());
        }
        let root = match directory(store.path()) {
            Ok(root) => root,
            Err(_)
                if fs::symlink_metadata(store.path())
                    .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
            {
                return Ok(())
            }
            Err(e) => return Err(e),
        };
        self.result.stores_scanned += 1;
        let limit = if is_set { MAX_SET_BYTES } else { MAX_RECORD };
        let before = current_limit(store.path(), limit);
        let current = before
            .as_ref()
            .ok()
            .and_then(|bytes| bytes.as_ref())
            .and_then(|bytes| {
                if is_set {
                    let m: Manifest = serde_json::from_slice(bytes).ok()?;
                    m.validate(&store.source).ok()?;
                    Some(m.revision)
                } else {
                    let r: Record = serde_json::from_slice(bytes).ok()?;
                    r.validate(&store.source).ok()?;
                    Some(r.revision)
                }
            });
        let current_unknown =
            before.is_err() || (matches!(&before, Ok(Some(_))) && current.is_none());
        for name in self.names(store.path(), stop)? {
            check_cancelled(stop)?;
            let path = store.path().join(&name);
            let Some(revision) = name.to_str().filter(|s| valid_id(s)) else {
                // Pointer/pending evidence is accounted but never interpreted as
                // a revision path. Unknown root entries are not traversed.
                let known = name == "current.json"
                    || name.to_str().is_some_and(|n| {
                        n.strip_prefix(".current-")
                            .and_then(|s| s.strip_suffix(".tmp"))
                            .is_some_and(valid_id)
                    });
                match fs::symlink_metadata(&path) {
                    Ok(m) if m.is_file() && m.nlink() == 1 => self.add_bytes(m.len())?,
                    _ => {
                        self.result.unavailable_entries += 1;
                        self.result.partial = true;
                    }
                }
                if !known {
                    self.result.unknown_entries += 1;
                    self.result.partial = true;
                }
                continue;
            };
            if self.result.rows.len() >= MAX_ROWS || self.remaining == 0 {
                self.result.partial = true;
                break;
            }
            let dir = match directory(&path) {
                Ok(dir) => dir,
                Err(_) => {
                    self.result.unavailable_entries += 1;
                    self.result.partial = true;
                    continue;
                }
            };
            let (bytes, extra) = self.payload(&path, is_set, stop)?;
            let mut row = Entry {
                kind: if is_set { "set" } else { "source" },
                source_number,
                revision: revision.to_owned(),
                logical_bytes: bytes.to_string(),
                format: None,
                current: current.as_deref() == Some(revision),
                current_unknown,
                seal: "invalid",
                readers: "not_checked",
                owner: "unknown",
                set_revision: None,
                members: None,
                extra_entries: extra,
            };
            let seal = path.join("revision.json");
            let mut seal_limit = false;
            let raw = regular(&seal, false).and_then(|f| {
                let len = f.metadata()?.len();
                if len > self.seal_bytes_left {
                    seal_limit = true;
                    self.result.partial = true;
                    return Err(invalid("inventory seal read limit"));
                }
                self.seal_bytes_left -= len;
                record_bytes_limit(f, limit)
            });
            if let Ok(raw) = raw {
                if is_set {
                    if let Ok(m) = serde_json::from_slice::<Manifest>(&raw) {
                        if m.revision == revision
                            && m.validate(&store.source)
                                .is_ok_and(|members| members.is_subset(expected))
                        {
                            row.format = Some(m.version);
                            row.seal = "valid";
                            row.owner = "this_dataset";
                            row.members = Some(m.members.len());
                        }
                    }
                } else if let Ok(r) = serde_json::from_slice::<Record>(&raw) {
                    if r.revision == revision
                        && r.validate(&store.source).is_ok()
                        && files(&path, false).is_ok_and(|files| files == r.files)
                    {
                        row.format = Some(r.version);
                        row.seal = "valid";
                        match r.owner {
                            Some(owner) if owner.source == registered.path() => {
                                row.owner = "this_dataset";
                                row.set_revision = Some(owner.revision);
                            }
                            Some(_) => row.owner = "other_dataset",
                            None => row.owner = "standalone_or_legacy",
                        }
                    }
                }
            } else if seal_limit {
                row.seal = "not_checked_limit";
            } else if fs::symlink_metadata(seal)
                .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
            {
                row.seal = "unsealed";
            }
            // Never infer absence of readers for v1: old executables did not
            // participate in leases. Unknown formats are protected as well.
            if row.format == Some(2) {
                row.readers = match dir.try_lock() {
                    Ok(()) => "idle_at_scan",
                    Err(std::fs::TryLockError::WouldBlock) => "in_use",
                    Err(std::fs::TryLockError::Error(_)) => "unavailable",
                };
            } else if row.format == Some(1) {
                row.readers = "legacy_untracked";
            }
            // Closing this independent fd releases only our probe lock.
            self.result.rows.push(row);
        }
        if id(&directory(store.path())?)? != id(&root)? {
            return Err(Error::new(
                ErrorKind::Busy,
                "revision store changed during inventory",
            ));
        }
        if let Ok(before) = before {
            if current_limit(store.path(), limit)? != before {
                return Err(Error::new(
                    ErrorKind::Busy,
                    "current changed during inventory; refresh",
                ));
            }
        }
        Ok(())
    }
}

/// No store/lock creation, geometry decoding, source-path exposure or deletion.
/// Totals are logical file lengths, not allocated space or reclaimable bytes.
pub fn inspect(source: &RegisteredSource, stop: &AtomicUsize) -> Result<Inventory> {
    source.validate(stop)?;
    let expected = source.selected_sources(None, stop)?;
    let mut scan = Scan {
        result: Inventory {
            logical_bytes: "0".into(),
            rows: Vec::new(),
            stores_scanned: 0,
            unknown_entries: 0,
            unavailable_entries: 0,
            partial: false,
        },
        bytes: 0,
        remaining: MAX_ENTRIES,
        seal_bytes_left: MAX_SEAL_BYTES,
    };
    let set = Store::new(source.path())?;
    source.scoped_output(set.path())?;
    scan.store(&set.raw, true, 0, source, &expected, stop)?;
    for (i, path) in expected.iter().enumerate() {
        if i + 1 >= MAX_STORES || scan.remaining == 0 || scan.result.rows.len() >= MAX_ROWS {
            scan.result.partial = true;
            break;
        }
        source.validate_revision_member(path, stop)?;
        scan.store(
            &super::super::Store::new(path)?,
            false,
            i + 1,
            source,
            &expected,
            stop,
        )?;
    }
    source.validate(stop)?;
    scan.result.logical_bytes = scan.bytes.to_string();
    Ok(scan.result)
}

#[cfg(test)]
mod tests;
