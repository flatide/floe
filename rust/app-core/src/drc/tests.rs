use super::*;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
};
static SERIAL: AtomicU64 = AtomicU64::new(0);
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
fn bytes(n: u64) -> Vec<u8> {
    geometry_bytes(n, 2)
}
fn geometry_bytes(n: u64, vertices: usize) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend(MAGIC);
    push32(&mut out, 4);
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
    push32(&mut out, 0);
    out.extend(MAGIC);
    out
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
fn circular_step_matches_filtered_file_order_across_blocks_and_resumes() {
    let mut b = bytes(130);
    let offset = b.len() - 136 + 4 * 8;
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
    push32(&mut side, 1);
    for v in [1234, 5678, 130] {
        push64(&mut side, v);
    }
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
        let off = b.len() - 136 + field * 8;
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
    assert!(waive_paths(&p, "../other").is_err());
    assert!(waive_paths(&p, &"x".repeat(201)).is_err());
    assert_eq!(reviewer_tag(Some("  Kim / 김  ")), "Kim___김");
}

fn replace_one_blob(blob: &[u8]) -> Vec<u8> {
    let mut b = bytes(1);
    let foot = b.len() - 136;
    let old = u64::from_le_bytes(b[foot + 16..foot + 24].try_into().unwrap()) as usize;
    let shift = blob.len() as i64 - (old - 40) as i64;
    b.splice(40..old, blob.iter().copied());
    let foot = b.len() - 136;
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
