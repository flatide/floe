//! Real subprocess death at existing private publication hooks. These hooks and
//! environment selectors are test-only; no product fault-injection API is added.
use super::*;
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::process::ExitStatusExt,
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
};

const CHILD_TEST: &str = "drc::review::store::tests::process_crash::publication_crash_child";
const CASE_ENV: &str = "FLOE_TEST_REVIEW_CRASH_CASE";
const READY: &str = "FLOE_REVIEW_CRASH_READY";

fn draft(f: &Fixture, s: &Arc<Store>, new: bool) -> Draft {
    match s.kind {
        Kind::Notes => f.note(
            s,
            if new {
                "new synthetic note"
            } else {
                "old synthetic note"
            },
        ),
        Kind::Waives => s
            .snapshot(&f.stop)
            .unwrap()
            .prepare_waives(&[(0, u8::from(new)), (1, 0)], &f.stop)
            .unwrap(),
    }
}

fn ready_then_wait_for_death() -> ! {
    // A newline separates our fixed marker from libtest's progress output.
    println!("\n{READY}");
    std::io::stdout().flush().unwrap();
    let mut byte = [0];
    let _ = std::io::stdin().read_exact(&mut byte);
    panic!("parent must kill the owned child without resuming publication");
}

#[test]
#[ignore = "private subprocess entry; run subprocess_death_preserves_publication_boundary"]
fn publication_crash_child() {
    let case = std::env::var(CASE_ENV).expect("parent selects a fixed synthetic case");
    let (kind, existing, point) = parse_case(&case);
    // No caller path: this exact child creates and owns its synthetic directory.
    let f = Fixture::with_count(2);
    fs::set_permissions(&f.root, fs::Permissions::from_mode(0o700)).unwrap();
    let s = f.store(kind);
    if existing {
        draft(&f, &s, false).publish(&f.stop).unwrap();
    }
    if point.starts_with("recovery_") {
        assert!(!existing);
        draft(&f, &s, true).publish(&f.stop).unwrap();
        let security = Security::read(&File::open(s.target()).unwrap()).unwrap();
        let marker: serde_json::Value =
            serde_json::from_slice(security.attribute(super::super::recovery::MARKER).unwrap())
                .unwrap();
        let stage = f.dir.join(marker["stage"].as_str().unwrap());
        fs::hard_link(s.target(), &stage).unwrap(); // marked gap fixture
        s.prepare_recovery(&f.stop)
            .unwrap()
            .recover_using(
                &f.stop,
                || {
                    if point == "recovery_before" {
                        ready_then_wait_for_death();
                    }
                    fs::remove_file(stage)?;
                    ready_then_wait_for_death(); // repair committed, receipt not returned
                },
                File::sync_all,
            )
            .unwrap();
        panic!("recovery unexpectedly returned");
    }
    draft(&f, &s, true)
        .publish_with_link_hook(
            &f.stop,
            || {
                if point == "link_gap" {
                    assert!(!existing);
                    // Old-version gap without provenance is still NOT adopted.
                    let stages = temporary_stages(&f);
                    assert_eq!(stages.len(), 1);
                    let file = File::open(&stages[0]).unwrap();
                    Security::read(&file)
                        .unwrap()
                        .without_attribute(super::super::recovery::MARKER)
                        .apply(&file)
                        .unwrap();
                    fs::hard_link(&stages[0], s.target()).unwrap();
                    ready_then_wait_for_death();
                }
                if point == "before" {
                    ready_then_wait_for_death();
                }
                Ok(())
            },
            |directory| {
                if point == "committed" {
                    ready_then_wait_for_death();
                }
                directory.sync_all()?;
                ready_then_wait_for_death(); // synced, before returning a receipt
            },
            || {
                if point == "real_link_gap" {
                    ready_then_wait_for_death();
                }
            },
        )
        .unwrap();
    panic!("publication unexpectedly returned");
}

fn parse_case(case: &str) -> (Kind, bool, &str) {
    let parts: Vec<_> = case.split(':').collect();
    assert_eq!(parts.len(), 3);
    let kind = match parts[0] {
        "notes" => Kind::Notes,
        "waives" => Kind::Waives,
        _ => panic!("invalid synthetic kind"),
    };
    let existing = match parts[1] {
        "existing" => true,
        "missing" => false,
        _ => panic!("invalid synthetic target"),
    };
    assert!([
        "before",
        "committed",
        "synced",
        "link_gap",
        "real_link_gap",
        "recovery_before",
        "recovery_after"
    ]
    .contains(&parts[2]));
    (kind, existing, parts[2])
}

struct OwnedChild {
    child: Child,
    reader: Option<thread::JoinHandle<()>>,
    reaped: bool,
}
impl OwnedChild {
    fn spawn(case: &str) -> (Self, mpsc::Receiver<bool>) {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                CHILD_TEST,
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CASE_ENV, case)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let output = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        let reader = thread::spawn(move || {
            let mut marked = false;
            for line in BufReader::new(output).lines() {
                let Ok(line) = line else { break };
                if line == READY {
                    marked = true;
                    let _ = tx.send(true);
                }
            }
            if !marked {
                let _ = tx.send(false);
            }
        });
        (
            Self {
                child,
                reader: Some(reader),
                reaped: false,
            },
            rx,
        )
    }
    fn kill_and_reap(&mut self) {
        // Child::kill targets only this still-owned, unreaped process (SIGKILL
        // on Unix). Never look up a PID by a filename or kill a process group.
        self.child.kill().unwrap();
        let status = self.child.wait().unwrap();
        self.reaped = true;
        self.reader.take().unwrap().join().unwrap();
        assert_eq!(status.signal(), Some(libc::SIGKILL));
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if !self.reaped {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

fn temporary_stages(f: &Fixture) -> Vec<PathBuf> {
    fs::read_dir(&f.dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with(".floe-review-")
        })
        .collect()
}

#[test]
fn subprocess_death_preserves_publication_boundary() {
    for kind_name in ["notes", "waives"] {
        for target in ["missing", "existing"] {
            for point in ["before", "committed", "synced"] {
                run_case(&format!("{kind_name}:{target}:{point}"));
            }
        }
    }
}

#[test]
fn legacy_unmarked_link_gap_is_not_adopted() {
    for kind_name in ["notes", "waives"] {
        run_case(&format!("{kind_name}:missing:link_gap"));
    }
}

#[test]
fn actual_link_gap_and_repair_death_allow_only_explicit_bound_recovery() {
    for kind_name in ["notes", "waives"] {
        for point in ["real_link_gap", "recovery_before", "recovery_after"] {
            run_case(&format!("{kind_name}:missing:{point}"));
        }
    }
}

fn run_case(case: &str) {
    let (kind, existing, point) = parse_case(case);
    let (mut child, marker) = OwnedChild::spawn(case);
    assert_eq!(
        marker.recv_timeout(Duration::from_secs(30)),
        Ok(true),
        "{case}: child did not reach publication hook"
    );
    // The exact single-test child starts Fixture's serial at zero.
    // Adopt cleanup only AFTER it confirms successful creation and
    // the hook; no directory from a failed startup is deleted.
    let root = std::env::temp_dir().join(format!("floe-review-store-{}-0", child.child.id()));
    let root_meta = fs::symlink_metadata(&root).unwrap();
    assert!(root_meta.is_dir() && !root_meta.file_type().is_symlink());
    assert_eq!(root_meta.permissions().mode() & 0o777, 0o700);
    let root = fs::canonicalize(root).unwrap();
    let dir = root.join("inputs");
    let pack = dir.join("한 글.db.ice");
    let bytes = crate::drc::tests::bytes(2);
    assert_eq!(fs::read(&pack).unwrap(), bytes);
    let target_path = paths(&pack, "reviewer", kind).unwrap()[0].clone();
    let read_target = || match fs::read(&target_path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => panic!("synthetic target read failed: {error}"),
    };
    let bytes_at_hook = read_target();
    let scope = AccessScope::new(std::slice::from_ref(&root)).unwrap();
    child.kill_and_reap();
    assert_eq!(
        read_target(),
        bytes_at_hook,
        "{case}: kill changed target bytes"
    );
    let f = Fixture {
        root,
        dir,
        pack,
        bytes,
        scope,
        stop: AtomicUsize::new(0),
    };
    let s = f.store(kind); // fresh reader; no state survives from writer
    if ["link_gap", "real_link_gap", "recovery_before"].contains(&point) {
        let stages = temporary_stages(&f);
        assert_eq!(stages.len(), 1);
        let target = fs::metadata(s.target()).unwrap();
        let stage = fs::metadata(&stages[0]).unwrap();
        assert_eq!(target.nlink(), 2);
        assert_eq!(identity(&target), identity(&stage));
        assert_eq!(target.mode() & 0o777, 0o600);
        let error = s
            .snapshot(&f.stop)
            .err()
            .expect("double-link target must remain rejected");
        assert_eq!(error.kind, ErrorKind::InvalidInput);
        assert_eq!(fs::read(&f.pack).unwrap(), f.bytes);
        if point == "link_gap" {
            assert!(s.prepare_recovery(&f.stop).is_err());
            println!("REVIEW CRASH GAP: {case}: unmarked legacy pair refused without deletion");
            return;
        }
        let before = fs::read(s.target()).unwrap();
        let recovery = s.prepare_recovery(&f.stop).unwrap();
        assert_eq!(fs::metadata(s.target()).unwrap().nlink(), 2);
        assert_eq!(
            recovery.recover(&f.stop).unwrap(),
            RecoveryResult::Recovered {
                already_completed: false,
                directory_synced: true,
            }
        );
        assert_eq!(fs::read(s.target()).unwrap(), before);
        assert!(temporary_stages(&f).is_empty());
        println!("REVIEW CRASH RECOVERY: {case}: exact explicit unlink restored single-link read");
    }
    let published = point != "before";
    assert_eq!(s.target().exists(), existing || published, "{case}");
    if existing || published {
        assert_eq!(fs::metadata(s.target()).unwrap().mode() & 0o777, 0o600);
    }
    let snapshot = s.snapshot(&f.stop).unwrap();
    assert!(!snapshot.legacy_unverified(), "{case}: binding lost");
    match kind {
        Kind::Notes => {
            let value = if published {
                Some("new synthetic note")
            } else if existing {
                Some("old synthetic note")
            } else {
                None
            };
            assert_eq!(snapshot.notes().unwrap().get(0), value, "{case}");
            assert_eq!(snapshot.notes().unwrap().get(1), value, "{case}");
        }
        Kind::Waives => assert_eq!(
            snapshot.selected_statuses(&[0, 1], &f.stop).unwrap(),
            [u8::from(published), 0],
            "{case}"
        ),
    }
    let stages = temporary_stages(&f);
    assert_eq!(stages.len(), usize::from(!published), "{case}");
    let orphan = stages.first().map(|path| {
        let metadata = fs::symlink_metadata(path).unwrap();
        assert!(metadata.is_file() && !metadata.file_type().is_symlink());
        assert_eq!(metadata.mode() & 0o777, 0o600);
        (path.clone(), fs::read(path).unwrap())
    });
    drop(snapshot);
    // A NEW explicit approval succeeds: the crashed process's flock
    // was released. Its private orphan is neither adopted nor purged.
    draft(&f, &s, false).publish(&f.stop).unwrap();
    if let Some((path, bytes)) = orphan {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
    assert_eq!(temporary_stages(&f), stages);
    assert_eq!(fs::read(&f.pack).unwrap(), f.bytes);
    println!("REVIEW CRASH: {case}: old/new boundary, binding, lock release OK");
}
