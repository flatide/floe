//! One bounded control thread owns one worker. It polls/drains even without a
//! frame subscriber; HTTP/WS credit never gates worker cleanup or cancellation.
use super::{margin, Model, Patch, ViewState, Viewport};
use crate::{
    managed::{ManagedDataset, Permit, Resources},
    render::{RenderOptions, RenderSession},
    Error, ErrorKind, Result,
};
use floe_worker_client::{Event, Frame, RenderRequest, Style};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
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
    snapshot: Snapshot,
    latest: Option<Arc<DisplayFrame>>,
    margin: Option<Arc<DisplayFrame>>,
}
impl Shared {
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
    shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicUsize>,
    thread: Option<JoinHandle<()>>,
}
trait Engine: Send {
    fn submit(&mut self, request: RenderRequest) -> Result<u64>;
    fn cancel(&mut self) -> Result<u64>;
    fn pending(&self) -> usize;
    fn poll(&mut self, timeout: Duration) -> Result<Option<Event>>;
    fn styles(&mut self, styles: &[Style]) -> Result<()>;
    fn base(&self) -> RenderRequest;
    fn close(&mut self) -> Result<()>;
    fn max_depth(&self) -> Option<u64> {
        None
    }
}
impl Engine for RenderSession {
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
        let model = Model::new(&dataset)?;
        let permit = resources.render(&options)?;
        Self::spawn(
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
        )
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
        initial.validate(&model)?;
        let epoch = resources.next_id()?;
        let stop = Arc::new(AtomicUsize::new(0));
        let shared = Arc::new(Mutex::new(Shared {
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
                s.snapshot.margin_working = false;
                if flag.load(Ordering::Relaxed) != 0 || result.is_ok() {
                    s.snapshot.phase = Phase::Closed;
                } else if let Err(e) = result {
                    s.snapshot.phase = Phase::Failed;
                    s.snapshot.failure = Some((e.kind, e.message));
                }
            })?;
        Ok(Self {
            model,
            shared,
            stop,
            thread: Some(thread),
        })
    }
    pub fn snapshot(&self) -> Snapshot {
        self.shared.lock().unwrap().snapshot()
    }
    pub fn latest(&self) -> Option<Arc<DisplayFrame>> {
        self.shared.lock().unwrap().latest.clone()
    }
    pub fn margin(&self) -> Option<Arc<DisplayFrame>> {
        let s = self.shared.lock().unwrap();
        s.margin.clone().filter(|f| f.matches(&s.snapshot()))
    }
    /// A conflict changes neither view nor pending render. Caller returns the
    /// authoritative snapshot, rather than retrying relative deltas blindly.
    pub fn edit(&self, base_state_rev: u64, patch: Patch) -> Result<Snapshot> {
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
                handled_rev = current.render_rev;
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
            if let Some((purpose, viewport)) = submit {
                if current.state.styles != styles {
                    engine.styles(&current.state.styles)?;
                    styles = Arc::clone(&current.state.styles);
                }
                let mut request = current.state.request(model, base.clone());
                request.view = viewport.bbox;
                request.width = viewport.width;
                request.height = viewport.height;
                let generation = engine.submit(request)?;
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
                    if t.purpose == Purpose::Foreground {
                        s.latest = Some(f);
                    } else {
                        s.snapshot.margin_working = false;
                        s.margin = Some(f);
                    }
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

#[cfg(test)]
#[path = "controller_tests.rs"]
mod tests;
