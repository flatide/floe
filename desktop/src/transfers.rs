//! Native downloads never interpret a web-supplied filename as a path, never
//! overwrite a destination, and publish only after WebKit reports completion.
use crate::download_fs as held;
use std::ffi::CString;
#[cfg(test)]
use std::fs::DirBuilder;
use std::fs::{self, File};
use std::io;
#[cfg(test)]
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub const MAX_BYTES: u64 = 512 * 1024 * 1024;

pub fn download_allowed(origin: &str, url: &str, method: &str) -> bool {
    if !floe_app::embedded::navigation_allowed(origin, &format!("{origin}/")) {
        return false;
    }
    if method == "GET" {
        return url
            .strip_prefix(&format!("blob:{origin}/"))
            .is_some_and(|id| {
                id.len() == 36
                    && id.bytes().enumerate().all(|(i, b)| {
                        if [8, 13, 18, 23].contains(&i) {
                            b == b'-'
                        } else {
                            b.is_ascii_hexdigit()
                        }
                    })
            });
    }
    if method != "POST" {
        return false;
    }
    let Some(path) = url.strip_prefix(origin) else {
        return false;
    };
    let id = [
        "/api/v1/artifacts/",
        "/api/v1/drc/review/notes/artifacts/",
        "/api/v1/drc/review/waives/artifacts/",
    ]
    .iter()
    .find_map(|prefix| path.strip_prefix(prefix))
    .and_then(|s| s.strip_suffix("/download"));
    id.is_some_and(|n| n.parse::<u64>().is_ok_and(|v| v > 0 && v.to_string() == n))
}

pub fn suggested_name(name: &str) -> String {
    // Only a suggestion; NSSavePanel owns the user's final destination choice.
    let safe: String = name
        .chars()
        .filter(|c| !c.is_control() && !"/\\:".contains(*c))
        .take(180)
        .collect();
    if safe.is_empty() || safe.starts_with('.') {
        "floe-export".into()
    } else {
        safe
    }
}

pub struct PendingFile {
    directory: PathBuf,
    parent_path: PathBuf,
    parent: File,
    stage: File,
    stage_name: CString,
    destination_name: CString,
    finalized: bool,
}
#[must_use]
pub struct Publication {
    // Cleanup is independent: a successfully published file is still saved
    // when removing the private staging directory fails. Never retry the save.
    pub publication: io::Result<()>,
    pub cleanup: io::Result<()>,
}
impl PendingFile {
    pub fn new(destination: &Path) -> io::Result<Self> {
        if !destination.is_absolute() || destination.file_name().is_none() {
            return Err(io::Error::other("select an absolute file destination"));
        }
        let parent_path = destination.parent().unwrap().canonicalize()?;
        let parent = held::open_directory(&parent_path)?;
        let destination_name = held::leaf(destination.file_name().unwrap())?;
        // Existing entries (including dangling symlinks) are checked through
        // the held parent. linkat repeats no-clobber atomically at publication.
        match held::stat_at(&parent, &destination_name) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            _ => {
                return Err(io::Error::other(
                    "choose a new filename; existing files are never replaced",
                ))
            }
        }
        let mut random = [0u8; 16];
        getrandom::fill(&mut random).map_err(io::Error::other)?;
        let suffix: String = random.iter().map(|b| format!("{b:02x}")).collect();
        let stage_name = held::leaf(std::ffi::OsStr::new(&format!(".floe-download-{suffix}")))?;
        let directory = parent_path.join(format!(".floe-download-{suffix}"));
        held::mkdir_at(&parent, &stage_name)?;
        let stage = held::open_at(&parent, &stage_name, true).inspect_err(|error| {
            // We cannot prove ownership if opening the just-created name fails.
            // Do not remove whatever now occupies that name as error cleanup.
            eprintln!("floe2-desktop: cannot bind private download directory ({:?}); inspect the selected folder", error.kind());
        })?;
        let stage_meta = stage.metadata()?;
        // SAFETY: geteuid only reads this process's effective uid. A replacement
        // owned by another uid or with non-private mode must not become staging.
        let uid = unsafe { libc::geteuid() };
        if stage_meta.uid() != uid || stage_meta.mode() & 0o077 != 0 {
            return Err(changed());
        }
        let pending = Self {
            directory,
            parent_path,
            parent,
            stage,
            stage_name,
            destination_name,
            finalized: false,
        };
        pending.validate_paths()?;
        Ok(pending)
    }
    pub fn staging(&self) -> PathBuf {
        self.directory.join("payload")
    }
    pub fn staging_for_download(&self) -> io::Result<PathBuf> {
        self.validate_paths()?;
        Ok(self.staging())
    }
    pub fn received_size(&self) -> io::Result<Option<u64>> {
        self.validate_paths()?;
        match held::stat_at(&self.stage, c"payload") {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
            Ok(entry) if entry.regular => entry.bytes.map(Some).ok_or_else(changed),
            Ok(_) => Err(changed()),
        }
    }
    fn validate_paths(&self) -> io::Result<()> {
        let same = |path: &Path, directory: &File| -> io::Result<bool> {
            let found = fs::symlink_metadata(path)?;
            let expected = directory.metadata()?;
            Ok(found.is_dir() && identity(&found) == identity(&expected))
        };
        if !same(&self.parent_path, &self.parent)? || !same(&self.directory, &self.stage)? {
            return Err(changed());
        }
        Ok(())
    }
    pub fn publish(mut self) -> Publication {
        let publication = self.publish_destination();
        let cleanup = self.cleanup();
        self.finalized = true;
        Publication {
            publication,
            cleanup,
        }
    }
    pub fn discard(mut self) -> io::Result<()> {
        let cleanup = self.cleanup();
        // A failed explicit cleanup is reported, not silently retried in Drop.
        self.finalized = true;
        cleanup
    }
    fn cleanup(&self) -> io::Result<()> {
        fn missing_is_ok(result: io::Result<()>) -> io::Result<()> {
            match result {
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                result => result,
            }
        }
        // Try only these two known targets. Unexpected directory contents or
        // a directory where the payload should be MUST survive, never recurse.
        let payload = missing_is_ok(held::unlink_at(&self.stage, c"payload", false));
        let directory = match held::stat_at(&self.parent, &self.stage_name) {
            Ok(entry) if entry.id == identity(&self.stage.metadata()?) => {
                held::unlink_at(&self.parent, &self.stage_name, true)
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                if held::directory_missing(&self.stage)? {
                    Ok(())
                } else {
                    Err(e) // Still linked elsewhere: moved, not cleaned.
                }
            }
            Err(e) => Err(e),
            Ok(_) => Err(changed()),
        };
        payload.and(directory)
    }
    fn publish_destination(&self) -> io::Result<()> {
        self.validate_paths()?;
        let file = held::open_at(&self.stage, c"payload", false)?;
        let meta = file.metadata()?;
        if !meta.is_file() || meta.nlink() != 1 || meta.len() > MAX_BYTES {
            return Err(io::Error::other("invalid or oversized download"));
        }
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        file.sync_all()?;
        self.validate_paths()?;
        if held::stat_at(&self.stage, c"payload")?.id != identity(&meta) {
            return Err(changed());
        }
        held::link_at(
            &self.stage,
            c"payload",
            &self.parent,
            &self.destination_name,
        )?;
        // An error after linking is an uncertain durability outcome, not an
        // invitation to replay. The UI tells the user to check the destination.
        self.parent.sync_all()?;
        self.validate_paths()?;
        if held::stat_at(&self.parent, &self.destination_name)?.id != identity(&meta) {
            return Err(changed());
        }
        Ok(())
    }
}
fn identity(meta: &fs::Metadata) -> (u64, u64) {
    (meta.dev(), meta.ino())
}
fn changed() -> io::Error {
    io::Error::other("download directory or payload changed; explicit retry required")
}
impl Drop for PendingFile {
    fn drop(&mut self) {
        // Last-resort RAII for unwinding/abandoned setup. Normal host paths use
        // discard/publish so they can preserve a visible warning until exit.
        if !self.finalized {
            if let Err(error) = self.cleanup() {
                eprintln!("floe2-desktop: private download cleanup incomplete ({:?}); inspect the selected download folder; no recursive cleanup attempted", error.kind());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn private_root() -> PathBuf {
        let mut random = [0u8; 16];
        getrandom::fill(&mut random).unwrap();
        let suffix: String = random.iter().map(|b| format!("{b:02x}")).collect();
        let root = std::env::temp_dir().join(format!("floe-cleanup-test-{suffix}"));
        DirBuilder::new().mode(0o700).create(&root).unwrap();
        root
    }
    #[test]
    fn moved_staging_without_replacement_is_not_reported_clean() {
        let root = private_root();
        let pending = PendingFile::new(&root.join("result")).unwrap();
        let moved = root.join("moved-stage");
        fs::write(pending.staging(), b"partial").unwrap();
        fs::rename(pending.staging().parent().unwrap(), &moved).unwrap();
        let cleanup = pending.discard();
        let partial_removed = !moved.join("payload").exists();
        let _ = fs::remove_file(moved.join("payload"));
        fs::remove_dir(&moved).unwrap();
        fs::remove_dir(&root).unwrap();
        assert!(cleanup.is_err(), "moved directory is still present");
        assert!(
            partial_removed,
            "owned payload is removed through held directory"
        );
    }
    #[test]
    fn cleanup_path_replacement_keeps_foreign_payload() {
        let root = private_root();
        let foreign = root.join("foreign");
        fs::create_dir(&foreign).unwrap();
        fs::write(foreign.join("payload"), b"foreign sentinel").unwrap();
        let pending = PendingFile::new(&root.join("result")).unwrap();
        let old = pending.staging().parent().unwrap().to_path_buf();
        fs::write(pending.staging(), b"owned partial").unwrap();
        let moved = root.join("moved-stage");
        fs::rename(&old, &moved).unwrap();
        std::os::unix::fs::symlink(&foreign, &old).unwrap();
        let result = pending.discard();
        let sentinel = fs::read(foreign.join("payload")).ok();
        let owned_removed = !moved.join("payload").exists();
        // All paths below belong to this new synthetic fixture, not production
        // cleanup. Teardown runs before assertions, including on the old bug.
        fs::remove_file(&old).unwrap();
        let _ = fs::remove_file(moved.join("payload"));
        fs::remove_dir(&moved).unwrap();
        let _ = fs::remove_file(foreign.join("payload"));
        fs::remove_dir(&foreign).unwrap();
        fs::remove_dir(&root).unwrap();
        assert_eq!(sentinel.as_deref(), Some(b"foreign sentinel".as_slice()));
        assert!(
            owned_removed,
            "cleanup must use the original directory handle"
        );
        assert!(
            result.is_err(),
            "replacement name must remain untouched and be reported"
        );
    }
    #[test]
    fn publish_parent_replacement_keeps_foreign_destination() {
        let root = private_root();
        let chosen = root.join("chosen");
        let foreign = root.join("foreign");
        fs::create_dir(&chosen).unwrap();
        fs::create_dir(&foreign).unwrap();
        let pending = PendingFile::new(&chosen.join("result")).unwrap();
        let stage_name = pending
            .staging()
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_owned();
        fs::write(pending.staging(), b"owned download").unwrap();
        let moved = root.join("chosen-moved");
        fs::rename(&chosen, &moved).unwrap();
        std::os::unix::fs::symlink(&foreign, &chosen).unwrap();
        let impostor = foreign.join(&stage_name);
        fs::create_dir(&impostor).unwrap();
        fs::write(impostor.join("payload"), b"foreign download").unwrap();
        let delivery_rejected = pending.staging_for_download().is_err();
        let size_rejected = pending.received_size().is_err();
        let outcome = pending.publish();
        let sentinel = fs::read(impostor.join("payload")).ok();
        let foreign_published = foreign.join("result").exists();
        let original_cleaned = !moved.join(&stage_name).exists();
        fs::remove_file(&chosen).unwrap();
        let _ = fs::remove_file(impostor.join("payload"));
        let _ = fs::remove_dir(&impostor);
        let _ = fs::remove_file(foreign.join("result"));
        fs::remove_dir(&foreign).unwrap();
        let _ = fs::remove_file(moved.join(&stage_name).join("payload"));
        let _ = fs::remove_dir(moved.join(&stage_name));
        fs::remove_dir(&moved).unwrap();
        fs::remove_dir(&root).unwrap();
        assert!(
            outcome.publication.is_err(),
            "a changed chosen path must refuse publication"
        );
        assert_eq!(sentinel.as_deref(), Some(b"foreign download".as_slice()));
        assert!(!foreign_published);
        assert!(original_cleaned);
        assert!(delivery_rejected && size_rejected);
        outcome.cleanup.unwrap();
    }
    #[test]
    fn linked_payload_never_changes_external_contents_or_permissions() {
        for hard in [false, true] {
            let root = private_root();
            let outside = root.join("external");
            fs::write(&outside, b"external bytes").unwrap();
            fs::set_permissions(&outside, fs::Permissions::from_mode(0o640)).unwrap();
            let dest = root.join("result");
            let pending = PendingFile::new(&dest).unwrap();
            if hard {
                fs::hard_link(&outside, pending.staging()).unwrap();
            } else {
                std::os::unix::fs::symlink(&outside, pending.staging()).unwrap();
            }
            let outcome = pending.publish();
            assert!(outcome.publication.is_err());
            outcome.cleanup.unwrap();
            assert_eq!(fs::read(&outside).unwrap(), b"external bytes");
            assert_eq!(
                fs::metadata(&outside).unwrap().permissions().mode() & 0o777,
                0o640
            );
            assert!(!dest.exists());
            fs::remove_file(&outside).unwrap();
            fs::remove_dir(&root).unwrap();
        }
    }
    #[test]
    fn publication_and_cleanup_are_independent_even_on_destination_conflict() {
        for conflict in [false, true] {
            let root = private_root();
            let dest = root.join("result");
            let pending = PendingFile::new(&dest).unwrap();
            let stage = pending.staging();
            let directory = stage.parent().unwrap().to_path_buf();
            let obstacle = directory.join("unknown-entry");
            fs::write(&stage, b"new download").unwrap();
            fs::write(&obstacle, b"preserve me").unwrap();
            if conflict {
                fs::write(&dest, b"existing download").unwrap();
            }
            let outcome = pending.publish();
            assert_eq!(outcome.publication.is_err(), conflict);
            assert_eq!(
                outcome.cleanup.unwrap_err().kind(),
                io::ErrorKind::DirectoryNotEmpty
            );
            assert!(!stage.exists());
            assert_eq!(fs::read(&obstacle).unwrap(), b"preserve me");
            assert_eq!(
                fs::read(&dest).unwrap(),
                if conflict {
                    b"existing download".as_slice()
                } else {
                    b"new download".as_slice()
                }
            );
            // Test teardown knows this entry; production cleanup must not.
            fs::remove_file(&obstacle).unwrap();
            fs::remove_dir(&directory).unwrap();
            fs::remove_file(&dest).unwrap();
            fs::remove_dir(&root).unwrap();
        }
    }
    #[test]
    fn directory_payload_is_never_recursively_removed() {
        let root = private_root();
        let dest = root.join("result");
        let pending = PendingFile::new(&dest).unwrap();
        let stage = pending.staging();
        let directory = stage.parent().unwrap().to_path_buf();
        fs::create_dir(&stage).unwrap();
        let child = stage.join("unknown-child");
        fs::write(&child, b"preserve this file").unwrap();
        assert!(pending.discard().is_err());
        assert_eq!(fs::read(&child).unwrap(), b"preserve this file");
        assert!(!dest.exists());
        fs::remove_file(&child).unwrap();
        fs::remove_dir(&stage).unwrap();
        fs::remove_dir(&directory).unwrap();
        fs::remove_dir(&root).unwrap();
    }
    #[test]
    fn missing_staging_payload_and_directory_are_already_clean() {
        let root = private_root();
        for removed in [false, true] {
            let pending = PendingFile::new(&root.join("result")).unwrap();
            let directory = pending.staging().parent().unwrap().to_path_buf();
            if removed {
                fs::remove_dir(&directory).unwrap();
            }
            pending.discard().unwrap();
            assert!(!directory.exists());
            assert!(!root.join("result").exists());
        }
        fs::remove_dir(&root).unwrap();
    }
    #[test]
    fn only_owned_blobs_and_exact_post_download_routes() {
        let origin = "http://127.0.0.1:12345";
        let blob = format!("blob:{origin}/12345678-abcd-1234-1234-123456789abc");
        assert!(download_allowed(origin, &blob, "GET"));
        for path in [
            "/api/v1/artifacts/1/download",
            "/api/v1/drc/review/notes/artifacts/23/download",
            "/api/v1/drc/review/waives/artifacts/1/download",
        ] {
            assert!(download_allowed(origin, &format!("{origin}{path}"), "POST"));
            assert!(!download_allowed(origin, &format!("{origin}{path}"), "GET"));
        }
        for tail in [
            "0/download",
            "01/download",
            "../1/download",
            "1/download?x=1",
            "1/download#x",
            "18446744073709551616/download",
        ] {
            assert!(!download_allowed(
                origin,
                &format!("{origin}/api/v1/artifacts/{tail}"),
                "POST"
            ));
        }
        for url in [
            "file:///tmp/x",
            "data:text/plain,x",
            "https://example.com/",
            "blob:http://127.0.0.1:23456/12345678-abcd-1234-1234-123456789abc",
            "http://127.0.0.1:12345@evil/api/v1/artifacts/1/download",
        ] {
            assert!(!download_allowed(origin, url, "GET"));
            assert!(!download_allowed(origin, url, "POST"));
        }
        assert!(!download_allowed(
            "https://example.com",
            "blob:https://example.com/12345678-abcd-1234-1234-123456789abc",
            "GET"
        ));
    }
    #[test]
    fn filename_is_not_a_path() {
        assert_eq!(suggested_name("../../secret"), "floe-export");
        assert_eq!(suggested_name("한글/파일\n.png"), "한글파일.png");
    }
    #[test]
    fn incomplete_or_racing_download_never_clobbers() {
        let root = std::env::temp_dir().join(format!("floe-transfer-test-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let dest = root.join("result");
        let pending = PendingFile::new(&dest).unwrap();
        let stage = pending.staging();
        fs::write(&stage, b"partial").unwrap();
        drop(pending);
        assert!(!dest.exists());
        assert!(!stage.exists());
        let pending = PendingFile::new(&dest).unwrap();
        fs::write(pending.staging(), b"new").unwrap();
        fs::write(&dest, b"existing").unwrap();
        let outcome = pending.publish();
        assert!(outcome.publication.is_err());
        outcome.cleanup.unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"existing");
        assert!(PendingFile::new(&dest).is_err());
        fs::remove_file(&dest).unwrap();
        let pending = PendingFile::new(&dest).unwrap();
        fs::write(pending.staging(), b"complete").unwrap();
        let outcome = pending.publish();
        outcome.publication.unwrap();
        outcome.cleanup.unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"complete");
        assert_eq!(
            fs::metadata(&dest).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::remove_file(&dest).unwrap();
        let pending = PendingFile::new(&dest).unwrap();
        File::create(pending.staging())
            .unwrap()
            .set_len(MAX_BYTES + 1)
            .unwrap();
        let outcome = pending.publish();
        assert!(outcome.publication.is_err());
        outcome.cleanup.unwrap();
        assert!(!dest.exists(), "oversized transfer was published");
        std::os::unix::fs::symlink(root.join("absent"), &dest).unwrap();
        assert!(PendingFile::new(&dest).is_err());
        fs::remove_file(&dest).unwrap();
        fs::remove_dir(&root).unwrap();
    }
}
