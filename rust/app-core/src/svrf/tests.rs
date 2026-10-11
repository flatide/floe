use super::*;
use serde_json::{json, Value};
use std::sync::atomic::Ordering;

fn stop() -> AtomicUsize {
    AtomicUsize::new(0)
}
fn parse(checks: Value, derived: Value) -> Rules {
    Rules::parse(
        &serde_json::to_vec(
            &json!({"format":"floe-svrf-rules","version":1,"checks":checks,"derived":derived}),
        )
        .unwrap(),
        &stop(),
    )
    .unwrap()
}
fn constraint(metric: &str, op: &str, value: Option<f64>) -> Value {
    json!({"metric":metric,"op":op,"value":value,"text":format!("{metric} {op} bound")})
}

#[test]
fn typed_rules_reject_invalid_format_duplicates_and_unknown_versions() {
    let base = json!({"format":"floe-svrf-rules","version":1,"checks":{"한글.Rule":{}}, "deck":"/must/not/read", "stats":{"includes":["/also/not/read"]}});
    let r = Rules::parse(&serde_json::to_vec(&base).unwrap(), &stop()).unwrap();
    assert!(r.rule("한글.Rule").is_some());
    assert!(r.rule("missing").is_none());
    for (field, value) in [
        ("format", json!("oops")),
        ("version", json!(2)),
        ("version", json!(0)),
        ("checks", Value::Null),
    ] {
        let mut b = base.clone();
        b[field] = value;
        assert!(Rules::parse(&serde_json::to_vec(&b).unwrap(), &stop()).is_err());
    }
    for s in [
        r#"{"format":"floe-svrf-rules","version":1,"checks":{"a":{},"a":{}}}"#,
        r#"{"format":"floe-svrf-rules","version":1,"checks":{},"derived":{"a":"x","a":"y"}}"#,
        r#"{"format":"floe-svrf-rules","version":1,"version":1,"checks":{}}"#,
        r#"{"format":"floe-svrf-rules","version":1,"checks":{"a":{"source_gds":[[1,-1]]}}}"#,
        r#"{"format":"floe-svrf-rules","version":1,"checks":{"a":{"source_gds":[[1,0,2]]}}}"#,
        r#"{"format":"floe-svrf-rules","version":1,"checks":{"a":{"constraints":[{"metric":"area","op":"<","value":1e999,"text":"x"}]}}}"#,
    ] {
        assert!(Rules::parse(s.as_bytes(), &stop()).is_err(), "{s}");
    }
    assert!(Rules::parse(&[0xff], &stop()).is_err());
}

#[test]
fn bounded_collections_text_bytes_and_cancel() {
    let raw = |rule: Value| {
        serde_json::to_vec(&json!({"format":"floe-svrf-rules","version":1,"checks":{"r":rule}}))
            .unwrap()
    };
    for rule in [
        json!({"layers":vec!["x";4097]}),
        json!({"constraints":vec![constraint("width","<",Some(1.));1025]}),
        json!({"desc":"x".repeat(MAX_TEXT+1)}),
    ] {
        assert!(Rules::parse(&raw(rule), &stop()).is_err());
    }
    assert!(Rules::parse(&raw(json!({"desc":"x".repeat(MAX_TEXT)})), &stop()).is_ok());
    let pairs: BTreeMap<_, _> = (0..=MAX_ENTRIES)
        .map(|n| (n.to_string(), json!({})))
        .collect();
    let b = serde_json::to_vec(&json!({"format":"floe-svrf-rules","version":1,"checks":pairs}))
        .unwrap();
    assert!(Rules::parse(&b, &stop()).is_err());
    assert!(matches!(
        Rules::parse(&vec![b' '; MAX_RULES_BYTES + 1], &stop())
            .unwrap_err()
            .kind,
        ErrorKind::Incomplete
    ));
    let s = stop();
    s.store(1, Ordering::Relaxed);
    assert!(matches!(
        Rules::parse(b"{}", &s).unwrap_err().kind,
        ErrorKind::Cancelled
    ));
    let r = parse(json!({"r":{}}), json!({}));
    assert!(r.detail("r", &s).is_err());
    assert!(r.catalog(["r"], &s).is_err());
}

#[test]
fn types_count_rules_not_constraints_and_keep_legacy_order() {
    let r = parse(
        json!({
            "a":{"constraints":[constraint("space","<",Some(1.)),constraint("width",">",Some(0.)),constraint("width","<",Some(1.)),constraint("z-custom","<",None)]},
            "b":{},"unused":{"constraints":[constraint("area","<",Some(5.))]}
        }),
        json!({}),
    );
    let c = r.catalog(["a", "a", "b", "unmatched"], &stop()).unwrap();
    assert_eq!((c.matched, c.checks), (3, 4));
    assert_eq!(
        c.types
            .iter()
            .map(|t| (t.metric, t.checks))
            .collect::<Vec<_>>(),
        vec![("width", 2), ("space", 2), ("other", 2), ("z-custom", 2)]
    );
    let empty = parse(json!({}), json!({}));
    assert!(empty.catalog(["a"], &stop()).unwrap().types.is_empty());
}

#[test]
fn derivation_breadth_first_cycle_truncation_unicode_and_source_layers() {
    let r = parse(
        json!({"r":{"layers":["A","A","B","M1"],"source_gds":[[7,null],[8,3]],"unresolved":["missing"]}}),
        json!({
            "A":"B OR C", "B":"A AND D", "C":"M1 not E", "D":"E XOR F", "E":"F", "F":"G", "G":"M2"
        }),
    );
    let d = r.detail("r", &stop()).unwrap().unwrap();
    assert_eq!(
        d.derivations.iter().map(|x| x.name).collect::<Vec<_>>(),
        vec!["A", "B", "C", "D", "E", "F"]
    );
    assert!(d.derivations_more);
    assert!(d.rule.includes_layer(7, 999));
    assert!(d.rule.includes_layer(8, 3));
    assert!(!d.rule.includes_layer(8, 4));
    assert!(!d.rule.includes_layer(1, 0));
    assert_eq!(
        operands("한글 A AND foo.bar Size _x-2 BY .3 原본").collect::<Vec<_>>(),
        vec!["A", "foo.bar", "_x-2"]
    );
    let cycle = parse(json!({"r":{"layers":["A"]}}), json!({"A":"B", "B":"A"}));
    let d = cycle.detail("r", &stop()).unwrap().unwrap();
    assert_eq!(d.derivations.len(), 2);
    assert!(!d.derivations_more);
}

#[test]
fn comparison_prefers_first_measurable_upper_bound_without_verdict() {
    let r = parse(
        json!({"r":{"constraints":[constraint("density","<",Some(0.5)),constraint("width",">",Some(0.)),constraint("space","<",Some(4.)),constraint("area","<=",Some(100.))]}}),
        json!({}),
    );
    let p = [[0, 0], [3000, 0], [3000, 5000], [0, 5000]];
    let c = r
        .rule("r")
        .unwrap()
        .compare('p', &p, 1000., &stop())
        .unwrap()
        .unwrap();
    assert_eq!(
        (c.constraint, c.metric, c.op, c.unit),
        (2, "space", "<", "um")
    );
    assert_eq!(
        (c.measured, c.bound, c.delta, c.percent),
        (3., 4., -1., Some(-25.))
    );
    let r = parse(
        json!({"r":{"constraints":[constraint("area",">",Some(0.))]}}),
        json!({}),
    );
    let c = r
        .rule("r")
        .unwrap()
        .compare('p', &p, 1000., &stop())
        .unwrap()
        .unwrap();
    assert_eq!((c.measured, c.unit, c.percent), (15., "um2", None));
    assert!(r
        .rule("r")
        .unwrap()
        .compare('e', &p, 1000., &stop())
        .unwrap()
        .is_none());
    assert!(r
        .rule("r")
        .unwrap()
        .compare('p', &p, 1000., &AtomicUsize::new(1))
        .is_err());
}

#[test]
fn measurement_only_supported_shapes_and_exact_translated_area() {
    let m = |kind, p: &[[i64; 2]], metric| measured(kind, p, 1., metric, &stop()).unwrap();
    assert_eq!(m('e', &[[0, 0], [3, 4]], "length"), Some(5.));
    assert_eq!(m('e', &[[2, 2], [2, 2]], "length"), Some(0.));
    assert_eq!(m('p', &[[0, 0], [3, 0], [0, 4]], "area"), Some(6.));
    assert_eq!(m('p', &[[0, 4], [3, 0], [0, 0]], "area"), Some(6.));
    assert_eq!(m('p', &[[0, 0], [3, 0], [0, 4]], "width"), None);
    assert_eq!(m('e', &[[0, 0], [3, 4]], "width"), None);
    assert_eq!(m('e', &[[0, 0], [3, 4]], "angle"), None);
    let origin = i64::MAX - 10;
    assert_eq!(
        m(
            'p',
            &[
                [origin, origin],
                [origin + 1, origin],
                [origin + 1, origin + 1],
                [origin, origin + 1]
            ],
            "area"
        ),
        Some(1.)
    );
    assert_eq!(
        m('e', &[[origin, origin], [origin + 1, origin]], "length"),
        Some(1.)
    );
    assert!(measured(
        'p',
        &[
            [i64::MIN, i64::MIN],
            [i64::MAX, i64::MIN],
            [i64::MAX, i64::MAX]
        ],
        1.,
        "area",
        &stop()
    )
    .is_err());
    assert!(measured('p', &[[0, 0], [1, 0], [1, 1]], f64::MAX, "area", &stop()).is_err());
    for precision in [0., -1., f64::NAN, f64::INFINITY] {
        assert!(measured('p', &[], precision, "area", &stop()).is_err());
    }
}
