use super::*;

fn setup(margin: bool) -> (Arc<Resources>, Arc<Control>, ViewController) {
    let r = Resources::new(Limits::default()).unwrap();
    let m = model(false);
    let mut initial = ViewState::initial(&m, 800, 640).unwrap();
    initial.labels = false;
    let c = Arc::new(Control::default());
    let v = start_configured(
        &r,
        m,
        initial,
        Arc::clone(&c),
        ControllerOptions {
            margin_prefetch: margin,
            frame_cache: true,
        },
    );
    wait(|| v.latest().is_some());
    (r, c, v)
}
fn input(v: &ViewController, operation: super::super::super::QueryOperation) -> ViewQuery {
    ViewQuery {
        anchor: v.query_anchor(v.latest().unwrap().id).unwrap(),
        operation,
        position: [0.5, 0.5],
        radius_px: 10.,
        layers: Layers::All,
    }
}
fn result(v: &ViewController, id: u64, kind: QueryKind) -> Arc<ViewQueryResult> {
    let mut out = None;
    wait(|| {
        let q = v.query_snapshot();
        out = match kind {
            QueryKind::Snap => q.snap,
            QueryKind::Pick => q.pick,
        }
        .filter(|r| r.id == id);
        out.is_some()
    });
    out.unwrap()
}

#[test]
fn identities_geometry_and_coordinates_are_checked_before_enqueue() {
    use floe_worker_client::QueryOperation;
    let (r, c, mut v) = setup(false);
    let valid = input(&v, QueryOperation::Snap);
    for field in 0..6 {
        let mut bad = valid.clone();
        match field {
            0 => bad.anchor.dataset_revision += 1,
            1 => bad.anchor.worker_epoch += 1,
            2 => bad.anchor.frame_id += 1,
            3 => bad.anchor.state_rev += 1,
            4 => bad.anchor.render_rev += 1,
            _ => bad.anchor.render_key += 1,
        }
        assert_eq!(v.query(bad).unwrap_err().kind, ErrorKind::Busy);
    }
    for point in [
        [-0.001, 0.5],
        [0.5, 1.001],
        [f64::NAN, 0.5],
        [0.5, f64::INFINITY],
    ] {
        let mut bad = valid.clone();
        bad.position = point;
        assert_eq!(v.query(bad).unwrap_err().kind, ErrorKind::InvalidInput);
    }
    for radius in [-1., 64.001, f64::NAN, f64::INFINITY] {
        let mut bad = valid.clone();
        bad.radius_px = radius;
        assert_eq!(v.query(bad).unwrap_err().kind, ErrorKind::InvalidInput);
    }
    let mut bad = valid.clone();
    bad.layers = Layers::Only(vec![(99, 0)]);
    assert_eq!(v.query(bad).unwrap_err().kind, ErrorKind::InvalidInput);
    assert_eq!(v.query_snapshot().accepted, 0);
    assert!(c.query_requests.lock().unwrap().is_empty());
    let id = v.query(valid).unwrap();
    let q = result(&v, id, QueryKind::Snap);
    assert_eq!(
        (q.reply.request.x, q.reply.request.y, q.reply.request.radius),
        (400, 320, 10)
    );
    assert_eq!(q.reply.status, QueryStatus::Ok);
    c.geometry_partial.store(true, Ordering::Relaxed);
    v.edit(v.snapshot().state_rev, pan()).unwrap();
    wait(|| v.latest().is_some_and(|f| f.frame.partial));
    assert_eq!(
        v.query(input(&v, QueryOperation::Snap)).unwrap_err().kind,
        ErrorKind::Incomplete
    );
    assert!(v.query_snapshot().snap.is_none());
    v.close().unwrap();
    assert_eq!(r.usage(), Usage::default());
}

#[test]
fn query_positions_match_gtk_truncation_and_hidden_layers_are_rejected() {
    use floe_worker_client::QueryOperation;
    let r = Resources::new(Limits::default()).unwrap();
    let mut m = model(false);
    let second = Style {
        layer: (2, 0),
        ..m.styles[0].clone()
    };
    let mm = Arc::get_mut(&mut m).unwrap();
    mm.styles = Arc::new(vec![mm.styles[0].clone(), second]);
    mm.pairs.insert((2, 0));
    let mut initial = ViewState::initial(&m, 800, 640).unwrap();
    initial.layers = Layers::Only(vec![(1, 0)]);
    initial.viewport = Viewport::new([-10.75, -5.5, 9.25, 10.5], 800, 640).unwrap();
    let c = Arc::new(Control::default());
    let mut v = start(&r, m, initial, Arc::clone(&c));
    wait(|| v.latest().is_some());
    let mut q = input(&v, QueryOperation::Pick { nth: -1 });
    q.position = [0., 1.];
    q.layers = Layers::Only(vec![(2, 0)]);
    assert_eq!(
        v.query(q.clone()).unwrap_err().kind,
        ErrorKind::InvalidInput
    );
    q.layers = Layers::All;
    let id = v.query(q).unwrap();
    let hit = result(&v, id, QueryKind::Pick);
    assert_eq!(
        (
            hit.reply.request.x,
            hit.reply.request.y,
            hit.reply.request.radius
        ),
        (-10, -5, 1)
    );
    assert_eq!(hit.reply.request.layers, Layers::Only(vec![(1, 0)]));
    let old = hit.anchor;
    v.close().unwrap();
    assert_eq!(
        v.query(ViewQuery {
            anchor: old,
            ..input_from_anchor(old)
        })
        .unwrap_err()
        .kind,
        ErrorKind::Cancelled
    );
    assert!(v.query_snapshot().pick.is_none());
    assert_eq!(r.usage(), Usage::default());
}
fn input_from_anchor(anchor: QueryAnchor) -> ViewQuery {
    ViewQuery {
        anchor,
        operation: floe_worker_client::QueryOperation::Snap,
        position: [0.5, 0.5],
        radius_px: 10.,
        layers: Layers::All,
    }
}

#[test]
fn pending_input_is_latest_only_and_native_work_stays_bounded_per_kind() {
    use floe_worker_client::QueryOperation;
    let (r, c, mut v) = setup(false);
    c.query_busy.store(true, Ordering::Relaxed);
    let mut latest = 0;
    for i in 0..1000 {
        let mut q = input(&v, QueryOperation::Snap);
        q.position[0] = f64::from(i) / 1000.;
        latest = v.query(q).unwrap();
    }
    assert_eq!(v.query_snapshot().queued, 1);
    assert_eq!(v.query_snapshot().in_flight, 0);
    c.query_busy.store(false, Ordering::Relaxed);
    let q = result(&v, latest, QueryKind::Snap);
    assert_eq!(q.reply.request.x, 799);
    assert_eq!(v.query_snapshot().submitted, 1);
    c.query_reply.store(false, Ordering::Relaxed);
    for _ in 0..40 {
        v.query(input(&v, QueryOperation::Snap)).unwrap();
        v.query(input(&v, QueryOperation::Pick { nth: 0 })).unwrap();
        thread::sleep(Duration::from_millis(1));
        assert!(v.query_snapshot().queued <= 2 && v.query_snapshot().in_flight <= 4);
    }
    wait(|| v.query_snapshot().in_flight == 4);
    let q = v.query_snapshot();
    let snap = q.snap_id.unwrap();
    let pick = q.pick_id.unwrap();
    c.query_reply.store(true, Ordering::Relaxed);
    result(&v, snap, QueryKind::Snap);
    result(&v, pick, QueryKind::Pick);
    wait(|| v.query_snapshot().in_flight == 0);
    assert!(v.query_snapshot().discarded >= 999);
    assert_eq!(
        v.snapshot().submitted,
        1,
        "queries must not trigger renders"
    );
    v.close().unwrap();
    assert_eq!(r.usage(), Usage::default());
}

#[test]
fn cancel_kind_is_independent_and_styles_wait_for_replies_not_just_ack() {
    use floe_worker_client::QueryOperation;
    let (r, c, mut v) = setup(false);
    c.query_reply.store(false, Ordering::Relaxed);
    v.query(input(&v, QueryOperation::Snap)).unwrap();
    let pick = v.query(input(&v, QueryOperation::Pick { nth: 0 })).unwrap();
    wait(|| v.query_snapshot().in_flight == 2);
    v.cancel_query(QueryKind::Snap);
    wait(|| !c.query_cancels.lock().unwrap().is_empty());
    assert_eq!(*c.query_cancels.lock().unwrap(), [QueryKind::Snap]);
    assert_eq!(v.query_snapshot().pick_id, Some(pick));
    c.query_reply.store(true, Ordering::Relaxed);
    result(&v, pick, QueryKind::Pick);
    wait(|| v.query_snapshot().in_flight == 0);
    assert!(v.query_snapshot().snap.is_none());
    c.query_reply.store(false, Ordering::Relaxed);
    v.query(input(&v, QueryOperation::Snap)).unwrap();
    wait(|| v.query_snapshot().in_flight == 1);
    let style = Style {
        color: [255, 0, 0, 255],
        ..v.model.styles[0].clone()
    };
    v.edit(
        v.snapshot().state_rev,
        Patch {
            style_changes: vec![style],
            ..Default::default()
        },
    )
    .unwrap();
    wait(|| c.query_cancels.lock().unwrap().len() == 2);
    thread::sleep(Duration::from_millis(30));
    assert_eq!(v.snapshot().phase, Phase::Cancelling);
    assert!(
        c.styles.lock().unwrap().is_empty(),
        "cancel ACK cannot release query credit"
    );
    assert_eq!(v.snapshot().submitted, 1);
    c.query_reply.store(true, Ordering::Relaxed);
    wait(|| {
        v.latest()
            .is_some_and(|f| f.render_rev == v.snapshot().render_rev)
    });
    assert_eq!(c.styles.lock().unwrap().len(), 1);
    assert!(v.query_snapshot().snap.is_none());
    v.close().unwrap();
    assert_eq!(r.usage(), Usage::default());
}

#[test]
fn stale_queries_do_not_delay_navigation_and_failures_release_reservations() {
    use floe_worker_client::QueryOperation;
    let (r, c, mut v) = setup(false);
    c.query_reply.store(false, Ordering::Relaxed);
    let q = input(&v, QueryOperation::Snap);
    v.query(q.clone()).unwrap();
    wait(|| v.query_snapshot().in_flight == 1);
    v.edit(v.snapshot().state_rev, pan()).unwrap();
    wait(|| v.snapshot().submitted == 2 && v.latest().is_some());
    assert_eq!(v.query(q).unwrap_err().kind, ErrorKind::Busy);
    c.query_reply.store(true, Ordering::Relaxed);
    wait(|| v.query_snapshot().in_flight == 0);
    assert!(v.query_snapshot().snap.is_none());
    c.query_reply.store(false, Ordering::Relaxed);
    v.query(input(&v, QueryOperation::Snap)).unwrap();
    wait(|| v.query_snapshot().in_flight == 1);
    c.query_failed.store(true, Ordering::Relaxed);
    wait(|| v.snapshot().phase == Phase::Failed);
    v.close().unwrap();
    assert_eq!(r.usage(), Usage::default());
    assert!(v.query_snapshot().snap_id.is_none());
}

#[test]
fn displayed_foreground_binds_the_covering_margin_and_crop_keeps_that_scene() {
    use floe_worker_client::QueryOperation;
    let (r, _c, mut v) = setup(true);
    wait(|| v.margin().is_some());
    let foreground = v.latest().unwrap();
    let margin = v.margin().unwrap();
    assert_ne!(foreground.frame.generation, margin.frame.generation);
    let q = input(&v, QueryOperation::Snap);
    let id = v.query(q).unwrap();
    let found = result(&v, id, QueryKind::Snap);
    assert_eq!(found.anchor.frame_id, foreground.id);
    assert_eq!(found.reply.scene.id, margin.frame.query_scene().unwrap().id);
    let mut p = pan();
    p.navigation = Some(Navigation::Pan {
        x: 0.1,
        y: 0.,
        snap: true,
    });
    v.edit(v.snapshot().state_rev, p).unwrap();
    wait(|| v.snapshot().crop_hits == 1);
    assert_eq!(v.snapshot().submitted, 2);
    let anchor = v.query_anchor(margin.id).unwrap();
    let id = v.query(input_from_anchor(anchor)).unwrap();
    let found = result(&v, id, QueryKind::Snap);
    assert_eq!(found.reply.scene.id, margin.frame.query_scene().unwrap().id);
    assert_eq!(found.reply.request.x, 480);
    assert!(v.query_anchor(foreground.id).is_err());
    v.close().unwrap();
    assert_eq!(r.usage(), Usage::default());
}

#[test]
fn label_only_source_identity_preserves_wide_bounds_but_not_changed_metadata() {
    let (_r, _c, mut v) = setup(true);
    wait(|| v.margin().is_some());
    let f = v.latest().unwrap();
    let m = v.margin().unwrap();
    let mut q = query::Queries::default();
    q.observe(&m).unwrap();
    let make = |request, fields| DisplayFrame {
        id: m.id + 1,
        dataset_revision: m.dataset_revision,
        state_rev: m.state_rev + 1,
        render_rev: m.render_rev + 1,
        render_key: m.render_key + 1,
        worker_epoch: m.worker_epoch,
        deck_skipped: 0,
        purpose: Purpose::Foreground,
        frame: Frame {
            generation: m.frame.generation + 1,
            round: 1,
            final_frame: true,
            partial: false,
            deferred: 0,
            labels_truncated: true,
            request,
            bytes: Vec::new(),
            fields,
        },
    };
    let mut request = f.frame.request.clone();
    request.labels = !request.labels;
    request.font_px += 1;
    q.observe(&make(request.clone(), m.frame.fields.clone()))
        .unwrap();
    assert_eq!(q.source.as_ref().unwrap().viewport, m.viewport());
    assert_eq!(q.source.as_ref().unwrap().key, m.render_key + 1);
    let mut changed = m.frame.fields.clone();
    changed.0.insert("scene_complete".into(), "0".into());
    assert!(q.observe(&make(request.clone(), changed)).is_err());
    let mut changed = m.frame.fields.clone();
    changed.0.insert("style_epoch".into(), "99".into());
    assert!(q.observe(&make(request.clone(), changed)).is_err());
    request.thin = floe_worker_client::ThinPolicy::Keep;
    assert_ne!(request.thin, f.frame.request.thin);
    assert!(q.observe(&make(request, m.frame.fields.clone())).is_err());
    v.close().unwrap();
}

#[test]
fn deck_queries_are_explicitly_unsupported() {
    let r = Resources::new(Limits::default()).unwrap();
    let m = model(true);
    let initial = ViewState::initial(&m, 800, 640).unwrap();
    let mut v = start(&r, m, initial, Arc::new(Control::default()));
    wait(|| v.latest().is_some());
    let q = input(&v, floe_worker_client::QueryOperation::Snap);
    assert_eq!(v.query(q).unwrap_err().kind, ErrorKind::Unsupported);
    assert_eq!(v.query_snapshot().accepted, 0);
    v.close().unwrap();
    assert_eq!(r.usage(), Usage::default());
}

#[test]
fn departing_consumer_cannot_cancel_another_consumers_newer_query() {
    use floe_worker_client::QueryOperation;
    let (r, c, mut v) = setup(false);
    c.query_reply.store(false, Ordering::Relaxed);
    let old = v.query(input(&v, QueryOperation::Snap)).unwrap();
    let current = v.query(input(&v, QueryOperation::Snap)).unwrap();
    let pick = v.query(input(&v, QueryOperation::Pick { nth: 0 })).unwrap();
    assert!(!v.cancel_query_if_current(QueryKind::Snap, old));
    assert_eq!(v.query_snapshot().snap_id, Some(current));
    assert!(v.cancel_query_if_current(QueryKind::Snap, current));
    assert!(!v.cancel_query_if_current(QueryKind::Snap, current));
    assert_eq!(v.query_snapshot().pick_id, Some(pick));
    c.query_reply.store(true, Ordering::Relaxed);
    result(&v, pick, QueryKind::Pick);
    assert!(v.query_snapshot().snap.is_none());
    v.close().unwrap();
    assert_eq!(r.usage(), Usage::default());
}
