//! Isolated download cancellation fixture. No caller filename or scope is used.
use crate::transfers::PendingFile;
use std::cell::RefCell;
use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;

const COMPLETE: &[u8] = b"Previously completed synthetic download";
const PARTIAL: &[u8] = b"Synthetic partial staging bytes";

pub fn requested(args: &[String]) -> bool {
    args == ["--smoke-test-download-cancel"]
}

pub struct Fixture {
    root: PathBuf,
    stage: RefCell<Option<PathBuf>>,
}
impl Fixture {
    pub fn create() -> io::Result<Self> {
        let mut random = [0u8; 16];
        getrandom::fill(&mut random).map_err(io::Error::other)?;
        let suffix: String = random.iter().map(|b| format!("{b:02x}")).collect();
        let root = std::env::temp_dir().join(format!("floe-download-cancel-{suffix}"));
        DirBuilder::new().mode(0o700).create(&root)?;
        let fixture = Self {
            root,
            stage: RefCell::new(None),
        };
        fs::write(fixture.root.join("completed.txt"), COMPLETE)?;
        Ok(fixture)
    }
    pub fn pending(&self) -> io::Result<PendingFile> {
        if self.stage.borrow().is_some() {
            return Err(io::Error::other("download QA already prepared"));
        }
        let pending = PendingFile::new(&self.root.join("cancelled.txt"))?;
        fs::write(pending.staging(), PARTIAL)?;
        *self.stage.borrow_mut() = Some(pending.staging());
        Ok(pending)
    }
    pub fn intact(&self, cancelled: bool) -> bool {
        let stage = self.stage.borrow();
        stage.as_ref().is_some_and(|stage| {
            (if cancelled {
                !stage.exists() && !stage.parent().unwrap().exists()
            } else {
                fs::read(stage).is_ok_and(|bytes| bytes == PARTIAL)
            }) && !self.root.join("cancelled.txt").exists()
                && fs::read(self.root.join("completed.txt")).is_ok_and(|bytes| bytes == COMPLETE)
        })
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        // Only fixed files in this freshly created private fixture; no recursion.
        let _ = fs::remove_file(self.root.join("completed.txt"));
        let _ = fs::remove_dir(&self.root);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_only_removes_its_pending_file() {
        let fixture = Fixture::create().unwrap();
        let pending = fixture.pending().unwrap();
        assert!(fixture.intact(false));
        assert!(fixture.pending().is_err());
        drop(pending);
        assert!(fixture.intact(true));
    }
    #[test]
    fn caller_paths_cannot_enter_download_qa() {
        let flag = "--smoke-test-download-cancel";
        assert!(requested(&[flag.into()]));
        for args in [
            vec!["view", flag],
            vec![flag, "user.oas"],
            vec![flag, "--root", "/"],
        ] {
            assert!(!requested(
                &args.into_iter().map(String::from).collect::<Vec<_>>()
            ));
        }
    }
}
