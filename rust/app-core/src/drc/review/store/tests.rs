use super::*;
use std::{
    fs::OpenOptions,
    io::Read,
    os::unix::fs::{symlink, PermissionsExt},
    sync::atomic::{AtomicU64, Ordering},
};
static SERIAL: AtomicU64 = AtomicU64::new(0);

#[test]
fn dynamic_input_protection_rechecks_old_review_drafts_without_granting_writes() {
    for review_kind in [Kind::Notes, Kind::Waives] {
        for lock_target in [false, true] {
            let f = Fixture::new();
            let store = f.store(review_kind);
            let draft = if review_kind == Kind::Notes {
                f.note(&store, "saved")
            } else {
                store
                    .snapshot(&f.stop)
                    .unwrap()
                    .prepare_waives(&[(0, 1)], &f.stop)
                    .unwrap()
            };
            let target = if lock_target {
                store.lock_path()
            } else {
                store.target().to_owned()
            };
            let mut p = store.sources.begin(&f.stop).unwrap();
            p.protect_inputs(std::slice::from_ref(&target), &[], &f.stop)
                .unwrap();
            p.commit(&f.stop).unwrap();
            assert_eq!(kind(draft.publish(&f.stop)), ErrorKind::InvalidInput);
            assert!(!store.target().exists() && !store.lock_path().exists());
            f.clean();
        }
        let f = Fixture::new();
        let store = f.store(review_kind);
        let mut p = store.sources.begin(&f.stop).unwrap();
        p.protect_review_targets(&[store.target().to_owned(), store.lock_path()], &f.stop)
            .unwrap();
        p.commit(&f.stop).unwrap();
        // Default-publication protection is not a veto of this existing,
        // separately authorized review writer's exact target.
        if review_kind == Kind::Notes {
            f.note(&store, "still authorized").publish(&f.stop).unwrap();
        } else {
            store
                .snapshot(&f.stop)
                .unwrap()
                .prepare_waives(&[(0, 1)], &f.stop)
                .unwrap()
                .publish(&f.stop)
                .unwrap();
        }
        let bytes = fs::read(store.target()).unwrap();
        let mut p = store.sources.begin(&f.stop).unwrap();
        p.protect_inputs(&[store.target().to_owned()], &[], &f.stop)
            .unwrap();
        p.commit(&f.stop).unwrap();
        // A deny-publication input registration does not revoke read access.
        let reader = Store::open_readonly_catalog(
            f.scope.clone(),
            &f.pack,
            "reviewer",
            review_kind,
            vec![],
            vec![],
            Arc::clone(&store.sources),
            store.target(),
            &f.stop,
        )
        .unwrap();
        assert!(reader.snapshot(&f.stop).is_ok());
        assert_eq!(fs::read(store.target()).unwrap(), bytes);
    }
}

#[test]
fn selected_readonly_review_cannot_create_drafts_or_publish() {
    for review_kind in [Kind::Notes, Kind::Waives] {
        let f = Fixture::new();
        let writer = f.store(review_kind);
        if review_kind == Kind::Notes {
            f.note(&writer, "saved legacy note")
                .publish(&f.stop)
                .unwrap();
        } else {
            writer
                .snapshot(&f.stop)
                .unwrap()
                .prepare_waives(&[(0, 1)], &f.stop)
                .unwrap()
                .publish(&f.stop)
                .unwrap();
        }
        let target = read_paths(&f.pack, "reader", review_kind).unwrap()[1].clone();
        let bytes = fs::read(writer.target()).unwrap();
        fs::write(&target, &bytes).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o444)).unwrap();
        let reader = Store::open_readonly_catalog(
            f.scope.clone(),
            &f.pack,
            "reader",
            review_kind,
            vec![],
            vec![],
            crate::registered::SourceSet::new(vec![]).unwrap(),
            &target,
            &f.stop,
        )
        .unwrap();
        let snap = || reader.snapshot(&f.stop).unwrap();
        assert!(snap().legacy_unverified());
        if review_kind == Kind::Notes {
            assert_eq!(snap().notes().unwrap().get(0), Some("saved legacy note"));
            snap().check_note_display(&f.stop).unwrap();
            assert_eq!(
                kind(snap().prepare_note(&[0], "denied", &f.stop)),
                ErrorKind::Unsupported
            );
            assert_eq!(
                kind(snap().prepare_notes_import("", &f.stop)),
                ErrorKind::Unsupported
            );
        } else {
            assert_eq!(snap().selected_statuses(&[0], &f.stop).unwrap(), vec![1]);
            assert_eq!(
                kind(snap().prepare_waives(&[(0, 0)], &f.stop)),
                ErrorKind::Unsupported
            );
            assert_eq!(
                kind(snap().prepare_waives_import(File::open(&target).unwrap(), &f.stop)),
                ErrorKind::Unsupported
            );
        }
        // Defense in depth even if a future internal caller constructs a draft.
        let forged = Draft {
            snapshot: snap(),
            change: Change::Notes(vec![]),
            expires: Instant::now() + TTL,
            accept_legacy: true,
            imported: false,
        };
        assert_eq!(kind(forged.publish(&f.stop)), ErrorKind::Unsupported);
        assert_eq!(fs::read(&target).unwrap(), bytes);
        assert!(!reader.lock_path().exists());
        assert!(Store::open_readonly_catalog(
            f.scope.clone(),
            &f.pack,
            "reader",
            review_kind,
            vec![],
            vec![],
            crate::registered::SourceSet::new(vec![]).unwrap(),
            &f.root.join("arbitrary"),
            &f.stop,
        )
        .is_err());
        fs::remove_file(target).unwrap();
        f.clean();
    }
}

#[test]
fn dynamic_sources_fence_old_note_and_waive_drafts_and_staging() {
    for review_kind in [Kind::Notes, Kind::Waives] {
        for lock_target in [false, true] {
            let f = Fixture::new();
            let set = crate::registered::SourceSet::new(vec![]).unwrap();
            let store = Store::open_catalog(
                Arc::clone(&f.scope),
                &f.pack,
                "dynamic",
                review_kind,
                vec![],
                vec![],
                Arc::clone(&set),
                &f.stop,
            )
            .unwrap();
            let draft = if review_kind == Kind::Notes {
                f.note(&store, "saved")
            } else {
                store
                    .snapshot(&f.stop)
                    .unwrap()
                    .prepare_waives(&[(0, 1)], &f.stop)
                    .unwrap()
            };
            let [target, lock] = paths(&f.pack, "dynamic", review_kind).unwrap();
            let protected = if lock_target { &lock } else { &target };
            let deck = f.dir.join("additional.jb");
            fs::write(
                &deck,
                format!(
                    "CHIP A\n$ (1,A,TC='{}')\n",
                    protected.file_name().unwrap().to_str().unwrap()
                ),
            )
            .unwrap();
            let mut pending = set.begin(&f.stop).unwrap();
            pending
                .register(Arc::clone(&f.scope), &deck, &f.stop)
                .unwrap();
            pending.commit(&f.stop).unwrap();
            assert_eq!(kind(draft.publish(&f.stop)), ErrorKind::InvalidInput);
            assert!(!target.exists() && !lock.exists());
            f.clean();
        }
    }
    let f = Fixture::new();
    let store = f.store(Kind::Notes);
    let pending = store.sources.begin(&f.stop).unwrap();
    assert_eq!(
        kind(f.note(&store, "busy").publish(&f.stop)),
        ErrorKind::Busy
    );
    assert!(!store.target().exists() && !store.lock_path().exists());
    drop(pending);
    f.note(&store, "saved")
        .publish_using(
            &f.stop,
            || {
                assert_eq!(kind(store.sources.begin(&f.stop)), ErrorKind::Busy);
                assert!(store.sources.snapshot().is_empty());
                Ok(())
            },
            File::sync_all,
        )
        .unwrap();
    drop(store.sources.begin(&f.stop).unwrap());
    f.clean();
}
#[path = "transfer_tests.rs"]
mod transfer;
struct Fixture {
    root: PathBuf,
    dir: PathBuf,
    pack: PathBuf,
    bytes: Vec<u8>,
    scope: Arc<AccessScope>,
    stop: AtomicUsize,
}
impl Fixture {
    fn new() -> Self {
        Self::with_count(65)
    }
    fn with_count(count: u64) -> Self {
        let root = std::env::temp_dir().join(format!(
            "floe-review-store-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let dir = root.join("inputs");
        fs::create_dir(&dir).unwrap();
        let pack = dir.join("한 글.db.ice");
        let bytes = crate::drc::tests::bytes(count);
        fs::write(&pack, &bytes).unwrap();
        let scope = AccessScope::new(std::slice::from_ref(&root)).unwrap();
        Self {
            root,
            dir,
            pack,
            bytes,
            scope,
            stop: AtomicUsize::new(0),
        }
    }
    fn store(&self, kind: Kind) -> Arc<Store> {
        self.reviewer(kind, "reviewer")
    }
    fn reviewer(&self, kind: Kind, tag: &str) -> Arc<Store> {
        Store::open(
            Arc::clone(&self.scope),
            &self.pack,
            tag,
            kind,
            vec![],
            vec![],
            &self.stop,
        )
        .unwrap()
    }
    fn note(&self, s: &Arc<Store>, text: &str) -> Draft {
        s.snapshot(&self.stop)
            .unwrap()
            .prepare_note(&[0, 1], text, &self.stop)
            .unwrap()
    }
    fn clean(&self) {
        assert_eq!(fs::read(&self.pack).unwrap(), self.bytes);
        assert!(!fs::read_dir(&self.dir).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".floe-review-")));
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}
fn kind<T>(r: Result<T>) -> ErrorKind {
    match r {
        Err(e) => e.kind,
        Ok(_) => panic!("expected failure"),
    }
}

#[test]
fn note_display_uses_the_captured_model_and_rejects_external_changes() {
    let f = Fixture::new();
    let s = f.store(Kind::Notes);
    let missing = s.snapshot(&f.stop).unwrap();
    missing.check_note_display(&f.stop).unwrap();
    assert!(!s.target().exists() && !s.lock_path().exists());
    f.note(&s, "saved note").publish(&f.stop).unwrap();
    assert_eq!(kind(missing.check_note_display(&f.stop)), ErrorKind::Busy);
    let captured = s.snapshot(&f.stop).unwrap();
    let bytes = fs::read(s.target()).unwrap();
    for _ in 0..3 {
        captured.check_note_display(&f.stop).unwrap();
        assert_eq!(captured.notes().unwrap().get(0), Some("saved note"));
    }
    assert_eq!(
        kind(captured.check_note_display(&AtomicUsize::new(1))),
        ErrorKind::Cancelled
    );
    fs::write(s.target(), &bytes).unwrap(); // even identical bytes are a new expected version
    assert_eq!(kind(captured.check_note_display(&f.stop)), ErrorKind::Busy);
    let captured = s.snapshot(&f.stop).unwrap();
    let replacement = f.dir.join("replacement.fe");
    fs::write(&replacement, &bytes).unwrap();
    fs::rename(replacement, s.target()).unwrap();
    assert_eq!(kind(captured.check_note_display(&f.stop)), ErrorKind::Busy);
    let captured = s.snapshot(&f.stop).unwrap();
    fs::remove_file(s.target()).unwrap();
    assert_eq!(kind(captured.check_note_display(&f.stop)), ErrorKind::Busy);
    assert_eq!(captured.notes().unwrap().get(0), Some("saved note"));
    assert_eq!(
        kind(
            f.store(Kind::Waives)
                .snapshot(&f.stop)
                .unwrap()
                .check_note_display(&f.stop)
        ),
        ErrorKind::InvalidInput
    );
    f.clean();
}

#[test]
fn committed_waive_proof_rejects_replacement_mutation_and_other_targets() {
    let f = Fixture::new();
    let s = f.store(Kind::Waives);
    let publish = || {
        s.snapshot(&f.stop)
            .unwrap()
            .prepare_waives(&[(0, 1)], &f.stop)
            .unwrap()
            .publish(&f.stop)
            .unwrap()
    };
    let first = publish();
    assert!(first.file.is_some());
    s.snapshot(&f.stop)
        .unwrap()
        .verify_published(&first)
        .unwrap();
    let second = publish();
    assert_eq!(
        kind(s.snapshot(&f.stop).unwrap().verify_published(&first)),
        ErrorKind::Busy
    );
    s.snapshot(&f.stop)
        .unwrap()
        .verify_published(&second)
        .unwrap();
    let other = f.reviewer(Kind::Waives, "other");
    let copied = other
        .snapshot(&f.stop)
        .unwrap()
        .prepare_waives(&[(0, 1)], &f.stop)
        .unwrap()
        .publish(&f.stop)
        .unwrap();
    assert_eq!(
        kind(s.snapshot(&f.stop).unwrap().verify_published(&copied)),
        ErrorKind::Busy
    );
    let notes = f.store(Kind::Notes);
    let note = f.note(&notes, "unrelated").publish(&f.stop).unwrap();
    assert!(note.file.is_none());
    assert_eq!(
        kind(s.snapshot(&f.stop).unwrap().verify_published(&note)),
        ErrorKind::Busy
    );
    let mut bytes = fs::read(s.target()).unwrap();
    *bytes.last_mut().unwrap() = 239;
    fs::write(s.target(), bytes).unwrap();
    assert_eq!(
        kind(s.snapshot(&f.stop).unwrap().verify_published(&second)),
        ErrorKind::Busy
    );
    f.clean();
}

#[test]
fn waive_install_last_check_rejects_cancel_and_input_mutation() {
    for change in ["cancel", "pack", "sidecar"] {
        let f = Fixture::new();
        let s = f.store(Kind::Waives);
        s.snapshot(&f.stop)
            .unwrap()
            .prepare_waives(&[(0, 1)], &f.stop)
            .unwrap()
            .publish(&f.stop)
            .unwrap();
        let mut reader = crate::drc::Database::open_explicit(&f.pack, None, &f.stop).unwrap();
        let snapshot = s.snapshot(&f.stop).unwrap();
        let result = snapshot.apply_waives_using(&mut reader, &f.stop, || {
            match change {
                "cancel" => f.stop.store(1, Ordering::Relaxed),
                "pack" => fs::write(&f.pack, &f.bytes).unwrap(),
                _ => {
                    let bytes = fs::read(s.target()).unwrap();
                    fs::write(s.target(), bytes).unwrap();
                }
            }
            Ok(())
        });
        assert!(result.is_err(), "installed after {change}");
        assert!(!reader.has_waives(), "partial installation after {change}");
    }
}

#[test]
fn reader_and_review_bind_the_same_open_pack_not_equal_legacy_headers() {
    use crate::drc::Database;
    let f = Fixture::new();
    let store = f.store(Kind::Notes);
    let identity = store.identity();
    let reader = Database::packed(Pack::open(&f.pack, &f.stop).unwrap());
    reader.validate_review_identity(&identity).unwrap();
    assert_eq!(
        reader
            .review_targets(&identity, &[(1, 0), (1, 64), (1, 0)], &f.stop)
            .unwrap(),
        [0, 64, 0]
    );
    for refs in [
        vec![],
        vec![(0, 0)],
        vec![(1, 65)],
        vec![(3, 0)],
        vec![(1, 0); 5001],
    ] {
        assert!(reader.review_targets(&identity, &refs, &f.stop).is_err());
    }
    assert_eq!(
        kind(reader.review_targets(&identity, &[(1, 0)], &AtomicUsize::new(1))),
        ErrorKind::Cancelled
    );
    let ascii_path = f.dir.join("unpacked.db");
    fs::write(
        &ascii_path,
        b"TOP 1000\nRULE\n1 1 1\nrule text\np 1 4\n0 0\n1 0\n1 1\n0 1\n",
    )
    .unwrap();
    let ascii = Database::open_explicit(&ascii_path, None, &f.stop).unwrap();
    assert!(ascii.validate_review_identity(&identity).is_err());

    let copy = f.dir.join("identical-copy.ice");
    fs::write(&copy, &f.bytes).unwrap();
    let other = Database::packed(Pack::open(&copy, &f.stop).unwrap());
    assert_eq!(
        kind(other.validate_review_identity(&identity)),
        ErrorKind::Cache
    );
    // Reader predates an external replace; the writer opens the new inode.
    fs::rename(&copy, &f.pack).unwrap();
    let replacement = f.store(Kind::Notes);
    assert!(reader
        .validate_review_identity(&replacement.identity())
        .is_err());
    let reopened = Database::packed(Pack::open(&f.pack, &f.stop).unwrap());
    reopened
        .validate_review_identity(&replacement.identity())
        .unwrap();
    assert!(reopened.validate_review_identity(&identity).is_err());
    // Even touching the same inode invalidates its conservative run identity.
    fs::write(&f.pack, &f.bytes).unwrap();
    assert!(reopened
        .validate_review_identity(&replacement.identity())
        .is_err());
    f.clean();
}

#[test]
fn selected_waives_preserve_order_duplicates_and_reserved_bytes() {
    let f = Fixture::new();
    let store = f.store(Kind::Waives);
    let snapshot = store.snapshot(&f.stop).unwrap();
    let ids = [64, 2, 0, 1, 2, 3, 63];
    assert_eq!(snapshot.selected_statuses(&ids, &f.stop).unwrap(), [0; 7]);
    assert!(snapshot.selected_statuses(&[], &f.stop).unwrap().is_empty());
    assert!(snapshot.selected_statuses(&[65], &f.stop).is_err());
    assert_eq!(
        kind(snapshot.selected_statuses(&vec![0; EDIT_ITEMS + 1], &f.stop)),
        ErrorKind::Incomplete
    );
    assert!(f
        .store(Kind::Notes)
        .snapshot(&f.stop)
        .unwrap()
        .selected_statuses(&ids, &f.stop)
        .is_err());
    store
        .snapshot(&f.stop)
        .unwrap()
        .prepare_waives(&[(0, 1), (1, 2), (2, 255), (64, 1)], &f.stop)
        .unwrap()
        .publish(&f.stop)
        .unwrap();
    // A missing-file snapshot cannot silently start reading a newly created file.
    assert_eq!(
        kind(snapshot.selected_statuses(&ids, &f.stop)),
        ErrorKind::Busy
    );
    let current = store.snapshot(&f.stop).unwrap();
    assert_eq!(
        current.selected_statuses(&ids, &f.stop).unwrap(),
        [1, 255, 1, 2, 255, 0, 0]
    );
    assert_eq!(
        kind(current.selected_statuses(&ids, &AtomicUsize::new(1))),
        ErrorKind::Cancelled
    );
    current
        .store
        .snapshot(&f.stop)
        .unwrap()
        .prepare_waives(&[(2, 0)], &f.stop)
        .unwrap()
        .publish(&f.stop)
        .unwrap();
    // Atomic replacement does not keep serving the open old inode as current.
    assert_eq!(
        kind(current.selected_statuses(&ids, &f.stop)),
        ErrorKind::Busy
    );
    f.clean();
}

#[test]
fn selected_status_reads_coalesce_without_scanning_sparse_gaps() {
    use std::os::unix::fs::FileExt;
    let f = Fixture::new();
    let path = f.dir.join("status.bin");
    let bytes: Vec<_> = (0..6000).map(|i| (i % 256) as u8).collect();
    fs::write(&path, &bytes).unwrap();
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    let ids: Vec<_> = (0..EDIT_ITEMS as u64)
        .rev()
        .map(|n| n * 47 % 6000)
        .collect();
    let result = super::super::selected_statuses(&file, 0, 6000, &ids, &f.stop).unwrap();
    assert_eq!(
        result,
        ids.iter().map(|&n| bytes[n as usize]).collect::<Vec<_>>()
    );
    let far = 1u64 << 34;
    file.write_all_at(&[253], far).unwrap(); // sparse file, not a 16 GiB allocation
    assert_eq!(
        super::super::selected_statuses(&file, 0, far + 1, &[far, 0, far], &f.stop).unwrap(),
        [253, 0, 253]
    );
    assert!(super::super::selected_statuses(&file, u64::MAX, 2, &[1], &f.stop).is_err());
    assert!(super::super::selected_statuses(&file, far + 1, 2, &[0], &f.stop).is_err());
}

#[test]
fn registration_snapshot_and_preparation_do_not_write() {
    let f = Fixture::new();
    let w = f.store(Kind::Waives);
    let n = f.store(Kind::Notes);
    assert_eq!(w.target().file_name().unwrap(), ".한 글.db.waive.reviewer");
    assert_eq!(
        n.target().file_name().unwrap(),
        ".한 글.db.notes.reviewer.fe"
    );
    let s = w.snapshot(&f.stop).unwrap();
    assert!(!s.exists());
    assert_eq!(s.waives().unwrap().per_rule, [0, 0, 0]);
    s.prepare_waives(&[(0, 1)], &f.stop).unwrap();
    f.note(&n, "한글\n메모");
    assert_eq!(fs::read_dir(&f.dir).unwrap().count(), 1);
    f.clean();
}

#[test]
fn guarded_review_cannot_replace_a_registered_source_or_its_cache() {
    let f = Fixture::new();
    let [target, lock] = paths(&f.pack, "protected", Kind::Notes).unwrap();
    fs::write(&target, b"protected registered input").unwrap();
    let deck = f.dir.join("owner.jb");
    fs::write(
        &deck,
        format!(
            "CHIP A\n$ (1,PATTERN,TC='{}',AD=0.001,LY={{1}},DT={{0}},UX=1,UY=1)\nROWS 0/0\n",
            target.file_name().unwrap().to_str().unwrap()
        ),
    )
    .unwrap();
    let source = RegisteredSource::register(Arc::clone(&f.scope), &deck, &f.stop).unwrap();
    let open = |sources| {
        Store::open_guarded(
            Arc::clone(&f.scope),
            &f.pack,
            "protected",
            Kind::Notes,
            vec![],
            vec![],
            sources,
            &f.stop,
        )
    };
    assert!(open(vec![source]).is_err());
    assert_eq!(fs::read(&target).unwrap(), b"protected registered input");
    assert!(!lock.exists());
    // Parent of the pack is a registered layout's cache. Neither a review
    // target nor its lock may be introduced inside that protected tree.
    let layout = f.root.join("owner.oas");
    fs::write(&layout, b"synthetic registration").unwrap();
    let cache = crate::cache::cache_path(&layout).unwrap();
    fs::create_dir(&cache).unwrap();
    let packed = cache.join("nested.ice");
    fs::write(&packed, &f.bytes).unwrap();
    let owner = f.root.join("cache-owner.jb");
    fs::write(
        &owner,
        "CHIP A\n$ (1,PATTERN,TC='owner.oas',AD=0.001,LY={1},DT={0},UX=1,UY=1)\nROWS 0/0\n",
    )
    .unwrap();
    let source = RegisteredSource::register(Arc::clone(&f.scope), &owner, &f.stop).unwrap();
    assert!(Store::open_guarded(
        Arc::clone(&f.scope),
        &packed,
        "protected",
        Kind::Notes,
        vec![],
        vec![],
        vec![source],
        &f.stop
    )
    .is_err());
    assert_eq!(fs::read_dir(cache).unwrap().count(), 1);
    f.clean();
}
#[test]
fn waive_creation_seeds_pack_and_replacement_preserves_open_old_inode() {
    let f = Fixture::new();
    let s = f.store(Kind::Waives);
    let d = s
        .snapshot(&f.stop)
        .unwrap()
        .prepare_waives(&[(0, 1), (64, 255)], &f.stop)
        .unwrap();
    assert!(d.publish(&f.stop).unwrap().directory_synced);
    assert_eq!(fs::metadata(s.target()).unwrap().mode() & 0o777, 0o600);
    let first = fs::read(s.target()).unwrap();
    assert_eq!(first.len(), 40 + 65 + 12);
    assert_eq!(first[40], 1);
    assert_eq!(first[104], 255);
    let mut reader = File::open(s.target()).unwrap();
    s.snapshot(&f.stop)
        .unwrap()
        .prepare_waives(&[(0, 0), (1, 1), (64, 1)], &f.stop)
        .unwrap()
        .publish(&f.stop)
        .unwrap();
    let mut old = Vec::new();
    reader.read_to_end(&mut old).unwrap();
    assert_eq!(old, first);
    let mut p = Pack::open(&f.pack, &f.stop).unwrap();
    p.attach_waives(s.target()).unwrap();
    assert_eq!(p.status(1, 0).unwrap(), 0);
    assert_eq!(p.status(1, 1).unwrap(), 1);
    assert_eq!(p.status(1, 64).unwrap(), 1);
    assert_eq!(
        s.snapshot(&f.stop).unwrap().waives().unwrap().per_rule,
        [0, 2, 0]
    );
    assert_eq!(
        f.reviewer(Kind::Waives, "other")
            .snapshot(&f.stop)
            .unwrap()
            .waives()
            .unwrap()
            .waived,
        0
    );
    f.clean();
}
#[test]
fn notes_use_real_pack_centers_and_clear_is_an_empty_compatible_tombstone() {
    let f = Fixture::new();
    let s = f.store(Kind::Notes);
    f.note(&s, "첫 줄\nsecond").publish(&f.stop).unwrap();
    let text = fs::read_to_string(s.target()).unwrap();
    assert!(text.contains("text=0.001,0.0015,16,#FFD819,#00000059,첫 줄\\nsecond"));
    assert!(text.contains("text=0.011,0.0015,16,#FFD819,#00000059,첫 줄\\nsecond"));
    s.snapshot(&f.stop)
        .unwrap()
        .prepare_note(&[1, 2], "replacement", &f.stop)
        .unwrap()
        .publish(&f.stop)
        .unwrap();
    let snap = s.snapshot(&f.stop).unwrap();
    assert_eq!(snap.notes().unwrap().get(0), Some("첫 줄\nsecond"));
    assert_eq!(snap.notes().unwrap().get(1), Some("replacement"));
    snap.prepare_note(&[0, 1, 2], "", &f.stop)
        .unwrap()
        .publish(&f.stop)
        .unwrap();
    assert!(s.target().is_file());
    let empty = s.snapshot(&f.stop).unwrap();
    assert_eq!(empty.notes().unwrap().member_count(), 0);
    assert!(fs::read_to_string(s.target())
        .unwrap()
        .contains("floe_pack=1234,5678,65"));
    f.clean();
}
#[test]
fn note_import_reports_normalization_and_rejects_foreign_data_without_write() {
    let f = Fixture::new();
    let s = f.store(Kind::Notes);
    f.note(&s, "old").publish(&f.stop).unwrap();
    let before = fs::read(s.target()).unwrap();
    assert!(s
        .snapshot(&f.stop)
        .unwrap()
        .prepare_notes_import("floe_pack=0,0,0\nfloe_note=0|bad", &f.stop)
        .is_err());
    assert_eq!(fs::read(s.target()).unwrap(), before);
    let input="floe_pack=1234,5678,65\nfloe_note=0,1|first\nfloe_note=1,2,999|last\nfloe_note=malformed\n";
    let (d, report) = s
        .snapshot(&f.stop)
        .unwrap()
        .prepare_notes_import(input, &f.stop)
        .unwrap();
    assert_eq!(
        report,
        ImportReport {
            skipped_lines: 1,
            invalid_members: 1,
            reassigned_members: 1
        }
    );
    assert_eq!(fs::read(s.target()).unwrap(), before);
    assert!(d.legacy_unverified());
    d.accept_legacy_run().publish(&f.stop).unwrap();
    let snap = s.snapshot(&f.stop).unwrap();
    assert_eq!(snap.import_report(), &ImportReport::default());
    assert_eq!(snap.notes().unwrap().get(1), Some("last"));
    f.clean();
}
#[test]
fn two_independent_writers_reject_stale_and_newly_created_revisions() {
    let f = Fixture::new();
    let a = f.store(Kind::Notes);
    let b = f.store(Kind::Notes);
    let missing = f.note(&a, "missing snapshot");
    f.note(&b, "other writer").publish(&f.stop).unwrap();
    let before = fs::read(a.target()).unwrap();
    assert_eq!(kind(missing.publish(&f.stop)), ErrorKind::Busy);
    assert_eq!(fs::read(a.target()).unwrap(), before);
    let stale = f.note(&a, "stale");
    f.note(&b, "newest").publish(&f.stop).unwrap();
    let before = fs::read(a.target()).unwrap();
    assert_eq!(kind(stale.publish(&f.stop)), ErrorKind::Busy);
    assert_eq!(fs::read(a.target()).unwrap(), before);
    f.clean();
}
#[test]
fn metadata_content_removal_and_same_byte_replacement_are_conflicts() {
    for change in 0..4 {
        let f = Fixture::new();
        let s = f.store(Kind::Notes);
        f.note(&s, "old").publish(&f.stop).unwrap();
        let d = f.note(&s, "new");
        match change {
            0 => {
                let mut file = OpenOptions::new().append(true).open(s.target()).unwrap();
                file.write_all(b"# external\n").unwrap();
            }
            1 => fs::set_permissions(s.target(), fs::Permissions::from_mode(0o640)).unwrap(),
            2 => {
                let p = f.dir.join("replacement");
                fs::write(&p, fs::read(s.target()).unwrap()).unwrap();
                fs::rename(p, s.target()).unwrap();
            }
            _ => fs::remove_file(s.target()).unwrap(),
        }
        let before = fs::read(s.target()).ok();
        assert_eq!(kind(d.publish(&f.stop)), ErrorKind::Busy);
        assert_eq!(fs::read(s.target()).ok(), before);
        f.clean();
    }
}
#[test]
fn pack_replacement_with_identical_legacy_fingerprint_and_inplace_change_are_rejected() {
    for replace in [false, true] {
        let f = Fixture::new();
        let s = f.store(Kind::Notes);
        let d = f.note(&s, "new");
        if replace {
            let p = f.dir.join("new.ice");
            fs::write(&p, &f.bytes).unwrap();
            fs::rename(p, &f.pack).unwrap();
        } else {
            let file = OpenOptions::new().write(true).open(&f.pack).unwrap();
            let old = fs::metadata(&f.pack).unwrap().modified().unwrap();
            std::os::unix::fs::FileExt::write_all_at(&file, b"OTHER", 40).unwrap();
            file.set_times(fs::FileTimes::new().set_modified(old))
                .unwrap();
        }
        assert_eq!(kind(d.publish(&f.stop)), ErrorKind::Cache);
        assert!(!s.target().exists());
    }
}
#[test]
fn parent_directory_replacement_cannot_redirect_a_draft() {
    let f = Fixture::new();
    let s = f.store(Kind::Notes);
    let d = f.note(&s, "new");
    fs::rename(&f.dir, f.root.join("old-inputs")).unwrap();
    fs::create_dir(&f.dir).unwrap();
    fs::write(&f.pack, &f.bytes).unwrap();
    assert!(d.publish(&f.stop).is_err());
    assert_eq!(fs::read_dir(&f.dir).unwrap().count(), 1);
    assert_eq!(fs::read_dir(f.root.join("old-inputs")).unwrap().count(), 1);
}
#[test]
fn cancellation_faults_and_late_commit_outcomes_preserve_the_publication_boundary() {
    let f = Fixture::new();
    let s = f.store(Kind::Notes);
    f.note(&s, "old").publish(&f.stop).unwrap();
    let old = fs::read(s.target()).unwrap();
    let d = f.note(&s, "new");
    f.stop.store(1, Ordering::Relaxed);
    assert_eq!(kind(d.publish(&f.stop)), ErrorKind::Cancelled);
    assert_eq!(fs::read(s.target()).unwrap(), old);
    f.stop.store(0, Ordering::Relaxed);
    let d = f.note(&s, "new");
    assert_eq!(
        kind(d.publish_using(
            &f.stop,
            || Err(std::io::Error::from_raw_os_error(libc::ENOSPC).into()),
            File::sync_all
        )),
        ErrorKind::Io
    );
    assert_eq!(fs::read(s.target()).unwrap(), old);
    f.clean();
    let d = f.note(&s, "cancel");
    assert_eq!(
        kind(d.publish_using(
            &f.stop,
            || {
                f.stop.store(1, Ordering::Relaxed);
                Ok(())
            },
            File::sync_all
        )),
        ErrorKind::Cancelled
    );
    assert_eq!(fs::read(s.target()).unwrap(), old);
    f.stop.store(0, Ordering::Relaxed);
    f.clean();
    let d = f.note(&s, "committed");
    let result = d
        .publish_using(
            &f.stop,
            || Ok(()),
            |_| {
                f.stop.store(1, Ordering::Relaxed);
                Err(std::io::ErrorKind::Other.into())
            },
        )
        .unwrap();
    assert!(!result.directory_synced);
    assert_ne!(fs::read(s.target()).unwrap(), old);
    f.clean();
}
#[test]
fn stable_lock_busy_replacement_and_invalid_lock_are_not_bypassed() {
    let f = Fixture::new();
    let s = f.store(Kind::Notes);
    let d = f.note(&s, "new");
    let lock = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(s.lock_path())
        .unwrap();
    // SAFETY: live private test file descriptor.
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    assert_eq!(kind(d.publish(&f.stop)), ErrorKind::Busy);
    drop(lock);
    assert!(!s.target().exists());
    let d = f.note(&s, "new");
    assert_eq!(
        kind(d.publish_using(
            &f.stop,
            || {
                fs::remove_file(s.lock_path())?;
                fs::write(s.lock_path(), b"")?;
                Ok(())
            },
            File::sync_all
        )),
        ErrorKind::Busy
    );
    assert!(!s.target().exists());
    f.clean();
    fs::write(s.lock_path(), b"not a lock").unwrap();
    assert!(f.note(&s, "new").publish(&f.stop).is_err());
    assert!(!s.target().exists());
}
#[test]
fn permissions_and_extended_attributes_survive_replacement() {
    let f = Fixture::new();
    let s = f.store(Kind::Notes);
    f.note(&s, "old").publish(&f.stop).unwrap();
    fs::set_permissions(s.target(), fs::Permissions::from_mode(0o640)).unwrap();
    let file = File::open(s.target()).unwrap();
    #[cfg(target_os = "macos")]
    let attr = c"com.floe.review-test";
    #[cfg(not(target_os = "macos"))]
    let attr = c"user.floe-review-test";
    crate::layer_defaults::security::set(&file, attr, b"keep").unwrap();
    let security = Security::read(&file).unwrap();
    f.note(&s, "new").publish(&f.stop).unwrap();
    assert!(Security::read(&File::open(s.target()).unwrap()).unwrap() == security);
    let old = fs::read(s.target()).unwrap();
    let d = f.note(&s, "stale attributes");
    crate::layer_defaults::security::set(&File::open(s.target()).unwrap(), attr, b"changed")
        .unwrap();
    assert_eq!(kind(d.publish(&f.stop)), ErrorKind::Busy);
    assert_eq!(fs::read(s.target()).unwrap(), old);
    f.clean();
}
#[test]
fn scope_protected_inputs_symlinks_hardlinks_and_expired_drafts_fail_closed() {
    let f = Fixture::new();
    let s = f.store(Kind::Notes);
    assert!(Store::open(
        Arc::clone(&f.scope),
        &f.pack,
        "../other",
        Kind::Notes,
        vec![],
        vec![],
        &f.stop
    )
    .is_err());
    assert!(Store::open(
        Arc::clone(&f.scope),
        &f.pack,
        "reviewer",
        Kind::Notes,
        vec![s.target().to_owned()],
        vec![],
        &f.stop
    )
    .is_err());
    assert!(Store::open(
        Arc::clone(&f.scope),
        &f.pack,
        "reviewer",
        Kind::Notes,
        vec![s.lock_path()],
        vec![],
        &f.stop
    )
    .is_err());
    symlink(&f.pack, s.target()).unwrap();
    assert!(s.snapshot(&f.stop).is_err());
    fs::remove_file(s.target()).unwrap();
    fs::hard_link(&f.pack, s.target()).unwrap();
    assert!(s.snapshot(&f.stop).is_err());
    fs::remove_file(s.target()).unwrap();
    let other = f.dir.join("unrelated");
    fs::write(&other, b"").unwrap();
    fs::hard_link(&other, s.target()).unwrap();
    assert!(s.snapshot(&f.stop).is_err());
    fs::remove_file(s.target()).unwrap();
    // Creating/removing a hardlink changes the pack's ctime. A previous store
    // is correctly invalidated; explicitly register the now-stable pack again.
    let s = f.store(Kind::Notes);
    let mut d = f.note(&s, "expired");
    d.expires = Instant::now() - Duration::from_secs(1);
    assert_eq!(kind(d.publish(&f.stop)), ErrorKind::Busy);
    assert!(!s.target().exists());
    f.clean();
}
#[test]
fn readonly_sidecar_is_not_replaced_using_directory_permissions() {
    // Root can open mode-0400 files for writing; this is an access-control test.
    // SAFETY: geteuid has no arguments and cannot access caller memory.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let f = Fixture::new();
    let s = f.store(Kind::Notes);
    f.note(&s, "old").publish(&f.stop).unwrap();
    fs::set_permissions(s.target(), fs::Permissions::from_mode(0o400)).unwrap();
    let old = fs::read(s.target()).unwrap();
    assert!(f.note(&s, "new").publish(&f.stop).is_err());
    assert_eq!(fs::read(s.target()).unwrap(), old);
    f.clean();
}

#[test]
fn concurrent_cooperating_publishers_have_exactly_one_winner() {
    for _ in 0..64 {
        let f = Fixture::new();
        let a = f.store(Kind::Notes);
        let b = f.store(Kind::Notes);
        let da = f.note(&a, "writer a");
        let db = f.note(&b, "writer b");
        let barrier = std::sync::Barrier::new(2);
        let results = std::thread::scope(|scope| {
            let x = scope.spawn(|| {
                barrier.wait();
                da.publish(&f.stop)
            });
            let y = scope.spawn(|| {
                barrier.wait();
                db.publish(&f.stop)
            });
            [x.join().unwrap(), y.join().unwrap()]
        });
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter_map(|r| r.as_ref().err())
                .next()
                .unwrap()
                .kind,
            ErrorKind::Busy,
            "{results:?}"
        );
        assert!(matches!(
            a.snapshot(&f.stop).unwrap().notes().unwrap().get(0),
            Some("writer a" | "writer b")
        ));
        f.clean();
    }
}

#[test]
fn legacy_adoption_is_explicit_and_new_pack_registration_does_not_inherit_review() {
    let f = Fixture::new();
    let s = f.store(Kind::Notes);
    fs::write(s.target(), b"floe_pack=1234,5678,65\nfloe_note=0|legacy\n").unwrap();
    assert!(s.snapshot(&f.stop).unwrap().legacy_unverified());
    let old = fs::read(s.target()).unwrap();
    assert_eq!(
        kind(f.note(&s, "unapproved").publish(&f.stop)),
        ErrorKind::Unsupported
    );
    assert_eq!(fs::read(s.target()).unwrap(), old);
    assert!(!s.lock_path().exists());
    f.note(&s, "approved adoption")
        .accept_legacy_run()
        .publish(&f.stop)
        .unwrap();
    assert!(!s.snapshot(&f.stop).unwrap().legacy_unverified());
    let saved = fs::read(s.target()).unwrap();
    let replacement = f.dir.join("copy.ice");
    fs::write(&replacement, &f.bytes).unwrap();
    fs::rename(replacement, &f.pack).unwrap();
    let reopened = f.store(Kind::Notes);
    assert_eq!(kind(reopened.snapshot(&f.stop)), ErrorKind::Cache);
    assert_eq!(fs::read(s.target()).unwrap(), saved);
}
