use floe_app_core::{
    dataset::Dataset,
    jobdeck::color::Mode,
    managed::{Limits, ManagedDataset, Resources},
    render::{require_complete, RenderOptions, RenderSession},
    shots::Shot,
    styles,
    view::{Model, Phase, ViewController, ViewState, Viewport},
};
use floe_worker_client::{Fill, Layers};
use serde_json::{json, Value};
use std::{
    path::Path,
    sync::{atomic::AtomicUsize, Arc},
    time::{Duration, Instant},
};

fn fill_text(fill: &Fill) -> String {
    match fill {
        Fill::Solid => "solid".into(),
        Fill::Clear => "clear".into(),
        Fill::Speckle => "speckle".into(),
        Fill::Pattern(p) if p.iter().all(|&n| n == 0xffff) => "solid".into(),
        Fill::Pattern(p) if p.iter().all(|&n| n == 0) => "clear".into(),
        Fill::Pattern(p)
            if p.iter()
                .enumerate()
                .all(|(i, &n)| n == if i % 2 == 0 { 0xaaaa } else { 0x5555 }) =>
        {
            "speckle".into()
        }
        Fill::Pattern(p) => format!(
            "pat:{}",
            p.iter().map(|v| format!("{v:04X}")).collect::<String>()
        ),
    }
}

#[test]
#[ignore = "run tools/validate_app_deck_render.py for generated dataset/PNG fixtures"]
fn snapshots_styles_and_native_frames_match_python() {
    let cases: Vec<Value> = serde_json::from_slice(
        &std::fs::read(std::env::var_os("FLOE_APP_DATASET_ORACLE").expect("oracle required"))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(cases.len(), 6);
    let flag = Arc::new(AtomicUsize::new(0));
    for case in &cases {
        let dataset = Dataset::open(
            Path::new(case["source"].as_str().unwrap()),
            serde_json::from_value(case["levels"].clone()).unwrap(),
            Mode::parse(case["mode"].as_str().unwrap()).unwrap(),
            &flag,
        )
        .unwrap();
        assert_eq!(
            dataset.info()["metadata"],
            case["metadata"],
            "{}",
            case["mode"]
        );
        let actual: Vec<_> = dataset.styles(false).unwrap().iter().map(|s| {
            json!({"layer":s.layer,"color":styles::color_text(s.color),"fill":fill_text(&s.fill),"width":s.width})
        }).collect();
        assert_eq!(json!(actual), case["styles"], "styles {}", case["mode"]);
        for s in dataset.styles(true).unwrap() {
            assert!(matches!(s.fill, Fill::Solid));
            assert_eq!(s.width, 1);
        }
        let mut session = RenderSession::open(
            &dataset,
            RenderOptions::local().unwrap(),
            true,
            Arc::clone(&flag),
        )
        .unwrap();
        let shot = Shot {
            bbox: Some(serde_json::from_value(case["bbox"].clone()).unwrap()),
            pixels: (103, Some(91)),
            stretch: true,
            ..Default::default()
        };
        let req = session.shot_request(&dataset, &shot).unwrap();
        let frame = session.capture(req).unwrap();
        require_complete(&frame).unwrap();
        assert_eq!(
            frame.bytes,
            std::fs::read(case["png"].as_str().unwrap()).unwrap(),
            "PNG {}",
            case["mode"]
        );
        session.close().unwrap();
        let resources = Resources::new(Limits::default()).unwrap();
        let managed = ManagedDataset::open(
            &resources,
            Path::new(case["source"].as_str().unwrap()),
            serde_json::from_value(case["levels"].clone()).unwrap(),
            Mode::parse(case["mode"].as_str().unwrap()).unwrap(),
            &flag,
        )
        .unwrap();
        let model = Model::new(&managed).unwrap();
        let mut live = ViewState::initial(&model, 103, 91).unwrap();
        assert_eq!(case["live"].as_array().unwrap().len(), 16);
        for step in case["live"].as_array().unwrap() {
            live = live
                .edit(
                    &model,
                    floe_app_core::view::Patch {
                        properties: Some(
                            floe_app_core::layerprops::parse(step["text"].as_str().unwrap())
                                .unwrap(),
                        ),
                        ..Default::default()
                    },
                )
                .unwrap();
            let styles:Vec<_> = live.styles.iter().map(|s|json!({"layer":s.layer,"color":styles::color_text(s.color),"fill":floe_app_core::layerprops::Row::from_style(s,"",true).unwrap().fill,"width":s.width})).collect();
            assert_eq!(
                json!(styles),
                step["styles"],
                "GTK live styles {}",
                case["mode"]
            );
            let visible: Vec<_> = match &live.layers {
                Layers::All => live
                    .styles
                    .iter()
                    .filter(|s| {
                        let Dataset::Deck(d) = &managed.dataset else {
                            panic!("deck")
                        };
                        !d.metadata.layers.iter().any(|r| {
                            r.jobdeck_head && (r.layer as u32, r.datatype as u32) == s.layer
                        })
                    })
                    .map(|s| s.layer)
                    .collect(),
                Layers::None => vec![],
                Layers::Only(p) => p.clone(),
            };
            assert_eq!(
                json!(visible),
                step["visible"],
                "GTK live visibility {}",
                case["mode"]
            );
        }
        let mut state = ViewState::initial(&model, 103, 91).unwrap();
        let selected = match &state.layers {
            Layers::All => {
                let floe_app_core::dataset::Dataset::Deck(d) = &managed.dataset else {
                    panic!("deck required");
                };
                model
                    .styles
                    .iter()
                    .filter(|s| {
                        !d.metadata.layers.iter().any(|r| {
                            r.jobdeck_head && (r.layer as u32, r.datatype as u32) == s.layer
                        })
                    })
                    .map(|s| s.layer)
                    .collect()
            }
            Layers::None => vec![],
            Layers::Only(p) => p.clone(),
        };
        assert_eq!(
            json!(selected),
            case["visible"],
            "GTK default visibility {}",
            case["mode"]
        );
        let bounds: [f64; 4] = serde_json::from_value(case["bbox"].clone()).unwrap();
        state.viewport = Viewport::new(bounds.map(|n| n / model.dbu), 103, 91).unwrap();
        state.detail = floe_app_core::shots::Detail::Exact;
        state.styles = Arc::new(managed.dataset.styles(true).unwrap());
        let mut options = RenderOptions::local().unwrap();
        options.decode_jobs = 2;
        options.raster_jobs = 2;
        options.raw = false;
        let mut controller = ViewController::start(&resources, managed, options, state).unwrap();
        for (pass, path) in ["view_png", "png"].into_iter().enumerate() {
            let expected_rev = if pass == 0 {
                1
            } else {
                controller
                    .edit(
                        controller.snapshot().state_rev,
                        floe_app_core::view::Patch {
                            layers: Some(Layers::All),
                            ..Default::default()
                        },
                    )
                    .unwrap()
                    .render_rev
            };
            let deadline = Instant::now() + Duration::from_secs(10);
            let frame = loop {
                let snapshot = controller.snapshot();
                assert_ne!(snapshot.phase, Phase::Failed, "{:?}", snapshot.failure);
                if let Some(f) = controller
                    .latest()
                    .filter(|f| f.frame.final_frame && f.render_rev == expected_rev)
                {
                    break f;
                }
                assert!(Instant::now() < deadline, "deck controller deadline");
                std::thread::sleep(Duration::from_millis(2));
            };
            assert_eq!(
                frame.frame.bytes,
                std::fs::read(case[path].as_str().unwrap()).unwrap(),
                "managed deck PNG {}",
                case["mode"]
            );
            assert_eq!(frame.render_rev, expected_rev);
        }
        controller.close().unwrap();
    }
    println!("RUST APP DECK DATASET: ALL OK (6 cases) + 6 managed controllers");
}
