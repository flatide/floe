//! Real native controller queries; the generator is shared with real_queries.
use floe_app_core::{
    jobdeck::color::Mode,
    managed::{Limits, ManagedDataset, Resources, Usage},
    render::{RenderOptions, RenderSession},
    shots::{Detail, Thin},
    view::*,
};
use floe_worker_client::{Event, Layers, QueryHit, QueryRequest, QueryStatus, SnapHit, SnapKind};
use std::{
    path::PathBuf,
    sync::{atomic::AtomicUsize, Arc},
    time::{Duration, Instant},
};

fn wait(mut ready: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(Instant::now() < end, "native controller deadline");
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn settled(v: &ViewController) -> Arc<DisplayFrame> {
    let mut out = None;
    wait(|| {
        let s = v.snapshot();
        assert_ne!(s.phase, Phase::Failed, "{:?}", s.failure);
        out = v
            .latest()
            .filter(|f| f.render_rev == s.render_rev && f.frame.complete());
        out.is_some()
    });
    out.unwrap()
}
fn request(
    v: &ViewController,
    frame: u64,
    world: [f64; 2],
    operation: QueryOperation,
    layers: Layers,
) -> ViewQuery {
    let b = v.snapshot().state.viewport.bbox;
    ViewQuery {
        anchor: v.query_anchor(frame).unwrap(),
        operation,
        position: [
            (world[0] - b[0]) / (b[2] - b[0]),
            (b[3] - world[1]) / (b[3] - b[1]),
        ],
        radius_px: 1.,
        layers,
    }
}
fn answer(v: &ViewController, q: ViewQuery) -> Arc<ViewQueryResult> {
    let kind = q.operation.kind();
    let id = v.query(q).unwrap();
    let mut out = None;
    wait(|| {
        let s = v.snapshot();
        assert_ne!(s.phase, Phase::Failed, "{:?}", s.failure);
        let q = v.query_snapshot();
        out = match kind {
            QueryKind::Snap => q.snap,
            QueryKind::Pick => q.pick,
        }
        .filter(|q| q.id == id);
        out.is_some()
    });
    out.unwrap()
}

#[test]
#[ignore = "run tools/validate_worker_queries.py"]
fn synchronous_capture_refuses_pending_queries_without_closing_the_worker() {
    let src = PathBuf::from(std::env::var_os("FLOE_QUERY_SOURCE").expect("private fixture"));
    let resources = Resources::new(Limits::default()).unwrap();
    let stop = Arc::new(AtomicUsize::new(0));
    let data = ManagedDataset::open(&resources, &src, None, Mode::Level, &stop).unwrap();
    let model = Model::new(&data).unwrap();
    let mut state = ViewState::initial(&model, 256, 256).unwrap();
    state.viewport = Viewport::new([-32000., -32000., 96000., 96000.], 256, 256).unwrap();
    state.detail = Detail::Exact;
    let mut options = RenderOptions::local().unwrap();
    options.decode_jobs = 2;
    options.raster_jobs = 2;
    options.budget_mb = 64;
    let mut session = RenderSession::open(&data.dataset, options, false, stop).unwrap();
    let request = state.request(&model, session.base_request());
    let frame = session.capture(request.clone()).unwrap();
    let pid = session.pid();
    let sequence = session
        .query(QueryRequest {
            scene: frame.query_scene().unwrap().id.unwrap(),
            operation: QueryOperation::Snap,
            x: 1,
            y: 1,
            radius: 10,
            layers: Layers::All,
        })
        .unwrap();
    assert_eq!(session.pending_queries(), 1);
    assert_eq!(
        session.capture(request.clone()).unwrap_err().kind,
        floe_app_core::ErrorKind::Busy
    );
    let frontier = session.cancel_queries(QueryKind::Snap).unwrap();
    assert!(frontier > sequence);
    assert_eq!(session.pending_queries(), 2);
    assert_eq!(
        session.capture(request.clone()).unwrap_err().kind,
        floe_app_core::ErrorKind::Busy
    );
    assert_eq!(session.pid(), pid);
    assert_eq!(session.pending_generations(), 0);
    let (mut reply, mut ack) = (false, false);
    wait(|| {
        match session.poll(Duration::from_millis(10)).unwrap() {
            Some(Event::Query(q)) => {
                assert_eq!(q.sequence, sequence);
                assert!(matches!(
                    q.status,
                    QueryStatus::Ok | QueryStatus::Superseded
                ));
                reply = true;
            }
            Some(Event::QueryCancelAcknowledged {
                kind,
                before_sequence,
            }) => {
                assert_eq!(kind, QueryKind::Snap);
                assert_eq!(before_sequence, frontier);
                ack = true;
            }
            None => (),
            event => panic!("unexpected capture/query event: {event:?}"),
        }
        session.pending_queries() == 0
    });
    assert!(reply && ack);
    assert!(session.capture(request).unwrap().complete());
    assert_eq!(session.pid(), pid);
    session.close().unwrap();
    drop(data);
    assert_eq!(resources.usage(), Usage::default());
    println!("RUST CAPTURE QUERY GUARD: ALL OK");
}

#[test]
#[ignore = "run tools/validate_worker_queries.py"]
fn native_queries_follow_display_margin_visibility_and_summary_without_rerender() {
    let src = PathBuf::from(std::env::var_os("FLOE_QUERY_SOURCE").expect("private fixture"));
    let resources = Resources::new(Limits::default()).unwrap();
    let stop = Arc::new(AtomicUsize::new(0));
    let data = ManagedDataset::open(&resources, &src, None, Mode::Level, &stop).unwrap();
    let model = Model::new(&data).unwrap();
    let mut initial = ViewState::initial(&model, 512, 512).unwrap();
    initial.viewport = Viewport::new([-32000., -32000., 96000., 96000.], 512, 512).unwrap();
    initial.detail = Detail::Exact;
    initial.labels = true;
    let mut options = RenderOptions::local().unwrap();
    options.decode_jobs = 2;
    options.raster_jobs = 2;
    options.budget_mb = 64;
    let mut v = ViewController::start_configured(
        &resources,
        Arc::clone(&data),
        options.clone(),
        initial.clone(),
        ControllerOptions {
            margin_prefetch: true,
            frame_cache: true,
        },
    )
    .unwrap();
    let f = settled(&v);
    wait(|| v.margin().is_some_and(|f| f.frame.complete()));
    let margin = v.margin().unwrap();
    let source = margin.frame.query_scene().unwrap().id.unwrap();
    assert_ne!(f.frame.query_scene().unwrap().id, Some(source));
    for nth in [0, 1, -1, 2, i64::MIN] {
        let q = answer(
            &v,
            request(
                &v,
                f.id,
                [5000., 5000.],
                QueryOperation::Pick { nth },
                Layers::All,
            ),
        );
        assert_eq!(q.anchor.frame_id, f.id);
        assert_eq!(q.reply.scene.id, Some(source));
        assert_eq!((q.reply.request.x, q.reply.request.y), (5000, 5000));
        let Some(QueryHit::Pick(hit)) = &q.reply.hit else {
            panic!("missing pick: {:?}", q.reply);
        };
        assert_eq!(hit.count, 2);
        assert_eq!(hit.layer, ((7 + nth.rem_euclid(2)) as u32, 0));
        assert_eq!(hit.cell_name, "TOP_QUERY");
        assert!(!hit.points_truncated);
    }
    let snap = answer(
        &v,
        request(&v, f.id, [1., 1.], QueryOperation::Snap, Layers::All),
    );
    assert_eq!(
        snap.reply.hit,
        Some(QueryHit::Snap(SnapHit {
            x: 0,
            y: 0,
            kind: SnapKind::Vertex
        }))
    );
    let stale = request(
        &v,
        f.id,
        [5000., 5000.],
        QueryOperation::Pick { nth: 0 },
        Layers::All,
    );
    v.edit(
        v.snapshot().state_rev,
        Patch {
            navigation: Some(Navigation::Pan {
                x: 0.1,
                y: 0.,
                snap: true,
            }),
            ..Default::default()
        },
    )
    .unwrap();
    wait(|| v.snapshot().crop_hits == 1);
    assert_eq!(v.snapshot().submitted, 2);
    assert!(v.query(stale.clone()).is_err());
    let cropped = answer(
        &v,
        request(
            &v,
            margin.id,
            [5000., 5000.],
            QueryOperation::Pick { nth: 0 },
            Layers::All,
        ),
    );
    assert_eq!(cropped.reply.scene.id, Some(source));
    assert!(matches!(&cropped.reply.hit,Some(QueryHit::Pick(hit)) if hit.layer==(7,0)));
    assert_eq!(
        v.snapshot().submitted,
        2,
        "query must not submit geometry renders"
    );
    for (labels, font_px) in [(false, 14), (true, 18)] {
        v.edit(
            v.snapshot().state_rev,
            Patch {
                labels: Some(labels),
                font_px: Some(font_px),
                ..Default::default()
            },
        )
        .unwrap();
        let frame = settled(&v);
        wait(|| v.margin().is_some_and(|m| m.frame.complete()));
        let q = answer(
            &v,
            request(
                &v,
                frame.id,
                [5000., 5000.],
                QueryOperation::Pick { nth: 0 },
                Layers::All,
            ),
        );
        assert_eq!(q.reply.status, QueryStatus::Ok);
        assert!(matches!(&q.reply.hit,Some(QueryHit::Pick(hit)) if hit.layer==(7,0)));
    }
    for pairs in [Layers::Only(vec![(8, 0)]), Layers::All] {
        v.edit(
            v.snapshot().state_rev,
            Patch {
                layers: Some(pairs.clone()),
                ..Default::default()
            },
        )
        .unwrap();
        let frame = settled(&v);
        // Prefetch may publish immediately after the foreground. Use the landed
        // margin as the source bound; the anchor remains the displayed foreground.
        wait(|| v.margin().is_some_and(|m| m.frame.complete()));
        let q = answer(
            &v,
            request(
                &v,
                frame.id,
                [5000., 5000.],
                QueryOperation::Pick { nth: 0 },
                Layers::All,
            ),
        );
        let Some(QueryHit::Pick(hit)) = &q.reply.hit else {
            panic!("missing visible layer");
        };
        assert_eq!(
            hit.layer,
            if pairs == Layers::All { (7, 0) } else { (8, 0) }
        );
        assert_ne!(q.reply.scene.id, Some(source));
        assert!(v.query(stale.clone()).is_err());
        if pairs != Layers::All {
            assert!(v
                .query(request(
                    &v,
                    frame.id,
                    [5000., 5000.],
                    QueryOperation::Snap,
                    Layers::Only(vec![(7, 0)])
                ))
                .is_err());
        }
    }
    let mut second =
        ViewController::start(&resources, Arc::clone(&data), options, initial).unwrap();
    settled(&second);
    let old = request(
        &v,
        v.latest().unwrap().id,
        [5000., 5000.],
        QueryOperation::Snap,
        Layers::All,
    );
    assert!(
        second.query(old).is_err(),
        "same data is not the same worker/controller"
    );
    second.close().unwrap();
    v.edit(
        v.snapshot().state_rev,
        Patch {
            navigation: Some(Navigation::Goto {
                center_um: [0., 0.],
                width_um: Some(2048.),
            }),
            pixels: Some((256, 256)),
            detail: Some(Detail::High),
            thin: Some(Thin::Keep),
            ..Default::default()
        },
    )
    .unwrap();
    let summary = settled(&v);
    wait(|| v.margin().is_some_and(|m| m.frame.complete()));
    assert_eq!(summary.frame.query_scene().unwrap().summary_layers, 1);
    let q = answer(
        &v,
        request(
            &v,
            summary.id,
            [5000., 5000.],
            QueryOperation::Pick { nth: 0 },
            Layers::All,
        ),
    );
    assert_eq!(q.reply.status, QueryStatus::Summary);
    assert!(q.reply.hit.is_none());
    let q = answer(
        &v,
        request(
            &v,
            summary.id,
            [5000., 5000.],
            QueryOperation::Pick { nth: 0 },
            Layers::Only(vec![(7, 0)]),
        ),
    );
    assert_eq!(q.reply.status, QueryStatus::Ok);
    assert!(matches!(&q.reply.hit,Some(QueryHit::Pick(hit)) if hit.layer==(7,0)));
    let q = answer(
        &v,
        request(
            &v,
            summary.id,
            [5000., 5000.],
            QueryOperation::Snap,
            Layers::None,
        ),
    );
    assert_eq!(q.reply.status, QueryStatus::Ok);
    assert!(q.reply.hit.is_none());
    v.request_close();
    assert!(v.query_snapshot().snap.is_none());
    assert!(v.query_snapshot().pick.is_none());
    v.close().unwrap();
    drop(data);
    assert_eq!(resources.usage(), Usage::default());
    println!("RUST VIEW QUERIES: ALL OK (foreground/margin scene, crop without render, overlap, visibility, worker isolation, mixed summary, close/leases)");
}
