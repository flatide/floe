//! Self-contained runtime lane: no Python, shell, Cargo, fixture generator or
//! checkout is needed when this compiled test and the three product binaries run.
//! A macOS pass is not Linux execution or whole-G4/browser acceptance.
use floe_app_core::{
    annotations::png,
    cache,
    dataset::Dataset,
    drc::{
        build,
        review::{
            managed::{ManagedStore, Phase, Publication, Registration},
            store::Kind,
        },
    },
    jobdeck::color::Mode,
    managed::{Limits, Resources, Usage},
    registered::AccessScope,
};
use floe_oasis::{
    doc::{RectRec, Rep},
    write::write_cell,
};
use floe_worker_client::{
    Config, Event, Layers, QueryHit, QueryOperation, QueryRequest, QueryStatus, RenderRequest,
    Source, WorkerClient,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs,
    os::unix::{fs::DirBuilderExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{atomic::AtomicUsize, Arc},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct Private(PathBuf);
impl Private {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("floe-runtime-{}-{stamp}", std::process::id()));
        // No existing tree is ever reused or recursively removed.
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        Self(path)
    }
}
impl Drop for Private {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Runtime {
    root: Private,
    bin: PathBuf,
    workers: PathBuf,
}
impl Runtime {
    fn new() -> Self {
        assert_eq!(
            std::env::var("PATH").unwrap_or_default(),
            "",
            "run with PATH empty"
        );
        let source = PathBuf::from(
            std::env::var_os("FLOE_RUNTIME_BIN_DIR")
                .expect("explicit directory of floe2-web, floe-index and floe-renderd required"),
        );
        assert!(source.is_absolute());
        let root = Private::new();
        let bin = root.0.join("relocated binaries with spaces");
        let workers = root.0.join("workers");
        fs::create_dir(&bin).unwrap();
        fs::create_dir(&workers).unwrap();
        for name in ["floe2-web", "floe-index", "floe-renderd"] {
            let from = source.join(name);
            assert!(
                fs::symlink_metadata(&from).unwrap().is_file(),
                "regular binary required"
            );
            fs::copy(from, bin.join(name)).unwrap();
        }
        Self { root, bin, workers }
    }
    fn command(&self, args: &[&OsStr]) -> Vec<u8> {
        let stdout = self.root.0.join("command.stdout");
        let stderr = self.root.0.join("command.stderr");
        let mut child = Command::new(self.bin.join("floe2-web"))
            .args(args)
            .current_dir(&self.root.0)
            .env_clear()
            .env("PATH", "")
            .env("TMPDIR", &self.workers)
            .env("FLOE_RUST_JOBS", "1")
            .env("FLOE_RUST_RASTER_JOBS", "1")
            .env("FLOE_RUST_BUDGET_MB", "64")
            .stdin(Stdio::null())
            .stdout(fs::File::create(&stdout).unwrap())
            .stderr(fs::File::create(&stderr).unwrap())
            .process_group(0)
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(60);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                // Only the child we just spawned; SIGTERM lets its native worker
                // shutdown path run before the last-resort group kill.
                unsafe {
                    libc::kill(child.id() as i32, libc::SIGTERM);
                }
                let grace = Instant::now() + Duration::from_secs(10);
                while child.try_wait().unwrap().is_none() && Instant::now() < grace {
                    thread::sleep(Duration::from_millis(10));
                }
                if child.try_wait().unwrap().is_none() {
                    unsafe {
                        libc::kill(-(child.id() as i32), libc::SIGKILL);
                    }
                    child.wait().unwrap();
                }
                panic!("runtime command deadline: {args:?}");
            }
            thread::sleep(Duration::from_millis(10));
        };
        assert!(fs::metadata(&stdout).unwrap().len() < 2 * 1024 * 1024);
        assert!(fs::metadata(&stderr).unwrap().len() < 2 * 1024 * 1024);
        assert!(
            status.success(),
            "{args:?}: {status}\n{}",
            String::from_utf8_lossy(&fs::read(stderr).unwrap())
        );
        fs::read(stdout).unwrap()
    }
    fn info(&self, source: &Path) -> Value {
        serde_json::from_slice(&self.command(&[
            "info".as_ref(),
            source.as_os_str(),
            "--json".as_ref(),
        ]))
        .unwrap()
    }
    fn index(&self, source: &Path, policy: &str) {
        self.command(&[
            "index".as_ref(),
            source.as_os_str(),
            "--jobs=1".as_ref(),
            policy.as_ref(),
        ]);
    }
    fn render(&self, source: &Path, tag: &str) {
        let output = self.root.0.join(format!("{tag}.png"));
        let report = self.root.0.join(format!("{tag}.json"));
        self.command(&[
            "render".as_ref(),
            source.as_os_str(),
            "--px=96x64".as_ref(),
            "--stretch".as_ref(),
            "--detail=exact".as_ref(),
            "--out".as_ref(),
            output.as_os_str(),
            "--report".as_ref(),
            report.as_os_str(),
        ]);
        let (w, h, _) = png::display_snapshot(&output, &AtomicUsize::new(0))
            .unwrap()
            .into_parts();
        assert_eq!((w, h), (96, 64));
        let report: Value = serde_json::from_slice(&fs::read(report).unwrap()).unwrap();
        assert_eq!(report["complete"], true);
    }
}
fn contents(path: &Path) -> BTreeMap<PathBuf, (Vec<u8>, SystemTime)> {
    fs::read_dir(path)
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            assert!(e.file_type().unwrap().is_file());
            (
                e.file_name().into(),
                (
                    fs::read(e.path()).unwrap(),
                    e.metadata().unwrap().modified().unwrap(),
                ),
            )
        })
        .collect()
}
fn published(mut job: Publication) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !job.is_finished() {
        if Instant::now() >= deadline {
            job.cancel();
            panic!("review publication deadline");
        }
        thread::sleep(Duration::from_millis(1));
    }
    job.close().unwrap();
    let status = job.status();
    assert_eq!(status.phase, Phase::Succeeded);
    assert!(!status.outcome_unknown);
    assert!(status.outcome.unwrap().directory_synced);
}
fn render_queries(runtime: &Runtime, source: &Path) {
    let flag = AtomicUsize::new(0);
    let data = Dataset::open(source, None, Mode::Level, &flag).unwrap();
    let mut config = Config::new(runtime.bin.join("floe-renderd"));
    config.temp_root = runtime.workers.clone();
    config.render_timeout = Duration::from_secs(15);
    let mut worker = WorkerClient::spawn(config).unwrap();
    let opened_source = match &data {
        Dataset::Layout(layout) => Source::Layout(layout.directory.clone()),
        Dataset::Deck(deck) => {
            let spec = worker.work_dir().join("synthetic-deck.spec");
            fs::write(&spec, &deck.spec.text).unwrap();
            Source::Deck(spec)
        }
    };
    worker.open(opened_source, 64, 1).unwrap();
    worker.set_styles(&data.styles(true).unwrap()).unwrap();
    worker
        .render(RenderRequest {
            view: data.bbox().map(|v| v as f64),
            width: 120,
            height: 60,
            exact: true,
            decode_jobs: 1,
            raster_jobs: 1,
            ..RenderRequest::default()
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    let frame = loop {
        assert!(Instant::now() < deadline, "render deadline");
        match worker.poll(Duration::from_millis(10)).unwrap() {
            Some(Event::Frame(frame)) if frame.complete() => break frame,
            Some(Event::Failed { code, message, .. }) => panic!("{code}: {message}"),
            _ => {}
        }
    };
    assert_eq!(&frame.bytes[..8], b"FLOERAW1");
    assert!(frame.bytes[16..]
        .chunks_exact(4)
        .any(|p| p[0] != 0 || p[1] != 0 || p[2] != 0));
    let scene = frame.query_scene().unwrap();
    let operations: &[QueryOperation] = if data.is_deck() {
        assert!(
            scene.id.is_none(),
            "jobdeck must not advertise exact queries"
        );
        &[]
    } else {
        &[QueryOperation::Pick { nth: 0 }, QueryOperation::Snap]
    };
    for &operation in operations {
        let (x, y, radius) = if operation == QueryOperation::Snap {
            (10_001, 10_001, 10)
        } else {
            (20_000, 15_000, 1)
        };
        let sequence = worker
            .query(QueryRequest {
                scene: scene.id.unwrap(),
                operation,
                x,
                y,
                radius,
                layers: Layers::All,
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            assert!(Instant::now() < deadline, "query deadline");
            if let Some(Event::Query(reply)) = worker.poll(Duration::from_millis(10)).unwrap() {
                assert_eq!(reply.sequence, sequence);
                assert_eq!(reply.status, QueryStatus::Ok);
                match reply.hit.unwrap() {
                    QueryHit::Pick(hit) => {
                        assert_eq!(hit.layer, (1, 0));
                        assert_eq!(hit.bbox, [10_000, 10_000, 30_000, 20_000]);
                    }
                    QueryHit::Snap(hit) => assert_eq!((hit.x, hit.y), (10_000, 10_000)),
                }
                break;
            }
        }
    }
    worker.close().unwrap();
    drop(worker);
    assert!(fs::read_dir(&runtime.workers).unwrap().next().is_none());
}
fn reviews(runtime: &Runtime) {
    let source = runtime.root.0.join("synthetic.db");
    let bytes =
        b"TOP 1000\nWIDTH\n1 1 0\np 1 4\n10000 10000\n30000 10000\n30000 20000\n10000 20000\n";
    fs::write(&source, bytes).unwrap();
    runtime.command(&[
        "drc".as_ref(),
        source.as_os_str(),
        "--build".as_ref(),
        "--jobs=1".as_ref(),
    ]);
    let rules: Value = serde_json::from_slice(&runtime.command(&[
        "drc".as_ref(),
        source.as_os_str(),
        "--rules".as_ref(),
    ]))
    .unwrap();
    assert_eq!(rules[0]["name"], "WIDTH");
    assert_eq!(rules[0]["errors"], 1);
    let pack = build::output_path(&source).unwrap();
    let pack_before = fs::read(&pack).unwrap();
    let resources = Resources::new(Limits::default()).unwrap();
    let stop = Arc::new(AtomicUsize::new(0));
    let scope = AccessScope::new(std::slice::from_ref(&runtime.root.0)).unwrap();
    for kind in [Kind::Notes, Kind::Waives] {
        let store = ManagedStore::open(
            &resources,
            Registration {
                scope: Arc::clone(&scope),
                pack: pack.clone(),
                reviewer: "runtime-smoke".into(),
                kind,
                protected_files: vec![source.clone()],
                protected_trees: vec![],
            },
            &stop,
        )
        .unwrap();
        let snapshot = store.snapshot(Arc::clone(&stop)).unwrap();
        let draft = match kind {
            Kind::Notes => snapshot.prepare_note(&[0], "합성 runtime note").unwrap(),
            Kind::Waives => snapshot.prepare_waives(&[(0, 1)]).unwrap(),
        };
        published(draft.publish(false).unwrap());
        let target = store.target().to_owned();
        drop(store);
        // Reopen from disk; do not accept an in-memory draft as persistence.
        let store = ManagedStore::open(
            &resources,
            Registration {
                scope: Arc::clone(&scope),
                pack: pack.clone(),
                reviewer: "runtime-smoke".into(),
                kind,
                protected_files: vec![source.clone()],
                protected_trees: vec![],
            },
            &stop,
        )
        .unwrap();
        let snapshot = store.snapshot(Arc::clone(&stop)).unwrap();
        match kind {
            Kind::Notes => assert_eq!(snapshot.notes().unwrap().get(0), Some("합성 runtime note")),
            Kind::Waives => assert_eq!(snapshot.selected_statuses(&[0]).unwrap(), vec![1]),
        }
        let mut exported = Vec::new();
        let info = snapshot.export(&mut exported).unwrap();
        assert_eq!(info.bytes, exported.len() as u64);
        assert_eq!(exported, fs::read(target).unwrap());
        drop(snapshot);
        drop(store);
    }
    assert_eq!(resources.usage(), Usage::default());
    assert_eq!(fs::read(pack).unwrap(), pack_before);
    assert_eq!(fs::read(source).unwrap(), bytes);
}

#[test]
#[ignore = "requires three real native binaries; see docs/WEBUI_RUNTIME_ACCEPTANCE.ko.md"]
fn native_runtime_without_python() {
    let runtime = Runtime::new();
    let data = runtime.root.0.join("합성 input with spaces");
    fs::create_dir(&data).unwrap();
    let source = data.join("mask.oas");
    let bytes = write_cell(
        "TOP",
        1000.,
        &mut [
            RectRec {
                layer: 1,
                dt: 0,
                x: 10_000,
                y: 10_000,
                w: 20_000,
                h: 10_000,
                rep: Rep::One,
            },
            RectRec {
                layer: 2,
                dt: 0,
                x: 40_000,
                y: 10_000,
                w: 1000,
                h: 1000,
                rep: Rep::Grid {
                    na: 4,
                    nb: 4,
                    va: (2000, 0),
                    vb: (0, 2000),
                },
            },
        ],
        &mut [],
    )
    .unwrap();
    fs::write(&source, &bytes).unwrap();
    runtime.index(&source, "--occupancy");
    assert_eq!(
        runtime.info(&source)["metadata"]["bbox"],
        json!([10_000, 10_000, 47_000, 20_000])
    );
    let cache = cache::cache_path(&source).unwrap();
    assert!(cache.join("design.ovo").is_file());
    let before = contents(&cache);
    runtime.render(&source, "layout");
    runtime.command(&["probe".as_ref(), source.as_os_str()]);
    render_queries(&runtime, &source);

    let clip = runtime.root.0.join("clip.oas");
    runtime.command(&[
        "clip".as_ref(),
        source.as_os_str(),
        "--bbox=15,12,25,18".as_ref(),
        "--out".as_ref(),
        clip.as_os_str(),
    ]);
    runtime.index(&clip, "--no-occupancy");
    assert_eq!(
        runtime.info(&clip)["metadata"]["bbox"],
        json!([15_000, 12_000, 25_000, 18_000])
    );
    runtime.render(&clip, "clip");

    let deck = data.join("synthetic.jb");
    fs::write(
        &deck,
        concat!(
            "SLICE 1,17\nRETICLE\nOPTION PA, AA=0.02\nMTITLE 1,MASK\n",
            "*PLACE-INFO\nCHIP C1, * MAIN 1\n",
            "$ (1, MASK, AD=0.001, SF=1, TC=mask.oas, LY={1}, DT={0}, ",
            "BX=0, BY=0, UX=100, UY=100)\nROWS 100/200\n*END-PLACE\nEND\n",
        ),
    )
    .unwrap();
    runtime.index(&deck, "--no-occupancy");
    runtime.render(&deck, "deck");
    render_queries(&runtime, &deck);
    reviews(&runtime);
    assert_eq!(contents(&cache), before);
    assert_eq!(fs::read(&source).unwrap(), bytes);
    assert!(fs::read_dir(&runtime.workers).unwrap().next().is_none());
    println!(
        concat!(
            "NATIVE RUNTIME SMOKE: ALL OK (os={}, arch={}, ",
            "relocated CLI/index/occupancy/layout/deck/clip, raw render/pick/snap, ",
            "DRC build/read/note/waive/export/reopen, source/cache unchanged)",
        ),
        std::env::consts::OS,
        std::env::consts::ARCH,
    );
}
