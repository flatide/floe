use super::{drc_isolation::ReviewSocket, *};

fn cache_dir(path: &Path) -> PathBuf {
    cache::cache_path(path).unwrap()
}

fn area(name: &str) -> PathBuf {
    let root = PathBuf::from(std::env::var_os("FLOE_OWNER_MODE_FIXTURE").unwrap());
    let dir = root.parent().unwrap().join(name);
    fs::create_dir(&dir).unwrap();
    for name in ["A", "B", "C"] {
        fs::copy(&root, dir.join(format!("{name}.oas"))).unwrap();
    }
    dir
}
fn request(seq: u64, original: u64, target: Value) -> Value {
    json!({"kind":"index_open","seq":seq.to_string(),"open_seq":original.to_string(),
        "request_id":format!("{seq:064x}"),"approved":true,"target":target,"pixels":[137,103],"options":{"jobs":2}})
}
async fn operation(h: &Harness, login: &Login, input: Value) -> Value {
    let seq = input["seq"].as_str().unwrap().parse().unwrap();
    assert_eq!(
        h.call(login, "POST", "/api/v1/operations", input).await.0,
        202
    );
    h.finished(login, seq).await
}
async fn state(h: &Harness, login: &Login) -> Value {
    h.call(login, "GET", "/api/v1/view", Value::Null).await.1["view"].clone()
}
fn target(view: &Value) -> Value {
    json!({"kind":"replace","view_id":view["view_id"],"state_rev":view["state_rev"]})
}
async fn failed(h: &Harness, login: &Login, input: Value) -> Value {
    let result = operation(h, login, input).await;
    assert_eq!(result["phase"], "failed", "{result}");
    assert_eq!(result["error"], "index_unavailable", "{result}");
    assert_eq!(result["index_open"]["open_seq"], result["seq"]);
    result
}

async fn failed_replacement(h: &Harness, login: &Login, source: &Value, seq: u64, old: &Value) {
    let (id, _) = h.launches.reserve().unwrap();
    h.launches
        .ready(
            &id,
            Some(json!({"kind":"open","seq":"1","source_id":source,
        "mode":"level","display_policy":"window","body":{}})),
            false,
        )
        .unwrap();
    assert_eq!(
        h.call(
            login,
            "POST",
            &format!("/api/v1/launch/{id}"),
            json!({"action":"open",
        "seq":seq.to_string(),"pixels":[137,103],"levels":{"mode":"all"},
        "view_id":old["view_id"],"state_rev":old["state_rev"]})
        )
        .await
        .0,
        200
    );
    let failure = h.finished(login, seq).await;
    assert_eq!(failure["error"], "index_unavailable", "{failure}");
}
async fn paused_pid(path: &Path) -> i32 {
    timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(s) = fs::read_to_string(path) {
                if let Ok(pid) = s.parse() {
                    return pid;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}
fn signal(pid: i32, flag: &str) -> bool {
    assert!(pid > 0);
    std::process::Command::new("/bin/kill")
        .args([flag, &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success()
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "run tools/validate_owner_service.py on private fixtures"]
async fn index_open_checks_revision_before_writes_and_after_index_and_reaps_on_cancel() {
    let dir = area("index-open-races");
    let a = dir.join("A.oas");
    let b = dir.join("B.oas");
    let c = dir.join("C.oas");
    let real = native();
    let built = std::process::Command::new(real.path())
        .arg("vfs")
        .arg(&a)
        .arg(cache_dir(&a))
        .args(["--jobs", "2", "--no-lod"])
        .output()
        .unwrap();
    assert!(built.status.success(), "{:?}", built);
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
    let h = Harness::start(&[a, b.clone(), c.clone()], indexer).await;
    let login = h.login().await;
    let sources = h.service.catalog()["sources"].as_array().unwrap().clone();
    assert_eq!(
        operation(
            &h,
            &login,
            open(
                "1",
                &sources[0]["source_id"],
                "level",
                json!({"mode":"all"})
            )
        )
        .await["phase"],
        "succeeded"
    );
    let mut ws = h.connect(&login).await;
    frame(&mut ws).await;
    ws.close(None).await.unwrap();
    let old = state(&h, &login).await;
    failed_replacement(&h, &login, &sources[1]["source_id"], 2, &old).await;
    let mut edits = ReviewSocket::new(&h, &login).await;
    edits.set(json!({"detail":"low"})).await;
    let stale = operation(&h, &login, request(3, 2, target(&old))).await;
    assert_eq!(stale["phase"], "failed");
    assert_eq!(stale["stage"], "index");
    assert_eq!(stale["error"], "busy");
    assert!(!cache_dir(&b).exists());
    assert!(!b.with_extension("oas.started").exists());
    let newer = state(&h, &login).await;
    assert_eq!(
        h.call(
            &login,
            "POST",
            "/api/v1/operations",
            request(4, 2, target(&newer))
        )
        .await
        .0,
        202
    );
    let pid = paused_pid(&b.with_extension("oas.started")).await;
    let running = h
        .call(&login, "GET", "/api/v1/operations/4", Value::Null)
        .await
        .1;
    assert_eq!(running["kind"], "index_open");
    assert_eq!(
        running["request_id"],
        request(4, 2, target(&newer))["request_id"]
    );
    let changed_frames = !newer["frames"].as_bool().unwrap();
    edits.set(json!({"frames":changed_frames})).await;
    let before_cutover = state(&h, &login).await;
    assert_ne!(before_cutover["state_rev"], newer["state_rev"]);
    assert!(signal(pid, "-CONT"));
    let declined = h.finished(&login, 4).await;
    assert_eq!(declined["phase"], "failed", "{declined}");
    assert_eq!(declined["stage"], "open");
    assert_eq!(declined["index"]["phase"], "succeeded");
    assert_eq!(declined["error"], "busy");
    assert_eq!(state(&h, &login).await["view_id"], old["view_id"]);
    assert!(cache_dir(&b).join("design.ovm").exists());
    let retry = operation(&h, &login, request(5, 2, target(&before_cutover))).await;
    assert_eq!(retry["phase"], "succeeded", "{retry}");
    assert_eq!(retry["index"]["kept"], 1);
    edits.closed().await;
    let mut ws = h.connect(&login).await;
    let (_, header) = frame(&mut ws).await;
    assert_eq!(header["generation"], "1");
    ws.close(None).await.unwrap();
    let current = state(&h, &login).await;
    assert_eq!(current["frames"], changed_frames);
    assert_eq!(current["detail"], "low");
    failed_replacement(&h, &login, &sources[2]["source_id"], 6, &current).await;
    let cancel = request(7, 6, target(&current));
    assert_eq!(
        h.call(&login, "POST", "/api/v1/operations", cancel.clone())
            .await
            .0,
        202
    );
    let pid = paused_pid(&c.with_extension("oas.started")).await;
    assert_eq!(
        h.call(&login, "POST", "/api/v1/operations/7/cancel", json!({}))
            .await
            .0,
        202
    );
    let cancelled = h.finished(&login, 7).await;
    assert_eq!(cancelled["request_id"], cancel["request_id"]);
    assert_eq!(cancelled["phase"], "cancelled");
    assert_eq!(cancelled["stage"], "index");
    assert!(
        !signal(pid, "-0"),
        "native child survived terminal cancellation"
    );
    assert_eq!(state(&h, &login).await["view_id"], current["view_id"]);
    assert_eq!(
        h.call(&login, "POST", "/api/v1/operations", cancel).await.1,
        cancelled
    );
    // Force cannot retire a live reader merely to make the writer lease fit.
    let bytes = fs::read(cache_dir(&b).join("design.ovm")).unwrap();
    let mut busy = request(8, 2, target(&current));
    busy["options"]["force"] = json!(true);
    let busy = operation(&h, &login, busy).await;
    assert_eq!(busy["phase"], "failed");
    assert_eq!(busy["stage"], "index");
    assert_eq!(busy["error"], "busy");
    assert_eq!(state(&h, &login).await["view_id"], current["view_id"]);
    assert_eq!(fs::read(cache_dir(&b).join("design.ovm")).unwrap(), bytes);
    h.shutdown().await;
    println!("RUST INDEX OPEN RACES: ALL OK (before-write CAS, post-index CAS, original display, cancel/reap, replay, live cache lease)");
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "run tools/validate_owner_service.py on private fixtures"]
async fn approved_index_open_preserves_first_frame_replays_and_explicit_force() {
    let dir = area("index-open-native");
    let a = dir.join("A.oas");
    let h = Harness::start(&[a.clone(), dir.join("B.oas"), dir.join("C.oas")], native()).await;
    let login = h.login().await;
    let sources = h.service.catalog()["sources"].as_array().unwrap().clone();
    let mut initial = open(
        "1",
        &sources[0]["source_id"],
        "level",
        json!({"mode":"all"}),
    );
    initial["body"] = json!({"depth":"7","detail":"high","thin":"keep","frames":true,
        "labels":false,"font_px":23,"navigation":{"kind":"goto","center_um":["5","6"],"width_um":"300"}});
    failed(&h, &login, initial).await;
    assert!(!cache_dir(&a).exists(), "opening alone indexed the source");
    let approved = request(2, 1, json!({"kind":"empty"}));
    let mut denied = approved.clone();
    denied["approved"] = json!(false);
    assert_eq!(
        h.call(&login, "POST", "/api/v1/operations", denied).await.0,
        400
    );
    let mut bad = approved.clone();
    bad["pixels"] = json!([0, 103]);
    assert_eq!(
        h.call(&login, "POST", "/api/v1/operations", bad).await.0,
        400
    );
    assert!(!cache_dir(&a).exists());
    let done = operation(&h, &login, approved.clone()).await;
    assert_eq!(done["kind"], "index_open");
    assert_eq!(done["request_id"], approved["request_id"]);
    assert_eq!(done["phase"], "succeeded", "{done}");
    assert_eq!(done["stage"], "open");
    assert_eq!(done["index"]["phase"], "succeeded");
    let mut ws = h.connect(&login).await;
    let (_, header) = frame(&mut ws).await;
    assert_eq!(header["generation"], "1");
    let view = state(&h, &login).await;
    for (name, want) in [
        ("depth", json!("7")),
        ("detail", json!("high")),
        ("thin", json!("keep")),
        ("frames", json!(true)),
        ("labels", json!(false)),
        ("font_px", json!(23)),
    ] {
        assert_eq!(view[name], want, "{name}");
    }
    assert_eq!(view["state_rev"], "1");
    let bbox = view["bbox_dbu"].as_array().unwrap();
    assert!(
        (bbox[2].as_str().unwrap().parse::<f64>().unwrap()
            - bbox[0].as_str().unwrap().parse::<f64>().unwrap()
            - 300_000.0)
            .abs()
            < 1.0
    );
    let marker = fs::read(cache_dir(&a).join("design.ovm")).unwrap();
    assert_eq!(
        h.call(&login, "POST", "/api/v1/operations", approved.clone())
            .await
            .1,
        done
    );
    let mut conflict = approved;
    conflict["options"]["force"] = json!(true);
    assert_eq!(
        h.call(&login, "POST", "/api/v1/operations", conflict)
            .await
            .0,
        409
    );
    assert_eq!(fs::read(cache_dir(&a).join("design.ovm")).unwrap(), marker);
    assert_eq!(state(&h, &login).await["view_id"], view["view_id"]);
    ws.close(None).await.unwrap();
    h.call(
        &login,
        "DELETE",
        &format!("/api/v1/views/{}", view["view_id"].as_str().unwrap()),
        Value::Null,
    )
    .await;
    timeout(Duration::from_secs(5), async {
        while h.resources.usage().workers != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    // A corrupt existing cache is not implicit --force. Approval of indexing
    // and approval of replacing old files are independent fields.
    let b = dir.join("B.oas");
    let bcache = cache_dir(&b);
    fs::create_dir(&bcache).unwrap();
    fs::write(bcache.join("design.ovm"), b"sentinel").unwrap();
    failed(
        &h,
        &login,
        open(
            "3",
            &sources[1]["source_id"],
            "level",
            json!({"mode":"all"}),
        ),
    )
    .await;
    let no_force = operation(&h, &login, request(4, 3, json!({"kind":"empty"}))).await;
    assert_eq!(no_force["phase"], "failed");
    assert_eq!(no_force["stage"], "index");
    assert_eq!(fs::read(bcache.join("design.ovm")).unwrap(), b"sentinel");
    let mut force = request(5, 3, json!({"kind":"empty"}));
    force["options"]["force"] = json!(true);
    assert_eq!(operation(&h, &login, force).await["phase"], "succeeded");
    // A registered source changed on disk: indexing cannot repair its stale
    // registration, even though that source has no cache either.
    let close = state(&h, &login).await;
    h.call(
        &login,
        "DELETE",
        &format!("/api/v1/views/{}", close["view_id"].as_str().unwrap()),
        Value::Null,
    )
    .await;
    timeout(Duration::from_secs(5), async {
        while h.resources.usage().workers != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    fs::write(dir.join("C.oas"), b"changed source").unwrap();
    let changed = operation(
        &h,
        &login,
        open(
            "6",
            &sources[2]["source_id"],
            "level",
            json!({"mode":"all"}),
        ),
    )
    .await;
    assert_eq!(changed["phase"], "failed");
    assert!(changed.get("index_open").is_none());
    assert!(!cache_dir(&dir.join("C.oas")).exists());
    // Expired approval contexts cannot be reinterpreted as a new file request.
    for seq in 7..=39 {
        let result=operation(&h,&login,json!({"kind":"mode","seq":seq.to_string(),"view_id":"gone","base_state_rev":"1","mode":"chip"})).await;
        assert_eq!(result["phase"], "failed");
    }
    assert_eq!(
        h.call(
            &login,
            "POST",
            "/api/v1/operations",
            request(40, 1, json!({"kind":"empty"}))
        )
        .await
        .0,
        410
    );
    h.shutdown().await;
    println!("RUST INDEX OPEN NATIVE: ALL OK (consent, first-frame options, native indexing, replay, separate force, expiry)");
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "run tools/validate_owner_service.py on private fixtures"]
async fn index_open_keeps_selected_deck_levels_and_does_not_open_partial_decks() {
    let dir = area("index-open-deck");
    let deck = dir.join("selected.jb");
    fs::write(&deck,"MTITLE 1,ONE\nMTITLE 2,TWO\nCHIP A\n$ (1,A,TC=A.oas,AD=0.001,LY={1},DT={0},UX=100,UY=100)\nROWS 0/0\nCHIP B\n$ (2,B,TC=B.oas,AD=0.001,LY={1},DT={0},UX=100,UY=100)\nROWS 0/0\n").unwrap();
    let partial = dir.join("partial.jb");
    fs::write(&partial,"MTITLE 1,PARTIAL\nCHIP C\n$ (1,C,TC=C.oas,AD=0.001,LY={1},DT={0},UX=100,UY=100)\n$ (2,X,TC=absent.oas,AD=0.001,LY={1},DT={0},UX=100,UY=100)\nROWS 0/0\n").unwrap();
    let bad_layer = dir.join("bad-layer.jb");
    fs::write(&bad_layer,"MTITLE 1,ONE\nMTITLE 2,TWO\nCHIP A\n$ (1,A,TC=A.oas,AD=0.001,LY={999},DT={0},UX=100,UY=100)\nROWS 0/0\nCHIP B\n$ (2,B,TC=B.oas,AD=0.001,LY={1},DT={0},UX=100,UY=100)\nROWS 0/0\n").unwrap();
    let absent = dir.join("absent.jb");
    fs::write(
        &absent,
        "CHIP X\n$ (1,X,TC=absent.oas,AD=0.001,LY={1},DT={0},UX=100,UY=100)\nROWS 0/0\n",
    )
    .unwrap();
    let h = Harness::start(&[deck, partial, bad_layer, absent], native()).await;
    let login = h.login().await;
    let sources = h.service.catalog()["sources"].as_array().unwrap().clone();
    let result = failed(
        &h,
        &login,
        open(
            "1",
            &sources[0]["source_id"],
            "chip",
            json!({"mode":"only","ids":["1"]}),
        ),
    )
    .await;
    assert_eq!(
        result["index_open"]["levels"],
        json!({"mode":"only","count":1})
    );
    let mut consent = request(2, 1, json!({"kind":"empty"}));
    consent["options"]["lod"] = json!(true);
    assert_eq!(operation(&h, &login, consent).await["phase"], "succeeded");
    assert!(cache_dir(&dir.join("A.oas")).exists());
    assert!(!cache_dir(&dir.join("B.oas")).exists());
    let mut ws = h.connect(&login).await;
    let (_, header) = frame(&mut ws).await;
    assert_eq!(header["generation"], "1");
    let view = state(&h, &login).await;
    let info = h.call(&login, "GET", "/api/v1/view", Value::Null).await.1;
    assert_eq!(info["levels"], json!(["1"]));
    assert_eq!(info["mode"], "chip");
    ws.close(None).await.unwrap();
    h.call(
        &login,
        "DELETE",
        &format!("/api/v1/views/{}", view["view_id"].as_str().unwrap()),
        Value::Null,
    )
    .await;
    timeout(Duration::from_secs(5), async {
        while h.resources.usage().workers != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    failed(
        &h,
        &login,
        open(
            "3",
            &sources[1]["source_id"],
            "level",
            json!({"mode":"all"}),
        ),
    )
    .await;
    let partial = operation(&h, &login, request(4, 3, json!({"kind":"empty"}))).await;
    assert_eq!(partial["phase"], "incomplete", "{partial}");
    assert_eq!(partial["stage"], "index");
    assert_eq!(partial["index"]["skipped"], 1);
    assert!(partial.get("view_id").is_none());
    assert_eq!(h.resources.usage().workers, 0);
    // An unselected unindexed TC must not turn a missing-layer error into an
    // index proposal. Nor can indexing create a missing OASIS source file.
    for (seq, id, levels) in [
        (5, 2, json!({"mode":"only","ids":["1"]})),
        (6, 3, json!({"mode":"all"})),
    ] {
        let failed = operation(
            &h,
            &login,
            open(&seq.to_string(), &sources[id]["source_id"], "level", levels),
        )
        .await;
        assert_eq!(failed["phase"], "failed");
        assert!(failed.get("index_open").is_none(), "{failed}");
    }
    assert!(!cache_dir(&dir.join("B.oas")).exists());
    h.shutdown().await;
    println!("RUST INDEX OPEN DECK: ALL OK (selected levels, LOD, untouched other source, partial never opens)");
}
