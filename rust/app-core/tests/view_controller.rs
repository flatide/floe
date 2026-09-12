//! Invoked by validate_view_controller.py with a private synthetic cache.
use floe_app_core::{
    cache,
    jobdeck::color::Mode,
    managed::{Limits, ManagedDataset, Resources, Usage},
    render::{RenderOptions, RenderSession},
    shots::{Detail, Thin},
    view::{Depth, Model, Navigation, Patch, Phase, ViewController, ViewState},
};
use floe_worker_client::{Fill, Layers, Style};
use std::{
    path::PathBuf,
    sync::{atomic::AtomicUsize, Arc},
    time::{Duration, Instant},
};

#[test]
#[ignore = "run tools/validate_view_controller.py with a private valmini fixture"]
fn actual_worker_frames_and_leases() {
    let source =
        PathBuf::from(std::env::var_os("FLOE_VIEW_FIXTURE").expect("private fixture required"));
    let resource = Resources::new(Limits::default()).unwrap();
    let cancelled = Arc::new(AtomicUsize::new(0));
    let data = ManagedDataset::open(&resource, &source, None, Mode::Level, &cancelled).unwrap();
    let model = Model::new(&data).unwrap();
    let mut options = RenderOptions::local().unwrap();
    options.decode_jobs = 2;
    options.raster_jobs = 2;
    options.raw = false;
    options.budget_mb = 64;
    options.open_timeout_s = 10;
    let initial = ViewState::initial(&model, 257, 191)
        .unwrap()
        .edit(
            &model,
            Patch {
                detail: Some(Detail::High),
                labels: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
    let mut reference = RenderSession::open(
        &data.dataset,
        options.clone(),
        false,
        Arc::clone(&cancelled),
    )
    .unwrap();
    let mut view =
        ViewController::start(&resource, Arc::clone(&data), options.clone(), initial).unwrap();
    let first = model.styles[0].clone();
    let patches = vec![
        Patch::default(),
        Patch {
            navigation: Some(Navigation::Pan {
                x: 0.5,
                y: 0.,
                snap: true,
            }),
            ..Default::default()
        },
        Patch {
            navigation: Some(Navigation::Pan {
                x: -0.5,
                y: 0.1,
                snap: true,
            }),
            ..Default::default()
        },
        Patch {
            navigation: Some(Navigation::Zoom {
                factor: 0.8,
                anchor: [0.5, 0.5],
            }),
            ..Default::default()
        },
        Patch {
            navigation: Some(Navigation::Fit),
            pixels: Some((273, 203)),
            ..Default::default()
        },
        Patch {
            depth: Some(Depth::Levels(0)),
            frames: Some(true),
            ..Default::default()
        },
        Patch {
            depth: Some(Depth::Levels(1)),
            ..Default::default()
        },
        Patch {
            depth: Some(Depth::Full),
            thin: Some(Thin::Keep),
            labels: Some(true),
            ..Default::default()
        },
        Patch {
            thin: Some(Thin::Cull),
            detail: Some(Detail::Medium),
            ..Default::default()
        },
        Patch {
            layers: Some(Layers::None),
            ..Default::default()
        },
        Patch {
            layers: Some(Layers::Only(vec![first.layer])),
            ..Default::default()
        },
        Patch {
            style_changes: vec![Style {
                fill: Fill::Clear,
                width: 4,
                ..first.clone()
            }],
            ..Default::default()
        },
        Patch {
            style_changes: vec![Style {
                fill: Fill::Pattern([0xaaaa; 16]),
                width: 1,
                ..first
            }],
            layers: Some(Layers::All),
            mono: Some(true),
            ..Default::default()
        },
    ];
    for (i, patch) in patches.into_iter().enumerate() {
        let before = view.snapshot();
        let snapshot = view.edit(before.state_rev, patch).unwrap();
        reference.set_styles(&snapshot.state.styles).unwrap();
        let want = reference
            .capture(snapshot.state.request(&model, reference.base_request()))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let actual = loop {
            let s = view.snapshot();
            assert_ne!(s.phase, Phase::Failed, "{:?}", s.failure);
            if let Some(frame) = view
                .latest()
                .filter(|f| f.render_rev == snapshot.render_rev && f.frame.final_frame)
            {
                break frame;
            }
            assert!(Instant::now() < deadline, "frame {i} deadline {s:?}");
            std::thread::sleep(Duration::from_millis(2));
        };
        assert_eq!(actual.frame.bytes, want.bytes, "native PNG case {i}");
        assert_eq!(actual.frame.complete(), want.complete());
        assert_eq!(actual.frame.request.view, want.request.view);
        assert_eq!(actual.render_key, snapshot.render_key);
        assert_eq!(actual.dataset_revision, data.revision);
    }
    assert!(resource
        .index([cache::cache_path(&source).unwrap()], 4)
        .is_err());
    // A second immutable open gets another revision while both readers pin it.
    let another = ManagedDataset::open(&resource, &source, None, Mode::Level, &cancelled).unwrap();
    assert_ne!(data.revision, another.revision);
    drop(another);
    reference.close().unwrap();
    drop(data);
    view.close().unwrap();
    assert_eq!(resource.usage(), Usage::default());
    drop(
        resource
            .index([cache::cache_path(&source).unwrap()], 4)
            .unwrap(),
    );
    // Failed daemon startup releases both reservation and consumed dataset lease.
    let failed_data =
        ManagedDataset::open(&resource, &source, None, Mode::Level, &cancelled).unwrap();
    let m = Model::new(&failed_data).unwrap();
    options.binary = source.with_extension("missing-renderd");
    let mut failed = ViewController::start(
        &resource,
        failed_data,
        options,
        ViewState::initial(&m, 80, 64).unwrap(),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while failed.snapshot().phase != Phase::Failed {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    failed.close().unwrap();
    assert_eq!(resource.usage(), Usage::default());
    drop(
        resource
            .index([cache::cache_path(&source).unwrap()], 4)
            .unwrap(),
    );
    println!("RUST VIEW CONTROLLER: ALL OK (13 PNG pairs, leases, startup failure)");
}
