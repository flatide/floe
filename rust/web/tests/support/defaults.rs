use super::drc_isolation::ReviewSocket;
use super::*;

fn fixture(name: &str) -> PathBuf {
    let original = PathBuf::from(std::env::var_os("FLOE_OWNER_FIXTURE").unwrap());
    let root = original.parent().unwrap().join(name);
    fs::create_dir(&root).unwrap();
    let source = root.join("design.oas");
    fs::copy(original, &source).unwrap();
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
    source
}
async fn opened(h: &Harness, l: &Login) -> ReviewSocket {
    let id =
        h.call(l, "GET", "/api/v1/catalog", Value::Null).await.1["sources"][0]["source_id"].clone();
    assert_eq!(
        h.call(
            l,
            "POST",
            "/api/v1/operations",
            open("1", &id, "level", json!({"mode":"all"}))
        )
        .await
        .0,
        202
    );
    assert_eq!(h.finished(l, 1).await["phase"], "succeeded");
    ReviewSocket::new(h, l).await
}
fn prepare_body(s: &ReviewSocket) -> Value {
    json!({"view_id":s.hello["view_id"],"state_rev":s.state["state_rev"]})
}
async fn close(h: &Harness, l: &Login, s: ReviewSocket) {
    assert_eq!(
        h.call(
            l,
            "DELETE",
            &format!("/api/v1/views/{}", s.hello["view_id"].as_str().unwrap()),
            Value::Null
        )
        .await
        .0,
        202
    );
    s.closed().await;
}
fn approval(seq: &str, d: &Value) -> Value {
    json!({"seq":seq,"view_id":d["view_id"],"state_rev":d["state_rev"],"token":d["token"],"approve":true})
}
async fn result(h: &Harness, l: &Login, seq: &str) -> Value {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        assert!(Instant::now() < end);
        let (status, v) = h
            .call(l, "GET", &format!("/api/v1/defaults/{seq}"), Value::Null)
            .await;
        assert_eq!(status, 200);
        if matches!(
            v["phase"].as_str(),
            Some("succeeded" | "failed" | "cancelled")
        ) {
            return v;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}
fn contents(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_file())
        .map(|p| {
            let b = fs::read(&p).unwrap();
            (p, b)
        })
        .collect();
    out.sort();
    out
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "run tools/validate_owner_service.py on private sources"]
async fn owner_default_publication_requires_opt_in_and_single_use_approval() {
    let source = fixture("defaults");
    let target = source.with_extension("oas.layerprops");
    let original = fs::read(&source).unwrap();
    let cache = contents(&source.with_extension("oas.floe"));
    let h = Harness::start(std::slice::from_ref(&source), native()).await;
    let l = h.login().await;
    assert_eq!(
        h.call(&l, "GET", "/api/v1/capabilities", Value::Null)
            .await
            .1["design_defaults"],
        false
    );
    for (method, path) in [
        ("GET", "/api/v1/defaults"),
        ("POST", "/api/v1/defaults/prepare"),
        ("POST", "/api/v1/defaults"),
    ] {
        assert_eq!(h.call(&l, method, path, json!({})).await.0, 403);
    }
    assert!(!target.exists());
    h.shutdown().await;
    let h = Harness::configured(std::slice::from_ref(&source), native(), None, false, true).await;
    let l = h.login().await;
    let mut s = opened(&h, &l).await;
    assert_eq!(
        h.call(&l, "GET", "/api/v1/capabilities", Value::Null)
            .await
            .1["design_defaults"],
        true
    );
    assert_eq!(
        h.raw(
            "POST",
            "/api/v1/defaults/prepare",
            &[
                ("Origin", &format!("http://{}", h.addr)),
                ("Content-Type", "application/json")
            ],
            &prepare_body(&s).to_string()
        )
        .await
        .0,
        401
    );
    let mut arbitrary = prepare_body(&s);
    arbitrary["path"] = json!("/tmp/not-allowed");
    assert_eq!(
        h.call(&l, "POST", "/api/v1/defaults/prepare", arbitrary)
            .await
            .0,
        400
    );
    let (status, d) = h
        .call(&l, "POST", "/api/v1/defaults/prepare", prepare_body(&s))
        .await;
    assert_eq!(status, 200, "{d}");
    assert_eq!(d["name"], "design.oas.layerprops");
    assert_eq!(d["scope"], "shared_design_default");
    assert_eq!(d["replaces_existing"], false);
    assert!(!d
        .to_string()
        .contains(source.parent().unwrap().to_str().unwrap()));
    assert!(!target.exists());
    assert!(!source.with_extension("oas.layerprops.lock").exists());
    let mut denied = approval("1", &d);
    denied["approve"] = json!(false);
    assert_eq!(h.call(&l, "POST", "/api/v1/defaults", denied).await.0, 400);
    assert!(!target.exists());
    let requested = approval("1", &d);
    assert_eq!(
        h.call(&l, "POST", "/api/v1/defaults", requested.clone())
            .await
            .0,
        202
    );
    let complete = result(&h, &l, "1").await;
    assert_eq!(complete["phase"], "succeeded");
    assert_eq!(complete["published"], true);
    let text = fs::read_to_string(&target).unwrap();
    assert!(!text.is_empty());
    let before = fs::metadata(&target).unwrap().modified().unwrap();
    assert_eq!(
        h.call(&l, "POST", "/api/v1/defaults", requested.clone())
            .await
            .1,
        complete
    );
    assert_eq!(fs::metadata(&target).unwrap().modified().unwrap(), before);
    let mut conflict = requested;
    conflict["state_rev"] = json!("999");
    assert_eq!(
        h.call(&l, "POST", "/api/v1/defaults", conflict).await.0,
        409
    );
    assert_eq!(
        h.call(&l, "POST", "/api/v1/defaults", approval("2", &d))
            .await
            .0,
        410
    );
    let d = h
        .call(&l, "POST", "/api/v1/defaults/prepare", prepare_body(&s))
        .await
        .1;
    assert_eq!(d["replaces_existing"], true);
    // A new view revision consumes a recognized old approval without writing.
    s.set(json!({"mono":true})).await;
    assert_eq!(
        h.call(&l, "POST", "/api/v1/defaults", approval("2", &d))
            .await
            .0,
        409
    );
    assert_eq!(
        h.call(&l, "POST", "/api/v1/defaults", approval("2", &d))
            .await
            .0,
        410
    );
    let d = h
        .call(&l, "POST", "/api/v1/defaults/prepare", prepare_body(&s))
        .await
        .1;
    assert_eq!(
        h.call(
            &l,
            "POST",
            "/api/v1/defaults/revoke",
            json!({"token":d["token"]})
        )
        .await
        .0,
        204
    );
    assert_eq!(
        h.call(&l, "POST", "/api/v1/defaults", approval("2", &d))
            .await
            .0,
        410
    );
    let d = h
        .call(&l, "POST", "/api/v1/defaults/prepare", prepare_body(&s))
        .await
        .1;
    fs::write(&target, "external change").unwrap();
    assert_eq!(
        h.call(&l, "POST", "/api/v1/defaults", approval("2", &d))
            .await
            .0,
        202
    );
    let changed = result(&h, &l, "2").await;
    assert_eq!(changed["phase"], "failed");
    assert_eq!(changed["error"], "default_changed");
    assert_eq!(changed["published"], false);
    assert_eq!(fs::read_to_string(&target).unwrap(), "external change");
    // Cancel of a completed request reports the authoritative committed result.
    assert_eq!(
        h.call(&l, "POST", "/api/v1/defaults/1/cancel", json!({}))
            .await
            .1,
        complete
    );
    assert_eq!(fs::read(&source).unwrap(), original);
    assert_eq!(contents(&source.with_extension("oas.floe")), cache);
    drop(s);
    h.shutdown().await;
    println!("RUST OWNER DEFAULTS: ALL OK (opt-in/auth, read-only draft, explicit approval, replay/stale/revoke, conflict, protected source/cache)");
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "run tools/validate_owner_service.py on private sources"]
async fn registered_drc_cannot_be_replaced_by_a_design_default() {
    let source = fixture("defaults-drc");
    let target = source.with_extension("oas.layerprops");
    fs::write(
        &target,
        "TOP 1000\nWIDTH\n1 1 0\np 1 4\n0 0\n100 0\n100 20\n0 20\n",
    )
    .unwrap();
    let before = fs::read(&target).unwrap();
    let h = Harness::configured(
        std::slice::from_ref(&source),
        native(),
        Some((&target, None)),
        false,
        true,
    )
    .await;
    let l = h.login().await;
    let s = opened(&h, &l).await;
    assert_eq!(
        h.call(&l, "POST", "/api/v1/defaults/prepare", prepare_body(&s))
            .await
            .0,
        400
    );
    assert_eq!(fs::read(&target).unwrap(), before);
    drop(s);
    h.shutdown().await;
    println!("RUST OWNER DEFAULT PROTECTION: ALL OK (DRC registration is not a default output)");
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "run tools/validate_owner_service.py on private sources"]
async fn deck_default_approval_is_bound_to_the_open_mode() {
    let source = fixture("defaults-deck");
    let deck = source.parent().unwrap().join("mask.jb");
    fs::write(&deck, "MTITLE 1,Mask\nCHIP C\n$ (1,PATTERN,TC=design.oas,AD=0.001,LY={1},DT={0},UX=100,UY=100)\nROWS 0/0\n").unwrap();
    let h = Harness::configured(std::slice::from_ref(&deck), native(), None, false, true).await;
    let l = h.login().await;
    let s = opened(&h, &l).await;
    let (code, stale) = h
        .call(&l, "POST", "/api/v1/defaults/prepare", prepare_body(&s))
        .await;
    assert_eq!(code, 200, "{stale}");
    assert_eq!(stale["mode"], "level");
    assert_eq!(stale["name"], "mask.jb.layerprops");
    close(&h, &l, s).await;
    let id = h.call(&l, "GET", "/api/v1/catalog", Value::Null).await.1["sources"][0]["source_id"]
        .clone();
    for (i, mode, filename) in [
        (0, "chip", "mask.chip-by-level.jb.layerprops"),
        (1, "layer", "mask.layer.jb.layerprops"),
    ] {
        let seq = (i + 2).to_string();
        assert_eq!(
            h.call(
                &l,
                "POST",
                "/api/v1/operations",
                open(&seq, &id, mode, json!({"mode":"only","ids":["1"]}))
            )
            .await
            .0,
            202
        );
        let complete = h.finished(&l, i + 2).await;
        assert_eq!(complete["phase"], "succeeded", "{complete}");
        let s = ReviewSocket::new(&h, &l).await;
        if i == 0 {
            assert_eq!(
                h.call(&l, "POST", "/api/v1/defaults", approval("1", &stale))
                    .await
                    .0,
                404
            );
            assert_eq!(
                h.call(&l, "POST", "/api/v1/defaults", approval("1", &stale))
                    .await
                    .0,
                410
            );
            assert!(!deck.with_extension("jb.layerprops").exists());
        }
        let (code, d) = h
            .call(&l, "POST", "/api/v1/defaults/prepare", prepare_body(&s))
            .await;
        assert_eq!(code, 200, "{d}");
        assert_eq!(d["mode"], mode);
        assert_eq!(d["levels"], json!(["1"]));
        assert_eq!(d["name"], filename);
        assert!(d["rows"].as_u64().unwrap() > 0);
        let seq = (i + 1).to_string();
        assert_eq!(
            h.call(&l, "POST", "/api/v1/defaults", approval(&seq, &d))
                .await
                .0,
            202
        );
        assert_eq!(result(&h, &l, &seq).await["published"], true);
        assert!(!fs::read_to_string(deck.parent().unwrap().join(filename))
            .unwrap()
            .is_empty());
        close(&h, &l, s).await;
    }
    assert!(!deck.with_extension("jb.layerprops").exists());
    h.shutdown().await;
    println!("RUST OWNER DECK DEFAULTS: ALL OK (mode/reopen invalidation, selected-level preview, separate chip/layer targets)");
}
