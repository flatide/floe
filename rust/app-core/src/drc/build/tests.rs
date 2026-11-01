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
        failure_message: None,
        snapshot: Snapshot {
            id: 1,
            phase: Phase::Preparing,
            elapsed_ms: 0,
            native: Progress::default(),
            failure: None,
            outcome: None,
            native_pid: None,
            cleanup_warning: false,
            migration: None,
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
    assert_eq!(output, folder.0.join(".input.db.tray"));
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
#[test]
fn the_pack_lock_is_keyed_on_the_pack_and_refuses_a_writer_or_readers_under_a_replacement() {
    // user 2026-11-01: "pack의 .floe-lock 잠금도 .tray 키로 잡게 고쳐줘"
    let folder = Folder::new();
    let source = folder.0.join("input.db");
    fs::write(&source, b"TOP 1000\n").unwrap();
    let target = output_path(&source).unwrap();
    let key = pack_key(&target);
    assert_eq!(
        (key.dir.clone(), key.name.as_str(), key.subject.as_str()),
        (folder.0.join(".floe-lock"), "input.db.pack", "input.db")
    );
    let scope = AccessScope::new(&[folder.0.clone()]).unwrap();
    let indexer = Indexer::discover(&crate::native::Discovery {
        override_path: Some(PathBuf::from("/bin/sh")),
        development_root: None,
        executable: PathBuf::from("/unused"),
        search_path: None,
    })
    .unwrap();
    let stop = AtomicUsize::new(0);
    let attempt = |force: bool| {
        run(
            &scope,
            &source,
            &target,
            Options { jobs: 2, force },
            &indexer,
            &stop,
            &state(),
        )
    };
    // another build of this pack holds its .build: refused, naming it
    let other =
        floe_vfs::lock::writer(&key, floe_vfs::lock::Mode::Additive, "floe-index drc").unwrap();
    let e = attempt(false).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Busy, "{}", e.message);
    assert!(e.message.starts_with("input.db is being indexed by "), "{}", e.message);
    assert!(
        e.message.ends_with("(floe-index drc) - try again when it finishes"),
        "{}",
        e.message
    );
    drop(other);
    // a reader holds the pack: a build that would replace it is refused as
    // in use, naming the reader - before the indexer runs
    let reader = floe_vfs::lock::readers(&[key.clone()], "floe2 view").unwrap();
    let e = attempt(true).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Busy, "{}", e.message);
    assert!(e.message.starts_with("input.db is in use by "), "{}", e.message);
    assert!(e.message.contains("(floe2 view)"), "{}", e.message);
    drop(reader);
    // nothing in the way: the run goes on to its indexer (/bin/sh is none)
    // under the pack's lock, which says who it was
    let e = attempt(true).unwrap_err();
    assert_ne!(e.kind, ErrorKind::Busy, "{}", e.message);
    let text = fs::read_to_string(key.dir.join("input.db.pack.build")).unwrap();
    assert!(
        text.contains(&format!("what={} drc --build", crate::program())),
        "{text}"
    );
    assert!(!target.exists());
}
