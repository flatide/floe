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

#[test]
fn input_protection_is_atomic_deny_only_and_survives_source_additions() {
    let f = Fixture::new();
    let set = SourceSet::new(vec![]).unwrap();
    let stop = AtomicUsize::new(0);
    let file = f.0.join("review.db");
    let target = f.0.join("review.fe");
    let tree = f.0.join("pack");
    let check = |p: &Path, k| set.protect_output(p, k);
    let mut pending = set.begin(&stop).unwrap();
    pending
        .protect_inputs(
            std::slice::from_ref(&file),
            std::slice::from_ref(&tree),
            &stop,
        )
        .unwrap();
    pending
        .protect_review_targets(std::slice::from_ref(&target), &stop)
        .unwrap();
    assert!(
        check(&file, PublicationKind::Defaults).is_ok(),
        "uncommitted protection leaked"
    );
    assert_eq!(error(set.publication(&stop)), ErrorKind::Busy);
    assert_eq!(
        error(pending.commit(&AtomicUsize::new(1))),
        ErrorKind::Cancelled
    );
    assert!(check(&file, PublicationKind::Review).is_ok());
    let mut pending = set.begin(&stop).unwrap();
    pending
        .protect_inputs(
            std::slice::from_ref(&file),
            std::slice::from_ref(&tree),
            &stop,
        )
        .unwrap();
    pending
        .protect_review_targets(std::slice::from_ref(&target), &stop)
        .unwrap();
    pending.commit(&stop).unwrap();
    assert_eq!(
        error(check(&file, PublicationKind::Defaults)),
        ErrorKind::InvalidInput
    );
    assert_eq!(
        error(check(&file, PublicationKind::Review)),
        ErrorKind::InvalidInput
    );
    assert_eq!(
        error(check(&tree.join("new"), PublicationKind::Review)),
        ErrorKind::InvalidInput
    );
    assert_eq!(
        error(check(&target, PublicationKind::Defaults)),
        ErrorKind::InvalidInput
    );
    assert!(
        check(&target, PublicationKind::Review).is_ok(),
        "protection is not a review grant or veto"
    );
    let mut pending = set.begin(&stop).unwrap();
    pending.register(f.scope(), &f.deck(0), &stop).unwrap();
    // Marking a path as a review target cannot relax an immutable-input deny.
    pending
        .protect_review_targets(std::slice::from_ref(&file), &stop)
        .unwrap();
    pending.commit(&stop).unwrap();
    assert_eq!(set.snapshot().len(), 1);
    assert_eq!(
        error(check(&file, PublicationKind::Review)),
        ErrorKind::InvalidInput
    );
    assert!(!file.exists() && !target.exists() && !tree.exists());
}

#[test]
fn protected_paths_are_bounded_deduplicated_and_a_failed_batch_is_atomic() {
    let f = Fixture::new();
    let set = SourceSet::new(vec![]).unwrap();
    let stop = AtomicUsize::new(0);
    let files: Vec<_> = (0..1024).map(|n| f.0.join(format!("input-{n}"))).collect();
    let mut p = set.begin(&stop).unwrap();
    p.protect_inputs(&files, &[], &stop).unwrap();
    p.protect_inputs(&files, &[], &stop).unwrap();
    let denied = f.0.join("over-limit");
    assert_eq!(
        error(p.protect_review_targets(std::slice::from_ref(&denied), &stop)),
        ErrorKind::InvalidInput
    );
    p.commit(&stop).unwrap();
    assert!(set
        .protect_output(&denied, PublicationKind::Defaults)
        .is_ok());
    assert_eq!(
        error(set.protect_output(&files[1023], PublicationKind::Review)),
        ErrorKind::InvalidInput
    );
    let set = SourceSet::new(vec![]).unwrap();
    let mut p = set.begin(&stop).unwrap();
    assert_eq!(
        error(p.protect_inputs(&[denied.clone(), PathBuf::new()], &[], &stop)),
        ErrorKind::InvalidInput
    );
    assert_eq!(
        error(p.protect_inputs(&[], &[PathBuf::from("/")], &stop)),
        ErrorKind::InvalidInput
    );
    assert_eq!(
        error(p.protect_inputs(std::slice::from_ref(&denied), &[], &AtomicUsize::new(1))),
        ErrorKind::Cancelled
    );
    p.commit(&stop).unwrap();
    assert!(set
        .protect_output(&denied, PublicationKind::Defaults)
        .is_ok());
}

#[test]
fn dynamic_input_identity_aliases_and_symlinked_trees_are_protected() {
    let f = Fixture::new();
    let set = SourceSet::new(vec![]).unwrap();
    let stop = AtomicUsize::new(0);
    let file = f.0.join("input.json");
    fs::write(&file, b"synthetic input").unwrap();
    let alias = f.0.join("alias.json");
    fs::hard_link(&file, &alias).unwrap();
    let tree = f.0.join("pack");
    fs::create_dir(&tree).unwrap();
    let link = f.0.join("pack-link");
    std::os::unix::fs::symlink(&tree, &link).unwrap();
    let mut p = set.begin(&stop).unwrap();
    p.protect_inputs(
        std::slice::from_ref(&file),
        std::slice::from_ref(&tree),
        &stop,
    )
    .unwrap();
    p.commit(&stop).unwrap();
    assert_eq!(
        error(set.protect_output(&alias, PublicationKind::Review)),
        ErrorKind::InvalidInput
    );
    assert_eq!(
        error(set.protect_output(&link.join("new"), PublicationKind::Defaults)),
        ErrorKind::InvalidInput
    );
    assert_eq!(fs::read(&file).unwrap(), b"synthetic input");
}

#[test]
fn drc_input_and_layout_cache_collisions_are_order_independent_and_atomic() {
    for layout_first in [true, false] {
        let f = Fixture::new();
        let source = f.deck(0);
        let input = f.0.join("missing.oas.floe/results.db");
        let stop = AtomicUsize::new(0);
        let set = SourceSet::new(vec![]).unwrap();
        let mut registration = set.begin(&stop).unwrap();
        if layout_first {
            registration.register(f.scope(), &source, &stop).unwrap();
            registration.commit(&stop).unwrap();
            let mut next = set.begin(&stop).unwrap();
            assert_eq!(
                error(next.protect_inputs(std::slice::from_ref(&input), &[], &stop)),
                ErrorKind::InvalidInput
            );
            next.commit(&stop).unwrap();
            assert!(
                set.protect_output(&input, PublicationKind::Defaults)
                    .is_ok(),
                "failed input batch was installed"
            );
            assert_eq!(set.snapshot().len(), 1);
        } else {
            registration
                .protect_inputs(std::slice::from_ref(&input), &[], &stop)
                .unwrap();
            registration.commit(&stop).unwrap();
            let mut next = set.begin(&stop).unwrap();
            assert_eq!(
                error(next.register(f.scope(), &source, &stop)),
                ErrorKind::InvalidInput
            );
            next.commit(&stop).unwrap();
            assert!(set.snapshot().is_empty());
            assert_eq!(
                error(set.protect_output(&input, PublicationKind::Defaults)),
                ErrorKind::InvalidInput
            );
        }
        assert!(!input.exists());
    }
}
