use floe_app_core::jobdeck::{
    color::{ColorScheme, Mode},
    plan::{Analysis, AnalysisOptions},
    spec, view,
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;

fn normalize(mut report: Value) -> Value {
    let sources = &mut report["plan"]["sources"];
    sources.as_object_mut().unwrap().remove("probe_s");
    for file in sources["files"].as_array_mut().unwrap() {
        file.as_object_mut().unwrap().remove("probe_s");
    }
    report
}
fn spec_rows(text: &str) -> Value {
    json!(text
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|line| {
            let mut fields = line.split_whitespace();
            let command = fields.next().unwrap();
            let fields: serde_json::Map<String, Value> = fields
                .map(|f| {
                    let (key, value) = f.split_once('=').unwrap();
                    (
                        key.into(),
                        if matches!(key, "unit" | "scale") {
                            json!(value.parse::<f64>().unwrap())
                        } else {
                            json!(value)
                        },
                    )
                })
                .collect();
            json!([command, fields])
        })
        .collect::<Vec<_>>())
}
#[test]
#[ignore = "run tools/validate_app_jobdeck_plan.py for generated analysis/spec fixtures"]
fn analysis_load_selection_spec_and_skip_ledger_match_python() {
    let oracle: Value = serde_json::from_slice(
        &std::fs::read(std::env::var_os("FLOE_APP_ANALYSIS_ORACLE").expect("oracle required"))
            .unwrap(),
    )
    .unwrap();
    let flag = AtomicUsize::new(0);
    let mark_meta = PathBuf::from(oracle["mark_cache"].as_str().unwrap()).join("meta.json");
    let metadata = std::fs::read(&mark_meta).unwrap();
    let cases = oracle["cases"].as_array().unwrap();
    assert!(cases.len() >= 20);
    for case in cases {
        if case["bad_mark_cache"] == true {
            std::fs::write(&mark_meta, b"bad metadata").unwrap();
        }
        let options = AnalysisOptions {
            sources: Some(case["sources"].as_str().unwrap().into()),
            ids: serde_json::from_value(case["ids"].clone()).unwrap(),
            load_ids: serde_json::from_value(case["load_ids"].clone()).unwrap(),
            mode: Mode::parse(case["mode"].as_str().unwrap()).unwrap(),
            scheme: if case["scheme"].is_null() {
                None
            } else {
                Some(ColorScheme::from_json(&serde_json::to_vec(&case["scheme"]).unwrap()).unwrap())
            },
            skip_missing: true,
            strict: case["strict"].as_bool().unwrap(),
            ..Default::default()
        };
        let analysis =
            Analysis::open(Path::new(case["path"].as_str().unwrap()), &options, &flag).unwrap();
        assert_eq!(
            normalize(analysis.report().unwrap()),
            case["report"],
            "report {}",
            case["name"]
        );
        assert_eq!(
            json!(analysis.view_rows(&flag).unwrap().rows),
            case["rows"],
            "view {}",
            case["name"]
        );
        assert_eq!(
            json!(view::level_rows(&analysis.deck, &flag).unwrap()),
            case["levels"]
        );
        let spec = spec::compose(&analysis, &flag).unwrap();
        assert_eq!(
            spec_rows(&spec.text),
            spec_rows(case["spec"].as_str().unwrap()),
            "spec {}",
            case["name"]
        );
        assert_eq!(
            json!(spec.skipped),
            case["ledger"],
            "ledger {}",
            case["name"]
        );
        if case["bad_mark_cache"] == true {
            std::fs::write(&mark_meta, &metadata).unwrap();
        }
    }
    println!("RUST APP JOBDECK LOAD/SPEC: ALL OK ({} cases)", cases.len());
}
