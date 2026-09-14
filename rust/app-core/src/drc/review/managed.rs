//! Admitted local review lifetime. Paths/reviewer are trusted registration,
//! never request-selected authority. Reads/prepares run off the HTTP reactor;
//! publication has its own joinable job, independent of response subscribers.
use super::{store, ImportReport, Notes, WaiveStats};
use crate::{
    check_cancelled,
    managed::{Permit, Resources},
    registered::{AccessScope, RegisteredSource},
    Error, ErrorKind, Result,
};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Instant,
};

/// Trusted local selection. Every protected file also receives a read lease;
/// include the registered ASCII source/rules and other immutable dependencies.
/// Trees are output exclusions, not recursively expanded cache read leases.
pub struct Registration {
    pub scope: Arc<AccessScope>,
    pub pack: PathBuf,
    pub reviewer: String,
    pub kind: store::Kind,
    pub protected_files: Vec<PathBuf>,
    pub protected_trees: Vec<PathBuf>,
}
struct State {
    closed: bool,
    active: Option<Arc<AtomicUsize>>,
}
pub struct ManagedStore {
    store: Arc<store::Store>,
    resources: Arc<Resources>,
    state: Mutex<State>,
    // Outlives the store, snapshot, draft, and running publication. This is
    // process-local admission, not a cross-process lock or hard RSS ceiling.
    _permit: Permit,
}
impl ManagedStore {
    /// Off-reactor. Admit before opening/decoding the pack; one registration
    /// reserves one CPU slot and 256 MiB from the existing DRC admission pool.
    pub fn open(
        resources: &Arc<Resources>,
        r: Registration,
        stop: &AtomicUsize,
    ) -> Result<Arc<Self>> {
        Self::open_guarded(resources, r, vec![], stop)
    }
    /// Sources only restrict outputs; they grant no additional output authority.
    pub fn open_guarded(
        resources: &Arc<Resources>,
        r: Registration,
        sources: Vec<Arc<RegisteredSource>>,
        stop: &AtomicUsize,
    ) -> Result<Arc<Self>> {
        check_cancelled(stop)?;
        if r.protected_files.len() > 128 || r.protected_trees.len() > 128 || sources.len() > 32 {
            return Err(Error::input("too many protected review paths"));
        }
        let pack = r.scope.check(&r.pack)?;
        let permit = resources
            .drc(std::iter::once(pack.clone()).chain(r.protected_files.iter().cloned()))?;
        let store = store::Store::open_guarded(
            r.scope,
            &pack,
            &r.reviewer,
            r.kind,
            r.protected_files,
            r.protected_trees,
            sources,
            stop,
        )?;
        check_cancelled(stop)?;
        Ok(Arc::new(Self {
            store,
            resources: Arc::clone(resources),
            state: Mutex::new(State {
                closed: false,
                active: None,
            }),
            _permit: permit,
        }))
    }
    pub fn target(&self) -> &Path {
        self.store.target()
    }
    pub fn kind(&self) -> store::Kind {
        self.store.kind()
    }
    pub fn identity(&self) -> super::Identity {
        self.store.identity()
    }
    /// At most one snapshot/draft/job owns this registration. No hidden queue
    /// or unlimited retained models behind a one-CPU admission reservation.
    /// Caller owns the flag before this blocking call so timeout can cancel it.
    pub fn snapshot(self: &Arc<Self>, stop: Arc<AtomicUsize>) -> Result<Snapshot> {
        check_cancelled(&stop)?;
        let lease = {
            let mut s = self.state.lock().unwrap();
            if s.closed {
                return Err(Error::new(ErrorKind::Cancelled, "review store retired"));
            }
            if s.active.is_some() {
                return Err(Error::new(ErrorKind::Busy, "review operation is active"));
            }
            s.active = Some(Arc::clone(&stop));
            Borrow {
                owner: Arc::clone(self),
                stop,
            }
        };
        let value = self.store.snapshot(&lease.stop)?;
        check_cancelled(&lease.stop)?;
        Ok(Snapshot { value, lease })
    }
    pub fn snapshot_published(
        self: &Arc<Self>,
        published: &store::Published,
        stop: Arc<AtomicUsize>,
    ) -> Result<Snapshot> {
        let snapshot = self.snapshot(stop)?;
        snapshot.value.verify_published(published)?;
        snapshot.lease.check()?;
        Ok(snapshot)
    }
    /// Retire before pack replacement/logout. Nonblocking, no force release;
    /// live work keeps the pack lease until it actually unwinds/finishes.
    pub fn request_stop(&self) {
        let mut s = self.state.lock().unwrap();
        s.closed = true;
        if let Some(stop) = &s.active {
            stop.store(1, Ordering::Relaxed);
        }
    }
    pub fn is_idle(&self) -> bool {
        self.state.lock().unwrap().active.is_none()
    }
}
struct Borrow {
    owner: Arc<ManagedStore>,
    stop: Arc<AtomicUsize>,
}
impl Borrow {
    fn check(&self) -> Result<()> {
        check_cancelled(&self.stop)?;
        if self.owner.state.lock().unwrap().closed {
            return Err(Error::new(ErrorKind::Cancelled, "review store retired"));
        }
        Ok(())
    }
}
impl Drop for Borrow {
    fn drop(&mut self) {
        let mut s = self.owner.state.lock().unwrap();
        // Only this borrow can exist, but don't clear a future owner's flag.
        if s.active
            .as_ref()
            .is_some_and(|p| Arc::ptr_eq(p, &self.stop))
        {
            s.active = None;
        }
    }
}
/// Non-serializable expected version; cannot outlive its managed read lease.
pub struct Snapshot {
    value: store::Snapshot,
    lease: Borrow,
}
impl Snapshot {
    /// An actor owns the snapshot and its admission/pack lease until installation
    /// finishes. Both request cancellation and store retirement are checked before
    /// switching state; failure is not reported as a failed disk publication.
    pub fn apply_waives(
        self,
        database: &mut crate::drc::Database,
        stop: &AtomicUsize,
    ) -> Result<store::AppliedWaives> {
        self.lease.check()?;
        let Self { value, lease } = self;
        value.apply_waives_using(database, stop, || lease.check())
    }
    pub fn selected_statuses(&self, gids: &[u64]) -> Result<Vec<u8>> {
        self.lease.check()?;
        self.value.selected_statuses(gids, &self.lease.stop)
    }
    pub fn exists(&self) -> bool {
        self.value.exists()
    }
    pub fn legacy_unverified(&self) -> bool {
        self.value.legacy_unverified()
    }
    pub fn notes(&self) -> Option<&Notes> {
        self.value.notes()
    }
    pub fn waives(&self) -> Option<&WaiveStats> {
        self.value.waives()
    }
    pub fn import_report(&self) -> &ImportReport {
        self.value.import_report()
    }
    pub fn prepare_waives(self, edits: &[(u64, u8)]) -> Result<Prepared> {
        self.lease.check()?;
        let legacy = self.value.legacy_unverified();
        let draft = self.value.prepare_waives(edits, &self.lease.stop)?;
        Ok(Prepared {
            draft,
            lease: self.lease,
            legacy,
        })
    }
    pub fn prepare_note(self, gids: &[u64], text: &str) -> Result<Prepared> {
        self.lease.check()?;
        let legacy = self.value.legacy_unverified();
        let draft = self.value.prepare_note(gids, text, &self.lease.stop)?;
        Ok(Prepared {
            draft,
            lease: self.lease,
            legacy,
        })
    }
    /// Caller must show the normalization report before approving publication.
    pub fn prepare_notes_import(self, text: &str) -> Result<(Prepared, ImportReport)> {
        self.lease.check()?;
        let legacy = self.value.legacy_unverified();
        let (draft, report) = self.value.prepare_notes_import(text, &self.lease.stop)?;
        Ok((
            Prepared {
                draft,
                lease: self.lease,
                legacy,
            },
            report,
        ))
    }
}
pub struct Prepared {
    draft: store::Draft,
    lease: Borrow,
    legacy: bool,
}
impl Prepared {
    pub fn store(&self) -> Arc<ManagedStore> {
        Arc::clone(&self.lease.owner)
    }
    pub fn target(&self) -> &Path {
        self.draft.target()
    }
    pub fn kind(&self) -> store::Kind {
        self.draft.kind()
    }
    pub fn legacy_unverified(&self) -> bool {
        self.legacy
    }
    /// Explicit local approval consumes the draft once. The future owner actor
    /// must bind its auth/DRC/review revision and retry receipt before this call.
    /// It must not blindly replay this operation after a lost HTTP response.
    pub fn publish(self, confirm_legacy: bool) -> Result<Publication> {
        self.start_using(confirm_legacy, |draft, stop| draft.publish(stop))
    }
    fn start_using(
        self,
        confirm_legacy: bool,
        publish: impl FnOnce(store::Draft, &AtomicUsize) -> Result<store::Published> + Send + 'static,
    ) -> Result<Publication> {
        self.lease.check()?;
        if self.legacy && !confirm_legacy {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "legacy review run needs explicit confirmation",
            ));
        }
        let id = self.lease.owner.resources.next_id()?;
        let started = Instant::now();
        let state = Arc::new(Mutex::new(Status {
            id,
            kind: self.kind(),
            phase: Phase::Publishing,
            elapsed_ms: 0,
            failure: None,
            outcome: None,
            outcome_unknown: false,
        }));
        let stop = Arc::clone(&self.lease.stop);
        let shared = Arc::clone(&state);
        let flag = Arc::clone(&stop);
        let thread = thread::Builder::new()
            .name("floe-review-publish".into())
            .spawn(move || {
                let Prepared { draft, lease, .. } = self;
                // A panic after the filesystem commit cannot be called rollback.
                // Retain an explicit uncertain outcome instead of auto retrying.
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let draft = if confirm_legacy {
                        draft.accept_legacy_run()
                    } else {
                        draft
                    };
                    publish(draft, &flag)
                }));
                drop(lease);
                let mut s = shared.lock().unwrap();
                s.elapsed_ms = millis(started);
                match result {
                    Ok(Ok(outcome)) => {
                        s.outcome = Some(outcome);
                        s.phase = Phase::Succeeded;
                    }
                    Ok(Err(e)) => {
                        s.failure = Some(e.kind);
                        s.phase = if e.kind == ErrorKind::Cancelled {
                            Phase::Cancelled
                        } else {
                            Phase::Failed
                        };
                    }
                    Err(_) => {
                        s.failure = Some(ErrorKind::Worker);
                        s.outcome_unknown = true;
                        s.phase = Phase::Failed;
                    }
                }
            })?;
        Ok(Publication {
            state,
            stop,
            thread: Some(thread),
            started,
        })
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Publishing,
    Cancelling,
    Succeeded,
    Failed,
    Cancelled,
}
#[derive(Clone, Debug)]
pub struct Status {
    /// Process-local identifier, not a credential or a persisted retry key.
    pub id: u64,
    pub kind: store::Kind,
    pub phase: Phase,
    pub elapsed_ms: u64,
    pub failure: Option<ErrorKind>,
    pub outcome: Option<store::Published>,
    /// Worker panic may have happened after commit. Inspect/reload; don't retry.
    pub outcome_unknown: bool,
}
impl Status {
    pub fn terminal(&self) -> bool {
        matches!(
            self.phase,
            Phase::Succeeded | Phase::Failed | Phase::Cancelled
        )
    }
}
pub struct Publication {
    state: Arc<Mutex<Status>>,
    stop: Arc<AtomicUsize>,
    thread: Option<JoinHandle<()>>,
    started: Instant,
}
fn millis(start: Instant) -> u64 {
    start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}
impl Publication {
    pub fn status(&self) -> Status {
        let mut s = self.state.lock().unwrap().clone();
        if !s.terminal() {
            s.elapsed_ms = millis(self.started);
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
    /// Off-reactor. A stopped observer does not own this handle: its owner keeps
    /// it until terminal and can recover the result even after response loss.
    pub fn close(&mut self) -> Result<()> {
        self.cancel();
        if let Some(t) = self.thread.take() {
            t.join().map_err(|_| {
                Error::new(ErrorKind::Worker, "review publication supervisor panicked")
            })?;
        }
        Ok(())
    }
}
impl Drop for Publication {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

#[cfg(test)]
mod tests;
