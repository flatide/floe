//! Independent native views, scoped display edits, and shared admission.
use super::*;

#[tokio::test]
#[ignore = "run tools/validate_view_stream.py with a private synthetic fixture"]
async fn local_explore_deck_parent_cannot_expand_beyond_granted_child() {
    use floe_worker_client::Layers;
    let source = PathBuf::from(std::env::var_os("FLOE_VIEW_FIXTURE").unwrap());
    let second = PathBuf::from(std::env::var_os("FLOE_VIEW_SECOND_FIXTURE").unwrap());
    let deck = source.parent().unwrap().join("share-scope.jb");
    std::fs::write(&deck, format!("MTITLE 1,MASK\nCHIP C1\n$ (1,A,TC='{}',AD=0.001,LY={{1}},DT={{0}},UX=500,UY=500)\nROWS 0/0\nCHIP C2\n$ (1,B,TC='{}',AD=0.001,LY={{1}},DT={{0}},UX=500,UY=500)\nROWS 500/0\n", source.display(), second.display())).unwrap();
    let resources = Resources::new(Limits::default()).unwrap();
    let data =
        ManagedDataset::open(&resources, &deck, None, Mode::Chip, &AtomicUsize::new(0)).unwrap();
    let model = Model::new(&data).unwrap();
    let mut initial = ViewState::initial(&model, 257, 191).unwrap();
    let parent = (1, 0);
    let expanded = initial
        .edit(
            &model,
            floe_app_core::view::Patch {
                layers: Some(Layers::Only(vec![parent])),
                ..Default::default()
            },
        )
        .unwrap();
    let Layers::Only(children) = expanded.layers else {
        panic!("deck needs a group");
    };
    assert_eq!(children.len(), 2);
    initial.layers = Layers::Only(vec![children[0]]);
    initial.frames = false;
    initial.labels = false;
    let mut options = RenderOptions::local().unwrap();
    options.decode_jobs = 1;
    options.raster_jobs = 1;
    options.budget_mb = 64;
    options.raw = true;
    let controller = Arc::new(
        ViewController::start_configured(
            &resources,
            data,
            options,
            initial,
            floe_app_core::view::ControllerOptions {
                margin_prefetch: false,
                frame_cache: true,
            },
        )
        .unwrap(),
    );
    let h = Harness::launch(resources, controller, false, true).await;
    let owner = h.login().await;
    let (mut ws, oh, _) = h.connect(&owner).await;
    // Current native jobdeck renderer has no pick/snap capability.
    let (of, _) = frame_with_query(&mut ws, false).await;
    ack(&mut ws, &oh, 1, &of).await;
    let a = invite(&h, &owner, "explore").await;
    let ac = exchange(&h, &a).await;
    let (mut av, ah) = guest_connect(&h, a["share_id"].as_str().unwrap(), &ac, "explore").await;
    let (af, _) = explore_frame_with_query(&mut av, false).await;
    ack(&mut av, &ah, 1, &af).await;
    let reply = edit(
        &mut av,
        &ah,
        2,
        &af["state_rev"],
        json!({"layers":{"mode":"only","pairs":[parent]}}),
        "error",
    )
    .await;
    assert_eq!(reply["code"], "forbidden");
    let reply = edit(
        &mut av,
        &ah,
        3,
        &af["state_rev"],
        json!({"layers":{"mode":"all"}}),
        "accepted",
    )
    .await;
    assert_eq!(reply["state_rev"], af["state_rev"]);
    h.shutdown().await;
    closed(&mut av).await;
    std::fs::remove_file(deck).unwrap();
    println!("RUST LOCAL EXPLORE DECK SCOPE: ALL OK (normalize parent before grant check, all means granted child)");
}

pub(super) async fn explore_frame(ws: &mut Socket) -> (Value, Vec<u8>) {
    explore_frame_with_query(ws, true).await
}
async fn explore_frame_with_query(ws: &mut Socket, query: bool) -> (Value, Vec<u8>) {
    loop {
        match next(ws).await {
            Message::Binary(b) => {
                let n = u32::from_le_bytes(b[..4].try_into().unwrap()) as usize;
                let h: Value = serde_json::from_slice(&b[4..4 + n]).unwrap();
                assert_eq!(h["query"], query);
                assert_eq!(h["query_scene"]["complete"], query);
                assert!(h.get("perf").is_none());
                assert_eq!(h["payload_length"], (b.len() - 4 - n).to_string());
                return (h, b[4 + n..].to_vec());
            }
            Message::Text(t) => {
                let s: Value = serde_json::from_str(&t).unwrap();
                assert_eq!(s["type"], "share.state");
                assert!(s["failure"].is_null(), "{s}");
                for key in [
                    "layers",
                    "minimap",
                    "title",
                    "styles",
                    "capabilities",
                    "drc",
                ] {
                    assert!(s.get(key).is_none(), "explore state leaked {key}");
                }
            }
            m => panic!("unexpected explore message {m:?}"),
        }
    }
}
pub(super) async fn edit(
    ws: &mut Socket,
    hello: &Value,
    seq: u64,
    base: &Value,
    body: Value,
    kind: &str,
) -> Value {
    ws.send(Message::Text(
        json!({"type":"explore.set","seq":seq.to_string(),
        "connection_epoch":hello["connection_epoch"],"view_id":hello["view_id"],
        "base_state_rev":base,"body":body})
        .to_string()
        .into(),
    ))
    .await
    .unwrap();
    until_reply(ws, seq, kind).await
}
async fn revoke(h: &Harness, owner: &Login, id: &str) {
    assert_eq!(
        h.http(
            "DELETE",
            &format!("/api/v1/shares/{id}"),
            &headers(&format!("http://{}", h.addr), owner),
            ""
        )
        .await
        .0,
        204
    );
}
async fn usage(h: &Harness, expected: Usage) {
    let deadline = Instant::now() + Duration::from_secs(4);
    while h.resources.usage() != expected {
        assert!(
            Instant::now() < deadline,
            "worker reservation leak {:?} != {expected:?}",
            h.resources.usage()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
#[ignore = "run tools/validate_view_stream.py with a private synthetic fixture"]
async fn local_explore_views_are_independent_and_reconnect_without_new_worker() {
    for raw in [true, false] {
        // Test-only capacity. Production worker limit remains two.
        let h = Harness::start_with_limits(
            raw,
            false,
            false,
            None,
            false,
            true,
            Limits {
                workers: 3,
                ..Limits::default()
            },
        )
        .await;
        let owner = h.login().await;
        let (mut ws, oh, _) = h.connect(&owner).await;
        let (of, _) = frame(&mut ws).await;
        ack(&mut ws, &oh, 1, &of).await;
        let before = h.controller.snapshot();
        let baseline = h.resources.usage();
        let a = invite(&h, &owner, "explore").await;
        let b = invite(&h, &owner, "explore").await;
        let ac = exchange(&h, &a).await;
        let bc = exchange(&h, &b).await;
        let aid = a["share_id"].as_str().unwrap();
        let bid = b["share_id"].as_str().unwrap();
        let (mut av, ah) = guest_connect(&h, aid, &ac, "explore").await;
        let (af, original) = explore_frame(&mut av).await;
        ack(&mut av, &ah, 1, &af).await;
        let (mut bv, bh) = guest_connect(&h, bid, &bc, "explore").await;
        let (bf, other) = explore_frame(&mut bv).await;
        ack(&mut bv, &bh, 1, &bf).await;
        assert_eq!(original, other);
        assert_ne!(ah["view_id"], oh["view_id"]);
        assert_ne!(ah["view_id"], bh["view_id"]);
        assert_ne!(af["worker_epoch"], bf["worker_epoch"]);
        assert_ne!(af["worker_epoch"], of["worker_epoch"]);
        assert_eq!(
            h.resources.usage(),
            Usage {
                cpu_slots: baseline.cpu_slots + 4,
                workers: baseline.workers + 2,
                decoded_mb: baseline.decoded_mb + 256,
                ..baseline
            }
        );
        let moved = edit(
            &mut av,
            &ah,
            2,
            &af["state_rev"],
            json!({"navigation":{"kind":"pan","x":0.25,"y":0.0,"snap":true}}),
            "accepted",
        )
        .await;
        let (af2, _) = explore_frame(&mut av).await;
        assert_eq!(af2["state_rev"], moved["state_rev"]);
        assert_ne!(af2["bbox_dbu"], af["bbox_dbu"]);
        ack(&mut av, &ah, 3, &af2).await;
        let edited = edit(
            &mut bv,
            &bh,
            2,
            &bf["state_rev"],
            json!({"navigation":{"kind":"zoom","factor":0.5,"anchor":[0.5,0.5]},
                "depth":"full","detail":"exact","thin":"keep","frames":false,"labels":false}),
            "accepted",
        )
        .await;
        let (bf2, _) = explore_frame(&mut bv).await;
        assert_eq!(bf2["state_rev"], edited["state_rev"]);
        assert_ne!(bf2["bbox_dbu"], af2["bbox_dbu"]);
        ack(&mut bv, &bh, 3, &bf2).await;
        // Stale CAS affects neither this guest nor the owner/sibling.
        let conflict = edit(
            &mut av,
            &ah,
            4,
            &af["state_rev"],
            json!({"mono":true}),
            "error",
        )
        .await;
        assert_eq!(conflict["code"], "conflict");
        assert_eq!(h.controller.snapshot().state_rev, before.state_rev);
        assert_eq!(h.controller.snapshot().submitted, before.submitted);
        let reserved = h.resources.usage();
        av.close(None).await.unwrap();
        closed(&mut av).await;
        let deadline = Instant::now() + Duration::from_secs(2);
        while h.gate.transport_usage().guest_sockets != 1 {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let (mut again, ah2) = guest_connect(&h, aid, &ac, "explore").await;
        let (resumed, _) = explore_frame(&mut again).await;
        assert_eq!(ah2["view_id"], ah["view_id"]);
        assert_ne!(ah2["connection_epoch"], ah["connection_epoch"]);
        assert_eq!(resumed["bbox_dbu"], af2["bbox_dbu"]);
        assert_eq!(resumed["worker_epoch"], af2["worker_epoch"]);
        assert_eq!(h.resources.usage(), reserved);
        revoke(&h, &owner, aid).await;
        closed(&mut again).await;
        revoke(&h, &owner, bid).await;
        closed(&mut bv).await;
        usage(&h, baseline).await;
        h.shutdown().await;
    }
    println!("RUST LOCAL EXPLORE ISOLATION: ALL OK (raw/PNG, owner+two views, independent workers/camera/state, CAS, reconnect, reap)");
}

#[tokio::test]
#[ignore = "run tools/validate_view_stream.py with a private synthetic fixture"]
async fn local_explore_admission_and_scope_are_not_authority_expansion() {
    use floe_app_core::view::Patch;
    use floe_worker_client::Layers;
    let h = Harness::start_with_access(true, false, false, None, false, true).await;
    let owner = h.login().await;
    let (mut ws, oh, _) = h.connect(&owner).await;
    let (of, _) = frame(&mut ws).await;
    ack(&mut ws, &oh, 1, &of).await;
    let s = h.controller.snapshot();
    let planes: Vec<_> = s.state.styles.iter().map(|s| s.layer).collect();
    assert!(planes.len() > 1);
    h.controller
        .edit(
            s.state_rev,
            Patch {
                layers: Some(Layers::Only(vec![planes[0]])),
                ..Default::default()
            },
        )
        .unwrap();
    let (limited, _) = frame(&mut ws).await;
    ack(&mut ws, &oh, 2, &limited).await;
    let baseline = h.resources.usage();
    let a = invite(&h, &owner, "explore").await;
    let b = invite(&h, &owner, "explore").await;
    let ac = exchange(&h, &a).await;
    let bc = exchange(&h, &b).await;
    let aid = a["share_id"].as_str().unwrap();
    let bid = b["share_id"].as_str().unwrap();
    let (mut av, ah) = guest_connect(&h, aid, &ac, "explore").await;
    let (af, pixels) = explore_frame(&mut av).await;
    ack(&mut av, &ah, 1, &af).await;
    denied(guest_request(&h, bid, &bc), 429).await;
    let denied_edit = edit(
        &mut av,
        &ah,
        2,
        &af["state_rev"],
        json!({"layers":{"mode":"only","pairs":[planes[1]]}}),
        "error",
    )
    .await;
    assert_eq!(denied_edit["code"], "forbidden");
    let hidden = edit(
        &mut av,
        &ah,
        3,
        &af["state_rev"],
        json!({"layers":{"mode":"none"}}),
        "accepted",
    )
    .await;
    let (none, _) = explore_frame(&mut av).await;
    ack(&mut av, &ah, 4, &none).await;
    edit(
        &mut av,
        &ah,
        5,
        &hidden["state_rev"],
        json!({"layers":{"mode":"all"}}),
        "accepted",
    )
    .await;
    let (restored, bytes) = explore_frame(&mut av).await;
    assert_eq!(pixels, bytes); // all means granted plane, not whole dataset
    ack(&mut av, &ah, 6, &restored).await;
    assert_eq!(
        h.controller.snapshot().state.layers,
        Layers::Only(vec![planes[0]])
    );
    // Unknown owner-only operation closes this connection, not the grant/owner.
    av.send(Message::Text(
        json!({"type":"view.set","seq":"7","body":{}})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
    closed(&mut av).await;
    revoke(&h, &owner, aid).await;
    usage(&h, baseline).await;
    let (mut bv, bh) = guest_connect(&h, bid, &bc, "explore").await;
    let (bf, _) = explore_frame(&mut bv).await;
    ack(&mut bv, &bh, 1, &bf).await;
    // Owner scope change retires a running independently rendered guest.
    let s = h.controller.snapshot();
    h.controller
        .edit(
            s.state_rev,
            Patch {
                layers: Some(Layers::All),
                ..Default::default()
            },
        )
        .unwrap();
    closed(&mut bv).await;
    denied(guest_request(&h, bid, &bc), 401).await;
    usage(&h, baseline).await;
    h.shutdown().await;
    println!("RUST LOCAL EXPLORE SCOPE: ALL OK (same-manager busy/retry, granted layers/all, owner command denial, scope revoke)");
}

#[tokio::test]
#[ignore = "run tools/validate_view_stream.py with a private synthetic fixture"]
async fn local_explore_rejects_forged_identity_and_reaps_disconnected_workers() {
    let h = Harness::start_with_access(true, false, false, None, false, true).await;
    let owner = h.login().await;
    let (mut ws, oh, _) = h.connect(&owner).await;
    let (of, _) = frame(&mut ws).await;
    ack(&mut ws, &oh, 1, &of).await;
    let baseline = h.resources.usage();
    let a = invite(&h, &owner, "explore").await;
    let ac = exchange(&h, &a).await;
    let aid = a["share_id"].as_str().unwrap();
    let mut previous = Value::Null;
    for attack in 0..5 {
        let (mut av, ah) = guest_connect(&h, aid, &ac, "explore").await;
        let (af, _) = explore_frame(&mut av).await;
        ack(&mut av, &ah, 1, &af).await;
        let mut control = json!({"type":"explore.set","seq":"2","connection_epoch":ah["connection_epoch"],
            "view_id":ah["view_id"],"base_state_rev":af["state_rev"],"body":{"mono":true}});
        match attack {
            0 => control["view_id"] = oh["view_id"].clone(),
            1 => control["connection_epoch"] = previous["connection_epoch"].clone(),
            2 => control["body"] = json!({"styles":[]}),
            3 => control["body"] = json!({"layers":null}),
            _ => control["type"] = json!("view.query"),
        }
        av.send(Message::Text(control.to_string().into()))
            .await
            .unwrap();
        closed(&mut av).await;
        guest_drained(&h).await;
        assert_eq!(
            h.controller.snapshot().state_rev.to_string(),
            of["state_rev"]
        );
        previous = ah;
    }
    let (mut av, ah) = guest_connect(&h, aid, &ac, "explore").await;
    let (af, _) = explore_frame(&mut av).await;
    ack(&mut av, &ah, 1, &af).await;
    let moved = edit(
        &mut av,
        &ah,
        2,
        &af["state_rev"],
        json!({"navigation":{"kind":"pan","x":0.1,"y":0.1,"snap":true}}),
        "accepted",
    )
    .await;
    let (saved, _) = explore_frame(&mut av).await;
    assert_eq!(saved["state_rev"], moved["state_rev"]);
    av.close(None).await.unwrap();
    closed(&mut av).await;
    guest_drained(&h).await;
    let begin = Instant::now();
    let mut seq = 2u64;
    // Exercise the real maintenance timer, not a shortened product timeout.
    // The owner remains active; guest sockets must NOT keep it alive for us.
    while h.resources.usage() != baseline {
        assert!(
            begin.elapsed() < Duration::from_secs(70),
            "idle worker was not reaped"
        );
        ws.send(Message::Text(
            json!({"type":"ping","seq":seq.to_string()})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
        until_reply(&mut ws, seq, "pong").await;
        seq += 1;
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(begin.elapsed() >= Duration::from_secs(59));
    let (mut resumed, rh) = guest_connect(&h, aid, &ac, "explore").await;
    let (rf, _) = explore_frame(&mut resumed).await;
    assert_ne!(rh["view_id"], ah["view_id"]);
    assert_ne!(rf["worker_epoch"], saved["worker_epoch"]);
    assert_eq!(rf["bbox_dbu"], saved["bbox_dbu"]);
    // Shutdown must stop a newly reopened worker and an unacknowledged frame.
    h.shutdown().await;
    closed(&mut resumed).await;
    println!("RUST LOCAL EXPLORE LIFETIME: ALL OK (foreign IDs/epoch, strict DTO, 60s idle reap, saved camera/new worker, shutdown)");
}
