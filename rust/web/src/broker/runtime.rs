//! Native S1b adapter, not an owner Service. One runtime
//! per broker, one controller/child per admitted session, one shared Resources.
//! Missing immutable revisions fail closed; opening NEVER starts an indexer.
use super::{Access, Broker, Error, Result, WorkerLease};
use floe_app_core::{
    jobdeck::color::Mode,
    managed::{ManagedDataset, Resources},
    render::RenderOptions,
    view::{
        CellRequest, CellWait, ControllerOptions, DisplayFrame, Model, Patch, ReservedView,
        Snapshot, ViewController, ViewState, Viewport,
    },
    ErrorKind,
};
use std::{
    collections::BTreeMap,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{atomic::Ordering, Arc, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

struct Control {
    view: Mutex<Option<ViewController>>,
    last_used: Mutex<Instant>,
}
impl Default for Control {
    fn default() -> Self {
        Self {
            view: Mutex::new(None),
            last_used: Mutex::new(Instant::now()),
        }
    }
}
impl Control {
    fn idle(&self, now: Instant) -> bool {
        now.duration_since(*self.last_used.lock().unwrap_or_else(|e| e.into_inner()))
            >= Duration::from_secs(120)
    }
}
struct Runner {
    control: Arc<Control>,
    thread: JoinHandle<Result<()>>,
}
#[derive(Default)]
struct State {
    stopping: bool,
    runners: BTreeMap<String, Runner>,
}

pub struct Runtime {
    pub(super) broker: Arc<Broker>,
    resources: Arc<Resources>,
    options: RenderOptions,
    state: Mutex<State>,
}
impl Runtime {
    /// Trusted host configuration only. The host must pass its SAME Resources
    /// to index/export services later; separate managers are not a global cap.
    /// No per-session binary, root, thread count or memory budget is accepted.
    pub fn new(
        broker: Arc<Broker>,
        resources: Arc<Resources>,
        mut options: RenderOptions,
    ) -> Result<Self> {
        if !options.binary.is_absolute()
            || !(1..=16).contains(&options.decode_jobs)
            || !(1..=16).contains(&options.raster_jobs)
            || options.budget_mb == 0
        {
            return Err(Error::Invalid);
        }
        options.debug = false;
        {
            let mut state = broker.lock()?;
            if state.stopping || state.runtime_attached {
                return Err(Error::Busy);
            }
            state.runtime_attached = true;
        }
        Ok(Self {
            broker,
            resources,
            options,
            state: Mutex::new(State::default()),
        })
    }

    /// Only an already authenticated immutable Access can start a view. CPU,
    /// worker and decoded-memory reservations precede metadata/source reads.
    /// Repeated opens are rejected, not queued into unbounded background work.
    pub fn open(&self, access: &Access, width: u32, height: u32) -> Result<()> {
        self.check_pixels(width, height)?;
        Viewport::new([0., 0., 1., 1.], width, height).map_err(core_error)?;
        self.maintain()?;
        self.broker.with_access(access, || {
            let mut state = self.state.lock().map_err(|_| Error::Unavailable)?;
            if state.stopping {
                return Err(Error::Unavailable);
            }
            if state.runners.contains_key(access.id()) {
                return Err(Error::Busy);
            }
            let reserved = ViewController::reserve(
                &self.resources,
                self.options.clone(),
                ControllerOptions::default(),
            )
            .map_err(core_error)?;
            let control = Arc::new(Control::default());
            let runner_control = Arc::clone(&control);
            let access = access.clone();
            let id = access.id().to_owned();
            let broker = Arc::clone(&self.broker);
            let resources = Arc::clone(&self.resources);
            let thread = thread::Builder::new()
                .name("floe-server-view".into())
                .spawn(move || {
                    // No state mutex is held across scope checks or child cleanup.
                    let lease = match broker.claim_worker(&access, Instant::now()) {
                        Ok(lease) => lease,
                        Err(_) => return broker.abandon_unclaimed(&access),
                    };
                    let result = catch_unwind(AssertUnwindSafe(|| {
                        run(
                            &broker,
                            &resources,
                            &lease,
                            reserved,
                            &runner_control,
                            width,
                            height,
                        )
                    }));
                    // Always remove the published controller before joining it.
                    // On a failed/panicked join keep the broker claim fail-closed.
                    let view = runner_control
                        .view
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .take();
                    if let Some(mut view) = view {
                        view.close().map_err(core_error)?;
                        drop(view);
                    }
                    if result.is_err() {
                        return Err(Error::Unavailable);
                    }
                    // run's preparation permit or the controller permit and all
                    // cache pins have now dropped; even open failures are reaped.
                    broker.worker_reaped(&lease)
                })
                .map_err(|_| Error::Unavailable)?;
            state.runners.insert(id, Runner { control, thread });
            Ok(())
        })
    }

    fn with_view<T>(
        &self,
        access: &Access,
        f: impl FnOnce(Option<&ViewController>) -> Result<T>,
    ) -> Result<T> {
        self.broker.with_access(access, || {
            let state = self.state.lock().map_err(|_| Error::Unavailable)?;
            let runner = state.runners.get(access.id()).ok_or(Error::Invalid)?;
            *runner
                .control
                .last_used
                .lock()
                .map_err(|_| Error::Unavailable)? = Instant::now();
            let view = runner.control.view.lock().map_err(|_| Error::Unavailable)?;
            f(view.as_ref())
        })
    }
    /// None means source/revision preparation, not a blank final frame. These
    /// Rust values are not wire DTOs; the HTTP adapter must apply its own schema.
    pub fn snapshot(&self, access: &Access) -> Result<Option<Snapshot>> {
        self.with_view(access, |view| {
            Ok(view.map(|view| {
                let mut snapshot = view.snapshot();
                for (_, text) in [&mut snapshot.failure, &mut snapshot.margin_failure]
                    .into_iter()
                    .flatten()
                {
                    *text = "renderer failed".into();
                }
                snapshot
            }))
        })
    }
    /// Read-only panel data for the session's own view: the same bounded
    /// palette rows, minimap base and fill-slot table the owner shell reads.
    /// Rows are rebuilt from the immutable model per request; no owner
    /// catalog, source path or write path is reached.
    pub(crate) fn palette(
        &self,
        access: &Access,
        request: crate::layer_catalog::PaletteRead,
    ) -> Result<serde_json::Value> {
        self.with_view(access, |view| {
            let view = view.ok_or(Error::Invalid)?;
            crate::layer_catalog::LayerCatalog::model(&view.model)
                .read(&view.snapshot(), request)
                .map_err(|_| Error::Invalid)
        })
    }
    pub fn minimap(&self, access: &Access, base: &str) -> Result<serde_json::Value> {
        self.with_view(access, |view| {
            let view = view.ok_or(Error::Invalid)?;
            let model = &view.model;
            let pixels = model.minimap.base(base).ok_or(Error::Invalid)?;
            Ok(serde_json::json!({"view_id":access.id(),"dataset_revision":model.dataset_revision.to_string(),
                "base":base,"size":180,"pixels":pixels}))
        })
    }
    pub fn fill_slots(&self, access: &Access, key: &str) -> Result<serde_json::Value> {
        self.with_view(access, |view| {
            let view = view.ok_or(Error::Invalid)?;
            let state = view.snapshot().state;
            if state.fill_slots_key() != key {
                return Err(Error::Invalid);
            }
            Ok(
                serde_json::json!({"version":1,"view_id":access.id(),"fill_slots_key":key,
                "editable":false,"fills":state.fill_slots()}),
            )
        })
    }
    /// One cell-tree question for the session's own view. Only the ticket is
    /// taken under the registry lock; the caller waits for the answer after
    /// release (off the reactor), so a slow tree never stalls other sessions.
    /// Extents and instance walks count under the displayed root, as its frame.
    pub(crate) fn cell_ticket(
        &self,
        access: &Access,
        question: crate::cells::Question,
    ) -> Result<CellWait> {
        self.with_view(access, |view| {
            let view = view.ok_or(Error::Invalid)?;
            let root = view.snapshot().state.root.map(|r| r.cell);
            let request = question
                .core(access.id(), root)
                .map_err(|_| Error::Invalid)?;
            view.cell_ticket(request).map_err(core_error)
        })
    }
    /// A cell question already in core form (its root chosen by the caller).
    pub fn cell_request(&self, access: &Access, request: CellRequest) -> Result<CellWait> {
        self.with_view(access, |view| {
            view.ok_or(Error::Invalid)?
                .cell_ticket(request)
                .map_err(core_error)
        })
    }
    /// The queueing half of a root edit: the edit that commits the resolved
    /// root is a separate, non-blocking `edit`.
    pub fn root_ticket(&self, access: &Access, source: usize, cell: u32) -> Result<CellWait> {
        self.with_view(access, |view| {
            view.ok_or(Error::Busy)?
                .root_ticket(source, cell)
                .map_err(core_error)
        })
    }
    pub fn latest(&self, access: &Access) -> Result<Option<Arc<DisplayFrame>>> {
        self.with_view(access, |view| Ok(view.and_then(ViewController::latest)))
    }
    pub(super) fn sample(&self, access: &Access) -> Result<Option<(Snapshot, Arc<Model>)>> {
        self.with_view(access, |v| {
            Ok(v.map(|v| (v.snapshot(), Arc::clone(&v.model))))
        })
    }
    pub fn edit(&self, access: &Access, state_rev: u64, patch: Patch) -> Result<()> {
        if let Some((w, h)) = patch.pixels {
            self.check_pixels(w, h)?;
        }
        self.with_view(access, |view| {
            view.ok_or(Error::Busy)?
                .edit(state_rev, patch)
                .map(|_| ())
                .map_err(core_error)
        })
    }
    fn check_pixels(&self, width: u32, height: u32) -> Result<()> {
        if self.broker.is_public_demo()
            && (width > 2048 || height > 2048 || u64::from(width) * u64::from(height) > 2_097_152)
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }

    /// Nonblocking maintenance. Finished threads only; no native join under a
    /// broker lock. Failed native lifecycle cleanup is not reported as success.
    pub fn maintain(&self) -> Result<()> {
        self.broker.maintain(Instant::now())?;
        let completed = {
            let mut state = self.state.lock().map_err(|_| Error::Unavailable)?;
            let ids: Vec<_> = state
                .runners
                .iter()
                .filter(|(_, r)| r.thread.is_finished())
                .map(|(id, _)| id.clone())
                .collect();
            ids.into_iter()
                .map(|id| state.runners.remove(&id).unwrap())
                .collect::<Vec<_>>()
        };
        let mut failed = false;
        for runner in completed {
            failed |= !matches!(runner.thread.join(), Ok(Ok(())));
        }
        if failed {
            Err(Error::Unavailable)
        } else {
            Ok(())
        }
    }
    /// Stops this broker's session views, never any shared index coordinator.
    pub fn request_stop(&self) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .stopping = true;
        let _ = self.broker.stop();
    }
    /// Call on the host/control thread, NOT the HTTP reactor. NFS syscalls can
    /// still block preparation: do not free slots early on an arbitrary timeout.
    pub fn close(&mut self) -> Result<()> {
        self.drain()
    }
    /// Host shutdown only, on a blocking thread. Stops admission before taking
    /// the handles; never join while holding either broker or runtime mutex.
    pub(super) fn drain(&self) -> Result<()> {
        self.request_stop();
        let runners =
            std::mem::take(&mut self.state.lock().unwrap_or_else(|e| e.into_inner()).runners);
        let mut failed = false;
        for (_, runner) in runners {
            failed |= !matches!(runner.thread.join(), Ok(Ok(())));
        }
        if failed || self.broker.pending_workers()? != 0 {
            Err(Error::Unavailable)
        } else {
            Ok(())
        }
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

fn core_error(error: floe_app_core::Error) -> Error {
    match error.kind {
        ErrorKind::Busy => Error::Busy,
        ErrorKind::InvalidInput | ErrorKind::Unsupported => Error::Invalid,
        _ => Error::Unavailable,
    }
}
fn run(
    broker: &Broker,
    resources: &Arc<Resources>,
    lease: &WorkerLease,
    reserved: ReservedView,
    control: &Control,
    width: u32,
    height: u32,
) -> floe_app_core::Result<()> {
    let stop = &lease.stop;
    let source = broker
        .policy
        .register_source(&lease.binding.selection, stop)?;
    if source.path() != lease.binding.source
        || super::source_stamp(source.path()).ok() != Some(lease.binding.stamp)
    {
        return Err(floe_app_core::Error::input("delegated source changed"));
    }
    let data = ManagedDataset::open_revisions(resources, &source, None, Mode::Level, stop)?;
    source.validate(stop)?;
    if super::source_stamp(source.path()).ok() != Some(lease.binding.stamp) {
        return Err(floe_app_core::Error::input("delegated source changed"));
    }
    // Preparation can take longer than the credential lifetime even without
    // an HTTP maintenance loop. Expire it before starting a native process.
    broker
        .maintain(Instant::now())
        .map_err(|_| floe_app_core::Error::input("broker unavailable"))?;
    floe_app_core::check_cancelled(stop)?;
    if control.idle(Instant::now()) {
        return Err(floe_app_core::Error::new(
            ErrorKind::Cancelled,
            "view idle during preparation",
        ));
    }
    let model = Model::new(&data)?;
    let mut initial = ViewState::initial(&model, width, height)?;
    // Native jobdeck label drawing is not supported. Layout labels remain on.
    if model.deck {
        initial.labels = false;
    }
    let view = reserved.start(data, initial)?;
    *control.view.lock().unwrap_or_else(|e| e.into_inner()) = Some(view);
    let mut maintenance = Instant::now();
    loop {
        if stop.load(Ordering::Relaxed) != 0 || control.idle(Instant::now()) {
            break;
        }
        if control
            .view
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_none_or(ViewController::is_finished)
        {
            break;
        }
        if maintenance.elapsed() >= Duration::from_millis(250) {
            broker
                .maintain(Instant::now())
                .map_err(|_| floe_app_core::Error::input("broker unavailable"))?;
            maintenance = Instant::now();
        }
        thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn activity_grace_is_bounded_and_read_activity_refreshes_it() {
        let control = Control::default();
        let now = Instant::now();
        assert!(!control.idle(now));
        assert!(control.idle(now + Duration::from_secs(120)));
        *control.last_used.lock().unwrap() = now + Duration::from_secs(100);
        assert!(!control.idle(now + Duration::from_secs(120)));
        assert!(control.idle(now + Duration::from_secs(220)));
    }
}
