use super::*;
use floe_app_core::registered::{AccessScope, RegisteredSource};
use std::{fs, path::PathBuf, sync::atomic::AtomicU64};
static SERIAL: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    dir: PathBuf,
    source: Arc<RegisteredSource>,
    service: Arc<Service>,
}
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "floe-default-api-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("mask.jb");
        fs::write(&path, "CHIP A\n").unwrap();
        let scope = AccessScope::new(std::slice::from_ref(&dir)).unwrap();
        let source = RegisteredSource::register(scope, &path, &AtomicUsize::new(0)).unwrap();
        let p = Publisher::new(vec![Arc::clone(&source)]).unwrap();
        Self {
            dir,
            source,
            service: Service::start(p).unwrap(),
        }
    }
    fn draft(&self) -> Draft {
        self.service
            .publisher
            .prepare(
                Arc::clone(&self.source),
                floe_app_core::jobdeck::color::Mode::Level,
                "1 red solid MASK 1 2",
                &AtomicUsize::new(0),
            )
            .unwrap()
    }
    fn finish(&self, serial: u64, owner: SessionId) -> std::result::Result<Value, &'static str> {
        self.service.finish(
            serial,
            owner,
            Prepare {
                view_id: "a".repeat(64),
                state_rev: "1".into(),
                recover: false,
            },
            self.draft(),
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.service.request_stop();
        let end = Instant::now() + Duration::from_secs(3);
        while !self.service.is_finished() {
            assert!(Instant::now() < end);
            thread::sleep(Duration::from_millis(2));
        }
        fs::remove_dir_all(&self.dir).unwrap();
    }
}
fn owner() -> SessionId {
    let (mut auth, secret) = crate::auth::Auth::new(
        Instant::now(),
        Duration::from_secs(30),
        Duration::from_secs(60),
    )
    .unwrap();
    auth.exchange(&secret.expose(), Instant::now()).unwrap().id
}
#[test]
fn drafts_are_bounded_owner_scoped_expiring_and_newest_only() {
    let f = Fixture::new();
    let a = owner();
    let b = owner();
    let first = f.service.begin().unwrap();
    let second = f.service.begin().unwrap();
    assert_eq!(
        f.finish(first, a.clone()).unwrap_err(),
        "default_draft_expired"
    );
    let d = f.finish(second, a.clone()).unwrap();
    let token = d["token"].as_str().unwrap();
    f.service.revoke(&b, token);
    assert!(f.service.inner.state.lock().unwrap().ready.is_some());
    f.service.revoke(&a, token);
    assert!(f.service.inner.state.lock().unwrap().ready.is_none());
    let serial = f.service.begin().unwrap();
    f.finish(serial, a.clone()).unwrap();
    f.service
        .inner
        .state
        .lock()
        .unwrap()
        .ready
        .as_mut()
        .unwrap()
        .expires = Instant::now();
    f.service.maintain();
    assert!(f.service.inner.state.lock().unwrap().ready.is_none());
    let permit = Arc::clone(&f.service.preparations)
        .try_acquire_owned()
        .unwrap();
    assert!(Arc::clone(&f.service.preparations)
        .try_acquire_owned()
        .is_err());
    drop(permit);
    let pending = f.service.begin().unwrap();
    f.service.request_stop();
    assert_eq!(f.finish(pending, a).unwrap_err(), "default_draft_expired");
    assert_eq!(f.service.begin().unwrap_err(), "closed");
    assert!(!f.dir.join("mask.jb.layerprops").exists());
    assert_eq!(fs::read_dir(&f.dir).unwrap().count(), 1);
}
#[test]
fn cancelled_admitted_work_is_drained_without_publication_or_stuck_ledger() {
    let f = Fixture::new();
    let draft = f.draft();
    {
        let mut s = f.service.inner.state.lock().unwrap();
        s.ledger
            .admit(1, "cancelled test".into(), "design_default")
            .unwrap();
        let stop = Arc::new(AtomicUsize::new(1));
        s.stop = Some(Arc::clone(&stop));
        s.pending = Some(Work {
            seq: 1,
            view_id: "a".repeat(64),
            rev: 1,
            name: "mask.jb.layerprops".into(),
            draft: draft.into(),
            check: false,
            stop,
        });
    }
    f.service.request_stop();
    let end = Instant::now() + Duration::from_secs(3);
    while !f.service.is_finished() {
        assert!(Instant::now() < end);
        thread::sleep(Duration::from_millis(2));
    }
    let result = f.service.operation(1).unwrap();
    assert_eq!(result["phase"], "cancelled");
    assert_eq!(result["published"], false);
    assert!(f
        .service
        .inner
        .state
        .lock()
        .unwrap()
        .ledger
        .active()
        .is_none());
    assert!(!f.dir.join("mask.jb.layerprops").exists());
}
#[test]
fn wire_rejects_unapproved_paths_and_unknown_fields() {
    for bad in [
        json!({"view_id":"a","state_rev":1}),
        json!({"view_id":"a","state_rev":"1","path":"x"}),
    ] {
        assert!(serde_json::from_value::<Prepare>(bad).is_err());
    }
    let mut good = json!({"view_id":"a".repeat(64),"state_rev":"1","seq":"1","token":"b".repeat(64),"approve":true});
    assert!(serde_json::from_value::<Submit>(good.clone()).is_ok());
    good["text"] = json!("arbitrary replacement");
    assert!(serde_json::from_value::<Submit>(good).is_err());
}

#[test]
#[cfg(target_os = "macos")]
fn recovery_preview_is_separate_and_actor_never_rewrites_settings() {
    let f = Fixture::new();
    f.draft().publish(&AtomicUsize::new(0)).unwrap();
    let target = f.dir.join("mask.jb.layerprops");
    let payload = fs::read(&target).unwrap();
    // Tests can model a hard-link gap with the actual emitted marker, without
    // starting an app or terminating a process.
    // Read ONLY our just-created fixture's known xattr. Keep the transport
    // crate unsafe-free; OS-independent fault checks live in app-core.
    let output = std::process::Command::new("/usr/bin/xattr")
        .args(["-p", "com.floe.default-stage-v1"])
        .arg(&target)
        .output()
        .unwrap();
    assert!(output.status.success());
    let marker: Value = serde_json::from_slice(&output.stdout).unwrap();
    let stage = f.dir.join(marker["stage"].as_str().unwrap());
    fs::hard_link(&target, &stage).unwrap();
    let proof = f
        .service
        .publisher
        .prepare_recovery(
            Arc::clone(&f.source),
            floe_app_core::jobdeck::color::Mode::Level,
            &AtomicUsize::new(0),
        )
        .unwrap();
    let serial = f.service.begin().unwrap();
    let preview = f
        .service
        .finish(
            serial,
            owner(),
            Prepare {
                view_id: "a".repeat(64),
                state_rev: "1".into(),
                recover: true,
            },
            Prepared::Recover(Arc::new(proof)),
        )
        .unwrap();
    assert_eq!(preview["recover"], true);
    assert!(stage.exists());
    {
        let mut s = f.service.inner.state.lock().unwrap();
        let ready = s.ready.take().unwrap();
        s.ledger
            .admit(1, "explicit repair".into(), "design_default")
            .unwrap();
        let stop = Arc::new(AtomicUsize::new(0));
        s.stop = Some(Arc::clone(&stop));
        s.pending = Some(Work {
            seq: 1,
            view_id: ready.view_id,
            rev: ready.rev,
            name: ready.name,
            draft: ready.draft,
            check: false,
            stop,
        });
    }
    f.service.inner.wake.notify_one();
    let end = Instant::now() + Duration::from_secs(3);
    while f.service.operation(1).unwrap()["phase"] != "succeeded" {
        assert!(Instant::now() < end);
        thread::sleep(Duration::from_millis(2));
    }
    let done = f.service.operation(1).unwrap();
    assert_eq!(done["recover"], true);
    assert_eq!(done["recovered"], true);
    assert_eq!(done["published"], false);
    assert_eq!(fs::read(target).unwrap(), payload);
    assert!(!stage.exists());
}

#[test]
fn uncertain_publication_cannot_be_cleared_by_cancel_or_new_draft() {
    let f = Fixture::new();
    {
        let mut s = f.service.inner.state.lock().unwrap();
        s.ledger
            .admit(1, "ambiguous syscall".into(), "design_default")
            .unwrap();
        s.ledger.update(
            1,
            json!({"seq":"1","phase":"uncertain","published":null,"error":"publication_unknown"}),
            false,
        );
    }
    assert_eq!(f.service.begin().unwrap_err(), "busy");
    assert_eq!(f.service.cancel(1).unwrap()["phase"], "uncertain");
    assert_eq!(f.service.reconcile(1).unwrap_err(), "publication_unknown");
    assert_eq!(f.service.status()["operations"]["active"], "1");
}
