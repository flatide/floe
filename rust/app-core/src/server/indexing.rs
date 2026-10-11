//! Process-local shared indexing ledger. A server-owned runner (not the first
//! browser) claims jobs. No file is built/deleted here. Before request, validate
//! cache freshness; before execution/publication, recheck source and options.
use super::IndexPolicy;
use crate::{Error, ErrorKind, Result};
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::Mutex,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildKey {
    source: PathBuf,
    stamp: [u64; 7],
    native_version: String,
    options: String,
}
impl BuildKey {
    /// A trusted adapter supplies the FULL normalized build-affecting options,
    /// not just an option label. Neither options nor paths become shell argv.
    pub fn capture(
        source: &Path,
        native_version: &str,
        options: &serde_json::Value,
    ) -> Result<Self> {
        if native_version.is_empty()
            || native_version.len() > 128
            || native_version.chars().any(char::is_control)
            || !options.is_object()
        {
            return Err(Error::input("invalid index build identity"));
        }
        let options =
            serde_json::to_string(options).map_err(|_| Error::input("invalid build options"))?;
        if options.len() > 8192 {
            return Err(Error::input("index options exceed identity limit"));
        }
        let source = fs::canonicalize(source)?;
        let stamp = source_stamp(&source)?;
        Ok(Self {
            source,
            stamp,
            native_version: native_version.into(),
            options,
        })
    }
    pub fn source(&self) -> &Path {
        &self.source
    }
    pub fn unchanged(&self) -> Result<bool> {
        Ok(source_stamp(&self.source)? == self.stamp)
    }
}
fn source_stamp(path: &Path) -> Result<[u64; 7]> {
    let m = fs::metadata(path)?;
    if !m.is_file() {
        return Err(Error::input("index source must be a regular file"));
    }
    Ok([
        m.dev(),
        m.ino(),
        m.len(),
        m.mtime() as u64,
        m.mtime_nsec() as u64,
        m.ctime() as u64,
        m.ctime_nsec() as u64,
    ])
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobId {
    epoch: [u8; 16],
    sequence: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Queued,
    Running,
    Succeeded,
    Failed,
}
impl Phase {
    fn terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed)
    }
}
#[derive(Clone, Debug)]
pub struct Submission {
    pub id: JobId,
    pub joined: bool,
}
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub id: JobId,
    pub phase: Phase,
}
/// Native runner owns this receipt until the child is reaped. Dropping the
/// receipt deliberately DOES NOT free capacity or cancel a shared job.
#[derive(Debug)]
pub struct Running {
    id: JobId,
    key: BuildKey,
}
impl Running {
    pub fn id(&self) -> &JobId {
        &self.id
    }
    pub fn key(&self) -> &BuildKey {
        &self.key
    }
}
struct Entry {
    key: BuildKey,
    phase: Phase,
}
struct State {
    sequence: u64,
    entries: BTreeMap<u64, Entry>,
}
pub struct Coordinator {
    epoch: [u8; 16],
    limits: IndexPolicy,
    state: Mutex<State>,
}
fn busy(message: &str) -> Error {
    Error::new(ErrorKind::Busy, message)
}
impl Coordinator {
    pub fn new(limits: IndexPolicy) -> Result<Self> {
        limits.validate()?;
        let mut epoch = [0; 16];
        getrandom::fill(&mut epoch)
            .map_err(|_| Error::new(ErrorKind::Io, "index coordinator entropy unavailable"))?;
        Ok(Self {
            epoch,
            limits,
            state: Mutex::new(State {
                sequence: 0,
                entries: BTreeMap::new(),
            }),
        })
    }
    fn id(&self, sequence: u64) -> JobId {
        JobId {
            epoch: self.epoch,
            sequence,
        }
    }
    pub fn request(&self, key: BuildKey) -> Result<Submission> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| busy("index coordinator unavailable"))?;
        // Join before capacity checking: an existing build never needs a
        // second slot, even when every slot/queue entry is occupied.
        if let Some((&seq, _)) = state
            .entries
            .iter()
            .find(|(_, e)| !e.phase.terminal() && e.key == key)
        {
            return Ok(Submission {
                id: self.id(seq),
                joined: true,
            });
        }
        let next = state
            .sequence
            .checked_add(1)
            .ok_or_else(|| busy("index receipt sequence exhausted"))?;
        if state.entries.len() >= usize::from(self.limits.max_entries) {
            let old = state
                .entries
                .iter()
                .find_map(|(&seq, e)| e.phase.terminal().then_some(seq));
            if let Some(old) = old {
                state.entries.remove(&old);
            } else {
                return Err(busy("shared index queue is full"));
            }
        }
        state.sequence = next;
        state.entries.insert(
            next,
            Entry {
                key,
                phase: Phase::Queued,
            },
        );
        Ok(Submission {
            id: self.id(next),
            joined: false,
        })
    }
    pub fn start_next(&self) -> Result<Option<Running>> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| busy("index coordinator unavailable"))?;
        let active: Vec<_> = state
            .entries
            .values()
            .filter(|e| e.phase == Phase::Running)
            .map(|e| e.key.source.clone())
            .collect();
        if active.len() >= usize::from(self.limits.max_running) {
            return Ok(None);
        }
        // Different revisions/profiles of one source must not write the same
        // cache concurrently. An unrelated queued source can still proceed.
        let next = state.entries.iter().find_map(|(&seq, e)| {
            (e.phase == Phase::Queued && !active.contains(&e.key.source)).then_some(seq)
        });
        let Some(next) = next else {
            return Ok(None);
        };
        let entry = state.entries.get_mut(&next).unwrap();
        entry.phase = Phase::Running;
        Ok(Some(Running {
            id: self.id(next),
            key: entry.key.clone(),
        }))
    }
    /// Success means a native adapter has validated publication; process exit
    /// 0 alone is not sufficient. Both outcomes require worker cleanup first.
    pub fn finish(&self, running: &Running, success: bool) -> Result<()> {
        if running.id.epoch != self.epoch {
            return Err(Error::input("foreign index receipt"));
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| busy("index coordinator unavailable"))?;
        let entry = state
            .entries
            .get_mut(&running.id.sequence)
            .ok_or_else(|| Error::input("index receipt expired"))?;
        if entry.phase != Phase::Running || entry.key != running.key {
            return Err(Error::input("index receipt is not running"));
        }
        entry.phase = if success {
            Phase::Succeeded
        } else {
            Phase::Failed
        };
        Ok(())
    }
    pub fn snapshot(&self, id: &JobId) -> Result<Option<Snapshot>> {
        if id.epoch != self.epoch {
            return Ok(None);
        }
        let state = self
            .state
            .lock()
            .map_err(|_| busy("index coordinator unavailable"))?;
        Ok(state.entries.get(&id.sequence).map(|e| Snapshot {
            id: id.clone(),
            phase: e.phase,
        }))
    }
}

#[cfg(test)]
mod tests;
