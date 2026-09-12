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

#[test]
#[ignore = "run tools/validate_view_controller.py with a private valmini fixture"]
fn native_margin_pixels_match_direct_views_without_geometry_changes() {
    use floe_app_core::view::{margin, ControllerOptions};
    let source = PathBuf::from(std::env::var_os("FLOE_VIEW_FIXTURE").unwrap());
    for pattern in [false, true] {
        let resources = Resources::new(Limits::default()).unwrap();
        let stop = Arc::new(AtomicUsize::new(0));
        let data = ManagedDataset::open(&resources, &source, None, Mode::Level, &stop).unwrap();
        let model = Model::new(&data).unwrap();
        let mut options = RenderOptions::local().unwrap();
        options.decode_jobs = 1;
        options.raster_jobs = 1;
        options.budget_mb = 64;
        options.raw = true;
        let mut initial = ViewState::initial(&model, 257, 191).unwrap();
        initial.labels = false;
        initial.detail = Detail::High;
        initial.viewport.bbox[0] += 0.0625;
        initial.viewport.bbox[2] += 0.0625;
        initial.viewport.bbox[1] -= 0.0625;
        initial.viewport.bbox[3] -= 0.0625;
        if pattern {
            initial.styles = Arc::new(
                initial
                    .styles
                    .iter()
                    .cloned()
                    .map(|mut s| {
                        s.fill = Fill::Pattern([0xa55a; 16]);
                        s
                    })
                    .collect(),
            );
        }
        let mut reference =
            RenderSession::open(&data.dataset, options.clone(), false, Arc::clone(&stop)).unwrap();
        reference.set_styles(&initial.styles).unwrap();
        let mut v = ViewController::start_configured(
            &resources,
            Arc::clone(&data),
            options,
            initial.clone(),
            ControllerOptions {
                margin_prefetch: true,
                frame_cache: true,
            },
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let retained = loop {
            assert!(Instant::now() < deadline, "{:?}", v.snapshot().failure);
            if let Some(f) = v.margin() {
                break f;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        assert!(retained.frame.complete());
        assert_eq!(retained.frame.generation, 2);
        assert!(retained.frame.bytes[16..]
            .chunks_exact(4)
            .any(|c| c[0] > 0 || c[1] > 0 || c[2] > 0));
        for (x, y) in [
            (0., 0.),
            (0.1, 0.),
            (-0.1, 0.),
            (0., 0.1),
            (0., -0.1),
            (0.5, 0.),
            (-0.5, 0.),
            (0., 0.5),
            (0., -0.5),
        ] {
            let s = initial
                .edit(
                    &model,
                    Patch {
                        navigation: Some(Navigation::Pan { x, y, snap: true }),
                        ..Default::default()
                    },
                )
                .unwrap();
            let expected = reference
                .capture(s.request(&model, reference.base_request()))
                .unwrap();
            assert!(expected.complete());
            let [ox, oy] = margin::origin(retained.viewport(), s.viewport).unwrap();
            assert!(margin::covers(retained.viewport(), s.viewport));
            for row in 0..s.viewport.height as usize {
                let src = 16
                    + ((row + oy as usize) * retained.frame.request.width as usize + ox as usize)
                        * 4;
                let dst = 16 + row * s.viewport.width as usize * 4;
                let n = s.viewport.width as usize * 4;
                assert_eq!(
                    &retained.frame.bytes[src..src + n],
                    &expected.bytes[dst..dst + n],
                    "margin/direct mismatch pattern={pattern} pan={x},{y} row={row}"
                );
            }
        }
        let accepted = v
            .edit(
                1,
                Patch {
                    navigation: Some(Navigation::Pan {
                        x: 0.1,
                        y: 0.,
                        snap: true,
                    }),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(accepted.margin.unwrap().crop_safe);
        let deadline = Instant::now() + Duration::from_secs(3);
        while v.snapshot().crop_hits != 1 {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(v.snapshot().submitted - v.snapshot().margin_submitted, 1);
        reference.close().unwrap();
        v.close().unwrap();
        drop(data);
        assert_eq!(resources.usage(), Usage::default());
    }
    println!("RUST VIEW MARGIN: ALL OK (18 raw crop/direct pixel pairs, half phase, speckle/pattern, no foreground pan)");
}
