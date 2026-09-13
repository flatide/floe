use super::*;
use std::sync::Barrier;
struct Folder(PathBuf);
impl Folder {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "floe-drc-build-test-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        DirBuilder::new().mode(0o700).create(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }
}
impl Drop for Folder {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn state() -> Mutex<State> {
    Mutex::new(State {
        sealed: false,
        snapshot: Snapshot {
            id: 1,
            phase: Phase::Preparing,
            elapsed_ms: 0,
            native: Progress::default(),
            failure: None,
            outcome: None,
            native_pid: None,
            cleanup_warning: false,
        },
    })
}
#[test]
fn staging_is_private_and_only_removes_its_own_flat_namespace() {
    let folder = Folder::new();
    let s = state();
    let sibling = folder.0.join("result.ice.tmp-other-user");
    fs::write(&sibling, b"keep").unwrap();
    let path;
    {
        let stage = Staging::create(&folder.0, &s).unwrap();
        path = stage.path.clone();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o700
        );
        fs::write(path.join("result.ice.tmp0"), b"native temporary").unwrap();
        fs::write(path.join("result.ice"), b"native output").unwrap();
    }
    assert!(!path.exists());
    assert_eq!(fs::read(sibling).unwrap(), b"keep");
    assert!(!s.lock().unwrap().snapshot.cleanup_warning);
    let stage = Staging::create(&folder.0, &s).unwrap();
    let unexpected = stage.path.join("unrelated");
    fs::create_dir(&unexpected).unwrap();
    drop(stage);
    assert!(unexpected.is_dir());
    assert!(s.lock().unwrap().snapshot.cleanup_warning);
}
#[test]
fn targets_reject_aliases_and_detect_source_or_destination_replacement() {
    let folder = Folder::new();
    let source = folder.0.join("input.db");
    fs::write(&source, b"TOP 1000\n").unwrap();
    let input = Input::open(&source).unwrap();
    let output = output_path(&source).unwrap();
    assert_eq!(output, folder.0.join("input.db.ice"));
    std::os::unix::fs::symlink(&source, &output).unwrap();
    assert!(output_path(&source).is_err());
    fs::remove_file(&output).unwrap();
    fs::hard_link(&source, &output).unwrap();
    assert!(existing(&output).is_err());
    fs::remove_file(&output).unwrap();
    fs::write(&output, b"old").unwrap();
    let old = existing(&output).unwrap().unwrap();
    let replacement = folder.0.join("replacement");
    fs::write(&replacement, b"new").unwrap();
    fs::rename(&replacement, &output).unwrap();
    assert!(unchanged_target(&output, Some(&old)).is_err());
    assert!(unchanged_target(&output, None).is_err());
    fs::write(&replacement, b"TOP 1000\n").unwrap();
    fs::rename(&replacement, &source).unwrap();
    assert!(input.unchanged_at(&source).is_err());
    for jobs in [0, 17, usize::MAX] {
        assert!(Options { jobs, force: false }.validate().is_err());
    }
}
#[test]
fn cancellation_and_commit_have_one_winner_and_creation_never_clobbers() {
    let folder = Folder::new();
    for replace in [false, true] {
        for n in 0..32 {
            let s = state();
            let stop = AtomicUsize::new(0);
            let tmp = folder.0.join(format!("tmp-{replace}-{n}"));
            let target = folder.0.join(format!("out-{replace}-{n}"));
            fs::write(&tmp, b"new").unwrap();
            if replace {
                fs::write(&target, b"old").unwrap();
            }
            let barrier = Barrier::new(2);
            let result = thread::scope(|scope| {
                scope.spawn(|| {
                    barrier.wait();
                    cancel(&s, &stop);
                });
                barrier.wait();
                commit(&tmp, &target, replace, &s, &stop)
            });
            match result {
                Ok(()) => {
                    assert_eq!(fs::read(&target).unwrap(), b"new");
                    cancel(&s, &stop);
                    assert_eq!(
                        stop.load(Ordering::Relaxed),
                        0,
                        "late cancel changed a published result"
                    );
                }
                Err(e) => {
                    assert_eq!(e.kind, ErrorKind::Cancelled);
                    if replace {
                        assert_eq!(fs::read(&target).unwrap(), b"old");
                    } else {
                        assert!(!target.exists());
                    }
                }
            }
        }
    }
    let tmp = folder.0.join("last-temp");
    let target = folder.0.join("last-output");
    fs::write(&tmp, b"new").unwrap();
    fs::write(&target, b"other writer").unwrap();
    assert!(commit(&tmp, &target, false, &state(), &AtomicUsize::new(0)).is_err());
    assert_eq!(fs::read(&target).unwrap(), b"other writer");
}
