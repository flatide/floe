use super::*;
use crate::registered::AccessScope;
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    path: PathBuf,
    source: Arc<RegisteredSource>,
    store: super::super::super::Store,
}
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "floe-inventory-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        let source_path = path.join("synthetic.oas");
        fs::write(
            &source_path,
            floe_oasis::write::write_cell("TOP", 1000., &mut [], &mut []).unwrap(),
        )
        .unwrap();
        let scope = AccessScope::new(std::slice::from_ref(&path)).unwrap();
        let source = RegisteredSource::register(scope, &source_path, &AtomicUsize::new(0)).unwrap();
        let store = super::super::super::Store::new(&source_path).unwrap();
        Self {
            path,
            source,
            store,
        }
    }
    fn sealed(&self, digit: char, version: u32, owner: Option<SetOwner>) -> Vec<u8> {
        fs::create_dir_all(self.store.path()).unwrap();
        let revision = digit.to_string().repeat(32);
        let path = self.store.path().join(&revision);
        fs::create_dir(&path).unwrap();
        for name in ["meta.json", "design.ovm", "design.ovp"] {
            fs::write(path.join(name), b"synthetic, not a VFS cache").unwrap();
        }
        let r = Record {
            version,
            source: self.store.source.clone(),
            revision,
            source_stamp: Stamp::source(self.source.path()).unwrap(),
            files: files(&path, false).unwrap(),
            owner,
        };
        let bytes = serde_json::to_vec(&r).unwrap();
        create_record(&path.join("revision.json"), &bytes).unwrap();
        bytes
    }
    fn inspect(&self) -> Inventory {
        inspect(&self.source, &AtomicUsize::new(0)).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.path).unwrap();
    }
}

#[test]
fn inspection_creates_nothing_and_keeps_legacy_and_unsealed_protected() {
    let f = Fixture::new();
    assert_eq!(f.inspect().logical_bytes, "0");
    assert_eq!(fs::read_dir(&f.path).unwrap().count(), 1);
    let bytes = f.sealed('a', 1, None);
    fs::write(f.store.path().join("current.json"), &bytes).unwrap();
    let unsealed = f.store.path().join("b".repeat(32));
    fs::create_dir(&unsealed).unwrap();
    fs::write(unsealed.join("design.ovp"), b"partial payload").unwrap();
    let inventory = f.inspect();
    assert!(!inventory.partial);
    assert!(inventory.rows[0].current);
    assert_eq!(inventory.rows[0].readers, "legacy_untracked");
    assert_eq!(inventory.rows[1].seal, "unsealed");
    let expected =
        bytes.len() * 2 + 3 * b"synthetic, not a VFS cache".len() + b"partial payload".len();
    assert_eq!(inventory.logical_bytes, expected.to_string());
    assert_eq!(
        fs::read(f.store.path().join("current.json")).unwrap(),
        bytes
    );
    assert_eq!(
        fs::read_dir(&f.path).unwrap().count(),
        2,
        "no index lock created"
    );
    assert!(inspect(&f.source, &AtomicUsize::new(1)).is_err());
}

#[test]
fn recovery_ids_are_bounded_unverified_observations_without_path_traversal() {
    let f = Fixture::new();
    let set = Store::new(f.source.path()).unwrap();
    fs::create_dir(set.path()).unwrap();
    for i in 0..=MAX_ROWS {
        fs::write(
            set.path().join(format!(".reclaim-{i:032x}.json")),
            b"unverified, not a valid journal",
        )
        .unwrap();
    }
    std::os::unix::fs::symlink(
        f.source.path(),
        set.path().join(format!(".reclaim-{}.json", "f".repeat(32))),
    )
    .unwrap();
    let result = f.inspect();
    assert!(result.partial);
    assert_eq!(result.recoveries.len(), MAX_ROWS);
    assert_eq!(result.recoveries[0], "0".repeat(32));
    assert!(!result.recoveries.contains(&"f".repeat(32)));
    assert!(result.rows.is_empty());
    assert!(!serde_json::to_string(&result)
        .unwrap()
        .contains(f.path.to_str().unwrap()));
    assert_eq!(fs::read_dir(set.path()).unwrap().count(), MAX_ROWS + 2);
}

#[test]
fn snapshot_clones_and_independent_processes_keep_shared_leases() {
    let f = Fixture::new();
    let bytes = f.sealed('a', 2, None);
    fs::write(f.store.path().join("current.json"), bytes).unwrap();
    assert_eq!(f.inspect().rows[0].readers, "idle_at_scan");
    let snapshot = f.store.pin().unwrap().unwrap();
    let clone = snapshot.clone();
    drop(snapshot);
    assert_eq!(f.inspect().rows[0].readers, "in_use");
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    let status = command
        .args([
            "--exact",
            "cache::revision::set::inventory::tests::lease_child",
            "--nocapture",
        ])
        .env("FLOE_TEST_REVISION_LEASE_PATH", clone.directory())
        .status()
        .unwrap();
    assert!(status.success());
    clone.validate().unwrap();
    drop(clone);
    assert_eq!(f.inspect().rows[0].readers, "idle_at_scan");
    let exclusive = directory(&f.store.path().join("a".repeat(32))).unwrap();
    exclusive.try_lock().unwrap();
    assert_eq!(f.store.pin().unwrap_err().kind, ErrorKind::Busy);
    drop(exclusive);
    assert!(f.store.pin().unwrap().is_some());
}

#[test]
fn lease_child() {
    let Some(path) = std::env::var_os("FLOE_TEST_REVISION_LEASE_PATH") else {
        return;
    };
    let file = directory(Path::new(&path)).unwrap();
    assert!(matches!(
        file.try_lock(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    let shared = reader_lease(file).unwrap();
    assert!(shared.metadata().unwrap().is_dir());
}

#[test]
fn inventory_never_traverses_unknown_or_manifest_supplied_paths() {
    let f = Fixture::new();
    f.sealed(
        'a',
        2,
        Some(SetOwner {
            source: PathBuf::from("/outside/private.jb"),
            revision: "f".repeat(32),
        }),
    );
    let secret = f.path.join("outside");
    fs::create_dir(&secret).unwrap();
    fs::write(secret.join("secret"), b"not inventory data").unwrap();
    std::os::unix::fs::symlink(&secret, f.store.path().join("b".repeat(32))).unwrap();
    std::os::unix::fs::symlink(&secret, f.store.path().join("a".repeat(32)).join("unknown"))
        .unwrap();
    let inv = f.inspect();
    assert!(inv.partial);
    assert_eq!(inv.rows.len(), 1);
    assert_eq!(inv.rows[0].owner, "other_dataset");
    assert!(inv.rows[0].set_revision.is_none());
    assert!(inv.rows[0].extra_entries);
    let wire = serde_json::to_string(&inv).unwrap();
    assert!(!wire.contains("private.jb") && !wire.contains(f.path.to_str().unwrap()));
    assert_eq!(
        fs::read(secret.join("secret")).unwrap(),
        b"not inventory data"
    );
}

#[test]
fn accounting_is_bounded_and_bad_current_is_never_treated_as_absent() {
    let f = Fixture::new();
    f.sealed('a', 2, None);
    fs::write(f.store.path().join("current.json"), b"invalid").unwrap();
    assert!(f.inspect().rows[0].current_unknown);
    for i in 1..=MAX_ROWS {
        fs::create_dir(f.store.path().join(format!("{i:032x}"))).unwrap();
    }
    let inv = f.inspect();
    assert_eq!(inv.rows.len(), MAX_ROWS);
    assert!(inv.partial);
}

#[test]
fn set_ownership_rejects_cross_set_and_old_reader_references() {
    let f = Fixture::new();
    let bytes = f.sealed(
        'a',
        2,
        Some(SetOwner {
            source: f.source.path().into(),
            revision: "c".repeat(32),
        }),
    );
    let record: Record = serde_json::from_slice(&bytes).unwrap();
    let mut manifest = Manifest {
        version: 2,
        source: f.source.path().into(),
        source_stamp: record.source_stamp.clone(),
        revision: "c".repeat(32),
        levels: None,
        members: vec![Member {
            source: f.source.path().into(),
            revision: "a".repeat(32),
        }],
    };
    assert!(validate_owner(&manifest, &record).is_ok());
    manifest.revision = "d".repeat(32);
    assert!(validate_owner(&manifest, &record).is_err());
    manifest.revision = "c".repeat(32);
    manifest.version = 1;
    assert!(validate_owner(&manifest, &record).is_err());
    let mut legacy = record.clone();
    legacy.version = 1;
    legacy.owner = None;
    assert!(validate_owner(&manifest, &legacy).is_ok());
    manifest.version = 2;
    assert!(validate_owner(&manifest, &legacy).is_err());
    let mut native_pinned = record.clone();
    native_pinned.version = 3;
    assert!(validate_owner(&manifest, &native_pinned).is_err());
    manifest.version = 3;
    assert!(validate_owner(&manifest, &record).is_err());
    assert!(validate_owner(&manifest, &native_pinned).is_ok());
    manifest.version = 2;
    let set = Store::new(f.source.path()).unwrap();
    fs::create_dir_all(set.path().join(&manifest.revision)).unwrap();
    let bytes = serde_json::to_vec(&manifest).unwrap();
    fs::write(
        set.path().join(&manifest.revision).join("revision.json"),
        &bytes,
    )
    .unwrap();
    fs::write(set.path().join("current.json"), &bytes).unwrap();
    let pin = set
        .pin(
            &[f.source.path().into()].into(),
            &None,
            &AtomicUsize::new(0),
        )
        .unwrap()
        .unwrap();
    let inventory = f.inspect();
    assert_eq!(inventory.rows[0].readers, "in_use");
    assert_eq!(inventory.rows[1].readers, "in_use");
    assert_eq!(inventory.rows[1].set_revision.as_deref(), Some(pin.id()));
    drop(pin);
    assert!(f.inspect().rows.iter().all(|r| r.readers == "idle_at_scan"));
}

#[test]
fn seal_budget_marks_unchecked_rows_partial_without_skipping_usage_bytes() {
    let f = Fixture::new();
    f.sealed('a', 2, None);
    let expected = f.inspect().logical_bytes;
    let mut scan = Scan {
        result: Inventory {
            logical_bytes: "0".into(),
            rows: Vec::new(),
            recoveries: Vec::new(),
            stores_scanned: 0,
            unknown_entries: 0,
            unavailable_entries: 0,
            partial: false,
        },
        bytes: 0,
        remaining: MAX_ENTRIES,
        seal_bytes_left: 1,
    };
    scan.store(
        &f.store,
        false,
        1,
        &f.source,
        &[f.source.path().into()].into(),
        &AtomicUsize::new(0),
    )
    .unwrap();
    assert!(scan.result.partial);
    assert_eq!(scan.bytes.to_string(), expected);
    assert_eq!(scan.result.rows[0].seal, "not_checked_limit");
    assert_eq!(scan.result.rows[0].readers, "not_checked");
}
