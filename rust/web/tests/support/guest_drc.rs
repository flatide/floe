//! Native owner+two guests use one immutable DRC pack, different panels/views.
use super::*;
fn stamp(root: &Path) -> Vec<(PathBuf, Vec<u8>, std::time::SystemTime)> {
    fn visit(dir: &Path, out: &mut Vec<(PathBuf, Vec<u8>, std::time::SystemTime)>) {
        for item in fs::read_dir(dir).unwrap() {
            let p = item.unwrap().path();
            let m = fs::symlink_metadata(&p).unwrap();
            assert!(!m.file_type().is_symlink());
            if m.is_dir() {
                visit(&p, out);
            } else {
                out.push((p.clone(), fs::read(p).unwrap(), m.modified().unwrap()));
            }
        }
    }
    let mut out = vec![];
    visit(root, &mut out);
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

async fn call(
    h: &Harness,
    login: &Login,
    id: &str,
    method: &str,
    suffix: &str,
    body: Value,
) -> (u16, Value) {
    let origin = format!("http://{}", h.addr);
    let (code, _, text) = h
        .raw(
            method,
            &format!("/api/v1/guest/{id}/drc{suffix}"),
            &[
                ("Origin", &origin),
                ("Content-Type", "application/json"),
                ("Cookie", &login.cookie),
                ("X-Floe-Guest-CSRF", &login.csrf),
            ],
            &if body.is_null() {
                String::new()
            } else {
                body.to_string()
            },
        )
        .await;
    assert!(
        !text.contains("/private/") && !text.contains("/Users/"),
        "{text}"
    );
    (code, serde_json::from_str(&text).unwrap_or(Value::Null))
}
async fn invitation(h: &Harness, owner: &Login, mode: &str, drc: Value) -> (String, Login) {
    let view = h.call(owner, "GET", "/api/v1/view", Value::Null).await.1["view"].clone();
    let (code,i)=h.call(owner,"POST","/api/v1/shares",json!({"view_id":view["view_id"],"base_state_rev":view["state_rev"],"mode":mode,"approve":true,"drc":drc})).await;
    assert_eq!(code, 200, "{i}");
    if !drc.is_null() {
        assert_eq!(i["drc"], json!({"id":drc["id"],"revision":drc["revision"]}));
    }
    let id = i["share_id"].as_str().unwrap().to_owned();
    let origin = format!("http://{}", h.addr);
    let (code, headers, text) = h
        .raw(
            "POST",
            &format!("/api/v1/guest/{id}/exchange"),
            &[("Origin", &origin), ("Content-Type", "application/json")],
            &json!({"invite":i["invite"],"protocol":1,"bundle":BUNDLE}).to_string(),
        )
        .await;
    assert_eq!(code, 200);
    let cookie = headers
        .lines()
        .find_map(|s| s.strip_prefix("set-cookie: "))
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .into();
    let login = Login {
        cookie,
        csrf: serde_json::from_str::<Value>(&text).unwrap()["csrf"]
            .as_str()
            .unwrap()
            .into(),
    };
    (id, login)
}
struct Guest {
    id: String,
    login: Login,
    ws: Socket,
    hello: Value,
    frame: Value,
    seq: u64,
}
impl Guest {
    async fn connect(h: &Harness, id: String, login: Login) -> Self {
        let mut req = format!("ws://{}/api/v1/guest/{id}/events", h.addr)
            .into_client_request()
            .unwrap();
        req.headers_mut()
            .insert("Origin", format!("http://{}", h.addr).parse().unwrap());
        req.headers_mut()
            .insert("Cookie", login.cookie.parse().unwrap());
        req.headers_mut().insert(
            "Sec-WebSocket-Protocol",
            format!("{PROTOCOL}, bundle.{BUNDLE}, guest-csrf.{}", login.csrf)
                .parse()
                .unwrap(),
        );
        let mut ws = connect_async(req).await.unwrap().0;
        let hello = text(&mut ws).await;
        assert_eq!(hello["type"], "share.hello");
        let mut g = Self {
            id,
            login,
            ws,
            hello,
            frame: Value::Null,
            seq: 0,
        };
        loop {
            let v = g.next().await;
            if v["type"] == "frame" {
                g.frame = v;
                break;
            }
        }
        g
    }
    async fn next(&mut self) -> Value {
        loop {
            match timeout(Duration::from_secs(10), self.ws.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap()
            {
                Message::Text(t) => {
                    let value: Value = serde_json::from_str(&t).unwrap();
                    if value["type"] == "share.state" {
                        assert!(value["dbu_um"].as_str().unwrap().parse::<f64>().unwrap() > 0.);
                        let camera = value["camera_um"].as_array().unwrap();
                        assert_eq!(camera.len(), 3);
                        assert!(camera[2].as_str().unwrap().parse::<f64>().unwrap() > 0.);
                    }
                    return value;
                }
                Message::Ping(p) => self.ws.send(Message::Pong(p)).await.unwrap(),
                Message::Binary(b) => {
                    let n = u32::from_le_bytes(b[..4].try_into().unwrap()) as usize;
                    let value: Value = serde_json::from_slice(&b[4..4 + n]).unwrap();
                    assert_eq!(value["view_id"], self.hello["view_id"]);
                    assert!(b[4 + n..].starts_with(b"\x89PNG\r\n\x1a\n"));
                    self.seq += 1;
                    self.ws.send(Message::Text(json!({"type":"frame.ack","seq":self.seq.to_string(),"connection_epoch":self.hello["connection_epoch"],"frame_id":value["frame_id"],"disposition":"displayed"}).to_string().into())).await.unwrap();
                    return value;
                }
                m => panic!("unexpected guest control: {m:?}"),
            }
        }
    }
    async fn goto(&mut self, navigation: Value) {
        self.seq += 1;
        let seq = self.seq.to_string();
        self.ws.send(Message::Text(json!({"type":"explore.set","seq":seq,"view_id":self.hello["view_id"],"connection_epoch":self.hello["connection_epoch"],"base_state_rev":self.frame["state_rev"],"body":{"navigation":navigation}}).to_string().into())).await.unwrap();
        let accepted = loop {
            let v = self.next().await;
            if v["seq"] == seq {
                assert_eq!(v["type"], "accepted", "{v}");
                break v;
            }
        };
        loop {
            let v = self.next().await;
            if v["type"] == "frame"
                && v["purpose"] == "foreground"
                && v["state_rev"] == accepted["state_rev"]
            {
                self.frame = v;
                break;
            }
        }
    }
    fn body(&self, revision: &Value, body: Value) -> Value {
        json!({"view_id":self.hello["view_id"],"revision":revision,"state_rev":self.frame["state_rev"],"body":body})
    }
}
fn panel(local: &str) -> Value {
    json!({"search":"WIDTH","rule_start":"0","check":"0","error_start":"0","query":null,"waived":false,
        "selected":{"check":"0","error":local},"markers":true,"shown":true,"jump_scale":null,"zoom_lock":false,"jump_active":false,"focus_visible":false})
}

#[tokio::test]
#[ignore = "run tools/validate_owner_service.py with private valmini"]
async fn independent_drc_guests_are_explicit_read_only_and_revision_bound() {
    let fixture = PathBuf::from(std::env::var_os("FLOE_OWNER_FIXTURE").unwrap());
    let root = fixture.parent().unwrap().join("guest-drc");
    fs::create_dir(&root).unwrap();
    let source = root.join("design.oas");
    fs::copy(&fixture, &source).unwrap();
    let index = std::env::var_os("FLOE_INDEX_BIN").unwrap();
    assert!(std::process::Command::new(&index)
        .arg("vfs")
        .arg(&source)
        .arg(source.with_extension("oas.floe"))
        .args(["--jobs", "2"])
        .output()
        .unwrap()
        .status
        .success());
    let db = root.join("review.db");
    fs::write(&db,"TOP 1000\nWIDTH\n3 3 0\np 1 4\n30000 30000\n31000 30000\n31000 31000\n30000 31000\np 2 4\n50000 30000\n51000 30000\n51000 31000\n50000 31000\np 3 4\n30000 60000\n31000 60000\n31000 61000\n30000 61000\n").unwrap();
    assert!(std::process::Command::new(&index)
        .arg("drc")
        .arg(&db)
        .args(["--jobs", "2"])
        .output()
        .unwrap()
        .status
        .success());
    let pack = db.with_extension("db.ice");
    let tree_before = stamp(&root);
    let before = [&source, &db, &pack].map(|p| {
        (
            fs::read(p).unwrap(),
            fs::metadata(p).unwrap().modified().unwrap(),
        )
    });
    let h = Harness::configured_sharing(
        std::slice::from_ref(&source),
        native(),
        Some((&pack, None)),
        false,
        false,
        Limits {
            workers: 3,
            ..Limits::default()
        },
        true,
    )
    .await;
    let owner = h.login().await;
    let reader = h.drc_reader.as_ref().unwrap();
    reader
        .submit(serde_json::from_value(json!({"kind":"rule","check":"0"})).unwrap())
        .unwrap()
        .result()
        .await
        .unwrap();
    let catalog = h.call(&owner, "GET", "/api/v1/drc", Value::Null).await.1["drc"].clone();
    let revision = catalog["revision"].clone();
    let source_id = h.service.catalog()["sources"][0]["source_id"].clone();
    assert_eq!(
        h.call(
            &owner,
            "POST",
            "/api/v1/operations",
            open("1", &source_id, "level", json!({"mode":"all"}))
        )
        .await
        .0,
        202
    );
    assert_eq!(h.finished(&owner, 1).await["phase"], "succeeded");
    let mut ow = h.connect(&owner).await;
    let (oh, _) = frame(&mut ow).await;
    let view = h.call(&owner, "GET", "/api/v1/view", Value::Null).await.1["view"].clone();
    let owner_panel = format!(
        "/api/v1/drc/{}/views/{}/panel",
        reader.id,
        oh["view_id"].as_str().unwrap()
    );
    assert_eq!(
        h.call(
            &owner,
            "POST",
            &owner_panel,
            json!({"revision":revision,"base_panel_rev":"1","body":panel("0")})
        )
        .await
        .0,
        200
    );
    let owner_saved = h.call(&owner, "GET", &owner_panel, Value::Null).await.1;
    let approval = json!({"id":reader.id,"revision":revision,"approve":true});
    for bad in [
        json!({"id":reader.id,"revision":revision,"approve":false}),
        json!({"id":"unknown","revision":revision,"approve":true}),
        json!({"id":reader.id,"revision":"old","approve":true}),
    ] {
        assert_ne!(h.call(&owner,"POST","/api/v1/shares",json!({"view_id":view["view_id"],"base_state_rev":view["state_rev"],"mode":"follow","approve":true,"drc":bad})).await.0,200);
    }
    let (noid, nologin) = invitation(&h, &owner, "follow", Value::Null).await;
    for (method, path) in [
        ("GET", ""),
        ("POST", "/read"),
        ("GET", "/panel"),
        ("POST", "/panel"),
        ("GET", "/selection"),
        ("POST", "/selection"),
    ] {
        assert_eq!(
            call(&h, &nologin, &noid, method, path, json!({})).await.0,
            403
        );
    }
    assert_eq!(
        call(&h, &nologin, &noid, "GET", "", Value::Null).await.0,
        403
    );
    let (aid, al) = invitation(&h, &owner, "explore", approval.clone()).await;
    let mut a = Guest::connect(&h, aid, al).await;
    let (bid, bl) = invitation(&h, &owner, "explore", approval.clone()).await;
    let mut b = Guest::connect(&h, bid, bl).await;
    let (fid, fl) = invitation(&h, &owner, "follow", approval).await;
    assert_eq!(call(&h, &owner, &a.id, "GET", "", Value::Null).await.0, 401);
    assert_eq!(
        call(&h, &b.login, &a.id, "GET", "", Value::Null).await.0,
        401
    );
    for (guest, local) in [(&mut a, "1"), (&mut b, "2")] {
        let (code, m) = call(&h, &guest.login, &guest.id, "GET", "", Value::Null).await;
        assert_eq!(code, 200, "{m}");
        assert_eq!(m["data"]["errors"], "3");
        for key in ["title", "source_id", "reviewer", "review", "notes", "svrf"] {
            assert!(m["data"].get(key).is_none());
        }
        let context = json!({"view_id":guest.hello["view_id"],"revision":revision,"base_panel_rev":"1","body":panel(local)});
        assert_eq!(
            call(
                &h,
                &guest.login,
                &guest.id,
                "POST",
                "/panel",
                context.clone()
            )
            .await
            .0,
            200
        );
        let mut stale = context.clone();
        stale["body"]["search"] = json!("stale");
        assert_eq!(
            call(&h, &guest.login, &guest.id, "POST", "/panel", stale)
                .await
                .0,
            409
        );
        let select = json!({"view_id":guest.hello["view_id"],"revision":revision,"base_selection_rev":"1","body":{"kind":"apply","check":"0","errors":[local],"mode":"replace"}});
        let (code, s) = call(
            &h,
            &guest.login,
            &guest.id,
            "POST",
            "/selection",
            select.clone(),
        )
        .await;
        assert_eq!(code, 200, "{s}");
        assert_eq!(s["data"]["rules"][0]["errors"], json!([local]));
        assert_eq!(
            call(&h, &guest.login, &guest.id, "POST", "/selection", select)
                .await
                .0,
            409
        );
        let list=guest.body(&revision,json!({"kind":"list","check":"0","start":"0","in_view":false,"selection_rev":s["data"]["selection_rev"],"limit":64}));
        let (code, rows) = call(&h, &guest.login, &guest.id, "POST", "/read", list).await;
        assert_eq!(code, 200, "{rows}");
        assert_eq!(rows["data"]["rows"].as_array().unwrap().len(), 1);
        assert_eq!(rows["data"]["rows"][0]["local"], local);
        let (code, geo) = call(
            &h,
            &guest.login,
            &guest.id,
            "POST",
            "/read",
            guest.body(
                &revision,
                json!({"kind":"geometry","check":"0","error":local,"start":"0","limit":64}),
            ),
        )
        .await;
        assert_eq!(code, 200, "{geo}");
        assert_eq!(geo["data"]["points_dbu"].as_array().unwrap().len(), 4);
        let (code, focus) = call(
            &h,
            &guest.login,
            &guest.id,
            "POST",
            "/read",
            guest.body(
                &revision,
                json!({"kind":"focus","check":"0","error":local,"fit":true}),
            ),
        )
        .await;
        assert_eq!(code, 200, "{focus}");
        guest.goto(focus["data"]["navigation"].clone()).await;
        for forbidden in [
            json!({"kind":"focus","check":"0","error":local,"fit":true,"isolate":true}),
            json!({"kind":"types","start":"0","limit":1}),
            json!({"kind":"comparison","check":"0","error":local}),
        ] {
            assert_eq!(
                call(
                    &h,
                    &guest.login,
                    &guest.id,
                    "POST",
                    "/read",
                    guest.body(&revision, forbidden)
                )
                .await
                .0,
                403
            );
        }
        let mut forged = guest.body(&revision, json!({"kind":"rule","check":"0"}));
        forged["view_id"] = oh["view_id"].clone();
        assert_eq!(
            call(&h, &guest.login, &guest.id, "POST", "/read", forged)
                .await
                .0,
            409
        );
        assert_ne!(
            call(
                &h,
                &guest.login,
                &guest.id,
                "POST",
                "/read",
                guest.body(&revision, json!({"kind":"note","check":"0","error":local}))
            )
            .await
            .0,
            200
        );
    }
    assert_ne!(a.frame["bbox_dbu"], b.frame["bbox_dbu"]);
    assert_eq!(
        stamp(&root),
        tree_before,
        "guest operations wrote source/index/DRC files"
    );
    for (g, local) in [(&a, "1"), (&b, "2")] {
        assert_eq!(
            call(&h, &g.login, &g.id, "GET", "/panel", Value::Null)
                .await
                .1["data"]["body"]["selected"]["error"],
            local
        );
    }
    let previous = b.hello.clone();
    b.ws.close(None).await.unwrap();
    closed(&mut b.ws).await;
    let deadline = Instant::now() + Duration::from_secs(5);
    while h.gate.transport_usage().guest_sockets > 1 {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    b = Guest::connect(
        &h,
        b.id.clone(),
        Login {
            cookie: b.login.cookie.clone(),
            csrf: b.login.csrf.clone(),
        },
    )
    .await;
    assert_eq!(b.hello["view_id"], previous["view_id"]);
    assert_ne!(b.hello["connection_epoch"], previous["connection_epoch"]);
    assert_eq!(
        call(&h, &b.login, &b.id, "GET", "/panel", Value::Null)
            .await
            .1["data"]["body"]["selected"]["error"],
        "2"
    );
    assert_eq!(
        call(&h, &b.login, &b.id, "GET", "/selection", Value::Null)
            .await
            .1["data"]["rules"][0]["errors"],
        json!(["2"])
    );
    // Follow has a private panel but cannot move the owner's view.
    let mut follow_stream = Guest::connect(
        &h,
        fid.clone(),
        Login {
            cookie: fl.cookie.clone(),
            csrf: fl.csrf.clone(),
        },
    )
    .await;
    follow_stream.ws.close(None).await.unwrap();
    let follow = call(&h, &fl, &fid, "GET", "", Value::Null).await.1;
    assert_eq!(call(&h,&fl,&fid,"POST","/panel",json!({"view_id":follow["view_id"],"revision":revision,"base_panel_rev":"1","body":panel("2")})).await.0,200);
    assert_eq!(
        h.call(&owner, "GET", &owner_panel, Value::Null).await.1,
        owner_saved
    );
    assert_eq!(
        h.call(&owner, "GET", "/api/v1/view", Value::Null).await.1["view"]["state_rev"],
        view["state_rev"]
    );
    assert_eq!(
        [&source, &db, &pack].map(|p| (
            fs::read(p).unwrap(),
            fs::metadata(p).unwrap().modified().unwrap()
        )),
        before
    );
    assert_eq!(
        h.call(
            &owner,
            "DELETE",
            &format!("/api/v1/shares/{}", a.id),
            Value::Null
        )
        .await
        .0,
        204
    );
    closed(&mut a.ws).await;
    assert_eq!(
        call(&h, &a.login, &a.id, "GET", "", Value::Null).await.0,
        401
    );
    assert_eq!(
        call(&h, &b.login, &b.id, "GET", "", Value::Null).await.0,
        200
    );
    // In-memory waive application changes the read revision even for this
    // same immutable geometry/reader. Existing grants must not follow it.
    use floe_app_core::drc::review::{
        managed::{ManagedStore, Registration},
        store::Kind,
    };
    let stop = Arc::new(AtomicUsize::new(0));
    let store = ManagedStore::open(
        &h.resources,
        Registration {
            scope: AccessScope::new(std::slice::from_ref(&root)).unwrap(),
            pack: pack.clone(),
            reviewer: "synthetic-guest".into(),
            kind: Kind::Waives,
            protected_files: vec![],
            protected_trees: vec![],
        },
        &stop,
    )
    .unwrap();
    reader
        .apply_waives(store.snapshot(Arc::clone(&stop)).unwrap())
        .unwrap()
        .result()
        .await
        .unwrap();
    assert_ne!(reader.revision(), revision.as_str().unwrap());
    assert_eq!(
        call(&h, &b.login, &b.id, "GET", "", Value::Null).await.0,
        401
    );
    assert_eq!(call(&h, &fl, &fid, "GET", "", Value::Null).await.0, 401);
    closed(&mut b.ws).await;
    drop(store);
    assert_eq!(
        [&source, &db, &pack].map(|p| (
            fs::read(p).unwrap(),
            fs::metadata(p).unwrap().modified().unwrap()
        )),
        before
    );
    h.shutdown().await;
    println!("RUST GUEST DRC: ALL OK (explicit scope, owner+two error views, private panels/selection/goto, follow isolation, bounded reads, no notes/writes, revoke/review revision, native cleanup)");
}
