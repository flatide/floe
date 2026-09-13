//! Real exact exports and controlled native faults. Private fixtures and a
//! PATH-empty runtime are supplied by tools/validate_managed_clip.py.
use floe_app_core::{
    cache,
    clip::ClipOptions,
    exports::{
        artifacts::{self, Store},
        clip::{Job, Phase, Snapshot},
    },
    jobdeck::color::Mode,
    managed::{self, ManagedDataset, Resources},
    registered::{AccessScope, RegisteredSource},
    ErrorKind,
};
use floe_worker_client::{ClipRequest, Layers};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{atomic::AtomicUsize, Arc},
    thread,
    time::{Duration, Instant},
};

fn open(resources: &Arc<Resources>, path: &Path) -> (Arc<RegisteredSource>, Arc<ManagedDataset>) {
    let flag = AtomicUsize::new(0);
    let scope = AccessScope::new(&[path.parent().unwrap().to_owned()]).unwrap();
    let source = RegisteredSource::register(scope, path, &flag).unwrap();
    let data = ManagedDataset::open(resources, path, None, Mode::Level, &flag).unwrap();
    (source, data)
}
fn request(jobs: u16, layers: Layers) -> ClipRequest {
    ClipRequest {
        bbox: [250, 1100, 14000, 4700],
        layers,
        jobs,
        cell_name: "MANAGED_CLIP".into(),
    }
}
fn options(binary: &Path, jobs: u16) -> ClipOptions {
    ClipOptions {
        binary: binary.to_owned(),
        jobs,
        budget_mb: 64,
        open_timeout_s: 10,
        clip_timeout_s: 10,
    }
}
fn wait(job: &mut Job) -> Snapshot {
    let end = Instant::now() + Duration::from_secs(20);
    while !job.snapshot().terminal() && !job.is_finished() {
        assert!(Instant::now() < end, "{:?}", job.snapshot());
        thread::sleep(Duration::from_millis(2));
    }
    job.join().unwrap();
    let s = job.snapshot();
    assert!(s.terminal(), "{s:?}");
    assert_eq!(s.native_pid, None);
    s
}
fn started(job: &Job, marker: &Path) -> i32 {
    let end = Instant::now() + Duration::from_secs(8);
    loop {
        if let Ok(pid) = fs::read_to_string(marker) {
            if let Ok(pid) = pid.parse::<i32>() {
                assert!(pid > 0);
                return pid;
            }
        }
        assert!(!job.is_finished(), "{:?}", job.snapshot());
        assert!(Instant::now() < end, "{:?}", job.snapshot());
        thread::sleep(Duration::from_millis(2));
    }
}
fn reaped(pid: i32) {
    // SAFETY: signal zero only checks the recorded private test child.
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
}
fn released(resources: &Arc<Resources>, store: &Arc<Store>, source: &Path) {
    assert_eq!(resources.usage(), managed::Usage::default());
    assert_eq!(store.usage(), artifacts::Usage::default());
    let writer = resources
        .index([cache::cache_path(source).unwrap()], 1)
        .unwrap();
    drop(writer);
    assert!(!fs::read_dir(std::env::temp_dir()).unwrap().next().is_some());
}
fn compare(store: &Arc<Store>, id: u64, golden: &Path, output: &Path) {
    let mut download = store.open(id).unwrap();
    let mut expected = File::open(golden).unwrap();
    let mut target = File::create(output).unwrap();
    assert_eq!(download.size_bytes(), expected.metadata().unwrap().len());
    loop {
        // Deliberately split native records/header/footer across chunks.
        let bytes = download.read_chunk(17).unwrap();
        if bytes.is_empty() {
            break;
        }
        let mut want = vec![0; bytes.len()];
        expected.read_exact(&mut want).unwrap();
        assert_eq!(bytes, want, "{golden:?}");
        target.write_all(&bytes).unwrap();
    }
    let mut end = [0; 1];
    assert_eq!(expected.read(&mut end).unwrap(), 0);
}

#[test]
#[ignore = "run tools/validate_managed_clip.py with private fixtures"]
fn native_bytes_and_resource_lifecycle() {
    let root = PathBuf::from(std::env::var_os("FLOE_MANAGED_EXPORT_ROOT").unwrap());
    let binary = PathBuf::from(std::env::var_os("FLOE_RENDERD_BIN").unwrap());
    let resources = Resources::new(managed::Limits::default()).unwrap();
    let source_path = root.join("layout 설계.oas");
    for (tag, layers) in [
        ("all", Layers::All),
        ("selected", Layers::Only(vec![(7, 0)])),
        ("none", Layers::None),
    ] {
        for jobs in [1, 8] {
            let store = Store::new(artifacts::Limits::default()).unwrap();
            let (source, data) = open(&resources, &source_path);
            let revision = data.revision;
            let mut job = Job::start(
                &resources,
                &store,
                source,
                data,
                request(jobs, layers.clone()),
                options(&binary, jobs),
            )
            .unwrap();
            let s = wait(&mut job);
            assert_eq!(s.phase, Phase::Ready, "{s:?}");
            assert_eq!(s.dataset_revision, revision);
            assert!(!job.cancel(), "late cancellation changed committed result");
            let result = s.outcome.unwrap();
            assert_eq!(result.artifact_id, s.id);
            assert_eq!(result.bbox_dbu, [250, 1100, 14000, 4700]);
            assert!(!result.source_stale);
            assert_eq!(result.records == 0, tag == "none");
            assert_eq!(resources.usage(), managed::Usage::default());
            compare(
                &store,
                s.id,
                &root.join(format!("golden-{tag}.oas")),
                &root.join(format!("managed-{tag}-j{jobs}.oas")),
            );
            assert_eq!(store.usage().bytes, result.bytes);
            drop(job); // A completed Job does not own/erase its published result.
            assert!(store.open(s.id).is_ok());
            assert!(store.release(s.id));
            released(&resources, &store, &source_path);
        }
    }
    // Cached geometry is explicitly labelled stale, never implicitly rebuilt.
    let stale = root.join("stale.oas");
    let store = Store::new(artifacts::Limits::default()).unwrap();
    let (source, data) = open(&resources, &stale);
    let mut job = Job::start(
        &resources,
        &store,
        source,
        data,
        request(1, Layers::All),
        options(&binary, 1),
    )
    .unwrap();
    let s = wait(&mut job);
    assert_eq!(s.phase, Phase::Ready, "{s:?}");
    assert!(s.outcome.unwrap().source_stale);
    compare(
        &store,
        s.id,
        &root.join("golden-all.oas"),
        &root.join("managed-stale.oas"),
    );
    store.release(s.id);
    released(&resources, &store, &stale);

    // Rejection happens before native work; read leases/resource counters survive.
    let (source, data) = open(&resources, &source_path);
    for request in [
        request(1, Layers::Only(vec![(123456, 0)])),
        request(17, Layers::All),
        ClipRequest {
            bbox: [0, 0, 0, 1],
            ..request(1, Layers::All)
        },
    ] {
        assert!(Job::start(
            &resources,
            &store,
            Arc::clone(&source),
            Arc::clone(&data),
            request,
            options(&binary, 1)
        )
        .is_err());
    }
    let (_, other) = open(&resources, &root.join("alternate.oas"));
    assert!(
        matches!(Job::start(&resources, &store, Arc::clone(&source), other, request(1, Layers::All), options(&binary, 1)), Err(e) if e.kind == ErrorKind::InvalidInput)
    );
    let (deck, deck_data) = open(&resources, &root.join("test.jb"));
    assert!(
        matches!(Job::start(&resources, &store, deck, deck_data, request(1, Layers::All), options(&binary, 1)), Err(e) if e.kind == ErrorKind::Unsupported)
    );
    let occupied = resources.export(16, 64).unwrap();
    assert!(
        matches!(Job::start(&resources, &store, Arc::clone(&source), Arc::clone(&data), request(1, Layers::All), options(&binary, 1)), Err(e) if e.kind == ErrorKind::Busy)
    );
    drop(occupied);
    drop(data);
    drop(source);
    released(&resources, &store, &source_path);

    // Retained-file cap is checked after generation; an oversize file is never ready.
    let tiny = Store::new(artifacts::Limits {
        artifact_bytes: 1,
        total_bytes: 1,
        ..artifacts::Limits::default()
    })
    .unwrap();
    let (source, data) = open(&resources, &source_path);
    let mut job = Job::start(
        &resources,
        &tiny,
        source,
        data,
        request(1, Layers::All),
        options(&binary, 1),
    )
    .unwrap();
    let s = wait(&mut job);
    assert_eq!(
        (s.phase, s.failure),
        (Phase::Failed, Some(ErrorKind::Incomplete))
    );
    assert!(tiny.open(s.id).is_err());
    released(&resources, &tiny, &source_path);

    for phase in ["ready", "open", "clip", "quit"] {
        for action in ["cancel", "drop", "close"] {
            let name = format!("{phase}_{action}");
            let store = Store::new(artifacts::Limits::default()).unwrap();
            let (source, data) = open(&resources, &source_path);
            let mut job = Job::start(
                &resources,
                &store,
                source,
                data,
                request(1, Layers::All),
                options(&root.join("fake").join(&name), 1),
            )
            .unwrap();
            let pid = started(&job, &root.join("fake").join(format!("{name}.started")));
            assert_eq!(resources.usage().workers, 1);
            assert_eq!(store.usage().pending, 1);
            assert!(resources
                .index([cache::cache_path(&source_path).unwrap()], 1)
                .is_err());
            let begin = Instant::now();
            if action == "drop" {
                drop(job);
            } else {
                if action == "close" {
                    store.close();
                } else {
                    assert!(job.cancel());
                }
                let s = wait(&mut job);
                assert_eq!(
                    (s.phase, s.failure),
                    (Phase::Cancelled, Some(ErrorKind::Cancelled)),
                    "{name}: {s:?}"
                );
                assert!(s.outcome.is_none() && store.open(s.id).is_err());
            }
            assert!(begin.elapsed() < Duration::from_secs(5), "{name}");
            reaped(pid);
            released(&resources, &store, &source_path);
        }
    }
    for (name, file, kind) in [
        ("failure", "layout 설계.oas", ErrorKind::Worker),
        ("corrupt", "layout 설계.oas", ErrorKind::Worker),
        ("timeout", "layout 설계.oas", ErrorKind::Worker),
        ("source-change", "changed.oas", ErrorKind::Cache),
        ("cache-change", "cache-change.oas", ErrorKind::Cache),
        ("failure_cancel", "layout 설계.oas", ErrorKind::Worker),
    ] {
        let path = root.join(file);
        let store = Store::new(artifacts::Limits::default()).unwrap();
        let (source, data) = open(&resources, &path);
        let mut opt = options(&root.join("fake").join(name), 1);
        opt.clip_timeout_s = 1;
        let mut job = Job::start(
            &resources,
            &store,
            source,
            data,
            request(1, Layers::All),
            opt,
        )
        .unwrap();
        let mut pid = None;
        if name == "failure_cancel" {
            pid = Some(started(&job, &root.join("fake/failure_cancel.started")));
            assert!(job.cancel());
        }
        let s = wait(&mut job);
        assert_eq!(
            (s.phase, s.failure),
            (Phase::Failed, Some(kind)),
            "{name}: {s:?}"
        );
        assert!(s.outcome.is_none() && store.open(s.id).is_err());
        if let Some(pid) = pid {
            reaped(pid);
        }
        released(&resources, &store, &path);
    }
    println!("RUST MANAGED CLIP: ALL OK (native bytes, explicit none, admission/leases, cancel/reap, retained-file limits, faults)");
}
