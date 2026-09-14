//! Owner-only note review. Fixed trusted reviewer/targets; opaque snapshots and
//! one-use prepared tokens. Native publication outlives HTTP subscribers.
mod http;
use super::{Failure, Service as Reader};
use crate::{
    auth::SessionId,
    operations::{Admission, Ledger},
};
use floe_app_core::{
    drc::review::{managed, store, NOTE_BYTES},
    registered::RegisteredSource,
    ErrorKind, Result,
};
pub(crate) use http::{is_large_body, routes};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Condvar, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub(super) struct Config {
    pub reviewer: String,
    pub files: Vec<PathBuf>,
    pub trees: Vec<PathBuf>,
    pub sources: Vec<Arc<RegisteredSource>>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Context {
    drc_id: String,
    revision: String,
    view_id: String,
}
impl Context {
    fn validate(&self) -> std::result::Result<(), Failure> {
        if [&self.drc_id, &self.revision, &self.view_id]
            .iter()
            .any(|v| {
                v.len() != 64
                    || !v
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
        {
            return Err("invalid_drc_request");
        }
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Read {
    context: Context,
    errors: Vec<super::dto::CursorDto>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Prepare {
    context: Context,
    token: String,
    text: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Submit {
    seq: String,
    context: Context,
    token: String,
    approve: bool,
    #[serde(default)]
    confirm_legacy: bool,
}
enum Model {
    Snapshot(managed::Snapshot, Vec<u64>),
    Prepared(managed::Prepared),
}
struct Ready {
    owner: SessionId,
    context: Context,
    token: String,
    model: Model,
    stop: Arc<AtomicUsize>,
    expires: Instant,
}
struct Work {
    seq: u64,
    context: Context,
    draft: managed::Prepared,
    confirm_legacy: bool,
    stop: Arc<AtomicUsize>,
}
struct State {
    closed: bool,
    serial: u64,
    review_rev: u64,
    preparing: Option<(u64, Arc<AtomicUsize>)>,
    ready: Option<Ready>,
    pending: Option<Work>,
    stop: Option<Arc<AtomicUsize>>,
    ledger: Ledger,
}
struct Inner {
    config: Config,
    state: Mutex<State>,
    wake: Condvar,
}
pub(super) struct Service {
    inner: Arc<Inner>,
    preparations: Arc<Semaphore>,
    thread: Mutex<Option<JoinHandle<()>>>,
}
/// Shared with blocking work: dropping the HTTP future does not release its
/// reservation/coordination gate while native preparation is still unwinding.
struct Operation {
    service: Arc<Service>,
    serial: u64,
    stop: Arc<AtomicUsize>,
    _permit: OwnedSemaphorePermit,
    _body: Arc<OwnedSemaphorePermit>,
}
impl Drop for Operation {
    fn drop(&mut self) {
        let mut s = self.service.inner.state.lock().unwrap();
        if s.preparing.as_ref().is_some_and(|(n, _)| *n == self.serial) {
            s.preparing = None;
        }
    }
}
impl Service {
    pub(super) fn protected_targets(&self, pack: &std::path::Path) -> Result<Vec<PathBuf>> {
        Ok([store::Kind::Notes, store::Kind::Waives]
            .into_iter()
            .map(|kind| store::paths(pack, &self.inner.config.reviewer, kind))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect())
    }
    pub(super) fn start(config: Config) -> Result<Arc<Self>> {
        let inner = Arc::new(Inner {
            config,
            state: Mutex::new(State {
                closed: false,
                serial: 0,
                review_rev: 0,
                preparing: None,
                ready: None,
                pending: None,
                stop: None,
                ledger: Ledger::default(),
            }),
            wake: Condvar::new(),
        });
        let task = Arc::clone(&inner);
        let thread = thread::Builder::new()
            .name("floe-note-owner".into())
            .spawn(move || run(task))?;
        Ok(Arc::new(Self {
            inner,
            preparations: Arc::new(Semaphore::new(1)),
            thread: Mutex::new(Some(thread)),
        }))
    }
    fn begin(
        self: &Arc<Self>,
        body: Arc<OwnedSemaphorePermit>,
    ) -> std::result::Result<Arc<Operation>, Failure> {
        let permit = Arc::clone(&self.preparations)
            .try_acquire_owned()
            .map_err(|_| "drc_busy")?;
        let mut s = self.inner.state.lock().unwrap();
        if s.closed {
            return Err("drc_closed");
        }
        if s.ledger.active().is_some() {
            return Err("drc_busy");
        }
        s.serial = s.serial.checked_add(1).ok_or("review_limit")?;
        s.ready = None;
        let stop = Arc::new(AtomicUsize::new(0));
        s.preparing = Some((s.serial, Arc::clone(&stop)));
        Ok(Arc::new(Operation {
            service: Arc::clone(self),
            serial: s.serial,
            stop,
            _permit: permit,
            _body: body,
        }))
    }
    fn begin_prepare(
        self: &Arc<Self>,
        owner: &SessionId,
        req: &Prepare,
        body: Arc<OwnedSemaphorePermit>,
    ) -> std::result::Result<(Arc<Operation>, managed::Snapshot, Vec<u64>), Failure> {
        if req.text.trim().len() > NOTE_BYTES {
            return Err("drc_read_limit");
        }
        let permit = Arc::clone(&self.preparations)
            .try_acquire_owned()
            .map_err(|_| "drc_busy")?;
        let mut s = self.inner.state.lock().unwrap();
        if s.closed {
            return Err("drc_closed");
        }
        if s.ledger.active().is_some() {
            return Err("drc_busy");
        }
        let ready = s
            .ready
            .as_ref()
            .filter(|r| {
                r.owner == *owner
                    && r.token == req.token
                    && r.context == req.context
                    && r.expires > Instant::now()
            })
            .ok_or("review_expired")?;
        if !matches!(&ready.model, Model::Snapshot(..)) {
            return Err("review_expired");
        }
        s.serial = s.serial.checked_add(1).ok_or("review_limit")?;
        let r = s.ready.take().unwrap();
        let Model::Snapshot(snapshot, gids) = r.model else {
            unreachable!()
        };
        s.preparing = Some((s.serial, Arc::clone(&r.stop)));
        Ok((
            Arc::new(Operation {
                service: Arc::clone(self),
                serial: s.serial,
                stop: r.stop,
                _permit: permit,
                _body: body,
            }),
            snapshot,
            gids,
        ))
    }
    fn finish(
        &self,
        op: &Operation,
        owner: SessionId,
        context: Context,
        model: Model,
        mut value: Value,
    ) -> std::result::Result<Value, Failure> {
        let mut s = self.inner.state.lock().unwrap();
        if s.closed || s.serial != op.serial || op.stop.load(Ordering::Relaxed) != 0 {
            return Err("review_expired");
        }
        let seconds = if matches!(&model, Model::Snapshot(..)) {
            120
        } else {
            30
        };
        let token = crate::auth::public_id().map_err(|_| "review_unavailable")?;
        value["token"] = json!(token);
        value["context"] = json!(context);
        value["review_rev"] = json!(s.review_rev.to_string());
        value["reviewer"] = json!(self.inner.config.reviewer);
        value["expires_in_ms"] = json!((seconds * 1000).to_string());
        s.ready = Some(Ready {
            owner,
            context,
            token,
            model,
            stop: Arc::clone(&op.stop),
            expires: Instant::now() + Duration::from_secs(seconds),
        });
        Ok(value)
    }
    fn signature(owner: &SessionId, req: &Submit) -> String {
        format!("{} {req:?}", owner.as_str())
    }
    fn replay(
        &self,
        owner: &SessionId,
        req: &Submit,
    ) -> std::result::Result<Option<Value>, Failure> {
        let seq = crate::view::counter(&req.seq)?;
        self.inner
            .state
            .lock()
            .unwrap()
            .ledger
            .replay(seq, &Self::signature(owner, req))
    }
    /// Called while the registry holds the current-reader lock; acceptance is
    /// ordered against a rebuild. Replays are checked before mutable context.
    fn submit(&self, owner: &SessionId, req: Submit) -> std::result::Result<Value, Failure> {
        let seq = crate::view::counter(&req.seq)?;
        if !req.approve {
            return Err("review_approval_required");
        }
        let signature = Self::signature(owner, &req);
        let mut s = self.inner.state.lock().unwrap();
        if s.closed {
            return Err("drc_closed");
        }
        if let Some(v) = s.ledger.replay(seq, &signature)? {
            return Ok(v);
        }
        if s.preparing.is_some() {
            return Err("drc_busy");
        }
        let r = s
            .ready
            .as_ref()
            .filter(|r| {
                r.owner == *owner
                    && r.token == req.token
                    && r.context == req.context
                    && r.expires > Instant::now()
            })
            .ok_or("review_expired")?;
        let Model::Prepared(draft) = &r.model else {
            return Err("review_expired");
        };
        if draft.legacy_unverified() && !req.confirm_legacy {
            return Err("review_legacy_confirmation_required");
        }
        if s.review_rev == u64::MAX {
            return Err("review_limit");
        }
        match s.ledger.admit(seq, signature, "drc_note")? {
            Admission::Replay(v) => return Ok(v),
            Admission::New => (),
        }
        let r = s.ready.take().unwrap();
        let Model::Prepared(draft) = r.model else {
            unreachable!()
        };
        s.stop = Some(Arc::clone(&r.stop));
        s.pending = Some(Work {
            seq,
            context: req.context,
            draft,
            confirm_legacy: req.confirm_legacy,
            stop: r.stop,
        });
        self.inner.wake.notify_one();
        Ok(s.ledger.get(seq).unwrap())
    }
    pub(super) fn status(&self) -> Value {
        let s = self.inner.state.lock().unwrap();
        json!({"available":!s.closed,"kind":"drc_note","reviewer":self.inner.config.reviewer,
            "review_rev":s.review_rev.to_string(),"operations":s.ledger.snapshot(),
            "note_bytes":NOTE_BYTES,"selection_limit":floe_app_core::drc::review::EDIT_ITEMS,
            "preparing":s.preparing.is_some(),"autosave":false})
    }
    fn revoke(&self, owner: &SessionId, token: &str) {
        let mut s = self.inner.state.lock().unwrap();
        if s.ready
            .as_ref()
            .is_some_and(|r| r.owner == *owner && r.token == token)
        {
            s.ready = None;
        }
    }
    pub(super) fn maintain(&self) {
        let mut s = self.inner.state.lock().unwrap();
        if s.ready
            .as_ref()
            .is_some_and(|r| r.expires <= Instant::now())
        {
            s.ready = None;
        }
    }
    /// Registry -> review lock order. Builds reject active preparation/write;
    /// an unapproved preview is discarded before the build takes a write lease.
    pub(super) fn admit_build<T>(
        &self,
        f: impl FnOnce() -> std::result::Result<T, Failure>,
    ) -> std::result::Result<T, Failure> {
        let mut s = self.inner.state.lock().unwrap();
        if s.preparing.is_some()
            || self.preparations.available_permits() == 0
            || s.ledger.active().is_some()
        {
            return Err("drc_busy");
        }
        let result = f()?;
        s.ready = None;
        Ok(result)
    }
    pub(super) fn request_stop(&self) {
        let mut s = self.inner.state.lock().unwrap();
        s.closed = true;
        s.ready = None;
        if let Some((_, stop)) = &s.preparing {
            stop.store(1, Ordering::Relaxed);
        }
        if let Some(stop) = &s.stop {
            stop.store(1, Ordering::Relaxed);
        }
        self.inner.wake.notify_all();
    }
    pub(super) fn is_finished(&self) -> bool {
        self.thread
            .lock()
            .unwrap()
            .as_ref()
            .is_none_or(JoinHandle::is_finished)
            && self.preparations.available_permits() == 1
    }
    fn operation(&self, seq: u64) -> Option<Value> {
        self.inner.state.lock().unwrap().ledger.get(seq)
    }
    fn cancel(&self, seq: u64) -> std::result::Result<Value, Failure> {
        let s = self.inner.state.lock().unwrap();
        let value = s.ledger.get(seq).ok_or("operation_expired")?;
        if s.ledger.active() == Some(seq) {
            if let Some(stop) = &s.stop {
                stop.store(1, Ordering::Relaxed);
            }
        }
        Ok(value)
    }
    fn open(&self, reader: &Reader, stop: &AtomicUsize) -> Result<Arc<managed::ManagedStore>> {
        let r = &reader.registration;
        let c = &self.inner.config;
        let files = c
            .files
            .iter()
            .cloned()
            .chain(std::iter::once(r.path.clone()))
            .chain(r.waives.clone())
            .chain(r.rules.clone())
            .collect();
        managed::ManagedStore::open_guarded(
            &r.resources,
            managed::Registration {
                scope: Arc::clone(&r.scope),
                pack: r.path.clone(),
                reviewer: c.reviewer.clone(),
                kind: store::Kind::Notes,
                protected_files: files,
                protected_trees: c.trees.clone(),
            },
            c.sources.clone(),
            stop,
        )
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        self.request_stop();
        if let Some(t) = self.thread.get_mut().unwrap().take() {
            if t.is_finished() {
                let _ = t.join();
            }
        }
    }
}
fn safe(kind: ErrorKind) -> Failure {
    match kind {
        ErrorKind::InvalidInput => "invalid_drc_request",
        ErrorKind::Busy => "review_changed",
        ErrorKind::Cache => "drc_changed_or_corrupt",
        ErrorKind::Cancelled => "drc_cancelled",
        ErrorKind::Incomplete => "drc_read_limit",
        ErrorKind::Io => "review_io_error",
        _ => "review_unavailable",
    }
}
fn progress(seq: u64, context: &Context, status: &managed::Status) -> Value {
    use managed::Phase;
    let phase = match status.phase {
        Phase::Publishing => "publishing",
        Phase::Cancelling => "cancelling",
        Phase::Succeeded => "succeeded",
        Phase::Failed => "failed",
        Phase::Cancelled => "cancelled",
    };
    json!({"seq":seq.to_string(),"kind":"drc_note","phase":phase,"context":context,
        "elapsed_ms":status.elapsed_ms.to_string(),"error":status.failure.map(safe),
        "published":if status.outcome_unknown {None} else {Some(status.outcome.is_some())},"outcome_unknown":status.outcome_unknown,
        "directory_synced":status.outcome.map(|o|o.directory_synced)})
}
fn execute(inner: &Inner, w: Work) -> Value {
    let context = w.context;
    let result = (|| -> Result<Value> {
        let mut job = w.draft.publish(w.confirm_legacy)?;
        loop {
            if w.stop.load(Ordering::Relaxed) != 0 {
                job.cancel();
            }
            let status = job.status();
            if matches!(
                status.phase,
                managed::Phase::Succeeded | managed::Phase::Failed | managed::Phase::Cancelled
            ) {
                job.close()?;
                return Ok(progress(w.seq, &context, &job.status()));
            }
            inner.state.lock().unwrap().ledger.update(
                w.seq,
                progress(w.seq, &context, &status),
                false,
            );
            thread::sleep(Duration::from_millis(20));
        }
    })();
    result.unwrap_or_else(|e| json!({"seq":w.seq.to_string(),"kind":"drc_note","context":context,
        "phase":if e.kind == ErrorKind::Cancelled {"cancelled"} else {"failed"},"error":safe(e.kind),
        "published":if e.kind == ErrorKind::Worker {None} else {Some(false)},"outcome_unknown":e.kind == ErrorKind::Worker}))
}
fn run(inner: Arc<Inner>) {
    loop {
        let w = {
            let mut s = inner.state.lock().unwrap();
            while s.pending.is_none() && !s.closed {
                s = inner.wake.wait(s).unwrap();
            }
            let Some(w) = s.pending.take() else {
                break;
            };
            w
        };
        let seq = w.seq;
        let mut value = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| execute(&inner, w)))
            .unwrap_or_else(|_| json!({"seq":seq.to_string(),"kind":"drc_note","phase":"failed", "error":"review_unavailable","published":null,"outcome_unknown":true}));
        let mut s = inner.state.lock().unwrap();
        if value["published"] == true || value["outcome_unknown"] == true {
            s.review_rev += 1;
        }
        value["review_rev"] = json!(s.review_rev.to_string());
        s.ledger.update(seq, value, true);
        s.stop = None;
    }
}

#[cfg(test)]
mod tests;
