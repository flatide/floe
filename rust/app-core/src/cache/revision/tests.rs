use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

static SERIAL: AtomicU64 = AtomicU64::new(0);
struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "floe-revisions-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        Self(path)
    }
    fn store(&self) -> Store {
        let source = self.0.join("한 글.oas");
        fs::write(&source, b"synthetic source").unwrap();
        Store::new(&source).unwrap()
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

// File-set pin tests do not pretend these bytes are a valid geometry cache.
// The separate actual-index integration gate covers validation/publication.
fn sealed(store: &Store, digit: char) -> (PathBuf, Vec<u8>) {
    sealed_with_files(store, digit, &[])
}
fn sealed_with_files(store: &Store, digit: char, optional: &[&str]) -> (PathBuf, Vec<u8>) {
    fs::create_dir_all(store.path()).unwrap();
    let revision = digit.to_string().repeat(32);
    let path = store.path().join(&revision);
    fs::create_dir(&path).unwrap();
    for name in ["meta.json", "design.ovm", "design.ovp"]
        .iter()
        .chain(optional)
    {
        fs::write(path.join(name), revision.as_bytes()).unwrap();
    }
    let record = Record {
        version: 1,
        source: store.source.clone(),
        revision,
        source_stamp: Stamp::source(&store.source).unwrap(),
        files: files(&path, false).unwrap(),
        owner: None,
    };
    let bytes = serde_json::to_vec(&record).unwrap();
    create_record(&path.join("revision.json"), &bytes).unwrap();
    (path, bytes)
}

#[test]
fn resolution_is_read_only_and_old_pin_does_not_follow_current() {
    let root = Root::new();
    let store = root.store();
    assert!(store.pin().unwrap().is_none());
    assert!(!store.path().exists());
    let (a, bytes) = sealed(&store, 'a');
    create_record(&store.path().join("current.json"), &bytes).unwrap();
    let first = store.pin().unwrap().unwrap();
    let (b, bytes) = sealed(&store, 'b');
    fs::write(store.path().join("current.json"), bytes).unwrap();
    assert_eq!(first.directory(), a);
    first.validate().unwrap();
    assert_eq!(store.pin().unwrap().unwrap().directory(), b);
    drop(first);
    assert!(a.is_dir() && b.is_dir());
}

#[test]
fn malformed_pointer_never_falls_back_to_legacy_or_traverses() {
    let root = Root::new();
    let store = root.store();
    let (_, bytes) = sealed(&store, 'a');
    let base: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    for (key, value) in [
        ("revision", serde_json::json!("../external")),
        ("revision", serde_json::json!("한".repeat(32))),
        ("revision", serde_json::json!("A".repeat(32))),
        ("version", serde_json::json!(4)),
        ("source", serde_json::json!(root.0.join("other.oas"))),
    ] {
        let mut value_record = base.clone();
        value_record[key] = value;
        fs::write(
            store.path().join("current.json"),
            serde_json::to_vec(&value_record).unwrap(),
        )
        .unwrap();
        assert!(store.pin().is_err(), "{key}");
    }
    fs::write(
        store.path().join("current.json"),
        vec![b' '; MAX_RECORD as usize + 1],
    )
    .unwrap();
    assert!(store.pin().is_err());
}

#[test]
fn optional_summaries_and_inode_replacement_are_part_of_pin() {
    for change in 0..5 {
        let root = Root::new();
        let store = root.store();
        let (path, bytes) = sealed(&store, 'a');
        create_record(&store.path().join("current.json"), &bytes).unwrap();
        let pin = store.pin().unwrap().unwrap();
        match change {
            0 => fs::write(path.join("design.ovo"), b"new summary").unwrap(),
            1 => {
                fs::remove_file(path.join("design.ovp")).unwrap();
                fs::write(path.join("design.ovp"), b"a".repeat(32)).unwrap();
            }
            2 => fs::write(path.join("design.ovm"), b"altered geometry").unwrap(),
            3 => fs::write(path.join("design.ovh"), b"unsealed hierarchy").unwrap(),
            _ => fs::remove_file(path.join("revision.json")).unwrap(),
        }
        assert!(pin.validate().is_err());
        assert!(store.pin().is_err());
    }
}

#[test]
fn hierarchy_summary_stamp_detects_mutation_removal_replacement_and_links() {
    for change in 0..5 {
        let root = Root::new();
        let store = root.store();
        let (path, bytes) = sealed_with_files(&store, 'a', &["design.ovh"]);
        create_record(&store.path().join("current.json"), &bytes).unwrap();
        let pin = store.pin().unwrap().unwrap();
        assert!(pin.record.files.contains_key("design.ovh"));
        pin.validate().unwrap();
        let summary = path.join("design.ovh");
        match change {
            0 => fs::write(&summary, b"changed hierarchy").unwrap(),
            1 => fs::remove_file(&summary).unwrap(),
            2 => {
                let replacement = root.0.join("replacement");
                fs::write(&replacement, fs::read(&summary).unwrap()).unwrap();
                fs::rename(replacement, &summary).unwrap();
            }
            3 => {
                fs::remove_file(&summary).unwrap();
                std::os::unix::fs::symlink(root.0.join("missing"), &summary).unwrap();
            }
            _ => fs::hard_link(&summary, root.0.join("alias")).unwrap(),
        }
        assert!(pin.validate().is_err(), "change {change}");
        assert!(store.pin().is_err(), "change {change}");
    }
}

#[test]
fn symlink_and_hardlink_inputs_are_rejected() {
    let root = Root::new();
    let store = root.store();
    let (path, bytes) = sealed(&store, 'a');
    create_record(&store.path().join("current.json"), &bytes).unwrap();
    fs::hard_link(path.join("design.ovp"), root.0.join("alias")).unwrap();
    assert!(store.pin().is_err());
    fs::remove_file(root.0.join("alias")).unwrap();
    fs::remove_file(path.join("design.ovp")).unwrap();
    std::os::unix::fs::symlink(root.0.join("missing"), path.join("design.ovp")).unwrap();
    assert!(store.pin().is_err());
    fs::remove_file(store.path().join("current.json")).unwrap();
    std::os::unix::fs::symlink(root.0.join("missing"), store.path().join("current.json")).unwrap();
    assert!(store.pin().is_err());
}

#[test]
fn candidate_is_exclusive_and_cancel_or_drop_never_publishes_or_deletes() {
    let root = Root::new();
    let store = root.store();
    let stop = AtomicUsize::new(0);
    let first = store.begin(&stop).unwrap();
    assert!(matches!(store.begin(&stop), Err(e) if e.kind == ErrorKind::Busy));
    let candidate = first.directory();
    assert!(store.pin().unwrap().is_none());
    drop(first);
    assert!(candidate.is_dir());
    assert!(store.begin(&AtomicUsize::new(1)).is_err());
    let second = store.begin(&stop).unwrap();
    assert_ne!(candidate, second.directory());
    assert!(second.publish(&stop).is_err());
    assert!(store.pin().unwrap().is_none());
    assert!(candidate.is_dir());
}

#[test]
fn witness_failure_after_pointer_commit_is_warning_not_rollback_or_overwrite() {
    for conflict in [false, true] {
        let root = Root::new();
        let store = root.store();
        let stop = AtomicUsize::new(0);
        let candidate = store.begin(&stop).unwrap();
        let witness = candidate.directory().join("published.json");
        if conflict {
            fs::write(&witness, b"preserve unexpected evidence").unwrap();
        }
        // This tests the publication primitive, not geometry validity.
        let bytes = b"synthetic publication record";
        assert_eq!(candidate.commit_current(bytes, &stop).unwrap(), !conflict);
        assert_eq!(fs::read(store.path().join("current.json")).unwrap(), bytes);
        assert_eq!(
            fs::read(witness).unwrap(),
            if conflict {
                b"preserve unexpected evidence".as_slice()
            } else {
                bytes
            }
        );
    }
}

#[test]
fn publication_error_is_unknown_on_either_side_of_commit_and_sync_failure_is_warning() {
    for committed in [false, true] {
        let root = Root::new();
        let store = root.store();
        let (old, bytes) = sealed(&store, 'a');
        let current = store.path().join("current.json");
        create_record(&current, &bytes).unwrap();
        let (new, bytes) = sealed(&store, 'b');
        let pending = store.path().join("pending");
        create_record(&pending, &bytes).unwrap();
        let result = commit_pointer(
            || {
                if committed {
                    fs::rename(&pending, &current)?;
                }
                Err(std::io::Error::other("simulated publication reply failure"))
            },
            || panic!("sync after unknown rename"),
        );
        assert!(matches!(result, Err(e) if e.kind == ErrorKind::PublicationUnknown));
        assert_eq!(
            store.pin().unwrap().unwrap().directory(),
            if committed { new.clone() } else { old.clone() }
        );
        assert!(old.is_dir() && new.is_dir());
        assert_eq!(pending.exists(), !committed);
    }
    assert!(!commit_pointer(
        || Ok(()),
        || Err(std::io::Error::other("simulated fsync failure"))
    )
    .unwrap());
}

#[test]
fn readers_racing_atomic_current_replacement_never_mix_revision_files() {
    let root = Root::new();
    let store = root.store();
    let (a, ab) = sealed(&store, 'a');
    let (b, bb) = sealed(&store, 'b');
    let current = store.path().join("current.json");
    create_record(&current, &ab).unwrap();
    let writer = store.clone();
    let thread = std::thread::spawn(move || {
        for i in 0..64 {
            let pending = writer.path().join(format!("test-current-{i}"));
            create_record(&pending, if i % 2 == 0 { &bb } else { &ab }).unwrap();
            fs::rename(pending, writer.path().join("current.json")).unwrap();
        }
    });
    let reads = (0..128).map(|_| store.pin()).collect::<Vec<_>>();
    thread.join().unwrap();
    for pin in reads {
        let pin = pin.unwrap().unwrap();
        assert!(pin.directory() == a || pin.directory() == b);
        pin.validate().unwrap();
    }
}
