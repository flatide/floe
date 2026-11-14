use super::*;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
};
static SERIAL: AtomicU64 = AtomicU64::new(0);

#[test]
fn readonly_selection_uses_only_fresh_pack_and_fixed_reviewer_paths() {
    use std::os::unix::fs::MetadataExt;
    let f = Fixture::new(&bytes(3));
    let source = f.dir.join("sample");
    fs::write(&source, "synthetic ASCII source\n").unwrap();
    let m = fs::metadata(&source).unwrap();
    let mut packed = bytes(3);
    packed[24..32].copy_from_slice(&m.len().to_le_bytes());
    packed[32..40].copy_from_slice(&(m.mtime() as u64).to_le_bytes());
    fs::write(&f.path, &packed).unwrap();
    let stop = AtomicUsize::new(0);
    let selected = select_review(&source, "fixed", &stop).unwrap();
    assert!(selected.targets.is_some() && selected.warning.is_none());
    assert_eq!(selected.path, crate::cache::absolute(&f.path).unwrap());
    assert_eq!(selected.source, crate::cache::absolute(&source).unwrap());
    let targets = selected.targets.unwrap();
    assert_eq!(
        targets.notes,
        review::store::read_paths(&f.path, "fixed", review::store::Kind::Notes).unwrap()[0]
    );
    assert_eq!(targets.waives, waive_paths(&f.path, "fixed").unwrap()[0]);
    assert!(select_review(&source, "../escape", &stop).is_err());
    assert!(matches!(
        select_review(&source, "fixed", &AtomicUsize::new(1)),
        Err(crate::Error {
            kind: crate::ErrorKind::Cancelled,
            ..
        })
    ));
    let current = Database::open_explicit(&f.path, None, &stop).unwrap();
    current.validate_cache_source(&source).unwrap();
    fs::write(&source, "changed source").unwrap();
    assert!(current.validate_cache_source(&source).is_err());
    let stale = select_review(&source, "fixed", &stop).unwrap();
    assert!(stale.targets.is_none() && stale.warning.is_some());
    fs::write(&f.path, b"broken").unwrap();
    assert!(select_review(&source, "fixed", &stop)
        .unwrap()
        .targets
        .is_none());
    assert!(select_review(&f.path, "fixed", &stop).is_ok()); // non-ICE input is an ASCII candidate
    fs::remove_file(&f.path).unwrap();
    let missing = select_review(&source, "fixed", &stop).unwrap();
    assert!(missing.targets.is_none() && missing.warning.is_none());
}
struct Fixture {
    dir: PathBuf,
    path: PathBuf,
}
impl Fixture {
    fn new(bytes: &[u8]) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "floe-drc-reader-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("sample.ice");
        fs::write(&path, bytes).unwrap();
        Self { dir, path }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.dir).unwrap();
    }
}
fn uv(b: &mut Vec<u8>, mut v: u64) {
    loop {
        let low = (v & 127) as u8;
        v >>= 7;
        b.push(low | if v == 0 { 0 } else { 128 });
        if v == 0 {
            break;
        }
    }
}
fn zz(b: &mut Vec<u8>, v: i64) {
    uv(b, ((v << 1) ^ (v >> 63)) as u64);
}
fn push64(b: &mut Vec<u8>, v: u64) {
    b.extend(v.to_le_bytes());
}
fn push32(b: &mut Vec<u8>, v: u32) {
    b.extend(v.to_le_bytes());
}
fn string(b: &mut Vec<u8>, s: &str) -> u32 {
    let off = b.len() as u32;
    push32(b, s.len() as u32);
    b.extend(s.as_bytes());
    off
}
pub(super) fn bytes(n: u64) -> Vec<u8> {
    geometry_bytes(n, 2)
}
fn geometry_bytes(n: u64, vertices: usize) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend(MAGIC);
    push32(&mut out, 5);
    push32(&mut out, 1);
    out.extend(1000f64.to_le_bytes());
    push64(&mut out, 1234);
    push64(&mut out, 5678);
    let mut blocks = Vec::new();
    for start in (0..n).step_by(64) {
        let count = (n - start).min(64);
        push64(&mut blocks, out.len() as u64);
        push32(&mut blocks, count as u32);
        push32(&mut blocks, 0);
        for v in [start * 10, 0, (start + count - 1) * 10 + 2, 3] {
            push64(&mut blocks, v);
        }
        let mut px = 0;
        for i in start..start + count {
            uv(&mut out, 2 * vertices as u64 + (i % 2));
            zz(&mut out, i as i64 * 10 - px);
            zz(&mut out, 0);
            for j in 1..vertices {
                zz(&mut out, if j % 2 == 1 { 2 } else { -2 });
                zz(&mut out, if j % 2 == 1 { 3 } else { -3 });
            }
            px = i as i64 * 10;
        }
    }
    let qb = out.len() as u64;
    for i in 0..n {
        let span = ((n - 1) * 10 + 2) as f64;
        out.extend([
            (i as f64 * 10. * 255. / span).floor() as u8,
            0,
            ((i * 10 + 2) as f64 * 255. / span).ceil() as u8,
            255,
        ]);
    }
    let status = out.len() as u64;
    out.resize(out.len() + n as usize, 0);
    let wcount = out.len() as u64;
    out.extend([0; 12]);
    let boff = out.len() as u64;
    out.extend(&blocks);
    let mut strings = Vec::new();
    let top = string(&mut strings, "TOP");
    let empty = string(&mut strings, "empty");
    let check = string(&mut strings, "check");
    let tail = string(&mut strings, "tail");
    let desc = string(&mut strings, "rule text");
    let dir = out.len() as u64;
    for (name, start, count, bs, bc, dc) in [
        (empty, 0, 0, 0, 0, 0),
        (check, 0, n, 0, n.div_ceil(64), 1),
        (tail, n, 0, n.div_ceil(64), 0, 0),
    ] {
        for v in [name, 0, dc, 0] {
            push32(&mut out, v);
        }
        for v in [start, count, count + 5, count, bs, bc] {
            push64(&mut out, v);
        }
    }
    let refs = out.len() as u64;
    push32(&mut out, desc);
    let stroff = out.len() as u64;
    out.extend(&strings);
    for v in [
        40,
        qb - 40,
        qb,
        n * 4,
        status,
        wcount,
        boff,
        n.div_ceil(64),
        dir,
        3,
        refs,
        1,
        stroff,
        strings.len() as u64,
        n,
    ] {
        push64(&mut out, v);
    }
    push32(&mut out, top);
    // the source's CRC-32 and the content id (layout 5): the fixture's id
    // is 0xab.. - its first 16 bytes are the waive and notes files' tag -
    // with the error count in its last bytes, so packs of other sizes are
    // other results
    push32(&mut out, 0x1234_abcd);
    let mut content = [0xabu8; 32];
    content[24..].copy_from_slice(&n.to_le_bytes());
    out.extend(content);
    out.extend(MAGIC);
    out
}
/// The fixture pack stamped as made of `source`: its size, time and CRC-32.
fn pack_of(source: &std::path::Path, n: u64) -> Vec<u8> {
    use std::os::unix::fs::MetadataExt;
    let m = fs::metadata(source).unwrap();
    let mut packed = bytes(n);
    packed[24..32].copy_from_slice(&m.len().to_le_bytes());
    packed[32..40].copy_from_slice(&(m.mtime() as u64).to_le_bytes());
    let crc = packed.len() - 168 + 124;
    packed[crc..crc + 4]
        .copy_from_slice(&crc32fast::hash(&fs::read(source).unwrap()).to_le_bytes());
    packed
}
/// User 2026-11-11 ("waive 처리 작업을 매번 자동 저장"): a waive request's
/// bytes are flushed to the disk before it answers - once a request, whatever
/// it wrote, and not at all when nothing changed; a span written chunk by
/// chunk waits for its job's flush; FLOE_WAIVE_SYNC=off writes as before.
#[test]
fn a_waive_request_is_flushed_once_and_the_switch_turns_it_off() {
    use super::desktop::{waive_sync_of, Review, STATUS_NONE, STATUS_WAIVED};
    let dir = std::env::temp_dir().join(format!(
        "floe-waive-sync-{}-{}",
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&dir).unwrap();
    let pack = dir.join(".results.db.tray");
    fs::write(&pack, bytes(8)).unwrap();
    let stop = AtomicUsize::new(0);
    // (the fixture's rules: an empty one, `check` with the errors, an empty tail)
    let mut r = Review::open(&pack, None, Some("kim"), &stop).unwrap();
    assert!(!r.read_only(), "{:?}", r.notices);
    r.set_waive_sync(true); // whatever FLOE_WAIVE_SYNC says where the tests run
    assert_eq!(r.waive_syncs(), 0);
    // one request, three errors: one flush
    r.set_status(1, &[0, 1, 2], STATUS_WAIVED).unwrap();
    assert_eq!(r.waive_syncs(), 1);
    // nothing changes: nothing written, nothing flushed
    r.set_status(1, &[0, 1, 2], STATUS_WAIVED).unwrap();
    assert_eq!(r.waive_syncs(), 1);
    // a span (an application's chunk) waits for the flush its job asks for
    let before = r.statuses(1, 0, 4).unwrap();
    r.write_status_span(1, 0, &before, &[STATUS_NONE; 4]).unwrap();
    assert_eq!(r.waive_syncs(), 1);
    r.sync_waives().unwrap();
    assert_eq!(r.waive_syncs(), 2);
    r.sync_waives().unwrap();
    assert_eq!(r.waive_syncs(), 2, "nothing was written since");
    // the switch: written at once as ever, left to the kernel's writeback
    r.set_waive_sync(false);
    r.set_status(1, &[5], STATUS_WAIVED).unwrap();
    assert_eq!(r.waive_syncs(), 2);
    assert_eq!(r.status(1, 5).unwrap(), STATUS_WAIVED);
    assert_eq!(r.status_counts(1).unwrap().0, 1);
    for (value, on) in [
        (None, true),
        (Some(""), true),
        (Some("on"), true),
        (Some("1"), true),
        (Some("off"), false),
        (Some(" OFF "), false),
        (Some("0"), false),
        (Some("no"), false),
        (Some("false"), false),
    ] {
        assert_eq!(waive_sync_of(value), on, "{value:?}");
    }
    drop(r);
    fs::remove_dir_all(&dir).unwrap();
}
fn set_time(path: &std::path::Path, seconds: u64) {
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds))
        .unwrap();
}
#[test]
fn a_source_whose_time_changed_is_known_by_its_content() {
    const TEXT: &str = "synthetic ASCII source\n";
    let f = Fixture::new(&bytes(3));
    let source = f.dir.join("sample");
    fs::write(&source, TEXT).unwrap();
    set_time(&source, 1_700_000_000);
    fs::write(&f.path, pack_of(&source, 3)).unwrap();
    let stop = AtomicUsize::new(0);
    let p = Pack::open(&f.path, &stop).unwrap();
    let crc = crc32fast::hash(TEXT.as_bytes());
    assert_eq!(
        (p.source_size, p.source_mtime, p.source_crc),
        (TEXT.len() as u64, 1_700_000_000, crc)
    );
    let hint = f.dir.join("sample.ice.source");
    let stamp = |time: u64| format!("floe-drc-source 1\n{} {time} {crc:08x}\n", TEXT.len());
    // as indexed: nothing read, nothing kept
    assert!(p.source_matches(&source).unwrap() && !hint.exists());
    // another time, the same bytes. An open that must not wait does not
    // read a source over its limit: stale, nothing kept ...
    set_time(&source, 1_700_000_007);
    let size = TEXT.len() as u64;
    assert!(!p.source_check(&source, Some(0), &stop).unwrap());
    assert!(!p.source_check(&source, Some(size - 1), &stop).unwrap() && !hint.exists());
    // ... within the limit, and for a caller that reads a source of any
    // size, the content decides and its stamp is kept
    assert!(p.source_check(&source, Some(size), &stop).unwrap());
    assert_eq!(fs::read_to_string(&hint).unwrap(), stamp(1_700_000_007));
    fs::remove_file(&hint).unwrap();
    assert!(p.source_current(&source, &stop).unwrap());
    assert_eq!(fs::read_to_string(&hint).unwrap(), stamp(1_700_000_007));
    // (the kept stamp answers where nothing may be read, too)
    assert!(p.source_check(&source, Some(0), &stop).unwrap());
    // the kept stamp answers with nothing read - other bytes under that size
    // and time pass, the trust a size and a time always had ...
    fs::write(&source, "SYNTHETIC ascii source\n").unwrap();
    set_time(&source, 1_700_000_007);
    assert!(p.source_matches(&source).unwrap());
    // ... and under another time they are read: not the file, no stamp of it
    set_time(&source, 1_700_000_009);
    assert!(!p.source_current(&source, &stop).unwrap() && !p.source_verified(&source, &stop).unwrap());
    assert_eq!(fs::read_to_string(&hint).unwrap(), stamp(1_700_000_007));
    // a stamp of another pack (its CRC), of another format or a cut one is
    // no answer
    for text in [
        format!("floe-drc-source 1\n{} 1700000009 {:08x}\n", TEXT.len(), crc ^ 1),
        format!("floe-drc-source 2\n{} 1700000009 {crc:08x}\n", TEXT.len()),
        format!("floe-drc-source 1\n{} 1700000009 {crc:08x} more\n", TEXT.len()),
        format!("floe-drc-source 1\n{} 1700000009\n", TEXT.len()),
        String::new(),
    ] {
        fs::write(&hint, &text).unwrap();
        assert!(!p.source_current(&source, &stop).unwrap(), "{text:?}");
    }
    // the file as it was, its time changed again: read once more, the stamp
    // renewed; a cancelled read says so
    fs::write(&source, TEXT).unwrap();
    set_time(&source, 1_700_000_011);
    assert!(matches!(
        p.source_verified(&source, &AtomicUsize::new(1)),
        Err(crate::Error {
            kind: crate::ErrorKind::Cancelled,
            ..
        })
    ));
    assert!(p.source_verified(&source, &stop).unwrap());
    assert_eq!(fs::read_to_string(&hint).unwrap(), stamp(1_700_000_011));
    // another size is never the file, whatever a stamp says
    fs::write(&source, "a longer synthetic ASCII source\n").unwrap();
    set_time(&source, 1_700_000_011);
    assert!(!p.source_current(&source, &stop).unwrap() && !p.source_matches(&source).unwrap());
    // a source that is not there is an error, as before
    fs::remove_file(&source).unwrap();
    assert!(p.source_matches(&source).is_err() && p.source_current(&source, &stop).is_err());
}
#[test]
fn a_file_moved_aside_never_takes_a_name_in_use() {
    // two review files moved in one second: the second must not replace
    // the first (a rename onto its name would), and both names stay
    // `stale-<digits>` - what the statistics skip as leftovers
    let f = Fixture::new(&bytes(3));
    let side = f.dir.join(".r.db.waive.kim");
    let number = |p: &PathBuf| {
        p.file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_prefix(".r.db.waive.kim.stale-"))
            .and_then(|n| n.parse::<u64>().ok())
            .expect("stale-<digits>")
    };
    let first = local::aside_path(&side);
    fs::write(&first, b"kept first").unwrap();
    let second = local::aside_path(&side);
    fs::write(&second, b"kept second").unwrap();
    let third = local::aside_path(&side);
    assert!(number(&first) < number(&second) && number(&second) < number(&third));
    assert_eq!(fs::read(&first).unwrap(), b"kept first");
}
#[test]
fn the_inline_verify_limit_is_megabytes_with_a_default() {
    use super::pack::verify_limit_of;
    assert_eq!(verify_limit_of(None), 1024 << 20);
    assert_eq!(verify_limit_of(Some(" 8 ")), 8 << 20);
    assert_eq!(verify_limit_of(Some("0")), 0);
    assert_eq!(verify_limit_of(Some("many")), 1024 << 20);
    assert_eq!(verify_limit_of(Some("18446744073709551615")), u64::MAX);
}
#[test]
fn a_pack_says_its_source_crc_and_content_id() {
    let f = Fixture::new(&bytes(65));
    let p = Pack::open(&f.path, &AtomicUsize::new(0)).unwrap();
    let mut content = [0xab; 32];
    content[24..].copy_from_slice(&65u64.to_le_bytes());
    assert_eq!((p.source_crc, p.content_id), (0x1234_abcd, content));
    // the review identity is the content id; the waive and notes files
    // carry its first 16 bytes with the error count
    assert_eq!(
        p.review_fingerprint().unwrap(),
        format!("{}4100000000000000", "ab".repeat(24))
    );
    let tag = review::Fingerprint::of(&p);
    assert_eq!((tag.content, tag.total), ([0xab; 16], 65));
    // a layout of before is refused whole
    let mut old = bytes(65);
    old[8..12].copy_from_slice(&4u32.to_le_bytes());
    fs::write(&f.path, &old).unwrap();
    assert!(Pack::open(&f.path, &AtomicUsize::new(0)).is_err());
}
#[test]
fn metadata_query_shares_geometry_members_but_not_its_copy_budget() {
    let f = Fixture::new(&geometry_bytes(65, 16384));
    let stop = AtomicUsize::new(0);
    let mut p = Pack::open(&f.path, &stop).unwrap();
    let b = [-1., -1., 2., 2.];
    let start = Cursor { check: 1, error: 0 };
    let geometry = p.query(b, None, None, start, 65, &stop).unwrap();
    assert_eq!(geometry.hits.len(), 64);
    assert_eq!(geometry.next.unwrap().error, 64);
    let metadata = p.query_info(b, None, None, start, 65, &stop).unwrap();
    assert_eq!(metadata.hits.len(), 65);
    for (i, h) in metadata.hits.iter().enumerate() {
        assert_eq!(
            (h.local, h.record.number, h.record.points),
            (i as u64, i as u64 + 1, 16384)
        );
        if i < geometry.hits.len() {
            assert_eq!(h.record, RecordInfo::from(&geometry.hits[i].violation));
        }
    }
}
#[test]
fn rule_list_and_step_intersect_selection_waive_and_bounds_before_limits() {
    use std::collections::BTreeSet;
    let mut b = bytes(130);
    let offset = b.len() - 168 + 4 * 8;
    let at = u64::from_le_bytes(b[offset..offset + 8].try_into().unwrap()) as usize;
    for i in 0..130 {
        b[at + i] = if i % 3 == 0 {
            1
        } else if i % 7 == 0 {
            2
        } else {
            0
        };
    }
    let f = Fixture::new(&b);
    let stop = AtomicUsize::new(0);
    let mut p = Pack::open(&f.path, &stop).unwrap();
    let all = p.errors(1, 0, 130, &stop).unwrap().hits;
    let some = BTreeSet::from([0, 2, 7, 63, 64, 66, 128, 129]);
    let empty = BTreeSet::new();
    for selected in [None, Some(&some), Some(&empty)] {
        for waived in [None, Some(true), Some(false)] {
            for bbox_um in [
                None,
                Some([0.63, 0., 0.66, 0.003]),
                Some([0.64, 0., 0.64, 0.]),
                Some([5., 5., 6., 6.]),
            ] {
                let expected: Vec<_> = all
                    .iter()
                    .filter(|h| {
                        selected.is_none_or(|s| s.contains(&h.local))
                            && waived.is_none_or(|w| (h.status == 1) == w)
                            && bbox_um.is_none_or(|b| {
                                let e = p.bbox_um(h.violation.bbox).unwrap();
                                b[0] <= e[2] && b[2] >= e[0] && b[1] <= e[3] && b[3] >= e[1]
                            })
                    })
                    .map(|h| h.local)
                    .collect();
                for limit in [1, 7, 64] {
                    let (mut start, mut found, mut scanned) = (0, Vec::new(), 0);
                    loop {
                        let page = p
                            .filtered_errors(
                                ListRequest {
                                    check: 1,
                                    start,
                                    waived,
                                    bbox_um,
                                    selected,
                                    limit,
                                },
                                &stop,
                            )
                            .unwrap();
                        assert!(page.hits.len() <= limit);
                        scanned += page.scanned;
                        for h in page.hits {
                            assert_eq!(
                                h.record,
                                RecordInfo::from(&all[h.local as usize].violation)
                            );
                            found.push(h.local);
                        }
                        let Some(next) = page.next else {
                            break;
                        };
                        assert!(next > start);
                        start = next;
                    }
                    assert_eq!(found, expected);
                    assert!(scanned <= 130);
                    if let Some(s) = selected {
                        assert!(scanned <= s.len() as u64);
                    }
                }
                for backwards in [false, true] {
                    for after in [None, Some(0), Some(63), Some(64), Some(129)] {
                        let expected_hit = if backwards {
                            after
                                .and_then(|a| expected.iter().rev().copied().find(|&n| n < a))
                                .or_else(|| expected.last().copied())
                        } else {
                            after
                                .and_then(|a| expected.iter().copied().find(|&n| n > a))
                                .or_else(|| expected.first().copied())
                        };
                        let page = p
                            .filtered_step(
                                StepRequest {
                                    check: 1,
                                    backwards,
                                    after,
                                    waived,
                                    bbox_um,
                                    cursor: None,
                                },
                                selected,
                                &stop,
                            )
                            .unwrap();
                        assert!(page.next.is_none());
                        assert_eq!(page.hit.map(|h| h.local), expected_hit);
                        assert!(page.scanned <= selected.map_or(130, |s| s.len()) as u64);
                    }
                }
            }
        }
    }
}
#[test]
fn filtered_reads_reject_invalid_empty_filters_cancel_and_truncate() {
    use std::collections::BTreeSet;
    let f = Fixture::new(&bytes(130));
    let stop = AtomicUsize::new(0);
    let mut p = Pack::open(&f.path, &stop).unwrap();
    let empty = BTreeSet::new();
    for (check, start, limit, bbox) in [
        (3, 0, 1, None),
        (1, 131, 1, None),
        (1, 0, 65, None),
        (1, 0, 0, None),
        (0, 0, 1, Some([f64::NAN, 0., 1., 1.])),
        (1, 0, 1, Some([1., 0., 0., 1.])),
    ] {
        assert!(p
            .filtered_errors(
                ListRequest {
                    check,
                    start,
                    limit,
                    bbox_um: bbox,
                    waived: None,
                    selected: Some(&empty)
                },
                &stop
            )
            .is_err());
    }
    for ids in [BTreeSet::from([130]), (0..5001).collect()] {
        assert!(p
            .filtered_errors(
                ListRequest {
                    check: 1,
                    start: 0,
                    limit: 64,
                    bbox_um: None,
                    waived: None,
                    selected: Some(&ids)
                },
                &stop
            )
            .is_err());
    }
    assert!(p
        .filtered_step(
            StepRequest {
                check: 1,
                cursor: Some(StepCursor {
                    next: 0,
                    remaining: 130
                }),
                ..Default::default()
            },
            Some(&empty),
            &stop
        )
        .is_err());
    stop.store(1, Ordering::Relaxed);
    assert!(p
        .filtered_errors(
            ListRequest {
                check: 1,
                start: 0,
                limit: 64,
                bbox_um: None,
                waived: None,
                selected: Some(&empty)
            },
            &stop
        )
        .is_err());
    stop.store(0, Ordering::Relaxed);
    fs::OpenOptions::new()
        .write(true)
        .open(&f.path)
        .unwrap()
        .set_len(1)
        .unwrap();
    assert!(p
        .filtered_errors(
            ListRequest {
                check: 1,
                start: 0,
                limit: 64,
                bbox_um: None,
                waived: None,
                selected: None
            },
            &stop
        )
        .is_err());
}
#[test]
fn large_record_metadata_and_point_pages_do_not_clone_the_whole_record() {
    let f = Fixture::new(&geometry_bytes(1, 5000));
    let flag = AtomicUsize::new(0);
    let mut p = Pack::open(&f.path, &flag).unwrap();
    let meta = p.error_info(1, 0, &flag).unwrap();
    assert_eq!(
        (meta.number, meta.kind, meta.points, meta.bbox),
        (1, 'p', 5000, [0, 0, 2, 3])
    );
    let mut points = Vec::new();
    let mut start = 0;
    loop {
        let page = p.error_points(1, 0, start, 2048, &flag).unwrap();
        assert_eq!(page.start, start);
        assert_eq!(page.record, meta);
        assert!(page.points.len() <= 2048);
        points.extend(page.points);
        let Some(next) = page.next else {
            break;
        };
        assert!(next > start);
        start = next;
    }
    assert_eq!(points, p.error(1, 0, &flag).unwrap().points);
    assert_eq!(p.decoded_blocks, 1);
    assert!(p.error_points(1, 0, 5001, 1, &flag).is_err());
    assert!(p.error_points(1, 0, 0, 2049, &flag).is_err());
    assert!(p.error_points(1, 0, usize::MAX, 1, &flag).is_err());
    assert!(p
        .error_points(1, 0, 5000, 1, &flag)
        .unwrap()
        .points
        .is_empty());
    flag.store(1, Ordering::Relaxed);
    assert_eq!(
        p.error_info(1, 0, &flag).unwrap_err().kind,
        ErrorKind::Cancelled
    );
}
#[test]
fn selection_candidates_are_page_bounded_and_do_not_clone_coordinates() {
    let f = Fixture::new(&bytes(130));
    let stop = AtomicUsize::new(0);
    let mut p = Pack::open(&f.path, &stop).unwrap();
    // One point touches the left bbox corner; its marker center is elsewhere.
    let hits = p
        .selection_candidates(
            1,
            &[129, 63, 63, 64, 0],
            Some([0.63, 0., 0.63, 0.]),
            None,
            &stop,
        )
        .unwrap();
    assert_eq!(hits.iter().map(|h| h.local).collect::<Vec<_>>(), vec![63]);
    assert!(p
        .selection_candidates(1, &[63], Some([0.633, 0., 0.64, 0.003]), None, &stop)
        .unwrap()
        .is_empty());
    // A wide box only returns the provided page members, not the whole rule.
    assert_eq!(
        p.selection_candidates(1, &[129, 63, 63, 64, 0], None, None, &stop)
            .unwrap()
            .iter()
            .map(|h| h.local)
            .collect::<Vec<_>>(),
        vec![0, 63, 64, 129]
    );
    assert!(p
        .selection_candidates(1, &[0; 65], None, None, &stop)
        .is_err());
    assert!(p
        .selection_candidates(1, &[u64::MAX], None, Some(true), &stop)
        .is_err());
    assert!(p.selection_candidates(3, &[], None, None, &stop).is_err());
    assert!(p
        .selection_candidates(1, &[0], Some([0., 0., -1., 1.]), None, &stop)
        .is_err());
    stop.store(1, Ordering::Relaxed);
    assert_eq!(
        p.selection_candidates(1, &[0], None, None, &stop)
            .unwrap_err()
            .kind,
        ErrorKind::Cancelled
    );
    stop.store(0, Ordering::Relaxed);
    fs::write(&f.path, b"truncated").unwrap();
    assert!(p.selection_candidates(1, &[0], None, None, &stop).is_err());
    let large = Fixture::new(&geometry_bytes(1, 5000));
    let mut p = Pack::open(&large.path, &stop).unwrap();
    let metadata = p.selection_candidates(1, &[0], None, None, &stop).unwrap();
    assert_eq!(metadata[0].record.points, 5000);
    assert_eq!(p.decoded_blocks, 1);
}
#[test]
fn circular_step_matches_filtered_file_order_across_blocks_and_resumes() {
    let mut b = bytes(130);
    let offset = b.len() - 168 + 4 * 8;
    let statuses = u64::from_le_bytes(b[offset..offset + 8].try_into().unwrap()) as usize;
    for i in 0..130 {
        b[statuses + i] = if i % 3 == 0 {
            1
        } else if i % 7 == 0 {
            2
        } else {
            0
        };
    }
    // Deliberately leave waived counters zero: the status bytes are truth.
    let f = Fixture::new(&b);
    let flag = AtomicUsize::new(0);
    let mut p = Pack::open(&f.path, &flag).unwrap();
    let all = p.errors(1, 0, 130, &flag).unwrap().hits;
    for backwards in [false, true] {
        for after in [
            None,
            Some(0),
            Some(62),
            Some(63),
            Some(64),
            Some(128),
            Some(129),
        ] {
            for waived in [None, Some(false), Some(true)] {
                for bbox_um in [
                    None,
                    Some([0.631, -1., 1.271, 1.]),
                    Some([0.0031, 0., 0.0032, 0.001]),
                ] {
                    let start = after.map_or(if backwards { 129 } else { 0 }, |i| {
                        if backwards {
                            (i + 129) % 130
                        } else {
                            (i + 1) % 130
                        }
                    });
                    let expected = (0..130)
                        .map(|i| {
                            if backwards {
                                (start + 130 - i) % 130
                            } else {
                                (start + i) % 130
                            }
                        })
                        .find(|i| {
                            let h = &all[*i as usize];
                            waived.is_none_or(|w| (h.status == 1) == w)
                                && bbox_um.is_none_or(|b| {
                                    let e = p.bbox_um(h.violation.bbox).unwrap();
                                    b[0] <= e[2] && b[2] >= e[0] && b[1] <= e[3] && b[3] >= e[1]
                                })
                        });
                    for limit in [1, 7, SCAN_ITEMS] {
                        let mut req = StepRequest {
                            check: 1,
                            backwards,
                            after,
                            waived,
                            bbox_um,
                            cursor: None,
                        };
                        let mut scanned = 0;
                        loop {
                            let page = p.step_with_limit(req, limit, &flag).unwrap();
                            assert!(page.scanned <= limit);
                            scanned += page.scanned;
                            assert!(scanned <= 130);
                            if let Some(next) = page.next {
                                assert!(page.hit.is_none() && page.scanned > 0);
                                assert_eq!(next.remaining, 130 - scanned);
                                req.after = None;
                                req.cursor = Some(next);
                                continue;
                            }
                            assert_eq!(page.hit.map(|h| h.local), expected, "{req:?}");
                            if let Some(h) = page.hit {
                                assert_eq!(
                                    h.record,
                                    RecordInfo::from(&all[h.local as usize].violation)
                                );
                                assert_eq!(h.status, all[h.local as usize].status);
                            }
                            break;
                        }
                    }
                }
            }
        }
    }
    for check in [0, 2] {
        let page = p
            .step(
                StepRequest {
                    check,
                    ..Default::default()
                },
                &flag,
            )
            .unwrap();
        assert!(page.hit.is_none() && page.next.is_none());
        assert_eq!(page.scanned, 0);
    }
    let req = StepRequest {
        check: 1,
        ..Default::default()
    };
    for invalid in [
        StepRequest { check: 3, ..req },
        StepRequest {
            after: Some(130),
            ..req
        },
        StepRequest {
            cursor: Some(StepCursor {
                next: 130,
                remaining: 1,
            }),
            ..req
        },
        StepRequest {
            cursor: Some(StepCursor {
                next: 0,
                remaining: 131,
            }),
            ..req
        },
        StepRequest {
            cursor: Some(StepCursor {
                next: 0,
                remaining: 0,
            }),
            ..req
        },
        StepRequest {
            after: Some(0),
            cursor: Some(StepCursor {
                next: 1,
                remaining: 130,
            }),
            ..req
        },
        StepRequest {
            bbox_um: Some([0., 0., f64::NAN, 1.]),
            ..req
        },
        StepRequest {
            bbox_um: Some([0., 0., -1., 1.]),
            ..req
        },
    ] {
        assert!(p.step(invalid, &flag).is_err());
    }
    assert!(p.step_with_limit(req, 0, &flag).is_err());
    flag.store(1, Ordering::Relaxed);
    assert_eq!(p.step(req, &flag).unwrap_err().kind, ErrorKind::Cancelled);
    assert_eq!(fs::read(&f.path).unwrap(), b);
}
#[test]
fn sparse_step_exposes_incomplete_search_without_decoding_unmatched_statuses() {
    let f = Fixture::new(&bytes(SCAN_ITEMS + 64));
    let flag = AtomicUsize::new(0);
    let mut p = Pack::open(&f.path, &flag).unwrap();
    for backwards in [false, true] {
        let request = StepRequest {
            check: 1,
            backwards,
            after: Some(31),
            waived: Some(true),
            ..Default::default()
        };
        let first = p.step(request, &flag).unwrap();
        assert!(first.hit.is_none() && first.next.is_some());
        assert!(first.scanned > 0 && first.scanned <= SCAN_ITEMS);
        let last = p
            .step(
                StepRequest {
                    after: None,
                    cursor: first.next,
                    ..request
                },
                &flag,
            )
            .unwrap();
        assert!(last.hit.is_none() && last.next.is_none());
        assert_eq!(first.scanned + last.scanned, SCAN_ITEMS + 64);
        assert_eq!(p.decoded_blocks, 0);
    }
}
#[test]
fn lazy_decode_numbering_and_resumable_spatial_pages() {
    let f = Fixture::new(&bytes(130));
    let flag = AtomicUsize::new(0);
    let mut p = Pack::open(&f.path, &flag).unwrap();
    assert_eq!(p.total, 130);
    assert_eq!(p.cell, "TOP");
    assert_eq!(p.checks[1].desc, "rule text");
    assert_eq!(p.decoded_blocks, 0);
    assert_eq!(p.error(1, 129, &flag).unwrap().number, 130);
    assert_eq!(p.decoded_blocks, 1);
    assert_eq!(
        p.error(1, 128, &flag).unwrap().points,
        [[1280, 0], [1282, 3]]
    );
    assert_eq!(p.decoded_blocks, 1);
    let mut cursor = Cursor::default();
    let mut found = Vec::new();
    loop {
        let page = p
            .query([0.645, -1., 1.30, 1.], None, None, cursor, 7, &flag)
            .unwrap();
        found.extend(page.hits.iter().map(|h| (h.local, h.violation.number)));
        if let Some(next) = page.next {
            assert_ne!(next, cursor);
            cursor = next;
        } else {
            break;
        }
    }
    assert_eq!(found, (65..130).map(|i| (i, i + 1)).collect::<Vec<_>>());
    assert!(p.error(0, 0, &flag).is_err());
    assert!(p.error(1, 130, &flag).is_err());
    assert!(p
        .query(
            [f64::NAN, 0., 1., 1.],
            None,
            None,
            Cursor::default(),
            1,
            &flag
        )
        .is_err());
    assert!(p
        .query([0., 0., 1., 1.], None, None, Cursor::default(), 0, &flag)
        .is_err());
    flag.store(2, Ordering::Relaxed);
    assert_eq!(
        p.error(1, 0, &flag).unwrap_err().kind,
        crate::ErrorKind::Cancelled
    );
}
#[test]
fn waive_filter_is_inside_query_and_never_writes_pack_or_sidecar() {
    let original = bytes(130);
    let f = Fixture::new(&original);
    let flag = AtomicUsize::new(0);
    let mut p = Pack::open(&f.path, &flag).unwrap();
    let mut side = Vec::new();
    side.extend(b"FLOEWAIV");
    push32(&mut side, 2);
    side.extend([0xab; 16]);
    push64(&mut side, 130);
    push32(&mut side, 3);
    side.extend([0; 130]);
    side[40 + 128] = 1;
    side[40 + 129] = 2;
    for v in [0, 1, 0] {
        push32(&mut side, v);
    }
    let w = f.dir.join("review.waive");
    fs::write(&w, &side).unwrap();
    p.attach_waives(&w).unwrap();
    assert_eq!(p.waived_count(1).unwrap(), 1);
    assert_eq!(p.status(1, 129).unwrap(), 2);
    let page = p
        .query(
            [-1., -1., 2., 2.],
            None,
            Some(true),
            Cursor::default(),
            1,
            &flag,
        )
        .unwrap();
    assert_eq!(page.hits.len(), 1);
    assert_eq!(page.hits[0].local, 128);
    assert_eq!(fs::read(&w).unwrap(), side);
    assert_eq!(fs::read(&f.path).unwrap(), original);
    side[12] ^= 1;
    fs::write(&w, &side).unwrap();
    assert!(p.unchanged().is_err());
    assert!(Pack::open(&f.path, &flag)
        .unwrap()
        .attach_waives(&w)
        .is_err());
}
#[test]
fn damaged_packs_fail_without_panic_or_unbounded_allocation() {
    let original = bytes(66);
    let flag = AtomicUsize::new(0);
    for size in [0, 12, 40, 175, original.len() - 1] {
        let f = Fixture::new(&original[..size]);
        assert!(Pack::open(&f.path, &flag).is_err());
    }
    for field in 0..15 {
        let mut b = original.clone();
        let off = b.len() - 168 + field * 8;
        b[off..off + 8].copy_from_slice(&u64::MAX.to_le_bytes());
        let f = Fixture::new(&b);
        assert!(Pack::open(&f.path, &flag).is_err(), "footer {field}");
    }
    for (off, value) in [(8, 3), (12, 9), (40, 0), (40, 255)] {
        let mut b = original.clone();
        b[off] = value;
        let f = Fixture::new(&b);
        if let Ok(mut p) = Pack::open(&f.path, &flag) {
            assert!(p.error(1, 0, &flag).is_err());
        }
    }
    let mut b = original.clone();
    b[16..24].copy_from_slice(&f64::NAN.to_le_bytes());
    let f = Fixture::new(&b);
    assert!(Pack::open(&f.path, &flag).is_err());
    // A truncated source must not be dereferenced through a stale mmap/cache.
    let f = Fixture::new(&original);
    let mut p = Pack::open(&f.path, &flag).unwrap();
    p.error(1, 0, &flag).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(&f.path)
        .unwrap()
        .set_len(10)
        .unwrap();
    assert!(p.error(1, 0, &flag).is_err());
}
#[test]
fn sparse_empty_query_has_a_bounded_scan_and_continuation() {
    let f = Fixture::new(&bytes(SCAN_ITEMS + 64));
    let flag = AtomicUsize::new(0);
    let mut p = Pack::open(&f.path, &flag).unwrap();
    let first = p
        .query(
            [0.0031, -1., (SCAN_ITEMS * 10) as f64 / 1000., 1.],
            None,
            Some(true),
            Cursor::default(),
            20,
            &flag,
        )
        .unwrap();
    assert!(first.hits.is_empty());
    assert!(first.next.is_some());
    assert!(first.scanned <= SCAN_ITEMS);
    assert_eq!(p.decoded_blocks, 0);
    let next = p
        .query(
            [0.0031, -1., (SCAN_ITEMS * 10) as f64 / 1000., 1.],
            None,
            Some(true),
            first.next.unwrap(),
            20,
            &flag,
        )
        .unwrap();
    assert!(next.next.is_none());
}

#[test]
fn reviewer_paths_are_bounded_and_deterministic() {
    let p = std::env::temp_dir().join("한 글.db.ice");
    let paths = waive_paths(&p, "engineer-1").unwrap();
    assert_eq!(paths[0].file_name().unwrap(), ".한 글.db.waive.engineer-1");
    assert_eq!(paths, waive_paths(&p, "engineer-1").unwrap());
    let new = p.with_file_name(".한 글.db.tray");
    assert_eq!(paths, waive_paths(&new, "engineer-1").unwrap());
    for kind in [review::store::Kind::Waives, review::store::Kind::Notes] {
        assert_eq!(
            review::store::read_paths(&p, "engineer-1", kind).unwrap(),
            review::store::read_paths(&new, "engineer-1", kind).unwrap(),
        );
    }
    assert!(waive_paths(&p, "../other").is_err());
    assert!(waive_paths(&p, &"x".repeat(201)).is_err());
    assert_eq!(reviewer_tag(Some("  Kim / 김  ")), "Kim___김");
    // an id given is said as given, whatever the environment; one taken
    // from where the viewer runs is not a stated one (2026-11-10)
    let who = reviewer(Some(" kim@corp "));
    assert_eq!(who.tag, "kim_corp");
    assert!(who.source == ReviewerSource::Given && who.source.stated());
    assert!(ReviewerSource::Environment.stated());
    for guessed in [ReviewerSource::Display, ReviewerSource::Ssh, ReviewerSource::Account] {
        assert!(!guessed.stated(), "{}", guessed.text());
    }
}

fn replace_one_blob(blob: &[u8]) -> Vec<u8> {
    let mut b = bytes(1);
    let foot = b.len() - 168;
    let old = u64::from_le_bytes(b[foot + 16..foot + 24].try_into().unwrap()) as usize;
    let shift = blob.len() as i64 - (old - 40) as i64;
    b.splice(40..old, blob.iter().copied());
    let foot = b.len() - 168;
    for field in [1, 2, 4, 5, 6, 8, 10, 12] {
        let off = foot + field * 8;
        let v = u64::from_le_bytes(b[off..off + 8].try_into().unwrap());
        b[off..off + 8].copy_from_slice(&v.checked_add_signed(shift).unwrap().to_le_bytes());
    }
    b
}
#[test]
fn varint_coordinate_overflow_and_trailing_bytes_are_explicit() {
    let flag = AtomicUsize::new(0);
    let mut overflow = Vec::new();
    uv(&mut overflow, 4);
    zz(&mut overflow, i64::MAX);
    zz(&mut overflow, 0);
    zz(&mut overflow, 1);
    zz(&mut overflow, 0);
    let f = Fixture::new(&replace_one_blob(&overflow));
    let mut p = Pack::open(&f.path, &flag).unwrap();
    assert!(p
        .error(1, 0, &flag)
        .unwrap_err()
        .message
        .contains("delta overflow"));
    for blob in [
        vec![255; 10],
        vec![132, 0, 0, 0, 4, 6],
        vec![4, 0, 0, 4, 6, 0],
    ] {
        let f = Fixture::new(&replace_one_blob(&blob));
        let mut p = Pack::open(&f.path, &flag).unwrap();
        assert!(p.error(1, 0, &flag).is_err());
    }
    let b = bytes(66);
    for i in (0..b.len()).step_by(7) {
        let mut altered = b.clone();
        altered[i] ^= 255;
        let f = Fixture::new(&altered);
        if let Ok(mut p) = Pack::open(&f.path, &flag) {
            for ci in 0..p.checks.len() {
                let _ = p.errors(ci, 0, 7, &flag);
            }
            let _ = p.query(
                [-10., -10., 10., 10.],
                None,
                None,
                Cursor::default(),
                7,
                &flag,
            );
        }
    }
}

/// User 2026-11-13 ("waive ... 처리할 때 사유를 입력할 수 있게 ... error마다
/// 동일한 note가 생겨서"): a waive's reason is the action's, kept once in the
/// reviewer's reasons file, and it goes where the status goes - every writer
/// of statuses drops the reason of an error it turns back or changes without
/// one, so a reason is only ever shown of an error that is waived.
#[test]
fn a_waive_reason_is_the_actions_and_goes_with_the_status() {
    use super::analysis::scope::Selection;
    use super::desktop::{Review, STATUS_NONE, STATUS_WAIVED};
    use super::reasons::Kind;
    let dir = std::env::temp_dir().join(format!(
        "floe-waive-reason-{}-{}",
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&dir).unwrap();
    let pack = dir.join(".results.db.tray");
    fs::write(&pack, bytes(8)).unwrap();
    let stop = AtomicUsize::new(0);
    let file = dir.join(".results.db.reasons.kim");
    fn why(r: &Review, error: u64) -> Option<String> {
        r.waive_reason(1, error).unwrap().map(|e| e.text.clone())
    }
    let lines = |file: &std::path::Path| -> Vec<String> {
        fs::read_to_string(file)
            .unwrap()
            .lines()
            .skip(3)
            // (the time of each line is the clock's)
            .map(|l| {
                let mut p = l.splitn(4, ' ');
                format!("{} {} {}", p.next().unwrap(), p.next().unwrap(), p.nth(1).unwrap())
            })
            .collect()
    };
    let mut r = Review::open(&pack, None, Some("kim"), &stop).unwrap();
    assert!(!r.read_only(), "{:?}", r.notices);
    // the file is the waive sidecar's neighbour, and is not made by the open
    assert_eq!(r.reasons.path, file);
    assert!(dir.join(".results.db.waive.kim").is_file() && !file.exists());
    // waived without a reason: nothing is written of it
    r.set_status(1, &[0, 1], STATUS_WAIVED).unwrap();
    assert!(!file.exists() && why(&r, 0).is_none() && why(&r, 1).is_none());
    // with one: every error of the action holds it - one waived before too
    r.set_status(1, &[1, 2, 3], STATUS_WAIVED).unwrap();
    r.reason_set(Kind::Waive, 1, &[(1, 4)], "  dummy fill\n").unwrap();
    assert_eq!(
        (0..5).map(|e| why(&r, e)).collect::<Vec<_>>(),
        [None, Some("dummy fill".into()), Some("dummy fill".into()), Some("dummy fill".into()), None]
    );
    let head = fs::read_to_string(&file).unwrap();
    assert!(head.starts_with("# floe drc review reasons\nfloe_reasons=1\nfloe_pack="), "{head}");
    assert_eq!(lines(&file), ["w 1 1,3 dummy fill"]);
    // turned back: the reason goes with the status, and the file says so
    r.set_status(1, &[2], STATUS_NONE).unwrap();
    assert_eq!(why(&r, 2), None);
    assert_eq!(lines(&file), ["w 1 1,3 dummy fill", "W 1 2,1"]);
    // waived again without a reason: it has none - not the one it had
    r.set_status(1, &[2], STATUS_WAIVED).unwrap();
    assert_eq!(why(&r, 2), None);
    assert_eq!(why(&r, 3), Some("dummy fill".into()));
    assert_eq!(lines(&file).len(), 2);
    // a reason given again replaces the one held
    r.set_status(1, &[3], STATUS_WAIVED).unwrap();
    r.reason_set(Kind::Waive, 1, &[(3, 4)], "seal ring").unwrap();
    assert_eq!((why(&r, 1), why(&r, 3)), (Some("dummy fill".into()), Some("seal ring".into())));
    // a job's chunks (the service's review job): turned back a chunk at a
    // time, gone at once, said once when the job's flush comes
    let all = Selection::from_indices(8, 0..8);
    assert_eq!(r.set_status_range(1, &all, STATUS_NONE, 0, 4, &mut Default::default()).unwrap(), 4);
    assert_eq!(r.set_status_range(1, &all, STATUS_NONE, 4, 4, &mut Default::default()).unwrap(), 0);
    assert!((0..8).all(|e| why(&r, e).is_none()));
    assert_eq!(lines(&file).len(), 3, "not said before the flush");
    r.sync_waives().unwrap();
    assert_eq!(lines(&file).last().unwrap(), "W 1 1,1,1,1", "errors 1 and 3 held one");
    // a span an application writes (a peer's decision, a previous run's):
    // the reason of what it changes goes, the rest stays
    r.set_status(1, &[4, 5, 6], STATUS_WAIVED).unwrap();
    r.reason_set(Kind::Waive, 1, &[(4, 7)], "seal ring").unwrap();
    let before = r.statuses(1, 4, 3).unwrap();
    r.write_status_span(1, 4, &before, &[STATUS_WAIVED, STATUS_NONE, STATUS_WAIVED]).unwrap();
    r.sync_waives().unwrap();
    assert_eq!(
        (4..7).map(|e| why(&r, e)).collect::<Vec<_>>(),
        [Some("seal ring".into()), None, Some("seal ring".into())]
    );
    assert_eq!(lines(&file).last().unwrap(), "W 1 5,1");
    // a saved waive file loaded over the review: a reason stays with an
    // error the file has waived too, and goes with one it turns back
    let saved = dir.join("saved.waive");
    r.waive_export(&saved).unwrap(); // 4 and 6 waived
    r.set_status(1, &[4], STATUS_NONE).unwrap();
    r.set_status(1, &[7], STATUS_WAIVED).unwrap();
    r.reason_set(Kind::Waive, 1, &[(7, 8)], "later").unwrap();
    assert_eq!(r.waive_import(&saved).unwrap(), (2, Default::default()));
    assert_eq!(why(&r, 4), None, "waived again by the file: turned back since, no reason");
    assert_eq!(why(&r, 6), Some("seal ring".into()));
    assert_eq!(why(&r, 7), None, "turned back by the file");
    assert_eq!(r.status(1, 7).unwrap(), STATUS_NONE);
    // the other kind's reasons are apart from the statuses
    r.reason_set(Kind::NotRequired, 1, &[(0, 2)], "out of scope").unwrap();
    r.set_status(1, &[0, 1], STATUS_WAIVED).unwrap();
    r.set_status(1, &[0, 1], STATUS_NONE).unwrap();
    assert_eq!(r.reasons.of(Kind::NotRequired, 1, 1).map(|e| e.text.as_str()), Some("out of scope"));
    r.reason_drop(Kind::NotRequired, 1, &[(1, 2)]);
    r.sync_waives().unwrap();
    assert_eq!(lines(&file).last().unwrap(), "N 1 1,1");
    assert!(r.notices.is_empty(), "{:?}", r.notices);
    drop(r);
    // read again: what was kept
    let said = fs::read_to_string(&file).unwrap();
    let r = Review::open(&pack, None, Some("kim"), &stop).unwrap();
    assert_eq!(why(&r, 6), Some("seal ring".into()));
    assert_eq!(r.reasons.of(Kind::NotRequired, 1, 0).map(|e| e.text.as_str()), Some("out of scope"));
    assert!(r.reasons.of(Kind::NotRequired, 1, 1).is_none());
    assert_eq!(fs::read_to_string(&file).unwrap(), said, "an open writes nothing");
    drop(r);
    // a reason a file holds for an error that is not waived - a build
    // before 0.12.463 turned it back, a run ended between its two writes -
    // is never shown: the readers ask the status
    fs::write(&file, format!("{said}w 1 99 0,1 left behind\n")).unwrap();
    let r = Review::open(&pack, None, Some("kim"), &stop).unwrap();
    assert_eq!(r.reasons.of(Kind::Waive, 1, 0).map(|e| e.text.as_str()), Some("left behind"));
    assert_eq!(r.status(1, 0).unwrap(), STATUS_NONE);
    assert_eq!(why(&r, 0), None);
    drop(r);
    // another reviewer's are another file
    let mut lee = Review::open(&pack, None, Some("lee"), &stop).unwrap();
    assert_eq!(lee.reasons.path, dir.join(".results.db.reasons.lee"));
    assert!(lee.reasons.is_empty());
    lee.set_status(1, &[6], STATUS_WAIVED).unwrap();
    assert_eq!(why(&lee, 6), None);
    drop(lee);
    fs::remove_dir_all(&dir).unwrap();
}

/// User 2026-11-13: "confirm된 waive는 되돌릴 수 없게 잠구고 confirmer만 되돌릴
/// 수 있게 해줘" - a waive a confirmer has confirmed is not turned back by a
/// session that is no confirmer's, whichever writer of statuses is asked (a
/// click, a job's chunk, an application's span, a saved file loaded), and
/// its reason is not the caller's to replace: the writers leave those errors
/// and say which. A confirmer's session is held by no lock, and takes its
/// own confirmation back with a waive it turns back.
#[test]
fn a_confirmed_waive_is_locked_and_only_a_confirmer_turns_it_back() {
    use super::analysis::scope::Selection;
    use super::desktop::{Kept, Left, Review, STATUS_NONE, STATUS_WAIVED};
    use super::reasons::Kind;
    let dir = std::env::temp_dir().join(format!(
        "floe-waive-lock-{}-{}",
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&dir).unwrap();
    let pack = dir.join(".results.db.tray");
    fs::write(&pack, bytes(8)).unwrap();
    let stop = AtomicUsize::new(0);
    let open = |who: &str, confirmer: bool| {
        // (a confirmer here is a session of every role - `floe2 view --role
        // requester,waiver,confirmer`: it waives as a reviewer does, and
        // confirms)
        let roles = if confirmer { super::local::Roles::ALL } else { super::local::Roles::NONE };
        let mut r = Review::open_as(&pack, None, Some(who), roles, &stop).unwrap();
        assert!(!r.read_only(), "{:?}", r.notices);
        // (the confirmers' files are looked for at each ask here)
        r.set_lock_refresh(std::time::Duration::ZERO);
        r
    };
    fn why(r: &Review, error: u64) -> Option<String> {
        r.waive_reason(1, error).unwrap().map(|e| e.text.clone())
    }
    let (w, o) = (STATUS_WAIVED, STATUS_NONE);
    let mut kim = open("kim", false);
    let mut hong = open("hong", true);
    // nobody has confirmed: nothing is locked, a waive is turned back freely
    assert!(kim.set_status(1, &[0, 1, 2, 3, 4, 5], w).unwrap().is_empty());
    kim.reason_set(Kind::Waive, 1, &[(0, 6)], "dummy fill").unwrap();
    assert!(kim.locked(1, 0, 8).unwrap().is_none() && kim.confirmed_by(1, 0).unwrap().is_empty());
    assert!(kim.set_status(1, &[5], o).unwrap().is_empty());
    assert_eq!(kim.statuses(1, 0, 8).unwrap(), [w, w, w, w, w, o, o, o]);
    // hong confirms 0, 1, 2 - and 7, which nobody has waived
    assert_eq!(hong.confirm_set(1, &[0, 1, 2, 7], true).unwrap(), 4);
    assert_eq!(kim.confirmed_by(1, 0).unwrap(), ["hong"]);
    assert_eq!(
        kim.locked(1, 0, 8).unwrap(),
        Some(vec![true, true, true, false, false, false, false, true])
    );
    // a click: the confirmed waives stay and are said, the others go
    assert_eq!(kim.set_status(1, &[3, 1, 0, 1], o).unwrap().locked, [0, 1]);
    assert_eq!(kim.statuses(1, 0, 8).unwrap(), [w, w, w, o, w, o, o, o]);
    assert_eq!(kim.status_counts(1).unwrap().0, 4);
    assert_eq!((why(&kim, 0), why(&kim, 1), why(&kim, 3)), (Some("dummy fill".into()), Some("dummy fill".into()), None));
    // waived again: the locked one is said too - its reason is not the
    // caller's to replace, who gives the action's to the others
    assert_eq!(kim.set_status(1, &[0, 3], w).unwrap().locked, [0]);
    kim.reason_set(Kind::Waive, 1, &[(3, 4)], "seal ring").unwrap();
    assert_eq!((why(&kim, 0), why(&kim, 3)), (Some("dummy fill".into()), Some("seal ring".into())));
    // an error confirmed while nobody has it waived: waiving it is free,
    // and it is locked from then
    assert!(kim.set_status(1, &[7], w).unwrap().is_empty());
    assert_eq!(kim.set_status(1, &[7], o).unwrap().locked, [7]);
    assert_eq!(kim.status(1, 7).unwrap(), w);
    // a job's chunk: what is locked is left, as runs
    let all = Selection::from_indices(8, 0..8);
    let mut kept: Left<(u64, u64)> = Left::default();
    assert_eq!(kim.set_status_range(1, &all, o, 0, 8, &mut kept).unwrap(), 2);
    assert_eq!(kept.locked, [(0, 3), (7, 8)]);
    assert_eq!(kim.statuses(1, 0, 8).unwrap(), [w, w, w, o, o, o, o, w]);
    assert_eq!(kim.status_counts(1).unwrap().0, 4);
    assert_eq!((why(&kim, 2), why(&kim, 3), why(&kim, 4)), (Some("dummy fill".into()), None, None));
    // (chunk after chunk: the runs join)
    kept = Left::default();
    assert_eq!(kim.set_status_range(1, &all, o, 0, 2, &mut kept).unwrap(), 0);
    assert_eq!(kim.set_status_range(1, &all, o, 2, 6, &mut kept).unwrap(), 0);
    assert_eq!(kept.locked, [(0, 3), (7, 8)]);
    // a job that waives says them too, for its reason's sake
    kept = Left::default();
    assert_eq!(kim.set_status_range(1, &all, w, 0, 8, &mut kept).unwrap(), 4);
    assert_eq!(kept.locked, [(0, 3), (7, 8)]);
    // a part of the rule: only its own members
    let some = Selection::from_indices(8, [1, 4]);
    kept = Left::default();
    assert_eq!(kim.set_status_range(1, &some, o, 0, 8, &mut kept).unwrap(), 1);
    assert_eq!((kept.locked.clone(), kim.status(1, 4).unwrap()), (vec![(1, 2)], o));
    kim.set_status(1, &[4], w).unwrap();
    // a span an application writes: the locked waives stay, and how many
    let before = kim.statuses(1, 0, 8).unwrap();
    assert_eq!(before, [w; 8]);
    assert_eq!(kim.write_status_span(1, 0, &before, &[o; 8]).unwrap().locked, 4);
    assert_eq!(kim.statuses(1, 0, 8).unwrap(), [w, w, w, o, o, o, o, w]);
    assert_eq!(kim.status_counts(1).unwrap().0, 4);
    // (one that turns nothing back keeps nothing)
    let before = kim.statuses(1, 0, 8).unwrap();
    assert_eq!(kim.write_status_span(1, 0, &before, &[w, w, w, w, o, o, o, w]).unwrap(), Kept::default());
    assert_eq!(kim.status_counts(1).unwrap().0, 5);
    // a saved file loaded - one with nothing waived: the locked waives stay
    // with their reasons, the other goes
    let saved = dir.join("nothing.waive");
    open("lee", false).waive_export(&saved).unwrap();
    assert_eq!(kim.waive_import(&saved).unwrap(), (4, Kept { locked: 4, rejected: 0, unrequested: 0 }));
    assert_eq!(kim.statuses(1, 0, 8).unwrap(), [w, w, w, o, o, o, o, w]);
    assert_eq!(kim.status_counts(1).unwrap().0, 4);
    assert_eq!((why(&kim, 0), why(&kim, 2)), (Some("dummy fill".into()), Some("dummy fill".into())));
    // the confirmer takes a confirmation back: that waive is free again
    assert_eq!(hong.confirm_set(1, &[1], false).unwrap(), 1);
    assert!(kim.set_status(1, &[1], o).unwrap().is_empty());
    assert_eq!(kim.status(1, 1).unwrap(), o);
    // two confirmers: one taking theirs back leaves the other's lock
    let mut park = open("park", true);
    assert_eq!(park.confirm_set(1, &[0], true).unwrap(), 1);
    assert_eq!(kim.confirmed_by(1, 0).unwrap(), ["hong", "park"]);
    assert_eq!(hong.confirm_set(1, &[0], false).unwrap(), 1);
    assert_eq!(kim.set_status(1, &[0], o).unwrap().locked, [0]);
    assert_eq!(kim.confirmed_by(1, 0).unwrap(), ["park"]);
    // the role is what turns it back: the same review opened with it
    drop(kim);
    let mut kim = open("kim", true);
    assert!(kim.locked(1, 0, 8).unwrap().is_none());
    assert!(kim.set_status(1, &[0], o).unwrap().is_empty());
    assert_eq!(kim.status(1, 0).unwrap(), o);
    drop(kim);
    let mut kim = open("kim", false);
    // (park's confirmation is left on an error nobody has waived: it counts
    // for nothing, and locks the error the moment it is waived again)
    assert!(kim.set_status(1, &[0], w).unwrap().is_empty());
    assert_eq!(kim.set_status(1, &[0], o).unwrap().locked, [0]);
    assert_eq!(kim.statuses(1, 0, 8).unwrap(), [w, o, w, o, o, o, o, w]);
    // a confirmer's own waives are theirs to turn back, confirmed or not -
    // and their own confirmation goes with the waive: a click,
    assert!(hong.set_status(1, &[2, 3], w).unwrap().is_empty());
    assert_eq!(hong.confirm_set(1, &[3], true).unwrap(), 1);
    assert_eq!(hong.confirms.counts()[1], 3, "2, 3 and 7");
    assert!(hong.set_status(1, &[3], o).unwrap().is_empty());
    assert!(!hong.confirms.get(1, 3).unwrap() && hong.confirms.get(1, 2).unwrap());
    // a job's chunk (which frees kim's waive of the same error),
    kept = Left::default();
    assert_eq!(hong.set_status_range(1, &all, o, 0, 8, &mut kept).unwrap(), 1);
    assert!(kept.is_empty() && !hong.confirms.get(1, 2).unwrap());
    assert_eq!(hong.confirms.counts()[1], 1, "7 alone");
    assert!(kim.set_status(1, &[2], o).unwrap().is_empty());
    // a span,
    assert!(hong.set_status(1, &[7], w).unwrap().is_empty());
    let before = hong.statuses(1, 4, 4).unwrap();
    assert_eq!(hong.write_status_span(1, 4, &before, &[o; 4]).unwrap(), Kept::default());
    assert_eq!((hong.status(1, 7).unwrap(), hong.confirms.counts()[1]), (o, 0));
    assert!(kim.set_status(1, &[7], o).unwrap().is_empty(), "nobody has 7 confirmed now");
    // and a saved file loaded: the confirmation of what it turns back alone
    assert!(hong.set_status(1, &[6], w).unwrap().is_empty());
    let saved = dir.join("six.waive");
    hong.waive_export(&saved).unwrap();
    assert!(hong.set_status(1, &[5], w).unwrap().is_empty());
    assert_eq!(hong.confirm_set(1, &[5, 6], true).unwrap(), 2);
    assert_eq!(hong.waive_import(&saved).unwrap(), (1, Kept::default()));
    assert!(!hong.confirms.get(1, 5).unwrap() && hong.confirms.get(1, 6).unwrap());
    assert_eq!(hong.confirms.counts()[1], 1);
    hong.sync_waives().unwrap();
    for r in [&kim, &hong, &park] {
        assert!(r.notices.is_empty(), "{:?}", r.notices);
    }
    drop((kim, hong, park));
    fs::remove_dir_all(&dir).unwrap();
}

/// User 2026-11-14: "confirmer가 다른 리뷰어의 waive를 반려할 수 있게 해줘",
/// and then: "reject 된 에러는 rejected로 분류되어야 ... 리뷰어가 사유를
/// 수정해서 다시 waive를 할 수 있게 하되, 이 때 이전의 waive사유나
/// 반려사유는 계속 유지되어야 ... REJECTED에서 ... waive로 WAIVED로 가도록" -
/// a confirmer rejects the waive of errors in a log of their own; the waive
/// stays in its reviewer's file with its reason and is a state of its own -
/// no waive in any count, not locked - until the reviewer waives the error
/// again with a reason, which keeps the rejection and the reason the waive
/// had; taken back, or confirmed, by its confirmer it is over too.
#[test]
fn a_rejected_waive_is_a_state_until_its_reviewer_waives_it_again() {
    use super::analysis::scope::Selection;
    use super::desktop::{Kept, Left, Lens, Review, STATUS_NONE, STATUS_WAIVED};
    use super::reasons::Kind;
    let dir = std::env::temp_dir().join(format!(
        "floe-waive-reject-{}-{}",
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&dir).unwrap();
    let pack = dir.join(".results.db.tray");
    fs::write(&pack, bytes(8)).unwrap();
    let stop = AtomicUsize::new(0);
    let open = |who: &str, confirmer: bool| {
        // (a confirmer here is a session of every role - `floe2 view --role
        // requester,waiver,confirmer`: it waives as a reviewer does, and
        // confirms)
        let roles = if confirmer { super::local::Roles::ALL } else { super::local::Roles::NONE };
        let mut r = Review::open_as(&pack, None, Some(who), roles, &stop).unwrap();
        assert!(!r.read_only(), "{:?}", r.notices);
        r.set_lock_refresh(std::time::Duration::ZERO);
        r
    };
    fn why(r: &Review, error: u64) -> Option<String> {
        r.waive_reason(1, error).unwrap().map(|e| e.text.clone())
    }
    // (what is kept of an error's waive: what happened, by whom, the words)
    fn past(r: &Review, error: u64) -> Vec<(&'static str, String, String)> {
        r.history(1, error).unwrap().into_iter().map(|e| (e.what, e.by, e.text)).collect()
    }
    let was = |what: &'static str, by: &str, text: &str| (what, by.to_owned(), text.to_owned());
    let who = |by: &[super::confirm::Rejected]| by.iter().map(|b| (b.reviewer.clone(), b.text.clone())).collect::<Vec<_>>();
    let lines = |path: &std::path::Path| -> Vec<String> { fs::read_to_string(path).unwrap().lines().map(str::to_owned).collect() };
    let (w, o) = (STATUS_WAIVED, STATUS_NONE);
    let mut kim = open("kim", false);
    let mut hong = open("hong", true);
    let (log, own) = (dir.join(".results.db.rejects.hong"), dir.join(".results.db.rejects.kim"));
    kim.set_status(1, &[0, 1, 2, 3, 4, 5], w).unwrap();
    kim.reason_set(Kind::Waive, 1, &[(0, 6)], "dummy fill").unwrap();
    hong.set_status(1, &[5], w).unwrap();
    assert_eq!(hong.confirm_set(1, &[0, 5], true).unwrap(), 2);
    // only a confirmer rejects, and nothing is made by a refusal or a look
    assert_eq!(kim.reject_set(1, &[(0, 2)], "no").unwrap_err().kind, crate::ErrorKind::Unsupported);
    assert_eq!(kim.reject_drop(1, &[(0, 2)]).unwrap_err().kind, crate::ErrorKind::Unsupported);
    let looked = kim.look().unwrap();
    assert!(looked.rejected.is_empty() && looked.rules.is_empty() && kim.rejected(1, 0, 8).unwrap().is_none());
    assert!(!log.exists() && !own.exists());
    assert_eq!(kim.states(1, 0, 8).unwrap(), [3, 1, 1, 1, 1, 3, 0, 0], "hong confirmed 0 and 5");
    // a rejection is one line of the confirmer's log, with why or without,
    // each with a time of its own
    hong.reject_set(1, &[(0, 2)], " not a dummy\n").unwrap();
    hong.reject_set(1, &[(5, 6)], "").unwrap();
    hong.reject_set(1, &[], "nothing").unwrap();
    assert!(hong.reject_set(1, &[(7, 9)], "past the rule").is_err());
    let said = lines(&log);
    assert_eq!(said.len(), 5, "{said:?}");
    assert!(said[0] == "# floe drc waive rejections" && said[1] == "floe_reasons=2" && said[2].starts_with("floe_pack="));
    let parts = |line: &str| line.split(' ').map(str::to_owned).collect::<Vec<_>>();
    assert_eq!((&parts(&said[3])[..2], &parts(&said[3])[3..]), (&["r".to_owned(), "1".into()][..], &["0,2".to_owned(), "not".into(), "a".into(), "dummy".into()][..]));
    assert_eq!((parts(&said[4]).len(), parts(&said[4])[3].as_str()), (4, "5,1"));
    let (first, second): (u64, u64) = (parts(&said[3])[2].parse().unwrap(), parts(&said[4])[2].parse().unwrap());
    assert!(first > 1_600_000_000 && second > first, "{first} {second}");
    // the confirmer's own waive of a rejected error goes with it, and their
    // own confirmation of what they reject
    assert_eq!((hong.status(1, 5).unwrap(), hong.status_counts(1).unwrap().0), (o, 0));
    assert!(!hong.confirms.get(1, 0).unwrap() && !hong.confirms.get(1, 5).unwrap());
    // the reviewer's session: nothing of theirs is written. Their waives
    // are rejected from the look that finds the confirmer's log (its first
    // rejection made a file - a waive asks no folder), which says them with
    // who rejected them and why
    assert!(kim.rejected(1, 0, 8).unwrap().is_none());
    let looked = kim.look().unwrap();
    assert_eq!((looked.rules.clone(), looked.changed, looked.rejected.len()), (vec![1], true, 1));
    let back = &looked.rejected[0];
    assert_eq!((back.check, back.count, back.runs.clone()), (1, 3, vec![(0, 2), (5, 6)]));
    assert_eq!(who(&back.by), [("hong".to_owned(), "not a dummy".to_owned()), ("hong".to_owned(), String::new())]);
    assert_eq!((back.by[0].time, back.by[1].time), (first, second));
    assert_eq!(kim.rejected(1, 0, 8).unwrap(), Some(vec![true, true, false, false, false, true, false, false]));
    // rejected is a state: no waive where waives are read, the waive and
    // its reason where the file is
    assert_eq!(kim.states(1, 0, 8).unwrap(), [2, 2, 1, 1, 1, 2, 0, 0]);
    assert_eq!(kim.statuses(1, 0, 8).unwrap(), [o, o, w, w, w, o, o, o]);
    assert_eq!((kim.status(1, 0).unwrap(), kim.status(1, 2).unwrap()), (o, w));
    assert_eq!((kim.status_counts(1).unwrap(), kim.state_counts(1).unwrap()), ((3, 8), [2, 3, 3, 0, 0, 0]));
    assert_eq!((why(&kim, 0), why(&kim, 2)), (Some("dummy fill".into()), Some("dummy fill".into())));
    assert!(!own.exists() && past(&kim, 0).is_empty());
    // looked again: nothing to say while the files stand
    let looked = kim.look().unwrap();
    assert!(looked.rejected.is_empty() && looked.rules.is_empty() && !looked.changed);
    // the class a filter shows: what "waived" means to every page and count
    let page = |r: &mut Review, lens: Lens, waived: bool| {
        r.set_lens(lens);
        (r.status_page(1, waived, 0, 10).unwrap(), r.lens_counts(1).unwrap().0)
    };
    assert_eq!(page(&mut kim, Lens::Rejected, true), (vec![0, 1, 5], 3));
    assert_eq!(kim.status_rank(1, true, 5).unwrap(), Some(2));
    assert_eq!(kim.lensed(1, 0, 8).unwrap(), [1, 1, 0, 0, 0, 1, 0, 0]);
    assert_eq!(page(&mut kim, Lens::Held, false), (vec![6, 7], 6), "not waived: no waive at all");
    assert_eq!(page(&mut kim, Lens::Waived, true), (vec![2, 3, 4], 3));
    assert_eq!(page(&mut kim, Lens::Confirmed, true), (vec![], 0));
    assert_eq!(page(&mut kim, Lens::Live, true), (vec![2, 3, 4], 3));
    assert_eq!(page(&mut kim, Lens::Live, false).0, [0, 1, 5, 6, 7]);
    // an error under a rejection is not waived by a write (the others are),
    assert_eq!(kim.set_status(1, &[1, 6, 0, 1], w).unwrap(), Left { locked: vec![], rejected: vec![0, 1], unrequested: vec![] });
    assert_eq!(kim.statuses(1, 0, 8).unwrap(), [o, o, w, w, w, o, w, o]);
    // it is waived again, with a reason: the rejection it answers is kept
    // in the reviewer's own log, with the reason the waive had
    assert_eq!(kim.resubmit(1, &[(0, 1)]).unwrap(), 1);
    kim.reason_set(Kind::Waive, 1, &[(0, 1)], "dummy fill, see rule note").unwrap();
    assert_eq!(kim.take_reclassed(), [1]);
    assert_eq!(kim.states(1, 0, 8).unwrap(), [1, 2, 1, 1, 1, 2, 1, 0]);
    assert_eq!((kim.status_counts(1).unwrap().0, why(&kim, 0)), (5, Some("dummy fill, see rule note".into())));
    assert_eq!(past(&kim, 0), [was("waived", "", "dummy fill"), was("rejected", "hong", "not a dummy")]);
    assert!(past(&kim, 1).is_empty(), "an error at a time");
    let said = lines(&own);
    assert_eq!(said.len(), 5, "{said:?}");
    assert!(said[0] == "# floe drc waive rejections" && said[1] == "floe_reasons=2");
    assert!(said[3].starts_with("p 1 ") && said[3].ends_with(" 0,1 dummy fill"), "{}", said[3]);
    assert_eq!(said[4], format!("j 1 {first} 0,1 hong not a dummy"));
    assert_eq!(kim.resubmit(1, &[(0, 1), (2, 5)]).unwrap(), 0, "nothing of them is under a rejection now");
    // the confirmer rejects it again: a new rejection, to answer anew
    hong.reject_set(1, &[(0, 1)], "still not").unwrap();
    let looked = kim.look().unwrap();
    assert_eq!(looked.rejected.len(), 1);
    assert_eq!((looked.rejected[0].runs.clone(), who(&looked.rejected[0].by)), (vec![(0, 1)], vec![("hong".to_owned(), "still not".to_owned())]));
    assert_eq!(kim.states(1, 0, 2).unwrap(), [2, 2]);
    assert_eq!(kim.set_status(1, &[0], w).unwrap().rejected, [0]);
    assert_eq!(kim.resubmit(1, &[(0, 1)]).unwrap(), 1);
    kim.reason_set(Kind::Waive, 1, &[(0, 1)], "third").unwrap();
    assert_eq!(
        past(&kim, 0),
        [
            was("waived", "", "dummy fill"),
            was("rejected", "hong", "not a dummy"),
            was("waived", "", "dummy fill, see rule note"),
            was("rejected", "hong", "still not"),
        ]
    );
    assert_eq!((kim.states(1, 0, 1).unwrap(), why(&kim, 0)), (vec![1], Some("third".into())));
    // a rejected waive is turned back like any other - its reason kept -
    // and waived again later it answers the rejection that still stands
    assert!(kim.set_status(1, &[1], o).unwrap().is_empty(), "a rejected waive is not locked");
    assert_eq!((kim.states(1, 1, 1).unwrap(), why(&kim, 1)), (vec![0], None));
    assert_eq!(past(&kim, 1), [was("waived", "", "dummy fill")]);
    assert_eq!(kim.set_status(1, &[1], w).unwrap().rejected, [1]);
    assert_eq!(kim.status(1, 1).unwrap(), o);
    assert_eq!(kim.resubmit(1, &[(1, 2)]).unwrap(), 1);
    assert_eq!((kim.states(1, 1, 1).unwrap(), why(&kim, 1)), (vec![1], None));
    assert_eq!(past(&kim, 1), [was("waived", "", "dummy fill"), was("rejected", "hong", "not a dummy")]);
    // a job's chunk leaves the rejected, and says them,
    let all = Selection::from_indices(8, 0..8);
    let mut left = Left::default();
    assert_eq!(kim.set_status_range(1, &all, w, 0, 8, &mut left).unwrap(), 1);
    assert_eq!(left, Left { locked: vec![], rejected: vec![(5, 6)], unrequested: vec![] });
    assert_eq!(kim.statuses(1, 0, 8).unwrap(), [w, w, w, w, w, o, w, w]);
    // an application's span too - and what it leaves as it read it stays as
    // the file has it: a rejected waive reads as none, and is none of its
    let before = kim.statuses(1, 0, 8).unwrap();
    assert_eq!(kim.write_status_span(1, 0, &before, &[w; 8]).unwrap(), Kept { locked: 0, rejected: 1, unrequested: 0 });
    assert_eq!(kim.statuses(1, 0, 8).unwrap(), before);
    assert_eq!(kim.write_status_span(1, 4, &before[4..6], &[o, o]).unwrap(), Kept::default());
    assert_eq!(kim.states(1, 4, 2).unwrap(), [0, 2]);
    assert!(kim.set_status(1, &[4], w).unwrap().is_empty());
    // a job turns a rejected waive back as well (its reason kept), and a
    // saved file with everything waived does not waive it again
    let five = Selection::from_indices(8, [5u64]);
    left = Left::default();
    assert_eq!(kim.set_status_range(1, &five, o, 0, 8, &mut left).unwrap(), 1);
    assert!(left.is_empty() && past(&kim, 5) == [was("waived", "", "dummy fill")]);
    let saved = dir.join("all.waive");
    kim.waive_export(&saved).unwrap();
    let mut raw = fs::read(&saved).unwrap();
    raw[40..48].fill(w);
    fs::write(&saved, &raw).unwrap();
    kim.set_status(1, &[2], o).unwrap();
    assert_eq!(kim.waive_import(&saved).unwrap(), (7, Kept { locked: 0, rejected: 1, unrequested: 0 }));
    assert_eq!(kim.states(1, 0, 8).unwrap(), [1, 1, 1, 1, 1, 0, 1, 1]);
    assert_eq!(kim.resubmit(1, &[(5, 6)]).unwrap(), 1);
    assert_eq!(past(&kim, 5), [was("waived", "", "dummy fill"), was("rejected", "hong", "")]);
    assert_eq!((kim.states(1, 0, 8).unwrap(), kim.rejected(1, 0, 8).unwrap()), (vec![1; 8], None));
    // a rejected waive is not locked, whoever confirmed it - and waived
    // again it is under the other confirmer's confirmation, which stood
    let mut park = open("park", true);
    assert_eq!(park.confirm_set(1, &[3], true).unwrap(), 1);
    assert_eq!(kim.set_status(1, &[3], o).unwrap().locked, [3], "confirmed by park: locked");
    assert_eq!(kim.states(1, 3, 1).unwrap(), [3]);
    hong.reject_set(1, &[(3, 4)], "still no").unwrap();
    assert!(kim.locked(1, 0, 8).unwrap().is_none());
    let looked = kim.look().unwrap();
    assert_eq!((looked.rejected[0].runs.clone(), who(&looked.rejected[0].by)), (vec![(3, 4)], vec![("hong".to_owned(), "still no".to_owned())]));
    assert_eq!((kim.states(1, 3, 1).unwrap(), kim.state_counts(1).unwrap()), (vec![2], [0, 7, 1, 0, 0, 0]));
    assert!(park.confirms.get(1, 3).unwrap(), "the other confirmer's confirmation is theirs to take back");
    assert_eq!(kim.resubmit(1, &[(3, 4)]).unwrap(), 1);
    kim.reason_set(Kind::Waive, 1, &[(3, 4)], "ok now").unwrap();
    assert_eq!(kim.states(1, 3, 1).unwrap(), [3]);
    assert_eq!(kim.set_status(1, &[3], o).unwrap().locked, [3]);
    // two confirmers' rejections of one error: both said, and one taking
    // theirs back leaves the other's; taken back by both, the waive is as
    // it was - at once, nothing of it having been touched
    park.reject_set(1, &[(0, 1)], "").unwrap();
    let looked = kim.look().unwrap();
    assert_eq!((looked.rejected[0].runs.clone(), who(&looked.rejected[0].by)), (vec![(0, 1)], vec![("park".to_owned(), String::new())]));
    hong.reject_set(1, &[(0, 1)], "three").unwrap();
    assert!(kim.look().unwrap().rejected.is_empty(), "said once: it was rejected already");
    assert_eq!(
        who(&kim.rejected_by(1, &[(0, 1)]).unwrap()),
        [("hong".to_owned(), "three".to_owned()), ("park".to_owned(), String::new())]
    );
    assert!(hong.reject_drop(1, &[(0, 1)]).unwrap());
    assert_eq!(kim.set_status(1, &[0], w).unwrap().rejected, [0], "park's stands");
    assert!(park.reject_drop(1, &[(0, 1)]).unwrap());
    assert!(!park.reject_drop(1, &[(0, 1)]).unwrap(), "nothing of theirs is left to take back");
    assert!(kim.set_status(1, &[0], w).unwrap().is_empty());
    assert_eq!((kim.states(1, 0, 1).unwrap(), why(&kim, 0)), (vec![1], Some("third".into())));
    assert_eq!(past(&kim, 0).len(), 4, "nothing was answered: nothing more is kept");
    // a confirmer who confirms an error takes their own rejection of it back
    hong.reject_set(1, &[(2, 3)], "hm").unwrap();
    assert_eq!(kim.look().unwrap().rejected[0].runs, [(2, 3)]);
    assert_eq!(kim.resubmit(1, &[(2, 3)]).unwrap(), 1);
    kim.reason_set(Kind::Waive, 1, &[(2, 3)], "hm indeed").unwrap();
    assert_eq!(hong.confirm_set(1, &[2], true).unwrap(), 1);
    assert!(hong.rejects.held(Kind::Reject, 1, &[(2, 3)]).is_empty());
    let last = lines(&log).last().unwrap().to_owned();
    assert!(last.starts_with("R 1 ") && last.ends_with(" 2,1"), "{last}");
    let looked = kim.look().unwrap();
    assert!(looked.changed && looked.rejected.is_empty());
    assert_eq!((kim.states(1, 0, 8).unwrap(), kim.state_counts(1).unwrap()), (vec![1, 1, 3, 3, 1, 1, 1, 1], [0, 6, 0, 2, 0, 0]));
    assert_eq!(page(&mut kim, Lens::Confirmed, true), (vec![2, 3], 2));
    assert_eq!(page(&mut kim, Lens::Waived, true), (vec![0, 1, 4, 5, 6, 7], 6));
    assert_eq!(page(&mut kim, Lens::Live, true).1, 8);
    // the confirmer's log says what stands, and their next session reads it;
    // a confirmer's own waive of an error they rejected is under their
    // rejection like anybody's
    drop(hong);
    let mut hong = open("hong", true);
    let held: Vec<(u64, u64, String)> = hong
        .rejects
        .runs(Kind::Reject, 1)
        .into_iter()
        .map(|(a, b, entry)| (a, b, hong.rejects.entries()[entry].text.clone()))
        .collect();
    assert_eq!(held, [(1, 2, "not a dummy".to_owned()), (3, 4, "still no".into()), (5, 6, String::new())]);
    assert_eq!(
        hong.rejects.texts(Kind::Reject, 9),
        ["hm", "three", "still no", "still not", "not a dummy"],
        "the reasons given so far - what the dialog offers again -, the empty one left out"
    );
    assert_eq!(hong.set_status(1, &[5, 6], w).unwrap().rejected, [5]);
    assert_eq!((hong.status(1, 5).unwrap(), hong.status(1, 6).unwrap()), (o, w));
    assert_eq!(hong.resubmit(1, &[(5, 6)]).unwrap(), 1);
    assert_eq!(hong.status(1, 5).unwrap(), w);
    // the reviewer's next session: its states are the files' from the open,
    // and its first look says the waives that are rejected
    hong.reject_set(1, &[(7, 8)], "last").unwrap();
    drop(kim);
    let mut kim = open("kim", false);
    assert_eq!(kim.states(1, 0, 8).unwrap(), [1, 1, 3, 3, 1, 1, 1, 2]);
    assert_eq!(past(&kim, 0).len(), 4);
    let looked = kim.look().unwrap();
    assert_eq!((looked.rejected.len(), looked.rejected[0].runs.clone(), who(&looked.rejected[0].by)), (1, vec![(7, 8)], vec![("hong".to_owned(), "last".to_owned())]));
    for r in [&kim, &hong, &park] {
        assert!(r.notices.is_empty(), "{:?}", r.notices);
    }
    drop((kim, hong, park));
    fs::remove_dir_all(&dir).unwrap();
}

/// The reviewers' kept counts are read for an answer at once (0.12.464, user
/// 2026-11-13: "이 통계는 실시간 요청으로 들어올 거라서"): the pack is opened
/// for its directory alone - the rules, their errors, the identity - and the
/// block table, three quarters of a byte an error, is not read.
#[test]
fn a_pack_opened_for_its_directory_reads_no_block() {
    let f = Fixture::new(&bytes(200));
    let stop = AtomicUsize::new(0);
    let whole = Pack::open(&f.path, &stop).unwrap();
    let light = Pack::open_directory(&f.path, &stop).unwrap();
    // the same rules, errors and identity
    let rules = |p: &Pack| {
        p.checks
            .iter()
            .map(|c| (c.name.clone(), c.desc.clone(), c.start, c.count))
            .collect::<Vec<_>>()
    };
    assert_eq!(rules(&light), rules(&whole));
    assert_eq!(light.checks.iter().map(|c| c.count).collect::<Vec<_>>(), [0, 200, 0]);
    assert_eq!(
        (light.total, light.content_id, light.source_size, light.source_mtime, light.source_crc),
        (whole.total, whole.content_id, whole.source_size, whole.source_mtime, whole.source_crc)
    );
    assert_eq!(light.cell, whole.cell);
    assert_eq!(light.review_fingerprint().unwrap(), whole.review_fingerprint().unwrap());
    // a rule's box is the block table's to give
    assert!(whole.checks[1].bbox.is_some());
    assert!(light.checks.iter().all(|c| c.bbox.is_none()));
    drop((whole, light));
    // the block table damaged: the pack does not open, its directory does
    let mut raw = bytes(200);
    let foot = raw.len() - 168;
    let blocks = u64::from_le_bytes(raw[foot + 48..foot + 56].try_into().unwrap()) as usize;
    let count = u64::from_le_bytes(raw[foot + 56..foot + 64].try_into().unwrap());
    assert_eq!(count, 4, "200 errors, 64 a block");
    raw[blocks + 8] ^= 1; // the first block's error count
    fs::write(&f.path, &raw).unwrap();
    let e = Pack::open(&f.path, &stop).err().expect("a damaged block table");
    assert!(e.message.contains("block metadata"), "{}", e.message);
    let light = Pack::open_directory(&f.path, &stop).unwrap();
    assert_eq!((light.total, light.checks[1].count), (200, 200));
    drop(light);
    // what the directory itself says is held to as ever
    fs::write(&f.path, &raw[..raw.len() - 1]).unwrap();
    assert!(Pack::open_directory(&f.path, &stop).is_err());
    let mut raw = bytes(200);
    let foot = raw.len() - 168;
    raw[foot + 72] ^= 1; // the footer's rule count
    fs::write(&f.path, &raw).unwrap();
    assert!(Pack::open_directory(&f.path, &stop).is_err());
}

/// The roles' workflow (0.12.472; user 2026-11-14: "error -> waive request
/// -> waive -> confirm 의 업무 흐름이며 role은 requester, waiver, confirmer로
/// 나누어야 함"): each session does its role's step, in its own file, and
/// every role's session reads the error's state from everybody's.
#[test]
fn the_roles_take_an_error_from_a_request_to_a_confirmation() {
    use super::analysis::scope::Selection;
    use super::desktop::{
        Kept, Left, Lens, Review, STATE_CONFIRMED, STATE_NONE, STATE_REJECTED, STATE_REQUESTED, STATE_WAIVED,
        STATUS_NONE, STATUS_WAIVED,
    };
    use super::local::Roles;
    use super::reasons::Kind;
    let dir = std::env::temp_dir().join(format!(
        "floe-waive-roles-{}-{}",
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&dir).unwrap();
    let pack = dir.join(".results.db.tray");
    fs::write(&pack, bytes(8)).unwrap();
    let stop = AtomicUsize::new(0);
    let open = |who: &str, role: Roles| {
        let mut r = Review::open_as(&pack, None, Some(who), role, &stop).unwrap();
        assert!(!r.read_only(), "{:?}", r.notices);
        r.set_lock_refresh(std::time::Duration::ZERO);
        r
    };
    let lines = |path: &std::path::Path| -> Vec<String> { fs::read_to_string(path).unwrap().lines().map(str::to_owned).collect() };
    let (w, o) = (STATUS_WAIVED, STATUS_NONE);
    let (none, asked, waived, rejected, confirmed) =
        (STATE_NONE, STATE_REQUESTED, STATE_WAIVED, STATE_REJECTED, STATE_CONFIRMED);
    let nothing = || Left::<u64> { locked: vec![], rejected: vec![], unrequested: vec![] };
    let mut park = open("park", Roles::REQUESTER);
    let mut kim = open("kim", Roles::WAIVER);
    let mut hong = open("hong", Roles::CONFIRMER);
    // a requester's marks are a file of their own, made with nothing asked
    // for; their session makes no waive sidecar, a waiver's no request file
    let at = |name: &str| dir.join(format!(".results.db.{name}"));
    assert!(at("request.park").exists() && !at("waive.park").exists());
    assert!(at("waive.kim").exists() && !at("request.kim").exists() && !at("request.hong").exists());
    assert!(!at("waive.hong").exists(), "a confirmer who is no waiver has no waive sidecar");
    assert_eq!(fs::read(at("request.park")).unwrap().len(), fs::read(at("waive.kim")).unwrap().len());
    assert_eq!(&fs::read(at("request.park")).unwrap()[..8], b"FLOEREQS");
    assert_eq!((park.roles(), kim.roles(), hong.roles()), (Roles::REQUESTER, Roles::WAIVER, Roles::CONFIRMER));
    assert_eq!((park.mark(), kim.mark(), hong.mark()), (Kind::Request, Kind::Waive, Kind::Waive));
    assert_eq!((park.confirmer(), kim.confirmer(), hong.confirmer()), (false, false, true));

    // 1. nothing is asked for: a waiver waives nothing, and says which; a
    // confirmer writes no status; nobody but a confirmer confirms
    let left = kim.set_status(1, &[0, 1], w).unwrap();
    assert_eq!(left, Left { unrequested: vec![0, 1], ..nothing() });
    assert_eq!(kim.states(1, 0, 8).unwrap(), [none; 8]);
    assert_eq!(hong.set_status(1, &[0], w).unwrap_err().kind, crate::ErrorKind::Unsupported);
    assert_eq!(park.confirm_set(1, &[0], true).unwrap_err().kind, crate::ErrorKind::Unsupported);
    assert_eq!(kim.confirm_set(1, &[0], true).unwrap_err().kind, crate::ErrorKind::Unsupported);
    assert_eq!(park.reject_set(1, &[(0, 1)], "no").unwrap_err().kind, crate::ErrorKind::Unsupported);

    // 2. the requester asks for the waive of four errors, and says why: a
    // line `q` of their reasons file. Their own marks are the requests
    assert_eq!(park.set_status(1, &[0, 1, 2, 3], w).unwrap(), nothing());
    park.reason_set(park.mark(), 1, &[(0, 4)], "dummy fill").unwrap();
    assert_eq!(park.states(1, 0, 8).unwrap(), [asked, asked, asked, asked, none, none, none, none]);
    assert_eq!(park.statuses(1, 0, 8).unwrap(), [w, w, w, w, o, o, o, o]);
    assert_eq!(park.state_counts(1).unwrap(), [4, 0, 0, 0, 4, 0]);
    assert_eq!(park.status_counts(1).unwrap(), (4, 8));
    assert_eq!(park.waive_reason(1, 2).unwrap().map(|e| e.text.clone()), Some("dummy fill".into()));
    let said = lines(&at("reasons.park"));
    assert!(said.iter().any(|line| line.starts_with("q 1 ") && line.ends_with(" 0,4 dummy fill")), "{said:?}");
    assert!(!said.iter().any(|line| line.starts_with("w ")), "{said:?}");
    // the others read it from their next look on
    assert_eq!(kim.states(1, 0, 8).unwrap(), [none; 8]);
    let looked = kim.look().unwrap();
    assert!(looked.changed && looked.rules == [1] && looked.rejected.is_empty(), "{looked:?}");
    assert_eq!(kim.states(1, 0, 8).unwrap(), [asked, asked, asked, asked, none, none, none, none]);
    assert_eq!(kim.statuses(1, 0, 8).unwrap(), [o; 8], "the waiver's own marks are waives");
    assert_eq!(kim.requesters_of(1, &[(0, 8)]).unwrap(), ["park"]);
    assert_eq!(kim.requested(1, 2, 4).unwrap(), Some(vec![true, true, false, false]));
    let looked = kim.look().unwrap();
    assert!(!looked.changed && looked.rules.is_empty());
    // the class a filter shows: the requested errors are one
    let page = |r: &mut Review, lens: Lens, waived: bool| {
        r.set_lens(lens);
        (r.status_page(1, waived, 0, 10).unwrap(), r.lens_counts(1).unwrap().0)
    };
    assert_eq!(page(&mut kim, Lens::Requested, true), (vec![0, 1, 2, 3], 4));
    assert_eq!(kim.status_rank(1, true, 2).unwrap(), Some(2));
    assert_eq!(kim.lensed(1, 0, 8).unwrap(), [1, 1, 1, 1, 0, 0, 0, 0]);
    assert_eq!(page(&mut kim, Lens::Held, false), (vec![4, 5, 6, 7], 4), "not waived: nobody asked, nobody waived");
    assert_eq!(page(&mut kim, Lens::Live, true), (vec![], 0));
    assert!(!kim.steps(1));

    // 3. the waiver waives what was asked for, and no more
    let left = kim.set_status(1, &[0, 1, 5], w).unwrap();
    assert_eq!(left, Left { unrequested: vec![5], ..nothing() });
    kim.reason_set(kim.mark(), 1, &[(0, 2)], "agreed").unwrap();
    assert_eq!(kim.states(1, 0, 8).unwrap(), [waived, waived, asked, asked, none, none, none, none]);
    assert_eq!(kim.statuses(1, 0, 8).unwrap(), [w, w, o, o, o, o, o, o]);
    assert_eq!(kim.state_counts(1).unwrap(), [4, 2, 0, 0, 2, 0]);
    assert_eq!(page(&mut kim, Lens::Live, true), (vec![0, 1], 2));
    assert_eq!(page(&mut kim, Lens::Requested, true), (vec![2, 3], 2));
    assert!(park.look().unwrap().changed);
    assert_eq!(park.states(1, 0, 8).unwrap(), [waived, waived, asked, asked, none, none, none, none]);
    assert_eq!(park.statuses(1, 0, 8).unwrap(), [w, w, w, w, o, o, o, o], "the requests stand under the waives");

    // 4. a request is locked while somebody has its error waived
    let left = park.set_status(1, &[0, 2], o).unwrap();
    assert_eq!(left, Left { locked: vec![0], ..nothing() });
    assert_eq!(park.confirmers_of(1, &[(0, 1)]).unwrap(), ["kim"]);
    assert_eq!(park.states(1, 0, 4).unwrap(), [waived, waived, none, asked]);
    assert_eq!(park.waive_reason(1, 2).unwrap().map(|e| e.text.clone()), None, "a request taken back has no reason");
    assert_eq!(park.waive_reason(1, 0).unwrap().map(|e| e.text.clone()), Some("dummy fill".into()));

    // 5. a job of the waiver's over the rule: the asked ones, the rest said
    kim.look().unwrap();
    let all = Selection::from_indices(8, 0..8);
    let mut left: Left<(u64, u64)> = Left::default();
    assert_eq!(kim.set_status_range(1, &all, w, 0, 8, &mut left).unwrap(), 1);
    assert_eq!(left, Left { locked: vec![], rejected: vec![], unrequested: vec![(2, 3), (4, 8)] });
    assert_eq!(kim.states(1, 0, 8).unwrap(), [waived, waived, none, waived, none, none, none, none]);
    // ... and an applied decision's span
    let before = kim.statuses(1, 0, 8).unwrap();
    assert_eq!(
        kim.write_status_span(1, 0, &before, &[w; 8]).unwrap(),
        Kept { locked: 0, rejected: 0, unrequested: 5 }
    );
    assert_eq!(kim.statuses(1, 0, 8).unwrap(), before);

    // 6. the confirmer confirms: the waive is locked, and everybody reads
    // the error confirmed
    hong.look().unwrap();
    assert_eq!(hong.states(1, 0, 4).unwrap(), [waived, waived, none, waived]);
    assert_eq!(hong.confirm_set(1, &[0], true).unwrap(), 1);
    assert_eq!(hong.states(1, 0, 4).unwrap(), [confirmed, waived, none, waived], "their own verdict: read at once");
    assert!(kim.look().unwrap().changed && park.look().unwrap().changed);
    assert_eq!(kim.states(1, 0, 4).unwrap(), [confirmed, waived, none, waived]);
    assert_eq!(park.states(1, 0, 4).unwrap(), [confirmed, waived, none, waived]);
    assert_eq!(kim.set_status(1, &[0], o).unwrap(), Left { locked: vec![0], ..nothing() });
    assert_eq!(kim.state_counts(1).unwrap(), [5, 2, 0, 1, 0, 0]);

    // 7. the confirmer rejects a waive: rejected for everybody, the request
    // under it stays locked, and the waiver answers by waiving again with
    // a reason
    hong.reject_set(1, &[(1, 2)], "not a dummy").unwrap();
    assert_eq!(hong.states(1, 0, 4).unwrap(), [confirmed, rejected, none, waived]);
    kim.look().unwrap();
    park.look().unwrap();
    for r in [&park, &kim, &hong] {
        assert_eq!(r.states(1, 0, 4).unwrap(), [confirmed, rejected, none, waived], "{}", r.reviewer);
    }
    assert_eq!(park.set_status(1, &[1], o).unwrap(), Left { locked: vec![1], ..nothing() });
    assert_eq!(kim.set_status(1, &[1], w).unwrap(), Left { rejected: vec![1], ..nothing() });
    assert_eq!(kim.resubmit(1, &[(1, 2)]).unwrap(), 1);
    kim.reason_set(kim.mark(), 1, &[(1, 2)], "a dummy, sheet 2").unwrap();
    assert_eq!(kim.states(1, 0, 4).unwrap(), [confirmed, waived, none, waived]);
    hong.look().unwrap();
    park.look().unwrap();
    assert_eq!(hong.states(1, 0, 4).unwrap(), [confirmed, waived, none, waived]);
    assert_eq!(park.states(1, 0, 4).unwrap(), [confirmed, waived, none, waived]);

    // 8. the waiver turns a waive back: asked for again - and the request
    // is its requester's to take back then
    assert_eq!(kim.set_status(1, &[3], o).unwrap(), nothing());
    assert_eq!(kim.states(1, 3, 1).unwrap(), [asked]);
    park.look().unwrap();
    assert_eq!(park.states(1, 3, 1).unwrap(), [asked]);
    assert_eq!(park.set_status(1, &[3], o).unwrap(), nothing());
    assert_eq!(park.states(1, 3, 1).unwrap(), [none]);
    kim.look().unwrap();
    assert_eq!(kim.states(1, 0, 8).unwrap(), [confirmed, waived, none, none, none, none, none, none]);
    assert_eq!(kim.set_status(1, &[3], w).unwrap(), Left { unrequested: vec![3], ..nothing() });

    // 9. a saved waive file: a waiver's session takes its waives of what
    // was asked for; a requester's and a confirmer's load none
    let saved = dir.join("all.waive");
    let mut data = kim.waive_header().to_vec();
    data.extend(std::iter::repeat(w).take(pack_total(&kim) as usize));
    for check in 0..kim.pack.checks.len() {
        data.extend((kim.pack.checks[check].count as u32).to_le_bytes());
    }
    fs::write(&saved, &data).unwrap();
    park.set_status(1, &[6], w).unwrap();
    kim.look().unwrap();
    let (count, kept) = kim.waive_import(&saved).unwrap();
    assert_eq!(kim.statuses(1, 0, 8).unwrap(), [w, w, o, o, o, o, w, o]);
    assert_eq!((count, kept.unrequested), (3, pack_total(&kim) - 3));
    assert_eq!(park.waive_import(&saved).unwrap_err().kind, crate::ErrorKind::Unsupported);
    assert_eq!(hong.waive_import(&saved).unwrap_err().kind, crate::ErrorKind::Unsupported);

    // 10. a session without a role is a reviewer as before: it waives at
    // once, reads its own waives - and its waive is one for the roles
    let mut lee = open("lee", Roles::NONE);
    assert_eq!(lee.set_status(1, &[7], w).unwrap(), nothing());
    assert_eq!(lee.states(1, 0, 8).unwrap(), [none, none, none, none, none, none, none, waived]);
    assert_eq!(lee.state_counts(1).unwrap(), [7, 1, 0, 0, 0, 0]);
    assert_eq!(lee.roles(), Roles::NONE);
    kim.look().unwrap();
    assert_eq!(kim.states(1, 6, 2).unwrap(), [waived, waived]);
    assert_eq!(kim.state_counts(1).unwrap(), [4, 3, 0, 1, 0, 0]);

    // 11. the next sessions read the same, from the files
    drop((park, kim, hong, lee));
    let park = open("park", Roles::REQUESTER);
    let hong = open("hong", Roles::CONFIRMER);
    let states = [confirmed, waived, none, none, none, none, waived, waived];
    assert_eq!(park.states(1, 0, 8).unwrap(), states);
    assert_eq!(hong.states(1, 0, 8).unwrap(), states);
    assert_eq!(park.statuses(1, 0, 8).unwrap(), [w, w, o, o, o, o, w, o]);
    assert_eq!(park.waive_reason(1, 1).unwrap().map(|e| e.text.clone()), Some("dummy fill".into()));
    // ... and the same reviewer as a waiver has their requests among them
    drop(park);
    let mut park = open("park", Roles::WAIVER);
    assert_eq!(park.states(1, 0, 8).unwrap(), states);
    assert_eq!(park.statuses(1, 0, 8).unwrap(), [o; 8]);
    assert_eq!(park.requesters_of(1, &[(0, 8)]).unwrap(), ["park"]);
    assert!(at("waive.park").exists());
    fs::remove_dir_all(&dir).unwrap();
}

/// A waiver's verdict on a request (0.12.473; user 2026-11-14: "요청 거절
/// 상태도 만들어줘"), as a confirmer's rejection is one on a waive: a
/// declined request stays with its requester, declined, until they ask again
/// with a reason - what was is kept. And the sessions of several roles
/// ("--role waiver,confirmer나 --role requester,waiver,confirmer가 가능하게"),
/// and the one that makes no mark and no file for one ("confirmer는 waive
/// 파일을 만들지 않게").
#[test]
fn a_declined_request_is_a_state_until_its_requester_asks_again() {
    use super::desktop::{
        Left, Lens, Review, STATE_CONFIRMED, STATE_DECLINED, STATE_NONE, STATE_REQUESTED, STATE_WAIVED, STATUS_NONE,
        STATUS_WAIVED,
    };
    use super::local::Roles;
    let dir = std::env::temp_dir().join(format!(
        "floe-waive-decline-{}-{}",
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&dir).unwrap();
    let pack = dir.join(".results.db.tray");
    fs::write(&pack, bytes(12)).unwrap();
    let stop = AtomicUsize::new(0);
    let open = |who: &str, roles: Roles| {
        let mut r = Review::open_as(&pack, None, Some(who), roles, &stop).unwrap();
        assert!(!r.read_only(), "{:?}", r.notices);
        r.set_lock_refresh(std::time::Duration::ZERO);
        r
    };
    let lines = |path: &std::path::Path| -> Vec<String> { fs::read_to_string(path).unwrap().lines().map(str::to_owned).collect() };
    let at = |name: &str| dir.join(format!(".results.db.{name}"));
    fn past(r: &Review, error: u64) -> Vec<(&'static str, String, String)> {
        r.history(1, error).unwrap().into_iter().map(|e| (e.what, e.by, e.text)).collect()
    }
    let was = |what: &'static str, by: &str, text: &str| (what, by.to_owned(), text.to_owned());
    let who = |by: &[super::confirm::Rejected]| by.iter().map(|b| (b.reviewer.clone(), b.text.clone())).collect::<Vec<_>>();
    let (w, o) = (STATUS_WAIVED, STATUS_NONE);
    let (none, asked, declined, waived, confirmed) =
        (STATE_NONE, STATE_REQUESTED, STATE_DECLINED, STATE_WAIVED, STATE_CONFIRMED);
    let nothing = || Left::<u64> { locked: vec![], rejected: vec![], unrequested: vec![] };
    let mut park = open("park", Roles::REQUESTER);
    let mut choi = open("choi", Roles::REQUESTER);
    let mut kim = open("kim", Roles::WAIVER);
    let mut hong = open("hong", Roles::CONFIRMER);
    // a confirmer who is neither waiver nor requester makes no mark, and
    // has no file of marks
    assert!(!at("waive.hong").exists() && !at("request.hong").exists());
    assert_eq!((hong.statuses(1, 0, 12).unwrap(), hong.status_counts(1).unwrap()), (vec![o; 12], (0, 12)));
    assert_eq!(hong.set_status(1, &[0], w).unwrap_err().kind, crate::ErrorKind::Unsupported);
    assert_eq!(hong.waive_export(&dir.join("x.waive")).unwrap_err().kind, crate::ErrorKind::Unsupported);
    park.set_status(1, &[0, 1, 2, 3, 4, 5], w).unwrap();
    park.reason_set(park.mark(), 1, &[(0, 6)], "dummy fill").unwrap();
    choi.set_status(1, &[4, 5, 6, 7], w).unwrap();
    kim.look().unwrap();
    assert_eq!(kim.states(1, 0, 12).unwrap()[..9], [asked, asked, asked, asked, asked, asked, asked, asked, none]);
    // only a waiver declines, and nothing is made by a refusal
    assert_eq!(park.decline_set(1, &[(0, 2)], "no").unwrap_err().kind, crate::ErrorKind::Unsupported);
    assert_eq!(hong.decline_set(1, &[(0, 2)], "no").unwrap_err().kind, crate::ErrorKind::Unsupported);
    assert_eq!(hong.decline_drop(1, &[(0, 2)]).unwrap_err().kind, crate::ErrorKind::Unsupported);
    assert!(!at("rejects.park").exists() && !at("rejects.hong").exists() && !at("rejects.kim").exists());
    // a decline is one line of the waiver's log, with why or without, each
    // with a time of its own
    kim.decline_set(1, &[(0, 2)], " not a dummy\n").unwrap();
    kim.decline_set(1, &[(4, 5)], "").unwrap();
    kim.decline_set(1, &[], "nothing").unwrap();
    assert!(kim.decline_set(1, &[(11, 13)], "past the rule").is_err());
    let said = lines(&at("rejects.kim"));
    assert_eq!(said.len(), 5, "{said:?}");
    assert!(said[0] == "# floe drc waive rejections" && said[1] == "floe_reasons=2" && said[2].starts_with("floe_pack="));
    let parts = |line: &str| line.split(' ').map(str::to_owned).collect::<Vec<_>>();
    assert_eq!((&parts(&said[3])[..2], &parts(&said[3])[3..]), (&["d".to_owned(), "1".into()][..], &["0,2".to_owned(), "not".into(), "a".into(), "dummy".into()][..]));
    assert_eq!((parts(&said[4]).len(), parts(&said[4])[3].as_str()), (4, "4,1"));
    let (first, second): (u64, u64) = (parts(&said[3])[2].parse().unwrap(), parts(&said[4])[2].parse().unwrap());
    assert!(first > 1_600_000_000 && second > first, "{first} {second}");
    // the waiver reads it at once: a request under a decline is declined -
    // every requester's of that error, until each answers
    let now = [declined, declined, asked, asked, declined, asked, asked, asked];
    assert_eq!(kim.states(1, 0, 8).unwrap(), now);
    assert_eq!(kim.state_counts(1).unwrap(), [4, 0, 0, 0, 5, 3]);
    kim.set_lens(Lens::Declined);
    assert_eq!((kim.status_page(1, true, 0, 10).unwrap(), kim.lens_counts(1).unwrap().0), (vec![0, 1, 4], 3));
    kim.set_lens(Lens::Requested);
    assert_eq!((kim.status_page(1, true, 0, 10).unwrap(), kim.lens_counts(1).unwrap().0), (vec![2, 3, 5, 6, 7], 5));
    kim.set_lens(Lens::Live);
    // no waiver waives an error for a request that is declined
    let mut lee = open("lee", Roles::WAIVER);
    assert_eq!(lee.set_status(1, &[0, 2], w).unwrap(), Left { unrequested: vec![0], ..nothing() });
    assert_eq!(lee.set_status(1, &[2], o).unwrap(), nothing());
    drop(lee);
    // the requesters' looks say what was declined, who and why, once -
    // nothing of theirs is written: the request stays, with its reason
    let before = fs::read(at("request.park")).unwrap();
    let looked = park.look().unwrap();
    assert_eq!(looked.rejected.len(), 1, "{looked:?}");
    assert_eq!((looked.rejected[0].check, &looked.rejected[0].runs[..], looked.rejected[0].count), (1, &[(0, 2), (4, 5)][..], 3));
    assert_eq!(who(&looked.rejected[0].by), [("kim".to_owned(), "not a dummy".to_owned()), ("kim".to_owned(), String::new())]);
    assert!(looked.rules == [1] && park.look().unwrap().rejected.is_empty());
    assert_eq!(fs::read(at("request.park")).unwrap(), before);
    assert_eq!(park.states(1, 0, 8).unwrap(), now);
    assert_eq!(park.statuses(1, 0, 8).unwrap(), [o, o, w, w, o, w, o, o], "a declined request reads as none of theirs");
    assert_eq!(park.status_counts(1).unwrap(), (3, 12));
    assert_eq!(park.waive_reason(1, 0).unwrap().map(|e| e.text.clone()), Some("dummy fill".into()));
    let looked = choi.look().unwrap();
    assert_eq!((&looked.rejected[0].runs[..], looked.rejected[0].count), (&[(4, 5)][..], 1));
    // a declined request is asked again with a reason, or not at all
    assert_eq!(park.set_status(1, &[0, 4], w).unwrap(), Left { rejected: vec![0, 4], ..nothing() });
    assert_eq!(who(&park.rejected_by(1, &[(0, 1)]).unwrap()), [("kim".to_owned(), "not a dummy".to_owned())]);
    assert_eq!(park.resubmit(1, &[(0, 1)]).unwrap(), 1);
    park.reason_set(park.mark(), 1, &[(0, 1)], "a dummy, sheet 2").unwrap();
    assert_eq!(park.states(1, 0, 2).unwrap(), [asked, declined]);
    assert_eq!(past(&park, 0), [was("requested", "", "dummy fill"), was("declined", "kim", "not a dummy")]);
    let own = lines(&at("rejects.park"));
    assert!(own.iter().any(|l| l.starts_with("o 1 ") && l.ends_with(" 0,1 dummy fill")), "{own:?}");
    assert!(own.iter().any(|l| l.starts_with("k 1 ") && l.ends_with(" 0,1 kim not a dummy")), "{own:?}");
    kim.look().unwrap();
    assert_eq!(kim.states(1, 0, 2).unwrap(), [asked, declined]);
    // each requester answers for their own request: one of two who asked
    // for an error asks again, and the error is asked for
    assert_eq!(choi.resubmit(1, &[(4, 5)]).unwrap(), 1);
    choi.reason_set(choi.mark(), 1, &[(4, 5)], "seal ring").unwrap();
    kim.look().unwrap();
    park.look().unwrap();
    assert_eq!((kim.states(1, 4, 1).unwrap(), park.states(1, 4, 1).unwrap()), (vec![asked], vec![asked]));
    assert_eq!(park.statuses(1, 4, 1).unwrap(), [o], "... the other's request of it is declined still");
    // declined again: a new decline, to be answered anew
    kim.decline_set(1, &[(0, 1)], "still not").unwrap();
    let looked = park.look().unwrap();
    assert_eq!((&looked.rejected[0].runs[..], who(&looked.rejected[0].by)), (&[(0, 1)][..], vec![("kim".to_owned(), "still not".to_owned())]));
    assert_eq!(park.states(1, 0, 1).unwrap(), [declined]);
    // its requester takes a declined request back: the reason it had is kept
    assert_eq!(park.set_status(1, &[0], o).unwrap(), nothing());
    assert_eq!(park.states(1, 0, 1).unwrap(), [none]);
    assert_eq!(
        past(&park, 0),
        [was("requested", "", "dummy fill"), was("declined", "kim", "not a dummy"), was("requested", "", "a dummy, sheet 2")]
    );
    // the waiver takes a decline back: the request is one again, at once
    assert!(kim.decline_drop(1, &[(1, 2)]).unwrap() && !kim.decline_drop(1, &[(1, 2)]).unwrap());
    assert_eq!(kim.states(1, 1, 1).unwrap(), [asked]);
    park.look().unwrap();
    assert_eq!((park.states(1, 1, 1).unwrap(), park.statuses(1, 1, 1).unwrap()), (vec![asked], vec![w]));
    // a waiver who declined waives the error after all: the decline goes
    // with the waive
    kim.decline_set(1, &[(5, 6)], "no").unwrap();
    assert_eq!(kim.states(1, 5, 1).unwrap(), [declined]);
    assert_eq!(kim.set_status(1, &[5], w).unwrap(), nothing());
    assert_eq!(kim.states(1, 5, 1).unwrap(), [waived]);
    assert!(lines(&at("rejects.kim")).last().unwrap().starts_with("D 1 "));
    // a session of two roles. A requester who is a waiver too is its own
    // requester: it waives an error nobody asked for, in its waive sidecar,
    // and makes no request file
    let mut yoon = open("yoon", Roles::REQUESTER.with(Roles::WAIVER));
    assert!(at("waive.yoon").exists() && !at("request.yoon").exists());
    assert_eq!(yoon.set_status(1, &[10], w).unwrap(), nothing());
    assert_eq!(yoon.states(1, 10, 1).unwrap(), [waived]);
    // ... a waiver who is a confirmer too waives what was asked for and
    // confirms; no lock holds its own confirmed waive, and its
    // confirmation goes with the waive it turns back
    let mut jung = open("jung", Roles::WAIVER.with(Roles::CONFIRMER));
    assert_eq!(jung.roles().name(), "waiver,confirmer");
    assert_eq!(jung.set_status(1, &[11, 6], w).unwrap(), Left { unrequested: vec![11], ..nothing() });
    assert_eq!(jung.confirm_set(1, &[6, 10], true).unwrap(), 2);
    assert_eq!(jung.states(1, 6, 1).unwrap(), [confirmed]);
    yoon.look().unwrap();
    assert_eq!(yoon.states(1, 10, 1).unwrap(), [confirmed]);
    assert_eq!(yoon.set_status(1, &[10], o).unwrap(), Left { locked: vec![10], ..nothing() });
    assert_eq!(jung.set_status(1, &[6], o).unwrap(), nothing());
    assert!(!jung.confirms.get(1, 6).unwrap() && jung.confirms.get(1, 10).unwrap());
    assert_eq!(jung.states(1, 6, 1).unwrap(), [asked]);
    // the roles' names, as a launcher says them
    assert_eq!(Roles::parse(" Confirmer , waiver ").unwrap(), Roles::WAIVER.with(Roles::CONFIRMER));
    assert_eq!(Roles::parse("requester,waiver,confirmer").unwrap(), Roles::ALL);
    assert_eq!((Roles::ALL.name(), Roles::NONE.name()), ("requester,waiver,confirmer".to_owned(), String::new()));
    assert!(Roles::parse("waiver,boss").is_none() && Roles::parse("").unwrap() == Roles::NONE);
    // the next sessions read the same, from the files
    drop((park, choi, kim, hong, yoon, jung));
    let hong = open("hong", Roles::CONFIRMER);
    assert_eq!(
        hong.states(1, 0, 12).unwrap(),
        [none, asked, asked, asked, asked, waived, asked, asked, none, none, confirmed, none]
    );
    assert!(!at("waive.hong").exists());
    fs::remove_dir_all(&dir).unwrap();
}

/// The errors of a review's pack.
#[cfg(test)]
fn pack_total(r: &super::desktop::Review) -> u64 {
    r.pack.total
}
