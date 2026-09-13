//! Actual owner HTTP/WS boundary, using the parent's native service harness.
use super::*;

fn source() -> PathBuf {
    PathBuf::from(std::env::var_os("FLOE_OWNER_EXPORT_FIXTURE").unwrap())
}
fn anchor(frame: &Value) -> Value {
    let mut a = json!({});
    for key in [
        "dataset_revision",
        "worker_epoch",
        "frame_id",
        "state_rev",
        "render_rev",
        "render_key",
    ] {
        a[key] = frame[key].clone();
    }
    a
}
async fn event(ws: &mut Socket, seq: u64) -> Value {
    let seq = seq.to_string();
    loop {
        match timeout(Duration::from_secs(8), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
        {
            Message::Text(t) => {
                let v: Value = serde_json::from_str(&t).unwrap();
                if v["seq"].as_str() == Some(seq.as_str()) {
                    return v;
                }
            }
            Message::Ping(p) => ws.send(Message::Pong(p)).await.unwrap(),
            Message::Binary(_) => (),
            other => panic!("unexpected {other:?}"),
        }
    }
}
async fn prepare(
    ws: &mut Socket,
    hello: &Value,
    frame: &Value,
    seq: u64,
    layers: &str,
    jobs: u16,
) -> Value {
    ws.send(Message::Text(json!({"type":"view.clip.prepare","seq":seq.to_string(),"view_id":hello["view_id"],"connection_epoch":hello["connection_epoch"],
        "body":{"anchor":anchor(frame),"bounds":{"kind":"dbu","bbox":["0","0","100000","100000"]},"layers":layers,"jobs":jobs,"cell_name":"WEB_CLIP"}}).to_string().into())).await.unwrap();
    event(ws, seq).await
}
fn submit(seq: u64, hello: &Value, draft: &Value) -> Value {
    json!({"seq":seq.to_string(),"view_id":hello["view_id"],"token":draft["draft"]["token"],"approve":true})
}
async fn done(h: &Harness, l: &Login, seq: u64) -> Value {
    let end = Instant::now() + Duration::from_secs(15);
    loop {
        let (status, v) = h
            .call(l, "GET", &format!("/api/v1/exports/{seq}"), Value::Null)
            .await;
        assert_eq!(status, 200);
        if matches!(v["phase"].as_str(), Some("ready" | "failed" | "cancelled")) {
            return v;
        }
        assert!(Instant::now() < end, "{v}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
async fn opened(h: &Harness, l: &Login, selected: bool) -> (Socket, Value, Value) {
    let (_, c) = h.call(l, "GET", "/api/v1/catalog", Value::Null).await;
    let mut req = open(
        "1",
        &c["sources"][0]["source_id"],
        "level",
        json!({"mode":"all"}),
    );
    if selected {
        req["body"]["layers"] = json!({"mode":"only","pairs":[[1,0]]});
    }
    assert_eq!(h.call(l, "POST", "/api/v1/operations", req).await.0, 202);
    assert_eq!(h.finished(l, 1).await["phase"], "succeeded");
    let mut ws = h.connect(l).await;
    let (hello, frame) = frame(&mut ws).await;
    (ws, hello, frame)
}
async fn download(h: &Harness, l: &Login, id: &str, form: bool) -> (u16, String, Vec<u8>) {
    let origin = format!("http://{}", h.addr);
    let path = format!("/api/v1/artifacts/{id}/download");
    if form {
        h.raw_bytes(
            "POST",
            &path,
            &[
                ("Origin", &origin),
                ("Cookie", &l.cookie),
                ("Content-Type", "application/x-www-form-urlencoded"),
            ],
            &format!("csrf={}", l.csrf),
        )
        .await
    } else {
        h.raw_bytes(
            "GET",
            &path,
            &[
                ("Origin", &origin),
                ("Cookie", &l.cookie),
                ("X-Floe-CSRF", &l.csrf),
            ],
            "",
        )
        .await
    }
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "run tools/validate_owner_service.py with private source/oracles"]
async fn exact_export_receipts_replays_and_authenticated_binary_downloads() {
    let path = source();
    let h = Harness::start(std::slice::from_ref(&path), native()).await;
    assert_eq!(h.raw("GET", "/api/v1/exports", &[], "").await.0, 401);
    assert_eq!(
        h.raw("GET", "/api/v1/artifacts/1/download", &[], "")
            .await
            .0,
        401
    );
    let l = h.login().await;
    let (mut ws, hello, f) = opened(&h, &l, true).await;
    let mut forged = f.clone();
    forged["dataset_revision"] = json!("999999");
    assert_eq!(
        prepare(&mut ws, &hello, &forged, 2, "all", 1).await["code"],
        "frame_not_displayed"
    );
    let first = prepare(&mut ws, &hello, &f, 3, "visible", 1).await;
    assert_eq!(first["type"], "clip.prepared");
    assert_eq!(first["draft"]["layers"], json!({"mode":"only","count":1}));
    let req = submit(1, &hello, &first);
    for (key, val, code) in [
        ("approve", json!(false), 400),
        ("view_id", json!("0".repeat(64)), 404),
        ("token", json!("0".repeat(64)), 410),
        ("path", json!("/etc/passwd"), 400),
    ] {
        let mut bad = req.clone();
        bad[key] = val;
        assert_eq!(h.call(&l, "POST", "/api/v1/exports", bad).await.0, code);
    }
    assert_eq!(
        h.call(&l, "GET", "/api/v1/exports", Value::Null).await.1["operations"]["last_seq"],
        "0"
    );
    let (a, b) = tokio::join!(
        h.call(&l, "POST", "/api/v1/exports", req.clone()),
        h.call(&l, "POST", "/api/v1/exports", req.clone())
    );
    assert_eq!((a.0, b.0), (202, 202));
    let result = done(&h, &l, 1).await;
    assert_eq!(result["phase"], "ready", "{result}");
    assert_eq!(
        h.call(&l, "GET", "/api/v1/exports", Value::Null).await.1["usage"]["entries"],
        1
    );
    let id = result["artifact"]["id"].as_str().unwrap();
    let expected = fs::read(path.parent().unwrap().join("golden-visible.oas")).unwrap();
    for form in [false, true] {
        let (code, headers, bytes) = download(&h, &l, id, form).await;
        assert_eq!(code, 200);
        assert!(headers.contains(&format!("filename=\"floe-clip-{id}.oas\"")));
        assert!(headers.contains("cache-control: no-store"));
        assert_eq!(bytes, expected);
    }
    let route = format!("/api/v1/artifacts/{id}/download");
    let origin = format!("http://{}", h.addr);
    assert_eq!(
        h.raw("GET", &route, &[("Cookie", &l.cookie)], "").await.0,
        401
    );
    assert_eq!(
        h.raw(
            "POST",
            &route,
            &[
                ("Origin", "http://elsewhere.invalid"),
                ("Cookie", &l.cookie),
                ("Content-Type", "application/x-www-form-urlencoded")
            ],
            &format!("csrf={}", l.csrf)
        )
        .await
        .0,
        403
    );
    for body in [
        String::new(),
        format!("csrf={}&csrf={}", l.csrf, l.csrf),
        format!("csrf=%{}", l.csrf),
    ] {
        assert_eq!(
            h.raw(
                "POST",
                &route,
                &[
                    ("Origin", &origin),
                    ("Cookie", &l.cookie),
                    ("Content-Type", "application/x-www-form-urlencoded")
                ],
                &body
            )
            .await
            .0,
            401
        );
    }
    assert_eq!(
        h.raw(
            "GET",
            &route,
            &[
                ("Cookie", &l.cookie),
                ("X-Floe-CSRF", &l.csrf),
                ("Range", "bytes=0-1")
            ],
            ""
        )
        .await
        .0,
        416
    );
    // Same sequence/options cannot execute again, even after dismissal.
    assert_eq!(
        h.call(
            &l,
            "DELETE",
            &format!("/api/v1/artifacts/{id}"),
            Value::Null
        )
        .await
        .0,
        204
    );
    assert_eq!(download(&h, &l, id, false).await.0, 410);
    assert_eq!(
        h.call(&l, "POST", "/api/v1/exports", req.clone()).await.1["artifact"]["available"],
        false
    );
    let mut conflict = req.clone();
    conflict["token"] = json!("1".repeat(64));
    assert_eq!(h.call(&l, "POST", "/api/v1/exports", conflict).await.0, 409);
    assert_eq!(
        h.call(&l, "POST", "/api/v1/exports", submit(2, &hello, &first))
            .await
            .0,
        410
    );
    // Explicit all differs from current-visible, and none is an empty OASIS.
    for (seq, name, jobs) in [(2, "all", 8), (3, "none", 1)] {
        let d = prepare(&mut ws, &hello, &f, seq + 3, name, jobs).await;
        assert_eq!(d["type"], "clip.prepared", "{d}");
        let req = submit(seq, &hello, &d);
        assert_eq!(
            h.call(&l, "POST", "/api/v1/exports", req.clone()).await.0,
            202
        );
        let result = done(&h, &l, seq).await;
        assert_eq!(result["phase"], "ready", "{result}");
        let id = result["artifact"]["id"].as_str().unwrap();
        assert_eq!(
            download(&h, &l, id, false).await.2,
            fs::read(path.parent().unwrap().join(format!("golden-{name}.oas"))).unwrap()
        );
        h.call(
            &l,
            "DELETE",
            &format!("/api/v1/artifacts/{id}"),
            Value::Null,
        )
        .await;
    }
    // Preparing is not approval, and a departed socket cannot donate a draft.
    let old = prepare(&mut ws, &hello, &f, 7, "all", 1).await;
    ws.close(None).await.unwrap();
    closed(&mut ws).await;
    assert_eq!(
        h.call(&l, "POST", "/api/v1/exports", submit(4, &hello, &old))
            .await
            .0,
        410
    );
    let mut fresh = h.connect(&l).await;
    let (hello2, f2) = frame(&mut fresh).await;
    let pending = prepare(&mut fresh, &hello2, &f2, 2, "all", 1).await;
    // Closing the view invalidates an unaccepted draft but not an old replay.
    h.call(
        &l,
        "DELETE",
        &format!("/api/v1/views/{}", hello["view_id"].as_str().unwrap()),
        Value::Null,
    )
    .await;
    assert_eq!(
        h.call(&l, "POST", "/api/v1/exports", submit(4, &hello2, &pending))
            .await
            .0,
        409
    );
    assert_eq!(h.call(&l, "POST", "/api/v1/exports", req).await.0, 202);
    fresh.close(None).await.ok();
    h.shutdown().await;
    println!("RUST OWNER EXPORT: ALL OK (receipts, one-use approval/replay, exact all/visible/none, authenticated download, expiry/release)");
}

async fn fake_harness(path: &Path, binary: &Path) -> Harness {
    let resources = Resources::new(Limits::default()).unwrap();
    let scope = AccessScope::new(&[path.parent().unwrap().into()]).unwrap();
    let source = RegisteredSource::register(scope, path, &AtomicUsize::new(0)).unwrap();
    let mut options = RenderOptions::local().unwrap();
    options.binary = binary.into();
    options.decode_jobs = 1;
    options.raster_jobs = 1;
    options.budget_mb = 64;
    options.raw = false;
    let service = Service::start(vec![source], Arc::clone(&resources), options, native()).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (gate, bootstrap) = Gateway::with_service(addr, Arc::clone(&service)).unwrap();
    let (stop, rx) = oneshot::channel();
    let task = tokio::spawn(transport::serve(listener, Arc::clone(&gate), async {
        let _ = rx.await;
    }));
    Harness {
        addr,
        gate,
        bootstrap,
        service,
        resources,
        stop,
        task,
    }
}
#[tokio::test(flavor = "current_thread")]
#[ignore = "run tools/validate_owner_service.py with controlled native fixtures"]
async fn export_cancel_and_logout_reap_workers_without_blocking_view_control() {
    let path = source();
    let root = path.parent().unwrap();
    for mode in ["cancel", "logout", "failure"] {
        let binary = root.join(format!("fake-{mode}"));
        let h = fake_harness(&path, &binary).await;
        let l = h.login().await;
        let (mut ws, hello, f) = opened(&h, &l, false).await;
        let d = prepare(&mut ws, &hello, &f, 2, "all", 1).await;
        assert_eq!(d["type"], "clip.prepared", "{d}");
        assert_eq!(
            h.call(&l, "POST", "/api/v1/exports", submit(1, &hello, &d))
                .await
                .0,
            202
        );
        if mode != "failure" {
            let marker = binary.with_extension("pid");
            let end = Instant::now() + Duration::from_secs(8);
            while !marker.exists() {
                assert!(Instant::now() < end);
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            assert_eq!(h.resources.usage().workers, 2);
            assert!(h
                .resources
                .index([cache::cache_path(&path).unwrap()], 1)
                .is_err());
            // Native clip is deliberately hung; HTTP and independent WS work.
            ws.send(Message::Text(
                json!({"type":"ping","seq":"3"}).to_string().into(),
            ))
            .await
            .unwrap();
            assert_eq!(event(&mut ws, 3).await["type"], "pong");
            if mode == "logout" {
                assert_eq!(
                    h.call(&l, "DELETE", "/api/v1/session", Value::Null).await.0,
                    204
                );
                assert_eq!(
                    h.call(&l, "GET", "/api/v1/exports", Value::Null).await.0,
                    401
                );
            } else {
                // The export alone retains the source read lease after view close.
                h.call(
                    &l,
                    "DELETE",
                    &format!("/api/v1/views/{}", hello["view_id"].as_str().unwrap()),
                    Value::Null,
                )
                .await;
                h.call(&l, "POST", "/api/v1/exports/1/cancel", json!({}))
                    .await;
                assert_eq!(done(&h, &l, 1).await["phase"], "cancelled");
                let end = Instant::now() + Duration::from_secs(5);
                while h.resources.usage().workers != 0 {
                    assert!(Instant::now() < end);
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                drop(
                    h.resources
                        .index([cache::cache_path(&path).unwrap()], 1)
                        .unwrap(),
                );
                assert_eq!(
                    h.call(&l, "GET", "/api/v1/exports", Value::Null).await.1["usage"]["entries"],
                    0
                );
            }
        } else {
            let done = done(&h, &l, 1).await;
            assert_eq!(done["phase"], "failed");
            assert_eq!(done["error"], "worker_failed");
            assert!(!done.to_string().contains("ENOSPC"));
        }
        ws.close(None).await.ok();
        h.shutdown().await;
    }
    println!("RUST OWNER EXPORT LIFECYCLE: ALL OK (responsive control, cancel/logout/reap, pinned source lease, safe errors)");
}
