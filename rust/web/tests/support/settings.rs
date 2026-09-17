use super::drc_isolation::ReviewSocket;
use super::*;

async fn request(
    h: &Harness,
    l: &Login,
    method: &str,
    path: &str,
    body: &str,
) -> (u16, String, String) {
    request_with_type(h, l, method, path, body, "text/plain; charset=utf-8").await
}
async fn request_with_type(
    h: &Harness,
    l: &Login,
    method: &str,
    path: &str,
    body: &str,
    content_type: &str,
) -> (u16, String, String) {
    let origin = format!("http://{}", h.addr);
    h.raw(
        method,
        path,
        &[
            ("Origin", &origin),
            ("Cookie", &l.cookie),
            ("X-Floe-CSRF", &l.csrf),
            ("Content-Type", content_type),
        ],
        body,
    )
    .await
}
fn path(s: &ReviewSocket, format: &str) -> String {
    format!(
        "/api/v1/views/{}/settings/{}/{format}",
        s.hello["view_id"].as_str().unwrap(),
        s.state["state_rev"].as_str().unwrap()
    )
}
fn files(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|p| p.unwrap().path())
        .filter(|p| p.is_file())
        .map(|p| {
            let bytes = fs::read(&p).unwrap();
            (p, bytes)
        })
        .collect();
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "run tools/validate_owner_service.py with private source files"]
async fn owner_settings_prepare_apply_restore_download_and_no_writes() {
    let fixture = PathBuf::from(std::env::var_os("FLOE_OWNER_FIXTURE").unwrap());
    let root = fixture.parent().unwrap().join("settings");
    fs::create_dir(&root).unwrap();
    let source = root.join("settings.oas");
    fs::copy(&fixture, &source).unwrap();
    assert!(
        std::process::Command::new(std::env::var_os("FLOE_INDEX_BIN").unwrap())
            .arg("vfs")
            .arg(&source)
            .arg(source.with_extension("oas.floe"))
            .args(["--jobs", "1"])
            .output()
            .unwrap()
            .status
            .success()
    );
    let before = files(&root);
    let cache_before = files(&source.with_extension("oas.floe"));
    let h = Harness::start(std::slice::from_ref(&source), native()).await;
    let login = h.login().await;
    let catalog = h
        .call(&login, "GET", "/api/v1/catalog", Value::Null)
        .await
        .1;
    assert_eq!(
        h.call(
            &login,
            "POST",
            "/api/v1/operations",
            open(
                "1",
                &catalog["sources"][0]["source_id"],
                "level",
                json!({"mode":"all"})
            )
        )
        .await
        .0,
        202
    );
    assert_eq!(h.finished(&login, 1).await["phase"], "succeeded");
    let mut s = ReviewSocket::new(&h, &login).await;
    assert_eq!(
        h.call(&login, "GET", "/api/v1/capabilities", Value::Null)
            .await
            .1["layer_settings"],
        true
    );
    let initial = request(&h, &login, "GET", &path(&s, "native"), "").await;
    assert_eq!(initial.0, 200);
    let original: Value = serde_json::from_str(&initial.2).unwrap();
    let pair = original["rows"][0]["pair"].clone();
    let row = format!("{}.{} red solid MASK 0 3\n", pair[0], pair[1]);
    let big = format!("#{}\n{row}", "x".repeat(32768));
    assert_eq!(
        h.raw(
            "POST",
            &path(&s, "calibre"),
            &[
                ("Origin", &format!("http://{}", h.addr)),
                ("Content-Type", "text/plain; charset=utf-8")
            ],
            &big
        )
        .await
        .0,
        401
    );
    assert_eq!(
        h.call(&login, "POST", &path(&s, "calibre"), json!({"text":row}))
            .await
            .0,
        400
    );
    assert_eq!(
        h.raw(
            "POST",
            &path(&s, "calibre"),
            &[
                ("Origin", &format!("http://{}", h.addr)),
                ("Cookie", &login.cookie),
                ("X-Floe-CSRF", &login.csrf),
                ("Content-Type", "text/plain; charset=utf-8"),
                ("Content-Type", "application/json"),
            ],
            &row
        )
        .await
        .0,
        400
    );
    // Chromium normalizes the charset of an XHR string body to UTF-8.
    // Raw lowercase-only requests previously hid this browser incompatibility.
    for format in ["native", "calibre"] {
        let body = if format == "native" { &initial.2 } else { &big };
        for content_type in ["text/plain; charset=UTF-8", "TEXT/PLAIN; CHARSET=Utf-8"] {
            let reply =
                request_with_type(&h, &login, "POST", &path(&s, format), body, content_type).await;
            assert_eq!(reply.0, 200, "{format} {content_type}: {}", reply.2);
            assert_eq!(
                request(&h, &login, "GET", &path(&s, "native"), "").await.2,
                initial.2,
                "preparation changed the view"
            );
        }
        for content_type in [
            "text/plain",
            "application/json; charset=UTF-8",
            "text/plain; charset=iso-8859-1",
            "text/plain; charset=utf-16",
            "text/plain; charset=UTF-8x",
            "text/plain; charset=UTF-8; charset=utf-16",
            "text/plain; charset=UTF-8, text/plain; charset=UTF-8",
        ] {
            let reply =
                request_with_type(&h, &login, "POST", &path(&s, format), body, content_type).await;
            assert_eq!(reply.0, 400, "{format} {content_type}: {}", reply.2);
        }
    }
    let prepared = request_with_type(
        &h,
        &login,
        "POST",
        &path(&s, "calibre"),
        &big,
        "text/plain; charset=UTF-8",
    )
    .await;
    assert_eq!(prepared.0, 200, "{}", prepared.2);
    let prepared: Value = serde_json::from_str(&prepared.2).unwrap();
    assert_eq!(prepared["rows"], 1);
    assert_eq!(prepared["malformed"], 0);
    assert_eq!(
        request(&h, &login, "GET", &path(&s, "native"), "").await.2,
        initial.2,
        "preparation changed the view"
    );
    let old = path(&s, "native");
    s.apply(&prepared["prepared_token"], None).await;
    assert_ne!(s.state["state_rev"], "1");
    assert_eq!(request(&h, &login, "GET", &old, "").await.0, 409);
    let applied = request(&h, &login, "GET", &path(&s, "native"), "").await;
    assert_eq!(applied.0, 200);
    let after: Value = serde_json::from_str(&applied.2).unwrap();
    assert_eq!(after["rows"][0]["color"], "#ff0000");
    assert_eq!(after["rows"][0]["visible"], false);
    assert_eq!(after["rows"][0]["width"], 3);
    s.apply(&prepared["prepared_token"], Some("prepared_edit_expired"))
        .await;
    let invalids = [
        "{\"format\":\"floe.layers\",\"version\":2,\"rows\":[],\"groups\":[]}",
        "{\"format\":\"floe.layers\",\"version\":1,\"rows\":[],\"groups\":[]}",
    ];
    for text in invalids {
        assert_eq!(
            request(&h, &login, "POST", &path(&s, "native"), text)
                .await
                .0,
            400
        );
        assert_eq!(
            request(&h, &login, "GET", &path(&s, "native"), "").await.2,
            applied.2
        );
    }
    let prepared: Value = serde_json::from_str(
        &request(&h, &login, "POST", &path(&s, "native"), &initial.2)
            .await
            .2,
    )
    .unwrap();
    s.set(json!({"mono":true})).await;
    s.apply(&prepared["prepared_token"], Some("stale_state"))
        .await;
    let prepared: Value = serde_json::from_str(
        &request_with_type(
            &h,
            &login,
            "POST",
            &path(&s, "native"),
            &initial.2,
            "text/plain; charset=UTF-8",
        )
        .await
        .2,
    )
    .unwrap();
    s.apply(&prepared["prepared_token"], None).await;
    assert_eq!(
        s.state["mono"], true,
        "layer restore changed unrelated view state"
    );
    assert_eq!(
        request(&h, &login, "GET", &path(&s, "native"), "").await.2,
        initial.2
    );
    let pattern = vec![4660u16; 16];
    s.set(json!({"style_deltas":[{"pair":pair,"color":"#112233","width":2,"fill":{"kind":"pattern","rows":pattern}}]})).await;
    s.set(json!({"style_deltas":[{"pair":pair,"color":"#123456"}]}))
        .await;
    let custom = request(&h, &login, "GET", &path(&s, "calibre"), "").await;
    assert_eq!(custom.0, 400);
    assert_eq!(
        serde_json::from_str::<Value>(&custom.2).unwrap()["error"],
        "unsupported"
    );
    let custom = request(&h, &login, "GET", &path(&s, "native"), "").await;
    assert_eq!(custom.0, 200);
    let document: Value = serde_json::from_str(&custom.2).unwrap();
    assert_eq!(document["rows"][0]["fill"]["rows"], json!(pattern));
    assert_eq!(document["rows"][0]["width"], 2);
    assert_eq!(document["rows"][0]["color"], "#123456");
    assert_eq!(document["version"], 1);
    // Existing approved settings transport, no new file path or slot editor.
    // Both the bitmap and its reference must survive download/reload.
    let mut v2 = document.clone();
    v2["version"] = 2.into();
    v2["fill_slots"] =
        serde_json::to_value(floe_app_core::styles::presets::bundled().unwrap().fills).unwrap();
    let brick = v2["fill_slots"]
        .as_array()
        .unwrap()
        .iter()
        .position(|p| p["name"] == "brick")
        .unwrap();
    v2["fill_slots"][brick]["rows"] = json!(vec![0x4321; 16]);
    v2["rows"][0]["fill"] = Value::Null;
    v2["rows"][0]["fill_slot"] = "brick".into();
    let mut invalid = v2.clone();
    invalid["fill_slots"][18]["rows"][0] = 0.into();
    assert_eq!(
        request(
            &h,
            &login,
            "POST",
            &path(&s, "native"),
            &invalid.to_string()
        )
        .await
        .0,
        400
    );
    let prepared = request(&h, &login, "POST", &path(&s, "native"), &v2.to_string()).await;
    assert_eq!(prepared.0, 200, "{}", prepared.2);
    assert_eq!(
        request(&h, &login, "GET", &path(&s, "native"), "").await.2,
        custom.2
    );
    let prepared: Value = serde_json::from_str(&prepared.2).unwrap();
    s.apply(&prepared["prepared_token"], None).await;
    let saved = request(&h, &login, "GET", &path(&s, "native"), "").await;
    assert_eq!(saved.0, 200);
    assert_eq!(serde_json::from_str::<Value>(&saved.2).unwrap(), v2);
    assert_eq!(
        request(&h, &login, "GET", &path(&s, "calibre"), "").await.0,
        400
    );
    let prepared = request(&h, &login, "POST", &path(&s, "native"), &custom.2).await;
    assert_eq!(prepared.0, 200);
    let prepared: Value = serde_json::from_str(&prepared.2).unwrap();
    s.apply(&prepared["prepared_token"], None).await;
    assert_eq!(
        request(&h, &login, "GET", &path(&s, "native"), "").await.2,
        custom.2,
        "v1 load must discard slot references and retain literal values"
    );
    assert_eq!(
        request(
            &h,
            &login,
            "POST",
            &path(&s, "calibre"),
            &format!("#{}", "x".repeat(4 * 1024 * 1024))
        )
        .await
        .0,
        413
    );
    assert_eq!(
        h.call(&login, "DELETE", "/api/v1/session", Value::Null)
            .await
            .0,
        204
    );
    h.shutdown().await;
    assert_eq!(files(&root), before);
    assert_eq!(files(&source.with_extension("oas.floe")), cache_before);
    println!("RUST OWNER SETTINGS: ALL OK (auth, 32KiB preparation, atomic CAS, replay/stale, native/custom roundtrip, 4MiB cap, no source writes)");
}
