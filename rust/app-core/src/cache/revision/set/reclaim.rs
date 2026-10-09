//! Explicit, bounded retirement of one inactive, published v3 set. No age GC,
//! recursive deletion, legacy cleanup, arbitrary paths or automatic retries.
//! An immutable journal is durable before the first unlink; interrupted work
//! can only continue after a fresh read-only preview and explicit approval.
use super::*;
use crate::registered::RegisteredSource;
use std::time::{Duration, Instant};
mod local;

const MAX_TARGETS: usize = 1024;
const MAX_JOURNAL: u64 = 32 * 1024 * 1024;
const PREVIEW_TTL: Duration = Duration::from_secs(300);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    source: PathBuf,
    revision: String,
    root_id: (u64, u64),
    dir_id: (u64, u64),
    files: BTreeMap<String, Stamp>,
    record: Option<Record>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    version: u32,
    manifest: Manifest,
    targets: Vec<Target>, // set first, then independently derived source stores
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Observed {
    current: BTreeMap<PathBuf, Option<Vec<u8>>>,
    remaining: Vec<BTreeSet<String>>,
    directories: Vec<bool>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Summary {
    pub token: String,
    pub revision: String,
    pub logical_bytes: String,
    pub files: usize,
    pub sources: usize,
    pub recovery: bool,
    pub complete: bool,
    pub expires_in_s: u64,
}
pub struct Prepared {
    summary: Summary,
    source: Arc<RegisteredSource>,
    journal: Journal,
    journal_stamp: Option<Stamp>,
    observed: Observed,
    created: Instant,
}
#[derive(Debug, Serialize)]
pub struct Outcome {
    pub revision: String,
    pub status: &'static str,
    pub removed_files: usize,
    pub removed_logical_bytes: String,
    pub sync_warning: bool,
}
struct Guards {
    _writers: Vec<WriteLease>,
    roots: Vec<File>,
    _directories: Vec<File>,
}

fn journal_path(store: &Store, revision: &str) -> PathBuf {
    store.path().join(format!(".reclaim-{revision}.json"))
}
fn target_store(target: &Target, is_set: bool) -> Result<super::super::Store> {
    Ok(if is_set {
        Store::new(&target.source)?.raw
    } else {
        super::super::Store::new(&target.source)?
    })
}
fn absent(path: &Path) -> bool {
    fs::symlink_metadata(path).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
}
fn lock_dir(path: &Path) -> Result<File> {
    let dir = directory(path)?;
    match dir.try_lock() {
        Ok(()) => Ok(dir),
        Err(std::fs::TryLockError::WouldBlock) => {
            Err(Error::new(ErrorKind::Busy, "revision is in use"))
        }
        Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
    }
}
fn bounded_names(path: &Path) -> Result<BTreeSet<String>> {
    let mut names = BTreeSet::new();
    for entry in fs::read_dir(path)? {
        let name = entry?
            .file_name()
            .into_string()
            .map_err(|_| invalid("unknown revision entry"))?;
        if names.len() >= FILES.len() + 2 {
            return Err(invalid("extra revision entries are protected"));
        }
        names.insert(name);
    }
    Ok(names)
}
fn stamp_files(path: &Path, names: &BTreeSet<String>) -> Result<BTreeMap<String, Stamp>> {
    names
        .iter()
        .map(|n| {
            Ok((
                n.clone(),
                Stamp::of(&regular(&path.join(n), false)?.metadata()?),
            ))
        })
        .collect()
}

impl Journal {
    fn validate(
        &self,
        source: &RegisteredSource,
        revision: &str,
        stop: &AtomicUsize,
    ) -> Result<()> {
        if self.version != 1
            || self.manifest.version != 3
            || self.manifest.revision != revision
            || self.targets.is_empty()
            || self.targets.len() > MAX_TARGETS + 1
        {
            return Err(invalid("unsupported reclamation journal"));
        }
        let expected = source.selected_sources(self.manifest.levels.as_ref(), stop)?;
        if self.manifest.validate(source.path())? != expected
            || self.targets.len() != expected.len() + 1
        {
            return Err(invalid("reclamation source selection changed"));
        }
        let first = &self.targets[0];
        if first.source != source.path()
            || first.revision != revision
            || first.record.is_some()
            || first
                .files
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>()
                != ["published.json", "revision.json"].into()
        {
            return Err(invalid("invalid set reclamation target"));
        }
        let mut seen = BTreeSet::new();
        for target in &self.targets[1..] {
            check_cancelled(stop)?;
            if !expected.contains(&target.source) || !seen.insert(target.source.clone()) {
                return Err(invalid("unregistered or duplicate reclamation member"));
            }
            source.validate_revision_member(&target.source, stop)?;
            let record = target
                .record
                .as_ref()
                .ok_or_else(|| invalid("missing source seal"))?;
            record.validate(&target.source)?;
            validate_owner(&self.manifest, record)?;
            if target.revision != record.revision
                || !self
                    .manifest
                    .members
                    .iter()
                    .any(|m| m.source == target.source && m.revision == target.revision)
                || target.files.len() != record.files.len() + 1
                || !target.files.contains_key("revision.json")
                || record
                    .files
                    .iter()
                    .any(|(name, stamp)| target.files.get(name) != Some(stamp))
            {
                return Err(invalid("source reclamation file set mismatch"));
            }
        }
        Ok(())
    }

    fn observe(
        &self,
        source: &RegisteredSource,
        recovery: bool,
        stop: &AtomicUsize,
    ) -> Result<(Guards, Observed)> {
        self.validate(source, &self.manifest.revision, stop)?;
        let stores = self
            .targets
            .iter()
            .enumerate()
            .map(|(i, t)| target_store(t, i == 0))
            .collect::<Result<Vec<_>>>()?;
        let ordered: BTreeSet<_> = stores.iter().map(|s| s.path().to_owned()).collect();
        let writers = ordered
            .iter()
            .map(|p| WriteLease::acquire_existing(p))
            .collect::<Result<Vec<_>>>()?;
        let mut guards = Guards {
            _writers: writers,
            roots: Vec::new(),
            _directories: Vec::new(),
        };
        let mut observed = Observed {
            current: BTreeMap::new(),
            remaining: Vec::new(),
            directories: Vec::new(),
        };
        for (i, (target, store)) in self.targets.iter().zip(&stores).enumerate() {
            check_cancelled(stop)?;
            let root = directory(store.path())?;
            local::require(&root)?;
            if id(&root)? != target.root_id {
                return Err(invalid("reclamation store changed"));
            }
            let bytes = current_limit(
                store.path(),
                if i == 0 { MAX_SET_BYTES } else { MAX_RECORD },
            )?;
            if i == 0 {
                let bytes = bytes
                    .as_ref()
                    .ok_or_else(|| invalid("missing current set; protected"))?;
                let current: Manifest =
                    serde_json::from_slice(bytes).map_err(|_| invalid("invalid current set"))?;
                current.validate(source.path())?;
                if current.revision == target.revision {
                    return Err(Error::new(ErrorKind::Busy, "current revision is protected"));
                }
                // Legitimate v3 builds never share owned members across sets.
                // Still protect an explicit current reference if a damaged
                // pointer+seal agree with each other but violate that invariant.
                let retiring: BTreeSet<_> = self.targets[1..]
                    .iter()
                    .map(|t| (&t.source, &t.revision))
                    .collect();
                if current
                    .members
                    .iter()
                    .any(|m| retiring.contains(&(&m.source, &m.revision)))
                {
                    return Err(Error::new(
                        ErrorKind::Busy,
                        "a current set member is protected",
                    ));
                }
                let dir = store.path().join(&current.revision);
                let _directory = directory(&dir)?;
                if record_bytes_limit(regular(&dir.join("revision.json"), false)?, MAX_SET_BYTES)?
                    != *bytes
                {
                    return Err(invalid("current set seal mismatch"));
                }
            } else if let Some(bytes) = &bytes {
                let current: Record =
                    serde_json::from_slice(bytes).map_err(|_| invalid("invalid source current"))?;
                current.validate(&target.source)?;
                if current.revision == target.revision {
                    return Err(Error::new(ErrorKind::Busy, "source current is protected"));
                }
                let dir = store.path().join(&current.revision);
                let _directory = directory(&dir)?;
                if read_record(&dir.join("revision.json"))? != *bytes {
                    return Err(invalid("source current seal mismatch"));
                }
            }
            observed.current.insert(store.path().to_owned(), bytes);
            if !absent(
                &store
                    .path()
                    .join(format!(".current-{}.tmp", target.revision)),
            ) {
                return Err(invalid("publication outcome evidence is protected"));
            }
            let path = store.path().join(&target.revision);
            let exists = !absent(&path);
            observed.directories.push(exists);
            if exists {
                let dir = lock_dir(&path)?;
                if id(&dir)? != target.dir_id {
                    return Err(invalid("reclamation directory changed"));
                }
                let names = bounded_names(&path)?;
                if (!recovery && names != target.files.keys().cloned().collect())
                    || names.iter().any(|n| !target.files.contains_key(n))
                {
                    return Err(invalid("reclamation contains extra/missing files"));
                }
                if stamp_files(&path, &names)?
                    .iter()
                    .any(|(n, s)| target.files.get(n) != Some(s))
                {
                    return Err(invalid("reclamation files changed"));
                }
                observed.remaining.push(names);
                guards._directories.push(dir);
            } else if recovery {
                observed.remaining.push(BTreeSet::new());
            } else {
                return Err(invalid("reclamation directory disappeared"));
            }
            guards.roots.push(root);
        }
        source.validate(stop)?;
        Ok((guards, observed))
    }
}

/// No creation or deletion. Locks are short-lived probes, released on return.
/// A persisted journal changes this into a recovery preview, never auto-resume.
pub fn prepare(
    source: Arc<RegisteredSource>,
    revision: &str,
    stop: &AtomicUsize,
) -> Result<Prepared> {
    if !valid_id(revision) {
        return Err(Error::input("invalid revision ID"));
    }
    source.validate(stop)?;
    let store = Store::new(source.path())?;
    source.scoped_output(store.path())?;
    let path = journal_path(&store, revision);
    let (journal, journal_stamp) = if absent(&path) {
        (new_journal(&store, &source, revision, stop)?, None)
    } else {
        let f = regular(&path, false)?;
        let stamp = Stamp::of(&f.metadata()?);
        let bytes = record_bytes_limit(f, MAX_JOURNAL)?;
        let journal = serde_json::from_slice::<Journal>(&bytes)
            .map_err(|_| invalid("invalid reclamation journal; no automatic repair"))?;
        (journal, Some(stamp))
    };
    let (_guards, observed) = journal.observe(&source, journal_stamp.is_some(), stop)?;
    if let Some(stamp) = &journal_stamp {
        if Stamp::of(&regular(&path, false)?.metadata()?) != *stamp {
            return Err(invalid("reclamation journal changed"));
        }
    } else if !absent(&path) {
        return Err(Error::new(
            ErrorKind::Busy,
            "another reclamation was prepared",
        ));
    }
    let (files, bytes) = remaining_totals(&journal, &observed)?;
    let mut token = [0; 32];
    getrandom::fill(&mut token)
        .map_err(|_| Error::new(ErrorKind::Io, "reclamation entropy unavailable"))?;
    let summary = Summary {
        token: token.iter().map(|b| format!("{b:02x}")).collect(),
        revision: revision.into(),
        logical_bytes: bytes.to_string(),
        files,
        sources: journal.targets.len() - 1,
        recovery: journal_stamp.is_some(),
        complete: observed.directories.iter().all(|v| !v),
        expires_in_s: PREVIEW_TTL.as_secs(),
    };
    Ok(Prepared {
        summary,
        source,
        journal,
        journal_stamp,
        observed,
        created: Instant::now(),
    })
}

fn new_journal(
    store: &Store,
    source: &RegisteredSource,
    revision: &str,
    stop: &AtomicUsize,
) -> Result<Journal> {
    let path = store.path().join(revision);
    let _dir = directory(&path)?;
    let bytes = record_bytes_limit(regular(&path.join("revision.json"), false)?, MAX_SET_BYTES)?;
    let manifest: Manifest =
        serde_json::from_slice(&bytes).map_err(|_| invalid("invalid target set"))?;
    let expected = source.selected_sources(manifest.levels.as_ref(), stop)?;
    if manifest.version != 3
        || manifest.revision != revision
        || manifest.validate(source.path())? != expected
        || expected.len() > MAX_TARGETS
    {
        return Err(Error::new(
            ErrorKind::Unsupported,
            "only bounded, published v3 sets can be reclaimed",
        ));
    }
    if record_bytes_limit(regular(&path.join("published.json"), false)?, MAX_SET_BYTES)? != bytes {
        return Err(invalid("publication witness is missing or invalid"));
    }
    let names: BTreeSet<String> = ["revision.json".into(), "published.json".into()].into();
    let mut targets = vec![Target {
        source: source.path().into(),
        revision: revision.into(),
        root_id: id(&directory(store.path())?)?,
        dir_id: id(&directory(&path)?)?,
        files: stamp_files(&path, &names)?,
        record: None,
    }];
    for member in &manifest.members {
        check_cancelled(stop)?;
        source.validate_revision_member(&member.source, stop)?;
        let raw = super::super::Store::new(&member.source)?;
        let path = raw.path().join(&member.revision);
        let dir = directory(&path)?;
        let record: Record = serde_json::from_slice(&read_record(&path.join("revision.json"))?)
            .map_err(|_| invalid("invalid source seal"))?;
        record.validate(&member.source)?;
        validate_owner(&manifest, &record)?;
        if record.revision != member.revision || files(&path, false)? != record.files {
            return Err(invalid("source file set changed"));
        }
        let mut stamps = record.files.clone();
        stamps.insert(
            "revision.json".into(),
            Stamp::of(&regular(&path.join("revision.json"), false)?.metadata()?),
        );
        targets.push(Target {
            source: member.source.clone(),
            revision: member.revision.clone(),
            root_id: id(&directory(raw.path())?)?,
            dir_id: id(&dir)?,
            files: stamps,
            record: Some(record),
        });
    }
    Ok(Journal {
        version: 1,
        manifest,
        targets,
    })
}
fn remaining_totals(journal: &Journal, observed: &Observed) -> Result<(usize, u64)> {
    let mut files = 0;
    let mut bytes = 0u64;
    for (target, names) in journal.targets.iter().zip(&observed.remaining) {
        files += names.len();
        for name in names {
            bytes = bytes
                .checked_add(target.files[name].len)
                .ok_or_else(|| invalid("reclamation size overflow"))?;
        }
    }
    Ok((files, bytes))
}

impl Prepared {
    pub fn summary(&self) -> &Summary {
        &self.summary
    }

    /// Caller must obtain explicit approval for this one preview token. A
    /// consumed preview cannot be retried; only durable-journal recovery can.
    pub fn execute(self, stop: &AtomicUsize) -> Result<Outcome> {
        self.execute_with(stop, |_, _| Ok(()))
    }
    fn execute_with(
        self,
        stop: &AtomicUsize,
        mut before_unlink: impl FnMut(usize, &Path) -> std::io::Result<()>,
    ) -> Result<Outcome> {
        check_cancelled(stop)?;
        if self.created.elapsed() > PREVIEW_TTL {
            return Err(Error::new(ErrorKind::Busy, "reclamation preview expired"));
        }
        let (guards, observed) =
            self.journal
                .observe(&self.source, self.journal_stamp.is_some(), stop)?;
        if observed != self.observed {
            return Err(Error::new(
                ErrorKind::Busy,
                "reclamation preview changed; prepare again",
            ));
        }
        let store = Store::new(self.source.path())?;
        let path = journal_path(&store, &self.journal.manifest.revision);
        if let Some(stamp) = self.journal_stamp {
            if Stamp::of(&regular(&path, false)?.metadata()?) != stamp {
                return Err(invalid("reclamation journal changed"));
            }
        } else {
            let bytes = serde_json::to_vec(&self.journal)
                .map_err(|_| invalid("cannot encode reclamation journal"))?;
            create_record_limit(&path, &bytes, MAX_JOURNAL)?;
        }
        // A previous attempt may have left a complete-looking journal after
        // either fsync failed. Recovery must make both durable again, too.
        regular(&path, false)?.sync_all()?;
        guards.roots[0].sync_all()?; // Nothing removed until both fsyncs succeed.
        let mut outcome = Outcome {
            revision: self.summary.revision,
            status: "complete",
            removed_files: 0,
            removed_logical_bytes: "0".into(),
            sync_warning: false,
        };
        let mut removed = 0u64;
        // Delete sources before the set seal. The journal, outside these
        // directories, survives every partial state; there is no rollback claim.
        for i in (0..self.journal.targets.len()).rev() {
            if !observed.directories[i] {
                continue;
            }
            let target = &self.journal.targets[i];
            let raw = target_store(target, i == 0)?;
            let dir = raw.path().join(&target.revision);
            let mut names: Vec<_> = observed.remaining[i].iter().collect();
            names.sort_by_key(|n| (n.as_str() == "revision.json", *n));
            for name in names {
                if check_cancelled(stop).is_err() {
                    outcome.status = "interrupted";
                    return Ok(outcome);
                }
                let file = dir.join(name);
                let checked = (|| {
                    if id(&directory(&dir)?)? != target.dir_id
                        || Stamp::of(&regular(&file, false)?.metadata()?) != target.files[name]
                    {
                        return Err(invalid("reclamation file changed before unlink"));
                    }
                    Ok(())
                })();
                if checked.is_err() {
                    outcome.status = "interrupted";
                    return Ok(outcome);
                }
                if before_unlink(outcome.removed_files, &file)
                    .and_then(|()| fs::remove_file(&file))
                    .is_err()
                {
                    outcome.status = "outcome_unknown";
                    return Ok(outcome);
                }
                outcome.removed_files += 1;
                removed += target.files[name].len; // Total was checked before mutation.
                outcome.removed_logical_bytes = removed.to_string();
            }
            if check_cancelled(stop).is_err() {
                outcome.status = "interrupted";
                return Ok(outcome);
            }
            let directory = match directory(&dir) {
                Ok(d) if id(&d).is_ok_and(|id| id == target.dir_id) => d,
                _ => {
                    outcome.status = "interrupted";
                    return Ok(outcome);
                }
            };
            if directory.sync_all().is_err() {
                outcome.sync_warning = true;
            }
            if fs::remove_dir(&dir).is_err() {
                outcome.status = "outcome_unknown";
                return Ok(outcome);
            }
            if guards.roots[i].sync_all().is_err() {
                outcome.sync_warning = true;
            }
        }
        Ok(outcome)
    }
}

#[cfg(test)]
mod tests;
