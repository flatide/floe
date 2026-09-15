use super::{drc_isolation::ReviewSocket, *};

async fn current(h: &Harness, l: &Login) -> Value {
    h.call(l, "GET", "/api/v1/view", Value::Null).await.1
}
async fn ready(h: &Harness, l: &Login, socket: &mut ReviewSocket) {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        let v = current(h, l).await;
        assert_eq!(v["view"]["view_id"], socket.hello["view_id"]);
        assert_ne!(v["view"]["status"], "failed", "{v}");
        if matches!(v["view"]["status"].as_str(), Some("idle" | "rendering")) {
            socket.state = v["view"].clone();
            return;
        }
        assert!(Instant::now() < until);
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}
fn request(seq: u64, s: &ReviewSocket, mode: &str) -> Value {
    json!({"kind":"mode","seq":seq.to_string(),"view_id":s.hello["view_id"],"base_state_rev":s.state["state_rev"],"mode":mode})
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "run tools/validate_owner_service.py with private mode fixtures"]
async fn live_modes_preserve_camera_selection_and_use_one_worker() {
    let source =
        PathBuf::from(std::env::var_os("FLOE_OWNER_MODE_FIXTURE").expect("private mode fixture"));
    let deck = source.parent().unwrap().join("modes.jb");
    fs::write(&deck, "MTITLE 1,MASK-A\nMTITLE 2,MASK-B\nMTITLE 3,NOT-LOADED\nCHIP C1\n$ (1,P1,TC=A.oas,AD=0.001,LY={1},DT={0},UX=500,UY=500)\n$ (2,P2,TC=A.oas,AD=0.001,LY={2},DT={0},UX=500,UY=500)\n$ (3,P3,TC=A.oas,AD=0.001,LY={3},DT={0},UX=500,UY=500)\nROWS 0/0\nCHIP C2\n$ (1,P1,TC=B.oas,AD=0.001,LY={1},DT={0},UX=500,UY=500)\n$ (2,P2,TC=B.oas,AD=0.001,LY={2},DT={0},UX=500,UY=500)\nROWS 0/0\n").unwrap();
    let h = Harness::configured_limits(
        &[deck, source],
        native(),
        None,
        false,
        false,
        Limits {
            cpu_slots: 2,
            foreground_reserve: 0,
            workers: 1,
            decoded_mb: 64,
        },
    )
    .await;
    let l = h.login().await;
    let catalog = h.service.catalog();
    let open = json!({"kind":"open","seq":"1","source_id":catalog["sources"][0]["source_id"],"mode":"chip","levels":{"mode":"only","ids":["1","2"]},"body":{
        "pixels":[113,97],"navigation":{"kind":"goto","center_um":["103","117"],"width_um":"311"},
        "depth":"3","detail":"exact","thin":"keep","frames":false,"labels":false,"font_px":20,"mono":true}});
    assert_eq!(h.call(&l, "POST", "/api/v1/operations", open).await.0, 202);
    assert_eq!(h.finished(&l, 1).await["phase"], "succeeded");
    let mut s = ReviewSocket::new(&h, &l).await;
    ready(&h, &l, &mut s).await;
    assert_eq!(
        h.call(&l, "GET", "/api/v1/capabilities", Value::Null)
            .await
            .1["jobdeck_modes"],
        true
    );
    let rows = h
        .call(
            &l,
            "GET",
            &format!(
                "/api/v1/views/{}/layers/0",
                s.hello["view_id"].as_str().unwrap()
            ),
            Value::Null,
        )
        .await
        .1;
    let leaves: Vec<_> = rows["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["head"] == false && r["pair"][0] == 1)
        .collect();
    assert_eq!(leaves.len(), 2, "{rows}");
    let selected = json!({"mode":"only","pairs":[leaves[0]["pair"]]});
    s.set(json!({"layers":selected})).await;
    ready(&h, &l, &mut s).await;
    let camera = s.state.clone();
    let usage = h.resources.usage();
    assert_eq!(usage.workers, 1);
    assert_eq!(usage.cpu_slots, 2);
    let mut seq = 2;
    let mut replay = None;
    for mode in ["level", "layer", "chip", "layer", "level"] {
        let old_id = s.hello["view_id"].clone();
        let old_epoch = s.state["worker_epoch"].clone();
        let command = request(seq, &s, mode);
        let mut invalid = command.clone();
        invalid["levels"] = json!({"mode":"all"});
        assert_eq!(
            h.call(&l, "POST", "/api/v1/operations", invalid).await.0,
            400
        );
        assert_eq!(
            h.call(&l, "POST", "/api/v1/operations", command.clone())
                .await
                .0,
            202
        );
        let result = h.finished(&l, seq).await;
        assert_eq!(result["phase"], "succeeded", "{result}");
        assert_ne!(result["view_id"], old_id);
        if replay.is_none() {
            replay = Some((command.clone(), result.clone()));
        }
        assert_eq!(
            h.call(&l, "POST", "/api/v1/operations", command).await.1,
            result
        );
        s.closed().await;
        s = ReviewSocket::new(&h, &l).await;
        ready(&h, &l, &mut s).await;
        let v = current(&h, &l).await;
        assert_eq!(v["mode"], mode);
        assert_eq!(v["levels"], json!(["1", "2"]));
        assert_eq!(v["source_id"], catalog["sources"][0]["source_id"]);
        assert_ne!(s.state["worker_epoch"], old_epoch);
        for field in [
            "bbox_dbu", "pixels", "depth", "detail", "thin", "frames", "labels", "font_px", "mono",
        ] {
            assert_eq!(s.state[field], camera[field], "{mode}: {field}");
        }
        assert_eq!(h.resources.usage(), usage);
        if mode != "layer" {
            assert_eq!(s.state["layers"], selected, "hidden chip reappeared");
        } else if seq == 3 {
            assert_eq!(s.state["layers"], json!({"mode":"all"}));
            s.set(json!({"layers":{"mode":"none"}})).await;
            ready(&h, &l, &mut s).await;
        } else {
            assert_eq!(
                s.state["layers"],
                json!({"mode":"none"}),
                "raw layer visibility was not independent"
            );
        }
        assert_eq!(
            h.call(
                &l,
                "GET",
                &format!("/api/v1/views/{}/layers/0", old_id.as_str().unwrap()),
                Value::Null
            )
            .await
            .0,
            404
        );
        let mut png = h.connect(&l).await;
        let (_, image) = frame(&mut png).await;
        assert_eq!(image["view_id"], s.hello["view_id"]);
        assert_eq!(image["worker_epoch"], s.state["worker_epoch"]);
        png.close(None).await.unwrap();
        seq += 1;
    }
    let stable = s.state.clone();
    let noop = request(seq, &s, "level");
    assert_eq!(h.call(&l, "POST", "/api/v1/operations", noop).await.0, 202);
    assert_eq!(h.finished(&l, seq).await["unchanged"], true);
    assert_eq!(
        current(&h, &l).await["view"]["worker_epoch"],
        stable["worker_epoch"]
    );
    assert_eq!(
        current(&h, &l).await["view"]["state_rev"],
        stable["state_rev"]
    );
    seq += 1;
    s.set(json!({"mono":false})).await;
    let before = s.state.clone();
    let mut stale = request(seq, &s, "chip");
    stale["base_state_rev"] = stable["state_rev"].clone();
    assert_eq!(h.call(&l, "POST", "/api/v1/operations", stale).await.0, 202);
    assert_eq!(h.finished(&l, seq).await["phase"], "failed");
    let (original, result) = replay.unwrap();
    assert_eq!(
        h.call(&l, "POST", "/api/v1/operations", original.clone())
            .await
            .1,
        result
    );
    let mut conflict = original;
    conflict["mode"] = json!("layer");
    assert_eq!(
        h.call(&l, "POST", "/api/v1/operations", conflict).await.0,
        409
    );
    assert_eq!(current(&h, &l).await["view"]["view_id"], before["view_id"]);
    assert_eq!(
        current(&h, &l).await["view"]["state_rev"],
        before["state_rev"]
    );
    assert_eq!(h.resources.usage(), usage);
    assert_eq!(
        h.call(&l, "DELETE", "/api/v1/session", Value::Null).await.0,
        204
    );
    s.closed().await;
    h.shutdown().await;
    println!("RUST OWNER DECK MODES: ALL OK (5 native cutovers, one reservation, camera/levels/visibility, no-op/stale/replay/old scope/logout)");
}
