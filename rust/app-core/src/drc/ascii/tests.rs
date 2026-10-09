use super::*;
use std::{
    fs,
    sync::atomic::{AtomicU64, Ordering},
};
static SERIAL: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    dir: PathBuf,
    path: PathBuf,
}
impl Fixture {
    fn new(bytes: &[u8]) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "floe-ascii-reader-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("한 글.db");
        fs::write(&path, bytes).unwrap();
        Self { dir, path }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.dir).unwrap();
    }
}
fn flag() -> AtomicUsize {
    AtomicUsize::new(0)
}
#[test]
fn metadata_queries_filters_and_steps_preserve_fractional_boundaries() {
    use crate::drc::{filters::Filtering, Cursor, ListRequest, StepRequest};
    use std::collections::BTreeSet;
    let f = Fixture::new(b"TOP 1\nR\np 1 1\n.125 -.5\np 2 1\n.25 .5\np 3 1\n.375 -.5\n");
    let mut p = Ascii::open(&f.path, &flag()).unwrap();
    let query = [0.125, -0.5, 0.375, -0.5];
    let first = p
        .query_info(query, None, None, Cursor::default(), 1, &flag())
        .unwrap();
    assert_eq!(first.hits[0].local, 0);
    let last = p
        .query_info(query, None, None, first.next.unwrap(), 1, &flag())
        .unwrap();
    assert_eq!(last.hits[0].local, 2);
    assert!(last.next.is_none());
    assert!(
        p.cached.is_none(),
        "metadata query decoded a coordinate record"
    );
    let ids = BTreeSet::from([1, 2]);
    let v = p
        .filtered_errors(
            ListRequest {
                check: 0,
                start: 0,
                waived: None,
                bbox_um: Some(query),
                selected: Some(&ids),
                limit: 1,
            },
            &flag(),
        )
        .unwrap();
    assert_eq!(v.hits[0].local, 2);
    assert!(v.next.is_none());
    let r = StepRequest {
        check: 0,
        bbox_um: Some([0.375, -0.5, 0.375, -0.5]),
        ..Default::default()
    };
    let a = p.step_limited(r, 1, &flag()).unwrap();
    assert!(a.hit.is_none());
    assert_eq!(a.next.unwrap().next, 1);
    let a = p
        .step_limited(
            StepRequest {
                cursor: a.next,
                ..r
            },
            1,
            &flag(),
        )
        .unwrap();
    assert!(a.hit.is_none());
    let a = p
        .step_limited(
            StepRequest {
                cursor: a.next,
                ..r
            },
            1,
            &flag(),
        )
        .unwrap();
    assert_eq!(a.hit.unwrap().local, 2);
    assert!(a.next.is_none());
    let a = p
        .filtered_step(
            StepRequest {
                after: Some(1),
                backwards: true,
                ..Default::default()
            },
            Some(&ids),
            &flag(),
        )
        .unwrap();
    assert_eq!(a.hit.unwrap().local, 2);
    assert!(p
        .selection_candidates(0, &[0, 0], Some(query), Some(true), &flag())
        .unwrap()
        .is_empty());
    assert!(p
        .query_info([f64::NAN; 4], None, None, Cursor::default(), 1, &flag())
        .is_err());
    assert_eq!(
        p.cached_error(0, 1, &flag()).unwrap().points_um,
        [[0.25, 0.5]]
    );
    assert!(p.cached.is_some());
    fs::write(&f.path, b"changed").unwrap();
    assert!(p.cached_error(0, 1, &flag()).is_err());
}
#[test]
fn empty_rule_walk_is_bounded_and_geometry_pages_have_explicit_units() {
    use crate::drc::{filters::Filtering, Cursor, Database, ReadPoints};
    let mut text = String::from("TOP 1\n");
    for i in 0..4100 {
        text.push_str(&format!("R{i}\n0 0 0\n"));
    }
    text.push_str("LAST\np 1 2\n.125 .25\n.375 .5\n");
    let f = Fixture::new(text.as_bytes());
    let mut p = Ascii::open(&f.path, &flag()).unwrap();
    let v = p
        .query_info([0., 0., 1., 1.], None, None, Cursor::default(), 1, &flag())
        .unwrap();
    assert!(v.hits.is_empty());
    assert_eq!(v.next.unwrap().check, 4096);
    let v = p
        .query_info([0., 0., 1., 1.], None, None, v.next.unwrap(), 1, &flag())
        .unwrap();
    assert_eq!(v.hits[0].check, 4100);
    let mut db = Database::open_explicit(&f.path, None, &flag()).unwrap();
    let v = db.error_points(4100, 0, 0, 1, &flag()).unwrap();
    assert_eq!(v.next, Some(1));
    assert!(matches!(v.points,ReadPoints::Um(p) if p==[[0.125,0.25]]));
    let v = db.error_points(4100, 0, 1, 1, &flag()).unwrap();
    assert!(v.next.is_none());
    assert!(matches!(v.points,ReadPoints::Um(p) if p==[[0.375,0.5]]));
    assert!(Database::open_explicit(&f.path, Some(&f.path), &flag()).is_err());
    assert!(db.error_points(4100, 0, 3, 1, &flag()).is_err());
}

#[test]
fn fractional_advisory_partial_unknown_internal_and_admin_sections() {
    let source = b"TOP 2\nRULE\n-001 9999999999999999999999999999999999 3 date\n\n desc \np 90 2\n1.25 -2.5 3e0 4.25 999\n5_000 0\nx 1 1\n100 200\np -8 9999999999999999999999999999999999\n1 2\nNEXT\ne 2 1\n0 0 .5 .25\n__RVE_ERROR_TAG2__\np 1 1\n9 9\nEMPTY_RDBS\n0 0 0\nREAL_RDBS\np 1 1\n6 8\n";
    let f = Fixture::new(source);
    let p = Ascii::open(&f.path, &flag()).unwrap();
    assert_eq!(p.cell, "TOP");
    assert_eq!(p.precision, 2.);
    assert_eq!(p.total, 4);
    assert_eq!(
        p.checks.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        ["RULE", "NEXT", "REAL_RDBS"]
    );
    assert_eq!(p.checks[0].declared, "-1");
    assert_eq!(p.checks[0].original, "9999999999999999999999999999999999");
    assert_eq!(p.checks[0].desc, "\ndesc");
    assert_eq!(p.checks[0].count, 2);
    assert_eq!(p.truncated_records, 1);
    assert_eq!(
        p.error(0, 0, &flag()).unwrap().points_um,
        [[0.625, -1.25], [1.5, 2.125], [2500., 0.]]
    );
    assert_eq!(p.error(1, 0, &flag()).unwrap().number, 3);
    assert_eq!(p.error(2, 0, &flag()).unwrap().bbox_um, [3., 4., 3., 4.]);
    assert!(p.error(4, 0, &flag()).is_err());
    assert!(p.error(0, 2, &flag()).is_err());
    assert_eq!(fs::read(&f.path).unwrap(), source);
    assert_eq!(fs::read_dir(&f.dir).unwrap().count(), 1);
}
#[test]
fn all_newlines_invalid_utf8_defaults_and_no_final_terminator() {
    for newline in ["\n", "\r\n", "\r"] {
        for precision in ["", " invalid", " 0", " -3", " +1_000"] {
            let bytes = format!("\n TOP{precision}\nR\n1 1 2\n\n\ne 5 1\n0 0 1.25 2.5")
                .replace('\n', newline)
                .into_bytes();
            let f = Fixture::new(&bytes);
            let p = Ascii::open(&f.path, &flag()).unwrap();
            assert_eq!(p.precision, 1000.);
            assert_eq!(p.checks[0].desc, "\n");
            assert_eq!(
                p.error(0, 0, &flag()).unwrap().points_um,
                [[0., 0.], [0.00125, 0.0025]]
            );
        }
    }
    let f = Fixture::new(b"TOP 1\nR\xff\np 1 1\n1 2\n");
    assert_eq!(
        Ascii::open(&f.path, &flag()).unwrap().checks[0].name,
        "R\u{fffd}"
    );
}
#[test]
fn reject_nonfinite_malformed_counts_and_limits_before_unbounded_allocation() {
    for source in [
        "",
        "\n \n",
        "TOP nan\n",
        "TOP inf\n",
        "TOP 1\nR\np --1 4\n",
        "TOP 1\nR\np 1 --4\n",
        "TOP 1\nR\np 1 1\nnan 3\n",
        "TOP 1e-308\nR\np 1 1\n1e308 3\n",
    ] {
        let f = Fixture::new(source.as_bytes());
        assert!(Ascii::open(&f.path, &flag()).is_err(), "{source}");
    }
    let f = Fixture::new(b"TOP 1\nR\np 1 99999999999999999999999999999999\n1 2\n");
    let p = Ascii::open(&f.path, &flag()).unwrap();
    assert_eq!(p.total, 1);
    assert_eq!(p.truncated_records, 1);
    assert_eq!(
        Ascii::open_limits(&f.path, &flag(), 8, 100, 10)
            .err()
            .unwrap()
            .kind,
        ErrorKind::Incomplete
    );
    assert_eq!(
        Ascii::open_limits(&f.path, &flag(), 10000, 5, 10)
            .err()
            .unwrap()
            .kind,
        ErrorKind::Incomplete
    );
    assert_eq!(
        Ascii::open_limits(&f.path, &flag(), 10000, 100, 0)
            .err()
            .unwrap()
            .kind,
        ErrorKind::Incomplete
    );
    assert_eq!(
        Ascii::open(&f.path, &AtomicUsize::new(1))
            .err()
            .unwrap()
            .kind,
        ErrorKind::Cancelled
    );
    assert_eq!(
        p.error(0, 0, &AtomicUsize::new(1)).err().unwrap().kind,
        ErrorKind::Cancelled
    );
}
#[test]
fn truncation_and_in_place_change_fail_and_replacement_keeps_open_revision() {
    let bytes = b"TOP 1\nR\np 1 1\n1 2\n";
    let f = Fixture::new(bytes);
    let p = Ascii::open(&f.path, &flag()).unwrap();
    fs::write(&f.path, b"TOP 1\n").unwrap();
    assert_eq!(p.error(0, 0, &flag()).err().unwrap().kind, ErrorKind::Cache);
    fs::write(&f.path, bytes).unwrap();
    let p = Ascii::open(&f.path, &flag()).unwrap();
    let old = f.dir.join("old.db");
    fs::rename(&f.path, &old).unwrap();
    fs::write(&f.path, b"NEW 2\n").unwrap();
    // rename changes ctime on common filesystems: either explicit revision
    // error or the pinned original, never coordinates from the replacement.
    match p.error(0, 0, &flag()) {
        Ok(v) => assert_eq!(v.points_um, [[1., 2.]]),
        Err(e) => assert_eq!(e.kind, ErrorKind::Cache),
    }
}
#[test]
fn geometry_memory_is_one_record_and_metadata_is_bounded_across_records() {
    let mut s = String::from("TOP 1000\nR\n");
    for _ in 0..3000 {
        s.push_str("p 1 1\n1.25 2.5\n");
    }
    let f = Fixture::new(s.as_bytes());
    let p = Ascii::open(&f.path, &flag()).unwrap();
    assert_eq!(p.total, 3000);
    assert_eq!(p.records.len(), 3000);
    assert_eq!(p.error(0, 2999, &flag()).unwrap().number, 3000);
    assert_eq!(
        Ascii::open_limits(&f.path, &flag(), 4096, 100, 10)
            .err()
            .unwrap()
            .kind,
        ErrorKind::Incomplete
    );
    let mut db = super::super::Database::ascii(p, None);
    let page = db.errors(0, 2998, 7, &flag()).unwrap();
    assert_eq!(page.hits.len(), 2);
    assert!(page.next.is_none());
    assert_eq!(db.waived_count(0).unwrap(), 0);
    assert!(db.errors(0, 3001, 7, &flag()).is_err());
    assert!(db.errors(0, 0, 0, &flag()).is_err());
}
