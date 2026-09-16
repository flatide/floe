//! Runs in the existing offline native owner gate, on a separate private copy.
use super::*;
use floe_app_core::{jobdeck::color::Mode, managed::ManagedDataset};

#[tokio::test(flavor = "current_thread")]
#[ignore = "run tools/validate_owner_service.py with private source files"]
async fn waive_refresh_fences_http_cursors_and_prepared_websocket_focus() {
    use floe_app_core::drc::review::{
        managed::{ManagedStore, Registration},
        store::Kind,
    };
    let fixture = PathBuf::from(std::env::var_os("FLOE_OWNER_FIXTURE").unwrap());
    let root = fixture.parent().unwrap().join("drc-waive-revision");
    fs::create_dir(&root).unwrap();
    let source = root.join("design.oas");
    fs::copy(&fixture, &source).unwrap();
    let indexer = std::env::var_os("FLOE_INDEX_BIN").unwrap();
    assert!(std::process::Command::new(&indexer)
        .arg("vfs")
        .arg(&source)
        .arg(source.with_extension("oas.floe"))
        .args(["--jobs", "2"])
        .output()
        .unwrap()
        .status
        .success());
    let db = root.join("review.db");
    fs::write(
        &db,
        "TOP 1000\nWIDTH\n1 1 0\np 1 4\n0 0\n100 0\n100 20\n0 20\n",
    )
    .unwrap();
    assert!(std::process::Command::new(&indexer)
        .arg("drc")
        .arg(&db)
        .args(["--jobs", "2"])
        .output()
        .unwrap()
        .status
        .success());
    let pack = db.with_file_name(".review.db.tray");
    let original = fs::read(&pack).unwrap();
    let h =
        Harness::start_with_drc(std::slice::from_ref(&source), native(), Some((&pack, None))).await;
    let login = h.login().await;
    let reader = h.drc_reader.as_ref().unwrap();
    reader
        .submit(serde_json::from_value(json!({"kind":"rule","check":"0"})).unwrap())
        .unwrap()
        .result()
        .await
        .unwrap();
    let old = h.call(&login, "GET", "/api/v1/drc", Value::Null).await.1["drc"].clone();
    let source_id = h.service.catalog()["sources"][0]["source_id"].clone();
    assert_eq!(
        h.call(
            &login,
            "POST",
            "/api/v1/operations",
            open("1", &source_id, "level", json!({"mode":"all"}))
        )
        .await
        .0,
        202
    );
    assert_eq!(h.finished(&login, 1).await["phase"], "succeeded");
    let mut socket = ReviewSocket::new(&h, &login).await;
    let token = prepare(&h, &login, &old, &socket, 0, true).await["prepared_token"].clone();
    let before = socket.state.clone();
    let prefix = format!(
        "/api/v1/drc/{}/views/{}",
        reader.id,
        socket.hello["view_id"].as_str().unwrap()
    );
    let panel_path = format!("{prefix}/panel");
    let selection_path = format!("{prefix}/selection");
    let panel = json!({"revision":old["revision"],"base_panel_rev":"1","body":{
        "search":"saved filter","rule_start":"0","check":"0","error_start":"0","query":null,
        "waived":false,"selected":{"check":"0","error":"0"},"markers":true,"shown":true,
        "jump_scale":null,"zoom_lock":false,"jump_active":false,"focus_visible":false}});
    assert_eq!(
        h.call(&login, "POST", &panel_path, panel.clone()).await.0,
        200
    );
    let selection = json!({"revision":old["revision"],"base_selection_rev":"1",
        "body":{"kind":"apply","check":"0","errors":["0"],"mode":"replace","waived":false}});
    assert_eq!(
        h.call(&login, "POST", &selection_path, selection.clone())
            .await
            .1["state"]["total"],
        "1"
    );
    let stop = Arc::new(AtomicUsize::new(0));
    let store = ManagedStore::open(
        &h.resources,
        Registration {
            scope: AccessScope::new(std::slice::from_ref(&root)).unwrap(),
            pack: pack.clone(),
            reviewer: "synthetic-revision".into(),
            kind: Kind::Waives,
            protected_files: vec![],
            protected_trees: vec![],
        },
        &stop,
    )
    .unwrap();
    let mut job = store
        .snapshot(Arc::clone(&stop))
        .unwrap()
        .prepare_waives(&[(0, 1)])
        .unwrap()
        .publish(false)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !job.is_finished() {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    job.close().unwrap();
    assert!(job.status().outcome.is_some());
    drop(job);
    reader
        .apply_waives(store.snapshot(Arc::clone(&stop)).unwrap())
        .unwrap()
        .result()
        .await
        .unwrap();
    let fresh = h.call(&login, "GET", "/api/v1/drc", Value::Null).await.1["drc"].clone();
    assert_eq!(fresh["id"], old["id"]);
    assert_eq!(fresh["phase"], "ready");
    assert_ne!(fresh["revision"], old["revision"]);
    let path = format!("/api/v1/drc/{}/read", reader.id);
    let mut read = json!({"view_id":socket.hello["view_id"],"revision":old["revision"],
        "body":{"kind":"filtered_step","check":"0","backwards":false,"in_view":false,"waived":false,
        "cursor":{"next":"0","remaining":"1"}}});
    for (path, body) in [
        (&path, read.clone()),
        (&panel_path, panel),
        (&selection_path, selection),
    ] {
        let (code, value) = h.call(&login, "POST", path, body).await;
        assert_eq!(code, 409, "{value}");
        assert_eq!(value["error"], "drc_context_changed");
    }
    socket.apply(&token, Some("drc_context_changed")).await;
    for key in ["view_id", "state_rev", "render_rev", "bbox_dbu", "layers"] {
        assert_eq!(socket.state[key], before[key], "{key}");
    }
    let state = h.call(&login, "GET", &panel_path, Value::Null).await.1;
    assert_eq!(state["revision"], fresh["revision"]);
    assert_eq!(state["state"]["body"], Value::Null);
    assert_eq!(
        h.call(&login, "GET", &selection_path, Value::Null).await.1["state"]["total"],
        "0"
    );
    read["revision"] = fresh["revision"].clone();
    read["body"] = json!({"kind":"errors","check":"0","start":"0","waived":true,"limit":64});
    let (code, value) = h.call(&login, "POST", &path, read).await;
    assert_eq!(code, 200, "{value}");
    assert_eq!(value["rows"][0]["status"], 1);
    let focus = prepare(&h, &login, &fresh, &socket, 0, true).await;
    socket.apply(&focus["prepared_token"], None).await;
    assert_eq!(fs::read(&pack).unwrap(), original);
    socket.socket.close(None).await.unwrap();
    drop(store);
    h.shutdown().await;
    println!("RUST DRC WAIVE REVISION: ALL OK (HTTP cursors/panel/groups, stale WS focus, new statuses, same geometry/view)");
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "run tools/validate_owner_service.py with private source files"]
async fn explicit_pack_build_retires_prepared_focus_without_reopening_layout() {
    let fixture = PathBuf::from(std::env::var_os("FLOE_OWNER_FIXTURE").unwrap());
    let root = fixture.parent().unwrap().join("drc-build-identity");
    fs::create_dir(&root).unwrap();
    let source = root.join("design.oas");
    fs::copy(&fixture, &source).unwrap();
    let indexer = std::env::var_os("FLOE_INDEX_BIN").unwrap();
    assert!(std::process::Command::new(&indexer)
        .arg("vfs")
        .arg(&source)
        .arg(source.with_extension("oas.floe"))
        .args(["--jobs", "2"])
        .output()
        .unwrap()
        .status
        .success());
    let db = root.join("review.db");
    fs::write(
        &db,
        "TOP 1000\nWIDTH\n1 1 0\np 1 4\n0 0\n100 0\n100 20\n0 20\n",
    )
    .unwrap();
    for writable in [false, true] {
        let h = Harness::start_with_drc_builds(
            std::slice::from_ref(&source),
            native(),
            Some((&db, None)),
            writable,
        )
        .await;
        let login = h.login().await;
        let source_id = h
            .call(&login, "GET", "/api/v1/catalog", Value::Null)
            .await
            .1["sources"][0]["source_id"]
            .clone();
        let deadline = Instant::now() + Duration::from_secs(5);
        let drc = loop {
            assert!(Instant::now() < deadline);
            let catalog = h.call(&login, "GET", "/api/v1/drc", Value::Null).await.1;
            assert_eq!(catalog["build"]["available"], writable);
            let d = catalog["drc"].clone();
            if d["phase"] == "ready" {
                break d;
            }
            assert_eq!(d["phase"], "opening");
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        assert_eq!(
            h.call(
                &login,
                "POST",
                "/api/v1/operations",
                open("1", &source_id, "level", json!({"mode":"all"}))
            )
            .await
            .0,
            202
        );
        assert_eq!(h.finished(&login, 1).await["phase"], "succeeded");
        let mut socket = ReviewSocket::new(&h, &login).await;
        let token = prepare(&h, &login, &drc, &socket, 0, true).await["prepared_token"].clone();
        let before = socket.state.clone();
        let req = json!({"seq":"1","drc_id":drc["id"],"revision":drc["revision"],"view_id":socket.hello["view_id"],"approve":true,"force":false,"jobs":2});
        let (code, value) = h
            .call(&login, "POST", "/api/v1/drc/builds", req.clone())
            .await;
        assert_eq!(code, if writable { 202 } else { 403 }, "{value}");
        if writable {
            socket.apply(&token, Some("prepared_edit_expired")).await;
            for key in ["view_id", "state_rev", "render_rev", "bbox_dbu", "layers"] {
                assert_eq!(socket.state[key], before[key], "{key}");
            }
            let deadline = Instant::now() + Duration::from_secs(10);
            let built = loop {
                assert!(Instant::now() < deadline, "pack build terminal deadline");
                let v = h
                    .call(&login, "GET", "/api/v1/drc/builds/1", Value::Null)
                    .await
                    .1;
                if v["phase"] == "succeeded" {
                    break v;
                }
                assert!(
                    !["failed", "cancelled"].iter().any(|p| v["phase"] == *p),
                    "{v}"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            };
            assert_eq!(
                h.call(&login, "POST", "/api/v1/drc/builds", req).await.1,
                built
            );
            let deadline = Instant::now() + Duration::from_secs(5);
            let adopted = loop {
                assert!(Instant::now() < deadline);
                let d = h.call(&login, "GET", "/api/v1/drc", Value::Null).await.1["drc"].clone();
                if d["phase"] == "ready" {
                    break d;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            };
            assert_ne!(adopted["id"], drc["id"]);
            assert_ne!(adopted["revision"], drc["revision"]);
            assert_eq!(adopted["metadata"]["format"], "ice");
            assert_eq!(h.resources.usage().index_jobs, 0);
            assert_eq!(h.resources.usage().cpu_slots, 3);
            let fresh = prepare(&h, &login, &adopted, &socket, 0, true).await;
            socket.apply(&fresh["prepared_token"], None).await;
            assert_ne!(socket.state["state_rev"], before["state_rev"]);
        } else {
            assert!(!db.with_file_name(".review.db.tray").exists());
            // A denied write does not revoke the read-only owner's valid move.
            socket.apply(&token, None).await;
        }
        socket.socket.close(None).await.unwrap();
        h.shutdown().await;
    }
    println!("RUST DRC BUILD IDENTITY: ALL OK (readonly deny, stale WebSocket token, new focus, same layout, resources)");
}

pub(super) struct ReviewSocket {
    socket: Socket,
    pub(super) hello: Value,
    pub(super) state: Value,
    seq: u64,
}
impl ReviewSocket {
    pub(super) async fn closed(mut self) {
        super::closed(&mut self.socket).await;
    }
    pub(super) async fn new(h: &Harness, login: &Login) -> Self {
        let mut socket = h.connect(login).await;
        let hello = text(&mut socket).await;
        let state = text(&mut socket).await;
        assert_eq!(hello["type"], "hello");
        assert_eq!(state["type"], "snapshot");
        Self {
            socket,
            hello,
            state,
            seq: 0,
        }
    }
    async fn read(&mut self) -> Value {
        loop {
            match timeout(Duration::from_secs(10), self.socket.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap()
            {
                Message::Text(t) => return serde_json::from_str(&t).unwrap(),
                Message::Ping(p) => self.socket.send(Message::Pong(p)).await.unwrap(),
                Message::Binary(bytes) => {
                    let n = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
                    let header: Value = serde_json::from_slice(&bytes[4..4 + n]).unwrap();
                    self.seq += 1;
                    self.socket.send(Message::Text(json!({"type":"frame.ack","seq":self.seq.to_string(),"connection_epoch":self.hello["connection_epoch"],"frame_id":header["frame_id"],"disposition":"discarded"}).to_string().into())).await.unwrap();
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    pub(super) async fn edit(&mut self, mut value: Value, error: Option<&str>) -> Value {
        // Exercise the protocol below its documented rate cap, including ACKs.
        tokio::time::sleep(Duration::from_millis(50)).await;
        self.seq += 1;
        let seq = self.seq.to_string();
        value["seq"] = json!(seq);
        value["view_id"] = self.hello["view_id"].clone();
        value["connection_epoch"] = self.hello["connection_epoch"].clone();
        if value.get("base_state_rev").is_none() {
            value["base_state_rev"] = self.state["state_rev"].clone();
        }
        self.socket
            .send(Message::Text(value.to_string().into()))
            .await
            .unwrap();
        loop {
            let v = self.read().await;
            if v["seq"] == seq {
                if let Some(error) = error {
                    assert_eq!(v["type"], "error");
                    assert_eq!(v["code"], error);
                } else {
                    assert_eq!(v["type"], "accepted", "{v}");
                }
                let state = self.read().await;
                assert_eq!(state["type"], "snapshot");
                self.state = state;
                return v;
            }
        }
    }
    pub(super) async fn set(&mut self, body: Value) {
        self.edit(json!({"type":"view.set","body":body}), None)
            .await;
    }
    pub(super) async fn apply(&mut self, token: &Value, error: Option<&str>) {
        self.edit(json!({"type":"view.apply","token":token}), error)
            .await;
    }
}
async fn prepare(
    h: &Harness,
    login: &Login,
    drc: &Value,
    s: &ReviewSocket,
    check: usize,
    isolate: bool,
) -> Value {
    let (code, value) = h.call(login, "POST", &format!("/api/v1/drc/{}/read", drc["id"].as_str().unwrap()),
        json!({"view_id":s.hello["view_id"],"revision":drc["revision"],"state_rev":s.state["state_rev"],"body":{"kind":"focus","check":check.to_string(),"error":"0","fit":true,"isolate":isolate}})).await;
    assert_eq!(code, 200, "{value}");
    assert!(value.to_string().len() < 1024);
    let actual = h.call(login, "GET", "/api/v1/view", Value::Null).await.1;
    assert_eq!(
        actual["view"]["state_rev"], s.state["state_rev"],
        "preparation edited the view"
    );
    value
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "run tools/validate_owner_service.py with private source files"]
async fn prepared_focus_is_atomic_scoped_and_restores_original_visibility_once() {
    let fixture = PathBuf::from(std::env::var_os("FLOE_OWNER_FIXTURE").unwrap());
    let root = fixture.parent().unwrap().join("drc-isolation");
    fs::create_dir(&root).unwrap();
    let source = root.join("design.oas");
    fs::copy(&fixture, &source).unwrap();
    let indexer = std::env::var_os("FLOE_INDEX_BIN").unwrap();
    let out = std::process::Command::new(&indexer)
        .args(["vfs"])
        .arg(&source)
        .arg(source.with_extension("oas.floe"))
        .args(["--jobs", "2"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let data = ManagedDataset::open(
        &Resources::new(Limits::default()).unwrap(),
        &source,
        None,
        Mode::Level,
        &AtomicUsize::new(0),
    )
    .unwrap();
    let pairs: Vec<_> = data
        .dataset
        .styles(false)
        .unwrap()
        .iter()
        .map(|s| s.layer)
        .collect();
    assert!(pairs.len() > 1);
    drop(data);
    let ascii = root.join("review.db");
    let names = ["FIRST", "SECOND", "UNKNOWN", "EMPTY", "UNMATCHED"];
    let mut text = "TOP 1000\n".to_owned();
    for name in names {
        text.push_str(&format!(
            "{name}\n1 1 1\nsynthetic rule\np 1 4\n0 0\n10 0\n10 20\n0 20\n"
        ));
    }
    fs::write(&ascii, text).unwrap();
    let out = std::process::Command::new(&indexer)
        .arg("drc")
        .arg(&ascii)
        .args(["--jobs", "2"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let pack = root.join(".review.db.tray");
    let original = fs::read(&pack).unwrap();
    let stamp = fs::metadata(&pack).unwrap().modified().unwrap();
    let rules = root.join("rules.json");
    fs::write(&rules, json!({"format":"floe-svrf-rules","version":1,"checks":{
        "FIRST":{"source_gds":[pairs[0]]},"SECOND":{"source_gds":[pairs[1]]},"EMPTY":{},"UNMATCHED":{"source_gds":[[u32::MAX,null]]}
    }}).to_string()).unwrap();
    for metadata in [Some(rules.as_path()), None] {
        let h = Harness::start_with_drc(
            std::slice::from_ref(&source),
            native(),
            Some((&pack, metadata)),
        )
        .await;
        let login = h.login().await;
        let catalog = h
            .call(&login, "GET", "/api/v1/catalog", Value::Null)
            .await
            .1;
        let source_id = &catalog["sources"][0]["source_id"];
        let deadline = Instant::now() + Duration::from_secs(5);
        let drc = loop {
            assert!(Instant::now() < deadline, "DRC actor open deadline");
            let v = h.call(&login, "GET", "/api/v1/drc", Value::Null).await.1["drc"].clone();
            if v["phase"] == "ready" {
                break v;
            }
            assert_eq!(v["phase"], "opening", "{v}");
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        assert_eq!(
            h.call(
                &login,
                "POST",
                "/api/v1/operations",
                open("1", source_id, "level", json!({"mode":"all"}))
            )
            .await
            .0,
            202
        );
        assert_eq!(h.finished(&login, 1).await["phase"], "succeeded");
        let mut s = ReviewSocket::new(&h, &login).await;
        assert_eq!(s.state["layers_isolated"], false);
        let legacy = prepare(&h, &login, &drc, &s, 0, false).await;
        assert!(legacy.get("prepared_token").is_none());
        let first = prepare(&h, &login, &drc, &s, 0, true).await;
        if metadata.is_none() {
            assert_eq!(first["layer_isolation"]["status"], "no_metadata");
            s.apply(&first["prepared_token"], None).await;
            assert_eq!(s.state["layers_isolated"], false);
            assert_eq!(s.state["layers"]["mode"], "all");
        } else {
            assert_eq!(
                first["layer_isolation"],
                json!({"status":"ready","matched":"1"})
            );
            let second = prepare(&h, &login, &drc, &s, 1, true).await;
            let before = s.state.clone();
            s.apply(&first["prepared_token"], Some("prepared_edit_expired"))
                .await;
            assert_eq!(s.state["state_rev"], before["state_rev"]);
            s.apply(&json!("wrong-token"), Some("prepared_edit_expired"))
                .await;
            s.apply(&second["prepared_token"], None).await;
            assert_eq!(s.state["state_rev"], "2");
            assert_eq!(
                s.state["render_rev"], "2",
                "isolate and goto rendered separately"
            );
            assert_eq!(s.state["layers_isolated"], true);
            assert_eq!(s.state["layers"], json!({"mode":"only","pairs":[pairs[1]]}));
            s.apply(&second["prepared_token"], Some("prepared_edit_expired"))
                .await;
            let stale = prepare(&h, &login, &drc, &s, 0, true).await;
            let stale_base = s.state["state_rev"].clone();
            s.set(json!({"thin":"cull"})).await;
            s.edit(json!({"type":"view.apply","token":stale["prepared_token"],"base_state_rev":stale_base}), Some("stale_state")).await;
            s.apply(&stale["prepared_token"], Some("prepared_edit_expired"))
                .await;
            assert_eq!(s.state["layers"]["pairs"], json!([pairs[1]]));
            let next = prepare(&h, &login, &drc, &s, 0, true).await;
            s.apply(&next["prepared_token"], None).await;
            assert_eq!(s.state["layers"]["pairs"], json!([pairs[0]]));
            for (check, reason) in [(2, "no_rule"), (3, "no_source_layers"), (4, "no_match")] {
                let next = prepare(&h, &login, &drc, &s, check, true).await;
                assert_eq!(next["layer_isolation"]["status"], reason);
                s.apply(&next["prepared_token"], None).await;
                assert_eq!(s.state["layers"]["pairs"], json!([pairs[0]]));
                assert_eq!(s.state["layers_isolated"], true);
            }
            s.set(json!({"layers":{"mode":"none"}})).await;
            let center = s.state["bbox_dbu"].clone();
            s.socket.close(None).await.unwrap();
            s = ReviewSocket::new(&h, &login).await;
            assert_eq!(
                s.state["layers_isolated"], true,
                "reload lost restore snapshot"
            );
            s.set(json!({"restore_layers":true})).await;
            assert_eq!(s.state["layers_isolated"], false);
            assert_eq!(s.state["layers"], json!({"mode":"all"}));
            assert_eq!(s.state["thin"], "cull");
            assert_eq!(s.state["bbox_dbu"], center);
            let rev = s.state["state_rev"].clone();
            s.set(json!({"restore_layers":true})).await;
            assert_eq!(s.state["state_rev"], rev);
            // A token belongs to exactly one attachment, not the reopened source.
            let previous = prepare(&h, &login, &drc, &s, 0, true).await;
            assert_eq!(
                h.call(
                    &login,
                    "DELETE",
                    &format!("/api/v1/views/{}", s.hello["view_id"].as_str().unwrap()),
                    Value::Null
                )
                .await
                .0,
                202
            );
            closed(&mut s.socket).await;
            assert_eq!(
                h.call(
                    &login,
                    "POST",
                    "/api/v1/operations",
                    open("2", source_id, "level", json!({"mode":"all"}))
                )
                .await
                .0,
                202
            );
            let reopened = h.finished(&login, 2).await;
            assert_eq!(reopened["phase"], "succeeded", "{reopened}");
            s = ReviewSocket::new(&h, &login).await;
            s.apply(&previous["prepared_token"], Some("prepared_edit_expired"))
                .await;
            assert_eq!(s.state["layers_isolated"], false);
        }
        s.socket.close(None).await.unwrap();
        h.shutdown().await;
        assert_eq!(fs::read(&pack).unwrap(), original);
        assert_eq!(fs::metadata(&pack).unwrap().modified().unwrap(), stamp);
    }
    println!("RUST DRC ISOLATION: ALL OK (prepare/CAS/replay/reload/restore/no-match/no writes)");
}
