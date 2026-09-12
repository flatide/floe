use super::*;
use crate::{
    managed::{Limits, Usage},
    shots::{Detail, Thin},
    view::{Depth, Navigation, Viewport},
};
use floe_worker_client::{Fields, Fill, FrameFormat, Layers};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::atomic::AtomicBool,
};

struct Control {
    open: AtomicBool,
    frame: AtomicBool,
    drain: AtomicBool,
    fail: AtomicBool,
    requests: Mutex<Vec<RenderRequest>>,
    styles: Mutex<Vec<Vec<Style>>>,
    cancels: AtomicUsize,
    acks: AtomicUsize,
    closed: AtomicUsize,
}
impl Default for Control {
    fn default() -> Self {
        Self {
            open: AtomicBool::new(true),
            frame: AtomicBool::new(true),
            drain: AtomicBool::new(true),
            fail: AtomicBool::new(false),
            requests: Mutex::new(Vec::new()),
            styles: Mutex::new(Vec::new()),
            cancels: AtomicUsize::new(0),
            acks: AtomicUsize::new(0),
            closed: AtomicUsize::new(0),
        }
    }
}
struct Fake {
    control: Arc<Control>,
    active: Option<(u64, RenderRequest)>,
    gen: u64,
    cancelled: bool,
    ack: bool,
}
impl Engine for Fake {
    fn submit(&mut self, r: RenderRequest) -> Result<u64> {
        assert!(self.active.is_none(), "controller queued a second render");
        self.gen += 1;
        self.control.requests.lock().unwrap().push(r.clone());
        self.active = Some((self.gen, r));
        Ok(self.gen)
    }
    fn cancel(&mut self) -> Result<u64> {
        assert!(!self.cancelled);
        self.cancelled = true;
        self.ack = false;
        self.control.cancels.fetch_add(1, Ordering::Relaxed);
        Ok(self.gen + 1)
    }
    fn pending(&self) -> usize {
        usize::from(self.active.is_some())
    }
    fn poll(&mut self, _: Duration) -> Result<Option<Event>> {
        thread::sleep(Duration::from_millis(1));
        if self.control.fail.load(Ordering::Relaxed) && self.active.is_some() {
            let (generation, _) = self.active.take().unwrap();
            return Ok(Some(Event::Failed {
                generation: Some(generation),
                code: "io".into(),
                message: "ENOSPC test".into(),
            }));
        }
        if self.cancelled {
            if !self.ack {
                self.ack = true;
                self.control.acks.fetch_add(1, Ordering::Relaxed);
                return Ok(Some(Event::CancelAcknowledged {
                    before_generation: self.gen + 1,
                }));
            }
            if self.control.drain.load(Ordering::Relaxed) {
                let (generation, _) = self.active.take().unwrap();
                self.cancelled = false;
                return Ok(Some(Event::Cancelled { generation }));
            }
        } else if self.control.frame.load(Ordering::Relaxed) {
            if let Some((generation, request)) = self.active.take() {
                let mut bytes = b"FLOERAW1".to_vec();
                bytes.extend(request.width.to_le_bytes());
                bytes.extend(request.height.to_le_bytes());
                bytes.resize(
                    16 + request.width as usize * request.height as usize * 4,
                    255,
                );
                return Ok(Some(Event::Frame(Frame {
                    generation,
                    round: 1,
                    final_frame: true,
                    partial: false,
                    deferred: 0,
                    labels_truncated: false,
                    request,
                    bytes,
                    fields: Fields(BTreeMap::new()),
                })));
            }
        }
        Ok(None)
    }
    fn styles(&mut self, styles: &[Style]) -> Result<()> {
        assert_eq!(self.pending(), 0);
        self.control.styles.lock().unwrap().push(styles.to_vec());
        Ok(())
    }
    fn base(&self) -> RenderRequest {
        RenderRequest {
            format: FrameFormat::Raw,
            decode_jobs: 1,
            raster_jobs: 1,
            ..Default::default()
        }
    }
    fn close(&mut self) -> Result<()> {
        self.active = None;
        self.control.closed.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}
fn model(deck: bool) -> Arc<Model> {
    let styles = Arc::new(vec![Style {
        layer: (1, 0),
        color: [0, 255, 0, 255],
        fill: Fill::Solid,
        width: 1,
    }]);
    Arc::new(Model {
        dataset_revision: 7,
        dbu: 0.001,
        bbox: [0., 0., 800., 640.],
        deck,
        skipped: 0,
        styles,
        pairs: BTreeSet::from([(1, 0)]),
        groups: BTreeMap::new(),
    })
}
fn options() -> RenderOptions {
    RenderOptions {
        binary: "/not-used".into(),
        budget_mb: 1,
        decode_jobs: 1,
        raster_jobs: 1,
        tile_px: 32,
        round_pages: 1 << 30,
        open_timeout_s: 1,
        label_font_px: 14,
        raw: true,
    }
}
fn start(r: &Arc<Resources>, m: Arc<Model>, initial: ViewState, c: Arc<Control>) -> ViewController {
    ViewController::spawn(r, m, initial, r.render(&options()).unwrap(), move |stop| {
        while !c.open.load(Ordering::Relaxed) {
            crate::check_cancelled(&stop)?;
            thread::sleep(Duration::from_millis(1));
        }
        Ok((
            Box::new(Fake {
                control: c,
                active: None,
                gen: 0,
                cancelled: false,
                ack: false,
            }),
            None,
        ))
    })
    .unwrap()
}
fn wait(check: impl Fn() -> bool) {
    let until = Instant::now() + Duration::from_secs(3);
    while !check() {
        assert!(Instant::now() < until, "test deadline");
        thread::sleep(Duration::from_millis(1));
    }
}
fn pan() -> Patch {
    Patch {
        navigation: Some(Navigation::Pan {
            x: 0.2,
            y: 0.,
            snap: true,
        }),
        ..Default::default()
    }
}

#[test]
fn startup_is_one_transaction_without_a_hidden_fit_render() {
    let r = Resources::new(Limits::default()).unwrap();
    let m = model(false);
    let c = Arc::new(Control::default());
    let initial = ViewState::initial(&m, 80, 64)
        .unwrap()
        .edit(
            &m,
            Patch {
                navigation: Some(Navigation::Goto {
                    center_um: [13.6, 8.6],
                    width_um: 0.5,
                }),
                depth: Some(Depth::Levels(99)),
                detail: Some(Detail::High),
                labels: Some(false),
                thin: Some(Thin::Keep),
                ..Default::default()
            },
        )
        .unwrap();
    let expected = initial.clone();
    let mut v = start(&r, m, initial, Arc::clone(&c));
    wait(|| v.latest().is_some());
    let f = v.latest().unwrap();
    assert_eq!(f.state_rev, 1);
    assert_eq!(f.render_rev, 1);
    assert_eq!(f.frame.generation, 1);
    assert_eq!(f.frame.request.view, expected.viewport.bbox);
    assert_eq!(f.frame.request.depth, Some(99));
    assert_eq!(f.frame.request.cut_px, 1.);
    assert_eq!(c.requests.lock().unwrap().len(), 1);
    v.close().unwrap();
    assert_eq!(r.usage(), Usage::default());
    assert_eq!(c.closed.load(Ordering::Relaxed), 1);
}
#[test]
fn hundred_relative_inputs_are_kept_while_rendering_is_coalesced() {
    let r = Resources::new(Limits::default()).unwrap();
    let m = model(false);
    let c = Arc::new(Control::default());
    c.open.store(false, Ordering::Relaxed);
    let mut v = start(
        &r,
        Arc::clone(&m),
        ViewState::initial(&m, 80, 64).unwrap(),
        Arc::clone(&c),
    );
    for rev in 1..=100 {
        assert_eq!(v.edit(rev, pan()).unwrap().state_rev, rev + 1);
    }
    assert_eq!(v.snapshot().state.viewport.bbox, [16000., 0., 16800., 640.]);
    assert!(v.edit(1, pan()).is_err());
    assert_eq!(v.snapshot().state_rev, 101);
    assert!(c.requests.lock().unwrap().is_empty());
    c.open.store(true, Ordering::Relaxed);
    wait(|| v.latest().is_some());
    assert_eq!(v.latest().unwrap().state_rev, 101);
    assert_eq!(v.snapshot().submitted, 1);
    v.close().unwrap();
}
#[test]
fn cancel_ack_does_not_release_the_next_render_until_terminal_drain() {
    let r = Resources::new(Limits::default()).unwrap();
    let m = model(false);
    let c = Arc::new(Control::default());
    c.frame.store(false, Ordering::Relaxed);
    c.drain.store(false, Ordering::Relaxed);
    let mut v = start(
        &r,
        Arc::clone(&m),
        ViewState::initial(&m, 80, 64).unwrap(),
        Arc::clone(&c),
    );
    wait(|| v.snapshot().submitted == 1);
    for rev in 1..=100 {
        v.edit(rev, pan()).unwrap();
    }
    wait(|| c.acks.load(Ordering::Relaxed) == 1);
    thread::sleep(Duration::from_millis(40));
    assert_eq!(c.requests.lock().unwrap().len(), 1);
    assert_eq!(c.cancels.load(Ordering::Relaxed), 1);
    assert_eq!(v.snapshot().phase, Phase::Cancelling);
    assert!(v.latest().is_none());
    c.drain.store(true, Ordering::Relaxed);
    c.frame.store(true, Ordering::Relaxed);
    wait(|| v.latest().is_some());
    assert_eq!(v.latest().unwrap().state_rev, 101);
    assert_eq!(c.requests.lock().unwrap().len(), 2);
    v.close().unwrap();
    assert_eq!(r.usage(), Usage::default());
}
#[test]
fn worker_progress_and_latest_frame_memory_do_not_need_a_subscriber() {
    let r = Resources::new(Limits::default()).unwrap();
    let m = model(false);
    let c = Arc::new(Control::default());
    let mut v = start(
        &r,
        Arc::clone(&m),
        ViewState::initial(&m, 80, 64).unwrap(),
        c,
    );
    wait(|| v.snapshot().consumed == 1);
    let weak = Arc::downgrade(&v.latest().unwrap());
    for rev in 1..=20 {
        v.edit(rev, pan()).unwrap();
        wait(|| v.snapshot().consumed == rev + 1);
    }
    assert!(weak.upgrade().is_none());
    assert_eq!(v.latest().unwrap().state_rev, 21);
    v.close().unwrap();
    assert!(v.latest().is_none());
}
#[test]
fn restore_revision_policy_key_and_render_revision_are_separate() {
    let r = Resources::new(Limits::default()).unwrap();
    let m = model(false);
    let c = Arc::new(Control::default());
    let mut v = start(
        &r,
        Arc::clone(&m),
        ViewState::initial(&m, 80, 64).unwrap(),
        Arc::clone(&c),
    );
    wait(|| v.latest().is_some());
    let id = v.latest().unwrap().id;
    let s = v
        .edit(
            1,
            Patch {
                thin: Some(Thin::Cull),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!((s.state_rev, s.render_rev, s.render_key), (2, 1, 1));
    assert_eq!(v.latest().unwrap().id, id);
    let s = v
        .edit(
            2,
            Patch {
                thin: Some(Thin::Keep),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!((s.state_rev, s.render_rev, s.render_key), (3, 2, 2));
    wait(|| v.latest().is_some());
    let s = v.edit(3, pan()).unwrap();
    assert_eq!((s.render_rev, s.render_key), (3, 2));
    wait(|| v.latest().is_some());
    let bad = Style {
        width: 9,
        ..m.styles[0].clone()
    };
    assert!(v
        .edit(
            4,
            Patch {
                style_changes: vec![bad],
                ..Default::default()
            }
        )
        .is_err());
    assert_eq!(v.snapshot().state_rev, 4);
    let good = Style {
        width: 4,
        ..m.styles[0].clone()
    };
    let s = v
        .edit(
            4,
            Patch {
                style_changes: vec![good.clone()],
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!((s.render_rev, s.render_key), (4, 3));
    wait(|| v.latest().is_some());
    assert_eq!(c.styles.lock().unwrap().last().unwrap(), &vec![good]);
    v.close().unwrap();
}
#[test]
fn a_real_error_during_supersession_is_not_relabelled_as_cancelled() {
    let r = Resources::new(Limits::default()).unwrap();
    let m = model(false);
    let c = Arc::new(Control::default());
    c.frame.store(false, Ordering::Relaxed);
    c.drain.store(false, Ordering::Relaxed);
    let mut v = start(
        &r,
        Arc::clone(&m),
        ViewState::initial(&m, 80, 64).unwrap(),
        Arc::clone(&c),
    );
    wait(|| v.snapshot().submitted == 1);
    v.edit(1, pan()).unwrap();
    wait(|| c.acks.load(Ordering::Relaxed) > 0);
    c.fail.store(true, Ordering::Relaxed);
    wait(|| v.snapshot().phase == Phase::Failed);
    assert!(v.snapshot().failure.unwrap().1.contains("ENOSPC"));
    assert!(v.edit(2, pan()).is_err());
    assert!(v.latest().is_none());
    v.close().unwrap();
    assert_eq!(r.usage(), Usage::default());
    assert_eq!(c.closed.load(Ordering::Relaxed), 1);
}
#[test]
fn close_interrupts_open_and_drain_without_waiting_for_browser_credit() {
    for during_open in [true, false] {
        let r = Resources::new(Limits::default()).unwrap();
        let m = model(false);
        let c = Arc::new(Control::default());
        c.open.store(!during_open, Ordering::Relaxed);
        c.frame.store(false, Ordering::Relaxed);
        c.drain.store(false, Ordering::Relaxed);
        let mut v = start(
            &r,
            Arc::clone(&m),
            ViewState::initial(&m, 80, 64).unwrap(),
            Arc::clone(&c),
        );
        if !during_open {
            wait(|| v.snapshot().submitted == 1);
            v.edit(1, pan()).unwrap();
            wait(|| c.acks.load(Ordering::Relaxed) > 0);
        }
        let before = Instant::now();
        v.close().unwrap();
        assert!(before.elapsed() < Duration::from_secs(1));
        assert_eq!(v.snapshot().phase, Phase::Closed);
        assert_eq!(r.usage(), Usage::default());
    }
}

#[test]
fn a_cancelled_but_stuck_worker_still_has_a_deadline() {
    let r = Resources::new(Limits::default()).unwrap();
    let m = model(false);
    let c = Arc::new(Control::default());
    c.frame.store(false, Ordering::Relaxed);
    c.drain.store(false, Ordering::Relaxed);
    let mut v = start(
        &r,
        Arc::clone(&m),
        ViewState::initial(&m, 80, 64).unwrap(),
        Arc::clone(&c),
    );
    wait(|| v.snapshot().submitted == 1);
    v.edit(1, pan()).unwrap();
    wait(|| c.acks.load(Ordering::Relaxed) > 0);
    let deadline = Instant::now() + Duration::from_secs(8);
    while v.snapshot().phase != Phase::Failed {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    assert!(v
        .snapshot()
        .failure
        .unwrap()
        .1
        .contains("cancel drain deadline"));
    v.close().unwrap();
    assert_eq!(r.usage(), Usage::default());
    assert_eq!(c.closed.load(Ordering::Relaxed), 1);
}
#[test]
fn deck_groups_and_capabilities_are_validated_before_mutating_state() {
    let mut m = Arc::try_unwrap(model(true)).ok().unwrap();
    m.groups.insert((1, 0), vec![(1, 1), (1, 2)]);
    m.pairs.extend([(1, 1), (1, 2)]);
    m.styles = Arc::new(vec![
        m.styles[0].clone(),
        Style {
            layer: (1, 1),
            ..m.styles[0].clone()
        },
        Style {
            layer: (1, 2),
            ..m.styles[0].clone()
        },
    ]);
    let initial = ViewState::initial(&m, 80, 64).unwrap();
    assert!(!initial.labels);
    assert!(initial
        .edit(
            &m,
            Patch {
                labels: Some(true),
                ..Default::default()
            }
        )
        .is_err());
    assert!(initial
        .edit(
            &m,
            Patch {
                layers: Some(Layers::Only(vec![(9, 9)])),
                ..Default::default()
            }
        )
        .is_err());
    let s = initial
        .edit(
            &m,
            Patch {
                layers: Some(Layers::Only(vec![(1, 0)])),
                style_changes: vec![Style {
                    color: [255, 0, 0, 255],
                    ..m.styles[0].clone()
                }],
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(s.layers, Layers::Only(vec![(1, 1), (1, 2)]));
    assert!(s.styles.iter().all(|s| s.color == [255, 0, 0, 255]));
    assert!(Viewport::fit([0.; 4], 80, 64).is_err());
}
