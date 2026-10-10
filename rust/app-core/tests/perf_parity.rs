//! The perf line's parity with the GTK viewer's Python (step P4b,
//! docs/SHARED_APP_LAYER.ko.md §7): tools/validate_perf_parity.py (gate
//! `perf_parity`) writes a corpus - synthetic result dicts with Python's
//! `perf_status` / `fmt_count` / `occ_note` / `_load_note` strings, and real
//! frame rounds recorded through floe_oracle/rust_render.py (FLOE_RUST_RECORD) with
//! the result `_emit_frame` emitted - and this replays it through
//! `floe_app_core::view::perf`: every string byte for byte, every result equal
//! in keys, value types and values. FLOE_PERF_OUT gets what Rust made.
//!
//!   FLOE_PERF_CORPUS=corpus.json cargo test --release --offline \
//!       -p floe-app-core --test perf_parity -- --ignored --nocapture
use floe_app_core::view::perf::{
    fmt_count, load_note, occ_note, perf_status_with, FrameReport, LoadMarks, PerfAdapter, PerfJob,
    PerfTiming,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};

fn text(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

fn fields_of(v: &Value) -> BTreeMap<String, String> {
    v.as_object()
        .expect("fields")
        .iter()
        .map(|(k, v)| (k.clone(), text(v)))
        .collect()
}

/// The first difference between two results, a path to it and both sides.
fn first_difference(path: &str, a: &Value, b: &Value) -> Option<String> {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for k in x.keys().chain(y.keys()) {
                let (u, v) = (x.get(k), y.get(k));
                match (u, v) {
                    (Some(u), Some(v)) => {
                        if let Some(d) = first_difference(&format!("{path}.{k}"), u, v) {
                            return Some(d);
                        }
                    }
                    _ => return Some(format!("{path}.{k}: rust {u:?} python {v:?}")),
                }
            }
            None
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => x
            .iter()
            .zip(y)
            .enumerate()
            .find_map(|(i, (u, v))| first_difference(&format!("{path}[{i}]"), u, v)),
        _ => (a != b).then(|| format!("{path}: rust {a} python {b}")),
    }
}

#[test]
#[ignore = "needs FLOE_PERF_CORPUS (tools/validate_perf_parity.py)"]
fn perf_line_matches_python() {
    let path = std::env::var("FLOE_PERF_CORPUS").expect("FLOE_PERF_CORPUS");
    let corpus: Value =
        serde_json::from_slice(&std::fs::read(&path).expect("read corpus")).expect("corpus json");
    let empty = Vec::new();
    let list = |k: &str| corpus[k].as_array().unwrap_or(&empty);
    let mut failures = Vec::new();
    let mut fail = |what: String| {
        if failures.len() < 40 {
            eprintln!("MISMATCH {what}");
        }
        failures.push(what);
    };

    let mut status_out = Vec::new();
    for (i, case) in list("status").iter().enumerate() {
        let (full, brief) = perf_status_with(
            &case["res"],
            case["depth_note"].as_str().unwrap_or(""),
            case["density_only"].as_bool().unwrap_or(false),
        );
        if full != text(&case["full"]) {
            fail(format!(
                "status #{i} full\n  rust   {full}\n  python {}",
                text(&case["full"])
            ));
        }
        if brief != text(&case["brief"]) {
            fail(format!(
                "status #{i} brief\n  rust   {brief}\n  python {}",
                text(&case["brief"])
            ));
        }
        status_out.push(json!([full, brief]));
    }
    let mut count_out = Vec::new();
    for (i, case) in list("count").iter().enumerate() {
        let out = fmt_count(&case["n"]);
        if out != text(&case["out"]) {
            fail(format!(
                "fmt_count #{i} of {}: rust {out} python {}",
                case["n"], case["out"]
            ));
        }
        count_out.push(Value::from(out));
    }
    let mut occ_out = Vec::new();
    for (i, case) in list("occ").iter().enumerate() {
        let out = occ_note(&case["res"], case["full"].as_bool().unwrap_or(false));
        if out != text(&case["out"]) {
            fail(format!(
                "occ_note #{i}: rust {out:?} python {}",
                case["out"]
            ));
        }
        occ_out.push(Value::from(out));
    }
    let mut load_out = Vec::new();
    for (i, case) in list("load").iter().enumerate() {
        let marks = case["marks"].as_object().map(|m| LoadMarks {
            t0: m["t0"].as_f64().unwrap(),
            cache: m["cache"].as_f64().unwrap(),
            service: m["service"].as_f64().unwrap(),
            renderd_open_ms: m.get("renderd_open_ms").and_then(Value::as_f64),
        });
        let (full, brief) = load_note(marks.as_ref(), &case["res"], case["now"].as_f64().unwrap());
        if full != text(&case["full"]) || brief != text(&case["brief"]) {
            fail(format!(
                "load_note #{i}: rust ({full:?}, {brief:?}) python ({}, {})",
                case["full"], case["brief"]
            ));
        }
        load_out.push(json!([full, brief]));
    }

    // the recorded rounds, generation by generation in the order they came
    let mut reports: HashMap<(i64, i64), FrameReport> = HashMap::new();
    let mut rounds_out = Vec::new();
    let mut generations = 0;
    for (i, rec) in list("rounds").iter().enumerate() {
        let key = (
            rec["session"].as_i64().unwrap_or(0),
            rec["state"].as_i64().expect("state"),
        );
        let report = reports.entry(key).or_insert_with(|| {
            generations += 1;
            FrameReport::new(PerfJob::from_python_job(&rec["job"]))
        });
        let adapter = PerfAdapter {
            raster_jobs: rec["raster_jobs"].as_i64().expect("raster_jobs"),
            max_depth: rec["max_depth"].as_i64(),
            dbu: rec["dbu"].as_f64(),
        };
        let timing = PerfTiming {
            adapter_read_us: rec["adapter_read_us"].as_i64().expect("adapter_read_us"),
            elapsed_ms: rec["elapsed_ms"].as_f64().expect("elapsed_ms"),
        };
        let probe = rec["probe"].as_bool().unwrap_or(false);
        let result = report.round(&fields_of(&rec["fields"]), probe, &adapter, timing);
        if let Some(d) = first_difference("result", &result, &rec["result"]) {
            fail(format!("round #{i} (gen {}): {d}", rec["result"]["gen"]));
        }
        let (full, brief) =
            perf_status_with(&result, rec["depth_note"].as_str().unwrap_or(""), false);
        // (null: a fuzzed value Python's perf_status itself refuses)
        if !rec["full"].is_null() && (full != text(&rec["full"]) || brief != text(&rec["brief"])) {
            fail(format!(
                "round #{i} perf line\n  rust   {full}\n  python {}\n  rust   {brief}\n  python {}",
                text(&rec["full"]),
                text(&rec["brief"])
            ));
        }
        rounds_out.push(json!({"result": result, "full": full, "brief": brief}));
    }

    if let Ok(out) = std::env::var("FLOE_PERF_OUT") {
        let doc = json!({
            "status": status_out, "count": count_out, "occ": occ_out,
            "load": load_out, "rounds": rounds_out,
        });
        std::fs::write(out, serde_json::to_vec(&doc).unwrap()).expect("write FLOE_PERF_OUT");
    }
    let synthetic =
        list("status").len() + list("count").len() + list("occ").len() + list("load").len();
    println!(
        "perf parity (rust): {} synthetic cases ({} perf lines, {} counts, {} occupancy notes, \
         {} load notes), {} recorded rounds in {} generations: {}",
        synthetic,
        list("status").len(),
        list("count").len(),
        list("occ").len(),
        list("load").len(),
        list("rounds").len(),
        generations,
        if failures.is_empty() {
            "all equal".to_string()
        } else {
            format!("{} MISMATCHES", failures.len())
        }
    );
    assert!(
        failures.is_empty(),
        "{} perf parity mismatches",
        failures.len()
    );
}
