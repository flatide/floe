//! Owner-only review. Fixed trusted reviewer/targets; opaque snapshots and
//! one-use prepared tokens. Native publication outlives HTTP subscribers.
mod display;
mod http;
mod transfer;
use super::{Failure, Service as Reader};
use crate::{
    auth::SessionId,
    operations::{Admission, Ledger},
};
use floe_app_core::{
    drc::review::{managed, store, NOTE_BYTES},
    registered::SourceSet,
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
    pub kind: store::Kind,
    pub editable: bool,
    pub read_target: Option<PathBuf>,
    // Read-only selection does not follow an approved pack replacement.
    pub reader_id: Option<String>,
    pub reviewer: String,
    pub files: Vec<PathBuf>,
    pub trees: Vec<PathBuf>,
    pub sources: Arc<SourceSet>,
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
    text: Option<String>,
    waived: Option<bool>,
}
impl Prepare {
    fn validate(&self, kind: store::Kind) -> std::result::Result<(), Failure> {
        match (kind, &self.text, self.waived) {
            (store::Kind::Notes, Some(text), None) if text.trim().len() <= NOTE_BYTES => Ok(()),
            (store::Kind::Notes, Some(_), None) => Err("drc_read_limit"),
            (store::Kind::Waives, None, Some(_)) => Ok(()),
            _ => Err("invalid_drc_request"),
        }
    }
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
    Prepared(
        managed::Prepared,
        Option<floe_app_core::exports::artifacts::Reservation>,
    ),
    Upload(transfer::Upload),
}
struct Ready {
    reader: Arc<Reader>,
    owner: SessionId,
    context: Context,
    token: String,
    model: Model,
    stop: Arc<AtomicUsize>,
    expires: Instant,
}
struct Work {
    reader: Arc<Reader>,
    seq: u64,
    context: Context,
    draft: managed::Prepared,
    confirm_legacy: bool,
    stop: Arc<AtomicUsize>,
    _charge: Option<floe_app_core::exports::artifacts::Reservation>,
}
struct State {
    closed: bool,
    detached: bool,
    serial: u64,
    review_rev: u64,
    preparing: Option<(u64, Arc<AtomicUsize>)>,
    ready: Option<Ready>,
    display: Option<Arc<display::Cache>>,
    pending: Option<Work>,
    stop: Option<Arc<AtomicUsize>>,
    ledger: Ledger,
    transfer: transfer::State,
    retired: Vec<Ready>,
}
struct Inner {
    config: Config,
    state: Mutex<State>,
    wake: Condvar,
    artifacts: Arc<floe_app_core::exports::artifacts::Store>,
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
    fn kind(&self) -> &'static str {
        kind_name(self.inner.config.kind)
    }
    pub(super) fn protected_targets(&self, pack: &std::path::Path) -> Result<Vec<PathBuf>> {
        let mut paths: Vec<PathBuf> = [store::Kind::Notes, store::Kind::Waives]
            .into_iter()
            .map(|kind| store::paths(pack, &self.inner.config.reviewer, kind))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect();
        if !self.inner.config.editable {
            for kind in [store::Kind::Notes, store::Kind::Waives] {
                paths.extend(store::read_paths(pack, &self.inner.config.reviewer, kind)?);
            }
        }
        Ok(paths)
    }
    pub(super) fn start(config: Config) -> Result<Arc<Self>> {
        if config.editable && config.read_target.is_some() {
            return Err(floe_app_core::Error::input(
                "a selected read target cannot grant editor authority",
            ));
        }
        let inner = Arc::new(Inner {
            config,
            state: Mutex::new(State {
                closed: false,
                detached: false,
                serial: 0,
                review_rev: 0,
                preparing: None,
                ready: None,
                display: None,
                pending: None,
                stop: None,
                ledger: Ledger::default(),
                transfer: transfer::State::default(),
                retired: Vec::new(),
            }),
            wake: Condvar::new(),
            artifacts: floe_app_core::exports::artifacts::Store::new(transfer::limits())?,
        });
        let task = Arc::clone(&inner);
        let thread = thread::Builder::new()
            .name(format!("floe-{}-owner", kind_name(inner.config.kind)))
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
        self.require_editor()?;
        self.begin_read(body, false)
    }
    fn begin_read(
        self: &Arc<Self>,
        body: Arc<OwnedSemaphorePermit>,
        display: bool,
    ) -> std::result::Result<Arc<Operation>, Failure> {
        let permit = Arc::clone(&self.preparations)
            .try_acquire_owned()
            .map_err(|_| "drc_busy")?;
        let mut s = self.inner.state.lock().unwrap();
        if s.closed {
            return Err("drc_closed");
        }
        if s.detached {
            return Err("review_disabled");
        }
        if s.ledger.active().is_some() || s.transfer.ledger.active().is_some() {
            return Err("drc_busy");
        }
        s.serial = s.serial.checked_add(1).ok_or("review_limit")?;
        if !display {
            transfer::retire(&mut s);
            self.inner.wake.notify_one();
            // Display must not make a user edit lose resource admission.
            s.display = None;
        }
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
        req.validate(self.inner.config.kind)?;
        let permit = Arc::clone(&self.preparations)
            .try_acquire_owned()
            .map_err(|_| "drc_busy")?;
        let mut s = self.inner.state.lock().unwrap();
        if s.closed {
            return Err("drc_closed");
        }
        if s.detached {
            return Err("review_disabled");
        }
        if s.ledger.active().is_some() || s.transfer.ledger.active().is_some() {
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
        reader: Arc<Reader>,
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
            reader,
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
        self.require_editor()?;
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
        if s.detached {
            return Err("review_disabled");
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
        let Model::Prepared(draft, _) = &r.model else {
            return Err("review_expired");
        };
        if draft.legacy_unverified() && !req.confirm_legacy {
            return Err("review_legacy_confirmation_required");
        }
        if s.review_rev == u64::MAX {
            return Err("review_limit");
        }
        match s.ledger.admit(seq, signature, self.kind())? {
            Admission::Replay(v) => return Ok(v),
            Admission::New => (),
        }
        let r = s.ready.take().unwrap();
        let Model::Prepared(draft, charge) = r.model else {
            unreachable!()
        };
        s.display = None;
        s.stop = Some(Arc::clone(&r.stop));
        s.pending = Some(Work {
            reader: r.reader,
            seq,
            context: req.context,
            draft,
            confirm_legacy: req.confirm_legacy,
            stop: r.stop,
            _charge: charge,
        });
        self.inner.wake.notify_one();
        Ok(s.ledger.get(seq).unwrap())
    }
    pub(super) fn status(&self) -> Value {
        let s = self.inner.state.lock().unwrap();
        let mut value = json!({"available":!s.closed && !s.detached,"detached":s.detached,"kind":self.kind(),"reviewer":self.inner.config.reviewer,
            "review_rev":s.review_rev.to_string(),"operations":s.ledger.snapshot(),
            "note_bytes":NOTE_BYTES,"selection_limit":floe_app_core::drc::review::EDIT_ITEMS,
            "preparing":s.preparing.is_some(),"autosave":false});
        if self.inner.config.kind == store::Kind::Notes {
            value["editable"] = json!(self.inner.config.editable && !s.detached);
        }
        value
    }
    fn require_editor(&self) -> std::result::Result<(), Failure> {
        if self.inner.config.editable {
            Ok(())
        } else {
            Err("review_disabled")
        }
    }
    fn revoke(&self, owner: &SessionId, token: &str) {
        let mut s = self.inner.state.lock().unwrap();
        if s.ready
            .as_ref()
            .is_some_and(|r| r.owner == *owner && r.token == token)
        {
            transfer::retire(&mut s);
            self.inner.wake.notify_one();
        }
    }
    pub(super) fn maintain(&self) {
        let mut s = self.inner.state.lock().unwrap();
        if s.ready
            .as_ref()
            .is_some_and(|r| r.expires <= Instant::now() || r.stop.load(Ordering::Relaxed) != 0)
        {
            transfer::retire(&mut s);
            self.inner.wake.notify_one();
        }
        s.transfer
            .artifacts
            .retain(|id, _| self.inner.artifacts.info(*id).is_some());
    }
    /// Registry -> review lock order. Builds reject active preparation/write;
    /// an unapproved preview is discarded before the build takes a write lease.
    pub(super) fn admit_build<T>(
        &self,
        f: impl FnOnce() -> std::result::Result<T, Failure>,
    ) -> std::result::Result<T, Failure> {
        self.admit_change(false, f)
    }
    pub(super) fn admit_detach<T>(
        &self,
        f: impl FnOnce() -> std::result::Result<T, Failure>,
    ) -> std::result::Result<T, Failure> {
        self.admit_change(true, f)
    }
    fn admit_change<T>(
        &self,
        detach: bool,
        f: impl FnOnce() -> std::result::Result<T, Failure>,
    ) -> std::result::Result<T, Failure> {
        let mut s = self.inner.state.lock().unwrap();
        if s.preparing.is_some()
            || self.preparations.available_permits() == 0
            || s.ledger.active().is_some()
            || s.transfer.ledger.active().is_some()
            || !s.retired.is_empty()
            || s.ready
                .as_ref()
                .is_some_and(|r| matches!(&r.model, Model::Upload(_) | Model::Prepared(_, Some(_))))
        {
            return Err("drc_busy");
        }
        let result = f()?;
        s.detached |= detach;
        transfer::retire(&mut s);
        self.inner.wake.notify_one();
        for id in s.transfer.artifacts.keys() {
            self.inner.artifacts.request_release(*id);
        }
        s.transfer.artifacts.clear();
        s.display = None;
        Ok(result)
    }
    pub(super) fn request_stop(&self) {
        let mut s = self.inner.state.lock().unwrap();
        s.closed = true;
        transfer::retire(&mut s);
        s.display = None;
        if let Some((_, stop)) = &s.preparing {
            stop.store(1, Ordering::Relaxed);
        }
        if let Some(stop) = &s.stop {
            stop.store(1, Ordering::Relaxed);
        }
        self.inner.artifacts.request_close();
        self.inner.wake.notify_all();
    }
    pub(super) fn is_finished(&self) -> bool {
        self.thread
            .lock()
            .unwrap()
            .as_ref()
            .is_none_or(JoinHandle::is_finished)
            && self.preparations.available_permits() == 1
            && self.inner.artifacts.usage().readers == 0
            && self.inner.artifacts.disposal_finished()
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
        if self.inner.state.lock().unwrap().detached {
            return Err(floe_app_core::Error::input(
                "review is detached from the current DRC",
            ));
        }
        let r = &reader.registration;
        let c = &self.inner.config;
        if c.reader_id.as_ref().is_some_and(|id| *id != reader.id) {
            return Err(floe_app_core::Error::input(
                "read-only review registration changed; reopen explicitly",
            ));
        }
        if c.kind == store::Kind::Waives
            && r.waives.as_ref().is_some_and(|p| {
                store::paths(&r.path, &c.reviewer, c.kind).is_ok_and(|paths| *p != paths[0])
            })
        {
            return Err(floe_app_core::Error::input(
                "waive write target differs from registered read sidecar",
            ));
        }
        let files = c
            .files
            .iter()
            .cloned()
            .chain(std::iter::once(r.path.clone()))
            .chain(r.waives.clone().filter(|_| c.kind == store::Kind::Notes))
            .chain(r.rules.clone())
            .collect();
        let registration = managed::Registration {
            scope: Arc::clone(&r.scope),
            pack: r.path.clone(),
            reviewer: c.reviewer.clone(),
            kind: c.kind,
            protected_files: files,
            protected_trees: c.trees.clone(),
        };
        if let Some(target) = &c.read_target {
            managed::ManagedStore::open_readonly_catalog(
                &r.resources,
                registration,
                c.sources.clone(),
                target,
                stop,
            )
        } else {
            managed::ManagedStore::open_catalog(&r.resources, registration, c.sources.clone(), stop)
        }
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
    json!({"seq":seq.to_string(),"kind":kind_name(status.kind),"phase":phase,"context":context,
        "elapsed_ms":status.elapsed_ms.to_string(),"error":status.failure.map(safe),
        "published":if status.outcome_unknown {None} else {Some(status.outcome.is_some())},"outcome_unknown":status.outcome_unknown,
        "directory_synced":status.outcome.map(|o|o.directory_synced)})
}
fn kind_name(kind: store::Kind) -> &'static str {
    match kind {
        store::Kind::Notes => "drc_note",
        store::Kind::Waives => "drc_waive",
    }
}
fn unknown_progress(seq: u64, context: &Context, kind: store::Kind) -> Value {
    let mut value = json!({"seq":seq.to_string(),"kind":kind_name(kind),"context":context,
        "phase":"failed","error":"review_unavailable","published":null,"outcome_unknown":true});
    if kind == store::Kind::Waives {
        value["reader_applied"] = Value::Null;
    }
    value
}
fn reader_result(value: &mut Value, result: std::result::Result<Vec<u8>, Failure>) {
    match result {
        Ok(_) => value["reader_applied"] = json!(true),
        Err(e) => {
            value["reader_applied"] = if e == "drc_apply_unknown" {
                Value::Null
            } else {
                json!(false)
            };
            value["reader_error"] = json!(e);
        }
    }
}
fn execute(inner: &Inner, w: Work) -> Value {
    let _charge = w._charge;
    let context = w.context;
    let kind = inner.config.kind;
    // The writer owns this boundary, not an HTTP subscriber. A cancelled or
    // failed publication still retires old read results without changing pixels.
    let change = if kind == store::Kind::Waives {
        match w.reader.begin_waive_write(&context.revision) {
            Ok(c) => Some(c),
            Err(e) => {
                return json!({"seq":w.seq.to_string(),"kind":kind_name(kind),"context":context,
                "phase":"failed","error":e,"published":false,"outcome_unknown":false,"reader_applied":false})
            }
        }
    } else {
        None
    };
    let result = (|| -> Result<Value> {
        let store = w.draft.store();
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
                let status = job.status();
                let mut value = progress(w.seq, &context, &status);
                if kind == store::Kind::Waives {
                    value["reader_applied"] = json!(false);
                    if let Some(published) = status.outcome {
                        // A successful file commit stays successful even when
                        // refresh fails, is cancelled, or its ACK is uncertain.
                        let mut refreshing = value.clone();
                        refreshing["phase"] = json!("refreshing_reader");
                        inner
                            .state
                            .lock()
                            .unwrap()
                            .ledger
                            .update(w.seq, refreshing, false);
                        let applied = store
                            .snapshot_published(&published, Arc::clone(&w.stop))
                            .map_err(|e| safe(e.kind))
                            .and_then(|snapshot| {
                                w.reader.apply_waives_in_change(snapshot, change.unwrap())
                            })
                            .and_then(|mut ticket| ticket.blocking_apply_result());
                        reader_result(&mut value, applied);
                    }
                    value["reader_revision"] = json!(w.reader.revision());
                }
                return Ok(value);
            }
            inner.state.lock().unwrap().ledger.update(
                w.seq,
                progress(w.seq, &context, &status),
                false,
            );
            thread::sleep(Duration::from_millis(20));
        }
    })();
    let mut value = result.unwrap_or_else(|e| json!({"seq":w.seq.to_string(),"kind":kind_name(kind),"context":context,
        "phase":if e.kind == ErrorKind::Cancelled {"cancelled"} else {"failed"},"error":safe(e.kind),
        "published":if e.kind == ErrorKind::Worker {None} else {Some(false)},"outcome_unknown":e.kind == ErrorKind::Worker}));
    if kind == store::Kind::Waives {
        if value.get("reader_applied").is_none() {
            value["reader_applied"] = json!(false);
        }
        value["reader_revision"] = json!(w.reader.revision());
    }
    value
}
fn run(inner: Arc<Inner>) {
    loop {
        let w = {
            let mut s = inner.state.lock().unwrap();
            while s.pending.is_none()
                && s.transfer.pending.is_none()
                && s.retired.is_empty()
                && !s.closed
            {
                s = inner.wake.wait(s).unwrap();
            }
            if !s.retired.is_empty() {
                let retired = std::mem::take(&mut s.retired);
                drop(s);
                drop(retired); // potentially large unlinked files close off-reactor
                continue;
            }
            if let Some(task) = s.transfer.pending.take() {
                drop(s);
                transfer::run(&inner, task);
                continue;
            }
            let Some(w) = s.pending.take() else {
                break;
            };
            w
        };
        let seq = w.seq;
        let context = w.context.clone();
        let mut value =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| execute(&inner, w)))
                .unwrap_or_else(|_| unknown_progress(seq, &context, inner.config.kind));
        let mut s = inner.state.lock().unwrap();
        if value["published"] == true || value["outcome_unknown"] == true {
            s.review_rev += 1;
            // Older downloads are no longer authoritative. Revoke now rather
            // than letting unreachable artifacts consume capacity until TTL.
            for id in s.transfer.artifacts.keys() {
                inner.artifacts.request_release(*id);
            }
            s.transfer.artifacts.clear();
        }
        value["review_rev"] = json!(s.review_rev.to_string());
        s.ledger.update(seq, value, true);
        s.stop = None;
    }
}

#[cfg(test)]
mod tests;
