//! Real HTTP/WS + native daemon. The launcher gate supplies private valmini.
include!("support/view_harness.rs");

#[tokio::test]
#[ignore = "run tools/validate_view_stream.py with a private synthetic fixture"]
async fn native_palette_batch_is_one_revision_and_pixel_reversible() {
    use floe_worker_client::Layers;
    let h = Harness::start(true).await;
    let login = h.login().await;
    let (mut ws, hello, _) = h.connect(&login).await;
    let (first, original) = frame(&mut ws).await;
    ack(&mut ws, &hello, 1, &first).await;
    let pairs: Vec<_> = h.controller.model.styles.iter().map(|s| s.layer).collect();
    assert!(pairs.len() >= 2);
    assert!(
        original[16..].chunks_exact(4).any(|p| p[..3] != [0, 0, 0]),
        "fixture must paint before batch hide"
    );
    let viewport = h.controller.snapshot().state.viewport;
    for (i, action) in ["hide", "show", "toggle", "toggle", "show"]
        .into_iter()
        .enumerate()
    {
        let before = h.controller.snapshot();
        let seq = 2 + i as u64 * 2;
        ws.send(Message::Text(json!({"type":"view.set","seq":seq.to_string(),"view_id":hello["view_id"],
            "connection_epoch":hello["connection_epoch"],"base_state_rev":before.state_rev.to_string(),
            "body":{"layer_batch":{"action":action,"pairs":pairs}}}).to_string().into())).await.unwrap();
        until_reply(&mut ws, seq, "accepted").await;
        let after = h.controller.snapshot();
        assert_eq!(after.state.viewport, viewport);
        if i == 4 {
            assert_eq!(after.state_rev, before.state_rev);
            assert_eq!(
                after.submitted, before.submitted,
                "no-op batch submitted a render"
            );
        } else {
            assert_eq!(after.state_rev, before.state_rev + 1);
            assert_eq!(after.render_rev, before.render_rev + 1);
            let (header, pixels) = frame(&mut ws).await;
            assert_eq!(header["state_rev"], after.state_rev.to_string());
            assert_eq!(
                h.controller.snapshot().submitted,
                before.submitted + 1,
                "one frame for the whole batch"
            );
            if i % 2 == 1 {
                assert!(
                    pixels == original,
                    "batch restore must reproduce original pixels"
                );
                assert_eq!(after.state.layers, Layers::All);
            } else {
                assert!(pixels != original, "batch hide must change pixels");
                assert_eq!(after.state.layers, Layers::None);
            }
            ack(&mut ws, &hello, seq + 1, &header).await;
        }
    }
    let before = h.controller.snapshot();
    for (seq, base, body, code) in [
        (
            12,
            "1".to_owned(),
            json!({"layer_batch":{"action":"toggle","pairs":pairs}}),
            "stale_state",
        ),
        (
            13,
            before.state_rev.to_string(),
            json!({"layer_batch":{"action":"hide","pairs":[pairs[0],(u32::MAX,u32::MAX)]}}),
            "invalid_request",
        ),
        (
            14,
            before.state_rev.to_string(),
            json!({"layer_batch":{"action":"show","pairs":pairs},"layers":{"mode":"none"}}),
            "invalid_request",
        ),
    ] {
        ws.send(Message::Text(
            json!({"type":"view.set","seq":seq.to_string(),"view_id":hello["view_id"],
            "connection_epoch":hello["connection_epoch"],"base_state_rev":base,"body":body})
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
        assert_eq!(until_reply(&mut ws, seq, "error").await["code"], code);
        assert_eq!(h.controller.snapshot().state_rev, before.state_rev);
        assert_eq!(h.controller.snapshot().submitted, before.submitted);
    }
    h.shutdown().await;
    println!("RUST PALETTE STREAM: ALL OK (atomic batch, raw pixels, noop, stale/invalid/conflicting edits, cleanup)");
}

#[tokio::test]
#[ignore = "run tools/validate_view_stream.py with a private synthetic fixture"]
async fn native_relative_depth_is_revision_bound_and_preserves_camera() {
    let h = Harness::start(true).await;
    let login = h.login().await;
    let (mut ws, hello, _) = h.connect(&login).await;
    let (first, _) = frame(&mut ws).await;
    ack(&mut ws, &hello, 1, &first).await;
    let original = h.controller.snapshot().state.viewport;
    assert_eq!(h.controller.snapshot().max_depth, Some(2));
    for (i, (delta, expected)) in [(-1, 1), (1, 2), (1, 2), (-1, 1), (-1, 0), (-1, 0)]
        .into_iter()
        .enumerate()
    {
        let before = h.controller.snapshot();
        let seq = 2 + i as u64 * 2;
        ws.send(Message::Text(json!({"type":"view.set","seq":seq.to_string(),"view_id":hello["view_id"],"connection_epoch":hello["connection_epoch"],
            "base_state_rev":before.state_rev.to_string(),"body":{"depth_step":delta}}).to_string().into())).await.unwrap();
        let reply = until_reply(&mut ws, seq, "accepted").await;
        let after = h.controller.snapshot();
        assert_eq!(after.state.depth, Some(expected));
        assert_eq!(after.state.viewport, original);
        assert_eq!(reply["state_rev"], after.state_rev.to_string());
        if before.state_rev != after.state_rev {
            let (next, _) = frame(&mut ws).await;
            ack(&mut ws, &hello, seq + 1, &next).await;
        }
    }
    h.shutdown().await;
    println!(
        "RUST DEPTH STREAM: ALL OK (relative steps, full/max/zero, noop, revision and camera)"
    );
}

#[tokio::test]
#[ignore = "run tools/validate_view_stream.py with a private synthetic fixture"]
async fn native_minimap_is_readonly_and_navigation_keeps_scale() {
    let h = Harness::start(true).await;
    let login = h.login().await;
    let (mut ws, hello, state) = h.connect(&login).await;
    let (first, _) = frame(&mut ws).await;
    ack(&mut ws, &hello, 1, &first).await;
    let id = hello["view_id"].as_str().unwrap();
    let path = format!("/api/v1/views/{id}/minimap/full");
    assert_eq!(h.http("GET", &path, &[], "").await.0, 401);
    let headers = [
        ("Cookie", login.cookie.as_str()),
        ("X-Floe-CSRF", login.csrf.as_str()),
    ];
    let before = h.controller.snapshot();
    let (status, _, body) = h.http("GET", &path, &headers, "").await;
    assert_eq!(status, 200);
    let base: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(base["view_id"], id);
    assert_eq!(base["dataset_revision"], state["dataset_revision"]);
    assert_eq!(base["pixels"].as_str().unwrap().len(), 180 * 180);
    assert!(base["pixels"]
        .as_str()
        .unwrap()
        .bytes()
        .all(|b| (b'0'..=b'3').contains(&b)));
    assert_eq!(
        h.controller.snapshot().submitted,
        before.submitted,
        "overview GET rendered"
    );
    assert_eq!(h.controller.snapshot().state_rev, before.state_rev);
    for suffix in ["00", "32", "999", "garbage"] {
        assert_eq!(
            h.http(
                "GET",
                &format!("/api/v1/views/{id}/minimap/{suffix}"),
                &headers,
                ""
            )
            .await
            .0,
            404
        );
    }
    assert_eq!(
        h.http(
            "GET",
            "/api/v1/views/not-current/minimap/full",
            &headers,
            ""
        )
        .await
        .0,
        404
    );
    ws.send(Message::Text(json!({"type":"view.set","seq":"2","view_id":id,"connection_epoch":hello["connection_epoch"],"base_state_rev":"1",
        "body":{"navigation":{"kind":"minimap","point":[60,60]}}}).to_string().into())).await.unwrap();
    assert_eq!(until_reply(&mut ws, 2, "accepted").await["state_rev"], "2");
    let (next, _) = frame(&mut ws).await;
    ack(&mut ws, &hello, 3, &next).await;
    let after = h.controller.snapshot();
    let a = before.state.viewport.bbox;
    let b = after.state.viewport.bbox;
    assert_eq!(next["render_key"], first["render_key"]);
    let spp = (a[2] - a[0]) / f64::from(before.state.viewport.width);
    for i in 0..2 {
        let n = (b[i] - a[i]) / spp / 16.;
        assert!((n - n.round()).abs() < 1e-8);
    }
    assert!(((b[2] - b[0]) / (a[2] - a[0]) - 1.).abs() < 1e-12);
    assert!(((b[3] - b[1]) / (a[3] - a[1]) - 1.).abs() < 1e-12);
    let (mut reconnect, _, restored) = h.connect(&login).await;
    assert_eq!(restored["minimap"]["base"], "full");
    assert!(!restored["minimap"]["marks"].as_array().unwrap().is_empty());
    let _ = frame(&mut reconnect).await;
    h.shutdown().await;
    println!("RUST MINIMAP STREAM: ALL OK (authenticated cached bases, no render on read, 16px same-scale navigation, reconnect)");
}

#[tokio::test]
#[ignore = "run tools/validate_view_stream.py with a private synthetic fixture"]
async fn native_box_zoom_round_trip_is_pixel_exact_and_revision_checked() {
    for raw in [true, false] {
        let h = Harness::start(raw).await;
        let login = h.login().await;
        let (mut ws, hello, _) = h.connect(&login).await;
        let (first, original) = frame(&mut ws).await;
        ack(&mut ws, &hello, 1, &first).await;
        let before = h.controller.snapshot();
        for (seq, base, nav) in [
            (
                2,
                "1",
                json!({"kind":"band","start":[0.25,0.25],"end":[0.75,0.75],"axes":[true,true],"outward":false}),
            ),
            (
                4,
                "2",
                json!({"kind":"band","start":[0.75,0.5],"end":[0.25,0.5],"axes":[true,false],"outward":true}),
            ),
        ] {
            ws.send(Message::Text(json!({"type":"view.set","seq":seq.to_string(),"view_id":hello["view_id"],
                "connection_epoch":hello["connection_epoch"],"base_state_rev":base,"body":{"navigation":nav}}).to_string().into())).await.unwrap();
            assert_eq!(
                until_reply(&mut ws, seq, "accepted").await["state_rev"],
                if seq == 2 { "2" } else { "3" }
            );
            let (f, bytes) = frame(&mut ws).await;
            assert_eq!(f["render_key"], first["render_key"]);
            if seq == 4 {
                assert_eq!(bytes, original, "box zoom in/out changed native pixels");
            }
            ack(&mut ws, &hello, seq + 1, &f).await;
        }
        let restored = h.controller.snapshot().state.viewport;
        let initial = before.state.viewport;
        assert_eq!(
            (restored.width, restored.height),
            (initial.width, initial.height)
        );
        // Fit has a non-binary aspect ratio. In/out world coordinates can
        // round by a few ULPs; native PNG/raw pixels above must remain exact.
        let tolerance = 32. * f64::EPSILON * initial.bbox.iter().fold(1_f64, |n, x| n.max(x.abs()));
        for (got, expected) in restored.bbox.iter().zip(initial.bbox.iter()) {
            assert!((got - expected).abs() <= tolerance);
        }
        for (seq, base, end, code) in [
            (6, "1", 0.5, "stale_state"),
            (7, "3", 9.0, "invalid_request"),
        ] {
            ws.send(Message::Text(json!({"type":"view.set","seq":seq.to_string(),"view_id":hello["view_id"],
                "connection_epoch":hello["connection_epoch"],"base_state_rev":base,"body":{"navigation":
                {"kind":"band","start":[0.25,0.25],"end":[end,0.5],"axes":[true,true],"outward":false}}}).to_string().into())).await.unwrap();
            assert_eq!(until_reply(&mut ws, seq, "error").await["code"], code);
            assert_eq!(h.controller.snapshot().state_rev, 3);
        }
        let (mut reconnect, _, state) = h.connect(&login).await;
        assert_eq!(state["state_rev"], "3");
        let (_, bytes) = frame(&mut reconnect).await;
        assert_eq!(bytes, original);
        h.shutdown().await;
    }
    println!("RUST BAND STREAM: ALL OK (PNG/raw in-out bytes, stale/invalid rejection, reconnect)");
}

#[tokio::test]
#[ignore = "run tools/validate_view_stream.py with a private synthetic fixture"]
async fn native_frames_reconnect_credit_errors_and_shutdown() {
    for raw in [true, false] {
        let h = Harness::start(raw).await;
        assert_eq!(h.http("GET", "/api/v1/view", &[], "").await.0, 401);
        let login = h.login().await;
        let r = h
            .http(
                "GET",
                "/api/v1/view",
                &[("Cookie", &login.cookie), ("X-Floe-CSRF", &login.csrf)],
                "",
            )
            .await;
        assert_eq!(r.0, 200);
        assert!(!r.2.contains("/private/"));
        assert!(!r.2.contains("/Users/"));
        let (mut slow, hello, _) = h.connect(&login).await;
        let (first, payload) = frame(&mut slow).await;
        assert_eq!(payload, h.controller.latest().unwrap().frame.bytes);
        assert_eq!(first["format"], if raw { "raw" } else { "png" });
        assert_eq!(first["state_rev"], "1");
        assert_eq!(first["complete"], true);
        if raw {
            assert!(payload.len() > 16 * 1024);
        }
        assert!(h.gate.transport_usage().reserved_output_bytes > 0);
        // Keep image credit withheld but process 100 relative inputs at an
        // allowed rate. No additional binary message may enter this socket.
        for n in 1..=100u64 {
            slow.send(Message::Text(json!({"type":"view.set","seq":n.to_string(),"view_id":hello["view_id"],"connection_epoch":hello["connection_epoch"],
                "base_state_rev":n.to_string(),"body":{"navigation":{"kind":"pan","x":0.1,"y":0.0,"snap":true}}}).to_string().into())).await.unwrap();
            let accepted = until_reply(&mut slow, n, "accepted").await;
            assert_eq!(accepted["state_rev"], (n + 1).to_string());
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while h.controller.latest().is_none_or(|f| f.render_rev != 101) {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(h.controller.snapshot().consumed > 1);
        let (mut fast, fast_hello, state) = h.connect(&login).await;
        assert_eq!(state["state_rev"], "101");
        assert_ne!(fast_hello["connection_epoch"], hello["connection_epoch"]);
        let (current, bytes) = frame(&mut fast).await;
        assert_eq!(current["render_rev"], "101");
        assert_eq!(bytes, h.controller.latest().unwrap().frame.bytes);
        ack(&mut fast, &fast_hello, 1, &current).await;
        ack(&mut slow, &hello, 101, &first).await;
        let (latest, _) = frame(&mut slow).await;
        assert_eq!(latest["render_rev"], "101");
        ack(&mut slow, &hello, 102, &latest).await;
        // Conflict and invalid body must not mutate state or kill the daemon.
        for (seq, base, body, code) in [
            (103, "1", json!({"thin":"keep"}), "stale_state"),
            (104, "101", json!({"depth":"-1"}), "invalid_request"),
        ] {
            slow.send(Message::Text(json!({"type":"view.set","seq":seq.to_string(),"view_id":hello["view_id"],"connection_epoch":hello["connection_epoch"],"base_state_rev":base,"body":body}).to_string().into())).await.unwrap();
            assert_eq!(until_reply(&mut slow, seq, "error").await["code"], code);
            assert_eq!(h.controller.snapshot().state_rev, 101);
        }
        slow.close(None).await.unwrap();
        closed(&mut slow).await;
        // Reconnect restarts sequence with a fresh connection identity and
        // restores current final only. An old epoch command is rejected.
        let (mut resumed, resumed_hello, state) = h.connect(&login).await;
        assert_eq!(state["state_rev"], "101");
        assert_ne!(resumed_hello["connection_epoch"], hello["connection_epoch"]);
        let (restored, _) = frame(&mut resumed).await;
        assert_eq!(restored["frame_id"], current["frame_id"]);
        resumed.send(Message::Text(json!({"type":"view.set","seq":"1","view_id":hello["view_id"],"connection_epoch":hello["connection_epoch"],"base_state_rev":"101","body":{"thin":"keep"}}).to_string().into())).await.unwrap();
        closed(&mut resumed).await;
        assert_eq!(h.controller.snapshot().state_rev, 101);
        let (mut malformed, _, _) = h.connect(&login).await;
        malformed
            .send(Message::Binary(vec![1, 2, 3].into()))
            .await
            .unwrap();
        closed(&mut malformed).await;
        assert!(!h.controller.is_finished());
        let (mut missing_ack, _, _) = h.connect(&login).await;
        let _ = frame(&mut missing_ack).await;
        if raw {
            // The slow subscriber alone expires; rendering/control stays up.
            closed(&mut missing_ack).await;
            assert!(!h.controller.is_finished());
        }
        let status = h
            .http(
                "DELETE",
                "/api/v1/session",
                &[
                    ("Cookie", &login.cookie),
                    ("X-Floe-CSRF", &login.csrf),
                    ("Origin", &format!("http://{}", h.addr)),
                ],
                "",
            )
            .await
            .0;
        assert_eq!(status, 204);
        closed(&mut fast).await;
        h.shutdown().await;
    }
    println!("RUST VIEW STREAM: ALL OK (PNG/raw, 2x100 inputs, slow subscriber, reconnect, stale epoch, logout)");
}

#[tokio::test]
#[ignore = "run tools/validate_view_stream.py with a private synthetic fixture"]
async fn native_margin_stream_keeps_foreground_credit_and_reconnect_identity() {
    for raw in [true, false] {
        let h = Harness::start_configured(raw, true, true).await;
        let login = h.login().await;
        let (mut ws, hello, _) = h.connect(&login).await;
        let (fg, _) = frame(&mut ws).await;
        assert_eq!(fg["purpose"], "foreground");
        assert_eq!(fg["generation"], "1");
        ack(&mut ws, &hello, 1, &fg).await;
        let (margin, bytes) = frame(&mut ws).await;
        assert_eq!(margin["purpose"], "margin");
        assert_eq!(margin["complete"], true);
        assert_eq!(margin["generation"], "2");
        assert_eq!(bytes, h.controller.margin().unwrap().frame.bytes);
        assert!(h.controller.margin().unwrap().frame.request.labels);
        assert!(h.controller.snapshot().margin.unwrap().crop_safe);
        ack(&mut ws, &hello, 2, &margin).await;
        ws.send(Message::Text(
            json!({"type":"view.set","seq":"3","view_id":hello["view_id"],
            "connection_epoch":hello["connection_epoch"],"base_state_rev":"1",
            "body":{"navigation":{"kind":"pan","x":0.1,"y":0.0,"snap":true}}})
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
        assert_eq!(until_reply(&mut ws, 3, "accepted").await["render_rev"], "2");
        let deadline = Instant::now() + Duration::from_secs(3);
        while h.controller.snapshot().crop_hits != 1 {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(h.controller.snapshot().submitted, 2);
        assert!(h.controller.latest().is_none());
        let (mut resumed, rhello, state) = h.connect(&login).await;
        assert_eq!(state["render_rev"], "2");
        assert_eq!(state["margin"]["crop_safe"], true);
        let (restored, _) = frame(&mut resumed).await;
        assert_eq!(restored["purpose"], "margin");
        assert_eq!(restored["render_rev"], "1"); // Valid crop, not a stale foreground.
        assert_eq!(restored["frame_id"], margin["frame_id"]);
        assert_ne!(restored["connection_epoch"], margin["connection_epoch"]);
        ack(&mut resumed, &rhello, 1, &restored).await;
        resumed.send(Message::Text(json!({"type":"view.set","seq":"2","view_id":rhello["view_id"],
            "connection_epoch":rhello["connection_epoch"],"base_state_rev":"2","body":{"thin":"keep"}}).to_string().into())).await.unwrap();
        until_reply(&mut resumed, 2, "accepted").await;
        let (changed, _) = frame(&mut resumed).await;
        assert_eq!(changed["purpose"], "foreground");
        assert_eq!(changed["render_key"], "2");
        assert_eq!(changed["render_rev"], "3");
        ack(&mut resumed, &rhello, 3, &changed).await;
        h.shutdown().await;
    }
    println!("RUST MARGIN STREAM: ALL OK (PNG/raw + labels, credit, crop without foreground, reconnect, policy invalidation)");
}
