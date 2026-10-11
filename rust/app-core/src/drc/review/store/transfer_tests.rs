use super::*;
use std::os::unix::fs::FileExt;

fn seed(s: &Arc<Store>, stop: &AtomicUsize) -> Vec<u8> {
    let mut bytes = Vec::new();
    s.snapshot(stop).unwrap().export(&mut bytes, stop).unwrap();
    bytes
}
fn import(f: &Fixture, s: &Arc<Store>, bytes: &[u8]) -> (Draft, WaiveStats) {
    let path = f.root.join("portable.waive");
    fs::write(&path, bytes).unwrap();
    s.snapshot(&f.stop)
        .unwrap()
        .prepare_waives_import(File::open(path).unwrap(), &f.stop)
        .unwrap()
}

#[test]
fn missing_exports_are_valid_and_create_no_sidecar_or_lock() {
    let f = Fixture::new();
    for k in [Kind::Notes, Kind::Waives] {
        let s = f.store(k);
        let mut bytes = Vec::new();
        let info = s
            .snapshot(&f.stop)
            .unwrap()
            .export(&mut bytes, &f.stop)
            .unwrap();
        assert_eq!(info.bytes, bytes.len() as u64);
        assert!(!info.legacy_unverified);
        assert_eq!(info.import_report, ImportReport::default());
        match k {
            Kind::Notes => {
                let (notes, report) = Notes::parse(
                    std::str::from_utf8(&bytes).unwrap(),
                    s.layout.fingerprint(),
                    &f.stop,
                )
                .unwrap();
                assert_eq!(notes.member_count(), 0);
                assert_eq!(report, ImportReport::default());
                assert_eq!(
                    info.contents,
                    ExportContents::Notes {
                        groups: 0,
                        members: 0
                    }
                );
            }
            Kind::Waives => {
                assert_eq!(bytes.len() as u64, s.max_bytes());
                assert_eq!(info.contents, ExportContents::Waives { waived: 0 });
                assert_eq!(
                    rewrite_waives(bytes.as_slice(), std::io::sink(), &s.layout, &[], &f.stop)
                        .unwrap()
                        .per_rule,
                    [0, 0, 0]
                );
            }
        }
        assert!(!s.target().exists() && !s.lock_path().exists());
    }
    f.clean();
}

#[test]
fn note_export_is_canonical_and_import_confirmation_is_independent_of_target_binding() {
    let f = Fixture::new();
    let s = f.store(Kind::Notes);
    f.note(&s, "saved 한글\n<script>").publish(&f.stop).unwrap();
    let before = fs::read(s.target()).unwrap();
    let snapshot = s.snapshot(&f.stop).unwrap();
    assert!(!snapshot.legacy_unverified());
    let mut out = Vec::new();
    let info = snapshot.export(&mut out, &f.stop).unwrap();
    assert_eq!(out, before);
    assert_eq!(
        info.contents,
        ExportContents::Notes {
            groups: 1,
            members: 2
        }
    );
    assert!(std::str::from_utf8(&out)
        .unwrap()
        .contains("text=0.011,0.0015,"));
    let (d, _) = snapshot
        .prepare_notes_import(std::str::from_utf8(&out).unwrap(), &f.stop)
        .unwrap();
    assert!(d.legacy_unverified());
    assert_eq!(kind(d.publish(&f.stop)), ErrorKind::Unsupported);
    assert_eq!(fs::read(s.target()).unwrap(), before);
    f.clean();
}

#[test]
fn whole_waive_transfer_streams_past_edit_and_chunk_caps_and_recounts_reserved_bytes() {
    let f = Fixture::with_count(2 * 65536 + 3);
    let s = f.store(Kind::Waives);
    let mut input = seed(&s, &f.stop);
    let total = s.layout.fingerprint().total as usize;
    for (i, status) in input[40..40 + total].iter_mut().enumerate() {
        *status = [1, 2, 255, 0][i % 4];
    }
    input[40 + total..].fill(255);
    let (d, stats) = import(&f, &s, &input);
    assert_eq!(stats.waived, total.div_ceil(4) as u64);
    assert_eq!(stats.per_rule, [0, total.div_ceil(4) as u32, 0]);
    assert!(d.legacy_unverified());
    assert!(!s.target().exists() && !s.lock_path().exists());
    assert_eq!(kind(d.publish(&f.stop)), ErrorKind::Unsupported);
    assert!(!s.target().exists() && !s.lock_path().exists());
    let (d, _) = import(&f, &s, &input);
    let proof = d.accept_legacy_run().publish(&f.stop).unwrap();
    let snapshot = s.snapshot(&f.stop).unwrap();
    snapshot.verify_published(&proof).unwrap();
    assert!(!snapshot.legacy_unverified());
    let mut output = Vec::new();
    let info = snapshot.export(&mut output, &f.stop).unwrap();
    assert_eq!(&output[..40 + total], &input[..40 + total]);
    assert_eq!(
        &output[40 + total..],
        &[0, total.div_ceil(4) as u32, 0]
            .map(u32::to_le_bytes)
            .concat()
    );
    assert_eq!(fs::read(s.target()).unwrap(), output);
    assert_eq!(
        info.contents,
        ExportContents::Waives {
            waived: stats.waived
        }
    );
    assert_eq!(fs::read(f.root.join("portable.waive")).unwrap(), input);
    // Explicit import still needs confirmation after a verified native publish.
    assert_eq!(
        kind(import(&f, &s, &input).0.publish(&f.stop)),
        ErrorKind::Unsupported
    );
    assert_eq!(fs::read(s.target()).unwrap(), output);
    // Whole import replaces old nonzero/reserved values too, not a union of
    // imported waives with the existing review.
    let mut cleared = output.clone();
    cleared[40..40 + total].fill(0);
    let (d, stats) = import(&f, &s, &cleared);
    assert_eq!(stats.waived, 0);
    d.accept_legacy_run().publish(&f.stop).unwrap();
    let replaced = seed(&s, &f.stop);
    assert!(replaced[40..].iter().all(|&b| b == 0));
    f.clean();
}

#[test]
fn export_recounts_legacy_waives_without_mutating_the_input() {
    let f = Fixture::new();
    let s = f.store(Kind::Waives);
    let mut raw = seed(&s, &f.stop);
    raw[40..44].copy_from_slice(&[1, 2, 255, 1]);
    raw[105..].fill(255);
    fs::write(s.target(), &raw).unwrap();
    let mut output = Vec::new();
    let info = s
        .snapshot(&f.stop)
        .unwrap()
        .export(&mut output, &f.stop)
        .unwrap();
    assert!(info.legacy_unverified);
    assert_eq!(info.contents, ExportContents::Waives { waived: 2 });
    assert_eq!(&output[..105], &raw[..105]);
    assert_eq!(&output[105..], &[0, 2, 0].map(u32::to_le_bytes).concat());
    assert_eq!(fs::read(s.target()).unwrap(), raw);
    assert!(!s.lock_path().exists());
}

#[test]
fn malformed_foreign_nonregular_wrong_kind_and_cancelled_imports_never_publish() {
    let f = Fixture::new();
    let s = f.store(Kind::Waives);
    let good = seed(&s, &f.stop);
    for mode in 0..4 {
        let mut bytes = good.clone();
        match mode {
            0 => bytes[0] ^= 1,
            1 => bytes[12] ^= 1,
            2 => {
                bytes.pop();
            }
            _ => bytes.push(0),
        }
        let path = f.root.join("bad.waive");
        fs::write(&path, bytes).unwrap();
        assert!(s
            .snapshot(&f.stop)
            .unwrap()
            .prepare_waives_import(File::open(path).unwrap(), &f.stop)
            .is_err());
    }
    assert_eq!(
        kind(
            s.snapshot(&f.stop)
                .unwrap()
                .prepare_waives_import(File::open(&f.root).unwrap(), &f.stop)
        ),
        ErrorKind::InvalidInput
    );
    let path = f.root.join("good.waive");
    fs::write(&path, good).unwrap();
    assert_eq!(
        kind(
            s.snapshot(&f.stop)
                .unwrap()
                .prepare_waives_import(File::open(&path).unwrap(), &AtomicUsize::new(1))
        ),
        ErrorKind::Cancelled
    );
    let notes = f.store(Kind::Notes);
    assert_eq!(
        kind(
            notes
                .snapshot(&f.stop)
                .unwrap()
                .prepare_waives_import(File::open(path).unwrap(), &f.stop)
        ),
        ErrorKind::InvalidInput
    );
    assert!(!notes.target().exists() && !s.target().exists() && !s.lock_path().exists());
    f.clean();
}

#[test]
fn imported_descriptor_cursor_is_independent_and_unlinked_staging_is_supported() {
    let f = Fixture::new();
    let s = f.store(Kind::Waives);
    let mut bytes = seed(&s, &f.stop);
    bytes[40] = 1;
    let p = f.root.join("upload-stage");
    fs::write(&p, &bytes).unwrap();
    let fd = File::open(&p).unwrap();
    fs::remove_file(p).unwrap(); // capture metadata AFTER unlink, no pathname authority
    let mut cursor = fd.try_clone().unwrap();
    cursor.seek(std::io::SeekFrom::End(0)).unwrap();
    let d = s
        .snapshot(&f.stop)
        .unwrap()
        .prepare_waives_import(fd, &f.stop)
        .unwrap()
        .0;
    cursor.seek(std::io::SeekFrom::Start(23)).unwrap();
    d.accept_legacy_run().publish(&f.stop).unwrap();
    assert_eq!(cursor.stream_position().unwrap(), 23);
    assert_eq!(
        s.snapshot(&f.stop)
            .unwrap()
            .selected_statuses(&[0, 1], &f.stop)
            .unwrap(),
        [1, 0]
    );
    f.clean();
}

#[test]
fn import_change_cancel_and_target_conflict_preserve_review() {
    for mode in ["source", "late-source", "target", "cancel"] {
        let f = Fixture::new();
        let s = f.store(Kind::Waives);
        s.snapshot(&f.stop)
            .unwrap()
            .prepare_waives(&[(1, 1)], &f.stop)
            .unwrap()
            .publish(&f.stop)
            .unwrap();
        let bytes = seed(&s, &f.stop);
        let d = import(&f, &s, &bytes).0.accept_legacy_run();
        let path = f.root.join("portable.waive");
        match mode {
            "source" => fs::write(&path, &bytes).unwrap(),
            "target" => fs::write(s.target(), &bytes).unwrap(),
            "cancel" => f.stop.store(1, Ordering::Relaxed),
            _ => (),
        }
        let before = fs::read(s.target()).unwrap();
        let result = d.publish_using(
            &f.stop,
            || {
                if mode == "late-source" {
                    fs::write(&path, &bytes).unwrap();
                }
                Ok(())
            },
            File::sync_all,
        );
        assert_eq!(
            kind(result),
            if mode == "cancel" {
                ErrorKind::Cancelled
            } else {
                ErrorKind::Busy
            }
        );
        assert_eq!(fs::read(s.target()).unwrap(), before);
        f.clean();
    }
}

struct Callback<F: FnMut()> {
    callback: F,
    bytes: Vec<u8>,
}
impl<F: FnMut()> Write for Callback<F> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        (self.callback)();
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn import_mutation_during_scan_is_not_a_complete_result() {
    let f = Fixture::with_count(131075);
    let s = f.store(Kind::Waives);
    let bytes = seed(&s, &f.stop);
    let d = import(&f, &s, &bytes).0;
    let Change::WaivesImport(input) = d.change else {
        panic!("wrong draft")
    };
    let fd = OpenOptions::new()
        .write(true)
        .open(f.root.join("portable.waive"))
        .unwrap();
    let mut once = false;
    let mut sink = Callback {
        callback: || {
            if !once {
                once = true;
                fd.write_all_at(&[1], 40).unwrap();
            }
        },
        bytes: Vec::new(),
    };
    assert_eq!(
        kind(input.write(&mut sink, &s.layout, &f.stop)),
        ErrorKind::Busy
    );
    assert!(!s.target().exists() && !s.lock_path().exists());
    f.clean();
}

#[test]
fn failed_export_sinks_cancel_or_input_changes_never_report_a_complete_artifact() {
    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("synthetic full disk"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    for k in [Kind::Notes, Kind::Waives] {
        let f = Fixture::new();
        let s = f.store(k);
        if k == Kind::Notes {
            f.note(&s, "old").publish(&f.stop).unwrap();
        } else {
            s.snapshot(&f.stop)
                .unwrap()
                .prepare_waives(&[(0, 1)], &f.stop)
                .unwrap()
                .publish(&f.stop)
                .unwrap();
        }
        let before = fs::read(s.target()).unwrap();
        assert_eq!(
            kind(s.snapshot(&f.stop).unwrap().export(Broken, &f.stop)),
            ErrorKind::Io
        );
        let snap = s.snapshot(&f.stop).unwrap();
        let mut sink = Callback {
            callback: || f.stop.store(1, Ordering::Relaxed),
            bytes: Vec::new(),
        };
        assert_eq!(kind(snap.export(&mut sink, &f.stop)), ErrorKind::Cancelled);
        f.stop.store(0, Ordering::Relaxed);
        let mut once = false;
        let mut sink = Callback {
            callback: || {
                if !once {
                    once = true;
                    fs::write(s.target(), &before).unwrap();
                }
            },
            bytes: Vec::new(),
        };
        assert_eq!(kind(snap.export(&mut sink, &f.stop)), ErrorKind::Busy);
        assert_eq!(fs::read(s.target()).unwrap(), before);
        f.clean();
    }
}
