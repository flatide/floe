use super::*;
use crate::managed::{Limits, Usage};
use std::{
    fs,
    sync::{atomic::AtomicU64, mpsc},
    time::Duration,
};

static SERIAL: AtomicU64 = AtomicU64::new(0);
fn flag() -> Arc<AtomicUsize> {
    Arc::new(AtomicUsize::new(0))
}

#[test]
fn blocked_export_keeps_admission_until_native_unwind_after_retirement() {
    struct Block {
        entered: Option<mpsc::SyncSender<()>>,
        release: mpsc::Receiver<()>,
    }
    impl std::io::Write for Block {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if let Some(tx) = self.entered.take() {
                tx.send(()).unwrap();
                self.release.recv_timeout(Duration::from_secs(10)).unwrap();
            }
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    for k in [store::Kind::Notes, store::Kind::Waives] {
        let f = Fixture::new();
        let m = f.open(k);
        let target = m.target().to_owned();
        let snapshot = m.snapshot(flag()).unwrap();
        let (tx, rx) = mpsc::sync_channel(1);
        let (release, wait) = mpsc::sync_channel(1);
        let worker = thread::spawn(move || {
            snapshot.export(Block {
                entered: Some(tx),
                release: wait,
            })
        });
        rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(kind(m.snapshot(flag())), ErrorKind::Busy);
        m.request_stop();
        drop(m);
        assert_eq!(f.resources.usage().cpu_slots, 1);
        assert_eq!(
            kind(f.resources.index([f.pack.clone()], 1)),
            ErrorKind::Busy
        );
        release.send(()).unwrap();
        assert_eq!(kind(worker.join().unwrap()), ErrorKind::Cancelled);
        assert_eq!(f.resources.usage(), Usage::default());
        assert!(!target.exists());
        f.preserved();
    }
}

#[test]
fn managed_whole_import_preserves_borrow_confirmation_cancellation_and_reader_proof() {
    let f = Fixture::new();
    let m = f.open(store::Kind::Waives);
    let mut bytes = Vec::new();
    m.snapshot(flag()).unwrap().export(&mut bytes).unwrap();
    bytes[40] = 1;
    bytes[104] = 255;
    let input = f.dir.join("portable.waive");
    fs::write(&input, &bytes).unwrap();
    let prepare = || {
        m.snapshot(flag())
            .unwrap()
            .prepare_waives_import(fs::File::open(&input).unwrap())
            .unwrap()
            .0
    };
    let draft = prepare();
    assert!(draft.legacy_unverified());
    assert_eq!(kind(m.snapshot(flag())), ErrorKind::Busy);
    assert_eq!(kind(draft.publish(false)), ErrorKind::Unsupported);
    assert!(m.is_idle());
    assert!(!m.target().exists());
    let mut job = prepare().publish(true).unwrap();
    let done = finish(&mut job);
    assert_eq!(done.phase, Phase::Succeeded);
    let proof = done.outcome.unwrap();
    let mut reader = crate::drc::Database::open_explicit(&f.pack, None, &flag()).unwrap();
    m.snapshot_published(&proof, flag())
        .unwrap()
        .apply_waives(&mut reader, &flag())
        .unwrap();
    assert!(reader.has_waives());
    assert_eq!(
        m.snapshot(flag())
            .unwrap()
            .selected_statuses(&[0, 64])
            .unwrap(),
        [1, 255]
    );
    let before = fs::read(m.target()).unwrap();
    let draft = prepare();
    m.request_stop();
    assert_eq!(kind(draft.publish(true)), ErrorKind::Cancelled);
    assert!(m.is_idle());
    assert_eq!(fs::read(m.target()).unwrap(), before);
    assert_eq!(fs::read(input).unwrap(), bytes);
    drop(m);
    assert_eq!(f.resources.usage(), Usage::default());
    f.preserved();
}

#[test]
fn selected_snapshot_is_bound_to_reader_and_managed_lifetime() {
    let f = Fixture::new();
    let m = f.open(store::Kind::Waives);
    let reader = crate::drc::Database::packed(crate::drc::Pack::open(&f.pack, &flag()).unwrap());
    reader.validate_review_identity(&m.identity()).unwrap();
    let snapshot = m.snapshot(flag()).unwrap();
    assert_eq!(snapshot.selected_statuses(&[64, 0, 64]).unwrap(), [0, 0, 0]);
    m.request_stop();
    assert_eq!(kind(snapshot.selected_statuses(&[0])), ErrorKind::Cancelled);
    drop(m);
    assert_ne!(f.resources.usage(), Usage::default());
    drop(snapshot);
    assert_eq!(f.resources.usage(), Usage::default());
    f.preserved();
}
fn kind<T>(r: Result<T>) -> ErrorKind {
    match r {
        Ok(_) => panic!("expected error"),
        Err(e) => e.kind,
    }
}
struct Fixture {
    dir: PathBuf,
    pack: PathBuf,
    source: PathBuf,
    bytes: Vec<u8>,
    scope: Arc<AccessScope>,
    resources: Arc<Resources>,
}
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "floe-managed-review-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        let dir = fs::canonicalize(dir).unwrap();
        let pack = dir.join("합성.db.ice");
        let source = dir.join("합성.db");
        let bytes = crate::drc::tests::bytes(65);
        fs::write(&pack, &bytes).unwrap();
        fs::write(&source, b"protected synthetic source").unwrap();
        let scope = AccessScope::new(std::slice::from_ref(&dir)).unwrap();
        Self {
            dir,
            pack,
            source,
            bytes,
            scope,
            resources: Resources::new(Limits::default()).unwrap(),
        }
    }
    fn registration(&self, kind: store::Kind) -> Registration {
        Registration {
            scope: Arc::clone(&self.scope),
            pack: self.pack.clone(),
            reviewer: "reviewer".into(),
            kind,
            protected_files: vec![self.source.clone()],
            protected_trees: vec![],
        }
    }
    fn open(&self, kind: store::Kind) -> Arc<ManagedStore> {
        ManagedStore::open(
            &self.resources,
            self.registration(kind),
            &AtomicUsize::new(0),
        )
        .unwrap()
    }
    fn note(&self, m: &Arc<ManagedStore>, text: &str) -> Prepared {
        m.snapshot(flag())
            .unwrap()
            .prepare_note(&[0, 1], text)
            .unwrap()
    }
    fn preserved(&self) {
        assert_eq!(fs::read(&self.pack).unwrap(), self.bytes);
        assert_eq!(
            fs::read(&self.source).unwrap(),
            b"protected synthetic source"
        );
        assert!(!fs::read_dir(&self.dir).unwrap().any(|p| p
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".floe-review-")));
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.dir).unwrap();
    }
}
fn finish(job: &mut Publication) -> Status {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !job.is_finished() {
        assert!(Instant::now() < deadline, "publication did not finish");
        thread::sleep(Duration::from_millis(1));
    }
    job.close().unwrap();
    let status = job.status();
    assert!(status.terminal());
    status
}

#[test]
fn admission_precedes_pack_open_and_failed_open_releases_every_reservation() {
    let f = Fixture::new();
    let write = f.resources.index([f.pack.clone()], 1).unwrap();
    let prior = f.resources.usage();
    assert_eq!(
        kind(ManagedStore::open(
            &f.resources,
            f.registration(store::Kind::Notes),
            &AtomicUsize::new(0)
        )),
        ErrorKind::Busy
    );
    assert_eq!(f.resources.usage(), prior);
    drop(write);
    fs::write(&f.pack, b"corrupt").unwrap();
    assert!(ManagedStore::open(
        &f.resources,
        f.registration(store::Kind::Notes),
        &AtomicUsize::new(0)
    )
    .is_err());
    assert_eq!(f.resources.usage(), Usage::default());
    fs::write(&f.pack, &f.bytes).unwrap();
    assert_eq!(
        kind(ManagedStore::open(
            &f.resources,
            f.registration(store::Kind::Notes),
            &AtomicUsize::new(1)
        )),
        ErrorKind::Cancelled
    );
    assert_eq!(f.resources.usage(), Usage::default());
    let limited = Resources::new(Limits {
        cpu_slots: 2,
        foreground_reserve: 0,
        workers: 1,
        decoded_mb: 255,
    })
    .unwrap();
    assert_eq!(
        kind(ManagedStore::open(
            &limited,
            f.registration(store::Kind::Notes),
            &AtomicUsize::new(0)
        )),
        ErrorKind::Busy
    );
    assert_eq!(limited.usage(), Usage::default());
    f.preserved();
}

#[test]
fn one_live_snapshot_or_draft_owns_admission_without_creating_files() {
    let f = Fixture::new();
    let m = f.open(store::Kind::Notes);
    assert_eq!(f.resources.usage().cpu_slots, 1);
    assert_eq!(f.resources.usage().decoded_mb, 256);
    assert_eq!(f.resources.usage().workers, 0);
    assert!(m.is_idle());
    let snapshot = m.snapshot(flag()).unwrap();
    assert!(!snapshot.exists());
    assert!(!snapshot.legacy_unverified());
    assert_eq!(kind(m.snapshot(flag())), ErrorKind::Busy);
    let draft = snapshot.prepare_note(&[0], "preview only").unwrap();
    assert_eq!(draft.kind(), store::Kind::Notes);
    assert_eq!(draft.target(), m.target());
    assert!(!draft.legacy_unverified());
    assert_eq!(kind(m.snapshot(flag())), ErrorKind::Busy);
    for path in [&f.pack, &f.source] {
        assert_eq!(kind(f.resources.index([path.clone()], 1)), ErrorKind::Busy);
    }
    drop(f.resources.index([f.dir.join("unrelated")], 1).unwrap());
    drop(draft);
    assert!(m.is_idle());
    assert_eq!(fs::read_dir(&f.dir).unwrap().count(), 2);
    drop(m);
    assert_eq!(f.resources.usage(), Usage::default());
    f.preserved();
}

#[test]
fn snapshot_and_ready_keep_pack_lease_after_registration_handle_is_dropped() {
    let f = Fixture::new();
    let m = f.open(store::Kind::Notes);
    let weak = Arc::downgrade(&m);
    let snapshot = m.snapshot(flag()).unwrap();
    drop(m);
    assert!(weak.upgrade().is_some());
    assert_eq!(
        kind(f.resources.index([f.pack.clone()], 1)),
        ErrorKind::Busy
    );
    let draft = snapshot.prepare_note(&[0], "held preview").unwrap();
    assert_eq!(
        kind(f.resources.index([f.pack.clone()], 1)),
        ErrorKind::Busy
    );
    drop(draft);
    assert!(weak.upgrade().is_none());
    assert_eq!(f.resources.usage(), Usage::default());
    drop(f.resources.index([f.pack.clone()], 1).unwrap());
    f.preserved();
}

#[test]
fn failed_preparation_and_caller_cancellation_release_the_busy_slot() {
    let f = Fixture::new();
    let m = f.open(store::Kind::Notes);
    assert!(m
        .snapshot(flag())
        .unwrap()
        .prepare_note(&[65], "invalid")
        .is_err());
    assert!(m.is_idle());
    assert!(m
        .snapshot(flag())
        .unwrap()
        .prepare_waives(&[(0, 1)])
        .is_err());
    assert!(m.is_idle());
    let stop = flag();
    let snapshot = m.snapshot(Arc::clone(&stop)).unwrap();
    stop.store(1, Ordering::Relaxed);
    assert_eq!(
        kind(snapshot.prepare_note(&[0], "cancelled")),
        ErrorKind::Cancelled
    );
    assert!(m.is_idle());
    assert_eq!(
        kind(m.snapshot(Arc::new(AtomicUsize::new(1)))),
        ErrorKind::Cancelled
    );
    assert!(m.is_idle());
    f.preserved();
}

#[test]
fn retirement_invalidates_ready_and_never_creates_a_lock() {
    let f = Fixture::new();
    let m = f.open(store::Kind::Notes);
    let d = f.note(&m, "retired");
    m.request_stop();
    assert_eq!(kind(d.publish(false)), ErrorKind::Cancelled);
    assert!(m.is_idle());
    assert_eq!(kind(m.snapshot(flag())), ErrorKind::Cancelled);
    assert_eq!(fs::read_dir(&f.dir).unwrap().count(), 2);
    drop(m);
    assert_eq!(f.resources.usage(), Usage::default());
}

#[test]
fn native_note_and_waive_publication_can_be_polled_and_reloaded_without_observers() {
    let f = Fixture::new();
    let n = f.open(store::Kind::Notes);
    let mut note = f.note(&n, "saved \n한글").publish(false).unwrap();
    let status = finish(&mut note);
    assert_eq!(status.phase, Phase::Succeeded);
    assert!(status.outcome.unwrap().directory_synced);
    assert!(!status.outcome_unknown);
    assert!(status.failure.is_none());
    assert_eq!(note.status().id, status.id);
    assert!(n.is_idle());
    assert_eq!(
        n.snapshot(flag()).unwrap().notes().unwrap().get(0),
        Some("saved \n한글")
    );
    let w = f.open(store::Kind::Waives);
    let draft = w
        .snapshot(flag())
        .unwrap()
        .prepare_waives(&[(0, 1), (64, 1)])
        .unwrap();
    let mut waive = draft.publish(false).unwrap();
    let status2 = finish(&mut waive);
    assert!(status2.id > status.id);
    assert_eq!(status2.phase, Phase::Succeeded);
    assert_eq!(w.snapshot(flag()).unwrap().waives().unwrap().waived, 2);
    // Terminal drops each job borrow, not the still-registered stores' leases.
    assert_eq!(f.resources.usage().cpu_slots, 2);
    drop((n, w));
    assert_eq!(f.resources.usage(), Usage::default());
    f.preserved();
}

#[test]
fn normalization_legacy_confirmation_and_stale_revision_are_preserved() {
    let f = Fixture::new();
    let m = f.open(store::Kind::Notes);
    fs::write(m.target(), b"floe_pack=1234,5678,65\nfloe_note=0|legacy\n").unwrap();
    let d = f.note(&m, "must confirm");
    assert!(d.legacy_unverified());
    assert_eq!(kind(d.publish(false)), ErrorKind::Unsupported);
    assert!(m.is_idle());
    let (d, report) = m
        .snapshot(flag())
        .unwrap()
        .prepare_notes_import("floe_pack=1234,5678,65\nfloe_note=0|imported\n")
        .unwrap();
    assert_eq!(report.invalid_members, 0);
    let mut job = d.publish(true).unwrap();
    assert_eq!(finish(&mut job).phase, Phase::Succeeded);
    let d = f.note(&m, "stale");
    let before = fs::read(m.target()).unwrap();
    fs::write(m.target(), &before).unwrap();
    let mut job = d.publish(false).unwrap();
    let status = finish(&mut job);
    assert_eq!(status.phase, Phase::Failed);
    assert_eq!(status.failure, Some(ErrorKind::Busy));
    assert!(!status.outcome_unknown);
    assert_eq!(fs::read(m.target()).unwrap(), before);
    f.preserved();
}

#[test]
fn cancel_during_work_keeps_leases_until_the_worker_releases_them() {
    let f = Fixture::new();
    let m = f.open(store::Kind::Notes);
    let weak = Arc::downgrade(&m);
    let target = m.target().to_owned();
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let mut job = f
        .note(&m, "blocked worker")
        .start_using(false, move |d, stop| {
            ready_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            d.publish(stop)
        })
        .unwrap();
    ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    m.request_stop();
    job.cancel();
    drop(m);
    assert_eq!(job.status().phase, Phase::Cancelling);
    assert!(!job.status().terminal());
    assert!(weak.upgrade().is_some());
    assert_eq!(
        kind(f.resources.index([f.pack.clone()], 1)),
        ErrorKind::Busy
    );
    release_tx.send(()).unwrap();
    assert_eq!(finish(&mut job).phase, Phase::Cancelled);
    assert!(!target.exists());
    assert!(weak.upgrade().is_none());
    assert_eq!(f.resources.usage(), Usage::default());
    f.preserved();
}

#[test]
fn late_cancel_durability_warning_and_worker_panic_do_not_claim_rollback() {
    let f = Fixture::new();
    let m = f.open(store::Kind::Notes);
    let mut job = f
        .note(&m, "committed")
        .start_using(false, |d, stop| {
            let mut outcome = d.publish(stop)?;
            stop.store(1, Ordering::Relaxed);
            outcome.directory_synced = false;
            Ok(outcome)
        })
        .unwrap();
    let status = finish(&mut job);
    assert_eq!(status.phase, Phase::Succeeded);
    assert!(!status.outcome.unwrap().directory_synced);
    assert!(!status.outcome_unknown);
    assert_eq!(
        m.snapshot(flag()).unwrap().notes().unwrap().get(0),
        Some("committed")
    );
    let mut job = f
        .note(&m, "committed before panic")
        .start_using(false, |d, stop| {
            d.publish(stop)?;
            panic!("synthetic post-commit worker failure");
        })
        .unwrap();
    let status = finish(&mut job);
    assert_eq!(status.phase, Phase::Failed);
    assert_eq!(status.failure, Some(ErrorKind::Worker));
    assert!(status.outcome_unknown);
    assert!(status.outcome.is_none());
    assert_eq!(
        m.snapshot(flag()).unwrap().notes().unwrap().get(0),
        Some("committed before panic")
    );
    f.preserved();
}

#[test]
fn dropping_job_requests_cancel_and_joins_instead_of_detaching() {
    let f = Fixture::new();
    let m = f.open(store::Kind::Notes);
    let weak = Arc::downgrade(&m);
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let job = f
        .note(&m, "drop cancels")
        .start_using(false, move |d, stop| {
            ready_tx.send(()).unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while stop.load(Ordering::Relaxed) == 0 {
                assert!(Instant::now() < deadline);
                thread::sleep(Duration::from_millis(1));
            }
            d.publish(stop)
        })
        .unwrap();
    ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let target = m.target().to_owned();
    drop(m);
    drop(job);
    assert!(weak.upgrade().is_none());
    assert_eq!(f.resources.usage(), Usage::default());
    assert!(!target.exists());
    f.preserved();
}
