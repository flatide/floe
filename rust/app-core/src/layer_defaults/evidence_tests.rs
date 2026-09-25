use super::*;
use crate::layer_defaults::tests::Fixture;
use std::os::unix::fs::{symlink, PermissionsExt};

fn unsupported() -> security::fault::Guard {
    security::fault::inject(libc::ENOTSUP, libc::EOPNOTSUPP)
}
fn paths(f: &Fixture) -> Vec<PathBuf> {
    fs::read_dir(&f.dir)
        .unwrap()
        .map(|v| v.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".floe-meta-")
        })
        .collect()
}
#[test]
fn no_xattr_defaults_create_replace_reopen_and_migrate_without_changing_format() {
    for list in [0, libc::ENOTSUP] {
        let f = Fixture::new();
        let guard = security::fault::inject(list, libc::EOPNOTSUPP);
        let expected = f.draft().text;
        f.draft().publish(&f.stop).unwrap();
        assert_eq!(fs::read(f.target()).unwrap(), expected);
        assert_eq!(paths(&f).len(), 1);
        fs::set_permissions(f.target(), fs::Permissions::from_mode(0o640)).unwrap();
        f.draft().publish(&f.stop).unwrap();
        assert_eq!(paths(&f).len(), 1); // autosave does not accumulate records
        assert_eq!(fs::metadata(f.target()).unwrap().mode() & 0o777, 0o640);
        assert_eq!(fs::metadata(&paths(&f)[0]).unwrap().mode() & 0o777, 0o640);
        let reopened = Publisher::new(vec![Arc::clone(&f.source)]).unwrap();
        reopened
            .prepare(
                Arc::clone(&f.source),
                crate::jobdeck::color::Mode::Level,
                "3 red solid M 1 3",
                &f.stop,
            )
            .unwrap()
            .publish(&f.stop)
            .unwrap();
        assert_eq!(paths(&f).len(), 1);
        // A later xattr-capable mount still reads fallback evidence, then
        // migrates on explicit publication only.
        drop(guard);
        f.draft().publish(&f.stop).unwrap();
        assert!(paths(&f).is_empty());
    }
}
#[test]
fn no_xattr_cancel_keeps_previous_evidence_and_cleans_only_unpublished_record() {
    let _guard = unsupported();
    let f = Fixture::new();
    f.draft().publish(&f.stop).unwrap();
    let path = paths(&f).pop().unwrap();
    let old = fs::read(&path).unwrap();
    let payload = fs::read(f.target()).unwrap();
    let err = f
        .draft()
        .publish_with(&f.stop, || {
            assert_eq!(paths(&f).len(), 2);
            f.stop.store(1, Ordering::Relaxed);
            Ok(())
        })
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Cancelled);
    assert_eq!(paths(&f), std::slice::from_ref(&path));
    assert_eq!(fs::read(path).unwrap(), old);
    assert_eq!(fs::read(f.target()).unwrap(), payload);
}
#[test]
fn no_xattr_directory_sync_warning_keeps_both_records_for_crash_rollback() {
    let _guard = unsupported();
    let f = Fixture::new();
    f.draft().publish(&f.stop).unwrap();
    let old = paths(&f).pop().unwrap();
    let receipt = f
        .draft()
        .publish_using(
            &f.stop,
            || Ok(()),
            |_| Err(std::io::Error::from_raw_os_error(libc::EIO)),
        )
        .unwrap();
    assert!(!receipt.directory_synced);
    assert_eq!(paths(&f).len(), 2);
    assert!(old.exists());
    assert!(f.draft().before.is_some());
}
#[test]
fn no_xattr_companion_tampering_symlink_hardlink_and_change_after_prepare_fail_closed() {
    for change in ["corrupt", "symlink", "hardlink", "delete", "replace"] {
        let _guard = unsupported();
        let f = Fixture::new();
        f.draft().publish(&f.stop).unwrap();
        let path = paths(&f).pop().unwrap();
        let bytes = fs::read(&path).unwrap();
        let payload = fs::read(f.target()).unwrap();
        let draft = f.draft();
        match change {
            "corrupt" => fs::write(&path, b"{}").unwrap(),
            "symlink" => {
                fs::remove_file(&path).unwrap();
                let decoy = f.dir.join("decoy.json");
                fs::write(&decoy, &bytes).unwrap();
                symlink(decoy, &path).unwrap();
            }
            "hardlink" => fs::hard_link(&path, f.dir.join("other.json")).unwrap(),
            "delete" => fs::remove_file(&path).unwrap(),
            _ => {
                let other = f.dir.join("other.json");
                fs::write(&other, bytes).unwrap();
                fs::rename(other, &path).unwrap();
            }
        }
        assert!(draft.publish(&f.stop).is_err(), "{change}");
        assert_eq!(fs::read(f.target()).unwrap(), payload);
    }
}
#[test]
fn only_enotsup_is_a_fallback_not_permission_io_or_space_errors() {
    for error in [
        libc::EACCES,
        libc::EPERM,
        libc::EIO,
        libc::ENOSPC,
        libc::EDQUOT,
    ] {
        let f = Fixture::new();
        let _guard = security::fault::inject(0, error);
        assert_eq!(f.draft().publish(&f.stop).unwrap_err().kind, ErrorKind::Io);
        assert!(!f.target().exists());
        assert!(paths(&f).is_empty());
    }
    for error in [libc::EACCES, libc::EIO] {
        let f = Fixture::new();
        f.draft().publish(&f.stop).unwrap();
        let _guard = security::fault::inject(error, 0);
        assert!(f
            .publisher
            .prepare(
                Arc::clone(&f.source),
                crate::jobdeck::color::Mode::Level,
                "3 red solid M 1 3",
                &f.stop
            )
            .is_err());
    }
}
#[test]
fn evidence_matches_payload_not_just_inode_and_stale_records_are_not_overwritten() {
    let _guard = unsupported();
    let f = Fixture::new();
    f.draft().publish(&f.stop).unwrap();
    let path = paths(&f).pop().unwrap();
    let bytes = fs::read(&path).unwrap();
    // In-place external payload edits must not inherit the previous binding.
    fs::write(f.target(), b"3 green solid N 1 3\n").unwrap();
    assert!(f
        .publisher
        .prepare(
            Arc::clone(&f.source),
            crate::jobdeck::color::Mode::Level,
            "3 red solid M 1 3",
            &f.stop
        )
        .is_err());
    assert_eq!(fs::read(path).unwrap(), bytes);

    let mut stage = Stage::create(Directory::open(&f.dir).unwrap(), &f.publisher).unwrap();
    stage.file.write_all(b"payload").unwrap();
    stage
        .set_owned(super::super::recovery::MARKER, b"marker")
        .unwrap();
    let name = name(c"new.layerprops", stage.file.metadata().unwrap().ino());
    let path = f.dir.join(std::ffi::OsStr::from_bytes(name.to_bytes()));
    fs::write(&path, b"do not overwrite").unwrap();
    assert!(stage
        .seal_evidence(c"new.layerprops", |_| Ok(()), &f.stop)
        .is_err());
    assert_eq!(fs::read(path).unwrap(), b"do not overwrite");
}
#[test]
fn evidence_path_is_checked_against_dynamic_registered_inputs() {
    let _guard = unsupported();
    let f = Fixture::new();
    let mut stage = Stage::create(Directory::open(&f.dir).unwrap(), &f.publisher).unwrap();
    stage.file.write_all(b"payload").unwrap();
    stage
        .set_owned(super::super::recovery::MARKER, b"marker")
        .unwrap();
    let record = name(c"new.layerprops", stage.file.metadata().unwrap().ino());
    let path = f.dir.join(std::ffi::OsStr::from_bytes(record.to_bytes()));
    let mut pending = f.publisher.sources.begin(&f.stop).unwrap();
    pending
        .protect_inputs(std::slice::from_ref(&path), &[], &f.stop)
        .unwrap();
    pending.commit(&f.stop).unwrap();
    assert!(stage
        .seal_evidence(c"new.layerprops", |p| f.publisher.protect(p), &f.stop)
        .is_err());
    assert!(!path.exists());
}
#[test]
fn no_xattr_sealed_payload_cannot_change_between_hash_and_commit() {
    let _guard = unsupported();
    let f = Fixture::new();
    let result = f.draft().publish_with(&f.stop, || {
        let stage = fs::read_dir(&f.dir)
            .unwrap()
            .map(|v| v.unwrap().path())
            .find(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(".floe-layerprops-")
            })
            .unwrap();
        fs::write(stage, b"3 green solid altered 1 3\n").unwrap();
        Ok(())
    });
    assert!(result.is_err());
    assert!(!f.target().exists());
    assert!(paths(&f).is_empty());
}
#[test]
fn no_xattr_ambiguous_link_error_preserves_both_payload_and_evidence() {
    let _guard = unsupported();
    let f = Fixture::new();
    let mut stage = Stage::create(Directory::open(&f.dir).unwrap(), &f.publisher).unwrap();
    stage.file.write_all(b"payload").unwrap();
    stage
        .set_owned(super::super::recovery::MARKER, b"marker")
        .unwrap();
    stage
        .seal_evidence(c"new.layerprops", |_| Ok(()), &f.stop)
        .unwrap();
    let tmp = f
        .dir
        .join(std::ffi::OsStr::from_bytes(stage.name().to_bytes()));
    let result = stage.commit_using(
        c"new.layerprops",
        false,
        || {},
        |_| {
            fs::hard_link(&tmp, f.dir.join("new.layerprops"))?;
            Err(std::io::Error::from_raw_os_error(libc::EIO))
        },
    );
    assert_eq!(result.unwrap_err().kind, ErrorKind::PublicationUnknown);
    drop(stage);
    assert!(tmp.exists());
    assert_eq!(
        fs::metadata(f.dir.join("new.layerprops")).unwrap().nlink(),
        2
    );
    assert_eq!(paths(&f).len(), 1);
}
