//! One atomic selection of source revisions, including a jobdeck's complete
//! selected source set. Source current pointers are deliberately independent.
use super::*;
use std::collections::BTreeSet;
pub mod inventory;

const MAX_SET_BYTES: u64 = 32 * 1024 * 1024;
const MAX_MEMBERS: usize = 65536;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Member {
    source: PathBuf,
    revision: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    source: PathBuf,
    source_stamp: Stamp,
    revision: String,
    levels: Option<BTreeSet<i64>>,
    members: Vec<Member>,
}
impl Manifest {
    fn validate(&self, source: &Path) -> Result<BTreeSet<PathBuf>> {
        let members: BTreeSet<_> = self.members.iter().map(|m| m.source.clone()).collect();
        if !matches!(self.version, 1 | 2)
            || self.source != source
            || !valid_id(&self.revision)
            || self.members.is_empty()
            || self.members.len() > MAX_MEMBERS
            || members.len() != self.members.len()
            || self
                .members
                .iter()
                .any(|m| !m.source.is_absolute() || !valid_id(&m.revision))
            || self
                .levels
                .as_ref()
                .is_some_and(|v| v.is_empty() || v.len() > 4096)
        {
            return Err(invalid("invalid revision set manifest"));
        }
        Ok(members)
    }
}

#[derive(Clone, Debug)]
pub struct Store {
    raw: super::Store,
}
impl Store {
    pub fn new(source: &Path) -> Result<Self> {
        let mut raw = super::Store::new(source)?;
        raw.root.as_mut_os_string().push(".sets");
        Ok(Self { raw })
    }
    pub fn path(&self) -> &Path {
        self.raw.path()
    }
    fn read(&self) -> Result<Option<(Manifest, Vec<u8>)>> {
        match fs::symlink_metadata(self.path()) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
            Ok(_) => (),
        }
        let _root = directory(self.path())?;
        let Some(bytes) = current_limit(self.path(), MAX_SET_BYTES)? else {
            return Ok(None);
        };
        let manifest: Manifest =
            serde_json::from_slice(&bytes).map_err(|_| invalid("unreadable revision set"))?;
        manifest.validate(&self.raw.source)?;
        let seal = self.path().join(&manifest.revision).join("revision.json");
        let _dir = directory(seal.parent().unwrap())?;
        if record_bytes_limit(regular(&seal, false)?, MAX_SET_BYTES)? != bytes {
            return Err(invalid("revision set seal and current disagree"));
        }
        Ok(Some((manifest, bytes)))
    }
    /// Expected sources are derived from a registered layout/deck, never from
    /// this manifest. Validate the complete set BEFORE opening any member path.
    pub fn pin(
        &self,
        expected: &BTreeSet<PathBuf>,
        levels: &Option<BTreeSet<i64>>,
        stop: &AtomicUsize,
    ) -> Result<Option<Snapshot>> {
        check_cancelled(stop)?;
        let Some((manifest, bytes)) = self.read()? else {
            return Ok(None);
        };
        if manifest.validate(&self.raw.source)? != *expected || manifest.levels != *levels {
            return Err(invalid(
                "revision set does not match the requested sources/levels",
            ));
        }
        let readers = reader_lease(directory(&self.path().join(&manifest.revision))?)?;
        if Stamp::source(&self.raw.source)? != manifest.source_stamp {
            return Err(invalid("revision set source changed; rebuild required"));
        }
        let mut members = BTreeMap::new();
        for member in &manifest.members {
            check_cancelled(stop)?;
            let pin = super::Store::new(&member.source)?.pin_id(&member.revision)?;
            validate_owner(&manifest, &pin.record)?;
            pin.source_unchanged()?;
            members.insert(member.source.clone(), pin);
        }
        let snapshot = Snapshot {
            root_id: id(&directory(self.path())?)?,
            dir_id: id(&readers)?,
            _readers: readers,
            store: self.clone(),
            manifest,
            bytes,
            members,
        };
        snapshot.validate(stop)?;
        Ok(Some(snapshot))
    }
    pub(crate) fn begin(
        &self,
        levels: Option<BTreeSet<i64>>,
        expected: BTreeSet<PathBuf>,
        stop: &AtomicUsize,
    ) -> Result<Builder> {
        if expected.is_empty() || expected.len() > MAX_MEMBERS {
            return Err(invalid("invalid revision set size"));
        }
        let candidate = self
            .raw
            .begin_checked(stop, MAX_SET_BYTES, || self.read().map(|_| ()))?;
        Ok(Builder {
            candidate,
            levels,
            expected,
        })
    }
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    store: Store,
    manifest: Manifest,
    bytes: Vec<u8>,
    members: BTreeMap<PathBuf, super::Snapshot>,
    root_id: (u64, u64),
    dir_id: (u64, u64),
    _readers: Arc<File>,
}
impl Snapshot {
    pub(crate) fn validate_inputs(&self, stop: &AtomicUsize) -> Result<()> {
        check_cancelled(stop)?;
        if Stamp::source(self.source())? != self.manifest.source_stamp {
            return Err(invalid("pinned dataset source changed"));
        }
        for pin in self.members.values() {
            check_cancelled(stop)?;
            pin.source_unchanged()?;
        }
        Ok(())
    }
    pub fn id(&self) -> &str {
        &self.manifest.revision
    }
    pub fn members(&self) -> &BTreeMap<PathBuf, super::Snapshot> {
        &self.members
    }
    pub fn source(&self) -> &Path {
        &self.manifest.source
    }
    pub fn levels(&self) -> &Option<BTreeSet<i64>> {
        &self.manifest.levels
    }
    pub fn validate(&self, stop: &AtomicUsize) -> Result<()> {
        check_cancelled(stop)?;
        let path = self.store.path().join(self.id());
        if id(&directory(self.store.path())?)? != self.root_id
            || id(&directory(&path)?)? != self.dir_id
            || record_bytes_limit(regular(&path.join("revision.json"), false)?, MAX_SET_BYTES)?
                != self.bytes
        {
            return Err(invalid("pinned revision set changed"));
        }
        for snapshot in self.members.values() {
            check_cancelled(stop)?;
            snapshot.validate()?;
        }
        Ok(())
    }
}

pub(crate) struct Builder {
    candidate: Candidate,
    levels: Option<BTreeSet<i64>>,
    expected: BTreeSet<PathBuf>,
}
pub(crate) struct Publication {
    pub revision: String,
    pub directory_synced: bool,
}
impl Builder {
    pub(crate) fn owner(&self) -> SetOwner {
        SetOwner {
            source: self.candidate.store.source.clone(),
            revision: self.candidate.revision.clone(),
        }
    }
    pub(crate) fn publish(
        self,
        members: BTreeMap<PathBuf, super::Snapshot>,
        stop: &AtomicUsize,
    ) -> Result<Publication> {
        if members.keys().cloned().collect::<BTreeSet<_>>() != self.expected {
            return Err(invalid("incomplete revision set cannot be published"));
        }
        for (source, pin) in &members {
            check_cancelled(stop)?;
            if pin.source() != source {
                return Err(invalid("revision member source mismatch"));
            }
            if pin.record.version != 2 || pin.record.owner.as_ref() != Some(&self.owner()) {
                return Err(invalid("revision member does not belong to this new set"));
            }
            pin.validate()?;
            pin.source_unchanged()?;
        }
        let manifest = Manifest {
            version: 2,
            source: self.candidate.store.source.clone(),
            source_stamp: self.candidate.source_stamp.clone(),
            revision: self.candidate.revision.clone(),
            levels: self.levels,
            members: members
                .iter()
                .map(|(source, pin)| Member {
                    source: source.clone(),
                    revision: pin.id().into(),
                })
                .collect(),
        };
        manifest.validate(&self.candidate.store.source)?;
        let bytes =
            serde_json::to_vec(&manifest).map_err(|_| invalid("cannot encode revision set"))?;
        create_record_limit(
            &self.candidate.directory().join("revision.json"),
            &bytes,
            MAX_SET_BYTES,
        )?;
        directory(&self.candidate.directory())?.sync_all()?;
        check_cancelled(stop)?;
        let directory_synced = self.candidate.commit_current(&bytes, stop)?;
        Ok(Publication {
            revision: manifest.revision,
            directory_synced,
        })
    }
}

fn validate_owner(manifest: &Manifest, record: &Record) -> Result<()> {
    let expected = SetOwner {
        source: manifest.source.clone(),
        revision: manifest.revision.clone(),
    };
    // Old readers reject v2. Old sets must never acquire v2 source data, which
    // would otherwise give a lease-unaware reader a hidden reference to it.
    let matches = match manifest.version {
        1 => record.version == 1 && record.owner.is_none(),
        2 => record.version == 2 && record.owner.as_ref() == Some(&expected),
        _ => false,
    };
    if !matches {
        return Err(invalid("revision set member ownership mismatch"));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
