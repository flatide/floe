use super::*;
use crate::exports::artifacts::Limits;
use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::Write,
    sync::{atomic::AtomicU64, Barrier},
};
static SERIAL: AtomicU64 = AtomicU64::new(0);
fn artifact() -> ClipArtifact {
    let p = std::env::temp_dir().join(format!(
        "floe-export-finish-{}-{}",
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    ));
    let mut writer = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&p)
        .unwrap();
    writer.write_all(b"export").unwrap();
    drop(writer);
    let file = fs::File::open(&p).unwrap();
    fs::remove_file(p).unwrap();
    ClipArtifact {
        file,
        size_bytes: 6,
        fields: floe_worker_client::Fields(BTreeMap::from([("records".into(), "1".into())])),
    }
}
fn state() -> State {
    State {
        sealed: false,
        snapshot: Snapshot {
            id: 1,
            dataset_revision: 1,
            phase: Phase::Finishing,
            elapsed_ms: 0,
            native_pid: None,
            outcome: None,
            failure: None,
        },
    }
}
#[test]
fn cancel_and_publish_have_exactly_one_winner() {
    for _ in 0..32 {
        let store = Store::new(Limits::default()).unwrap();
        let stop = Arc::new(AtomicUsize::new(0));
        let reservation = store.reserve(1, Arc::clone(&stop)).unwrap();
        let state = Mutex::new(state());
        let barrier = Barrier::new(2);
        let output = artifact();
        thread::scope(|scope| {
            scope.spawn(|| {
                barrier.wait();
                let s = state.lock().unwrap();
                if !s.sealed {
                    stop.store(1, Ordering::Relaxed);
                }
            });
            barrier.wait();
            finish(
                &mut state.lock().unwrap(),
                reservation,
                Ok(output),
                [0, 0, 1, 1],
                false,
                &stop,
            );
        });
        let s = state.lock().unwrap();
        match s.snapshot.phase {
            Phase::Ready => {
                assert!(s.sealed);
                assert!(store.open(1).is_ok());
                assert_eq!(stop.load(Ordering::Relaxed), 0);
            }
            Phase::Cancelled => {
                assert!(!s.sealed);
                assert!(store.open(1).is_err());
                assert_eq!(store.usage(), Default::default());
            }
            other => panic!("unexpected phase {other:?}"),
        }
    }
}
#[test]
fn errors_are_not_disguised_as_cancellation_and_retired_results_are_absent() {
    let store = Store::new(Limits::default()).unwrap();
    let stop = Arc::new(AtomicUsize::new(0));
    let reservation = store.reserve(1, Arc::clone(&stop)).unwrap();
    let mut s = state();
    stop.store(1, Ordering::Relaxed);
    finish(
        &mut s,
        reservation,
        Err(Error::new(ErrorKind::Io, "disk full")),
        [0, 0, 1, 1],
        false,
        &stop,
    );
    assert_eq!(s.snapshot.phase, Phase::Failed);
    assert_eq!(s.snapshot.failure, Some(ErrorKind::Io));
    assert!(store.open(1).is_err());
    assert_eq!(store.usage(), Default::default());
}
#[test]
fn export_cpu_memory_and_worker_reservations_balance() {
    let resources = Resources::new(crate::managed::Limits::default()).unwrap();
    let a = resources.export(4, 256).unwrap();
    assert_eq!(resources.usage().cpu_slots, 4);
    assert_eq!(resources.usage().workers, 1);
    let b = resources.export(4, 256).unwrap();
    assert!(resources.export(1, 1).is_err());
    assert!(resources.export(0, 1).is_err());
    assert!(resources.export(17, 1).is_err());
    assert!(resources.export(1, 0).is_err());
    drop(a);
    drop(b);
    assert_eq!(resources.usage(), Default::default());
}
