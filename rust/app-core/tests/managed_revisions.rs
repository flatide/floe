//! Actual-native synthetic set builds and headless reads; no browser/field data.
use floe_app_core::{
    cache::{
        self,
        revision::{self, set},
    },
    dataset::Dataset,
    index::IndexOptions,
    jobdeck::color::Mode,
    managed::{Limits, ManagedDataset, Resources, Usage},
    managed_index::{ManagedIndex, Phase, Snapshot},
    native::{Discovery, Indexer},
    registered::{AccessScope, RegisteredSource},
    render::{RenderOptions, RenderSession},
};
use floe_oasis::{
    doc::{RectRec, Rep},
    write::write_cell,
};
use std::{
    collections::BTreeSet,
    fs,
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{atomic::AtomicUsize, Arc},
    time::{Duration, Instant},
};

struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let mut bytes = [0; 16];
        getrandom::fill(&mut bytes).unwrap();
        let path = std::env::temp_dir().join(format!(
            "floe-managed-revisions-{:x}",
            u128::from_ne_bytes(bytes)
        ));
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        Self(path)
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn indexer(path: PathBuf) -> Indexer {
    Indexer::discover(&Discovery {
        override_path: Some(path),
        development_root: None,
        executable: std::env::current_exe().unwrap(),
        search_path: None,
    })
    .unwrap()
}
fn wait(job: &mut ManagedIndex) -> Snapshot {
    let deadline = Instant::now() + Duration::from_secs(90);
    while !job.is_finished() {
        assert!(Instant::now() < deadline, "{:?}", job.snapshot());
        std::thread::sleep(Duration::from_millis(5));
    }
    job.close().unwrap();
    job.snapshot()
}
fn register(scope: &Arc<AccessScope>, path: &Path) -> Arc<RegisteredSource> {
    RegisteredSource::register(Arc::clone(scope), path, &AtomicUsize::new(0)).unwrap()
}
fn build(
    resources: &Arc<Resources>,
    source: &Arc<RegisteredSource>,
    binary: &Indexer,
    levels: Option<BTreeSet<i64>>,
) -> Snapshot {
    let mut job = ManagedIndex::start_revisions(
        resources,
        Arc::clone(source),
        levels,
        IndexOptions {
            jobs: 1,
            ..Default::default()
        },
        binary.clone(),
    )
    .unwrap();
    wait(&mut job)
}
fn render(data: &ManagedDataset, binary: &Path) -> Vec<u8> {
    let mut worker = RenderSession::open(
        &data.dataset,
        RenderOptions {
            binary: binary.with_file_name("floe-renderd"),
            budget_mb: 64,
            decode_jobs: 1,
            raster_jobs: 1,
            tile_px: 128,
            round_pages: 1024,
            open_timeout_s: 60,
            label_font_px: 14,
            raw: false,
            debug: false,
        },
        true,
        Arc::new(AtomicUsize::new(0)),
    )
    .unwrap();
    let mut request = worker.base_request();
    request.width = 96;
    request.height = 64;
    request.depth = None;
    request.cut_px = 0.;
    request.view = data.dataset.bbox().map(|v| v as f64);
    let frame = worker.capture(request).unwrap();
    assert!(frame.complete());
    worker.close().unwrap();
    frame.bytes
}

#[test]
#[ignore = "requires release FLOE_INDEX_BIN and sibling renderd; cache_revision gate"]
fn layout_and_deck_sets_pin_geometry_and_failure_never_advances_current() {
    let root = Root::new();
    let stop = AtomicUsize::new(0);
    let binary = indexer(PathBuf::from(std::env::var_os("FLOE_INDEX_BIN").unwrap()));
    let scope = AccessScope::new(std::slice::from_ref(&root.0)).unwrap();
    let resources = Resources::new(Limits::default()).unwrap();
    let mut sources = BTreeSet::new();
    for (name, width) in [("a.oas", 40000), ("b.oas", 80000)] {
        let path = root.0.join(name);
        fs::write(
            &path,
            write_cell(
                "TOP",
                1000.,
                &mut [RectRec {
                    layer: 7,
                    dt: 0,
                    x: 0,
                    y: 0,
                    w: width,
                    h: 20000,
                    rep: Rep::One,
                }],
                &mut [],
            )
            .unwrap(),
        )
        .unwrap();
        sources.insert(path);
    }
    let layout = register(&scope, &root.0.join("a.oas"));
    assert!(ManagedDataset::open_revisions(&resources, &layout, None, Mode::Level, &stop).is_err());
    assert!(!set::Store::new(layout.path()).unwrap().path().exists());
    let result = build(&resources, &layout, &binary, None);
    assert_eq!(result.phase, Phase::Succeeded);
    assert!(!result.revision_sync_warning);
    let first_layout =
        ManagedDataset::open_revisions(&resources, &layout, None, Mode::Level, &stop).unwrap();
    assert_eq!(first_layout.index_revision, result.index_revision);
    let layout_pixels = render(&first_layout, binary.path());
    assert_eq!(
        build(&resources, &layout, &binary, None).phase,
        Phase::Succeeded
    );
    let next_layout =
        ManagedDataset::open_revisions(&resources, &layout, None, Mode::Level, &stop).unwrap();
    assert_ne!(first_layout.index_revision, next_layout.index_revision);
    assert_eq!(layout_pixels, render(&first_layout, binary.path()));
    assert_eq!(layout_pixels, render(&next_layout, binary.path()));
    assert!(!cache::default_cache_path(layout.path()).unwrap().exists());
    assert!(revision::Store::new(layout.path())
        .unwrap()
        .pin()
        .unwrap()
        .is_none());

    // No RenderSession wrapper: prove the separate native process, not a
    // surviving host Snapshot/Layout, owns the last reader lease.
    let old_id = first_layout.index_revision.clone().unwrap();
    let old_dir = match &first_layout.dataset {
        Dataset::Layout(l) => l.directory.clone(),
        _ => unreachable!(),
    };
    let mut native = floe_worker_client::WorkerClient::spawn(floe_worker_client::Config::new(
        binary.path().with_file_name("floe-renderd"),
    ))
    .unwrap();
    native
        .open(floe_worker_client::Source::Layout(old_dir.clone()), 32, 1)
        .unwrap();
    assert!(revision::reclaim::prepare(Arc::clone(&layout), &old_id, &stop).is_err());
    drop(first_layout);
    assert_eq!(
        revision::reclaim::prepare(Arc::clone(&layout), &old_id, &stop)
            .err()
            .unwrap()
            .kind,
        floe_app_core::ErrorKind::Busy
    );
    let probe = fs::File::open(&old_dir).unwrap();
    assert!(matches!(
        probe.try_lock(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    native.close().unwrap();
    probe.try_lock().unwrap();
    drop(probe);
    let preview = revision::reclaim::prepare(Arc::clone(&layout), &old_id, &stop).unwrap();
    assert!(!preview.summary().complete);
    assert_eq!(preview.execute(&stop).unwrap().status, "complete");
    assert!(!old_dir.exists());
    assert_eq!(layout_pixels, render(&next_layout, binary.path()));

    let deck_path = root.0.join("synthetic.jb");
    fs::write(&deck_path, "CHIP C\n$ (1,A,TC=a.oas,AD=0.001,LY={7},DT={0},UX=100,UY=100)\n$ (2,B,TC=b.oas,AD=0.001,LY={7},DT={0},UX=100,UY=100)\nROWS 0/0\n").unwrap();
    let deck = register(&scope, &deck_path);
    let first = build(&resources, &deck, &binary, None);
    assert_eq!(first.phase, Phase::Succeeded, "{first:?}");
    let store = set::Store::new(&deck_path).unwrap();
    let a = ManagedDataset::open_revisions(&resources, &deck, None, Mode::Level, &stop).unwrap();
    let old_pixels = render(&a, binary.path());
    let old_set = store.pin(&sources, &None, &stop).unwrap().unwrap();
    assert!(old_set
        .members()
        .values()
        .all(|p| p.directory().join("design.ovo").is_file()));
    let second = build(&resources, &deck, &binary, None);
    assert_eq!(second.phase, Phase::Succeeded);
    assert_ne!(first.index_revision, second.index_revision);
    let b = ManagedDataset::open_revisions(&resources, &deck, None, Mode::Level, &stop).unwrap();
    let new_set = store.pin(&sources, &None, &stop).unwrap().unwrap();
    for (path, old) in old_set.members() {
        assert_ne!(old.id(), new_set.members()[path].id());
    }
    old_set.validate(&stop).unwrap();
    assert_eq!(a.index_revision, first.index_revision);
    assert_eq!(b.index_revision, second.index_revision);
    assert_eq!(old_pixels, render(&a, binary.path()));
    assert_eq!(old_pixels, render(&b, binary.path()));
    let chip = a.reopen_mode(&resources, Mode::Chip, &stop).unwrap();
    assert_eq!(chip.index_revision, first.index_revision);
    assert_ne!(chip.revision, a.revision);
    if let Dataset::Deck(d) = &chip.dataset {
        for info in d.analysis.catalog.infos.values() {
            assert_eq!(info.cache_dir, old_set.members()[&info.path].directory());
        }
    } else {
        panic!("expected pinned deck");
    }
    assert!(a
        .dataset
        .output_path(&store.path().join("current.json"))
        .is_err());
    assert!(a
        .dataset
        .output_path(
            &set::Store::new(layout.path())
                .unwrap()
                .path()
                .join("current.json")
        )
        .is_err());
    assert!(ManagedDataset::open_revisions(
        &resources,
        &deck,
        Some([1].into()),
        Mode::Level,
        &stop
    )
    .is_err());

    // Verified wrapper fails only the second synthetic source, after the first
    // candidate was sealed. Never install a partly rebuilt deck/source current.
    let wrapper = root.0.join("fail-second-indexer");
    let quote = |s: &Path| format!("'{}'", s.to_str().unwrap().replace('\'', "'\"'\"'"));
    fs::write(&wrapper, format!("#!/bin/sh\nif [ \"${{1-}}\" = vfs ] && [ \"${{2##*/}}\" = b.oas ]; then exit 23; fi\nexec {} \"$@\"\n", quote(binary.path()))).unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
    let failed = build(&resources, &deck, &indexer(wrapper), None);
    assert_eq!(failed.phase, Phase::Failed);
    assert_eq!(failed.completed, 2);
    assert_eq!(failed.failed, 1);
    assert!(failed.index_revision.is_none());
    assert_eq!(
        store.pin(&sources, &None, &stop).unwrap().unwrap().id(),
        new_set.id()
    );
    for source in &sources {
        assert!(revision::Store::new(source)
            .unwrap()
            .pin()
            .unwrap()
            .is_none());
    }
    assert_eq!(old_pixels, render(&a, binary.path()));

    let mut cancelled = ManagedIndex::start_revisions(
        &resources,
        Arc::clone(&deck),
        None,
        IndexOptions {
            jobs: 1,
            ..Default::default()
        },
        binary.clone(),
    )
    .unwrap();
    cancelled.cancel();
    assert_eq!(wait(&mut cancelled).phase, Phase::Cancelled);
    assert_eq!(
        store.pin(&sources, &None, &stop).unwrap().unwrap().id(),
        new_set.id()
    );
    assert_eq!(resources.usage(), Usage::default());

    // Only registered expected paths authorize member resolution. A manifest
    // cannot redirect reads to a different source or turn into a file picker.
    let current = store.path().join("current.json");
    let original = fs::read(&current).unwrap();
    let seal = store.path().join(new_set.id()).join("revision.json");
    let mut value: serde_json::Value = serde_json::from_slice(&original).unwrap();
    value["members"][0]["source"] = serde_json::json!(root.0.join("not-authorized.oas"));
    let altered = serde_json::to_vec(&value).unwrap();
    fs::write(&current, &altered).unwrap();
    fs::write(&seal, altered).unwrap();
    let error = store.pin(&sources, &None, &stop).unwrap_err();
    assert_eq!(
        error.message,
        "revision set does not match the requested sources/levels"
    );
    fs::write(&current, &original).unwrap();
    fs::write(seal, original).unwrap();
    old_set.validate(&stop).unwrap();
    let selected = Some(BTreeSet::from([1]));
    let partial = build(&resources, &deck, &binary, selected.clone());
    assert_eq!(partial.phase, Phase::Succeeded);
    assert_eq!(partial.total, 1);
    let one =
        ManagedDataset::open_revisions(&resources, &deck, selected, Mode::Level, &stop).unwrap();
    assert_eq!(one.index_revision, partial.index_revision);
    if let Dataset::Deck(d) = &one.dataset {
        assert_eq!(d.analysis.catalog.infos.len(), 1);
    } else {
        panic!("expected selected deck");
    }
    assert_eq!(old_pixels, render(&a, binary.path()));
    assert!(ManagedDataset::open_revisions(&resources, &deck, None, Mode::Level, &stop).is_err());
    // A mode-only reopen must not read new placement commands with old pins.
    fs::write(
        &deck_path,
        format!(
            "{}\n! changed after pin\n",
            fs::read_to_string(&deck_path).unwrap()
        ),
    )
    .unwrap();
    assert!(a.reopen_mode(&resources, Mode::Chip, &stop).is_err());
    assert_eq!(old_pixels, render(&a, binary.path()));

    let old_id = old_set.id().to_owned();
    let old_dirs: Vec<_> = old_set.members().values().map(|p| p.directory()).collect();
    let spec = root.0.join("native-deck.spec");
    if let Dataset::Deck(d) = &a.dataset {
        fs::write(&spec, &d.spec.text).unwrap();
    } else {
        unreachable!();
    }
    let mut native = floe_worker_client::WorkerClient::spawn(floe_worker_client::Config::new(
        binary.path().with_file_name("floe-renderd"),
    ))
    .unwrap();
    native
        .open(floe_worker_client::Source::Deck(spec), 32, 1)
        .unwrap();
    drop(a);
    drop(chip);
    drop(old_set);
    let fresh_deck = register(&scope, &deck_path);
    assert_eq!(
        revision::reclaim::prepare(Arc::clone(&fresh_deck), &old_id, &stop)
            .err()
            .unwrap()
            .kind,
        floe_app_core::ErrorKind::Busy
    );
    for path in &old_dirs {
        assert!(matches!(
            fs::File::open(path).unwrap().try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
    }
    native.close().unwrap();
    let preview = revision::reclaim::prepare(Arc::clone(&fresh_deck), &old_id, &stop).unwrap();
    assert_eq!(preview.summary().sources, 2);
    assert_eq!(preview.execute(&stop).unwrap().status, "complete");
    assert!(old_dirs.iter().all(|p| !p.exists()));
    assert_eq!(old_pixels, render(&b, binary.path()));
    assert_eq!(
        revision::reclaim::prepare(fresh_deck, &old_id, &stop)
            .unwrap()
            .summary()
            .files,
        0
    );
}
