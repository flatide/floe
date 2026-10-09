//! Native pack + Python oracle, private generated DB only. No Python at runtime.
use floe_app_core::drc::{cd_segments, Cursor, Pack};
use serde_json::{json, Value};
use std::{collections::BTreeSet, path::Path, sync::atomic::AtomicUsize};
#[test]
#[ignore = "run tools/validate_app_drc.py with synthetic ASCII cases"]
fn ascii_metadata_coordinates_and_file_order_match_python() {
    let input: Value = serde_json::from_slice(
        &std::fs::read(std::env::var_os("FLOE_DRC_ASCII_ORACLE").expect("private ASCII oracle"))
            .unwrap(),
    )
    .unwrap();
    let flag = AtomicUsize::new(0);
    let mut errors = 0;
    let mut cases = 0;
    for case in input.as_array().unwrap() {
        let p = floe_app_core::drc::Ascii::open(Path::new(case["path"].as_str().unwrap()), &flag)
            .unwrap();
        assert_eq!(json!(p.cell), case["cell"]);
        assert_eq!(json!(p.precision), case["precision"]);
        assert_eq!(p.checks.len(), case["checks"].as_array().unwrap().len());
        for (ci, c) in case["checks"].as_array().unwrap().iter().enumerate() {
            assert_eq!(json!(p.checks[ci].name), c["name"]);
            assert_eq!(json!(p.checks[ci].desc), c["desc"]);
            assert_eq!(json!(p.checks[ci].declared), c["declared"]);
            assert_eq!(
                p.checks[ci].count as usize,
                c["errors"].as_array().unwrap().len()
            );
            for (ei, e) in c["errors"].as_array().unwrap().iter().enumerate() {
                let v = p.error(ci, ei as u64, &flag).unwrap();
                assert_eq!(json!(v.kind.to_string()), e["kind"]);
                assert_eq!(json!(v.number), e["num"]);
                assert_eq!(json!(v.points_um), e["pts"]);
                assert_eq!(json!(v.bbox_um), e["bbox"]);
                errors += 1;
            }
        }
        p.unchanged().unwrap();
        cases += 1;
    }
    assert!(cases >= 4 && errors > 1000);
    println!("RUST ASCII DRC READER: ALL OK ({cases} sources, {errors} records; exact float coordinates and metadata)");
}
#[test]
#[ignore = "run tools/validate_app_drc.py with synthetic native packs"]
fn packed_geometry_status_and_paged_queries_match_python() {
    let input: Value = serde_json::from_slice(
        &std::fs::read(std::env::var_os("FLOE_DRC_ORACLE").expect("private oracle")).unwrap(),
    )
    .unwrap();
    let flag = AtomicUsize::new(0);
    let mut errors = 0;
    let mut queries = 0;
    for case in input.as_array().unwrap() {
        let mut p = Pack::open(Path::new(case["pack"].as_str().unwrap()), &flag).unwrap();
        assert_eq!(p.decoded_blocks, 0);
        assert_eq!(json!(p.cell), case["cell"]);
        assert_eq!(json!(p.precision), case["precision"]);
        p.attach_waives(Path::new(case["waives"].as_str().unwrap()))
            .unwrap();
        assert_eq!(p.checks.len(), case["checks"].as_array().unwrap().len());
        for (ci, c) in case["checks"].as_array().unwrap().iter().enumerate() {
            assert_eq!(json!(p.checks[ci].name), c["name"]);
            assert_eq!(json!(p.checks[ci].desc), c["desc"]);
            assert_eq!(json!(p.checks[ci].declared), c["declared"]);
            assert_eq!(json!(p.waived_count(ci).unwrap()), c["waived"]);
            assert_eq!(
                p.checks[ci].count as usize,
                c["errors"].as_array().unwrap().len()
            );
            for (ei, e) in c["errors"].as_array().unwrap().iter().enumerate() {
                let record = p.error(ci, ei as u64, &flag).unwrap();
                assert_eq!(json!(record.number), e["num"]);
                assert_eq!(json!(record.kind.to_string()), e["kind"]);
                let pts: Vec<_> = record
                    .points
                    .iter()
                    .map(|xy| xy.map(|n| n as f64 / p.precision))
                    .collect();
                assert_eq!(json!(pts), e["pts"]);
                assert_eq!(json!(p.status(ci, ei as u64).unwrap()), e["status"]);
                let rulers = cd_segments(record.kind, &record.points, p.precision).unwrap();
                let expected: Vec<[f64; 4]> = serde_json::from_value(e["cd"].clone()).unwrap();
                assert_eq!(rulers.len(), expected.len(), "CD count {ci}/{ei}: {e}");
                let close = |a: f64, b: f64| (a - b).abs() <= 1e-11 + 1e-10 * a.abs().max(b.abs());
                for (got, want) in rulers.iter().zip(expected) {
                    assert!(
                        got.endpoints_um
                            .iter()
                            .flatten()
                            .zip(want)
                            .all(|(a, b)| close(*a, b)),
                        "CD endpoints {ci}/{ei}: {got:?} {want:?}"
                    );
                    let d = (want[2] - want[0]).hypot(want[3] - want[1]);
                    assert!(
                        close(got.distance_um, d),
                        "CD distance {ci}/{ei}: {got:?} {want:?}"
                    );
                    assert_eq!(got.offset, record.kind == 'e' && record.points.len() == 2);
                }
                errors += 1;
            }
            let mut cursor = 0;
            let mut paged = Vec::new();
            loop {
                let page = p.errors(ci, cursor, 7, &flag).unwrap();
                paged.extend(
                    page.hits
                        .into_iter()
                        .map(|h| (h.local, h.violation.number, h.status)),
                );
                if let Some(next) = page.next {
                    assert!(next.error > cursor);
                    cursor = next.error;
                } else {
                    break;
                }
            }
            let expected: Vec<_> = c["errors"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
                .map(|(i, e)| {
                    (
                        i as u64,
                        e["num"].as_u64().unwrap(),
                        e["status"].as_u64().unwrap() as u8,
                    )
                })
                .collect();
            assert_eq!(paged, expected);
        }
        for q in case["queries"].as_array().unwrap() {
            let bbox = serde_json::from_value(q["bbox"].clone()).unwrap();
            let checks: Option<BTreeSet<usize>> =
                serde_json::from_value(q["checks"].clone()).unwrap();
            let waived = q["waived"].as_bool();
            let limit = q["limit"].as_u64().unwrap() as usize;
            let mut cursor = Cursor::default();
            let mut got = Vec::new();
            let mut pages = 0;
            loop {
                let page = p
                    .query(bbox, checks.as_ref(), waived, cursor, limit, &flag)
                    .unwrap();
                assert!(page.scanned <= floe_app_core::drc::SCAN_ITEMS);
                assert!(page.hits.len() <= limit);
                got.extend(
                    page.hits
                        .into_iter()
                        .map(|h| json!([h.check, h.local, h.violation.number])),
                );
                pages += 1;
                if let Some(next) = page.next {
                    assert_ne!(cursor, next);
                    cursor = next;
                } else {
                    break;
                }
                assert!(pages <= p.total + 2 * p.checks.len() as u64 + 10);
            }
            assert_eq!(json!(got), q["expected"], "query {q}");
            queries += 1;
        }
        p.unchanged().unwrap();
    }
    assert!(errors > 1000);
    assert!(queries >= 100);
    println!(
        "RUST DRC READER: ALL OK ({errors} error records + CD rulers, {queries} paged query pairs)"
    );
}
