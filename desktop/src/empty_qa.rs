//! Explicit empty-workspace QA must not inherit the caller's browsing scope.
use floe_app::embedded::Session;
use floe_app_core::{Error, Result};
use std::{
    fs::{self, DirBuilder},
    os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt},
    path::PathBuf,
};

pub struct Scope {
    root: PathBuf,
    device: u64,
    inode: u64,
}

impl Scope {
    pub fn prepare(session: &mut Session, smoke: bool) -> Result<Option<Self>> {
        // Normal native launches still ask the user; explicit layout/review
        // fixtures retain their own scope. Never add a second live root.
        if !smoke || !session.needs_initial_directory() {
            return Ok(None);
        }
        let mut random = [0u8; 16];
        getrandom::fill(&mut random).map_err(|_| Error::input("empty QA scope entropy failed"))?;
        let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let root = std::env::temp_dir()
            .canonicalize()?
            .join(format!("floe-native-empty-{suffix}"));
        DirBuilder::new().mode(0o700).create(&root)?;
        let metadata = fs::symlink_metadata(&root)?;
        session.set_initial_directory(&root)?;
        // Like the other native QA fixtures, retain this small folder on
        // success/failure for inspection. No credential is stored here.
        eprintln!("[desktop-empty-qa] synthetic scope: {}", root.display());
        Ok(Some(Self {
            root,
            device: metadata.dev(),
            inode: metadata.ino(),
        }))
    }

    pub fn verify(&self) -> Result<()> {
        let metadata = fs::symlink_metadata(&self.root)?;
        if !metadata.is_dir()
            || metadata.dev() != self.device
            || metadata.ino() != self.inode
            || metadata.permissions().mode() & 0o7777 != 0o700
            || fs::read_dir(&self.root)?.next().is_some()
        {
            return Err(Error::input("empty native QA scope changed"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_launch_and_explicit_scopes_are_unchanged() {
        let mut normal = Session::parse(&[]).unwrap();
        assert!(Scope::prepare(&mut normal, false).unwrap().is_none());
        assert!(normal.needs_initial_directory());
        for args in [vec!["--root", "/tmp"], vec!["/tmp/synthetic.oas"]] {
            let mut explicit =
                Session::parse(&args.into_iter().map(String::from).collect::<Vec<_>>()).unwrap();
            assert!(Scope::prepare(&mut explicit, true).unwrap().is_none());
            assert!(!explicit.needs_initial_directory());
        }
    }

    #[test]
    fn empty_qa_gets_one_new_private_root_and_never_cleans_unexpected_files() {
        let mut session = Session::parse(&[]).unwrap();
        let scope = Scope::prepare(&mut session, true).unwrap().unwrap();
        assert!(!session.needs_initial_directory());
        assert!(Scope::prepare(&mut session, true).unwrap().is_none());
        assert!(session
            .set_initial_directory(std::path::Path::new("/tmp"))
            .is_err());
        scope.verify().unwrap();
        let root = scope.root.clone();
        let sentinel = root.join("unexpected");
        fs::write(&sentinel, b"preserve").unwrap();
        assert!(scope.verify().is_err());
        drop(scope);
        assert_eq!(fs::read(&sentinel).unwrap(), b"preserve");
        fs::remove_file(sentinel).unwrap();
        fs::remove_dir(root).unwrap();
    }

    #[test]
    fn root_directory_working_directory_does_not_become_the_scope() {
        // Process isolation: do not change the parallel Rust tests' cwd.
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "empty_qa::tests::root_cwd_child",
                "--ignored",
                "--nocapture",
            ])
            .env("FLOE_EMPTY_QA_CHILD", "1")
            .current_dir("/")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let started = std::time::Instant::now();
        while child.try_wait().unwrap().is_none() {
            if started.elapsed() > std::time::Duration::from_secs(30) {
                let _ = child.kill();
                let _ = child.wait();
                panic!("isolated empty QA scope child timed out");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        // Fixed small child output; no browser, worker or inherited writer.
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("EMPTY QA ROOT CWD: OK"));
    }

    #[test]
    #[ignore = "isolated child of root_directory_working_directory_does_not_become_the_scope"]
    fn root_cwd_child() {
        assert_eq!(std::env::var("FLOE_EMPTY_QA_CHILD").as_deref(), Ok("1"));
        assert_eq!(std::env::current_dir().unwrap(), std::path::Path::new("/"));
        let mut session = Session::parse(&[]).unwrap();
        let scope = Scope::prepare(&mut session, true).unwrap().unwrap();
        scope.verify().unwrap();
        assert!(scope.root.parent().is_some());
        assert!(!session.needs_initial_directory());
        fs::remove_dir(&scope.root).unwrap();
        println!("EMPTY QA ROOT CWD: OK");
    }
}
