//! Real owner queries use the same synthetic geometry/summary as native gates.
include!("support/view_harness.rs");

fn body(state: &Value, header: &Value, world: [f64; 2], operation: Value, layers: Value) -> Value {
    let b: Vec<f64> = state["bbox_dbu"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap().parse().unwrap())
        .collect();
    json!({"anchor":{"dataset_revision":state["dataset_revision"],"worker_epoch":state["worker_epoch"],
        "frame_id":header["frame_id"],"state_rev":state["state_rev"],"render_rev":state["render_rev"],"render_key":state["render_key"]},
        "operation":operation,"position":[(world[0]-b[0])/(b[2]-b[0]),(b[3]-world[1])/(b[3]-b[1])],
        "radius_px":1.,"layers":layers})
}
async fn send_query(ws: &mut Socket, hello: &Value, seq: u64, body: Value) {
    ws.send(Message::Text(
        json!({"type":"view.query","seq":seq.to_string(),"view_id":hello["view_id"],
        "connection_epoch":hello["connection_epoch"],"body":body})
        .to_string()
        .into(),
    ))
    .await
    .unwrap();
}
async fn answer(ws: &mut Socket, hello: &Value, seq: u64, body: Value) -> Value {
    let anchor = body["anchor"].clone();
    send_query(ws, hello, seq, body).await;
    let accepted = until_reply(ws, seq, "query.accepted").await;
    let result = until_reply(ws, seq, "query.result").await;
    assert_eq!(result["query_id"], accepted["query_id"]);
    assert_eq!(result["anchor"], anchor);
    assert_eq!(result["connection_epoch"], hello["connection_epoch"]);
    assert_eq!(result["view_id"], hello["view_id"]);
    assert!(!result.to_string().contains("/private/"));
    assert!(!result.to_string().contains("/Users/"));
    result
}
async fn edit(ws: &mut Socket, hello: &Value, seq: u64, state: &Value, patch: Value) -> Value {
    ws.send(Message::Text(json!({"type":"view.set","seq":seq.to_string(),"connection_epoch":hello["connection_epoch"],
        "view_id":hello["view_id"],"base_state_rev":state["state_rev"],"body":patch}).to_string().into())).await.unwrap();
    let accepted = until_reply(ws, seq, "accepted").await;
    loop {
        let s = next_json(ws).await;
        if s["type"] == "snapshot" && s["state_rev"] == accepted["state_rev"] {
            return s;
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn measure(
    ws: &mut Socket,
    hello: &Value,
    seq: u64,
    query: &Value,
    start: Value,
    free: bool,
    snap: Value,
    reply_type: &str,
) -> Value {
    ws.send(Message::Text(json!({"type":"view.measure","seq":seq.to_string(),"view_id":hello["view_id"],
        "connection_epoch":hello["connection_epoch"],"body":{"anchor":query["anchor"],"position":query["position"],
        "start_dbu":start,"free_angle":free,"snap_query":snap}}).to_string().into())).await.unwrap();
    until_reply(ws, seq, reply_type).await
}

#[tokio::test]
#[ignore = "run tools/validate_worker_queries.py with its private fixture"]
async fn owner_rulers_use_native_snap_and_rust_coordinates_without_rendering() {
    let h = Harness::start(true).await;
    let login = h.login().await;
    let (mut ws, hello, state) = h.connect(&login).await;
    let (f, _) = frame(&mut ws).await;
    let query = body(
        &state,
        &f,
        [1., 1.],
        json!({"kind":"snap"}),
        json!({"mode":"all"}),
    );
    assert_eq!(
        measure(
            &mut ws,
            &hello,
            1,
            &query,
            Value::Null,
            false,
            Value::Null,
            "error"
        )
        .await["code"],
        "frame_not_displayed"
    );
    ack(&mut ws, &hello, 2, &f).await;
    let raw = measure(
        &mut ws,
        &hello,
        3,
        &query,
        Value::Null,
        false,
        Value::Null,
        "measure.result",
    )
    .await;
    assert_eq!(raw["point_dbu"], json!(["1", "1"]));
    assert!(raw["segment"].is_null());
    let snapped = answer(&mut ws, &hello, 4, query.clone()).await;
    let first = measure(
        &mut ws,
        &hello,
        5,
        &query,
        Value::Null,
        false,
        snapped["query_id"].clone(),
        "measure.result",
    )
    .await;
    assert_eq!(first["point_dbu"], json!(["0", "0"]));
    assert_eq!(first["snap"], "vertex");
    let second = body(
        &state,
        &f,
        [10001., 10001.],
        json!({"kind":"snap"}),
        json!({"mode":"all"}),
    );
    let snap2 = answer(&mut ws, &hello, 6, second.clone()).await;
    let renders = h.controller.snapshot().submitted;
    let end = measure(
        &mut ws,
        &hello,
        7,
        &second,
        first["point_dbu"].clone(),
        false,
        snap2["query_id"].clone(),
        "measure.result",
    )
    .await;
    assert_eq!(
        end["segment"]["endpoints_dbu"],
        json!([["0", "0"], ["10000", "0"]])
    );
    assert_eq!(end["segment"]["distance_um"], "10");
    let free = measure(
        &mut ws,
        &hello,
        8,
        &second,
        first["point_dbu"].clone(),
        true,
        snap2["query_id"].clone(),
        "measure.result",
    )
    .await;
    let distance: f64 = free["segment"]["distance_um"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((distance - 200f64.sqrt()).abs() < 1e-12);
    assert_eq!(h.controller.snapshot().submitted, renders);
    // Another owner socket cannot borrow this connection's snap ticket.
    let (mut other, other_hello, other_state) = h.connect(&login).await;
    let other_frame = frame(&mut other).await.0;
    ack(&mut other, &other_hello, 1, &other_frame).await;
    let q = body(
        &other_state,
        &other_frame,
        [10001., 10001.],
        json!({"kind":"snap"}),
        json!({"mode":"all"}),
    );
    assert_eq!(
        measure(
            &mut other,
            &other_hello,
            2,
            &q,
            Value::Null,
            false,
            snap2["query_id"].clone(),
            "error"
        )
        .await["code"],
        "stale_snap"
    );
    assert_eq!(
        measure(
            &mut ws,
            &hello,
            9,
            &query,
            Value::Null,
            false,
            snapped["query_id"].clone(),
            "error"
        )
        .await["code"],
        "stale_snap"
    );
    let state = edit(
        &mut ws,
        &hello,
        10,
        &state,
        json!({"navigation":{"kind":"pan","x":0.1,"y":0.,"snap":true}}),
    )
    .await;
    assert_eq!(
        measure(
            &mut ws,
            &hello,
            11,
            &second,
            Value::Null,
            false,
            Value::Null,
            "error"
        )
        .await["code"],
        "stale_frame"
    );
    let f = frame(&mut ws).await.0;
    ack(&mut ws, &hello, 12, &f).await;
    let cursor = body(
        &state,
        &f,
        [7500.25, 4999.5],
        json!({"kind":"snap"}),
        json!({"mode":"all"}),
    );
    let r = measure(
        &mut ws,
        &hello,
        13,
        &cursor,
        json!(["0", "0"]),
        true,
        Value::Null,
        "measure.result",
    )
    .await;
    assert_eq!(r["point_dbu"], json!(["7500.25", "4999.5"]));
    h.shutdown().await;
    println!("WEB MANUAL RULERS: ALL OK");
}

#[tokio::test]
#[ignore = "run tools/validate_worker_queries.py with its private fixture"]
async fn owner_queries_require_displayed_receipts_and_preserve_typed_geometry() {
    let h = Harness::start(true).await;
    assert_eq!(h.http("GET", "/api/v1/view", &[], "").await.0, 401);
    let login = h.login().await;
    let (mut ws, hello, mut state) = h.connect(&login).await;
    assert_eq!(state["capabilities"]["query"], true);
    let (mut f, _) = frame(&mut ws).await;
    let pick = |s: &Value, f: &Value, nth: i64| {
        body(
            s,
            f,
            [5000., 5000.],
            json!({"kind":"pick","nth":nth.to_string()}),
            json!({"mode":"all"}),
        )
    };
    send_query(&mut ws, &hello, 1, pick(&state, &f, 0)).await;
    assert_eq!(
        until_reply(&mut ws, 1, "error").await["code"],
        "frame_not_displayed"
    );
    assert_eq!(h.controller.query_snapshot().accepted, 0);
    ack(&mut ws, &hello, 2, &f).await;
    for (seq, nth) in [(3, 0), (4, 1), (5, -1), (6, i64::MIN)] {
        let r = answer(&mut ws, &hello, seq, pick(&state, &f, nth)).await;
        assert_eq!(r["status"], "ok");
        assert_eq!(r["hit"]["count"], "2");
        assert_eq!(r["hit"]["pair"], json!([7 + nth.rem_euclid(2), 0]));
        assert_eq!(r["hit"]["cell_name"], "TOP_QUERY");
        assert_eq!(r["hit"]["points_truncated"], false);
        if nth == 0 {
            assert_eq!(r["hit"]["area_dbu2"], "100000000");
        }
    }
    let r = answer(
        &mut ws,
        &hello,
        7,
        body(
            &state,
            &f,
            [1., 1.],
            json!({"kind":"snap"}),
            json!({"mode":"all"}),
        ),
    )
    .await;
    assert_eq!(r["hit"]["point_dbu"], json!(["0", "0"]));
    assert_eq!(r["hit"]["snap"], "vertex");
    let r = answer(
        &mut ws,
        &hello,
        8,
        body(
            &state,
            &f,
            [35000., 5000.],
            json!({"kind":"pick","nth":"0"}),
            json!({"mode":"all"}),
        ),
    )
    .await;
    assert_eq!(r["hit"]["points_truncated"], true);
    assert_eq!(r["hit"]["points_dbu"].as_array().unwrap().len(), 512);
    let renders = h.controller.snapshot().submitted;
    ws.send(Message::Text(
        json!({"type":"view.query.cancel","seq":"9","connection_epoch":hello["connection_epoch"],
        "view_id":hello["view_id"],"kind":"snap"})
        .to_string()
        .into(),
    ))
    .await
    .unwrap();
    until_reply(&mut ws, 9, "query.cancelled").await;
    assert!(h.controller.query_snapshot().snap_id.is_none());
    assert!(h.controller.query_snapshot().pick_id.is_some());
    assert_eq!(h.controller.snapshot().submitted, renders);
    let stale = pick(&state, &f, 0);
    state = edit(
        &mut ws,
        &hello,
        10,
        &state,
        json!({"layers":{"mode":"only","pairs":[[7,0]]}}),
    )
    .await;
    f = frame(&mut ws).await.0;
    ack(&mut ws, &hello, 11, &f).await;
    send_query(&mut ws, &hello, 12, stale).await;
    assert!(matches!(
        until_reply(&mut ws, 12, "error").await["code"].as_str(),
        Some("frame_not_displayed" | "stale_frame")
    ));
    let mut hidden = pick(&state, &f, 0);
    hidden["layers"] = json!({"mode":"only","pairs":[[8,0]]});
    send_query(&mut ws, &hello, 13, hidden).await;
    assert_eq!(
        until_reply(&mut ws, 13, "error").await["code"],
        "invalid_request"
    );
    let r = answer(&mut ws, &hello, 14, pick(&state, &f, 0)).await;
    assert_eq!(r["hit"]["count"], "1");
    // Mixed summary: whole scene refuses, exact layer 7 remains queryable.
    state = edit(
        &mut ws,
        &hello,
        15,
        &state,
        json!({"navigation":{"kind":"goto","center_um":["0","0"],"width_um":"2048"},
        "pixels":[256,256],"detail":"high","thin":"keep","layers":{"mode":"all"}}),
    )
    .await;
    f = frame(&mut ws).await.0;
    assert_eq!(f["query_scene"]["summary_layers"], "1");
    ack(&mut ws, &hello, 16, &f).await;
    let r = answer(&mut ws, &hello, 17, pick(&state, &f, 0)).await;
    assert_eq!(r["status"], "scene_summary");
    assert!(r["hit"].is_null());
    let mut exact = pick(&state, &f, 0);
    exact["layers"] = json!({"mode":"only","pairs":[[7,0]]});
    let r = answer(&mut ws, &hello, 18, exact).await;
    assert_eq!(r["status"], "ok");
    assert_eq!(r["hit"]["pair"], json!([7, 0]));
    let mut empty = pick(&state, &f, 0);
    empty["layers"] = json!({"mode":"none"});
    let r = answer(&mut ws, &hello, 19, empty).await;
    assert_eq!(r["status"], "ok");
    assert!(r["hit"].is_null());
    h.shutdown().await;
    println!("WEB QUERY GEOMETRY: ALL OK");
}

#[tokio::test]
#[ignore = "run tools/validate_worker_queries.py with its private fixture"]
async fn margin_queries_ignore_image_credit_and_reconnect_cannot_reuse_receipts() {
    let h = Harness::start_configured(true, true, true).await;
    let login = h.login().await;
    let (mut ws, hello, state) = h.connect(&login).await;
    let (fg, _) = frame(&mut ws).await;
    ack(&mut ws, &hello, 1, &fg).await;
    let (margin, _) = frame(&mut ws).await; // Deliberately withhold its image ACK.
    let q = body(
        &state,
        &fg,
        [1., 1.],
        json!({"kind":"snap"}),
        json!({"mode":"all"}),
    );
    let r = answer(&mut ws, &hello, 2, q).await;
    assert_eq!(r["status"], "ok");
    assert_eq!(r["scene"], margin["query_scene"]);
    assert!(h.gate.transport_usage().reserved_output_bytes > 0);
    ack(&mut ws, &hello, 3, &margin).await;
    let state = edit(
        &mut ws,
        &hello,
        4,
        &state,
        json!({"navigation":{"kind":"pan","x":0.1,"y":0.,"snap":true}}),
    )
    .await;
    let r = answer(
        &mut ws,
        &hello,
        5,
        body(
            &state,
            &margin,
            [7500., 5000.], // Still inside the viewport after the snapped pan.
            json!({"kind":"pick","nth":"0"}),
            json!({"mode":"all"}),
        ),
    )
    .await;
    assert_eq!(r["status"], "ok");
    assert_eq!(h.controller.snapshot().submitted, 2);
    // Two authenticated owner sockets still have independent display receipts.
    let (mut fresh, fresh_hello, fresh_state) = h.connect(&login).await;
    let (restored, _) = frame(&mut fresh).await;
    let q = body(
        &fresh_state,
        &restored,
        [7500., 5000.],
        json!({"kind":"pick","nth":"0"}),
        json!({"mode":"all"}),
    );
    send_query(&mut fresh, &fresh_hello, 1, q.clone()).await;
    assert_eq!(
        until_reply(&mut fresh, 1, "error").await["code"],
        "frame_not_displayed"
    );
    ack(&mut fresh, &fresh_hello, 2, &restored).await;
    let result = answer(&mut fresh, &fresh_hello, 3, q.clone()).await;
    let current = h.controller.query_snapshot().pick_id;
    ws.close(None).await.unwrap();
    closed(&mut ws).await;
    assert_eq!(h.controller.query_snapshot().pick_id, current);
    // Wrong epoch is a protocol error, never silently rebound to the new socket.
    fresh
        .send(Message::Text(
            json!({"type":"view.query","seq":"4","view_id":fresh_hello["view_id"],
        "connection_epoch":hello["connection_epoch"],"body":q})
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
    closed(&mut fresh).await;
    assert_eq!(result["connection_epoch"], fresh_hello["connection_epoch"]);
    assert!(!h.controller.is_finished());
    h.shutdown().await;
    println!("WEB QUERY LIFECYCLE: ALL OK");
}

#[tokio::test]
#[ignore = "run tools/validate_worker_queries.py with its private fixture"]
async fn discarded_forged_and_stale_queries_never_reach_the_native_worker() {
    let h = Harness::start(true).await;
    let login = h.login().await;
    for mode in [
        "discarded",
        "frame",
        "worker",
        "revision",
        "point",
        "radius",
        "view",
        "epoch",
        "unknown",
        "duplicate",
    ] {
        let (mut ws, hello, state) = h.connect(&login).await;
        let (f, _) = frame(&mut ws).await;
        ws.send(Message::Text(json!({"type":"frame.ack","seq":"1","connection_epoch":hello["connection_epoch"],
            "frame_id":f["frame_id"],"disposition":if mode=="discarded" {"discarded"} else {"displayed"}}).to_string().into())).await.unwrap();
        let mut q = body(
            &state,
            &f,
            [5000., 5000.],
            json!({"kind":"snap"}),
            json!({"mode":"all"}),
        );
        match mode {
            "frame" => q["anchor"]["frame_id"] = json!(u64::MAX.to_string()),
            "worker" => q["anchor"]["worker_epoch"] = json!(u64::MAX.to_string()),
            "revision" => q["anchor"]["state_rev"] = json!(u64::MAX.to_string()),
            "point" => q["position"] = json!([-0.01, 0.5]),
            "radius" => q["radius_px"] = json!(65),
            "unknown" => q["path"] = json!("/must/not/be/read"),
            _ => (),
        }
        let mut message = json!({"type":"view.query","seq":"2","view_id":hello["view_id"],
            "connection_epoch":hello["connection_epoch"],"body":q});
        if mode == "view" {
            message["view_id"] = json!("not-this-view");
        }
        if mode == "epoch" {
            message["connection_epoch"] = json!("not-this-connection");
        }
        let mut text = message.to_string();
        if mode == "duplicate" {
            text = text.replacen(
                "\"radius_px\":1.0",
                "\"radius_px\":1.0,\"radius_px\":1.0",
                1,
            );
            assert_ne!(text, message.to_string());
        }
        ws.send(Message::Text(text.into())).await.unwrap();
        match mode {
            "view" | "epoch" | "unknown" | "duplicate" => closed(&mut ws).await,
            _ => {
                let error = until_reply(&mut ws, 2, "error").await;
                assert_eq!(
                    error["code"],
                    match mode {
                        "point" | "radius" => "invalid_request",
                        "revision" => "stale_frame",
                        _ => "frame_not_displayed",
                    }
                );
                ws.close(None).await.unwrap();
                closed(&mut ws).await;
            }
        }
        assert_eq!(h.controller.query_snapshot().accepted, 0, "{mode}");
        assert!(!h.controller.is_finished());
    }
    h.shutdown().await;
    println!("WEB QUERY REJECTIONS: ALL OK");
}
