//! One bounded control thread owns one worker. It polls/drains even without a
//! frame subscriber; HTTP/WS credit never gates worker cleanup or cancellation.
use super::{Model, Patch, ViewState};
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
#[derive(Debug)]
pub struct DisplayFrame {
    pub id: u64,
    pub dataset_revision: u64,
    pub state_rev: u64,
    pub render_rev: u64,
    pub render_key: u64,
    pub worker_epoch: u64,
    pub deck_skipped: usize,
    pub frame: Frame,
}
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub state: ViewState,
    pub state_rev: u64,
    pub render_rev: u64,
    pub render_key: u64,
    pub worker_epoch: u64,
    pub phase: Phase,
    pub submitted: u64,
    pub consumed: u64,
    pub discarded: u64,
    /// Local diagnostics only; web maps the kind to a safe message/code.
    pub failure: Option<(ErrorKind, String)>,
}
struct Shared {
    snapshot: Snapshot,
    latest: Option<Arc<DisplayFrame>>,
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
}
impl Engine for RenderSession {
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
        let model = Model::new(&dataset)?;
        let permit = resources.render(&options)?;
        Self::spawn(resources, model, initial, permit, move |stop| {
            let engine = RenderSession::open(&dataset.dataset, options, false, stop)?;
            // Keep the cache read lease until after engine close/drop/reap.
            Ok((Box::new(engine) as Box<dyn Engine>, Some(dataset)))
        })
    }
    fn spawn(
        resources: &Arc<Resources>,
        model: Arc<Model>,
        initial: ViewState,
        permit: Permit,
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
                submitted: 0,
                consumed: 0,
                discarded: 0,
                failure: None,
            },
            latest: None,
        }));
        let (state, flag, model2) = (Arc::clone(&shared), Arc::clone(&stop), Arc::clone(&model));
        let resources = Arc::clone(resources);
        let thread = thread::Builder::new()
            .name("floe-view-control".into())
            .spawn(move || {
                let _permit = permit;
                let result = (|| {
                    let (mut engine, lease) = open(Arc::clone(&flag))?;
                    let result = run(engine.as_mut(), &state, &flag, &model2, &resources);
                    let close = engine.close();
                    drop(engine);
                    drop(lease);
                    result.and(close)
                })();
                let mut s = state.lock().unwrap();
                s.latest = None;
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
        self.shared.lock().unwrap().snapshot.clone()
    }
    pub fn latest(&self) -> Option<Arc<DisplayFrame>> {
        self.shared.lock().unwrap().latest.clone()
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
            return Ok(s.snapshot.clone());
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
        Ok(s.snapshot.clone())
    }
    /// Non-blocking; interrupts ready/open/style and worker polling too.
    pub fn request_close(&self) {
        self.stop.store(1, Ordering::Relaxed);
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
}
fn run(
    engine: &mut dyn Engine,
    shared: &Mutex<Shared>,
    stop: &AtomicUsize,
    model: &Model,
    resources: &Resources,
) -> Result<()> {
    let mut styles = Arc::clone(&model.styles);
    let mut active: Option<Ticket> = None;
    let mut submitted_rev = 0;
    let mut draining: Option<Instant> = None;
    let base = engine.base();
    while stop.load(Ordering::Relaxed) == 0 {
        let current = shared.lock().unwrap().snapshot.clone();
        if current.render_rev != submitted_rev && draining.is_none() && engine.pending() > 0 {
            engine.cancel()?;
            draining = Some(Instant::now());
            shared.lock().unwrap().snapshot.phase = Phase::Cancelling;
        }
        if engine.pending() == 0 {
            draining = None;
            active = None;
            if current.render_rev != submitted_rev {
                if current.state.styles != styles {
                    engine.styles(&current.state.styles)?;
                    styles = Arc::clone(&current.state.styles);
                }
                let generation = engine.submit(current.state.request(model, base.clone()))?;
                submitted_rev = current.render_rev;
                active = Some(Ticket {
                    generation,
                    snapshot: current,
                });
                let mut s = shared.lock().unwrap();
                s.snapshot.phase = Phase::Rendering;
                s.snapshot.submitted += 1;
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
                        && t.snapshot.render_rev == s.snapshot.render_rev
                        && draining.is_none()
                }) {
                    if frame.final_frame {
                        s.snapshot.phase = Phase::Idle;
                    }
                    s.latest = Some(Arc::new(DisplayFrame {
                        id: resources.next_id()?,
                        dataset_revision: model.dataset_revision,
                        state_rev: t.snapshot.state_rev,
                        render_rev: t.snapshot.render_rev,
                        render_key: t.snapshot.render_key,
                        worker_epoch: t.snapshot.worker_epoch,
                        deck_skipped: model.skipped,
                        frame,
                    }));
                } else {
                    s.snapshot.discarded += 1;
                }
            }
            Some(Event::Failed { code, message, .. }) => {
                return Err(Error::new(ErrorKind::Worker, format!("{code}: {message}")))
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
