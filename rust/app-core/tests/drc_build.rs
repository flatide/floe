//! Actual native build + service admission/drop gate, invoked by validate_drc_build.py.
use floe_app_core::{
    drc::{
        build::{self, Build, Options, Phase},
        Pack,
    },
    managed::{Limits, Resources, Usage},
    native::{Discovery, Indexer},
    registered::AccessScope,
    ErrorKind,
};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::AtomicUsize,
    time::{Duration, Instant},
};
fn indexer(path: PathBuf) -> Indexer {
    Indexer::discover(&Discovery {
        override_path: Some(path),
        development_root: None,
        executable: std::env::current_exe().unwrap(),
        search_path: None,
    })
    .unwrap()
}
fn wait(job: &Build) -> build::Snapshot {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let s = job.snapshot();
        if s.terminal() {
            return s;
        }
        assert!(Instant::now() < deadline, "{s:?}");
        if job.is_finished() {
            let terminal = job.snapshot();
            assert!(
                terminal.terminal(),
                "build thread finished without terminal state"
            );
            return terminal;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}
#[test]
#[ignore = "synthetic native/fault-injection fixtures are provided by validate_drc_build.py"]
fn admitted_build_reuse_and_drop_release_children_and_leases() {
    let source = PathBuf::from(std::env::var_os("FLOE_DRC_BUILD_SOURCE").unwrap());
    let binary = indexer(PathBuf::from(std::env::var_os("FLOE_INDEX_BIN").unwrap()));
    let fake = indexer(PathBuf::from(
        std::env::var_os("FLOE_DRC_BUILD_FAKE").unwrap(),
    ));
    let scope = AccessScope::new(&[source.parent().unwrap().to_owned()]).unwrap();
    let output = build::output_path(&source).unwrap();
    let resources = Resources::new(Limits::default()).unwrap();
    let options = Options {
        jobs: 2,
        force: false,
    };
    for file in [source.clone(), output.clone()] {
        let reader = resources.drc([file]).unwrap();
        let result = Build::start(&resources, scope.clone(), &source, options, binary.clone());
        assert!(matches!(result, Err(e) if e.kind == ErrorKind::Busy));
        drop(reader);
    }
    let mut job =
        Build::start(&resources, scope.clone(), &source, options, binary.clone()).unwrap();
    assert_eq!(resources.usage().cpu_slots, 2);
    assert!(resources.drc([output.clone()]).is_err());
    let first = wait(&job);
    assert_eq!(first.phase, Phase::Succeeded);
    assert!(!first.outcome.as_ref().unwrap().reused);
    assert!(!first.cleanup_warning);
    job.cancel();
    job.close().unwrap();
    assert_eq!(job.snapshot().phase, Phase::Succeeded);
    assert_eq!(resources.usage(), Usage::default());
    let bytes = fs::read(&output).unwrap();
    let p = Pack::open(&output, &AtomicUsize::new(0)).unwrap();
    assert!(p.total > 0 && p.source_matches(&source).unwrap());
    drop(p);
    let mut job = Build::start(&resources, scope.clone(), &source, options, binary).unwrap();
    assert!(wait(&job).outcome.unwrap().reused);
    job.close().unwrap();
    assert_eq!(bytes, fs::read(&output).unwrap());
    let job = Build::start(
        &resources,
        scope,
        &source,
        Options {
            force: true,
            ..options
        },
        fake,
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let pid = loop {
        let s = job.snapshot();
        assert!(!s.terminal(), "{s:?}");
        if let Some(pid) = s.native_pid {
            break pid;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    };
    assert!(resources.read([output.clone()]).is_err());
    drop(job);
    assert_eq!(resources.usage(), Usage::default());
    // SAFETY: signal 0 only checks whether this recorded child remains alive.
    assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
    assert_eq!(bytes, fs::read(&output).unwrap());
    assert!(!fs::read_dir(source.parent().unwrap()).unwrap().any(|e| e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".floe-drc-build-")));
}
