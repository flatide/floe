//! Native downloads never interpret a web-supplied filename as a path, never
//! overwrite a destination, and publish only after WebKit reports completion.
use std::fs::{self, DirBuilder, File};
use std::io;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
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
    destination: PathBuf,
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
        // Existing files AND dangling symlinks are protected. hard_link below
        // repeats the no-clobber guarantee atomically at publication time.
        match fs::symlink_metadata(destination) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            _ => {
                return Err(io::Error::other(
                    "choose a new filename; existing files are never replaced",
                ))
            }
        }
        let parent = destination.parent().unwrap().canonicalize()?;
        let mut random = [0u8; 16];
        getrandom::fill(&mut random).map_err(io::Error::other)?;
        let suffix: String = random.iter().map(|b| format!("{b:02x}")).collect();
        let directory = parent.join(format!(".floe-download-{suffix}"));
        DirBuilder::new().mode(0o700).create(&directory)?;
        Ok(Self {
            destination: parent.join(destination.file_name().unwrap()),
            directory,
            finalized: false,
        })
    }
    pub fn staging(&self) -> PathBuf {
        self.directory.join("payload")
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
        let payload = missing_is_ok(fs::remove_file(self.staging()));
        let directory = missing_is_ok(fs::remove_dir(&self.directory));
        payload.and(directory)
    }
    fn publish_destination(&self) -> io::Result<()> {
        let path = self.staging();
        let meta = fs::symlink_metadata(&path)?;
        if !meta.file_type().is_file() || meta.len() > MAX_BYTES {
            return Err(io::Error::other("invalid or oversized download"));
        }
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        File::open(&path)?.sync_all()?;
        fs::hard_link(&path, &self.destination)?;
        // An error after linking is an uncertain durability outcome, not an
        // invitation to replay. The UI tells the user to check the destination.
        File::open(self.destination.parent().unwrap())?.sync_all()
    }
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
