use super::{drc_isolation::ReviewSocket, *};

async fn current(h: &Harness, l: &Login) -> Value {
    h.call(l, "GET", "/api/v1/view", Value::Null).await.1
}
fn action(seq: u64, target: Option<&Value>) -> Value {
    let mut value =
        json!({"action":"open","seq":seq.to_string(),"pixels":[137,103],"levels":{"mode":"all"}});
    if let Some(target) = target {
        value["view_id"] = target["view"]["view_id"].clone();
        value["state_rev"] = target["view"]["state_rev"].clone();
    }
    value
}
fn proposal(h: &Harness, source: &Value, body: Value) -> String {
    let (id, _) = h.launches.reserve().unwrap();
    h.launches
        .ready(
            &id,
            Some(json!({"kind":"open","seq":"1","source_id":source,"mode":"level","body":body})),
            false,
        )
        .unwrap();
    format!("/api/v1/launch/{id}")
}
async fn frame_ready(h: &Harness, l: &Login) -> Value {
    let mut stream = h.connect(l).await;
    let (_, f) = frame(&mut stream).await;
    stream.close(None).await.unwrap();
    let state = current(h, l).await;
    assert_eq!(state["view"]["view_id"], f["view_id"]);
    state
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "run tools/validate_owner_service.py with private mode fixtures"]
async fn launcher_once_only_navigation_and_atomic_worker_replacement() {
    let source = PathBuf::from(std::env::var_os("FLOE_OWNER_MODE_FIXTURE").unwrap());
    let dir = source.parent().unwrap();
    let other = dir.join("B.oas");
    let unindexed = dir.join("launch-unindexed.oas");
    fs::copy(&source, &unindexed).unwrap();
    let scope = AccessScope::new(&[dir.to_owned()]).unwrap();
    let h = Harness::configured_limits(
        &[],
        native(),
        None,
        false,
        false,
        Limits {
            cpu_slots: 2,
            foreground_reserve: 0,
            workers: 1,
            decoded_mb: 64,
        },
    )
    .await;
    let l = h.login().await;
    // Two authenticated long polls wait independently of catalogue reads and
    // are woken by preparation, not by a five-second HTTP timeout. The third
    // caller must fail promptly instead of occupying another pending reader.
    let (waiting, _) = h.launches.reserve().unwrap();
    let revision = h.call(&l, "GET", "/api/v1/launch", Value::Null).await.1["revision"]
        .as_str()
        .unwrap()
        .to_owned();
    let poll_path = format!("/api/v1/launch/poll/{revision}");
    let polls: Vec<_> = (0..3)
        .map(|_| Box::pin(h.call(&l, "GET", &poll_path, Value::Null)))
        .collect();
    let (first, _, remaining) = timeout(
        Duration::from_secs(3),
        futures_util::future::select_all(polls),
    )
    .await
    .unwrap();
    assert_eq!(first.0, 429);
    assert_eq!(
        h.call(&l, "GET", "/api/v1/catalog", Value::Null).await.0,
        200
    );
    h.launches.ready(&waiting, None, false).unwrap();
    for p in timeout(
        Duration::from_secs(3),
        futures_util::future::join_all(remaining),
    )
    .await
    .unwrap()
    {
        assert_eq!(p.0, 200);
        assert_eq!(p.1["pending"]["phase"], "ready");
    }
    assert_eq!(
        h.call(
            &l,
            "POST",
            &format!("/api/v1/launch/{waiting}"),
            json!({"action":"present"})
        )
        .await
        .0,
        200
    );
    assert_eq!(
        h.call(&l, "GET", "/api/v1/capabilities", Value::Null)
            .await
            .1["launcher"],
        true
    );
    // The whole CLI batch rolls back, including protected-source membership.
    let stop = AtomicUsize::new(0);
    assert!(h
        .service
        .register_sources(
            Arc::clone(&scope),
            &[source.clone(), dir.join("missing.oas")],
            &stop
        )
        .is_err());
    assert_eq!(h.service.catalog()["sources"], json!([]));
    let ids = h
        .service
        .register_sources(
            Arc::clone(&scope),
            &[source.clone(), source.clone(), other, unindexed.clone()],
            &stop,
        )
        .unwrap();
    assert_eq!(ids[0], ids[1]);
    assert_eq!(h.service.catalog()["sources"].as_array().unwrap().len(), 3);
    assert!(!cache::cache_path(&unindexed).unwrap().exists());
    let a = json!(ids[0]);
    let b = json!(ids[2]);
    let bad = json!(ids[3]);

    let endpoint = proposal(
        &h,
        &a,
        json!({"depth":"full","thin":"keep","labels":false,"mono":true,
        "navigation":{"kind":"goto","center_um":["103","117"],"width_um":"311"}}),
    );
    // Owner session, same-origin and CSRF remain mandatory. There is no path
    // registration or arbitrary operation body in the browser's action DTO.
    assert_eq!(h.raw("GET", "/api/v1/launch", &[], "").await.0, 401);
    assert_eq!(h.raw("GET", &endpoint, &[], "").await.0, 401);
    assert!(h.call(&l, "GET", &endpoint, Value::Null).await.1["receipt"].is_null());
    assert_eq!(
        h.raw("POST", &endpoint, &[], &action(1, None).to_string())
            .await
            .0,
        403
    );
    let mut invalid = action(1, None);
    invalid["path"] = json!(source);
    assert_eq!(h.call(&l, "POST", &endpoint, invalid).await.0, 400);
    assert_eq!(
        h.call(&l, "GET", "/api/v1/launch?after=0", Value::Null)
            .await
            .0,
        403
    );
    for after in ["00", "01", "-1", "18446744073709551616"] {
        assert_eq!(
            h.call(
                &l,
                "GET",
                &format!("/api/v1/launch/poll/{after}"),
                Value::Null
            )
            .await
            .0,
            400
        );
    }
    let queued = h
        .call(&l, "GET", "/api/v1/launch/poll/0", Value::Null)
        .await
        .1;
    assert_eq!(queued["pending"]["phase"], "ready");
    assert!(!queued.to_string().contains(dir.to_str().unwrap()));
    assert!(h.launches.reserve().is_err());
    assert_eq!(h.service.operations()["last_seq"], "0");
    let request = action(1, None);
    let receipt = h.call(&l, "POST", &endpoint, request.clone()).await;
    assert_eq!(receipt.0, 200, "{:?}", receipt);
    assert_eq!(receipt.1["phase"], "submitted");
    assert_eq!(
        h.call(&l, "GET", &endpoint, Value::Null).await.1["receipt"],
        receipt.1
    );
    assert_eq!(h.finished(&l, 1).await["phase"], "succeeded");
    assert_eq!(h.call(&l, "POST", &endpoint, request).await.1, receipt.1);
    assert_eq!(h.call(&l, "POST", &endpoint, action(2, None)).await.0, 409);
    assert_eq!(h.service.operations()["last_seq"], "1");
    let first = frame_ready(&h, &l).await;
    assert_eq!(first["view"]["pixels"], json!([137, 103]));
    let usage = h.resources.usage();
    assert_eq!(usage.workers, 1);

    // Same file edits exactly once; no replacement process, cache epoch or
    // implicit reset of unspecified preferences (mono remains on).
    let endpoint = proposal(
        &h,
        &a,
        json!({"detail":"high","navigation":{"kind":"goto","center_um":["123","117"],"width_um":"311"}}),
    );
    let edit = action(2, Some(&first));
    assert_eq!(h.call(&l, "POST", &endpoint, edit.clone()).await.0, 200);
    assert_eq!(h.finished(&l, 2).await["reused"], true);
    let moved = frame_ready(&h, &l).await;
    assert_eq!(moved["view"]["view_id"], first["view"]["view_id"]);
    assert_eq!(moved["view"]["worker_epoch"], first["view"]["worker_epoch"]);
    assert_eq!(moved["view"]["state_rev"], "2");
    assert_eq!(moved["view"]["mono"], true);
    h.call(&l, "POST", &endpoint, edit).await;
    assert_eq!(current(&h, &l).await["view"]["state_rev"], "2");

    // Stale target and failed preparation must not close the useful old view.
    for (seq, source, target) in [(3, &b, &first), (4, &bad, &moved)] {
        let endpoint = proposal(&h, source, json!({"depth":"full","labels":false}));
        assert_eq!(
            h.call(&l, "POST", &endpoint, action(seq, Some(target)))
                .await
                .0,
            200
        );
        assert_eq!(h.finished(&l, seq).await["phase"], "failed");
        let still = current(&h, &l).await;
        for field in ["view_id", "worker_epoch", "state_rev"] {
            assert_eq!(still["view"][field], moved["view"][field]);
        }
        assert_eq!(h.resources.usage(), usage);
    }
    assert!(!cache::cache_path(&unindexed).unwrap().exists());

    // Different source is prepared before the atomic cutover. One worker
    // reservation proves this is not a concurrent second renderer.
    let old_stream = ReviewSocket::new(&h, &l).await;
    let endpoint = proposal(&h, &b, json!({"depth":"full","labels":false}));
    let replace = action(5, Some(&moved));
    let accepted = h.call(&l, "POST", &endpoint, replace.clone()).await;
    assert_eq!(accepted.0, 200);
    assert_eq!(h.finished(&l, 5).await["phase"], "succeeded");
    old_stream.closed().await;
    let next = frame_ready(&h, &l).await;
    assert_ne!(next["view"]["view_id"], moved["view"]["view_id"]);
    assert_ne!(next["view"]["worker_epoch"], moved["view"]["worker_epoch"]);
    assert_eq!(next["source_id"], b);
    assert_eq!(next["view"]["state_rev"], "1");
    assert_eq!(h.resources.usage(), usage);
    assert_eq!(h.call(&l, "POST", &endpoint, replace).await.1, accepted.1);
    assert_eq!(
        h.call(&l, "POST", &endpoint, json!({"action":"dismiss"}))
            .await
            .0,
        409
    );
    // A second stale launch still cannot retire the newer attachment.
    let endpoint = proposal(&h, &a, json!({}));
    assert_eq!(
        h.call(&l, "POST", &endpoint, action(6, Some(&moved)))
            .await
            .0,
        200
    );
    assert_eq!(h.finished(&l, 6).await["phase"], "failed");
    assert_eq!(
        current(&h, &l).await["view"]["view_id"],
        next["view"]["view_id"]
    );

    // Dismissal fences background preparation, and a late completion cannot
    // replace a newer proposal. Present-only never consumes an operation seq.
    let (cancelled, flag) = h.launches.reserve().unwrap();
    let end = format!("/api/v1/launch/{cancelled}");
    let dismiss = h.call(&l, "POST", &end, json!({"action":"dismiss"})).await;
    assert_eq!(dismiss.0, 200);
    assert_eq!(flag.load(std::sync::atomic::Ordering::Relaxed), 1);
    let mut first_present = None;
    for _ in 0..17 {
        let (id, _) = h.launches.reserve().unwrap();
        h.launches.ready(&id, None, false).unwrap();
        assert!(h.launches.ready(&cancelled, None, false).is_err());
        let endpoint = format!("/api/v1/launch/{id}");
        if first_present.is_none() {
            first_present = Some(endpoint.clone());
        }
        let present = json!({"action":"present"});
        let result = h.call(&l, "POST", &endpoint, present.clone()).await;
        assert_eq!(result.0, 200);
        assert_eq!(result.1["phase"], "presented");
        assert_eq!(h.call(&l, "POST", &endpoint, present).await.1, result.1);
    }
    assert_eq!(
        h.call(
            &l,
            "POST",
            &first_present.unwrap(),
            json!({"action":"present"})
        )
        .await
        .1["error"],
        "launch_expired"
    );
    assert_eq!(h.service.operations()["last_seq"], "6");
    assert_eq!(
        current(&h, &l).await["view"]["view_id"],
        next["view"]["view_id"]
    );
    let (id, flag) = h.launches.reserve().unwrap();
    assert_eq!(
        h.call(&l, "DELETE", "/api/v1/session", Value::Null).await.0,
        204
    );
    assert_eq!(flag.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert!(h.launches.ready(&id, None, false).is_err());
    assert!(h.launches.reserve().is_err());
    h.shutdown().await;
    println!("RUST OWNER LAUNCH: ALL OK (batch rollback, authenticated opaque proposals, once-only edit/cutover, one worker, stale/failure/cancel/replay/expiry/logout)");
}
