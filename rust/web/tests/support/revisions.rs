use super::{
    drc_isolation::ReviewSocket,
    index_open::{area, operation, state, target},
    *,
};

fn build(seq: u64, source: &Value) -> Value {
    json!({"kind":"index_revision","seq":seq.to_string(),"source_id":source,
        "levels":{"mode":"all"},"approved":true,"options":{"jobs":2}})
}
fn check(seq: u64, source: &Value) -> Value {
    json!({"kind":"check_revision","seq":seq.to_string(),"source_id":source,"levels":{"mode":"all"}})
}
fn use_it(seq: u64, source: &Value, rev: &Value, target: Value, mode: &str) -> Value {
    json!({"kind":"use_revision","seq":seq.to_string(),"source_id":source,"levels":{"mode":"all"},
        "mode":mode,"revision":rev,"approved":true,"target":target,"pixels":[137,103]})
}

fn prepare_reclaim(seq: u64, source: &Value, revision: &Value) -> Value {
    json!({"kind":"prepare_reclaim","seq":seq.to_string(),"source_id":source,"revision":revision})
}
fn reclaim(seq: u64, preview: &Value) -> Value {
    json!({"kind":"reclaim_revision","seq":seq.to_string(),"source_id":preview["source_id"],
        "revision":preview["preview"]["revision"],"token":preview["preview"]["token"],"approved":true})
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "run tools/validate_owner_service.py on private fixtures"]
async fn explicit_reclamation_is_one_shot_and_new_session_requires_new_preview() {
    let dir = area("revision-reclaim");
    let path = dir.join("A.oas");
    let h = Harness::start(std::slice::from_ref(&path), native()).await;
    let login = h.login().await;
    let source = h.service.catalog()["sources"][0]["source_id"].clone();
    let first = operation(&h, &login, build(1, &source)).await;
    let old = first["index_revision"].clone();
    assert_eq!(first["phase"], "succeeded", "{first}");
    let store = cache::revision::set::Store::new(&path).unwrap();
    let pin = store
        .pin(&[path.clone()].into(), &None, &AtomicUsize::new(0))
        .unwrap()
        .unwrap();
    let second = operation(&h, &login, build(2, &source)).await;
    let current = fs::read(store.path().join("current.json")).unwrap();
    assert_eq!(
        operation(&h, &login, prepare_reclaim(3, &source, &old)).await["error"],
        "busy"
    );
    drop(pin);
    let preview = operation(&h, &login, prepare_reclaim(4, &source, &old)).await;
    assert_eq!(preview["phase"], "succeeded", "{preview}");
    assert!(!preview.to_string().contains(dir.to_str().unwrap()));
    assert!(!store
        .path()
        .join(format!(".reclaim-{}.json", old.as_str().unwrap()))
        .exists());
    let request = reclaim(5, &preview);
    // Even a valid preview grants no authority to an unauthenticated or
    // non-CSRF request. Neither denial changes the operation ledger.
    let origin = format!("http://{}", h.addr);
    for authenticated in [false, true] {
        let mut headers = vec![
            ("Origin", origin.as_str()),
            ("Content-Type", "application/json"),
        ];
        if authenticated {
            headers.push(("Cookie", login.cookie.as_str()));
        }
        let response = h
            .raw("POST", "/api/v1/operations", &headers, &request.to_string())
            .await;
        assert!(matches!(response.0, 401 | 403));
    }
    for (key, value, error) in [
        ("approved", json!(false), "approval_required"),
        ("token", json!("e".repeat(64)), "preview_unavailable"),
        (
            "revision",
            second["index_revision"].clone(),
            "preview_unavailable",
        ),
    ] {
        let mut bad = request.clone();
        bad[key] = value;
        assert_eq!(
            h.call(&login, "POST", "/api/v1/operations", bad).await.1["error"],
            error
        );
    }
    assert_eq!(h.service.operations()["last_seq"], "4");
    let usage = operation(
        &h,
        &login,
        json!({"kind":"revision_usage","seq":"5","source_id":source}),
    )
    .await;
    assert!(usage["inventory"]["recoveries"]
        .as_array()
        .unwrap()
        .is_empty());
    let mut stale = request.clone();
    stale["seq"] = json!("6");
    assert_eq!(
        h.call(&login, "POST", "/api/v1/operations", stale).await.1["error"],
        "preview_unavailable"
    );
    let preview = operation(&h, &login, prepare_reclaim(6, &source, &old)).await;
    let old_request = reclaim(7, &preview);
    // The accepted preview is not persisted as approval across a server restart.
    h.shutdown().await;
    let h = Harness::start(std::slice::from_ref(&path), native()).await;
    let login = h.login().await;
    let source = h.service.catalog()["sources"][0]["source_id"].clone();
    let mut stale = old_request;
    stale["seq"] = json!("1");
    stale["source_id"] = source.clone();
    assert_eq!(
        h.call(&login, "POST", "/api/v1/operations", stale).await.1["error"],
        "preview_unavailable"
    );
    let preview = operation(&h, &login, prepare_reclaim(1, &source, &old)).await;
    let request = reclaim(2, &preview);
    let done = operation(&h, &login, request.clone()).await;
    assert_eq!(done["phase"], "succeeded", "{done}");
    assert_eq!(done["outcome"]["status"], "complete");
    assert!(!store.path().join(old.as_str().unwrap()).exists());
    assert_eq!(
        fs::read(store.path().join("current.json")).unwrap(),
        current
    );
    assert_eq!(
        h.call(&login, "POST", "/api/v1/operations", request.clone())
            .await
            .1,
        done,
        "same receipt, no replayed deletion"
    );
    let mut used = request;
    used["seq"] = json!("3");
    assert_eq!(
        h.call(&login, "POST", "/api/v1/operations", used).await.1["error"],
        "preview_unavailable"
    );
    let usage = operation(
        &h,
        &login,
        json!({"kind":"revision_usage","seq":"3","source_id":source}),
    )
    .await;
    assert_eq!(usage["inventory"]["recoveries"], json!([old]));
    let checked = operation(&h, &login, prepare_reclaim(4, &source, &old)).await;
    assert_eq!(checked["preview"]["complete"], true);
    assert_eq!(checked["preview"]["files"], 0);
    assert_eq!(
        h.call(&login, "POST", "/api/v1/operations", reclaim(5, &checked))
            .await
            .1["error"],
        "preview_unavailable"
    );
    let opened = operation(
        &h,
        &login,
        use_it(
            5,
            &source,
            &second["index_revision"],
            json!({"kind":"empty"}),
            "level",
        ),
    )
    .await;
    assert_eq!(opened["phase"], "succeeded");
    let mut ws = h.connect(&login).await;
    frame(&mut ws).await;
    ws.close(None).await.ok();
    h.shutdown().await;
    println!("RUST REVISION RECLAIM: ALL OK (opaque preview, current/pin protection, separate consent, at-most-once deletion, restart reapproval, journal discovery, current frame)");
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "run tools/validate_owner_service.py on private fixtures"]
async fn killed_synthetic_reclaimer_is_recovered_only_after_new_http_consent() {
    let dir = area("reclaim-recovery");
    let path = dir.join("A.oas");
    let h = Harness::start(std::slice::from_ref(&path), native()).await;
    let login = h.login().await;
    let source = h.service.catalog()["sources"][0]["source_id"].clone();
    let old = operation(&h, &login, build(1, &source)).await["index_revision"].clone();
    let current_revision = operation(&h, &login, build(2, &source)).await["index_revision"].clone();
    let store = cache::revision::set::Store::new(&path).unwrap();
    let current = fs::read(store.path().join("current.json")).unwrap();
    h.shutdown().await;
    // A test-only backend barrier stops after one acknowledged unlink. Kill
    // and reap only this child; no production fault-injection switch is added.
    let mut child = std::process::Command::new(std::env::var_os("FLOE_RECLAIM_TEST_BIN").unwrap())
        .args([
            "--exact",
            "cache::revision::set::reclaim::tests::reclaim_child",
            "--nocapture",
        ])
        .env("FLOE_TEST_RECLAIM_ROOT", &dir)
        .env("FLOE_TEST_RECLAIM_SOURCE", "A.oas")
        .env("FLOE_TEST_RECLAIM_ID", old.as_str().unwrap())
        .spawn()
        .unwrap();
    let ready = dir.join("child-ready");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready.exists() && Instant::now() < deadline {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let reached = ready.exists();
    let _ = child.kill();
    let status = child.wait().unwrap();
    assert!(reached && !status.success());
    assert_eq!(
        fs::read(store.path().join("current.json")).unwrap(),
        current
    );
    let h = Harness::start(std::slice::from_ref(&path), native()).await;
    let login = h.login().await;
    let source = h.service.catalog()["sources"][0]["source_id"].clone();
    let usage = operation(
        &h,
        &login,
        json!({"kind":"revision_usage","seq":"1","source_id":source}),
    )
    .await;
    assert_eq!(usage["inventory"]["recoveries"], json!([old]));
    assert!(
        store.path().join(old.as_str().unwrap()).exists(),
        "listing must not resume deletion"
    );
    let preview = operation(&h, &login, prepare_reclaim(2, &source, &old)).await;
    assert_eq!(preview["phase"], "succeeded", "{preview}");
    assert_eq!(preview["preview"]["recovery"], true);
    assert_eq!(preview["preview"]["complete"], false);
    let mut denied = reclaim(3, &preview);
    denied["approved"] = json!(false);
    assert_eq!(
        h.call(&login, "POST", "/api/v1/operations", denied).await.1["error"],
        "approval_required"
    );
    assert!(store.path().join(old.as_str().unwrap()).exists());
    let done = operation(&h, &login, reclaim(3, &preview)).await;
    assert_eq!(done["outcome"]["status"], "complete", "{done}");
    assert!(!store.path().join(old.as_str().unwrap()).exists());
    assert_eq!(
        fs::read(store.path().join("current.json")).unwrap(),
        current
    );
    assert_eq!(
        operation(
            &h,
            &login,
            use_it(
                4,
                &source,
                &current_revision,
                json!({"kind":"empty"}),
                "level"
            )
        )
        .await["phase"],
        "succeeded"
    );
    let mut ws = h.connect(&login).await;
    frame(&mut ws).await;
    ws.close(None).await.ok();
    h.shutdown().await;
    println!("RUST REVISION RECLAIM RECOVERY: ALL OK (private helper kill/reap, persisted journal, new owner preview/consent, current frame preserved)");
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "run tools/validate_owner_service.py on private fixtures"]
async fn explicit_revision_cutover_pins_old_view_and_rejects_stale_approvals() {
    let dir = area("revision-cutover");
    let source_path = dir.join("A.oas");
    let h = Harness::start(std::slice::from_ref(&source_path), native()).await;
    let login = h.login().await;
    let source = h.service.catalog()["sources"][0]["source_id"].clone();
    // No current pointer and no cache means discovery stays read-only.
    let checked = operation(&h, &login, check(1, &source)).await;
    assert_eq!(checked["phase"], "succeeded");
    assert!(checked["index_revision"].is_null());
    assert!(!dir.join(".A.oas.ice.revisions.sets").exists());
    let mut denied = build(2, &source);
    denied["approved"] = json!(false);
    assert_eq!(
        h.call(&login, "POST", "/api/v1/operations", denied).await.1["error"],
        "approval_required"
    );
    let request = build(2, &source);
    let built = operation(&h, &login, request.clone()).await;
    assert_eq!(built["phase"], "succeeded", "{built}");
    assert!(!cache::cache_path(&source_path).unwrap().exists());
    assert_eq!(
        h.call(&login, "GET", "/api/v1/view", Value::Null).await.0,
        404
    );
    assert_eq!(
        h.call(&login, "POST", "/api/v1/operations", request)
            .await
            .1,
        built
    );
    let checked = operation(&h, &login, check(3, &source)).await;
    let old = checked["index_revision"].clone();
    assert_eq!(old, built["index_revision"]);
    let opened = operation(
        &h,
        &login,
        use_it(4, &source, &old, json!({"kind":"empty"}), "level"),
    )
    .await;
    assert_eq!(opened["phase"], "succeeded", "{opened}");
    let mut ws = h.connect(&login).await;
    let (_, old_frame) = frame(&mut ws).await;
    let mut editor = ReviewSocket::new(&h, &login).await;
    editor
        .set(
            json!({"navigation":{"kind":"goto","center_um":["33","44"],"width_um":"120"},
        "detail":"high","thin":"keep","frames":true,"labels":true,"mono":true,
        "font_px":18,"depth":"2","layers":{"mode":"none"}}),
        )
        .await;
    drop(editor);
    let current = state(&h, &login).await;
    let (_, envelope) = h.call(&login, "GET", "/api/v1/view", Value::Null).await;
    assert_eq!(envelope["index_revision"], old);
    let built2 = operation(&h, &login, build(5, &source)).await;
    assert_eq!(built2["phase"], "succeeded", "{built2}");
    assert_ne!(built2["index_revision"], old);
    let unchanged = state(&h, &login).await;
    for k in [
        "view_id",
        "state_rev",
        "render_key",
        "bbox_dbu",
        "pixels",
        "layers",
    ] {
        assert_eq!(unchanged[k], current[k], "{k}");
    }
    // An old checked button cannot silently choose the now-newer pointer.
    let stale = operation(
        &h,
        &login,
        use_it(6, &source, &old, target(&current), "level"),
    )
    .await;
    assert_eq!(stale["phase"], "failed");
    assert_eq!(stale["error"], "busy");
    assert_eq!(state(&h, &login).await["view_id"], current["view_id"]);
    let checked = operation(&h, &login, check(7, &source)).await;
    let next = checked["index_revision"].clone();
    let mut wrong = target(&current);
    wrong["state_rev"] = json!("999");
    assert_eq!(
        operation(&h, &login, use_it(8, &source, &next, wrong, "level")).await["error"],
        "busy"
    );
    assert_eq!(state(&h, &login).await["view_id"], current["view_id"]);
    let request = use_it(9, &source, &next, target(&current), "level");
    let done = operation(&h, &login, request.clone()).await;
    assert_eq!(done["phase"], "succeeded", "{done}");
    let mut next_ws = h.connect(&login).await;
    let (_, next_frame) = frame(&mut next_ws).await;
    let after = state(&h, &login).await;
    assert_ne!(after["view_id"], current["view_id"]);
    assert_ne!(after["dataset_revision"], current["dataset_revision"]);
    for k in [
        "bbox_dbu",
        "pixels",
        "layers",
        "fill_slots_key",
        "depth",
        "detail",
        "thin",
        "frames",
        "labels",
        "font_px",
        "mono",
    ] {
        assert!(current.get(k).is_some(), "missing assertion field {k}");
        assert_eq!(after[k], current[k], "preserve {k}");
    }
    assert_ne!(old_frame["worker_epoch"], next_frame["worker_epoch"]);
    assert!(next_frame["worker_epoch"].is_string());
    assert_eq!(h.resources.usage().workers, 1);
    assert_eq!(
        h.call(&login, "POST", "/api/v1/operations", request)
            .await
            .1,
        done
    );
    // Completed replay never allocates another controller or revision.
    assert_eq!(state(&h, &login).await["view_id"], after["view_id"]);
    let usage = operation(
        &h,
        &login,
        json!({"kind":"revision_usage","seq":"10","source_id":source}),
    )
    .await;
    assert_eq!(usage["phase"], "succeeded", "{usage}");
    assert_eq!(usage["source_id"], source);
    let inventory = &usage["inventory"];
    assert_eq!(inventory["partial"], false);
    let rows = inventory["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 4);
    assert!(rows.iter().any(|r| r["kind"] == "set"
        && r["revision"] == next
        && r["current"] == true
        && r["readers"] == "in_use"));
    assert!(rows
        .iter()
        .any(|r| r["kind"] == "source" && r["set_revision"] == next && r["readers"] == "in_use"));
    assert!(!usage.to_string().contains(dir.to_str().unwrap()));
    assert_eq!(state(&h, &login).await["view_id"], after["view_id"]);
    println!("RUST REVISION INVENTORY: ALL OK (read-only owner request, current and pinned protection, no filesystem paths)");
    ws.close(None).await.ok();
    next_ws.close(None).await.ok();
    h.shutdown().await;
    println!("RUST INDEX REVISION CUTOVER: ALL OK (read-only check, separate approval, immutable build, old view, pointer/view CAS, epoch, one worker, replay)");
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "run tools/validate_owner_service.py on private fixtures"]
async fn revision_deck_mode_never_adopts_a_new_set() {
    let dir = area("revision-deck-mode");
    let deck = dir.join("deck.jb");
    fs::write(&deck,"MTITLE 1,ONE\nCHIP A\n$ (1,A,TC=A.oas,AD=0.001,LY={1},DT={0},UX=100,UY=100)\nROWS 0/0\nMTITLE 2,TWO\nCHIP B\n$ (2,B,TC=B.oas,AD=0.001,LY={1},DT={0},UX=100,UY=100)\nROWS 0/0\n").unwrap();
    let h = Harness::start(&[deck], native()).await;
    let login = h.login().await;
    let source = h.service.catalog()["sources"][0]["source_id"].clone();
    let first = operation(&h, &login, build(1, &source)).await;
    assert_eq!(first["phase"], "succeeded", "{first}");
    let open = operation(
        &h,
        &login,
        use_it(
            2,
            &source,
            &first["index_revision"],
            json!({"kind":"empty"}),
            "level",
        ),
    )
    .await;
    assert_eq!(open["phase"], "succeeded", "{open}");
    let mut ws = h.connect(&login).await;
    frame(&mut ws).await;
    let before = state(&h, &login).await;
    assert_eq!(
        operation(&h, &login, build(3, &source)).await["phase"],
        "succeeded"
    );
    let mode=operation(&h,&login,json!({"kind":"mode","seq":"4","view_id":before["view_id"],"base_state_rev":before["state_rev"],"mode":"chip"})).await;
    assert_eq!(mode["phase"], "succeeded", "{mode}");
    let mut chip = h.connect(&login).await;
    frame(&mut chip).await;
    let (_, current) = h.call(&login, "GET", "/api/v1/view", Value::Null).await;
    assert_eq!(current["index_revision"], first["index_revision"]);
    assert_eq!(current["view"]["bbox_dbu"], before["bbox_dbu"]);
    // No silent fallback to legacy caches on loaded-level changes.
    let reselect = operation(
        &h,
        &login,
        json!({"kind":"reselect_levels","seq":"5","view_id":current["view"]["view_id"],
        "base_state_rev":current["view"]["state_rev"],"levels":{"mode":"only","ids":["1"]}}),
    )
    .await;
    assert_eq!(reselect["phase"], "failed");
    assert_eq!(
        state(&h, &login).await["view_id"],
        current["view"]["view_id"]
    );
    ws.close(None).await.ok();
    chip.close(None).await.ok();
    h.shutdown().await;
    println!("RUST INDEX REVISION DECK: ALL OK (atomic source set, mode pin, no implicit mutable fallback)");
}
