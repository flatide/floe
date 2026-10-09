//! Required by validate_app_jobdeck.py. The legacy Python implementation is
//! a development oracle only; no Python is called by the application library.
use floe_app_core::jobdeck::{
    color::{ColorScheme, Mode},
    geom,
    parser::JobDeck,
    view::{self, ViewRows},
};
use floe_worker_client::Layers;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::atomic::AtomicUsize;

#[test]
#[ignore = "run tools/validate_app_jobdeck.py to create the mandatory Python/model oracle"]
fn parser_and_placement_match_legacy_and_hand_model() {
    let path = std::env::var_os("FLOE_APP_JOBDECK_ORACLE").expect("oracle file required");
    let oracle: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let flag = AtomicUsize::new(0);
    let cases = oracle["cases"].as_array().unwrap();
    assert!(cases.len() >= 100, "vacuous parser oracle");
    let mut plans = 0;
    for case in cases {
        let path = case["path"].as_str().unwrap();
        let deck = JobDeck::read(std::path::Path::new(path), false, &flag).unwrap();
        assert_eq!(
            deck.report().unwrap(),
            case["report"],
            "parser report: {path}"
        );
        assert_eq!(
            serde_json::to_value(&deck.chips).unwrap(),
            case["chips"],
            "parser fields: {path}"
        );
        assert_eq!(
            json!(deck.sources(None)),
            case["sources"],
            "source order: {path}"
        );
        assert_eq!(
            deck.instance_count().unwrap(),
            case["instance_count"].as_u64().unwrap()
        );
        assert_eq!(
            JobDeck::read(std::path::Path::new(path), true, &flag).is_err(),
            !deck.errors.is_empty()
        );
        assert_eq!(
            json!(view::level_rows(&deck, &flag).unwrap()),
            case["level_rows"],
            "load dialog: {path}"
        );
        for c in case["colors"].as_array().unwrap() {
            let scheme =
                ColorScheme::from_json(&serde_json::to_vec(&c["scheme"]).unwrap()).unwrap();
            let ids: Option<std::collections::BTreeSet<i64>> =
                serde_json::from_value(c["ids"].clone()).unwrap();
            assert_eq!(
                scheme
                    .build(&deck, ids.as_ref())
                    .unwrap()
                    .report(scheme.mode),
                c["report"],
                "color table: {path}"
            );
        }
        for run in case["plans"].as_array().unwrap() {
            plans += 1;
            let dbus = serde_json::from_value(run["dbus"].clone()).unwrap();
            let bad = run["bad"]
                .as_object()
                .unwrap()
                .iter()
                .map(|(tc, row)| {
                    (
                        tc.clone(),
                        geom::SourceIssue {
                            reason: row[0].as_str().unwrap().into(),
                            detail: row[1].as_str().unwrap().into(),
                            stage: row[2].as_str().unwrap().into(),
                        },
                    )
                })
                .collect::<BTreeMap<_, _>>();
            let options = geom::PlanOptions {
                cross: run["cross"].as_bool().unwrap(),
                by_chip: run["by_chip"].as_bool().unwrap(),
                skip_missing: run["skip"].as_bool().unwrap(),
                selected: serde_json::from_value(run["selection"].clone()).unwrap(),
                ..Default::default()
            };
            let result = geom::plan(&deck, &dbus, &bad, &options, &flag);
            if run["error"].as_bool().unwrap() {
                assert!(result.is_err(), "expected missing-source failure: {path}");
                continue;
            }
            let result = result.unwrap();
            assert_eq!(
                serde_json::to_value(&result.placements).unwrap(),
                run["placements"],
                "placements {path}: {options:?}"
            );
            assert_eq!(
                serde_json::to_value(&result.stats).unwrap(),
                run["stats"],
                "plan stats {path}: {options:?}"
            );
            let outputs: Vec<_> = result
                .placements
                .iter()
                .map(|p| result.output_layer(&p.chip, p.idx, p.ly, p.dt).unwrap())
                .collect();
            assert_eq!(json!(outputs), run["outputs"], "layer lookup: {path}");
            for v in run["views"].as_array().unwrap() {
                let scheme = ColorScheme {
                    mode: Mode::parse(v["mode"].as_str().unwrap()).unwrap(),
                    cross_ly_dt: options.cross,
                    overrides: BTreeMap::from([
                        ("1".into(), "#123456".into()),
                        ("C1".into(), "#f00baa".into()),
                    ]),
                    ..Default::default()
                };
                let colors = scheme.build(&deck, None).unwrap();
                let rows = ViewRows::build(&deck, &result.stats, &scheme, &colors, &flag).unwrap();
                assert_eq!(json!(rows.rows), v["rows"], "view rows: {path}");
                assert_eq!(
                    json!(rows.metadata(&result.placements).unwrap()),
                    v["meta"],
                    "view metadata: {path}"
                );
                assert_eq!(
                    json!(result
                        .placements
                        .iter()
                        .map(|p| rows.output_layer(p).unwrap())
                        .collect::<Vec<_>>()),
                    v["outputs"]
                );
                for r in v["resolved"].as_array().unwrap() {
                    let resolved = rows.resolve_layers(&deck, r["spec"].as_str());
                    if r["error"] == true {
                        assert!(resolved.is_err(), "selector must fail: {r}");
                    } else {
                        let value = match resolved.unwrap() {
                            Layers::All => Value::Null,
                            Layers::None => json!([]),
                            Layers::Only(keys) => json!(keys),
                        };
                        assert_eq!(value, r["value"], "selector {path}: {r}");
                    }
                }
            }
            if let Some(hand) = case["hand"].as_object() {
                if options.selected.is_none() && !options.by_chip && options.cross {
                    assert_eq!(result.placements.len(), 17);
                    for want in hand["placements"].as_array().unwrap() {
                        let actual = result
                            .placements
                            .iter()
                            .find(|p| {
                                p.chip == want["chip"].as_str().unwrap()
                                    && p.idx == want["idx"].as_i64().unwrap()
                                    && p.row == want["row"].as_u64().unwrap() as usize
                            })
                            .unwrap();
                        for (value, key) in [
                            (actual.mag, "mag"),
                            (actual.dx_um, "dx"),
                            (actual.dy_um, "dy"),
                        ] {
                            assert!(
                                (value - want[key].as_f64().unwrap()).abs() < 1e-9,
                                "hand model {path}/{key}"
                            );
                        }
                    }
                    assert_eq!(result.stats.dbu, 2.5e-5);
                    assert_eq!(result.stats.residual_nonzero, 0);
                }
            }
        }
    }
    assert!(plans >= 100, "vacuous placement oracle");
    println!(
        "RUST APP JOBDECK MODEL: ALL OK ({} parser cases, {plans} plans)",
        cases.len()
    );
}
