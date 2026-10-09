use super::*;
use crate::{
    drc::review::{
        managed,
        store::{Kind, Store},
    },
    managed::{Limits, Resources, Usage},
    registered::AccessScope,
    ErrorKind,
};
use std::{
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

static SERIAL: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    dir: PathBuf,
    path: PathBuf,
    scope: Arc<AccessScope>,
    stop: AtomicUsize,
}
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "floe-waive-apply-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        let dir = fs::canonicalize(dir).unwrap();
        let path = dir.join("synthetic.db.ice");
        fs::write(&path, crate::drc::tests::bytes(65)).unwrap();
        let scope = AccessScope::new(std::slice::from_ref(&dir)).unwrap();
        Self {
            dir,
            path,
            scope,
            stop: AtomicUsize::new(0),
        }
    }
    fn store(&self, kind: Kind) -> Arc<Store> {
        Store::open(
            Arc::clone(&self.scope),
            &self.path,
            "synthetic",
            kind,
            vec![],
            vec![],
            &self.stop,
        )
        .unwrap()
    }
    fn reader(&self) -> Database {
        Database::open_explicit(&self.path, None, &self.stop).unwrap()
    }
    fn edit(&self, s: &Arc<Store>, edits: &[(u64, u8)]) {
        s.snapshot(&self.stop)
            .unwrap()
            .prepare_waives(edits, &self.stop)
            .unwrap()
            .publish(&self.stop)
            .unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.dir).unwrap();
    }
}
fn kind<T>(r: Result<T>) -> ErrorKind {
    match r {
        Ok(_) => panic!("expected error"),
        Err(e) => e.kind,
    }
}
fn blocks(d: &Database) -> u64 {
    match &d.backend {
        Backend::Pack(p) => p.decoded_blocks,
        _ => unreachable!(),
    }
}
fn statuses(d: &mut Database, stop: &AtomicUsize) -> Vec<u8> {
    d.errors(1, 0, 65, stop)
        .unwrap()
        .hits
        .into_iter()
        .map(|h| h.status)
        .collect()
}

#[test]
fn published_waives_refresh_statuses_counts_and_filters_without_redecoding() {
    let f = Fixture::new();
    let s = f.store(Kind::Waives);
    let mut d = f.reader();
    let original = d.errors(1, 0, 65, &f.stop).unwrap();
    let warm = blocks(&d);
    assert_eq!(warm, 2);
    f.edit(&s, &[(0, 1), (64, 1), (1, 239)]);
    let a = s
        .snapshot(&f.stop)
        .unwrap()
        .apply_waives(&mut d, &f.stop)
        .unwrap();
    assert_eq!(
        a,
        crate::drc::review::store::AppliedWaives {
            sidecar: true,
            legacy_unverified: false,
            waived: 2
        }
    );
    assert!(d.has_waives());
    assert_eq!(d.waived_count(1).unwrap(), 2);
    assert_eq!(statuses(&mut d, &f.stop)[..3], [1, 239, 0]);
    let page = d
        .query(
            [0., -1., 1., 1.],
            None,
            Some(true),
            Cursor::default(),
            65,
            &f.stop,
        )
        .unwrap();
    assert_eq!(
        page.hits.iter().map(|h| h.local).collect::<Vec<_>>(),
        [0, 64]
    );
    let after = d.errors(1, 0, 65, &f.stop).unwrap();
    for (a, b) in original.hits.iter().zip(&after.hits) {
        assert_eq!(a.violation.kind, b.violation.kind);
        assert_eq!(a.violation.number, b.violation.number);
        assert_eq!(a.violation.bbox_um, b.violation.bbox_um);
        assert_eq!(
            a.violation.points_um(&f.stop).unwrap(),
            b.violation.points_um(&f.stop).unwrap()
        );
    }
    f.edit(&s, &[(0, 0), (63, 1)]);
    // The old descriptor is stale after rename; only the explicit refresh can
    // bypass this review check, not arbitrary geometry reads.
    assert!(d.unchanged().is_err());
    s.snapshot(&f.stop)
        .unwrap()
        .apply_waives(&mut d, &f.stop)
        .unwrap();
    assert_eq!(d.waived_count(1).unwrap(), 2);
    let values = statuses(&mut d, &f.stop);
    assert_eq!(
        (values[0], values[1], values[63], values[64]),
        (0, 239, 1, 1)
    );
    assert_eq!(blocks(&d), warm, "refresh dropped the geometry LRU");
    assert_eq!(fs::read(&f.path).unwrap(), crate::drc::tests::bytes(65));
}

#[test]
fn cancelled_wrong_kind_pack_or_ascii_never_install_a_snapshot() {
    let f = Fixture::new();
    let s = f.store(Kind::Waives);
    let mut d = f.reader();
    f.edit(&s, &[(0, 1)]);
    assert_eq!(
        kind(
            s.snapshot(&f.stop)
                .unwrap()
                .apply_waives(&mut d, &AtomicUsize::new(1))
        ),
        ErrorKind::Cancelled
    );
    assert!(!d.has_waives());
    assert_eq!(d.waived_count(1).unwrap(), 0);
    assert_eq!(
        kind(
            f.store(Kind::Notes)
                .snapshot(&f.stop)
                .unwrap()
                .apply_waives(&mut d, &f.stop)
        ),
        ErrorKind::InvalidInput
    );
    let other = Fixture::new();
    let mut different = other.reader();
    assert_eq!(
        kind(
            s.snapshot(&f.stop)
                .unwrap()
                .apply_waives(&mut different, &f.stop)
        ),
        ErrorKind::Cache
    );
    assert!(!different.has_waives());
    let ascii = f.dir.join("input.db");
    fs::write(
        &ascii,
        b"TOP 1000\nRULE\n1 1 1\nrule\np 1 4\n0 0\n1 0\n1 1\n0 1\n",
    )
    .unwrap();
    let mut ascii = Database::open_explicit(&ascii, None, &f.stop).unwrap();
    assert_eq!(
        kind(
            s.snapshot(&f.stop)
                .unwrap()
                .apply_waives(&mut ascii, &f.stop)
        ),
        ErrorKind::InvalidInput
    );
    let copy = f.dir.join("copy.ice");
    fs::copy(&f.path, &copy).unwrap();
    fs::rename(copy, &f.path).unwrap();
    assert_eq!(kind(s.snapshot(&f.stop)), ErrorKind::Cache);
    assert!(!d.has_waives());
}

#[test]
fn refresh_checks_the_captured_revision_and_recounts_legacy_counters() {
    let f = Fixture::new();
    let s = f.store(Kind::Waives);
    let mut d = f.reader();
    let absent = s.snapshot(&f.stop).unwrap();
    f.edit(&s, &[(1, 1)]);
    assert_eq!(kind(absent.apply_waives(&mut d, &f.stop)), ErrorKind::Busy);
    let stale = s.snapshot(&f.stop).unwrap();
    f.edit(&s, &[(2, 1)]);
    assert_eq!(kind(stale.apply_waives(&mut d, &f.stop)), ErrorKind::Busy);
    assert!(!d.has_waives());
    let mut data = fs::read(s.target()).unwrap();
    data[40 + 65..].fill(0);
    let replacement = f.dir.join("legacy");
    fs::write(&replacement, &data).unwrap();
    fs::rename(replacement, s.target()).unwrap();
    let a = s
        .snapshot(&f.stop)
        .unwrap()
        .apply_waives(&mut d, &f.stop)
        .unwrap();
    assert!(a.legacy_unverified);
    assert_eq!(a.waived, 2);
    assert_eq!(
        d.waived_count(1).unwrap(),
        2,
        "used stale legacy counter bytes"
    );
    assert_eq!(
        fs::read(s.target()).unwrap(),
        data,
        "refresh rewrote the legacy file"
    );
    let stale = s.snapshot(&f.stop).unwrap();
    fs::remove_file(s.target()).unwrap();
    assert_eq!(kind(stale.apply_waives(&mut d, &f.stop)), ErrorKind::Busy);
    // Only a newly requested, explicitly absent snapshot restores the embedded
    // status stream. A missing sidecar does not trigger a background fallback.
    let a = s
        .snapshot(&f.stop)
        .unwrap()
        .apply_waives(&mut d, &f.stop)
        .unwrap();
    assert!(!a.sidecar);
    assert!(!d.has_waives());
    assert_eq!(d.waived_count(1).unwrap(), 0);
    assert!(statuses(&mut d, &f.stop).iter().all(|&s| s == 0));
}

#[test]
fn managed_snapshot_keeps_admission_and_retirement_prevents_install() {
    let f = Fixture::new();
    let mut d = f.reader();
    let resources = Resources::new(Limits::default()).unwrap();
    let open = || {
        managed::ManagedStore::open(
            &resources,
            managed::Registration {
                scope: Arc::clone(&f.scope),
                pack: f.path.clone(),
                reviewer: "synthetic".into(),
                kind: Kind::Waives,
                protected_files: vec![],
                protected_trees: vec![],
            },
            &f.stop,
        )
        .unwrap()
    };
    let s = open();
    let snapshot = s.snapshot(Arc::new(AtomicUsize::new(0))).unwrap();
    drop(s);
    assert_eq!(resources.usage().cpu_slots, 1);
    assert!(resources.index([f.path.clone()], 1).is_err());
    snapshot.apply_waives(&mut d, &f.stop).unwrap();
    assert_eq!(resources.usage(), Usage::default());
    let s = open();
    let snapshot = s.snapshot(Arc::new(AtomicUsize::new(0))).unwrap();
    s.request_stop();
    assert_eq!(
        kind(snapshot.apply_waives(&mut d, &f.stop)),
        ErrorKind::Cancelled
    );
    assert!(s.is_idle());
    drop(s);
    assert_eq!(resources.usage(), Usage::default());
    assert_eq!(
        fs::read_dir(&f.dir).unwrap().count(),
        1,
        "read refresh created a sidecar or lock"
    );
}
