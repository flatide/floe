use super::*;
use std::io::{self, Cursor};
use std::sync::atomic::Ordering;

fn fp(total: u64) -> Fingerprint {
    Fingerprint {
        source_size: 123,
        source_mtime: 456,
        total,
    }
}
fn input(layout: &Layout, statuses: &[u8]) -> Vec<u8> {
    let mut data = layout.header().to_vec();
    data.extend(statuses);
    // Deliberately stale counters, like interrupted legacy pwrite autosave.
    data.extend(vec![255; layout.counts.len() * 4]);
    data
}
#[test]
fn waive_round_trip_recounts_reserved_statuses_and_empty_rules() {
    let layout = Layout::new(fp(6), &[0, 3, 0, 3, 0]).unwrap();
    let mut out = Vec::new();
    let stop = AtomicUsize::new(0);
    let stats = rewrite_waives(
        Cursor::new(input(&layout, &[0, 1, 255, 2, 1, 1])),
        &mut out,
        &layout,
        &[(1, 0), (3, 1), (3, 2), (5, 255)],
        &stop,
    )
    .unwrap();
    assert_eq!(
        stats,
        WaiveStats {
            waived: 1,
            per_rule: vec![0, 0, 0, 1, 0]
        }
    );
    assert_eq!(&out[40..46], &[0, 0, 255, 2, 1, 255]);
    let mut copy = Vec::new();
    assert_eq!(
        rewrite_waives(out.as_slice(), &mut copy, &layout, &[], &stop).unwrap(),
        stats
    );
    assert_eq!(out, copy);
    let empty = Layout::new(fp(0), &[0, 0]).unwrap();
    assert_eq!(
        rewrite_waives(
            input(&empty, &[]).as_slice(),
            io::sink(),
            &empty,
            &[],
            &stop
        )
        .unwrap()
        .per_rule,
        [0, 0]
    );
}
#[test]
fn waive_rejects_foreign_truncated_extra_and_invalid_edits() {
    let stop = AtomicUsize::new(0);
    let layout = Layout::new(fp(2), &[2]).unwrap();
    let data = input(&layout, &[1, 0]);
    for end in 0..data.len() {
        assert!(rewrite_waives(&data[..end], io::sink(), &layout, &[], &stop).is_err());
    }
    let mut extra = data.clone();
    extra.push(0);
    assert!(rewrite_waives(extra.as_slice(), io::sink(), &layout, &[], &stop).is_err());
    for at in [0, 8, 12, 20, 28, 36] {
        let mut bad = data.clone();
        bad[at] ^= 1;
        let mut out = Vec::new();
        assert!(rewrite_waives(bad.as_slice(), &mut out, &layout, &[], &stop).is_err());
        assert!(out.is_empty());
    }
    for changes in [vec![(2, 1)], vec![(0, 1); EDIT_ITEMS + 1]] {
        let mut out = Vec::new();
        assert!(rewrite_waives(data.as_slice(), &mut out, &layout, &changes, &stop).is_err());
        assert!(out.is_empty());
    }
    assert!(Layout::new(fp(3), &[2]).is_err());
    assert!(Layout::new(fp(u64::MAX), &[u64::MAX]).is_err());
    assert!(Layout::new(fp(1u64 << 32), &[1u64 << 32]).is_err());
}
struct BoundedSink {
    total: usize,
    largest: usize,
}
impl Write for BoundedSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.total += buf.len();
        self.largest = self.largest.max(buf.len());
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[test]
fn waive_streams_without_materializing_all_statuses() {
    let n = 3_000_007;
    let layout = Layout::new(fp(n), &[n]).unwrap();
    let source = Cursor::new(layout.header())
        .chain(io::repeat(1).take(n))
        .chain(Cursor::new([0; 4]));
    let mut out = BoundedSink {
        total: 0,
        largest: 0,
    };
    let stats = rewrite_waives(
        source,
        &mut out,
        &layout,
        &[(n - 1, 0)],
        &AtomicUsize::new(0),
    )
    .unwrap();
    assert_eq!(stats.waived, n - 1);
    assert_eq!(out.total, n as usize + 44);
    assert!(out.largest <= STATUS_CHUNK);
}
struct CancelWriter<'a>(&'a AtomicUsize);
impl Write for CancelWriter<'_> {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.0.store(1, Ordering::Relaxed);
        Ok(b.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[test]
fn cancellation_and_sink_errors_are_not_success() {
    let stop = AtomicUsize::new(0);
    let layout = Layout::new(fp(2), &[2]).unwrap();
    let data = input(&layout, &[0, 1]);
    assert_eq!(
        rewrite_waives(data.as_slice(), CancelWriter(&stop), &layout, &[], &stop)
            .unwrap_err()
            .kind,
        ErrorKind::Cancelled
    );
    let mut out = Vec::new();
    assert!(rewrite_waives(data.as_slice(), &mut out, &layout, &[], &stop).is_err());
    assert!(out.is_empty());
    stop.store(0, Ordering::Relaxed);
    let mut tiny = [0u8; 3];
    assert_eq!(
        rewrite_waives(data.as_slice(), tiny.as_mut_slice(), &layout, &[], &stop)
            .unwrap_err()
            .kind,
        ErrorKind::Io
    );
}
#[test]
fn note_group_reassignment_clear_and_failure_are_atomic() {
    let stop = AtomicUsize::new(0);
    let mut notes = Notes::new(fp(5));
    notes.set(&[0, 1, 2, 2], " 첫 줄\n둘째 ", &stop).unwrap();
    notes.set(&[1, 3], "second", &stop).unwrap();
    assert_eq!(notes.get(0), Some("첫 줄\n둘째"));
    assert_eq!(notes.get(1), Some("second"));
    notes.set(&[1], "  ", &stop).unwrap();
    assert_eq!(notes.get(1), None);
    assert_eq!(notes.member_count(), 3);
    assert_eq!(
        notes
            .groups()
            .map(|n| n.members.clone())
            .collect::<Vec<_>>(),
        vec![BTreeSet::from([0, 2]), BTreeSet::from([3])]
    );
    let old = notes.clone();
    for (ids, text) in [
        (vec![0, 5], "bad".into()),
        (vec![0], "a".repeat(NOTE_BYTES + 1)),
        (vec![0], "bad\0text".into()),
        (vec![0; EDIT_ITEMS + 1], "bad".into()),
    ] {
        assert!(notes.set(&ids, &text, &stop).is_err());
        assert_eq!(notes, old);
    }
    stop.store(1, Ordering::Relaxed);
    assert!(notes.set(&[0], "bad", &stop).is_err());
    assert_eq!(notes, old);
    stop.store(0, Ordering::Relaxed);
    notes.next = u64::MAX;
    let old = notes.clone();
    assert!(notes.set(&[0], "bad", &stop).is_err());
    assert_eq!(notes, old);
}
#[test]
fn notes_import_guards_identity_reports_loss_and_does_not_resurrect_groups() {
    let stop = AtomicUsize::new(0);
    let tag = fp(5).tag();
    for text in [
        "floe_note=0|x".into(),
        format!("floe_pack={tag}\nfloe_pack={tag}\n"),
        "floe_pack=123,456,6\n".into(),
    ] {
        assert!(Notes::parse(&text, fp(5), &stop).is_err());
    }
    let text=format!("# flateyes annotations\nfloe_pack={tag}\nfloe_note=0,1,2|first\nfloe_note=1,3,999,bad,-1|새 메모\\nline\\\\n\nfloe_note=2,4|\nfloe_note=oops\ntext=ignored\n");
    let (notes, report) = Notes::parse(&text, fp(5), &stop).unwrap();
    assert_eq!(
        report,
        ImportReport {
            skipped_lines: 2,
            invalid_members: 3,
            reassigned_members: 1
        }
    );
    assert_eq!(notes.get(1), Some("새 메모\nline\\n"));
    assert_eq!(
        notes.groups().next().unwrap().members,
        BTreeSet::from([0, 2])
    );
    let exported = notes
        .serialize(|g| Ok([g as f64, -0.125]), &stop)
        .unwrap()
        .unwrap();
    assert!(exported.contains("text=0,-0.125,16,#FFD819,#00000059,first"));
    let (round, report) = Notes::parse(&exported, fp(5), &stop).unwrap();
    assert_eq!(round, notes);
    assert_eq!(report, ImportReport::default());
}
#[test]
fn notes_export_is_bounded_and_geometry_failures_do_not_fabricate_origins() {
    let stop = AtomicUsize::new(0);
    let mut notes = Notes::new(fp(300));
    assert!(notes
        .serialize(|_| panic!("empty lookup"), &stop)
        .unwrap()
        .is_none());
    notes
        .set(
            &(0..300).collect::<Vec<_>>(),
            &"x".repeat(NOTE_BYTES),
            &stop,
        )
        .unwrap();
    assert_eq!(
        notes.serialize(|_| Ok([0., 0.]), &stop).unwrap_err().kind,
        ErrorKind::Incomplete
    );
    assert!(notes
        .serialize(|_| Err(Error::input("missing error")), &stop)
        .is_err());
    assert!(notes.serialize(|_| Ok([f64::NAN, 0.]), &stop).is_err());
    assert!(notes
        .serialize(
            |_| {
                stop.store(1, Ordering::Relaxed);
                Ok([0., 0.])
            },
            &stop
        )
        .is_err());
}
#[test]
fn membership_and_text_limits_reject_without_dropping_existing_notes() {
    let stop = AtomicUsize::new(0);
    let mut notes = Notes::new(fp(NOTE_MEMBERS as u64 + 1));
    notes
        .assign((0..NOTE_MEMBERS as u64).collect(), "group", &stop)
        .unwrap();
    let old = notes.clone();
    assert!(notes
        .set(&[NOTE_MEMBERS as u64], "overflow", &stop)
        .is_err());
    assert_eq!(notes, old);
    notes.set(&[0], "", &stop).unwrap();
    notes.set(&[NOTE_MEMBERS as u64], "fits", &stop).unwrap();
    let mut notes = Notes::new(fp(1000));
    for i in 0..(SIDECAR_BYTES / NOTE_BYTES) as u64 {
        notes.set(&[i], &"x".repeat(NOTE_BYTES), &stop).unwrap();
    }
    let old = notes.clone();
    assert!(notes.set(&[999], "x", &stop).is_err());
    assert_eq!(notes, old);
}

#[test]
fn note_ids_remain_exact_above_javascript_integer_range() {
    let stop = AtomicUsize::new(0);
    let fingerprint = fp(u64::MAX);
    let mut notes = Notes::new(fingerprint);
    notes
        .set(&[9_007_199_254_740_993, u64::MAX - 1], "large IDs", &stop)
        .unwrap();
    let text = notes.serialize(|_| Ok([1., 2.]), &stop).unwrap().unwrap();
    assert!(text.contains("floe_note=9007199254740993,18446744073709551614|large IDs"));
    assert_eq!(Notes::parse(&text, fingerprint, &stop).unwrap().0, notes);
    let bad = format!(
        "floe_pack={}\nfloe_note=18446744073709551616,18446744073709551615|bad\n",
        fingerprint.tag()
    );
    let (empty, report) = Notes::parse(&bad, fingerprint, &stop).unwrap();
    assert_eq!(empty.member_count(), 0);
    assert_eq!(report.invalid_members, 2);
    assert_eq!(report.skipped_lines, 1);
}
