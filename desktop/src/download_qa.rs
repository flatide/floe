//! Isolated download fixture. No caller filename or scope is used.
use crate::transfers::PendingFile;
use std::cell::RefCell;
use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::PathBuf;

const COMPLETE: &[u8] = b"Previously completed synthetic download";
const PARTIAL: &[u8] = b"Synthetic partial staging bytes";
const OBSTACLE: &[u8] = b"Do not recursively remove unknown staging contents";
const PUBLISHED: &[u8] = b"Synthetic download cancellation QA";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Cancel,
    Publish,
    CleanupFailure,
}
impl Mode {
    pub fn expected_cleanup_error(self) -> Option<io::ErrorKind> {
        match self {
            Self::Cancel | Self::Publish => None,
            Self::CleanupFailure => Some(io::ErrorKind::DirectoryNotEmpty),
        }
    }
}
pub fn requested(args: &[String]) -> Option<Mode> {
    if args == ["--smoke-test-download-cancel"] {
        Some(Mode::Cancel)
    } else if args == ["--smoke-test-download-publish"] {
        Some(Mode::Publish)
    } else if args == ["--smoke-test-download-cleanup-failure"] {
        Some(Mode::CleanupFailure)
    } else {
        None
    }
}

pub struct Fixture {
    root: PathBuf,
    stage: RefCell<Option<PathBuf>>,
    pub mode: Mode,
}
impl Fixture {
    pub fn create(mode: Mode) -> io::Result<Self> {
        let mut random = [0u8; 16];
        getrandom::fill(&mut random).map_err(io::Error::other)?;
        let suffix: String = random.iter().map(|b| format!("{b:02x}")).collect();
        let root = std::env::temp_dir().join(format!("floe-download-cancel-{suffix}"));
        DirBuilder::new().mode(0o700).create(&root)?;
        let fixture = Self {
            root,
            stage: RefCell::new(None),
            mode,
        };
        fs::write(fixture.root.join("completed.txt"), COMPLETE)?;
        Ok(fixture)
    }
    pub fn pending(&self) -> io::Result<PendingFile> {
        if self.stage.borrow().is_some() {
            return Err(io::Error::other("download QA already prepared"));
        }
        let pending = PendingFile::new(&self.destination())?;
        if self.mode != Mode::Publish {
            fs::write(pending.staging(), PARTIAL)?;
        }
        *self.stage.borrow_mut() = Some(pending.staging());
        if self.mode == Mode::CleanupFailure {
            fs::write(
                pending.staging().parent().unwrap().join("qa-obstacle.txt"),
                OBSTACLE,
            )?;
        }
        Ok(pending)
    }
    pub fn intact(&self, completed: bool) -> bool {
        let stage = self.stage.borrow();
        stage.as_ref().is_some_and(|stage| {
            if self.mode == Mode::Publish {
                return (if completed {
                    !stage.exists()
                        && !stage.parent().unwrap().exists()
                        && fs::symlink_metadata(self.destination())
                            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o777 == 0o600)
                        && fs::read(self.destination()).is_ok_and(|bytes| bytes == PUBLISHED)
                } else {
                    stage.parent().unwrap().is_dir() && !self.destination().exists()
                }) && fs::read(self.root.join("completed.txt"))
                    .is_ok_and(|bytes| bytes == COMPLETE);
            }
            let obstacle = stage.parent().unwrap().join("qa-obstacle.txt");
            let expected_directory = if self.mode == Mode::CleanupFailure {
                fs::read(&obstacle).is_ok_and(|bytes| bytes == OBSTACLE)
            } else {
                !stage.parent().unwrap().exists()
            };
            (if completed {
                !stage.exists() && expected_directory
            } else {
                fs::read(stage).is_ok_and(|bytes| bytes == PARTIAL)
            }) && (self.mode != Mode::CleanupFailure || expected_directory)
                && !self.root.join("cancelled.txt").exists()
                && fs::read(self.root.join("completed.txt")).is_ok_and(|bytes| bytes == COMPLETE)
        })
    }
    fn destination(&self) -> PathBuf {
        self.root.join(if self.mode == Mode::Publish {
            "downloaded.txt"
        } else {
            "cancelled.txt"
        })
    }
    pub fn teardown(&self) -> io::Result<()> {
        fn absent_ok(result: io::Result<()>) -> io::Result<()> {
            match result {
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                result => result,
            }
        }
        // Native QA calls this explicitly before returning. Objective-C may
        // retain the host beyond run(), so Drop alone is not a teardown proof.
        if self.mode == Mode::CleanupFailure {
            if let Some(stage) = self.stage.borrow().as_ref() {
                let directory = stage.parent().unwrap();
                absent_ok(fs::remove_file(directory.join("qa-obstacle.txt")))?;
                absent_ok(fs::remove_dir(directory))?;
            }
        }
        if self.mode == Mode::Publish {
            absent_ok(fs::remove_file(self.destination()))?;
        }
        absent_ok(fs::remove_file(self.root.join("completed.txt")))?;
        absent_ok(fs::remove_dir(&self.root))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        // Only fixed files in this freshly created private fixture; no recursion.
        let _ = self.teardown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_only_removes_its_pending_file() {
        let fixture = Fixture::create(Mode::Cancel).unwrap();
        let pending = fixture.pending().unwrap();
        assert!(fixture.intact(false));
        assert!(fixture.pending().is_err());
        pending.discard().unwrap();
        assert!(fixture.intact(true));
        fixture.teardown().unwrap();
        assert!(!fixture.root.exists());
    }
    #[test]
    fn cleanup_obstacle_is_reported_without_removing_unknown_contents() {
        let fixture = Fixture::create(Mode::CleanupFailure).unwrap();
        let pending = fixture.pending().unwrap();
        assert!(fixture.intact(false));
        assert_eq!(
            pending.discard().unwrap_err().kind(),
            io::ErrorKind::DirectoryNotEmpty
        );
        assert!(fixture.intact(true));
        fixture.teardown().unwrap();
        assert!(!fixture.root.exists());
        fixture.teardown().unwrap();
    }
    #[test]
    fn publication_fixture_requires_the_exact_blob_and_private_output() {
        let fixture = Fixture::create(Mode::Publish).unwrap();
        let pending = fixture.pending().unwrap();
        assert!(!pending.staging().exists());
        assert!(fixture.intact(false));
        fs::write(pending.staging(), PUBLISHED).unwrap();
        let result = pending.publish();
        result.publication.unwrap();
        result.cleanup.unwrap();
        assert!(fixture.intact(true));
        fixture.teardown().unwrap();
        assert!(!fixture.root.exists());
    }
    #[test]
    fn caller_paths_cannot_enter_download_qa() {
        for (flag, mode) in [
            ("--smoke-test-download-cancel", Mode::Cancel),
            ("--smoke-test-download-publish", Mode::Publish),
            (
                "--smoke-test-download-cleanup-failure",
                Mode::CleanupFailure,
            ),
        ] {
            assert_eq!(requested(&[flag.into()]), Some(mode));
            for args in [
                vec!["view", flag],
                vec![flag, "user.oas"],
                vec![flag, "--root", "/"],
            ] {
                assert!(
                    requested(&args.into_iter().map(String::from).collect::<Vec<_>>()).is_none()
                );
            }
        }
    }
}
