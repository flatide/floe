//! Synthetic pack supplied by validate_web_drc.py, with no Python in runtime.
use floe_app_core::{
    managed::{Limits, Resources, Usage},
    registered::AccessScope,
};
use floe_web::drc::{Request, Service};
use serde_json::json;
use std::{path::PathBuf, sync::Arc, time::Duration};
fn rules() -> Request {
    serde_json::from_value(json!({"kind":"rules","start":"0","search":"","limit":1})).unwrap()
}
#[tokio::test]
#[ignore = "run tools/validate_web_drc.py with a synthetic pack"]
async fn real_pack_cancellation_admission_scope_and_reap() {
    let path = PathBuf::from(std::env::var_os("FLOE_DRC_WEB_PACK").expect("private pack"));
    let rules_path =
        PathBuf::from(std::env::var_os("FLOE_DRC_WEB_RULES").expect("private rules metadata"));
    let scope = AccessScope::new(&[path.parent().unwrap().to_owned()]).unwrap();
    let resources = Resources::new(Limits::default()).unwrap();
    assert!(Service::start(
        &resources,
        Arc::clone(&scope),
        &path,
        Some(std::path::Path::new("/etc/passwd")),
        "source"
    )
    .is_err());
    assert_eq!(resources.usage(), Usage::default());
    assert!(Service::start_with_rules(
        &resources,
        Arc::clone(&scope),
        &path,
        None,
        Some(std::path::Path::new("/etc/passwd")),
        "source"
    )
    .is_err());
    assert_eq!(resources.usage(), Usage::default());
    for (close_opening, with_rules) in [(false, false), (true, false), (false, true), (true, true)]
    {
        let service = Service::start_with_rules(
            &resources,
            Arc::clone(&scope),
            &path,
            None,
            with_rules.then_some(rules_path.as_path()),
            "source",
        )
        .unwrap();
        assert_eq!(resources.usage().cpu_slots, 1);
        assert_eq!(
            resources.usage().decoded_mb,
            if with_rules { 512 } else { 256 }
        );
        if with_rules {
            assert!(resources.index([rules_path.clone()], 1).is_err());
        }
        if !close_opening {
            let mut first = service.submit(rules()).unwrap();
            let bytes = tokio::time::timeout(Duration::from_secs(5), first.result())
                .await
                .unwrap()
                .unwrap();
            assert!(
                !serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["rows"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
            for _ in 0..100 {
                let ticket = service.submit(rules()).unwrap();
                drop(ticket);
            }
        }
        service.request_stop();
        tokio::time::timeout(Duration::from_secs(5), async {
            while !service.is_finished() {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(resources.usage(), Usage::default());
        assert!(matches!(service.submit(rules()), Err("drc_closed")));
    }
    let broken = Service::start(
        &resources,
        scope,
        &path.with_file_name("missing.ice"),
        None,
        "source",
    )
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !broken.is_finished() {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(broken.catalog()["phase"], "error");
    assert_eq!(broken.catalog()["error"], "drc_read_error");
    assert_eq!(resources.usage(), Usage::default());
    let bad_rules = Service::start_with_rules(
        &resources,
        AccessScope::new(&[path.parent().unwrap().to_owned()]).unwrap(),
        &path,
        None,
        Some(&path.with_file_name("missing.rules.json")),
        "source",
    )
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !bad_rules.is_finished() {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(bad_rules.catalog()["phase"], "error");
    assert_eq!(resources.usage(), Usage::default());
    println!("RUST DRC ACTOR: ALL OK");
}

#[tokio::test]
#[ignore = "run tools/validate_web_drc.py with a synthetic pack"]
async fn reader_actor_rejects_review_for_another_or_replaced_pack() {
    use floe_app_core::drc::review::{
        managed::{ManagedStore, Registration},
        store::Kind,
    };
    use std::{fs, sync::atomic::AtomicUsize};
    let fixture = PathBuf::from(std::env::var_os("FLOE_DRC_WEB_PACK").unwrap());
    let dir = fixture
        .parent()
        .unwrap()
        .join(format!("review-binding-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let path = dir.join("reader.ice");
    let copy = dir.join("copy.ice");
    fs::copy(&fixture, &path).unwrap();
    fs::copy(&fixture, &copy).unwrap();
    let scope = AccessScope::new(std::slice::from_ref(&dir)).unwrap();
    let resources = Resources::new(Limits::default()).unwrap();
    let store = |pack: &std::path::Path| {
        ManagedStore::open(
            &resources,
            Registration {
                scope: Arc::clone(&scope),
                pack: pack.to_owned(),
                reviewer: "synthetic-reader".into(),
                kind: Kind::Notes,
                protected_files: vec![],
                protected_trees: vec![],
            },
            &AtomicUsize::new(0),
        )
        .unwrap()
    };
    let original = store(&path);
    let other = store(&copy);
    let reader = Service::start(&resources, Arc::clone(&scope), &path, None, "source").unwrap();
    let mut ticket = reader
        .validate_review_identity(original.identity())
        .unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), ticket.result())
            .await
            .unwrap()
            .unwrap(),
        b"{}"
    );
    let mut mismatch = reader.validate_review_identity(other.identity()).unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), mismatch.result())
            .await
            .unwrap(),
        Err("drc_changed_or_corrupt")
    );
    // Equal bytes/legacy header do not authorize a different inode. Simulate an
    // external replace while the read actor still owns its first descriptor.
    fs::rename(&copy, &path).unwrap();
    let fresh = store(&path);
    let mut stale = reader.validate_review_identity(fresh.identity()).unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), stale.result())
            .await
            .unwrap(),
        Err("drc_changed_or_corrupt")
    );
    reader.request_stop();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !reader.is_finished() {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(
        reader.validate_review_identity(fresh.identity()),
        Err("drc_closed")
    ));
    drop((original, other, fresh, reader));
    assert_eq!(resources.usage(), Usage::default());
    assert_eq!(
        fs::read_dir(&dir).unwrap().count(),
        1,
        "no review/lock files from validation"
    );
    assert_eq!(fs::read(&path).unwrap(), fs::read(&fixture).unwrap());
    fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
#[ignore = "run tools/validate_web_drc.py with a synthetic pack"]
async fn reader_actor_refreshes_waives_without_changing_geometry_identity() {
    use floe_app_core::drc::{
        review::{
            managed::{ManagedStore, Registration},
            store::Kind,
        },
        Pack,
    };
    use std::{fs, sync::atomic::AtomicUsize};
    let fixture = PathBuf::from(std::env::var_os("FLOE_DRC_WEB_PACK").unwrap());
    let dir = fixture
        .parent()
        .unwrap()
        .join(format!("waive-refresh-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let path = dir.join("reader.ice");
    fs::copy(&fixture, &path).unwrap();
    let scope = AccessScope::new(std::slice::from_ref(&dir)).unwrap();
    let resources = Resources::new(Limits::default()).unwrap();
    let stop = Arc::new(AtomicUsize::new(0));
    let pack = Pack::open(&path, &stop).unwrap();
    let ci = pack.checks.iter().position(|c| c.count > 0).unwrap();
    let gid = pack.checks[ci].start;
    let store = ManagedStore::open(
        &resources,
        Registration {
            scope: Arc::clone(&scope),
            pack: path.clone(),
            reviewer: "synthetic-refresh".into(),
            kind: Kind::Waives,
            protected_files: vec![],
            protected_trees: vec![],
        },
        &stop,
    )
    .unwrap();
    let reader = Service::start(&resources, Arc::clone(&scope), &path, None, "source").unwrap();
    let query = || {
        serde_json::from_value(json!({"kind":"records","check":ci.to_string(),"errors":["0"]}))
            .unwrap()
    };
    async fn read(mut t: floe_web::drc::Ticket) -> serde_json::Value {
        serde_json::from_slice(
            &tokio::time::timeout(Duration::from_secs(5), t.result())
                .await
                .unwrap()
                .unwrap(),
        )
        .unwrap()
    }
    let before = read(reader.submit(query()).unwrap()).await;
    let identity = (reader.id.clone(), reader.revision.clone());
    assert_eq!(before["rows"][0]["status"], 0);
    let save = |value| {
        let mut job = store
            .snapshot(Arc::clone(&stop))
            .unwrap()
            .prepare_waives(&[(gid, value)])
            .unwrap()
            .publish(false)
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !job.is_finished() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        job.close().unwrap();
        assert!(job.status().outcome.is_some());
    };
    for value in [1, 0, 239] {
        save(value);
        let snapshot = store.snapshot(Arc::clone(&stop)).unwrap();
        let ticket = reader.apply_waives(snapshot).unwrap();
        // A queued read after the barrier sees the new status. The coordinator
        // still has to fence HTTP replies already sent before this barrier.
        let after = reader.submit(query()).unwrap();
        let applied = read(ticket).await;
        assert_eq!(applied["sidecar"], true);
        assert_eq!(applied["waived"], if value == 1 { "1" } else { "0" });
        let mut result = read(after).await;
        assert_eq!(result["rows"][0]["status"], value);
        result["rows"][0]["status"] = json!(0);
        assert_eq!(result, before);
        assert_eq!(reader.catalog()["metadata"]["waives"], true);
        let rule = serde_json::from_value(json!({"kind":"rule","check":ci.to_string()})).unwrap();
        assert_eq!(
            read(reader.submit(rule).unwrap()).await["waived"],
            if value == 1 { "1" } else { "0" }
        );
        assert_eq!((&reader.id, &reader.revision), (&identity.0, &identity.1));
    }
    let stale = store.snapshot(Arc::clone(&stop)).unwrap();
    // A second registered writer models external replacement. Reusing `store`
    // would correctly fail its one-live-snapshot admission before any write.
    let external = floe_app_core::drc::review::store::Store::open(
        Arc::clone(&scope),
        &path,
        "synthetic-refresh",
        Kind::Waives,
        vec![],
        vec![],
        &stop,
    )
    .unwrap();
    external
        .snapshot(&stop)
        .unwrap()
        .prepare_waives(&[(gid, 1)], &stop)
        .unwrap()
        .publish(&stop)
        .unwrap();
    drop(external);
    let mut rejected = reader.apply_waives(stale).unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), rejected.result())
            .await
            .unwrap(),
        Err("drc_busy")
    );
    read(
        reader
            .apply_waives(store.snapshot(Arc::clone(&stop)).unwrap())
            .unwrap(),
    )
    .await;
    assert_eq!(
        read(reader.submit(query()).unwrap()).await["rows"][0]["status"],
        1
    );
    assert!(
        serde_json::from_value::<Request>(json!({"kind":"apply_waives","path":"forged"})).is_err()
    );
    let cancelled = Arc::new(AtomicUsize::new(0));
    let snapshot = store.snapshot(Arc::clone(&cancelled)).unwrap();
    cancelled.store(1, std::sync::atomic::Ordering::Relaxed);
    let mut ticket = reader.apply_waives(snapshot).unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), ticket.result())
            .await
            .unwrap(),
        Err("drc_cancelled")
    );
    assert!(store.is_idle());
    assert_eq!(
        read(reader.submit(query()).unwrap()).await["rows"][0]["status"],
        1
    );
    reader.request_stop();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !reader.is_finished() {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(
        reader.apply_waives(store.snapshot(Arc::clone(&stop)).unwrap()),
        Err("drc_closed")
    ));
    assert!(store.is_idle());
    drop((store, reader, pack));
    assert_eq!(resources.usage(), Usage::default());
    assert_eq!(fs::read(&path).unwrap(), fs::read(&fixture).unwrap());
    assert_eq!(
        fs::read_dir(&dir).unwrap().count(),
        3,
        "only synthetic pack and approved native waive/lock"
    );
    fs::remove_dir_all(&dir).unwrap();
    println!("RUST DRC WAIVE REFRESH: ALL OK");
}
