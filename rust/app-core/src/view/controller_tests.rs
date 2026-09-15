use super::*;
use crate::{
    managed::{Limits, Usage},
    shots::{Detail, Thin},
    view::{Depth, LayerIsolation, Navigation, Viewport},
};
use floe_worker_client::{Fields, Fill, FrameFormat, Layers, QueryScene, QueryStatus};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::atomic::AtomicBool,
};
#[path = "query_controller_tests.rs"]
mod query_tests;

struct Control {
    opened: AtomicUsize,
    close_release: AtomicBool,
    closing: AtomicUsize,
    open: AtomicBool,
    frame: AtomicBool,
    drain: AtomicBool,
    fail: AtomicBool,
    margin_frame: AtomicBool,
    margin_truncated: AtomicBool,
    margin_fail: AtomicBool,
    requests: Mutex<Vec<RenderRequest>>,
    styles: Mutex<Vec<Vec<Style>>>,
    cancels: AtomicUsize,
    acks: AtomicUsize,
    closed: AtomicUsize,
    query_reply: AtomicBool,
    query_ack: AtomicBool,
    query_busy: AtomicBool,
    query_failed: AtomicBool,
    geometry_partial: AtomicBool,
    query_requests: Mutex<Vec<(u64, QueryRequest)>>,
    query_cancels: Mutex<Vec<QueryKind>>,
}
impl Default for Control {
    fn default() -> Self {
        Self {
            opened: AtomicUsize::new(0),
            close_release: AtomicBool::new(true),
            closing: AtomicUsize::new(0),
            open: AtomicBool::new(true),
            frame: AtomicBool::new(true),
            drain: AtomicBool::new(true),
            fail: AtomicBool::new(false),
            margin_frame: AtomicBool::new(true),
            margin_truncated: AtomicBool::new(false),
            margin_fail: AtomicBool::new(false),
            requests: Mutex::new(Vec::new()),
            styles: Mutex::new(Vec::new()),
            cancels: AtomicUsize::new(0),
            acks: AtomicUsize::new(0),
            closed: AtomicUsize::new(0),
            query_reply: AtomicBool::new(true),
            query_ack: AtomicBool::new(true),
            query_busy: AtomicBool::new(false),
            query_failed: AtomicBool::new(false),
            geometry_partial: AtomicBool::new(false),
            query_requests: Mutex::new(Vec::new()),
            query_cancels: Mutex::new(Vec::new()),
        }
    }
}
struct Fake {
    control: Arc<Control>,
    active: Option<(u64, RenderRequest)>,
    gen: u64,
    cancelled: bool,
    ack: bool,
    deck: bool,
    query_sequence: u64,
    queries: BTreeMap<u64, QueryRequest>,
    query_frontiers: BTreeMap<QueryKind, u64>,
    query_acks: VecDeque<(QueryKind, u64)>,
}
impl Engine for Fake {
    fn query(&mut self, r: QueryRequest) -> Result<u64> {
        assert!(!self.deck);
        if self.control.query_busy.load(Ordering::Relaxed) {
            return Err(Error::new(ErrorKind::Busy, "query busy"));
        }
        self.query_sequence += 1;
        self.query_frontiers
            .insert(r.operation.kind(), self.query_sequence);
        self.control
            .query_requests
            .lock()
            .unwrap()
            .push((self.query_sequence, r.clone()));
        self.queries.insert(self.query_sequence, r);
        Ok(self.query_sequence)
    }
    fn cancel_queries(&mut self, kind: QueryKind) -> Result<u64> {
        if self.query_acks.iter().any(|(k, _)| *k == kind) {
            return Err(Error::new(ErrorKind::Busy, "query cancel busy"));
        }
        self.query_sequence += 1;
        self.query_frontiers.insert(kind, self.query_sequence);
        self.query_acks.push_back((kind, self.query_sequence));
        self.control.query_cancels.lock().unwrap().push(kind);
        Ok(self.query_sequence)
    }
    fn pending_queries(&self) -> usize {
        self.queries.len() + self.query_acks.len()
    }
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
        if self.control.query_failed.load(Ordering::Relaxed) && !self.queries.is_empty() {
            return Err(Error::new(ErrorKind::Worker, "query deadline test"));
        }
        if self.control.query_ack.load(Ordering::Relaxed) {
            if let Some((kind, before_sequence)) = self.query_acks.pop_front() {
                return Ok(Some(Event::QueryCancelAcknowledged {
                    kind,
                    before_sequence,
                }));
            }
        }
        if self.control.query_reply.load(Ordering::Relaxed) {
            if let Some((seq, request)) = self.queries.pop_first() {
                let superseded = self.query_frontiers[&request.operation.kind()] > seq;
                return Ok(Some(Event::Query(QueryReply {
                    sequence: seq,
                    scene: QueryScene {
                        id: Some(request.scene),
                        complete: true,
                        summary_layers: 0,
                    },
                    request,
                    status: if superseded {
                        QueryStatus::Superseded
                    } else {
                        QueryStatus::Ok
                    },
                    summary_layers: 0,
                    hit: None,
                    error: superseded.then(|| "superseded".into()),
                })));
            }
        }
        if (self.control.fail.load(Ordering::Relaxed)
            || (self.gen == 2 && self.control.margin_fail.load(Ordering::Relaxed)))
            && self.active.is_some()
        {
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
        } else if self.control.frame.load(Ordering::Relaxed)
            && (self.gen != 2 || self.control.margin_frame.load(Ordering::Relaxed))
        {
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
                    partial: self.control.geometry_partial.load(Ordering::Relaxed),
                    deferred: 0,
                    labels_truncated: self.gen == 2
                        && self.control.margin_truncated.load(Ordering::Relaxed),
                    request,
                    bytes,
                    fields: Fields(
                        [
                            (
                                "scene_gen".into(),
                                if self.deck { 0 } else { generation }.to_string(),
                            ),
                            (
                                "scene_round".into(),
                                if self.deck { "0" } else { "1" }.into(),
                            ),
                            (
                                "scene_complete".into(),
                                if self.deck
                                    || self.control.geometry_partial.load(Ordering::Relaxed)
                                {
                                    "0"
                                } else {
                                    "1"
                                }
                                .into(),
                            ),
                            ("scene_summary".into(), "0".into()),
                            (
                                "style_epoch".into(),
                                (1 + self.control.styles.lock().unwrap().len()).to_string(),
                            ),
                        ]
                        .into(),
                    ),
                })));
            }
        }
        Ok(None)
    }
    fn styles(&mut self, styles: &[Style]) -> Result<()> {
        assert_eq!(self.pending(), 0);
        assert_eq!(
            self.pending_queries(),
            0,
            "style ACK would swallow query results"
        );
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
        self.control.closing.fetch_add(1, Ordering::Relaxed);
        while !self.control.close_release.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(1));
        }
        self.active = None;
        self.queries.clear();
        self.query_acks.clear();
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
        minimap: Arc::default(),
        initial_layers: Layers::All,
        assignments: Arc::default(),
        property_names: Vec::new(),
        dataset_revision: 7,
        dbu: 0.001,
        bbox: [0., 0., 800., 640.],
        deck,
        skipped: 0,
        source_stale: false,
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
fn fake(c: Arc<Control>, deck: bool) -> Box<dyn Engine> {
    c.opened.fetch_add(1, Ordering::Relaxed);
    Box::new(Fake {
        control: c,
        active: None,
        gen: 0,
        cancelled: false,
        ack: false,
        deck,
        query_sequence: 0,
        queries: BTreeMap::new(),
        query_frontiers: BTreeMap::new(),
        query_acks: VecDeque::new(),
    })
}
fn start(r: &Arc<Resources>, m: Arc<Model>, initial: ViewState, c: Arc<Control>) -> ViewController {
    start_configured(r, m, initial, c, ControllerOptions::default())
}
fn start_configured(
    r: &Arc<Resources>,
    m: Arc<Model>,
    initial: ViewState,
    c: Arc<Control>,
    configuration: ControllerOptions,
) -> ViewController {
    let deck = m.deck;
    ViewController::spawn(
        r,
        m,
        initial,
        r.render(&options()).unwrap(),
        configuration,
        move |stop| {
            while !c.open.load(Ordering::Relaxed) {
                crate::check_cancelled(&stop)?;
                thread::sleep(Duration::from_millis(1));
            }
            Ok((fake(c, deck), None))
        },
    )
    .unwrap()
}
fn wait(mut check: impl FnMut() -> bool) {
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

fn one_worker() -> (
    Arc<Resources>,
    Arc<Model>,
    Arc<Control>,
    Arc<ViewController>,
) {
    let r = Resources::new(Limits {
        cpu_slots: 2,
        foreground_reserve: 0,
        workers: 1,
        decoded_mb: 1,
    })
    .unwrap();
    let m = model(true);
    let c = Arc::new(Control::default());
    let v = Arc::new(start(
        &r,
        Arc::clone(&m),
        ViewState::initial(&m, 80, 64).unwrap(),
        Arc::clone(&c),
    ));
    wait(|| v.latest().is_some());
    (r, m, c, v)
}
struct ReleaseClose(Arc<Control>);
impl Drop for ReleaseClose {
    fn drop(&mut self) {
        self.0.close_release.store(true, Ordering::Relaxed);
    }
}

#[test]
fn replacement_prepare_drop_stale_revision_and_exclusive_slot_preserve_current() {
    let (r, m, c, v) = one_worker();
    let usage = r.usage();
    let original = v.snapshot();
    let new = Arc::new(Control::default());
    let n = Arc::clone(&new);
    let mut p = v
        .prepare_engine(Arc::clone(&m), original.state.clone(), move |_| {
            Ok((fake(n, true), None))
        })
        .unwrap();
    let next = p.controller();
    assert_eq!(r.usage(), usage);
    assert!(r.render(&options()).is_err());
    assert!(v
        .prepare_engine(Arc::clone(&m), original.state.clone(), |_| panic!(
            "duplicate open"
        ))
        .is_err());
    thread::sleep(Duration::from_millis(10));
    assert_eq!(new.opened.load(Ordering::Relaxed), 0);
    assert_eq!(v.snapshot().state, original.state);
    assert_eq!(c.closed.load(Ordering::Relaxed), 0);
    v.edit(original.state_rev, pan()).unwrap();
    assert!(p.commit(original.state_rev).is_err());
    drop(p);
    wait(|| next.is_finished());
    assert_eq!(r.usage(), usage);
    assert_eq!(new.opened.load(Ordering::Relaxed), 0);
    assert_eq!(c.closed.load(Ordering::Relaxed), 0);
    let p = v
        .prepare_engine(Arc::clone(&m), v.snapshot().state, |_| {
            panic!("aborted open")
        })
        .unwrap();
    let aborted = p.controller();
    drop(p);
    wait(|| aborted.is_finished());
    v.request_close();
    wait(|| v.is_finished());
    assert_eq!(r.usage(), Usage::default());
}

#[test]
fn replacement_cutover_waits_for_reap_and_never_borrows_another_reservation() {
    let (r, m, c, v) = one_worker();
    let usage = r.usage();
    let original = v.snapshot();
    let new = Arc::new(Control::default());
    let n = Arc::clone(&new);
    let old = Arc::clone(&c);
    let mut p = v
        .prepare_engine(Arc::clone(&m), original.state.clone(), move |_| {
            assert_eq!(
                old.closed.load(Ordering::Relaxed),
                1,
                "new worker overlapped old close"
            );
            Ok((fake(n, true), None))
        })
        .unwrap();
    let next = p.controller();
    let _release = ReleaseClose(Arc::clone(&c));
    c.close_release.store(false, Ordering::Relaxed);
    p.commit(original.state_rev).unwrap();
    assert!(p.commit(original.state_rev).is_err());
    wait(|| c.closing.load(Ordering::Relaxed) == 1);
    assert_eq!(new.opened.load(Ordering::Relaxed), 0);
    assert_eq!(r.usage(), usage);
    assert!(r.render(&options()).is_err());
    assert!(v.edit(original.state_rev, pan()).is_err());
    assert!(next
        .prepare_engine(Arc::clone(&m), next.snapshot().state, |_| panic!(
            "opening chain"
        ))
        .is_err());
    c.close_release.store(true, Ordering::Relaxed);
    wait(|| next.latest().is_some());
    assert!(v.is_finished());
    assert_ne!(next.snapshot().worker_epoch, original.worker_epoch);
    assert_eq!(next.snapshot().state, original.state);
    assert_eq!(new.opened.load(Ordering::Relaxed), 1);
    assert_eq!(r.usage(), usage);
    drop(p);
    next.request_close();
    wait(|| next.is_finished());
    assert_eq!(
        r.usage(),
        Usage::default(),
        "retained controller handles must not retain reservations"
    );
}

#[test]
fn replacement_shutdown_waits_for_its_predecessor_without_opening_an_engine() {
    let (r, m, c, v) = one_worker();
    let rev = v.snapshot().state_rev;
    let mut p = v
        .prepare_engine(m, v.snapshot().state, |_| {
            panic!("cancelled replacement opened")
        })
        .unwrap();
    let next = p.controller();
    let _release = ReleaseClose(Arc::clone(&c));
    c.close_release.store(false, Ordering::Relaxed);
    p.commit(rev).unwrap();
    wait(|| c.closing.load(Ordering::Relaxed) == 1);
    next.request_close();
    thread::sleep(Duration::from_millis(10));
    assert!(!next.is_finished());
    assert_eq!(r.usage().workers, 1);
    c.close_release.store(true, Ordering::Relaxed);
    wait(|| next.is_finished());
    assert_eq!(r.usage(), Usage::default());
    assert_eq!(next.snapshot().phase, Phase::Closed);
}

#[test]
fn replacement_open_failure_is_explicit_and_releases_the_transferred_quota() {
    let (r, m, c, v) = one_worker();
    let rev = v.snapshot().state_rev;
    let mut p = v
        .prepare_engine(m, v.snapshot().state, |_| {
            Err(Error::new(ErrorKind::Worker, "replacement open failed"))
        })
        .unwrap();
    let next = p.controller();
    p.commit(rev).unwrap();
    wait(|| next.is_finished());
    assert!(v.is_finished());
    assert_eq!(c.closed.load(Ordering::Relaxed), 1);
    assert_eq!(next.snapshot().phase, Phase::Failed);
    assert_eq!(r.usage(), Usage::default());
}

#[test]
fn relative_depth_is_clamped_under_revision_lock() {
    let r = Resources::new(Limits::default()).unwrap();
    let m = model(false);
    let c = Arc::new(Control::default());
    c.open.store(false, Ordering::Relaxed);
    let initial = ViewState::initial(&m, 80, 64).unwrap();
    assert!(initial
        .edit(
            &m,
            Patch {
                depth: Some(Depth::Step(1)),
                ..Default::default()
            }
        )
        .is_err());
    let mut v = start(&r, m, initial, c);
    v.shared.lock().unwrap().snapshot.max_depth = Some(2);
    for (delta, expected) in [(-1, 1), (1, 2), (1, 2), (-1, 1), (-1, 0), (-1, 0)] {
        let before = v.snapshot();
        let after = v
            .edit(
                before.state_rev,
                Patch {
                    depth: Some(Depth::Step(delta)),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(after.state.depth, Some(expected));
        assert_eq!(after.state.viewport, before.state.viewport);
        assert_eq!(
            after.state_rev == before.state_rev,
            before.state.depth == Some(expected)
        );
    }
    let before = v.snapshot();
    assert!(v
        .edit(
            1,
            Patch {
                depth: Some(Depth::Step(1)),
                ..Default::default()
            }
        )
        .is_err());
    for delta in [0, 2, -2, i8::MIN, i8::MAX] {
        assert!(v
            .edit(
                before.state_rev,
                Patch {
                    depth: Some(Depth::Step(delta)),
                    ..Default::default()
                }
            )
            .is_err());
    }
    assert_eq!(v.snapshot().state_rev, before.state_rev);
    v.close().unwrap();
}

#[test]
#[ignore = "run tools/validate_depth_keys.py against the actual GTK depth step"]
fn gtk_depth_step_oracle() {
    let path = std::env::var_os("FLOE_DEPTH_CASES").unwrap();
    let cases: Vec<serde_json::Value> =
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert!(cases.len() >= 100);
    for case in &cases {
        let got = Depth::stepped(
            case["current"].as_u64().map(|n| n as u32),
            case["maximum"].as_u64(),
            case["delta"].as_i64().unwrap() as i8,
        )
        .unwrap();
        let got = match got {
            Depth::Full => None,
            Depth::Levels(n) => Some(n),
            Depth::Step(_) => unreachable!(),
        };
        assert_eq!(got.map(u64::from), case["want"].as_u64(), "{case}");
    }
    println!("GTK DEPTH KEYS: ALL OK ({} cases)", cases.len());
}

#[test]
fn isolation_and_goto_are_atomic_and_restore_only_the_first_visibility() {
    let r = Resources::new(Limits::default()).unwrap();
    let mut m = model(false);
    let model = Arc::get_mut(&mut m).unwrap();
    let other = model.styles[0].clone();
    Arc::make_mut(&mut model.styles).push(Style {
        layer: (2, 0),
        ..other
    });
    model.pairs.insert((2, 0));
    let c = Arc::new(Control::default());
    c.open.store(false, Ordering::Relaxed);
    let mut initial = ViewState::initial(&m, 80, 64).unwrap();
    initial.layers = Layers::None;
    let mut v = start(&r, m, initial, Arc::clone(&c));
    let patch = Patch {
        navigation: Some(Navigation::Goto {
            center_um: [8., 9.],
            width_um: Some(2.),
        }),
        layer_isolation: Some(LayerIsolation::Set(Layers::Only(vec![(2, 0)]))),
        ..Default::default()
    };
    for isolation in [
        Layers::None,
        Layers::Only(vec![]),
        Layers::Only(vec![(9, 9)]),
        Layers::Only(vec![(1, 0); 4097]),
    ] {
        assert!(v
            .edit(
                1,
                Patch {
                    layer_isolation: Some(LayerIsolation::Set(isolation)),
                    ..patch.clone()
                }
            )
            .is_err());
        assert_eq!(v.snapshot().state_rev, 1);
        assert!(!v.snapshot().state.layers_isolated());
    }
    assert!(v
        .edit(
            1,
            Patch {
                layers: Some(Layers::All),
                ..patch.clone()
            }
        )
        .is_err());
    assert!(v
        .edit(
            1,
            Patch {
                layer_change: Some(((1, 0), true)),
                ..patch.clone()
            }
        )
        .is_err());
    let first = v.edit(1, patch).unwrap();
    assert_eq!(
        (first.state_rev, first.render_rev, first.render_key),
        (2, 2, 2)
    );
    assert_eq!(first.state.layers, Layers::Only(vec![(2, 0)]));
    assert_eq!(first.state.viewport.bbox, [7000., 8200., 9000., 9800.]);
    let saved = first.state.isolated_from.clone().unwrap();
    assert_eq!(*saved, Layers::None);
    let second = v
        .edit(
            2,
            Patch {
                layer_isolation: Some(LayerIsolation::Set(Layers::Only(vec![(1, 0)]))),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(Arc::ptr_eq(
        &saved,
        second.state.isolated_from.as_ref().unwrap()
    ));
    v.edit(
        3,
        Patch {
            layer_change: Some(((1, 0), false)),
            detail: Some(Detail::High),
            depth: Some(Depth::Levels(4)),
            ..Default::default()
        },
    )
    .unwrap();
    let restore = Patch {
        layer_isolation: Some(LayerIsolation::Restore),
        ..Default::default()
    };
    assert_eq!(
        v.edit(1, restore.clone()).unwrap_err().kind,
        ErrorKind::Busy
    );
    let before = v.snapshot();
    let restored = v.edit(4, restore.clone()).unwrap();
    assert_eq!(restored.state_rev, 5);
    assert_eq!(
        restored.render_rev, before.render_rev,
        "same visible layers need no new raster"
    );
    assert_eq!(restored.render_key, before.render_key);
    assert_eq!(restored.state.layers, Layers::None);
    assert!(!restored.state.layers_isolated());
    assert_eq!(restored.state.viewport, first.state.viewport);
    assert_eq!(restored.state.detail, Detail::High);
    assert_eq!(restored.state.depth, Some(4));
    assert_eq!(v.edit(5, restore.clone()).unwrap().state_rev, 5);
    assert!(c.requests.lock().unwrap().is_empty());
    c.open.store(true, Ordering::Relaxed);
    wait(|| v.latest().is_some());
    assert_eq!(v.snapshot().submitted, 1);
    v.close().unwrap();
    assert!(v.edit(5, restore).is_err());
    assert!(!ViewState::initial(&v.model, 80, 64)
        .unwrap()
        .layers_isolated());
}

#[test]
fn isolation_bookkeeping_does_not_invalidate_an_unchanged_frame() {
    let r = Resources::new(Limits::default()).unwrap();
    let m = model(false);
    let mut v = start(
        &r,
        Arc::clone(&m),
        ViewState::initial(&m, 80, 64).unwrap(),
        Arc::new(Control::default()),
    );
    wait(|| v.latest().is_some());
    let frame = v.latest().unwrap();
    let isolated = v
        .edit(
            1,
            Patch {
                layer_isolation: Some(LayerIsolation::Set(Layers::All)),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        (isolated.state_rev, isolated.render_rev, isolated.render_key),
        (2, 1, 1)
    );
    assert!(isolated.state.layers_isolated());
    let restored = v
        .edit(
            2,
            Patch {
                layer_isolation: Some(LayerIsolation::Restore),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        (restored.state_rev, restored.render_rev, restored.render_key),
        (3, 1, 1)
    );
    assert_eq!(v.latest().unwrap().id, frame.id);
    assert_eq!(v.snapshot().submitted, 1);
    v.close().unwrap();
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
                    width_um: Some(0.5),
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
    let mut initial = ViewState::initial(&m, 80, 64).unwrap();
    initial.viewport = Viewport::new(m.bbox, 80, 64).unwrap();
    let mut v = start(&r, Arc::clone(&m), initial, Arc::clone(&c));
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

fn margin_options() -> ControllerOptions {
    ControllerOptions {
        margin_prefetch: true,
        frame_cache: true,
    }
}
#[test]
fn complete_margin_crops_pan_without_another_foreground_and_invalidates_policy() {
    let r = Resources::new(Limits::default()).unwrap();
    let m = model(false);
    let c = Arc::new(Control::default());
    let initial = ViewState::initial(&m, 800, 640).unwrap();
    let mut v = start_configured(&r, m, initial, Arc::clone(&c), margin_options());
    wait(|| v.margin().is_some());
    let margin = v.margin().unwrap();
    assert_eq!(margin.purpose, Purpose::Margin);
    assert_eq!(margin.frame.generation, 2);
    assert!(margin.frame.request.labels);
    assert_eq!(
        (margin.frame.request.width, margin.frame.request.height),
        (1600, 1280)
    );
    let mut p = pan();
    p.navigation = Some(Navigation::Pan {
        x: 0.1,
        y: 0.,
        snap: true,
    });
    let accepted = v.edit(1, p).unwrap();
    assert!(accepted.margin.unwrap().crop_safe);
    wait(|| v.snapshot().crop_hits == 1);
    thread::sleep(Duration::from_millis(30));
    assert_eq!(c.requests.lock().unwrap().len(), 2);
    assert!(v.latest().is_none());
    assert!(margin.matches(&v.snapshot()));
    let changed = v
        .edit(
            2,
            Patch {
                thin: Some(Thin::Keep),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(changed.margin.is_none());
    assert!(!margin.matches(&changed));
    v.close().unwrap();
    assert_eq!(r.usage(), Usage::default());
}
#[test]
fn margin_cancel_ack_is_not_terminal_and_never_delays_new_foreground() {
    let r = Resources::new(Limits::default()).unwrap();
    let m = model(false);
    let c = Arc::new(Control::default());
    c.margin_frame.store(false, Ordering::Relaxed);
    c.drain.store(false, Ordering::Relaxed);
    let mut v = start_configured(
        &r,
        Arc::clone(&m),
        ViewState::initial(&m, 80, 64).unwrap(),
        Arc::clone(&c),
        margin_options(),
    );
    wait(|| c.requests.lock().unwrap().len() == 2);
    assert_eq!(v.latest().unwrap().frame.generation, 1);
    v.edit(1, pan()).unwrap();
    wait(|| c.acks.load(Ordering::Relaxed) == 1);
    assert_eq!(c.requests.lock().unwrap().len(), 2);
    c.drain.store(true, Ordering::Relaxed);
    wait(|| v.latest().is_some_and(|f| f.render_rev == 2));
    assert_eq!(c.cancels.load(Ordering::Relaxed), 1);
    assert_eq!(c.requests.lock().unwrap()[2].width, 80);
    v.close().unwrap();
}
#[test]
fn truncated_or_failed_margin_never_claims_complete_or_retries_in_a_loop() {
    for fail in [false, true] {
        let r = Resources::new(Limits::default()).unwrap();
        let m = model(false);
        let c = Arc::new(Control::default());
        c.margin_truncated.store(!fail, Ordering::Relaxed);
        c.margin_fail.store(fail, Ordering::Relaxed);
        let mut v = start_configured(
            &r,
            Arc::clone(&m),
            ViewState::initial(&m, 800, 640).unwrap(),
            Arc::clone(&c),
            margin_options(),
        );
        wait(|| {
            let s = v.snapshot();
            s.margin_submitted == 1 && !s.margin_working
        });
        let s = v.snapshot();
        assert_eq!(s.phase, Phase::Idle);
        assert_eq!(v.latest().unwrap().frame.generation, 1);
        if fail {
            assert!(s.margin_failure.is_some());
            assert!(s.margin.is_none());
        } else {
            assert!(!s.margin.unwrap().crop_safe);
        }
        thread::sleep(Duration::from_millis(30));
        assert_eq!(c.requests.lock().unwrap().len(), 2);
        let p = Patch {
            navigation: Some(Navigation::Pan {
                x: 0.1,
                y: 0.,
                snap: true,
            }),
            ..Default::default()
        };
        v.edit(1, p).unwrap();
        wait(|| v.latest().is_some_and(|f| f.render_rev == 2));
        assert_eq!(v.snapshot().crop_hits, 0);
        assert_eq!(c.requests.lock().unwrap().len(), 3);
        v.close().unwrap();
    }
}
#[test]
fn deck_or_frame_cache_off_never_prefetches() {
    for (deck, cache) in [(true, true), (false, false)] {
        let r = Resources::new(Limits::default()).unwrap();
        let m = model(deck);
        let c = Arc::new(Control::default());
        let mut v = start_configured(
            &r,
            Arc::clone(&m),
            ViewState::initial(&m, 80, 64).unwrap(),
            Arc::clone(&c),
            ControllerOptions {
                margin_prefetch: true,
                frame_cache: cache,
            },
        );
        wait(|| v.latest().is_some());
        thread::sleep(Duration::from_millis(30));
        assert!(!v.snapshot().margin_enabled);
        assert_eq!(c.requests.lock().unwrap().len(), 1);
        assert_eq!(c.requests.lock().unwrap()[0].frame_cache, cache);
        v.close().unwrap();
    }
}
