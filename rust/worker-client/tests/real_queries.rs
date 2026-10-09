//! tools/validate_worker_queries.py supplies private generated geometry and a
//! KLayout area oracle. Missing fixtures are failures when explicitly run.
use floe_worker_client::*;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

fn frame(w: &mut WorkerClient, request: RenderRequest) -> Frame {
    let f = any_frame(w, request);
    assert!(f.complete());
    f
}
fn any_frame(w: &mut WorkerClient, request: RenderRequest) -> Frame {
    let gen = w.render(request).unwrap();
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(Instant::now() < end);
        match w.poll(Duration::from_millis(20)).unwrap() {
            Some(Event::Frame(f)) if f.final_frame => {
                assert_eq!(f.generation, gen);
                return f;
            }
            Some(Event::Failed { message, .. }) => panic!("{message}"),
            _ => (),
        }
    }
}
fn request(scene: SceneId, operation: QueryOperation) -> QueryRequest {
    QueryRequest {
        scene,
        operation,
        x: 1,
        y: 1,
        radius: 2,
        layers: Layers::All,
    }
}
fn query(w: &mut WorkerClient, req: QueryRequest) -> QueryReply {
    let seq = w.query(req).unwrap();
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        assert!(Instant::now() < end);
        match w.poll(Duration::from_millis(20)).unwrap() {
            Some(Event::Query(q)) => {
                assert_eq!(q.sequence, seq);
                return q;
            }
            Some(Event::Failed { message, .. }) => panic!("{message}"),
            _ => (),
        }
    }
}
#[test]
#[ignore = "run tools/validate_worker_queries.py"]
fn pinned_margin_reuse_visibility_overlaps_summary_and_concurrent_render() {
    let binary = PathBuf::from(std::env::var_os("FLOE_QUERY_RENDERD").expect("private binary"));
    let cache = PathBuf::from(std::env::var_os("FLOE_QUERY_CACHE").expect("private cache"));
    let mut w = WorkerClient::spawn(Config::new(binary)).unwrap();
    let work = w.work_dir().to_owned();
    w.open(Source::Layout(cache), 64, 2).unwrap();
    w.set_styles(&[7, 8].map(|n| Style {
        layer: (n, 0),
        color: [64, 192, 128, 255],
        fill: Fill::Solid,
        width: 1,
    }))
    .unwrap();
    let absent = query(
        &mut w,
        request(
            SceneId {
                generation: 1,
                round: 1,
            },
            QueryOperation::Snap,
        ),
    );
    assert_eq!(absent.status, QueryStatus::Unavailable);
    assert!(absent.hit.is_none());
    let margin = RenderRequest {
        view: [-32000., -32000., 96000., 96000.],
        width: 512,
        height: 512,
        raster_jobs: 2,
        decode_jobs: 2,
        format: FrameFormat::Raw,
        ..Default::default()
    };
    let outer = frame(&mut w, margin.clone());
    let context = outer.query_scene().unwrap();
    assert!(context.queryable());
    let id = context.id.unwrap();
    assert_eq!(
        id,
        SceneId {
            generation: outer.generation,
            round: outer.round
        }
    );
    let snap = query(&mut w, request(id, QueryOperation::Snap));
    assert_eq!(snap.status, QueryStatus::Ok);
    assert_eq!(
        snap.hit,
        Some(QueryHit::Snap(SnapHit {
            x: 0,
            y: 0,
            kind: SnapKind::Vertex
        }))
    );
    for nth in [0, 1, 2, -1, i64::MIN] {
        let mut req = request(id, QueryOperation::Pick { nth });
        req.x = 5000;
        req.y = 5000;
        let reply = query(&mut w, req);
        assert_eq!(reply.status, QueryStatus::Ok);
        let Some(QueryHit::Pick(p)) = reply.hit else {
            panic!("expected pick")
        };
        let index = nth.rem_euclid(2) as u64;
        assert_eq!(
            (p.count, p.index, p.layer),
            (2, index, ((7 + index) as u32, 0))
        );
        assert_eq!(p.cell_name, "TOP_QUERY");
        assert!(!p.points_truncated);
        assert_eq!(
            p.area,
            if index == 0 {
                100_000_000.
            } else {
                144_000_000.
            }
        );
        if index == 0 {
            assert_eq!(
                p.points,
                vec![(0, 0), (0, 10000), (10000, 10000), (10000, 0)]
            );
        }
    }
    let mut inner = RenderRequest {
        view: [0., 0., 64000., 64000.],
        width: 256,
        height: 256,
        labels: true,
        ..margin
    };
    let crop = frame(&mut w, inner.clone());
    assert!(
        crop.fields.u64("tiles_reused").unwrap() > 0 && crop.fields.u64("plan_pages").unwrap() == 0,
        "must exercise label-only reuse"
    );
    assert_ne!(crop.generation, id.generation);
    assert_eq!(crop.query_scene().unwrap().id, Some(id));
    assert_eq!(
        query(&mut w, request(id, QueryOperation::Snap)).status,
        QueryStatus::Ok
    );
    let wrong = query(
        &mut w,
        request(
            SceneId {
                generation: crop.generation,
                round: crop.round,
            },
            QueryOperation::Snap,
        ),
    );
    assert_eq!(wrong.status, QueryStatus::Mismatch);
    assert_eq!(wrong.scene.id, Some(id));
    let wrong_round = query(
        &mut w,
        request(
            SceneId {
                generation: id.generation,
                round: id.round + 1,
            },
            QueryOperation::Snap,
        ),
    );
    assert_eq!(wrong_round.status, QueryStatus::Mismatch);
    assert_eq!(wrong_round.scene.id, Some(id));
    inner.layers = Layers::Only(vec![(8, 0)]);
    let b = frame(&mut w, inner.clone());
    let bid = b.query_scene().unwrap().id.unwrap();
    assert_ne!(bid, id);
    let stale = query(&mut w, request(id, QueryOperation::Pick { nth: 0 }));
    assert_eq!(stale.status, QueryStatus::Mismatch);
    assert!(stale.hit.is_none());
    assert_eq!(stale.scene.id, Some(bid));
    let bpick = query(&mut w, request(bid, QueryOperation::Pick { nth: 0 }));
    assert!(matches!(
        bpick.hit,
        Some(QueryHit::Pick(PickHit {
            layer: (8, 0),
            count: 1,
            ..
        }))
    ));
    inner.layers = Layers::All;
    let a = frame(&mut w, inner.clone());
    let aid = a.query_scene().unwrap().id.unwrap();
    assert_ne!(aid, id);
    assert_ne!(aid, bid);
    assert_eq!(
        query(&mut w, request(bid, QueryOperation::Snap)).status,
        QueryStatus::Mismatch
    );
    let mut polygon = request(aid, QueryOperation::Pick { nth: 0 });
    polygon.x = 22000;
    polygon.y = 1000;
    let Some(QueryHit::Pick(poly)) = query(&mut w, polygon).hit else {
        panic!("large polygon missing")
    };
    assert!(poly.points_truncated);
    assert_eq!(poly.points.len(), 512);
    assert_eq!(
        poly.area,
        std::env::var("FLOE_QUERY_POLYGON_AREA")
            .unwrap()
            .parse::<f64>()
            .unwrap()
    );
    let mut empty = request(aid, QueryOperation::Pick { nth: 0 });
    empty.layers = Layers::None;
    let empty = query(&mut w, empty);
    assert_eq!(empty.status, QueryStatus::Ok);
    assert!(empty.hit.is_none());
    let mut sequences = std::collections::BTreeSet::new();
    for op in [
        QueryOperation::Snap,
        QueryOperation::Snap,
        QueryOperation::Pick { nth: 0 },
    ] {
        sequences.insert(w.query(request(aid, op)).unwrap());
    }
    inner.frame_cache = false;
    let gen = w.render(inner.clone()).unwrap();
    let mut rendered = false;
    let mut rendered_scene = None;
    let end = Instant::now() + Duration::from_secs(5);
    while !rendered || !sequences.is_empty() {
        assert!(Instant::now() < end);
        match w.poll(Duration::from_millis(20)).unwrap() {
            Some(Event::Frame(f)) => {
                assert_eq!(f.generation, gen);
                rendered |= f.final_frame;
                rendered_scene = f.query_scene().unwrap().id;
            }
            Some(Event::Query(q)) => {
                assert!(sequences.remove(&q.sequence));
                assert!(matches!(
                    q.status,
                    QueryStatus::Ok | QueryStatus::Superseded | QueryStatus::Mismatch
                ));
                if q.status == QueryStatus::Ok {
                    assert_eq!(q.scene.id, Some(aid));
                }
            }
            Some(Event::Failed { message, .. }) => panic!("{message}"),
            _ => (),
        }
    }
    assert_eq!(w.pending_queries(), 0);
    assert_eq!(w.pending_generations(), 0);
    let current = rendered_scene.unwrap();
    for kind in [QueryKind::Snap, QueryKind::Pick] {
        let (operation, other) = match kind {
            QueryKind::Snap => (QueryOperation::Snap, QueryOperation::Pick { nth: 0 }),
            QueryKind::Pick => (QueryOperation::Pick { nth: 0 }, QueryOperation::Snap),
        };
        let old = w.query(request(current, operation)).unwrap();
        let frontier = w.cancel_queries(kind).unwrap();
        let next = w.query(request(current, other)).unwrap();
        assert!(old < frontier && frontier < next);
        let mut replies = std::collections::BTreeSet::from([old, next]);
        let mut ack = false;
        let end = Instant::now() + Duration::from_secs(5);
        while !ack || !replies.is_empty() {
            assert!(Instant::now() < end);
            match w.poll(Duration::from_millis(20)).unwrap() {
                Some(Event::QueryCancelAcknowledged {
                    kind: k,
                    before_sequence,
                }) => {
                    assert_eq!((k, before_sequence), (kind, frontier));
                    ack = true;
                }
                Some(Event::Query(q)) => {
                    assert!(replies.remove(&q.sequence));
                    assert_eq!(q.scene.id, Some(current));
                    if q.sequence == next {
                        assert_eq!(q.status, QueryStatus::Ok);
                    } else {
                        assert!(matches!(
                            q.status,
                            QueryStatus::Ok | QueryStatus::Superseded
                        ));
                    }
                }
                Some(e) => panic!("query cancellation affected render: {e:?}"),
                None => (),
            }
        }
        assert_eq!(
            query(&mut w, request(current, operation)).status,
            QueryStatus::Ok
        );
        assert_eq!(w.pending_generations(), 0);
        assert_eq!(w.pending_queries(), 0);
    }
    let summary = frame(
        &mut w,
        RenderRequest {
            view: [-1024000., -1024000., 1024000., 1024000.],
            thin: ThinPolicy::Keep,
            // cut=0 explicitly disables occupancy even with exact=false.
            cut_px: 0.5,
            ..inner.clone()
        },
    );
    let c = summary.query_scene().unwrap();
    assert!(c.complete);
    assert!(c.summary_layers > 0, "{:?}", summary.fields);
    assert_eq!(
        c.summary_layers, 1,
        "fixture must mix exact and summarized layers"
    );
    assert!(!c.queryable());
    for op in [QueryOperation::Snap, QueryOperation::Pick { nth: 0 }] {
        let q = query(&mut w, request(c.id.unwrap(), op));
        assert_eq!(q.status, QueryStatus::Summary);
        assert!(q.hit.is_none());
        assert!(q.error.is_some());
    }
    let mut exact_layer = request(c.id.unwrap(), QueryOperation::Pick { nth: 0 });
    exact_layer.layers = Layers::Only(vec![(7, 0)]);
    let q = query(&mut w, exact_layer);
    assert_eq!(q.status, QueryStatus::Ok);
    assert_eq!(q.summary_layers, 0);
    assert!(matches!(
        q.hit,
        Some(QueryHit::Pick(PickHit { layer: (7, 0), .. }))
    ));
    let partial = any_frame(
        &mut w,
        RenderRequest {
            decode_pages: Some(0),
            ..inner
        },
    );
    assert!(!partial.complete());
    assert!(partial.partial);
    assert!(partial.deferred > 0);
    let c = partial.query_scene().unwrap();
    assert!(!c.complete);
    for op in [QueryOperation::Snap, QueryOperation::Pick { nth: 0 }] {
        let q = query(&mut w, request(c.id.unwrap(), op));
        assert_eq!(q.status, QueryStatus::Incomplete);
        assert!(q.hit.is_none());
    }
    w.close().unwrap();
    assert!(!work.exists());
    println!("RUST PINNED QUERIES: ALL OK (overlap/KLayout area, margin identity, visibility, summary, partial, concurrent render)");
}
