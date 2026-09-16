use super::*;

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
        json!({"share_id":aid,"mode":"follow","read_only":true,"delivery":"not_connected"})
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
    println!("RUST LOCAL SHARE GRANTS: ALL OK (default off, scope/approval, isolated identities, one-use, HTTP/WS owner denial, logout, scope change, owner cascade; no frame/DRC delivery yet)");
}
