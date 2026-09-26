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
