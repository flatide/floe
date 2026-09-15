//! One bounded control thread owns one worker. It polls/drains even without a
//! frame subscriber; HTTP/WS credit never gates worker cleanup or cancellation.
use super::{
    margin, query, Model, Patch, QueryAnchor, QuerySnapshot, ViewQuery, ViewQueryResult, ViewState,
    Viewport,
};
use crate::{
    managed::{ManagedDataset, Permit, Resources},
    render::{RenderOptions, RenderSession},
    Error, ErrorKind, Result,
};
use floe_worker_client::{Event, Frame, QueryKind, QueryReply, QueryRequest, RenderRequest, Style};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex, Weak,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Opening,
    Rendering,
    Cancelling,
    Idle,
    Failed,
    Closed,
}
#[derive(Clone, Copy, Debug)]
pub struct ControllerOptions {
    pub margin_prefetch: bool,
    pub frame_cache: bool,
}
impl Default for ControllerOptions {
    fn default() -> Self {
        Self {
            margin_prefetch: false,
            frame_cache: true,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    Foreground,
    Margin,
}
#[derive(Clone, Copy, Debug)]
pub struct MarginStatus {
    pub frame_id: u64,
    pub origin_px: [i32; 2],
    pub crop_safe: bool,
}
#[derive(Debug)]
pub struct DisplayFrame {
    pub id: u64,
    pub dataset_revision: u64,
    pub state_rev: u64,
    pub render_rev: u64,
    pub render_key: u64,
    pub worker_epoch: u64,
    pub deck_skipped: usize,
    pub purpose: Purpose,
    pub frame: Frame,
}
impl DisplayFrame {
    pub fn viewport(&self) -> Viewport {
        Viewport {
            bbox: self.frame.request.view,
            width: self.frame.request.width,
            height: self.frame.request.height,
        }
    }
    pub fn matches(&self, snapshot: &Snapshot) -> bool {
        self.worker_epoch == snapshot.worker_epoch
            && self.render_key == snapshot.render_key
            && match self.purpose {
                Purpose::Foreground => self.render_rev == snapshot.render_rev,
                Purpose::Margin => snapshot.margin.is_some_and(|m| m.frame_id == self.id),
            }
    }
}
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub state: ViewState,
    pub state_rev: u64,
    pub render_rev: u64,
    pub render_key: u64,
    pub worker_epoch: u64,
    pub phase: Phase,
    pub max_depth: Option<u64>,
    pub submitted: u64,
    pub consumed: u64,
    pub discarded: u64,
    pub margin_enabled: bool,
    pub margin_working: bool,
    pub margin_submitted: u64,
    pub crop_hits: u64,
    pub margin: Option<MarginStatus>,
    pub margin_failure: Option<(ErrorKind, String)>,
    /// Local diagnostics only; web maps the kind to a safe message/code.
    pub failure: Option<(ErrorKind, String)>,
}
struct Shared {
    replacement_pending: bool,
    snapshot: Snapshot,
    latest: Option<Arc<DisplayFrame>>,
    margin: Option<Arc<DisplayFrame>>,
    queries: query::Queries,
}
impl Shared {
    fn prune_queries(&mut self, model: &Model) {
        for kind in query::KINDS {
            if self
                .queries
                .latest_anchor(kind)
                .is_some_and(|a| !self.anchor_valid(a, model))
            {
                self.queries.cancel(kind);
            }
        }
    }
    fn query_frame(&self, id: u64) -> Option<&DisplayFrame> {
        self.latest
            .as_deref()
            .filter(|f| f.id == id)
            .or_else(|| self.margin.as_deref().filter(|f| f.id == id))
    }
    fn anchor_valid(&self, anchor: QueryAnchor, model: &Model) -> bool {
        !matches!(
            self.snapshot.phase,
            Phase::Closed | Phase::Failed | Phase::Opening
        ) && self
            .query_frame(anchor.frame_id)
            .is_some_and(|f| anchor.matches(f, &self.snapshot(), model))
    }
    fn query_request(&self, input: &ViewQuery, model: &Model) -> Result<QueryRequest> {
        if model.deck {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "jobdeck queries are not implemented",
            ));
        }
        if !self.anchor_valid(input.anchor, model) {
            return Err(Error::new(
                ErrorKind::Busy,
                "displayed query frame or view state is stale",
            ));
        }
        let displayed = self
            .query_frame(input.anchor.frame_id)
            .unwrap()
            .frame
            .query_scene()?;
        if !displayed.complete {
            return Err(Error::new(
                ErrorKind::Incomplete,
                "displayed geometry is incomplete",
            ));
        }
        let source = self
            .queries
            .source
            .as_ref()
            .ok_or_else(|| Error::new(ErrorKind::Busy, "no published query scene"))?;
        query::request(input, &self.snapshot(), model, source)
    }
    fn snapshot(&self) -> Snapshot {
        let mut s = self.snapshot.clone();
        s.margin = self.margin.as_ref().and_then(|f| {
            if f.render_key != s.render_key || f.worker_epoch != s.worker_epoch {
                return None;
            }
            let origin_px = margin::origin(f.viewport(), s.state.viewport)?;
            Some(MarginStatus {
                frame_id: f.id,
                origin_px,
                crop_safe: f.frame.complete() && margin::covers(f.viewport(), s.state.viewport),
            })
        });
        s
    }
}
pub struct ViewController {
    pub model: Arc<Model>,
    // Does not extend the view's cache lease after its engine exits. An
    // accepted export explicitly upgrades/pins this before leaving the view.
    dataset: Weak<ManagedDataset>,
    shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicUsize>,
    thread: Option<JoinHandle<()>>,
    resources: Weak<Resources>,
    reservation: Weak<Permit>,
    native_options: Option<RenderOptions>,
    configuration: ControllerOptions,
}

/// A dormant replacement owns the SAME reservation, not a second worker slot.
/// Prepare can fail without stopping the original. Commit is the cutover: the
/// new engine waits for the original's complete close/drop/reap before opening.
pub struct PreparedReplacement {
    previous: Arc<ViewController>,
    next: Arc<ViewController>,
    ready: Arc<AtomicBool>,
    committed: bool,
}
impl PreparedReplacement {
    /// For preparing attachment metadata before cutover; do not publish this
    /// handle until commit succeeds. It cannot render before then.
    pub fn controller(&self) -> Arc<ViewController> {
        Arc::clone(&self.next)
    }
    pub fn commit(&mut self, base_state_rev: u64) -> Result<()> {
        let mut s = self.previous.shared.lock().unwrap();
        if self.committed
            || !replacement_ready(&s, &self.previous.stop)
            || s.snapshot.state_rev != base_state_rev
            || self.next.stop.load(Ordering::Relaxed) != 0
        {
            return Err(Error::new(
                ErrorKind::Busy,
                "replacement view is stale or closed",
            ));
        }
        self.previous.stop.store(1, Ordering::Relaxed);
        s.queries.invalidate();
        self.committed = true;
        self.ready.store(true, Ordering::Release);
        Ok(())
    }
}
impl Drop for PreparedReplacement {
    fn drop(&mut self) {
        if !self.committed {
            self.next.request_close();
            self.previous.shared.lock().unwrap().replacement_pending = false;
        }
    }
}
fn replacement_ready(s: &Shared, stop: &AtomicUsize) -> bool {
    stop.load(Ordering::Relaxed) == 0 && matches!(s.snapshot.phase, Phase::Idle | Phase::Rendering)
}
trait Engine: Send {
    fn submit(&mut self, request: RenderRequest) -> Result<u64>;
    fn cancel(&mut self) -> Result<u64>;
    fn pending(&self) -> usize;
    fn query(&mut self, request: QueryRequest) -> Result<u64>;
    fn cancel_queries(&mut self, kind: QueryKind) -> Result<u64>;
    fn pending_queries(&self) -> usize;
    fn poll(&mut self, timeout: Duration) -> Result<Option<Event>>;
    fn styles(&mut self, styles: &[Style]) -> Result<()>;
    fn base(&self) -> RenderRequest;
    fn close(&mut self) -> Result<()>;
    fn max_depth(&self) -> Option<u64> {
        None
    }
}
impl Engine for RenderSession {
    fn query(&mut self, r: QueryRequest) -> Result<u64> {
        self.query(r)
    }
    fn cancel_queries(&mut self, k: QueryKind) -> Result<u64> {
        self.cancel_queries(k)
    }
    fn pending_queries(&self) -> usize {
        self.pending_queries()
    }
    fn max_depth(&self) -> Option<u64> {
        Some(self.max_depth())
    }
    fn submit(&mut self, r: RenderRequest) -> Result<u64> {
        self.submit(r)
    }
    fn cancel(&mut self) -> Result<u64> {
        self.cancel()
    }
    fn pending(&self) -> usize {
        self.pending_generations()
    }
    fn poll(&mut self, d: Duration) -> Result<Option<Event>> {
        self.poll(d)
    }
    fn styles(&mut self, s: &[Style]) -> Result<()> {
        self.set_styles(s)
    }
    fn base(&self) -> RenderRequest {
        self.base_request()
    }
    fn close(&mut self) -> Result<()> {
        self.close()
    }
}
impl ViewController {
    pub fn start(
        resources: &Arc<Resources>,
        dataset: Arc<ManagedDataset>,
        options: RenderOptions,
        initial: ViewState,
    ) -> Result<Self> {
        Self::start_configured(
            resources,
            dataset,
            options,
            initial,
            ControllerOptions::default(),
        )
    }
    pub fn start_configured(
        resources: &Arc<Resources>,
        dataset: Arc<ManagedDataset>,
        options: RenderOptions,
        initial: ViewState,
        configuration: ControllerOptions,
    ) -> Result<Self> {
        let native_options = options.clone();
        let model = Model::new(&dataset)?;
        let permit = resources.render(&options)?;
        let weak = Arc::downgrade(&dataset);
        let mut controller = Self::spawn(
            resources,
            model,
            initial,
            permit,
            configuration,
            move |stop| {
                let engine = RenderSession::open(&dataset.dataset, options, false, stop)?;
                // Keep the cache read lease until after engine close/drop/reap.
                Ok((Box::new(engine) as Box<dyn Engine>, Some(dataset)))
            },
        )?;
        controller.dataset = weak;
        controller.native_options = Some(native_options);
        Ok(controller)
    }
    pub fn prepare_replacement(
        self: &Arc<Self>,
        dataset: Arc<ManagedDataset>,
        initial: ViewState,
    ) -> Result<PreparedReplacement> {
        let options = self.native_options.clone().ok_or_else(|| {
            Error::new(
                ErrorKind::Unsupported,
                "replacement needs a native controller",
            )
        })?;
        let native_options = options.clone();
        let model = Model::new(&dataset)?;
        let weak = Arc::downgrade(&dataset);
        let mut prepared = self.prepare_engine(model, initial, move |stop| {
            let engine = RenderSession::open(&dataset.dataset, options, false, stop)?;
            Ok((Box::new(engine) as Box<dyn Engine>, Some(dataset)))
        })?;
        let next = Arc::get_mut(&mut prepared.next).expect("unexposed replacement");
        next.dataset = weak;
        next.native_options = Some(native_options);
        Ok(prepared)
    }
    fn prepare_engine(
        self: &Arc<Self>,
        model: Arc<Model>,
        initial: ViewState,
        open: impl FnOnce(Arc<AtomicUsize>) -> Result<(Box<dyn Engine>, Option<Arc<ManagedDataset>>)>
            + Send
            + 'static,
    ) -> Result<PreparedReplacement> {
        initial.validate(&model)?;
        let resources = self
            .resources
            .upgrade()
            .ok_or_else(|| Error::new(ErrorKind::Busy, "resources closed"))?;
        let permit = self
            .reservation
            .upgrade()
            .ok_or_else(|| Error::new(ErrorKind::Busy, "worker closed"))?;
        {
            let mut s = self.shared.lock().unwrap();
            if s.replacement_pending || !replacement_ready(&s, &self.stop) {
                return Err(Error::new(
                    ErrorKind::Busy,
                    "view is not ready for replacement",
                ));
            }
            s.replacement_pending = true;
        }
        let ready = Arc::new(AtomicBool::new(false));
        let (gate, previous) = (Arc::clone(&ready), Arc::clone(self));
        let next = Self::spawn_reserved(
            &resources,
            model,
            initial,
            permit,
            self.configuration,
            move |stop| {
                while !gate.load(Ordering::Acquire) {
                    if stop.load(Ordering::Relaxed) != 0 {
                        // Serialize an unactivated abort with commit's final
                        // stop check: never miss a concurrent cutover and exit
                        // before the predecessor has been reaped.
                        let _state = previous.shared.lock().unwrap();
                        if !gate.load(Ordering::Acquire) {
                            crate::check_cancelled(&stop)?;
                        }
                    }
                    thread::sleep(Duration::from_millis(2));
                }
                // Even cancellation must not make this controller "finished"
                // while its predecessor still owns a live native worker.
                while !previous.is_finished() {
                    thread::sleep(Duration::from_millis(2));
                }
                crate::check_cancelled(&stop)?;
                open(stop)
            },
        );
        match next {
            Ok(next) => Ok(PreparedReplacement {
                previous: Arc::clone(self),
                next: Arc::new(next),
                ready,
                committed: false,
            }),
            Err(e) => {
                self.shared.lock().unwrap().replacement_pending = false;
                Err(e)
            }
        }
    }
    fn spawn(
        resources: &Arc<Resources>,
        model: Arc<Model>,
        initial: ViewState,
        permit: Permit,
        configuration: ControllerOptions,
        open: impl FnOnce(Arc<AtomicUsize>) -> Result<(Box<dyn Engine>, Option<Arc<ManagedDataset>>)>
            + Send
            + 'static,
    ) -> Result<Self> {
        Self::spawn_reserved(
            resources,
            model,
            initial,
            Arc::new(permit),
            configuration,
            open,
        )
    }
    fn spawn_reserved(
        resources: &Arc<Resources>,
        model: Arc<Model>,
        initial: ViewState,
        permit: Arc<Permit>,
        configuration: ControllerOptions,
        open: impl FnOnce(Arc<AtomicUsize>) -> Result<(Box<dyn Engine>, Option<Arc<ManagedDataset>>)>
            + Send
            + 'static,
    ) -> Result<Self> {
        initial.validate(&model)?;
        let reservation = Arc::downgrade(&permit);
        let resource_ref = Arc::downgrade(resources);
        let epoch = resources.next_id()?;
        let stop = Arc::new(AtomicUsize::new(0));
        let shared = Arc::new(Mutex::new(Shared {
            replacement_pending: false,
            snapshot: Snapshot {
                state: initial,
                state_rev: 1,
                render_rev: 1,
                render_key: 1,
                worker_epoch: epoch,
                phase: Phase::Opening,
                max_depth: None,
                submitted: 0,
                consumed: 0,
                discarded: 0,
                margin_enabled: configuration.margin_prefetch
                    && configuration.frame_cache
                    && !model.deck,
                margin_working: false,
                margin_submitted: 0,
                crop_hits: 0,
                margin: None,
                margin_failure: None,
                failure: None,
            },
            latest: None,
            margin: None,
            queries: query::Queries::default(),
        }));
        let (state, flag, model2) = (Arc::clone(&shared), Arc::clone(&stop), Arc::clone(&model));
        let resources = Arc::clone(resources);
        let thread = thread::Builder::new()
            .name("floe-view-control".into())
            .spawn(move || {
                let _permit = permit;
                let result = (|| {
                    let (mut engine, lease) = open(Arc::clone(&flag))?;
                    state.lock().unwrap().snapshot.max_depth = engine.max_depth();
                    let result = run(
                        engine.as_mut(),
                        &state,
                        &flag,
                        &model2,
                        &resources,
                        configuration,
                    );
                    let close = engine.close();
                    drop(engine);
                    drop(lease);
                    result.and(close)
                })();
                let mut s = state.lock().unwrap();
                s.latest = None;
                s.margin = None;
                s.queries.invalidate();
                s.queries.in_flight.clear();
                s.queries.source = None;
                s.snapshot.margin_working = false;
                if flag.load(Ordering::Relaxed) != 0 || result.is_ok() {
                    s.snapshot.phase = Phase::Closed;
                } else if let Err(e) = result {
                    s.snapshot.phase = Phase::Failed;
                    s.snapshot.failure = Some((e.kind, e.message));
                }
            })?;
        Ok(Self {
            dataset: Weak::new(),
            model,
            shared,
            stop,
            thread: Some(thread),
            resources: resource_ref,
            reservation,
            native_options: None,
            configuration,
        })
    }
    pub fn snapshot(&self) -> Snapshot {
        self.shared.lock().unwrap().snapshot()
    }
    /// Pin the immutable cache lease for owner-side preparation. Browser
    /// authorization and view/revision validation remain the owner's job.
    pub fn pin_dataset(&self) -> Result<Arc<ManagedDataset>> {
        self.dataset
            .upgrade()
            .ok_or_else(|| Error::new(ErrorKind::Busy, "view dataset closed"))
    }
    pub fn latest(&self) -> Option<Arc<DisplayFrame>> {
        self.shared.lock().unwrap().latest.clone()
    }
    pub fn margin(&self) -> Option<Arc<DisplayFrame>> {
        let s = self.shared.lock().unwrap();
        s.margin.clone().filter(|f| f.matches(&s.snapshot()))
    }
    /// Caller supplies the frame actually displayed, not merely the last frame
    /// received. The eventual transport must also bind its view/connection ID.
    pub fn query_anchor(&self, frame_id: u64) -> Result<QueryAnchor> {
        let s = self.shared.lock().unwrap();
        let f = s
            .query_frame(frame_id)
            .ok_or_else(|| Error::new(ErrorKind::Busy, "query frame is no longer retained"))?;
        let anchor = QueryAnchor::new(f, &s.snapshot());
        if self.stop.load(Ordering::Relaxed) != 0 || !s.anchor_valid(anchor, &self.model) {
            return Err(Error::new(
                ErrorKind::Busy,
                "query frame is not displayed in this state",
            ));
        }
        Ok(anchor)
    }
    /// Latest-only per kind: superseded IDs need not get a result. No native
    /// work or allocation proportional to history occurs on the caller thread.
    pub fn query(&self, input: ViewQuery) -> Result<u64> {
        let mut s = self.shared.lock().unwrap();
        if self.stop.load(Ordering::Relaxed) != 0 {
            return Err(Error::new(ErrorKind::Cancelled, "view is closing"));
        }
        let native = s.query_request(&input, &self.model)?;
        s.queries.enqueue(input, native)
    }
    pub fn query_snapshot(&self) -> QuerySnapshot {
        self.shared.lock().unwrap().queries.snapshot()
    }
    /// Freeze exact clip bounds and the current visible selection on an
    /// authenticated displayed receipt. The gateway checks that receipt's
    /// connection; this method checks its current controller state atomically.
    /// None bounds means the viewport; explicit bounds are already integer DBU.
    pub fn prepare_clip(
        &self,
        anchor: QueryAnchor,
        bbox: Option<[i64; 4]>,
        visible: bool,
        mut request: floe_worker_client::ClipRequest,
    ) -> Result<floe_worker_client::ClipRequest> {
        let s = self.shared.lock().unwrap();
        self.validate_clip_anchor(&s, anchor)?;
        request.bbox = match bbox {
            Some(b) => b,
            None => crate::clip::bbox_dbu(s.snapshot.state.viewport.bbox, 1.)?,
        };
        if visible {
            request.layers = s.snapshot.state.layers.clone();
        }
        request.validate()?;
        Ok(request)
    }
    /// Preparing a browser dialog does not pin a dataset. Only accepting its
    /// still-current receipt obtains an owning lease for the export lifetime.
    pub fn pin_clip(&self, anchor: QueryAnchor) -> Result<Arc<ManagedDataset>> {
        let s = self.shared.lock().unwrap();
        self.validate_clip_anchor(&s, anchor)?;
        self.dataset
            .upgrade()
            .ok_or_else(|| Error::new(ErrorKind::Busy, "view dataset closed"))
    }
    fn validate_clip_anchor(&self, s: &Shared, anchor: QueryAnchor) -> Result<()> {
        if self.model.deck {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "jobdeck clip is unsupported",
            ));
        }
        if self.stop.load(Ordering::Relaxed) != 0 || !s.anchor_valid(anchor, &self.model) {
            return Err(Error::new(ErrorKind::Busy, "clip frame is stale"));
        }
        Ok(())
    }
    /// Read-only coordinate measurement on a displayed frame. With snap off,
    /// this does not require an exact scene (including deck/summary views).
    /// A supplied snap ID must still be the current successful query at this
    /// position and anchor; a refusal is never converted to an unsnapped point.
    pub fn measure(
        &self,
        anchor: QueryAnchor,
        position: [f64; 2],
        start: Option<super::RulerPoint>,
        free_angle: bool,
        snap_id: Option<u64>,
    ) -> Result<super::RulerMeasurement> {
        use floe_worker_client::{QueryHit, QueryStatus};
        let s = self.shared.lock().unwrap();
        if self.stop.load(Ordering::Relaxed) != 0 || !s.anchor_valid(anchor, &self.model) {
            return Err(Error::new(ErrorKind::Busy, "measurement frame is stale"));
        }
        let mut point = super::RulerPoint::cursor(s.snapshot.state.viewport, position)?;
        let mut snap = None;
        if let Some(id) = snap_id {
            let q = &s.queries.slots[query::slot(QueryKind::Snap)];
            let result = q
                .result
                .as_ref()
                .filter(|r| q.latest == Some(id) && r.id == id && r.anchor == anchor)
                .ok_or_else(|| Error::new(ErrorKind::Busy, "snap result is stale"))?;
            let native = s.query_request(
                &ViewQuery {
                    anchor,
                    operation: super::QueryOperation::Snap,
                    position,
                    radius_px: 0.,
                    layers: floe_worker_client::Layers::All,
                },
                &self.model,
            )?;
            if result.reply.request.x != native.x || result.reply.request.y != native.y {
                return Err(Error::input("snap position changed"));
            }
            if result.reply.status != QueryStatus::Ok {
                return Err(Error::new(
                    ErrorKind::Incomplete,
                    "snap did not complete successfully",
                ));
            }
            match &result.reply.hit {
                Some(QueryHit::Snap(hit)) => {
                    point = super::RulerPoint::snapped(hit.x, hit.y);
                    snap = Some(hit.kind);
                }
                None => (),
                _ => return Err(Error::new(ErrorKind::Worker, "invalid snap result")),
            }
        }
        super::ruler::measure(start, point, free_angle, self.model.dbu, snap)
    }
    pub fn cancel_query(&self, kind: QueryKind) {
        self.shared.lock().unwrap().queries.cancel(kind);
    }
    /// Bounded bbox annotation measurement. No worker query or redraw is made.
    pub fn measure_selection(
        &self,
        anchor: QueryAnchor,
        boxes: &[[i64; 4]],
    ) -> Result<Vec<super::RulerSegment>> {
        let s = self.shared.lock().unwrap();
        if self.stop.load(Ordering::Relaxed) != 0 || !s.anchor_valid(anchor, &self.model) {
            return Err(Error::new(ErrorKind::Busy, "measurement frame is stale"));
        }
        super::ruler::measure_selection(boxes, self.model.dbu)
    }
    /// A departing consumer must not cancel a newer consumer's request. The
    /// local query ID is a compare-and-cancel stamp, not an authority token.
    pub fn cancel_query_if_current(&self, kind: QueryKind, id: u64) -> bool {
        let mut s = self.shared.lock().unwrap();
        if s.queries.slots[query::slot(kind)].latest != Some(id) {
            return false;
        }
        s.queries.cancel(kind);
        true
    }
    /// A conflict changes neither view nor pending render. Caller returns the
    /// authoritative snapshot, rather than retrying relative deltas blindly.
    pub fn edit(&self, base_state_rev: u64, mut patch: Patch) -> Result<Snapshot> {
        let mut s = self.shared.lock().unwrap();
        if matches!(s.snapshot.phase, Phase::Closed | Phase::Failed)
            || self.stop.load(Ordering::Relaxed) != 0
        {
            return Err(Error::new(
                ErrorKind::Worker,
                "view is closed or failed; reopen required",
            ));
        }
        if s.snapshot.state_rev != base_state_rev {
            return Err(Error::new(ErrorKind::Busy, "stale view state revision"));
        }
        if let Some(super::Depth::Step(delta)) = patch.depth {
            patch.depth = Some(super::Depth::stepped(
                s.snapshot.state.depth,
                s.snapshot.max_depth,
                delta,
            )?);
        }
        let next = s.snapshot.state.edit(&self.model, patch)?;
        if next == s.snapshot.state {
            return Ok(s.snapshot());
        }
        let key_changed = !next.same_policy(&s.snapshot.state, self.model.deck);
        let render_changed = key_changed || next.viewport != s.snapshot.state.viewport;
        let inc = |n: u64| {
            n.checked_add(1)
                .ok_or_else(|| Error::input("view revision exhausted"))
        };
        let rev = inc(s.snapshot.state_rev)?;
        let render_rev = if render_changed {
            inc(s.snapshot.render_rev)?
        } else {
            s.snapshot.render_rev
        };
        let key = if key_changed {
            inc(s.snapshot.render_key)?
        } else {
            s.snapshot.render_key
        };
        s.snapshot.state = next;
        s.snapshot.state_rev = rev;
        s.snapshot.render_rev = render_rev;
        s.snapshot.render_key = key;
        s.queries.invalidate();
        if render_changed {
            s.latest = None;
        }
        if key_changed
            || s.margin
                .as_ref()
                .is_some_and(|f| margin::origin(f.viewport(), s.snapshot.state.viewport).is_none())
        {
            s.margin = None;
        }
        Ok(s.snapshot())
    }
    /// Non-blocking; interrupts ready/open/style and worker polling too.
    pub fn request_close(&self) {
        self.stop.store(1, Ordering::Relaxed);
        self.shared.lock().unwrap().queries.invalidate();
    }
    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(|t| t.is_finished())
    }
    /// Join on a service/control thread, NOT the HTTP reactor. The worker owns
    /// its bounded terminate/reap path; no subscriber can prolong this wait.
    pub fn close(&mut self) -> Result<()> {
        self.request_close();
        if let Some(t) = self.thread.take() {
            t.join()
                .map_err(|_| Error::new(ErrorKind::Worker, "view controller panicked"))?;
        }
        Ok(())
    }
}
impl Drop for ViewController {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
struct Ticket {
    generation: u64,
    snapshot: Snapshot,
    purpose: Purpose,
    viewport: Viewport,
}
fn run(
    engine: &mut dyn Engine,
    shared: &Mutex<Shared>,
    stop: &AtomicUsize,
    model: &Model,
    resources: &Resources,
    configuration: ControllerOptions,
) -> Result<()> {
    let mut styles = Arc::clone(&model.styles);
    let mut active: Option<Ticket> = None;
    let mut handled_rev = 0;
    let mut draining: Option<Instant> = None;
    let mut margin_attempt: Option<(u64, Viewport)> = None;
    let mut base = engine.base();
    base.frame_cache = configuration.frame_cache;
    while stop.load(Ordering::Relaxed) == 0 {
        pump_queries(engine, &mut shared.lock().unwrap(), model)?;
        let current = shared.lock().unwrap().snapshot();
        let covered = current.margin.is_some_and(|m| m.crop_safe);
        if current.render_rev != handled_rev && covered {
            handled_rev = current.render_rev;
            let mut s = shared.lock().unwrap();
            s.snapshot.crop_hits += 1;
            s.snapshot.phase = Phase::Idle;
        }
        let needs_foreground = current.render_rev != handled_rev;
        let stale_active = active.as_ref().is_some_and(|t| match t.purpose {
            Purpose::Foreground => t.snapshot.render_rev != current.render_rev,
            Purpose::Margin => {
                needs_foreground
                    || t.snapshot.render_key != current.render_key
                    || margin::origin(t.viewport, current.state.viewport).is_none()
            }
        });
        if stale_active && draining.is_none() && engine.pending() > 0 {
            engine.cancel()?;
            draining = Some(Instant::now());
            let mut s = shared.lock().unwrap();
            s.snapshot.margin_working = false;
            if needs_foreground {
                s.snapshot.phase = Phase::Cancelling;
            }
        }
        if engine.pending() == 0 {
            draining = None;
            active = None;
            let mut submit = None;
            if needs_foreground {
                submit = Some((Purpose::Foreground, current.state.viewport));
            } else if current.margin_enabled {
                let s = shared.lock().unwrap();
                let settled = covered
                    || s.latest
                        .as_ref()
                        .is_some_and(|f| f.render_rev == current.render_rev && f.frame.complete());
                let landed = s.margin.as_ref().is_some_and(|f| {
                    f.render_key == current.render_key
                        && margin::comfortable(f.viewport(), current.state.viewport)
                });
                let attempted = margin_attempt.is_some_and(|(key, v)| {
                    key == current.render_key && margin::comfortable(v, current.state.viewport)
                });
                if settled && !landed && !attempted {
                    submit = margin::grow(current.state.viewport).map(|v| (Purpose::Margin, v));
                }
            }
            // Synchronous style ACK must not swallow query replies. An edit
            // invalidates/cancels queries; keep draining before changing styles.
            let waiting_styles = current.state.styles != styles && engine.pending_queries() != 0;
            if waiting_styles {
                shared.lock().unwrap().snapshot.phase = Phase::Cancelling;
            }
            if let Some((purpose, viewport)) = submit.filter(|_| !waiting_styles) {
                if current.state.styles != styles {
                    engine.styles(&current.state.styles)?;
                    styles = Arc::clone(&current.state.styles);
                }
                let mut request = current.state.request(model, base.clone());
                request.view = viewport.bbox;
                request.width = viewport.width;
                request.height = viewport.height;
                let generation = engine.submit(request)?;
                if purpose == Purpose::Foreground {
                    handled_rev = current.render_rev;
                }
                if purpose == Purpose::Margin {
                    margin_attempt = Some((current.render_key, viewport));
                }
                active = Some(Ticket {
                    generation,
                    snapshot: current,
                    purpose,
                    viewport,
                });
                let mut s = shared.lock().unwrap();
                s.snapshot.submitted += 1;
                if purpose == Purpose::Foreground {
                    s.snapshot.phase = Phase::Rendering;
                } else {
                    s.snapshot.margin_submitted += 1;
                    s.snapshot.margin_working = true;
                    s.snapshot.margin_failure = None;
                }
            }
        }
        if draining.is_some_and(|at| at.elapsed() > Duration::from_secs(5)) {
            return Err(Error::new(
                ErrorKind::Worker,
                "cancel drain deadline exceeded",
            ));
        }
        match engine.poll(Duration::from_millis(20))? {
            Some(Event::Query(reply)) => {
                consume_query(&mut shared.lock().unwrap(), reply, model)?;
            }
            Some(Event::Frame(frame)) => {
                let mut s = shared.lock().unwrap();
                s.snapshot.consumed += 1;
                if let Some(t) = active.as_ref().filter(|t| {
                    t.generation == frame.generation
                        && draining.is_none()
                        && match t.purpose {
                            Purpose::Foreground => t.snapshot.render_rev == s.snapshot.render_rev,
                            Purpose::Margin => {
                                frame.final_frame
                                    && t.snapshot.render_key == s.snapshot.render_key
                                    && margin::origin(t.viewport, s.snapshot.state.viewport)
                                        .is_some()
                            }
                        }
                }) {
                    if t.purpose == Purpose::Foreground && frame.final_frame {
                        s.snapshot.phase = Phase::Idle;
                    }
                    let f = Arc::new(DisplayFrame {
                        id: resources.next_id()?,
                        dataset_revision: model.dataset_revision,
                        state_rev: t.snapshot.state_rev,
                        render_rev: t.snapshot.render_rev,
                        render_key: t.snapshot.render_key,
                        worker_epoch: t.snapshot.worker_epoch,
                        deck_skipped: model.skipped,
                        purpose: t.purpose,
                        frame,
                    });
                    s.queries.observe(&f)?;
                    if t.purpose == Purpose::Foreground {
                        s.latest = Some(f);
                    } else {
                        s.snapshot.margin_working = false;
                        s.margin = Some(f);
                    }
                    s.prune_queries(model);
                } else {
                    s.snapshot.discarded += 1;
                    if frame.final_frame
                        && active
                            .as_ref()
                            .is_some_and(|t| t.purpose == Purpose::Margin)
                    {
                        s.snapshot.margin_working = false;
                    }
                }
            }
            Some(Event::Failed {
                generation,
                code,
                message,
            }) => {
                // Optional prefetch failure must not erase an already good
                // foreground. Record it and do not retry the same area in a loop.
                if active.as_ref().is_some_and(|t| {
                    t.purpose == Purpose::Margin && generation == Some(t.generation)
                }) {
                    let mut s = shared.lock().unwrap();
                    s.snapshot.margin_working = false;
                    s.snapshot.margin_failure =
                        Some((ErrorKind::Worker, format!("{code}: {message}")));
                } else {
                    return Err(Error::new(ErrorKind::Worker, format!("{code}: {message}")));
                }
            }
            Some(Event::Cancelled { .. }) if draining.is_none() => {
                return Err(Error::new(
                    ErrorKind::Cancelled,
                    "unexpected worker cancellation",
                ))
            }
            _ => (),
        }
    }
    Ok(())
}

fn pump_queries(engine: &mut dyn Engine, s: &mut Shared, model: &Model) -> Result<()> {
    for kind in query::KINDS {
        let k = query::slot(kind);
        let count = s
            .queries
            .in_flight
            .values()
            .filter(|t| t.input.operation.kind() == kind)
            .count();
        if s.queries.slots[k].cancel {
            if count != 0 {
                match engine.cancel_queries(kind) {
                    Ok(_) => (),
                    Err(e) if e.kind == ErrorKind::Busy => continue,
                    Err(e) => return Err(e),
                }
            }
            s.queries.slots[k].cancel = false;
        }
        if count >= query::IN_FLIGHT_PER_KIND {
            continue;
        }
        let Some(mut ticket) = s.queries.slots[k].pending.clone() else {
            continue;
        };
        // Rebind to the current equivalent geometry (e.g. a just-landed margin),
        // while preserving the original displayed frame/state anchor.
        let Ok(request) = s.query_request(&ticket.input, model) else {
            s.queries.slots[k].pending = None;
            s.queries.discard(&ticket);
            continue;
        };
        ticket.native = request;
        match engine.query(ticket.native.clone()) {
            Ok(seq) => {
                s.queries.slots[k].pending = None;
                s.queries.submitted = s.queries.submitted.saturating_add(1);
                if s.queries.in_flight.insert(seq, ticket).is_some() {
                    return Err(Error::new(
                        ErrorKind::Worker,
                        "duplicate engine query sequence",
                    ));
                }
            }
            Err(e) if e.kind == ErrorKind::Busy => (),
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn consume_query(s: &mut Shared, reply: QueryReply, model: &Model) -> Result<()> {
    let ticket = s
        .queries
        .in_flight
        .remove(&reply.sequence)
        .ok_or_else(|| Error::new(ErrorKind::Worker, "unissued controller query reply"))?;
    s.queries.consumed = s.queries.consumed.saturating_add(1);
    if reply.request != ticket.native {
        return Err(Error::new(
            ErrorKind::Worker,
            "controller query request changed",
        ));
    }
    let k = query::slot(ticket.input.operation.kind());
    if s.queries.slots[k].latest != Some(ticket.id) || !s.anchor_valid(ticket.input.anchor, model) {
        s.queries.discard(&ticket);
    } else {
        s.queries.slots[k].result = Some(Arc::new(ViewQueryResult {
            id: ticket.id,
            anchor: ticket.input.anchor,
            reply,
        }));
    }
    Ok(())
}

#[cfg(test)]
#[path = "controller_tests.rs"]
mod tests;
