//! Valmini's LEAF1 rectangles provide exact known pick/snap/ruler positions.
use super::exploration::{edit, explore_frame};
use super::*;
use floe_app_core::view::{Patch, Viewport};
use floe_worker_client::Layers;

async fn start(raw: bool, margin: bool) -> (Harness, Login, Socket, Value, Value) {
    let h = Harness::start_with_limits(
        raw,
        margin,
        false,
        Some(Viewport::new([28000., 28000., 52000., 40000.], 257, 191).unwrap()),
        false,
        true,
        Limits {
            workers: 3,
            ..Limits::default()
        },
    )
    .await;
    let s = h.controller.snapshot();
    h.controller
        .edit(
            s.state_rev,
            Patch {
                layers: Some(Layers::Only(vec![(3, 0)])),
                detail: Some(Detail::Exact),
                frames: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
    let owner = h.login().await;
    let (mut ws, hello, _) = h.connect(&owner).await;
    let (f, _) = frame(&mut ws).await;
    ack(&mut ws, &hello, 1, &f).await;
    (h, owner, ws, hello, f)
}
fn anchor(state: &Value, frame: &Value) -> Value {
    json!({"dataset_revision":state["dataset_revision"],"worker_epoch":state["worker_epoch"],
        "frame_id":frame["frame_id"],"state_rev":state["state_rev"],"render_rev":state["render_rev"],"render_key":state["render_key"]})
}
fn query(state: &Value, frame: &Value, world: [f64; 2], operation: Value, layers: Value) -> Value {
    let b: Vec<f64> = state["bbox_dbu"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap().parse().unwrap())
        .collect();
    json!({"anchor":anchor(state,frame),"operation":operation,"layers":layers,"radius_px":1.,
        "position":[(world[0]-b[0])/(b[2]-b[0]),(b[3]-world[1])/(b[3]-b[1])]})
}
async fn command(ws: &mut Socket, hello: &Value, seq: u64, kind: &str, body: Value) {
    ws.send(Message::Text(
        json!({"type":kind,"seq":seq.to_string(),"view_id":hello["view_id"],
        "connection_epoch":hello["connection_epoch"],"body":body})
        .to_string()
        .into(),
    ))
    .await
    .unwrap();
}
async fn answer(ws: &mut Socket, hello: &Value, seq: u64, body: Value) -> Value {
    let anchor = body["anchor"].clone();
    command(ws, hello, seq, "explore.query", body).await;
    let accepted = until_reply(ws, seq, "query.accepted").await;
    let result = until_reply(ws, seq, "query.result").await;
    assert_eq!(result["query_id"], accepted["query_id"]);
    assert_eq!(result["anchor"], anchor);
    assert_eq!(result["view_id"], hello["view_id"]);
    assert_eq!(result["connection_epoch"], hello["connection_epoch"]);
    assert_eq!(result["status"], "ok", "{result}");
    assert!(!result.to_string().contains("/private/"));
    assert!(!result.to_string().contains("/Users/"));
    result
}
fn measurement(q: &Value, start: Value, snap: Value) -> Value {
    json!({"anchor":q["anchor"],"position":q["position"],"start_dbu":start,"free_angle":false,"snap_query":snap})
}

#[tokio::test]
#[ignore = "run tools/validate_view_stream.py with private valmini"]
async fn guest_queries_and_rulers_are_scene_scoped_and_do_not_touch_owner() {
    for raw in [true, false] {
        let (h, owner, mut ws, oh, of) = start(raw, false).await;
        let before = h.controller.snapshot();
        let a = invite(&h, &owner, "explore").await;
        let ac = exchange(&h, &a).await;
        let (mut av, ah) = guest_connect(&h, a["share_id"].as_str().unwrap(), &ac, "explore").await;
        assert_eq!(ah["query"], true);
        assert_eq!(ah["measure"], true);
        let (af, _) = explore_frame(&mut av).await;
        let pick = query(
            &af,
            &af,
            [31000., 31000.],
            json!({"kind":"pick","nth":"0"}),
            json!({"mode":"all"}),
        );
        command(&mut av, &ah, 1, "explore.query", pick.clone()).await;
        assert_eq!(
            until_reply(&mut av, 1, "error").await["code"],
            "frame_not_displayed"
        );
        ack(&mut av, &ah, 2, &af).await;
        let b = invite(&h, &owner, "explore").await;
        let bc = exchange(&h, &b).await;
        let (mut bv, bh) = guest_connect(&h, b["share_id"].as_str().unwrap(), &bc, "explore").await;
        let (bf, _) = explore_frame(&mut bv).await;
        ack(&mut bv, &bh, 1, &bf).await;
        for (seq, foreign) in [(3, &of), (4, &bf)] {
            let mut forged = pick.clone();
            forged["anchor"] = anchor(foreign, foreign);
            command(&mut av, &ah, seq, "explore.query", forged).await;
            assert_eq!(
                until_reply(&mut av, seq, "error").await["code"],
                "frame_not_displayed"
            );
        }
        for (seq, pair) in [(5, [7, 0]), (6, [4000, 4000])] {
            let mut outside = pick.clone();
            outside["layers"] = json!({"mode":"only","pairs":[pair]});
            command(&mut av, &ah, seq, "explore.query", outside).await;
            assert_eq!(
                until_reply(&mut av, seq, "error").await["code"],
                "forbidden"
            );
        }
        let hit = answer(&mut av, &ah, 7, pick.clone()).await;
        assert_eq!(hit["hit"]["pair"], json!([3, 0]));
        assert_eq!(hit["hit"]["cell_name"], "LEAF1");
        assert_eq!(
            hit["hit"]["bbox_dbu"],
            json!(["30000", "30000", "48000", "33000"])
        );
        let snap = query(
            &af,
            &af,
            [30001., 30001.],
            json!({"kind":"snap"}),
            json!({"mode":"all"}),
        );
        let snapped = answer(&mut av, &ah, 8, snap.clone()).await;
        assert_eq!(snapped["hit"]["point_dbu"], json!(["30000", "30000"]));
        command(
            &mut av,
            &ah,
            9,
            "explore.measure",
            measurement(&snap, Value::Null, snapped["query_id"].clone()),
        )
        .await;
        let start = until_reply(&mut av, 9, "measure.result").await;
        assert_eq!(start["point_dbu"], json!(["30000", "30000"]));
        let endq = query(
            &af,
            &af,
            [48001., 33001.],
            json!({"kind":"snap"}),
            json!({"mode":"all"}),
        );
        let end = answer(&mut av, &ah, 10, endq.clone()).await;
        command(
            &mut av,
            &ah,
            11,
            "explore.measure",
            measurement(&endq, start["point_dbu"].clone(), end["query_id"].clone()),
        )
        .await;
        let ruler = until_reply(&mut av, 11, "measure.result").await;
        assert_eq!(ruler["segment"]["distance_um"], "18");
        command(
            &mut av,
            &ah,
            12,
            "explore.measure_selection",
            json!({"anchor":pick["anchor"],"boxes_dbu":[
            ["30000","30000","48000","33000"],["50000","30000","68000","33000"]]}),
        )
        .await;
        let rulers = until_reply(&mut av, 12, "measure_selection.result").await;
        assert_eq!(rulers["segments"][0]["distance_um"], "2");
        let other = query(
            &bf,
            &bf,
            [30001., 30001.],
            json!({"kind":"snap"}),
            json!({"mode":"all"}),
        );
        command(
            &mut bv,
            &bh,
            2,
            "explore.measure",
            measurement(&other, Value::Null, snapped["query_id"].clone()),
        )
        .await;
        assert_eq!(until_reply(&mut bv, 2, "error").await["code"], "stale_snap");
        edit(
            &mut bv,
            &bh,
            3,
            &bf["state_rev"],
            json!({"navigation":{"kind":"pan","x":0.5,"y":0.,"snap":false}}),
            "accepted",
        )
        .await;
        let (bf2, _) = explore_frame(&mut bv).await;
        ack(&mut bv, &bh, 4, &bf2).await;
        let picked = answer(
            &mut bv,
            &bh,
            5,
            query(
                &bf2,
                &bf2,
                [51000., 31000.],
                json!({"kind":"pick","nth":"0"}),
                json!({"mode":"all"}),
            ),
        )
        .await;
        assert_eq!(
            picked["hit"]["bbox_dbu"],
            json!(["50000", "30000", "68000", "33000"])
        );
        let still = answer(&mut av, &ah, 13, pick.clone()).await;
        assert_eq!(still["hit"], hit["hit"]);
        // All tracks the current visible set, never broadens back to the grant.
        edit(
            &mut av,
            &ah,
            14,
            &af["state_rev"],
            json!({"layers":{"mode":"none"}}),
            "accepted",
        )
        .await;
        let (hidden, _) = explore_frame(&mut av).await;
        ack(&mut av, &ah, 15, &hidden).await;
        let empty = answer(
            &mut av,
            &ah,
            16,
            query(
                &hidden,
                &hidden,
                [31000., 31000.],
                json!({"kind":"pick","nth":"0"}),
                json!({"mode":"all"}),
            ),
        )
        .await;
        assert!(empty["hit"].is_null());
        command(
            &mut av,
            &ah,
            17,
            "explore.measure",
            measurement(&snap, Value::Null, Value::Null),
        )
        .await;
        assert_eq!(
            until_reply(&mut av, 17, "error").await["code"],
            "frame_not_displayed"
        );
        assert_eq!(h.controller.snapshot().state_rev, before.state_rev);
        assert_eq!(h.controller.snapshot().submitted, before.submitted);
        assert_eq!(h.controller.query_snapshot().accepted, 0);
        ws.send(Message::Text(
            json!({"type":"ping","seq":"2"}).to_string().into(),
        ))
        .await
        .unwrap();
        until_reply(&mut ws, 2, "pong").await;
        assert_eq!(oh["view_id"], of["view_id"]);
        h.shutdown().await;
        closed(&mut av).await;
        closed(&mut bv).await;
    }
    println!("RUST GUEST QUERIES: ALL OK (raw/PNG, scoped receipts/layers, isolated owner+two scenes, picks/snaps/manual+selection rulers, no source writes)");
}

#[tokio::test]
#[ignore = "run tools/validate_view_stream.py with private valmini"]
async fn guest_query_receipts_survive_crop_but_not_discard_reconnect_or_revoke() {
    let (h, owner, mut ws, oh, _) = start(true, true).await;
    let (om, _) = frame(&mut ws).await;
    assert_eq!(om["purpose"], "margin");
    ack(&mut ws, &oh, 2, &om).await;
    let a = invite(&h, &owner, "explore").await;
    let ac = exchange(&h, &a).await;
    let id = a["share_id"].as_str().unwrap();
    let (mut av, ah) = guest_connect(&h, id, &ac, "explore").await;
    let (af, _) = explore_frame(&mut av).await;
    ack(&mut av, &ah, 1, &af).await;
    let (gm, _) = explore_frame(&mut av).await;
    assert_eq!(gm["purpose"], "margin");
    let pick = query(
        &af,
        &af,
        [31000., 31000.],
        json!({"kind":"pick","nth":"0"}),
        json!({"mode":"all"}),
    );
    // A fully written but unacknowledged margin must not block small queries
    // against the already displayed foreground receipt.
    let _ = answer(&mut av, &ah, 2, pick.clone()).await;
    ack(&mut av, &ah, 3, &gm).await;
    let snap = query(
        &af,
        &gm,
        [30001., 30001.],
        json!({"kind":"snap"}),
        json!({"mode":"all"}),
    );
    let snapped = answer(&mut av, &ah, 4, snap.clone()).await;
    av.send(Message::Text(
        json!({"type":"explore.query.cancel","seq":"5","view_id":ah["view_id"],
        "connection_epoch":ah["connection_epoch"],"kind":"snap"})
        .to_string()
        .into(),
    ))
    .await
    .unwrap();
    until_reply(&mut av, 5, "query.cancelled").await;
    command(
        &mut av,
        &ah,
        6,
        "explore.measure",
        measurement(&snap, Value::Null, snapped["query_id"].clone()),
    )
    .await;
    assert_eq!(until_reply(&mut av, 6, "error").await["code"], "stale_snap");
    let moved = edit(
        &mut av,
        &ah,
        7,
        &af["state_rev"],
        // Match keyboard pan: the 16px fill phase must survive margin reuse.
        json!({"navigation":{"kind":"pan","x":0.1,"y":0.,"snap":true}}),
        "accepted",
    )
    .await;
    let current = loop {
        let s = next_json(&mut av).await;
        assert_eq!(s["type"], "share.state");
        if s["state_rev"] == moved["state_rev"]
            && s["rendering"] == false
            && s["margin"]["frame_id"] == gm["frame_id"]
        {
            break s;
        }
    };
    command(&mut av, &ah, 8, "explore.query", pick).await;
    assert_eq!(
        until_reply(&mut av, 8, "error").await["code"],
        "stale_frame"
    );
    let cropped = query(
        &current,
        &gm,
        [31000., 31000.],
        json!({"kind":"pick","nth":"0"}),
        json!({"mode":"all"}),
    );
    let hit = answer(&mut av, &ah, 9, cropped.clone()).await;
    assert_eq!(
        hit["hit"]["bbox_dbu"],
        json!(["30000", "30000", "48000", "33000"])
    );
    av.close(None).await.unwrap();
    closed(&mut av).await;
    guest_drained(&h).await;
    let (mut again, rh) = guest_connect(&h, id, &ac, "explore").await;
    assert_eq!(rh["view_id"], ah["view_id"]);
    assert_ne!(rh["connection_epoch"], ah["connection_epoch"]);
    // After the crop only its covering margin is valid for retransmission.
    let (rf, _) = explore_frame(&mut again).await;
    assert_eq!(rf["purpose"], "margin");
    command(&mut again, &rh, 1, "explore.query", cropped.clone()).await;
    assert_eq!(
        until_reply(&mut again, 1, "error").await["code"],
        "frame_not_displayed"
    );
    again
        .send(Message::Text(
            json!({"type":"frame.ack","seq":"2","connection_epoch":rh["connection_epoch"],
        "frame_id":rf["frame_id"],"disposition":"discarded"})
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
    command(&mut again, &rh, 3, "explore.query", cropped.clone()).await;
    assert_eq!(
        until_reply(&mut again, 3, "error").await["code"],
        "frame_not_displayed"
    );
    // Old epoch, even on the same retained view, is a protocol violation.
    command(&mut again, &ah, 4, "explore.query", cropped.clone()).await;
    closed(&mut again).await;
    guest_drained(&h).await;
    let (mut last, lh) = guest_connect(&h, id, &ac, "explore").await;
    let (lf, _) = explore_frame(&mut last).await;
    ack(&mut last, &lh, 1, &lf).await;
    let _ = answer(&mut last, &lh, 2, cropped.clone()).await;
    command(&mut last, &lh, 3, "explore.query", cropped).await;
    until_reply(&mut last, 3, "query.accepted").await;
    assert_eq!(
        h.http(
            "DELETE",
            &format!("/api/v1/shares/{id}"),
            &headers(&format!("http://{}", h.addr), &owner),
            ""
        )
        .await
        .0,
        204
    );
    closed(&mut last).await;
    guest_drained(&h).await;
    denied(guest_request(&h, id, &ac), 401).await;
    assert_eq!(h.controller.query_snapshot().accepted, 0);
    h.shutdown().await;
    println!("RUST GUEST QUERY LIFETIME: ALL OK (image credit independent, margin crop, cancel, discard, reconnect/epoch, revoke/reap)");
}

#[tokio::test]
#[ignore = "run tools/validate_view_stream.py with private valmini"]
async fn follow_cannot_borrow_explore_queries_or_rulers() {
    let (h, owner, _ws, _oh, _) = start(true, false).await;
    let a = invite(&h, &owner, "follow").await;
    let ac = exchange(&h, &a).await;
    let id = a["share_id"].as_str().unwrap();
    for kind in [
        "explore.query",
        "explore.query.cancel",
        "explore.measure",
        "explore.measure_selection",
    ] {
        let (mut av, ah) = follow(&h, id, &ac).await;
        assert_eq!(ah["query"], false);
        assert_eq!(ah["measure"], false);
        let (af, _) = guest_frame(&mut av).await;
        ack(&mut av, &ah, 1, &af).await;
        let q = query(
            &af,
            &af,
            [31000., 31000.],
            json!({"kind":"snap"}),
            json!({"mode":"all"}),
        );
        if kind == "explore.query.cancel" {
            av.send(Message::Text(
                json!({"type":kind,"seq":"2","view_id":ah["view_id"],
                "connection_epoch":ah["connection_epoch"],"kind":"snap"})
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        } else {
            let body = match kind {
                "explore.measure" => measurement(&q, Value::Null, Value::Null),
                "explore.measure_selection" => json!({"anchor":q["anchor"],"boxes_dbu":[]}),
                _ => q,
            };
            command(&mut av, &ah, 2, kind, body).await;
        }
        closed(&mut av).await;
        guest_drained(&h).await;
        assert_eq!(h.controller.query_snapshot().accepted, 0);
    }
    h.shutdown().await;
    println!("RUST FOLLOW QUERY DENIAL: ALL OK (all four valid explore commands rejected before owner dispatch)");
}
