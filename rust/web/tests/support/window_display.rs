use super::{drc_isolation::ReviewSocket, *};

async fn current(h: &Harness, l: &Login) -> Value {
    h.call(l, "GET", "/api/v1/view", Value::Null).await.1
}
async fn picked(h: &Harness, l: &Login, source: &Value, seq: u64, old: Option<&Value>) -> Value {
    requested(
        h,
        l,
        json!({"kind":"open","seq":"1","source_id":source,
        "mode":"level","display_policy":"window","body":{}}),
        seq,
        old,
    )
    .await
}
async fn requested(h: &Harness, l: &Login, request: Value, seq: u64, old: Option<&Value>) -> Value {
    let (id, _) = h.launches.reserve().unwrap();
    h.launches.ready(&id, Some(request), false).unwrap();
    let mut action =
        json!({"action":"open","seq":seq.to_string(),"pixels":[137,103],"levels":{"mode":"all"}});
    if let Some(old) = old {
        action["view_id"] = old["view"]["view_id"].clone();
        action["state_rev"] = old["view"]["state_rev"].clone();
    }
    let path = format!("/api/v1/launch/{id}");
    let receipt = h.call(l, "POST", &path, action.clone()).await;
    assert_eq!(receipt.0, 200);
    let result = h.finished(l, seq).await;
    assert_eq!(h.call(l, "POST", &path, action).await.1, receipt.1);
    assert_eq!(result["phase"], "succeeded", "{result}");
    let mut socket = h.connect(l).await;
    let (_, header) = frame(&mut socket).await;
    socket.close(None).await.unwrap();
    let next = current(h, l).await;
    assert_eq!(header["view_id"], next["view"]["view_id"]);
    if result["reused"] != true {
        assert_eq!(
            header["generation"], "1",
            "settings must precede the first geometry frame"
        );
        assert_eq!(next["view"]["state_rev"], "1");
    }
    next
}
fn preferences(a: &Value, b: &Value) {
    for field in ["depth", "detail", "thin", "frames", "labels", "font_px"] {
        assert_eq!(a["view"][field], b["view"][field], "{field}");
    }
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "run tools/validate_owner_service.py with private source files"]
async fn picker_preserves_window_state_before_first_frame_and_across_deck_and_close() {
    let a = PathBuf::from(std::env::var_os("FLOE_OWNER_MODE_FIXTURE").unwrap());
    let dir = a.parent().unwrap();
    let deck = dir.join("window-display.jb");
    fs::write(
        &deck,
        "MTITLE 1,ONE\nCHIP C\n$ (1,A,TC=A.oas,AD=0.001,LY={1},DT={0},UX=500,UY=500)\nROWS 0/0\n",
    )
    .unwrap();
    let h = Harness::start(&[a.clone(), dir.join("B.oas"), deck], native()).await;
    let l = h.login().await;
    let rows = h.service.catalog()["sources"].as_array().unwrap().clone();
    let ids: Vec<_> = rows.iter().map(|v| v["source_id"].clone()).collect();
    let first = picked(&h, &l, &ids[0], 1, None).await;
    assert_eq!(first["view"]["depth"], "0");
    assert_eq!(first["view"]["frames"], true);
    assert_eq!(first["view"]["labels"], true);
    // Accepted user changes, not a DOM preference copy or the original CLI
    // request. Layer isolation and mono must stay only on same-source reuse.
    let mut socket = ReviewSocket::new(&h, &l).await;
    socket
        .set(
            json!({"depth":"7","detail":"high","thin":"auto","frames":true,
        "labels":true,"font_px":23,"mono":true,"layers":{"mode":"none"},
        "navigation":{"kind":"goto","center_um":["5","6"],"width_um":"300"}}),
        )
        .await;
    let configured = current(&h, &l).await;
    drop(socket);
    let same = picked(&h, &l, &ids[0], 2, Some(&configured)).await;
    preferences(&same, &configured);
    for field in [
        "view_id",
        "worker_epoch",
        "bbox_dbu",
        "mono",
        "layers",
        "state_rev",
    ] {
        assert_eq!(same["view"][field], configured["view"][field], "{field}");
    }
    let other = picked(&h, &l, &ids[1], 3, Some(&same)).await;
    preferences(&other, &same);
    assert_ne!(other["view"]["bbox_dbu"], same["view"]["bbox_dbu"]);
    assert_eq!(other["view"]["mono"], false);
    assert_eq!(other["view"]["layers"]["mode"], "all");
    let mask = picked(&h, &l, &ids[2], 4, Some(&other)).await;
    assert_eq!(mask["view"]["depth"], "full");
    assert_eq!(mask["view"]["labels"], false);
    assert_eq!(mask["view"]["effective_thin"], "keep");
    assert_eq!(mask["view"]["font_px"], 23);
    let back = picked(&h, &l, &ids[0], 5, Some(&mask)).await;
    assert_eq!(back["view"]["depth"], "full");
    assert_eq!(
        back["view"]["labels"], true,
        "deck capability must not erase the preference"
    );
    assert_eq!(
        back["view"]["effective_thin"], "cull",
        "inherit auto, not its deck resolution"
    );
    let mut socket = ReviewSocket::new(&h, &l).await;
    socket.set(json!({"depth":"2","detail":"low","thin":"keep","frames":false,"labels":false,"font_px":31})).await;
    let changed = current(&h, &l).await;
    h.call(
        &l,
        "DELETE",
        &format!(
            "/api/v1/views/{}",
            changed["view"]["view_id"].as_str().unwrap()
        ),
        Value::Null,
    )
    .await;
    socket.closed().await;
    timeout(Duration::from_secs(5), async {
        while h.resources.usage().workers != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let reopened = picked(&h, &l, &ids[1], 6, None).await;
    preferences(&reopened, &changed);
    let mask = picked(&h, &l, &ids[2], 7, Some(&reopened)).await;
    let back = picked(&h, &l, &ids[0], 8, Some(&mask)).await;
    assert_eq!(back["view"]["labels"], false);
    assert_eq!(back["view"]["frames"], false);
    let mut back = back;
    for (seq, labels) in [(9, false), (11, true)] {
        // The CLI sends the unmasked preference separately from the deck's
        // required labels=false. A following file-menu layout must honour it.
        let mask = requested(
            &h,
            &l,
            json!({"kind":"open","seq":"1","source_id":ids[2],
            "mode":"level","label_preference":labels,
            "body":{"depth":"full","frames":true,"labels":false}}),
            seq,
            Some(&back),
        )
        .await;
        assert_eq!(mask["view"]["labels"], false);
        back = picked(&h, &l, &ids[0], seq + 1, Some(&mask)).await;
        assert_eq!(back["view"]["labels"], labels);
    }
    let mask = picked(&h, &l, &ids[2], 13, Some(&back)).await;
    let mut socket = ReviewSocket::new(&h, &l).await;
    socket.set(json!({"depth":"1"})).await;
    let shallow = current(&h, &l).await;
    assert_ne!(shallow["view"]["state_rev"], mask["view"]["state_rev"]);
    drop(socket);
    let same = picked(&h, &l, &ids[2], 14, Some(&shallow)).await;
    assert_eq!(
        same["view"]["depth"], "1",
        "same deck selection must not reset full depth"
    );
    h.shutdown().await;
    println!("RUST WINDOW DISPLAY: ALL OK (first frame, same-source reuse, fitted replacement, labels across deck, auto thin, close/reopen)");
}
