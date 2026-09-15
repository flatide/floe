//! One bounded owner operation thread; filesystem/prepare/native waits never
//! run on the HTTP reactor. View rendering and index progress are independent
//! of browser subscriptions. No implicit indexing or destructive reopen.
mod index_open;
mod open;
use index_open::IndexTarget;

use crate::{
    auth::public_id,
    layer_catalog::LayerCatalog,
    operations::{Admission, Ledger},
    transport::Attachment,
    view::{self, Field, PatchDto},
    window_display::{OpenDisplay, WindowDisplay},
};
use floe_app_core::{
    index::IndexOptions,
    index_progress::NativePhase,
    jobdeck::color::Mode,
    managed::{ManagedDataset, Resources},
    managed_index::{ManagedIndex, Phase as IndexPhase, Snapshot as IndexSnapshot},
    native::Indexer,
    registered::{AccessScope, RegisteredSource, SourceSet, MAX_SOURCES},
    render::RenderOptions,
    view::{ControllerOptions, Model, Patch, ViewController, ViewState},
    Error, ErrorKind, Result,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::{BTreeSet, VecDeque},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Condvar, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

#[derive(Debug, Deserialize, serde::Serialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum LevelSelection {
    All {},
    Only { ids: Vec<String> },
}
impl Default for LevelSelection {
    fn default() -> Self {
        Self::All {}
    }
}
impl LevelSelection {
    fn core(self) -> Result<Option<BTreeSet<i64>>> {
        match self {
            Self::All {} => Ok(None),
            Self::Only { ids } => {
                if ids.is_empty() || ids.len() > 4096 {
                    return Err(Error::input("level selection limit"));
                }
                ids.into_iter()
                    .map(|s| {
                        let n = s
                            .parse::<i64>()
                            .map_err(|_| Error::input("invalid level ID"))?;
                        if s != n.to_string() {
                            return Err(Error::input("invalid level ID"));
                        }
                        Ok(n)
                    })
                    .collect::<Result<BTreeSet<_>>>()
                    .map(Some)
            }
        }
    }
}
#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IndexArgs {
    pub jobs: u16,
    pub force: bool,
    pub lod: bool,
    pub occupancy: bool,
    pub occupancy_only: bool,
    pub occupancy_um: Field<String>,
}
impl Default for IndexArgs {
    fn default() -> Self {
        Self {
            jobs: 12,
            force: false,
            lod: false,
            occupancy: true,
            occupancy_only: false,
            occupancy_um: Field::Absent,
        }
    }
}
impl IndexArgs {
    fn core(self) -> Result<IndexOptions> {
        if !(1..=16).contains(&self.jobs) {
            return Err(Error::input("managed index jobs must be 1..16"));
        }
        let occupancy_um = match self.occupancy_um {
            Field::Absent => None,
            Field::Value(s) => Some(
                s.parse::<f64>()
                    .map_err(|_| Error::input("invalid occupancy cell"))?,
            ),
        };
        let o = IndexOptions {
            jobs: usize::from(self.jobs),
            force: self.force,
            lod: self.lod,
            occupancy: self.occupancy,
            occupancy_only: self.occupancy_only,
            occupancy_um,
            ..Default::default()
        };
        o.validate()?;
        Ok(o)
    }
}
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum OperationDto {
    Mode {
        seq: String,
        view_id: String,
        base_state_rev: String,
        mode: String,
    },
    Open {
        seq: String,
        source_id: String,
        mode: String,
        #[serde(default)]
        levels: LevelSelection,
        body: Box<PatchDto>,
        #[serde(default)]
        display_policy: OpenDisplay,
        #[serde(default)]
        label_preference: Field<bool>,
    },
    IndexOpen {
        seq: String,
        request_id: String,
        open_seq: String,
        approved: bool,
        target: IndexTarget,
        pixels: [u32; 2],
        options: IndexArgs,
    },
    Index {
        seq: String,
        source_id: String,
        #[serde(default)]
        levels: LevelSelection,
        options: IndexArgs,
    },
}
enum Command {
    Mode {
        view_id: String,
        base_state_rev: u64,
        mode: Mode,
    },
    Open(Box<OpenCommand>),
    IndexOpen {
        open: Box<OpenCommand>,
        options: Box<IndexOptions>,
        request_id: String,
        open_seq: u64,
    },
    Index {
        source: Arc<RegisteredSource>,
        levels: Option<BTreeSet<i64>>,
        options: Box<IndexOptions>,
    },
}
#[derive(Clone)]
struct OpenCommand {
    source: Arc<RegisteredSource>,
    source_id: String,
    levels: Option<BTreeSet<i64>>,
    mode: Mode,
    patch: Box<Patch>,
    replace: Option<(String, u64)>,
    display_policy: OpenDisplay,
    label_preference: Option<bool>,
}
struct Work {
    seq: u64,
    command: Command,
    stop: Arc<AtomicUsize>,
}
struct Entry {
    id: String,
    source: Arc<RegisteredSource>,
}
struct State {
    ledger: Ledger,
    pending: Option<Work>,
    active_stop: Option<Arc<AtomicUsize>>,
    view: Option<Arc<Attachment>>,
    closed: bool,
    registering: bool,
    window_display: WindowDisplay,
    // Same maximum as the operation ledger. Only failed cache opens are kept.
    retry_opens: VecDeque<(u64, OpenCommand)>,
}
struct Inner {
    exports: Arc<crate::exports::Service>,
    sources: Mutex<Vec<Entry>>,
    source_set: Arc<SourceSet>,
    resources: Arc<Resources>,
    options: RenderOptions,
    indexer: Indexer,
    view_options: ControllerOptions,
    state: Mutex<State>,
    wake: Condvar,
}
pub struct Service {
    inner: Arc<Inner>,
    thread: Mutex<Option<JoinHandle<()>>>,
}
struct Registering<'a>(&'a Inner);
impl Drop for Registering<'_> {
    fn drop(&mut self) {
        self.0.state.lock().unwrap().registering = false;
    }
}
impl Service {
    /// Registration and paths belong to the trusted launcher. Inputs to submit
    /// contain only the random source IDs returned by catalog().
    pub fn start(
        sources: Vec<Arc<RegisteredSource>>,
        resources: Arc<Resources>,
        options: RenderOptions,
        indexer: Indexer,
    ) -> Result<Arc<Self>> {
        Self::start_configured(
            sources,
            resources,
            options,
            indexer,
            ControllerOptions::default(),
        )
    }
    pub fn start_configured(
        sources: Vec<Arc<RegisteredSource>>,
        resources: Arc<Resources>,
        options: RenderOptions,
        indexer: Indexer,
        view_options: ControllerOptions,
    ) -> Result<Arc<Self>> {
        if sources.len() > MAX_SOURCES {
            return Err(Error::input("catalog accepts at most 32 sources"));
        }
        let source_set = SourceSet::new(sources.clone())?;
        let sources = sources
            .into_iter()
            .map(|source| {
                Ok(Entry {
                    id: public_id()
                        .map_err(|_| Error::new(ErrorKind::Io, "entropy unavailable"))?,
                    source,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let inner = Arc::new(Inner {
            exports: crate::exports::Service::start(Arc::clone(&resources), &options)?,
            sources: Mutex::new(sources),
            source_set,
            resources,
            options,
            indexer,
            view_options,
            state: Mutex::new(State {
                ledger: Ledger::default(),
                pending: None,
                active_stop: None,
                view: None,
                closed: false,
                registering: false,
                window_display: WindowDisplay::default(),
                retry_opens: VecDeque::new(),
            }),
            wake: Condvar::new(),
        });
        let task = Arc::clone(&inner);
        let thread = thread::Builder::new()
            .name("floe-owner-service".into())
            .spawn(move || run(task))?;
        Ok(Arc::new(Self {
            inner,
            thread: Mutex::new(Some(thread)),
        }))
    }
    pub fn catalog(&self) -> Value {
        json!({"sources":self.inner.sources.lock().unwrap().iter().map(|s|json!({"source_id":s.id,"title":s.source.title,"deck":s.source.deck,"levels":s.source.levels.len()})).collect::<Vec<_>>()})
    }
    /// Trusted startup only. Seed an empty window's display options before any
    /// owner operation. No HTTP endpoint or dataset work; camera/layer state is
    /// intentionally not a window preference.
    pub fn seed_window_display(&self, patch: Patch) -> Result<()> {
        let display = WindowDisplay::initial(patch)?;
        let mut s = self.inner.state.lock().unwrap();
        if s.closed || s.registering || s.view.is_some() || s.ledger.cursor()["last_seq"] != "0" {
            return Err(Error::new(ErrorKind::Busy, "window has already started"));
        }
        s.window_display = display;
        Ok(())
    }
    /// Trusted local launcher only, off the HTTP reactor. Registration alone
    /// never opens a view, indexes a file, or grants a sidecar write capability.
    pub fn register_source(
        &self,
        scope: Arc<AccessScope>,
        path: &std::path::Path,
        stop: &AtomicUsize,
    ) -> Result<String> {
        self.register_sources(scope, &[path.to_owned()], stop)
            .map(|mut ids| ids.remove(0))
    }
    /// One multi-file CLI invocation registers atomically. A bad later source
    /// cannot leave earlier entries/protected-path membership partially added.
    pub fn register_sources(
        &self,
        scope: Arc<AccessScope>,
        paths: &[std::path::PathBuf],
        stop: &AtomicUsize,
    ) -> Result<Vec<String>> {
        self.register_sources_guarded(scope, paths, stop, || Ok(()))
    }
    /// A picker selection carries an inode witness, not an HTTP path. Validate
    /// before reads and again after metadata preparation, before publication.
    pub(crate) fn register_selected(
        &self,
        selected: &floe_app_core::browse::SelectedFile,
        stop: &AtomicUsize,
    ) -> Result<String> {
        self.register_sources_guarded(
            selected.scope(),
            &[selected.path().to_owned()],
            stop,
            || selected.validate(stop),
        )
        .map(|mut ids| ids.remove(0))
    }
    fn register_sources_guarded(
        &self,
        scope: Arc<AccessScope>,
        paths: &[std::path::PathBuf],
        stop: &AtomicUsize,
        validate: impl Fn() -> Result<()>,
    ) -> Result<Vec<String>> {
        if paths.is_empty() || paths.len() > MAX_SOURCES {
            return Err(Error::input("register 1..32 sources"));
        }
        {
            let mut state = self.inner.state.lock().unwrap();
            if state.closed {
                return Err(Error::new(ErrorKind::Cancelled, "owner service closed"));
            }
            if state.registering || state.ledger.active().is_some() {
                return Err(Error::new(ErrorKind::Busy, "owner operation is active"));
            }
            state.registering = true;
        }
        let _registering = Registering(&self.inner);
        validate()?;
        let mut registration = self.inner.source_set.begin(stop)?;
        let registered = paths
            .iter()
            .map(|path| {
                let source = registration.register(Arc::clone(&scope), path, stop)?;
                let id =
                    public_id().map_err(|_| Error::new(ErrorKind::Io, "entropy unavailable"))?;
                Ok(Entry { id, source })
            })
            .collect::<Result<Vec<_>>>()?;
        validate()?;
        let state = self.inner.state.lock().unwrap();
        if state.closed || self.is_finished() {
            return Err(Error::new(ErrorKind::Cancelled, "owner service closed"));
        }
        if state.ledger.active().is_some() {
            return Err(Error::new(ErrorKind::Busy, "owner operation is active"));
        }
        floe_app_core::check_cancelled(stop)?;
        let mut sources = self.inner.sources.lock().unwrap();
        let mut staged: Vec<Entry> = Vec::new();
        let mut ids = Vec::with_capacity(registered.len());
        for entry in registered {
            if let Some(old) = sources
                .iter()
                .chain(staged.iter())
                .find(|e| Arc::ptr_eq(&e.source, &entry.source))
            {
                ids.push(old.id.clone());
            } else {
                ids.push(entry.id.clone());
                staged.push(entry);
            }
        }
        // No I/O under catalogue/state locks. Protection becomes visible before
        // the opaque handle, and readers cannot observe the intermediate state.
        registration.commit(stop)?;
        sources.extend(staged);
        Ok(ids)
    }
    pub(crate) fn exports(&self) -> &Arc<crate::exports::Service> {
        &self.inner.exports
    }
    pub(crate) fn source(&self, id: &str) -> Option<Arc<RegisteredSource>> {
        self.inner
            .sources
            .lock()
            .unwrap()
            .iter()
            .find(|s| s.id == id)
            .map(|s| Arc::clone(&s.source))
    }
    pub(crate) fn source_set(&self) -> Arc<SourceSet> {
        Arc::clone(&self.inner.source_set)
    }
    /// Cheap admission checks only: never hold this lock across I/O or await.
    pub(crate) fn with_current<T>(
        &self,
        id: &str,
        f: impl FnOnce(&Arc<Attachment>) -> std::result::Result<T, &'static str>,
    ) -> std::result::Result<T, &'static str> {
        let s = self.inner.state.lock().unwrap();
        if s.closed || s.ledger.active().is_some() {
            return Err("view_unavailable");
        }
        let v = s
            .view
            .as_ref()
            .filter(|v| v.id == id)
            .ok_or("view_unavailable")?;
        f(v)
    }
    pub fn levels(&self, id: &str, start: usize) -> Option<Value> {
        let source = self.source(id)?;
        if start > source.levels.len() {
            return None;
        }
        let end = start.saturating_add(64).min(source.levels.len());
        Some(
            json!({"source_id":id,"start":start,"total":source.levels.len(),"next":if end<source.levels.len(){Some(end)}else{None},"levels":source.levels[start..end].iter().map(|r|json!({"id":r.id.to_string(),"title":r.title})).collect::<Vec<_>>()}),
        )
    }
    /// Trusted launcher preflight; no filesystem work or operation admission.
    pub fn validate_source_selection(
        &self,
        id: &str,
        mode: &str,
        levels: Option<&BTreeSet<i64>>,
    ) -> Result<()> {
        let source = self
            .source(id)
            .ok_or_else(|| Error::input("source unavailable"))?;
        let mode = Mode::parse(mode)?;
        if !source.deck && mode != Mode::Level {
            return Err(Error::input("mode requires a jobdeck"));
        }
        source.validate_levels(levels)
    }
    pub(crate) fn current(&self) -> Option<Arc<Attachment>> {
        self.inner.state.lock().unwrap().view.clone()
    }
    pub fn operations(&self) -> Value {
        self.inner.state.lock().unwrap().ledger.snapshot()
    }
    pub fn operation(&self, seq: u64) -> Option<Value> {
        self.inner.state.lock().unwrap().ledger.get(seq)
    }
    pub fn submit(&self, request: OperationDto) -> std::result::Result<Value, &'static str> {
        self.submit_with(request, None)
    }
    pub(crate) fn submit_launch(
        &self,
        request: OperationDto,
        replace: Option<(String, String)>,
    ) -> std::result::Result<Value, &'static str> {
        if !matches!(&request, OperationDto::Open { .. }) {
            return Err("invalid_request");
        }
        let replace = replace
            .map(|(id, rev)| view::counter(&rev).map(|rev| (id, rev)))
            .transpose()?;
        self.submit_with(request, replace)
    }
    fn submit_with(
        &self,
        request: OperationDto,
        replace: Option<(String, u64)>,
    ) -> std::result::Result<Value, &'static str> {
        if self.is_finished() {
            return Err("closed");
        }
        // Stable typed representation (field order independent), bounded by
        // the HTTP request limit and ledger cap. Never logged or sent back.
        let signature = format!("{request:?}/{replace:?}");
        let seq = match &request {
            OperationDto::Open { seq, .. }
            | OperationDto::Mode { seq, .. }
            | OperationDto::Index { seq, .. }
            | OperationDto::IndexOpen { seq, .. } => view::counter(seq)?,
        };
        // A completed mode change has retired its original view ID. Replays
        // must be resolved before consulting that mutable current attachment.
        {
            let s = self.inner.state.lock().unwrap();
            if s.closed {
                return Err("closed");
            }
            if let Some(replay) = s.ledger.replay(seq, &signature)? {
                return Ok(replay);
            }
        }
        let source = |id: &str| self.source(id).ok_or("source_unavailable");
        let (kind, command) = match request {
            OperationDto::Mode {
                view_id,
                base_state_rev,
                mode,
                ..
            } => (
                "mode",
                Command::Mode {
                    view_id,
                    base_state_rev: view::counter(&base_state_rev)?,
                    mode: Mode::parse(&mode).map_err(|_| "invalid_request")?,
                },
            ),
            OperationDto::Open {
                source_id,
                mode,
                levels,
                body,
                display_policy,
                label_preference,
                ..
            } => {
                let source = source(&source_id)?;
                let mode = Mode::parse(&mode).map_err(|_| "invalid_request")?;
                let levels = levels.core().map_err(|_| "invalid_request")?;
                source
                    .validate_levels(levels.as_ref())
                    .map_err(|_| "invalid_request")?;
                if !source.deck && mode != Mode::Level {
                    return Err("invalid_request");
                }
                let patch = body.core().map_err(|_| "invalid_request")?;
                (
                    "open",
                    Command::Open(Box::new(OpenCommand {
                        source,
                        source_id,
                        levels,
                        mode,
                        patch: Box::new(patch),
                        replace,
                        display_policy,
                        label_preference: match label_preference {
                            Field::Absent => None,
                            Field::Value(v) => Some(v),
                        },
                    })),
                )
            }
            OperationDto::IndexOpen {
                open_seq,
                request_id,
                approved,
                target,
                pixels,
                options,
                ..
            } => {
                if !approved {
                    return Err("approval_required");
                }
                index_open::identity(&request_id)?;
                let original = view::counter(&open_seq)?;
                let options = options.core().map_err(|_| "invalid_request")?;
                floe_app_core::view::Viewport::new([0.0, 0.0, 1.0, 1.0], pixels[0], pixels[1])
                    .map_err(|_| "invalid_request")?;
                let mut open = {
                    let s = self.inner.state.lock().unwrap();
                    let old = s.ledger.get(original).ok_or("operation_expired")?;
                    if old["kind"] != "open"
                        || old["phase"] != "failed"
                        || old["error"] != "index_unavailable"
                    {
                        return Err("invalid_request");
                    }
                    s.retry_opens
                        .iter()
                        .find(|(n, _)| *n == original)
                        .map(|(_, o)| o.clone())
                        .ok_or("operation_expired")?
                };
                open.replace = target.core()?;
                open.patch.pixels = Some((pixels[0], pixels[1]));
                (
                    "index_open",
                    Command::IndexOpen {
                        open: Box::new(open),
                        options: Box::new(options),
                        request_id,
                        open_seq: original,
                    },
                )
            }
            OperationDto::Index {
                source_id,
                levels,
                options,
                ..
            } => {
                let source = source(&source_id)?;
                let levels = levels.core().map_err(|_| "invalid_request")?;
                source
                    .validate_levels(levels.as_ref())
                    .map_err(|_| "invalid_request")?;
                let options = options.core().map_err(|_| "invalid_request")?;
                (
                    "index",
                    Command::Index {
                        source,
                        levels,
                        options: Box::new(options),
                    },
                )
            }
        };
        let mut s = self.inner.state.lock().unwrap();
        if s.closed {
            return Err("closed");
        }
        if s.registering {
            return Err("busy");
        }
        match s.ledger.admit(seq, signature, kind)? {
            Admission::Replay(state) => return Ok(state),
            Admission::New => (),
        }
        let stop = Arc::new(AtomicUsize::new(0));
        if let Command::IndexOpen {
            request_id,
            open_seq,
            ..
        } = &command
        {
            s.ledger.update(seq,json!({"seq":seq.to_string(),"kind":"index_open","phase":"queued","stage":"index","request_id":request_id,"open_seq":open_seq.to_string()}),false);
        }
        s.active_stop = Some(Arc::clone(&stop));
        s.pending = Some(Work { seq, command, stop });
        let state = s.ledger.get(seq).expect("admitted operation");
        self.inner.wake.notify_one();
        Ok(state)
    }
    /// Idempotent cancellation of the named active operation, not whichever
    /// job happens to be current by the time an old request arrives.
    pub fn cancel(&self, seq: u64) -> std::result::Result<Value, &'static str> {
        let s = self.inner.state.lock().unwrap();
        let state = s.ledger.get(seq).ok_or("operation_expired")?;
        if s.ledger.active() == Some(seq) {
            if let Some(flag) = &s.active_stop {
                flag.store(1, Ordering::Relaxed);
            }
        }
        self.inner.wake.notify_one();
        Ok(state)
    }
    /// A single bounded in-memory preview, never cache probing or indexing.
    pub fn index_open_preview(&self, seq: u64) -> std::result::Result<Value, &'static str> {
        let s = self.inner.state.lock().unwrap();
        if s.closed {
            return Err("closed");
        }
        s.ledger.get(seq).ok_or("operation_expired")?;
        let open = s
            .retry_opens
            .iter()
            .find(|(n, _)| *n == seq)
            .map(|(_, o)| o)
            .ok_or("invalid_request")?;
        let mut out = index_open::proposal(seq, open);
        out["levels"] = match &open.levels {
            None => json!({"mode":"all"}),
            Some(ids) => {
                json!({"mode":"only","ids":ids.iter().map(i64::to_string).collect::<Vec<_>>()})
            }
        };
        out["jobs_available"] = json!(self.inner.resources.index_slots());
        Ok(out)
    }
    pub fn close_view(&self, id: &str) -> std::result::Result<(), &'static str> {
        let s = self.inner.state.lock().unwrap();
        let view = s
            .view
            .as_ref()
            .filter(|v| v.id == id)
            .ok_or("view_unavailable")?;
        view.controller.request_close();
        Ok(())
    }
    pub fn request_stop(&self) {
        self.inner.exports.request_stop();
        let mut s = self.inner.state.lock().unwrap();
        s.closed = true;
        if let Some(flag) = &s.active_stop {
            flag.store(1, Ordering::Relaxed);
        }
        if let Some(view) = &s.view {
            view.controller.request_close();
        }
        self.inner.wake.notify_one();
    }
    pub fn is_finished(&self) -> bool {
        self.inner.exports.is_finished()
            && self
                .thread
                .lock()
                .unwrap()
                .as_ref()
                .is_none_or(|t| t.is_finished())
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
            while s.pending.is_none() && !s.closed {
                s = inner.wake.wait(s).unwrap();
            }
            if s.pending.is_none() && s.closed {
                break;
            }
            s.pending.take().expect("pending work")
        };
        let seq = work.seq;
        let kind = match &work.command {
            Command::Mode { .. } => "mode",
            Command::Open(_) => "open",
            Command::IndexOpen { .. } => "index_open",
            Command::Index { .. } => "index",
        };
        let retry = match &work.command {
            Command::Open(open) => Some((**open).clone()),
            _ => None,
        };
        let request_id = match &work.command {
            Command::IndexOpen {
                request_id,
                open_seq,
                ..
            } => Some((request_id.clone(), *open_seq)),
            _ => None,
        };
        let stop = Arc::clone(&work.stop);
        let result = execute(&inner, work);
        let retry = if matches!(&result,Err(e) if matches!(e.kind, ErrorKind::Cache | ErrorKind::Io | ErrorKind::InvalidInput))
        {
            retry.filter(|open| index_open::retryable(&inner, open, &stop).unwrap_or(false))
        } else {
            None
        };
        let mut state = match result {
            Ok(result) => result,
            Err(e) => {
                json!({"seq":seq.to_string(),"kind":kind,"phase":if e.kind==ErrorKind::Cancelled{"cancelled"}else{"failed"},"error":view::safe_error(e.kind)})
            }
        };
        let mut s = inner.state.lock().unwrap();
        if let Some((request_id, open_seq)) = request_id {
            state["request_id"] = json!(request_id);
            state["open_seq"] = json!(open_seq.to_string());
            if state.get("stage").is_none() {
                state["stage"] = json!("index");
            }
        }
        if let Some(open) = retry {
            state["error"] = json!("index_unavailable");
            state["index_open"] = index_open::proposal(seq, &open);
            if s.retry_opens.len() == 32 {
                s.retry_opens.pop_front();
            }
            s.retry_opens.push_back((seq, open));
        }
        s.ledger.update(seq, state, true);
        s.active_stop = None;
    }
    let view = { inner.state.lock().unwrap().view.clone() };
    if let Some(view) = view {
        view.controller.request_close();
        while !view.controller.is_finished() {
            thread::sleep(Duration::from_millis(10));
        }
    }
}
fn execute(inner: &Inner, work: Work) -> Result<Value> {
    let Work { seq, command, stop } = work;
    floe_app_core::check_cancelled(&stop)?;
    match command {
        Command::Mode {
            view_id,
            base_state_rev,
            mode,
        } => {
            let previous = inner
                .state
                .lock()
                .unwrap()
                .view
                .as_ref()
                .filter(|v| v.id == view_id)
                .cloned()
                .ok_or_else(|| Error::new(ErrorKind::Busy, "view changed"))?;
            let snapshot = previous.controller.snapshot();
            if !matches!(
                snapshot.phase,
                floe_app_core::view::Phase::Idle | floe_app_core::view::Phase::Rendering
            ) {
                return Err(Error::new(
                    ErrorKind::Busy,
                    "view is not ready for a mode change",
                ));
            }
            if snapshot.state_rev != base_state_rev {
                return Err(Error::new(ErrorKind::Busy, "view revision changed"));
            }
            if !previous.controller.model.deck {
                return Err(Error::new(
                    ErrorKind::Unsupported,
                    "mode changes require a jobdeck",
                ));
            }
            let mode_name = match mode {
                Mode::Level => "level",
                Mode::Chip => "chip",
                Mode::Layer => "layer",
            };
            if previous.mode == mode_name {
                return Ok(
                    json!({"seq":seq.to_string(),"kind":"mode","phase":"succeeded","view_id":view_id,"unchanged":true}),
                );
            }
            inner.state.lock().unwrap().ledger.update(
                seq,
                json!({"seq":seq.to_string(),"kind":"mode","phase":"preparing"}),
                false,
            );
            let source = inner
                .sources
                .lock()
                .unwrap()
                .iter()
                .find(|s| s.id == previous.source_id)
                .map(|s| Arc::clone(&s.source))
                .ok_or_else(|| Error::new(ErrorKind::Busy, "source registration changed"))?;
            source.validate(&stop)?;
            let current = previous.controller.pin_dataset()?;
            let floe_app_core::dataset::Dataset::Deck(deck) = &current.dataset else {
                unreachable!("deck model")
            };
            let data = ManagedDataset::open(
                &inner.resources,
                source.path(),
                deck.metadata.jobdeck.levels.clone(),
                mode,
                &stop,
            )?;
            let prepared = previous.mode_memory.prepare(
                &current,
                &previous.controller.model,
                &snapshot.state,
                &data,
            )?;
            let rows = LayerCatalog::dataset(&data.dataset, &prepared.model);
            let mut replacement = previous
                .controller
                .prepare_replacement(data, prepared.state)?;
            // Entropy, metadata and dormant thread creation all precede the
            // cutover. Failures/cancellation here leave the original alive.
            let mut view = Attachment::with_rows(replacement.controller(), &source.title, rows)
                .map_err(|_| Error::new(ErrorKind::Io, "entropy unavailable"))?;
            view.source_id = previous.source_id.clone();
            view.levels = previous.levels.clone();
            view.mode = mode_name;
            view.mode_memory = prepared.memory;
            let view = Arc::new(view);
            let mut s = inner.state.lock().unwrap();
            if s.closed || stop.load(Ordering::Relaxed) != 0 {
                return Err(Error::new(
                    ErrorKind::Cancelled,
                    "mode change cancelled before cutover",
                ));
            }
            if !s.view.as_ref().is_some_and(|v| Arc::ptr_eq(v, &previous)) {
                return Err(Error::new(ErrorKind::Busy, "view changed before cutover"));
            }
            replacement.commit(base_state_rev)?;
            let id = view.id.clone();
            s.view = Some(view);
            // Cutover is committed. A later operation cancel cannot undo it;
            // session stop still closes the newly installed controller.
            Ok(
                json!({"seq":seq.to_string(),"kind":"mode","phase":"succeeded","view_id":id,"unchanged":false}),
            )
        }
        Command::Open(command) => open::execute(inner, seq, *command, stop, None),
        Command::IndexOpen {
            open,
            options,
            request_id,
            open_seq,
        } => index_open::execute(inner, seq, *open, *options, stop, &request_id, open_seq),
        Command::Index {
            source,
            levels,
            options,
        } => {
            let mut job = ManagedIndex::start(
                &inner.resources,
                source,
                levels,
                *options,
                inner.indexer.clone(),
            )?;
            loop {
                if stop.load(Ordering::Relaxed) != 0 {
                    job.cancel();
                }
                let state = index_state(seq, &job.snapshot());
                if job.is_finished() {
                    job.close()?;
                    return Ok(index_state(seq, &job.snapshot()));
                }
                inner.state.lock().unwrap().ledger.update(seq, state, false);
                thread::sleep(Duration::from_millis(20));
            }
        }
    }
}
fn index_state(seq: u64, s: &IndexSnapshot) -> Value {
    let phase = match s.phase {
        IndexPhase::Preparing => "preparing",
        IndexPhase::Running => "running",
        IndexPhase::Cancelling => "cancelling",
        IndexPhase::Succeeded => "succeeded",
        IndexPhase::Incomplete => "incomplete",
        IndexPhase::Failed => "failed",
        IndexPhase::Cancelled => "cancelled",
    };
    let native = match s.native.phase {
        NativePhase::Starting => "starting",
        NativePhase::Reading => "reading",
        NativePhase::Parsing => "parsing",
        NativePhase::Preparing => "preparing",
        NativePhase::Building => "building",
        NativePhase::Publishing => "publishing",
        NativePhase::Occupancy => "occupancy",
    };
    json!({"seq":seq.to_string(),"kind":"index","phase":phase,"title":s.title,"current":s.current,"completed":s.completed,"total":s.total,"kept":s.kept,"skipped":s.skipped,"failed":s.failed,"elapsed_ms":s.elapsed_ms.to_string(),"error":s.failure.map(view::safe_error),
        "native":{"phase":native,"output_bytes":s.native.output_bytes.to_string(),"dropped_lines":s.native.dropped_lines.to_string(),"cells":s.native.cells.map(|n|n.to_string()),"total_cells":s.native.total_cells.map(|n|n.to_string()),"planned_pages":s.native.planned_pages.map(|n|n.to_string()),"encoded_pages":s.native.encoded_pages.map(|n|n.to_string())}})
}

#[cfg(test)]
mod index_args_tests {
    use super::*;
    #[test]
    fn summary_defaults_on_but_explicit_optout_and_summary_only_survive() {
        let parse = |v| {
            serde_json::from_value::<IndexArgs>(v)
                .unwrap()
                .core()
                .unwrap()
        };
        assert!(parse(json!({})).occupancy);
        assert!(!parse(json!({"occupancy":false})).occupancy);
        assert!(parse(json!({"occupancy_only":true})).occupancy_only);
        assert_eq!(
            parse(json!({"occupancy":false,"occupancy_um":"2"})).occupancy_um,
            Some(2.0)
        );
        assert!(serde_json::from_value::<IndexArgs>(json!({"occupancy":null})).is_err());
    }
}
