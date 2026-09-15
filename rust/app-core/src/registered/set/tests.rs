use super::*;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static SERIAL: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "floe-source-set-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn scope(&self) -> Arc<AccessScope> {
        AccessScope::new(std::slice::from_ref(&self.0)).unwrap()
    }
    fn deck(&self, n: usize) -> PathBuf {
        let path = self.0.join(format!("{n}.jb"));
        fs::write(&path, "CHIP A\n$ (1,A,TC=missing.oas)\n").unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn error<T>(r: Result<T>) -> ErrorKind {
    match r {
        Err(e) => e.kind,
        Ok(_) => panic!("expected error"),
    }
}

#[test]
fn registration_is_atomic_cancellable_bounded_and_reuses_unchanged_sources() {
    let f = Fixture::new();
    let set = SourceSet::new(vec![]).unwrap();
    let stop = AtomicUsize::new(0);
    let path = f.deck(0);
    let mut pending = set.begin(&stop).unwrap();
    let first = pending.register(f.scope(), &path, &stop).unwrap();
    assert!(set.snapshot().is_empty());
    assert!(Arc::ptr_eq(
        &first,
        &pending.register(f.scope(), &path, &stop).unwrap()
    ));
    assert_eq!(error(set.begin(&stop)), ErrorKind::Busy);
    assert_eq!(error(set.publication(&stop)), ErrorKind::Busy);
    assert_eq!(
        error(pending.commit(&AtomicUsize::new(1))),
        ErrorKind::Cancelled
    );
    assert!(set.snapshot().is_empty());
    let mut pending = set.begin(&stop).unwrap();
    for n in 0..MAX_SOURCES {
        pending.register(f.scope(), &f.deck(n), &stop).unwrap();
    }
    assert_eq!(
        error(pending.register(f.scope(), &f.deck(32), &stop)),
        ErrorKind::InvalidInput
    );
    pending.commit(&stop).unwrap();
    assert_eq!(set.snapshot().len(), MAX_SOURCES);
    let mut pending = set.begin(&stop).unwrap();
    let again = pending.register(f.scope(), &path, &stop).unwrap();
    assert!(Arc::ptr_eq(&set.snapshot()[0], &again));
    fs::write(&path, "changed source").unwrap();
    assert_eq!(
        error(pending.register(f.scope(), &path, &stop)),
        ErrorKind::Cache
    );
    drop(pending);
    assert_eq!(set.snapshot().len(), MAX_SOURCES);
}

#[test]
fn scope_failures_rollback_and_both_publication_leases_must_end() {
    let f = Fixture::new();
    let other = Fixture::new();
    let set = SourceSet::new(vec![]).unwrap();
    let stop = AtomicUsize::new(0);
    {
        let mut pending = set.begin(&stop).unwrap();
        pending.register(f.scope(), &f.deck(0), &stop).unwrap();
        assert_eq!(
            error(pending.register(f.scope(), &other.deck(0), &stop)),
            ErrorKind::InvalidInput
        );
    }
    assert!(set.snapshot().is_empty());
    let a = set.publication(&stop).unwrap();
    let b = set.publication(&stop).unwrap();
    assert_eq!(error(set.begin(&stop)), ErrorKind::Busy);
    drop(a);
    assert_eq!(error(set.begin(&stop)), ErrorKind::Busy);
    // Reads do not wait behind the long-lived publication reservation.
    assert!(set.snapshot().is_empty());
    drop(b);
    drop(set.begin(&stop).unwrap());
    assert_eq!(error(set.begin(&AtomicUsize::new(1))), ErrorKind::Cancelled);
    assert_eq!(
        error(set.publication(&AtomicUsize::new(1))),
        ErrorKind::Cancelled
    );
    drop(set.publication(&stop).unwrap());
}

#[test]
fn concurrent_registration_and_publication_have_one_winner() {
    let set = SourceSet::new(vec![]).unwrap();
    for _ in 0..32 {
        let barrier = std::sync::Barrier::new(3);
        let stop = AtomicUsize::new(0);
        let (registered, published) = std::thread::scope(|scope| {
            let a = scope.spawn(|| {
                barrier.wait();
                let lease = set.begin(&stop);
                barrier.wait();
                lease.is_ok()
            });
            let b = scope.spawn(|| {
                barrier.wait();
                let lease = set.publication(&stop);
                barrier.wait();
                lease.is_ok()
            });
            barrier.wait();
            barrier.wait();
            (a.join().unwrap(), b.join().unwrap())
        });
        assert_ne!(registered, published);
        drop(set.begin(&stop).unwrap());
        drop(set.publication(&stop).unwrap());
    }
}
