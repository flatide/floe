//! Registered, read-only DRC queries. One actor owns the pack; HTTP owns only
//! bounded DTOs/tickets. No path/reviewer/native command is accepted on the wire.
mod dto;
mod http;
pub(crate) mod panel;
mod read;
mod selection;
pub use dto::Request;
use floe_app_core::{
    drc::Pack, managed::Resources, registered::AccessScope, Error, ErrorKind, Result,
};
pub(crate) use http::routes;
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Condvar, Mutex,
    },
    thread::{self, JoinHandle},
};
use tokio::sync::oneshot;

const QUEUE: usize = 4;
pub const RESPONSE_BYTES: usize = 1024 * 1024;
pub type Failure = &'static str;
struct Work {
    request: dto::Command,
    stop: Arc<AtomicUsize>,
    reply: oneshot::Sender<std::result::Result<Vec<u8>, Failure>>,
}
struct State {
    pending: VecDeque<Work>,
    active: Option<Arc<AtomicUsize>>,
    closed: bool,
    failure: Option<Failure>,
    metadata: Option<Value>,
}
struct Inner {
    state: Mutex<State>,
    wake: Condvar,
}
pub struct Service {
    pub id: String,
    pub revision: String,
    pub source_id: String,
    title: String,
    inner: Arc<Inner>,
    thread: Mutex<Option<JoinHandle<()>>>,
}
/// Cancelling a handler (including timeout/disconnect) cancels its queued or
/// active work. A reply does not keep the actor or an HTTP connection alive.
pub struct Ticket {
    reply: oneshot::Receiver<std::result::Result<Vec<u8>, Failure>>,
    stop: Arc<AtomicUsize>,
}
impl Ticket {
    pub async fn result(&mut self) -> std::result::Result<Vec<u8>, Failure> {
        (&mut self.reply).await.unwrap_or(Err("drc_closed"))
    }
}
impl Drop for Ticket {
    fn drop(&mut self) {
        self.stop.store(1, Ordering::Relaxed);
    }
}
impl Service {
    /// Local launcher only. Pack and optional waive paths must be explicitly
    /// authorized; no ambient reviewer/temp lookup and no autosave creation.
    pub fn start(
        resources: &Arc<Resources>,
        scope: Arc<AccessScope>,
        path: &Path,
        waives: Option<&Path>,
        source_id: &str,
    ) -> Result<Arc<Self>> {
        if source_id.is_empty() || source_id.len() > 128 {
            return Err(Error::input("invalid registered DRC source"));
        }
        let path = scope.check(path)?;
        let waives = waives.map(|p| scope.check(p)).transpose()?;
        let permit = resources.drc(std::iter::once(path.clone()).chain(waives.clone()))?;
        let id = crate::auth::public_id()
            .map_err(|_| Error::new(ErrorKind::Io, "entropy unavailable"))?;
        let revision = crate::auth::public_id()
            .map_err(|_| Error::new(ErrorKind::Io, "entropy unavailable"))?;
        let stop = Arc::new(AtomicUsize::new(0));
        let inner = Arc::new(Inner {
            state: Mutex::new(State {
                pending: VecDeque::new(),
                active: Some(Arc::clone(&stop)),
                closed: false,
                failure: None,
                metadata: None,
            }),
            wake: Condvar::new(),
        });
        let service = Arc::new(Self {
            id,
            revision,
            source_id: source_id.into(),
            title: path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("DRC")
                .chars()
                .take(256)
                .collect(),
            inner: Arc::clone(&inner),
            thread: Mutex::new(None),
        });
        let handle = thread::Builder::new()
            .name("floe-drc-read".into())
            .spawn(move || {
                let _permit = permit;
                let opened: Result<Pack> = (|| {
                    scope.check(&path)?;
                    let mut pack = Pack::open(&path, &stop)?;
                    if let Some(p) = &waives {
                        scope.check(p)?;
                        pack.attach_waives(p)?;
                    }
                    floe_app_core::check_cancelled(&stop)?;
                    pack.unchanged()?;
                    Ok(pack)
                })();
                match opened {
                    Ok(mut pack) => {
                        inner.state.lock().unwrap().metadata = Some(json!({
                            "cell":pack.cell.chars().take(256).collect::<String>(),
                            "cell_truncated":pack.cell.chars().count()>256,
                            "precision":pack.precision.to_string(),"errors":pack.total.to_string(),
                            "checks":pack.checks.len().to_string(),"waives":waives.is_some(),
                        }));
                        run(&inner, &mut pack);
                    }
                    Err(e) => {
                        let mut state = inner.state.lock().unwrap();
                        if !state.closed || e.kind != ErrorKind::Cancelled {
                            state.failure = Some(code(&e));
                        }
                    }
                }
                let mut state = inner.state.lock().unwrap();
                state.closed = true;
                state.active = None;
                state.pending.clear();
            })?;
        *service.thread.lock().unwrap() = Some(handle);
        Ok(service)
    }
    pub fn catalog(&self) -> Value {
        let s = self.inner.state.lock().unwrap();
        json!({"id":self.id,"revision":self.revision,"source_id":self.source_id,"title":self.title,
            "phase":if s.failure.is_some(){"error"}else if s.closed{"closed"}else if s.metadata.is_some(){"ready"}else{"opening"},
            "metadata":s.metadata,"error":s.failure,"read_only":true,"response_bytes":RESPONSE_BYTES})
    }
    pub fn submit(&self, request: Request) -> std::result::Result<Ticket, Failure> {
        self.submit_context(request, None)
    }
    fn submit_context(
        &self,
        request: Request,
        context: Option<dto::FocusContext>,
    ) -> std::result::Result<Ticket, Failure> {
        self.submit_filters(request, context, None)
    }
    fn submit_filters(
        &self,
        request: Request,
        context: Option<dto::FocusContext>,
        selected: Option<std::collections::BTreeSet<u64>>,
    ) -> std::result::Result<Ticket, Failure> {
        let mut request = request.core()?;
        match &mut request {
            dto::Command::Focus {
                context: target, ..
            }
            | dto::Command::InView {
                context: target, ..
            } => *target = context,
            dto::Command::List { filters, .. } | dto::Command::FilteredStep { filters, .. } => {
                filters.context = context;
                filters.selected = selected;
            }
            _ => (),
        }
        self.enqueue(request)
    }
    fn enqueue(&self, request: dto::Command) -> std::result::Result<Ticket, Failure> {
        let mut s = self.inner.state.lock().unwrap();
        if let Some(code) = s.failure {
            return Err(code);
        }
        if s.closed {
            return Err("drc_closed");
        }
        s.pending
            .retain(|w| !w.reply.is_closed() && w.stop.load(Ordering::Relaxed) == 0);
        if s.pending.len() >= QUEUE {
            return Err("drc_busy");
        }
        let (reply, receiver) = oneshot::channel();
        let stop = Arc::new(AtomicUsize::new(0));
        s.pending.push_back(Work {
            request,
            stop: Arc::clone(&stop),
            reply,
        });
        self.inner.wake.notify_one();
        Ok(Ticket {
            reply: receiver,
            stop,
        })
    }
    pub fn request_stop(&self) {
        let mut s = self.inner.state.lock().unwrap();
        s.closed = true;
        if let Some(flag) = &s.active {
            flag.store(1, Ordering::Relaxed);
        }
        s.pending.clear();
        self.inner.wake.notify_all();
    }
    pub fn is_finished(&self) -> bool {
        self.thread
            .lock()
            .unwrap()
            .as_ref()
            .is_none_or(JoinHandle::is_finished)
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        self.request_stop();
        // Never block a runtime on pread/NFS. The actor owns its reservation
        // until it actually exits; serve separately reports a shutdown timeout.
        if let Some(t) = self.thread.get_mut().unwrap().take() {
            if t.is_finished() {
                let _ = t.join();
            }
        }
    }
}
fn run(inner: &Inner, pack: &mut Pack) {
    loop {
        let work = {
            let mut s = inner.state.lock().unwrap();
            s.active = None;
            while s.pending.is_empty() && !s.closed {
                s = inner.wake.wait(s).unwrap();
            }
            if s.closed {
                return;
            }
            let w = s.pending.pop_front().unwrap();
            s.active = Some(Arc::clone(&w.stop));
            w
        };
        if work.stop.load(Ordering::Relaxed) != 0 || work.reply.is_closed() {
            continue;
        }
        let result = read::execute(pack, work.request, &work.stop).map_err(|e| code(&e));
        let _ = work.reply.send(result);
    }
}
fn code(e: &Error) -> Failure {
    match e.kind {
        ErrorKind::Cancelled => "drc_cancelled",
        ErrorKind::Incomplete => "drc_read_limit",
        ErrorKind::Cache => "drc_changed_or_corrupt",
        ErrorKind::Io => "drc_read_error",
        ErrorKind::Busy => "drc_busy",
        _ => "invalid_drc_request",
    }
}

#[cfg(test)]
mod tests;
