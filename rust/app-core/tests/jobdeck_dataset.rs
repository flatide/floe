use floe_app_core::{
    dataset::Dataset,
    jobdeck::color::Mode,
    render::{require_complete, RenderOptions, RenderSession},
    shots::Shot,
    styles,
};
use floe_worker_client::Fill;
use serde_json::{json, Value};
use std::{
    path::Path,
    sync::{atomic::AtomicUsize, Arc},
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
    }
    println!("RUST APP DECK DATASET: ALL OK (6 cases)");
}
