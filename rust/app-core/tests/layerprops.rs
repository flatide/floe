use floe_app_core::{
    jobdeck::color::Mode,
    layerprops::{self, Row},
    managed::{Limits, ManagedDataset, Resources},
    view::{Model, Patch, ViewState},
};
use floe_worker_client::{Fill, Layers, Style};
use serde_json::{json, Value};
use std::{
    path::Path,
    sync::{atomic::AtomicUsize, Arc},
};

#[test]
#[ignore = "run tools/validate_layerprops.py for private Python/GTK oracles"]
fn python_codec_and_gtk_initial_visibility_match() {
    let oracle: Value = serde_json::from_slice(
        &std::fs::read(std::env::var_os("FLOE_LAYERPROPS_ORACLE").unwrap()).unwrap(),
    )
    .unwrap();
    let cases = oracle["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 72);
    for (i, case) in cases.iter().enumerate() {
        let doc = layerprops::parse(case["text"].as_str().unwrap()).unwrap();
        assert_eq!(json!(doc.rows), case["rows"], "parse {i}");
        assert_eq!(
            json!(doc.rows.iter().map(Row::line_width).collect::<Vec<_>>()),
            case["widths"],
            "width {i}"
        );
        assert_eq!(
            layerprops::format(&doc.rows).unwrap(),
            case["formatted"].as_str().unwrap(),
            "format {i}"
        );
    }
    let styles = oracle["styles"].as_array().unwrap();
    assert_eq!(styles.len(), 980);
    for case in styles {
        let style = Style {
            layer: (7, 20),
            color: serde_json::from_value(case["color"].clone()).unwrap(),
            fill: Fill::Pattern(serde_json::from_value(case["words"].clone()).unwrap()),
            width: 4,
        };
        let row = Row::from_style(&style, "mask with spaces", false).unwrap();
        assert_eq!(json!(row), case["row"]);
        assert_eq!(
            layerprops::format(&[row]).unwrap(),
            case["formatted"].as_str().unwrap()
        );
    }
    let views = oracle["views"].as_array().unwrap();
    assert_eq!(views.len(), 4);
    for case in views {
        let resources = Resources::new(Limits::default()).unwrap();
        let data = ManagedDataset::open(
            &resources,
            Path::new(case["source"].as_str().unwrap()),
            None,
            Mode::Level,
            &Arc::new(AtomicUsize::new(0)),
        )
        .unwrap();
        let model = Model::new(&data).unwrap();
        let state = ViewState::initial(&model, 257, 191).unwrap();
        let visible = match &state.layers {
            Layers::All => model.styles.iter().map(|s| s.layer).collect(),
            Layers::None => vec![],
            Layers::Only(p) => p.clone(),
        };
        assert_eq!(json!(visible), case["visible"]);
        assert_eq!(
            state
                .edit(
                    &model,
                    Patch {
                        layers: Some(Layers::All),
                        ..Default::default()
                    }
                )
                .unwrap()
                .layers,
            Layers::All
        );
    }
    println!("LAYERPROPS: ALL OK (72 documents, 980 styles, 4 native view models)");
}
