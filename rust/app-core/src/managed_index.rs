//! One managed index supervisor. The all-source write/CPU reservation outlives
//! prepare, every sequential deck source, cancellation and native child reap.
use crate::{
    check_cancelled,
    index::{Action, IndexOptions, PreparedIndex},
    index_progress::Progress,
    jobdeck::index::DeckIndexPlan,
    managed::Resources,
    native::Indexer,
    registered::RegisteredSource,
    Error, ErrorKind, Result,
};
use std::{
    collections::BTreeSet,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Preparing,
    Running,
    Cancelling,
    Succeeded,
    Incomplete,
    Failed,
    Cancelled,
}
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub id: u64,
    pub phase: Phase,
    pub title: String,
    pub current: Option<String>,
    pub completed: usize,
    pub total: usize,
    pub kept: usize,
    pub skipped: usize,
    pub failed: usize,
    pub elapsed_ms: u64,
    /// Safe category only; native text/paths are not stored in the event state.
    pub failure: Option<ErrorKind>,
    pub native: Progress,
}
impl Snapshot {
    pub fn terminal(&self) -> bool {
        matches!(
            self.phase,
            Phase::Succeeded | Phase::Incomplete | Phase::Failed | Phase::Cancelled
        )
    }
}
pub struct ManagedIndex {
    state: Arc<Mutex<Snapshot>>,
    stop: Arc<AtomicUsize>,
    started: Instant,
    thread: Option<JoinHandle<()>>,
}
impl ManagedIndex {
    /// Call from an admitted service thread, not an HTTP reactor: cache lease
    /// key resolution may touch the filesystem. There is no hidden job queue.
    pub fn start(
        resources: &Arc<Resources>,
        source: Arc<RegisteredSource>,
        levels: Option<BTreeSet<i64>>,
        options: IndexOptions,
        indexer: Indexer,
    ) -> Result<Self> {
        options.validate()?;
        if options.profile_cell.is_some() {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "managed indexing does not expose profile paths",
            ));
        }
        source.validate_levels(levels.as_ref())?;
        let permit = resources.index(source.cache_paths()?, options.jobs)?;
        let id = resources.next_id()?;
        let stop = Arc::new(AtomicUsize::new(0));
        let started = Instant::now();
        let state = Arc::new(Mutex::new(Snapshot {
            id,
            phase: Phase::Preparing,
            title: source.title.clone(),
            current: None,
            completed: 0,
            total: 0,
            kept: 0,
            skipped: 0,
            failed: 0,
            elapsed_ms: 0,
            failure: None,
            native: Progress::default(),
        }));
        let (shared, flag) = (Arc::clone(&state), Arc::clone(&stop));
        let thread = thread::Builder::new()
            .name("floe-managed-index".into())
            .spawn(move || {
                let result = run(&source, levels, &options, &indexer, &flag, &shared);
                // Terminal state means all native children AND leases are released.
                drop(permit);
                let mut s = shared.lock().unwrap();
                s.elapsed_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
                match result {
                    Ok(()) => {
                        s.phase = if s.failed > 0 || s.skipped > 0 {
                            Phase::Incomplete
                        } else {
                            Phase::Succeeded
                        }
                    }
                    Err(e) => {
                        s.failure = Some(e.kind);
                        s.phase = if e.kind == ErrorKind::Cancelled {
                            Phase::Cancelled
                        } else {
                            Phase::Failed
                        };
                    }
                }
            })?;
        Ok(Self {
            state,
            stop,
            started,
            thread: Some(thread),
        })
    }
    pub fn snapshot(&self) -> Snapshot {
        let mut s = self.state.lock().unwrap().clone();
        if !s.terminal() {
            s.elapsed_ms = self.started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
        }
        s
    }
    pub fn cancel(&self) {
        let mut s = self.state.lock().unwrap();
        if !s.terminal() {
            self.stop.store(1, Ordering::Relaxed);
            s.phase = Phase::Cancelling;
        }
    }
    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(|t| t.is_finished())
    }
    /// Off-reactor join. IndexJob's drop/cancel always kills/reaps before return.
    pub fn close(&mut self) -> Result<()> {
        self.cancel();
        if let Some(t) = self.thread.take() {
            t.join()
                .map_err(|_| Error::new(ErrorKind::Worker, "managed index thread panicked"))?;
        }
        Ok(())
    }
}
impl Drop for ManagedIndex {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

fn run(
    source: &RegisteredSource,
    levels: Option<BTreeSet<i64>>,
    options: &IndexOptions,
    indexer: &Indexer,
    flag: &AtomicUsize,
    state: &Mutex<Snapshot>,
) -> Result<()> {
    source.validate(flag)?;
    let entries = if source.deck {
        let plan = DeckIndexPlan::prepare(source.path(), levels, options, flag)?;
        let mut s = state.lock().unwrap();
        s.kept = plan.kept;
        s.skipped = plan.catalog.infos.values().filter(|i| !i.ok()).count();
        s.total = plan.todo.len() + s.kept + s.skipped;
        s.completed = s.kept + s.skipped;
        plan.todo
            .into_iter()
            .map(|e| (e.source, e.options))
            .collect::<Vec<_>>()
    } else {
        state.lock().unwrap().total = 1;
        vec![(source.path().to_owned(), options.clone())]
    };
    for (path, options) in entries {
        check_cancelled(flag)?;
        {
            let mut s = state.lock().unwrap();
            s.current = Some(
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .chars()
                    .take(256)
                    .collect(),
            );
            s.phase = Phase::Running;
            s.native = Progress::default();
        }
        let result = (|| {
            let prepared = PreparedIndex::prepare(&path, &options, indexer.clone(), flag)?;
            if matches!(prepared.action(), Action::Reuse | Action::OccupancyPresent) {
                return Ok(true);
            }
            let mut job = prepared.start_captured(flag)?;
            loop {
                if flag.load(Ordering::Relaxed) != 0 {
                    job.cancel(libc::SIGTERM)?;
                    return Err(Error::new(
                        ErrorKind::Cancelled,
                        "managed index cancelled; incomplete caches may need --force",
                    ));
                }
                let done = job.poll()?;
                state.lock().unwrap().native = job.progress().cloned().unwrap_or_default();
                match done {
                    Some(0) => return Ok(false),
                    Some(130 | 143) => {
                        return Err(Error::new(ErrorKind::Cancelled, "native index cancelled"))
                    }
                    Some(_) => return Err(Error::new(ErrorKind::Worker, "native index failed")),
                    None => thread::sleep(Duration::from_millis(20)),
                }
            }
        })();
        let mut s = state.lock().unwrap();
        match result {
            Ok(reused) => {
                s.completed += 1;
                s.kept += usize::from(reused);
            }
            Err(e) if e.kind == ErrorKind::Cancelled => return Err(e),
            Err(e) => {
                s.completed += 1;
                s.failed += 1;
                s.failure = Some(e.kind);
                if !source.deck {
                    return Err(e);
                }
            }
        }
    }
    Ok(())
}
