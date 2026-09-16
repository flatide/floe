//! Owner-only approved-root file catalogue. One blocking actor owns all filesystem
//! work. HTTP admits bounded, replayable operations and returns immediately.
mod http;
#[cfg(test)]
mod tests;
use crate::{
    launch::Launches,
    operations::{Admission, Ledger},
    service::Service,
    view,
};
use floe_app_core::{
    browse::{Browser, Crumb, Filter},
    managed::Resources,
    Error, ErrorKind, Result,
};
pub(crate) use http::routes;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Condvar, Mutex,
    },
    thread::{self, JoinHandle},
};

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    List {
        seq: String,
        directory: String,
        filter: Filter,
        query: String,
    },
    Page {
        seq: String,
        snapshot: String,
        start: usize,
    },
    Select {
        seq: String,
        handle: String,
    },
    OpenDrc {
        seq: String,
        handle: String,
        context: crate::drc::registry::OpenContext,
    },
    ReconnectDrcReview {
        seq: String,
        context: crate::drc::registry::OpenContext,
        approve: bool,
    },
    LoadDrcRules {
        seq: String,
        handle: String,
        context: crate::drc::registry::OpenContext,
    },
}
impl Request {
    fn identity(&self) -> (&str, &'static str) {
        match self {
            Self::List { seq, .. } => (seq, "list"),
            Self::Page { seq, .. } => (seq, "page"),
            Self::Select { seq, .. } => (seq, "select"),
            Self::OpenDrc { seq, .. } => (seq, "open_drc"),
            Self::ReconnectDrcReview { seq, .. } => (seq, "reconnect_drc_review"),
            Self::LoadDrcRules { seq, .. } => (seq, "load_drc_rules"),
        }
    }
    fn validate(&self) -> std::result::Result<(), &'static str> {
        let token = match self {
            Self::ReconnectDrcReview {
                context, approve, ..
            } => {
                if !approve || context.drc_id.is_none() {
                    return Err("invalid_request");
                }
                return context.validate();
            }
            Self::List {
                directory, query, ..
            } => {
                if query.len() > 128
                    || query.contains(['/', '\0'])
                    || query.chars().any(char::is_control)
                {
                    return Err("invalid_request");
                }
                directory
            }
            Self::Page {
                snapshot, start, ..
            } => {
                if !start.is_multiple_of(floe_app_core::browse::PAGE_ROWS) || *start >= 100_000 {
                    return Err("invalid_request");
                }
                snapshot
            }
            Self::Select { handle, .. } => handle,
            Self::LoadDrcRules {
                handle, context, ..
            } => {
                context.validate()?;
                if context.drc_id.is_none() {
                    return Err("invalid_request");
                }
                handle
            }
            Self::OpenDrc {
                handle, context, ..
            } => {
                context.validate()?;
                handle
            }
        };
        if token.len() != 64
            || !token
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("invalid_request");
        }
        Ok(())
    }
}
struct Work {
    seq: u64,
    request: Request,
    stop: Arc<AtomicUsize>,
}
#[derive(Default)]
struct State {
    ledger: Ledger,
    pending: Option<Work>,
    active_stop: Option<Arc<AtomicUsize>>,
    closed: bool,
}
struct Inner {
    state: Mutex<State>,
    wake: Condvar,
}
pub struct Picker {
    roots: Vec<Crumb>,
    inner: Arc<Inner>,
    thread: Mutex<Option<JoinHandle<()>>>,
}
impl Picker {
    /// Paths are trusted launcher configuration only, never HTTP input. Roots
    /// cannot be widened by a later browser request or a forwarded CLI call.
    pub fn start(
        paths: &[PathBuf],
        resources: &Arc<Resources>,
        service: Arc<Service>,
        launch: Arc<Launches>,
    ) -> Result<Arc<Self>> {
        Self::start_with_drc(paths, resources, service, launch, None)
    }
    pub(crate) fn start_with_drc(
        paths: &[PathBuf],
        resources: &Arc<Resources>,
        service: Arc<Service>,
        launch: Arc<Launches>,
        drc: Option<Arc<crate::drc::Registry>>,
    ) -> Result<Arc<Self>> {
        let permit = resources.browse()?;
        let browser = Browser::new(paths)?;
        let roots = browser.roots();
        let inner = Arc::new(Inner {
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
        });
        let task = Arc::clone(&inner);
        let thread = thread::Builder::new()
            .name("floe-file-catalogue".into())
            .spawn(move || {
                let _permit = permit;
                run(task, browser, service, launch, drc);
            })?;
        Ok(Arc::new(Self {
            roots,
            inner,
            thread: Mutex::new(Some(thread)),
        }))
    }
    fn snapshot(&self) -> Value {
        let mut v = self.inner.state.lock().unwrap().ledger.cursor();
        v["roots"] = json!(self.roots);
        v
    }
    fn operation(&self, seq: u64) -> Option<Value> {
        self.inner.state.lock().unwrap().ledger.get(seq)
    }
    fn submit(&self, request: Request) -> std::result::Result<Value, &'static str> {
        let (seq, kind) = request.identity();
        let seq = view::counter(seq)?;
        let signature = serde_json::to_string(&request).map_err(|_| "invalid_request")?;
        let mut s = self.inner.state.lock().unwrap();
        // Immutable receipt first: reconnect/retry must not rescan or register
        // again, including after a handle expired or a file was replaced.
        if let Some(value) = s.ledger.replay(seq, &signature)? {
            return Ok(value);
        }
        if s.closed || self.is_finished() {
            return Err("closed");
        }
        request.validate()?;
        match s.ledger.admit(seq, signature, kind)? {
            Admission::Replay(v) => Ok(v),
            Admission::New => {
                s.ledger.update(
                    seq,
                    json!({"seq":seq.to_string(),"kind":kind,"request":request,"phase":"queued"}),
                    false,
                );
                let stop = Arc::new(AtomicUsize::new(0));
                s.active_stop = Some(Arc::clone(&stop));
                s.pending = Some(Work { seq, request, stop });
                self.inner.wake.notify_one();
                Ok(s.ledger.get(seq).expect("admitted browse operation"))
            }
        }
    }
    fn cancel(&self, seq: u64) -> std::result::Result<Value, &'static str> {
        let mut s = self.inner.state.lock().unwrap();
        if s.ledger.active() == Some(seq) {
            s.active_stop
                .as_ref()
                .expect("active browse flag")
                .store(1, Ordering::Relaxed);
            let mut value = s.ledger.get(seq).unwrap();
            value["phase"] = json!("cancelling");
            s.ledger.update(seq, value, false);
            self.inner.wake.notify_one();
        }
        s.ledger.get(seq).ok_or("operation_expired")
    }
    pub fn request_stop(&self) {
        let mut s = self.inner.state.lock().unwrap();
        s.closed = true;
        if let Some(stop) = &s.active_stop {
            stop.store(1, Ordering::Relaxed);
        }
        self.inner.wake.notify_one();
    }
    pub fn is_finished(&self) -> bool {
        self.thread
            .lock()
            .unwrap()
            .as_ref()
            .is_none_or(JoinHandle::is_finished)
    }
    pub fn has_failed(&self) -> bool {
        !self.inner.state.lock().unwrap().closed && self.is_finished()
    }
}
impl Drop for Picker {
    fn drop(&mut self) {
        self.request_stop();
        if let Some(thread) = self.thread.get_mut().unwrap().take() {
            if thread.is_finished() {
                let _ = thread.join();
            }
        }
    }
}
enum Output {
    Page(Value),
    Drc(Box<crate::drc::registry::PreparedOpen>),
    Rules(Box<crate::drc::registry::PreparedRules>),
    Selected {
        source_id: String,
        deck: bool,
        levels: usize,
    },
}
fn execute(
    browser: &mut Browser,
    service: &Arc<Service>,
    drc: Option<&Arc<crate::drc::Registry>>,
    work: &Work,
) -> Result<Output> {
    let stop = &work.stop;
    match &work.request {
        Request::List {
            directory,
            filter,
            query,
            ..
        } => browser
            .list(directory, *filter, query, stop)
            .map(|p| Output::Page(json!(p))),
        Request::Page {
            snapshot, start, ..
        } => browser
            .page(snapshot, *start, stop)
            .map(|p| Output::Page(json!(p))),
        Request::Select { handle, .. } => {
            let chosen = browser.select(handle, stop)?;
            let source_id = service.register_selected(&chosen, stop)?;
            let source = service
                .source(&source_id)
                .ok_or_else(|| Error::input("source unavailable"))?;
            Ok(Output::Selected {
                source_id,
                deck: source.deck,
                levels: source.levels.len(),
            })
        }
        Request::OpenDrc {
            handle, context, ..
        } => {
            let drc = drc.ok_or_else(|| Error::input("DRC selection unavailable"))?;
            let selected = browser.select(handle, stop)?;
            drc.prepare_open(Arc::clone(service), selected, context.clone(), stop)
                .map(|p| Output::Drc(Box::new(p)))
        }
        Request::ReconnectDrcReview { context, .. } => drc
            .ok_or_else(|| Error::input("review reconnect unavailable"))?
            .prepare_reconnect(Arc::clone(service), context.clone(), stop)
            .map(|p| Output::Drc(Box::new(p))),
        Request::LoadDrcRules {
            handle, context, ..
        } => {
            let selected = browser.select(handle, stop)?;
            drc.ok_or_else(|| Error::input("DRC metadata unavailable"))?
                .prepare_rules(service.clone(), selected, context.clone(), stop)
                .map(|p| Output::Rules(Box::new(p)))
        }
    }
}
fn run(
    inner: Arc<Inner>,
    mut browser: Browser,
    service: Arc<Service>,
    launch: Arc<Launches>,
    drc: Option<Arc<crate::drc::Registry>>,
) {
    loop {
        let work = {
            let mut s = inner.state.lock().unwrap();
            while !s.closed && s.pending.is_none() {
                s = inner.wake.wait(s).unwrap();
            }
            let Some(work) = s.pending.take() else {
                return;
            };
            if !s.closed && work.stop.load(Ordering::Relaxed) == 0 {
                s.ledger.update(work.seq,json!({"seq":work.seq.to_string(),"kind":work.request.identity().1,"request":work.request,"phase":"running"}),false);
            }
            work
        };
        let result = execute(&mut browser, &service, drc.as_ref(), &work);
        // Cancellation and proposal publication share this lock. A cancelled
        // read may have completed metadata registration, but cannot auto-open.
        // Once ready is published, cancel returns success + the proposal ID;
        // use that broker's Dismiss action, not a new selection request.
        let mut s = inner.state.lock().unwrap();
        let result = if s.closed || work.stop.load(Ordering::Relaxed) != 0 {
            Err(Error::new(ErrorKind::Cancelled, "picker cancelled"))
        } else {
            result
        };
        let result=result.and_then(|output|match output {
            Output::Page(p)=>Ok(json!({"page":p})),
            Output::Drc(p)=>p.commit(&work.stop),
            Output::Rules(p)=>p.commit(&work.stop),
            Output::Selected{source_id,deck,levels}=>{
                let (id,_)=launch.reserve()?;
                let request=json!({"kind":"open","seq":"1","source_id":source_id,"mode":"level","levels":{"mode":"all"},
                    "display_policy":"window","body":{}});
                if let Err(e)=launch.ready(&id,Some(request),deck&&levels>1) {launch.failed(&id,e.kind);return Err(e);}
                Ok(json!({"source_id":source_id,"launch_id":id}))
            }
        });
        let mut value = json!({"seq":work.seq.to_string(),"kind":work.request.identity().1,"request":work.request});
        match result {
            Ok(data) => {
                value["phase"] = json!("succeeded");
                value["result"] = data;
            }
            Err(e) => {
                value["phase"] = json!(if e.kind == ErrorKind::Cancelled {
                    "cancelled"
                } else {
                    "failed"
                });
                value["error"] = json!(failure(e.kind));
            }
        }
        s.ledger.update(work.seq, value, true);
        s.active_stop = None;
    }
}
fn failure(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::Cache => "browse_changed",
        ErrorKind::Busy => "browse_busy_or_limit",
        ErrorKind::Cancelled => "browse_cancelled",
        ErrorKind::InvalidInput | ErrorKind::Unsupported => "browse_invalid_selection",
        _ => "browse_read_error",
    }
}
