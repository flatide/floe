use super::*;

// Keep the wire envelope and the expected reply together in each table case.
#[allow(clippy::too_many_arguments)]
async fn edit(
    h: &Harness,
    ws: &mut Socket,
    hello: &Value,
    seq: &mut u64,
    kind: &str,
    body: Value,
    base: Option<u64>,
    expected: &str,
) -> Value {
    *seq += 1;
    ws.send(Message::Text(json!({"type":kind,"seq":seq.to_string(),
        "view_id":hello["view_id"],"connection_epoch":hello["connection_epoch"],
        "base_state_rev":base.unwrap_or(h.controller.snapshot().state_rev).to_string(),"body":body
    }).to_string().into())).await.unwrap();
    until_reply(ws, *seq, expected).await
}
async fn pixels(ws: &mut Socket, hello: &Value, seq: &mut u64) -> Vec<u8> {
    let (header, pixels) = frame(ws).await;
    *seq += 1;
    ack(ws, hello, *seq, &header).await;
    pixels
}
fn slot(name: &str, bits: u16) -> Value {
    json!({"name":name,"rows":vec![bits;16]})
}

#[tokio::test]
#[ignore = "run tools/validate_view_stream.py with a private synthetic fixture"]
async fn native_fill_slots_require_opt_in_and_preserve_reference_semantics() {
    use floe_worker_client::Fill;
    for enabled in [false, true] {
        let h = Harness::start_with_tools(true, false, false, None, enabled).await;
        let login = h.login().await;
        let (mut ws, hello, initial) = h.connect(&login).await;
        let mut seq = 0;
        pixels(&mut ws, &hello, &mut seq).await;
        let id = hello["view_id"].as_str().unwrap();
        let before = h.controller.snapshot();
        let initial_key = before.state.fill_slots_key();
        assert_eq!(initial["fill_slots_key"], initial_key);
        let url = |key: &str| format!("/api/v1/views/{id}/fill-slots/{key}");
        let headers = [
            ("Cookie", login.cookie.as_str()),
            ("X-Floe-CSRF", login.csrf.as_str()),
        ];
        assert_eq!(h.http("GET", &url(&initial_key), &[], "").await.0, 401);
        assert_eq!(
            h.http("GET", &url(&initial_key), &headers[..1], "").await.0,
            401
        );
        let mut cross = headers.to_vec();
        cross.push(("Origin", "http://other.invalid"));
        assert_eq!(h.http("GET", &url(&initial_key), &cross, "").await.0, 403);
        assert_eq!(
            h.http("GET", &url(&"0".repeat(40)), &headers, "").await.0,
            409
        );
        let (_, meta, text) = h.http("GET", &url(&initial_key), &headers, "").await;
        assert!(meta.contains("cache-control: no-store"));
        assert!(text.len() < 16 * 1024);
        let table: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(table["editable"], enabled);
        assert_eq!(table["view_id"], id);
        assert_eq!(table["fill_slots_key"], initial_key);
        assert_eq!(table["fills"].as_array().unwrap().len(), 20);
        let compiled = h
            .http("GET", "/api/v1/palette/presets", &headers, "")
            .await
            .2;
        let caps: Value =
            serde_json::from_str(&h.http("GET", "/api/v1/capabilities", &headers, "").await.2)
                .unwrap();
        assert_eq!(caps["fill_slot_edit"], enabled);
        assert_eq!(
            caps["design_defaults"], false,
            "memory edit must not enable publication"
        );
        assert_eq!(h.controller.snapshot().submitted, before.submitted);
        assert_eq!(h.controller.snapshot().state_rev, before.state_rev);
        let pairs: Vec<_> = before.state.styles.iter().map(|s| s.layer).collect();
        assert!(pairs.len() >= 2);
        let outcome = edit(
            &h,
            &mut ws,
            &hello,
            &mut seq,
            "view.fill_slot",
            slot("brick", 0),
            None,
            if enabled { "accepted" } else { "error" },
        )
        .await;
        if !enabled {
            assert_eq!(outcome["code"], "fill_edit_disabled");
            assert_eq!(h.controller.snapshot().state_rev, before.state_rev);
            assert_eq!(
                h.controller.snapshot().state.fill_slots(),
                before.state.fill_slots()
            );
            // Ordinary preset assignment does not require developer permission.
            edit(
                &h,
                &mut ws,
                &hello,
                &mut seq,
                "view.set",
                json!({"style_batch":{"pairs":pairs,"fill_slot":"brick"}}),
                None,
                "accepted",
            )
            .await;
            pixels(&mut ws, &hello, &mut seq).await;
            h.shutdown().await;
            continue;
        }
        let edited = h.controller.snapshot();
        assert_eq!(edited.state_rev, before.state_rev + 1);
        assert_eq!(
            edited.render_rev, before.render_rev,
            "unused edit must not draw"
        );
        assert_eq!(edited.submitted, before.submitted);
        assert_eq!(h.http("GET", &url(&initial_key), &headers, "").await.0, 409);
        let table: Value = serde_json::from_str(
            &h.http("GET", &url(&edited.state.fill_slots_key()), &headers, "")
                .await
                .2,
        )
        .unwrap();
        assert_eq!(
            table["fills"]
                .as_array()
                .unwrap()
                .iter()
                .find(|s| s["name"] == "brick")
                .unwrap()["rows"],
            json!(vec![0; 16])
        );
        for (name, base, code) in [
            ("solid", None, "invalid_request"),
            ("clear", None, "invalid_request"),
            ("unknown", None, "invalid_request"),
            ("brick", Some(before.state_rev), "stale_state"),
        ] {
            let value = edit(
                &h,
                &mut ws,
                &hello,
                &mut seq,
                "view.fill_slot",
                slot(name, 1),
                base,
                "error",
            )
            .await;
            assert_eq!(value["code"], code);
            assert_eq!(h.controller.snapshot().state_rev, edited.state_rev);
        }
        edit(
            &h,
            &mut ws,
            &hello,
            &mut seq,
            "view.set",
            json!({"style_batch":{"pairs":pairs,"fill_slot":"brick"}}),
            None,
            "accepted",
        )
        .await;
        let clear = pixels(&mut ws, &hello, &mut seq).await;
        assert!(h
            .controller
            .snapshot()
            .state
            .styles
            .iter()
            .all(|s| s.fill == Fill::Pattern([0; 16])));
        // A value-equal literal detaches just one row without redrawing.
        let bound = h.controller.snapshot();
        edit(
            &h,
            &mut ws,
            &hello,
            &mut seq,
            "view.set",
            json!({"style_batch":{"pairs":[pairs[0]],"fill":{"kind":"pattern","rows":vec![0;16]}}}),
            None,
            "accepted",
        )
        .await;
        assert_eq!(h.controller.snapshot().render_rev, bound.render_rev);
        assert_eq!(
            h.controller.snapshot().state.fill_slots_key(),
            edited.state.fill_slots_key()
        );
        edit(
            &h,
            &mut ws,
            &hello,
            &mut seq,
            "view.fill_slot",
            slot("brick", u16::MAX),
            None,
            "accepted",
        )
        .await;
        let filled = pixels(&mut ws, &hello, &mut seq).await;
        assert_ne!(filled, clear);
        let current = h.controller.snapshot();
        for (i, s) in current.state.styles.iter().enumerate() {
            assert_eq!(
                s.fill,
                Fill::Pattern([if i == 0 { 0 } else { u16::MAX }; 16])
            );
        }
        assert_ne!(current.render_key, bound.render_key);
        let settings = current.state.settings(&h.controller.model);
        assert_eq!(settings.version, 2);
        assert_eq!(
            settings
                .rows
                .iter()
                .filter(|s| s.fill_slot.as_deref() == Some("brick"))
                .count(),
            pairs.len() - 1
        );
        edit(
            &h,
            &mut ws,
            &hello,
            &mut seq,
            "view.fill_slot",
            slot("brick", 0),
            None,
            "accepted",
        )
        .await;
        assert_eq!(pixels(&mut ws, &hello, &mut seq).await, clear);
        assert_eq!(
            h.http("GET", "/api/v1/palette/presets", &headers, "")
                .await
                .2,
            compiled
        );
        // The content hint has returned to a prior value; full state CAS still
        // rejects a draft from then. Never use cache identity as edit authority.
        assert_eq!(
            h.controller.snapshot().state.fill_slots_key(),
            edited.state.fill_slots_key()
        );
        let value = edit(
            &h,
            &mut ws,
            &hello,
            &mut seq,
            "view.fill_slot",
            slot("brick", 2),
            Some(edited.state_rev),
            "error",
        )
        .await;
        assert_eq!(value["code"], "stale_state");
        for field in ["view_id", "connection_epoch"] {
            let (mut other, greeting, _) = h.connect(&login).await;
            let mut request = json!({"type":"view.fill_slot","seq":"1",
                "view_id":greeting["view_id"],"connection_epoch":greeting["connection_epoch"],
                "base_state_rev":h.controller.snapshot().state_rev.to_string(),"body":slot("brick",3)});
            request[field] = json!("foreign");
            let revision = h.controller.snapshot().state_rev;
            other
                .send(Message::Text(request.to_string().into()))
                .await
                .unwrap();
            closed(&mut other).await;
            assert_eq!(h.controller.snapshot().state_rev, revision);
        }
        h.shutdown().await;
    }
    println!("RUST FILL SLOT STREAM: ALL OK (auth/opt-in, memory-only, cache hint, unused no-render, refs/literals, fixed/stale, exact pixel restore)");
}
