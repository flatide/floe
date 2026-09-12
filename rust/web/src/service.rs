//! One bounded owner operation thread; filesystem/prepare/native waits never
//! run on the HTTP reactor. View rendering and index progress are independent
//! of browser subscriptions. No implicit indexing or destructive reopen.
use crate::{
    auth::public_id,
    layer_catalog::LayerCatalog,
    operations::{Admission, Ledger},
    transport::Attachment,
    view::{self, Field, PatchDto},
};
use floe_app_core::{
    index::IndexOptions,
    index_progress::NativePhase,
    jobdeck::color::Mode,
    managed::{ManagedDataset, Resources},
    managed_index::{ManagedIndex, Phase as IndexPhase, Snapshot as IndexSnapshot},
    native::Indexer,
    registered::{RegisteredSource, MAX_SOURCES},
    render::RenderOptions,
    view::{Model, Patch, ViewController, ViewState},
    Error, ErrorKind, Result,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Condvar, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

#[derive(Debug, Deserialize)]
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
            occupancy: false,
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
    Open {
        seq: String,
        source_id: String,
        mode: String,
        #[serde(default)]
        levels: LevelSelection,
        body: Box<PatchDto>,
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
    Open {
        source: Arc<RegisteredSource>,
        source_id: String,
        levels: Option<BTreeSet<i64>>,
        mode: Mode,
        patch: Box<Patch>,
    },
    Index {
        source: Arc<RegisteredSource>,
        levels: Option<BTreeSet<i64>>,
        options: Box<IndexOptions>,
    },
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
}
struct Inner {
    sources: Vec<Entry>,
    resources: Arc<Resources>,
    options: RenderOptions,
    indexer: Indexer,
    state: Mutex<State>,
    wake: Condvar,
}
pub struct Service {
    inner: Arc<Inner>,
    thread: Mutex<Option<JoinHandle<()>>>,
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
        if sources.is_empty() || sources.len() > MAX_SOURCES {
            return Err(Error::input("catalog requires 1..32 sources"));
        }
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
            sources,
            resources,
            options,
            indexer,
            state: Mutex::new(State {
                ledger: Ledger::default(),
                pending: None,
                active_stop: None,
                view: None,
                closed: false,
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
        json!({"sources":self.inner.sources.iter().map(|s|json!({"source_id":s.id,"title":s.source.title,"deck":s.source.deck,"levels":s.source.levels.len()})).collect::<Vec<_>>()})
    }
    pub fn levels(&self, id: &str, start: usize) -> Option<Value> {
        let source = &self.inner.sources.iter().find(|s| s.id == id)?.source;
        if start > source.levels.len() {
            return None;
        }
        let end = start.saturating_add(64).min(source.levels.len());
        Some(
            json!({"source_id":id,"start":start,"total":source.levels.len(),"next":if end<source.levels.len(){Some(end)}else{None},"levels":source.levels[start..end].iter().map(|r|json!({"id":r.id.to_string(),"title":r.title})).collect::<Vec<_>>()}),
        )
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
        if self.is_finished() {
            return Err("closed");
        }
        // Stable typed representation (field order independent), bounded by
        // the HTTP request limit and ledger cap. Never logged or sent back.
        let signature = format!("{request:?}");
        let (seq, id) = match &request {
            OperationDto::Open { seq, source_id, .. }
            | OperationDto::Index { seq, source_id, .. } => (view::counter(seq)?, source_id),
        };
        let source = Arc::clone(
            &self
                .inner
                .sources
                .iter()
                .find(|s| s.id == *id)
                .ok_or("source_unavailable")?
                .source,
        );
        let source_id = id.clone();
        let (kind, command) = match request {
            OperationDto::Open {
                mode, levels, body, ..
            } => {
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
                    Command::Open {
                        source,
                        source_id,
                        levels,
                        mode,
                        patch: Box::new(patch),
                    },
                )
            }
            OperationDto::Index {
                levels, options, ..
            } => {
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
        match s.ledger.admit(seq, signature, kind)? {
            Admission::Replay(state) => return Ok(state),
            Admission::New => (),
        }
        let stop = Arc::new(AtomicUsize::new(0));
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
        self.thread
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
            Command::Open { .. } => "open",
            Command::Index { .. } => "index",
        };
        let result = execute(&inner, work);
        let state = match result {
            Ok(result) => result,
            Err(e) => {
                json!({"seq":seq.to_string(),"kind":kind,"phase":if e.kind==ErrorKind::Cancelled{"cancelled"}else{"failed"},"error":view::safe_error(e.kind)})
            }
        };
        let mut s = inner.state.lock().unwrap();
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
        Command::Open {
            source,
            source_id,
            levels,
            mode,
            patch,
        } => {
            {
                let s = inner.state.lock().unwrap();
                if s.view.as_ref().is_some_and(|v| !v.controller.is_finished()) {
                    return Err(Error::new(
                        ErrorKind::Busy,
                        "close the current view before opening another",
                    ));
                }
            }
            inner.state.lock().unwrap().ledger.update(
                seq,
                json!({"seq":seq.to_string(),"kind":"open","phase":"opening"}),
                false,
            );
            source.validate(&stop)?;
            let selected_levels = levels
                .as_ref()
                .map(|ids| ids.iter().map(i64::to_string).collect());
            let data = ManagedDataset::open(&inner.resources, source.path(), levels, mode, &stop)?;
            let model = Model::new(&data)?;
            let (width, height) = patch.pixels.unwrap_or((1024, 768));
            let initial = ViewState::initial(&model, width, height)?.edit(&model, *patch)?;
            let rows = LayerCatalog::dataset(&data.dataset, &model);
            let controller = Arc::new(ViewController::start(
                &inner.resources,
                data,
                inner.options.clone(),
                initial,
            )?);
            let mut view = Attachment::with_rows(controller, &source.title, rows)
                .map_err(|_| Error::new(ErrorKind::Io, "entropy unavailable"))?;
            view.source_id = source_id;
            view.levels = selected_levels;
            view.mode = match mode {
                Mode::Level => "level",
                Mode::Chip => "chip",
                Mode::Layer => "layer",
            };
            let view = Arc::new(view);
            let mut s = inner.state.lock().unwrap();
            if s.closed || stop.load(Ordering::Relaxed) != 0 {
                view.controller.request_close();
                drop(s);
                drop(view);
                return Err(Error::new(ErrorKind::Cancelled, "open cancelled"));
            }
            let id = view.id.clone();
            s.view = Some(view);
            Ok(json!({"seq":seq.to_string(),"kind":"open","phase":"succeeded","view_id":id}))
        }
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
