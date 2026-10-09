use floe_app_core::jobdeck::sources::{file_header, SourceCatalog};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::atomic::AtomicUsize;

fn normalize(mut report: Value) -> Value {
    report.as_object_mut().unwrap().remove("probe_s");
    for file in report["files"].as_array_mut().unwrap() {
        file.as_object_mut().unwrap().remove("probe_s");
        if file["status"] == "unreadable" {
            assert!(!file["error"].as_str().unwrap().is_empty());
            file.as_object_mut().unwrap().remove("error");
        }
    }
    report
}
#[test]
#[ignore = "run tools/validate_app_jobdeck_sources.py to supply real synthetic source fixtures"]
fn source_headers_catalog_and_selection_are_bounded() {
    let oracle: Value = serde_json::from_slice(
        &std::fs::read(std::env::var_os("FLOE_APP_SOURCE_ORACLE").expect("oracle required"))
            .unwrap(),
    )
    .unwrap();
    let flag = AtomicUsize::new(0);
    for case in oracle["headers"].as_array().unwrap() {
        let path = case["path"].as_str().unwrap();
        let result = file_header(Path::new(path), &flag);
        if case["error"].as_bool().unwrap() {
            assert!(result.is_err(), "{path}");
        } else {
            assert_eq!(
                serde_json::to_value(result.unwrap()).unwrap(),
                case["header"],
                "{path}"
            );
        }
    }
    let dir = Path::new(oracle["directory"].as_str().unwrap());
    let names: Vec<_> = oracle["names"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    let mut catalog = SourceCatalog::new(dir).unwrap();
    catalog.probe_all(names.iter().copied(), &flag).unwrap();
    let actual = normalize(catalog.report());
    let expected = normalize(oracle["catalog"].clone());
    assert_eq!(
        actual["files"].as_array().unwrap().len(),
        expected["files"].as_array().unwrap().len()
    );
    for (a, b) in actual["files"]
        .as_array()
        .unwrap()
        .iter()
        .zip(expected["files"].as_array().unwrap())
    {
        assert_eq!(a, b, "source {}", a["tc"]);
    }
    assert_eq!(actual, expected);
    for (tc, expected) in oracle["header_dbus"].as_object().unwrap() {
        assert_eq!(
            json!(catalog.header_dbu(tc, &flag).unwrap()),
            *expected,
            "{tc}"
        );
    }
    // This source has a FIFO meta.json and no index: header-only must neither
    // inspect it nor register it as fully loaded in the selected catalog.
    let mut selected = SourceCatalog::new(dir).unwrap();
    selected.probe("chipA.oas", &flag).unwrap();
    let dbu = selected.header_dbu("unselected.oas", &flag).unwrap();
    assert_eq!(json!(dbu), oracle["unselected_dbu"]);
    assert_eq!(selected.infos.len(), 1);
    let cancelled = AtomicUsize::new(2);
    assert!(selected.header_dbu("chipA.oas", &cancelled).is_err());
    assert!(selected.probe("chipA.oas", &cancelled).is_err());
    println!("RUST APP JOBDECK SOURCES: ALL OK");
}
