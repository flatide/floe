use super::*;
use tokio_tungstenite::connect_async;
#[path = "exploration.rs"]
mod exploration;
#[path = "guest_queries.rs"]
mod guest_queries;

fn headers<'a>(origin: &'a str, login: &'a Login) -> [(&'static str, &'a str); 4] {
    [
        ("Origin", origin),
        ("Content-Type", "application/json"),
        ("Cookie", &login.cookie),
        ("X-Floe-CSRF", &login.csrf),
    ]
}
async fn invite(h: &Harness, owner: &Login, mode: &str) -> Value {
    let origin = format!("http://{}", h.addr);
    let s = h.controller.snapshot();
    let (_, _, current) = h
        .http("GET", "/api/v1/view", &headers(&origin, owner), "")
        .await;
    let current: Value = serde_json::from_str(&current).unwrap();
    let body = json!({"view_id":current["view"]["view_id"],"base_state_rev":s.state_rev.to_string(),"mode":mode,"approve":true}).to_string();
    let (code, _, response) = h
        .http("POST", "/api/v1/shares", &headers(&origin, owner), &body)
        .await;
    assert_eq!(code, 200, "{response}");
    serde_json::from_str(&response).unwrap()
}
async fn exchange(h: &Harness, invite: &Value) -> Login {
    let origin = format!("http://{}", h.addr);
    let path = format!(
        "/api/v1/guest/{}/exchange",
        invite["share_id"].as_str().unwrap()
    );
    let body = json!({"invite":invite["invite"],"protocol":1,"bundle":BUNDLE}).to_string();
    let headers = [
        ("Origin", origin.as_str()),
        ("Content-Type", "application/json"),
    ];
    let (code, head, response) = h.http("POST", &path, &headers, &body).await;
    assert_eq!(code, 200);
    assert!(head.contains("HttpOnly; SameSite=Strict; Max-Age=1800"));
    assert_eq!(h.http("POST", &path, &headers, &body).await.0, 401);
    let data: Value = serde_json::from_str(&response).unwrap();
    Login {
        cookie: head
            .lines()
            .find_map(|s| s.strip_prefix("set-cookie: "))
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .into(),
        csrf: data["csrf"].as_str().unwrap().into(),
    }
}

#[tokio::test]
#[ignore = "run tools/validate_view_stream.py with a private synthetic fixture"]
async fn local_share_grants_are_opt_in_scoped_and_not_owner_credentials() {
    let off = Harness::start(true).await;
    let owner = off.login().await;
    let origin = format!("http://{}", off.addr);
    assert_eq!(off.http("GET", "/api/v1/shares", &[], "").await.0, 401);
    assert_eq!(
        off.http("GET", "/api/v1/shares", &headers(&origin, &owner), "")
            .await
            .0,
        404
    );
    assert_eq!(
        off.http(
            "POST",
            "/api/v1/guest/absent/exchange",
            &headers(&origin, &owner),
            "{}"
        )
        .await
        .0,
        404
    );
    off.shutdown().await;

    let h = Harness::start_with_access(true, false, false, None, false, true).await;
    let owner = h.login().await;
    let origin = format!("http://{}", h.addr);
    let (mut ws, hello, _) = h.connect(&owner).await;
    let (first, _) = frame(&mut ws).await;
    ack(&mut ws, &hello, 1, &first).await;
    let before = h.controller.snapshot();
    let resources = h.resources.usage();
    let bad = json!({"view_id":hello["view_id"],"base_state_rev":before.state_rev.to_string(),"mode":"follow","approve":false}).to_string();
    assert_eq!(
        h.http("POST", "/api/v1/shares", &headers(&origin, &owner), &bad)
            .await
            .0,
        403
    );
    assert_eq!(
        h.http(
            "POST",
            "/api/v1/shares",
            &headers("http://bad.invalid", &owner),
            &bad
        )
        .await
        .0,
        403
    );
    assert_eq!(
        h.http(
            "POST",
            "/api/v1/shares",
            &headers(&origin, &owner)[..3],
            &bad
        )
        .await
        .0,
        401
    );
    let a = invite(&h, &owner, "follow").await;
    let b = invite(&h, &owner, "explore").await;
    let ac = exchange(&h, &a).await;
    let bc = exchange(&h, &b).await;
    assert_ne!(owner.cookie, ac.cookie);
    assert_ne!(ac.cookie, bc.cookie);
    let aid = a["share_id"].as_str().unwrap();
    let bid = b["share_id"].as_str().unwrap();
    let apath = format!("/api/v1/guest/{aid}/session");
    let bpath = format!("/api/v1/guest/{bid}/session");
    let ah = [
        ("Origin", origin.as_str()),
        ("Cookie", ac.cookie.as_str()),
        ("X-Floe-Guest-CSRF", ac.csrf.as_str()),
    ];
    let bh = [
        ("Origin", origin.as_str()),
        ("Cookie", bc.cookie.as_str()),
        ("X-Floe-Guest-CSRF", bc.csrf.as_str()),
    ];
    let (code, _, body) = h.http("GET", &apath, &ah, "").await;
    assert_eq!(code, 200);
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap(),
        json!({"share_id":aid,"mode":"follow","read_only":true,"delivery":"follow_frames"})
    );
    assert_eq!(h.http("GET", &apath, &bh, "").await.0, 401);
    assert_eq!(h.http("GET", &bpath, &ah, "").await.0, 401);
    assert_eq!(
        h.http("GET", &apath, &headers(&origin, &owner), "").await.0,
        401
    );

    // Even a same-browser ambient owner cookie does not upgrade guest CSRF.
    // Rename the guest cookie to the owner name too: namespaces alone are not
    // the security boundary; the stored random proofs must be independent.
    for cookie in [
        format!("{}; {}", owner.cookie, ac.cookie),
        format!(
            "{}={}",
            owner.cookie.split('=').next().unwrap(),
            ac.cookie.split('=').nth(1).unwrap()
        ),
    ] {
        let pretending = Login {
            cookie,
            csrf: ac.csrf.clone(),
        };
        for (method, path) in [
            ("GET", "/api/v1/catalog"),
            ("GET", "/api/v1/view"),
            ("GET", "/api/v1/capabilities"),
            ("GET", "/api/v1/drc"),
            ("POST", "/api/v1/operations"),
            ("GET", "/api/v1/shares"),
            ("POST", "/api/v1/shares"),
            ("DELETE", "/api/v1/session"),
        ] {
            assert_eq!(
                h.http(method, path, &headers(&origin, &pretending), "{}")
                    .await
                    .0,
                401,
                "{method} {path}"
            );
        }
        let mut request = format!("ws://{}/api/v1/events", h.addr)
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert("origin", origin.parse().unwrap());
        request
            .headers_mut()
            .insert("cookie", pretending.cookie.parse().unwrap());
        request.headers_mut().insert(
            "sec-websocket-protocol",
            format!("{PROTOCOL}, bundle.{BUNDLE}, csrf.{}", ac.csrf)
                .parse()
                .unwrap(),
        );
        match connect_async(request).await {
            Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
                assert_eq!(response.status().as_u16(), 401)
            }
            _ => panic!("guest must not upgrade to owner WebSocket"),
        }
    }
    let (_, _, listing) = h
        .http("GET", "/api/v1/shares", &headers(&origin, &owner), "")
        .await;
    assert!(!listing.contains(a["invite"].as_str().unwrap()));
    assert!(!listing.contains(&ac.csrf));
    assert_eq!(h.controller.snapshot().state_rev, before.state_rev);
    assert_eq!(
        h.resources.usage(),
        resources,
        "grant must not start a renderer or native IO"
    );
    assert_eq!(h.http("DELETE", &apath, &ah, "").await.0, 204);
    assert_eq!(h.http("GET", &apath, &ah, "").await.0, 401);
    assert_eq!(h.http("GET", &bpath, &bh, "").await.0, 200);
    ws.send(Message::Text(
        json!({"type":"ping","seq":"2"}).to_string().into(),
    ))
    .await
    .unwrap();
    until_reply(&mut ws, 2, "pong").await;
    // A layer scope change retires the remaining grant, never widening it.
    h.controller
        .edit(
            before.state_rev,
            floe_app_core::view::Patch {
                layers: Some(floe_worker_client::Layers::None),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(h.http("GET", &bpath, &bh, "").await.0, 401);
    let c = invite(&h, &owner, "follow").await;
    let cc = exchange(&h, &c).await;
    let cpath = format!("/api/v1/guest/{}/session", c["share_id"].as_str().unwrap());
    assert_eq!(
        h.http("DELETE", "/api/v1/session", &headers(&origin, &owner), "")
            .await
            .0,
        204
    );
    assert_eq!(
        h.http(
            "GET",
            &cpath,
            &[
                ("Cookie", cc.cookie.as_str()),
                ("X-Floe-Guest-CSRF", cc.csrf.as_str())
            ],
            ""
        )
        .await
        .0,
        401
    );
    closed(&mut ws).await;
    h.shutdown().await;
    println!("RUST LOCAL SHARE GRANTS: ALL OK (default off, scope/approval, isolated identities, one-use, HTTP/WS owner denial, logout, scope change, owner cascade)");
}

fn guest_request(
    h: &Harness,
    id: &str,
    guest: &Login,
) -> tokio_tungstenite::tungstenite::http::Request<()> {
    let mut r = format!("ws://{}/api/v1/guest/{id}/events", h.addr)
        .into_client_request()
        .unwrap();
    r.headers_mut()
        .insert("origin", format!("http://{}", h.addr).parse().unwrap());
    r.headers_mut()
        .insert("cookie", guest.cookie.parse().unwrap());
    r.headers_mut().insert(
        "sec-websocket-protocol",
        format!("{PROTOCOL}, bundle.{BUNDLE}, guest-csrf.{}", guest.csrf)
            .parse()
            .unwrap(),
    );
    r
}
async fn follow(h: &Harness, id: &str, guest: &Login) -> (Socket, Value) {
    guest_connect(h, id, guest, "follow").await
}
async fn guest_connect(h: &Harness, id: &str, guest: &Login, mode: &str) -> (Socket, Value) {
    let (mut ws, response) = tokio_tungstenite::connect_async_with_config(
        guest_request(h, id, guest),
        Some(image_client_config()),
        false,
    )
    .await
    .unwrap();
    assert_eq!(response.headers()["sec-websocket-protocol"], PROTOCOL);
    let hello = next_json(&mut ws).await;
    assert_eq!(hello["type"], "share.hello");
    assert_eq!(hello["mode"], mode);
    assert_eq!(hello["read_only"], true);
    assert!(hello.get("title").is_none());
    (ws, hello)
}
async fn guest_frame(ws: &mut Socket) -> (Value, Vec<u8>) {
    loop {
        match next(ws).await {
            Message::Binary(b) => {
                let n = u32::from_le_bytes(b[..4].try_into().unwrap()) as usize;
                let h: Value = serde_json::from_slice(&b[4..4 + n]).unwrap();
                assert_eq!(h["query"], false);
                assert_eq!(
                    h["query_scene"],
                    json!({"generation":null,"round":null,"complete":false,"summary_layers":"0"})
                );
                assert!(h.get("perf").is_none());
                assert_eq!(h["payload_length"], (b.len() - 4 - n).to_string());
                return (h, b[4 + n..].to_vec());
            }
            Message::Text(t) => {
                let s: Value = serde_json::from_str(&t).unwrap();
                assert_eq!(s["type"], "share.state");
                for key in [
                    "layers",
                    "minimap",
                    "title",
                    "failure",
                    "styles",
                    "capabilities",
                ] {
                    assert!(s.get(key).is_none(), "guest state leaked {key}");
                }
            }
            m => panic!("unexpected follow message {m:?}"),
        }
    }
}
async fn guest_drained(h: &Harness) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let u = h.gate.transport_usage();
        if u.guest_sockets == 0 && u.guest_reserved_output_bytes == 0 && u.guest_encoders == 0 {
            break;
        }
        assert!(Instant::now() < deadline, "guest resource leak {u:?}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}
async fn denied(request: tokio_tungstenite::tungstenite::http::Request<()>, status: u16) {
    match connect_async(request).await {
        Err(tokio_tungstenite::tungstenite::Error::Http(r)) => {
            assert_eq!(r.status().as_u16(), status)
        }
        _ => panic!("expected guest upgrade status {status}"),
    }
}

#[tokio::test]
#[ignore = "run tools/validate_view_stream.py with a private synthetic fixture"]
async fn local_follow_reuses_pixels_has_private_credit_and_rejects_owner_commands() {
    for raw in [true, false] {
        let h = Harness::start_with_access(raw, false, false, None, false, true).await;
        let owner = h.login().await;
        let (mut ws, hello, _) = h.connect(&owner).await;
        let (first, original) = frame(&mut ws).await;
        ack(&mut ws, &hello, 1, &first).await;
        let a = invite(&h, &owner, "follow").await;
        let c = exchange(&h, &a).await;
        let id = a["share_id"].as_str().unwrap();
        let (mut guest, gh) = follow(&h, id, &c).await;
        let (gf, pixels) = guest_frame(&mut guest).await;
        assert_eq!(pixels, original);
        assert_eq!(gf["frame_id"], first["frame_id"]);
        let resources = h.resources.usage();
        denied(guest_request(&h, id, &c), 429).await;
        let held = h.gate.transport_usage().guest_reserved_output_bytes;
        assert!(held >= pixels.len() * 3);
        // Leave guest unacknowledged while the owner renders two generations.
        for (seq, labels) in [(2, true), (4, false)] {
            let before = h.controller.snapshot();
            ws.send(Message::Text(
                json!({"type":"view.set","seq":seq.to_string(),
                "connection_epoch":hello["connection_epoch"],"view_id":hello["view_id"],
                "base_state_rev":before.state_rev.to_string(),"body":{"labels":labels}})
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
            until_reply(&mut ws, seq, "accepted").await;
            let (current, _) = frame(&mut ws).await;
            ack(&mut ws, &hello, seq + 1, &current).await;
            assert_eq!(h.gate.transport_usage().guest_reserved_output_bytes, held);
        }
        assert_eq!(
            h.resources.usage(),
            resources,
            "follow created native resources"
        );
        ack(&mut guest, &gh, 1, &gf).await;
        let (latest, bytes) = guest_frame(&mut guest).await;
        assert_eq!(
            latest["render_rev"],
            h.controller.snapshot().render_rev.to_string()
        );
        assert_eq!(bytes, h.controller.latest().unwrap().frame.bytes);
        ack(&mut guest, &gh, 2, &latest).await;
        let before = h.controller.snapshot();
        // Every current owner Control name must fail closed, even with genuine
        // owner IDs/epochs. Reconnect reuses only this guest grant, not invite.
        for (i, kind) in [
            "view.set",
            "view.apply",
            "view.fill_slot",
            "view.clip.prepare",
            "view.query",
            "view.query.cancel",
            "view.measure",
            "view.measure_selection",
            "explore.set",
        ]
        .iter()
        .enumerate()
        {
            guest
                .send(Message::Text(
                    json!({"type":kind,"seq":"3", "connection_epoch":hello["connection_epoch"],
                "view_id":hello["view_id"],"base_state_rev":before.state_rev.to_string(),
                "body":{"layers":{"mode":"none"}}})
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
            closed(&mut guest).await;
            guest_drained(&h).await;
            assert_eq!(h.controller.snapshot().state_rev, before.state_rev);
            if i != 8 {
                let pair = follow(&h, id, &c).await;
                guest = pair.0;
                let (f, _) = guest_frame(&mut guest).await;
                ack(&mut guest, &pair.1, 1, &f).await;
            }
        }
        let (mut live, _) = follow(&h, id, &c).await;
        guest_frame(&mut live).await;
        // Pending pixel credit must be reclaimed when layers change, not used
        // to send the next (out-of-scope) frame or to keep this grant alive.
        h.controller
            .edit(
                before.state_rev,
                floe_app_core::view::Patch {
                    layers: Some(floe_worker_client::Layers::None),
                    ..Default::default()
                },
            )
            .unwrap();
        closed(&mut live).await;
        guest_drained(&h).await;
        denied(guest_request(&h, id, &c), 401).await;
        h.shutdown().await;
    }
    println!("RUST LOCAL FOLLOW: ALL OK (raw/PNG exact payload, bounded private credit, latest pending, owner progress, owner control denial, layer revocation)");
}

#[tokio::test]
#[ignore = "run tools/validate_view_stream.py with a private synthetic fixture"]
async fn local_follow_tracks_margin_crop_without_render_and_reconnects() {
    for raw in [true, false] {
        let h = Harness::start_with_access(raw, true, true, None, false, true).await;
        let owner = h.login().await;
        let (mut ws, hello, _) = h.connect(&owner).await;
        let (fg, _) = frame(&mut ws).await;
        ack(&mut ws, &hello, 1, &fg).await;
        let (margin, pixels) = frame(&mut ws).await;
        ack(&mut ws, &hello, 2, &margin).await;
        let a = invite(&h, &owner, "follow").await;
        let c = exchange(&h, &a).await;
        let id = a["share_id"].as_str().unwrap();
        let (mut guest, gh) = follow(&h, id, &c).await;
        let (gf, _) = guest_frame(&mut guest).await;
        ack(&mut guest, &gh, 1, &gf).await;
        let (gm, gp) = guest_frame(&mut guest).await;
        assert_eq!(gm["purpose"], "margin");
        assert_eq!(gm["frame_id"], margin["frame_id"]);
        assert_eq!(gp, pixels);
        ack(&mut guest, &gh, 2, &gm).await;
        ws.send(Message::Text(
            json!({"type":"view.set","seq":"3","view_id":hello["view_id"],
            "connection_epoch":hello["connection_epoch"],"base_state_rev":"1",
            "body":{"navigation":{"kind":"pan","x":0.1,"y":0.0,"snap":true}}})
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
        until_reply(&mut ws, 3, "accepted").await;
        loop {
            let state = next_json(&mut guest).await;
            assert_eq!(state["type"], "share.state");
            if state["render_rev"] == "2" {
                assert_eq!(state["margin"]["frame_id"], gm["frame_id"]);
                assert_eq!(state["margin"]["crop_safe"], true);
                assert_eq!(
                    state["bbox_dbu"],
                    json!(h
                        .controller
                        .snapshot()
                        .state
                        .viewport
                        .bbox
                        .map(|n| n.to_string()))
                );
                break;
            }
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while h.controller.snapshot().crop_hits != 1 {
            assert!(
                Instant::now() < deadline,
                "camera changed but native crop did not settle"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(h.controller.snapshot().submitted, 2);
        assert!(h.controller.latest().is_none());
        guest.close(None).await.unwrap();
        guest_drained(&h).await;
        let (mut again, ah) = follow(&h, id, &c).await;
        let (am, ap) = guest_frame(&mut again).await;
        assert_eq!(am["purpose"], "margin");
        assert_eq!(am["frame_id"], gm["frame_id"]);
        assert_ne!(ah["connection_epoch"], gh["connection_epoch"]);
        assert_eq!(ap, pixels);
        // Owner stop also closes a guest whose frame was never acknowledged.
        h.shutdown().await;
        closed(&mut again).await;
    }
    println!("RUST LOCAL FOLLOW MARGIN: ALL OK (raw/PNG, label margin, camera-only crop, reconnect epoch, owner stop)");
}

#[tokio::test]
#[ignore = "run tools/validate_view_stream.py with a private synthetic fixture"]
async fn local_follow_upgrade_expiry_and_revoke_are_independent() {
    let h = Harness::start_with_access(true, false, false, None, false, true).await;
    let owner = h.login().await;
    let (mut ws, hello, _) = h.connect(&owner).await;
    let (first, _) = frame(&mut ws).await;
    ack(&mut ws, &hello, 1, &first).await;
    let a = invite(&h, &owner, "follow").await;
    let c = exchange(&h, &a).await;
    let id = a["share_id"].as_str().unwrap();
    for (field, value, code) in [
        ("origin", "http://bad.invalid", 403),
        (
            "sec-websocket-protocol",
            "floe.v1, bundle.bad, guest-csrf.fake",
            426,
        ),
        ("cookie", "floe_session_fake=bad", 401),
    ] {
        let mut r = guest_request(&h, id, &c);
        r.headers_mut().insert(field, value.parse().unwrap());
        denied(r, code).await;
    }
    let mut no_origin = guest_request(&h, id, &c);
    no_origin.headers_mut().remove("origin");
    denied(no_origin, 403).await;
    let mut owner_proof = guest_request(&h, id, &c);
    owner_proof.headers_mut().insert(
        "sec-websocket-protocol",
        format!("{PROTOCOL}, bundle.{BUNDLE}, csrf.{}", owner.csrf)
            .parse()
            .unwrap(),
    );
    denied(owner_proof, 401).await;
    let (mut guest, _) = follow(&h, id, &c).await;
    guest_frame(&mut guest).await;
    let start = Instant::now();
    closed(&mut guest).await; // no ACK: ten-second deadline, not session TTL
    assert!(start.elapsed() >= Duration::from_secs(9));
    guest_drained(&h).await;
    let (mut again, _) = follow(&h, id, &c).await;
    guest_frame(&mut again).await;
    let origin = format!("http://{}", h.addr);
    assert_eq!(
        h.http(
            "DELETE",
            &format!("/api/v1/shares/{id}"),
            &headers(&origin, &owner),
            ""
        )
        .await
        .0,
        204
    );
    closed(&mut again).await;
    guest_drained(&h).await;
    denied(guest_request(&h, id, &c), 401).await;
    ws.send(Message::Text(
        json!({"type":"ping","seq":"2"}).to_string().into(),
    ))
    .await
    .unwrap();
    until_reply(&mut ws, 2, "pong").await;
    h.shutdown().await;
    println!("RUST LOCAL FOLLOW LIFETIME: ALL OK (strict upgrade, no-ACK deadline, revoke, owner survives)");
}

#[tokio::test]
#[ignore = "run tools/validate_view_stream.py with a private synthetic fixture"]
async fn local_follow_revokes_a_large_unread_socket_without_owner_credit() {
    let h = Harness::start_with_access(true, false, false, None, false, true).await;
    let owner = h.login().await;
    let (mut ws, hello, _) = h.connect(&owner).await;
    let (first, _) = frame(&mut ws).await;
    ack(&mut ws, &hello, 1, &first).await;
    let before = h.controller.snapshot();
    h.controller
        .edit(
            before.state_rev,
            floe_app_core::view::Patch {
                pixels: Some((4096, 4096)),
                ..Default::default()
            },
        )
        .unwrap();
    let (large, bytes) = frame(&mut ws).await;
    assert_eq!(bytes.len(), 16 + 4096 * 4096 * 4);
    drop(bytes);
    ack(&mut ws, &hello, 2, &large).await;
    let a = invite(&h, &owner, "follow").await;
    let c = exchange(&h, &a).await;
    let id = a["share_id"].as_str().unwrap();
    // No guest reads at all after the upgrade, including hello/state. A 64MiB
    // frame is larger than TCP buffers; revocation must interrupt the writer.
    let (mut unread, _) = tokio_tungstenite::connect_async_with_config(
        guest_request(&h, id, &c),
        Some(image_client_config()),
        false,
    )
    .await
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        let u = h.gate.transport_usage();
        if u.guest_reserved_output_bytes >= 3 * 4096 * 4096 * 4
            && u.guest_encoders == 0
            && u.reserved_output_bytes == 0
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "large frame did not reach writer {u:?}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let origin = format!("http://{}", h.addr);
    assert_eq!(
        h.http(
            "DELETE",
            &format!("/api/v1/shares/{id}"),
            &headers(&origin, &owner),
            ""
        )
        .await
        .0,
        204
    );
    guest_drained(&h).await;
    ws.send(Message::Text(
        json!({"type":"ping","seq":"3"}).to_string().into(),
    ))
    .await
    .unwrap();
    until_reply(&mut ws, 3, "pong").await;
    closed(&mut unread).await;
    h.shutdown().await;
    println!("RUST LOCAL FOLLOW BACKPRESSURE: ALL OK (64MiB unread raw frame, revoke, guest credits drained, owner alive)");
}
