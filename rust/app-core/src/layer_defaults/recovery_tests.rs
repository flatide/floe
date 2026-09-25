use super::*;
use crate::layer_defaults::tests::Fixture;

fn gap(f: &Fixture) -> PathBuf {
    f.draft().publish(&f.stop).unwrap();
    let file = File::open(f.target()).unwrap();
    let security = Security::read(&file).unwrap();
    let m: Marker = serde_json::from_slice(security.attribute(MARKER).unwrap()).unwrap();
    let stage = f.dir.join(m.stage);
    fs::hard_link(f.target(), &stage).unwrap();
    stage
}
fn prepare(f: &Fixture) -> Recovery {
    f.publisher
        .prepare_recovery(Arc::clone(&f.source), Mode::Level, &f.stop)
        .unwrap()
}
#[test]
fn explicit_repair_preserves_payload_permissions_and_other_files() {
    let f = Fixture::new();
    let stage = gap(&f);
    let bytes = fs::read(f.target()).unwrap();
    let file = File::open(f.target()).unwrap();
    let security = Security::read(&file).unwrap();
    let stamp = Stamp::of(&file.metadata().unwrap());
    fs::write(f.dir.join(".floe-layerprops-999-999.tmp"), b"untouched").unwrap();
    let proof = prepare(&f);
    assert_eq!(proof.bytes(), bytes.len());
    assert_eq!(proof.target(), fs::canonicalize(f.target()).unwrap());
    assert_eq!(file.metadata().unwrap().nlink(), 2);
    assert!(f
        .publisher
        .prepare(
            Arc::clone(&f.source),
            Mode::Level,
            "1 red solid X 1 1",
            &f.stop
        )
        .is_err());
    assert!(matches!(
        proof.recover(&f.stop).unwrap(),
        RecoveryResult::Recovered { .. }
    ));
    assert!(!stage.exists());
    assert_eq!(fs::read(f.target()).unwrap(), bytes);
    assert!(Security::read(&file).unwrap() == security);
    let after = Stamp::of(&file.metadata().unwrap());
    assert_eq!(after.id, stamp.id);
    assert_eq!(after.modified, stamp.modified);
    assert_eq!(after.nlink, 1);
    assert_eq!(proof.reconcile(&f.stop).unwrap(), RecoveryState::Completed);
    assert!(proof.recover(&f.stop).is_ok());
    assert_eq!(
        fs::read(f.dir.join(".floe-layerprops-999-999.tmp")).unwrap(),
        b"untouched"
    );
}
#[test]
fn stale_unmarked_cancelled_and_unrelated_links_are_refused() {
    for change in 0..5 {
        let f = Fixture::new();
        let stage = gap(&f);
        let proof = prepare(&f);
        match change {
            0 => {
                fs::write(f.target(), b"2 blue solid Changed 1 1").unwrap();
            }
            1 => {
                fs::hard_link(f.target(), f.dir.join("third-link")).unwrap();
            }
            2 => {
                fs::remove_file(&stage).unwrap();
                fs::write(&stage, b"unrelated").unwrap();
            }
            3 => {
                f.stop.store(1, Ordering::Relaxed);
            }
            _ => {
                fs::write(f.source.path(), b"changed source").unwrap();
            }
        }
        assert!(proof.recover(&f.stop).is_err());
        assert!(stage.exists());
    }
    let f = Fixture::new();
    fs::write(f.target(), b"1 red solid X 1 1").unwrap();
    fs::hard_link(f.target(), f.dir.join("unmarked")).unwrap();
    assert!(f
        .publisher
        .prepare_recovery(Arc::clone(&f.source), Mode::Level, &f.stop)
        .is_err());
    assert!(!f.dir.join("design.jb.layerprops.lock").exists());
}
#[test]
fn unknown_unlink_is_not_retried_and_can_only_be_inspected() {
    let f = Fixture::new();
    let stage = gap(&f);
    let proof = prepare(&f);
    let failed = || Err(std::io::Error::from_raw_os_error(libc::EIO));
    assert_eq!(
        proof
            .recover_using(&f.stop, failed, File::sync_all)
            .unwrap(),
        RecoveryResult::Uncertain
    );
    assert_eq!(proof.reconcile(&f.stop).unwrap(), RecoveryState::Pending);
    assert!(stage.exists());
    let result = proof
        .recover_using(
            &f.stop,
            || {
                fs::remove_file(&stage).unwrap();
                failed()
            },
            |_| failed(),
        )
        .unwrap();
    assert_eq!(
        result,
        RecoveryResult::Recovered {
            directory_synced: false
        }
    );
    assert_eq!(proof.reconcile(&f.stop).unwrap(), RecoveryState::Completed);
}

#[test]
fn marker_lock_expiry_and_dynamic_input_guards_are_not_bypassed() {
    for change in 0..5 {
        let f = Fixture::new();
        let stage = gap(&f);
        let mut proof = prepare(&f);
        match change {
            0 => {
                proof.draft.expires = Instant::now();
            }
            1 => {
                let lock = f.dir.join("design.jb.layerprops.lock");
                fs::rename(&lock, f.dir.join("old-lock")).unwrap();
                fs::write(lock, b"").unwrap();
            }
            2 => {
                let file = File::options().write(true).open(f.target()).unwrap();
                security::set(&file, MARKER, b"{\"v\":999}").unwrap();
            }
            3 => {
                let mut p = f.publisher.sources.begin(&f.stop).unwrap();
                p.protect_inputs(std::slice::from_ref(&stage), &[], &f.stop)
                    .unwrap();
                p.commit(&f.stop).unwrap();
            }
            _ => {
                let mut p = f.publisher.sources.begin(&f.stop).unwrap();
                p.protect_inputs(&[f.target()], &[], &f.stop).unwrap();
                p.commit(&f.stop).unwrap();
            }
        }
        assert!(proof.recover(&f.stop).is_err());
        assert!(stage.exists());
        assert_eq!(fs::metadata(f.target()).unwrap().nlink(), 2);
    }
}
