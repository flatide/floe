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
