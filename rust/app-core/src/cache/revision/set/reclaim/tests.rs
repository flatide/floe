use super::*;
use crate::registered::AccessScope;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    path: PathBuf,
    source: Arc<RegisteredSource>,
    store: Store,
    old: String,
    member: String,
}
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "floe-reclaim-{}-{}",
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
        let source = RegisteredSource::register(
            AccessScope::new(std::slice::from_ref(&path)).unwrap(),
            &source_path,
            &AtomicUsize::new(0),
        )
        .unwrap();
        let store = Store::new(source.path()).unwrap();
        let f = Self {
            path,
            source,
            store,
            old: "a".repeat(32),
            member: "b".repeat(32),
        };
        f.publish('a', 'b');
        f.publish('c', 'd');
        f
    }
    fn source_store(&self) -> super::super::super::Store {
        super::super::super::Store::new(self.source.path()).unwrap()
    }
    fn old_dir(&self) -> PathBuf {
        self.store.path().join(&self.old)
    }
    fn member_dir(&self) -> PathBuf {
        self.source_store().path().join(&self.member)
    }
    fn publish(&self, set: char, member: char) {
        let set = set.to_string().repeat(32);
        let member = member.to_string().repeat(32);
        let raw = self.source_store();
        for root in [self.store.path(), raw.path()] {
            fs::create_dir_all(root).unwrap();
            drop(WriteLease::acquire(root).unwrap());
        }
        let path = raw.path().join(&member);
        fs::create_dir(&path).unwrap();
        for name in ["meta.json", "design.ovm", "design.ovp"] {
            fs::write(path.join(name), b"synthetic payload").unwrap();
        }
        let record = Record {
            version: 3,
            source: self.source.path().into(),
            revision: member.clone(),
            source_stamp: Stamp::source(self.source.path()).unwrap(),
            files: files(&path, false).unwrap(),
            owner: Some(SetOwner {
                source: self.source.path().into(),
                revision: set.clone(),
            }),
        };
        create_record(
            &path.join("revision.json"),
            &serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        let manifest = Manifest {
            version: 3,
            source: self.source.path().into(),
            source_stamp: Stamp::source(self.source.path()).unwrap(),
            revision: set.clone(),
            levels: None,
            members: vec![Member {
                source: self.source.path().into(),
                revision: member,
            }],
        };
        let bytes = serde_json::to_vec(&manifest).unwrap();
        let dir = self.store.path().join(&set);
        fs::create_dir(&dir).unwrap();
        create_record_limit(&dir.join("revision.json"), &bytes, MAX_SET_BYTES).unwrap();
        create_record_limit(&dir.join("published.json"), &bytes, MAX_SET_BYTES).unwrap();
        fs::write(self.store.path().join("current.json"), bytes).unwrap();
    }
    fn prepare(&self) -> Result<Prepared> {
        prepare(Arc::clone(&self.source), &self.old, &AtomicUsize::new(0))
    }
    fn journal(&self) -> PathBuf {
        journal_path(&self.store, &self.old)
    }
    fn current_bytes(&self) -> Vec<u8> {
        fs::read(self.store.path().join("current.json")).unwrap()
    }
    fn assert_protected(&self) {
        assert!(self.prepare().is_err());
        assert!(self.old_dir().exists());
        assert!(self.member_dir().join("design.ovp").exists());
        assert!(!self.journal().exists());
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.path).unwrap();
    }
}

#[test]
fn read_only_preview_and_one_explicit_reclamation_preserve_current_and_source() {
    let f = Fixture::new();
    let source = fs::read(f.source.path()).unwrap();
    let current = f.current_bytes();
    let preview = f.prepare().unwrap();
    assert_eq!(preview.summary.files, 6);
    assert_eq!(preview.summary.sources, 1);
    assert_eq!(preview.summary.token.len(), 64);
    assert!(!preview.summary.recovery && !preview.summary.complete);
    assert!(!f.journal().exists());
    let bytes = preview.summary.logical_bytes.clone();
    let outcome = preview.execute(&AtomicUsize::new(0)).unwrap();
    assert_eq!(outcome.status, "complete");
    assert_eq!(outcome.removed_files, 6);
    assert_eq!(outcome.removed_logical_bytes, bytes);
    assert!(!outcome.sync_warning);
    assert!(!f.old_dir().exists());
    assert!(!f.member_dir().exists());
    assert!(f.journal().exists());
    assert_eq!(f.current_bytes(), current);
    assert_eq!(fs::read(f.source.path()).unwrap(), source);
    let inventory =
        crate::cache::revision::inventory::inspect(&f.source, &AtomicUsize::new(0)).unwrap();
    assert!(
        !inventory.partial,
        "the retained journal is accounted for, not unknown garbage"
    );
    assert_eq!(inventory.rows.len(), 2);
    assert!(f
        .source_store()
        .path()
        .join("d".repeat(32))
        .join("design.ovp")
        .exists());
    let completed = f.prepare().unwrap();
    assert!(completed.summary.complete && completed.summary.recovery);
    assert_eq!(completed.summary.files, 0);
    assert_eq!(
        completed
            .execute(&AtomicUsize::new(0))
            .unwrap()
            .removed_files,
        0
    );
}

#[test]
fn current_source_current_and_live_set_or_member_readers_are_protected() {
    for kind in 0..6 {
        let f = Fixture::new();
        let mut lease = None;
        match kind {
            0 => fs::write(
                f.store.path().join("current.json"),
                fs::read(f.old_dir().join("revision.json")).unwrap(),
            )
            .unwrap(),
            1 => fs::write(
                f.source_store().path().join("current.json"),
                fs::read(f.member_dir().join("revision.json")).unwrap(),
            )
            .unwrap(),
            2 | 3 => {
                let dir = directory(&if kind == 2 {
                    f.old_dir()
                } else {
                    f.member_dir()
                })
                .unwrap();
                dir.try_lock_shared().unwrap();
                lease = Some(dir);
            }
            4 => {
                let _writer = WriteLease::acquire_existing(f.store.path()).unwrap();
                f.assert_protected();
            }
            _ => {
                let mut current: Manifest = serde_json::from_slice(&f.current_bytes()).unwrap();
                current.members[0].revision = f.member.clone();
                let bytes = serde_json::to_vec(&current).unwrap();
                fs::write(f.store.path().join("current.json"), &bytes).unwrap();
                fs::write(
                    f.store.path().join(&current.revision).join("revision.json"),
                    bytes,
                )
                .unwrap();
            }
        }
        if kind != 4 {
            f.assert_protected();
        }
        drop(lease);
    }
}

#[test]
fn untracked_unpublished_extra_linked_pending_and_foreign_owned_revisions_are_protected() {
    for kind in 0..10 {
        let f = Fixture::new();
        match kind {
            0 | 1 => {
                let mut manifest: Manifest =
                    serde_json::from_slice(&fs::read(f.old_dir().join("revision.json")).unwrap())
                        .unwrap();
                manifest.version = kind + 1;
                for name in ["revision.json", "published.json"] {
                    fs::write(
                        f.old_dir().join(name),
                        serde_json::to_vec(&manifest).unwrap(),
                    )
                    .unwrap();
                }
            }
            2 => fs::remove_file(f.old_dir().join("published.json")).unwrap(),
            3 => fs::write(f.old_dir().join("published.json"), b"unknown outcome").unwrap(),
            4 => fs::write(f.member_dir().join("extra"), b"do not delete").unwrap(),
            5 => fs::hard_link(f.member_dir().join("design.ovp"), f.path.join("hardlink")).unwrap(),
            6 => {
                fs::remove_file(f.member_dir().join("design.ovp")).unwrap();
                fs::write(f.path.join("outside"), b"preserve").unwrap();
                std::os::unix::fs::symlink(
                    f.path.join("outside"),
                    f.member_dir().join("design.ovp"),
                )
                .unwrap();
            }
            7 => fs::write(
                f.store.path().join(format!(".current-{}.tmp", f.old)),
                b"pending",
            )
            .unwrap(),
            8 => {
                let path = f.member_dir().join("revision.json");
                let mut record: Record = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                record.owner.as_mut().unwrap().revision = "f".repeat(32);
                fs::write(path, serde_json::to_vec(&record).unwrap()).unwrap();
            }
            _ => fs::create_dir(f.old_dir().join("nested")).unwrap(),
        }
        f.assert_protected();
    }
}

#[test]
fn approval_rechecks_current_pins_files_and_preview_expiry_before_writing() {
    for kind in 0..5 {
        let f = Fixture::new();
        let mut preview = f.prepare().unwrap();
        let mut lease = None;
        match kind {
            0 => f.publish('e', 'f'),
            1 => {
                let dir = directory(&f.member_dir()).unwrap();
                dir.try_lock_shared().unwrap();
                lease = Some(dir);
            }
            2 => fs::write(f.member_dir().join("design.ovp"), b"changed").unwrap(),
            3 => preview.created -= PREVIEW_TTL + Duration::from_secs(1),
            _ => fs::write(f.old_dir().join("unexpected"), b"preserve").unwrap(),
        }
        assert!(preview.execute(&AtomicUsize::new(0)).is_err(), "{kind}");
        assert!(!f.journal().exists());
        assert!(f.member_dir().join("design.ovp").exists());
        drop(lease);
    }
}

#[test]
fn cancellation_before_mutation_and_during_deletion_require_fresh_approval() {
    let f = Fixture::new();
    assert!(f.prepare().unwrap().execute(&AtomicUsize::new(1)).is_err());
    assert!(!f.journal().exists());
    let stop = AtomicUsize::new(0);
    let outcome = f
        .prepare()
        .unwrap()
        .execute_with(&stop, |_, _| {
            stop.store(1, Ordering::Release);
            Ok(())
        })
        .unwrap();
    assert_eq!(outcome.status, "interrupted");
    assert_eq!(outcome.removed_files, 1);
    assert!(f.journal().exists());
    let preview = f.prepare().unwrap();
    assert!(preview.summary.recovery && !preview.summary.complete);
    assert_eq!(preview.summary.files, 5);
    assert_eq!(
        preview.execute(&AtomicUsize::new(0)).unwrap().status,
        "complete"
    );
}

#[test]
fn uncertain_unlink_is_not_retried_and_recovery_inspects_actual_remaining_files() {
    for unlink_happened in [false, true] {
        let f = Fixture::new();
        let outcome = f
            .prepare()
            .unwrap()
            .execute_with(&AtomicUsize::new(0), |count, path| {
                if count == 1 {
                    if unlink_happened {
                        fs::remove_file(path)?;
                    }
                    return Err(std::io::Error::other("injected unknown unlink result"));
                }
                Ok(())
            })
            .unwrap();
        assert_eq!(outcome.status, "outcome_unknown");
        assert_eq!(outcome.removed_files, 1);
        let preview = f.prepare().unwrap();
        assert_eq!(preview.summary.files, if unlink_happened { 4 } else { 5 });
        assert!(preview.summary.recovery);
        assert!(
            f.old_dir().exists(),
            "preparing recovery must not resume deletion"
        );
        assert_eq!(
            preview.execute(&AtomicUsize::new(0)).unwrap().status,
            "complete"
        );
    }
}

#[test]
fn recovery_rejects_tampered_journals_and_new_files_without_mutating_them() {
    for kind in 0..5 {
        let f = Fixture::new();
        let result = f
            .prepare()
            .unwrap()
            .execute_with(&AtomicUsize::new(0), |_, _| {
                Err(std::io::Error::other("interrupt"))
            })
            .unwrap();
        assert_eq!(result.status, "outcome_unknown");
        let original = fs::read(f.member_dir().join("design.ovp")).unwrap();
        match kind {
            0 => fs::write(f.journal(), b"partial journal").unwrap(),
            1 => {
                let mut journal: Journal =
                    serde_json::from_slice(&fs::read(f.journal()).unwrap()).unwrap();
                journal.targets[1].source = f.path.join("unregistered.oas");
                fs::write(f.journal(), serde_json::to_vec(&journal).unwrap()).unwrap();
            }
            2 => fs::write(f.member_dir().join("new-file"), b"preserve").unwrap(),
            3 => {
                let mut journal: Journal =
                    serde_json::from_slice(&fs::read(f.journal()).unwrap()).unwrap();
                journal.targets[1].dir_id.1 += 1;
                fs::write(f.journal(), serde_json::to_vec(&journal).unwrap()).unwrap();
            }
            _ => {
                let preview = f.prepare().unwrap();
                fs::write(f.journal(), b"changed after preview").unwrap();
                assert!(preview.execute(&AtomicUsize::new(0)).is_err());
            }
        }
        assert!(f.prepare().is_err());
        assert_eq!(
            fs::read(f.member_dir().join("design.ovp")).unwrap(),
            original
        );
    }
}

#[test]
fn invalid_ids_and_missing_stores_do_not_create_any_files() {
    let f = Fixture::new();
    for id in ["../outside", "", &"F".repeat(32), &"0".repeat(32)] {
        assert!(prepare(Arc::clone(&f.source), id, &AtomicUsize::new(0)).is_err());
    }
    assert!(!f.journal().exists());
}

#[test]
fn cancellation_after_last_unlink_still_requires_empty_directory_recovery() {
    let f = Fixture::new();
    let stop = AtomicUsize::new(0);
    let outcome = f
        .prepare()
        .unwrap()
        .execute_with(&stop, |count, _| {
            if count == 5 {
                stop.store(1, Ordering::Release);
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(outcome.status, "interrupted");
    assert_eq!(outcome.removed_files, 6);
    let preview = f.prepare().unwrap();
    assert_eq!(preview.summary.files, 0);
    assert!(!preview.summary.complete);
    assert!(f.old_dir().exists());
    assert_eq!(
        preview.execute(&AtomicUsize::new(0)).unwrap().status,
        "complete"
    );
    assert!(!f.old_dir().exists());
}

#[test]
fn killed_synthetic_reclaimer_releases_locks_but_does_not_resume_without_approval() {
    let f = Fixture::new();
    let current = f.current_bytes();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "cache::revision::set::reclaim::tests::reclaim_child",
            "--nocapture",
        ])
        .env("FLOE_TEST_RECLAIM_ROOT", &f.path)
        .spawn()
        .unwrap();
    let ready = f.path.join("child-ready");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready.exists() && Instant::now() < deadline {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let reached = ready.exists();
    // Always reap, even if the child failed to reach its deterministic barrier.
    let _ = child.kill();
    let status = child.wait().unwrap();
    assert!(reached && !status.success());
    let preview = f.prepare().unwrap();
    assert!(preview.summary.recovery && !preview.summary.complete);
    assert_eq!(preview.summary.files, 5);
    assert!(f.old_dir().exists());
    assert_eq!(f.current_bytes(), current);
    assert_eq!(
        preview.execute(&AtomicUsize::new(0)).unwrap().status,
        "complete"
    );
    assert_eq!(f.current_bytes(), current);
}

#[test]
fn reclaim_child() {
    let Some(root) = std::env::var_os("FLOE_TEST_RECLAIM_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let source = RegisteredSource::register(
        AccessScope::new(std::slice::from_ref(&root)).unwrap(),
        &root.join("synthetic.oas"),
        &AtomicUsize::new(0),
    )
    .unwrap();
    let preview = prepare(source, &"a".repeat(32), &AtomicUsize::new(0)).unwrap();
    preview
        .execute_with(&AtomicUsize::new(0), |count, _| {
            if count == 1 {
                fs::write(root.join("child-ready"), b"one acknowledged unlink")?;
                // Parent kills only this newly spawned synthetic helper. A bounded
                // fallback avoids leaving a helper alive if the parent itself dies.
                std::thread::sleep(Duration::from_secs(20));
                return Err(std::io::Error::other("barrier expired"));
            }
            Ok(())
        })
        .unwrap();
}
