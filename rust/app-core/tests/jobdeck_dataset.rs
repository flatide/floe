use floe_app_core::{
    dataset::Dataset,
    jobdeck::color::Mode,
    managed::{Limits, ManagedDataset, Resources},
    render::{require_complete, RenderOptions, RenderSession},
    shots::{Detail, Shot, Thin},
    styles,
    view::{
        deck_mode::DeckModeMemory, LayerIsolation, Model, Patch, Phase, StyleDelta, ViewController,
        ViewState, Viewport,
    },
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

fn selected_leaves(state: &ViewState, data: &ManagedDataset) -> Vec<(u32, u32)> {
    match &state.layers {
        Layers::All => {
            let Dataset::Deck(d) = &data.dataset else {
                panic!("deck required")
            };
            state
                .styles
                .iter()
                .filter(|s| {
                    !d.metadata
                        .layers
                        .iter()
                        .any(|r| r.jobdeck_head && (r.layer as u32, r.datatype as u32) == s.layer)
                })
                .map(|s| s.layer)
                .collect()
        }
        Layers::None => vec![],
        Layers::Only(p) => p.clone(),
    }
}

#[test]
#[ignore = "run tools/validate_app_deck_render.py for GTK mode/PNG oracle"]
fn deck_mode_preparation_matches_gtk_and_native_frames() {
    let cases: Vec<Value> = serde_json::from_slice(
        &std::fs::read(std::env::var_os("FLOE_APP_MODE_ORACLE").expect("mode oracle required"))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(cases.len(), 2);
    let flag = Arc::new(AtomicUsize::new(0));
    let resources = Resources::new(Limits {
        cpu_slots: 4,
        foreground_reserve: 0,
        workers: 1,
        decoded_mb: 1024,
    })
    .unwrap();
    let mut count = 0;
    for case in &cases {
        let open = |mode, levels| {
            ManagedDataset::open(
                &resources,
                Path::new(case["source"].as_str().unwrap()),
                levels,
                mode,
                &flag,
            )
            .unwrap()
        };
        let levels = serde_json::from_value(case["levels"].clone()).unwrap();
        let mut current = open(Mode::Chip, levels);
        let mut model = Model::new(&current).unwrap();
        let mut state = ViewState::initial(&model, 103, 91).unwrap();
        let mut memory = DeckModeMemory::default();
        let steps = case["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 12);
        let bounds: [f64; 4] = serde_json::from_value(steps[0]["bbox"].clone()).unwrap();
        state.viewport = Viewport::new(bounds.map(|n| n / model.dbu), 103, 91).unwrap();
        state.depth = Some(3);
        state.detail = Detail::High;
        state.thin = Thin::Keep;
        state.frames = true;
        state.font_px = 20;
        state.mono = true;
        let mut initial_shot = state.clone();
        initial_shot.detail = Detail::Exact;
        initial_shot.depth = None;
        initial_shot.frames = false;
        initial_shot.mono = false;
        initial_shot.styles = Arc::new(current.dataset.styles(true).unwrap());
        let mut options = RenderOptions::local().unwrap();
        options.decode_jobs = 2;
        options.raster_jobs = 2;
        options.raw = false;
        let mut controller = Arc::new(
            ViewController::start(&resources, Arc::clone(&current), options, initial_shot).unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while controller.latest().is_none() {
            assert_ne!(controller.snapshot().phase, Phase::Failed);
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        for step in steps {
            let before: Vec<(u32, u32)> = serde_json::from_value(step["before"].clone()).unwrap();
            let layers = if before.is_empty() {
                Layers::None
            } else {
                Layers::Only(before)
            };
            // Empty visibility is a checkbox operation, not a valid isolate.
            // Other cases exercise dropping the old namespace's restore handle.
            let patch = if layers == Layers::None {
                Patch {
                    layers: Some(layers),
                    ..Default::default()
                }
            } else {
                Patch {
                    layer_isolation: Some(LayerIsolation::Set(layers)),
                    ..Default::default()
                }
            };
            state = state.edit(&model, patch).unwrap();
            state = state
                .edit(
                    &model,
                    Patch {
                        style_deltas: vec![StyleDelta {
                            layer: model.styles[0].layer,
                            color: Some([11, 22, 33, 255]),
                            width: Some(7),
                            ..Default::default()
                        }],
                        ..Default::default()
                    },
                )
                .unwrap();
            let target = open(
                Mode::parse(step["mode"].as_str().unwrap()).unwrap(),
                serde_json::from_value(case["levels"].clone()).unwrap(),
            );
            let old_memory = memory.clone();
            let old_state = state.clone();
            let usage = resources.usage();
            assert!(memory.prepare(&current, &model, &state, &current).is_err());
            let mut invalid = state.clone();
            invalid.viewport.bbox[0] = f64::NAN;
            assert!(memory.prepare(&current, &model, &invalid, &target).is_err());
            let wrong_model = Model::new(&target).unwrap();
            assert!(memory
                .prepare(&current, &wrong_model, &state, &target)
                .is_err());
            let next = memory.prepare(&current, &model, &state, &target).unwrap();
            assert_eq!(memory, old_memory, "preparation must not commit memory");
            assert_eq!(
                state, old_state,
                "preparation must not edit the current view"
            );
            assert_eq!(
                resources.usage(),
                usage,
                "preparation must not start a worker"
            );
            assert_eq!(next.model.dataset_revision, target.revision);
            assert_eq!(next.state.viewport, state.viewport, "camera must not refit");
            assert_eq!(next.state.depth, state.depth);
            assert_eq!(next.state.detail, state.detail);
            assert_eq!(next.state.thin, state.thin);
            assert_eq!(next.state.frames, state.frames);
            assert_eq!(next.state.labels, state.labels);
            assert_eq!(next.state.font_px, state.font_px);
            assert_eq!(next.state.mono, state.mono);
            assert!(!next.state.layers_isolated());
            assert_eq!(*next.state.styles, target.dataset.styles(false).unwrap());
            assert_eq!(
                json!(selected_leaves(&next.state, &target)),
                step["visible"],
                "GTK mode {}",
                step["mode"]
            );
            let expected_box: [f64; 4] = serde_json::from_value(step["bbox"].clone()).unwrap();
            assert_eq!(
                next.state.viewport.bbox,
                expected_box.map(|n| n / next.model.dbu)
            );

            // Archival style/depth for the independent Python ShotRunner
            // oracle. The mode-prepared visibility and off-center camera stay.
            let mut shot = next.state.clone();
            shot.detail = Detail::Exact;
            shot.depth = None;
            shot.frames = false;
            shot.mono = false;
            shot.styles = Arc::new(target.dataset.styles(true).unwrap());
            let mut replacement = controller
                .prepare_replacement(Arc::clone(&target), shot)
                .unwrap();
            let next_controller = replacement.controller();
            assert_eq!(resources.usage(), usage);
            replacement.commit(controller.snapshot().state_rev).unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            let frame = loop {
                let snapshot = next_controller.snapshot();
                assert_ne!(snapshot.phase, Phase::Failed, "{:?}", snapshot.failure);
                if let Some(frame) = next_controller.latest().filter(|f| f.frame.final_frame) {
                    break frame;
                }
                assert!(Instant::now() < deadline, "deck mode frame deadline");
                std::thread::sleep(Duration::from_millis(2));
            };
            assert_eq!(
                frame.frame.bytes,
                std::fs::read(step["png"].as_str().unwrap()).unwrap(),
                "mode PNG {}",
                step["mode"]
            );
            assert!(controller.is_finished(), "native workers overlapped");
            assert_ne!(
                controller.snapshot().worker_epoch,
                next_controller.snapshot().worker_epoch
            );
            controller = next_controller;
            assert_eq!(resources.usage(), usage);
            memory = next.memory;
            model = next.model;
            state = next.state;
            current = target;
            count += 1;
        }
        controller.request_close();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !controller.is_finished() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(resources.usage(), floe_app_core::managed::Usage::default());
        let other = if case["levels"].is_null() {
            Some([2, 3].into_iter().collect())
        } else {
            None
        };
        let other_selection = open(Mode::Chip, other);
        assert!(
            memory
                .prepare(&current, &model, &state, &other_selection)
                .is_err(),
            "cannot carry visibility into a different level selection"
        );
        let other_model = Model::new(&other_selection).unwrap();
        let other_state = ViewState::initial(&other_model, 103, 91).unwrap();
        let other = if case["levels"].is_null() {
            Some([2, 3].into_iter().collect())
        } else {
            None
        };
        let other_target = open(Mode::Level, other);
        assert!(
            memory
                .prepare(&other_selection, &other_model, &other_state, &other_target)
                .is_err(),
            "saved memory cannot be reused by another level selection"
        );
    }
    assert_eq!(count, 24);
    println!("RUST DECK MODE PREPARATION: ALL OK (24 GTK transitions + native PNGs)");
    println!("RUST DECK MODE HANDOFF: ALL OK (24 native transitions, one worker reservation)");
}
