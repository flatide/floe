//! All inputs are private synthetic Python-oracle files. Test execution has PATH
//! empty. Codec tests are read-only; store tests write only explicitly named
//! synthetic-rust-store targets checked by the Python harness.
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

#[test]
#[ignore = "run tools/validate_drc_review.py with private synthetic sidecars"]
fn store_publication_matches_python() {
    use floe_app_core::{
        drc::review::{
            managed::{ManagedStore, Phase, Publication, Registration},
            store::Kind,
        },
        managed::{Limits, Resources, Usage},
        registered::AccessScope,
    };
    use std::{
        fs,
        sync::Arc,
        time::{Duration, Instant},
    };
    fn published(mut job: Publication) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !job.is_finished() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        job.close().unwrap();
        let status = job.status();
        assert_eq!(status.phase, Phase::Succeeded);
        assert!(!status.outcome_unknown);
        assert!(status.outcome.unwrap().directory_synced);
    }
    let cases: Value = serde_json::from_slice(
        &fs::read(std::env::var_os("FLOE_DRC_REVIEW_ORACLE").unwrap()).unwrap(),
    )
    .unwrap();
    let stop = Arc::new(AtomicUsize::new(0));
    let resources = Resources::new(Limits::default()).unwrap();
    let mut writes = 0;
    for case in cases.as_array().unwrap() {
        let pack = Path::new(case["pack"].as_str().unwrap());
        let scope = AccessScope::new(&[pack.parent().unwrap().to_owned()]).unwrap();
        let waives = ManagedStore::open(
            &resources,
            Registration {
                scope: Arc::clone(&scope),
                pack: pack.into(),
                reviewer: "synthetic-rust-store".into(),
                kind: Kind::Waives,
                protected_files: vec![],
                protected_trees: vec![],
            },
            &stop,
        )
        .unwrap();
        for expected in case["waives"].as_array().unwrap() {
            let data: Vec<u8> = serde_json::from_value(expected["input"].clone()).unwrap();
            let edits: Vec<(u64, u8)> = serde_json::from_value(expected["edits"].clone()).unwrap();
            // Only a named synthetic test target, never the Python/source file.
            fs::write(waives.target(), data).unwrap();
            let draft = waives
                .snapshot(Arc::clone(&stop))
                .unwrap()
                .prepare_waives(&edits)
                .unwrap();
            published(draft.publish(true).unwrap());
            assert_eq!(
                json!(fs::read(waives.target()).unwrap()),
                expected["output"]
            );
            assert_eq!(
                json!(
                    waives
                        .snapshot(Arc::clone(&stop))
                        .unwrap()
                        .waives()
                        .unwrap()
                        .per_rule
                ),
                expected["counts"]
            );
            writes += 1;
        }
        let notes = ManagedStore::open(
            &resources,
            Registration {
                scope,
                pack: pack.into(),
                reviewer: "synthetic-rust-store".into(),
                kind: Kind::Notes,
                protected_files: vec![],
                protected_trees: vec![],
            },
            &stop,
        )
        .unwrap();
        for expected in case["notes"].as_array().unwrap() {
            let snapshot = notes.snapshot(Arc::clone(&stop)).unwrap();
            let draft = if expected["op"] == "set" {
                let ids: Vec<u64> = serde_json::from_value(expected["ids"].clone()).unwrap();
                snapshot
                    .prepare_note(&ids, expected["text"].as_str().unwrap())
                    .unwrap()
            } else {
                snapshot
                    .prepare_notes_import(expected["input"].as_str().unwrap())
                    .unwrap()
                    .0
            };
            published(draft.publish(false).unwrap());
            let text = fs::read_to_string(notes.target()).unwrap();
            if let Some(expected) = expected["output"].as_str() {
                assert_eq!(text, expected);
            } else {
                assert!(text.contains("floe_pack="));
                assert!(!text.contains("floe_note="));
            }
            let loaded = notes.snapshot(Arc::clone(&stop)).unwrap();
            assert!(!loaded.legacy_unverified());
            assert_eq!(
                json!(loaded
                    .notes()
                    .unwrap()
                    .groups()
                    .map(|n| (&n.text, &n.members))
                    .collect::<Vec<_>>()),
                expected["groups"]
            );
            for (gid, expected) in expected["lookup"].as_array().unwrap().iter().enumerate() {
                assert_eq!(json!(loaded.notes().unwrap().get(gid as u64)), *expected);
            }
            writes += 1;
        }
        drop((waives, notes));
        assert_eq!(resources.usage(), Usage::default());
    }
    assert_eq!(writes, 28);
    println!("RUST DRC REVIEW STORE: ALL OK ({writes} managed native publications, admission release, real pack centers, Python bytes, reload and empty tombstones)");
}
