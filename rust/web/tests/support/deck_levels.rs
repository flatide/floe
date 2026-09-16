//! Actual owner HTTP/WS/native cutovers, never a fit followed by a goto.
use super::{
    drc_isolation::ReviewSocket,
    index_open::{area, operation, paused_pid, request, signal, state, target},
    *,
};

fn fixture(dir: &Path) -> PathBuf {
    let path = dir.join("levels.jb");
    let mut deck = String::new();
    for (id, name) in [(1, "A"), (2, "B"), (3, "C")] {
        deck.push_str(&format!("MTITLE {id},MASK-{name}\nCHIP {name}\n$ ({id},{name},TC={name}.oas,AD=0.001,LY={{1}},DT={{0}},UX=100,UY=100)\nROWS {}/{}\n", id*500, id*200));
    }
    fs::write(&path, deck).unwrap();
    path
}
fn select(seq: u64, view: &Value, levels: Value) -> Value {
    json!({"kind":"reselect_levels","seq":seq.to_string(),"view_id":view["view_id"],
        "base_state_rev":view["state_rev"],"levels":levels})
}
fn initial(source: &Value, seq: &str) -> Value {
    json!({"kind":"open","seq":seq,"source_id":source,"mode":"chip",
        "levels":{"mode":"only","ids":["1"]},"body":{"pixels":[137,103],
        "navigation":{"kind":"goto","center_um":["501.25","207.5"],"width_um":"123.75"},
        "detail":"high","thin":"keep","frames":false,"font_px":23}})
}
async fn first(h: &Harness, login: &Login, camera: Option<&Value>) -> Value {
    let mut socket = h.connect(login).await;
    let (_, header) = frame(&mut socket).await;
    assert_eq!(
        header["generation"], "1",
        "camera corrected by a later generation: {header}"
    );
    let current = state(h, login).await;
    assert_eq!(header["bbox_dbu"], current["bbox_dbu"]);
    if let Some(old) = camera {
        for key in [
            "bbox_dbu",
            "pixels",
            "camera_um",
            "detail",
            "thin",
            "frames",
            "font_px",
        ] {
            assert_eq!(current[key], old[key], "reselection changed {key}");
        }
        assert_ne!(current["view_id"], old["view_id"]);
    }
    assert_eq!(h.resources.usage().workers, 1);
    socket.close(None).await.unwrap();
    current
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "run tools/validate_owner_service.py on private fixtures"]
async fn reselect_cached_levels_preserves_first_camera_replay_and_one_worker() {
    let dir = area("reselect-cached");
    let deck = fixture(&dir);
    fs::write(&deck, format!("{}\nCHIP BAD\n$ (4,BAD,TC=A.oas,AD=0.001,LY={{999}},DT={{0}},UX=100,UY=100)\nROWS 0/0\n", fs::read_to_string(&deck).unwrap())).unwrap();
    let h = Harness::configured_limits(
        &[deck],
        native(),
        None,
        false,
        false,
        Limits {
            workers: 1,
            ..Limits::default()
        },
    )
    .await;
    let login = h.login().await;
    let source = h.service.catalog()["sources"][0]["source_id"].clone();
    assert_eq!(
        h.call(&login, "GET", "/api/v1/capabilities", Value::Null)
            .await
            .1["jobdeck_levels"],
        true
    );
    assert_eq!(
        operation(
            &h,
            &login,
            json!({"kind":"index","seq":"1","source_id":source,
        "levels":{"mode":"only","ids":["1","2"]},"options":{"jobs":2}})
        )
        .await["phase"],
        "succeeded"
    );
    assert_eq!(
        operation(&h, &login, initial(&source, "2")).await["phase"],
        "succeeded"
    );
    let old = first(&h, &login, None).await;
    let chosen = select(3, &old, json!({"mode":"only","ids":["2"]}));
    let done = operation(&h, &login, chosen.clone()).await;
    assert_eq!(done["kind"], "reselect_levels");
    assert_eq!(done["phase"], "succeeded", "{done}");
    let current = first(&h, &login, Some(&old)).await;
    let info = h.call(&login, "GET", "/api/v1/view", Value::Null).await.1;
    assert_eq!(info["levels"], json!(["2"]));
    assert_eq!(info["mode"], "chip");
    assert_eq!(
        h.call(&login, "POST", "/api/v1/operations", chosen.clone())
            .await
            .1,
        done
    );
    let mut conflict = chosen;
    conflict["levels"] = json!({"mode":"all"});
    assert_eq!(
        h.call(&login, "POST", "/api/v1/operations", conflict)
            .await
            .0,
        409
    );
    let same = operation(
        &h,
        &login,
        select(4, &current, json!({"mode":"only","ids":["2"]})),
    )
    .await;
    assert_eq!(same["reused"], true);
    let after = state(&h, &login).await;
    for key in ["view_id", "state_rev", "render_key", "bbox_dbu"] {
        assert_eq!(after[key], current[key]);
    }
    let mut edits = ReviewSocket::new(&h, &login).await;
    edits.set(json!({"navigation":{"kind":"pan","x":0.1,"y":0.0,"snap":false},"layers":{"mode":"none"}})).await;
    let stale = h
        .call(
            &login,
            "POST",
            "/api/v1/operations",
            select(5, &current, json!({"mode":"only","ids":["1","2"]})),
        )
        .await;
    assert_eq!(stale.0, 429);
    assert_eq!(stale.1["error"], "busy");
    let moved = state(&h, &login).await;
    assert_ne!(moved["bbox_dbu"], current["bbox_dbu"]);
    let changed = operation(
        &h,
        &login,
        select(5, &moved, json!({"mode":"only","ids":["1","2"]})),
    )
    .await;
    assert_eq!(changed["phase"], "succeeded", "{changed}");
    let both = first(&h, &login, Some(&moved)).await;
    assert_ne!(
        both["layers"],
        json!({"mode":"none"}),
        "old synthetic layer IDs leaked into new levels"
    );
    for levels in [
        json!({"mode":"only","ids":[]}),
        json!({"mode":"only","ids":["999"]}),
    ] {
        assert_eq!(
            h.call(
                &login,
                "POST",
                "/api/v1/operations",
                select(6, &both, levels)
            )
            .await
            .0,
            400
        );
    }
    assert_eq!(state(&h, &login).await["view_id"], both["view_id"]);
    let mode = operation(
        &h,
        &login,
        json!({"kind":"mode","seq":"6","view_id":both["view_id"],
        "base_state_rev":both["state_rev"],"mode":"layer"}),
    )
    .await;
    assert_eq!(mode["phase"], "succeeded");
    let raw = first(&h, &login, Some(&both)).await;
    assert_eq!(
        operation(
            &h,
            &login,
            select(7, &raw, json!({"mode":"only","ids":["1"]}))
        )
        .await["phase"],
        "succeeded"
    );
    let current = first(&h, &login, Some(&raw)).await;
    assert_eq!(
        h.call(&login, "GET", "/api/v1/view", Value::Null).await.1["mode"],
        "layer"
    );
    assert!(!cache::cache_path(&dir.join("C.oas")).unwrap().exists());
    let failed = operation(
        &h,
        &login,
        select(8, &current, json!({"mode":"only","ids":["4"]})),
    )
    .await;
    assert_eq!(failed["phase"], "failed", "{failed}");
    assert!(
        failed.get("index_open").is_none(),
        "unselected missing C must not turn a bad layer into a write proposal"
    );
    assert_eq!(state(&h, &login).await["view_id"], current["view_id"]);
    // Cancel may race an already committed cutover, but must never report a
    // cancelled operation after retiring the original view.
    assert_eq!(
        h.call(
            &login,
            "POST",
            "/api/v1/operations",
            select(9, &current, json!({"mode":"only","ids":["2"]}))
        )
        .await
        .0,
        202
    );
    assert_eq!(
        h.call(&login, "POST", "/api/v1/operations/9/cancel", json!({}))
            .await
            .0,
        202
    );
    let result = h.finished(&login, 9).await;
    if result["phase"] == "succeeded" {
        first(&h, &login, Some(&current)).await;
    } else {
        assert_eq!(result["phase"], "cancelled", "{result}");
        assert_eq!(state(&h, &login).await["view_id"], current["view_id"]);
    }
    h.shutdown().await;
    println!("RUST DECK LEVELS: ALL OK (camera in first frame, selected set, mode/defaults, no-op, retired-view replay, stale/invalid rejection, one worker)");
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "run tools/validate_owner_service.py on private fixtures"]
async fn reselect_index_retry_is_frozen_and_preserves_the_live_view_on_pan_and_cancel() {
    let dir = area("reselect-index");
    let a = dir.join("A.oas");
    let real = native();
    assert!(std::process::Command::new(real.path())
        .arg("vfs")
        .arg(&a)
        .arg(cache::cache_path(&a).unwrap())
        .args(["--jobs", "2", "--occupancy"])
        .output()
        .unwrap()
        .status
        .success());
    let binary = dir.join("controlled-indexer");
    let quoted = real.path().to_str().unwrap().replace('\'', "'\\''");
    fs::write(&binary,format!("#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then printf 'floe-index {}\\n'; exit 0; fi\nprintf '%s' \"$$\" > \"$2.started\"\nkill -STOP \"$$\"\nexec '{}' \"$@\"\n",INDEX_VERSION,quoted)).unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
    let indexer = Indexer::discover(&Discovery {
        override_path: Some(binary),
        development_root: None,
        executable: PathBuf::from("/unused"),
        search_path: None,
    })
    .unwrap();
    let h = Harness::start(&[fixture(&dir)], indexer).await;
    let login = h.login().await;
    let source = h.service.catalog()["sources"][0]["source_id"].clone();
    assert_eq!(
        operation(&h, &login, initial(&source, "1")).await["phase"],
        "succeeded"
    );
    let old = first(&h, &login, None).await;
    let levels = json!({"mode":"only","ids":["1","2"]});
    let failed = operation(&h, &login, select(2, &old, levels.clone())).await;
    assert_eq!(failed["phase"], "failed", "{failed}");
    assert_eq!(failed["error"], "index_unavailable");
    assert_eq!(state(&h, &login).await["view_id"], old["view_id"]);
    let preview = h
        .call(
            &login,
            "GET",
            "/api/v1/operations/2/index-open",
            Value::Null,
        )
        .await
        .1;
    assert_eq!(preview["reselect"]["target"], target(&old));
    assert_eq!(preview["reselect"]["pixels"], old["pixels"]);
    assert_eq!(preview["levels"], levels);
    let approved = request(3, 2, target(&old));
    for (field, value) in [
        ("target", json!({"kind":"empty"})),
        ("pixels", json!([200, 103])),
    ] {
        let mut changed = approved.clone();
        changed[field] = value;
        assert_eq!(
            h.call(&login, "POST", "/api/v1/operations", changed)
                .await
                .0,
            400
        );
    }
    let mut edits = ReviewSocket::new(&h, &login).await;
    edits
        .set(json!({"navigation":{"kind":"pan","x":0.1,"y":0.0,"snap":false}}))
        .await;
    let moved = state(&h, &login).await;
    let stale = operation(&h, &login, approved).await;
    assert_eq!(stale["phase"], "failed");
    assert_eq!(stale["stage"], "index");
    assert_eq!(stale["error"], "busy");
    assert!(!dir.join("B.oas.started").exists());
    assert!(!dir.join(".B.oas.ice.index.lock").exists());
    let reselect = operation(&h, &login, select(4, &moved, levels)).await;
    assert_eq!(reselect["error"], "index_unavailable");
    let index = request(5, 4, target(&moved));
    assert_eq!(
        h.call(&login, "POST", "/api/v1/operations", index.clone())
            .await
            .0,
        202
    );
    let pid = paused_pid(&dir.join("B.oas.started")).await;
    // Server revision changes while the native writer is deliberately paused.
    edits.set(json!({"pixels":[139,105]})).await;
    let resized = state(&h, &login).await;
    assert!(signal(pid, "-CONT"));
    let stale = h.finished(&login, 5).await;
    assert_eq!(stale["phase"], "failed", "{stale}");
    assert_eq!(stale["stage"], "open");
    assert_eq!(stale["index"]["phase"], "succeeded");
    assert_eq!(stale["error"], "busy");
    assert_eq!(state(&h, &login).await["view_id"], old["view_id"]);
    assert_eq!(
        h.call(&login, "POST", "/api/v1/operations", index).await.1,
        stale
    );
    assert!(dir.join(".B.oas.ice/design.ovo").is_file());
    let attached = operation(
        &h,
        &login,
        select(6, &resized, json!({"mode":"only","ids":["1","2"]})),
    )
    .await;
    assert_eq!(attached["phase"], "succeeded", "{attached}");
    let current = first(&h, &login, Some(&resized)).await;
    assert_eq!(
        operation(&h, &login, select(7, &current, json!({"mode":"all"}))).await["error"],
        "index_unavailable"
    );
    let mut approved = request(8, 7, target(&current));
    approved["pixels"] = current["pixels"].clone();
    assert_eq!(
        h.call(&login, "POST", "/api/v1/operations", approved)
            .await
            .0,
        202
    );
    let pid = paused_pid(&dir.join("C.oas.started")).await;
    assert_eq!(
        h.call(&login, "POST", "/api/v1/operations/8/cancel", json!({}))
            .await
            .0,
        202
    );
    let cancelled = h.finished(&login, 8).await;
    assert_eq!(cancelled["phase"], "cancelled", "{cancelled}");
    assert!(!signal(pid, "-0"));
    assert_eq!(state(&h, &login).await["view_id"], current["view_id"]);
    assert_eq!(h.resources.usage().workers, 1);
    assert_eq!(h.resources.usage().index_jobs, 0);
    assert!(!dir.join(".C.oas.ice").exists());
    // A fresh explicit retry at the unchanged anchor succeeds; it does not
    // need to close the old A/B view to build C, or fit and navigate afterward.
    fs::remove_file(dir.join("C.oas.started")).unwrap();
    let mut retry = request(9, 7, target(&current));
    retry["pixels"] = current["pixels"].clone();
    assert_eq!(
        h.call(&login, "POST", "/api/v1/operations", retry.clone())
            .await
            .0,
        202
    );
    let pid = paused_pid(&dir.join("C.oas.started")).await;
    assert!(signal(pid, "-CONT"));
    let done = h.finished(&login, 9).await;
    assert_eq!(done["phase"], "succeeded", "{done}");
    assert_eq!(done["index"]["phase"], "succeeded");
    first(&h, &login, Some(&current)).await;
    assert_eq!(
        h.call(&login, "GET", "/api/v1/view", Value::Null).await.1["levels"],
        Value::Null
    );
    assert_eq!(
        h.call(&login, "POST", "/api/v1/operations", retry).await.1,
        done
    );
    h.shutdown().await;
    println!("RUST DECK LEVELS INDEX: ALL OK (no silent unindexed levels, fixed consent/target/pixels, pre-write stale rejection, post-index resize, completed caches retained, cancel/reap, old view preserved)");
}
