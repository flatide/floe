//! Explicit owner design-default publication. A draft never writes; acceptance
//! freezes the approved state and one bounded actor owns publication/receipts.
mod http;
use crate::{
    auth::SessionId,
    operations::{Admission, Ledger},
    view,
};
use floe_app_core::{
    layer_defaults::{Draft, Publisher, Recovery, RecoveryResult, RecoveryState},
    ErrorKind, Result,
};
pub(crate) use http::routes;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Condvar, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Prepare {
    view_id: String,
    state_rev: String,
    #[serde(default)]
    recover: bool,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Submit {
    seq: String,
    view_id: String,
    state_rev: String,
    token: String,
    #[serde(default)]
    approve: bool,
    #[serde(default)]
    approve_recovery: bool,
}
enum Prepared {
    Publish(Box<Draft>),
    Recover(Arc<Recovery>),
}
impl From<Draft> for Prepared {
    fn from(d: Draft) -> Self {
        Self::Publish(Box::new(d))
    }
}
impl Prepared {
    fn target(&self) -> &std::path::Path {
        match self {
            Self::Publish(d) => d.target(),
            Self::Recover(d) => d.target(),
        }
    }
    fn bytes(&self) -> usize {
        match self {
            Self::Publish(d) => d.bytes(),
            Self::Recover(d) => d.bytes(),
        }
    }
    fn recovery(&self) -> bool {
        matches!(self, Self::Recover(_))
    }
}
struct Ready {
    token: String,
    owner: SessionId,
    view_id: String,
    rev: u64,
    draft: Prepared,
    expires: Instant,
    name: String,
}
struct Work {
    seq: u64,
    view_id: String,
    rev: u64,
    name: String,
    draft: Prepared,
    check: bool,
    stop: Arc<AtomicUsize>,
}
struct State {
    closed: bool,
    serial: u64,
    ready: Option<Ready>,
    ledger: Ledger,
    pending: Option<Work>,
    stop: Option<Arc<AtomicUsize>>,
    held: Option<(u64, Arc<Recovery>)>,
}
struct Inner {
    state: Mutex<State>,
    wake: Condvar,
}
pub(crate) struct Service {
    publisher: Arc<Publisher>,
    inner: Arc<Inner>,
    thread: Mutex<Option<JoinHandle<()>>>,
    preparations: Arc<Semaphore>,
}
impl Service {
    pub fn start(publisher: Arc<Publisher>) -> Result<Arc<Self>> {
        let inner = Arc::new(Inner {
            state: Mutex::new(State {
                closed: false,
                serial: 0,
                ready: None,
                ledger: Ledger::default(),
                pending: None,
                stop: None,
                held: None,
            }),
            wake: Condvar::new(),
        });
        let task = Arc::clone(&inner);
        let thread = thread::Builder::new()
            .name("floe-default-owner".into())
            .spawn(move || run(task))?;
        Ok(Arc::new(Self {
            publisher,
            inner,
            thread: Mutex::new(Some(thread)),
            preparations: Arc::new(Semaphore::new(1)),
        }))
    }
    fn begin(&self) -> std::result::Result<u64, &'static str> {
        let mut s = self.inner.state.lock().unwrap();
        if s.closed {
            return Err("closed");
        }
        if s.ledger.active().is_some() {
            return Err("busy");
        }
        s.serial = s.serial.checked_add(1).ok_or("default_draft_limit")?;
        s.ready = None;
        Ok(s.serial)
    }
    fn finish(
        &self,
        serial: u64,
        owner: SessionId,
        request: Prepare,
        draft: impl Into<Prepared>,
    ) -> std::result::Result<Value, &'static str> {
        let draft = draft.into();
        let mut s = self.inner.state.lock().unwrap();
        if s.closed || serial != s.serial {
            return Err("default_draft_expired");
        }
        let token = crate::auth::public_id().map_err(|_| "default_unavailable")?;
        let name = draft
            .target()
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or("default_unavailable")?
            .to_owned();
        let rev = view::counter(&request.state_rev)?;
        if request.recover != draft.recovery() {
            return Err("default_approval_required");
        }
        let mut result = json!({"token":token,"view_id":request.view_id,"state_rev":request.state_rev,"name":name,"bytes":draft.bytes().to_string(),"replaces_existing":matches!(&draft,Prepared::Publish(d) if d.replaces_existing()),"expires_in_ms":"30000","scope":"shared_design_default","affects":"future_opens"});
        if draft.recovery() {
            result["recover"] = json!(true);
        }
        s.ready = Some(Ready {
            token,
            owner,
            view_id: request.view_id,
            rev,
            draft,
            expires: Instant::now() + Duration::from_secs(30),
            name,
        });
        Ok(result)
    }
    fn submit(
        &self,
        owner: &SessionId,
        req: Submit,
        views: &crate::service::Service,
    ) -> std::result::Result<Value, &'static str> {
        let seq = view::counter(&req.seq)?;
        let rev = view::counter(&req.state_rev)?;
        if req.approve == req.approve_recovery || req.token.len() != 64 || req.view_id.len() != 64 {
            return Err("default_approval_required");
        }
        let signature = format!("{} {req:?}", owner.as_str());
        let mut s = self.inner.state.lock().unwrap();
        if s.closed {
            return Err("closed");
        }
        if let Some(replay) = s.ledger.replay(seq, &signature)? {
            return Ok(replay);
        }
        let r = s
            .ready
            .as_ref()
            .filter(|r| r.token == req.token && r.owner == *owner)
            .ok_or("default_draft_expired")?;
        if r.draft.recovery() != req.approve_recovery {
            return Err("default_approval_required");
        }
        if Instant::now() >= r.expires {
            s.ready = None;
            return Err("default_draft_expired");
        }
        if r.view_id != req.view_id || r.rev != rev {
            s.ready = None;
            return Err("stale_state");
        }
        // Lock order defaults -> owner -> controller. The acceptance point is
        // this current-state check; later navigation does not alter the bytes
        // explicitly approved here, and no filesystem work holds these locks.
        let admission = views.with_current(&req.view_id, |v| {
            if v.controller.snapshot().state_rev != rev || v.controller.is_finished() {
                return Err("stale_state");
            }
            s.ledger.admit(seq, signature, "design_default")
        });
        match admission {
            Ok(Admission::New) => (),
            Ok(Admission::Replay(v)) => return Ok(v),
            Err(e) => {
                if matches!(e, "stale_state" | "view_unavailable") {
                    s.ready = None;
                }
                return Err(e);
            }
        }
        let r = s.ready.take().unwrap();
        let stop = Arc::new(AtomicUsize::new(0));
        s.stop = Some(Arc::clone(&stop));
        s.pending = Some(Work {
            seq,
            view_id: req.view_id,
            rev,
            name: r.name,
            draft: r.draft,
            check: false,
            stop,
        });
        self.inner.wake.notify_one();
        Ok(s.ledger.get(seq).unwrap())
    }
    pub fn status(&self) -> Value {
        let mut s = self.inner.state.lock().unwrap();
        if s.ready
            .as_ref()
            .is_some_and(|r| Instant::now() >= r.expires)
        {
            s.ready = None;
        }
        json!({"available":!s.closed,"operations":s.ledger.snapshot(),"kind":"design_default","scope":"shared_design_default","max_bytes":"4194304","jobs":1})
    }
    pub fn maintain(&self) {
        let mut s = self.inner.state.lock().unwrap();
        if s.ready
            .as_ref()
            .is_some_and(|r| Instant::now() >= r.expires)
        {
            s.ready = None;
        }
    }
    pub fn operation(&self, seq: u64) -> Option<Value> {
        self.inner.state.lock().unwrap().ledger.get(seq)
    }
    fn revoke(&self, owner: &SessionId, token: &str) {
        let mut s = self.inner.state.lock().unwrap();
        if s.ready
            .as_ref()
            .is_some_and(|r| r.token == token && r.owner == *owner)
        {
            s.ready = None;
        }
    }
    fn cancel(&self, seq: u64) -> std::result::Result<Value, &'static str> {
        let s = self.inner.state.lock().unwrap();
        let v = s.ledger.get(seq).ok_or("operation_expired")?;
        if s.ledger.active() == Some(seq) {
            if let Some(stop) = &s.stop {
                stop.store(1, Ordering::Relaxed);
            }
        }
        Ok(v)
    }
    fn reconcile(&self, seq: u64) -> std::result::Result<Value, &'static str> {
        let mut s = self.inner.state.lock().unwrap();
        if s.closed {
            return Err("closed");
        }
        let receipt = s.ledger.get(seq).ok_or("operation_expired")?;
        if receipt["phase"] != "uncertain" {
            return Ok(receipt);
        }
        let proof = s
            .held
            .as_ref()
            .filter(|(n, _)| *n == seq)
            .map(|(_, p)| Arc::clone(p))
            .ok_or("publication_unknown")?;
        if s.pending.is_some() {
            return Err("busy");
        }
        let stop = Arc::new(AtomicUsize::new(0));
        s.stop = Some(Arc::clone(&stop));
        s.pending = Some(Work {
            seq,
            view_id: receipt["view_id"].as_str().unwrap().into(),
            rev: view::counter(receipt["state_rev"].as_str().unwrap())?,
            name: receipt["name"].as_str().unwrap().into(),
            draft: Prepared::Recover(proof),
            check: true,
            stop,
        });
        let mut checking = receipt;
        checking["phase"] = json!("publishing");
        checking.as_object_mut().unwrap().remove("error");
        checking.as_object_mut().unwrap().remove("published");
        checking.as_object_mut().unwrap().remove("recovered");
        s.ledger.update(seq, checking.clone(), false);
        self.inner.wake.notify_one();
        Ok(checking)
    }
    pub fn request_stop(&self) {
        let mut s = self.inner.state.lock().unwrap();
        s.closed = true;
        s.ready = None;
        if let Some(stop) = &s.stop {
            stop.store(1, Ordering::Relaxed);
        }
        self.inner.wake.notify_all();
    }
    pub fn is_finished(&self) -> bool {
        self.thread
            .lock()
            .unwrap()
            .as_ref()
            .is_none_or(|t| t.is_finished())
            && self.preparations.available_permits() == 1
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
fn run(inner: Arc<Inner>) {
    loop {
        let work = {
            let mut s = inner.state.lock().unwrap();
            while !s.closed && s.pending.is_none() {
                s = inner.wake.wait(s).unwrap();
            }
            let Some(w) = s.pending.take() else {
                break;
            };
            let mut progress = json!({"seq":w.seq.to_string(),"kind":"design_default","phase":"publishing","view_id":w.view_id,"state_rev":w.rev.to_string(),"name":w.name});
            if w.draft.recovery() {
                progress["recover"] = json!(true);
            }
            s.ledger.update(w.seq, progress, false);
            w
        };
        let Work {
            seq,
            view_id,
            rev,
            name,
            draft,
            check,
            stop,
        } = work;
        let proof = match &draft {
            Prepared::Recover(p) => Some(Arc::clone(p)),
            _ => None,
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match draft {
            Prepared::Publish(d) => d.publish(&stop).map(|p| Some(p.directory_synced)),
            Prepared::Recover(p) if check => match p.reconcile(&stop) {
                Ok(RecoveryState::Completed) => Ok(Some(false)), // inspection is not a durability claim
                Ok(RecoveryState::Pending) => Err(floe_app_core::Error::new(
                    ErrorKind::Busy,
                    "repair not completed; prepare again",
                )),
                Err(_) => Ok(None),
            },
            Prepared::Recover(p) => p.recover(&stop).map(|r| match r {
                RecoveryResult::Recovered { directory_synced } => Some(directory_synced),
                RecoveryResult::Uncertain => None,
            }),
        }))
        .unwrap_or_else(|_| {
            Err(floe_app_core::Error::new(
                ErrorKind::PublicationUnknown,
                "publication worker ended unexpectedly",
            ))
        });
        let unknown = matches!(&result, Ok(None))
            || matches!(&result, Err(e) if e.kind == ErrorKind::PublicationUnknown);
        let mut value = json!({"seq":seq.to_string(),"kind":"design_default","view_id":view_id,"state_rev":rev.to_string(),"name":name});
        if proof.is_some() {
            value["recover"] = json!(true);
        }
        if unknown {
            value["phase"] = json!("uncertain");
            value["published"] = Value::Null;
            value["error"] = json!("publication_unknown");
        } else {
            match result {
                Ok(Some(synced)) => {
                    value["phase"] = json!("succeeded");
                    value["published"] = json!(proof.is_none());
                    value["directory_synced"] = json!(synced);
                    if proof.is_some() {
                        value["recovered"] = json!(true);
                    }
                }
                Err(e) => {
                    value["phase"] = json!(if e.kind == ErrorKind::Cancelled {
                        "cancelled"
                    } else {
                        "failed"
                    });
                    value["published"] = json!(false);
                    value["error"] = json!(safe(e.kind));
                }
                Ok(None) => unreachable!(),
            }
        }
        let mut s = inner.state.lock().unwrap();
        s.held = if unknown {
            proof.map(|p| (seq, p))
        } else {
            None
        };
        s.ledger.update(seq, value, !unknown);
        s.stop = None;
    }
}
fn safe(kind: ErrorKind) -> &'static str {
    if kind == ErrorKind::Busy {
        "default_changed"
    } else {
        view::safe_error(kind)
    }
}

#[cfg(test)]
mod tests;
