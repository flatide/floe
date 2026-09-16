use super::*;
use crate::registered::AccessScope;
use std::os::unix::fs::{symlink, PermissionsExt};

const TEXT: &str = "3.0 red solid Mask 1 3\n";

#[test]
fn dynamic_drc_inputs_and_review_targets_recheck_already_prepared_defaults() {
    for review_target in [false, true] {
        for lock_target in [false, true] {
            let f = Fixture::new();
            let draft = f.draft();
            let target = if lock_target {
                f.dir.join("design.jb.layerprops.lock")
            } else {
                f.target()
            };
            let mut p = f.publisher.sources.begin(&f.stop).unwrap();
            if review_target {
                p.protect_review_targets(std::slice::from_ref(&target), &f.stop)
                    .unwrap();
            } else {
                p.protect_inputs(std::slice::from_ref(&target), &[], &f.stop)
                    .unwrap();
            }
            p.commit(&f.stop).unwrap();
            assert_eq!(kind(draft.publish(&f.stop)), ErrorKind::InvalidInput);
            assert!(!f.target().exists() && !f.dir.join("design.jb.layerprops.lock").exists());
            f.no_stage();
        }
    }
}

#[test]
fn dynamic_sources_protect_old_drafts_and_publication_excludes_registration() {
    for suffix in ["", ".lock"] {
        let f = Fixture::new();
        let draft = f.draft();
        let set = &f.publisher.sources;
        let path = f.dir.join("additional.jb");
        fs::write(
            &path,
            format!("CHIP A\n$ (1,A,TC=design.jb.layerprops{suffix})\n"),
        )
        .unwrap();
        let mut pending = set.begin(&f.stop).unwrap();
        pending
            .register(
                AccessScope::new(std::slice::from_ref(&f.dir)).unwrap(),
                &path,
                &f.stop,
            )
            .unwrap();
        pending.commit(&f.stop).unwrap();
        assert_eq!(kind(draft.publish(&f.stop)), ErrorKind::InvalidInput);
        assert!(!f.target().exists());
        assert!(!f.dir.join("design.jb.layerprops.lock").exists());
        f.no_stage();
    }
    let f = Fixture::new();
    let pending = f.publisher.sources.begin(&f.stop).unwrap();
    assert_eq!(kind(f.draft().publish(&f.stop)), ErrorKind::Busy);
    assert!(!f.target().exists());
    assert!(!f.dir.join("design.jb.layerprops.lock").exists());
    drop(pending);
    f.draft()
        .publish_with(&f.stop, || {
            assert_eq!(kind(f.publisher.sources.begin(&f.stop)), ErrorKind::Busy);
            assert_eq!(f.publisher.sources.snapshot().len(), 1);
            Ok(())
        })
        .unwrap();
    drop(f.publisher.sources.begin(&f.stop).unwrap());
    f.no_stage();
}
struct Fixture {
    dir: PathBuf,
    source: Arc<RegisteredSource>,
    publisher: Arc<Publisher>,
    stop: AtomicUsize,
}
impl Fixture {
    fn new() -> Self {
        Self::named("design.jb")
    }
    fn named(name: &str) -> Self {
        let n = SERIAL.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("floe-defaults-{}-{n}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let stop = AtomicUsize::new(0);
        let path = dir.join(name);
        fs::write(&path, "MTITLE 1,Mask\nCHIP A\n$ (1,A,TC=missing.oas)\n").unwrap();
        let scope = AccessScope::new(std::slice::from_ref(&dir)).unwrap();
        let source = RegisteredSource::register(scope, &path, &stop).unwrap();
        let publisher = Publisher::new(vec![Arc::clone(&source)]).unwrap();
        Self {
            dir,
            source,
            publisher,
            stop,
        }
    }
    fn draft(&self) -> Draft {
        self.publisher
            .prepare(Arc::clone(&self.source), Mode::Level, TEXT, &self.stop)
            .unwrap()
    }
    fn target(&self) -> PathBuf {
        let mut p = self.source.path().as_os_str().to_owned();
        p.push(".layerprops");
        p.into()
    }
    fn no_stage(&self) {
        assert!(!fs::read_dir(&self.dir).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".floe-layerprops-")));
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.dir).unwrap();
    }
}
fn kind<T>(result: Result<T>) -> ErrorKind {
    match result {
        Err(e) => e.kind,
        Ok(_) => panic!("expected rejection"),
    }
}

#[test]
fn concurrent_lock_creation_opens_one_stable_inode_without_truncation() {
    let f = Fixture::new();
    let dir = Directory::open(&f.dir).unwrap();
    for i in 0..64 {
        let name = CString::new(format!("concurrent-{i}.lock")).unwrap();
        let barrier = std::sync::Barrier::new(2);
        let files = std::thread::scope(|scope| {
            let open = || {
                barrier.wait();
                dir.open_lock(&name, 0o600).unwrap()
            };
            let a = scope.spawn(open);
            let b = scope.spawn(open);
            [a.join().unwrap(), b.join().unwrap()]
        });
        assert_eq!(
            identity(&files[0].metadata().unwrap()),
            identity(&files[1].metadata().unwrap())
        );
        (&files[0]).write_all(b"do not truncate").unwrap();
        let reopened = dir.open_lock(&name, 0o600).unwrap();
        assert_eq!(reopened.metadata().unwrap().len(), 15);
        assert_eq!(
            identity(&reopened.metadata().unwrap()),
            identity(&files[0].metadata().unwrap())
        );
    }
}

#[test]
fn prepare_is_read_only_and_modes_derive_exact_shared_names() {
    let f = Fixture::named("한 글.test.jb");
    for (mode, name) in [
        (Mode::Level, "한 글.test.jb.layerprops"),
        (Mode::Chip, "한 글.test.chip-by-level.jb.layerprops"),
        (Mode::Layer, "한 글.test.layer.jb.layerprops"),
    ] {
        let d = f
            .publisher
            .prepare(Arc::clone(&f.source), mode, TEXT, &f.stop)
            .unwrap();
        assert_eq!(d.target().file_name().unwrap(), name);
        assert!(!d.replaces_existing());
        assert!(d.bytes() > 0);
        assert_eq!(fs::read_dir(&f.dir).unwrap().count(), 1);
    }
    let other = Fixture::new();
    assert_eq!(
        kind(
            f.publisher
                .prepare(Arc::clone(&other.source), Mode::Level, TEXT, &f.stop)
        ),
        ErrorKind::InvalidInput
    );
    for text in ["", "junk", "3.0 red solid Mask 1 3\njunk"] {
        assert!(f
            .publisher
            .prepare(Arc::clone(&f.source), Mode::Level, text, &f.stop)
            .is_err());
    }
    assert_eq!(fs::read_dir(&f.dir).unwrap().count(), 1);
}

#[test]
fn create_and_replace_preserve_old_open_inode_and_shared_permissions() {
    let f = Fixture::new();
    let expected = layerprops::format(&layerprops::parse(TEXT).unwrap().rows).unwrap();
    f.draft().publish(&f.stop).unwrap();
    assert_eq!(fs::read_to_string(f.target()).unwrap(), expected);
    fs::write(f.target(), "previous").unwrap();
    fs::set_permissions(f.target(), fs::Permissions::from_mode(0o640)).unwrap();
    let mut old = File::open(f.target()).unwrap();
    let before = Security::read(&old).unwrap();
    let d = f.draft();
    assert!(d.replaces_existing());
    d.publish(&f.stop).unwrap();
    let new = File::open(f.target()).unwrap();
    assert!(Security::read(&new).unwrap() == before);
    assert_ne!(
        identity(&old.metadata().unwrap()),
        identity(&new.metadata().unwrap())
    );
    let mut old_text = String::new();
    old.read_to_string(&mut old_text).unwrap();
    assert_eq!(old_text, "previous");
    assert_eq!(fs::read_to_string(f.target()).unwrap(), expected);
    assert_eq!(
        fs::metadata(f.dir.join("design.jb.layerprops.lock"))
            .unwrap()
            .len(),
        0
    );
    f.no_stage();
}

#[test]
fn preserve_extended_attributes_and_detect_attribute_changes() {
    let f = Fixture::new();
    fs::write(f.target(), "previous").unwrap();
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(f.target())
        .unwrap();
    #[cfg(target_os = "macos")]
    let name = c"org.floe.test";
    #[cfg(not(target_os = "macos"))]
    let name = c"user.floe.test";
    security::set(&file, name, b"permission test").unwrap();
    let before = Security::read(&file).unwrap();
    f.draft().publish(&f.stop).unwrap();
    let current = File::open(f.target()).unwrap();
    assert!(Security::read(&current).unwrap() == before);
    let draft = f.draft();
    security::set(&current, name, b"changed").unwrap();
    assert_eq!(kind(draft.publish(&f.stop)), ErrorKind::Busy);
    f.no_stage();
}

#[test]
fn conflict_on_content_permissions_replacement_removal_and_creation() {
    for change in 0..5 {
        let f = Fixture::new();
        if change != 4 {
            fs::write(f.target(), "old").unwrap();
        }
        let d = f.draft();
        match change {
            0 => fs::write(f.target(), "new").unwrap(),
            1 => fs::set_permissions(f.target(), fs::Permissions::from_mode(0o640)).unwrap(),
            2 => {
                fs::write(f.dir.join("replacement"), "new inode").unwrap();
                fs::rename(f.dir.join("replacement"), f.target()).unwrap();
            }
            3 => fs::remove_file(f.target()).unwrap(),
            _ => fs::write(f.target(), "newly created").unwrap(),
        }
        let expected = fs::read(f.target()).ok();
        assert_eq!(kind(d.publish(&f.stop)), ErrorKind::Busy, "change {change}");
        assert_eq!(fs::read(f.target()).ok(), expected);
        f.no_stage();
    }
}

#[test]
fn cancellation_expiry_and_precommit_fault_leave_previous_file() {
    for fault in 0..5 {
        let f = Fixture::new();
        fs::write(f.target(), "old").unwrap();
        let mut d = f.draft();
        if fault == 0 {
            f.stop.store(1, Ordering::Relaxed);
        }
        if fault == 1 {
            d.expires = Instant::now();
        }
        let error = kind(d.publish_with(&f.stop, || {
            match fault {
                2 => return Err(Error::new(ErrorKind::Io, "injected write failure")),
                3 => f.stop.store(1, Ordering::Relaxed),
                4 => fs::write(f.target(), "other writer").unwrap(),
                _ => (),
            }
            Ok(())
        }));
        assert_eq!(
            error,
            match fault {
                0 | 3 => ErrorKind::Cancelled,
                1 | 4 => ErrorKind::Busy,
                _ => ErrorKind::Io,
            }
        );
        assert_eq!(
            fs::read_to_string(f.target()).unwrap(),
            if fault == 4 { "other writer" } else { "old" }
        );
        f.no_stage();
    }
}

#[test]
fn stable_lock_is_nonblocking_and_only_one_prepared_writer_wins() {
    let f = Fixture::new();
    let a = f.draft();
    let b = f.draft();
    let lock = a
        .directory
        .open_leaf(&a.lock_name, libc::O_RDWR | libc::O_CREAT, 0o600)
        .unwrap();
    // SAFETY: live fd, test-only advisory lock.
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    let start = Instant::now();
    assert_eq!(kind(a.publish(&f.stop)), ErrorKind::Busy);
    assert!(start.elapsed() < Duration::from_secs(2));
    drop(lock);
    let c = f.draft();
    b.publish(&f.stop).unwrap();
    assert_eq!(kind(c.publish(&f.stop)), ErrorKind::Busy);
    let id = identity(&fs::metadata(f.dir.join("design.jb.layerprops.lock")).unwrap());
    f.draft().publish(&f.stop).unwrap();
    assert_eq!(
        identity(&fs::metadata(f.dir.join("design.jb.layerprops.lock")).unwrap()),
        id
    );
    f.no_stage();
}

#[test]
fn refuses_special_targets_and_never_follows_links() {
    for variant in 0..5 {
        let f = Fixture::new();
        fs::write(f.dir.join("victim"), "unrelated").unwrap();
        match variant {
            0 => symlink(f.dir.join("victim"), f.target()).unwrap(),
            1 => fs::hard_link(f.dir.join("victim"), f.target()).unwrap(),
            2 => fs::create_dir(f.target()).unwrap(),
            3 => {
                let p = CString::new(f.target().as_os_str().as_bytes()).unwrap();
                // SAFETY: valid path in our private synthetic fixture directory.
                assert_eq!(unsafe { libc::mkfifo(p.as_ptr(), 0o600) }, 0);
            }
            _ => {
                let file = File::create(f.target()).unwrap();
                file.set_len(layerprops::MAX_BYTES as u64 + 1).unwrap();
            }
        }
        assert!(f
            .publisher
            .prepare(Arc::clone(&f.source), Mode::Level, TEXT, &f.stop)
            .is_err());
        assert_eq!(
            fs::read_to_string(f.dir.join("victim")).unwrap(),
            "unrelated"
        );
        f.no_stage();
    }
}

#[test]
fn source_mutation_and_protected_dependencies_prevent_publication() {
    let f = Fixture::new();
    let d = f.draft();
    fs::write(f.source.path(), "CHIP Changed\n").unwrap();
    assert_eq!(kind(d.publish(&f.stop)), ErrorKind::Cache);
    assert!(!f.target().exists());
    let g = Fixture::new();
    fs::write(g.source.path(), "CHIP A\n$ (1,A,TC=design.jb.layerprops)\n").unwrap();
    let scope = AccessScope::new(std::slice::from_ref(&g.dir)).unwrap();
    let source = RegisteredSource::register(scope, g.source.path(), &g.stop).unwrap();
    let p = Publisher::new(vec![Arc::clone(&source)]).unwrap();
    assert!(p.prepare(source, Mode::Level, TEXT, &g.stop).is_err());
    assert!(!g.target().exists());
}

#[test]
fn precommit_lock_or_stage_substitution_is_not_overwritten_or_deleted() {
    for replace_lock in [true, false] {
        let f = Fixture::new();
        fs::write(f.target(), "old").unwrap();
        let d = f.draft();
        assert_eq!(
            kind(d.publish_with(&f.stop, || {
                let name = if replace_lock {
                    "design.jb.layerprops.lock".into()
                } else {
                    fs::read_dir(&f.dir)
                        .unwrap()
                        .map(|e| e.unwrap().file_name())
                        .find(|n| n.to_string_lossy().starts_with(".floe-layerprops-"))
                        .unwrap()
                };
                let p = f.dir.join(name);
                fs::rename(&p, f.dir.join("moved"))?;
                fs::write(&p, "replacement")?;
                Ok(())
            })),
            ErrorKind::Busy
        );
        assert_eq!(fs::read_to_string(f.target()).unwrap(), "old");
        assert!(fs::read_dir(&f.dir)
            .unwrap()
            .filter_map(|e| fs::read_to_string(e.unwrap().path()).ok())
            .any(|s| s == "replacement"));
    }
}

#[test]
fn staging_cleanup_works_without_read_permission() {
    let f = Fixture::new();
    let d = f.draft();
    let stage = Stage::create(d.directory, &f.publisher).unwrap();
    stage
        .file
        .set_permissions(fs::Permissions::from_mode(0o000))
        .unwrap();
    stage.validate().unwrap();
    drop(stage);
    f.no_stage();
}

#[test]
fn committed_result_survives_late_cancel_and_directory_sync_error() {
    for replace in [false, true] {
        let f = Fixture::new();
        if replace {
            fs::write(f.target(), "old").unwrap();
        }
        let result = f
            .draft()
            .publish_using(
                &f.stop,
                || Ok(()),
                |_| {
                    f.stop.store(1, Ordering::Relaxed);
                    Err(std::io::Error::other("injected directory fsync failure"))
                },
            )
            .unwrap();
        assert!(!result.directory_synced);
        assert_eq!(
            fs::read_to_string(f.target()).unwrap(),
            layerprops::format(&layerprops::parse(TEXT).unwrap().rows).unwrap()
        );
        f.no_stage();
    }
}

#[test]
fn replacement_preserves_named_acl_and_new_file_inherits_directory_policy() {
    let f = Fixture::new();
    fs::write(f.target(), "old").unwrap();
    #[cfg(target_os = "macos")]
    assert!(std::process::Command::new("/bin/chmod")
        .args(["+a", "everyone allow read"])
        .arg(f.target())
        .status()
        .unwrap()
        .success());
    #[cfg(target_os = "linux")]
    {
        let file = File::open(f.target()).unwrap();
        let mut acl = 2u32.to_le_bytes().to_vec();
        // Linux POSIX ACL xattr: user_obj, named user, group_obj, mask, other.
        for (tag, perm, id) in [
            (1u16, 6u16, u32::MAX),
            (2, 4, file.metadata().unwrap().uid() + 1),
            (4, 4, u32::MAX),
            (16, 4, u32::MAX),
            (32, 0, u32::MAX),
        ] {
            acl.extend(tag.to_le_bytes());
            acl.extend(perm.to_le_bytes());
            acl.extend(id.to_le_bytes());
        }
        security::set(&file, c"system.posix_acl_access", &acl).unwrap();
    }
    let before = Security::read(&File::open(f.target()).unwrap()).unwrap();
    f.draft().publish(&f.stop).unwrap();
    assert!(Security::read(&File::open(f.target()).unwrap()).unwrap() == before);
    fs::remove_file(f.target()).unwrap();
    #[cfg(target_os = "macos")]
    assert!(std::process::Command::new("/bin/chmod")
        .args([
            "+a",
            "everyone allow read,readattr,readextattr,readsecurity,file_inherit,directory_inherit"
        ])
        .arg(&f.dir)
        .status()
        .unwrap()
        .success());
    let control = File::create(f.dir.join("normal-create")).unwrap();
    let expected = Security::read(&control).unwrap();
    f.draft().publish(&f.stop).unwrap();
    assert!(Security::read(&File::open(f.target()).unwrap()).unwrap() == expected);
    f.no_stage();
}

#[test]
fn all_registered_sources_and_their_caches_are_protected() {
    let f = Fixture::new();
    let other = f.dir.join("other.jb");
    fs::write(&other, "CHIP A\n$ (1,A,TC=design.jb.layerprops)\n").unwrap();
    let scope = AccessScope::new(std::slice::from_ref(&f.dir)).unwrap();
    let registered = RegisteredSource::register(scope, &other, &f.stop).unwrap();
    let p = Publisher::new(vec![Arc::clone(&f.source), registered]).unwrap();
    assert!(p
        .prepare(Arc::clone(&f.source), Mode::Level, TEXT, &f.stop)
        .is_err());
    for path in [
        f.dir.join("missing.oas"),
        f.dir.join("missing.oas.floe/page.bin"),
        f.dir.join("missing.oas.floe.index.lock"),
        f.dir.join(".missing.oas.ice/page.bin"),
        f.dir.join(".missing.oas.ice.index.lock"),
        other,
    ] {
        assert!(p.protect(&path).is_err(), "{}", path.display());
    }
    assert!(!f.target().exists());
}

#[test]
fn directory_replacement_and_unsafe_lock_do_not_write_elsewhere() {
    let f = Fixture::new();
    let d = f.draft();
    let old = f.dir.with_extension("moved");
    fs::rename(&f.dir, &old).unwrap();
    fs::create_dir(&f.dir).unwrap();
    assert_eq!(kind(d.publish(&f.stop)), ErrorKind::Busy);
    assert_eq!(fs::read_dir(&f.dir).unwrap().count(), 0);
    fs::remove_dir(&f.dir).unwrap();
    fs::rename(&old, &f.dir).unwrap();
    fs::write(f.dir.join("victim"), "unrelated").unwrap();
    symlink(
        f.dir.join("victim"),
        f.dir.join("design.jb.layerprops.lock"),
    )
    .unwrap();
    assert!(f
        .publisher
        .prepare(Arc::clone(&f.source), Mode::Level, TEXT, &f.stop)
        .is_err());
    assert_eq!(
        fs::read_to_string(f.dir.join("victim")).unwrap(),
        "unrelated"
    );
    assert!(!f.target().exists());
    f.no_stage();
}

#[test]
fn extra_registrations_and_inode_aliases_are_never_publication_targets() {
    let f = Fixture::new();
    let p =
        Publisher::with_protected(vec![Arc::clone(&f.source)], vec![f.target()], vec![]).unwrap();
    assert!(p
        .prepare(Arc::clone(&f.source), Mode::Level, TEXT, &f.stop)
        .is_err());
    let file = f.dir.join("private-session");
    fs::write(&file, "private synthetic value").unwrap();
    fs::hard_link(&file, f.target()).unwrap();
    assert!(reject_aliases(&f.target(), std::slice::from_ref(&file)).is_err());
    fs::remove_file(f.target()).unwrap();
    let p = Publisher::with_protected(vec![Arc::clone(&f.source)], vec![], vec![f.dir.clone()])
        .unwrap();
    assert!(p
        .prepare(Arc::clone(&f.source), Mode::Level, TEXT, &f.stop)
        .is_err());
    assert_eq!(fs::read_to_string(file).unwrap(), "private synthetic value");
}
