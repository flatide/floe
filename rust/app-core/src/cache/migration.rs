//! Explicit index-only name migration. Readers only use names::select.
use crate::{
    check_cancelled,
    layer_defaults::{identity, leaf, Directory, Stamp},
    Error, ErrorKind, Result,
};
use serde::Serialize;
use std::{
    ffi::CStr,
    fs::Metadata,
    io::Write,
    os::{fd::AsRawFd, unix::fs::MetadataExt},
    path::Path,
    sync::atomic::AtomicUsize,
};

/// A completed name change is independent of a later rebuild's outcome.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Migration {
    pub directory_synced: bool,
}

/// Caller owns both alias write leases and has validated the selected input.
/// Never copy, overwrite a destination, silently fall back, or roll back a
/// completed rename. Uncoordinated external source replacement is unsupported.
pub(crate) fn rename_legacy(
    old: &Path,
    new: &Path,
    expected: &Metadata,
    directory: bool,
    stop: &AtomicUsize,
) -> Result<Migration> {
    check_cancelled(stop)?;
    let parent = old.parent().filter(|p| Some(*p) == new.parent());
    let parent = parent.ok_or_else(|| Error::input("migration requires sibling names"))?;
    if old == new {
        return Err(Error::input("migration requires distinct names"));
    }
    let from = leaf(
        old.file_name()
            .ok_or_else(|| Error::input("missing old name"))?,
    )?;
    let to = leaf(
        new.file_name()
            .ok_or_else(|| Error::input("missing new name"))?,
    )?;
    let dir = Directory::open(parent)?;
    let file = dir.open_leaf(
        &from,
        libc::O_RDONLY | if directory { libc::O_DIRECTORY } else { 0 },
        0,
    )?;
    let actual = file.metadata()?;
    if !(if directory {
        actual.is_dir()
    } else {
        actual.is_file() && actual.nlink() == 1
    }) {
        return Err(Error::input(
            "migration source has an invalid file type or link count",
        ));
    }
    dir.validate()?;
    if Stamp::of(&actual) != Stamp::of(expected) || dir.leaf_identity(&from)? != identity(expected)
    {
        return Err(Error::new(
            ErrorKind::Cache,
            "legacy cache changed before migration",
        ));
    }
    check_cancelled(stop)?;
    rename_exclusive(dir.file.as_raw_fd(), &from, &to)?;
    // Rename already committed. A directory-sync error is a durability warning,
    // not a failed/rolled-back migration; report it even if a rebuild fails later.
    let result = Migration {
        directory_synced: dir.file.sync_all().is_ok(),
    };
    let _ = writeln!(
        std::io::stderr(),
        "[floe2-web] renamed legacy cache: {} -> {}",
        old.display(),
        new.display()
    );
    if !result.directory_synced {
        let _ = writeln!(
            std::io::stderr(),
            "[floe2-web] cache renamed, but directory sync failed"
        );
    }
    Ok(result)
}

fn rename_exclusive(fd: i32, from: &CStr, to: &CStr) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    // SAFETY: live directory fd and validated NUL-terminated leaf names.
    let rc = unsafe { libc::renameatx_np(fd, from.as_ptr(), fd, to.as_ptr(), libc::RENAME_EXCL) };
    #[cfg(target_os = "linux")]
    // SAFETY: same invariants; syscall avoids adding a newer glibc dependency.
    let rc = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            fd,
            from.as_ptr(),
            fd,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    return if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    };
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (fd, from, to);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "atomic no-replace rename unavailable",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    struct Root(PathBuf);
    impl Root {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!(
                "floe-name-migration-{}-{}",
                std::process::id(),
                SERIAL.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Root {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    #[test]
    fn rename_preserves_data_identity_and_modified_time() {
        for directory in [false, true] {
            let root = Root::new();
            let old = root.0.join("old 한 글");
            let new = root.0.join(".new 한 글");
            if directory {
                fs::create_dir(&old).unwrap();
            }
            let content = |p: &Path| {
                if directory {
                    p.join("sentinel")
                } else {
                    p.to_owned()
                }
            };
            fs::write(content(&old), b"unchanged bytes").unwrap();
            let before = fs::symlink_metadata(&old).unwrap();
            let inner = fs::metadata(content(&old)).unwrap();
            rename_legacy(&old, &new, &before, directory, &AtomicUsize::new(0)).unwrap();
            assert!(!old.exists());
            let after = fs::symlink_metadata(&new).unwrap();
            assert_eq!(identity(&before), identity(&after));
            assert_eq!(before.modified().unwrap(), after.modified().unwrap());
            assert_eq!(
                identity(&inner),
                identity(&fs::metadata(content(&new)).unwrap())
            );
            assert_eq!(fs::read(content(&new)).unwrap(), b"unchanged bytes");
        }
    }
    #[test]
    fn cancellation_collision_and_source_replacement_preserve_inputs() {
        let root = Root::new();
        let old = root.0.join("old");
        let new = root.0.join("new");
        fs::write(&old, b"old bytes").unwrap();
        let before = fs::symlink_metadata(&old).unwrap();
        assert_eq!(
            rename_legacy(&old, &new, &before, false, &AtomicUsize::new(1))
                .unwrap_err()
                .kind,
            ErrorKind::Cancelled
        );
        assert!(!new.exists());
        // Destination may appear after caller validation: the syscall must
        // reject it atomically, including a dangling symlink or empty directory.
        for kind in 0..3 {
            match kind {
                0 => fs::write(&new, b"other writer").unwrap(),
                1 => std::os::unix::fs::symlink(root.0.join("missing"), &new).unwrap(),
                _ => fs::create_dir(&new).unwrap(),
            }
            let destination = fs::symlink_metadata(&new).unwrap();
            assert!(rename_legacy(&old, &new, &before, false, &AtomicUsize::new(0)).is_err());
            assert_eq!(
                identity(&destination),
                identity(&fs::symlink_metadata(&new).unwrap())
            );
            assert_eq!(fs::read(&old).unwrap(), b"old bytes");
            if kind == 2 {
                fs::remove_dir(&new).unwrap();
            } else {
                fs::remove_file(&new).unwrap();
            }
        }
        fs::rename(&old, root.0.join("original")).unwrap();
        fs::write(&old, b"replacement").unwrap();
        assert!(rename_legacy(&old, &new, &before, false, &AtomicUsize::new(0)).is_err());
        assert_eq!(fs::read(&old).unwrap(), b"replacement");
        assert!(!new.exists());
    }
    #[test]
    fn aliases_and_special_files_are_not_migrated() {
        let root = Root::new();
        let target = root.0.join("target");
        let old = root.0.join("old");
        let new = root.0.join("new");
        fs::write(&target, b"target bytes").unwrap();
        std::os::unix::fs::symlink(&target, &old).unwrap();
        assert!(rename_legacy(
            &old,
            &new,
            &fs::symlink_metadata(&old).unwrap(),
            false,
            &AtomicUsize::new(0)
        )
        .is_err());
        fs::remove_file(&old).unwrap();
        fs::hard_link(&target, &old).unwrap();
        assert!(rename_legacy(
            &old,
            &new,
            &fs::symlink_metadata(&old).unwrap(),
            false,
            &AtomicUsize::new(0)
        )
        .is_err());
        fs::remove_file(&old).unwrap();
        fs::create_dir(&old).unwrap();
        assert!(rename_legacy(
            &old,
            &new,
            &fs::symlink_metadata(&old).unwrap(),
            false,
            &AtomicUsize::new(0)
        )
        .is_err());
        assert!(!new.exists());
        fs::remove_dir(&old).unwrap();
        let _socket = std::os::unix::net::UnixListener::bind(&old).unwrap();
        assert!(rename_legacy(
            &old,
            &new,
            &fs::symlink_metadata(&old).unwrap(),
            false,
            &AtomicUsize::new(0)
        )
        .is_err());
        assert!(!new.exists());
        assert_eq!(fs::read(target).unwrap(), b"target bytes");
    }
}
