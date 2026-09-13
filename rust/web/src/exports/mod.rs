//! Owner-only exact export jobs. No guest authority, source-path input, or
//! native diagnostics in replies. Long work runs on a separate service thread.
mod http;
mod prepared;
use crate::{
    operations::{Admission, Ledger},
    transport::Attachment,
    view,
};
use floe_app_core::{
    clip::ClipOptions,
    exports::{
        artifacts::{self, Store},
        clip::{Job, Snapshot},
    },
    managed::{ManagedDataset, Resources},
    registered::RegisteredSource,
    render::RenderOptions,
    ErrorKind, Result,
};
use floe_worker_client::ClipRequest;
pub(crate) use http::routes;
pub(crate) use prepared::{Drafts, PrepareDto};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Condvar, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Submit {
    seq: String,
    view_id: String,
    token: String,
    approve: bool,
}
struct Work {
    seq: u64,
    view_id: String,
    source: Arc<RegisteredSource>,
    data: Arc<ManagedDataset>,
    request: ClipRequest,
    stop: Arc<AtomicUsize>,
}
struct State {
    closed: bool,
    ledger: Ledger,
    pending: Option<Work>,
    stop: Option<Arc<AtomicUsize>>,
}
struct Inner {
    resources: Arc<Resources>,
    options: ClipOptions,
    store: Arc<Store>,
    state: Mutex<State>,
    wake: Condvar,
}
pub(crate) struct Service {
    inner: Arc<Inner>,
    thread: Mutex<Option<JoinHandle<()>>>,
}
impl Service {
    pub fn start(resources: Arc<Resources>, render: &RenderOptions) -> Result<Arc<Self>> {
        let inner = Arc::new(Inner {
            resources,
            options: ClipOptions {
                binary: render.binary.clone(),
                jobs: 4,
                budget_mb: render.budget_mb,
                open_timeout_s: render.open_timeout_s,
                clip_timeout_s: 300,
            },
            store: Store::new(artifacts::Limits::default())?,
            state: Mutex::new(State {
                closed: false,
                ledger: Ledger::default(),
                pending: None,
                stop: None,
            }),
            wake: Condvar::new(),
        });
        let task = Arc::clone(&inner);
        let thread = thread::Builder::new()
            .name("floe-export-owner".into())
            .spawn(move || run(task))?;
        Ok(Arc::new(Self {
            inner,
            thread: Mutex::new(Some(thread)),
        }))
    }
    pub fn store(&self) -> &Arc<Store> {
        &self.inner.store
    }
    pub fn status(&self) -> Value {
        let s = self.inner.state.lock().unwrap();
        let mut ledger = s.ledger.snapshot();
        for row in ledger["history"].as_array_mut().unwrap() {
            self.decorate(row);
        }
        let usage = self.inner.store.usage();
        let artifacts=self.inner.store.inventory().into_iter().map(|(id,i)|json!({
            "id":id.to_string(),"bytes":i.size_bytes.to_string(),"expires_in_ms":i.expires_in_ms.to_string(),"name":format!("floe-clip-{id}.oas")
        })).collect::<Vec<_>>();
        json!({"operations":ledger,"available":!s.closed,"kind":"exact_clip","jobs_default":4,"jobs_min":1,"jobs_max":16,
            "artifacts":artifacts,
            "limits":{"artifacts":4,"artifact_bytes":"536870912","total_bytes":"2147483648","readers":2,"ttl_seconds":600},
            "usage":{"entries":usage.entries,"pending":usage.pending,"bytes":usage.bytes.to_string(),"readers":usage.readers}})
    }
    fn decorate(&self, state: &mut Value) {
        if let Some(id) = state["artifact"]["id"]
            .as_str()
            .and_then(|s| view::counter(s).ok())
        {
            let info = self.inner.store.info(id);
            state["artifact"]["available"] = json!(info.is_some());
            state["artifact"]["expires_in_ms"] = json!(info.map(|i| i.expires_in_ms.to_string()));
        }
    }
    pub fn operation(&self, seq: u64) -> Option<Value> {
        let mut state = self.inner.state.lock().unwrap().ledger.get(seq)?;
        self.decorate(&mut state);
        Some(state)
    }
    pub fn submit(
        &self,
        req: Submit,
        attached: Option<Arc<Attachment>>,
        source: Option<Arc<RegisteredSource>>,
    ) -> std::result::Result<Value, &'static str> {
        let seq = view::counter(&req.seq)?;
        if !req.approve || req.token.len() != 64 || req.view_id.len() != 64 {
            return Err("export_approval_required");
        }
        let signature = format!("{req:?}");
        let mut s = self.inner.state.lock().unwrap();
        if s.closed {
            return Err("closed");
        }
        // A retry stays a replay even after view close, token use, or expiry.
        if let Some(mut replay) = s.ledger.replay(seq, &signature)? {
            self.decorate(&mut replay);
            return Ok(replay);
        }
        let attached = attached
            .filter(|v| v.id == req.view_id)
            .ok_or("view_unavailable")?;
        let source = source.ok_or("source_unavailable")?;
        let mut drafts = attached.clips.lock().unwrap();
        let draft = drafts.check(&req.token)?;
        let data = attached.controller.pin_clip(draft.anchor).map_err(safe)?;
        if data.dataset.source() != source.path() {
            return Err("source_unavailable");
        }
        let request = draft.request.clone();
        match s.ledger.admit(seq, signature, "exact_clip")? {
            Admission::Replay(v) => return Ok(v),
            Admission::New => (),
        }
        drafts.consume();
        let stop = Arc::new(AtomicUsize::new(0));
        s.stop = Some(Arc::clone(&stop));
        s.pending = Some(Work {
            seq,
            view_id: req.view_id,
            source,
            data,
            request,
            stop,
        });
        let state = s.ledger.get(seq).unwrap();
        self.inner.wake.notify_one();
        Ok(state)
    }
    pub fn cancel(&self, seq: u64) -> std::result::Result<Value, &'static str> {
        let s = self.inner.state.lock().unwrap();
        let mut state = s.ledger.get(seq).ok_or("operation_expired")?;
        if s.ledger.active() == Some(seq) {
            if let Some(stop) = &s.stop {
                stop.store(1, Ordering::Relaxed);
            }
        }
        self.decorate(&mut state);
        Ok(state)
    }
    pub fn request_stop(&self) {
        let mut s = self.inner.state.lock().unwrap();
        s.closed = true;
        if let Some(stop) = &s.stop {
            stop.store(1, Ordering::Relaxed);
        }
        self.inner.store.request_close();
        self.inner.wake.notify_one();
    }
    pub fn is_finished(&self) -> bool {
        self.thread
            .lock()
            .unwrap()
            .as_ref()
            .is_none_or(|t| t.is_finished())
            && self.inner.store.usage().readers == 0
            && self.inner.store.disposal_finished()
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        self.request_stop();
        if let Some(t) = self.thread.get_mut().unwrap().take() {
            let _ = t.join();
        }
    }
}
fn safe(e: floe_app_core::Error) -> &'static str {
    if e.kind == ErrorKind::Busy {
        "stale_frame"
    } else {
        view::safe_error(e.kind)
    }
}
fn run(inner: Arc<Inner>) {
    loop {
        let work = {
            let mut s = inner.state.lock().unwrap();
            while !s.closed && s.pending.is_none() {
                s = inner.wake.wait(s).unwrap();
            }
            let Some(work) = s.pending.take() else {
                break;
            };
            work
        };
        let seq = work.seq;
        let result = execute(&inner, work);
        let mut s = inner.state.lock().unwrap();
        let terminal = result.unwrap_or_else(|e|json!({"seq":seq.to_string(),"kind":"exact_clip","phase":if e.kind==ErrorKind::Cancelled{"cancelled"}else{"failed"},"error":view::safe_error(e.kind)}));
        // Publish terminal state and clear its cancellation handle in one
        // critical section, before admitting the next operation.
        s.ledger.update(seq, terminal, true);
        s.stop = None;
    }
}
fn execute(inner: &Inner, work: Work) -> Result<Value> {
    floe_app_core::check_cancelled(&work.stop)?;
    let mut options = inner.options.clone();
    options.jobs = work.request.jobs;
    let mut job = Job::start(
        &inner.resources,
        &inner.store,
        work.source,
        work.data,
        work.request,
        options,
    )?;
    loop {
        if work.stop.load(Ordering::Relaxed) != 0 {
            job.cancel();
        }
        let snapshot = job.snapshot();
        let terminal = snapshot.terminal();
        if terminal || job.is_finished() {
            job.join()?;
            let snapshot = job.snapshot();
            if !snapshot.terminal() {
                return Err(floe_app_core::Error::new(
                    ErrorKind::Worker,
                    "export ended without terminal state",
                ));
            }
            return Ok(dto(work.seq, &work.view_id, &snapshot));
        }
        inner.state.lock().unwrap().ledger.update(
            work.seq,
            dto(work.seq, &work.view_id, &snapshot),
            false,
        );
        thread::sleep(Duration::from_millis(10));
    }
}
fn dto(seq: u64, view_id: &str, s: &Snapshot) -> Value {
    use floe_app_core::exports::clip::Phase;
    let phase = match s.phase {
        Phase::Preparing => "preparing",
        Phase::Opening => "opening",
        Phase::Clipping => "clipping",
        Phase::Finishing => "finishing",
        Phase::Cancelling => "cancelling",
        Phase::Ready => "ready",
        Phase::Failed => "failed",
        Phase::Cancelled => "cancelled",
    };
    json!({"seq":seq.to_string(),"kind":"exact_clip","view_id":view_id,"dataset_revision":s.dataset_revision.to_string(),"phase":phase,"elapsed_ms":s.elapsed_ms.to_string(),"error":s.failure.map(view::safe_error),
        "artifact":s.outcome.as_ref().map(|o|json!({"id":o.artifact_id.to_string(),"bytes":o.bytes.to_string(),"records":o.records.to_string(),"bbox_dbu":o.bbox_dbu.map(|n|n.to_string()),"source_stale":o.source_stale,"name":format!("floe-clip-{}.oas",o.artifact_id)}))})
}
