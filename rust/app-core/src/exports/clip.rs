//! One explicitly accepted exact export, independent of frame credit/zoom.
//! The gateway supplies a registered source and pinned ManagedDataset; it must
//! validate its owner/view/revision/selection receipt before calling start.
use super::artifacts::{Reservation, Store};
use crate::{
    cache, check_cancelled,
    clip::{self, ClipOptions, CollectPhase},
    dataset::Dataset,
    managed::{ManagedDataset, Resources},
    registered::RegisteredSource,
    Error, ErrorKind, Result,
};
use floe_worker_client::{ClipArtifact, ClipRequest, Layers};
use std::{
    fs::{self, Metadata},
    os::unix::fs::MetadataExt,
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Instant,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Preparing,
    Opening,
    Clipping,
    Finishing,
    Cancelling,
    Ready,
    Failed,
    Cancelled,
}
#[derive(Clone, Debug)]
pub struct Outcome {
    pub artifact_id: u64,
    pub bytes: u64,
    pub records: u64,
    pub bbox_dbu: [i64; 4],
    pub source_stale: bool,
}
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub id: u64,
    pub dataset_revision: u64,
    pub phase: Phase,
    pub elapsed_ms: u64,
    pub native_pid: Option<u32>,
    pub outcome: Option<Outcome>,
    pub failure: Option<ErrorKind>,
}
impl Snapshot {
    pub fn terminal(&self) -> bool {
        matches!(self.phase, Phase::Ready | Phase::Failed | Phase::Cancelled)
    }
}
struct State {
    snapshot: Snapshot,
    sealed: bool,
}
pub struct Job {
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicUsize>,
    started: Instant,
    thread: Option<JoinHandle<()>>,
}
impl Job {
    /// Off-reactor: source/cache scope checks and admission perform filesystem
    /// I/O. Request coordinates are canonical DBU and layer pairs already
    /// resolved by the caller. No browser filename or executable is accepted.
    pub fn start(
        resources: &Arc<Resources>,
        store: &Arc<Store>,
        source: Arc<RegisteredSource>,
        data: Arc<ManagedDataset>,
        request: ClipRequest,
        options: ClipOptions,
    ) -> Result<Self> {
        request.validate()?;
        if request.jobs != options.jobs || !(1..=16).contains(&request.jobs) {
            return Err(Error::input(
                "managed clip jobs must match options and be 1..16",
            ));
        }
        let Dataset::Layout(layout) = &data.dataset else {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "jobdeck clip is unsupported",
            ));
        };
        if source.deck || source.path() != data.dataset.source() {
            return Err(Error::input("export registration/dataset mismatch"));
        }
        if let Layers::Only(pairs) = &request.layers {
            if pairs
                .iter()
                .any(|pair| !layout.metadata.layers.iter().any(|l| l.key() == *pair))
            {
                return Err(Error::input("export references an unknown layer"));
            }
        }
        let stop = Arc::new(AtomicUsize::new(0));
        source.validate(&stop)?;
        let permit = resources.export(request.jobs, options.budget_mb)?;
        let id = resources.next_id()?;
        let reservation = store.reserve(id, Arc::clone(&stop))?;
        let started = Instant::now();
        let state = Arc::new(Mutex::new(State {
            sealed: false,
            snapshot: Snapshot {
                id,
                dataset_revision: data.revision,
                phase: Phase::Preparing,
                elapsed_ms: 0,
                native_pid: None,
                outcome: None,
                failure: None,
            },
        }));
        let shared = Arc::clone(&state);
        let flag = Arc::clone(&stop);
        let thread = thread::Builder::new()
            .name("floe-clip-export".into())
            .spawn(move || {
                let stale = data.dataset.source_stale();
                let result = run(&source, &data, &request, &options, &flag, &shared);
                // Ready/Failed/Cancelled means native cleanup and admission release
                // completed, not merely receipt of the worker's response line.
                drop(data);
                drop(source);
                drop(permit);
                let mut state = shared.lock().unwrap();
                state.snapshot.native_pid = None;
                state.snapshot.elapsed_ms = millis(started);
                finish(&mut state, reservation, result, request.bbox, stale, &flag);
            })?;
        Ok(Self {
            state,
            stop,
            started,
            thread: Some(thread),
        })
    }
    pub fn snapshot(&self) -> Snapshot {
        let mut s = self.state.lock().unwrap().snapshot.clone();
        if !s.terminal() {
            s.elapsed_ms = millis(self.started);
        }
        s
    }
    /// false means the commit already won or work is terminal. Never convert
    /// an available artifact into an ambiguous cancelled result.
    pub fn cancel(&self) -> bool {
        let mut s = self.state.lock().unwrap();
        if s.sealed || s.snapshot.terminal() {
            return false;
        }
        self.stop.store(1, Ordering::Relaxed);
        s.snapshot.phase = Phase::Cancelling;
        true
    }
    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }
    pub fn join(&mut self) -> Result<()> {
        if let Some(t) = self.thread.take() {
            t.join()
                .map_err(|_| Error::new(ErrorKind::Worker, "export task panicked"))?;
        }
        Ok(())
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel();
        let _ = self.join();
    }
}
fn millis(start: Instant) -> u64 {
    start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}
fn finish(
    state: &mut State,
    reservation: Reservation,
    result: Result<ClipArtifact>,
    bbox: [i64; 4],
    stale: bool,
    stop: &AtomicUsize,
) {
    let result = result.and_then(|artifact| {
        check_cancelled(stop)?;
        let records = artifact.fields.u64("records")?;
        let bytes = artifact.size_bytes;
        let id = reservation.commit(artifact.file, bytes)?;
        state.sealed = true;
        Ok(Outcome {
            artifact_id: id,
            bytes,
            records,
            bbox_dbu: bbox,
            source_stale: stale,
        })
    });
    match result {
        Ok(outcome) => {
            state.snapshot.outcome = Some(outcome);
            state.snapshot.phase = Phase::Ready;
        }
        Err(e) => {
            state.snapshot.failure = Some(e.kind);
            state.snapshot.phase = if e.kind == ErrorKind::Cancelled {
                Phase::Cancelled
            } else {
                Phase::Failed
            };
        }
    }
}

#[derive(PartialEq, Eq)]
struct Stamp {
    dev: u64,
    ino: u64,
    len: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}
impl Stamp {
    fn of(m: Metadata) -> Result<Self> {
        if !m.is_file() {
            return Err(Error::input("export input must be regular"));
        }
        Ok(Self {
            dev: m.dev(),
            ino: m.ino(),
            len: m.len(),
            modified: (m.mtime(), m.mtime_nsec()),
            changed: (m.ctime(), m.ctime_nsec()),
        })
    }
}
fn stamp_at(path: &std::path::Path, optional: bool) -> Result<Option<Stamp>> {
    match fs::metadata(path) {
        Ok(m) => Stamp::of(m).map(Some),
        Err(e) if optional && e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
fn stamps(data: &ManagedDataset) -> Result<Vec<(PathBuf, bool, Option<Stamp>)>> {
    let Dataset::Layout(l) = &data.dataset else {
        unreachable!()
    };
    let files = [
        l.source.clone(),
        l.directory.join("meta.json"),
        l.directory.join("design.ovm"),
        l.directory.join("design.ovp"),
        l.directory.join("design.ovt"),
    ];
    files
        .into_iter()
        .enumerate()
        .map(|(i, p)| Ok((p.clone(), i == 4, stamp_at(&p, i == 4)?)))
        .collect()
}
fn run(
    source: &RegisteredSource,
    data: &ManagedDataset,
    request: &ClipRequest,
    options: &ClipOptions,
    stop: &Arc<AtomicUsize>,
    state: &Mutex<State>,
) -> Result<ClipArtifact> {
    source.validate(stop)?;
    let before = stamps(data)?;
    let Dataset::Layout(layout) = &data.dataset else {
        unreachable!()
    };
    // Local writers are excluded by data's read lease. Stat checks additionally
    // reject observed external changes during export; these are not an OS
    // sandbox or an immutable snapshot of externally replaced open views.
    let artifact = clip::collect(layout, request, options, stop, |phase, pid| {
        let mut s = state.lock().unwrap();
        s.snapshot.native_pid = pid;
        if s.snapshot.phase != Phase::Cancelling {
            s.snapshot.phase = match phase {
                CollectPhase::Opening => Phase::Opening,
                CollectPhase::Clipping => Phase::Clipping,
                CollectPhase::Closing => Phase::Finishing,
            };
        }
    })?;
    source.validate(stop)?;
    for (path, optional, stamp) in before {
        check_cancelled(stop)?;
        if stamp_at(&path, optional)? != stamp {
            return Err(Error::new(ErrorKind::Cache, "export input changed"));
        }
    }
    // Fail on damaged marker/version too, not just a changed source mtime.
    cache::validated_vfs(&layout.directory)?;
    Ok(artifact)
}

#[cfg(test)]
mod tests;
