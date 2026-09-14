//! All inputs are private synthetic Python-oracle files. Test execution has PATH
//! empty and never writes the source pack, sidecars or an ambient reviewer file.
use floe_app_core::drc::{
    review::{rewrite_waives, Layout, Notes},
    Pack,
};
use serde_json::{json, Value};
use std::{path::Path, sync::atomic::AtomicUsize};

#[test]
#[ignore = "run tools/validate_drc_review.py with private synthetic sidecars"]
fn sidecars_and_note_edits_match_python() {
    let cases: Value = serde_json::from_slice(
        &std::fs::read(std::env::var_os("FLOE_DRC_REVIEW_ORACLE").unwrap()).unwrap(),
    )
    .unwrap();
    let stop = AtomicUsize::new(0);
    let mut steps = 0;
    let mut statuses = 0;
    for case in cases.as_array().unwrap() {
        let pack = Pack::open(Path::new(case["pack"].as_str().unwrap()), &stop).unwrap();
        let layout = Layout::from_pack(&pack).unwrap();
        let fp = layout.fingerprint();
        let centers: Vec<[f64; 2]> = serde_json::from_value(case["centers"].clone()).unwrap();
        for waive in case["waives"].as_array().unwrap() {
            let data: Vec<u8> = serde_json::from_value(waive["input"].clone()).unwrap();
            let edits: Vec<(u64, u8)> = serde_json::from_value(waive["edits"].clone()).unwrap();
            let mut out = Vec::new();
            let stats = rewrite_waives(data.as_slice(), &mut out, &layout, &edits, &stop).unwrap();
            assert_eq!(json!(out), waive["output"]);
            assert_eq!(json!(stats.per_rule), waive["counts"]);
            statuses += pack.total;
        }
        let mut notes = Notes::new(fp);
        for step in case["notes"].as_array().unwrap() {
            if step["op"] == "set" {
                let ids: Vec<u64> = serde_json::from_value(step["ids"].clone()).unwrap();
                notes
                    .set(&ids, step["text"].as_str().unwrap(), &stop)
                    .unwrap();
            } else {
                let (parsed, report) =
                    Notes::parse(step["input"].as_str().unwrap(), fp, &stop).unwrap();
                assert_eq!(report.invalid_members, 0);
                assert_eq!(report.skipped_lines, 0);
                assert_eq!(report.reassigned_members, 0);
                notes = parsed;
            }
            assert_eq!(
                json!(notes
                    .groups()
                    .map(|n| (&n.text, &n.members))
                    .collect::<Vec<_>>()),
                step["groups"]
            );
            let out = notes.serialize(|g| Ok(centers[g as usize]), &stop).unwrap();
            assert_eq!(json!(out), step["output"]);
            for gid in 0..pack.total {
                assert_eq!(json!(notes.get(gid)), step["lookup"][gid as usize]);
            }
            steps += 1;
        }
        pack.unchanged().unwrap();
    }
    assert!(statuses > 1000 && steps >= 20);
    println!("RUST DRC REVIEW CODECS: ALL OK ({steps} note states, {statuses} status bytes; no filesystem writes)");
}
