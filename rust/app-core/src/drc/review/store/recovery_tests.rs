use super::*;
use std::{
    os::unix::fs::{symlink, PermissionsExt},
    sync::atomic::{AtomicU64, Ordering},
};
static SERIAL: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    root: PathBuf,
    store: Arc<Store>,
    stage: PathBuf,
    stop: AtomicUsize,
}
impl Fixture {
    fn new(kind: Kind) -> Self {
        let root = std::env::temp_dir().join(format!(
            "floe-review-recovery-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let pack = root.join("합성.db.ice");
        fs::write(&pack, crate::drc::tests::bytes(2)).unwrap();
        let scope = AccessScope::new(std::slice::from_ref(&root)).unwrap();
        let stop = AtomicUsize::new(0);
        let store = Store::open(scope, &pack, "reviewer", kind, vec![], vec![], &stop).unwrap();
        let snap = store.snapshot(&stop).unwrap();
        let draft = match kind {
            Kind::Notes => snap
                .prepare_note(&[0, 1], "synthetic recovery", &stop)
                .unwrap(),
            Kind::Waives => snap.prepare_waives(&[(0, 1)], &stop).unwrap(),
        };
        draft.publish(&stop).unwrap();
        let capture = Capture::read(&store, false, &stop).unwrap().unwrap();
        let name = marker(&store, &capture).unwrap();
        let stage = root.join(OsStr::from_bytes(name.as_bytes()));
        // Unit fault model. The separate subprocess gate kills the real writer
        // between Stage::commit's two syscalls, without this reconstruction.
        fs::hard_link(store.target(), &stage).unwrap();
        Self {
            root,
            store,
            stage,
            stop,
        }
    }
    fn prepare(&self) -> Recovery {
        self.store.prepare_recovery(&self.stop).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn preview_is_read_only_and_explicit_recovery_is_exact_and_idempotent() {
    for kind in [Kind::Notes, Kind::Waives] {
        let f = Fixture::new(kind);
        let bytes = fs::read(f.store.target()).unwrap();
        let pack = fs::read(&f.store.pack_path).unwrap();
        let security = Security::read(&File::open(&f.stage).unwrap()).unwrap();
        let inode = identity(&fs::metadata(&f.stage).unwrap());
        let lock = identity(&fs::metadata(f.store.lock_path()).unwrap());
        let preview = f.prepare();
        assert_eq!(preview.kind(), kind);
        assert_eq!(preview.bytes(), bytes.len() as u64);
        assert_eq!(preview.reconcile(&f.stop).unwrap(), RecoveryState::Pending);
        assert_eq!(fs::metadata(&f.stage).unwrap().nlink(), 2);
        assert!(f.store.snapshot(&f.stop).is_err()); // normal read still refuses
        assert_eq!(
            preview.recover(&f.stop).unwrap(),
            RecoveryResult::Recovered {
                already_completed: false,
                directory_synced: true
            }
        );
        assert!(!f.stage.exists());
        assert_eq!(identity(&fs::metadata(f.store.target()).unwrap()), inode);
        assert_eq!(fs::read(f.store.target()).unwrap(), bytes);
        assert_eq!(fs::read(&f.store.pack_path).unwrap(), pack);
        assert!(Security::read(&File::open(f.store.target()).unwrap()).unwrap() == security);
        assert_eq!(identity(&fs::metadata(f.store.lock_path()).unwrap()), lock);
        assert_eq!(
            preview.reconcile(&f.stop).unwrap(),
            RecoveryState::Completed
        );
        assert_eq!(
            preview.recover(&f.stop).unwrap(),
            RecoveryResult::Recovered {
                already_completed: true,
                directory_synced: true
            }
        );
        let snapshot = f.store.snapshot(&f.stop).unwrap();
        match kind {
            Kind::Notes => assert_eq!(snapshot.notes().unwrap().get(0), Some("synthetic recovery")),
            Kind::Waives => assert_eq!(
                snapshot.selected_statuses(&[0, 1], &f.stop).unwrap(),
                [1, 0]
            ),
        }
    }
}

#[test]
fn legacy_malformed_foreign_and_path_markers_fail_without_deletion() {
    for mutation in [
        "legacy", "invalid", "version", "parent", "inode", "target", "stage", "binding",
    ] {
        let f = Fixture::new(Kind::Notes);
        let file = File::open(&f.stage).unwrap();
        let security = Security::read(&file).unwrap();
        let original = security.attribute(MARKER).unwrap();
        let mut m: serde_json::Value = serde_json::from_slice(original).unwrap();
        match mutation {
            "legacy" => security.without_attribute(MARKER).apply(&file).unwrap(),
            "invalid" => crate::layer_defaults::security::set(&file, MARKER, b"{").unwrap(),
            "binding" => crate::layer_defaults::security::set(&file, BINDING, b"wrong").unwrap(),
            field => {
                match field {
                    "version" => m["v"] = 2.into(),
                    "parent" => m["directory"] = serde_json::json!([0, 0]),
                    "inode" => m["file"] = serde_json::json!([0, 0]),
                    "target" => m["target"] = serde_json::json!([1]),
                    "stage" => m["stage"] = "../.floe-review-1-1.tmp".into(),
                    _ => unreachable!(),
                }
                crate::layer_defaults::security::set(
                    &file,
                    MARKER,
                    &serde_json::to_vec(&m).unwrap(),
                )
                .unwrap();
            }
        }
        let bytes = fs::read(&f.stage).unwrap();
        assert!(f.store.prepare_recovery(&f.stop).is_err(), "{mutation}");
        assert_eq!(fs::metadata(&f.stage).unwrap().nlink(), 2);
        assert_eq!(fs::read(f.store.target()).unwrap(), bytes);
    }
}

#[test]
fn changed_targets_stages_locks_and_extra_links_invalidate_approval() {
    for mutation in [
        "bytes",
        "mode",
        "target",
        "stage",
        "symlink",
        "lock",
        "empty_lock",
        "extra",
        "pack",
    ] {
        let f = Fixture::new(Kind::Notes);
        let preview = f.prepare();
        match mutation {
            "bytes" => fs::write(&f.stage, b"changed").unwrap(),
            "mode" => fs::set_permissions(&f.stage, fs::Permissions::from_mode(0o640)).unwrap(),
            "target" => {
                let security = Security::read(&File::open(&f.stage).unwrap()).unwrap();
                fs::remove_file(f.store.target()).unwrap();
                fs::write(f.store.target(), fs::read(&f.stage).unwrap()).unwrap();
                security
                    .apply(&File::open(f.store.target()).unwrap())
                    .unwrap();
            }
            "stage" | "symlink" => {
                fs::remove_file(&f.stage).unwrap();
                if mutation == "stage" {
                    fs::write(&f.stage, b"do not remove").unwrap();
                } else {
                    symlink(f.store.target(), &f.stage).unwrap();
                }
            }
            "lock" => {
                fs::remove_file(f.store.lock_path()).unwrap();
                fs::write(f.store.lock_path(), b"not a lock").unwrap();
            }
            "empty_lock" => {
                fs::rename(f.store.lock_path(), f.root.join("original-lock")).unwrap();
                fs::write(f.store.lock_path(), b"").unwrap();
            }
            "extra" => fs::hard_link(&f.stage, f.root.join("third-link")).unwrap(),
            "pack" => fs::write(&f.store.pack_path, b"changed pack").unwrap(),
            _ => unreachable!(),
        }
        assert!(preview.recover(&f.stop).is_err(), "{mutation}");
        assert!(fs::symlink_metadata(&f.stage).is_ok(), "{mutation}");
    }
}

#[test]
fn readonly_dynamic_protection_cancellation_and_expiry_do_not_repair() {
    let f = Fixture::new(Kind::Notes);
    let reader = Store::open_readonly_catalog(
        f.store.scope.clone(),
        &f.store.pack_path,
        "reviewer",
        Kind::Notes,
        vec![],
        vec![],
        Arc::clone(&f.store.sources),
        f.store.target(),
        &f.stop,
    )
    .unwrap();
    assert!(reader.prepare_recovery(&f.stop).is_err());
    let mut preview = f.prepare();
    f.stop.store(1, Ordering::Relaxed);
    assert!(preview.recover(&f.stop).is_err());
    f.stop.store(0, Ordering::Relaxed);
    preview.expires = Instant::now();
    assert!(preview.recover(&f.stop).is_err());
    assert_eq!(preview.reconcile(&f.stop).unwrap(), RecoveryState::Pending);
    let preview = f.prepare();
    let mut pending = f.store.sources.begin(&f.stop).unwrap();
    pending
        .protect_inputs(std::slice::from_ref(&f.stage), &[], &f.stop)
        .unwrap();
    pending.commit(&f.stop).unwrap();
    assert!(preview.recover(&f.stop).is_err());
    assert_eq!(fs::metadata(&f.stage).unwrap().nlink(), 2);
}

#[test]
fn failed_unlink_is_uncertain_and_committed_error_is_reconciled_without_retry() {
    let f = Fixture::new(Kind::Waives);
    let preview = f.prepare();
    assert_eq!(
        preview
            .recover_using(
                &f.stop,
                || Err(std::io::Error::from_raw_os_error(libc::EIO)),
                File::sync_all
            )
            .unwrap(),
        RecoveryResult::Uncertain
    );
    assert_eq!(preview.reconcile(&f.stop).unwrap(), RecoveryState::Pending);
    assert!(f.stage.exists());
    assert_eq!(
        preview
            .recover_using(
                &f.stop,
                || {
                    fs::remove_file(&f.stage).unwrap();
                    f.stop.store(1, Ordering::Relaxed); // late cancellation cannot undo commit
                    Err(std::io::Error::from_raw_os_error(libc::EIO))
                },
                |_| Err(std::io::Error::from_raw_os_error(libc::EIO))
            )
            .unwrap(),
        RecoveryResult::Recovered {
            already_completed: false,
            directory_synced: false
        }
    );
    f.stop.store(0, Ordering::Relaxed);
    assert_eq!(
        preview.reconcile(&f.stop).unwrap(),
        RecoveryState::Completed
    );
    assert_eq!(
        preview
            .recover_using(&f.stop, || panic!("must not retry unlink"), File::sync_all)
            .unwrap(),
        RecoveryResult::Recovered {
            already_completed: true,
            directory_synced: true
        }
    );
}

#[test]
fn successful_unlink_with_changed_target_stays_uncertain() {
    let f = Fixture::new(Kind::Notes);
    let preview = f.prepare();
    assert_eq!(
        preview
            .recover_using(
                &f.stop,
                || {
                    fs::remove_file(&f.stage)?;
                    fs::write(f.store.target(), b"concurrent writer")?;
                    Ok(())
                },
                File::sync_all
            )
            .unwrap(),
        RecoveryResult::Uncertain
    );
    assert!(preview.reconcile(&f.stop).is_err());
    assert_eq!(fs::read(f.store.target()).unwrap(), b"concurrent writer");
}
